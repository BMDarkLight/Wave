// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Transport control for the background playback daemon.

use crate::cli::{daemon_cmd, render, ui, PlaybackCmd};
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
        PlaybackCmd::Status => cmd_playback_status(),
        PlaybackCmd::Shutdown => cmd_playback_shutdown(),
    }
}

fn cmd_playback_start(id: String) {
    match daemon_request(DaemonRequest::Start { id }) {
        Ok(resp) if resp.ok => {
            if let Some(msg) = resp.message {
                println!("{msg}");
            }
            println!("Playback running in background. Use `wave playback` subcommands to control.");
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
            if let Some(msg) = resp.message {
                println!("{msg}");
            }
        }
        Ok(Some(resp)) => {
            ui::fail(
                resp.error
                    .unwrap_or_else(|| "Failed to shut down daemon".to_string()),
                None,
                ui::EXIT_GENERAL,
            );
        }
        Ok(None) => println!("Playback daemon is not running."),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playback_status() {
    match daemon_request_if_running(DaemonRequest::Status) {
        Ok(Some(resp)) if resp.ok => {
            if let Some(status) = resp.status {
                print!("{}", render::playback_status(ui::current(), &status, None));
            }
        }
        Ok(Some(resp)) => {
            ui::fail(
                resp.error.unwrap_or_else(|| "Daemon error".to_string()),
                None,
                ui::EXIT_GENERAL,
            );
        }
        Ok(None) => ui::no_daemon(),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}
