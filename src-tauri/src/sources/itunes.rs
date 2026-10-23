// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! iTunes: Apple's public search API.
//!
//! Like Deezer, it is free, needs no key, and has excellent metadata and
//! artwork, but the only audio it exposes is `previewUrl`: a 30-second AAC
//! clip in an `.m4a` container. Every result is therefore a preview that is
//! never offered for download.

use reqwest::blocking::Client;
use std::time::Duration;

use super::deezer::urlencoding_encode;
use super::{get_json, id_field, str_field, SourceError, SourceProvider, SourceTrack};

/// iTunes previews are 30 seconds, give or take a few hundred milliseconds.
const PREVIEW_SECONDS: f64 = 30.0;

/// The size search results link to is 100px. The artwork server renders any
/// size from the same path, so ask for one that survives the album view.
const ARTWORK_SIZE: &str = "600x600bb";

pub struct ITunes;

impl SourceProvider for ITunes {
    fn id(&self) -> &'static str {
        "itunes"
    }

    fn display_name(&self) -> &'static str {
        "iTunes"
    }

    /// Apple documents a limit of roughly 20 calls a minute.
    fn rate_limit(&self) -> Duration {
        Duration::from_secs(3)
    }

    fn search(
        &self,
        client: &Client,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>, SourceError> {
        let url = format!(
            "https://itunes.apple.com/search?term={}&country=US&media=music&entity=song&limit={}",
            urlencoding_encode(query),
            limit.clamp(1, 50)
        );
        let value = get_json(client, &url)?;

        // A bad request comes back as HTTP 200 with only `errorMessage`.
        if let Some(message) = str_field(&value, "errorMessage") {
            return Err(SourceError::Network(message));
        }

        let items = value
            .get("results")
            .and_then(|r| r.as_array())
            .ok_or_else(|| SourceError::Parse("missing `results` array".into()))?;

        Ok(items.iter().filter_map(parse_track).collect())
    }

    fn resolve_audio(&self, _client: &Client, track: &SourceTrack) -> Result<String, SourceError> {
        track.audio_url.clone().ok_or(SourceError::NoAudio)
    }
}

fn parse_track(item: &serde_json::Value) -> Option<SourceTrack> {
    // Song searches can still return music videos; only songs have audio we
    // can decode.
    if str_field(item, "kind").as_deref() != Some("song") {
        return None;
    }
    let id = id_field(item, "trackId")?;
    let audio_url = str_field(item, "previewUrl")?;

    let artwork_url = str_field(item, "artworkUrl100")
        .or_else(|| str_field(item, "artworkUrl60"))
        .map(|url| upscale_artwork(&url));

    Some(SourceTrack {
        provider: "itunes".to_string(),
        id,
        title: str_field(item, "trackName").unwrap_or_else(|| "Unknown title".to_string()),
        artist: str_field(item, "artistName").unwrap_or_else(|| "Unknown artist".to_string()),
        album: str_field(item, "collectionName"),
        // `trackTimeMillis` is the full track; the clip is what actually plays.
        duration_seconds: Some(PREVIEW_SECONDS),
        artwork_url,
        audio_url: Some(audio_url),
        is_full_length: false,
        downloadable: false,
        attribution: Some("30-second preview from iTunes".to_string()),
        already_in_library: None,
    })
}

/// `.../100x100bb.jpg` becomes `.../600x600bb.jpg`. A URL that does not end
/// in a size segment is left alone rather than guessed at.
fn upscale_artwork(url: &str) -> String {
    let Some((base, file)) = url.rsplit_once('/') else {
        return url.to_string();
    };
    let Some((size, ext)) = file.split_once('.') else {
        return url.to_string();
    };
    let is_size = size
        .strip_suffix("bb")
        .and_then(|dims| dims.split_once('x'))
        .is_some_and(|(w, h)| {
            !w.is_empty()
                && !h.is_empty()
                && w.chars().all(|c| c.is_ascii_digit())
                && h.chars().all(|c| c.is_ascii_digit())
        });
    if is_size {
        format!("{base}/{ARTWORK_SIZE}.{ext}")
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "resultCount": 3,
      "results": [
        {
          "wrapperType": "track",
          "kind": "song",
          "trackId": 617154366,
          "artistName": "Daft Punk, Pharrell Williams & Nile Rodgers",
          "collectionName": "Random Access Memories",
          "trackName": "Get Lucky",
          "previewUrl": "https://audio-ssl.itunes.apple.com/itunes-assets/AudioPreview211/v4/57/a5/85/mzaf_6906185026678279401.plus.aac.p.m4a",
          "artworkUrl100": "https://is1-ssl.mzstatic.com/image/thumb/Music115/v4/e8/43/5f/886443919266.jpg/100x100bb.jpg",
          "trackTimeMillis": 369629
        },
        {
          "wrapperType": "track",
          "kind": "music-video",
          "trackId": 1,
          "artistName": "Someone",
          "trackName": "A Video",
          "previewUrl": "https://video-ssl.itunes.apple.com/a.m4v"
        },
        {
          "wrapperType": "track",
          "kind": "song",
          "trackId": 2,
          "artistName": "Someone",
          "trackName": "No Preview"
        }
      ]
    }"#;

    #[test]
    fn keeps_only_songs_with_a_preview() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let tracks: Vec<_> = value["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(parse_track)
            .collect();
        assert_eq!(tracks.len(), 1);

        let track = &tracks[0];
        assert_eq!(track.id, "617154366");
        assert_eq!(track.title, "Get Lucky");
        assert_eq!(track.album.as_deref(), Some("Random Access Memories"));
        assert_eq!(track.extension(), "m4a");
    }

    #[test]
    fn reports_the_preview_length_and_never_offers_a_download() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let track = parse_track(&value["results"][0]).unwrap();
        assert_eq!(track.duration_seconds, Some(30.0));
        assert!(!track.is_full_length);
        assert!(!track.downloadable);
    }

    #[test]
    fn asks_for_larger_artwork() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let track = parse_track(&value["results"][0]).unwrap();
        assert_eq!(
            track.artwork_url.as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/Music115/v4/e8/43/5f/886443919266.jpg/600x600bb.jpg")
        );
        // Anything that is not a size segment stays as the API sent it.
        assert_eq!(
            upscale_artwork("https://example.com/cover.jpg"),
            "https://example.com/cover.jpg"
        );
    }
}
