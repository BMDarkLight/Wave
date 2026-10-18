// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Jellyfin and Emby servers, which share one API. Signed in with a username
//! and password on the server itself; Wave keeps the access token it gets back,
//! never the password.

pub mod api;

use crate::remote::backend::{ByteStream, CatalogPage, RemoteBackend, RemoteError};
use crate::remote::gate::NetworkGate;
use crate::remote::http::{api_client, stream_client};
use crate::remote::uri::percent_encode;
use api::{item_to_meta, AuthResult, PublicInfo, View};
use reqwest::blocking::{RequestBuilder, Response};
use serde::{Deserialize, Serialize};

/// Tracks per catalog page.
const PAGE: usize = 500;
/// Catalog cursors are `library index * CURSOR_STRIDE + offset`. It fits a
/// 32-bit `usize` with room for 16 libraries.
const CURSOR_STRIDE: usize = 1 << 28;
/// The port both servers listen on out of the box.
const DEFAULT_PORT: u16 = 8096;

/// A music library chosen for a source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JellyfinLibrary {
    pub id: String,
    pub name: String,
}

/// Everything needed to talk to one server as one user, stored as
/// `config_json`. Holds the access token, so it deliberately has no `Debug`.
#[derive(Clone, Serialize, Deserialize)]
pub struct JellyfinConfig {
    pub url: String,
    pub server_id: String,
    pub server_name: String,
    /// "Jellyfin" or "Emby".
    pub product: String,
    pub user_id: String,
    pub username: String,
    pub token: String,
    /// The server ties a sign-in to this id, and signing in again with the
    /// same id replaces the old token rather than piling up sessions.
    pub device_id: String,
    #[serde(default)]
    pub libraries: Vec<JellyfinLibrary>,
}

/// The address as typed, made usable: a bare host gets `http://` and port
/// 8096, and a pasted web UI path (`/web/#/home`) is dropped.
pub fn normalize_url(input: &str) -> Result<String, String> {
    let input = input.split('#').next().unwrap_or_default();
    let mut url = input.trim().trim_end_matches('/').to_string();
    for suffix in ["/web/index.html", "/web"] {
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
        None => {
            let host = url.split('/').next().unwrap_or_default();
            // A bracketed IPv6 address has colons of its own.
            let has_port = match host.rsplit_once(']') {
                Some((_, after)) => after.starts_with(':'),
                None => host.contains(':'),
            };
            if has_port || host != url {
                Ok(format!("http://{url}"))
            } else {
                Ok(format!("http://{url}:{DEFAULT_PORT}"))
            }
        }
    }
}

/// Identifies the same user on the same server, so adding it twice is refused
/// and re-adding it revives its tracks.
pub fn stable_key(config: &JellyfinConfig) -> String {
    format!("{}:{}", config.server_id, config.user_id)
}

/// "Home (Jellyfin)".
pub fn display_name(config: &JellyfinConfig) -> String {
    format!("{} ({})", config.server_name, config.product)
}

fn quoted(value: &str) -> String {
    value.replace(['"', ','], "")
}

/// The `MediaBrowser` authorization value both servers read, with the token
/// once there is one.
fn authorization(device_id: &str, token: Option<&str>) -> String {
    let mut value = format!(
        "MediaBrowser Client=\"Wave\", Device=\"Wave\", DeviceId=\"{}\", Version=\"{}\"",
        quoted(device_id),
        env!("CARGO_PKG_VERSION")
    );
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        value.push_str(&format!(", Token=\"{}\"", quoted(token)));
    }
    value
}

/// Adds the authorization in every form the two servers accept: Jellyfin
/// reads `Authorization`, Emby `X-Emby-Authorization` and `X-Emby-Token`.
fn signed(request: RequestBuilder, device_id: &str, token: Option<&str>) -> RequestBuilder {
    let value = authorization(device_id, token);
    let request = request
        .header("Authorization", &value)
        .header("X-Emby-Authorization", &value);
    match token.filter(|t| !t.is_empty()) {
        Some(token) => request.header("X-Emby-Token", token),
        None => request,
    }
}

fn host_of(url: &str) -> &str {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
}

fn unreachable_at(url: &str) -> RemoteError {
    RemoteError::Unreachable(format!("Can't reach {}", host_of(url)))
}

fn check_status(resp: Response) -> Result<Response, RemoteError> {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(RemoteError::Unauthorized);
    }
    if !status.is_success() {
        return Err(RemoteError::Http(status.as_u16()));
    }
    Ok(resp)
}

/// Reads `/System/Info/Public`, which needs no sign-in.
pub fn public_info(gate: &NetworkGate, url: &str) -> Result<PublicInfo, RemoteError> {
    gate.check()?;
    let resp = api_client()
        .get(format!("{url}/System/Info/Public"))
        .header("Accept", "application/json")
        .send()
        .map_err(|_| unreachable_at(url))?;
    let body = check_status(resp)?
        .text()
        .map_err(|_| unreachable_at(url))?;
    api::parse_public_info(&body)
}

/// Signs in with a username and password. A wrong password is
/// [`RemoteError::Unauthorized`].
pub fn authenticate(
    gate: &NetworkGate,
    url: &str,
    device_id: &str,
    username: &str,
    password: &str,
) -> Result<AuthResult, RemoteError> {
    gate.check()?;
    let request = api_client()
        .post(format!("{url}/Users/AuthenticateByName"))
        .header("Accept", "application/json")
        .json(&serde_json::json!({ "Username": username, "Pw": password }));
    let resp = signed(request, device_id, None)
        .send()
        .map_err(|_| unreachable_at(url))?;
    let body = check_status(resp)?
        .text()
        .map_err(|_| unreachable_at(url))?;
    api::parse_auth(&body)
}

pub struct JellyfinBackend {
    config: JellyfinConfig,
    gate: &'static NetworkGate,
    total: parking_lot::Mutex<Option<usize>>,
}

impl JellyfinBackend {
    pub fn new(config: JellyfinConfig) -> Self {
        Self::with_gate(config, NetworkGate::global())
    }

    pub fn with_gate(config: JellyfinConfig, gate: &'static NetworkGate) -> Self {
        Self {
            config,
            gate,
            total: parking_lot::Mutex::new(None),
        }
    }

    fn unreachable(&self) -> RemoteError {
        unreachable_at(&self.config.url)
    }

    fn get(&self, path_and_query: &str, client: &reqwest::blocking::Client) -> RequestBuilder {
        signed(
            client.get(format!("{}{path_and_query}", self.config.url)),
            &self.config.device_id,
            Some(&self.config.token),
        )
    }

    fn get_json(&self, path_and_query: &str) -> Result<String, RemoteError> {
        self.gate.check()?;
        let resp = self
            .get(path_and_query, api_client())
            .header("Accept", "application/json")
            .send()
            .map_err(|_| self.unreachable())?;
        check_status(resp)?.text().map_err(|_| self.unreachable())
    }

    /// The user's music libraries. Jellyfin 10.9 moved the list to
    /// `/UserViews`; Emby and older Jellyfin answer at `/Users/{id}/Views`.
    pub fn music_libraries(&self) -> Result<Vec<View>, RemoteError> {
        let user = percent_encode(&self.config.user_id);
        let body = match self.get_json(&format!("/Users/{user}/Views")) {
            Err(RemoteError::Http(404)) => self.get_json(&format!("/UserViews?userId={user}"))?,
            other => other?,
        };
        api::parse_music_views(&body)
    }

    fn items_path(&self, library: &str, start: usize, limit: usize) -> String {
        format!(
            "/Items?userId={}&ParentId={}&IncludeItemTypes=Audio&Recursive=true\
             &Fields=MediaSources,Path,Genres,DateCreated&SortBy=SortName&SortOrder=Ascending\
             &EnableImageTypes=Primary&StartIndex={start}&Limit={limit}",
            percent_encode(&self.config.user_id),
            percent_encode(library),
        )
    }

    /// How many tracks the chosen libraries hold, for sync progress.
    fn measure_catalog(&self) -> Option<usize> {
        let mut total = 0;
        for library in &self.config.libraries {
            let body = self.get_json(&self.items_path(&library.id, 0, 0)).ok()?;
            total += api::parse_items(&body).ok()?.total_record_count?;
        }
        Some(total)
    }

    fn open(&self, path_and_query: &str, offset: Option<u64>) -> Result<Response, RemoteError> {
        self.gate.check()?;
        let mut request = self.get(path_and_query, stream_client());
        if let Some(offset) = offset {
            request = request.header("Range", format!("bytes={offset}-"));
        }
        let resp = request.send().map_err(|_| self.unreachable())?;
        let resp = check_status(resp)?;
        match (resp.status().as_u16(), offset) {
            (206, _) | (200, None | Some(0)) => Ok(resp),
            (code, _) => Err(RemoteError::Http(code)),
        }
    }
}

impl RemoteBackend for JellyfinBackend {
    fn kind(&self) -> &'static str {
        "jellyfin"
    }

    fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
        self.gate.check()?;
        if cursor == 0 {
            *self.total.lock() = self.measure_catalog();
        }
        let total = *self.total.lock();
        let index = cursor / CURSOR_STRIDE;
        let offset = cursor % CURSOR_STRIDE;
        let Some(library) = self.config.libraries.get(index) else {
            return Ok(CatalogPage {
                tracks: Vec::new(),
                total,
                next: None,
            });
        };
        let body = self.get_json(&self.items_path(&library.id, offset, PAGE))?;
        let page = api::parse_items(&body)?;
        let consumed = offset + page.items.len();
        if consumed >= CURSOR_STRIDE {
            return Err(RemoteError::Parse("library too large to page".to_string()));
        }
        let more_here = !page.items.is_empty()
            && match page.total_record_count {
                Some(count) => consumed < count,
                None => page.items.len() >= PAGE,
            };
        let next = if more_here {
            Some(index * CURSOR_STRIDE + consumed)
        } else if index + 1 < self.config.libraries.len() {
            Some((index + 1) * CURSOR_STRIDE)
        } else {
            None
        };
        Ok(CatalogPage {
            tracks: page.items.iter().filter_map(item_to_meta).collect(),
            total,
            next,
        })
    }

    fn open_bytes(&self, part_key: &str, offset: u64) -> Result<Box<dyn ByteStream>, RemoteError> {
        // `static=true` sends the original file, never a transcode.
        let resp = self.open(
            &format!("/Audio/{}/stream?static=true", percent_encode(part_key)),
            Some(offset),
        )?;
        Ok(Box::new(resp))
    }

    fn check_reachable(&self) -> Result<(), RemoteError> {
        self.get_json("/System/Info").map(|_| ())
    }

    fn fetch_artwork(&self, art_ref: &str) -> Result<Vec<u8>, RemoteError> {
        let resp = self.open(
            &format!(
                "/Items/{}/Images/Primary?maxWidth=600&maxHeight=600",
                percent_encode(art_ref)
            ),
            None,
        )?;
        resp.bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|_| self.unreachable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::gate::GateState;
    use crate::remote::testkit::{fake_server, http};
    use std::io::Read;

    fn open_gate() -> &'static NetworkGate {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(true, true);
        gate
    }

    fn config_for(port: u16, libraries: &[&str]) -> JellyfinConfig {
        JellyfinConfig {
            url: format!("http://127.0.0.1:{port}"),
            server_id: "srv".into(),
            server_name: "Home".into(),
            product: "Jellyfin".into(),
            user_id: "u1".into(),
            username: "alice".into(),
            token: "tok".into(),
            device_id: "dev".into(),
            libraries: libraries
                .iter()
                .map(|id| JellyfinLibrary {
                    id: id.to_string(),
                    name: id.to_string(),
                })
                .collect(),
        }
    }

    fn item(id: &str) -> String {
        format!(r#"{{"Id":"{id}","Name":"{id}","MediaSources":[{{"Container":"mp3","Size":3}}]}}"#)
    }

    fn page(ids: &[&str], total: usize) -> String {
        let items: Vec<String> = ids.iter().map(|id| item(id)).collect();
        http(
            "200 OK",
            &format!(
                r#"{{"Items":[{}],"TotalRecordCount":{total}}}"#,
                items.join(",")
            ),
        )
    }

    fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
        request.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    #[test]
    fn normalize_url_adds_scheme_and_port() {
        assert_eq!(normalize_url("nas").unwrap(), "http://nas:8096");
        assert_eq!(normalize_url("nas:8920").unwrap(), "http://nas:8920");
        assert_eq!(normalize_url("[::1]").unwrap(), "http://[::1]:8096");
        assert_eq!(normalize_url("[::1]:9000").unwrap(), "http://[::1]:9000");
        assert_eq!(
            normalize_url("https://media.example.com/jellyfin/").unwrap(),
            "https://media.example.com/jellyfin"
        );
        assert_eq!(
            normalize_url("http://nas:8096/web/#/home.html").unwrap(),
            "http://nas:8096"
        );
        assert_eq!(
            normalize_url("http://nas:8096/web/index.html#!/home").unwrap(),
            "http://nas:8096"
        );
        assert!(normalize_url("  ").is_err());
        assert!(normalize_url("ftp://nas").is_err());
    }

    #[test]
    fn names_and_keys() {
        let config = config_for(1, &[]);
        assert_eq!(stable_key(&config), "srv:u1");
        assert_eq!(display_name(&config), "Home (Jellyfin)");
    }

    #[test]
    fn authorization_carries_the_token_once_signed_in() {
        let before = authorization("dev", None);
        assert!(before.starts_with("MediaBrowser Client=\"Wave\""));
        assert!(before.contains("DeviceId=\"dev\""));
        assert!(!before.contains("Token="));
        assert!(authorization("dev", Some("tok")).ends_with(", Token=\"tok\""));
        assert!(!authorization("d\"e,v", None).contains("d\"e"));
    }

    #[test]
    fn authenticate_posts_credentials_and_reads_the_token() {
        let (port, seen) = fake_server(vec![http(
            "200 OK",
            r#"{"User":{"Id":"u1"},"AccessToken":"tok","ServerId":"srv"}"#,
        )]);
        let auth = authenticate(
            open_gate(),
            &format!("http://127.0.0.1:{port}"),
            "dev",
            "alice",
            "sesame",
        )
        .unwrap();
        assert_eq!(auth.user.id, "u1");
        assert_eq!(auth.access_token, "tok");
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("POST /Users/AuthenticateByName "));
        let value = header(&log[0], "X-Emby-Authorization").unwrap();
        assert!(value.contains("DeviceId=\"dev\""));
        assert!(header(&log[0], "X-Emby-Token").is_none());
    }

    #[test]
    fn a_wrong_password_is_unauthorized() {
        let (port, _) = fake_server(vec![http("401 Unauthorized", "")]);
        let err = authenticate(
            open_gate(),
            &format!("http://127.0.0.1:{port}"),
            "dev",
            "alice",
            "wrong",
        )
        .err()
        .unwrap();
        assert_eq!(err, RemoteError::Unauthorized);
    }

    #[test]
    fn public_info_needs_no_token() {
        let (port, seen) = fake_server(vec![http(
            "200 OK",
            r#"{"Id":"srv","ServerName":"Home","ProductName":"Jellyfin Server"}"#,
        )]);
        let info = public_info(open_gate(), &format!("http://127.0.0.1:{port}")).unwrap();
        assert_eq!(info.id, "srv");
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("GET /System/Info/Public "));
        assert!(header(&log[0], "X-Emby-Token").is_none());
    }

    #[test]
    fn music_libraries_fall_back_to_user_views() {
        let (port, seen) = fake_server(vec![
            http("404 Not Found", ""),
            http(
                "200 OK",
                r#"{"Items":[{"Id":"m","Name":"Music","CollectionType":"music"}]}"#,
            ),
        ]);
        let backend = JellyfinBackend::with_gate(config_for(port, &[]), open_gate());
        let views = backend.music_libraries().unwrap();
        assert_eq!(views.len(), 1);
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("GET /Users/u1/Views "));
        assert!(log[1].starts_with("GET /UserViews?userId=u1 "));
        assert_eq!(header(&log[1], "X-Emby-Token"), Some("tok"));
    }

    #[test]
    fn catalog_page_walks_libraries_in_order() {
        let (port, seen) = fake_server(vec![
            page(&[], 2),
            page(&[], 1),
            page(&["a", "b"], 2),
            page(&["c"], 1),
        ]);
        let backend = JellyfinBackend::with_gate(config_for(port, &["L1", "L2"]), open_gate());
        let first = backend.catalog_page(0).unwrap();
        assert_eq!(first.total, Some(3));
        assert_eq!(first.tracks.len(), 2);
        assert_eq!(first.next, Some(CURSOR_STRIDE));
        let second = backend.catalog_page(CURSOR_STRIDE).unwrap();
        assert_eq!(second.tracks[0].remote_id, "c");
        assert_eq!(second.next, None);
        let log = seen.lock().unwrap();
        assert!(log[0].contains("ParentId=L1"));
        assert!(log[0].contains("Limit=0"));
        assert!(log[2].contains("ParentId=L1"));
        assert!(log[2].contains("StartIndex=0&Limit=500"));
        assert!(log[3].contains("ParentId=L2"));
    }

    #[test]
    fn catalog_page_continues_within_a_library() {
        let (port, _) = fake_server(vec![page(&[], 600), page(&["a"; 500], 600)]);
        let backend = JellyfinBackend::with_gate(config_for(port, &["L1"]), open_gate());
        let first = backend.catalog_page(0).unwrap();
        assert_eq!(first.next, Some(500));
    }

    #[test]
    fn open_bytes_asks_for_the_original_file_from_an_offset() {
        let (port, seen) = fake_server(vec![http("206 Partial Content", "tail")]);
        let backend = JellyfinBackend::with_gate(config_for(port, &[]), open_gate());
        let mut stream = backend.open_bytes("i1", 10).unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).unwrap();
        assert_eq!(body, "tail");
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("GET /Audio/i1/stream?static=true "));
        assert_eq!(header(&log[0], "Range"), Some("bytes=10-"));
        assert_eq!(header(&log[0], "X-Emby-Token"), Some("tok"));
    }

    #[test]
    fn a_full_answer_to_a_ranged_request_is_refused() {
        let (port, _) = fake_server(vec![http("200 OK", "whole")]);
        let backend = JellyfinBackend::with_gate(config_for(port, &[]), open_gate());
        assert_eq!(
            backend.open_bytes("i1", 10).err().unwrap(),
            RemoteError::Http(200)
        );
    }

    #[test]
    fn a_revoked_token_is_unauthorized() {
        let (port, _) = fake_server(vec![http("401 Unauthorized", "")]);
        let backend = JellyfinBackend::with_gate(config_for(port, &["L1"]), open_gate());
        assert_eq!(
            backend.check_reachable().err().unwrap(),
            RemoteError::Unauthorized
        );
    }

    #[test]
    fn fetch_artwork_reads_the_primary_image() {
        let (port, seen) = fake_server(vec![http("200 OK", "png")]);
        let backend = JellyfinBackend::with_gate(config_for(port, &[]), open_gate());
        assert_eq!(backend.fetch_artwork("al1").unwrap(), b"png");
        assert!(seen.lock().unwrap()[0].starts_with("GET /Items/al1/Images/Primary?"));
    }

    #[test]
    fn a_closed_gate_blocks_every_request() {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        let backend = JellyfinBackend::with_gate(config_for(1, &["L1"]), gate);
        assert_eq!(
            backend.catalog_page(0).err().unwrap(),
            RemoteError::Blocked(GateState::Disabled)
        );
        assert!(matches!(
            authenticate(gate, "http://127.0.0.1:1", "d", "u", "p"),
            Err(RemoteError::Blocked(GateState::Disabled))
        ));
    }
}
