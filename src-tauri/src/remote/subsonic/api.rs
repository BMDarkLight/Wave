// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Subsonic JSON (`f=json`) responses and their mapping to library tracks.

use crate::remote::backend::{RemoteError, RemoteTrackMeta};
use serde::Deserialize;

/// Subsonic's error codes that mean the credentials are the problem.
const WRONG_CREDENTIALS: u32 = 40;
/// The server can't check a salted token (LDAP users, for one); the password
/// has to be sent instead.
pub const TOKEN_UNSUPPORTED: u32 = 41;
const NOT_AUTHORIZED: u32 = 50;
const NOT_FOUND: u32 = 70;

/// A failed call: the server's error code, or a response that wasn't one.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiFailure {
    Code(u32, String),
    Malformed(String),
}

impl From<ApiFailure> for RemoteError {
    fn from(failure: ApiFailure) -> Self {
        match failure {
            ApiFailure::Code(WRONG_CREDENTIALS | TOKEN_UNSUPPORTED | NOT_AUTHORIZED, _) => {
                RemoteError::Unauthorized
            }
            ApiFailure::Code(NOT_FOUND, _) => RemoteError::Http(404),
            ApiFailure::Code(_, message) | ApiFailure::Malformed(message) => {
                RemoteError::Parse(message)
            }
        }
    }
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Song {
    pub id: String,
    pub title: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    /// OpenSubsonic's album artist line.
    pub display_album_artist: Option<String>,
    pub track: Option<i32>,
    pub disc_number: Option<i32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub cover_art: Option<String>,
    pub size: Option<u64>,
    pub content_type: Option<String>,
    pub suffix: Option<String>,
    pub duration: Option<f64>,
    pub is_video: Option<bool>,
}

#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct AlbumRef {
    pub id: String,
}

/// The fields Wave reads from any response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Envelope {
    pub server_type: Option<String>,
    pub search_songs: Vec<Song>,
    pub albums: Vec<AlbumRef>,
    pub album_songs: Vec<Song>,
    pub scan_count: Option<usize>,
}

#[derive(Deserialize)]
struct Outer {
    #[serde(rename = "subsonic-response")]
    response: Inner,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Inner {
    status: String,
    #[serde(rename = "type")]
    server_type: Option<String>,
    error: Option<ErrorBody>,
    search_result3: Option<SongList>,
    album_list2: Option<AlbumList>,
    album: Option<SongList>,
    scan_status: Option<ScanStatus>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ErrorBody {
    code: u32,
    message: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SongList {
    song: Vec<Song>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AlbumList {
    album: Vec<AlbumRef>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ScanStatus {
    count: Option<usize>,
}

/// Reads a response, turning `status: failed` into its error code.
pub fn parse_response(body: &str) -> Result<Envelope, ApiFailure> {
    let outer: Outer = serde_json::from_str(body)
        .map_err(|_| ApiFailure::Malformed("Not a Subsonic server response".to_string()))?;
    let inner = outer.response;
    if inner.status != "ok" {
        let error = inner.error.unwrap_or_default();
        return Err(ApiFailure::Code(error.code, error.message));
    }
    Ok(Envelope {
        server_type: inner.server_type.filter(|t| !t.trim().is_empty()),
        search_songs: inner.search_result3.unwrap_or_default().song,
        albums: inner.album_list2.unwrap_or_default().album,
        album_songs: inner.album.unwrap_or_default().song,
        scan_count: inner.scan_status.and_then(|s| s.count),
    })
}

/// A file extension for a song, from its suffix or else its content type.
fn extension(song: &Song) -> String {
    if let Some(suffix) = song.suffix.as_deref().filter(|s| !s.trim().is_empty()) {
        return suffix.trim().to_ascii_lowercase();
    }
    match song.content_type.as_deref().unwrap_or_default() {
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/ogg" | "audio/vorbis" => "ogg",
        "audio/opus" => "opus",
        "audio/mp4" | "audio/x-m4a" | "audio/aac" => "m4a",
        "audio/wav" | "audio/x-wav" => "wav",
        _ => "mp3",
    }
    .to_string()
}

fn text(value: &Option<String>, fallback: &str) -> String {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// A song as library metadata. Videos, and entries without an id, are left
/// out.
pub fn song_to_meta(song: &Song) -> Option<RemoteTrackMeta> {
    if song.id.trim().is_empty() || song.is_video == Some(true) {
        return None;
    }
    Some(RemoteTrackMeta {
        remote_id: song.id.clone(),
        title: text(&Some(song.title.clone()), "Untitled"),
        artist: text(&song.artist, "Unknown Artist"),
        album_artist: song
            .display_album_artist
            .clone()
            .filter(|a| !a.trim().is_empty()),
        album: text(&song.album, "Unknown Album"),
        track_number: song.track,
        disc_number: song.disc_number,
        year: song.year,
        genre: song.genre.clone().filter(|g| !g.trim().is_empty()),
        duration_seconds: song.duration,
        size: song.size.unwrap_or(0),
        extension: extension(song),
        // The id is all `stream` needs.
        part_key: song.id.clone(),
        art_ref: song.cover_art.clone().filter(|a| !a.is_empty()),
        updated_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = r#"{"subsonic-response":{"status":"ok","version":"1.16.1",
      "type":"navidrome","serverVersion":"0.53.3","openSubsonic":true,
      "searchResult3":{"song":[
        {"id":"s1","title":"Song One","album":"Album","artist":"Band",
         "displayAlbumArtist":"Band and Friends","track":3,"discNumber":1,"year":2001,
         "genre":"Rock","coverArt":"al-9","size":1234567,"contentType":"audio/flac",
         "suffix":"flac","duration":215,"isVideo":false},
        {"id":"v1","title":"A Video","isVideo":true,"suffix":"mp4","size":10},
        {"id":"","title":"No Id"},
        {"id":"s2","title":"Song Two","contentType":"audio/mpeg","size":99,"duration":61}
      ]}}}"#;

    #[test]
    fn reads_the_server_type_and_search_songs() {
        let envelope = parse_response(SEARCH).unwrap();
        assert_eq!(envelope.server_type.as_deref(), Some("navidrome"));
        assert_eq!(envelope.search_songs.len(), 4);
    }

    #[test]
    fn maps_a_song_to_library_metadata() {
        let envelope = parse_response(SEARCH).unwrap();
        let meta = song_to_meta(&envelope.search_songs[0]).unwrap();
        assert_eq!(meta.remote_id, "s1");
        assert_eq!(meta.title, "Song One");
        assert_eq!(meta.artist, "Band");
        assert_eq!(meta.album_artist.as_deref(), Some("Band and Friends"));
        assert_eq!(meta.album, "Album");
        assert_eq!(meta.track_number, Some(3));
        assert_eq!(meta.disc_number, Some(1));
        assert_eq!(meta.year, Some(2001));
        assert_eq!(meta.genre.as_deref(), Some("Rock"));
        assert_eq!(meta.duration_seconds, Some(215.0));
        assert_eq!(meta.size, 1234567);
        assert_eq!(meta.extension, "flac");
        assert_eq!(meta.part_key, "s1");
        assert_eq!(meta.art_ref.as_deref(), Some("al-9"));
    }

    #[test]
    fn fills_gaps_and_guesses_the_extension_from_the_content_type() {
        let envelope = parse_response(SEARCH).unwrap();
        let meta = song_to_meta(&envelope.search_songs[3]).unwrap();
        assert_eq!(meta.artist, "Unknown Artist");
        assert_eq!(meta.album, "Unknown Album");
        assert_eq!(meta.extension, "mp3");
    }

    #[test]
    fn leaves_out_videos_and_songs_without_an_id() {
        let envelope = parse_response(SEARCH).unwrap();
        assert!(song_to_meta(&envelope.search_songs[1]).is_none());
        assert!(song_to_meta(&envelope.search_songs[2]).is_none());
    }

    #[test]
    fn a_failed_status_carries_its_error_code() {
        let failed = r#"{"subsonic-response":{"status":"failed","version":"1.16.1",
          "error":{"code":40,"message":"Wrong username or password"}}}"#;
        let failure = parse_response(failed).unwrap_err();
        assert_eq!(
            failure,
            ApiFailure::Code(40, "Wrong username or password".into())
        );
        assert_eq!(RemoteError::from(failure), RemoteError::Unauthorized);
        let unsupported = r#"{"subsonic-response":{"status":"failed",
          "error":{"code":41,"message":"Token authentication not supported"}}}"#;
        assert!(matches!(
            parse_response(unsupported),
            Err(ApiFailure::Code(41, _))
        ));
    }

    #[test]
    fn reads_album_lists_albums_and_the_scan_count() {
        let list = r#"{"subsonic-response":{"status":"ok",
          "albumList2":{"album":[{"id":"a1"},{"id":"a2"}]}}}"#;
        assert_eq!(parse_response(list).unwrap().albums.len(), 2);
        let album = r#"{"subsonic-response":{"status":"ok",
          "album":{"id":"a1","song":[{"id":"s9","title":"Nine","suffix":"ogg","size":5}]}}}"#;
        assert_eq!(parse_response(album).unwrap().album_songs[0].id, "s9");
        let scan = r#"{"subsonic-response":{"status":"ok",
          "scanStatus":{"scanning":false,"count":4821}}}"#;
        assert_eq!(parse_response(scan).unwrap().scan_count, Some(4821));
    }

    #[test]
    fn something_that_is_not_a_subsonic_response_is_malformed() {
        assert!(matches!(
            parse_response("<html>login</html>"),
            Err(ApiFailure::Malformed(_))
        ));
    }
}
