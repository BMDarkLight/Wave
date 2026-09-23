// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Transport control for the background playback daemon.

use serde_json::json;

use crate::cli::{daemon_cmd, ui, PlaybackCmd};
use crate::playback_daemon::{daemon_request, daemon_request_if_running, DaemonRequest};

pub fn run(cmd: PlaybackCmd) {
    match cmd {
        PlaybackCmd::Start { id } => cmd_playback_start(id),
        PlaybackCmd::Pause => daemon_cmd(DaemonRequest::Pause),
        PlaybackCmd::Resume => daemon_cmd(DaemonRequest::Resume),
        PlaybackCmd::Stop => daemon_cmd(DaemonRequest::Stop),
        PlaybackCmd::Next => daemon_cmd(DaemonRequest::Next),
        PlaybackCmd::Previous => daemon_cmd(DaemonRequest::Previous),
        PlaybackCmd::Seek { seconds } => daemon_cmd(DaemonRequest::Seek { seconds }),
        // One renderer for both, so the two views cannot drift apart.
        PlaybackCmd::Status => crate::cli::now::run(true, 0.5),
        PlaybackCmd::Shutdown => cmd_playback_shutdown(),
    }
}

fn cmd_playback_start(id: String) {
    match daemon_request(DaemonRequest::Start { id }) {
        Ok(resp) if resp.ok => {
            let msg = resp
                .message
                .unwrap_or_else(|| "Playback started.".to_string());
            ui::done(msg, json!({ "status": resp.status }));
            println!(
                "  {}",
                ui::current().dim("playing in the background; watch it with: wave now")
            );
        }
        Ok(resp) => {
            ui::fail(
                resp.error
                    .unwrap_or_else(|| "Failed to start playback".to_string()),
                None,
                ui::EXIT_GENERAL,
            );
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playback_shutdown() {
    match daemon_request_if_running(DaemonRequest::Shutdown) {
        Ok(Some(resp)) if resp.ok => {
            let msg = resp
                .message
                .unwrap_or_else(|| "Playback daemon stopped.".to_string());
            ui::done(msg, json!({}));
        }
        Ok(Some(resp)) => {
            ui::fail(
                resp.error
                    .unwrap_or_else(|| "Failed to shut down daemon".to_string()),
                None,
                ui::EXIT_GENERAL,
            );
        }
        // Shutting down something that is already down is not a failure.
        Ok(None) => ui::done("Playback daemon is not running.", json!({})),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}
