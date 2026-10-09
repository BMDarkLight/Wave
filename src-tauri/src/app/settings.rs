// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::audio::dsp::EqConfig;
use crate::audio::player::RepeatMode;
use crate::dto::CloseAction;

const SETTINGS_FILE: &str = "wave-settings.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub close_action: CloseAction,
    pub volume: f32,
    pub equalizer: EqConfig,
    // Playback state. Saved on close, restored on launch.
    pub last_track_path: Option<String>,
    pub last_position_seconds: f64,
    pub last_queue: Vec<String>,
    pub last_queue_index: Option<usize>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    // Media folders scanned for music (content:// URIs on Android, paths on desktop).
    pub media_folders: Vec<String>,
    /// User dismissed the Android first-run "select music folder" prompt.
    pub folder_setup_dismissed: bool,
    /// Seamless transitions between consecutive queue tracks (no dead air).
    #[serde(default = "default_gapless_enabled")]
    pub gapless_enabled: bool,
    /// Automatically fetch missing lyrics from the network when a track plays.
    #[serde(default = "default_auto_lyrics_download")]
    pub auto_lyrics_download: bool,
    /// Boost quieter tracks toward the median loudness of played tracks.
    #[serde(default)]
    pub volume_normalization_enabled: bool,
    /// Draw the seek bar as the track's waveform, moving with the music
    /// around the playhead. Off gives the plain slider and skips analysis.
    #[serde(default = "default_waveform_seek_enabled")]
    pub waveform_seek_enabled: bool,
    /// Blur what sits behind panels, bars and sheets. Off paints them solid,
    /// which is cheaper to draw on slower phones.
    #[serde(default = "default_glass_effects_enabled")]
    pub glass_effects_enabled: bool,
    /// Slide, fade and scroll the interface. Off makes every change instant.
    #[serde(default = "default_animations_enabled")]
    pub animations_enabled: bool,
    /// While animations are on, fade panels in place instead of sliding them
    /// and drop the smaller flourishes.
    #[serde(default)]
    pub reduce_motion_enabled: bool,
    /// Master switch for the remote-source search tier. When off, no provider
    /// is queried and the escalation button never appears, so nothing in the
    /// app reaches the network for music discovery.
    ///
    /// Off by default because Wave is offline-first. Existing installs keep
    /// whatever they had; serde only applies this when the field is absent.
    #[serde(default = "default_outside_sourcing_enabled")]
    pub outside_sourcing_enabled: bool,
    /// Spotify application client id, used for OAuth PKCE.
    ///
    /// Spotify serves no audio through its API, so this powers catalogue search
    /// and playlist import only, never streaming or caching.
    #[serde(default)]
    pub spotify_client_id: Option<String>,
    /// Free Jamendo API client id. Empty means the Jamendo source reports
    /// itself as needing setup instead of silently returning nothing.
    #[serde(default)]
    pub jamendo_client_id: Option<String>,
    /// Size cap for streamed-audio cache. Exceeded only when every cached file
    /// is protected (playing or queued).
    #[serde(default = "default_source_cache_limit_mb")]
    pub source_cache_limit_mb: u64,
}

fn default_source_cache_limit_mb() -> u64 {
    512
}

fn default_outside_sourcing_enabled() -> bool {
    false
}

fn default_gapless_enabled() -> bool {
    true
}

fn default_auto_lyrics_download() -> bool {
    true
}

fn default_waveform_seek_enabled() -> bool {
    true
}

fn default_glass_effects_enabled() -> bool {
    true
}

fn default_animations_enabled() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            close_action: CloseAction::Quit,
            volume: 0.8,
            equalizer: EqConfig::default(),
            last_track_path: None,
            last_position_seconds: 0.0,
            last_queue: Vec::new(),
            last_queue_index: None,
            shuffle: false,
            repeat: RepeatMode::Off,
            media_folders: Vec::new(),
            folder_setup_dismissed: false,
            gapless_enabled: true,
            auto_lyrics_download: true,
            volume_normalization_enabled: false,
            waveform_seek_enabled: default_waveform_seek_enabled(),
            glass_effects_enabled: default_glass_effects_enabled(),
            animations_enabled: default_animations_enabled(),
            reduce_motion_enabled: false,
            outside_sourcing_enabled: default_outside_sourcing_enabled(),
            spotify_client_id: None,
            jamendo_client_id: None,
            source_cache_limit_mb: default_source_cache_limit_mb(),
        }
    }
}

pub struct AppSettingsState(pub Mutex<AppSettings>);

impl AppSettings {
    pub fn load(app: &tauri::AppHandle) -> Self {
        Self::load_from_path(&settings_path(app))
    }

    /// Load settings from the default CLI/desktop data directory (no Tauri handle).
    pub fn load_from_disk() -> Self {
        Self::load_from_path(&crate::app_paths::data_dir().join(SETTINGS_FILE))
    }

    fn load_from_path(path: &std::path::Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        let mut settings = match fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        settings.normalize();
        settings
    }

    pub fn save(&self, app: &tauri::AppHandle) -> Result<(), String> {
        self.save_to_path(&settings_path(app))
    }

    /// Persist settings to the default CLI/desktop data directory.
    pub fn save_to_disk(&self) -> Result<(), String> {
        self.save_to_path(&crate::app_paths::data_dir().join(SETTINGS_FILE))
    }

    fn save_to_path(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create settings directory: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?;
        fs::write(path, json).map_err(|e| format!("Failed to write settings: {e}"))
    }

    pub fn toggle_close_action(&mut self) {
        self.close_action = match self.close_action {
            CloseAction::Quit => CloseAction::HideWindow,
            CloseAction::HideWindow => CloseAction::Quit,
        };
    }

    fn normalize(&mut self) {
        if !self.volume.is_finite() {
            self.volume = Self::default().volume;
        }
        self.volume = self.volume.clamp(0.0, 1.0);
        for gain in &mut self.equalizer.bands {
            if !gain.is_finite() {
                *gain = 0.0;
            }
            *gain = gain.clamp(-24.0, 24.0);
        }
        if !self.equalizer.crossfade_duration.is_finite() {
            self.equalizer.crossfade_duration = 0.0;
        }
        self.equalizer.crossfade_duration = self.equalizer.crossfade_duration.clamp(0.0, 8.0);
        if !self.last_position_seconds.is_finite() {
            self.last_position_seconds = 0.0;
        }
        self.last_position_seconds = self.last_position_seconds.max(0.0);
    }
}

fn settings_path(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join(SETTINGS_FILE))
        .unwrap_or_else(|_| PathBuf::from(SETTINGS_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_saved_before_the_waveform_switch_keep_it_on() {
        let mut saved = serde_json::to_value(AppSettings::default()).unwrap();
        saved
            .as_object_mut()
            .unwrap()
            .remove("waveform_seek_enabled");
        let loaded: AppSettings = serde_json::from_value(saved).unwrap();
        assert!(loaded.waveform_seek_enabled);
    }

    #[test]
    fn turning_the_waveform_off_survives_a_save() {
        let settings = AppSettings {
            waveform_seek_enabled: false,
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: AppSettings = serde_json::from_str(&json).unwrap();
        assert!(!loaded.waveform_seek_enabled);
    }

    #[test]
    fn settings_saved_before_the_appearance_switches_keep_them_on() {
        let mut saved = serde_json::to_value(AppSettings::default()).unwrap();
        let fields = saved.as_object_mut().unwrap();
        fields.remove("glass_effects_enabled");
        fields.remove("animations_enabled");
        fields.remove("reduce_motion_enabled");
        let loaded: AppSettings = serde_json::from_value(saved).unwrap();
        assert!(loaded.glass_effects_enabled);
        assert!(loaded.animations_enabled);
        assert!(!loaded.reduce_motion_enabled);
    }

    #[test]
    fn turning_glass_and_animations_off_survives_a_save() {
        let settings = AppSettings {
            glass_effects_enabled: false,
            animations_enabled: false,
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: AppSettings = serde_json::from_str(&json).unwrap();
        assert!(!loaded.glass_effects_enabled);
        assert!(!loaded.animations_enabled);
    }

    #[test]
    fn turning_reduce_motion_on_survives_a_save() {
        let settings = AppSettings {
            reduce_motion_enabled: true,
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: AppSettings = serde_json::from_str(&json).unwrap();
        assert!(loaded.reduce_motion_enabled);
    }
}
