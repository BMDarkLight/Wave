// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Library tracks: list, import, inspect, search, add, remove, reset.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::Instant;

use serde_json::json;

use crate::cli::{json, open_library, playlist_or_exit, render, track_path_or_exit, ui, TracksCmd};
use crate::metadata::{extract_track, Track};

pub fn run(cmd: TracksCmd) {
    match cmd {
        TracksCmd::List { playlist_id } => cmd_tracks_list(playlist_id),
        TracksCmd::Import { paths } => cmd_tracks_import(paths),
        TracksCmd::Info { track_id } => cmd_tracks_info(track_id),
        TracksCmd::Query { query } => cmd_tracks_query(query),
        TracksCmd::Add {
            track_id,
            playlist_id,
        } => cmd_tracks_add(track_id, playlist_id),
        TracksCmd::Remove {
            track_id,
            playlist_id,
        } => cmd_tracks_remove(track_id, playlist_id),
        TracksCmd::Reset { yes } => cmd_tracks_reset(yes),
    }
}

fn cmd_tracks_list(playlist_id: Option<String>) {
    let library = open_library();
    let playlist = playlist_id.map(|query| playlist_or_exit(&library, &query));
    let tracks = match &playlist {
        Some(info) => library.get_playlist_tracks(&info.id),
        None => all_tracks(&library),
    };
    match tracks {
        Ok(tracks) => {
            json::maybe_emit(&tracks);
            if tracks.is_empty() {
                println!("No tracks found.");
                return;
            }
            let ui = ui::current();
            let heading = ui::count(tracks.len(), "track", "tracks");
            let heading = match &playlist {
                Some(info) => format!("{heading} in \"{}\"", info.name),
                None => heading,
            };
            println!("{}\n", ui.heading(&heading));
            print!("{}", render::track_table(ui, &tracks));
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn all_tracks(library: &crate::library::Library) -> Result<Vec<Track>, String> {
    let conn = library.lock_connection()?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM {} ORDER BY t.artist, t.album, t.track_number",
            crate::library::TRACK_SELECT_COLUMNS,
            crate::library::TRACK_FROM
        ))
        .map_err(|e| format!("Failed to prepare query: {e}"))?;
    let cover_root = library.cover_root().to_path_buf();
    let rows = stmt
        .query_map([], |row| crate::library::row_to_track(row, &cover_root))
        .map_err(|e| format!("Failed to query tracks: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read tracks: {e}"))
}

fn cmd_tracks_import(paths: Vec<String>) {
    let library = open_library();
    let ui = ui::current();
    // Progress goes to stderr, so `wave tracks import ~/Music > out.txt`
    // leaves the file holding only the summary.
    let live = !ui.json && std::io::stderr().is_terminal();
    let mut total = 0;
    let mut skipped: Vec<String> = Vec::new();

    for path in &paths {
        let p = Path::new(path);
        if p.is_dir() {
            let mut last_drawn: Option<Instant> = None;
            let mut frame = 0;
            let result = library.index_directory_with_progress(
                None,
                None,
                path.clone(),
                &mut |done, of, file| {
                    // Redrawing per file would spend more time on the terminal
                    // than on the import, so hold it to ten frames a second.
                    if !live || last_drawn.is_some_and(|t| t.elapsed().as_millis() < 100) {
                        return;
                    }
                    last_drawn = Some(Instant::now());
                    frame += 1;
                    let spinner = ui.glyphs.spinner[frame % ui.glyphs.spinner.len()];
                    let name = Path::new(file)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(file);
                    let line = format!("  {spinner} Importing  {done} / {of}  {name}");
                    let line = ui::truncate(&line, ui.width.saturating_sub(1), ui.glyphs.ellipsis);
                    eprint!("\r\x1b[2K{line}");
                    let _ = std::io::stderr().flush();
                },
            );
            if live {
                eprint!("\r\x1b[2K");
            }
            match result {
                Ok((tracks, failed)) => {
                    if !ui.json && paths.len() > 1 {
                        println!("  {} new from {path}", tracks.len());
                    }
                    total += tracks.len();
                    skipped.extend(failed);
                }
                Err(e) => ui::report(format!("Could not import {path}: {e}"), None),
            }
        } else if p.is_file() {
            match library.add_track_to_default_playlist(path.clone()) {
                Ok(track) => {
                    if !ui.json {
                        println!("  {} by {}", track.title, track.artist);
                    }
                    total += 1;
                }
                Err(e) => skipped.push(format!("{path}: {e}")),
            }
        } else {
            ui::report(format!("Path not found: {path}"), None);
        }
    }

    let mut message = format!("Imported {}", ui::count(total, "new track", "new tracks"));
    if !skipped.is_empty() {
        message.push_str(&format!(" {} {} skipped", ui.glyphs.dot, skipped.len()));
    }
    ui::done(
        format!("{message}."),
        json!({ "imported": total, "skipped": skipped }),
    );
    if !skipped.is_empty() {
        if ui.verbose {
            for reason in &skipped {
                println!("  {}", ui.dim(reason));
            }
        } else {
            println!("  {}", ui.dim("re-run with --verbose to see why"));
        }
    }
}

fn cmd_tracks_info(track_id: String) {
    let library = open_library();
    let path = track_path_or_exit(&library, &track_id);
    // Try library first, then extract directly
    let track = library
        .get_tracks_by_paths(std::slice::from_ref(&path))
        .ok()
        .and_then(|v| v.into_iter().next().flatten())
        .or_else(|| extract_track(None, &path).ok());

    match track {
        Some(t) => {
            json::maybe_emit(&t);
            print!("{}", render::metadata_block(ui::current(), &t))
        }
        None => {
            ui::fail(
                format!("Could not read track: {path}"),
                None,
                ui::EXIT_GENERAL,
            );
        }
    }
}

fn cmd_tracks_query(query: String) {
    let library = open_library();
    match library.search_tracks(&query) {
        Ok(tracks) => {
            json::maybe_emit(&tracks);
            if tracks.is_empty() {
                println!("No tracks matching \"{query}\".");
                return;
            }
            let ui = ui::current();
            println!(
                "{}\n",
                ui.heading(&format!(
                    "{} matching \"{query}\"",
                    ui::count(tracks.len(), "track", "tracks")
                ))
            );
            print!("{}", render::track_table(ui, &tracks));
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_tracks_add(track_id: String, playlist_id: Option<String>) {
    let library = open_library();
    let path = track_path_or_exit(&library, &track_id);
    let playlist = playlist_id.map(|query| playlist_or_exit(&library, &query));
    let result = match &playlist {
        Some(info) => library.add_track_to_playlist(&info.id, path),
        None => library.add_track_to_default_playlist(path),
    };
    let target = playlist.map_or_else(|| "Library".to_string(), |info| info.name);
    match result {
        Ok(track) => ui::done(
            format!("Added {} by {} to \"{target}\".", track.title, track.artist),
            json!({ "track": track }),
        ),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_tracks_remove(track_id: String, playlist_id: Option<String>) {
    let library = open_library();
    let path = track_path_or_exit(&library, &track_id);
    let playlist = playlist_id.map(|query| playlist_or_exit(&library, &query));
    let result = match &playlist {
        Some(info) => library
            .remove_track_from_playlist_by_path(&info.id, &path)
            .map(|_| format!("Track removed from \"{}\".", info.name)),
        None => library
            .remove_track_from_library(&path)
            .map(|_| "Track removed from the Library.".to_string()),
    };
    match result {
        Ok(msg) => ui::done(msg, json!({ "path": path })),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_tracks_reset(yes: bool) {
    if !yes {
        let confirmed = confirm_prompt(
            "This will delete ALL tracks and ALL playlists except Library and Favorites.\n\
             Type 'reset' to confirm: ",
            "reset",
        );
        if !confirmed {
            ui::fail("Aborted.", None, ui::EXIT_GENERAL);
        }
    }

    let library = open_library();
    match library.reset_library() {
        Ok((tracks, playlists)) => {
            ui::done(
                format!(
                    "Library reset: removed {} and {}.",
                    ui::count(tracks as usize, "track", "tracks"),
                    ui::count(playlists as usize, "playlist", "playlists")
                ),
                json!({ "tracks_removed": tracks, "playlists_deleted": playlists }),
            );
            println!(
                "  {}",
                ui::current().dim("Library and Favorites were kept, empty.")
            );
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn confirm_prompt(prompt: &str, expected: &str) -> bool {
    use std::io::{self, Write};
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(_) => line.trim().eq_ignore_ascii_case(expected),
        Err(_) => false,
    }
}
