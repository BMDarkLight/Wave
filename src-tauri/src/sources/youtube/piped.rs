// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Public Piped servers, the last way in. Piped relays audio through its own
//! proxy, so the URLs it hands out work from anywhere.

use reqwest::blocking::Client;
use serde_json::Value;

use super::{
    codec_of, expires_from_url, ext_for_mime, is_video_id, loose_string, loose_u64, Candidate,
    Stream, PREFERRED_ITAGS,
};

/// Piped's own instance directory.
const INSTANCE_LIST: &str = "https://piped-instances.kavin.rocks/";
/// The public instance table in Piped's documentation, read when the
/// directory above is down.
const INSTANCE_DOCS: &str =
    "https://raw.githubusercontent.com/TeamPiped/documentation/main/content/docs/public-instances/index.md";

fn get_json(client: &Client, url: &str) -> Result<Value, String> {
    client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| e.to_string())
}

/// API addresses of public Piped servers.
pub fn instances(client: &Client) -> Result<Vec<String>, String> {
    if let Ok(list) = get_json(client, INSTANCE_LIST) {
        let found = parse_instances(&list);
        if !found.is_empty() {
            return Ok(found);
        }
    }
    let docs = client
        .get(INSTANCE_DOCS)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| e.to_string())?;
    Ok(parse_instance_docs(&docs))
}

fn parse_instances(list: &Value) -> Vec<String> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i.get("api_url").and_then(Value::as_str))
        .filter(|url| url.starts_with("https://"))
        .map(|url| url.trim_end_matches('/').to_string())
        .collect()
}

/// API addresses from the documentation's Markdown table: the HTTPS links in
/// table rows whose host names an API.
fn parse_instance_docs(markdown: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in markdown.lines().filter(|l| l.trim_start().starts_with('|')) {
        for cell in line.split(['|', ' ', '(', ')', '<', '>']) {
            let Some(rest) = cell.strip_prefix("https://") else {
                continue;
            };
            let host = rest.split('/').next().unwrap_or_default();
            let url = format!("https://{host}");
            if host.contains("api") && !found.contains(&url) {
                found.push(url);
            }
        }
    }
    found
}

pub fn search(client: &Client, base: &str, query: &str) -> Result<Vec<Candidate>, String> {
    let url = reqwest::Url::parse_with_params(
        &format!("{base}/search"),
        &[("q", query), ("filter", "music_songs")],
    )
    .map_err(|e| e.to_string())?;
    Ok(parse_search(&get_json(client, url.as_str())?))
}

fn parse_search(results: &Value) -> Vec<Candidate> {
    results
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let url = item.get("url").and_then(Value::as_str)?;
            let id = url.split("v=").nth(1)?.split('&').next()?;
            if !is_video_id(id) {
                return None;
            }
            Some(Candidate {
                id: id.to_string(),
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                channel: item
                    .get("uploaderName")
                    .and_then(Value::as_str)
                    .map(String::from),
                duration_seconds: loose_u64(item.get("duration"))
                    .filter(|&s| s > 0)
                    .map(|s| s as f64),
            })
        })
        .collect()
}

/// The audio stream for `video_id` through `base`. With `itag`, only that
/// format will do.
pub fn stream(
    client: &Client,
    base: &str,
    video_id: &str,
    itag: Option<&str>,
) -> Result<Stream, String> {
    if !is_video_id(video_id) {
        return Err(format!("Not a YouTube video id: {video_id}"));
    }
    let streams = get_json(client, &format!("{base}/streams/{video_id}"))?;
    if let Some(error) = streams.get("error").and_then(Value::as_str) {
        return Err(error.to_string());
    }
    parse_stream(&streams, itag).ok_or_else(|| "No audio stream on this server".to_string())
}

fn parse_stream(streams: &Value, itag: Option<&str>) -> Option<Stream> {
    let audio = streams.get("audioStreams").and_then(Value::as_array)?;
    let with_itag = |wanted: &str| {
        audio
            .iter()
            .find(|f| loose_string(f.get("itag")).as_deref() == Some(wanted))
    };
    let format = match itag {
        Some(wanted) => with_itag(wanted)?,
        None => PREFERRED_ITAGS
            .iter()
            .find_map(|&wanted| with_itag(wanted))?,
    };
    let url = format
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| u.starts_with("https://"))?;
    let mime = format
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(Stream {
        expires_at: expires_from_url(url),
        url: url.to_string(),
        http_headers: Vec::new(),
        itag: loose_string(format.get("itag")),
        ext: ext_for_mime(mime).to_string(),
        codec: format
            .get("codec")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| codec_of(mime)),
        duration_seconds: loose_u64(streams.get("duration")).map(|s| s as f64),
        filesize: loose_u64(format.get("contentLength")).filter(|&n| n > 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_directory() {
        let list = json!([
            {"name": "a", "api_url": "https://pipedapi.a.example/"},
            {"name": "b", "api_url": "http://plain.example"},
            {"name": "c"}
        ]);
        assert_eq!(
            parse_instances(&list),
            vec!["https://pipedapi.a.example".to_string()]
        );
    }

    #[test]
    fn reads_api_hosts_from_the_documentation_table() {
        let docs = "\
| Instance API | Frontend | Locations |
| --- | --- | --- |
| https://pipedapi.kavin.rocks | https://piped.video | Canada |
| [x](https://api.piped.private.coffee/) | https://piped.private.coffee | Austria |
| https://pipedapi.kavin.rocks | dup | |
Not a table line https://pipedapi.ignored.example
";
        assert_eq!(
            parse_instance_docs(docs),
            vec![
                "https://pipedapi.kavin.rocks".to_string(),
                "https://api.piped.private.coffee".to_string()
            ]
        );
    }

    #[test]
    fn reads_song_results() {
        let results = json!({"items": [
            {"url": "/watch?v=5NV6Rdv1a3I", "title": "Get Lucky", "uploaderName": "Daft Punk - Topic", "duration": 248},
            {"url": "/channel/UC123", "title": "a channel"},
            {"url": "/watch?v=aaaaaaaaaaa&list=x", "title": "Live", "duration": -1}
        ]});
        let found = parse_search(&results);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, "5NV6Rdv1a3I");
        assert_eq!(found[0].duration_seconds, Some(248.0));
        assert_eq!(found[1].id, "aaaaaaaaaaa");
        assert_eq!(
            found[1].duration_seconds, None,
            "a negative length means unknown"
        );
    }

    #[test]
    fn picks_opus_unless_a_format_is_pinned() {
        let streams = json!({
            "duration": 249,
            "audioStreams": [
                {"itag": 140, "mimeType": "audio/mp4", "codec": "mp4a.40.2", "contentLength": 4025466,
                 "url": "https://proxy.example/videoplayback?expire=1793545710&itag=140"},
                {"itag": 251, "mimeType": "audio/webm", "codec": "opus", "contentLength": 4155462,
                 "url": "https://proxy.example/videoplayback?expire=1793545710&itag=251"}
            ]
        });
        let stream = parse_stream(&streams, None).unwrap();
        assert_eq!(stream.itag.as_deref(), Some("251"));
        assert_eq!(stream.ext, "webm");
        assert_eq!(stream.filesize, Some(4_155_462));
        let aac = parse_stream(&streams, Some("140")).unwrap();
        assert_eq!(aac.ext, "m4a");
        assert!(parse_stream(&streams, Some("249")).is_none());
    }
}
