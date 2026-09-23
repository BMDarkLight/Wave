// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The `wave now` dashboard.
//!
//! Redraws in place with cursor-up and erase-line rather than the alternate
//! screen, so scrollback survives and the terminal is left as it was found.

use std::io::{IsTerminal, Write};
use std::path::Path;

use crate::cli::ui::{self, Ui};
use crate::cli::{json, render};
use crate::playback_daemon::{daemon_request_if_running, DaemonRequest, PlaybackStatus};

/// Tracks shown under "Up next". Fixed, because the frame height must not
/// change between redraws.
const PEEK: usize = 2;

const SHOW_CURSOR: &str = "\x1b[?25h";
const HIDE_CURSOR: &str = "\x1b[?25l";

pub fn clamp_interval(seconds: f64) -> f64 {
    if seconds.is_nan() {
        0.5
    } else {
        seconds.clamp(0.1, 10.0)
    }
}

/// One screenful of the dashboard. Always the same height at a given width.
pub fn frame(ui: &Ui, status: &PlaybackStatus, queue: &[String], album: Option<&str>) -> String {
    let mut out = String::from("\n");
    out.push_str(&render::playback_status(ui, status, album));
    out.push('\n');
    out.push_str(&format!("  {}\n", ui.heading("Up next")));

    // queue_index counts from 1, so it is also the 0-based position of the
    // track after the current one. Numbers match `wave queue list`.
    let digits = queue.len().max(1).to_string().len();
    for slot in 0..PEEK {
        let position = status.queue_index + slot;
        match queue.get(position) {
            Some(path) => {
                let name = Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path);
                let room = ui.width.saturating_sub(5 + digits + 2);
                out.push_str(&format!(
                    "     {}  {}\n",
                    ui.dim(&format!("{position:>digits$}")),
                    ui::truncate(name, room, ui.glyphs.ellipsis)
                ));
            }
            // Blank filler keeps the frame height constant.
            None => out.push('\n'),
        }
    }
    out
}

/// Bring the cursor back before leaving. `process::exit` does not flush
/// stdout, so an unflushed escape would leave the terminal without one.
fn restore_cursor(live: bool) {
    if live {
        print!("{SHOW_CURSOR}");
        let _ = std::io::stdout().flush();
    }
}

/// One round trip for everything a frame needs. `QueueList` answers with the
/// status as well, which saves a second request and keeps the two in step.
/// Leaves through the standard no-daemon error when nothing is running.
fn fetch(live: bool) -> (PlaybackStatus, Vec<String>) {
    let response = match daemon_request_if_running(DaemonRequest::QueueList) {
        Ok(Some(response)) => response,
        Ok(None) => {
            restore_cursor(live);
            ui::no_daemon()
        }
        Err(e) => {
            restore_cursor(live);
            ui::fail(e, None, ui::EXIT_GENERAL)
        }
    };
    match response.status {
        Some(status) => (status, response.queue.unwrap_or_default()),
        None => {
            restore_cursor(live);
            let error = response
                .error
                .unwrap_or_else(|| "The daemon returned no status.".to_string());
            ui::fail(error, None, ui::EXIT_GENERAL)
        }
    }
}

fn lookup_album(file: &str) -> Option<String> {
    let library =
        crate::library::Library::new_with_path(&crate::app_paths::library_db_path()).ok()?;
    let tracks = library
        .get_tracks_by_paths(std::slice::from_ref(&file.to_string()))
        .ok()?;
    tracks
        .into_iter()
        .next()
        .flatten()
        .map(|t| t.album)
        .filter(|album| !album.is_empty())
}

pub fn run(once: bool, interval: f64) {
    let ui = ui::current();
    // Redrawing into a pipe writes escape sequences nobody can read, so a
    // non-terminal stdout gets exactly one frame, and so does --json.
    let live = !(once || ui.json || !std::io::stdout().is_terminal());
    let interval = clamp_interval(interval);

    if live {
        // Ctrl-C skips destructors, so the cursor has to come back from here.
        let _ = ctrlc::set_handler(|| {
            println!("{SHOW_CURSOR}");
            let _ = std::io::stdout().flush();
            std::process::exit(0);
        });
        print!("{HIDE_CURSOR}");
    }

    let mut album_for: Option<(String, Option<String>)> = None;
    let mut previous_height = 0usize;
    loop {
        let (status, queue) = fetch(live);
        if ui.json {
            json::emit(&status);
        }

        // Album is not in the daemon's status, so it comes from the library.
        // The status only names the file, so the lookup uses the queue's full
        // path for the current track, and repeats only when that changes.
        let current = status
            .queue_index
            .checked_sub(1)
            .and_then(|i| queue.get(i))
            .cloned()
            .unwrap_or_else(|| status.file.clone());
        let album = match &album_for {
            Some((path, album)) if *path == current => album.clone(),
            _ => {
                let album = lookup_album(&current);
                album_for = Some((current, album.clone()));
                album
            }
        };

        let mut text = frame(ui, &status, &queue, album.as_deref());
        if live {
            text.push_str(&format!(
                "\n  {}\n",
                ui.dim(&format!(
                    "Ctrl-C to exit {} refreshing every {interval}s",
                    ui.glyphs.dot
                ))
            ));
        }

        let mut out = String::new();
        if live && previous_height > 0 {
            out.push_str(&format!("\x1b[{previous_height}A\r"));
        }
        for line in text.lines() {
            out.push_str(line);
            if live {
                // Overwrite in place: erase whatever the last frame left on
                // this line past the new text.
                out.push_str("\x1b[K");
            }
            out.push('\n');
        }
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
        drop(stdout);
        previous_height = text.lines().count();

        if !live {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs_f64(interval));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ui::{display_width, Ui};

    fn status() -> PlaybackStatus {
        PlaybackStatus {
            state: "playing".into(),
            file: "/music/current.flac".into(),
            position_seconds: 72.0,
            duration_seconds: 260.0,
            volume: 0.62,
            device: "MacBook Pro Speakers".into(),
            repeat: "all".into(),
            shuffle: false,
            queue_index: 1,
            queue_total: 3,
            title: Some("Once in a Lifetime".into()),
            artist: Some("Talking Heads".into()),
        }
    }

    fn at(width: usize) -> Ui {
        Ui {
            width,
            ..Ui::plain()
        }
    }

    fn queue(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| format!("/music/{n}.flac")).collect()
    }

    #[test]
    fn interval_is_clamped_to_something_sane() {
        assert_eq!(clamp_interval(0.5), 0.5);
        assert_eq!(clamp_interval(0.0), 0.1);
        assert_eq!(clamp_interval(-5.0), 0.1);
        assert_eq!(clamp_interval(9999.0), 10.0);
        assert_eq!(clamp_interval(f64::NAN), 0.5);
        assert_eq!(clamp_interval(f64::INFINITY), 10.0);
    }

    #[test]
    fn a_frame_fits_the_terminal_at_every_width() {
        let q = queue(&[
            "current",
            "a much longer next track name than usual",
            "third",
        ]);
        for width in [40, 60, 80, 120, 200] {
            for line in frame(&at(width), &status(), &q, Some("Remain in Light")).lines() {
                assert!(
                    display_width(line) <= width,
                    "width {width} produced a {} column line: {line:?}",
                    display_width(line)
                );
            }
        }
    }

    #[test]
    fn a_frame_has_a_stable_height_so_redraw_can_rewind() {
        let ui = at(80);
        let empty = frame(&ui, &status(), &[], None).lines().count();
        let full = frame(
            &ui,
            &status(),
            &queue(&["current", "b", "c", "d"]),
            Some("Album"),
        )
        .lines()
        .count();
        assert_eq!(empty, full, "height must not depend on queue or album");
    }

    #[test]
    fn the_queue_peek_skips_the_track_already_playing() {
        let out = frame(&at(100), &status(), &queue(&["current", "next"]), None);
        assert!(out.contains("next.flac"));
        assert!(!out.contains("current.flac"));
    }

    #[test]
    fn the_queue_peek_numbers_match_queue_list() {
        // `wave queue list` numbers from 0, so the next track after the first
        // is shown as 1 here as well.
        let out = frame(&at(100), &status(), &queue(&["current", "next"]), None);
        let line = out.lines().find(|l| l.contains("next.flac")).unwrap();
        assert!(line.trim_start().starts_with('1'), "{line:?}");
    }
}
