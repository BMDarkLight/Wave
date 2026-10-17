// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

pub mod backend;
pub mod gate;
pub mod plex;
pub mod refresh;
pub mod store;
pub mod stream;
pub mod sync;
pub mod uri;

use crate::library::Library;
use backend::RemoteBackend;
use parking_lot::{Mutex, RwLock};
use plex::connection::{PlexBackend, PlexSourceConfig};
use plex::PlexHeaders;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

/// Where a source stands as far as the UI is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Ok,
    Syncing,
    Unreachable,
    SignedOut,
    Blocked,
}

/// What the runtime tells its host. The host decides how to deliver these (a
/// Tauri event, a daemon log line), so nothing here depends on Tauri.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum RemoteEvent {
    SyncProgress {
        source_id: String,
        synced: usize,
        total: Option<usize>,
    },
    SyncFinished {
        source_id: String,
        error: Option<String>,
    },
    StatusChanged,
    StreamFailed {
        path: String,
        message: String,
    },
}

type EventSink = Box<dyn Fn(RemoteEvent) + Send + Sync>;

/// The live backends, their statuses and a database handle of their own, so
/// the GUI and the playback daemon can both drive remote sources.
pub struct RemoteRuntime {
    library: Mutex<Library>,
    backends: RwLock<HashMap<String, Arc<dyn RemoteBackend>>>,
    statuses: Mutex<HashMap<String, SourceState>>,
    syncing: Mutex<HashMap<String, Arc<AtomicBool>>>,
    plex_headers: RwLock<Option<PlexHeaders>>,
    events: EventSink,
}

static RUNTIME: OnceLock<RemoteRuntime> = OnceLock::new();

impl RemoteRuntime {
    pub fn global() -> Option<&'static RemoteRuntime> {
        RUNTIME.get()
    }

    /// Opens the runtime's own connection to `db_path`. A second call keeps
    /// the first runtime and drops its arguments. An unopenable database is an
    /// error for the caller to log: remote sources are optional.
    pub fn init_runtime(
        db_path: PathBuf,
        events: EventSink,
    ) -> Result<&'static RemoteRuntime, String> {
        if let Some(existing) = RUNTIME.get() {
            return Ok(existing);
        }
        let runtime = Self::open_runtime(&db_path, events)?;
        Ok(RUNTIME.get_or_init(|| runtime))
    }

    fn open_runtime(db_path: &std::path::Path, events: EventSink) -> Result<Self, String> {
        let library = Library::new_with_path(db_path)?;
        Ok(Self::from_library(library, events))
    }

    fn from_library(library: Library, events: EventSink) -> Self {
        // This is a second connection beside the GUI's, so a write that lands
        // while the other one holds the lock waits instead of failing.
        let _ = library
            .write_connection()
            .execute_batch("PRAGMA busy_timeout = 5000");
        Self {
            library: Mutex::new(library),
            backends: RwLock::new(HashMap::new()),
            statuses: Mutex::new(HashMap::new()),
            syncing: Mutex::new(HashMap::new()),
            plex_headers: RwLock::new(None),
            events,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(library: Library, events: EventSink) -> Self {
        Self::from_library(library, events)
    }

    #[cfg(test)]
    pub(crate) fn insert_backend_for_test(&self, source_id: &str, backend: Arc<dyn RemoteBackend>) {
        self.backends.write().insert(source_id.to_string(), backend);
    }

    pub fn emit(&self, event: RemoteEvent) {
        (self.events)(event);
    }

    /// Plex rows are skipped by [`Self::reload_sources`] until this is set.
    #[allow(dead_code)]
    pub fn set_plex_headers(&self, headers: PlexHeaders) {
        *self.plex_headers.write() = Some(headers);
    }

    pub fn backend(&self, source_id: &str) -> Option<Arc<dyn RemoteBackend>> {
        self.backends.read().get(source_id).cloned()
    }

    pub fn source_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.backends.read().keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Rebuilds every backend from the stored sources. Sources that are gone
    /// lose their backend and status.
    #[allow(dead_code)]
    pub fn reload_sources(&self, library: &Library) {
        let rows = match library.list_remote_sources() {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("Could not list remote sources: {e}");
                return;
            }
        };
        let headers = self.plex_headers.read().clone();
        let mut backends: HashMap<String, Arc<dyn RemoteBackend>> = HashMap::new();
        for row in rows {
            match row.kind.as_str() {
                "plex" => {
                    let Some(headers) = headers.clone() else {
                        tracing::warn!("Plex headers are not set; skipping {}", row.id);
                        continue;
                    };
                    match serde_json::from_str::<PlexSourceConfig>(&row.config_json) {
                        Ok(config) => {
                            backends.insert(row.id, Arc::new(PlexBackend::new(config, headers)));
                        }
                        Err(_) => tracing::warn!("Unreadable config for remote source {}", row.id),
                    }
                }
                other => tracing::warn!("Unknown remote source kind {other}; skipping {}", row.id),
            }
        }
        self.statuses
            .lock()
            .retain(|id, _| backends.contains_key(id));
        *self.backends.write() = backends;
        self.emit(RemoteEvent::StatusChanged);
    }

    #[allow(dead_code)]
    pub fn status(&self, source_id: &str) -> SourceState {
        self.statuses
            .lock()
            .get(source_id)
            .copied()
            .unwrap_or(SourceState::Ok)
    }

    pub fn set_status(&self, source_id: &str, state: SourceState) {
        let previous = self.statuses.lock().insert(source_id.to_string(), state);
        if previous.unwrap_or(SourceState::Ok) != state {
            self.emit(RemoteEvent::StatusChanged);
        }
    }

    #[allow(dead_code)]
    pub fn statuses(&self) -> Vec<(String, SourceState)> {
        let mut all: Vec<(String, SourceState)> = self
            .backends
            .read()
            .keys()
            .map(|id| (id.clone(), self.status(id)))
            .collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all
    }

    pub fn with_library<T>(&self, f: impl FnOnce(&Library) -> T) -> T {
        f(&self.library.lock())
    }

    /// The flag that marks a source as mid-sync.
    pub(crate) fn sync_slot(&self, source_id: &str) -> Arc<AtomicBool> {
        self.syncing
            .lock()
            .entry(source_id.to_string())
            .or_default()
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::plex::auth::PlexSection;
    use uuid::Uuid;

    fn test_library() -> Library {
        let dir: PathBuf =
            std::env::temp_dir().join(format!("wave-test-runtime-{}", Uuid::new_v4()));
        Library::new_with_path(&dir.join("library.db")).unwrap()
    }

    fn collector() -> (EventSink, Arc<std::sync::Mutex<Vec<RemoteEvent>>>) {
        let seen: Arc<std::sync::Mutex<Vec<RemoteEvent>>> = Arc::default();
        let sink = seen.clone();
        (Box::new(move |e| sink.lock().unwrap().push(e)), seen)
    }

    fn plex_config() -> String {
        let config = PlexSourceConfig {
            server_id: "srv".into(),
            server_name: "Home".into(),
            server_token: "t".into(),
            account_token: "a".into(),
            sections: vec![PlexSection {
                key: "3".into(),
                title: "Music".into(),
            }],
            connections: Vec::new(),
            preferred_uri: None,
        };
        serde_json::to_string(&config).unwrap()
    }

    #[test]
    fn set_status_emits_only_on_change() {
        let (events, seen) = collector();
        let rt = RemoteRuntime::new_for_test(test_library(), events);
        assert_eq!(rt.status("s"), SourceState::Ok);
        rt.set_status("s", SourceState::Ok);
        assert!(seen.lock().unwrap().is_empty());
        rt.set_status("s", SourceState::Unreachable);
        rt.set_status("s", SourceState::Unreachable);
        assert_eq!(*seen.lock().unwrap(), vec![RemoteEvent::StatusChanged]);
        assert_eq!(rt.status("s"), SourceState::Unreachable);
    }

    #[test]
    fn reload_builds_plex_backends_and_skips_the_rest() {
        let library = test_library();
        let plex = library
            .insert_remote_source("plex", "srv", "Home", &plex_config())
            .unwrap();
        let other = library
            .insert_remote_source("jellyfin", "j", "Other", "{}")
            .unwrap();
        let broken = library
            .insert_remote_source("plex", "bad", "Bad", "not json")
            .unwrap();
        let (events, _) = collector();
        let rt = RemoteRuntime::new_for_test(test_library(), events);

        rt.reload_sources(&library);
        assert!(rt.backend(&plex).is_none(), "no headers yet");

        rt.set_plex_headers(PlexHeaders::new("client-1".into()));
        rt.reload_sources(&library);
        assert_eq!(rt.backend(&plex).unwrap().kind(), "plex");
        assert!(rt.backend(&other).is_none());
        assert!(rt.backend(&broken).is_none());
        assert_eq!(rt.statuses(), vec![(plex, SourceState::Ok)]);
    }

    #[test]
    fn an_unopenable_database_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("wave-test-runtime-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("not-a-directory");
        std::fs::write(&file, b"x").unwrap();
        let (events, _) = collector();
        assert!(RemoteRuntime::open_runtime(&file.join("library.db"), events).is_err());
    }

    #[test]
    fn with_library_sets_a_busy_timeout() {
        let (events, _) = collector();
        let rt = RemoteRuntime::new_for_test(test_library(), events);
        let timeout: i64 = rt.with_library(|l| {
            l.read_connection()
                .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
                .unwrap()
        });
        assert_eq!(timeout, 5000);
    }
}
