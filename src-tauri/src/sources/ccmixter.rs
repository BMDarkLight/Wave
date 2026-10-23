// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! ccMixter: full-length Creative Commons music, mostly remixes and
//! instrumentals.
//!
//! Every upload is under a CC licence, so every result can be kept. An upload
//! is a bundle of files: the mix itself, often a FLAC of it, and sometimes
//! stems, zips, or cover images. The first MP3 is the mix.
//!
//! The file server has hotlink protection, so audio is fetched with a
//! ccMixter `Referer`; see [`CcMixter::fetch_referer`].

use reqwest::blocking::Client;

use super::deezer::urlencoding_encode;
use super::{get_json, id_field, str_field, SourceError, SourceProvider, SourceTrack};

/// Containers the engine can decode, in the order we would rather fetch them.
/// MP3 comes first because the FLAC alongside it is often ten times larger.
const PREFERRED_EXTENSIONS: &[&str] = &["mp3", "ogg", "flac", "wav"];

pub struct CcMixter;

impl SourceProvider for CcMixter {
    fn id(&self) -> &'static str {
        "ccmixter"
    }

    fn display_name(&self) -> &'static str {
        "ccMixter"
    }

    fn search(
        &self,
        client: &Client,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>, SourceError> {
        // `search_type=all` requires every word. The default matches any of
        // them, so "jazz piano" returned piano tracks with no jazz in sight.
        let url = format!(
            "https://ccmixter.org/api/query?f=json&search={}&search_type=all&limit={}",
            urlencoding_encode(query),
            limit.clamp(1, 50)
        );
        let value = get_json(client, &url)?;

        let items = value
            .as_array()
            .ok_or_else(|| SourceError::Parse("expected an array of uploads".into()))?;

        Ok(items.iter().filter_map(parse_track).collect())
    }

    fn resolve_audio(&self, _client: &Client, track: &SourceTrack) -> Result<String, SourceError> {
        track.audio_url.clone().ok_or(SourceError::NoAudio)
    }

    /// Files under `/content/` answer 403 unless the request comes from a
    /// ccMixter page, which is what a browser sends when the download link
    /// on an upload's page is clicked. Without a trailing slash it is refused.
    fn fetch_referer(&self) -> Option<&'static str> {
        Some("https://ccmixter.org/")
    }
}

fn parse_track(item: &serde_json::Value) -> Option<SourceTrack> {
    let id = id_field(item, "upload_id")?;
    let file = pick_audio_file(item.get("files")?.as_array()?)?;
    let audio_url = str_field(file, "download_url")?;
    let duration_seconds = file
        .get("file_format_info")
        .and_then(|info| str_field(info, "ps"))
        .and_then(|ps| parse_clock(&ps));

    // Some licence names carry a doubled space, "Attribution Noncommercial  (3.0)".
    let attribution = str_field(item, "license_name")
        .map(|license| license.split_whitespace().collect::<Vec<_>>().join(" "))
        .map(|license| format!("CC {license}, ccMixter"))
        .or_else(|| Some("Creative Commons, ccMixter".to_string()));

    Some(SourceTrack {
        provider: "ccmixter".to_string(),
        id,
        title: str_field(item, "upload_name").unwrap_or_else(|| "Unknown title".to_string()),
        artist: str_field(item, "user_real_name")
            .or_else(|| str_field(item, "user_name"))
            .unwrap_or_else(|| "Unknown artist".to_string()),
        album: None,
        duration_seconds,
        artwork_url: None,
        audio_url: Some(audio_url),
        is_full_length: true,
        downloadable: true,
        attribution,
        already_in_library: None,
    })
}

fn pick_audio_file(files: &[serde_json::Value]) -> Option<&serde_json::Value> {
    let extension_of = |file: &serde_json::Value| {
        let info = file.get("file_format_info")?;
        if str_field(info, "media-type").as_deref() != Some("audio") {
            return None;
        }
        str_field(info, "default-ext").map(|ext| ext.to_ascii_lowercase())
    };
    PREFERRED_EXTENSIONS.iter().find_map(|wanted| {
        files
            .iter()
            .find(|file| extension_of(file).as_deref() == Some(*wanted))
    })
}

/// "3:38" or "1:02:05" to seconds.
fn parse_clock(text: &str) -> Option<f64> {
    let mut seconds = 0u64;
    for part in text.trim().split(':') {
        let value: u64 = part.parse().ok()?;
        seconds = seconds.checked_mul(60)?.checked_add(value)?;
    }
    (seconds > 0).then_some(seconds as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {
        "upload_id": 71291,
        "upload_name": "The Light Between Us",
        "user_name": "Karstenholymoly",
        "user_real_name": "Kraftamt",
        "license_name": "Attribution Noncommercial (4.0)",
        "files": [
          {
            "file_format_info": { "media-type": "audio", "default-ext": "flac", "ps": "3:38" },
            "download_url": "https://ccmixter.org/content/Karstenholymoly/The_Light.flac"
          },
          {
            "file_format_info": { "media-type": "audio", "default-ext": "mp3", "ps": "3:38" },
            "download_url": "https://ccmixter.org/content/Karstenholymoly/The_Light.mp3"
          }
        ]
      },
      {
        "upload_id": 500,
        "upload_name": "Sample Pack",
        "user_name": "someone",
        "files": [
          { "file_format_info": { "media-type": "archive", "default-ext": "zip" },
            "download_url": "https://ccmixter.org/content/someone/pack.zip" },
          { "file_format_info": { "media-type": "image", "default-ext": "jpg" },
            "download_url": "https://ccmixter.org/content/someone/cover.jpg" }
        ]
      },
      {
        "upload_id": 501,
        "upload_name": "Lossless Only",
        "user_name": "flacfan",
        "files": [
          { "file_format_info": { "media-type": "audio", "default-ext": "flac", "ps": "1:02:05" },
            "download_url": "https://ccmixter.org/content/flacfan/long.flac" }
        ]
      }
    ]"#;

    fn parsed() -> Vec<SourceTrack> {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        value
            .as_array()
            .unwrap()
            .iter()
            .filter_map(parse_track)
            .collect()
    }

    #[test]
    fn prefers_the_mp3_over_a_larger_flac() {
        let track = &parsed()[0];
        assert_eq!(track.id, "71291");
        assert_eq!(track.artist, "Kraftamt");
        assert_eq!(track.extension(), "mp3");
        assert_eq!(track.duration_seconds, Some(218.0));
        assert!(track.is_full_length);
        assert!(track.downloadable);
        assert_eq!(
            track.attribution.as_deref(),
            Some("CC Attribution Noncommercial (4.0), ccMixter")
        );
    }

    #[test]
    fn skips_uploads_with_no_audio_and_falls_back_to_flac() {
        let tracks = parsed();
        let ids: Vec<_> = tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["71291", "501"]);
        assert_eq!(tracks[1].extension(), "flac");
        assert_eq!(tracks[1].duration_seconds, Some(3725.0));
    }

    #[test]
    fn clock_parsing_rejects_garbage() {
        assert_eq!(parse_clock("0:39"), Some(39.0));
        assert_eq!(parse_clock("0:00"), None);
        assert_eq!(parse_clock("3:x8"), None);
        assert_eq!(parse_clock(""), None);
    }
}
