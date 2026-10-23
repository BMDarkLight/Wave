// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! An Audius account as a library: the tracks it uploaded, favorited and
//! reposted.
//!
//! All of that is public, so the account is added by its handle and no
//! password is involved. The catalog is walked in three phases, one after
//! another in the cursor space, and a track that shows up in more than one is
//! kept once. Audio is the `/stream` endpoint, which redirects to a signed copy
//! and honours Range; the catalog has no file sizes, so the stream cache asks
//! for one with a one-byte request before the first play.

use super::backend::{
    total_size, unix_seconds, ByteStream, CatalogPage, RemoteBackend, RemoteError, RemoteTrackMeta,
};
use super::gate::NetworkGate;
use super::http::{api_client, stream_client};
use crate::sources::audius::{is_streamable, stream_url, API_BASE, APP_NAME};
use crate::sources::{duration_field, id_field, str_field};
use reqwest::blocking::Response;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Read;

/// Items per request; the most the API returns.
const PAGE: usize = 100;
/// Where each phase of the walk starts in the cursor space.
const FAVORITES: usize = 1 << 28;
const REPOSTS: usize = 2 << 28;
const MAX_ART_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudiusConfig {
    pub user_id: String,
    pub handle: String,
    pub name: String,
}

/// The handle in whatever the user pasted: "@name", "name", or a profile link
/// such as "https://audius.co/name".
pub fn parse_handle(input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_end_matches('/');
    let handle = match trimmed.split_once("audius.co/") {
        Some((_, rest)) => rest.split(['/', '?', '#']).next().unwrap_or_default(),
        None => trimmed,
    }
    .trim_start_matches('@');
    if handle.is_empty()
        || !handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return Err("Enter an Audius handle, such as @name".to_string());
    }
    Ok(handle.to_string())
}

fn api_url(base: &str, path: &str) -> String {
    let joiner = if path.contains('?') { '&' } else { '?' };
    format!("{base}{path}{joiner}app_name={APP_NAME}")
}

fn unreachable() -> RemoteError {
    RemoteError::Unreachable("Can't reach Audius".into())
}

fn get_data(gate: &NetworkGate, base: &str, path: &str) -> Result<serde_json::Value, RemoteError> {
    gate.check()?;
    let resp = api_client()
        .get(api_url(base, path))
        .send()
        .map_err(|_| unreachable())?;
    if !resp.status().is_success() {
        return Err(RemoteError::Http(resp.status().as_u16()));
    }
    let body: serde_json::Value = resp.json().map_err(|e| RemoteError::Parse(e.to_string()))?;
    body.get("data")
        .cloned()
        .ok_or_else(|| RemoteError::Parse("no data".into()))
}

/// Finds the account behind a handle.
pub fn resolve(gate: &NetworkGate, handle: &str) -> Result<AudiusConfig, RemoteError> {
    let user = get_data(gate, API_BASE, &format!("/users/handle/{handle}"))?;
    Ok(AudiusConfig {
        user_id: id_field(&user, "id").ok_or_else(|| RemoteError::Parse("no user id".into()))?,
        handle: str_field(&user, "handle").unwrap_or_else(|| handle.to_string()),
        name: str_field(&user, "name").unwrap_or_else(|| handle.to_string()),
    })
}

pub fn display_name(config: &AudiusConfig) -> String {
    format!("{} on Audius", config.name)
}

fn track_to_meta(item: &serde_json::Value) -> Option<RemoteTrackMeta> {
    if !is_streamable(item) {
        return None;
    }
    let id = id_field(item, "id")?;
    let art_ref = item.get("artwork").and_then(|art| {
        str_field(art, "480x480")
            .or_else(|| str_field(art, "1000x1000"))
            .or_else(|| str_field(art, "150x150"))
    });
    let album = item
        .get("album_backlink")
        .and_then(|album| str_field(album, "playlist_name"));
    Some(RemoteTrackMeta {
        title: str_field(item, "title").unwrap_or_else(|| "Untitled".to_string()),
        artist: item
            .get("user")
            .and_then(|u| str_field(u, "name").or_else(|| str_field(u, "handle")))
            .unwrap_or_else(|| "Unknown Artist".to_string()),
        album_artist: None,
        album: album.unwrap_or_else(|| "Singles".to_string()),
        track_number: None,
        disc_number: None,
        year: str_field(item, "release_date").and_then(|d| d.get(..4)?.parse().ok()),
        genre: str_field(item, "genre"),
        duration_seconds: duration_field(item, "duration"),
        // Not in the catalog; learned on first play.
        size: 0,
        extension: "mp3".to_string(),
        part_key: id.clone(),
        art_ref,
        updated_at: str_field(item, "created_at")
            .map(|d| d.replace(' ', "T"))
            .and_then(|d| unix_seconds(&d)),
        remote_id: id,
    })
}

pub struct AudiusBackend {
    config: AudiusConfig,
    gate: &'static NetworkGate,
    /// The API's address; a local fake in tests.
    base: String,
    seen: parking_lot::Mutex<HashSet<String>>,
    /// The raw ids of the page before, to notice a server answering a later
    /// offset with the same page again.
    last_page: parking_lot::Mutex<Vec<String>>,
}

impl AudiusBackend {
    pub fn new(config: AudiusConfig) -> Self {
        Self::with_gate(config, NetworkGate::global())
    }

    pub fn with_gate(config: AudiusConfig, gate: &'static NetworkGate) -> Self {
        Self {
            config,
            gate,
            base: API_BASE.to_string(),
            seen: parking_lot::Mutex::new(HashSet::new()),
            last_page: parking_lot::Mutex::new(Vec::new()),
        }
    }

    fn list(&self, kind: &str, offset: usize) -> Result<Vec<serde_json::Value>, RemoteError> {
        let path = format!(
            "/users/{}/{kind}?limit={PAGE}&offset={offset}",
            self.config.user_id
        );
        Ok(get_data(self.gate, &self.base, &path)?
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// Favorites list ids only, so their tracks are fetched in one request.
    /// One page of favorites, as tracks, with the raw ids the page listed.
    fn favorite_tracks(
        &self,
        offset: usize,
    ) -> Result<(Vec<serde_json::Value>, Vec<String>), RemoteError> {
        let favorites = self.list("favorites", offset)?;
        let listed: Vec<String> = favorites
            .iter()
            .filter_map(|f| id_field(f, "favorite_item_id"))
            .collect();
        let ids: Vec<String> = favorites
            .iter()
            .filter(|f| str_field(f, "favorite_type").is_some_and(|t| t.ends_with("track")))
            .filter_map(|f| id_field(f, "favorite_item_id"))
            .collect();
        if ids.is_empty() {
            return Ok((Vec::new(), listed));
        }
        let query: Vec<String> = ids.iter().map(|id| format!("id={id}")).collect();
        let tracks = get_data(
            self.gate,
            &self.base,
            &format!("/tracks?{}", query.join("&")),
        )?
        .as_array()
        .cloned()
        .unwrap_or_default();
        // The bulk lookup can answer with extra tracks; keep the asked-for
        // ones. The spec types the favorite's id as a string while the API
        // sends the numeric `track_id`, so either form counts.
        let wanted: HashSet<&String> = ids.iter().collect();
        let tracks = tracks
            .into_iter()
            .filter(|t| {
                ["track_id", "id"]
                    .iter()
                    .filter_map(|key| id_field(t, key))
                    .any(|id| wanted.contains(&id))
            })
            .collect();
        Ok((tracks, listed))
    }

    fn get(&self, part_key: &str, range: &str) -> Result<Response, RemoteError> {
        self.gate.check()?;
        let resp = stream_client()
            .get(stream_url(part_key))
            .header("Range", range)
            .send()
            .map_err(|_| unreachable())?;
        match resp.status().as_u16() {
            206 => Ok(resp),
            200 if range.starts_with("bytes=0-") => Ok(resp),
            404 => Err(RemoteError::Unreachable(
                "Audius no longer streams this track".into(),
            )),
            code => Err(RemoteError::Http(code)),
        }
    }
}

impl RemoteBackend for AudiusBackend {
    fn kind(&self) -> &'static str {
        "audius"
    }

    fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
        self.gate.check()?;
        if cursor == 0 {
            self.seen.lock().clear();
        }
        let raw_id = |item: &serde_json::Value| id_field(item, "id");
        let (items, listed, phase_start, next_phase): (Vec<serde_json::Value>, Vec<String>, _, _) =
            if cursor >= REPOSTS {
                let reposts = self.list("reposts", cursor - REPOSTS)?;
                let listed = reposts
                    .iter()
                    .filter_map(|r| r.get("item").and_then(raw_id))
                    .collect();
                let items = reposts
                    .into_iter()
                    .filter(|r| str_field(r, "item_type").as_deref() == Some("track"))
                    .filter_map(|r| r.get("item").cloned())
                    .collect();
                (items, listed, REPOSTS, None)
            } else if cursor >= FAVORITES {
                let (items, listed) = self.favorite_tracks(cursor - FAVORITES)?;
                (items, listed, FAVORITES, Some(REPOSTS))
            } else {
                let items = self.list("tracks", cursor)?;
                let listed = items.iter().filter_map(raw_id).collect();
                (items, listed, 0, Some(FAVORITES))
            };
        // A later offset answered with the very same page means the server
        // isn't paging this list; the phase is over rather than endless.
        let repeated =
            cursor != phase_start && !listed.is_empty() && *self.last_page.lock() == listed;
        if repeated {
            return Ok(CatalogPage {
                tracks: Vec::new(),
                total: None,
                next: next_phase,
            });
        }
        // A full page means there may be more of this phase.
        let next = if listed.len() == PAGE {
            Some(cursor + PAGE)
        } else {
            next_phase
        };
        *self.last_page.lock() = listed;
        let mut seen = self.seen.lock();
        let tracks = items
            .iter()
            .filter_map(track_to_meta)
            .filter(|meta| seen.insert(meta.remote_id.clone()))
            .collect();
        Ok(CatalogPage {
            tracks,
            total: None,
            next,
        })
    }

    fn open_bytes(&self, part_key: &str, offset: u64) -> Result<Box<dyn ByteStream>, RemoteError> {
        Ok(Box::new(self.get(part_key, &format!("bytes={offset}-"))?))
    }

    fn check_reachable(&self) -> Result<(), RemoteError> {
        get_data(
            self.gate,
            &self.base,
            &format!("/users/{}", self.config.user_id),
        )
        .map(|_| ())
    }

    fn fetch_artwork(&self, art_ref: &str) -> Result<Vec<u8>, RemoteError> {
        self.gate.check()?;
        let resp = api_client()
            .get(art_ref)
            .send()
            .map_err(|_| unreachable())?;
        if !resp.status().is_success() {
            return Err(RemoteError::Http(resp.status().as_u16()));
        }
        let mut bytes = Vec::new();
        resp.take(MAX_ART_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|_| unreachable())?;
        Ok(bytes)
    }

    fn content_length(&self, part_key: &str) -> Result<u64, RemoteError> {
        let resp = self.get(part_key, "bytes=0-0")?;
        total_size(&resp).ok_or_else(|| RemoteError::Parse("no size".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::gate::GateState;
    use crate::remote::testkit::fake_server;

    #[test]
    fn handles_come_from_whatever_was_pasted() {
        assert_eq!(parse_handle("@chillhopmusic").unwrap(), "chillhopmusic");
        assert_eq!(parse_handle(" omgkirbyDAO ").unwrap(), "omgkirbyDAO");
        assert_eq!(
            parse_handle("https://audius.co/chillhopmusic/reposts").unwrap(),
            "chillhopmusic"
        );
        assert!(parse_handle("").is_err());
        assert!(parse_handle("two words").is_err());
    }

    #[test]
    fn tracks_map_to_library_rows() {
        let item = serde_json::json!({
            "id": "YmJWK", "title": "Late Night", "duration": 190, "genre": "Electronic",
            "release_date": "2021-05-01T00:00:00Z", "created_at": "2021-05-02 10:00:00",
            "is_streamable": true, "is_stream_gated": false,
            "artwork": { "480x480": "https://node/art/480x480.jpg" },
            "album_backlink": { "playlist_name": "Nights" },
            "user": { "name": "kirby", "handle": "kirbyDAO" }
        });
        let meta = track_to_meta(&item).unwrap();
        assert_eq!(meta.remote_id, "YmJWK");
        assert_eq!(meta.part_key, "YmJWK");
        assert_eq!(meta.artist, "kirby");
        assert_eq!(meta.album, "Nights");
        assert_eq!(meta.year, Some(2021));
        assert_eq!(meta.duration_seconds, Some(190.0));
        assert_eq!(meta.size, 0);
        assert_eq!(
            meta.art_ref.as_deref(),
            Some("https://node/art/480x480.jpg")
        );
        assert!(meta.updated_at.is_some());

        let gated = serde_json::json!({ "id": "g", "is_stream_gated": true });
        assert!(track_to_meta(&gated).is_none());
    }

    fn backend_at(port: u16) -> AudiusBackend {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(true, true);
        let mut backend = AudiusBackend::with_gate(
            AudiusConfig {
                user_id: "u1".into(),
                handle: "h".into(),
                name: "n".into(),
            },
            gate,
        );
        backend.base = format!("http://127.0.0.1:{port}/v1");
        backend
    }

    fn data(items: &str) -> String {
        let body = format!(r#"{{"data":[{items}]}}"#);
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn track(id: &str, numeric: u64) -> String {
        format!(
            r#"{{"id":"{id}","track_id":{numeric},"title":"T{id}","is_streamable":true,"user":{{"name":"A"}}}}"#
        )
    }

    fn walk(backend: &AudiusBackend) -> Vec<String> {
        let mut cursor = Some(0);
        let mut ids = Vec::new();
        let mut pages = 0;
        while let Some(at) = cursor {
            let page = backend.catalog_page(at).unwrap();
            ids.extend(page.tracks.into_iter().map(|t| t.remote_id));
            cursor = page.next;
            pages += 1;
            assert!(pages < 20, "the walk never ended");
        }
        ids
    }

    #[test]
    fn favorites_match_by_either_id_form_and_reposts_follow() {
        let (port, seen) = fake_server(vec![
            data(&track("up1", 1)),
            // Favorites name tracks by the string form the spec gives.
            data(
                r#"{"favorite_item_id":"fav2","favorite_type":"SaveType.track","user_id":"u1","created_at":"x"},
                    {"favorite_item_id":"pl9","favorite_type":"SaveType.playlist","user_id":"u1","created_at":"x"}"#,
            ),
            data(&track("fav2", 2)),
            data(&format!(
                r#"{{"item_type":"track","item":{}}},{{"item_type":"track","item":{}}}"#,
                track("up1", 1),
                track("rep3", 3)
            )),
        ]);
        assert_eq!(walk(&backend_at(port)), ["up1", "fav2", "rep3"]);
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("GET /v1/users/u1/tracks?limit=100&offset=0&app_name=Wave "));
        assert!(
            log[2].starts_with("GET /v1/tracks?id=fav2&app_name=Wave "),
            "{}",
            log[2]
        );
    }

    #[test]
    fn a_list_that_ignores_the_offset_ends_its_phase() {
        let full: Vec<String> = (0..PAGE)
            .map(|i| track(&format!("t{i}"), i as u64))
            .collect();
        // The fake answers every request with the same full page of uploads,
        // as a server that ignores `offset` would, then nothing for the rest.
        let (port, _) = fake_server(vec![
            data(&full.join(",")),
            data(&full.join(",")),
            data(""),
            data(""),
        ]);
        assert_eq!(walk(&backend_at(port)).len(), PAGE);
    }

    #[test]
    fn a_closed_gate_blocks_every_call() {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        let backend = AudiusBackend::with_gate(
            AudiusConfig {
                user_id: "u".into(),
                handle: "h".into(),
                name: "n".into(),
            },
            gate,
        );
        assert_eq!(
            backend.catalog_page(0).err(),
            Some(RemoteError::Blocked(GateState::Disabled))
        );
        assert!(backend.open_bytes("t", 0).is_err());
        assert!(backend.content_length("t").is_err());
    }
}

/// Live checks against the real API. Run with
/// `cargo test --lib -- --ignored live_audius --nocapture`.
#[cfg(test)]
mod live_tests {
    use super::*;

    fn open_gate() -> &'static NetworkGate {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(true, true);
        gate
    }

    #[test]
    #[ignore = "hits the network"]
    fn live_audius_account_walks_every_phase_and_streams() {
        let gate = open_gate();
        let config = resolve(gate, "chillhopmusic").expect("handle should resolve");
        let backend = AudiusBackend::with_gate(config, gate);
        let mut cursor = Some(0);
        let mut tracks = Vec::new();
        let mut pages = 0;
        while let Some(at) = cursor {
            let page = backend.catalog_page(at).expect("catalog page");
            tracks.extend(page.tracks);
            cursor = page.next;
            pages += 1;
        }
        let unique: HashSet<_> = tracks.iter().map(|t| t.remote_id.clone()).collect();
        assert_eq!(unique.len(), tracks.len(), "a track was listed twice");
        assert!(tracks.len() > 100, "only {} tracks", tracks.len());

        let first = &tracks[0];
        let size = backend.content_length(&first.part_key).expect("size");
        let mut tail = Vec::new();
        backend
            .open_bytes(&first.part_key, size - 1000)
            .expect("ranged read")
            .read_to_end(&mut tail)
            .unwrap();
        assert_eq!(tail.len(), 1000);
        println!(
            "OK  {} tracks over {pages} pages; {} is {size} bytes",
            tracks.len(),
            first.title
        );
    }
}
