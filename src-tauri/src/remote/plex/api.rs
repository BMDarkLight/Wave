// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Plex library payloads and their mapping to `RemoteTrackMeta`.

use crate::remote::backend::{RemoteError, RemoteTrackMeta};
use serde::Deserialize;

#[derive(Deserialize, Default)]
struct Envelope {
    #[serde(rename = "MediaContainer", default)]
    container: Container,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Container {
    total_size: Option<usize>,
    #[serde(rename = "Metadata")]
    metadata: Vec<RawTrack>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawTrack {
    rating_key: String,
    title: Option<String>,
    original_title: Option<String>,
    grandparent_title: Option<String>,
    parent_title: Option<String>,
    index: Option<i32>,
    parent_index: Option<i32>,
    year: Option<i32>,
    parent_year: Option<i32>,
    duration: Option<f64>,
    parent_thumb: Option<String>,
    thumb: Option<String>,
    updated_at: Option<i64>,
    #[serde(rename = "Genre")]
    genre: Vec<RawTag>,
    #[serde(rename = "Media")]
    media: Vec<RawMedia>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawTag {
    tag: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawMedia {
    container: Option<String>,
    #[serde(rename = "Part")]
    part: Vec<RawPart>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPart {
    key: Option<String>,
    size: Option<u64>,
    container: Option<String>,
    file: Option<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

fn file_extension(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next()?;
    let (_, ext) = name.rsplit_once('.')?;
    non_empty(Some(ext.to_string()))
}

fn map_track(raw: RawTrack) -> Option<RemoteTrackMeta> {
    let media = raw.media.into_iter().next();
    let media_container = media.as_ref().and_then(|m| non_empty(m.container.clone()));
    let part = media?.part.into_iter().next()?;
    let part_key = non_empty(part.key)?;

    let extension = non_empty(part.container)
        .or_else(|| part.file.as_deref().and_then(file_extension))
        .or(media_container)
        .unwrap_or_default()
        .to_lowercase();

    let album_artist = non_empty(raw.grandparent_title);
    let artist = non_empty(raw.original_title)
        .or_else(|| album_artist.clone())
        .unwrap_or_else(|| "Unknown Artist".to_string());

    Some(RemoteTrackMeta {
        remote_id: raw.rating_key,
        title: raw.title.unwrap_or_default(),
        artist,
        album_artist,
        album: non_empty(raw.parent_title).unwrap_or_else(|| "Unknown Album".to_string()),
        track_number: raw.index,
        disc_number: raw.parent_index,
        year: raw.year.or(raw.parent_year),
        genre: raw.genre.into_iter().next().and_then(|g| non_empty(g.tag)),
        duration_seconds: raw.duration.map(|ms| ms / 1000.0),
        size: part.size.unwrap_or(0),
        extension,
        part_key,
        art_ref: non_empty(raw.parent_thumb).or_else(|| non_empty(raw.thumb)),
        updated_at: raw.updated_at,
    })
}

pub struct ParsedPage {
    pub tracks: Vec<RemoteTrackMeta>,
    /// The section's total item count when the server reports it.
    pub total: Option<usize>,
    /// Items the server returned, including skipped ones, so paging advances
    /// by what was sent and not by what was kept.
    pub raw_count: usize,
}

/// Maps one `/library/sections/{key}/all?type=10` response. Tracks without a
/// playable part are dropped.
pub fn parse_page(json: &str) -> Result<ParsedPage, RemoteError> {
    let envelope: Envelope =
        serde_json::from_str(json).map_err(|e| RemoteError::Parse(e.to_string()))?;
    let container = envelope.container;
    let raw_count = container.metadata.len();
    Ok(ParsedPage {
        tracks: container
            .metadata
            .into_iter()
            .filter_map(map_track)
            .collect(),
        total: container.total_size,
        raw_count,
    })
}

#[cfg(test)]
fn parse_track_page(json: &str) -> Result<(Vec<RemoteTrackMeta>, Option<usize>), RemoteError> {
    parse_page(json).map(|p| (p.tracks, p.total))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"{
      "MediaContainer": {
        "size": 3,
        "totalSize": 1200,
        "offset": 0,
        "Metadata": [
          {
            "ratingKey": "101",
            "title": "Hello",
            "originalTitle": "Track Artist",
            "grandparentTitle": "Album Artist",
            "parentTitle": "First Album",
            "index": 3,
            "parentIndex": 1,
            "parentYear": 1999,
            "duration": 215000,
            "parentThumb": "/library/metadata/50/thumb/1700",
            "thumb": "/library/metadata/101/thumb/1700",
            "updatedAt": 1700000000,
            "Genre": [{ "tag": "Rock" }, { "tag": "Pop" }],
            "Media": [{
              "container": "flac",
              "Part": [{
                "key": "/library/parts/9/1/file.flac",
                "size": 31457280,
                "container": "FLAC",
                "file": "/music/a/Hello.flac"
              }]
            }]
          },
          {
            "ratingKey": "102",
            "title": "No Part",
            "grandparentTitle": "Album Artist",
            "duration": 1000
          },
          {
            "ratingKey": "103",
            "title": "Bare",
            "grandparentTitle": "Solo",
            "parentTitle": "",
            "year": 2005,
            "parentYear": 1990,
            "thumb": "/library/metadata/103/thumb/1",
            "Media": [{
              "container": "MP3",
              "Part": [{ "key": "/library/parts/10/2/file.m4a", "size": 5, "file": "/music/Bare.M4A" }]
            }]
          }
        ]
      }
    }"#;

    #[test]
    fn parses_track_page_and_maps_fields() {
        let (tracks, total) = parse_track_page(PAGE).unwrap();
        assert_eq!(total, Some(1200));
        assert_eq!(tracks.len(), 2);

        let t = &tracks[0];
        assert_eq!(t.remote_id, "101");
        assert_eq!(t.title, "Hello");
        assert_eq!(t.artist, "Track Artist");
        assert_eq!(t.album_artist.as_deref(), Some("Album Artist"));
        assert_eq!(t.album, "First Album");
        assert_eq!(t.track_number, Some(3));
        assert_eq!(t.disc_number, Some(1));
        assert_eq!(t.year, Some(1999));
        assert_eq!(t.genre.as_deref(), Some("Rock"));
        assert_eq!(t.duration_seconds, Some(215.0));
        assert_eq!(t.size, 31457280);
        assert_eq!(t.extension, "flac");
        assert_eq!(t.part_key, "/library/parts/9/1/file.flac");
        assert_eq!(
            t.art_ref.as_deref(),
            Some("/library/metadata/50/thumb/1700")
        );
        assert_eq!(t.updated_at, Some(1700000000));
    }

    #[test]
    fn falls_back_for_artist_album_year_art_and_extension() {
        let (tracks, _) = parse_track_page(PAGE).unwrap();
        let t = &tracks[1];
        assert_eq!(t.remote_id, "103");
        assert_eq!(t.artist, "Solo");
        assert_eq!(t.album, "Unknown Album");
        assert_eq!(t.year, Some(2005));
        assert_eq!(t.genre, None);
        assert_eq!(t.art_ref.as_deref(), Some("/library/metadata/103/thumb/1"));
        assert_eq!(t.extension, "m4a");
    }

    #[test]
    fn empty_container_is_an_empty_page() {
        let (tracks, total) = parse_track_page(r#"{"MediaContainer":{"size":0}}"#).unwrap();
        assert!(tracks.is_empty());
        assert_eq!(total, None);
    }

    #[test]
    fn garbage_is_a_parse_error() {
        assert!(matches!(
            parse_track_page("<html>nope</html>"),
            Err(crate::remote::backend::RemoteError::Parse(_))
        ));
    }
}
