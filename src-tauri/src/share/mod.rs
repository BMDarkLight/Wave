// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Sharing the library with other Waves on the local network.
//!
//! Runs while Outside Sourcing is on. Other Waves find this one, ask to
//! pair, and then read and play what they were given through the `/wave/`
//! API. Nothing is reachable from outside the local network: requests from
//! other addresses are dropped.
//!
//! [`catalog`] reads the shared songs from the library, [`http`] serves
//! them, and [`crate::peers::discovery`] makes this Wave findable.

pub mod catalog;
pub mod http;
pub mod subsonic;

use crate::library::Library;
use catalog::Snapshot;
use clap::ValueEnum;
use http::{Catalog, Opener, Server};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The port asked for first, so a client that remembered the address finds
/// the server again after a restart. Any free port is used when it's taken.
pub(crate) const PREFERRED_PORT: u16 = 50800;

/// Listener loops get a moment to release their sockets on shutdown. A worker
/// still serving a slow client is detached after that: it cannot outlive the
/// server process, and making `stop` wait forever would make it unreliable.
const STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Which API a hosted server answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    /// Paired Waves only: the `/wave/` peer API.
    Wave,
    /// Subsonic players only: the `/rest/` API.
    Subsonic,
    /// Both APIs on the same port.
    Both,
}

impl ServiceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wave => "wave",
            Self::Subsonic => "subsonic",
            Self::Both => "both",
        }
    }

    pub fn allows_wave(self) -> bool {
        matches!(self, Self::Wave | Self::Both)
    }

    pub fn allows_subsonic(self) -> bool {
        matches!(self, Self::Subsonic | Self::Both)
    }
}

pub fn default_service() -> ServiceKind {
    ServiceKind::Both
}

/// Who may connect to the HTTP listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ServeScope {
    /// This machine only: bind and accept on `127.0.0.1`.
    Loopback,
    /// This machine and its local network; the default.
    Lan,
    /// Any address. Only the Subsonic API is allowed in this scope.
    Internet,
}

impl ServeScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::Lan => "lan",
            Self::Internet => "internet",
        }
    }

    pub fn accepts(self, ip: IpAddr) -> bool {
        match self {
            Self::Loopback => ip.is_loopback(),
            Self::Lan => http::is_local(ip),
            Self::Internet => true,
        }
    }
}

pub fn default_scope() -> ServeScope {
    ServeScope::Lan
}

/// What the server is started with.
#[derive(Clone)]
pub struct ShareConfig {
    pub name: String,
    pub uuid: String,
    pub db_path: PathBuf,
    pub opener: Opener,
    pub peers: Arc<crate::peers::api::PeerService>,
    /// Port for a CLI-hosted server. The desktop app leaves this unset and
    /// keeps using the preferred sharing port.
    pub port: Option<u16>,
    /// Library connection used by the Subsonic playlists API.
    pub library: Option<Arc<parking_lot::Mutex<Library>>>,
    /// Credentials for Subsonic clients; absent on a desktop Wave.
    pub subsonic: Option<Arc<subsonic::Auth>>,
    /// Which API this listener answers.
    pub service: ServiceKind,
    /// Who may connect to the listener.
    pub scope: ServeScope,
    /// Whether the LAN discovery beacon should run.
    pub beacon: bool,
}

/// The settings screen's view of the server.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ShareStatus {
    pub running: bool,
    pub name: String,
    /// `http://<address>:<port>` for each network the server is on.
    pub addresses: Vec<String>,
    pub track_count: usize,
    pub error: Option<String>,
}

/// The library, read through a connection of the server's own, and the last
/// snapshot built from it.
struct LibraryCatalog {
    library: Arc<Mutex<Library>>,
    /// The last snapshot and when its version was last confirmed.
    cached: Mutex<Option<(Arc<Snapshot>, Instant)>>,
}

/// How long a snapshot is trusted without asking the library again. Seeking
/// through a song sends a request per seek, and each version check counts
/// the whole library.
const FRESH_FOR: Duration = Duration::from_secs(2);

impl LibraryCatalog {
    fn rebuild(&self, version: u32) -> Result<Arc<Snapshot>, String> {
        let tracks = self.library.lock().shareable_tracks()?;
        Ok(Arc::new(Snapshot::build(version, tracks)))
    }
}

impl Catalog for LibraryCatalog {
    /// The snapshot, rebuilt only when the library has changed since.
    fn snapshot(&self) -> Result<Arc<Snapshot>, String> {
        if let Some((snapshot, checked)) = self.cached.lock().as_ref() {
            if checked.elapsed() < FRESH_FOR {
                return Ok(snapshot.clone());
            }
        }
        let version = self.library.lock().share_version()?;
        if let Some((snapshot, checked)) = self.cached.lock().as_mut() {
            if snapshot.version == version {
                *checked = Instant::now();
                return Ok(snapshot.clone());
            }
        }
        let snapshot = self.rebuild(version)?;
        *self.cached.lock() = Some((snapshot.clone(), Instant::now()));
        Ok(snapshot)
    }
}

struct Running {
    config_key: (
        String,
        String,
        PathBuf,
        Option<u16>,
        Option<String>,
        ServiceKind,
        ServeScope,
        bool,
    ),
    port: u16,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    catalog: Arc<LibraryCatalog>,
    name: String,
    error: Arc<Mutex<Option<String>>>,
    beacon: bool,
}

fn state() -> &'static Mutex<Option<Running>> {
    static STATE: OnceLock<Mutex<Option<Running>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn last_error() -> &'static Mutex<Option<String>> {
    static ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    ERROR.get_or_init(|| Mutex::new(None))
}

fn bind_http(scope: ServeScope, port: Option<u16>) -> std::io::Result<TcpListener> {
    let preferred = port.unwrap_or(PREFERRED_PORT);
    let addr = if scope == ServeScope::Loopback {
        Ipv4Addr::LOCALHOST
    } else {
        Ipv4Addr::UNSPECIFIED
    };
    TcpListener::bind((addr, preferred)).or_else(|_| TcpListener::bind((addr, 0)))
}

fn start(config: ShareConfig) -> Result<Running, String> {
    // A hosted server passes the connection it already opened for its peer
    // host; the desktop app lets sharing open its own read-only connection.
    let library = match config.library.clone() {
        Some(library) => library,
        None => Arc::new(Mutex::new(Library::new_with_path(&config.db_path)?)),
    };
    let catalog = Arc::new(LibraryCatalog {
        library: Arc::clone(&library),
        cached: Mutex::new(None),
    });
    let listener = bind_http(config.scope, config.port)
        .map_err(|e| format!("Couldn't open a port to share on: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let error = Arc::new(Mutex::new(None));
    let server = Arc::new(Server {
        catalog: catalog.clone(),
        opener: config.opener.clone(),
        peers: config.peers.clone(),
        library: Some(library),
        subsonic: config.subsonic.clone(),
        service: config.service,
    });

    let mut threads = Vec::new();
    let http_stop = stop.clone();
    let http_scope = config.scope;
    threads.push(
        std::thread::Builder::new()
            .name("wave-share-http".into())
            .spawn(move || http::serve(listener, server, http_stop, http_scope))
            .map_err(|e| e.to_string())?,
    );
    // The beacon announces the Wave peer API. Skip it when that API is off or
    // the listener is not reachable from the LAN it advertises on.
    let beacon = config.beacon && config.service.allows_wave() && config.scope == ServeScope::Lan;
    if beacon {
        let peers = config.peers.clone();
        let discovery_stop = stop.clone();
        let discovery_error = error.clone();
        threads.push(
            std::thread::Builder::new()
                .name("wave-peer-discovery".into())
                .spawn(move || {
                    crate::peers::discovery::run(peers, port, discovery_stop, discovery_error)
                })
                .map_err(|e| e.to_string())?,
        );
        #[cfg(target_os = "android")]
        crate::android::multicast::acquire();
    }
    tracing::info!(
        "Sharing the library as \"{}\" on port {port} ({}, {})",
        config.name,
        config.service.as_str(),
        config.scope.as_str()
    );
    Ok(Running {
        config_key: (
            config.name.clone(),
            config.uuid,
            config.db_path,
            config.port,
            config.subsonic.as_ref().map(|a| a.user().to_string()),
            config.service,
            config.scope,
            config.beacon,
        ),
        port,
        stop,
        threads,
        catalog,
        name: config.name,
        error,
        beacon,
    })
}

fn stop(running: Running) {
    running.stop.store(true, Ordering::Relaxed);
    let deadline = Instant::now() + STOP_JOIN_TIMEOUT;
    for thread in running.threads {
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if thread.is_finished() {
            let _ = thread.join();
        } else {
            // Dropping the handle detaches the loop. This is a last resort for
            // a stuck listener; the hosted server exits immediately after this
            // call, which tears down the remaining threads with the process.
            tracing::warn!("A sharing thread didn't stop within three seconds");
        }
    }
    #[cfg(target_os = "android")]
    if running.beacon {
        crate::android::multicast::release();
    }
    #[cfg(not(target_os = "android"))]
    let _ = running.beacon;
    tracing::info!("Stopped sharing the library");
}

/// Starts, restarts or stops the server to match `config`; `None` stops it.
pub fn apply(config: Option<ShareConfig>) {
    let mut slot = state().lock();
    let wanted = config.as_ref().map(|c| {
        (
            c.name.clone(),
            c.uuid.clone(),
            c.db_path.clone(),
            c.port,
            c.subsonic.as_ref().map(|a| a.user().to_string()),
            c.service,
            c.scope,
            c.beacon,
        )
    });
    if slot.as_ref().map(|r| r.config_key.clone()) == wanted {
        return;
    }
    if let Some(running) = slot.take() {
        stop(running);
    }
    *last_error().lock() = None;
    if let Some(config) = config {
        match start(config) {
            Ok(running) => *slot = Some(running),
            Err(e) => {
                tracing::warn!("Couldn't share the library: {e}");
                *last_error().lock() = Some(e);
            }
        }
    }
}

#[cfg(target_os = "android")]
pub fn is_running() -> bool {
    state().lock().is_some()
}

/// The shared song stored at `path`, and its fingerprint, while the server
/// runs. Reads through the server's own connection, so it never waits on
/// the library the rest of the app holds.
pub fn shared_track_at(path: &str) -> Option<(crate::metadata::Track, Option<String>)> {
    let catalog = state().lock().as_ref()?.catalog.clone();
    let track = catalog.snapshot().ok()?.track_at(path)?.clone();
    let fingerprint = catalog.library.lock().audio_fingerprint_of(&track.id);
    Some((track, fingerprint))
}

/// The port the server answers on, while it runs.
pub fn port() -> Option<u16> {
    state().lock().as_ref().map(|r| r.port)
}

/// True while the library is shared and set to stay shared with Wave closed,
/// so closing or quitting from the window must leave the process running.
pub fn in_background(app: &tauri::AppHandle) -> bool {
    use tauri::Manager;
    state().lock().is_some()
        && app
            .try_state::<crate::app_settings::AppSettingsState>()
            .and_then(|state| state.0.lock().ok().map(|s| s.share_in_background))
            .unwrap_or(false)
}

pub fn status() -> ShareStatus {
    let slot = state().lock();
    let Some(running) = slot.as_ref() else {
        return ShareStatus {
            error: last_error().lock().clone(),
            ..ShareStatus::default()
        };
    };
    // Only addresses the server answers on: a VPN's, say, is refused.
    let addresses = crate::remote::dlna::multicast_interfaces()
        .into_iter()
        .filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
        .filter(|ip| http::is_local(std::net::IpAddr::V4(*ip)))
        .map(|ip| format!("http://{ip}:{}", running.port))
        .collect();
    let error = running.error.lock().clone();
    let track_count = running
        .catalog
        .snapshot()
        .map(|s| s.track_count())
        .unwrap_or(0);
    ShareStatus {
        running: true,
        name: running.name.clone(),
        addresses,
        track_count,
        error,
    }
}

/// The name other devices see when the user hasn't picked one.
pub fn default_name() -> String {
    format!("Wave on {}", device_name())
}

fn device_name() -> String {
    #[cfg(target_os = "android")]
    {
        "Android".to_string()
    }
    #[cfg(all(unix, not(target_os = "android")))]
    {
        nix::unistd::gethostname()
            .ok()
            .and_then(|h| h.into_string().ok())
            .map(|h| h.trim_end_matches(".local").to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "this computer".to_string())
    }
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this computer".to_string())
    }
}

/// Opens a shared file. Plain paths everywhere; on Android, `content://`
/// documents go through the ContentResolver like every other SAF read.
pub fn file_opener(app: Option<tauri::AppHandle>) -> Opener {
    Arc::new(move |path: &str| {
        if path.starts_with("content://") {
            // Through the Application context first, which works with no
            // window open, as sharing in the background needs.
            #[cfg(target_os = "android")]
            if let Ok(file) = crate::android::share_service::open_document(path) {
                return Ok(file);
            }
            let Some(app) = app.as_ref() else {
                return Err(std::io::Error::other("no app to open a document with"));
            };
            return crate::android::saf_io::open_for_reading(app, path)
                .map_err(std::io::Error::other);
        }
        std::fs::File::open(path)
    })
}
