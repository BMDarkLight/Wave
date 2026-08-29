//! Deezer — discovery and metadata.
//!
//! Deezer's public search API is free and unauthenticated, and its metadata
//! and artwork are the best of the three providers. Its audio is *not*:
//! `preview` is a 30-second MP3, and that is deliberately all this provider
//! ever returns. Full-length Deezer audio is encrypted and requires account
//! token decryption, which is out of scope permanently — hence
//! `is_full_length: false` and `downloadable: false` on every result, which is
//! what makes the UI show a "30s preview" badge and hide the download button.

use reqwest::blocking::Client;

use super::{
    duration_field, get_json, id_field, str_field, SourceError, SourceProvider, SourceTrack,
};

/// Every Deezer preview is exactly this long.
const PREVIEW_SECONDS: f64 = 30.0;

pub struct Deezer;

impl SourceProvider for Deezer {
    fn id(&self) -> &'static str {
        "deezer"
    }

    fn display_name(&self) -> &'static str {
        "Deezer"
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

        Ok(items.iter().filter_map(parse_track).collect())
    }

    fn resolve_audio(&self, _client: &Client, track: &SourceTrack) -> Result<String, SourceError> {
        track.audio_url.clone().ok_or(SourceError::NoAudio)
    }
}

fn parse_track(item: &serde_json::Value) -> Option<SourceTrack> {
    let id = id_field(item, "id")?;
    let title = str_field(item, "title")
        .or_else(|| str_field(item, "title_short"))
        .unwrap_or_else(|| "Unknown title".to_string());
    let artist = item
        .get("artist")
        .and_then(|a| str_field(a, "name"))
        .unwrap_or_else(|| "Unknown artist".to_string());
    let album = item.get("album").and_then(|a| str_field(a, "title"));
    let artwork_url = item.get("album").and_then(|a| {
        str_field(a, "cover_medium")
            .or_else(|| str_field(a, "cover_big"))
            .or_else(|| str_field(a, "cover"))
    });

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
        let tracks: Vec<_> = items.iter().filter_map(parse_track).collect();

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
        let track = parse_track(&value["data"][0]).unwrap();
        // The API's 224s is the full track; playing it would desync the seek bar.
        assert_eq!(track.duration_seconds, Some(30.0));
        assert!(!track.is_full_length);
        assert!(!track.downloadable);
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
            .filter_map(parse_track)
            .collect();
        assert!(tracks.is_empty());
    }
}
