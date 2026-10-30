// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The songs this Wave shares with paired Waves, read from the library.
//!
//! Only files Wave can read itself are shared: local files, downloads, and
//! `content://` documents on Android. Tracks of someone else's server are
//! left out.

use crate::library::{Library, LIBRARY_TRACK_FROM, TRACK_SELECT_COLUMNS};
use crate::metadata::Track;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

impl Library {
    /// Every track the library shows whose file Wave can open itself.
    pub fn shareable_tracks(&self) -> Result<Vec<Track>, String> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(&format!(
                "SELECT {TRACK_SELECT_COLUMNS} FROM {LIBRARY_TRACK_FROM}
                 WHERE t.path NOT LIKE 'wave-remote://%'"
            ))
            .map_err(|e| format!("Failed to list shared tracks: {e}"))?;
        let cover_root = self.cover_root().to_path_buf();
        let rows = statement
            .query_map([], |row| crate::library::row_to_track(row, &cover_root))
            .map_err(|e| format!("Failed to list shared tracks: {e}"))?;
        rows.collect::<Result<_, _>>()
            .map_err(|e| format!("Failed to read a shared track: {e}"))
    }

    /// Changes whenever the shared songs do: one added, removed or retagged.
    pub fn share_version(&self) -> Result<u32, String> {
        let connection = self.read_connection();
        let (count, indexed, modified): (i64, i64, i64) = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(indexed_at), 0), COALESCE(MAX(modified_at), 0)
                 FROM library_tracks WHERE path NOT LIKE 'wave-remote://%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| format!("Failed to read the library version: {e}"))?;
        let digest = Sha256::digest(format!("{count}:{indexed}:{modified}").as_bytes());
        Ok(u32::from_be_bytes([
            digest[0], digest[1], digest[2], digest[3],
        ]))
    }
}

fn fold(text: &str) -> String {
    text.trim().to_lowercase()
}

/// The artist an album is filed under: its album artist, else its artist.
fn album_artist(track: &Track) -> String {
    track
        .album_artist
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .unwrap_or(track.artist.trim())
        .to_string()
}

/// The shared songs at one moment, by album.
pub struct Snapshot {
    pub version: u32,
    tracks: Vec<Track>,
    track_index: HashMap<String, usize>,
}

impl Snapshot {
    pub fn build(version: u32, mut tracks: Vec<Track>) -> Self {
        tracks.sort_by_cached_key(|t| {
            (
                fold(&album_artist(t)),
                fold(&t.album),
                t.disc_number.unwrap_or(0),
                t.track_number.unwrap_or(0),
                fold(&t.title),
            )
        });
        let track_index = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.clone(), i))
            .collect();
        Self {
            version,
            tracks,
            track_index,
        }
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Every shared track, in library order.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// A shared track by its library id.
    pub fn track(&self, id: &str) -> Option<&Track> {
        self.track_index.get(id).map(|&i| &self.tracks[i])
    }

    /// A shared track by its path.
    pub fn track_at(&self, path: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.path == path)
    }
}

/// The MIME type a file of this format is served as.
pub fn mime_for(format: &str) -> &'static str {
    match format.to_ascii_lowercase().as_str() {
        "mp3" | "mp2" | "mp1" => "audio/mpeg",
        "flac" => "audio/flac",
        "m4a" | "m4b" | "m4p" | "mp4" | "alac" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" | "wave" => "audio/wav",
        "aiff" => "audio/aiff",
        "mka" | "mkv" => "audio/x-matroska",
        "weba" => "audio/webm",
        "caf" => "audio/x-caf",
        _ => UNKNOWN_MIME,
    }
}

/// The file extension a shared track's URL ends in.
///
/// Android stores a document's MIME subtype as its format ("MPEG",
/// "X-FLAC", "X-M4A"), so those are mapped to the extension they stand for;
/// failing that, the extension of the file's own name is used. Without
/// that, most of a phone's library had no type the other side could play.
pub fn extension_of(track: &Track) -> String {
    let format = track.format.trim().to_ascii_lowercase();
    let known = match format.as_str() {
        "mpeg" | "mpga" | "x-mpeg" | "mp3" => "mp3",
        "x-flac" | "flac" => "flac",
        "mp4" | "x-m4a" | "m4a" | "x-mp4" | "mp4a-latm" => "m4a",
        "aac" | "x-aac" | "aacp" => "aac",
        "x-wav" | "wav" | "wave" | "vnd.wave" => "wav",
        "x-aiff" | "aiff" | "aif" => "aiff",
        "x-ogg" | "ogg" | "oga" | "vorbis" => "ogg",
        "x-matroska" | "mka" => "mka",
        "webm" | "weba" => "weba",
        "x-caf" | "caf" => "caf",
        other => other,
    };
    if mime_for(known) != UNKNOWN_MIME {
        return known.to_string();
    }
    // `content://` paths end in an encoded document id, `.../Song.mp3`.
    let decoded = percent_encoding::percent_decode_str(&track.path).decode_utf8_lossy();
    let from_name = decoded
        .rsplit(['/', ':'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| mime_for(ext) != UNKNOWN_MIME);
    from_name.unwrap_or_else(|| {
        if known.is_empty() {
            "bin".to_string()
        } else {
            known.to_string()
        }
    })
}

const UNKNOWN_MIME: &str = "application/octet-stream";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_formats_are_served_as_audio() {
        let with = |format: &str, path: &str| {
            extension_of(&Track {
                format: format.into(),
                path: path.into(),
                ..Track::default()
            })
        };
        assert_eq!(with("MPEG", "content://x/a"), "mp3");
        assert_eq!(with("X-FLAC", "content://x/a"), "flac");
        assert_eq!(with("X-M4A", "content://x/a"), "m4a");
        assert_eq!(with("AAC", "content://x/a"), "aac");
        assert_eq!(mime_for("aac"), "audio/aac");
        // Nothing to go on in the format: the document's own name decides.
        assert_eq!(
            with(
                "AUDIO",
                "content://com.android.externalstorage.documents/tree/primary%3AMusic/document/primary%3AMusic%2FSong.MP3"
            ),
            "mp3"
        );
        assert_eq!(with("flac", "/music/a.flac"), "flac");
    }

    #[test]
    fn songs_are_found_by_id_and_listed_by_album() {
        let track = |id: &str, album: &str, number: i32| Track {
            id: id.into(),
            path: format!("/music/{id}.mp3"),
            title: id.into(),
            artist: "Band".into(),
            album: album.into(),
            track_number: Some(number),
            ..Track::default()
        };
        let snapshot = Snapshot::build(
            7,
            vec![track("c", "B", 1), track("b", "A", 2), track("a", "A", 1)],
        );
        let order: Vec<_> = snapshot.tracks().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(order, ["a", "b", "c"]);
        assert_eq!(snapshot.track("b").unwrap().album, "A");
        assert_eq!(snapshot.track_at("/music/c.mp3").unwrap().id, "c");
        assert!(snapshot.track("zzz").is_none());
    }
}
