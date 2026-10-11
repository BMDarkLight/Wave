// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

#[cfg(target_os = "windows")]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
#[cfg(not(target_os = "android"))]
use std::time::Duration;
use tauri::AppHandle;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_seconds: Option<f64>,
    pub cover_url: Option<String>,
}

#[cfg(target_os = "windows")]
mod cover_art {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::path::PathBuf;

    pub struct Cache {
        path: Option<PathBuf>,
    }

    impl Cache {
        pub fn new() -> Self {
            Self { path: None }
        }

        pub fn resolve_path(&mut self, cover_url: Option<&str>) -> Option<PathBuf> {
            let url = cover_url?;
            // Prefer shared album thumb file paths, no temp rewrite.
            if let Some(path) = url.strip_prefix("file://") {
                let p = PathBuf::from(path);
                if p.is_file() {
                    return Some(p);
                }
            }
            if std::path::Path::new(url).is_file() {
                return Some(PathBuf::from(url));
            }
            if url.starts_with("data:") {
                let (header, data) = url.split_once(',')?;
                let mime = header.strip_prefix("data:")?.split(';').next()?;
                let ext = match mime {
                    "image/jpeg" | "image/jpg" => "jpg",
                    "image/png" => "png",
                    "image/gif" => "gif",
                    "image/webp" => "webp",
                    "image/bmp" => "bmp",
                    _ => return None,
                };
                let bytes = STANDARD.decode(data).ok()?;
                let path = std::env::temp_dir().join(format!(
                    "wave-cover-{}-{}.{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0),
                    ext
                ));
                std::fs::write(&path, bytes).ok()?;
                if let Some(old) = self.path.replace(path.clone()) {
                    let _ = std::fs::remove_file(old);
                }
                return Some(path);
            }
            None
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod cover_art {
    pub struct Cache;

    impl Cache {
        pub fn new() -> Self {
            Self
        }

        /// Preserve embedded artwork as a data URL so the native Android
        /// plugin can decode it directly. This avoids Android's cleartext
        /// HTTP restrictions, which can block loopback artwork servers.
        #[cfg(target_os = "android")]
        pub fn resolve_artwork_url(&mut self, cover_url: Option<&str>) -> Option<String> {
            normalize_cover_url(cover_url)
        }

        /// Souvlaki (macOS/Linux) needs a real URL with a scheme. Bare
        /// absolute thumb paths from the library crash NSURL / MPRIS.
        #[cfg(not(target_os = "android"))]
        pub fn resolve_url(&mut self, cover_url: Option<&str>) -> Option<String> {
            normalize_cover_url(cover_url)
        }
    }

    fn normalize_cover_url(cover_url: Option<&str>) -> Option<String> {
        let url = cover_url?.trim();
        if url.is_empty() {
            return None;
        }

        if url.starts_with("http://")
            || url.starts_with("https://")
            || url.starts_with("data:")
            || url.starts_with("file://")
        {
            return Some(url.to_string());
        }

        let path = std::path::Path::new(url);
        if !path.is_absolute() {
            return None;
        }
        // Prefer canonicalize so spaces / symlinks become a real file URL.
        let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !abs.is_file() {
            return None;
        }
        url::Url::from_file_path(&abs)
            .ok()
            .map(|u| u.to_string())
            .or_else(|| {
                // Fallback for exotic paths Url rejects.
                let s = abs.to_string_lossy();
                #[cfg(windows)]
                {
                    Some(format!("file:///{}", s.replace('\\', "/")))
                }
                #[cfg(not(windows))]
                {
                    Some(format!("file://{s}"))
                }
            })
    }
}

#[cfg(target_os = "windows")]
struct MediaBridge {
    backend: crate::os_media::WindowsMedia,
    cover_art_cache: cover_art::Cache,
}

#[cfg(target_os = "android")]
struct MediaBridge {
    cover_art_cache: cover_art::Cache,
    /// Last track metadata, resent when playback resumes after the
    /// notification was dismissed.
    last_meta: Option<TrackMetadata>,
    cleared: bool,
    speed: f64,
    last_playing: Option<bool>,
    last_position: f64,
    last_sent: Option<std::time::Instant>,
}

/// Position reports within this many seconds of where the session already
/// extrapolates are not worth a JNI round trip.
#[cfg(target_os = "android")]
const ANDROID_DRIFT_TOLERANCE_SECS: f64 = 1.0;

#[cfg(all(not(target_os = "windows"), not(target_os = "android")))]
struct MediaBridge {
    controls: souvlaki::MediaControls,
    cover_art_cache: cover_art::Cache,
}

unsafe impl Send for MediaBridge {}

impl MediaBridge {
    fn new(app: &AppHandle) -> Result<Self, String> {
        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                backend: crate::os_media::WindowsMedia::new(app)?,
                cover_art_cache: cover_art::Cache::new(),
            })
        }

        #[cfg(target_os = "android")]
        {
            // The session itself is driven over JNI (see android::media_bridge);
            // the plugin asks for POST_NOTIFICATIONS when its WebView loads.
            let _ = app;
            Ok(Self {
                cover_art_cache: cover_art::Cache::new(),
                last_meta: None,
                cleared: false,
                speed: 1.0,
                last_playing: None,
                last_position: 0.0,
                last_sent: None,
            })
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "android")))]
        {
            use souvlaki::{MediaControlEvent, MediaControls, PlatformConfig};
            use tauri::Emitter;

            let config = PlatformConfig {
                dbus_name: "wave",
                display_name: "Wave",
                hwnd: None,
            };

            let mut controls = MediaControls::new(config)
                .map_err(|e| format!("Failed to init media controls: {e:?}"))?;

            let app_handle = app.clone();
            controls
                .attach(move |event: MediaControlEvent| {
                    let event_name = match event {
                        MediaControlEvent::Play => "media-control-play",
                        MediaControlEvent::Pause => "media-control-pause",
                        MediaControlEvent::Toggle => "media-control-toggle",
                        MediaControlEvent::Next => "media-control-next",
                        MediaControlEvent::Previous => "media-control-previous",
                        MediaControlEvent::Stop => "media-control-stop",
                        MediaControlEvent::Seek(dir) => {
                            let payload = format!("{:?}", dir).to_lowercase();
                            let _ = app_handle.emit("media-control-seek-relative", payload);
                            return;
                        }
                        MediaControlEvent::SeekBy(dir, dur) => {
                            use souvlaki::SeekDirection;
                            let secs = match dir {
                                SeekDirection::Forward => dur.as_secs_f64(),
                                SeekDirection::Backward => -dur.as_secs_f64(),
                            };
                            let _ = app_handle.emit("media-control-seek-by", secs);
                            return;
                        }
                        MediaControlEvent::SetPosition(pos) => {
                            let _ =
                                app_handle.emit("media-control-set-position", pos.0.as_secs_f64());
                            return;
                        }
                        MediaControlEvent::OpenUri(_)
                        | MediaControlEvent::Raise
                        | MediaControlEvent::Quit
                        | MediaControlEvent::SetVolume(_) => return,
                    };
                    let _ = app_handle.emit(event_name, ());
                })
                .map_err(|e| format!("Failed to attach media controls handler: {e:?}"))?;

            Ok(Self {
                controls,
                cover_art_cache: cover_art::Cache::new(),
            })
        }
    }

    fn set_metadata(&mut self, meta: &TrackMetadata) {
        #[cfg(target_os = "windows")]
        {
            let preferred = crate::cover_art::prefer_media_artwork_url(meta.cover_url.as_deref());
            let cover_path = self.cover_art_cache.resolve_path(preferred.as_deref());
            let cover_ref = cover_path.as_ref().and_then(|p| p.to_str());
            self.backend.set_metadata(meta, cover_ref);
        }

        #[cfg(target_os = "android")]
        {
            // Always send artworkUrl, empty when there is no cover, so a track
            // without art does not inherit the previous track's.
            let artwork_url = self
                .cover_art_cache
                .resolve_artwork_url(
                    crate::cover_art::prefer_media_artwork_url(meta.cover_url.as_deref())
                        .as_deref(),
                )
                .unwrap_or_default();
            let mut state = serde_json::json!({
                "title": meta.title.clone().unwrap_or_default(),
                "artist": meta.artist.clone().unwrap_or_default(),
                "album": meta.album.clone().unwrap_or_default(),
                "artworkUrl": artwork_url,
                "canPrev": true,
                "canNext": true,
                "canSeek": true,
            });
            if let Some(duration) = meta.duration_seconds.filter(|d| d.is_finite()) {
                state["duration"] = duration.into();
            }
            crate::android::media_bridge::session_update(&state.to_string());
            self.last_meta = Some(meta.clone());
            self.cleared = false;
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "android")))]
        {
            use souvlaki::MediaMetadata;
            let duration = meta.duration_seconds.map(Duration::from_secs_f64);
            // Prefer 512px media-session JPEG over the 96px UI list thumb.
            let cover_url = self.cover_art_cache.resolve_url(
                crate::cover_art::prefer_media_artwork_url(meta.cover_url.as_deref()).as_deref(),
            );
            if let Err(error) = self.controls.set_metadata(MediaMetadata {
                title: meta.title.as_deref(),
                artist: meta.artist.as_deref(),
                album: meta.album.as_deref(),
                duration,
                cover_url: cover_url.as_deref(),
            }) {
                tracing::warn!("Failed to update OS media metadata: {error:?}");
            }
        }
    }

    /// Push the shuffle/repeat state to the shuffle/repeat notification
    /// buttons. No-op on platforms whose OS media widgets don't expose
    /// shuffle/repeat controls.
    #[allow(unused_variables)]
    fn set_playback_mode(&mut self, shuffle_enabled: bool, repeat_mode: &str) {
        #[cfg(target_os = "android")]
        {
            let state = serde_json::json!({
                "shuffleEnabled": shuffle_enabled,
                "repeatMode": repeat_mode,
            });
            crate::android::media_bridge::session_update(&state.to_string());
        }
    }

    fn now_playing_at(&mut self, meta: &TrackMetadata, position_secs: f64) {
        self.set_metadata(meta);
        self.set_playing(position_secs);
    }

    fn set_playing(&mut self, position_secs: f64) {
        #[cfg(target_os = "windows")]
        {
            self.backend.set_playback(true, position_secs, false);
        }

        #[cfg(target_os = "android")]
        self.android_set_playback(true, position_secs);

        #[cfg(all(not(target_os = "windows"), not(target_os = "android")))]
        {
            use souvlaki::{MediaPlayback, MediaPosition};
            let pos = MediaPosition(Duration::from_secs_f64(position_secs));
            let _ = self.controls.set_playback(MediaPlayback::Playing {
                progress: Some(pos),
            });
        }
    }

    fn set_paused(&mut self, position_secs: f64) {
        #[cfg(target_os = "windows")]
        {
            self.backend.set_playback(false, position_secs, false);
        }

        #[cfg(target_os = "android")]
        self.android_set_playback(false, position_secs);

        #[cfg(all(not(target_os = "windows"), not(target_os = "android")))]
        {
            use souvlaki::{MediaPlayback, MediaPosition};
            let pos = MediaPosition(Duration::from_secs_f64(position_secs));
            let _ = self.controls.set_playback(MediaPlayback::Paused {
                progress: Some(pos),
            });
        }
    }

    fn set_stopped(&mut self) {
        #[cfg(target_os = "windows")]
        {
            self.backend.set_playback(false, 0.0, true);
        }

        #[cfg(target_os = "android")]
        {
            self.last_playing = None;
            self.last_sent = None;
            self.cleared = true;
            crate::android::media_bridge::session_clear();
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "android")))]
        {
            use souvlaki::MediaPlayback;
            let _ = self.controls.set_playback(MediaPlayback::Stopped);
        }
    }

    fn update_position(&mut self, position_secs: f64, playing: bool) {
        #[cfg(target_os = "android")]
        {
            if self.last_playing != Some(playing) {
                self.android_set_playback(playing, position_secs);
                return;
            }
            // The session extrapolates position on its own. Only correct it
            // when it has drifted, e.g. after a seek from the UI.
            if (position_secs - self.android_expected_position()).abs()
                < ANDROID_DRIFT_TOLERANCE_SECS
            {
                return;
            }
            crate::android::media_bridge::session_timeline(Some(position_secs), None, None);
            self.last_position = position_secs;
            self.last_sent = Some(std::time::Instant::now());
            return;
        }

        #[cfg(not(target_os = "android"))]
        if playing {
            self.set_playing(position_secs);
        } else {
            self.set_paused(position_secs);
        }
    }
}

#[cfg(target_os = "android")]
impl MediaBridge {
    fn android_set_playback(&mut self, playing: bool, position_secs: f64) {
        // After the notification was dismissed the session holds no track;
        // bring it back before resuming.
        if self.cleared {
            if let Some(meta) = self.last_meta.clone() {
                self.set_metadata(&meta);
            }
        }
        let state = serde_json::json!({
            "isPlaying": playing,
            "position": position_secs,
            "playbackSpeed": self.speed,
        });
        crate::android::media_bridge::session_update(&state.to_string());
        self.last_playing = Some(playing);
        self.last_position = position_secs;
        self.last_sent = Some(std::time::Instant::now());
    }

    fn android_expected_position(&self) -> f64 {
        match (self.last_playing, self.last_sent) {
            (Some(true), Some(at)) => self.last_position + at.elapsed().as_secs_f64() * self.speed,
            _ => self.last_position,
        }
    }

    fn set_speed(&mut self, speed: f64, position_secs: f64) {
        if !speed.is_finite() || speed <= 0.0 {
            return;
        }
        self.speed = speed;
        if self.last_playing.is_some() {
            crate::android::media_bridge::session_timeline(Some(position_secs), None, Some(speed));
            self.last_position = position_secs;
            self.last_sent = Some(std::time::Instant::now());
        }
    }
}

pub struct MediaBridgeState {
    bridge: Arc<Mutex<Option<MediaBridge>>>,
    app: AppHandle,
}

impl MediaBridgeState {
    pub fn new(app: AppHandle) -> Self {
        Self {
            bridge: Arc::new(Mutex::new(None)),
            app,
        }
    }

    fn is_initialized(&self) -> bool {
        self.bridge
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    fn store_bridge(&self, result: Result<MediaBridge, String>) {
        match result {
            Ok(bridge) => {
                if let Ok(mut guard) = self.bridge.lock() {
                    *guard = Some(bridge);
                }
                tracing::info!("OS media controls ready");
            }
            Err(error) => tracing::warn!("OS media controls unavailable: {error}"),
        }
    }

    pub fn ensure_initialized_main(&self) {
        if self.is_initialized() {
            return;
        }
        self.store_bridge(MediaBridge::new(&self.app));
    }

    pub fn ensure_initialized(&self) {
        if self.is_initialized() {
            return;
        }

        #[cfg(target_os = "windows")]
        {
            let app = self.app.clone();
            let (tx, rx) = mpsc::sync_channel(1);
            let init_app = app.clone();
            if app
                .run_on_main_thread(move || {
                    let _ = tx.send(MediaBridge::new(&init_app));
                })
                .is_err()
            {
                tracing::warn!("Failed to schedule OS media controls init on main thread");
                return;
            }
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(result) => self.store_bridge(result),
                Err(_) => tracing::warn!("OS media controls init timed out, will retry"),
            }
        }

        #[cfg(target_os = "android")]
        {
            self.store_bridge(MediaBridge::new(&self.app));
            return;
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "android")))]
        self.store_bridge(MediaBridge::new(&self.app));
    }

    fn run_on_ui_thread<F>(&self, op: F)
    where
        F: FnOnce(&mut MediaBridge) + Send + 'static,
    {
        self.ensure_initialized();
        if !self.is_initialized() {
            tracing::warn!("OS media controls not initialized, skipping update");
            return;
        }

        #[cfg(target_os = "windows")]
        {
            let app = self.app.clone();
            let bridge = Arc::clone(&self.bridge);
            let (tx, rx) = mpsc::sync_channel(0);
            if app
                .run_on_main_thread(move || {
                    if let Ok(mut guard) = bridge.lock() {
                        if let Some(ref mut inner) = *guard {
                            op(inner);
                        }
                    }
                    let _ = tx.send(());
                })
                .is_err()
            {
                tracing::warn!("Failed to schedule OS media controls update on main thread");
                return;
            }
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }

        #[cfg(any(
            target_os = "android",
            all(not(target_os = "windows"), not(target_os = "android"))
        ))]
        {
            if let Ok(mut guard) = self.bridge.lock() {
                if let Some(ref mut inner) = *guard {
                    op(inner);
                }
            }
        }
    }

    pub fn set_metadata(&self, meta: TrackMetadata) {
        self.run_on_ui_thread(move |bridge| bridge.set_metadata(&meta));
    }

    pub fn now_playing(&self, meta: TrackMetadata) {
        self.now_playing_at(meta, 0.0);
    }

    pub fn now_playing_at(&self, meta: TrackMetadata, position_secs: f64) {
        self.run_on_ui_thread(move |bridge| bridge.now_playing_at(&meta, position_secs));
    }

    pub fn set_playing(&self, position_secs: f64) {
        self.run_on_ui_thread(move |bridge| bridge.set_playing(position_secs));
    }

    pub fn set_paused(&self, position_secs: f64) {
        self.run_on_ui_thread(move |bridge| bridge.set_paused(position_secs));
    }

    pub fn set_stopped(&self) {
        self.run_on_ui_thread(|bridge| bridge.set_stopped());
    }

    pub fn set_playback_mode(&self, shuffle_enabled: bool, repeat_mode: String) {
        self.run_on_ui_thread(move |bridge| {
            bridge.set_playback_mode(shuffle_enabled, &repeat_mode)
        });
    }

    pub fn update_position(&self, position_secs: f64, playing: bool) {
        self.run_on_ui_thread(move |bridge| bridge.update_position(position_secs, playing));
    }

    /// Tell the OS session about a playback speed change so its scrubber
    /// moves at the right rate. Only Android's session tracks speed.
    #[allow(unused_variables)]
    pub fn set_speed(&self, speed: f64, position_secs: f64, playing: bool) {
        #[cfg(target_os = "android")]
        self.run_on_ui_thread(move |bridge| bridge.set_speed(speed, position_secs));
        #[cfg(not(target_os = "android"))]
        self.update_position(position_secs, playing);
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    use super::cover_art::Cache;

    #[cfg(target_os = "windows")]
    #[test]
    fn cover_art_cache_writes_data_url_to_temp_file() {
        use std::fs;
        let data_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
        let mut cache = Cache::new();
        let path = cache.resolve_path(Some(data_url)).expect("should resolve");
        assert!(fs::metadata(&path).is_ok());
    }
}
