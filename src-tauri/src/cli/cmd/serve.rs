// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! `wave serve` — host a Wave server on this machine.

use clap::Subcommand;

use crate::cli::{json, ui, Switch};
use crate::server::{
    self, ConfigOptions, ServeScope, ServerMode, ServiceKind, ShareSetting, StartOptions,
};

const MODE_HELP: &str = "`server` (the default) shares the library only. `instance` also starts the playback daemon when one isn't already running; `wave serve stop` shuts down a daemon this server started, but leaves a daemon owned by the desktop app or another node alone.";
const SERVICE_HELP: &str = "Which API this server answers. `wave` serves only the `/wave/` peer API used by the Wave GUI for discovery and pairing; `subsonic` serves only the `/rest/` API for Subsonic players; `both` (the default) serves both on one port. Saved in wave-server.json.";
const SCOPE_HELP: &str = "Who may connect. `loopback` binds only 127.0.0.1 and accepts only local clients. `lan` (the default) binds all interfaces but accepts only loopback, private (RFC 1918) and link-local addresses; this is the scope the Wave GUI needs. `internet` accepts any client that can reach the port and automatically forces `--service subsonic` with the beacon off. Saved in wave-server.json.";
const BEACON_HELP: &str = "`on` runs the Radii LAN discovery beacon so other Wave apps can find and pair with this server. `off` disables it. It defaults to `on` only for `--service wave|both` at `--scope lan`; it is forced off for loopback, internet and Subsonic-only servers. An explicit `on` in those cases is a usage error. Saved in wave-server.json.";
const PORT_HELP: &str = "TCP port for the enabled APIs. Defaults to 50800; if that port is taken, Wave picks a free one and reports it. Saved in wave-server.json.";
const NAME_HELP: &str = "The name other devices see for this server. Saved in wave-server.json.";
const USER_HELP: &str = "Subsonic login name. Defaults to `wave`. Saved in wave-server.json.";
const PASSWORD_HELP: &str = "Subsonic password. On the first start, if omitted, Wave generates a random one, saves it owner-only in wave-server.json, and prints it once. Save it from the start output; it is never shown again by `status`.";
const SHARE_HELP: &str = "What this server shares with a peer by default: `everything` (the default) or `nothing`. Changing this to `nothing` also stops automatic pairing unless --auto-accept on is given explicitly. Saved in wave-server.json.";
const AUTO_ACCEPT_HELP: &str = "Answer incoming Wave pairing requests straight away with the --share scope. The default follows --share: auto with `everything`, wait for approval with `nothing`. An explicit `on` or `off` overrides that default; `off` leaves requests for `wave serve accept <id>`. Saved in wave-server.json.";

const CONFIG_SERVICE_HELP: &str = "Which API this server answers. `wave` serves only the `/wave/` peer API used by the Wave GUI; `subsonic` only `/rest/`; `both` serves both on one port. Omit to keep the saved value (initially `both`).";
const CONFIG_SCOPE_HELP: &str = "Who may connect. `loopback` binds only 127.0.0.1; `lan` (initially the default) also accepts private and link-local clients and is what the Wave GUI needs; `internet` accepts any client and forces Subsonic-only with the beacon off. Omit to keep the saved value.";
const CONFIG_BEACON_HELP: &str = "LAN discovery beacon. `on` is allowed only for `--service wave|both` at `--scope lan`; otherwise the beacon is forced off. Omit to keep the saved value (initially `on` for LAN Wave servers).";
const CONFIG_PORT_HELP: &str =
    "TCP port for the enabled APIs (initially 50800). Omit to keep the saved value.";
const CONFIG_NAME_HELP: &str =
    "The name other devices see for this server. Omit to keep the saved value.";
const CONFIG_USER_HELP: &str =
    "Subsonic login name (initially `wave`). Omit to keep the saved value.";
const CONFIG_PASSWORD_HELP: &str = "Subsonic password. Supplying one replaces the saved password; omit to keep it. Passwords are stored owner-only and are never printed.";
const CONFIG_SHARE_HELP: &str = "Default scope for paired peers: `everything` or `nothing`. Omit to keep the saved value (initially `everything`). Changing it affects peers paired after the next start; existing peers keep their stored scope. Restricting it to `nothing` also stops automatic pairing unless --auto-accept on is set.";
const CONFIG_AUTO_ACCEPT_HELP: &str = "Answer incoming pairing requests without anyone here: `on` or `off`. Omit to keep the saved policy, which is auto with share `everything` and manual with share `nothing`. Use `wave serve peers` and `wave serve accept` to approve held requests.";

const ACCEPT_SHARE_HELP: &str = "What to share with this device: `everything` or `nothing`. Defaults to the server's configured --share scope; stored for this peer only.";

const START_LONG_ABOUT: &str = "Start a headless Wave server on this machine and print its status. If a server is already running, this reports it and changes nothing.\n\nFlags you omit reuse the settings saved in wave-server.json; the first start creates that file. Use `wave serve config` to change saved settings without starting a server.";
const START_AFTER_HELP: &str = "Defaults: --service both, --scope lan, --beacon on, --port 50800, --user wave, --share everything, --auto-accept on.\n\nValidation: --scope internet forces --service subsonic, never serves /wave/, and turns the beacon off; an explicit conflicting --service or --beacon on is rejected. The beacon runs only for --service wave|both at --scope lan. --scope loopback binds 127.0.0.1 only.\n\nThe Wave GUI can only pair with the /wave/ API at LAN scope. Use --service wave or both with --scope lan; a Subsonic-only or internet-scoped server cannot be paired from the GUI. Incoming requests are paired automatically while the default share is `everything`. Restricting the share to `nothing` holds them for `wave serve accept`, as does --auto-accept off.\n\nAll APIs are plain HTTP, so credentials and music are unencrypted. Internet scope accepts any client that can reach the port: firewall it deliberately and put a TLS-terminating reverse proxy in front of it.";
const CONFIG_LONG_ABOUT: &str = "Change saved server settings without touching a server that is already running. The new values take effect the next time `wave serve start` starts a server.";
const CONFIG_AFTER_HELP: &str = "This command accepts the same flags as `serve start` except --mode. Omitted flags keep their current saved values. It prints `applies_on_restart: true` when a server is running.\n\nThe same validation rules apply: --scope internet forces --service subsonic with the beacon off, and --beacon on is rejected unless the service is wave|both and the scope is lan. A running server keeps its active settings until it is stopped and started again.";
const PEERS_LONG_ABOUT: &str = "List Waves found by discovery, the pairing requests waiting on the running server, and the devices already paired with it.\n\nFound entries are nearby Waves seen by the LAN beacon and are not trusted or paired. Pending requests show a direction: `incoming` is a device that asked this server, `outgoing` is a request this server sent. Paired entries show the share scope this server gives that device.";
const PEERS_AFTER_HELP: &str = "With --json, prints {\"found\": [...], \"pending\": [...], \"paired\": [...]} directly. Found entries include `name`, `address` and `last_seen_seconds`.\n\nAn unrestricted server answers incoming requests automatically, so the pending list is usually empty. Use `wave serve config --share nothing` or `--auto-accept off` first if you want to approve each device here.";
const ACCEPT_LONG_ABOUT: &str = "Accept a waiting pairing request and share the chosen scope with that device. Sends the answer the other Wave is waiting for, which completes the pairing handshake.";
const ACCEPT_AFTER_HELP: &str = "Run `wave serve peers` to find the id. Omit --share to use the server's configured default scope. Re-running for an id that is no longer waiting is an error.";
const DECLINE_LONG_ABOUT: &str =
    "Turn down a waiting pairing request and tell the other Wave so it stops waiting.";
const DECLINE_AFTER_HELP: &str =
    "Run `wave serve peers` to find the id. Declining does not remove an existing pairing.";

#[derive(Subcommand)]
pub enum ServeCmd {
    /// Start the server in the background (or confirm it is already up)
    #[command(long_about = START_LONG_ABOUT, after_help = START_AFTER_HELP)]
    Start {
        /// `server` serves the library; `instance` also starts local playback
        #[arg(long, value_enum, long_help = MODE_HELP)]
        mode: Option<ServerMode>,
        /// Which API is exposed: wave, subsonic, or both
        #[arg(long, value_enum, long_help = SERVICE_HELP)]
        service: Option<ServiceKind>,
        /// Who may connect: loopback, lan, or internet
        #[arg(long, value_enum, long_help = SCOPE_HELP)]
        scope: Option<ServeScope>,
        /// Advertise this server on the LAN
        #[arg(long, value_enum, long_help = BEACON_HELP)]
        beacon: Option<Switch>,
        /// Port for Wave peers and Subsonic clients (default: 50800)
        #[arg(long, long_help = PORT_HELP)]
        port: Option<u16>,
        /// Name other devices see
        #[arg(long, long_help = NAME_HELP)]
        name: Option<String>,
        /// Subsonic user name (default: wave)
        #[arg(long, long_help = USER_HELP)]
        user: Option<String>,
        /// Subsonic password (a random one is generated and shown if omitted)
        #[arg(long, long_help = PASSWORD_HELP)]
        password: Option<String>,
        /// Default share scope for peers (default: everything)
        #[arg(long, value_enum, long_help = SHARE_HELP)]
        share: Option<ShareSetting>,
        /// Answer pairing requests automatically (default: on)
        #[arg(long, value_enum, long_help = AUTO_ACCEPT_HELP)]
        auto_accept: Option<Switch>,
    },
    /// Change saved server settings (they take effect on the next start)
    #[command(long_about = CONFIG_LONG_ABOUT, after_help = CONFIG_AFTER_HELP)]
    Config {
        /// Which API is exposed: wave, subsonic, or both
        #[arg(long, value_enum, long_help = CONFIG_SERVICE_HELP)]
        service: Option<ServiceKind>,
        /// Who may connect: loopback, lan, or internet
        #[arg(long, value_enum, long_help = CONFIG_SCOPE_HELP)]
        scope: Option<ServeScope>,
        /// Advertise this server on the LAN
        #[arg(long, value_enum, long_help = CONFIG_BEACON_HELP)]
        beacon: Option<Switch>,
        /// Port for Wave peers and Subsonic clients
        #[arg(long, long_help = CONFIG_PORT_HELP)]
        port: Option<u16>,
        /// Name other devices see
        #[arg(long, long_help = CONFIG_NAME_HELP)]
        name: Option<String>,
        /// Subsonic user name
        #[arg(long, long_help = CONFIG_USER_HELP)]
        user: Option<String>,
        /// Subsonic password
        #[arg(long, long_help = CONFIG_PASSWORD_HELP)]
        password: Option<String>,
        /// Default share scope for peers
        #[arg(long, value_enum, long_help = CONFIG_SHARE_HELP)]
        share: Option<ShareSetting>,
        /// Answer pairing requests automatically
        #[arg(long, value_enum, long_help = CONFIG_AUTO_ACCEPT_HELP)]
        auto_accept: Option<Switch>,
    },
    /// List pending pairing requests and paired devices
    #[command(long_about = PEERS_LONG_ABOUT, after_help = PEERS_AFTER_HELP)]
    Peers,
    /// Accept a pending pairing request
    #[command(long_about = ACCEPT_LONG_ABOUT, after_help = ACCEPT_AFTER_HELP)]
    Accept {
        /// Device id from `serve peers`
        id: String,
        /// What to share with this device
        #[arg(long, value_enum, long_help = ACCEPT_SHARE_HELP)]
        share: Option<ShareSetting>,
    },
    /// Decline a pending pairing request
    #[command(long_about = DECLINE_LONG_ABOUT, after_help = DECLINE_AFTER_HELP)]
    Decline {
        /// Device id from `serve peers`
        id: String,
    },
    /// Stop the hosted server
    Stop,
    /// Show whether the hosted server is running and where
    Status,
}

fn switch(value: Option<Switch>) -> Option<bool> {
    value.map(|value| matches!(value, Switch::On))
}

fn shape(status: &server::ServerStatus) -> String {
    format!(
        "{}, {}, beacon {}",
        status.service.as_deref().unwrap_or("both"),
        status.scope.as_deref().unwrap_or("lan"),
        if status.beacon.unwrap_or(false) {
            "on"
        } else {
            "off"
        }
    )
}

pub fn run(command: ServeCmd) {
    match command {
        ServeCmd::Start {
            mode,
            service,
            scope,
            beacon,
            port,
            name,
            user,
            password,
            share,
            auto_accept,
        } => {
            let options = StartOptions {
                mode,
                service,
                scope,
                beacon: switch(beacon),
                port,
                name,
                user,
                password,
                share,
                auto_accept: switch(auto_accept),
            };
            let result = server::start(&options).unwrap_or_else(|e| ui::fail_with(e));
            let status = &result.status;
            let detail = serde_json::to_value(status).unwrap_or_default();
            let message = if let Some(password) = &status.password {
                format!(
                    "Wave {} \"{}\" running on port {} ({}). Subsonic login: {}/{} (save this password now)",
                    status.mode.as_deref().unwrap_or("server"),
                    status.name.as_deref().unwrap_or("Wave"),
                    status.port.unwrap_or_default(),
                    shape(status),
                    status.user.as_deref().unwrap_or("wave"),
                    password
                )
            } else if result.already_running {
                format!(
                    "Wave server already running on port {} ({}) ({})",
                    status.port.unwrap_or_default(),
                    status.name.as_deref().unwrap_or("Wave"),
                    shape(status)
                )
            } else {
                format!(
                    "Wave {} \"{}\" running on port {} ({})",
                    status.mode.as_deref().unwrap_or("server"),
                    status.name.as_deref().unwrap_or("Wave"),
                    status.port.unwrap_or_default(),
                    shape(status)
                )
            };
            ui::done(message, detail);
        }
        ServeCmd::Config {
            service,
            scope,
            beacon,
            port,
            name,
            user,
            password,
            share,
            auto_accept,
        } => {
            let options = ConfigOptions {
                service,
                scope,
                beacon: switch(beacon),
                port,
                name,
                user,
                password,
                share,
                auto_accept: switch(auto_accept),
            };
            let (config, applies_on_restart) =
                server::configure(&options).unwrap_or_else(|e| ui::fail_with(e));
            let detail = serde_json::json!({
                "port": config.port,
                "name": config.name,
                "user": config.user,
                "service": config.service.as_str(),
                "scope": config.scope.as_str(),
                "beacon": config.beacon,
                "share": config.share.as_str(),
                "auto_accept": config.auto_accepts(),
                "applies_on_restart": applies_on_restart,
            });
            let message = if applies_on_restart {
                "Saved Wave server settings; they take effect on the next start."
            } else {
                "Saved Wave server settings."
            };
            ui::done(message, detail);
        }
        ServeCmd::Peers => {
            let list = server::peers().unwrap_or_else(|e| ui::fail_with(e));
            json::maybe_emit(&list);
            let mut parts = Vec::new();
            if !list.found.is_empty() {
                let found = list
                    .found
                    .iter()
                    .map(|p| format!("{} ({})", p.name, p.address))
                    .collect::<Vec<_>>()
                    .join(", ");
                parts.push(format!("Found: {found}"));
            }
            if !list.pending.is_empty() {
                let pending = list
                    .pending
                    .iter()
                    .map(|p| format!("{} ({}, {})", p.name, p.id, p.direction))
                    .collect::<Vec<_>>()
                    .join(", ");
                parts.push(format!("Pending: {pending}"));
            }
            if !list.paired.is_empty() {
                let paired = list
                    .paired
                    .iter()
                    .map(|p| format!("{} ({}, shares {})", p.name, p.id, p.share))
                    .collect::<Vec<_>>()
                    .join(", ");
                parts.push(format!("Paired: {paired}"));
            }
            let message = if parts.is_empty() {
                "No pending requests or paired devices.".to_string()
            } else {
                parts.join("; ")
            };
            ui::done(message, serde_json::json!({}));
        }
        ServeCmd::Accept { id, share } => {
            let peer = server::accept_peer(&id, share).unwrap_or_else(|e| ui::fail_with(e));
            let detail = serde_json::to_value(&peer).unwrap_or_default();
            let message = format!(
                "Accepted {} ({}); sharing {}.",
                peer.name, peer.id, peer.share
            );
            ui::done(message, detail);
        }
        ServeCmd::Decline { id } => {
            let peer = server::decline_peer(&id).unwrap_or_else(|e| ui::fail_with(e));
            let detail = serde_json::to_value(&peer).unwrap_or_default();
            let message = format!("Declined {} ({}).", peer.name, peer.id);
            ui::done(message, detail);
        }
        ServeCmd::Stop => {
            let stopped = server::stop().unwrap_or_else(|e| ui::fail_with(e));
            let message = if stopped {
                "Wave server stopped."
            } else {
                "Wave server wasn't running."
            };
            ui::done(message, serde_json::json!({ "stopped": stopped }));
        }
        ServeCmd::Status => {
            let status = server::status().unwrap_or_else(|e| ui::fail_with(e));
            // Status is a read: under --json it prints the status itself, not
            // a change envelope around it.
            if ui::current().json {
                json::emit(&status);
            }
            let message = if status.running {
                format!(
                    "Wave {} \"{}\" is running on port {} (pid {}, {})",
                    status.mode.as_deref().unwrap_or("server"),
                    status.name.as_deref().unwrap_or("Wave"),
                    status.port.unwrap_or_default(),
                    status.pid.unwrap_or_default(),
                    shape(&status)
                )
            } else {
                "Wave server is not running.".to_string()
            };
            ui::done(message, serde_json::json!({}));
        }
    }
}
