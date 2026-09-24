// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Playlists: list, import and export, edit, and folder sync.

use std::path::Path;

use serde_json::json;

use crate::cli::{
    json, open_library, playlist_or_exit, render, track_path_or_exit, ui, PlaylistsCmd,
};

pub fn run(cmd: PlaylistsCmd) {
    match cmd {
        PlaylistsCmd::List => cmd_playlists_list(),
        PlaylistsCmd::Import { file, name } => cmd_playlists_import(file, name),
        PlaylistsCmd::Export { id, target } => cmd_playlists_export(id, target),
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
            json::maybe_emit(&playlists);
            if playlists.is_empty() {
                println!("No playlists found.");
                return;
            }
            let ui = ui::current();
            println!(
                "{}\n",
                ui.heading(&ui::count(playlists.len(), "playlist", "playlists"))
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
            ui::done(
                format!(
                    "Imported playlist \"{name}\" with {}.",
                    ui::count(tracks.len(), "track", "tracks")
                ),
                json!({ "id": id, "name": name, "tracks": tracks.len() }),
            );
            println!("  {}", ui::current().dim(&format!("id {id}")));
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

/// The export format and output file from `export`'s trailing arguments:
/// either just the file, whose extension picks the format, or the older
/// explicit format followed by the file.
fn export_target(target: &[String]) -> Result<(&'static str, &str), String> {
    let (format, output) = match target {
        [output] => (None, output.as_str()),
        [format, output] => (Some(format.to_ascii_lowercase()), output.as_str()),
        _ => return Err("Give the output file, for example: mix.m3u".to_string()),
    };
    let ext = Path::new(output)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let format = match format.as_deref().unwrap_or(ext.as_str()) {
        "m3u" | "m3u8" => "m3u",
        "json" => "json",
        "" => {
            return Err(format!(
                "Can't tell the format of {output} without an extension."
            ))
        }
        other => return Err(format!("Unknown export format: {other}")),
    };
    Ok((format, output))
}

fn cmd_playlists_export(query: String, target: Vec<String>) {
    let library = open_library();
    let (format, output) = export_target(&target).unwrap_or_else(|e| {
        ui::fail(
            e,
            Some("use a .m3u, .m3u8, or .json file"),
            ui::EXIT_GENERAL,
        )
    });
    let info = playlist_or_exit(&library, &query);
    let ext = Path::new(output)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    // M3U8 is M3U in UTF-8, which is what the M3U writer produces anyway.
    let expected_ext = if format == "m3u" && ext == "m3u8" {
        "m3u8"
    } else {
        format
    };
    if let Err(e) = crate::path_validation::validate_safe_output_path(output, expected_ext) {
        ui::fail(e, None, ui::EXIT_GENERAL);
    }
    match format {
        "m3u" => library.export_playlist_m3u(&info.id, output),
        _ => library.export_playlist_json(&info.id, output),
    }
    .unwrap_or_else(|e| {
        ui::fail(e, None, ui::EXIT_GENERAL);
    });
    ui::done(
        format!(
            "Exported \"{}\" ({}) to {output}",
            info.name,
            ui::count(info.track_count as usize, "track", "tracks")
        ),
        json!({ "id": info.id, "output": output, "format": format }),
    );
}

fn cmd_playlists_info(query: String) {
    let library = open_library();
    let info = playlist_or_exit(&library, &query);
    // The details are still worth showing if the track list fails.
    let tracks = library.get_playlist_tracks(&info.id).unwrap_or_else(|e| {
        ui::report(format!("Error fetching tracks: {e}"), None);
        Vec::new()
    });
    json::maybe_emit(&json!({ "playlist": info, "tracks": tracks }));
    print!("{}", render::playlist_info(ui::current(), &info, &tracks));
}

fn cmd_playlists_query(query: String) {
    let library = open_library();
    match library.search_playlists(&query) {
        Ok(playlists) => {
            json::maybe_emit(&playlists);
            if playlists.is_empty() {
                println!("No playlists matching \"{query}\".");
                return;
            }
            let ui = ui::current();
            println!(
                "{}\n",
                ui.heading(&format!(
                    "{} matching \"{query}\"",
                    ui::count(playlists.len(), "playlist", "playlists")
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
        Ok(info) => {
            ui::done(
                format!("Created playlist \"{}\"", info.name),
                json!({ "playlist": info }),
            );
            println!("  {}", ui::current().dim(&format!("id {}", info.id)));
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_delete(query: String) {
    let library = open_library();
    let info = playlist_or_exit(&library, &query);
    match library.delete_playlist(&info.id) {
        Ok(()) => ui::done(
            format!("Deleted playlist \"{}\".", info.name),
            json!({ "id": info.id }),
        ),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_rename(query: String, name: String) {
    let library = open_library();
    let info = playlist_or_exit(&library, &query);
    match library.rename_playlist(&info.id, &name) {
        Ok(()) => ui::done(
            format!("Renamed playlist \"{}\" to \"{name}\".", info.name),
            json!({ "id": info.id, "name": name }),
        ),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_clear(query: String) {
    let library = open_library();
    let info = playlist_or_exit(&library, &query);
    match library.clear_playlist(&info.id) {
        Ok(()) => ui::done(
            format!(
                "Cleared playlist \"{}\" ({} removed).",
                info.name,
                ui::count(info.track_count as usize, "track", "tracks")
            ),
            json!({ "id": info.id }),
        ),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_add_track(query: String, track_id: String) {
    let library = open_library();
    let info = playlist_or_exit(&library, &query);
    let path = track_path_or_exit(&library, &track_id);
    match library.add_track_to_playlist(&info.id, path) {
        Ok(track) => ui::done(
            format!(
                "Added {} by {} to \"{}\".",
                track.title, track.artist, info.name
            ),
            json!({ "id": info.id, "track": track }),
        ),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_remove_track(query: String, track_id: String) {
    let library = open_library();
    let info = playlist_or_exit(&library, &query);
    let path = track_path_or_exit(&library, &track_id);
    match library.remove_track_from_playlist_by_path(&info.id, &path) {
        Ok(()) => ui::done(
            format!("Removed the track from \"{}\".", info.name),
            json!({ "id": info.id }),
        ),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playlists_sync(query: String) {
    use crate::metadata::is_supported_audio_file;
    use walkdir::WalkDir;

    let library = open_library();
    let info = playlist_or_exit(&library, &query);
    let id = info.id.clone();

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

    if !ui::current().json {
        println!(
            "Syncing \"{}\" with {} ({} audio files found)",
            info.name,
            folder,
            paths.len()
        );
    }

    match library.sync_playlist_to_paths(&id, &paths) {
        Ok((added, removed)) => {
            let count = library
                .get_playlist_info(&id)
                .ok()
                .flatten()
                .map(|updated| updated.track_count);
            let mut message = format!("Synced: {added} added, {removed} removed");
            if let Some(count) = count {
                message.push_str(&format!(
                    ", {} now",
                    ui::count(count as usize, "track", "tracks")
                ));
            }
            ui::done(
                format!("{message}."),
                json!({ "id": id, "added": added, "removed": removed, "tracks": count }),
            );
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_export_format_follows_the_file_extension() {
        assert_eq!(export_target(&args(&["mix.m3u"])), Ok(("m3u", "mix.m3u")));
        assert_eq!(export_target(&args(&["mix.M3U8"])), Ok(("m3u", "mix.M3U8")));
        assert_eq!(
            export_target(&args(&["mix.json"])),
            Ok(("json", "mix.json"))
        );
    }

    #[test]
    fn the_older_explicit_format_form_still_works() {
        assert_eq!(
            export_target(&args(&["json", "out.json"])),
            Ok(("json", "out.json"))
        );
        assert_eq!(
            export_target(&args(&["M3U", "out.m3u"])),
            Ok(("m3u", "out.m3u"))
        );
    }

    #[test]
    fn an_export_without_a_known_format_is_refused() {
        assert!(export_target(&args(&["mix"])).is_err());
        assert!(export_target(&args(&["mix.txt"])).is_err());
        assert!(export_target(&args(&["xml", "mix.xml"])).is_err());
        assert!(export_target(&args(&[])).is_err());
    }
}
