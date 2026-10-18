// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Connection choice and the `RemoteBackend` implementation for Plex.

use super::api::parse_page;
use super::auth::{PlexConnection, PlexSection};
use super::{api_client, check_status, stream_client, PlexHeaders};
use crate::remote::backend::{ByteStream, CatalogPage, RemoteBackend, RemoteError};
use crate::remote::gate::NetworkGate;
use crate::remote::uri::percent_encode;
use parking_lot::Mutex;
use reqwest::blocking::{RequestBuilder, Response};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const PAGE_SIZE: usize = 500;
/// A catalog cursor packs the section index and the offset inside it:
/// `section * CURSOR_STRIDE + offset`.
const CURSOR_STRIDE: usize = 10_000_000;
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Candidate base URIs in the order they should be tried: local https, the
/// plain `http://address:port` of local connections (the fallback when
/// plex.direct DNS is blocked), remote direct, then relay.
pub fn ordered(conns: &[PlexConnection]) -> Vec<String> {
    let local_https = conns
        .iter()
        .filter(|c| c.local && c.protocol == "https")
        .map(|c| c.uri.clone());
    let local_plain = conns
        .iter()
        .filter(|c| c.local)
        .map(|c| format!("http://{}:{}", c.address, c.port));
    let remote = conns
        .iter()
        .filter(|c| !c.local && !c.relay)
        .map(|c| c.uri.clone());
    let relay = conns.iter().filter(|c| c.relay).map(|c| c.uri.clone());

    let mut out: Vec<String> = Vec::new();
    for uri in local_https.chain(local_plain).chain(remote).chain(relay) {
        if !out.contains(&uri) {
            out.push(uri);
        }
    }
    out
}

/// The first candidate the probe accepts.
pub fn pick(candidates: &[String], probe: &dyn Fn(&str) -> bool) -> Option<String> {
    candidates.iter().find(|uri| probe(uri)).cloned()
}

pub(crate) fn probe_identity(headers: &PlexHeaders, uri: &str) -> bool {
    headers
        .apply(
            api_client()
                .get(format!("{uri}/identity"))
                .timeout(PROBE_TIMEOUT),
        )
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Everything needed to talk to one Plex server, stored as `config_json`.
/// Holds tokens, so it deliberately has no `Debug`.
#[derive(Clone, Serialize, Deserialize)]
pub struct PlexSourceConfig {
    pub server_id: String,
    pub server_name: String,
    pub server_token: String,
    pub account_token: String,
    pub sections: Vec<PlexSection>,
    pub connections: Vec<PlexConnection>,
    pub preferred_uri: Option<String>,
    /// Added by address and token rather than through plex.tv, so signing in
    /// again means entering a token, not opening plex.tv.
    #[serde(default)]
    pub direct: bool,
}

pub struct PlexBackend {
    config: PlexSourceConfig,
    headers: PlexHeaders,
    gate: &'static NetworkGate,
    active: Mutex<Option<String>>,
    /// Item count across the selected sections, measured when a walk starts.
    catalog_total: Mutex<Option<usize>>,
}

fn with_token(url: String, token: &str) -> String {
    if token.is_empty() {
        return url;
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}X-Plex-Token={}", percent_encode(token))
}

impl PlexBackend {
    pub fn new(config: PlexSourceConfig, headers: PlexHeaders) -> Self {
        Self::with_gate(config, headers, NetworkGate::global())
    }

    pub fn with_gate(
        config: PlexSourceConfig,
        headers: PlexHeaders,
        gate: &'static NetworkGate,
    ) -> Self {
        let active = Mutex::new(config.preferred_uri.clone());
        Self {
            config,
            headers,
            gate,
            active,
            catalog_total: Mutex::new(None),
        }
    }

    /// The connection last known to work, if any, for the caller to persist as
    /// `preferred_uri`.
    pub fn active_uri(&self) -> Option<String> {
        self.active.lock().clone()
    }

    fn unreachable(&self) -> RemoteError {
        RemoteError::Unreachable(format!("Can't reach {}", self.config.server_name))
    }

    fn repick(&self) -> Result<String, RemoteError> {
        let candidates = ordered(&self.config.connections);
        match pick(&candidates, &|uri| probe_identity(&self.headers, uri)) {
            Some(uri) => {
                *self.active.lock() = Some(uri.clone());
                Ok(uri)
            }
            None => {
                *self.active.lock() = None;
                Err(self.unreachable())
            }
        }
    }

    fn current_uri(&self) -> Result<String, RemoteError> {
        let cached = self.active.lock().clone();
        match cached {
            Some(uri) => Ok(uri),
            None => self.repick(),
        }
    }

    /// Sends the request built for the current connection. A connect or
    /// timeout failure drops the cached connection, picks again once and
    /// retries.
    fn send(&self, build: &dyn Fn(&str) -> RequestBuilder) -> Result<Response, RemoteError> {
        let uri = self.current_uri()?;
        match build(&uri).send() {
            Ok(resp) => Ok(resp),
            Err(e) if e.is_connect() || e.is_timeout() => {
                *self.active.lock() = None;
                let uri = self.repick()?;
                build(&uri).send().map_err(|_| self.unreachable())
            }
            Err(_) => Err(self.unreachable()),
        }
    }

    /// Sums each selected section's size, asking for zero items so only the
    /// count comes back. `None` when any section does not report one.
    fn measure_catalog(&self) -> Result<Option<usize>, RemoteError> {
        let mut sum = Some(0usize);
        for section in &self.config.sections {
            let body = self.get_json(&format!(
                "/library/sections/{}/all?type=10&X-Plex-Container-Start=0&X-Plex-Container-Size=0",
                percent_encode(&section.key)
            ))?;
            sum = sum.zip(parse_page(&body)?.total).map(|(a, b)| a + b);
        }
        Ok(sum)
    }

    fn get_json(&self, path_and_query: &str) -> Result<String, RemoteError> {
        let token = &self.config.server_token;
        let resp = self.send(&|base| {
            self.headers
                .apply(api_client().get(with_token(format!("{base}{path_and_query}"), token)))
        })?;
        check_status(resp)?.text().map_err(|_| self.unreachable())
    }
}

impl RemoteBackend for PlexBackend {
    fn kind(&self) -> &'static str {
        "plex"
    }

    fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
        self.gate.check()?;
        if cursor == 0 {
            let total = self.measure_catalog()?;
            *self.catalog_total.lock() = total;
        }
        let section_index = cursor / CURSOR_STRIDE;
        let offset = cursor % CURSOR_STRIDE;
        let Some(section) = self.config.sections.get(section_index) else {
            return Ok(CatalogPage {
                tracks: Vec::new(),
                total: None,
                next: None,
            });
        };

        let body = self.get_json(&format!(
            "/library/sections/{}/all?type=10&X-Plex-Container-Start={offset}&X-Plex-Container-Size={PAGE_SIZE}",
            percent_encode(&section.key)
        ))?;
        let page = parse_page(&body)?;

        let consumed = offset + page.raw_count;
        if consumed >= CURSOR_STRIDE {
            return Err(RemoteError::Parse("section too large to page".to_string()));
        }
        let more_here = page.raw_count > 0
            && match page.total {
                Some(total) => consumed < total,
                None => page.raw_count >= PAGE_SIZE,
            };
        let next = if more_here {
            Some(section_index * CURSOR_STRIDE + consumed)
        } else if section_index + 1 < self.config.sections.len() {
            Some((section_index + 1) * CURSOR_STRIDE)
        } else {
            None
        };

        Ok(CatalogPage {
            tracks: page.tracks,
            total: *self.catalog_total.lock(),
            next,
        })
    }

    fn open_bytes(&self, part_key: &str, offset: u64) -> Result<Box<dyn ByteStream>, RemoteError> {
        self.gate.check()?;
        let token = &self.config.server_token;
        let resp = self.send(&|base| {
            self.headers
                .apply_identity(stream_client().get(with_token(format!("{base}{part_key}"), token)))
                .header("Range", format!("bytes={offset}-"))
        })?;
        match resp.status() {
            StatusCode::PARTIAL_CONTENT => Ok(Box::new(resp)),
            StatusCode::OK if offset == 0 => Ok(Box::new(resp)),
            other if other.is_success() => Err(RemoteError::Http(other.as_u16())),
            _ => check_status(resp).map(|r| Box::new(r) as Box<dyn ByteStream>),
        }
    }

    fn check_reachable(&self) -> Result<(), RemoteError> {
        self.gate.check()?;
        let cached = self.active.lock().clone();
        if let Some(uri) = cached {
            if probe_identity(&self.headers, &uri) {
                return Ok(());
            }
        }
        self.repick().map(|_| ())
    }

    fn working_uri(&self) -> Option<String> {
        self.active_uri()
    }

    fn fetch_artwork(&self, art_ref: &str) -> Result<Vec<u8>, RemoteError> {
        self.gate.check()?;
        let token = &self.config.server_token;
        let resp = self.send(&|base| {
            self.headers.apply_identity(api_client().get(with_token(
                format!(
                    "{base}/photo/:/transcode?width=600&height=600&minSize=1&url={}",
                    percent_encode(art_ref)
                ),
                token,
            )))
        })?;
        check_status(resp)?
            .bytes()
            .map(|b| b.to_vec())
            .map_err(|_| self.unreachable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::gate::GateState;
    use crate::remote::testkit::{fake_server, http};
    use std::io::Read;
    use std::net::TcpListener;

    fn conn(
        uri: &str,
        local: bool,
        relay: bool,
        address: &str,
        port: u16,
        proto: &str,
    ) -> PlexConnection {
        PlexConnection {
            uri: uri.into(),
            local,
            relay,
            address: address.into(),
            port,
            protocol: proto.into(),
        }
    }

    fn headers() -> PlexHeaders {
        PlexHeaders {
            client_id: "client-1".into(),
            version: "1.0.0".into(),
            platform: "macos".into(),
            device_name: "Mac".into(),
        }
    }

    fn open_gate() -> &'static NetworkGate {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(true, true);
        gate
    }

    fn config_for(uri: &str) -> PlexSourceConfig {
        PlexSourceConfig {
            server_id: "srv-1".into(),
            server_name: "Home Server".into(),
            server_token: "secret-token".into(),
            account_token: "account-token".into(),
            sections: vec![
                PlexSection {
                    key: "3".into(),
                    title: "Music".into(),
                },
                PlexSection {
                    key: "4".into(),
                    title: "More".into(),
                },
            ],
            connections: Vec::new(),
            preferred_uri: Some(uri.into()),
            direct: false,
        }
    }

    #[test]
    fn orders_local_then_plain_local_then_remote_then_relay() {
        let conns = vec![
            conn(
                "https://relay.example:443",
                false,
                true,
                "relay.example",
                443,
                "https",
            ),
            conn(
                "https://203-0-113-9.abc.plex.direct:32400",
                false,
                false,
                "203.0.113.9",
                32400,
                "https",
            ),
            conn(
                "http://192.168.1.5:32400",
                true,
                false,
                "192.168.1.5",
                32400,
                "http",
            ),
            conn(
                "https://192-168-1-5.abc.plex.direct:32400",
                true,
                false,
                "192.168.1.5",
                32400,
                "https",
            ),
        ];
        assert_eq!(
            ordered(&conns),
            vec![
                "https://192-168-1-5.abc.plex.direct:32400",
                "http://192.168.1.5:32400",
                "https://203-0-113-9.abc.plex.direct:32400",
                "https://relay.example:443",
            ]
        );
    }

    #[test]
    fn ordered_adds_plain_fallback_for_local_https() {
        let conns = vec![conn(
            "https://192-168-1-5.abc.plex.direct:32400",
            true,
            false,
            "192.168.1.5",
            32400,
            "https",
        )];
        assert_eq!(
            ordered(&conns),
            vec![
                "https://192-168-1-5.abc.plex.direct:32400",
                "http://192.168.1.5:32400",
            ]
        );
    }

    #[test]
    fn pick_returns_first_probe_success() {
        let candidates = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let calls = std::cell::RefCell::new(Vec::new());
        let picked = pick(&candidates, &|uri| {
            calls.borrow_mut().push(uri.to_string());
            uri == "b" || uri == "c"
        });
        assert_eq!(picked.as_deref(), Some("b"));
        assert_eq!(*calls.borrow(), vec!["a", "b"]);
        assert_eq!(pick(&candidates, &|_| false), None);
    }

    #[test]
    fn open_bytes_sends_range_and_token() {
        let (port, seen) = fake_server(vec![http("206 Partial Content", "hello")]);
        let backend = PlexBackend::with_gate(
            config_for(&format!("http://127.0.0.1:{port}")),
            headers(),
            open_gate(),
        );
        let mut stream = backend
            .open_bytes("/library/parts/9/1/file.flac", 1000)
            .unwrap();
        let mut body = Vec::new();
        stream.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"hello");

        let log = seen.lock().unwrap();
        let head = &log[0];
        assert!(head.starts_with("GET /library/parts/9/1/file.flac?X-Plex-Token=secret-token "));
        assert!(head.to_ascii_lowercase().contains("range: bytes=1000-\r\n"));
    }

    #[test]
    fn a_server_without_a_token_gets_requests_without_one() {
        let (port, seen) = fake_server(vec![http("206 Partial Content", "hello")]);
        let mut config = config_for(&format!("http://127.0.0.1:{port}"));
        config.server_token = String::new();
        config.direct = true;
        let backend = PlexBackend::with_gate(config, headers(), open_gate());
        let mut stream = backend
            .open_bytes("/library/parts/9/1/file.flac", 0)
            .unwrap();
        let mut body = Vec::new();
        stream.read_to_end(&mut body).unwrap();

        let log = seen.lock().unwrap();
        assert!(
            log[0].starts_with("GET /library/parts/9/1/file.flac HTTP/1.1"),
            "{}",
            log[0]
        );
    }

    #[test]
    fn open_bytes_accepts_200_only_from_the_start() {
        let (port, _) = fake_server(vec![http("200 OK", "whole")]);
        let backend = PlexBackend::with_gate(
            config_for(&format!("http://127.0.0.1:{port}")),
            headers(),
            open_gate(),
        );
        assert!(backend.open_bytes("/p", 0).is_ok());
        assert_eq!(
            backend.open_bytes("/p", 10).err(),
            Some(RemoteError::Http(200))
        );
    }

    #[test]
    fn open_bytes_maps_401_to_unauthorized() {
        let (port, _) = fake_server(vec![http("401 Unauthorized", "")]);
        let backend = PlexBackend::with_gate(
            config_for(&format!("http://127.0.0.1:{port}")),
            headers(),
            open_gate(),
        );
        assert_eq!(
            backend.open_bytes("/p", 0).err(),
            Some(RemoteError::Unauthorized)
        );
    }

    #[test]
    fn open_bytes_maps_other_statuses_to_http() {
        let (port, _) = fake_server(vec![http("500 Internal Server Error", "")]);
        let backend = PlexBackend::with_gate(
            config_for(&format!("http://127.0.0.1:{port}")),
            headers(),
            open_gate(),
        );
        assert_eq!(
            backend.open_bytes("/p", 0).err(),
            Some(RemoteError::Http(500))
        );
    }

    #[test]
    fn unreachable_server_names_the_server_without_the_token() {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let backend = PlexBackend::with_gate(
            config_for(&format!("http://127.0.0.1:{port}")),
            headers(),
            open_gate(),
        );
        let err = backend.open_bytes("/p", 0).err().unwrap();
        assert_eq!(
            err,
            RemoteError::Unreachable("Can't reach Home Server".into())
        );
        assert!(!err.to_string().contains("secret-token"));
    }

    #[test]
    fn gate_disabled_blocks_every_call() {
        let (port, seen) = fake_server(vec![http("200 OK", "{}")]);
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(false, false);
        let backend = PlexBackend::with_gate(
            config_for(&format!("http://127.0.0.1:{port}")),
            headers(),
            gate,
        );
        let blocked = RemoteError::Blocked(GateState::Disabled);
        assert_eq!(backend.open_bytes("/p", 0).err(), Some(blocked.clone()));
        assert_eq!(backend.catalog_page(0).err(), Some(blocked.clone()));
        assert_eq!(backend.check_reachable().err(), Some(blocked.clone()));
        assert_eq!(backend.fetch_artwork("/a").err(), Some(blocked));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn catalog_page_walks_sections_in_order() {
        let first = r#"{"MediaContainer":{"size":1,"totalSize":2,"Metadata":[
          {"ratingKey":"1","title":"A","Media":[{"Part":[{"key":"/p/1","container":"mp3"}]}]}]}}"#;
        let second = r#"{"MediaContainer":{"size":1,"totalSize":2,"Metadata":[
          {"ratingKey":"2","title":"B","Media":[{"Part":[{"key":"/p/2","container":"mp3"}]}]}]}}"#;
        let third = r#"{"MediaContainer":{"size":1,"totalSize":1,"Metadata":[
          {"ratingKey":"3","title":"C","Media":[{"Part":[{"key":"/p/3","container":"mp3"}]}]}]}}"#;
        let (port, seen) = fake_server(vec![
            http("200 OK", r#"{"MediaContainer":{"size":0,"totalSize":2}}"#),
            http("200 OK", r#"{"MediaContainer":{"size":0,"totalSize":1}}"#),
            http("200 OK", first),
            http("200 OK", second),
            http("200 OK", third),
        ]);
        let backend = PlexBackend::with_gate(
            config_for(&format!("http://127.0.0.1:{port}")),
            headers(),
            open_gate(),
        );

        let p1 = backend.catalog_page(0).unwrap();
        assert_eq!(p1.tracks[0].remote_id, "1");
        assert_eq!(p1.total, Some(3));
        let c2 = p1.next.expect("more tracks in the same section");
        let p2 = backend.catalog_page(c2).unwrap();
        assert_eq!(p2.tracks[0].remote_id, "2");
        let c3 = p2.next.expect("next section");
        let p3 = backend.catalog_page(c3).unwrap();
        assert_eq!(p3.tracks[0].remote_id, "3");
        assert_eq!(p3.next, None);

        let log = seen.lock().unwrap();
        assert!(log[2].starts_with(
            "GET /library/sections/3/all?type=10&X-Plex-Container-Start=0&X-Plex-Container-Size=500&X-Plex-Token=secret-token "
        ));
        assert!(log[3].contains("/library/sections/3/all?type=10&X-Plex-Container-Start=1&"));
        assert!(log[4].contains("/library/sections/4/all?type=10&X-Plex-Container-Start=0&"));
    }

    #[test]
    fn catalog_total_is_the_sum_of_section_sizes_on_every_page() {
        let size = |n: usize| format!(r#"{{"MediaContainer":{{"size":0,"totalSize":{n}}}}}"#);
        let page = |id: &str, total: usize| {
            format!(
                r#"{{"MediaContainer":{{"size":1,"totalSize":{total},"Metadata":[
              {{"ratingKey":"{id}","title":"T","Media":[{{"Part":[{{"key":"/p/{id}","container":"mp3"}}]}}]}}]}}}}"#
            )
        };
        let (port, seen) = fake_server(vec![
            http("200 OK", &size(1)),
            http("200 OK", &size(1)),
            http("200 OK", &page("1", 1)),
            http("200 OK", &page("2", 1)),
        ]);
        let backend = PlexBackend::with_gate(
            config_for(&format!("http://127.0.0.1:{port}")),
            headers(),
            open_gate(),
        );

        let p1 = backend.catalog_page(0).unwrap();
        assert_eq!(p1.total, Some(2));
        let p2 = backend.catalog_page(p1.next.unwrap()).unwrap();
        assert_eq!(p2.total, Some(2));
        assert_eq!(p2.next, None);

        let log = seen.lock().unwrap();
        assert!(log[0].starts_with(
            "GET /library/sections/3/all?type=10&X-Plex-Container-Start=0&X-Plex-Container-Size=0&"
        ));
        assert!(log[1].contains(
            "/library/sections/4/all?type=10&X-Plex-Container-Start=0&X-Plex-Container-Size=0&"
        ));
        assert_eq!(log.len(), 4);
    }

    #[test]
    fn check_reachable_picks_a_live_connection() {
        let (port, seen) = fake_server(vec![http("200 OK", "{}")]);
        let mut config = config_for("http://127.0.0.1:9");
        config.preferred_uri = None;
        config.connections = vec![
            conn("http://127.0.0.1:9", false, false, "127.0.0.1", 9, "http"),
            conn(
                &format!("http://127.0.0.1:{port}"),
                false,
                false,
                "127.0.0.1",
                port,
                "http",
            ),
        ];
        let backend = PlexBackend::with_gate(config, headers(), open_gate());
        assert_eq!(backend.check_reachable(), Ok(()));
        assert!(seen.lock().unwrap()[0].starts_with("GET /identity "));
    }

    #[test]
    fn working_uri_follows_the_connection_that_answered() {
        let (port, _) = fake_server(vec![http("200 OK", "{}")]);
        let live = format!("http://127.0.0.1:{port}");
        let mut config = config_for("http://127.0.0.1:9");
        config.connections = vec![conn(&live, false, false, "127.0.0.1", port, "http")];
        let backend = PlexBackend::with_gate(config, headers(), open_gate());
        assert_eq!(backend.working_uri().as_deref(), Some("http://127.0.0.1:9"));

        backend.check_reachable().unwrap();
        assert_eq!(backend.working_uri().as_deref(), Some(live.as_str()));
    }

    #[test]
    fn fetch_artwork_requests_a_transcoded_photo() {
        let (port, seen) = fake_server(vec![http("200 OK", "IMG")]);
        let backend = PlexBackend::with_gate(
            config_for(&format!("http://127.0.0.1:{port}")),
            headers(),
            open_gate(),
        );
        assert_eq!(
            backend
                .fetch_artwork("/library/metadata/5/thumb/1")
                .unwrap(),
            b"IMG"
        );
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with(
            "GET /photo/:/transcode?width=600&height=600&minSize=1&url=%2Flibrary%2Fmetadata%2F5%2Fthumb%2F1&X-Plex-Token=secret-token "
        ));
    }
}
