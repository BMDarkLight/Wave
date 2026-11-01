// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Deezer: discovery and metadata.
//!
//! Deezer's public search API is free and unauthenticated, and its metadata
//! and artwork are as good as any provider's. Its audio is *not*:
//! `preview` is a 30-second MP3, and that is deliberately all this provider
//! ever returns. Full-length Deezer audio is encrypted and requires account
//! token decryption, which is out of scope permanently. Hence
//! `is_full_length: false` and `downloadable: false` on every result, which is
//! what makes the UI show a "30s preview" badge and hide the download button.

use reqwest::blocking::Client;

use super::{
    duration_field, get_json, id_field, str_field, SourceError, SourceProvider, SourceTrack,
};

/// Every Deezer preview is exactly this long.
const PREVIEW_SECONDS: f64 = 30.0;

pub struct Deezer {
    /// Play results in full through Online play rather than as previews.
    pub online_play: bool,
}

impl SourceProvider for Deezer {
    fn id(&self) -> &'static str {
        "deezer"
    }

    /// Under Online play its results are songs to play, wherever the audio
    /// comes from, so the section says so rather than naming the catalog.
    fn display_name(&self) -> &'static str {
        if self.online_play {
            "Online"
        } else {
            "Deezer"
        }
    }

    fn search(
        &self,
        client: &Client,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>, SourceError> {
        let url = format!(
            "https://api.deezer.com/search?q={}&limit={}",
            urlencoding_encode(query),
            limit.clamp(1, 50)
        );
        let value = get_json(client, &url)?;

        // Deezer reports errors with HTTP 200 and an `error` object.
        if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
            let message = str_field(error, "message").unwrap_or_else(|| "Deezer error".into());
            return Err(SourceError::Network(message));
        }

        let items = value
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| SourceError::Parse("missing `data` array".into()))?;

        Ok(items
            .iter()
            .filter_map(|item| parse_track(item, self.online_play))
            .collect())
    }

    /// A preview plays from its URL. A full-length result is found on YouTube
    /// now, when it is played, so a search never waits on YouTube.
    fn resolve_audio(&self, _client: &Client, track: &SourceTrack) -> Result<String, SourceError> {
        if let Some(url) = &track.audio_url {
            return Ok(url.clone());
        }
        use super::youtube::matching::Wanted;
        use super::youtube::router::{self, FindError};
        let router = router::global()
            .ok_or_else(|| SourceError::Network("Online play isn't ready".into()))?;
        let wanted = Wanted {
            title: &track.title,
            artist: &track.artist,
            duration_seconds: track.duration_seconds,
        };
        router.find(&wanted).map_err(|e| match e {
            FindError::Unreachable(message) => SourceError::Network(message),
            FindError::NoMatch => SourceError::NotFound,
        })
    }
}

/// Deezer's error object, which it sends with HTTP 200.
fn check(value: serde_json::Value) -> Result<serde_json::Value, SourceError> {
    match value.get("error").filter(|e| !e.is_null()) {
        Some(error) => Err(SourceError::Network(
            str_field(error, "message").unwrap_or_else(|| "Deezer error".into()),
        )),
        None => Ok(value),
    }
}

/// Deezer ids are numbers; anything else never reaches a URL.
fn checked_id(id: &str) -> Result<&str, SourceError> {
    if !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(id)
    } else {
        Err(SourceError::Parse(format!("not a Deezer id: {id}")))
    }
}

fn cover(value: &serde_json::Value) -> Option<String> {
    str_field(value, "cover_big")
        .or_else(|| str_field(value, "cover_xl"))
        .or_else(|| str_field(value, "cover_medium"))
}

fn year(value: &serde_json::Value) -> Option<String> {
    str_field(value, "release_date").map(|d| d.chars().take(4).collect())
}

/// One of an artist's albums, as listed on their page.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct AlbumSummary {
    pub id: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub year: Option<String>,
    /// `album`, `single`, `ep` or `compile`.
    pub record_type: Option<String>,
}

/// An artist's page, fetched from Deezer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ArtistPage {
    pub id: String,
    pub name: String,
    pub picture_url: Option<String>,
    pub fans: Option<u64>,
    pub top_tracks: Vec<SourceTrack>,
    pub albums: Vec<AlbumSummary>,
}

/// An album's page, fetched from Deezer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AlbumPage {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub artist_ref: Option<String>,
    pub cover_url: Option<String>,
    pub year: Option<String>,
    pub tracks: Vec<SourceTrack>,
}

impl Deezer {
    pub fn artist_page(&self, client: &Client, id: &str) -> Result<ArtistPage, SourceError> {
        let id = checked_id(id)?;
        let artist = check(get_json(
            client,
            &format!("https://api.deezer.com/artist/{id}"),
        )?)?;
        let top = check(get_json(
            client,
            &format!("https://api.deezer.com/artist/{id}/top?limit=10"),
        )?)?;
        let albums = check(get_json(
            client,
            &format!("https://api.deezer.com/artist/{id}/albums?limit=100"),
        )?)?;
        Ok(parse_artist_page(
            id,
            &artist,
            &top,
            &albums,
            self.online_play,
        ))
    }

    /// The artist and album ids of one track, for links from a song that is
    /// playing.
    pub fn track_refs(
        &self,
        client: &Client,
        id: &str,
    ) -> Result<(Option<String>, Option<String>), SourceError> {
        let id = checked_id(id)?;
        let track = check(get_json(
            client,
            &format!("https://api.deezer.com/track/{id}"),
        )?)?;
        Ok((
            track.get("artist").and_then(|a| id_field(a, "id")),
            track.get("album").and_then(|a| id_field(a, "id")),
        ))
    }

    pub fn album_page(&self, client: &Client, id: &str) -> Result<AlbumPage, SourceError> {
        let id = checked_id(id)?;
        let album = check(get_json(
            client,
            &format!("https://api.deezer.com/album/{id}"),
        )?)?;
        Ok(parse_album_page(id, &album, self.online_play))
    }
}

fn parse_artist_page(
    id: &str,
    artist: &serde_json::Value,
    top: &serde_json::Value,
    albums: &serde_json::Value,
    online_play: bool,
) -> ArtistPage {
    let list = |value: &serde_json::Value| -> Vec<serde_json::Value> {
        value
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default()
    };
    ArtistPage {
        id: id.to_string(),
        name: str_field(artist, "name").unwrap_or_else(|| "Unknown artist".into()),
        picture_url: str_field(artist, "picture_big").or_else(|| str_field(artist, "picture_xl")),
        fans: artist.get("nb_fan").and_then(|n| n.as_u64()),
        top_tracks: list(top)
            .iter()
            .filter_map(|t| parse_track(t, online_play))
            .collect(),
        albums: list(albums)
            .iter()
            .filter_map(|a| {
                Some(AlbumSummary {
                    id: id_field(a, "id")?,
                    title: str_field(a, "title")?,
                    cover_url: cover(a),
                    year: year(a),
                    record_type: str_field(a, "record_type"),
                })
            })
            .collect(),
    }
}

fn parse_album_page(id: &str, album: &serde_json::Value, online_play: bool) -> AlbumPage {
    let title = str_field(album, "title").unwrap_or_else(|| "Unknown album".into());
    let cover_url = cover(album);
    let artist = album.get("artist");
    // An album's own track list leaves the album out of each track.
    let tracks = album
        .get("tracks")
        .and_then(|t| t.get("data"))
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
        .filter_map(|t| parse_track(t, online_play))
        .map(|mut track| {
            track.album = Some(title.clone());
            track.album_ref = Some(id.to_string());
            if track.artwork_url.is_none() {
                track.artwork_url = cover_url.clone();
            }
            track
        })
        .collect();
    AlbumPage {
        id: id.to_string(),
        artist: artist
            .and_then(|a| str_field(a, "name"))
            .unwrap_or_else(|| "Unknown artist".into()),
        artist_ref: artist.and_then(|a| id_field(a, "id")),
        cover_url,
        year: year(album),
        title,
        tracks,
    }
}

fn parse_track(item: &serde_json::Value, online_play: bool) -> Option<SourceTrack> {
    let id = id_field(item, "id")?;
    let title = str_field(item, "title")
        .or_else(|| str_field(item, "title_short"))
        .unwrap_or_else(|| "Unknown title".to_string());
    let artist = item
        .get("artist")
        .and_then(|a| str_field(a, "name"))
        .unwrap_or_else(|| "Unknown artist".to_string());
    let album = item.get("album").and_then(|a| str_field(a, "title"));
    let artist_ref = item.get("artist").and_then(|a| id_field(a, "id"));
    let album_ref = item.get("album").and_then(|a| id_field(a, "id"));
    let artwork_url = item.get("album").and_then(|a| {
        // 500px: the kept cover is shown full size in Now Playing, and the
        // 250px "medium" one went soft there.
        str_field(a, "cover_big")
            .or_else(|| str_field(a, "cover_xl"))
            .or_else(|| str_field(a, "cover_medium"))
            .or_else(|| str_field(a, "cover"))
    });

    if online_play {
        return Some(SourceTrack {
            provider: "deezer".to_string(),
            id,
            title,
            artist,
            album,
            duration_seconds: duration_field(item, "duration"),
            artwork_url,
            // Found on YouTube when played.
            audio_url: None,
            is_full_length: true,
            // Kept by its stream, the way Online play saves played songs.
            downloadable: true,
            attribution: None,
            already_in_library: None,
            artist_ref,
            album_ref,
        });
    }

    // A result with no preview is unplayable for us; drop it rather than
    // offering a row that fails on click.
    let audio_url = str_field(item, "preview")?;

    Some(SourceTrack {
        provider: "deezer".to_string(),
        id,
        title,
        artist,
        album,
        // `duration` is the full track length. Verified against the live API:
        // a 230s track ships a 30s preview. Reporting the full length would
        // make the seek bar lie about a track that stops a quarter of the way in.
        duration_seconds: duration_field(item, "duration").map(|_| PREVIEW_SECONDS),
        artwork_url,
        audio_url: Some(audio_url),
        is_full_length: false,
        downloadable: false,
        attribution: Some("30-second preview — Deezer".to_string()),
        already_in_library: None,
        artist_ref,
        album_ref,
    })
}

/// Minimal percent-encoding for query strings. Avoids pulling in a crate for
/// the one thing we need.
pub(crate) fn urlencoding_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "data": [
        {
          "id": 3135556,
          "title": "Harder Better Faster Stronger",
          "duration": 224,
          "preview": "https://cdns-preview.dzcdn.net/stream/abc.mp3",
          "artist": { "name": "Daft Punk" },
          "album": { "title": "Discovery", "cover_medium": "https://api.deezer.com/album/302127/image" }
        },
        { "id": 999, "title": "No Preview", "artist": { "name": "X" } }
      ]
    }"#;

    #[test]
    fn parses_results_and_drops_unplayable_ones() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let items = value["data"].as_array().unwrap();
        let tracks: Vec<_> = items
            .iter()
            .filter_map(|item| parse_track(item, false))
            .collect();

        // The entry without a preview is unplayable and must not be offered.
        assert_eq!(tracks.len(), 1);
        let track = &tracks[0];
        assert_eq!(track.id, "3135556");
        assert_eq!(track.title, "Harder Better Faster Stronger");
        assert_eq!(track.artist, "Daft Punk");
        assert_eq!(track.album.as_deref(), Some("Discovery"));
        assert!(track.artwork_url.is_some());
    }

    #[test]
    fn preview_length_is_reported_not_full_length() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let track = parse_track(&value["data"][0], false).unwrap();
        // The API's 224s is the full track; playing it would desync the seek bar.
        assert_eq!(track.duration_seconds, Some(30.0));
        assert!(!track.is_full_length);
        assert!(!track.downloadable);
    }

    #[test]
    fn online_play_offers_every_result_in_full() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let items = value["data"].as_array().unwrap();
        let tracks: Vec<_> = items.iter().filter_map(|i| parse_track(i, true)).collect();
        // No preview is needed: the song comes from YouTube.
        assert_eq!(tracks.len(), 2);
        let track = &tracks[0];
        assert_eq!(track.duration_seconds, Some(224.0), "the real length");
        assert!(track.is_full_length);
        assert!(track.audio_url.is_none(), "found when played");
        assert!(track.downloadable, "saved from its stream");
    }

    #[test]
    fn reads_an_artist_page() {
        let artist = serde_json::json!({"id": 27, "name": "Daft Punk", "picture_big": "https://e/p.jpg", "nb_fan": 4500000});
        let top = serde_json::json!({"data": [{
            "id": 67238735, "title": "Get Lucky", "duration": 248,
            "preview": "https://cdns-preview.dzcdn.net/p.mp3",
            "artist": {"id": 27, "name": "Daft Punk"},
            "album": {"id": 6575789, "title": "Random Access Memories", "cover_big": "https://e/c.jpg"}
        }]});
        let albums = serde_json::json!({"data": [
            {"id": 6575789, "title": "Random Access Memories", "cover_big": "https://e/c.jpg", "release_date": "2013-05-17", "record_type": "album"},
            {"title": "no id"}
        ]});
        let page = parse_artist_page("27", &artist, &top, &albums, true);
        assert_eq!(page.name, "Daft Punk");
        assert_eq!(page.fans, Some(4_500_000));
        assert_eq!(page.top_tracks.len(), 1);
        assert_eq!(page.top_tracks[0].artist_ref.as_deref(), Some("27"));
        assert_eq!(page.top_tracks[0].album_ref.as_deref(), Some("6575789"));
        assert!(page.top_tracks[0].is_full_length);
        assert_eq!(page.albums.len(), 1);
        assert_eq!(page.albums[0].year.as_deref(), Some("2013"));
    }

    #[test]
    fn an_album_page_fills_its_tracks_in() {
        let album = serde_json::json!({
            "id": 6575789, "title": "Random Access Memories", "cover_big": "https://e/c.jpg",
            "release_date": "2013-05-17", "artist": {"id": 27, "name": "Daft Punk"},
            "tracks": {"data": [{
                "id": 67238735, "title": "Get Lucky", "duration": 369,
                "preview": "https://cdns-preview.dzcdn.net/p.mp3",
                "artist": {"id": 27, "name": "Daft Punk"}
            }]}
        });
        let page = parse_album_page("6575789", &album, false);
        assert_eq!(page.artist_ref.as_deref(), Some("27"));
        let track = &page.tracks[0];
        assert_eq!(track.album.as_deref(), Some("Random Access Memories"));
        assert_eq!(track.album_ref.as_deref(), Some("6575789"));
        assert_eq!(track.artwork_url.as_deref(), Some("https://e/c.jpg"));
        assert!(!track.is_full_length, "previews without Online play");
    }

    #[test]
    fn only_numeric_ids_reach_a_url() {
        assert!(checked_id("27").is_ok());
        assert!(checked_id("../27").is_err());
        assert!(checked_id("").is_err());
    }

    #[test]
    fn encodes_query_safely() {
        assert_eq!(urlencoding_encode("daft punk"), "daft%20punk");
        assert_eq!(urlencoding_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(urlencoding_encode("Sigur Rós"), "Sigur%20R%C3%B3s");
    }

    #[test]
    fn empty_payload_yields_no_tracks() {
        let value: serde_json::Value = serde_json::from_str(r#"{"data":[]}"#).unwrap();
        let tracks: Vec<_> = value["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| parse_track(item, false))
            .collect();
        assert!(tracks.is_empty());
    }
}
