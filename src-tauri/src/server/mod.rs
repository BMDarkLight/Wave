// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! A Wave server hosted from the terminal.
//!
//! `wave serve start` launches a detached copy of this binary with `--wave-server`.
//! The server shares the library with paired Waves through the existing `/wave/`
//! API, and with Subsonic clients through `/rest/`. In `instance` mode it also
//! starts the playback daemon, so the machine acts as an always-on Wave node.

pub mod host;

pub use crate::share::{ServeScope, ServiceKind};

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::ValueEnum;
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};

use crate::app_paths::{
    library_db_path, server_config_path, server_lock_path, server_start_lock_path,
    server_state_path,
};
use crate::cli::ui::EXIT_GENERAL;
use crate::peers::api::PeerService;
use crate::peers::{PeerRecord, ShareScope};

/// Hidden argv switch used for the detached server process.
pub const SERVER_FLAG: &str = "--wave-server";

/// The control socket is local and speaks one short JSON line. Bound both the
/// line length and the number of simultaneous clients so a local process can't
/// exhaust the server with half-open requests.
const MAX_CONTROL_LINE: u64 = 8 * 1024;
const MAX_CONTROL_CLIENTS: usize = 32;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(3);
/// Graceful shutdown joins the sharing threads, which can be waiting on a
/// slow client. Give them enough time to finish before reporting failure.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

fn default_beacon() -> bool {
    true
}

/// What a hosted server shares with a peer by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ShareSetting {
    /// Share the whole library, including songs added later.
    Everything,
    /// Share no songs with this peer.
    Nothing,
}

impl ShareSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Everything => "everything",
            Self::Nothing => "nothing",
        }
    }

    fn scope(self) -> ShareScope {
        match self {
            Self::Everything => ShareScope::Everything,
            Self::Nothing => ShareScope::Nothing,
        }
    }
}

fn default_share() -> ShareSetting {
    ShareSetting::Everything
}

/// A readable name for a stored share scope.
fn share_name(scope: &ShareScope) -> &'static str {
    match scope {
        ShareScope::Everything => "everything",
        ShareScope::Nothing => "nothing",
        ShareScope::Tracks(ids) if ids.is_empty() => "nothing",
        ShareScope::Tracks(_) => "tracks",
    }
}

/// What a hosted server is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ServerMode {
    /// Library server only: no local playback engine is started.
    Server,
    /// A headless Wave node: library server plus the playback daemon.
    Instance,
}

impl ServerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Instance => "instance",
        }
    }
}

/// Settings accepted by both `serve start` and `serve config`.
#[derive(Debug, Clone, Default)]
pub struct ConfigOptions {
    pub port: Option<u16>,
    pub name: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub service: Option<ServiceKind>,
    pub scope: Option<ServeScope>,
    pub beacon: Option<bool>,
    pub share: Option<ShareSetting>,
    pub auto_accept: Option<bool>,
}

/// Options accepted by `wave serve start`.
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    pub mode: Option<ServerMode>,
    pub port: Option<u16>,
    pub name: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub service: Option<ServiceKind>,
    pub scope: Option<ServeScope>,
    pub beacon: Option<bool>,
    pub share: Option<ShareSetting>,
    pub auto_accept: Option<bool>,
}

impl StartOptions {
    fn settings(&self) -> ConfigOptions {
        ConfigOptions {
            port: self.port,
            name: self.name.clone(),
            user: self.user.clone(),
            password: self.password.clone(),
            service: self.service,
            scope: self.scope,
            beacon: self.beacon,
            share: self.share,
            auto_accept: self.auto_accept,
        }
    }
}

/// Persistent server settings. Deliberately has no `Debug`: it holds a password.
#[derive(Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub mode: ServerMode,
    pub port: u16,
    pub name: String,
    pub user: String,
    pub password: String,
    pub device_id: String,
    #[serde(default = "crate::share::default_service")]
    pub service: ServiceKind,
    #[serde(default = "crate::share::default_scope")]
    pub scope: ServeScope,
    #[serde(default = "default_beacon")]
    pub beacon: bool,
    /// Scope offered to a device whose pairing we accept automatically.
    #[serde(default = "default_share")]
    pub share: ShareSetting,
    /// Explicit override of the default pairing policy. `None` means auto for
    /// an unrestricted share, and hold requests when the share is `nothing`.
    #[serde(default)]
    pub auto_accept: Option<bool>,
    #[serde(default)]
    pub peers: Vec<PeerRecord>,
}

impl ServerConfig {
    /// An unrestricted server pairs arriving devices on its own. Restricting
    /// the default share to nothing turns that off until the user opts back in.
    pub fn auto_accepts(&self) -> bool {
        self.auto_accept
            .unwrap_or(self.share == ShareSetting::Everything)
    }
}

/// How a CLI client finds and authenticates with a running server.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerState {
    pid: u32,
    port: u16,
    control_port: u16,
    token: String,
    mode: ServerMode,
    name: String,
    user: String,
    #[serde(default = "crate::share::default_service")]
    service: ServiceKind,
    #[serde(default = "crate::share::default_scope")]
    scope: ServeScope,
    #[serde(default = "default_beacon")]
    beacon: bool,
    #[serde(default = "default_share")]
    share: ShareSetting,
    started_at: i64,
    /// True when this server spawned the playback daemon itself. A daemon that
    /// was already running belongs to the desktop app or another node and must
    /// be left alone when this server stops.
    #[serde(default)]
    owns_daemon: bool,
}

/// What `wave serve status` reports.
#[derive(Debug, Clone, Serialize)]
pub struct ServerStatus {
    pub running: bool,
    pub mode: Option<String>,
    pub name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub service: Option<String>,
    pub scope: Option<String>,
    pub beacon: Option<bool>,
    pub addresses: Vec<String>,
    /// Only set on the first start, when a password was generated for them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ControlRequest {
    token: String,
    request: Control,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Control {
    Ping,
    Shutdown,
    Peers,
    Accept {
        id: String,
        share: Option<ShareSetting>,
    },
    Decline {
        id: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct ControlResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

/// A Wave seen by discovery as reported by `serve peers`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoundPeer {
    pub name: String,
    pub address: String,
    pub last_seen_seconds: u64,
}

/// A pending pairing request as reported by `serve peers`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPeer {
    pub id: String,
    pub name: String,
    pub address: String,
    pub direction: String,
}

/// A paired device as reported by `serve peers`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedPeer {
    pub id: String,
    pub name: String,
    pub address: String,
    pub share: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeerList {
    pub found: Vec<FoundPeer>,
    pub pending: Vec<PendingPeer>,
    pub paired: Vec<PairedPeer>,
}

fn ok_response(data: Option<serde_json::Value>) -> ControlResponse {
    ControlResponse {
        ok: true,
        error: None,
        data,
    }
}

fn error_response(error: impl Into<String>) -> ControlResponse {
    ControlResponse {
        ok: false,
        error: Some(error.into()),
        data: None,
    }
}

fn open_server_lock() -> Result<std::fs::File, String> {
    let path = server_lock_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| format!("Failed to open the server lock: {e}"))
}

/// Take the server-only lock, held for the lifetime of the server process.
/// It is separate from the GUI/daemon lock so all three can coexist.
fn acquire_server_lock() -> Result<std::fs::File, String> {
    let file = open_server_lock()?;
    file.try_lock_exclusive()
        .map_err(|_| "A Wave server is already running on this machine.".to_string())?;
    Ok(file)
}

/// True while another process holds the server lock.
fn server_lock_held() -> bool {
    open_server_lock()
        .map(|file| file.try_lock_exclusive().is_err())
        .unwrap_or(false)
}

/// Open the short-lived lock used by `start` and `stop` to keep their view of
/// config and state stable while a server is coming up or going down.
fn open_start_lock() -> Result<std::fs::File, String> {
    let path = server_start_lock_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| format!("Failed to open the server start lock: {e}"))
}

/// Wait briefly for another `start`/`stop` to finish. The wait is bounded
/// because the first start already gives the server ten seconds to become
/// ready; a longer wait would only hide a hung process.
fn acquire_start_lock() -> Result<std::fs::File, String> {
    let file = open_start_lock()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if file.try_lock_exclusive().is_ok() {
            return Ok(file);
        }
        if Instant::now() >= deadline {
            return Err("Another Wave server command is still in progress.".to_string());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Write a file that holds a password or token without a readable window.
///
/// The new contents go to a unique temporary file first, then replace the old
/// file in one rename. A crash can therefore leave either the old settings or
/// the new ones, never a half-written or missing config.
fn write_secret_file(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let write = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(contents.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        // `std::fs::rename` replaces the destination on Windows too, so the
        // swap is atomic there as well and needs no remove-then-rename gap.
        std::fs::rename(&temp, path)
    })();
    if write.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    write.map_err(|e| e.to_string())
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn read_config() -> Result<ServerConfig, String> {
    let path = server_config_path();
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "Couldn't read {}: {e}. Run `wave serve start` first.",
            path.display()
        )
    })?;
    serde_json::from_str(&text)
        .map_err(|e| format!("Invalid server settings in {}: {e}", path.display()))
}

fn save_config(config: &ServerConfig) -> Result<(), String> {
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    write_secret_file(&server_config_path(), &text)
}

fn default_config() -> ServerConfig {
    ServerConfig {
        mode: ServerMode::Server,
        port: 50800,
        name: crate::share::default_name(),
        user: "wave".to_string(),
        password: uuid::Uuid::new_v4().to_string(),
        device_id: uuid::Uuid::new_v4().to_string(),
        service: crate::share::default_service(),
        scope: crate::share::default_scope(),
        beacon: default_beacon(),
        share: default_share(),
        auto_accept: None,
        peers: Vec::new(),
    }
}

/// Apply the settings shared by `serve start` and `serve config`, including the
/// rules that keep the internet scope Subsonic-only.
fn apply_options(config: &mut ServerConfig, options: &ConfigOptions) -> Result<(), String> {
    if let Some(port) = options.port {
        if port == 0 {
            return Err("The port must be between 1 and 65535.".to_string());
        }
        config.port = port;
    }
    if let Some(name) = &options.name {
        let name = name.trim();
        if name.is_empty() {
            return Err("The server name cannot be empty.".to_string());
        }
        config.name = name.to_string();
    }
    if let Some(user) = &options.user {
        let user = user.trim();
        if user.is_empty() {
            return Err("The user name cannot be empty.".to_string());
        }
        config.user = user.to_string();
    }
    if let Some(password) = &options.password {
        if password.is_empty() {
            return Err("The password cannot be empty.".to_string());
        }
        config.password = password.clone();
    }
    if let Some(service) = options.service {
        config.service = service;
    }
    if let Some(scope) = options.scope {
        config.scope = scope;
    }
    if let Some(beacon) = options.beacon {
        config.beacon = beacon;
    }
    if let Some(share) = options.share {
        config.share = share;
    }
    if let Some(auto_accept) = options.auto_accept {
        config.auto_accept = Some(auto_accept);
    }

    if config.scope == ServeScope::Internet {
        if options.service.is_some() && config.service != ServiceKind::Subsonic {
            return Err(
                "The wave service is unsupported with --scope internet; use --service subsonic."
                    .to_string(),
            );
        }
        config.service = ServiceKind::Subsonic;
    }
    if config.scope != ServeScope::Lan || !config.service.allows_wave() {
        if options.beacon == Some(true) {
            return Err(
                "The beacon is unsupported without a LAN Wave service; use --beacon off."
                    .to_string(),
            );
        }
        config.beacon = false;
    }
    if config.device_id.is_empty() {
        config.device_id = uuid::Uuid::new_v4().to_string();
    }
    Ok(())
}

/// Build or update the saved config. Returns true when a password was generated.
fn prepare_config(options: &StartOptions) -> Result<(ServerConfig, bool), String> {
    let existed = server_config_path().exists();
    let mut config = if existed {
        read_config()?
    } else {
        default_config()
    };
    if let Some(mode) = options.mode {
        config.mode = mode;
    }
    apply_options(&mut config, &options.settings())?;
    save_config(&config)?;
    // Only a password we made up needs to be shown; one the user chose is
    // already known to them.
    Ok((config, !existed && options.password.is_none()))
}

/// Change saved settings without touching a server that is already running.
pub fn configure(options: &ConfigOptions) -> Result<(ServerConfig, bool), String> {
    let _start_lock = acquire_start_lock()?;
    let mut config = if server_config_path().exists() {
        read_config()?
    } else {
        default_config()
    };
    apply_options(&mut config, options)?;
    save_config(&config)?;
    // `status` reads the settings we just saved when nothing is running, and
    // the older runtime state when something is.
    let applies_on_restart = status()?.running;
    Ok((config, applies_on_restart))
}

fn read_state() -> Option<ServerState> {
    let text = std::fs::read_to_string(server_state_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn control_request(state: &ServerState, request: Control) -> Result<ControlResponse, String> {
    let addr = format!("127.0.0.1:{}", state.control_port);
    let mut stream = TcpStream::connect_timeout(
        &addr
            .parse()
            .map_err(|e| format!("Invalid control address: {e}"))?,
        Duration::from_millis(500),
    )
    .map_err(|_| "The server is starting or stopping; try again.".to_string())?;
    stream
        .set_read_timeout(Some(CONTROL_TIMEOUT))
        .and_then(|_| stream.set_write_timeout(Some(CONTROL_TIMEOUT)))
        .map_err(|e| e.to_string())?;
    let envelope = ControlRequest {
        token: state.token.clone(),
        request,
    };
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&envelope).map_err(|e| e.to_string())?
    )
    .map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new((&mut stream).take(MAX_CONTROL_LINE))
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(line.trim()).map_err(|e| e.to_string())
}

/// An authenticated link to a running server, for peer management commands.
fn peer_state() -> Result<ServerState, String> {
    let state = read_state().ok_or("The Wave server isn't running.")?;
    match control_request(&state, Control::Ping) {
        Ok(response) if response.ok => Ok(state),
        Ok(response) => Err(response
            .error
            .unwrap_or_else(|| "The stored control token was refused.".to_string())),
        Err(_) if server_lock_held() => {
            Err("The server is starting or stopping; try again.".to_string())
        }
        Err(_) => Err("The Wave server isn't running.".to_string()),
    }
}

/// Pairing requests and paired devices of the running server.
pub fn peers() -> Result<PeerList, String> {
    let state = peer_state()?;
    let response = control_request(&state, Control::Peers)?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "Couldn't list peers".to_string()));
    }
    response
        .data
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| "The server returned an invalid peer list.".to_string())
}

/// Accept a pending pairing request, sharing `share` with that device.
pub fn accept_peer(id: &str, share: Option<ShareSetting>) -> Result<PairedPeer, String> {
    let state = peer_state()?;
    let response = control_request(
        &state,
        Control::Accept {
            id: id.to_string(),
            share,
        },
    )?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "Couldn't accept the request".to_string()));
    }
    response
        .data
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| "The server returned an invalid peer.".to_string())
}

/// Decline a pending pairing request.
pub fn decline_peer(id: &str) -> Result<PendingPeer, String> {
    let state = peer_state()?;
    let response = control_request(&state, Control::Decline { id: id.to_string() })?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "Couldn't decline the request".to_string()));
    }
    response
        .data
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| "The server returned an invalid peer.".to_string())
}

/// Remove the state file only if it still describes this exact server. A slow
/// shutdown must never delete a newer server's state.
fn clear_state_if_current(state: &ServerState) {
    let Ok(text) = std::fs::read_to_string(server_state_path()) else {
        return;
    };
    let Ok(current) = serde_json::from_str::<ServerState>(&text) else {
        return;
    };
    if current.pid == state.pid
        && current.control_port == state.control_port
        && same_secret(&current.token, &state.token)
    {
        let _ = std::fs::remove_file(server_state_path());
    }
}

/// A daemon left behind by a crashed instance server has no owner any more.
fn shutdown_daemon_if_owned(state: &ServerState) {
    if state.owns_daemon {
        let _ = crate::playback_daemon::daemon_request_if_running(
            crate::playback_daemon::DaemonRequest::Shutdown,
        );
    }
}

/// Retire state that can't belong to a live server. The long-lived server lock
/// is the authority: if nobody holds it, no Wave server is running, whatever
/// an old pid file says.
fn retire_stale_state(state: &ServerState) {
    if server_lock_held() {
        return;
    }
    shutdown_daemon_if_owned(state);
    clear_state_if_current(state);
}

/// Tear down everything a running server owns. Called on every graceful exit,
/// including a Ctrl+C/SIGTERM handled by `ctrlc`.
fn cleanup_server(state: &ServerState) {
    crate::share::apply(None);
    shutdown_daemon_if_owned(state);
    clear_state_if_current(state);
}

/// Normalize a saved config for display, including files written by an older
/// build that didn't have these fields.
fn effective_settings(config: &ServerConfig) -> (ServiceKind, ServeScope, bool) {
    let scope = config.scope;
    let service = if scope == ServeScope::Internet {
        ServiceKind::Subsonic
    } else {
        config.service
    };
    let beacon = scope == ServeScope::Lan && service.allows_wave() && config.beacon;
    (service, scope, beacon)
}

/// Addresses the server can actually answer on, for status output. In internet
/// scope these are the local addresses to port-forward: Wave cannot know the
/// router's public address without asking an outside service.
fn addresses(port: u16, scope: ServeScope) -> Vec<String> {
    let mut result = vec![format!("http://127.0.0.1:{port}")];
    if scope == ServeScope::Loopback {
        return result;
    }
    result.extend(
        crate::remote::dlna::multicast_interfaces()
            .into_iter()
            .filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
            .filter(|ip| crate::share::http::is_local(std::net::IpAddr::V4(*ip)))
            .map(|ip| format!("http://{ip}:{port}")),
    );
    result
}

/// Outcome of `wave serve start`, so the CLI can tell a fresh launch from an
/// already-running server.
pub struct StartResult {
    pub status: ServerStatus,
    pub already_running: bool,
}

/// Start (or confirm) the hosted server.
pub fn start(options: &StartOptions) -> Result<StartResult, String> {
    // Serialize start/stop so a second start cannot rewrite the config while
    // the first server is still reading it. The detached server holds the
    // separate, long-lived `wave-server.lock` instead.
    let _start_lock = acquire_start_lock()?;

    // Validate the request before the already-running shortcut: an impossible
    // combination is a usage error whether or not a server happens to be up.
    // This is a dry run; only `prepare_config` writes the saved settings.
    let mut requested = if server_config_path().exists() {
        read_config()?
    } else {
        default_config()
    };
    apply_options(&mut requested, &options.settings())?;

    // Don't overwrite the saved settings when a server is already running:
    // it would keep listening on its old port while the file advertises the
    // new one.
    let current = status()?;
    if current.running {
        return Ok(StartResult {
            status: current,
            already_running: true,
        });
    }
    if server_lock_held() {
        return Err("A Wave server is already starting on this machine.".to_string());
    }
    let (config, generated) = prepare_config(options)?;

    let ready = (|| -> Result<ServerStatus, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut command = std::process::Command::new(exe);
        command.arg(SERVER_FLAG);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const DETACHED_PROCESS: u32 = 0x00000008;
            command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
        }
        #[cfg(unix)]
        {
            command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            std::os::unix::process::CommandExt::process_group(&mut command, 0);
        }
        command
            .spawn()
            .map_err(|e| format!("Failed to start the Wave server: {e}"))?;

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if Instant::now() >= deadline {
                return Err(
                    "The Wave server didn't start within 10 seconds. Check the log in the data folder."
                        .to_string(),
                );
            }
            let current = status()?;
            if current.running {
                return Ok(current);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    })();
    let status = match ready {
        Ok(status) => status,
        Err(e) => {
            // The password was just generated and saved, but a failure here
            // means the success envelope that would have shown it never runs.
            return Err(if generated {
                format!(
                    "{e} The generated Subsonic password is {}; save it now.",
                    config.password
                )
            } else {
                e
            });
        }
    };
    if generated {
        return Ok(StartResult {
            status: status.with_password(&config.password),
            already_running: false,
        });
    }
    Ok(StartResult {
        status,
        already_running: false,
    })
}

/// Stop the hosted server, and a playback daemon it started itself.
pub fn stop() -> Result<bool, String> {
    let _start_lock = acquire_start_lock()?;
    let Some(state) = read_state() else {
        return Ok(false);
    };

    // The token, not the pid, proves this is still our server: a stale pid can
    // have been reused by an unrelated process.
    match control_request(&state, Control::Ping) {
        Ok(response) if response.ok => {}
        Ok(_) => {
            retire_stale_state(&state);
            return Err("The stored control token was refused by the running server.".to_string());
        }
        Err(_) => {
            if server_lock_held() {
                return Err("The server is starting or stopping; try again.".to_string());
            }
            retire_stale_state(&state);
            return Ok(false);
        }
    }

    let response = control_request(&state, Control::Shutdown)?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "Shutdown failed".to_string()));
    }

    // The server writes its reply before it tears down the sharing threads, so
    // the process can still be alive here. Don't report success—or delete its
    // state—until it has really gone.
    let deadline = Instant::now() + STOP_TIMEOUT;
    while server_state_path().exists() && crate::single_instance::is_process_alive(state.pid) {
        if Instant::now() >= deadline {
            return Err("The Wave server didn't stop within 30 seconds.".to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Reaching this point because the state file vanished is normal. Give the
    // process a short grace period to return from `exit`.
    let deadline = Instant::now() + Duration::from_secs(2);
    while crate::single_instance::is_process_alive(state.pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    if crate::single_instance::is_process_alive(state.pid) {
        return Err("The Wave server process is still running after shutdown.".to_string());
    }
    shutdown_daemon_if_owned(&state);
    clear_state_if_current(&state);
    Ok(true)
}

/// Current state of the hosted server.
///
/// The saved settings are reported even when nothing is running, so `status`
/// always has the documented fields; `pid` is the one that has no value then.
pub fn status() -> Result<ServerStatus, String> {
    let mut state = read_state();
    if let Some(current) = &state {
        match control_request(current, Control::Ping) {
            Ok(response) if response.ok => {}
            _ => {
                retire_stale_state(current);
                // Never describe an unreachable server as running, even if its
                // pid file hasn't been cleaned up yet.
                state = None;
            }
        }
    }
    let config = read_config().unwrap_or_else(|_| default_config());
    let (mode, name, user, port, pid, service, scope, beacon) = match &state {
        Some(current) => (
            current.mode.as_str().to_string(),
            current.name.clone(),
            current.user.clone(),
            current.port,
            Some(current.pid),
            current.service,
            current.scope,
            current.beacon,
        ),
        None => {
            let (service, scope, beacon) = effective_settings(&config);
            (
                config.mode.as_str().to_string(),
                config.name.clone(),
                config.user.clone(),
                config.port,
                None,
                service,
                scope,
                beacon,
            )
        }
    };
    Ok(ServerStatus {
        running: state.is_some(),
        mode: Some(mode),
        name: Some(name),
        user: Some(user),
        port: Some(port),
        pid,
        service: Some(service.as_str().to_string()),
        scope: Some(scope.as_str().to_string()),
        beacon: Some(beacon),
        addresses: addresses(port, scope),
        password: None,
    })
}

impl ServerStatus {
    /// Reveal a freshly generated password once, on the first start.
    fn with_password(mut self, password: &str) -> Self {
        self.password = Some(password.to_string());
        self
    }
}

/// Entry point for the detached `--wave-server` process.
pub fn run_server() -> ! {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let _server_lock = acquire_server_lock().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(EXIT_GENERAL);
    });

    let config = match read_config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(EXIT_GENERAL);
        }
    };

    let library = match crate::library::Library::new_with_path(&library_db_path()) {
        Ok(library) => Arc::new(parking_lot::Mutex::new(library)),
        Err(e) => {
            eprintln!("Failed to open the library: {e}");
            std::process::exit(EXIT_GENERAL);
        }
    };

    let host = Arc::new(host::HeadlessHost::new(
        config.clone(),
        Arc::clone(&library),
    ));
    let peers = Arc::new(crate::peers::api::PeerService::new(host));
    peers.set_auto_accept(config.auto_accepts().then(|| config.share.scope()));
    let stop = Arc::new(AtomicBool::new(false));
    let signal_stop = Arc::clone(&stop);
    if let Err(e) = ctrlc::set_handler(move || signal_stop.store(true, Ordering::Relaxed)) {
        tracing::warn!("Could not install a shutdown handler: {e}");
    }

    // The control socket is loopback-only and token-authenticated; the LAN
    // listener below never accepts management requests.
    let control = TcpListener::bind(("127.0.0.1", 0)).unwrap_or_else(|e| {
        eprintln!("Failed to bind the server control socket: {e}");
        std::process::exit(EXIT_GENERAL);
    });
    let control_port = control.local_addr().map(|a| a.port()).unwrap_or(0);
    let token = uuid::Uuid::new_v4().to_string();

    let control_stop = Arc::clone(&stop);
    let control_token = token.clone();
    let control_peers = Arc::clone(&peers);
    let control_share = config.share;
    std::thread::Builder::new()
        .name("wave-server-control".into())
        .spawn(move || {
            control_loop(
                control,
                control_token,
                control_stop,
                control_peers,
                control_share,
            )
        })
        .expect("spawn server control loop");

    let (service, scope, beacon) = effective_settings(&config);
    let subsonic = service.allows_subsonic().then(|| {
        Arc::new(crate::share::subsonic::Auth::new(
            config.user.clone(),
            config.password.clone(),
        ))
    });
    let share_config = crate::share::ShareConfig {
        name: config.name.clone(),
        uuid: config.device_id.clone(),
        db_path: library_db_path(),
        opener: crate::share::file_opener(None),
        peers,
        port: Some(config.port),
        library: Some(Arc::clone(&library)),
        subsonic,
        service,
        scope,
        beacon,
    };
    crate::share::apply(Some(share_config));

    // Start the playback engine before publishing readiness, so `serve start`
    // never returns before instance mode is fully up. Only shut down a daemon
    // this server actually spawned; one owned by the desktop app or another
    // node must keep running when we stop.
    let mut owns_daemon = false;
    if config.mode == ServerMode::Instance {
        match crate::playback_daemon::ensure_daemon_running() {
            Ok((_, spawned)) => owns_daemon = spawned,
            Err(e) => tracing::warn!("The playback engine couldn't start: {e}"),
        }
    }

    // The requested port may already be taken, in which case sharing falls
    // back to a free one. Publish the port it actually got.
    let port = crate::share::port().unwrap_or(config.port);
    let state = ServerState {
        pid: std::process::id(),
        port,
        control_port,
        token: token.clone(),
        mode: config.mode,
        name: config.name.clone(),
        user: config.user.clone(),
        service,
        scope,
        beacon,
        share: config.share,
        started_at: now_secs(),
        owns_daemon,
    };
    if let Err(e) = write_secret_file(
        &server_state_path(),
        &serde_json::to_string_pretty(&state).unwrap_or_default(),
    ) {
        eprintln!("Failed to write server state: {e}");
        crate::share::apply(None);
        shutdown_daemon_if_owned(&state);
        std::process::exit(EXIT_GENERAL);
    }

    tracing::info!(
        "Wave {} \"{}\" listening on port {}",
        config.mode.as_str(),
        config.name,
        port
    );
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(200));
    }

    cleanup_server(&state);
    std::process::exit(0)
}

fn control_loop(
    listener: TcpListener,
    token: String,
    stop: Arc<AtomicBool>,
    peers: Arc<PeerService>,
    default_share: ShareSetting,
) {
    let _ = listener.set_nonblocking(true);
    let clients = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if !reserve_client(&clients) {
                    tracing::debug!("Too many control clients; dropping a connection");
                    continue;
                }
                let client_count = Arc::clone(&clients);
                let token = token.clone();
                let control_stop = Arc::clone(&stop);
                let control_peers = Arc::clone(&peers);
                let spawned = std::thread::Builder::new()
                    .name("wave-server-control-client".into())
                    .spawn(move || {
                        control_client(stream, token, control_stop, &control_peers, default_share);
                        client_count.fetch_sub(1, Ordering::Release);
                    });
                if let Err(e) = spawned {
                    clients.fetch_sub(1, Ordering::Release);
                    tracing::debug!("Could not answer a control request: {e}");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                tracing::debug!("Server control accept failed: {e}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn reserve_client(clients: &std::sync::atomic::AtomicUsize) -> bool {
    let mut active = clients.load(Ordering::Relaxed);
    loop {
        if active >= MAX_CONTROL_CLIENTS {
            return false;
        }
        match clients.compare_exchange_weak(active, active + 1, Ordering::AcqRel, Ordering::Relaxed)
        {
            Ok(_) => return true,
            Err(current) => active = current,
        }
    }
}

fn control_client(
    mut stream: TcpStream,
    token: String,
    stop: Arc<AtomicBool>,
    peers: &PeerService,
    default_share: ShareSetting,
) {
    // Never inherit a listener's O_NONBLOCK: some platforms pass it to accepted
    // sockets, which would make a normal status request fail spuriously.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    if stream.set_read_timeout(Some(CONTROL_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(CONTROL_TIMEOUT)).is_err()
    {
        return;
    }

    let (response, shutdown) = {
        let mut reader = BufReader::new((&mut stream).take(MAX_CONTROL_LINE));
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(_) => match serde_json::from_str::<ControlRequest>(line.trim()) {
                Ok(request) if same_secret(&request.token, &token) => {
                    handle_control(request.request, peers, default_share)
                }
                _ => (error_response("unauthorized"), false),
            },
            Err(e) => (error_response(e.to_string()), false),
        }
    };

    // Answer before setting the stop flag so `wave serve stop` sees a clean
    // reply even though the main loop starts tearing down immediately.
    let _ = writeln!(
        stream,
        "{}",
        serde_json::to_string(&response).unwrap_or_default()
    );
    let _ = stream.flush();
    if shutdown {
        stop.store(true, Ordering::Relaxed);
    }
}

/// Run an authenticated control request. Returns the response and whether the
/// server should stop after it.
fn handle_control(
    request: Control,
    peers: &PeerService,
    configured_share: ShareSetting,
) -> (ControlResponse, bool) {
    match request {
        Control::Ping => (ok_response(None), false),
        Control::Shutdown => (ok_response(None), true),
        Control::Peers => {
            let now = Instant::now();
            let (found, pending) = {
                let registry = peers.registry.lock();
                let mut found = registry
                    .found
                    .values()
                    .map(|p| FoundPeer {
                        name: p.name.clone(),
                        address: p.address.clone(),
                        last_seen_seconds: now.saturating_duration_since(p.last_seen).as_secs(),
                    })
                    .collect::<Vec<_>>();
                found.sort_by(|a, b| (&a.name, &a.address).cmp(&(&b.name, &b.address)));
                let mut pending = registry
                    .pending
                    .values()
                    .map(|p| PendingPeer {
                        id: p.id.clone(),
                        name: p.name.clone(),
                        address: p.address.clone(),
                        direction: if p.incoming { "incoming" } else { "outgoing" }.to_string(),
                    })
                    .collect::<Vec<_>>();
                pending.sort_by(|a, b| a.id.cmp(&b.id));
                (found, pending)
            };
            let mut paired = peers
                .host()
                .peers()
                .into_iter()
                .map(|p| PairedPeer {
                    share: share_name(&p.share).to_string(),
                    id: p.id,
                    name: p.name,
                    address: p.address,
                })
                .collect::<Vec<_>>();
            paired.sort_by(|a, b| a.id.cmp(&b.id));
            let list = PeerList {
                found,
                pending,
                paired,
            };
            match serde_json::to_value(list) {
                Ok(value) => (ok_response(Some(value)), false),
                Err(e) => (error_response(e.to_string()), false),
            }
        }
        Control::Accept { id, share } => {
            let scope = share.unwrap_or(configured_share).scope();
            match peers.accept(&id, scope) {
                Ok(record) => {
                    let host = Arc::clone(peers.host());
                    let answer_record = record.clone();
                    if let Err(e) = std::thread::Builder::new()
                        .name("wave-peer-cli-answer".into())
                        .spawn(move || {
                            let _ = crate::peers::api::answer_to(host, &answer_record, true);
                        })
                    {
                        tracing::warn!("Couldn't send the pairing answer: {e}");
                    }
                    let dto = PairedPeer {
                        share: share_name(&record.share).to_string(),
                        id: record.id,
                        name: record.name,
                        address: record.address,
                    };
                    match serde_json::to_value(dto) {
                        Ok(value) => (ok_response(Some(value)), false),
                        Err(e) => (error_response(e.to_string()), false),
                    }
                }
                Err(e) => (error_response(e), false),
            }
        }
        Control::Decline { id } => match peers.decline(&id) {
            Some(pending) => {
                let host = Arc::clone(peers.host());
                let answer_record = crate::peers::PeerRecord {
                    id: pending.id.clone(),
                    name: pending.name.clone(),
                    address: pending.address.clone(),
                    secret: pending.secret.clone(),
                    share: ShareScope::Nothing,
                    source_id: None,
                    paired_at: 0,
                };
                if let Err(e) = std::thread::Builder::new()
                    .name("wave-peer-cli-decline".into())
                    .spawn(move || {
                        let _ = crate::peers::api::answer_to(host, &answer_record, false);
                    })
                {
                    tracing::warn!("Couldn't send the decline: {e}");
                }
                let dto = PendingPeer {
                    direction: "incoming".to_string(),
                    id: pending.id,
                    name: pending.name,
                    address: pending.address,
                };
                match serde_json::to_value(dto) {
                    Ok(value) => (ok_response(Some(value)), false),
                    Err(e) => (error_response(e.to_string()), false),
                }
            }
            None => (error_response("That request is no longer waiting"), false),
        },
    }
}

fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

#[cfg(test)]
mod tests {
    use super::{
        apply_options, default_config, ConfigOptions, Control, ControlRequest, PeerList,
        ServeScope, ServerMode, ServiceKind, ShareSetting,
    };

    #[test]
    fn control_requests_survive_the_wire() {
        for request in [
            Control::Ping,
            Control::Shutdown,
            Control::Peers,
            Control::Accept {
                id: "peer-1".into(),
                share: Some(ShareSetting::Nothing),
            },
            Control::Decline {
                id: "peer-2".into(),
            },
        ] {
            let line = serde_json::to_string(&ControlRequest {
                token: "secret".into(),
                request: request.clone(),
            })
            .unwrap();
            let back: ControlRequest = serde_json::from_str(&line).unwrap();
            assert_eq!(back.token, "secret");
            assert_eq!(back.request, request);
        }
    }

    #[test]
    fn peer_lists_survive_the_wire() {
        let list = PeerList {
            found: vec![super::FoundPeer {
                name: "Nearby".into(),
                address: "http://10.0.0.4:50800".into(),
                last_seen_seconds: 3,
            }],
            pending: vec![super::PendingPeer {
                id: "p".into(),
                name: "Phone".into(),
                address: "http://10.0.0.2:50800".into(),
                direction: "incoming".into(),
            }],
            paired: vec![super::PairedPeer {
                id: "x".into(),
                name: "Laptop".into(),
                address: "http://10.0.0.3:50800".into(),
                share: "everything".into(),
            }],
        };
        let line = serde_json::to_string(&list).unwrap();
        let back: PeerList = serde_json::from_str(&line).unwrap();
        assert_eq!(back.found[0].last_seen_seconds, 3);
        assert_eq!(back.pending[0].direction, "incoming");
        assert_eq!(back.paired[0].share, "everything");
    }

    #[test]
    fn default_server_config_is_sane() {
        let config = default_config();
        assert_eq!(config.mode, ServerMode::Server);
        assert_eq!(config.port, 50800);
        assert_eq!(config.service, ServiceKind::Both);
        assert_eq!(config.scope, ServeScope::Lan);
        assert!(config.beacon);
        assert_eq!(config.share, ShareSetting::Everything);
        assert!(config.auto_accepts());
        assert!(!config.device_id.is_empty());
        assert!(!config.password.is_empty());
    }

    #[test]
    fn share_and_auto_accept_can_be_saved() {
        let mut config = default_config();
        apply_options(
            &mut config,
            &ConfigOptions {
                share: Some(ShareSetting::Nothing),
                auto_accept: Some(false),
                ..ConfigOptions::default()
            },
        )
        .unwrap();
        assert_eq!(config.share, ShareSetting::Nothing);
        assert!(!config.auto_accepts());
    }

    #[test]
    fn restricting_the_share_turns_off_implicit_auto_accept() {
        let mut config = default_config();
        apply_options(
            &mut config,
            &ConfigOptions {
                share: Some(ShareSetting::Nothing),
                ..ConfigOptions::default()
            },
        )
        .unwrap();
        assert!(!config.auto_accepts());

        apply_options(
            &mut config,
            &ConfigOptions {
                auto_accept: Some(true),
                ..ConfigOptions::default()
            },
        )
        .unwrap();
        assert!(config.auto_accepts());

        apply_options(
            &mut config,
            &ConfigOptions {
                share: Some(ShareSetting::Everything),
                auto_accept: None,
                ..ConfigOptions::default()
            },
        )
        .unwrap();
        // An explicit override still wins until it is cleared.
        assert!(config.auto_accepts());
    }

    #[test]
    fn internet_scope_forces_subsonic_and_disables_the_beacon() {
        let mut config = default_config();
        apply_options(
            &mut config,
            &ConfigOptions {
                scope: Some(ServeScope::Internet),
                ..ConfigOptions::default()
            },
        )
        .unwrap();
        assert_eq!(config.service, ServiceKind::Subsonic);
        assert!(!config.beacon);

        let mut config = default_config();
        let error = apply_options(
            &mut config,
            &ConfigOptions {
                scope: Some(ServeScope::Internet),
                service: Some(ServiceKind::Both),
                ..ConfigOptions::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("unsupported"), "{error}");
    }

    #[test]
    fn beacon_on_requires_a_lan_wave_service() {
        let mut config = default_config();
        let error = apply_options(
            &mut config,
            &ConfigOptions {
                scope: Some(ServeScope::Loopback),
                beacon: Some(true),
                ..ConfigOptions::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("unsupported"), "{error}");

        let mut config = default_config();
        apply_options(
            &mut config,
            &ConfigOptions {
                service: Some(ServiceKind::Subsonic),
                ..ConfigOptions::default()
            },
        )
        .unwrap();
        assert!(!config.beacon);
    }
}
