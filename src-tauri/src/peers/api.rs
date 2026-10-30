// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The `/wave/` API another Wave talks to. It sits on the share server next
//! to the DLNA endpoints but answers on its own terms: anyone on the network
//! may say hello or ask to pair, everything else needs the secret the two
//! devices agreed on, and a device only ever sees what it was given.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::cast::{CastControlBody, CastPhase, CastPlay, CastState};
use super::{find_by_secret, scope_allows, PeerRecord, PendingRequest, Registry, ShareScope};
use crate::share::catalog::{extension_of, Snapshot};

/// Songs per catalog page.
pub const PAGE: usize = 200;
/// Shortest secret accepted in a request.
const MIN_SECRET: usize = 16;

/// What the API needs from the app around it, so the logic can be tested
/// without one.
pub trait Host: Send + Sync {
    /// This Wave's id and the name other devices see.
    fn identity(&self) -> (String, String);
    fn peers(&self) -> Vec<PeerRecord>;
    fn update_peers(&self, change: &mut dyn FnMut(&mut Vec<PeerRecord>));
    /// Audio fingerprints by track id.
    fn fingerprints(&self) -> HashMap<String, String>;
    fn emit(&self, event: &str, payload: serde_json::Value);
    /// A device was paired: start reading what it shares.
    fn paired(&self, peer: &PeerRecord);
    /// A device was forgotten: drop what it shared.
    fn forgotten(&self, peer_id: &str);
    /// A paired device came into reach or went out of it.
    fn reachable(&self, peer_id: &str, online: bool);
    /// A paired device wants a song played here.
    fn cast_play(&self, _from: &PeerRecord, _play: CastPlay) -> Result<(), String> {
        Err("This Wave can't be played to".to_string())
    }
    /// Pause, seek and the like, for what a paired device is playing here.
    fn cast_control(&self, _from: &PeerRecord, _control: CastControlBody) -> Result<(), String> {
        Err("This Wave can't be played to".to_string())
    }
    /// How what a paired device plays here is going.
    fn cast_state(&self, _from: &PeerRecord) -> CastState {
        CastState {
            session: 0,
            phase: CastPhase::Idle,
            position: 0.0,
            duration: None,
        }
    }
}

/// One request, as much of it as the API looks at.
pub struct PeerRequest<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a str,
    pub peer: Option<&'a str>,
    pub secret: Option<&'a str>,
    pub body: &'a [u8],
    pub from: IpAddr,
}

/// What the share server sends back.
#[derive(Debug, PartialEq)]
pub enum PeerReply {
    Json(&'static str, String),
    /// Serve this track's audio.
    Media(String),
    /// Serve this track's cover.
    Art(String),
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Hello {
    pub id: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AskBody {
    pub id: String,
    pub name: String,
    /// The port the asking Wave answers on; its address is where the request
    /// came from.
    pub port: u16,
    pub secret: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AnswerBody {
    pub id: String,
    pub name: String,
    pub port: u16,
    pub secret: String,
    pub accepted: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UnpairBody {
    pub id: String,
    pub secret: String,
}

/// One shared song in a catalog page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SharedTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album_artist: Option<String>,
    pub album: String,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub duration: Option<f64>,
    pub size: u64,
    pub extension: String,
    pub fingerprint: Option<String>,
    /// Whether `/wave/art/<id>.jpg` has a cover for it.
    pub art: bool,
}

impl SharedTrack {
    pub fn of(track: &crate::metadata::Track, fingerprint: Option<String>) -> Self {
        Self {
            id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album_artist: track.album_artist.clone(),
            album: track.album.clone(),
            track_number: track.track_number,
            disc_number: track.disc_number,
            year: track.year,
            genre: track.genre.clone(),
            duration: track.duration_seconds,
            size: u64::try_from(track.file_size).unwrap_or(0),
            extension: extension_of(track),
            fingerprint,
            art: track.cover_art_data_url.is_some() || track.album_art_id.is_some(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct CatalogPage {
    pub tracks: Vec<SharedTrack>,
    pub total: usize,
    pub next: Option<usize>,
}

pub struct PeerService {
    host: Arc<dyn Host>,
    pub registry: Mutex<Registry>,
}

fn json(status: &'static str, value: impl Serialize) -> PeerReply {
    PeerReply::Json(status, serde_json::to_string(&value).unwrap_or_default())
}

fn denied() -> PeerReply {
    json(
        "403 Forbidden",
        serde_json::json!({ "error": "not paired" }),
    )
}

fn address_of(from: IpAddr, port: u16) -> String {
    match from {
        IpAddr::V6(v6) if v6.to_ipv4_mapped().is_none() => format!("http://[{v6}]:{port}"),
        IpAddr::V6(v6) => format!("http://{}:{port}", v6.to_ipv4_mapped().unwrap()),
        IpAddr::V4(v4) => format!("http://{v4}:{port}"),
    }
}

impl PeerService {
    pub fn new(host: Arc<dyn Host>) -> Self {
        Self {
            host,
            registry: Mutex::new(Registry::default()),
        }
    }

    pub fn host(&self) -> &Arc<dyn Host> {
        &self.host
    }

    /// The reply to a `/wave/` request, or `None` for a path it doesn't know.
    pub fn handle(&self, request: &PeerRequest, snapshot: Option<&Snapshot>) -> Option<PeerReply> {
        match (request.method, request.path) {
            ("GET" | "HEAD", "/wave/hello") => {
                let (id, name) = self.host.identity();
                Some(json(
                    "200 OK",
                    Hello {
                        id,
                        name,
                        version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                ))
            }
            ("POST", "/wave/request") => Some(self.ask(request)),
            ("POST", "/wave/answer") => Some(self.answered(request)),
            ("POST", "/wave/unpair") => Some(self.unpair(request)),
            ("GET", "/wave/catalog") => Some(match self.authorised(request) {
                Some(peer) => self.catalog(&peer, request.query, snapshot),
                None => denied(),
            }),
            ("GET" | "HEAD", path)
                if path.starts_with("/wave/media/") || path.starts_with("/wave/art/") =>
            {
                let Some(peer) = self.authorised(request) else {
                    return Some(denied());
                };
                let art = path.starts_with("/wave/art/");
                let name = path.rsplit('/').next().unwrap_or_default();
                let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
                let id = crate::remote::uri::percent_decode(stem).unwrap_or_default();
                let allowed =
                    scope_allows(&peer.share, &id) || self.registry.lock().cast_to(&peer.id, &id);
                let shared =
                    !id.is_empty() && allowed && snapshot.is_some_and(|s| s.track(&id).is_some());
                if !shared {
                    return Some(json(
                        "404 Not Found",
                        serde_json::json!({ "error": "not shared" }),
                    ));
                }
                if art {
                    Some(PeerReply::Art(id))
                } else {
                    self.registry
                        .lock()
                        .streaming
                        .insert(peer.id, Instant::now());
                    Some(PeerReply::Media(id))
                }
            }
            ("POST", "/wave/cast/play") => Some(self.cast(request, |peer, body| {
                let play: CastPlay = serde_json::from_slice(body).map_err(|_| "unreadable")?;
                self.host.cast_play(peer, play).map_err(|_| "can't play")
            })),
            ("POST", "/wave/cast/control") => Some(self.cast(request, |peer, body| {
                let control: CastControlBody =
                    serde_json::from_slice(body).map_err(|_| "unreadable")?;
                self.host
                    .cast_control(peer, control)
                    .map_err(|_| "can't control")
            })),
            ("GET", "/wave/cast/state") => Some(match self.authorised(request) {
                Some(peer) => json("200 OK", self.host.cast_state(&peer)),
                None => denied(),
            }),
            _ => None,
        }
    }

    /// A cast request from a paired device, handed to `run` with its body.
    fn cast(
        &self,
        request: &PeerRequest,
        run: impl FnOnce(&PeerRecord, &[u8]) -> Result<(), &'static str>,
    ) -> PeerReply {
        let Some(peer) = self.authorised(request) else {
            return denied();
        };
        match run(&peer, request.body) {
            Ok(()) => json("202 Accepted", serde_json::json!({ "status": "playing" })),
            Err(error) => json("400 Bad Request", serde_json::json!({ "error": error })),
        }
    }

    /// Lets `peer_id` stream `track_id` from here while it plays it for
    /// this Wave, shared or not.
    pub fn grant_cast(&self, peer_id: &str, track_id: &str) {
        self.registry
            .lock()
            .cast_grants
            .entry(peer_id.to_string())
            .or_default()
            .insert(track_id.to_string());
    }

    /// The paired device a request's headers name, if its secret matches.
    fn authorised(&self, request: &PeerRequest) -> Option<PeerRecord> {
        let peers = self.host.peers();
        find_by_secret(&peers, request.peer?, request.secret?).cloned()
    }

    fn ask(&self, request: &PeerRequest) -> PeerReply {
        let Ok(ask) = serde_json::from_slice::<AskBody>(request.body) else {
            return json(
                "400 Bad Request",
                serde_json::json!({ "error": "unreadable" }),
            );
        };
        let (own_id, _) = self.host.identity();
        if ask.id.is_empty() || ask.id == own_id || ask.secret.len() < MIN_SECRET || ask.port == 0 {
            return json("400 Bad Request", serde_json::json!({ "error": "invalid" }));
        }
        let pending = PendingRequest {
            id: ask.id.clone(),
            name: ask.name.clone(),
            address: address_of(request.from, ask.port),
            secret: ask.secret,
            incoming: true,
            at: Instant::now(),
        };
        self.registry.lock().pending.insert(ask.id.clone(), pending);
        self.host.emit(
            "wave-peer-request",
            serde_json::json!({ "id": ask.id, "name": ask.name }),
        );
        json("202 Accepted", serde_json::json!({ "status": "asked" }))
    }

    fn answered(&self, request: &PeerRequest) -> PeerReply {
        let Ok(answer) = serde_json::from_slice::<AnswerBody>(request.body) else {
            return json(
                "400 Bad Request",
                serde_json::json!({ "error": "unreadable" }),
            );
        };
        let pending = {
            let mut registry = self.registry.lock();
            let matches = registry.pending.get(&answer.id).is_some_and(|p| {
                !p.incoming
                    && find_by_secret(
                        &[record_from(p, ShareScope::Nothing)],
                        &p.id,
                        &answer.secret,
                    )
                    .is_some()
            });
            if !matches {
                return denied();
            }
            registry.pending.remove(&answer.id)
        };
        let Some(pending) = pending else {
            return denied();
        };
        if !answer.accepted {
            self.host.emit(
                "wave-peer-declined",
                serde_json::json!({ "id": answer.id, "name": answer.name }),
            );
            return json("200 OK", serde_json::json!({ "status": "noted" }));
        }
        let mut record = record_from(&pending, ShareScope::Nothing);
        record.name = answer.name.clone();
        record.address = address_of(request.from, answer.port);
        self.store(&record);
        self.host.emit(
            "wave-peer-answered",
            serde_json::json!({ "id": answer.id, "name": answer.name }),
        );
        json("200 OK", serde_json::json!({ "status": "paired" }))
    }

    fn unpair(&self, request: &PeerRequest) -> PeerReply {
        let Ok(body) = serde_json::from_slice::<UnpairBody>(request.body) else {
            return json(
                "400 Bad Request",
                serde_json::json!({ "error": "unreadable" }),
            );
        };
        let peers = self.host.peers();
        if find_by_secret(&peers, &body.id, &body.secret).is_none() {
            return denied();
        }
        self.forget(&body.id);
        json("200 OK", serde_json::json!({ "status": "forgotten" }))
    }

    fn catalog(&self, peer: &PeerRecord, query: &str, snapshot: Option<&Snapshot>) -> PeerReply {
        let Some(snapshot) = snapshot else {
            return json(
                "503 Service Unavailable",
                serde_json::json!({ "error": "library not ready" }),
            );
        };
        let cursor = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("cursor="))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        let fingerprints = self.host.fingerprints();
        let shared: Vec<_> = snapshot
            .tracks()
            .iter()
            .filter(|t| scope_allows(&peer.share, &t.id))
            .collect();
        let total = shared.len();
        let tracks = shared
            .iter()
            .skip(cursor)
            .take(PAGE)
            .map(|t| SharedTrack::of(t, fingerprints.get(&t.id).cloned()))
            .collect();
        let next = (cursor + PAGE < total).then_some(cursor + PAGE);
        json(
            "200 OK",
            CatalogPage {
                tracks,
                total,
                next,
            },
        )
    }

    fn store(&self, record: &PeerRecord) {
        let mut stored = record.clone();
        stored.paired_at = crate::library::now_timestamp();
        self.host.update_peers(&mut |peers| {
            peers.retain(|p| p.id != stored.id);
            peers.push(stored.clone());
        });
        self.host.paired(&stored);
    }

    /// Another Wave answered a hello at `address`. Keeps the found list and
    /// a paired device's address current, and says when a new one turns up.
    pub fn sighted(&self, id: &str, name: &str, address: &str) {
        let (own_id, _) = self.host.identity();
        if id.is_empty() || id == own_id {
            return;
        }
        let paired = self.host.peers().into_iter().find(|p| p.id == id);
        if let Some(peer) = &paired {
            if peer.address != address || peer.name != name {
                let (address, name) = (address.to_string(), name.to_string());
                self.host.update_peers(&mut |peers| {
                    if let Some(p) = peers.iter_mut().find(|p| p.id == id) {
                        p.address = address.clone();
                        p.name = name.clone();
                    }
                });
            }
        }
        let (new, announce) = {
            let mut registry = self.registry.lock();
            let new = !registry.found.contains_key(id);
            registry.found.insert(
                id.to_string(),
                super::FoundPeer {
                    name: name.to_string(),
                    address: address.to_string(),
                    last_seen: Instant::now(),
                },
            );
            let announce = paired.is_none()
                && !registry.dismissed.contains(id)
                && registry.announced.insert(id.to_string());
            (new, announce)
        };
        if new {
            if paired.is_some() {
                self.host.reachable(id, true);
            }
            self.host.emit("wave-peers-changed", serde_json::json!({}));
        }
        if announce {
            self.host.emit(
                "wave-peer-found",
                serde_json::json!({ "id": id, "name": name }),
            );
        }
    }

    /// Drop devices not seen for `max_age`, telling the app a paired one
    /// went out of reach.
    pub fn expire(&self, max_age: std::time::Duration) {
        let gone: Vec<String> = {
            let mut registry = self.registry.lock();
            let stale: Vec<String> = registry
                .found
                .iter()
                .filter(|(_, f)| f.last_seen.elapsed() > max_age)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &stale {
                registry.found.remove(id);
            }
            stale
        };
        if gone.is_empty() {
            return;
        }
        let paired = self.host.peers();
        for id in &gone {
            if paired.iter().any(|p| &p.id == id) {
                self.host.reachable(id, false);
            }
        }
        self.host.emit("wave-peers-changed", serde_json::json!({}));
    }

    /// Drop a device on this side.
    pub fn forget(&self, id: &str) {
        self.host
            .update_peers(&mut |peers| peers.retain(|p| p.id != id));
        {
            let mut registry = self.registry.lock();
            registry.streaming.remove(id);
            registry.cast_grants.remove(id);
        }
        self.host.forgotten(id);
        self.host.emit("wave-peers-changed", serde_json::json!({}));
    }

    /// Allow an incoming request, sharing `share` with the device.
    pub fn accept(&self, id: &str, share: ShareScope) -> Result<PeerRecord, String> {
        let pending = self
            .registry
            .lock()
            .pending
            .remove(id)
            .filter(|p| p.incoming)
            .ok_or("That request is no longer waiting")?;
        let record = record_from(&pending, share);
        self.store(&record);
        Ok(record)
    }

    /// Turn an incoming request down.
    pub fn decline(&self, id: &str) -> Option<PendingRequest> {
        let mut registry = self.registry.lock();
        let incoming = registry.pending.get(id).is_some_and(|p| p.incoming);
        incoming.then(|| registry.pending.remove(id)).flatten()
    }

    /// Note a request this device sent, so the answer can be checked.
    pub fn asked(&self, id: &str, name: &str, address: &str, secret: &str) {
        self.registry.lock().pending.insert(
            id.to_string(),
            PendingRequest {
                id: id.to_string(),
                name: name.to_string(),
                address: address.to_string(),
                secret: secret.to_string(),
                incoming: false,
                at: Instant::now(),
            },
        );
    }
}

fn record_from(pending: &PendingRequest, share: ShareScope) -> PeerRecord {
    PeerRecord {
        id: pending.id.clone(),
        name: pending.name.clone(),
        address: pending.address.clone(),
        secret: pending.secret.clone(),
        share,
        source_id: None,
        paired_at: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::Track;
    use std::net::Ipv4Addr;

    #[derive(Default)]
    struct FakeHost {
        peers: Mutex<Vec<PeerRecord>>,
        events: Mutex<Vec<String>>,
        paired: Mutex<Vec<String>>,
        forgotten: Mutex<Vec<String>>,
    }

    impl Host for FakeHost {
        fn identity(&self) -> (String, String) {
            ("me".into(), "Wave on me".into())
        }
        fn peers(&self) -> Vec<PeerRecord> {
            self.peers.lock().clone()
        }
        fn update_peers(&self, change: &mut dyn FnMut(&mut Vec<PeerRecord>)) {
            change(&mut self.peers.lock());
        }
        fn fingerprints(&self) -> HashMap<String, String> {
            HashMap::from([("t1".to_string(), "a1:one".to_string())])
        }
        fn emit(&self, event: &str, _payload: serde_json::Value) {
            self.events.lock().push(event.to_string());
        }
        fn paired(&self, peer: &PeerRecord) {
            self.paired.lock().push(peer.id.clone());
        }
        fn forgotten(&self, peer_id: &str) {
            self.forgotten.lock().push(peer_id.to_string());
        }
        fn reachable(&self, peer_id: &str, online: bool) {
            self.events
                .lock()
                .push(format!("reachable:{peer_id}:{online}"));
        }
    }

    const SECRET: &str = "0123456789abcdef0123";
    const PHONE: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7));

    fn service() -> (Arc<FakeHost>, PeerService) {
        let host = Arc::new(FakeHost::default());
        (host.clone(), PeerService::new(host))
    }

    fn snapshot() -> Snapshot {
        let track = |id: &str| Track {
            id: id.into(),
            path: format!("/m/{id}.mp3"),
            title: id.to_uppercase(),
            artist: "Band".into(),
            album: "Album".into(),
            format: "mp3".into(),
            file_size: 10,
            ..Track::default()
        };
        Snapshot::build(1, vec![track("t1"), track("t2"), track("t3")])
    }

    fn call<'a>(
        service: &PeerService,
        method: &'a str,
        path: &'a str,
        query: &'a str,
        auth: Option<(&'a str, &'a str)>,
        body: &'a str,
    ) -> Option<PeerReply> {
        let snapshot = snapshot();
        service.handle(
            &PeerRequest {
                method,
                path,
                query,
                peer: auth.map(|a| a.0),
                secret: auth.map(|a| a.1),
                body: body.as_bytes(),
                from: PHONE,
            },
            Some(&snapshot),
        )
    }

    fn paired_with(host: &FakeHost, share: ShareScope) {
        host.peers.lock().push(PeerRecord {
            id: "phone".into(),
            name: "Wave on Android".into(),
            address: "http://10.0.0.7:50800".into(),
            secret: SECRET.into(),
            share,
            source_id: None,
            paired_at: 0,
        });
    }

    #[test]
    fn hello_says_who_this_is() {
        let (_, service) = service();
        let Some(PeerReply::Json(status, body)) =
            call(&service, "GET", "/wave/hello", "", None, "")
        else {
            panic!("no reply");
        };
        assert_eq!(status, "200 OK");
        let hello: Hello = serde_json::from_str(&body).unwrap();
        assert_eq!(
            (hello.id.as_str(), hello.name.as_str()),
            ("me", "Wave on me")
        );
    }

    #[test]
    fn a_request_waits_for_the_user_with_the_senders_real_address() {
        let (host, service) = service();
        let body = format!(
            r#"{{"id":"phone","name":"Wave on Android","port":50800,"secret":"{SECRET}"}}"#
        );
        let reply = call(&service, "POST", "/wave/request", "", None, &body).unwrap();
        assert!(matches!(reply, PeerReply::Json("202 Accepted", _)));
        let pending = service
            .registry
            .lock()
            .pending
            .get("phone")
            .cloned()
            .unwrap();
        assert!(pending.incoming);
        assert_eq!(pending.address, "http://10.0.0.7:50800");
        assert_eq!(host.events.lock().as_slice(), ["wave-peer-request"]);
    }

    #[test]
    fn a_request_with_a_short_secret_or_from_itself_is_refused() {
        let (_, service) = service();
        let short = r#"{"id":"phone","name":"x","port":1,"secret":"abc"}"#;
        assert!(matches!(
            call(&service, "POST", "/wave/request", "", None, short),
            Some(PeerReply::Json("400 Bad Request", _))
        ));
        let own = format!(r#"{{"id":"me","name":"x","port":1,"secret":"{SECRET}"}}"#);
        assert!(matches!(
            call(&service, "POST", "/wave/request", "", None, &own),
            Some(PeerReply::Json("400 Bad Request", _))
        ));
    }

    #[test]
    fn accepting_pairs_with_the_shared_scope() {
        let (host, service) = service();
        let body = format!(
            r#"{{"id":"phone","name":"Wave on Android","port":50800,"secret":"{SECRET}"}}"#
        );
        call(&service, "POST", "/wave/request", "", None, &body);
        let record = service
            .accept("phone", ShareScope::Tracks(vec!["t1".into()]))
            .unwrap();
        assert_eq!(record.secret, SECRET);
        assert_eq!(
            host.peers.lock()[0].share,
            ShareScope::Tracks(vec!["t1".into()])
        );
        assert_eq!(host.paired.lock().as_slice(), ["phone"]);
        assert!(service.registry.lock().pending.is_empty());
        assert!(service.accept("phone", ShareScope::Everything).is_err());
    }

    #[test]
    fn an_answer_counts_only_for_a_request_this_device_sent() {
        let (host, service) = service();
        let answer = format!(
            r#"{{"id":"phone","name":"Wave on Android","port":50800,"secret":"{SECRET}","accepted":true}}"#
        );
        assert!(matches!(
            call(&service, "POST", "/wave/answer", "", None, &answer),
            Some(PeerReply::Json("403 Forbidden", _))
        ));
        service.asked("phone", "Wave on Android", "http://10.0.0.7:50800", SECRET);
        let wrong = answer.replace(SECRET, "ffffffffffffffffffff");
        assert!(matches!(
            call(&service, "POST", "/wave/answer", "", None, &wrong),
            Some(PeerReply::Json("403 Forbidden", _))
        ));
        assert!(matches!(
            call(&service, "POST", "/wave/answer", "", None, &answer),
            Some(PeerReply::Json("200 OK", _))
        ));
        let peers = host.peers.lock().clone();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].share, ShareScope::Nothing);
        assert_eq!(host.paired.lock().as_slice(), ["phone"]);
        assert!(host
            .events
            .lock()
            .contains(&"wave-peer-answered".to_string()));
    }

    #[test]
    fn the_catalog_shows_only_what_was_shared() {
        let (host, service) = service();
        assert!(matches!(
            call(&service, "GET", "/wave/catalog", "", None, ""),
            Some(PeerReply::Json("403 Forbidden", _))
        ));
        paired_with(&host, ShareScope::Tracks(vec!["t1".into(), "t3".into()]));
        let Some(PeerReply::Json("200 OK", body)) = call(
            &service,
            "GET",
            "/wave/catalog",
            "cursor=0",
            Some(("phone", SECRET)),
            "",
        ) else {
            panic!("no catalog");
        };
        let page: CatalogPage = serde_json::from_str(&body).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.next, None);
        let ids: Vec<_> = page.tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["t1", "t3"]);
        assert_eq!(page.tracks[0].fingerprint.as_deref(), Some("a1:one"));
        assert_eq!(page.tracks[0].extension, "mp3");
    }

    #[test]
    fn media_outside_the_share_is_not_served_even_with_a_secret() {
        let (host, service) = service();
        paired_with(&host, ShareScope::Tracks(vec!["t1".into()]));
        assert_eq!(
            call(
                &service,
                "GET",
                "/wave/media/t1.mp3",
                "",
                Some(("phone", SECRET)),
                ""
            ),
            Some(PeerReply::Media("t1".into()))
        );
        assert!(matches!(
            call(
                &service,
                "GET",
                "/wave/media/t2.mp3",
                "",
                Some(("phone", SECRET)),
                ""
            ),
            Some(PeerReply::Json("404 Not Found", _))
        ));
        assert!(matches!(
            call(
                &service,
                "GET",
                "/wave/media/t1.mp3",
                "",
                Some(("phone", "nope-nope-nope-nope")),
                ""
            ),
            Some(PeerReply::Json("403 Forbidden", _))
        ));
        assert_eq!(
            call(
                &service,
                "GET",
                "/wave/art/t1.jpg",
                "",
                Some(("phone", SECRET)),
                ""
            ),
            Some(PeerReply::Art("t1".into()))
        );
        assert!(service.registry.lock().streaming.contains_key("phone"));
    }

    #[test]
    fn unpair_needs_the_secret_and_forgets_the_device() {
        let (host, service) = service();
        paired_with(&host, ShareScope::Everything);
        let bad = r#"{"id":"phone","secret":"wrong-wrong-wrong-wrong"}"#;
        assert!(matches!(
            call(&service, "POST", "/wave/unpair", "", None, bad),
            Some(PeerReply::Json("403 Forbidden", _))
        ));
        let good = format!(r#"{{"id":"phone","secret":"{SECRET}"}}"#);
        assert!(matches!(
            call(&service, "POST", "/wave/unpair", "", None, &good),
            Some(PeerReply::Json("200 OK", _))
        ));
        assert!(host.peers.lock().is_empty());
        assert_eq!(host.forgotten.lock().as_slice(), ["phone"]);
    }

    #[test]
    fn catalog_pages_follow_on() {
        let (host, service) = service();
        paired_with(&host, ShareScope::Everything);
        let tracks: Vec<Track> = (0..PAGE + 5)
            .map(|i| Track {
                id: format!("t{i}"),
                path: format!("/m/{i}.mp3"),
                format: "mp3".into(),
                ..Track::default()
            })
            .collect();
        let snapshot = Snapshot::build(1, tracks);
        let page = |cursor: &str| {
            let Some(PeerReply::Json(_, body)) = service.handle(
                &PeerRequest {
                    method: "GET",
                    path: "/wave/catalog",
                    query: cursor,
                    peer: Some("phone"),
                    secret: Some(SECRET),
                    body: b"",
                    from: PHONE,
                },
                Some(&snapshot),
            ) else {
                panic!()
            };
            serde_json::from_str::<CatalogPage>(&body).unwrap()
        };
        let first = page("cursor=0");
        assert_eq!(
            (first.tracks.len(), first.next, first.total),
            (PAGE, Some(PAGE), PAGE + 5)
        );
        let second = page(&format!("cursor={PAGE}"));
        assert_eq!((second.tracks.len(), second.next), (5, None));
    }

    #[test]
    fn a_new_wave_is_announced_once_and_never_itself() {
        let (host, service) = service();
        service.sighted("me", "Wave on me", "http://10.0.0.1:50800");
        assert!(host.events.lock().is_empty());
        service.sighted("phone", "Wave on Android", "http://10.0.0.7:50800");
        service.sighted("phone", "Wave on Android", "http://10.0.0.7:50800");
        let found = host
            .events
            .lock()
            .iter()
            .filter(|e| *e == "wave-peer-found")
            .count();
        assert_eq!(found, 1);
        assert!(service.registry.lock().found.contains_key("phone"));
    }

    #[test]
    fn a_dismissed_wave_is_not_announced_again() {
        let (host, service) = service();
        service.registry.lock().dismissed.insert("phone".into());
        service.sighted("phone", "Wave on Android", "http://10.0.0.7:50800");
        assert!(!host.events.lock().contains(&"wave-peer-found".to_string()));
    }

    #[test]
    fn a_paired_wave_that_moved_keeps_working() {
        let (host, service) = service();
        paired_with(&host, ShareScope::Everything);
        service.sighted("phone", "Phone", "http://10.0.0.99:41000");
        let peer = host.peers.lock()[0].clone();
        assert_eq!(
            (peer.address.as_str(), peer.name.as_str()),
            ("http://10.0.0.99:41000", "Phone")
        );
        assert!(host
            .events
            .lock()
            .contains(&"reachable:phone:true".to_string()));
        assert!(!host.events.lock().contains(&"wave-peer-found".to_string()));
    }

    #[test]
    fn a_wave_not_seen_for_a_while_goes_offline() {
        let (host, service) = service();
        paired_with(&host, ShareScope::Everything);
        service.sighted("phone", "Wave on Android", "http://10.0.0.7:50800");
        service.expire(std::time::Duration::from_secs(60));
        assert!(service.registry.lock().found.contains_key("phone"));
        service.expire(std::time::Duration::ZERO);
        assert!(!service.registry.lock().found.contains_key("phone"));
        assert!(host
            .events
            .lock()
            .contains(&"reachable:phone:false".to_string()));
    }

    #[test]
    fn unknown_paths_are_left_alone() {
        let (_, service) = service();
        assert!(call(&service, "GET", "/wave/nope", "", None, "").is_none());
    }
}
