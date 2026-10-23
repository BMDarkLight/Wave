// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Audius: full-length streaming from an artist-run catalog.
//!
//! The public API needs no key, only an `app_name` so Audius can tell apps
//! apart. Search results carry a `stream.url`, but it is signed with a
//! timestamp and goes stale, so a result instead points at the stable
//! `/stream` endpoint, which redirects to a freshly signed MP3 each time.
//!
//! Tracks gated behind a purchase or a follow are dropped, since their
//! stream endpoint answers 404 for an anonymous caller.

use reqwest::blocking::Client;

use super::deezer::urlencoding_encode;
use super::{
    duration_field, get_json, id_field, str_field, SourceError, SourceProvider, SourceTrack,
};

pub(crate) const API_BASE: &str = "https://api.audius.co/v1";
pub(crate) const APP_NAME: &str = "Wave";

pub struct Audius;

impl SourceProvider for Audius {
    fn id(&self) -> &'static str {
        "audius"
    }

    fn display_name(&self) -> &'static str {
        "Audius"
    }

    fn search(
        &self,
        client: &Client,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>, SourceError> {
        let url = format!(
            "{API_BASE}/tracks/search?query={}&limit={}&app_name={APP_NAME}",
            urlencoding_encode(query),
            limit.clamp(1, 50)
        );
        let value = get_json(client, &url)?;

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

/// False for tracks an anonymous listener can't stream: their stream
/// endpoint answers 404.
pub(crate) fn is_streamable(item: &serde_json::Value) -> bool {
    let flag = |key: &str| item.get(key).and_then(|v| v.as_bool());
    flag("is_streamable") != Some(false) && flag("is_stream_gated") != Some(true)
}

/// The stable endpoint that redirects to a freshly signed copy of the audio.
pub(crate) fn stream_url(id: &str) -> String {
    format!("{API_BASE}/tracks/{id}/stream?app_name={APP_NAME}")
}

fn parse_track(item: &serde_json::Value) -> Option<SourceTrack> {
    let flag = |key: &str| item.get(key).and_then(|v| v.as_bool());
    if !is_streamable(item) {
        return None;
    }
    let id = id_field(item, "id")?;

    let artwork_url = item.get("artwork").and_then(|art| {
        str_field(art, "480x480")
            .or_else(|| str_field(art, "1000x1000"))
            .or_else(|| str_field(art, "150x150"))
    });

    // Audius stores the licence the artist picked. Only show it when it says
    // something beyond the default.
    let attribution = str_field(item, "license")
        .filter(|license| !license.eq_ignore_ascii_case("All rights reserved"));

    Some(SourceTrack {
        provider: "audius".to_string(),
        audio_url: Some(stream_url(&id)),
        id,
        title: str_field(item, "title").unwrap_or_else(|| "Unknown title".to_string()),
        artist: item
            .get("user")
            .and_then(|u| str_field(u, "name").or_else(|| str_field(u, "handle")))
            .unwrap_or_else(|| "Unknown artist".to_string()),
        album: None,
        duration_seconds: duration_field(item, "duration"),
        artwork_url,
        is_full_length: true,
        // The artist decides. What gets kept is the streamed MP3, which is
        // the same recording the download button on Audius would hand over.
        downloadable: flag("is_downloadable") == Some(true)
            && flag("is_download_gated") != Some(true),
        attribution,
        already_in_library: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "data": [
        {
          "id": "YmJWK",
          "title": "Late Night Mix",
          "duration": 1690,
          "is_streamable": true,
          "is_stream_gated": false,
          "is_downloadable": false,
          "is_download_gated": false,
          "license": "All rights reserved",
          "artwork": {
            "150x150": "https://creatornode.audius.co/content/Qm1/150x150.jpg",
            "480x480": "https://creatornode.audius.co/content/Qm1/480x480.jpg"
          },
          "user": { "name": "omgkirby", "handle": "omgkirby" }
        },
        {
          "id": "Rpm6j",
          "title": "Free Download",
          "duration": 200,
          "is_streamable": true,
          "is_stream_gated": false,
          "is_downloadable": true,
          "is_download_gated": false,
          "license": "Creative Commons Attribution CC BY",
          "artwork": null,
          "user": { "handle": "someone" }
        },
        {
          "id": "95wro",
          "title": "Not Streamable",
          "is_streamable": false,
          "user": { "name": "X" }
        },
        {
          "id": "g4t3d",
          "title": "Pay To Listen",
          "is_streamable": true,
          "is_stream_gated": true,
          "user": { "name": "Y" }
        }
      ]
    }"#;

    fn parsed() -> Vec<SourceTrack> {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        value["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(parse_track)
            .collect()
    }

    #[test]
    fn drops_tracks_an_anonymous_listener_cannot_stream() {
        let ids: Vec<_> = parsed().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, ["YmJWK", "Rpm6j"]);
    }

    #[test]
    fn points_at_the_stable_stream_endpoint() {
        let track = &parsed()[0];
        assert_eq!(
            track.audio_url.as_deref(),
            Some("https://api.audius.co/v1/tracks/YmJWK/stream?app_name=Wave")
        );
        // No extension in that URL; the cache file must still be an mp3.
        assert_eq!(track.extension(), "mp3");
        assert!(track.is_full_length);
        assert_eq!(track.duration_seconds, Some(1690.0));
        assert_eq!(track.artist, "omgkirby");
        assert!(track
            .artwork_url
            .as_deref()
            .unwrap()
            .ends_with("480x480.jpg"));
    }

    #[test]
    fn download_and_licence_follow_the_artist_choice() {
        let tracks = parsed();
        assert!(!tracks[0].downloadable);
        assert_eq!(tracks[0].attribution, None);

        assert!(tracks[1].downloadable);
        assert_eq!(
            tracks[1].attribution.as_deref(),
            Some("Creative Commons Attribution CC BY")
        );
        // Falls back to the handle when the display name is missing.
        assert_eq!(tracks[1].artist, "someone");
        assert_eq!(tracks[1].artwork_url, None);
    }
}
