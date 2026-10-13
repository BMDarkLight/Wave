// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Lyrics in the terminal: a window of lines that follows playback, and a
//! loader that finds a track's lyrics off the drawing thread.
//!
//! Shared by `wave lyrics show --live` and the lyrics panel in `wave now`.

use std::sync::{Arc, Mutex};

use crate::cli::ui::{display_width, truncate, Ui};
use crate::lyrics::{LyricsKind, LyricsLine, LyricsSheet};

const INDENT: &str = "    ";

/// The line being sung at `position`: the last one that has started.
/// `None` before the first line, during the intro.
pub fn active_line(lines: &[LyricsLine], position: f64) -> Option<usize> {
    lines.iter().rposition(|line| line.time <= position)
}

/// First line of a `rows` tall window that keeps `active` near the middle
/// without running past either end of the sheet.
fn window_start(total: usize, rows: usize, active: Option<usize>) -> usize {
    if total <= rows {
        return 0;
    }
    let wanted = active.unwrap_or(0).saturating_sub(rows / 2);
    wanted.min(total - rows)
}

/// Byte offset into `text` up to which the words that have started are
/// found. Word chunks are matched in order rather than glued together,
/// because TTML words do not always carry the spaces between them.
fn sung_prefix(line: &LyricsLine, position: f64) -> usize {
    let mut cursor = 0;
    for word in line.words.iter().filter(|w| w.time <= position) {
        let needle = word.text.trim();
        match line.text[cursor..].find(needle) {
            Some(at) => cursor += at + needle.len(),
            None => break,
        }
    }
    cursor
}

fn active_text(ui: &Ui, line: &LyricsLine, position: f64, room: usize) -> String {
    let text = truncate(&line.text, room, ui.glyphs.ellipsis);
    if line.words.is_empty() || !ui.color {
        return ui.heading(&text);
    }
    // Truncation only ever shortens the tail, so the split still lands on
    // the same characters; clamp it to what survived.
    let mut split = sung_prefix(line, position).min(text.len());
    while !text.is_char_boundary(split) {
        split -= 1;
    }
    let (sung, ahead) = text.split_at(split);
    format!("{}{ahead}", ui.heading(sung))
}

/// Exactly `rows` lines of lyrics for a playback `position`.
///
/// Synced lyrics centre the line being sung and highlight it, word by word
/// when the sheet has word timings. Plain lyrics have nothing to follow, so
/// they scroll through the song at the pace of `progress` (0 to 1).
pub fn window(
    ui: &Ui,
    sheet: &LyricsSheet,
    position: f64,
    progress: f64,
    rows: usize,
) -> Vec<String> {
    let room = ui.width.saturating_sub(display_width(INDENT));
    let mut out = Vec::with_capacity(rows);

    if sheet.kind == LyricsKind::Plain {
        let lines: Vec<&str> = sheet.plain.lines().map(str::trim_end).collect();
        let spare = lines.len().saturating_sub(rows);
        let start = (progress.clamp(0.0, 1.0) * spare as f64).round() as usize;
        for line in lines.iter().skip(start).take(rows) {
            out.push(format!(
                "{INDENT}{}",
                truncate(line, room, ui.glyphs.ellipsis)
            ));
        }
    } else {
        let active = active_line(&sheet.lines, position);
        let start = window_start(sheet.lines.len(), rows, active);
        for (index, line) in sheet.lines.iter().enumerate().skip(start).take(rows) {
            let text = if line.text.is_empty() {
                // An empty timed line is an instrumental break.
                ui.dim(ui.glyphs.ellipsis)
            } else if Some(index) == active {
                active_text(ui, line, position, room)
            } else {
                ui.dim(&truncate(&line.text, room, ui.glyphs.ellipsis))
            };
            // The marker carries the highlight when there is no colour.
            let lead = if Some(index) == active {
                format!("  {} ", ui.glyphs.playing)
            } else {
                INDENT.to_string()
            };
            out.push(format!("{lead}{text}"));
        }
    }

    out.resize(rows, String::new());
    out
}

/// Exactly `rows` lines with a single message in the middle, for when there
/// is nothing to show yet.
pub fn notice(ui: &Ui, message: &str, rows: usize) -> Vec<String> {
    let room = ui.width.saturating_sub(display_width(INDENT));
    let mut out = vec![String::new(); rows];
    if rows > 0 {
        out[rows / 2] = format!(
            "{INDENT}{}",
            ui.dim(&truncate(message, room, ui.glyphs.ellipsis))
        );
    }
    out
}

// ── Loading ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Lyrics {
    /// Reading the library and the file.
    Loading,
    /// Nothing local; asking the online provider.
    Searching,
    /// Nothing anywhere.
    Missing,
    Ready(LyricsSheet),
}

/// Follows the playing track and loads its lyrics on a background thread,
/// so a slow lookup never stalls the redraw loop.
pub struct Follower {
    path: Option<String>,
    online: bool,
    shared: Arc<Mutex<(Option<String>, Lyrics)>>,
}

impl Follower {
    /// With `online`, a track with no lyrics of its own is looked up on
    /// LRCLIB, and what turns up is saved to the library.
    pub fn new(online: bool) -> Self {
        Self {
            path: None,
            online,
            shared: Arc::new(Mutex::new((None, Lyrics::Missing))),
        }
    }

    /// The lyrics for `path`, starting a lookup when the track has changed.
    pub fn get(&mut self, path: Option<&str>) -> Lyrics {
        if self.path.as_deref() != path {
            self.path = path.map(str::to_string);
            *self.shared.lock().unwrap_or_else(|e| e.into_inner()) =
                (self.path.clone(), Lyrics::Loading);
            if let Some(path) = self.path.clone() {
                self.spawn(path);
            }
        }
        let Some(path) = path else {
            return Lyrics::Missing;
        };
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        match &*guard {
            (Some(for_path), lyrics) if for_path == path => lyrics.clone(),
            _ => Lyrics::Loading,
        }
    }

    /// The lyrics for `path`, read on this thread from the library and the
    /// file only. For a single frame, which has no next redraw to wait for.
    pub fn load_now(path: Option<&str>) -> Lyrics {
        match path {
            Some(path) => load(path, false, |_| {}),
            None => Lyrics::Missing,
        }
    }

    fn spawn(&self, path: String) {
        let shared = Arc::clone(&self.shared);
        let online = self.online;
        std::thread::spawn(move || {
            // A result for a track that is no longer playing is dropped.
            let publish = |lyrics: Lyrics| {
                let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                if guard.0.as_deref() == Some(path.as_str()) {
                    guard.1 = lyrics;
                }
            };
            publish(load(&path, online, publish));
        });
    }
}

/// Find the lyrics for `path`: the library first, then the file, then, with
/// `online`, LRCLIB. `progress` hears about the online step before it starts.
fn load(path: &str, online: bool, progress: impl Fn(Lyrics)) -> Lyrics {
    use crate::cli::cmd::lyrics::{fetch_online, local_track};

    let library = crate::library::Library::new_with_path(&crate::app_paths::library_db_path()).ok();
    let mut track = local_track(library.as_ref(), path);
    let has_synced = track
        .as_ref()
        .and_then(|t| t.lyrics.as_deref())
        .is_some_and(crate::lyrics::is_synced);
    if online && !has_synced {
        if let Some(track) = track.as_mut() {
            progress(Lyrics::Searching);
            fetch_online(library.as_ref(), track);
        }
    }
    match track
        .and_then(|t| t.lyrics)
        .filter(|text| !text.trim().is_empty())
    {
        Some(text) => Lyrics::Ready(crate::lyrics::parse_sheet(&text)),
        None => Lyrics::Missing,
    }
}

/// Exactly `rows` lines for whatever state the lyrics are in.
pub fn panel(ui: &Ui, lyrics: &Lyrics, position: f64, progress: f64, rows: usize) -> Vec<String> {
    match lyrics {
        Lyrics::Ready(sheet) => window(ui, sheet, position, progress, rows),
        Lyrics::Loading => notice(ui, "Loading lyrics", rows),
        Lyrics::Searching => notice(ui, "Looking up lyrics online", rows),
        Lyrics::Missing => notice(ui, "No lyrics found for this track", rows),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::parse_sheet;

    fn at(width: usize) -> Ui {
        Ui {
            width,
            ..Ui::plain()
        }
    }

    const LRC: &str = "[00:01.00]one\n[00:02.00]two\n[00:03.00]three\n[00:04.00]four\n\
                       [00:05.00]five\n[00:06.00]six\n[00:07.00]seven";

    #[test]
    fn the_active_line_is_the_last_one_started() {
        let sheet = parse_sheet(LRC);
        assert_eq!(active_line(&sheet.lines, 0.5), None);
        assert_eq!(active_line(&sheet.lines, 1.0), Some(0));
        assert_eq!(active_line(&sheet.lines, 3.5), Some(2));
        assert_eq!(active_line(&sheet.lines, 99.0), Some(6));
    }

    #[test]
    fn the_window_keeps_the_active_line_in_the_middle() {
        let sheet = parse_sheet(LRC);
        let lines = window(&at(80), &sheet, 4.2, 0.0, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("three"));
        assert!(lines[1].contains("four"));
        assert!(lines[2].contains("five"));
    }

    #[test]
    fn the_window_stops_at_either_end() {
        let sheet = parse_sheet(LRC);
        let start = window(&at(80), &sheet, 0.0, 0.0, 3);
        assert!(start[0].contains("one"));
        let end = window(&at(80), &sheet, 60.0, 1.0, 3);
        assert!(end[2].contains("seven"));
    }

    #[test]
    fn the_window_always_has_the_height_asked_for() {
        let sheet = parse_sheet("[00:01.00]only");
        assert_eq!(window(&at(80), &sheet, 2.0, 0.5, 7).len(), 7);
        let plain = parse_sheet("just\nwords");
        assert_eq!(window(&at(80), &plain, 0.0, 0.5, 7).len(), 7);
        assert_eq!(notice(&at(80), "nothing", 5).len(), 5);
    }

    #[test]
    fn plain_lyrics_scroll_with_the_song() {
        let text: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let sheet = parse_sheet(&text);
        assert!(window(&at(80), &sheet, 0.0, 0.0, 5)[0].ends_with("line 1"));
        assert!(window(&at(80), &sheet, 0.0, 1.0, 5)[4].ends_with("line 20"));
    }

    #[test]
    fn lines_fit_the_terminal() {
        let sheet =
            parse_sheet("[00:01.00]a line that is far too long to fit on a narrow terminal at all");
        for width in [20, 40, 80] {
            for line in window(&at(width), &sheet, 1.5, 0.0, 3) {
                assert!(display_width(&line) <= width, "{line:?}");
            }
        }
    }

    #[test]
    fn word_timings_split_the_line_where_the_singer_is() {
        let sheet = parse_sheet("[00:01.00]<00:01.00>hello <00:02.00>there <00:03.00>world");
        let line = &sheet.lines[0];
        assert_eq!(&line.text[..sung_prefix(line, 1.5)], "hello");
        assert_eq!(&line.text[..sung_prefix(line, 2.5)], "hello there");
        assert_eq!(sung_prefix(line, 0.5), 0);
    }
}
