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
    // Playback state — saved on close, restored on launch.
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
}

fn default_gapless_enabled() -> bool {
    true
}

fn default_auto_lyrics_download() -> bool {
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
