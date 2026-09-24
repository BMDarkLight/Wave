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
        PlaybackCmd::Start { id } => play(id),
        PlaybackCmd::Pause => daemon_cmd(DaemonRequest::Pause),
        PlaybackCmd::Resume => daemon_cmd(DaemonRequest::Resume),
        PlaybackCmd::Stop => daemon_cmd(DaemonRequest::Stop),
        PlaybackCmd::Next => daemon_cmd(DaemonRequest::Next),
        PlaybackCmd::Previous => daemon_cmd(DaemonRequest::Previous),
        PlaybackCmd::Seek { position } => cmd_playback_seek(&position),
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

/// Where a seek goes, before a step is added to the current position.
#[derive(Debug, PartialEq)]
enum SeekTo {
    At(f64),
    Step(f64),
}

/// Seconds ("90"), clock time ("1:30", "1:02:03"), or a signed step ("+10").
fn parse_seek(raw: &str) -> Result<SeekTo, String> {
    let bad = || format!("Can't read \"{raw}\" as a position; try 90, 1:30, or +10.");
    let text = raw.trim();
    let (sign, body) = match text.chars().next() {
        Some('+') => (Some(1.0), &text[1..]),
        Some('-') => (Some(-1.0), &text[1..]),
        _ => (None, text),
    };
    let parts: Vec<&str> = body.split(':').collect();
    if parts.len() > 3 || parts.iter().any(|p| p.trim().is_empty()) {
        return Err(bad());
    }
    let mut seconds = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let value: f64 = part.trim().parse().map_err(|_| bad())?;
        // Minutes and seconds past the first field must stay under 60.
        if !value.is_finite() || value < 0.0 || (i > 0 && value >= 60.0) {
            return Err(bad());
        }
        seconds = seconds * 60.0 + value;
    }
    Ok(match sign {
        Some(sign) => SeekTo::Step(sign * seconds),
        None => SeekTo::At(seconds),
    })
}

fn cmd_playback_seek(raw: &str) {
    let seconds = match parse_seek(raw) {
        Ok(SeekTo::At(seconds)) => seconds,
        Ok(SeekTo::Step(step)) => match daemon_request_if_running(DaemonRequest::Status) {
            Ok(Some(resp)) => {
                let status = resp.status.unwrap_or_else(|| {
                    ui::fail("The daemon returned no status.", None, ui::EXIT_GENERAL)
                });
                let end = status.duration_seconds.max(0.0);
                (status.position_seconds + step).clamp(0.0, end)
            }
            Ok(None) => ui::no_daemon(),
            Err(e) => ui::fail(e, None, ui::EXIT_GENERAL),
        },
        Err(e) => ui::fail(e, None, ui::EXIT_GENERAL),
    };
    daemon_cmd(DaemonRequest::Seek { seconds });
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
/// what people remember); otherwise the query is tried as an id prefix, then
/// as the start of a name.
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
    const MIN_ID_PREFIX: usize = 4;
    const MIN_NAME_PREFIX: usize = 2;
    let lower = query.to_lowercase();
    let pick = |hits: Vec<&crate::library::PlaylistInfo>| match hits.as_slice() {
        [] => PlaylistMatch::None,
        [only] => PlaylistMatch::One(only.id.clone()),
        many => PlaylistMatch::Many(many.iter().map(|p| p.name.clone()).collect()),
    };
    if query.len() >= MIN_ID_PREFIX {
        let by_id = pick(
            playlists
                .iter()
                .filter(|p| p.id.starts_with(&lower))
                .collect(),
        );
        if by_id != PlaylistMatch::None {
            return by_id;
        }
    }
    if query.chars().count() < MIN_NAME_PREFIX {
        return PlaylistMatch::None;
    }
    pick(
        playlists
            .iter()
            .filter(|p| p.name.to_lowercase().starts_with(&lower))
            .collect(),
    )
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

    #[test]
    fn a_seek_reads_seconds_or_clock_time() {
        assert_eq!(parse_seek("90"), Ok(SeekTo::At(90.0)));
        assert_eq!(parse_seek("1:30"), Ok(SeekTo::At(90.0)));
        assert_eq!(parse_seek("1:02:03"), Ok(SeekTo::At(3723.0)));
        assert_eq!(parse_seek("12.5"), Ok(SeekTo::At(12.5)));
    }

    #[test]
    fn a_signed_seek_is_a_step_from_here() {
        assert_eq!(parse_seek("+10"), Ok(SeekTo::Step(10.0)));
        assert_eq!(parse_seek("-1:00"), Ok(SeekTo::Step(-60.0)));
    }

    #[test]
    fn a_malformed_seek_is_refused() {
        for raw in ["", "abc", "1:", ":30", "1:75", "1:2:3:4", "+", "--5"] {
            assert!(parse_seek(raw).is_err(), "{raw:?} was accepted");
        }
    }

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
    fn the_start_of_a_name_is_enough_when_only_one_fits() {
        assert_eq!(
            match_playlist(&sample(), "road"),
            PlaylistMatch::One("dc6600aa-0000-0000-0000-000000000000".into())
        );
        assert_eq!(
            match_playlist(&sample(), "FAV"),
            PlaylistMatch::One("e18e9105-35aa-48a0-b0c0-177d7487a374".into())
        );
        let mut more = sample();
        more.push(playlist(
            "aaaa0000-0000-0000-0000-000000000000",
            "Road Songs",
        ));
        assert_eq!(
            match_playlist(&more, "road"),
            PlaylistMatch::Many(vec!["Road Trip".into(), "Road Songs".into()])
        );
        // A single letter says too little to pick a playlist over a track.
        assert_eq!(match_playlist(&sample(), "r"), PlaylistMatch::None);
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
