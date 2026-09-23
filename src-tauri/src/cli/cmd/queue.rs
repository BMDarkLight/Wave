// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The daemon's play queue, shuffle and repeat.

use std::path::Path;

use crate::cli::{daemon_cmd, json, ui, QueueCmd};
use crate::playback_daemon::{daemon_request_if_running, DaemonRequest};

pub fn run(cmd: QueueCmd) {
    match cmd {
        QueueCmd::List => match daemon_request_if_running(DaemonRequest::QueueList) {
            Ok(Some(resp)) if resp.ok => {
                let tracks = resp.queue.unwrap_or_default();
                let current = resp.status.and_then(|s| {
                    if s.queue_index > 0 {
                        Some(s.queue_index - 1)
                    } else {
                        None
                    }
                });
                json::maybe_emit(&serde_json::json!({ "queue": tracks, "current": current }));
                let ui = ui::current();
                if tracks.is_empty() {
                    println!("Queue is empty.");
                    return;
                }
                println!("{}\n", ui.heading(&format!("{} in queue", tracks.len())));
                // The index shown is the one `wave queue remove` takes.
                let digits = (tracks.len() - 1).to_string().len();
                for (i, path) in tracks.iter().enumerate() {
                    let name = Path::new(path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(path);
                    let room = ui.width.saturating_sub(digits + 7);
                    let name = ui::truncate(name, room, ui.glyphs.ellipsis);
                    if Some(i) == current {
                        let line = format!("{} {i:>digits$}  {name}", ui.glyphs.playing);
                        println!("  {}", ui.paint(ui::style::OK, &line));
                    } else {
                        println!("    {}  {name}", ui.dim(&format!("{i:>digits$}")));
                    }
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
        },
        QueueCmd::Add { track_id } => daemon_cmd(DaemonRequest::QueueAdd { track_id }),
        QueueCmd::Remove { index } => daemon_cmd(DaemonRequest::QueueRemove { index }),
        QueueCmd::Next { track_id } => daemon_cmd(DaemonRequest::QueueInsertNext { track_id }),
        QueueCmd::Shuffle { state } => {
            let enable = match state.as_deref() {
                Some("on") => Some(true),
                Some("off") => Some(false),
                None => None,
                Some(other) => {
                    ui::fail(
                        format!("Invalid shuffle value: {other}"),
                        Some("use on, off, or omit to toggle"),
                        ui::EXIT_GENERAL,
                    );
                }
            };
            daemon_cmd(DaemonRequest::QueueShuffle { enable });
        }
        QueueCmd::Repeat { mode } => daemon_cmd(DaemonRequest::QueueRepeat { mode }),
        QueueCmd::Clear => daemon_cmd(DaemonRequest::QueueClear),
    }
}
