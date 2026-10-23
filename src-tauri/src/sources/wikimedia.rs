// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Wikimedia Commons: freely licensed and public-domain recordings, strong on
//! classical and early jazz.
//!
//! Commons is a media library, not a music catalog, so search needs more
//! filtering than the other providers. MIDI files are excluded in the query
//! because the engine cannot render them, and short clips are dropped after
//! parsing because `filetype:audio` also matches thousands of one-second word
//! pronunciations. Metadata comes from the file page's `extmetadata`, where
//! the artist is an HTML fragment.

use reqwest::blocking::Client;

use super::deezer::urlencoding_encode;
use super::{
    duration_field, get_json, id_field, str_field, SourceError, SourceProvider, SourceTrack,
};

/// Anything shorter is a pronunciation, a sound effect, or a jingle.
const MIN_SECONDS: f64 = 30.0;

/// Longer than this and the artist field is a paragraph, not a name.
const MAX_ARTIST_CHARS: usize = 80;

const PLAYABLE_EXTENSIONS: &[&str] = &["mp3", "ogg", "oga", "opus", "flac", "wav"];

pub struct WikimediaCommons;

impl SourceProvider for WikimediaCommons {
    fn id(&self) -> &'static str {
        "wikimedia"
    }

    fn display_name(&self) -> &'static str {
        "Wikimedia Commons"
    }

    fn search(
        &self,
        client: &Client,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>, SourceError> {
        let search = format!("{query} filetype:audio -filemime:audio/midi");
        let url = format!(
            "https://commons.wikimedia.org/w/api.php?action=query&format=json&formatversion=2\
             &generator=search&gsrnamespace=6&gsrlimit={}&gsrsearch={}\
             &prop=imageinfo&iiprop=url|size|extmetadata\
             &iiextmetadatafilter=Artist|ObjectName|LicenseShortName",
            limit.clamp(1, 50),
            urlencoding_encode(&search),
        );
        parse_response(&get_json(client, &url)?)
    }

    fn resolve_audio(&self, _client: &Client, track: &SourceTrack) -> Result<String, SourceError> {
        track.audio_url.clone().ok_or(SourceError::NoAudio)
    }
}

fn parse_response(value: &serde_json::Value) -> Result<Vec<SourceTrack>, SourceError> {
    // The API reports bad requests as HTTP 200 with an `error` object.
    if let Some(error) = value.get("error") {
        let message = str_field(error, "info").unwrap_or_else(|| "Commons error".into());
        return Err(SourceError::Network(message));
    }

    // No `query` at all is how Commons says "nothing matched".
    let Some(pages) = value.get("query").and_then(|q| q.get("pages")) else {
        return Ok(Vec::new());
    };
    let pages = pages
        .as_array()
        .ok_or_else(|| SourceError::Parse("`pages` is not an array".into()))?;

    // Pages come back in page id order, not by rank; `index` carries the rank.
    let mut ranked: Vec<_> = pages
        .iter()
        .filter_map(|page| {
            let rank = page
                .get("index")
                .and_then(|i| i.as_u64())
                .unwrap_or(u64::MAX);
            parse_track(page).map(|track| (rank, track))
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    Ok(ranked.into_iter().map(|(_, track)| track).collect())
}

fn parse_track(page: &serde_json::Value) -> Option<SourceTrack> {
    let id = id_field(page, "pageid")?;
    let info = page.get("imageinfo")?.as_array()?.first()?;

    let duration_seconds = duration_field(info, "duration")?;
    if duration_seconds < MIN_SECONDS {
        return None;
    }

    // Drop the tracking query Commons appends to every file URL.
    let audio_url = str_field(info, "url")?.split('?').next()?.to_string();
    let extension = audio_url.rsplit('.').next()?.to_ascii_lowercase();
    if !PLAYABLE_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }

    let meta = info.get("extmetadata");
    let meta_text = |key: &str| {
        meta.and_then(|m| m.get(key))
            .and_then(|field| str_field(field, "value"))
            .map(|html| plain_text(&html))
            .filter(|text| !text.is_empty())
    };

    let title = meta_text("ObjectName")
        .or_else(|| title_from_file(page))
        .unwrap_or_else(|| "Unknown title".to_string());
    let artist = meta_text("Artist")
        .and_then(|artist| artist.lines().next().map(str::trim).map(str::to_string))
        .filter(|artist| !artist.is_empty() && artist.chars().count() <= MAX_ARTIST_CHARS)
        .unwrap_or_else(|| "Unknown artist".to_string());
    let attribution = Some(match meta_text("LicenseShortName") {
        Some(license) => format!("{license}, Wikimedia Commons"),
        None => "Wikimedia Commons".to_string(),
    });

    Some(SourceTrack {
        provider: "wikimedia".to_string(),
        id,
        title,
        artist,
        album: None,
        duration_seconds: Some(duration_seconds),
        artwork_url: None,
        audio_url: Some(audio_url),
        is_full_length: true,
        // Commons only hosts freely licensed or public-domain media.
        downloadable: true,
        attribution,
        already_in_library: None,
    })
}

/// "File:Jazz at the park.ogg" to "Jazz at the park".
fn title_from_file(page: &serde_json::Value) -> Option<String> {
    let title = str_field(page, "title")?;
    let name = title.strip_prefix("File:").unwrap_or(&title);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    Some(stem.trim().to_string()).filter(|s| !s.is_empty())
}

/// Strip tags and decode the handful of entities Commons emits. Keeps line
/// breaks so the caller can take the first line of a multi-line credit.
fn plain_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&");
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "batchcomplete": true,
      "query": { "pages": [
        {
          "pageid": 1908117, "index": 2,
          "title": "File:Beethoven Moonlight 3rd movement.ogg",
          "imageinfo": [{
            "duration": 415.42,
            "url": "https://upload.wikimedia.org/wikipedia/commons/d/d4/Beethoven_Moonlight_3rd_movement.ogg?utm_source=commons.wikimedia.org",
            "extmetadata": {
              "ObjectName": { "value": "Beethoven Moonlight 3rd movement" },
              "Artist": { "value": "<bdi><a href=\"https://en.wikipedia.org/wiki/Beethoven\"><span>Ludwig van Beethoven</span></a></bdi>" },
              "LicenseShortName": { "value": "CC BY-SA 2.0 de" }
            }
          }]
        },
        {
          "pageid": 1908097, "index": 1,
          "title": "File:Jazz at the park.ogg",
          "imageinfo": [{
            "duration": 206.0,
            "url": "https://upload.wikimedia.org/wikipedia/commons/a/ab/Jazz_at_the_park.ogg",
            "extmetadata": {
              "Artist": { "value": "Composer: Someone &amp; Friends\nPerformer: Others" }
            }
          }]
        },
        {
          "pageid": 5, "index": 3,
          "title": "File:LL-Q150 (fra)-WikiLucas00-jazz.wav",
          "imageinfo": [{ "duration": 1.2, "url": "https://upload.wikimedia.org/LL.wav" }]
        },
        {
          "pageid": 6, "index": 4,
          "title": "File:Clip.webm",
          "imageinfo": [{ "duration": 120.0, "url": "https://upload.wikimedia.org/Clip.webm" }]
        }
      ]}
    }"#;

    fn parsed() -> Vec<SourceTrack> {
        parse_response(&serde_json::from_str(SAMPLE).unwrap()).unwrap()
    }

    #[test]
    fn keeps_playable_music_in_rank_order() {
        // Short clips and formats the engine cannot play are gone, and the
        // survivors follow `index`, not their order in the payload.
        let ids: Vec<_> = parsed().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, ["1908097", "1908117"]);
    }

    #[test]
    fn cleans_up_metadata() {
        let tracks = parsed();
        let moonlight = &tracks[1];
        assert_eq!(moonlight.title, "Beethoven Moonlight 3rd movement");
        assert_eq!(moonlight.artist, "Ludwig van Beethoven");
        assert_eq!(
            moonlight.audio_url.as_deref(),
            Some("https://upload.wikimedia.org/wikipedia/commons/d/d4/Beethoven_Moonlight_3rd_movement.ogg")
        );
        assert_eq!(moonlight.extension(), "ogg");
        assert_eq!(
            moonlight.attribution.as_deref(),
            Some("CC BY-SA 2.0 de, Wikimedia Commons")
        );
        assert!(moonlight.is_full_length);
        assert!(moonlight.downloadable);

        // No ObjectName: the title comes from the file name.
        let jazz = &tracks[0];
        assert_eq!(jazz.title, "Jazz at the park");
        // A multi-line credit keeps its first line, entities decoded.
        assert_eq!(jazz.artist, "Composer: Someone & Friends");
    }

    #[test]
    fn no_match_is_empty_and_an_api_error_is_reported() {
        let empty = serde_json::json!({ "batchcomplete": true });
        assert!(parse_response(&empty).unwrap().is_empty());

        let error = serde_json::json!({ "error": { "code": "x", "info": "bad search" } });
        assert_eq!(
            parse_response(&error).unwrap_err().to_string(),
            "Network error: bad search"
        );
    }
}
