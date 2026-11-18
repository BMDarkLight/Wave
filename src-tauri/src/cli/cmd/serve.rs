// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! `wave serve` — host a Wave server on this machine.

use clap::Subcommand;

use crate::cli::{json, ui};
use crate::server::{self, ServerMode, StartOptions};

#[derive(Subcommand)]
pub enum ServeCmd {
    /// Start the server in the background (or confirm it is already up)
    Start {
        /// `server` serves the library; `instance` also starts local playback
        #[arg(long, value_enum)]
        mode: Option<ServerMode>,
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
    /// Stop the hosted server
    Stop,
    /// Show whether the hosted server is running and where
    Status,
}

pub fn run(command: ServeCmd) {
    match command {
        ServeCmd::Start {
            mode,
            port,
            name,
            user,
            password,
        } => {
            let options = StartOptions {
                mode,
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
                    "Wave {} \"{}\" running on port {}. Subsonic login: {}/{} (save this password now)",
                    status.mode.as_deref().unwrap_or("server"),
                    status.name.as_deref().unwrap_or("Wave"),
                    status.port.unwrap_or_default(),
                    status.user.as_deref().unwrap_or("wave"),
                    password
                )
            } else if result.already_running {
                format!(
                    "Wave server already running on port {} ({})",
                    status.port.unwrap_or_default(),
                    status.name.as_deref().unwrap_or("Wave")
                )
            } else {
                format!(
                    "Wave {} \"{}\" running on port {} ({})",
                    status.mode.as_deref().unwrap_or("server"),
                    status.name.as_deref().unwrap_or("Wave"),
                    status.port.unwrap_or_default(),
                    status
                        .addresses
                        .first()
                        .map(String::as_str)
                        .unwrap_or("127.0.0.1")
                )
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
                    "Wave {} \"{}\" is running on port {} (pid {})",
                    status.mode.as_deref().unwrap_or("server"),
                    status.name.as_deref().unwrap_or("Wave"),
                    status.port.unwrap_or_default(),
                    status.pid.unwrap_or_default()
                )
            } else {
                "Wave server is not running.".to_string()
            };
            ui::done(message, serde_json::json!({}));
        }
    }
}
