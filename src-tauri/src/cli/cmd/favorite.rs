// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The Favorites playlist.

use serde_json::json;

use crate::cli::{json, open_library, render, track_path_or_exit, ui, FavoriteCmd};

pub fn run(cmd: FavoriteCmd) {
    let library = open_library();
    match cmd {
        FavoriteCmd::Add { track_id } => {
            let path = track_path_or_exit(&library, &track_id);
            match library.add_track_to_favorites(path) {
                Ok(track) => ui::done(
                    format!("Added to favorites: {} by {}", track.title, track.artist),
                    json!({ "track": track }),
                ),
                Err(e) => ui::fail(e, None, ui::EXIT_GENERAL),
            }
        }
        FavoriteCmd::Remove { track_id } => {
            let path = track_path_or_exit(&library, &track_id);
            match library.remove_track_from_favorites(&path) {
                Ok(()) => ui::done("Removed from favorites.", json!({ "path": path })),
                Err(e) => ui::fail(e, None, ui::EXIT_GENERAL),
            }
        }
        FavoriteCmd::List => match library.get_favorites() {
            Ok(tracks) => {
                json::maybe_emit(&tracks);
                if tracks.is_empty() {
                    println!("No favorites.");
                    return;
                }
                let ui = ui::current();
                println!("{}\n", ui.heading(&format!("{} favorites", tracks.len())));
                print!("{}", render::track_table(ui, &tracks));
            }
            Err(e) => ui::fail(e, None, ui::EXIT_GENERAL),
        },
        FavoriteCmd::Clear => match library.clear_favorites() {
            Ok(()) => ui::done("Favorites cleared.", json!({})),
            Err(e) => ui::fail(e, None, ui::EXIT_GENERAL),
        },
    }
}
