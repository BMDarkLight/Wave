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
            // Prefer shared album thumb file paths — no temp rewrite.
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
    app: AppHandle,
    cover_art_cache: cover_art::Cache,
    last_playing: Option<bool>,
}

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
            use tauri_plugin_media_session::MediaSessionExt;

            // Request POST_NOTIFICATIONS early so the first play can show the shade.
            if let Err(error) = app.media_session().initialize() {
                tracing::warn!("Android media session initialize: {error}");
            }

            Ok(Self {
                app: app.clone(),
                cover_art_cache: cover_art::Cache::new(),
                last_playing: None,
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

            let mut controls =
                MediaControls::new(config).map_err(|e| format!("Failed to init media controls: {e:?}"))?;

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
                            let _ = app_handle.emit(
                                "media-control-set-position",
                                pos.0.as_secs_f64(),
                            );
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
            let cover_path = self
                .cover_art_cache
                .resolve_path(preferred.as_deref());
            let cover_ref = cover_path.as_ref().and_then(|p| p.to_str());
            self.backend.set_metadata(meta, cover_ref);
        }

        #[cfg(target_os = "android")]
        {
            use tauri_plugin_media_session::{MediaSessionExt, MediaState};

            let artwork_url = self.cover_art_cache.resolve_artwork_url(
                crate::cover_art::prefer_media_artwork_url(meta.cover_url.as_deref()).as_deref(),
            );

            if let Err(error) = self.app.media_session().update_state(MediaState {
                title: meta.title.clone(),
                artist: meta.artist.clone(),
                album: meta.album.clone(),
                artwork_url,
                duration: meta.duration_seconds,
                can_prev: Some(true),
                can_next: Some(true),
                can_seek: Some(true),
                ..Default::default()
            }) {
                tracing::warn!("Failed to update Android media session metadata: {error}");
            }
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
            use tauri_plugin_media_session::{MediaSessionExt, MediaState};

            if let Err(error) = self.app.media_session().update_state(MediaState {
                shuffle_enabled: Some(shuffle_enabled),
                repeat_mode: Some(repeat_mode.to_string()),
                ..Default::default()
            }) {
                tracing::debug!("Android media playback-mode update skipped: {error}");
            }
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
        {
            use tauri_plugin_media_session::{MediaSessionExt, MediaState};

            self.last_playing = Some(true);
            if let Err(error) = self.app.media_session().update_state(MediaState {
                is_playing: Some(true),
                position: Some(position_secs),
                playback_speed: Some(1.0),
                can_prev: Some(true),
                can_next: Some(true),
                can_seek: Some(true),
                ..Default::default()
            }) {
                tracing::warn!("Failed to set Android media session playing: {error}");
            }
        }

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
        {
            use tauri_plugin_media_session::{MediaSessionExt, MediaState};

            self.last_playing = Some(false);
            if let Err(error) = self.app.media_session().update_state(MediaState {
                is_playing: Some(false),
                position: Some(position_secs),
                playback_speed: Some(1.0),
                can_prev: Some(true),
                can_next: Some(true),
                can_seek: Some(true),
                ..Default::default()
            }) {
                tracing::warn!("Failed to set Android media session paused: {error}");
            }
        }

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
            use tauri_plugin_media_session::MediaSessionExt;

            self.last_playing = None;
            if let Err(error) = self.app.media_session().clear() {
                tracing::warn!("Failed to clear Android media session: {error}");
            }
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
            use tauri_plugin_media_session::{MediaSessionExt, TimelineUpdate};

            // Avoid rebuilding the notification on every UI poll tick.
            if self.last_playing != Some(playing) {
                if playing {
                    self.set_playing(position_secs);
                } else {
                    self.set_paused(position_secs);
                }
                return;
            }

            if let Err(error) = self.app.media_session().update_timeline(TimelineUpdate {
                position: Some(position_secs),
                ..Default::default()
            }) {
                // Session may not exist yet (before first update_state).
                tracing::debug!("Android media timeline update skipped: {error}");
            }
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
                Err(_) => tracing::warn!("OS media controls init timed out — will retry"),
            }
            return;
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
            tracing::warn!("OS media controls not initialized — skipping update");
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
            return;
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
        self.run_on_ui_thread(move |bridge| bridge.set_playback_mode(shuffle_enabled, &repeat_mode));
    }

    pub fn update_position(&self, position_secs: f64, playing: bool) {
        self.run_on_ui_thread(move |bridge| bridge.update_position(position_secs, playing));
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
