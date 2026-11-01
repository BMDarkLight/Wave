// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Online play: full-length audio for catalog results, found on YouTube.
//!
//! Wave ships none of the code that talks to YouTube. When the user turns
//! Online play on and agrees to the install, [`tools`] fetches yt-dlp and the
//! Deno runtime it needs from their own GitHub releases, and [`ytdlp`] runs
//! them. Desktop only: Android can't run programs an app downloads.
//!
//! When yt-dlp can't get through, [`router`] tries the other ways in, in
//! order: a paired desktop Wave on the network, then public [`invidious`] and
//! [`piped`] servers. [`matching`] picks the video that is the catalog song.

pub mod commands;
pub mod invidious;
#[cfg(all(test, not(target_os = "android")))]
mod live;
pub mod matching;
pub mod piped;
pub mod router;
#[cfg(not(target_os = "android"))]
pub mod tools;
#[cfg(not(target_os = "android"))]
pub mod ytdlp;

/// Marks a source track's audio as a YouTube video for [`router`] to find,
/// written `youtube:<id>.<ext>` so the extension still reads as one.
const AUDIO_REF_PREFIX: &str = "youtube:";

/// The audio reference stored for `video_id`, whose stream is a `ext` file.
pub fn audio_ref(video_id: &str, ext: &str) -> String {
    format!("{AUDIO_REF_PREFIX}{video_id}.{ext}")
}

/// The video id in an audio reference, or `None` for an ordinary URL.
pub fn video_id_of(audio_ref: &str) -> Option<&str> {
    let rest = audio_ref.strip_prefix(AUDIO_REF_PREFIX)?;
    let id = rest.split_once('.').map_or(rest, |(id, _)| id);
    is_video_id(id).then_some(id)
}

/// One YouTube result for a search.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub channel: Option<String>,
    pub duration_seconds: Option<f64>,
}

/// A playable audio stream. The URL stops working at `expires_at`.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub url: String,
    /// Headers to send on every request for `url`.
    pub http_headers: Vec<(String, String)>,
    /// YouTube's format number, `251` for Opus or `140` for AAC. The same
    /// format is the same file whichever way it is fetched.
    pub itag: Option<String>,
    /// Container extension, `webm` or `m4a`.
    pub ext: String,
    pub codec: Option<String>,
    pub duration_seconds: Option<f64>,
    pub filesize: Option<u64>,
    /// Unix seconds, from the URL's `expire` parameter.
    pub expires_at: Option<u64>,
}

/// YouTube's audio-only formats in the order Wave prefers them: AAC in MP4,
/// then Opus. Opus is the better codec, but Wave can't seek in YouTube's WebM
/// files, while its MP4 files seek to the sample.
const PREFERRED_ITAGS: [&str; 2] = ["140", "251"];

/// Container for a YouTube audio MIME type such as `audio/webm; codecs="opus"`.
fn ext_for_mime(mime: &str) -> &'static str {
    if mime.starts_with("audio/mp4") {
        "m4a"
    } else {
        "webm"
    }
}

/// YouTube serves at most this much per request without slowing down, the
/// chunk yt-dlp uses. A longer read ends early and the reader asks again from
/// where it stopped.
const CHUNK_BYTES: u64 = 10 * 1024 * 1024;

/// How long a new YouTube stream URL is refused before it serves, measured
/// at one to two seconds, and how many times to ask through it.
const FRESH_URL_WAIT: std::time::Duration = std::time::Duration::from_millis(800);
const FRESH_URL_RETRIES: usize = 4;

/// Part of a stream, opened for an HTTP `Range`.
pub struct Opened {
    pub response: reqwest::blocking::Response,
    pub start: u64,
    /// The last byte, inclusive.
    pub end: u64,
    pub total: u64,
}

/// The first and, if given, last byte of a `bytes=a-` or `bytes=a-b` range.
fn parse_range(range: &str) -> (u64, Option<u64>) {
    let Some(spec) = range.trim().strip_prefix("bytes=") else {
        return (0, None);
    };
    let (start, end) = spec.split_once('-').unwrap_or((spec, ""));
    (start.trim().parse().unwrap_or(0), end.trim().parse().ok())
}

/// YouTube's own servers turn away a `Range` header that doesn't start at
/// byte 0. They take the range as a `range` query parameter instead, which
/// is how yt-dlp reads them.
fn is_googlevideo(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.ends_with(".googlevideo.com")))
        .unwrap_or(false)
}

/// `range` of `stream`, asked for the way its host understands.
pub fn open_range(stream: &Stream, range: &str) -> Result<Opened, String> {
    let (start, wanted_end) = parse_range(range);
    let total = stream.filesize.or_else(|| {
        reqwest::Url::parse(&stream.url).ok().and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "clen")
                .and_then(|(_, v)| v.parse().ok())
        })
    });
    let client = crate::remote::http::stream_client();
    let with_headers = |mut request: reqwest::blocking::RequestBuilder| {
        for (name, value) in &stream.http_headers {
            request = request.header(name.as_str(), value.as_str());
        }
        request
    };

    if let (true, Some(total)) = (is_googlevideo(&stream.url), total) {
        if start >= total {
            return Err("past the end of the song".into());
        }
        let end = wanted_end
            .unwrap_or(u64::MAX)
            .min(total - 1)
            .min(start + CHUNK_BYTES - 1);
        // Appended as text: re-encoding the query would break its signature.
        let separator = if stream.url.contains('?') { '&' } else { '?' };
        let url = format!("{}{separator}range={start}-{end}", stream.url);
        for attempt in 0..=FRESH_URL_RETRIES {
            let response = with_headers(client.get(&url))
                .send()
                .map_err(|e| e.to_string())?;
            match response.status().as_u16() {
                200 | 206 => {
                    return Ok(Opened {
                        response,
                        start,
                        end,
                        total,
                    })
                }
                // A URL YouTube has only just handed out is refused for a
                // second or two, then served.
                403 if attempt < FRESH_URL_RETRIES => std::thread::sleep(FRESH_URL_WAIT),
                code => return Err(format!("answered {code}")),
            }
        }
        return Err("answered 403".into());
    }

    let header = match wanted_end {
        Some(end) => format!("bytes={start}-{end}"),
        None => format!("bytes={start}-"),
    };
    let response = with_headers(client.get(&stream.url).header("Range", header))
        .send()
        .map_err(|e| e.to_string())?;
    match response.status().as_u16() {
        206 => {
            let content_range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes "))
                .map(String::from);
            let (span, size) = content_range
                .as_deref()
                .and_then(|v| v.split_once('/'))
                .ok_or("no Content-Range")?;
            let (from, to) = span.split_once('-').ok_or("bad Content-Range")?;
            Ok(Opened {
                response,
                start: from.parse().map_err(|_| "bad Content-Range")?,
                end: to.parse().map_err(|_| "bad Content-Range")?,
                total: size.parse().ok().or(total).ok_or("no length")?,
            })
        }
        200 if start == 0 => {
            let total = response.content_length().or(total).ok_or("no length")?;
            Ok(Opened {
                response,
                start: 0,
                end: total.saturating_sub(1),
                total,
            })
        }
        code => Err(format!("answered {code}")),
    }
}

/// Client for public Invidious and Piped servers. Their health varies by the
/// hour, so a slow one is given up on quickly and the next one tried.
fn public_client() -> &'static reqwest::blocking::Client {
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(4))
            .timeout(std::time::Duration::from_secs(8))
            .user_agent(super::USER_AGENT)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
}

/// A JSON field that servers send as either a number or a string.
fn loose_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn loose_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    loose_string(value)?.parse().ok()
}

/// The codec named in a MIME type's `codecs` parameter.
fn codec_of(mime: &str) -> Option<String> {
    let codecs = mime.split("codecs=").nth(1)?;
    let codec = codecs.trim_matches(|c| c == '"' || c == ' ' || c == ';');
    (!codec.is_empty()).then(|| codec.to_string())
}

/// YouTube video ids are 11 characters of URL-safe base64. Checking keeps
/// anything else, an option-looking `-x` included, out of command lines and
/// URLs.
pub fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The `expire` parameter YouTube stream URLs carry, in Unix seconds.
fn expires_from_url(url: &str) -> Option<u64> {
    reqwest::Url::parse(url).ok().and_then(|u| {
        u.query_pairs()
            .find(|(k, _)| k == "expire")
            .and_then(|(_, v)| v.parse().ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_audio_ref_round_trips_and_keeps_its_extension() {
        let reference = audio_ref("dQw4w9WgXcQ", "webm");
        assert_eq!(reference, "youtube:dQw4w9WgXcQ.webm");
        assert_eq!(video_id_of(&reference), Some("dQw4w9WgXcQ"));
        assert_eq!(video_id_of("youtube:dQw4w9WgXcQ"), Some("dQw4w9WgXcQ"));
        assert_eq!(video_id_of("https://example.com/a.mp3"), None);
        assert_eq!(video_id_of("youtube:../../etc.x"), None);
    }

    #[test]
    fn reads_byte_ranges() {
        assert_eq!(parse_range("bytes=0-"), (0, None));
        assert_eq!(parse_range("bytes=1000-1999"), (1000, Some(1999)));
        assert_eq!(parse_range("nonsense"), (0, None));
        assert!(is_googlevideo(
            "https://rr1---sn-x.googlevideo.com/videoplayback?x=1"
        ));
        assert!(!is_googlevideo("https://inv.example/videoplayback?x=1"));
        assert!(!is_googlevideo("https://googlevideo.com.evil.example/v"));
    }

    fn stream_at(url: String) -> Stream {
        Stream {
            url,
            http_headers: vec![("X-Up".into(), "1".into())],
            itag: Some("251".into()),
            ext: "webm".into(),
            codec: None,
            duration_seconds: None,
            filesize: Some(1000),
            expires_at: None,
        }
    }

    #[test]
    fn a_relay_reports_the_range_it_served() {
        let (port, seen) = crate::remote::testkit::fake_server(vec![
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 10-19/1000\r\n\
             Content-Length: 10\r\n\r\n0123456789"
                .to_string(),
        ]);
        let opened = open_range(
            &stream_at(format!("http://127.0.0.1:{port}/a")),
            "bytes=10-19",
        )
        .unwrap();
        assert_eq!((opened.start, opened.end, opened.total), (10, 19, 1000));
        assert_eq!(opened.response.text().unwrap(), "0123456789");
        let asked = seen.lock().unwrap()[0].to_lowercase();
        assert!(
            asked.contains("range: bytes=10-19") && asked.contains("x-up: 1"),
            "{asked}"
        );
    }

    #[test]
    fn a_whole_file_answer_only_counts_from_the_start() {
        let whole = "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabcd".to_string();
        let (port, _) = crate::remote::testkit::fake_server(vec![whole.clone(), whole]);
        let url = format!("http://127.0.0.1:{port}/a");
        let opened = open_range(&stream_at(url.clone()), "bytes=0-").unwrap();
        assert_eq!((opened.start, opened.end, opened.total), (0, 3, 4));
        assert!(open_range(&stream_at(url), "bytes=2-").is_err());
    }

    #[test]
    fn reads_the_container_from_the_mime_type() {
        assert_eq!(ext_for_mime("audio/mp4; codecs=\"mp4a.40.2\""), "m4a");
        assert_eq!(ext_for_mime("audio/webm; codecs=\"opus\""), "webm");
        assert_eq!(
            codec_of("audio/webm; codecs=\"opus\"").as_deref(),
            Some("opus")
        );
        assert_eq!(codec_of("audio/mp4").as_deref(), None);
    }

    #[test]
    fn reads_when_a_stream_url_expires() {
        assert_eq!(
            expires_from_url("https://h/videoplayback?expire=1793545710&itag=251"),
            Some(1_793_545_710)
        );
        assert_eq!(expires_from_url("https://h/videoplayback"), None);
    }
}
