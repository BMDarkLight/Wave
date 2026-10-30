// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Other Waves on the local network, and what this one shares with each.
//!
//! Two Waves find each other with [`discovery`], one asks and the other
//! allows, and from then on each reads what the other shares through the
//! `/wave/` API in [`api`], with [`client`] on the reading side. A paired
//! device is known by its id, so a new address or name changes nothing.

pub mod api;
pub mod cast;
pub mod client;
pub mod commands;
pub mod discovery;

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// What this Wave lets one paired device see.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "tracks", rename_all = "snake_case")]
pub enum ShareScope {
    /// The whole library, including songs added later.
    Everything,
    /// These songs only, by track id.
    Tracks(Vec<String>),
    /// Paired, but nothing shared from this side.
    #[default]
    Nothing,
}

/// A paired device, as saved in settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerRecord {
    /// The other Wave's own id.
    pub id: String,
    pub name: String,
    /// `http://<ip>:<port>`, updated whenever discovery sees it somewhere new.
    pub address: String,
    /// Proves a request comes from this device. Made by whoever asked.
    pub secret: String,
    /// What this side shares with it.
    #[serde(default)]
    pub share: ShareScope,
    /// The remote source holding what it shares with this side.
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub paired_at: i64,
}

pub fn scope_allows(scope: &ShareScope, track_id: &str) -> bool {
    match scope {
        ShareScope::Everything => true,
        ShareScope::Tracks(ids) => ids.iter().any(|id| id == track_id),
        ShareScope::Nothing => false,
    }
}

/// The paired device `id` whose secret is `secret`.
pub fn find_by_secret<'a>(
    peers: &'a [PeerRecord],
    id: &str,
    secret: &str,
) -> Option<&'a PeerRecord> {
    if secret.is_empty() {
        return None;
    }
    peers
        .iter()
        .find(|peer| peer.id == id && same_secret(&peer.secret, secret))
}

/// Compares every byte, so how long it takes says nothing about how much of
/// a guessed secret was right.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

/// A Wave seen on the network.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundPeer {
    pub name: String,
    pub address: String,
    pub last_seen: Instant,
}

/// A request waiting for an answer, on either side.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingRequest {
    pub id: String,
    pub name: String,
    pub address: String,
    pub secret: String,
    /// True on the device that was asked.
    pub incoming: bool,
    pub at: Instant,
}

/// What this run of Wave knows about other Waves, beyond settings.
#[derive(Debug, Default)]
pub struct Registry {
    pub found: HashMap<String, FoundPeer>,
    pub pending: HashMap<String, PendingRequest>,
    /// Devices whose "found" popup was turned down this session.
    pub dismissed: HashSet<String>,
    /// Devices the "found" popup was already shown for this session.
    pub announced: HashSet<String>,
    /// Devices that streamed a song from this one, and when they last did.
    pub streaming: HashMap<String, Instant>,
    /// Songs this Wave sent to a device to play, which it may stream
    /// whether they are shared with it or not.
    pub cast_grants: HashMap<String, HashSet<String>>,
}

impl Registry {
    pub fn cast_to(&self, peer_id: &str, track_id: &str) -> bool {
        self.cast_grants
            .get(peer_id)
            .is_some_and(|ids| ids.contains(track_id))
    }
}

static SERVICE: OnceLock<std::sync::Arc<api::PeerService>> = OnceLock::new();

/// The peer service the share server and the commands use.
pub fn service(app: &tauri::AppHandle) -> std::sync::Arc<api::PeerService> {
    SERVICE
        .get_or_init(|| {
            std::sync::Arc::new(api::PeerService::new(std::sync::Arc::new(AppHost(
                app.clone(),
            ))))
        })
        .clone()
}

/// A paired device as it stands now, and this Wave's own id, for the remote
/// source reading from it.
pub fn lookup(peer_id: &str) -> Option<(PeerRecord, String)> {
    let host = SERVICE.get()?.host().clone();
    let (own_id, _) = host.identity();
    let peer = host.peers().into_iter().find(|p| p.id == peer_id)?;
    Some((peer, own_id))
}

/// Forget a device on this side only, after it turned out to have forgotten
/// this one.
pub fn forget_locally(peer_id: &str) {
    let Some(service) = SERVICE.get().cloned() else {
        return;
    };
    let peer_id = peer_id.to_string();
    // The caller may hold the library; forgetting needs it too.
    let _ = std::thread::Builder::new()
        .name("wave-peer-forget".into())
        .spawn(move || service.forget(&peer_id));
}

/// The app as the peer API sees it: settings for identity and pairings, the
/// library for fingerprints, and events for the window.
struct AppHost(tauri::AppHandle);

impl AppHost {
    fn settings(&self) -> Option<tauri::State<'_, crate::app_settings::AppSettingsState>> {
        use tauri::Manager;
        self.0.try_state::<crate::app_settings::AppSettingsState>()
    }
}

impl api::Host for AppHost {
    fn identity(&self) -> (String, String) {
        let Some(state) = self.settings() else {
            return (String::new(), crate::share::default_name());
        };
        let Ok(settings) = state.0.lock() else {
            return (String::new(), crate::share::default_name());
        };
        (
            settings.share_device_id.clone().unwrap_or_default(),
            settings
                .share_name
                .clone()
                .unwrap_or_else(crate::share::default_name),
        )
    }

    fn peers(&self) -> Vec<PeerRecord> {
        self.settings()
            .and_then(|state| state.0.lock().ok().map(|s| s.wave_peers.clone()))
            .unwrap_or_default()
    }

    fn update_peers(&self, change: &mut dyn FnMut(&mut Vec<PeerRecord>)) {
        let Some(state) = self.settings() else {
            return;
        };
        let Ok(mut settings) = state.0.lock() else {
            return;
        };
        change(&mut settings.wave_peers);
        if let Err(e) = settings.save(&self.0) {
            tracing::warn!("Couldn't save paired devices: {e}");
        }
    }

    fn fingerprints(&self) -> HashMap<String, String> {
        use tauri::Manager;
        self.0
            .try_state::<crate::commands::LibraryState>()
            .and_then(|state| {
                state
                    .0
                    .lock()
                    .ok()
                    .and_then(|l| l.local_fingerprints().ok())
            })
            .map(|by_print| {
                by_print
                    .into_iter()
                    .map(|(print, id)| (id, print))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn emit(&self, event: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        let _ = self.0.emit(event, payload);
    }

    fn paired(&self, peer: &PeerRecord) {
        crate::remote::commands::add_wave_source(&self.0, peer);
    }

    fn forgotten(&self, peer_id: &str) {
        crate::remote::commands::remove_wave_source(&self.0, peer_id);
    }

    fn reachable(&self, peer_id: &str, online: bool) {
        crate::remote::commands::set_wave_reachable(&self.0, peer_id, online);
    }

    fn cast_play(&self, from: &PeerRecord, play: cast::CastPlay) -> Result<(), String> {
        cast::receive_play(&self.0, from, play)
    }

    fn cast_control(
        &self,
        from: &PeerRecord,
        control: cast::CastControlBody,
    ) -> Result<(), String> {
        cast::receive_control(&self.0, from, control)
    }

    fn cast_state(&self, from: &PeerRecord) -> cast::CastState {
        cast::receive_state(&self.0, from)
    }
}

/// Forget the paired Wave behind a remote source, on both sides: the other
/// device is told, then everything it shared goes from here.
pub fn forget_by_source(app: &tauri::AppHandle, source_id: &str) {
    let service = service(app);
    let host = service.host().clone();
    let (own_id, _) = host.identity();
    let peer = host
        .peers()
        .into_iter()
        .find(|p| crate::remote::commands::wave_source_id(&p.id) == source_id);
    match peer {
        Some(peer) => {
            let told = client::unpair(
                &peer.address,
                &api::UnpairBody {
                    id: own_id,
                    secret: peer.secret.clone(),
                },
            );
            if let Err(e) = told {
                // It drops the pairing the next time it reaches this Wave and
                // is turned away.
                tracing::debug!("Couldn't tell {} it was forgotten: {e}", peer.name);
            }
            service.forget(&peer.id);
        }
        // A playlist left from a pairing that is already gone.
        None => crate::remote::commands::remove_wave_source_by_id(app, source_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, secret: &str) -> PeerRecord {
        PeerRecord {
            id: id.into(),
            name: "Wave on test".into(),
            address: "http://10.0.0.2:50800".into(),
            secret: secret.into(),
            share: ShareScope::Everything,
            source_id: None,
            paired_at: 0,
        }
    }

    #[test]
    fn scopes_decide_what_is_seen() {
        assert!(scope_allows(&ShareScope::Everything, "any"));
        let picked = ShareScope::Tracks(vec!["a".into(), "b".into()]);
        assert!(scope_allows(&picked, "a"));
        assert!(!scope_allows(&picked, "c"));
        assert!(!scope_allows(&ShareScope::Nothing, "a"));
    }

    #[test]
    fn a_peer_is_found_only_with_its_own_secret() {
        let peers = vec![record("phone", "s1"), record("pc", "s2")];
        assert_eq!(
            find_by_secret(&peers, "phone", "s1").map(|p| p.id.as_str()),
            Some("phone")
        );
        assert!(find_by_secret(&peers, "phone", "s2").is_none());
        assert!(find_by_secret(&peers, "tablet", "s1").is_none());
        assert!(find_by_secret(&peers, "phone", "").is_none());
    }

    #[test]
    fn records_round_trip_and_old_settings_load() {
        let mut peer = record("phone", "s1");
        peer.share = ShareScope::Tracks(vec!["t1".into()]);
        let json = serde_json::to_string(&peer).unwrap();
        assert_eq!(serde_json::from_str::<PeerRecord>(&json).unwrap(), peer);
        let settings: crate::app_settings::AppSettings = serde_json::from_str("{}").unwrap();
        assert!(settings.wave_peers.is_empty());
    }
}
