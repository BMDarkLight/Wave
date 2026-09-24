// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The daemon's play queue, shuffle and repeat.

use crate::cli::{
    daemon_cmd, json, library_tracks_for, open_library, render, track_path_or_exit, ui, QueueCmd,
    Switch,
};
use crate::playback_daemon::{daemon_request_if_running, DaemonRequest};

pub fn run(cmd: QueueCmd) {
    match cmd {
        QueueCmd::List => match daemon_request_if_running(DaemonRequest::QueueList) {
            Ok(Some(resp)) if resp.ok => {
                let tracks = resp.queue.unwrap_or_default();
                let shuffle = resp.status.as_ref().is_some_and(|s| s.shuffle);
                let current = resp.status.and_then(|s| s.queue_index.checked_sub(1));
                json::maybe_emit(&serde_json::json!({ "queue": tracks, "current": current }));
                let known = library_tracks_for(&tracks);
                let labels: Vec<String> = tracks
                    .iter()
                    .zip(&known)
                    .map(|(path, track)| render::queue_label(track.as_ref(), path))
                    .collect();
                let durations: Vec<Option<f64>> = known
                    .iter()
                    .map(|t| t.as_ref().and_then(|t| t.duration_seconds))
                    .collect();
                print!(
                    "{}",
                    render::queue_list(ui::current(), &labels, &durations, current, shuffle)
                );
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
        },
        QueueCmd::Add { track_id } => daemon_cmd(DaemonRequest::QueueAdd {
            track_id: track_path_or_exit(&open_library(), &track_id),
        }),
        QueueCmd::Remove { index } => daemon_cmd(DaemonRequest::QueueRemove { index }),
        QueueCmd::Next { track_id } => daemon_cmd(DaemonRequest::QueueInsertNext {
            track_id: track_path_or_exit(&open_library(), &track_id),
        }),
        QueueCmd::Shuffle { state } => daemon_cmd(DaemonRequest::QueueShuffle {
            enable: state.map(|s| matches!(s, Switch::On)),
        }),
        QueueCmd::Repeat { mode: Some(mode) } => daemon_cmd(DaemonRequest::QueueRepeat {
            mode: mode.as_str().to_string(),
        }),
        QueueCmd::Repeat { mode: None } => match daemon_request_if_running(DaemonRequest::Status) {
            Ok(Some(resp)) => {
                let repeat = resp
                    .status
                    .map(|s| s.repeat.to_ascii_lowercase())
                    .unwrap_or_else(|| "off".to_string());
                json::maybe_emit(&serde_json::json!({ "repeat": repeat }));
                println!("{}", ui::current().kv("Repeat", &repeat, 6));
            }
            Ok(None) => ui::no_daemon(),
            Err(e) => ui::fail(e, None, ui::EXIT_GENERAL),
        },
        QueueCmd::Clear => daemon_cmd(DaemonRequest::QueueClear),
    }
}
