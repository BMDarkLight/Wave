// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

use std::path::PathBuf;

/// Application data directory (`app.bmdarklight.wave` under the platform data root).
pub fn data_dir() -> PathBuf {
    if let Some(base) = data_root() {
        base.join("app.bmdarklight.wave")
    } else {
        PathBuf::from(".")
    }
}

/// Default SQLite library database path.
pub fn library_db_path() -> PathBuf {
    if let Ok(path) = std::env::var("WAVE_DB_PATH") {
        return PathBuf::from(path);
    }
    data_dir().join("wave-library.sqlite")
}

/// Path to the playback daemon state file (pid + port).
pub fn daemon_state_path() -> PathBuf {
    data_dir().join("playback-daemon.json")
}

/// Where the desktop app publishes the loopback port (and token) that a second
/// launch uses to hand it files to open.
pub fn gui_handoff_path() -> PathBuf {
    data_dir().join("wave-gui.json")
}

/// Cache for audio streamed from a remote source. Files here are disposable:
/// they back `source_state = 'cached'` rows and are evicted under a size cap.
pub fn source_cache_dir() -> PathBuf {
    data_dir().join("source-cache")
}

/// Where downloads land on desktop. Registered as a media folder on first use
/// so a download is browsable immediately, even if no media folder was ever
/// configured. Android downloads go to the primary media folder instead; see
/// `sources::download`.
pub fn downloads_dir() -> PathBuf {
    data_dir().join("Downloads")
}

/// Path to the primary-instance lock file.
pub fn instance_lock_path() -> PathBuf {
    data_dir().join("wave-instance.lock")
}

#[cfg(target_os = "macos")]
fn data_root() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join("Library/Application Support"))
}

#[cfg(target_os = "linux")]
fn data_root() -> Option<PathBuf> {
    std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local/share"))
        })
}

#[cfg(target_os = "windows")]
fn data_root() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(PathBuf::from)
}

#[cfg(target_os = "android")]
fn data_root() -> Option<PathBuf> {
    // Prefer the app sandbox home when the runtime provides it.
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| Some(std::env::temp_dir()))
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "windows",
    target_os = "android"
)))]
fn data_root() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".local/share"))
}

static APP_DATA_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static APP_CACHE_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Tauri's app data directory, resolved once and then served from memory.
///
/// Use this instead of `app.path().app_data_dir()`. On Android that call is a
/// plugin round trip through the Activity, and once Android has destroyed the
/// Activity (task swiped away while music plays) it panics inside Tauri, which
/// aborts the whole process. The directory never changes, so one lookup while
/// the UI exists is enough.
pub fn app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    cached_dir(&APP_DATA_DIR, || {
        tauri::Manager::path(app)
            .app_data_dir()
            .map_err(|e| e.to_string())
    })
}

/// Tauri's app cache directory, cached like [`app_data_dir`].
pub fn app_cache_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    cached_dir(&APP_CACHE_DIR, || {
        tauri::Manager::path(app)
            .app_cache_dir()
            .map_err(|e| e.to_string())
    })
}

/// Resolve both directories up front, while the Activity is still alive.
pub fn warm_app_dirs(app: &tauri::AppHandle) {
    if let Err(error) = app_data_dir(app) {
        tracing::warn!("Could not resolve the app data directory: {error}");
    }
    if let Err(error) = app_cache_dir(app) {
        tracing::warn!("Could not resolve the app cache directory: {error}");
    }
}

fn cached_dir(
    cell: &std::sync::OnceLock<PathBuf>,
    resolve: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    if let Some(dir) = cell.get() {
        return Ok(dir.clone());
    }
    // Belt and braces for a lookup that slips through after the Activity is
    // gone: report an error instead of letting Tauri's panic abort playback.
    let dir = std::panic::catch_unwind(std::panic::AssertUnwindSafe(resolve))
        .map_err(|_| "the app directory cannot be resolved without a UI".to_string())??;
    Ok(cell.get_or_init(|| dir).clone())
}
