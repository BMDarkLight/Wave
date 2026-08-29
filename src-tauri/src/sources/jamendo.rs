//! Jamendo — full-length, legally downloadable Creative Commons audio.
//!
//! This is one of the two providers where "stream it or download it" actually
//! completes. Jamendo requires a free client id; without one the provider
//! reports [`SourceError::NotConfigured`] so the UI can tell the user what to
//! set up, rather than silently vanishing from the results.

use reqwest::blocking::Client;
use std::time::Duration;

use super::deezer::urlencoding_encode;
use super::{
    duration_field, get_json, id_field, str_field, SourceError, SourceProvider, SourceTrack,
};

pub struct Jamendo {
    client_id: Option<String>,
}

impl Jamendo {
    pub fn new(client_id: Option<String>) -> Self {
        Self {
            client_id: client_id.filter(|id| !id.trim().is_empty()),
        }
    }

    fn client_id(&self) -> Result<&str, SourceError> {
        self.client_id.as_deref().ok_or_else(|| {
            SourceError::NotConfigured(
                "Add a free Jamendo client ID in Settings to search Jamendo".to_string(),
            )
        })
    }
}

impl SourceProvider for Jamendo {
    fn id(&self) -> &'static str {
        "jamendo"
    }

    fn display_name(&self) -> &'static str {
        "Jamendo"
    }

    fn rate_limit(&self) -> Duration {
        Duration::from_millis(350)
    }

    fn search(
        &self,
        client: &Client,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>, SourceError> {
        let client_id = self.client_id()?;
        let url = format!(
            "https://api.jamendo.com/v3.0/tracks/?client_id={}&format=json&limit={}&search={}&audioformat=mp32&include=licenses",
            urlencoding_encode(client_id),
            limit.clamp(1, 50),
            urlencoding_encode(query),
        );
        let value = get_json(client, &url)?;

        // Jamendo signals failure inside `headers.status`, with HTTP 200.
        if let Some(headers) = value.get("headers") {
            if str_field(headers, "status").as_deref() == Some("failed") {
                let message = str_field(headers, "error_message")
                    .unwrap_or_else(|| "Jamendo rejected the request".into());
                return Err(SourceError::Network(message));
            }
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
    let id = id_field(item, "id")?;
    let title = str_field(item, "name").unwrap_or_else(|| "Unknown title".to_string());
    let artist = str_field(item, "artist_name").unwrap_or_else(|| "Unknown artist".to_string());

    // Prefer the download URL when the licence permits keeping a copy; it is
    // the same audio without the streaming wrapper.
    let downloadable = item
        .get("audiodownload_allowed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let audio_url = if downloadable {
        str_field(item, "audiodownload").or_else(|| str_field(item, "audio"))
    } else {
        str_field(item, "audio")
    }?;

    Some(SourceTrack {
        provider: "jamendo".to_string(),
        id,
        title,
        artist,
        album: str_field(item, "album_name"),
        duration_seconds: duration_field(item, "duration"),
        artwork_url: str_field(item, "image").or_else(|| str_field(item, "album_image")),
        audio_url: Some(audio_url),
        is_full_length: true,
        downloadable,
        attribution: str_field(item, "license_ccurl")
            .map(|url| format!("Creative Commons — {url}"))
            .or_else(|| Some("Creative Commons — Jamendo".to_string())),
        already_in_library: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "headers": { "status": "success", "results_count": 2 },
      "results": [
        {
          "id": "1532771",
          "name": "Sunrise",
          "artist_name": "Bensound",
          "album_name": "Royalty Free",
          "duration": 185,
          "audio": "https://prod-1.storage.jamendo.com/?trackid=1532771&format=mp31",
          "audiodownload": "https://prod-1.storage.jamendo.com/download/track/1532771/mp32/",
          "audiodownload_allowed": true,
          "image": "https://usercontent.jamendo.com/1532771.jpg",
          "license_ccurl": "http://creativecommons.org/licenses/by-nd/3.0/"
        },
        {
          "id": "999",
          "name": "Stream Only",
          "artist_name": "Someone",
          "duration": 200,
          "audio": "https://prod-1.storage.jamendo.com/?trackid=999&format=mp31",
          "audiodownload_allowed": false
        }
      ]
    }"#;

    #[test]
    fn parses_full_length_downloadable_track() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let track = parse_track(&value["results"][0]).unwrap();
        assert_eq!(track.id, "1532771");
        assert_eq!(track.artist, "Bensound");
        assert_eq!(track.duration_seconds, Some(185.0));
        assert!(track.is_full_length);
        assert!(track.downloadable);
        // Downloadable tracks must use the download URL, not the stream wrapper.
        assert!(track.audio_url.unwrap().contains("/download/track/"));
        assert!(track.attribution.unwrap().contains("creativecommons.org"));
    }

    #[test]
    fn stream_only_track_is_not_offered_for_download() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        let track = parse_track(&value["results"][1]).unwrap();
        assert!(!track.downloadable);
        // Still streamable, and still full length.
        assert!(track.is_full_length);
        assert!(track.audio_url.unwrap().contains("format=mp31"));
    }

    #[test]
    fn missing_client_id_is_a_setup_hint_not_a_crash() {
        let jamendo = Jamendo::new(None);
        let error = jamendo.client_id().unwrap_err();
        assert!(matches!(error, SourceError::NotConfigured(_)));
        assert!(error.to_string().contains("Settings"));
    }

    #[test]
    fn blank_client_id_is_treated_as_absent() {
        assert!(Jamendo::new(Some("   ".into())).client_id.is_none());
    }
}
