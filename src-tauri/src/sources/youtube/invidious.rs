// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Public Invidious servers, one of the last ways in when neither yt-dlp nor
//! another Wave can reach YouTube.
//!
//! Audio is asked for with `local=true`, so the server relays it itself:
//! YouTube's own stream URLs only work from the address that asked for them.

use reqwest::blocking::Client;
use serde_json::Value;

use super::{
    codec_of, expires_from_url, ext_for_mime, is_video_id, loose_string, loose_u64, Candidate,
    Stream, PREFERRED_ITAGS,
};

/// The instance directory, healthiest first.
const INSTANCE_LIST: &str = "https://api.invidious.io/instances.json?sort_by=health";

fn get_json(client: &Client, url: &str) -> Result<Value, String> {
    client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| e.to_string())
}

/// Public servers that answer API calls over HTTPS, healthiest first.
pub fn instances(client: &Client) -> Result<Vec<String>, String> {
    Ok(parse_instances(&get_json(client, INSTANCE_LIST)?))
}

fn parse_instances(list: &Value) -> Vec<String> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let info = entry.get(1)?;
            let api = info.get("api").and_then(Value::as_bool) == Some(true);
            let https = info.get("type").and_then(Value::as_str) == Some("https");
            let uri = info.get("uri").and_then(Value::as_str)?;
            (api && https && uri.starts_with("https://"))
                .then(|| uri.trim_end_matches('/').to_string())
        })
        .collect()
}

pub fn search(client: &Client, base: &str, query: &str) -> Result<Vec<Candidate>, String> {
    let url = reqwest::Url::parse_with_params(
        &format!("{base}/api/v1/search"),
        &[("q", query), ("type", "video")],
    )
    .map_err(|e| e.to_string())?;
    Ok(parse_search(&get_json(client, url.as_str())?))
}

fn parse_search(results: &Value) -> Vec<Candidate> {
    results
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r.get("type").and_then(Value::as_str) == Some("video"))
        .filter_map(|r| {
            let id = r
                .get("videoId")
                .and_then(Value::as_str)
                .filter(|id| is_video_id(id))?;
            Some(Candidate {
                id: id.to_string(),
                title: r
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                channel: r.get("author").and_then(Value::as_str).map(String::from),
                duration_seconds: loose_u64(r.get("lengthSeconds")).map(|s| s as f64),
            })
        })
        .collect()
}

/// The audio stream for `video_id`, relayed through `base`. With `itag`, only
/// that format will do.
pub fn stream(
    client: &Client,
    base: &str,
    video_id: &str,
    itag: Option<&str>,
) -> Result<Stream, String> {
    if !is_video_id(video_id) {
        return Err(format!("Not a YouTube video id: {video_id}"));
    }
    let video = get_json(
        client,
        &format!("{base}/api/v1/videos/{video_id}?local=true"),
    )?;
    parse_stream(&video, base, itag).ok_or_else(|| "No audio stream on this server".to_string())
}

fn parse_stream(video: &Value, base: &str, itag: Option<&str>) -> Option<Stream> {
    let formats: Vec<&Value> = video
        .get("adaptiveFormats")
        .and_then(Value::as_array)?
        .iter()
        .filter(|f| {
            f.get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.starts_with("audio/"))
        })
        .collect();
    let with_itag = |wanted: &str| {
        formats
            .iter()
            .find(|f| loose_string(f.get("itag")).as_deref() == Some(wanted))
            .copied()
    };
    let format = match itag {
        Some(wanted) => with_itag(wanted)?,
        None => PREFERRED_ITAGS
            .iter()
            .find_map(|&wanted| with_itag(wanted))?,
    };
    let url = format.get("url").and_then(Value::as_str)?;
    // Relayed URLs come back relative to the server on some versions.
    let url = if url.starts_with('/') {
        format!("{base}{url}")
    } else {
        url.to_string()
    };
    if !url.starts_with("https://") {
        return None;
    }
    let mime = format
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(Stream {
        expires_at: expires_from_url(&url),
        url,
        http_headers: Vec::new(),
        itag: loose_string(format.get("itag")),
        ext: ext_for_mime(mime).to_string(),
        codec: codec_of(mime),
        duration_seconds: loose_u64(video.get("lengthSeconds")).map(|s| s as f64),
        filesize: loose_u64(format.get("clen")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_only_https_servers_with_the_api_open() {
        let list = json!([
            ["a.example", {"api": true, "type": "https", "uri": "https://a.example/"}],
            ["b.example", {"api": false, "type": "https", "uri": "https://b.example"}],
            ["c.onion", {"api": true, "type": "onion", "uri": "http://c.onion"}],
            ["d.example", {"api": null, "type": "https", "uri": "https://d.example"}]
        ]);
        assert_eq!(
            parse_instances(&list),
            vec!["https://a.example".to_string()]
        );
        assert!(parse_instances(&json!({"oops": 1})).is_empty());
    }

    #[test]
    fn reads_video_results_only() {
        let results = json!([
            {"type": "video", "videoId": "5NV6Rdv1a3I", "title": "Get Lucky", "author": "Daft Punk", "lengthSeconds": 249},
            {"type": "channel", "authorId": "UC123"},
            {"type": "video", "videoId": "bad", "title": "broken id"}
        ]);
        let found = parse_search(&results);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].channel.as_deref(), Some("Daft Punk"));
        assert_eq!(found[0].duration_seconds, Some(249.0));
    }

    fn video() -> Value {
        // The shape a live instance returned, with `itag` and `clen` as strings.
        json!({
            "lengthSeconds": 249,
            "adaptiveFormats": [
                {"itag": "140", "type": "audio/mp4; codecs=\"mp4a.40.2\"", "clen": "4025466",
                 "url": "https://inv.example/videoplayback?expire=1793545710&itag=140&local=true"},
                {"itag": "251", "type": "audio/webm; codecs=\"opus\"", "clen": "4155462",
                 "url": "/videoplayback?expire=1793545710&itag=251&local=true"},
                {"itag": "137", "type": "video/mp4; codecs=\"avc1\"", "url": "https://inv.example/v"}
            ]
        })
    }

    #[test]
    fn prefers_aac_and_makes_relayed_urls_absolute() {
        let stream = parse_stream(&video(), "https://inv.example", None).unwrap();
        assert_eq!(stream.itag.as_deref(), Some("140"));
        assert_eq!(stream.ext, "m4a");
        assert_eq!(stream.codec.as_deref(), Some("mp4a.40.2"));
        assert_eq!(stream.filesize, Some(4_025_466));
        assert_eq!(stream.expires_at, Some(1_793_545_710));

        let opus = parse_stream(&video(), "https://inv.example", Some("251")).unwrap();
        assert_eq!(opus.ext, "webm");
        assert!(
            opus.url.starts_with("https://inv.example/videoplayback"),
            "a relative URL is made absolute: {}",
            opus.url
        );
    }

    #[test]
    fn a_pinned_format_is_the_only_one_taken() {
        let stream = parse_stream(&video(), "https://inv.example", Some("140")).unwrap();
        assert_eq!(stream.ext, "m4a");
        assert_eq!(stream.filesize, Some(4_025_466));
        assert!(parse_stream(&video(), "https://inv.example", Some("250")).is_none());
    }
}
