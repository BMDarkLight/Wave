// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Subsonic and OpenSubsonic servers: Navidrome, Airsonic-Advanced, Gonic,
//! Ampache, Funkwhale, LMS, Supysonic and others that speak the same API.
//! Signed in with a username and password on the server itself; no account
//! anywhere else.

pub mod api;
pub mod md5;

use crate::remote::backend::{ByteStream, CatalogPage, RemoteBackend, RemoteError};
use crate::remote::gate::NetworkGate;
use crate::remote::http::{api_client, stream_client};
use crate::remote::uri::percent_encode;
use api::{parse_response, song_to_meta, ApiFailure, Envelope, TOKEN_UNSUPPORTED};
use reqwest::blocking::Response;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

/// The Subsonic API version Wave speaks; 1.13 added salted tokens.
const API_VERSION: &str = "1.16.1";
/// Songs per catalog page.
const PAGE: usize = 500;
/// Albums per page when a server has to be walked album by album.
const ALBUM_BATCH: usize = 50;
/// Catalog cursors at or above this walk albums instead of searching. It fits
/// a 32-bit `usize` and is far past any song offset a search reaches.
const ALBUM_MODE: usize = 1 << 30;

/// Everything needed to talk to one Subsonic server, stored as `config_json`.
/// Holds the password, so it deliberately has no `Debug`.
#[derive(Clone, Serialize, Deserialize)]
pub struct SubsonicConfig {
    pub url: String,
    pub username: String,
    pub password: String,
    /// The server can't check salted tokens, so the password goes instead.
    #[serde(default)]
    pub plain_password: bool,
    #[serde(default)]
    pub server_type: Option<String>,
}

/// The address as typed, made usable: a scheme is added when missing, and a
/// trailing `/` or `/rest` is dropped.
pub fn normalize_url(input: &str) -> Result<String, String> {
    // A pasted web UI address ends in a fragment (Navidrome's /app/#/album).
    let input = input.split('#').next().unwrap_or_default();
    let mut url = input.trim().trim_end_matches('/').to_string();
    for suffix in ["/rest", "/app"] {
        if let Some(stripped) = url.strip_suffix(suffix) {
            url = stripped.trim_end_matches('/').to_string();
        }
    }
    if url.is_empty() {
        return Err("Enter the server's address".to_string());
    }
    match url.split_once("://") {
        Some((scheme, rest)) => {
            if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
                return Err("Use an http:// or https:// address".to_string());
            }
            if rest.is_empty() {
                return Err("Enter the server's address".to_string());
            }
            Ok(url)
        }
        None => Ok(format!("http://{url}")),
    }
}

/// The URL without its scheme.
fn without_scheme(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, rest)| rest)
}

/// Identifies the same account on the same server, so adding it twice is
/// refused and re-adding it revives its tracks.
pub fn stable_key(config: &SubsonicConfig) -> String {
    format!(
        "{}@{}",
        config.username,
        without_scheme(&config.url).to_ascii_lowercase()
    )
}

/// "Navidrome (music.example.com)", or "Subsonic (...)" when the server
/// doesn't say what it is.
pub fn display_name(server_type: Option<&str>, url: &str) -> String {
    let kind = server_type
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| {
            let mut chars = t.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .unwrap_or_else(|| "Subsonic".to_string());
    let host = without_scheme(url).split('/').next().unwrap_or_default();
    format!("{kind} ({host})")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A JSON or XML body where audio or an image was expected is an API error.
fn is_api_body(resp: &Response) -> bool {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("json") || ct.contains("xml"))
}

pub struct SubsonicBackend {
    config: SubsonicConfig,
    gate: &'static NetworkGate,
    plain: AtomicBool,
    total: parking_lot::Mutex<Option<usize>>,
}

impl SubsonicBackend {
    pub fn new(config: SubsonicConfig) -> Self {
        Self::with_gate(config, NetworkGate::global())
    }

    pub fn with_gate(config: SubsonicConfig, gate: &'static NetworkGate) -> Self {
        let plain = AtomicBool::new(config.plain_password);
        Self {
            config,
            gate,
            plain,
            total: parking_lot::Mutex::new(None),
        }
    }

    /// Checks the server and the credentials. Returns the config to store,
    /// with what the server said it is and how it wants the password.
    pub fn probe(&self) -> Result<SubsonicConfig, RemoteError> {
        let envelope = self.call("ping", &[])?;
        let mut config = self.config.clone();
        config.server_type = envelope.server_type;
        config.plain_password = self.plain.load(Ordering::Relaxed);
        Ok(config)
    }

    fn unreachable(&self) -> RemoteError {
        let host = without_scheme(&self.config.url)
            .split('/')
            .next()
            .unwrap_or_default();
        RemoteError::Unreachable(format!("Can't reach {host}"))
    }

    /// The URL of an API endpoint, signed in. The password never appears in
    /// it as text: it is a salted token, or hex-encoded when the server can't
    /// check tokens.
    fn url(&self, endpoint: &str, params: &[(&str, &str)]) -> String {
        let mut query: Vec<String> = params
            .iter()
            .map(|(k, v)| format!("{k}={}", percent_encode(v)))
            .collect();
        query.push(format!("u={}", percent_encode(&self.config.username)));
        if self.plain.load(Ordering::Relaxed) {
            query.push(format!("p=enc:{}", hex(self.config.password.as_bytes())));
        } else {
            let salt: String = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
            let token = md5::md5_hex(format!("{}{salt}", self.config.password).as_bytes());
            query.push(format!("t={token}"));
            query.push(format!("s={salt}"));
        }
        query.push(format!("v={API_VERSION}"));
        query.push("c=Wave".to_string());
        query.push("f=json".to_string());
        format!("{}/rest/{endpoint}?{}", self.config.url, query.join("&"))
    }

    /// Switches to sending the password when the server can't check tokens.
    /// Returns whether a retry is worth it.
    fn fall_back_to_password(&self, failure: &ApiFailure) -> bool {
        matches!(failure, ApiFailure::Code(TOKEN_UNSUPPORTED, _))
            && !self.plain.swap(true, Ordering::Relaxed)
    }

    fn call(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<Envelope, RemoteError> {
        self.gate.check()?;
        loop {
            let resp = api_client()
                .get(self.url(endpoint, params))
                .header("Accept", "application/json")
                .send()
                .map_err(|_| self.unreachable())?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(RemoteError::Unauthorized);
            }
            if !status.is_success() {
                return Err(RemoteError::Http(status.as_u16()));
            }
            let body = resp.text().map_err(|_| self.unreachable())?;
            match parse_response(&body) {
                Ok(envelope) => return Ok(envelope),
                Err(failure) if self.fall_back_to_password(&failure) => continue,
                Err(failure) => return Err(failure.into()),
            }
        }
    }

    /// Searches with an empty query, which most servers answer with every
    /// song.
    fn search_page(&self, offset: usize) -> Result<Vec<api::Song>, RemoteError> {
        let offset = offset.to_string();
        let count = PAGE.to_string();
        Ok(self
            .call(
                "search3",
                &[
                    ("query", ""),
                    ("songCount", &count),
                    ("songOffset", &offset),
                    ("artistCount", "0"),
                    ("albumCount", "0"),
                ],
            )?
            .search_songs)
    }

    /// One batch of albums, alphabetically, with their songs.
    fn album_page(&self, album_offset: usize) -> Result<(Vec<api::Song>, bool), RemoteError> {
        let offset = album_offset.to_string();
        let size = ALBUM_BATCH.to_string();
        let albums = self
            .call(
                "getAlbumList2",
                &[
                    ("type", "alphabeticalByName"),
                    ("size", &size),
                    ("offset", &offset),
                ],
            )?
            .albums;
        let mut songs = Vec::new();
        for album in &albums {
            songs.extend(self.call("getAlbum", &[("id", &album.id)])?.album_songs);
        }
        Ok((songs, albums.len() == ALBUM_BATCH))
    }

    /// Opens a binary endpoint, turning an API error in its place into the
    /// matching error.
    fn open_binary(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
        offset: Option<u64>,
    ) -> Result<Response, RemoteError> {
        self.gate.check()?;
        loop {
            let mut request = stream_client().get(self.url(endpoint, params));
            if let Some(offset) = offset {
                request = request.header("Range", format!("bytes={offset}-"));
            }
            let resp = request.send().map_err(|_| self.unreachable())?;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(RemoteError::Unauthorized);
            }
            if status.is_success() && is_api_body(&resp) {
                let body = resp.text().map_err(|_| self.unreachable())?;
                match parse_response(&body) {
                    Err(failure) if self.fall_back_to_password(&failure) => continue,
                    Err(failure) => return Err(failure.into()),
                    Ok(_) => return Err(RemoteError::Parse("No audio in the response".into())),
                }
            }
            return match (status.as_u16(), offset) {
                (206, _) => Ok(resp),
                (200, None | Some(0)) => Ok(resp),
                (code, _) => Err(RemoteError::Http(code)),
            };
        }
    }
}

impl RemoteBackend for SubsonicBackend {
    fn kind(&self) -> &'static str {
        "subsonic"
    }

    fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
        self.gate.check()?;
        // The song count lets sync show progress; not every server reports
        // it. Measured once, when a walk starts.
        if cursor == 0 {
            *self.total.lock() = self
                .call("getScanStatus", &[])
                .ok()
                .and_then(|envelope| envelope.scan_count)
                .filter(|count| *count > 0);
        }
        let total = *self.total.lock();
        let (songs, next) = if cursor >= ALBUM_MODE {
            let offset = cursor - ALBUM_MODE;
            let (songs, more) = self.album_page(offset)?;
            (songs, more.then_some(cursor + ALBUM_BATCH))
        } else {
            let songs = self.search_page(cursor)?;
            if cursor == 0 && songs.is_empty() {
                // This server won't list everything for an empty search.
                let (songs, more) = self.album_page(0)?;
                (songs, more.then_some(ALBUM_MODE + ALBUM_BATCH))
            } else {
                let more = songs.len() == PAGE;
                (songs, more.then_some(cursor + PAGE))
            }
        };
        Ok(CatalogPage {
            tracks: songs.iter().filter_map(song_to_meta).collect(),
            total,
            next,
        })
    }

    fn open_bytes(&self, part_key: &str, offset: u64) -> Result<Box<dyn ByteStream>, RemoteError> {
        let resp = self.open_binary(
            "stream",
            &[("id", part_key), ("format", "raw")],
            Some(offset),
        )?;
        Ok(Box::new(resp))
    }

    fn check_reachable(&self) -> Result<(), RemoteError> {
        self.call("ping", &[]).map(|_| ())
    }

    fn fetch_artwork(&self, art_ref: &str) -> Result<Vec<u8>, RemoteError> {
        let resp = self.open_binary("getCoverArt", &[("id", art_ref), ("size", "600")], None)?;
        resp.bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|_| self.unreachable())
    }
}

#[cfg(test)]
mod tests {
    use super::md5::md5_hex;
    use super::*;
    use crate::remote::gate::GateState;
    use crate::remote::testkit::{fake_server, http};
    use std::io::Read;

    fn open_gate() -> &'static NetworkGate {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(true, true);
        gate
    }

    fn json(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn ok(extra: &str) -> String {
        json(&format!(
            r#"{{"subsonic-response":{{"status":"ok","version":"1.16.1","type":"navidrome"{extra}}}}}"#
        ))
    }

    fn failed(code: u32) -> String {
        json(&format!(
            r#"{{"subsonic-response":{{"status":"failed","version":"1.16.1","error":{{"code":{code},"message":"nope"}}}}}}"#
        ))
    }

    fn backend_for(port: u16) -> SubsonicBackend {
        SubsonicBackend::with_gate(
            SubsonicConfig {
                url: format!("http://127.0.0.1:{port}"),
                username: "alice".into(),
                password: "sesame".into(),
                plain_password: false,
                server_type: None,
            },
            open_gate(),
        )
    }

    /// The value of `name` in a recorded request line's query.
    fn param(request: &str, name: &str) -> Option<String> {
        let line = request.lines().next()?;
        let query = line.split_whitespace().nth(1)?.split_once('?')?.1;
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")).map(str::to_string))
    }

    #[test]
    fn addresses_get_a_scheme_and_lose_rest() {
        assert_eq!(
            normalize_url("music.example.com").unwrap(),
            "http://music.example.com"
        );
        assert_eq!(
            normalize_url("https://music.example.com/rest/").unwrap(),
            "https://music.example.com"
        );
        assert_eq!(
            normalize_url("http://nas:4533/").unwrap(),
            "http://nas:4533"
        );
        assert_eq!(
            normalize_url("http://nas:4533/app/#/album/all").unwrap(),
            "http://nas:4533"
        );
        assert!(normalize_url(" ").is_err());
        assert!(normalize_url("ftp://nas").is_err());
    }

    #[test]
    fn names_and_keys_come_from_the_server_and_account() {
        assert_eq!(
            display_name(Some("navidrome"), "https://music.example.com"),
            "Navidrome (music.example.com)"
        );
        assert_eq!(display_name(None, "http://nas:4040"), "Subsonic (nas:4040)");
        let config = SubsonicConfig {
            url: "https://Music.Example.com".into(),
            username: "alice".into(),
            password: "x".into(),
            plain_password: false,
            server_type: None,
        };
        assert_eq!(stable_key(&config), "alice@music.example.com");
    }

    #[test]
    fn probe_signs_in_with_a_salted_token() {
        let (port, seen) = fake_server(vec![ok("")]);
        let config = backend_for(port).probe().unwrap();
        assert_eq!(config.server_type.as_deref(), Some("navidrome"));
        assert!(!config.plain_password);
        let log = seen.lock().unwrap();
        let request = &log[0];
        assert!(request.starts_with("GET /rest/ping?"), "{request}");
        assert_eq!(param(request, "u").as_deref(), Some("alice"));
        assert_eq!(param(request, "v").as_deref(), Some("1.16.1"));
        assert_eq!(param(request, "c").as_deref(), Some("Wave"));
        assert_eq!(param(request, "f").as_deref(), Some("json"));
        let salt = param(request, "s").unwrap();
        assert!(salt.len() >= 6);
        assert_eq!(
            param(request, "t").unwrap(),
            md5_hex(format!("sesame{salt}").as_bytes())
        );
        assert!(param(request, "p").is_none());
    }

    #[test]
    fn a_server_without_token_support_gets_the_password_instead() {
        let (port, seen) = fake_server(vec![failed(41), ok("")]);
        let config = backend_for(port).probe().unwrap();
        assert!(config.plain_password);
        let log = seen.lock().unwrap();
        assert_eq!(param(&log[1], "p").as_deref(), Some("enc:736573616d65"));
        assert!(param(&log[1], "t").is_none());
    }

    #[test]
    fn wrong_credentials_are_unauthorized() {
        let (port, _) = fake_server(vec![failed(40)]);
        assert_eq!(
            backend_for(port).probe().err(),
            Some(RemoteError::Unauthorized)
        );
    }

    #[test]
    fn the_catalog_is_read_with_an_empty_search() {
        let songs = r#","searchResult3":{"song":[
            {"id":"s1","title":"One","suffix":"flac","size":10},
            {"id":"s2","title":"Two","suffix":"mp3","size":20}]}"#;
        let (port, seen) = fake_server(vec![
            ok(r#","scanStatus":{"scanning":false,"count":2}"#),
            ok(songs),
        ]);
        let page = backend_for(port).catalog_page(0).unwrap();
        assert_eq!(page.tracks.len(), 2);
        assert_eq!(page.total, Some(2));
        assert_eq!(page.next, None);
        let log = seen.lock().unwrap();
        let search = log
            .iter()
            .find(|r| r.starts_with("GET /rest/search3?"))
            .unwrap();
        assert_eq!(param(search, "query").as_deref(), Some(""));
        assert_eq!(param(search, "songCount").as_deref(), Some("500"));
        assert_eq!(param(search, "songOffset").as_deref(), Some("0"));
    }

    #[test]
    fn a_full_page_asks_for_the_next_one() {
        let many: Vec<String> = (0..500)
            .map(|i| format!(r#"{{"id":"s{i}","title":"T{i}","suffix":"mp3","size":1}}"#))
            .collect();
        let songs = format!(r#","searchResult3":{{"song":[{}]}}"#, many.join(","));
        let (port, _) = fake_server(vec![failed(0), ok(&songs)]);
        let page = backend_for(port).catalog_page(0).unwrap();
        assert_eq!(page.tracks.len(), 500);
        assert_eq!(page.next, Some(500));
    }

    #[test]
    fn a_server_whose_empty_search_finds_nothing_is_walked_by_album() {
        let (port, seen) = fake_server(vec![
            failed(0),
            ok(r#","searchResult3":{}"#),
            ok(r#","albumList2":{"album":[{"id":"a1"}]}"#),
            ok(
                r#","album":{"id":"a1","song":[{"id":"s9","title":"Nine","suffix":"ogg","size":5}]}"#,
            ),
        ]);
        let page = backend_for(port).catalog_page(0).unwrap();
        assert_eq!(page.tracks.len(), 1);
        assert_eq!(page.tracks[0].remote_id, "s9");
        assert_eq!(page.next, None);
        let log = seen.lock().unwrap();
        let list = log
            .iter()
            .find(|r| r.starts_with("GET /rest/getAlbumList2?"))
            .unwrap();
        assert_eq!(param(list, "type").as_deref(), Some("alphabeticalByName"));
        assert!(log.iter().any(|r| r.starts_with("GET /rest/getAlbum?")));
    }

    #[test]
    fn audio_is_the_original_file_with_a_range() {
        let (port, seen) = fake_server(vec![http("206 Partial Content", "abc")]);
        let mut stream = backend_for(port).open_bytes("s1", 10).unwrap();
        let mut body = Vec::new();
        stream.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"abc");
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("GET /rest/stream?"), "{}", log[0]);
        assert_eq!(param(&log[0], "id").as_deref(), Some("s1"));
        assert_eq!(param(&log[0], "format").as_deref(), Some("raw"));
        assert!(log[0].to_ascii_lowercase().contains("range: bytes=10-\r\n"));
    }

    #[test]
    fn an_error_in_place_of_audio_is_reported() {
        let (port, _) = fake_server(vec![failed(40)]);
        assert_eq!(
            backend_for(port).open_bytes("s1", 0).err(),
            Some(RemoteError::Unauthorized)
        );
        let (port, _) = fake_server(vec![http("200 OK", "whole file")]);
        assert_eq!(
            backend_for(port).open_bytes("s1", 5).err(),
            Some(RemoteError::Http(200))
        );
    }

    #[test]
    fn a_closed_gate_blocks_every_call() {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        let (port, seen) = fake_server(vec![ok("")]);
        let backend = SubsonicBackend::with_gate(
            SubsonicConfig {
                url: format!("http://127.0.0.1:{port}"),
                username: "a".into(),
                password: "b".into(),
                plain_password: false,
                server_type: None,
            },
            gate,
        );
        assert_eq!(
            backend.catalog_page(0).err(),
            Some(RemoteError::Blocked(GateState::Disabled))
        );
        assert!(backend.open_bytes("s1", 0).is_err());
        assert!(backend.probe().is_err());
        assert!(seen.lock().unwrap().is_empty());
    }
}
