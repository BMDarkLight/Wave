// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Listening history rollups.

use crate::cli::{json, open_library, render, ui, StatsCmd};

pub fn run(cmd: StatsCmd) {
    let library = open_library();
    match cmd {
        StatsCmd::Summary { limit } => {
            let stats = library.get_listening_stats(limit).unwrap_or_else(|e| {
                ui::fail(
                    format!("Failed to load listening stats: {e}"),
                    None,
                    ui::EXIT_GENERAL,
                );
            });
            json::maybe_emit(&stats);
            print!("{}", render::listening_overview(ui::current(), &stats));
        }
        StatsCmd::Recent { limit } => {
            let tracks = library.get_recently_played(limit).unwrap_or_else(|e| {
                ui::fail(
                    format!("Failed to load recently played: {e}"),
                    None,
                    ui::EXIT_GENERAL,
                );
            });
            json::maybe_emit(&tracks);
            if tracks.is_empty() {
                println!("No recently played tracks yet.");
                return;
            }
            print!(
                "{}",
                render::track_rank_list(ui::current(), "Recently played", &tracks)
            );
        }
        StatsCmd::Most { limit } => {
            let tracks = library.get_most_played(limit).unwrap_or_else(|e| {
                ui::fail(
                    format!("Failed to load most played: {e}"),
                    None,
                    ui::EXIT_GENERAL,
                );
            });
            json::maybe_emit(&tracks);
            if tracks.is_empty() {
                println!("No listen history yet.");
                return;
            }
            print!(
                "{}",
                render::track_rank_list(ui::current(), "Most played", &tracks)
            );
        }
        StatsCmd::Artists { limit } => {
            let stats = library.get_listening_stats(limit).unwrap_or_else(|e| {
                ui::fail(
                    format!("Failed to load listening stats: {e}"),
                    None,
                    ui::EXIT_GENERAL,
                );
            });
            json::maybe_emit(&stats.top_artists);
            if stats.top_artists.is_empty() {
                println!("No artist listen history yet.");
                return;
            }
            print!(
                "{}",
                render::named_rank_list(ui::current(), "Top artists", &stats.top_artists)
            );
        }
        StatsCmd::Albums { limit } => {
            let stats = library.get_listening_stats(limit).unwrap_or_else(|e| {
                ui::fail(
                    format!("Failed to load listening stats: {e}"),
                    None,
                    ui::EXIT_GENERAL,
                );
            });
            json::maybe_emit(&stats.top_albums);
            if stats.top_albums.is_empty() {
                println!("No album listen history yet.");
                return;
            }
            print!(
                "{}",
                render::named_rank_list(ui::current(), "Top albums", &stats.top_albums)
            );
        }
        StatsCmd::Genres { limit } => {
            let stats = library.get_listening_stats(limit).unwrap_or_else(|e| {
                ui::fail(
                    format!("Failed to load listening stats: {e}"),
                    None,
                    ui::EXIT_GENERAL,
                );
            });
            json::maybe_emit(&stats.top_genres);
            if stats.top_genres.is_empty() {
                println!("No genre listen history yet.");
                return;
            }
            print!(
                "{}",
                render::named_rank_list(ui::current(), "Top genres", &stats.top_genres)
            );
        }
    }
}
