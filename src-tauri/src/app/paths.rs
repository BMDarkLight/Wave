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

/// Cache for audio streamed from a remote source. Files here are disposable:
/// they back `source_state = 'cached'` rows and are evicted under a size cap.
pub fn source_cache_dir() -> PathBuf {
    data_dir().join("source-cache")
}

/// Where downloads land on desktop. Registered as a media folder on first use
/// so a download is browsable immediately, even if no media folder was ever
/// configured. Android downloads go to the primary media folder instead — see
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
