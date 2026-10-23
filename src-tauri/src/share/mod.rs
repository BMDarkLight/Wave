// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Sharing the library on the local network as a DLNA / UPnP media server.
//!
//! Off unless both Outside Sourcing and "Share my library" are on. While it
//! runs, any DLNA player on the network (a TV, VLC, Kodi, another copy of
//! Wave) can browse the library by artist, album, song and playlist, and play
//! the files with seeking. Nothing is reachable from outside the local
//! network: requests from other addresses are dropped.
//!
//! The pieces are kept apart so later work can reuse them: [`catalog`] turns
//! the library into objects, [`http`] serves them, [`ssdp`] makes the server
//! findable.

pub mod catalog;
pub mod http;
pub mod ssdp;

use crate::library::Library;
use catalog::{PlaylistSource, Snapshot};
use http::{Catalog, Opener, Server};
use parking_lot::Mutex;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The port asked for first, so a client that remembered the address finds
/// the server again after a restart. Any free port is used when it's taken.
const PREFERRED_PORT: u16 = 50800;

/// What the server is started with.
#[derive(Clone)]
pub struct ShareConfig {
    pub name: String,
    pub uuid: String,
    pub db_path: PathBuf,
    pub opener: Opener,
}

/// The settings screen's view of the server.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ShareStatus {
    pub running: bool,
    pub name: String,
    /// `http://<address>:<port>` for each network the server is on.
    pub addresses: Vec<String>,
    pub track_count: usize,
    pub error: Option<String>,
}

/// The library, read through a connection of the server's own, and the last
/// snapshot built from it.
struct LibraryCatalog {
    library: Mutex<Library>,
    /// The last snapshot and when its version was last confirmed.
    cached: Mutex<Option<(Arc<Snapshot>, Instant)>>,
}

/// How long a snapshot is trusted without asking the library again. A
/// player seeking through a song sends a request per seek, and each version
/// check counts the whole library.
const FRESH_FOR: Duration = Duration::from_secs(2);

impl LibraryCatalog {
    fn rebuild(&self, version: u32) -> Result<Arc<Snapshot>, String> {
        let library = self.library.lock();
        let tracks = library.shareable_tracks()?;
        let default_playlist = library.default_playlist_id().ok();
        let mut playlists = Vec::new();
        for info in library.list_playlists(None)? {
            // The default playlist is the whole library; it adds nothing.
            if Some(&info.id) == default_playlist.as_ref() {
                continue;
            }
            playlists.push(PlaylistSource {
                paths: library.playlist_track_paths(&info.id)?,
                id: info.id,
                name: info.name,
            });
        }
        Ok(Arc::new(Snapshot::build(version, tracks, playlists)))
    }
}

impl Catalog for LibraryCatalog {
    /// The snapshot, rebuilt only when the library has changed since.
    fn snapshot(&self) -> Result<Arc<Snapshot>, String> {
        if let Some((snapshot, checked)) = self.cached.lock().as_ref() {
            if checked.elapsed() < FRESH_FOR {
                return Ok(snapshot.clone());
            }
        }
        let version = self.library.lock().share_version()?;
        if let Some((snapshot, checked)) = self.cached.lock().as_mut() {
            if snapshot.version == version {
                *checked = Instant::now();
                return Ok(snapshot.clone());
            }
        }
        let snapshot = self.rebuild(version)?;
        *self.cached.lock() = Some((snapshot.clone(), Instant::now()));
        Ok(snapshot)
    }
}

struct Running {
    config_key: (String, String, PathBuf),
    port: u16,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    catalog: Arc<LibraryCatalog>,
    name: String,
    error: Arc<Mutex<Option<String>>>,
}

fn state() -> &'static Mutex<Option<Running>> {
    static STATE: OnceLock<Mutex<Option<Running>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn last_error() -> &'static Mutex<Option<String>> {
    static ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    ERROR.get_or_init(|| Mutex::new(None))
}

fn bind_http() -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::UNSPECIFIED, PREFERRED_PORT))
        .or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)))
}

fn start(config: ShareConfig) -> Result<Running, String> {
    let library = Library::new_with_path(&config.db_path)?;
    let catalog = Arc::new(LibraryCatalog {
        library: Mutex::new(library),
        cached: Mutex::new(None),
    });
    let listener = bind_http().map_err(|e| format!("Couldn't open a port to share on: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let error = Arc::new(Mutex::new(None));
    let server = Arc::new(Server {
        name: config.name.clone(),
        uuid: config.uuid.clone(),
        catalog: catalog.clone(),
        opener: config.opener.clone(),
    });

    let mut threads = Vec::new();
    let http_stop = stop.clone();
    threads.push(
        std::thread::Builder::new()
            .name("wave-share-http".into())
            .spawn(move || http::serve(listener, server, http_stop))
            .map_err(|e| e.to_string())?,
    );
    let ssdp_stop = stop.clone();
    let ssdp_error = error.clone();
    let uuid = config.uuid.clone();
    threads.push(
        std::thread::Builder::new()
            .name("wave-share-ssdp".into())
            .spawn(move || {
                if let Err(e) = ssdp::run(uuid, port, ssdp_stop) {
                    tracing::warn!("Library sharing can't announce itself: {e}");
                    *ssdp_error.lock() = Some(format!(
                        "Other devices can't find Wave on their own ({e}); they can still add it at its address"
                    ));
                }
            })
            .map_err(|e| e.to_string())?,
    );
    #[cfg(target_os = "android")]
    crate::android::multicast::acquire();
    tracing::info!("Sharing the library as \"{}\" on port {port}", config.name);
    Ok(Running {
        config_key: (config.name.clone(), config.uuid, config.db_path),
        port,
        stop,
        threads,
        catalog,
        name: config.name,
        error,
    })
}

fn stop(running: Running) {
    running.stop.store(true, Ordering::Relaxed);
    for thread in running.threads {
        let _ = thread.join();
    }
    #[cfg(target_os = "android")]
    crate::android::multicast::release();
    tracing::info!("Stopped sharing the library");
}

/// Starts, restarts or stops the server to match `config`; `None` stops it.
pub fn apply(config: Option<ShareConfig>) {
    let mut slot = state().lock();
    let wanted = config
        .as_ref()
        .map(|c| (c.name.clone(), c.uuid.clone(), c.db_path.clone()));
    if slot.as_ref().map(|r| r.config_key.clone()) == wanted {
        return;
    }
    if let Some(running) = slot.take() {
        stop(running);
    }
    *last_error().lock() = None;
    if let Some(config) = config {
        match start(config) {
            Ok(running) => *slot = Some(running),
            Err(e) => {
                tracing::warn!("Couldn't share the library: {e}");
                *last_error().lock() = Some(e);
            }
        }
    }
}

pub fn is_running() -> bool {
    state().lock().is_some()
}

pub fn status() -> ShareStatus {
    let slot = state().lock();
    let Some(running) = slot.as_ref() else {
        return ShareStatus {
            error: last_error().lock().clone(),
            ..ShareStatus::default()
        };
    };
    let addresses = crate::remote::dlna::multicast_interfaces()
        .into_iter()
        .filter(|ip| !ip.is_unspecified() && !ip.is_loopback())
        .map(|ip| format!("http://{ip}:{}", running.port))
        .collect();
    let error = running.error.lock().clone();
    let track_count = running
        .catalog
        .snapshot()
        .map(|s| s.track_count())
        .unwrap_or(0);
    ShareStatus {
        running: true,
        name: running.name.clone(),
        addresses,
        track_count,
        error,
    }
}

/// The name other devices see when the user hasn't picked one.
pub fn default_name() -> String {
    format!("Wave on {}", device_name())
}

fn device_name() -> String {
    #[cfg(target_os = "android")]
    {
        "Android".to_string()
    }
    #[cfg(all(unix, not(target_os = "android")))]
    {
        nix::unistd::gethostname()
            .ok()
            .and_then(|h| h.into_string().ok())
            .map(|h| h.trim_end_matches(".local").to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "this computer".to_string())
    }
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this computer".to_string())
    }
}

/// Opens a shared file. Plain paths everywhere; on Android, `content://`
/// documents go through the ContentResolver like every other SAF read.
pub fn file_opener(app: Option<tauri::AppHandle>) -> Opener {
    Arc::new(move |path: &str| {
        if path.starts_with("content://") {
            // Through the Application context first, which works with no
            // window open, as sharing in the background needs.
            #[cfg(target_os = "android")]
            if let Ok(file) = crate::android::share_service::open_document(path) {
                return Ok(file);
            }
            let Some(app) = app.as_ref() else {
                return Err(std::io::Error::other("no app to open a document with"));
            };
            return crate::android::saf_io::open_for_reading(app, path)
                .map_err(std::io::Error::other);
        }
        std::fs::File::open(path)
    })
}
