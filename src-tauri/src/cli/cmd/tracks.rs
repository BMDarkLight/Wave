// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Library tracks: list, import, inspect, search, add, remove, reset.

use std::path::Path;

use crate::cli::{open_library, render, track_path_or_exit, ui, TracksCmd};
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
    let tracks = if let Some(pid) = &playlist_id {
        library.get_playlist_tracks(pid)
    } else {
        // Return all tracks from the library
        let conn = library.lock_connection().unwrap();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM {} ORDER BY t.artist, t.album, t.track_number",
                crate::library::TRACK_SELECT_COLUMNS,
                crate::library::TRACK_FROM
            ))
            .map_err(|e| format!("Failed to prepare query: {e}"))
            .unwrap();
        let cover_root = library.cover_root().to_path_buf();
        let rows = stmt
            .query_map([], |row| crate::library::row_to_track(row, &cover_root))
            .map_err(|e| format!("Failed to query tracks: {e}"))
            .unwrap();
        let tracks: Vec<Track> = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read tracks: {e}"))
            .unwrap();
        Ok(tracks)
    };
    match tracks {
        Ok(tracks) => {
            if tracks.is_empty() {
                println!("No tracks found.");
                return;
            }
            let ui = ui::current();
            println!("{}\n", ui.heading(&format!("{} tracks", tracks.len())));
            print!("{}", render::track_table(ui::current(), &tracks));
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_tracks_import(paths: Vec<String>) {
    let library = open_library();
    let mut total = 0;
    for path in &paths {
        let p = Path::new(path);
        if p.is_dir() {
            match library.index_directory(None, None, path.clone()) {
                Ok(tracks) => {
                    println!("Imported {} track(s) from {}", tracks.len(), path);
                    total += tracks.len();
                }
                Err(e) => ui::report(format!("Error importing directory {path}: {e}"), None),
            }
        } else if p.is_file() {
            match library.add_track_to_default_playlist(path.clone()) {
                Ok(track) => {
                    println!("Imported: {} — {}", track.artist, track.title);
                    total += 1;
                }
                Err(e) => ui::report(format!("Error importing {path}: {e}"), None),
            }
        } else {
            ui::report(format!("Path not found: {path}"), None);
        }
    }
    if total > 0 {
        println!("Successfully imported {total} track(s).");
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
        Some(t) => print!("{}", render::metadata_block(ui::current(), &t)),
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
            if tracks.is_empty() {
                println!("No tracks matching \"{query}\".");
                return;
            }
            let ui = ui::current();
            println!(
                "{}\n",
                ui.heading(&format!("{} tracks matching \"{query}\"", tracks.len()))
            );
            print!("{}", render::track_table(ui::current(), &tracks));
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_tracks_add(track_id: String, playlist_id: Option<String>) {
    let library = open_library();
    let path = track_path_or_exit(&library, &track_id);
    let result = if let Some(pid) = playlist_id {
        library.add_track_to_playlist(&pid, path)
    } else {
        library.add_track_to_default_playlist(path)
    };
    match result {
        Ok(track) => println!("Added: {} — {}", track.artist, track.title),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_tracks_remove(track_id: String, playlist_id: Option<String>) {
    let library = open_library();
    let path = track_path_or_exit(&library, &track_id);
    let result = if let Some(pid) = playlist_id {
        library
            .remove_track_from_playlist_by_path(&pid, &path)
            .map(|_| "Track removed from playlist.")
    } else {
        library
            .remove_track_from_library(&path)
            .map(|_| "Track removed from library.")
    };
    match result {
        Ok(msg) => println!("{msg}"),
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
            println!(
                "Library reset: removed {tracks} track(s) and deleted {playlists} playlist(s)."
            );
            println!("Library and Favorites were kept (empty).");
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
