// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Full-length search results, streamed through the same cache and
//! progressive reader as a remote library.
//!
//! A streamed result is a `tracks` row of the built-in [`SEARCH_SOURCE`],
//! never visible in browse or search, so it plays and caches like a Plex track
//! without becoming library content until it is downloaded. It is not a
//! remote source the user added: it has no `remote_sources` row, is never
//! synced, and its traffic follows the search switch through
//! [`NetworkGate::search`] rather than Outside Sourcing.
//!
//! The part key is `<provider>|<audio url>`, so a fetch can send the
//! provider's `Referer` where its host demands one (ccMixter).

use super::backend::{total_size, ByteStream, CatalogPage, RemoteBackend, RemoteError};
use super::gate::NetworkGate;
use super::http::stream_client;
use crate::sources::{self, SourceConfig};
use reqwest::blocking::Response;

/// The source id every streamed search result belongs to.
pub const SEARCH_SOURCE: &str = "search";

/// The row id for a provider's track under [`SEARCH_SOURCE`].
pub fn remote_id(provider: &str, id: &str) -> String {
    format!("{provider}:{id}")
}

pub fn part_key(provider: &str, audio_url: &str) -> String {
    format!("{provider}|{audio_url}")
}

fn split_part_key(part_key: &str) -> Result<(&str, &str), RemoteError> {
    part_key
        .split_once('|')
        .ok_or_else(|| RemoteError::Parse("bad search stream key".into()))
}

pub struct SearchStreamBackend {
    gate: &'static NetworkGate,
}

impl SearchStreamBackend {
    pub fn new() -> Self {
        Self::with_gate(NetworkGate::search())
    }

    pub fn with_gate(gate: &'static NetworkGate) -> Self {
        Self { gate }
    }

    fn get(&self, part_key: &str, range: &str) -> Result<Response, RemoteError> {
        self.gate.check()?;
        let (provider, url) = split_part_key(part_key)?;
        let referer = sources::provider_by_id(&SourceConfig::default(), provider)
            .and_then(|p| p.fetch_referer());
        // Wikimedia turns away requests that don't say who is asking.
        let mut request = stream_client()
            .get(url)
            .header(reqwest::header::USER_AGENT, sources::USER_AGENT)
            .header("Range", range);
        if let Some(referer) = referer {
            request = request.header(reqwest::header::REFERER, referer);
        }
        let resp = request
            .send()
            .map_err(|_| RemoteError::Unreachable("Can't reach the source".into()))?;
        match resp.status().as_u16() {
            206 => Ok(resp),
            200 if range.starts_with("bytes=0-") => Ok(resp),
            401 | 403 => Err(RemoteError::Unreachable(
                "This recording isn't available for streaming".into(),
            )),
            404 | 410 => Err(RemoteError::Unreachable(
                "This recording is no longer available at the source".into(),
            )),
            code => Err(RemoteError::Http(code)),
        }
    }
}

impl Default for SearchStreamBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteBackend for SearchStreamBackend {
    fn kind(&self) -> &'static str {
        SEARCH_SOURCE
    }

    /// There is no catalog to walk; results arrive one at a time from search.
    fn catalog_page(&self, _cursor: usize) -> Result<CatalogPage, RemoteError> {
        Ok(CatalogPage {
            tracks: Vec::new(),
            total: Some(0),
            next: None,
        })
    }

    fn open_bytes(&self, part_key: &str, offset: u64) -> Result<Box<dyn ByteStream>, RemoteError> {
        Ok(Box::new(self.get(part_key, &format!("bytes={offset}-"))?))
    }

    fn check_reachable(&self) -> Result<(), RemoteError> {
        self.gate.check()
    }

    /// Artwork is fetched when the result is streamed, not through here.
    fn fetch_artwork(&self, _art_ref: &str) -> Result<Vec<u8>, RemoteError> {
        Err(RemoteError::Parse("no artwork".into()))
    }

    fn content_length(&self, part_key: &str) -> Result<u64, RemoteError> {
        let resp = self.get(part_key, "bytes=0-0")?;
        total_size(&resp).ok_or_else(|| RemoteError::Parse("no size".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::testkit::fake_server;
    use std::io::Read;

    fn open_gate() -> &'static NetworkGate {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(true, true);
        gate
    }

    #[test]
    fn keys_carry_the_provider() {
        assert_eq!(remote_id("audius", "YmJWK"), "audius:YmJWK");
        let key = part_key("ccmixter", "https://ccmixter.org/content/a|b.mp3");
        assert_eq!(
            split_part_key(&key).unwrap(),
            ("ccmixter", "https://ccmixter.org/content/a|b.mp3")
        );
        assert!(split_part_key("no provider").is_err());
    }

    #[test]
    fn learns_the_size_and_reads_ranges_with_the_providers_referer() {
        let (port, seen) = fake_server(vec![
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/2048\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx".to_string(),
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc".to_string(),
        ]);
        let backend = SearchStreamBackend::with_gate(open_gate());
        let key = part_key(
            "ccmixter",
            &format!("http://127.0.0.1:{port}/content/a.mp3"),
        );
        assert_eq!(backend.content_length(&key).unwrap(), 2048);
        let mut body = Vec::new();
        backend
            .open_bytes(&key, 100)
            .unwrap()
            .read_to_end(&mut body)
            .unwrap();
        assert_eq!(body, b"abc");
        let log = seen.lock().unwrap();
        assert!(
            log[1].contains("referer: https://ccmixter.org/\r\n"),
            "{}",
            log[1]
        );
        assert!(log[1]
            .to_ascii_lowercase()
            .contains("range: bytes=100-\r\n"));
    }

    #[test]
    fn the_search_switch_closes_streaming() {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        let backend = SearchStreamBackend::with_gate(gate);
        assert!(backend
            .open_bytes(&part_key("audius", "http://127.0.0.1:9/x"), 0)
            .is_err());
    }
}

/// Live checks that each full-length provider's audio can be streamed from
/// the middle, which is what seeking ahead of the download needs. Run with
/// `cargo test --lib -- --ignored live_search_stream --nocapture`.
#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::sources::{archive, audius, ccmixter, wikimedia, SourceProvider};
    use std::io::Read;

    fn check(provider: &dyn SourceProvider, query: &str) {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(true, true);
        let backend = SearchStreamBackend::with_gate(gate);
        let client = sources::source_client();
        let tracks = provider.search(client, query, 10).expect("search");
        let (track, url) = tracks
            .iter()
            .find_map(|t| provider.resolve_audio(client, t).ok().map(|url| (t, url)))
            .expect("nothing resolved");
        let key = part_key(provider.id(), &url);
        let size = backend.content_length(&key).expect("size");
        let mut middle = Vec::new();
        backend
            .open_bytes(&key, size / 2)
            .expect("ranged read")
            .take(4096)
            .read_to_end(&mut middle)
            .unwrap();
        assert_eq!(middle.len(), 4096);
        println!("OK  {}: {} is {size} bytes", provider.id(), track.title);
    }

    #[test]
    #[ignore = "hits the network"]
    fn live_search_stream_seeks_on_every_full_length_provider() {
        check(&audius::Audius, "lofi");
        check(&ccmixter::CcMixter, "piano");
        check(&wikimedia::WikimediaCommons, "bach cello suite");
        check(&archive::InternetArchive, "grateful dead 1977");
    }
}
