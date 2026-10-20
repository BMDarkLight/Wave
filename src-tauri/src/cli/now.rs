// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The `wave now` dashboard. Redraws in place through `cli::term`. Left and
//! Right seek, Up and Down move through the queue, Space pauses and resumes,
//! and L shows and hides the lyrics of the playing track.

use std::io::{IsTerminal, Write};
use std::time::Duration;

use crate::cli::lyrics_view::{self, Follower, Lyrics};
use crate::cli::term::{self, Key, Live};
use crate::cli::ui::{self, Ui};
use crate::cli::{json, render};
use crate::lyrics::LyricsKind;
use crate::playback_daemon::{daemon_request_if_running, DaemonRequest, PlaybackStatus};

/// Tracks shown under "Up next". Fixed, because the frame height must not
/// change between redraws.
const PEEK: usize = 2;

/// Rows of lyrics under the status, at most. Fewer on a short terminal.
const LYRIC_ROWS: usize = 9;

/// Seconds Left and Right move, the same step as the arrow keys in the app.
const SEEK_STEP: f64 = 5.0;

pub fn clamp_interval(seconds: f64) -> f64 {
    if seconds.is_nan() {
        0.5
    } else {
        seconds.clamp(0.1, 10.0)
    }
}

/// Queue positions that play next, as `wave queue list` numbers them. Shuffle
/// reorders the queue itself, so this holds with shuffle on as well.
pub fn upcoming(status: &PlaybackStatus, queue_len: usize) -> Vec<usize> {
    let Some(current) = status.queue_position else {
        return Vec::new();
    };
    if queue_len == 0 {
        return Vec::new();
    }
    let wraps = status.repeat.eq_ignore_ascii_case("all");
    (0..PEEK.min(queue_len - 1))
        .map(|slot| current + 1 + slot)
        .filter_map(|p| match p < queue_len {
            true => Some(p),
            false if wraps => Some(p % queue_len),
            false => None,
        })
        .collect()
}

/// What the daemon should do for a key, given where playback is. `None` for
/// keys that only change the dashboard, or that mean nothing right now.
pub fn request_for(key: Key, status: &PlaybackStatus) -> Option<DaemonRequest> {
    match key {
        Key::Left | Key::Right => {
            status.path.as_ref()?;
            let step = if key == Key::Right {
                SEEK_STEP
            } else {
                -SEEK_STEP
            };
            let mut to = (status.position_seconds + step).max(0.0);
            if let Some(duration) = status.duration_seconds.filter(|d| *d > 0.0) {
                to = to.min(duration);
            }
            Some(DaemonRequest::Seek { seconds: to })
        }
        // "Up next" is listed below the playing track, so Down goes there.
        Key::Down => Some(DaemonRequest::Next),
        Key::Up => Some(DaemonRequest::Previous),
        Key::PlayPause => match status.state.as_str() {
            "playing" => Some(DaemonRequest::Pause),
            "paused" => Some(DaemonRequest::Resume),
            _ => None,
        },
        _ => None,
    }
}

/// The keys the dashboard answers to, for the line under it.
pub fn key_hint(ui: &Ui, status: &PlaybackStatus, show_lyrics: bool) -> String {
    let pause = if status.state == "playing" {
        "pause"
    } else {
        "play"
    };
    let lyrics = if show_lyrics { "hide" } else { "show" };
    let dot = format!(" {} ", ui.glyphs.dot);
    [
        format!("{} to seek", ui.glyphs.left_right),
        format!("{} to skip", ui.glyphs.up_down),
        format!("Space to {pause}"),
        format!("L to {lyrics} lyrics"),
        "Q to exit".to_string(),
    ]
    .join(&dot)
}

/// One screenful of the dashboard. Always the same height at a given width.
/// `next` pairs each upcoming queue position with its label.
pub fn frame(
    ui: &Ui,
    status: &PlaybackStatus,
    next: &[(usize, String)],
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
    for (position, label) in next {
        lines.push(format!(
            "     {}  {}",
            ui.dim(&format!("{position:>digits$}")),
            ui::truncate(label, room, ui.glyphs.ellipsis)
        ));
    }
    // Blank filler keeps the frame height constant.
    lines.resize(PEEK, String::new());
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// One round trip for everything a frame needs. `QueueList` answers with the
/// status as well, which saves a second request and keeps the two in step.
/// Leaves through the standard no-daemon error when nothing is running.
fn fetch() -> (PlaybackStatus, Vec<String>) {
    let response = match daemon_request_if_running(DaemonRequest::QueueList) {
        Ok(Some(response)) => response,
        Ok(None) => {
            term::restore();
            ui::no_daemon()
        }
        Err(e) => {
            term::restore();
            ui::fail_with(e)
        }
    };
    match response.status {
        Some(status) => (status, response.queue.unwrap_or_default()),
        None => {
            term::restore();
            let error = response
                .error
                .unwrap_or_else(|| "The daemon returned no status.".to_string());
            ui::fail(error, None, ui::EXIT_GENERAL)
        }
    }
}

/// The lyrics panel, placed between the status and the queue peek.
fn lyrics_block(ui: &Ui, status: &PlaybackStatus, lyrics: &Lyrics, rows: usize) -> String {
    let duration = status.duration_seconds.unwrap_or(0.0);
    let progress = if duration > 0.0 {
        status.position_seconds / duration
    } else {
        0.0
    };
    let mut out = format!("  {}\n", ui.heading("Lyrics"));
    for line in lyrics_view::panel(ui, lyrics, status.position_seconds, progress, rows) {
        out.push_str(&line);
        out.push('\n');
    }
    out.push('\n');
    out
}

/// Rows the lyrics panel gets: what the terminal has left over after the
/// rest of the dashboard, within reason.
fn lyric_rows(dashboard_height: usize) -> usize {
    term::terminal_height()
        .saturating_sub(dashboard_height + 2)
        .clamp(3, LYRIC_ROWS)
}

/// Album and upcoming labels for one frame, looked up together.
struct Labels {
    album: Option<String>,
    next: Vec<(usize, String)>,
}

fn label(status: &PlaybackStatus, queue: &[String]) -> Labels {
    let current = status.path.clone();
    let positions = upcoming(status, queue.len());
    let mut paths: Vec<String> = current.iter().cloned().collect();
    paths.extend(positions.iter().map(|&p| queue[p].clone()));
    let known = crate::cli::library_tracks_for(&paths);
    let offset = usize::from(current.is_some());
    let album = current
        .and(known.first().cloned().flatten())
        .map(|t| t.album)
        .filter(|album| !album.is_empty());
    let next = positions
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let track = known.get(offset + i).and_then(Option::as_ref);
            (p, render::queue_label(track, &queue[p]))
        })
        .collect();
    Labels { album, next }
}

pub fn run(once: bool, interval: f64, show_lyrics: bool) {
    let ui = ui::current();
    // Redrawing into a pipe writes escape sequences nobody can read, so a
    // non-terminal stdout gets exactly one frame, and so does --json.
    let live = !(once || ui.json || !std::io::stdout().is_terminal());
    let interval = clamp_interval(interval);
    let mut show_lyrics = show_lyrics && !ui.json;
    // Asking LRCLIB is a network call, so a single frame does without it and
    // shows what the library and the file already have.
    let mut follower = Follower::new(live);
    let mut screen = live.then(Live::start);

    type LabelKey = (Option<String>, Option<usize>, String, Vec<String>);
    let mut labels_for: Option<(LabelKey, Labels)> = None;
    loop {
        let (status, queue) = fetch();
        if ui.json {
            json::emit(&status);
        }

        // Album and the upcoming titles are not in the daemon's status, so
        // they come from the library, and only again when the queue moves.
        let key: LabelKey = (
            status.path.clone(),
            status.queue_position,
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
            &labels.next,
            queue.len(),
            labels.album.as_deref(),
        );
        let lyrics = if show_lyrics {
            let lyrics = match screen {
                Some(_) => follower.get(status.path.as_deref()),
                // One frame cannot wait for a background load, so read the
                // lyrics here instead.
                None => Follower::load_now(status.path.as_deref()),
            };
            // The panel goes in after the status, before "Up next".
            let at = text
                .find(&format!("  {}", ui.heading("Up next")))
                .unwrap_or(text.len());
            let rows = lyric_rows(text.lines().count() + 2);
            text.insert_str(at, &lyrics_block(ui, &status, &lyrics, rows));
            Some(lyrics)
        } else {
            None
        };

        let Some(screen) = screen.as_mut() else {
            print!("{text}");
            let _ = std::io::stdout().flush();
            return;
        };
        let hint = if screen.reads_keys() {
            key_hint(ui, &status, show_lyrics)
        } else {
            "Ctrl-C to exit".to_string()
        };
        let hint = ui::truncate(&hint, ui.width.saturating_sub(2), ui.glyphs.ellipsis);
        text.push_str(&format!("\n  {}\n", ui.dim(&hint)));
        screen.draw(&text);

        // Word highlighting needs more than the dashboard's default pace.
        let wait = match lyrics {
            Some(Lyrics::Ready(ref sheet)) if sheet.kind != LyricsKind::Plain => interval.min(0.2),
            _ => interval,
        };
        match screen.wait(Duration::from_secs_f64(wait)) {
            Some(Key::Quit) => return,
            Some(Key::Char('l')) => show_lyrics = !show_lyrics,
            Some(Key::Redraw) | None => {}
            Some(key) => {
                // A slow --interval leaves the frame's position behind, so
                // seek from where playback is now.
                let (now, _) = fetch();
                if let Some(request) = request_for(key, &now) {
                    // What changed shows up in the next frame, drawn
                    // straight away; an error leaves playback as it was.
                    let _ = daemon_request_if_running(request);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ui::{display_width, Ui};

    fn status() -> PlaybackStatus {
        PlaybackStatus {
            state: "playing".into(),
            file: Some("current.flac".into()),
            path: Some("/music/current.flac".into()),
            position_seconds: 72.0,
            duration_seconds: Some(260.0),
            volume: 0.62,
            device: "MacBook Pro Speakers".into(),
            repeat: "off".into(),
            shuffle: false,
            queue_position: Some(0),
            queue_length: 3,
            title: Some("Once in a Lifetime".into()),
            artist: Some("Talking Heads".into()),
            sleep_timer: Default::default(),
            speed: 1.0,
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
        let next: Vec<_> = upcoming(status, q.len())
            .into_iter()
            .map(|p| (p, render::queue_label(None, &q[p])))
            .collect();
        frame(ui, status, &next, q.len(), album)
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
        s.queue_position = Some(2);
        assert_eq!(upcoming(&s, 3), Vec::<usize>::new());
        s.repeat = "all".into();
        assert_eq!(upcoming(&s, 3), vec![0, 1]);
        // A lone track under repeat all is not its own "next".
        s.queue_position = Some(0);
        assert_eq!(upcoming(&s, 1), Vec::<usize>::new());
    }

    fn seek_target(key: Key, status: &PlaybackStatus) -> Option<f64> {
        match request_for(key, status) {
            Some(DaemonRequest::Seek { seconds }) => Some(seconds),
            _ => None,
        }
    }

    #[test]
    fn left_and_right_seek_five_seconds_within_the_track() {
        let mut s = status();
        assert_eq!(seek_target(Key::Right, &s), Some(77.0));
        assert_eq!(seek_target(Key::Left, &s), Some(67.0));
        s.position_seconds = 2.0;
        assert_eq!(seek_target(Key::Left, &s), Some(0.0));
        s.position_seconds = 258.0;
        assert_eq!(seek_target(Key::Right, &s), Some(260.0));
    }

    #[test]
    fn seeking_needs_a_loaded_track() {
        let mut s = status();
        s.path = None;
        s.state = "stopped".into();
        assert!(request_for(Key::Right, &s).is_none());
    }

    #[test]
    fn down_plays_what_is_up_next_and_up_goes_back() {
        let s = status();
        assert!(matches!(
            request_for(Key::Down, &s),
            Some(DaemonRequest::Next)
        ));
        assert!(matches!(
            request_for(Key::Up, &s),
            Some(DaemonRequest::Previous)
        ));
    }

    #[test]
    fn space_pauses_what_plays_and_resumes_what_is_paused() {
        let mut s = status();
        assert!(matches!(
            request_for(Key::PlayPause, &s),
            Some(DaemonRequest::Pause)
        ));
        s.state = "paused".into();
        assert!(matches!(
            request_for(Key::PlayPause, &s),
            Some(DaemonRequest::Resume)
        ));
        s.state = "stopped".into();
        assert!(request_for(Key::PlayPause, &s).is_none());
    }

    #[test]
    fn the_key_hint_names_every_key_in_plain_text_too() {
        let ascii = Ui {
            glyphs: crate::cli::ui::ASCII,
            ..Ui::plain()
        };
        let mut s = status();
        let hint = key_hint(&ascii, &s, false);
        for part in [
            "Left/Right to seek",
            "Up/Down to skip",
            "Space to pause",
            "L to show lyrics",
            "Q to exit",
        ] {
            assert!(hint.contains(part), "{hint:?} lacks {part:?}");
        }
        s.state = "paused".into();
        assert!(key_hint(&ascii, &s, true).contains("Space to play"));
        assert!(key_hint(&ascii, &s, true).contains("L to hide lyrics"));
    }

    #[test]
    fn a_shuffled_queue_peeks_in_queue_order() {
        let mut s = status();
        s.shuffle = true;
        assert_eq!(upcoming(&s, 5), vec![1, 2]);
        let out = frame_for(&at(100), &s, &queue(&["current", "next"]), None);
        assert!(out.contains("next.flac"));
    }
}
