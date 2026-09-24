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

/// What "Up next" can honestly show, given only the raw queue.
#[derive(Debug, PartialEq)]
pub enum Upcoming {
    /// Queue positions, as `wave queue list` numbers them.
    Positions(Vec<usize>),
    /// The daemon does not report its shuffled order, so the raw order
    /// would name the wrong tracks.
    Shuffled,
}

pub fn upcoming(status: &PlaybackStatus, queue_len: usize) -> Upcoming {
    if status.shuffle {
        return Upcoming::Shuffled;
    }
    if queue_len == 0 || status.queue_index == 0 {
        return Upcoming::Positions(Vec::new());
    }
    let wraps = status.repeat.eq_ignore_ascii_case("all");
    // queue_index counts from 1, so it is also the 0-based position of the
    // track after the current one.
    let positions = (0..PEEK.min(queue_len - 1))
        .map(|slot| status.queue_index + slot)
        .filter_map(|p| match p < queue_len {
            true => Some(p),
            false if wraps => Some(p % queue_len),
            false => None,
        })
        .collect();
    Upcoming::Positions(positions)
}

/// One screenful of the dashboard. Always the same height at a given width.
/// `next` pairs each upcoming queue position with its label.
pub fn frame(
    ui: &Ui,
    status: &PlaybackStatus,
    next: Option<&[(usize, String)]>,
    queue_len: usize,
    album: Option<&str>,
) -> String {
    let mut out = String::from("\n");
    out.push_str(&render::playback_status(ui, status, album));
    out.push('\n');
    out.push_str(&format!("  {}\n", ui.heading("Up next")));

    let digits = queue_len.max(1).to_string().len();
    let room = ui.width.saturating_sub(5 + digits + 2);
    let mut lines = Vec::new();
    match next {
        None => lines.push(format!(
            "     {}",
            ui.dim(&ui::truncate(
                "Shuffle is on, so the next track is not known here.",
                ui.width.saturating_sub(5),
                ui.glyphs.ellipsis
            ))
        )),
        Some(next) => {
            for (position, label) in next {
                lines.push(format!(
                    "     {}  {}",
                    ui.dim(&format!("{position:>digits$}")),
                    ui::truncate(label, room, ui.glyphs.ellipsis)
                ));
            }
        }
    }
    // Blank filler keeps the frame height constant.
    lines.resize(PEEK, String::new());
    for line in lines {
        out.push_str(&line);
        out.push('\n');
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

/// The queue entry that is playing. The status names only the file, so the
/// queue supplies the full path, and a disagreement between the two means the
/// status is the one to trust.
fn current_path(status: &PlaybackStatus, queue: &[String]) -> Option<String> {
    let at_index = status
        .queue_index
        .checked_sub(1)
        .and_then(|i| queue.get(i))
        .filter(|path| file_name(path) == status.file);
    at_index
        .or_else(|| queue.iter().find(|path| file_name(path) == status.file))
        .cloned()
}

fn file_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

/// Album and upcoming labels for one frame, looked up together.
struct Labels {
    album: Option<String>,
    next: Option<Vec<(usize, String)>>,
}

fn label(status: &PlaybackStatus, queue: &[String]) -> Labels {
    let current = current_path(status, queue);
    let positions = match upcoming(status, queue.len()) {
        Upcoming::Positions(positions) => Some(positions),
        Upcoming::Shuffled => None,
    };
    let mut paths: Vec<String> = current.iter().cloned().collect();
    if let Some(positions) = &positions {
        paths.extend(positions.iter().map(|&p| queue[p].clone()));
    }
    let known = crate::cli::library_tracks_for(&paths);
    let offset = usize::from(current.is_some());
    let album = current
        .and(known.first().cloned().flatten())
        .map(|t| t.album)
        .filter(|album| !album.is_empty());
    let next = positions.map(|positions| {
        positions
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let track = known.get(offset + i).and_then(Option::as_ref);
                (p, render::queue_label(track, &queue[p]))
            })
            .collect()
    });
    Labels { album, next }
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

    type LabelKey = (String, usize, bool, String, Vec<String>);
    let mut labels_for: Option<(LabelKey, Labels)> = None;
    let mut previous_height = 0usize;
    loop {
        let (status, queue) = fetch(live);
        if ui.json {
            json::emit(&status);
        }

        // Album and the upcoming titles are not in the daemon's status, so
        // they come from the library, and only again when the queue moves.
        let key: LabelKey = (
            status.file.clone(),
            status.queue_index,
            status.shuffle,
            status.repeat.clone(),
            queue.clone(),
        );
        if labels_for.as_ref().is_none_or(|(k, _)| *k != key) {
            labels_for = Some((key, label(&status, &queue)));
        }
        let labels = &labels_for.as_ref().expect("set just above").1;

        let mut text = frame(
            ui,
            &status,
            labels.next.as_deref(),
            queue.len(),
            labels.album.as_deref(),
        );
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
            file: "current.flac".into(),
            position_seconds: 72.0,
            duration_seconds: 260.0,
            volume: 0.62,
            device: "MacBook Pro Speakers".into(),
            repeat: "Off".into(),
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

    /// A frame labelled by file name, the way it looks for untagged files.
    fn frame_for(ui: &Ui, status: &PlaybackStatus, q: &[String], album: Option<&str>) -> String {
        let next = match upcoming(status, q.len()) {
            Upcoming::Positions(positions) => Some(
                positions
                    .into_iter()
                    .map(|p| (p, render::queue_label(None, &q[p])))
                    .collect::<Vec<_>>(),
            ),
            Upcoming::Shuffled => None,
        };
        frame(ui, status, next.as_deref(), q.len(), album)
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
        let mut shuffled = status();
        shuffled.shuffle = true;
        for width in [40, 60, 80, 120, 200] {
            for s in [status(), shuffled.clone()] {
                for line in frame_for(&at(width), &s, &q, Some("Remain in Light")).lines() {
                    assert!(
                        display_width(line) <= width,
                        "width {width} produced a {} column line: {line:?}",
                        display_width(line)
                    );
                }
            }
        }
    }

    #[test]
    fn a_frame_has_a_stable_height_so_redraw_can_rewind() {
        let ui = at(80);
        let empty = frame_for(&ui, &status(), &[], None).lines().count();
        let full = frame_for(
            &ui,
            &status(),
            &queue(&["current", "b", "c", "d"]),
            Some("Album"),
        )
        .lines()
        .count();
        let mut shuffled = status();
        shuffled.shuffle = true;
        let note = frame_for(&ui, &shuffled, &queue(&["current", "b"]), None)
            .lines()
            .count();
        assert_eq!(empty, full, "height must not depend on queue or album");
        assert_eq!(empty, note, "height must not depend on shuffle");
    }

    #[test]
    fn the_queue_peek_skips_the_track_already_playing() {
        let out = frame_for(&at(100), &status(), &queue(&["current", "next"]), None);
        assert!(out.contains("next.flac"));
        assert!(!out.contains("current.flac"));
    }

    #[test]
    fn the_queue_peek_numbers_match_queue_list() {
        // `wave queue list` numbers from 0, so the next track after the first
        // is shown as 1 here as well.
        let out = frame_for(&at(100), &status(), &queue(&["current", "next"]), None);
        let line = out.lines().find(|l| l.contains("next.flac")).unwrap();
        assert!(line.trim_start().starts_with('1'), "{line:?}");
    }

    #[test]
    fn the_peek_wraps_to_the_start_only_under_repeat_all() {
        let mut s = status();
        s.queue_index = 3;
        assert_eq!(upcoming(&s, 3), Upcoming::Positions(vec![]));
        s.repeat = "All".into();
        assert_eq!(upcoming(&s, 3), Upcoming::Positions(vec![0, 1]));
        // A lone track under repeat all is not its own "next".
        s.queue_index = 1;
        assert_eq!(upcoming(&s, 1), Upcoming::Positions(vec![]));
    }

    #[test]
    fn a_shuffled_queue_does_not_guess_what_plays_next() {
        let mut s = status();
        s.shuffle = true;
        assert_eq!(upcoming(&s, 5), Upcoming::Shuffled);
        let out = frame_for(&at(100), &s, &queue(&["current", "next"]), None);
        assert!(!out.contains("next.flac"));
        assert!(out.contains("Shuffle is on"));
    }

    #[test]
    fn the_current_path_follows_the_file_the_daemon_names() {
        let q = queue(&["a", "current", "b"]);
        let mut s = status();
        s.queue_index = 2;
        assert_eq!(current_path(&s, &q).as_deref(), Some("/music/current.flac"));
        // An index that points somewhere else loses to the named file.
        s.queue_index = 1;
        assert_eq!(current_path(&s, &q).as_deref(), Some("/music/current.flac"));
        s.file = "None".into();
        assert_eq!(current_path(&s, &q), None);
    }
}
