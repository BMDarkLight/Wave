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
use crate::server::{self, ConfigOptions, ServeScope, ServerMode, ServiceKind, StartOptions};

#[derive(Subcommand)]
pub enum ServeCmd {
    /// Start the server in the background (or confirm it is already up)
    Start {
        /// `server` serves the library; `instance` also starts local playback
        #[arg(long, value_enum)]
        mode: Option<ServerMode>,
        /// Which API is exposed: wave, subsonic, or both
        #[arg(long, value_enum)]
        service: Option<ServiceKind>,
        /// Who may connect: loopback, lan, or internet
        #[arg(long, value_enum)]
        scope: Option<ServeScope>,
        /// Advertise this server on the LAN (`internet` is Subsonic-only)
        #[arg(long, value_enum)]
        beacon: Option<Switch>,
        /// Port for Wave peers and Subsonic clients (default: 50800)
        #[arg(long)]
        port: Option<u16>,
        /// Name other devices see
        #[arg(long)]
        name: Option<String>,
        /// Subsonic user name (default: wave)
        #[arg(long)]
        user: Option<String>,
        /// Subsonic password (a random one is generated and shown if omitted)
        #[arg(long)]
        password: Option<String>,
    },
    /// Change saved server settings (they take effect on the next start)
    Config {
        /// Which API is exposed: wave, subsonic, or both
        #[arg(long, value_enum)]
        service: Option<ServiceKind>,
        /// Who may connect: loopback, lan, or internet
        #[arg(long, value_enum)]
        scope: Option<ServeScope>,
        /// Advertise this server on the LAN (`internet` is Subsonic-only)
        #[arg(long, value_enum)]
        beacon: Option<Switch>,
        /// Port for Wave peers and Subsonic clients
        #[arg(long)]
        port: Option<u16>,
        /// Name other devices see
        #[arg(long)]
        name: Option<String>,
        /// Subsonic user name
        #[arg(long)]
        user: Option<String>,
        /// Subsonic password
        #[arg(long)]
        password: Option<String>,
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
        } => {
            let options = ConfigOptions {
                service,
                scope,
                beacon: switch(beacon),
                port,
                name,
                user,
                password,
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
                "applies_on_restart": applies_on_restart,
            });
            let message = if applies_on_restart {
                "Saved Wave server settings; they take effect on the next start."
            } else {
                "Saved Wave server settings."
            };
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
