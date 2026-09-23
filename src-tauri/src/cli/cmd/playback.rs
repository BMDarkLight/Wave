// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Transport control for the background playback daemon.

use serde_json::json;

use crate::cli::{daemon_cmd, ui, PlaybackCmd};
use crate::playback_daemon::{daemon_request, daemon_request_if_running, DaemonRequest};

pub fn run(cmd: PlaybackCmd) {
    match cmd {
        PlaybackCmd::Start { id } => cmd_playback_start(id),
        PlaybackCmd::Pause => daemon_cmd(DaemonRequest::Pause),
        PlaybackCmd::Resume => daemon_cmd(DaemonRequest::Resume),
        PlaybackCmd::Stop => daemon_cmd(DaemonRequest::Stop),
        PlaybackCmd::Next => daemon_cmd(DaemonRequest::Next),
        PlaybackCmd::Previous => daemon_cmd(DaemonRequest::Previous),
        PlaybackCmd::Seek { seconds } => daemon_cmd(DaemonRequest::Seek { seconds }),
        // One renderer for both, so the two views cannot drift apart.
        PlaybackCmd::Status => crate::cli::now::run(true, 0.5),
        PlaybackCmd::Shutdown => cmd_playback_shutdown(),
    }
}

pub(crate) fn cmd_playback_start(id: String) {
    match daemon_request(DaemonRequest::Start { id }) {
        Ok(resp) if resp.ok => {
            let msg = resp
                .message
                .unwrap_or_else(|| "Playback started.".to_string());
            ui::done(msg, json!({ "status": resp.status }));
            println!(
                "  {}",
                ui::current().dim("playing in the background; watch it with: wave now")
            );
        }
        Ok(resp) => {
            ui::fail(
                resp.error
                    .unwrap_or_else(|| "Failed to start playback".to_string()),
                None,
                ui::EXIT_GENERAL,
            );
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

fn cmd_playback_shutdown() {
    match daemon_request_if_running(DaemonRequest::Shutdown) {
        Ok(Some(resp)) if resp.ok => {
            let msg = resp
                .message
                .unwrap_or_else(|| "Playback daemon stopped.".to_string());
            ui::done(msg, json!({}));
        }
        Ok(Some(resp)) => {
            ui::fail(
                resp.error
                    .unwrap_or_else(|| "Failed to shut down daemon".to_string()),
                None,
                ui::EXIT_GENERAL,
            );
        }
        // Shutting down something that is already down is not a failure.
        Ok(None) => ui::done("Playback daemon is not running.", json!({})),
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

/// What `wave play` decided the user meant.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PlaylistMatch {
    /// Exactly one playlist, by full id, exact name, or id prefix.
    One(String),
    /// More than one playlist fits; the names, for the error.
    Many(Vec<String>),
    None,
}

/// Find the playlist `query` names. An exact name wins outright (names are
/// what people remember); otherwise the query is tried as an id prefix.
pub(crate) fn match_playlist(
    playlists: &[crate::library::PlaylistInfo],
    query: &str,
) -> PlaylistMatch {
    let by_name: Vec<_> = playlists
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(query))
        .collect();
    if let [only] = by_name.as_slice() {
        return PlaylistMatch::One(only.id.clone());
    }
    const MIN_PREFIX: usize = 4;
    if query.len() < MIN_PREFIX {
        return PlaylistMatch::None;
    }
    let by_id: Vec<_> = playlists
        .iter()
        .filter(|p| p.id.starts_with(&query.to_ascii_lowercase()))
        .collect();
    match by_id.as_slice() {
        [] => PlaylistMatch::None,
        [only] => PlaylistMatch::One(only.id.clone()),
        many => PlaylistMatch::Many(many.iter().map(|p| p.name.clone()).collect()),
    }
}

/// `wave play <id>`: a track (id, id prefix, or path) or a playlist (id, id
/// prefix, or name), resolved here so short ids work for both before the
/// daemon sees them.
pub fn play(id: String) {
    let library = crate::cli::open_library();
    let playlists = library.list_playlists(None).unwrap_or_default();
    match match_playlist(&playlists, &id) {
        PlaylistMatch::One(playlist_id) => return cmd_playback_start(playlist_id),
        PlaylistMatch::Many(names) => ui::fail(
            format!("\"{id}\" matches more than one playlist."),
            Some(&format!("narrow it, or use a name: {}", names.join(", "))),
            ui::EXIT_NOT_FOUND,
        ),
        PlaylistMatch::None => {}
    }
    let path = crate::cli::track_path_or_exit(&library, &id);
    cmd_playback_start(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::PlaylistInfo;

    fn playlist(id: &str, name: &str) -> PlaylistInfo {
        PlaylistInfo {
            id: id.into(),
            profile_id: "default".into(),
            name: name.into(),
            track_count: 0,
            created_at: 0,
            updated_at: 0,
            sync_folder: None,
        }
    }

    fn sample() -> Vec<PlaylistInfo> {
        vec![
            playlist("dc66db6b-b7ed-42bc-9af2-696edbd7598e", "Library"),
            playlist("dc6600aa-0000-0000-0000-000000000000", "Road Trip"),
            playlist("e18e9105-35aa-48a0-b0c0-177d7487a374", "Favorites"),
        ]
    }

    #[test]
    fn an_exact_name_wins_regardless_of_case() {
        assert_eq!(
            match_playlist(&sample(), "favorites"),
            PlaylistMatch::One("e18e9105-35aa-48a0-b0c0-177d7487a374".into())
        );
    }

    #[test]
    fn a_unique_id_prefix_or_full_id_matches() {
        let full = "dc66db6b-b7ed-42bc-9af2-696edbd7598e";
        assert_eq!(
            match_playlist(&sample(), "dc66db6b"),
            PlaylistMatch::One(full.into())
        );
        assert_eq!(
            match_playlist(&sample(), full),
            PlaylistMatch::One(full.into())
        );
        assert_eq!(
            match_playlist(&sample(), "DC66DB"),
            PlaylistMatch::One(full.into())
        );
    }

    #[test]
    fn a_shared_prefix_is_ambiguous() {
        assert_eq!(
            match_playlist(&sample(), "dc66"),
            PlaylistMatch::Many(vec!["Library".into(), "Road Trip".into()])
        );
    }

    #[test]
    fn short_or_unknown_queries_fall_through_to_tracks() {
        // Anything that is not a playlist is left for track resolution.
        assert_eq!(match_playlist(&sample(), "dc6"), PlaylistMatch::None);
        assert_eq!(match_playlist(&sample(), "3ac687ae"), PlaylistMatch::None);
        assert_eq!(
            match_playlist(&sample(), "/music/a.flac"),
            PlaylistMatch::None
        );
    }
}
