// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Reading and writing a track's tags and cover art.

use std::path::Path;

use serde_json::json;

use crate::cli::{json, open_library, render, track_path_or_exit, ui, MetadataCmd};
use crate::metadata::extract_track;
use crate::tag_edit::{Change, CoverEdit, TagEdit};

pub fn run(cmd: MetadataCmd) {
    match cmd {
        MetadataCmd::Get { track_id } => cmd_metadata_get(track_id),
        MetadataCmd::CoverExport { track_id, output } => {
            cmd_metadata_cover_export(track_id, output)
        }
        MetadataCmd::CoverSet { track_id, image } => cmd_metadata_cover_set(track_id, image),
        MetadataCmd::Set { track_id, fields } => cmd_metadata_set(track_id, fields.into()),
    }
}

fn cmd_metadata_set(track_id: String, edit: TagEdit) {
    let library = open_library();
    let path = track_path_or_exit(&library, &track_id);

    let edit = edit.resolve().unwrap_or_else(|e| {
        ui::fail_with(e);
    });
    if edit.is_empty() {
        ui::fail(
            "Nothing to change. Pass at least one field, such as --title.",
            None,
            ui::EXIT_USAGE,
        );
    }

    let indexed = matches!(library.get_track_details(&path), Ok(Some(_)));
    if let Err(e) = crate::tag_edit::write_to_file(Path::new(&path), &edit) {
        ui::fail_with(e);
    }

    if !indexed {
        ui::done(format!("Tags written to {path}"), json!({ "path": path }));
        println!(
            "  {}",
            ui::current().dim("the file is not in the library, so nothing was re-indexed")
        );
        return;
    }
    match library.update_track_tags(&path, &edit) {
        Ok(track) => {
            if ui::current().json {
                json::emit_ok(json!({ "message": "Tags written.", "track": track }));
            }
            ui::done("Tags written.", json!({}));
            println!();
            print!("{}", render::metadata_block(ui::current(), &track));
        }
        Err(e) => {
            ui::fail(
                format!("Tags were written, but the library entry could not be updated: {e}"),
                None,
                ui::EXIT_GENERAL,
            );
        }
    }
}

fn cmd_metadata_get(track_id: String) {
    let library = open_library();
    let path = track_path_or_exit(&library, &track_id);
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

fn cmd_metadata_cover_export(track_id: String, output: String) {
    let library = open_library();
    let path = track_path_or_exit(&library, &track_id);

    // The file's own cover comes first, at full size. The library only keeps
    // a thumbnail, as a file path rather than a data URL, so it is the
    // fallback for a cover that came from somewhere other than the file.
    let cover = crate::metadata::extract_embedded_cover(&path)
        .ok()
        .flatten()
        .or_else(|| library_cover(&library, &path));
    let Some((bytes, mime)) = cover else {
        ui::fail(
            "No cover art available for this track.",
            None,
            ui::EXIT_NOT_FOUND,
        );
    };
    std::fs::write(&output, &bytes).unwrap_or_else(|e| {
        ui::fail(
            format!("Failed to write cover art: {e}"),
            None,
            ui::EXIT_GENERAL,
        );
    });
    ui::done(
        format!("Cover art exported to {output}"),
        json!({ "output": output, "mime": mime, "bytes": bytes.len() }),
    );
}

/// The cover the library holds for `path`: a data URL on older rows, a path
/// to a stored thumbnail on current ones.
fn library_cover(library: &crate::library::Library, path: &str) -> Option<(Vec<u8>, String)> {
    let track = library
        .get_tracks_by_paths(&[path.to_string()])
        .ok()?
        .into_iter()
        .next()
        .flatten()?;
    let stored = track.cover_art_data_url?;
    if stored.starts_with("data:") {
        return crate::cover_art::decode_data_url(&stored).ok();
    }
    let bytes = std::fs::read(&stored).ok()?;
    let mime = track
        .cover_art_mime
        .unwrap_or_else(|| "image/jpeg".to_string());
    Some((bytes, mime))
}
fn cmd_metadata_cover_set(track_id: String, image: String) {
    let library = open_library();

    // Resolve track
    let path = track_path_or_exit(&library, &track_id);

    // Read and re-encode through the same path the metadata editor uses, so a
    // cover set here goes into the file as well as the library.
    let edit = TagEdit {
        cover: Some(CoverEdit::Replace {
            path: image.clone(),
        }),
        ..Default::default()
    }
    .resolve()
    .unwrap_or_else(|e| {
        ui::fail_with(e);
    });
    let Some(Change::Set(jpeg)) = edit.cover.clone() else {
        ui::fail(
            format!("Failed to read image file {image}"),
            None,
            ui::EXIT_GENERAL,
        );
    };

    if let Err(e) = crate::tag_edit::write_to_file(Path::new(&path), &edit) {
        ui::fail_with(e);
    }

    // Look up the track ID in the database
    let track_id_uuid = library
        .get_tracks_by_paths(std::slice::from_ref(&path))
        .ok()
        .and_then(|v| v.into_iter().next().flatten())
        .map(|t| t.id)
        .unwrap_or_else(|| {
            library
                .add_track_to_default_playlist(path.clone())
                .unwrap_or_else(|e| {
                    ui::fail(
                        format!("Failed to add track to library: {e}"),
                        None,
                        ui::EXIT_GENERAL,
                    );
                })
                .id
        });

    library
        .set_track_cover(&track_id_uuid, &jpeg)
        .unwrap_or_else(|e| {
            ui::fail(
                format!("Failed to set cover art: {e}"),
                None,
                ui::EXIT_GENERAL,
            );
        });

    ui::done(
        format!(
            "Cover art set for track {}.",
            render::short_id(&track_id_uuid)
        ),
        json!({ "id": track_id_uuid }),
    );
}
