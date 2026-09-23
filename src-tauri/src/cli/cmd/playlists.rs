// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Playlists: list, import and export, edit, and folder sync.

use std::path::Path;

use crate::cli::{open_library, render, track_path_or_exit, ui, PlaylistsCmd};

pub fn run(cmd: PlaylistsCmd) {
    match cmd {
        PlaylistsCmd::List => cmd_playlists_list(),
        PlaylistsCmd::Import { file, name } => cmd_playlists_import(file, name),
        PlaylistsCmd::Export { id, format, output } => cmd_playlists_export(id, format, output),
        PlaylistsCmd::Info { id } => cmd_playlists_info(id),
        PlaylistsCmd::Query { query } => cmd_playlists_query(query),
        PlaylistsCmd::Create { name } => cmd_playlists_create(name),
        PlaylistsCmd::Delete { id } => cmd_playlists_delete(id),
        PlaylistsCmd::Rename { id, name } => cmd_playlists_rename(id, name),
        PlaylistsCmd::Clear { id } => cmd_playlists_clear(id),
        PlaylistsCmd::AddTrack { id, track_id } => cmd_playlists_add_track(id, track_id),
        PlaylistsCmd::RemoveTrack { id, track_id } => cmd_playlists_remove_track(id, track_id),
        PlaylistsCmd::Sync { id } => cmd_playlists_sync(id),
    }
}

fn cmd_playlists_list() {
    let library = open_library();
    match library.list_playlists(None) {
        Ok(playlists) => {
            if playlists.is_empty() {
                println!("No playlists found.");
                return;
            }
            let ui = ui::current();
            println!(
                "{}
",
                ui.heading(&format!("{} playlists", playlists.len()))
            );
            print!("{}", render::playlist_table(ui, &playlists));
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_import(file: String, name: Option<String>) {
    let library = open_library();
    if let Err(e) = crate::path_validation::validate_playlist_import_path(&file) {
        ui::fail(e, None, ui::EXIT_GENERAL);
    }
    let ext = Path::new(&file)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let result = match ext.as_str() {
        "json" => library.import_playlist_json(&file, name.as_deref()),
        "m3u" | "m3u8" => library.import_playlist_m3u(&file, name.as_deref()),
        _ => ui::fail(
            format!("Unsupported playlist format: .{ext}"),
            Some("use .m3u, .m3u8, or .json"),
            ui::EXIT_GENERAL,
        ),
    };
    match result {
        Ok((id, tracks)) => {
            let name = library
                .get_playlist_info(&id)
                .ok()
                .flatten()
                .map(|info| info.name)
                .unwrap_or_else(|| "Unknown".to_string());
            println!(
                "Imported playlist \"{name}\" (ID: {id}) with {} track(s).",
                tracks.len()
            );
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_export(id: String, format: String, output: String) {
    let library = open_library();
    let expected_ext = match format.as_str() {
        "m3u" => "m3u",
        "json" => "json",
        _ => {
            ui::fail(
                format!("Unknown export format: {format}"),
                Some("use m3u or json"),
                ui::EXIT_GENERAL,
            );
        }
    };
    if let Err(e) = crate::path_validation::validate_safe_output_path(&output, expected_ext) {
        ui::fail(e, None, ui::EXIT_GENERAL);
    }
    match format.as_str() {
        "m3u" => library.export_playlist_m3u(&id, &output),
        "json" => library.export_playlist_json(&id, &output),
        _ => unreachable!(),
    }
    .unwrap_or_else(|e| {
        ui::fail(e, None, ui::EXIT_GENERAL);
    });
    println!("Exported playlist {id} to {output}");
}

fn cmd_playlists_info(id: String) {
    let library = open_library();
    match library.get_playlist_info(&id) {
        Ok(Some(info)) => {
            // The details are still worth showing if the track list fails.
            let tracks = library.get_playlist_tracks(&id).unwrap_or_else(|e| {
                ui::report(format!("Error fetching tracks: {e}"), None);
                Vec::new()
            });
            print!("{}", render::playlist_info(ui::current(), &info, &tracks));
        }
        Ok(None) => {
            ui::fail(
                format!("Playlist not found: {id}"),
                Some("see them all with: wave playlists list"),
                ui::EXIT_NOT_FOUND,
            );
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_query(query: String) {
    let library = open_library();
    match library.search_playlists(&query) {
        Ok(playlists) => {
            if playlists.is_empty() {
                println!("No playlists matching \"{query}\".");
                return;
            }
            let ui = ui::current();
            println!(
                "{}
",
                ui.heading(&format!(
                    "{} playlists matching \"{query}\"",
                    playlists.len()
                ))
            );
            print!("{}", render::playlist_table(ui, &playlists));
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_create(name: String) {
    let library = open_library();
    match library.create_playlist(&name, None) {
        Ok(info) => println!("Created playlist \"{}\" (ID: {})", info.name, info.id),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_delete(id: String) {
    let library = open_library();
    match library.delete_playlist(&id) {
        Ok(()) => println!("Deleted playlist {id}."),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_rename(id: String, name: String) {
    let library = open_library();
    match library.rename_playlist(&id, &name) {
        Ok(()) => println!("Renamed playlist {id} to \"{name}\"."),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_clear(id: String) {
    let library = open_library();
    match library.clear_playlist(&id) {
        Ok(()) => println!("Cleared all tracks from playlist {id}."),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_add_track(id: String, track_id: String) {
    let library = open_library();
    let path = track_path_or_exit(&library, &track_id);
    match library.add_track_to_playlist(&id, path) {
        Ok(track) => println!("Added to playlist: {} — {}", track.artist, track.title),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_remove_track(id: String, track_id: String) {
    let library = open_library();
    let path = track_path_or_exit(&library, &track_id);
    match library.remove_track_from_playlist_by_path(&id, &path) {
        Ok(()) => println!("Removed track from playlist {id}."),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_sync(id: String) {
    use crate::metadata::is_supported_audio_file;
    use walkdir::WalkDir;

    let library = open_library();
    let info = match library.get_playlist_info(&id) {
        Ok(Some(info)) => info,
        Ok(None) => {
            ui::fail(
                format!("Playlist not found: {id}"),
                Some("see them all with: wave playlists list"),
                ui::EXIT_NOT_FOUND,
            );
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    };

    let Some(folder) = info.sync_folder.as_deref() else {
        ui::fail(
            format!("Playlist \"{}\" is not linked to a sync folder.", info.name),
            Some("link one in the app: Create playlist, then Sync with folder"),
            ui::EXIT_GENERAL,
        );
    };

    let dir_path = Path::new(folder);
    if !dir_path.is_dir() {
        ui::fail(
            format!("Sync folder is missing or not a directory: {folder}"),
            None,
            ui::EXIT_GENERAL,
        );
    }

    let paths: Vec<String> = WalkDir::new(dir_path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| is_supported_audio_file(e.path()))
        .filter_map(|e| e.path().to_str().map(str::to_string))
        .collect();

    println!(
        "Syncing \"{}\" with {} ({} audio file(s) found)…",
        info.name,
        folder,
        paths.len()
    );

    match library.sync_playlist_to_paths(&id, &paths) {
        Ok((added, removed)) => {
            println!("Done. Added {added}, removed {removed}.");
            if let Ok(Some(updated)) = library.get_playlist_info(&id) {
                println!("Playlist now has {} track(s).", updated.track_count);
            }
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}
