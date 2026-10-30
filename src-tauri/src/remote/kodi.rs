// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Kodi's music library, through the JSON-RPC API of its web server
//! (Settings, Services, Control, "Allow remote control via HTTP").
//!
//! The catalog is `AudioLibrary.GetSongs`, 500 songs a page. A song's audio is
//! its original file, which Kodi serves under `/vfs/` once `Files.PrepareDownload`
//! names the path; that works for files Kodi itself reads over NFS or SMB too.
//! The catalog has no file sizes, so the stream cache asks for one first.

use super::backend::{
    total_size, unix_seconds, ByteStream, CatalogPage, RemoteBackend, RemoteError, RemoteTrackMeta,
};
use super::gate::NetworkGate;
use super::http::{api_client, basic_auth_travels_safely, stream_client};
use super::uri::percent_encode;
use reqwest::blocking::{RequestBuilder, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;

const PAGE: usize = 500;
const MAX_ART_BYTES: u64 = 10 * 1024 * 1024;
const SONG_PROPERTIES: &[&str] = &[
    "title",
    "artist",
    "albumartist",
    "album",
    "track",
    "disc",
    "year",
    "genre",
    "duration",
    "file",
    "thumbnail",
    "dateadded",
    "displayartist",
];

/// Holds the password, so it deliberately has no `Debug`.
#[derive(Clone, Serialize, Deserialize)]
pub struct KodiConfig {
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

/// The address as typed, made usable: `http://` and Kodi's default port 8080
/// are added when missing, and a pasted `/jsonrpc` or web UI path is dropped.
pub fn normalize_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter the address of the Kodi device".to_string());
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("http://{input}")
    };
    let url = url::Url::parse(&with_scheme).map_err(|_| "That address isn't valid".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Use an http:// or https:// address".to_string());
    }
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "Enter the address of the Kodi device".to_string())?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let port = url.port().unwrap_or(if input.contains("://") {
        url.port_or_known_default().unwrap_or(8080)
    } else {
        8080
    });
    Ok(format!("{}://{host}:{port}", url.scheme()))
}

pub fn stable_key(config: &KodiConfig) -> String {
    let host = config
        .url
        .split_once("://")
        .map_or(config.url.as_str(), |(_, rest)| rest)
        .to_ascii_lowercase();
    format!("{}@{host}", config.username)
}

pub fn display_name(url: &str) -> String {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    format!("Kodi ({host})")
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Kodi lists artists and genres as arrays.
fn joined(value: &Value, key: &str) -> Option<String> {
    match value.get(key) {
        Some(Value::Array(items)) => {
            let parts: Vec<&str> = items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        _ => text(value, key),
    }
}

fn positive(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64).filter(|n| *n > 0)
}

fn song_to_meta(song: &Value) -> Option<RemoteTrackMeta> {
    let id = positive(song, "songid")?.to_string();
    let file = text(song, "file")?;
    let extension = file
        .rsplit(['/', '\\'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| crate::metadata::supported_audio_extensions().contains(ext))?;
    Some(RemoteTrackMeta {
        audio_fingerprint: None,
        remote_id: id,
        title: text(song, "title")
            .or_else(|| text(song, "label"))
            .unwrap_or_else(|| "Untitled".to_string()),
        // "A feat. B" as Kodi shows it, rather than the bare list of names.
        artist: text(song, "displayartist")
            .or_else(|| joined(song, "artist"))
            .unwrap_or_else(|| "Unknown Artist".to_string()),
        album_artist: joined(song, "albumartist"),
        album: text(song, "album").unwrap_or_else(|| "Unknown Album".to_string()),
        track_number: positive(song, "track").map(|n| n as i32),
        disc_number: positive(song, "disc").map(|n| n as i32),
        year: positive(song, "year").map(|n| n as i32),
        genre: joined(song, "genre"),
        duration_seconds: positive(song, "duration").map(|n| n as f64),
        size: 0,
        extension,
        part_key: file,
        art_ref: text(song, "thumbnail"),
        updated_at: text(song, "dateadded")
            .map(|d| d.replace(' ', "T"))
            .and_then(|d| unix_seconds(&d)),
    })
}

pub struct KodiBackend {
    config: KodiConfig,
    gate: &'static NetworkGate,
}

impl KodiBackend {
    pub fn new(config: KodiConfig) -> Self {
        Self::with_gate(config, NetworkGate::global())
    }

    pub fn with_gate(config: KodiConfig, gate: &'static NetworkGate) -> Self {
        Self { config, gate }
    }

    fn unreachable(&self) -> RemoteError {
        let host = url::Url::parse(&self.config.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default();
        RemoteError::Unreachable(format!("Can't reach {host}"))
    }

    fn authorized(&self, request: RequestBuilder) -> Result<RequestBuilder, RemoteError> {
        self.gate.check()?;
        if self.config.username.is_empty() {
            return Ok(request);
        }
        if !basic_auth_travels_safely(&self.config.url) {
            return Err(RemoteError::Insecure);
        }
        Ok(request.basic_auth(&self.config.username, Some(&self.config.password)))
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, RemoteError> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let resp = self
            .authorized(api_client().post(format!("{}/jsonrpc", self.config.url)))?
            .json(&body)
            .send()
            .map_err(|_| self.unreachable())?;
        match resp.status().as_u16() {
            401 | 403 => return Err(RemoteError::Unauthorized),
            200 => {}
            code => return Err(RemoteError::Http(code)),
        }
        let reply: Value = resp.json().map_err(|e| RemoteError::Parse(e.to_string()))?;
        if let Some(error) = reply.get("error") {
            return Err(RemoteError::Parse(
                text(error, "message").unwrap_or_else(|| "Kodi refused the request".into()),
            ));
        }
        reply
            .get("result")
            .cloned()
            .ok_or_else(|| RemoteError::Parse("no result".into()))
    }

    /// Checks the device and the login before the source is stored.
    pub fn probe(&self) -> Result<(), RemoteError> {
        match self.call("JSONRPC.Ping", json!({}))?.as_str() {
            Some("pong") => Ok(()),
            _ => Err(RemoteError::Parse("not Kodi".into())),
        }
    }

    /// The `/vfs/` address Kodi serves a library file from.
    fn file_url(&self, file: &str) -> Result<String, RemoteError> {
        let result = self.call("Files.PrepareDownload", json!({ "path": file }))?;
        let path = result
            .get("details")
            .and_then(|d| text(d, "path"))
            .ok_or_else(|| RemoteError::Parse("no download path".into()))?;
        Ok(format!(
            "{}/{}",
            self.config.url,
            path.trim_start_matches('/')
        ))
    }

    fn get(&self, url: &str, range: &str) -> Result<Response, RemoteError> {
        let resp = self
            .authorized(stream_client().get(url))?
            .header("Range", range)
            .send()
            .map_err(|_| self.unreachable())?;
        match resp.status().as_u16() {
            206 => Ok(resp),
            200 if range.starts_with("bytes=0-") => Ok(resp),
            401 | 403 => Err(RemoteError::Unauthorized),
            code => Err(RemoteError::Http(code)),
        }
    }
}

impl RemoteBackend for KodiBackend {
    fn kind(&self) -> &'static str {
        "kodi"
    }

    fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
        let result = self.call(
            "AudioLibrary.GetSongs",
            json!({
                "properties": SONG_PROPERTIES,
                // No sort: a sort with ties, such as by track number across
                // albums, could move a song between pages while paging.
                "limits": { "start": cursor, "end": cursor + PAGE },
            }),
        )?;
        let songs = result
            .get("songs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total = result
            .get("limits")
            .and_then(|l| l.get("total"))
            .and_then(Value::as_u64)
            .map(|t| t as usize);
        let reached = cursor + songs.len();
        let more = !songs.is_empty() && total.is_none_or(|total| reached < total);
        Ok(CatalogPage {
            tracks: songs.iter().filter_map(song_to_meta).collect(),
            total,
            next: more.then_some(reached),
        })
    }

    fn open_bytes(&self, part_key: &str, offset: u64) -> Result<Box<dyn ByteStream>, RemoteError> {
        let url = self.file_url(part_key)?;
        Ok(Box::new(self.get(&url, &format!("bytes={offset}-"))?))
    }

    fn check_reachable(&self) -> Result<(), RemoteError> {
        self.probe()
    }

    fn fetch_artwork(&self, art_ref: &str) -> Result<Vec<u8>, RemoteError> {
        let url = format!("{}/image/{}", self.config.url, percent_encode(art_ref));
        let resp = self.get(&url, "bytes=0-")?;
        let mut bytes = Vec::new();
        resp.take(MAX_ART_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|_| self.unreachable())?;
        Ok(bytes)
    }

    fn content_length(&self, part_key: &str) -> Result<u64, RemoteError> {
        let url = self.file_url(part_key)?;
        let resp = self.get(&url, "bytes=0-0")?;
        total_size(&resp).ok_or_else(|| RemoteError::Parse("no size".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::gate::GateState;
    use crate::remote::testkit::{fake_server, http};

    fn open_gate() -> &'static NetworkGate {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(true, true);
        gate
    }

    fn rpc(result: &str) -> String {
        let body = format!(r#"{{"id":1,"jsonrpc":"2.0","result":{result}}}"#);
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn backend_for(port: u16) -> KodiBackend {
        KodiBackend::with_gate(
            KodiConfig {
                url: format!("http://127.0.0.1:{port}"),
                username: "kodi".into(),
                password: "kodi".into(),
            },
            open_gate(),
        )
    }

    #[test]
    fn addresses_get_a_scheme_and_the_default_port() {
        assert_eq!(
            normalize_url("192.168.1.30").unwrap(),
            "http://192.168.1.30:8080"
        );
        assert_eq!(
            normalize_url("livingroom:8081/jsonrpc").unwrap(),
            "http://livingroom:8081"
        );
        assert_eq!(
            normalize_url("https://kodi.example.com").unwrap(),
            "https://kodi.example.com:443"
        );
        assert!(normalize_url("").is_err());
        assert!(normalize_url("ftp://kodi").is_err());
    }

    #[test]
    fn songs_map_to_library_rows() {
        let song = json!({
            "songid": 42, "label": "Song", "title": "Song",
            "artist": ["Band", "Guest"], "albumartist": ["Band"], "album": "Album",
            "track": 3, "disc": 0, "year": 1999, "genre": ["Rock"], "duration": 201,
            "file": "nfs://nas/music/Band/Album/03 Song.flac",
            "thumbnail": "image://nfs%3a%2f%2fnas%2fcover.jpg/",
            "dateadded": "2023-01-02 10:11:12"
        });
        let meta = song_to_meta(&song).unwrap();
        assert_eq!(meta.remote_id, "42");
        assert_eq!(meta.artist, "Band, Guest");
        let mut credited = song.clone();
        credited["displayartist"] = json!("Band feat. Guest");
        assert_eq!(song_to_meta(&credited).unwrap().artist, "Band feat. Guest");
        assert_eq!(meta.album_artist.as_deref(), Some("Band"));
        assert_eq!(meta.track_number, Some(3));
        assert_eq!(meta.disc_number, None);
        assert_eq!(meta.extension, "flac");
        assert_eq!(meta.part_key, "nfs://nas/music/Band/Album/03 Song.flac");
        assert_eq!(meta.updated_at, unix_seconds("2023-01-02T10:11:12Z"));
        // An internet radio entry or a format we can't play is left out.
        assert!(song_to_meta(&json!({"songid": 7, "file": "http://radio/stream"})).is_none());
    }

    #[test]
    fn the_catalog_is_read_in_pages() {
        let (port, seen) = fake_server(vec![rpc(
            r#"{"limits":{"start":0,"end":1,"total":2},"songs":[{"songid":1,"title":"One","file":"/m/1.mp3"}]}"#,
        )]);
        let page = backend_for(port).catalog_page(0).unwrap();
        assert_eq!(page.tracks.len(), 1);
        assert_eq!(page.total, Some(2));
        assert_eq!(page.next, Some(1));
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("POST /jsonrpc "));
        assert!(
            log[0].contains("authorization: Basic a29kaTprb2Rp"),
            "{}",
            log[0]
        );
    }

    #[test]
    fn audio_comes_from_the_prepared_download_path() {
        let (port, seen) = fake_server(vec![
            rpc(r#"{"details":{"path":"vfs/%2fm%2f1.mp3"},"mode":"redirect","protocol":"http"}"#),
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/5555\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx".to_string(),
            rpc(r#"{"details":{"path":"vfs/%2fm%2f1.mp3"},"mode":"redirect","protocol":"http"}"#),
            http("206 Partial Content", "abc"),
        ]);
        let backend = backend_for(port);
        assert_eq!(backend.content_length("/m/1.mp3").unwrap(), 5555);
        let mut body = Vec::new();
        backend
            .open_bytes("/m/1.mp3", 9)
            .unwrap()
            .read_to_end(&mut body)
            .unwrap();
        assert_eq!(body, b"abc");
        let log = seen.lock().unwrap();
        assert!(log[1].starts_with("GET /vfs/%2fm%2f1.mp3 "), "{}", log[1]);
        assert!(log[3].to_ascii_lowercase().contains("range: bytes=9-\r\n"));
    }

    #[test]
    fn a_refused_login_is_unauthorized() {
        let (port, _) = fake_server(vec![http("401 Unauthorized", "")]);
        assert_eq!(
            backend_for(port).probe().err(),
            Some(RemoteError::Unauthorized)
        );
    }

    #[test]
    fn a_closed_gate_blocks_every_call() {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        let (port, seen) = fake_server(vec![rpc(r#""pong""#)]);
        let backend = KodiBackend::with_gate(
            KodiConfig {
                url: format!("http://127.0.0.1:{port}"),
                username: String::new(),
                password: String::new(),
            },
            gate,
        );
        assert_eq!(
            backend.probe().err(),
            Some(RemoteError::Blocked(GateState::Disabled))
        );
        assert!(seen.lock().unwrap().is_empty());
    }
}
