//! Internet Archive — full-length public-domain and CC audio.
//!
//! Archive's search API is *item*-level, and an item is often a whole concert
//! or album rather than a single track. Rather than firing a metadata request
//! per result at search time (which would make one search into N+1 requests),
//! search returns one row per item and [`InternetArchive::resolve_audio`] does
//! the file listing lazily, on play. The tradeoff is that a result represents
//! "the first audio file in this item", which is why the item title is shown
//! as-is instead of being dressed up as a track title.

use reqwest::blocking::Client;
use std::time::Duration;

use super::deezer::urlencoding_encode;
use super::{get_json, str_field, SourceError, SourceProvider, SourceTrack};

/// Audio formats we can decode, best first.
const PREFERRED_FORMATS: &[&str] = &["flac", "mp3", "ogg", "m4a", "wav"];

/// Narrows a `mediatype:(audio)` search to things a music player should offer.
///
/// Both halves are load-bearing, and both are indexed fields so they cost no
/// extra requests:
///
/// - `collection:(audio_music OR etree)` keeps podcasts, lectures, and YouTube
///   rips out. Without it, searching "blackened" returns a talk-radio episode
///   before any music.
/// - `NOT access-restricted-item:true` drops items whose files the Archive
///   serves as 401/403. Those look identical in search results but fail the
///   moment you press play.
const MUSIC_FILTER: &str =
    "AND collection:(audio_music OR etree) AND NOT access-restricted-item:true";

pub struct InternetArchive;

impl SourceProvider for InternetArchive {
    fn id(&self) -> &'static str {
        "archive"
    }

    fn display_name(&self) -> &'static str {
        "Internet Archive"
    }

    fn rate_limit(&self) -> Duration {
        Duration::from_millis(500)
    }

    fn search(
        &self,
        client: &Client,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>, SourceError> {
        let url = format!(
            "https://archive.org/advancedsearch.php?q={}&fl%5B%5D=identifier&fl%5B%5D=title&fl%5B%5D=creator&fl%5B%5D=year&rows={}&page=1&output=json",
            urlencoding_encode(&format!("({query}) AND mediatype:(audio) {MUSIC_FILTER}")),
            limit.clamp(1, 50),
        );
        let value = get_json(client, &url)?;

        let items = value
            .get("response")
            .and_then(|r| r.get("docs"))
            .and_then(|d| d.as_array())
            .ok_or_else(|| SourceError::Parse("missing `response.docs`".into()))?;

        Ok(items.iter().filter_map(parse_item).collect())
    }

    fn resolve_audio(&self, client: &Client, track: &SourceTrack) -> Result<String, SourceError> {
        let url = format!("https://archive.org/metadata/{}", track.id);
        let value = get_json(client, &url)?;
        let files = value
            .get("files")
            .and_then(|f| f.as_array())
            .ok_or(SourceError::NoAudio)?;
        let name = best_audio_file(files).ok_or(SourceError::NoAudio)?;
        Ok(format!(
            "https://archive.org/download/{}/{}",
            track.id,
            urlencoding_encode(&name)
        ))
    }
}

fn parse_item(item: &serde_json::Value) -> Option<SourceTrack> {
    let id = str_field(item, "identifier")?;
    let title = str_field(item, "title").unwrap_or_else(|| id.clone());
    // `creator` comes back as a string or an array depending on the item.
    let artist = match item.get("creator") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(serde_json::Value::Array(values)) => values
            .first()
            .and_then(|v| v.as_str())
            .unwrap_or("Internet Archive")
            .to_string(),
        _ => "Internet Archive".to_string(),
    };

    Some(SourceTrack {
        provider: "archive".to_string(),
        id,
        title,
        artist,
        album: None,
        // Unknown until the file listing is fetched; the player picks it up
        // from the decoded stream.
        duration_seconds: None,
        artwork_url: None,
        // Resolved lazily — see the module docs.
        audio_url: None,
        is_full_length: true,
        downloadable: true,
        attribution: Some("Internet Archive".to_string()),
        already_in_library: None,
    })
}

/// Pick the best decodable audio file from an item's file listing, preferring
/// higher-fidelity formats and ignoring Archive's derivative junk.
fn best_audio_file(files: &[serde_json::Value]) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for file in files {
        let Some(name) = str_field(file, "name") else {
            continue;
        };
        let Some(ext) = name.rsplit('.').next().map(|e| e.to_ascii_lowercase()) else {
            continue;
        };
        let Some(rank) = PREFERRED_FORMATS.iter().position(|f| *f == ext) else {
            continue;
        };
        if best.as_ref().is_none_or(|(best_rank, _)| rank < *best_rank) {
            best = Some((rank, name));
        }
    }
    best.map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_query_excludes_restricted_and_non_music() {
        // Both filters fix a bug seen in the running app: a 401 on a
        // restricted podcast item that a bare mediatype:(audio) search
        // ranked first for a music query.
        assert!(MUSIC_FILTER.contains("NOT access-restricted-item:true"));
        assert!(MUSIC_FILTER.contains("collection:(audio_music OR etree)"));
    }

    #[test]
    fn parses_string_and_array_creators() {
        let docs = serde_json::json!([
            { "identifier": "gd1977-05-08", "title": "Grateful Dead Live", "creator": "Grateful Dead" },
            { "identifier": "multi", "title": "Compilation", "creator": ["First Artist", "Second"] },
            { "identifier": "bare", "title": "No Creator" }
        ]);
        let tracks: Vec<_> = docs
            .as_array()
            .unwrap()
            .iter()
            .filter_map(parse_item)
            .collect();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].artist, "Grateful Dead");
        assert_eq!(tracks[1].artist, "First Artist");
        assert_eq!(tracks[2].artist, "Internet Archive");
        // Audio is always resolved lazily.
        assert!(tracks.iter().all(|t| t.audio_url.is_none()));
    }

    #[test]
    fn item_without_identifier_is_skipped() {
        let doc = serde_json::json!({ "title": "Orphan" });
        assert!(parse_item(&doc).is_none());
    }

    #[test]
    fn best_audio_file_prefers_lossless_and_ignores_non_audio() {
        let files = serde_json::json!([
            { "name": "cover.jpg" },
            { "name": "notes.txt" },
            { "name": "track01.mp3" },
            { "name": "track01.flac" },
            { "name": "meta.xml" }
        ]);
        let files = files.as_array().unwrap();
        assert_eq!(best_audio_file(files).as_deref(), Some("track01.flac"));
    }

    #[test]
    fn best_audio_file_returns_none_when_item_has_no_audio() {
        let files = serde_json::json!([{ "name": "scan.pdf" }, { "name": "cover.jpg" }]);
        assert!(best_audio_file(files.as_array().unwrap()).is_none());
    }
}
