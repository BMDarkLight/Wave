// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Fetching and reading a track's lyrics, as text or following playback.

use std::time::Duration;

use serde_json::json;

use crate::cli::lyrics_view::{self, Follower, Lyrics};
use crate::cli::term::{self, Key, Live};
use crate::cli::{json, open_library, render, track_path_or_exit, ui, LyricsCmd};
use crate::library::Library;
use crate::lyrics::{parse_sheet, LyricsKind};
use crate::metadata::{extract_track, Track};
use crate::playback_daemon::{daemon_request_if_running, DaemonRequest, PlaybackStatus};

/// Rows of lyrics in the live view, at most. Fewer on a short terminal.
const LIVE_ROWS: usize = 15;

pub fn run(cmd: LyricsCmd) {
    match cmd {
        LyricsCmd::Fetch { track_id, force } => cmd_fetch(track_id, force),
        LyricsCmd::Show {
            live: true,
            interval,
            ..
        } => cmd_show_live(interval),
        LyricsCmd::Show {
            track_id,
            timestamps,
            ..
        } => cmd_show(track_id, timestamps),
    }
}

/// The track as the library knows it, with its lyrics, or as read from the
/// file when it is not in the library. The file read picks up embedded
/// lyrics and sidecar `.lrc`/`.ttml` files.
pub(crate) fn local_track(library: Option<&Library>, path: &str) -> Option<Track> {
    library
        .and_then(|library| library.get_track_details(path).ok().flatten())
        .or_else(|| extract_track(None, path).ok())
}

/// Ask LRCLIB for the track's lyrics and, when it has some, put them on the
/// track and save them to the library. Returns whether any were found.
/// What the track already had is kept when the lookup comes back empty.
pub(crate) fn fetch_online(library: Option<&Library>, track: &mut Track) -> bool {
    let mut probe = track.clone();
    probe.lyrics = None;
    probe.lyrics_source = None;
    crate::metadata::enrich_lyrics_online(&mut probe);
    let (Some(lyrics), Some(source)) = (probe.lyrics, probe.lyrics_source) else {
        return false;
    };
    if let Some(library) = library {
        // A file outside the library has no row to update, which is fine.
        let _ = library.set_track_lyrics(&track.id, &lyrics, &source);
    }
    track.has_synced_lyrics = probe.has_synced_lyrics;
    track.lyrics = Some(lyrics);
    track.lyrics_source = Some(source);
    true
}

/// The path of the track the daemon has loaded, or leave with a hint.
fn playing_path_or_exit() -> String {
    let status = match daemon_request_if_running(DaemonRequest::Status) {
        Ok(Some(response)) => response.status,
        Ok(None) => ui::fail(
            "Nothing is playing, so there is no track to use.",
            Some("name one, as in: wave lyrics show TRACK"),
            ui::EXIT_NO_DAEMON,
        ),
        Err(e) => ui::fail_with(e),
    };
    status.and_then(|s| s.path).unwrap_or_else(|| {
        ui::fail(
            "Nothing is playing, so there is no track to use.",
            Some("name one, as in: wave lyrics show TRACK"),
            ui::EXIT_NOT_FOUND,
        )
    })
}

fn path_for(library: &Library, track_id: Option<String>) -> String {
    match track_id {
        Some(id) => track_path_or_exit(library, &id),
        None => playing_path_or_exit(),
    }
}

fn track_or_exit(library: &Library, path: &str) -> Track {
    local_track(Some(library), path).unwrap_or_else(|| {
        ui::fail(
            format!("Could not read track: {path}"),
            None,
            ui::EXIT_GENERAL,
        )
    })
}

fn describe(kind: LyricsKind) -> &'static str {
    match kind {
        LyricsKind::Plain => "plain",
        LyricsKind::Line => "synced by line",
        LyricsKind::Word => "synced by word",
    }
}

fn cmd_fetch(track_id: Option<String>, force: bool) {
    let library = open_library();
    let path = path_for(&library, track_id);
    let mut track = track_or_exit(&library, &path);

    // Synced lyrics are as good as the provider serves, so asking again
    // would only repeat the request. Plain ones are worth upgrading.
    let synced = track
        .lyrics
        .as_deref()
        .is_some_and(crate::lyrics::is_synced);
    if synced && !force {
        let kind = parse_sheet(track.lyrics.as_deref().unwrap_or("")).kind;
        ui::done(
            format!("{} already has synced lyrics.", label(&track)),
            json!({
                "path": path,
                "source": track.lyrics_source,
                "kind": kind,
                "fetched": false,
            }),
        );
        println!(
            "  {}",
            ui::current().dim("ask again anyway with: wave lyrics fetch --force")
        );
        return;
    }

    if !fetch_online(Some(&library), &mut track) {
        let hint = match track.lyrics {
            Some(_) => "the lyrics it already had are kept",
            None => "add an .lrc file next to the audio file to use your own",
        };
        ui::fail(
            format!("No lyrics found online for {}.", label(&track)),
            Some(hint),
            ui::EXIT_NOT_FOUND,
        );
    }

    let sheet = parse_sheet(track.lyrics.as_deref().unwrap_or(""));
    let lines = match sheet.kind {
        LyricsKind::Plain => sheet.plain.lines().count(),
        _ => sheet.lines.len(),
    };
    ui::done(
        format!(
            "Found {} for {} ({}).",
            ui::count(lines, "line", "lines"),
            label(&track),
            describe(sheet.kind)
        ),
        json!({
            "path": path,
            "source": track.lyrics_source,
            "kind": sheet.kind,
            "lines": lines,
            "fetched": true,
        }),
    );
}

fn label(track: &Track) -> String {
    render::queue_label(Some(track), &track.path)
}

fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

fn cmd_show(track_id: Option<String>, timestamps: bool) {
    let library = open_library();
    let path = path_for(&library, track_id);
    let track = track_or_exit(&library, &path);
    let Some(text) = track.lyrics.as_deref().filter(|t| !t.trim().is_empty()) else {
        ui::fail(
            format!("No lyrics for {}.", label(&track)),
            Some(&format!(
                "look them up with: wave lyrics fetch \"{}\"",
                track.path
            )),
            ui::EXIT_NOT_FOUND,
        );
    };
    let sheet = parse_sheet(text);

    if ui::current().json {
        json::emit(&json!({
            "path": track.path,
            "title": track.title,
            "artist": track.artist,
            "source": track.lyrics_source,
            "sheet": sheet,
        }));
    }

    let ui = ui::current();
    let mut out = format!("\n  {}\n", ui.heading(&track.title));
    if !track.artist.is_empty() {
        out.push_str(&format!("  {}\n", ui.dim(&track.artist)));
    }
    out.push('\n');
    if sheet.kind == LyricsKind::Plain {
        for line in sheet.plain.lines() {
            out.push_str(&format!("  {}\n", line.trim_end()));
        }
    } else {
        for line in &sheet.lines {
            let stamp = if timestamps {
                format!("{}  ", ui.dim(&format!("{:>5}", clock(line.time))))
            } else {
                String::new()
            };
            out.push_str(&format!("  {stamp}{}\n", line.text));
        }
    }
    print!("{out}");
}

fn status_or_exit() -> PlaybackStatus {
    let response = match daemon_request_if_running(DaemonRequest::Status) {
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
    response.status.unwrap_or_else(|| {
        term::restore();
        let error = response
            .error
            .unwrap_or_else(|| "The daemon returned no status.".to_string());
        ui::fail(error, None, ui::EXIT_GENERAL)
    })
}

/// The live view: the playing track's lyrics, following along, and moving
/// on with the queue.
fn cmd_show_live(interval: f64) {
    let ui = ui::current();
    if ui.json {
        ui::fail(
            "--live draws to the terminal and has no JSON form.",
            Some("follow playback as JSON with: wave now --json --watch"),
            ui::EXIT_USAGE,
        );
    }
    // Check for the daemon before taking over the terminal.
    status_or_exit();

    let interval = crate::cli::now::clamp_interval(interval);
    let mut follower = Follower::new(true);
    let mut live = Live::start();
    loop {
        let status = status_or_exit();
        let lyrics = follower.get(status.path.as_deref());
        live.draw(&live_frame(ui, &status, &lyrics, live.reads_keys()));

        // Word highlighting needs more than the dashboard's default pace.
        let wait = match lyrics {
            Lyrics::Ready(ref sheet) if sheet.kind != LyricsKind::Plain => interval.min(0.2),
            _ => interval,
        };
        if live.wait(Duration::from_secs_f64(wait)) == Some(Key::Quit) {
            return;
        }
    }
}

fn live_frame(ui: &ui::Ui, status: &PlaybackStatus, lyrics: &Lyrics, keys: bool) -> String {
    let title = status
        .title
        .as_deref()
        .or(status.file.as_deref())
        .unwrap_or("Nothing playing");
    let duration = status.duration_seconds.unwrap_or(0.0);
    let progress = if duration > 0.0 {
        status.position_seconds / duration
    } else {
        0.0
    };
    let timing = format!("{} / {}", clock(status.position_seconds), clock(duration));
    let room = ui.width.saturating_sub(4 + timing.len());
    let mut heading = title.to_string();
    if let Some(artist) = status.artist.as_deref().filter(|a| !a.is_empty()) {
        heading = format!("{heading} {} {artist}", ui.glyphs.dot);
    }

    let mut out = format!(
        "\n  {}  {}\n\n",
        ui.heading(&ui::truncate(&heading, room, ui.glyphs.ellipsis)),
        ui.dim(&timing)
    );
    // Header, footer and the blank lines between them take seven rows.
    let rows = term::terminal_height()
        .saturating_sub(7)
        .clamp(3, LIVE_ROWS);
    for line in lyrics_view::panel(ui, lyrics, status.position_seconds, progress, rows) {
        out.push_str(&line);
        out.push('\n');
    }
    let exit = if keys { "Q to exit" } else { "Ctrl-C to exit" };
    out.push_str(&format!("\n  {}\n", ui.dim(exit)));
    out
}
