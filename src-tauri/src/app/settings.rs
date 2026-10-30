// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::audio::dsp::EqConfig;
use crate::audio::player::RepeatMode;
use crate::dto::{CloseAction, ThemeMode};

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
    /// The saved queue is a mashup of several albums or playlists.
    pub last_queue_mashup: bool,
    /// The playlist the saved queue plays, so it keeps following it.
    pub last_queue_source: Option<SavedQueueSource>,
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
    /// Light, dark, or following the system.
    #[serde(default)]
    pub theme_mode: ThemeMode,
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
    /// Master switch for the remote library (Plex and later backends). Off
    /// hides every remote source and keeps the app from contacting them.
    #[serde(default)]
    pub remote_sources_enabled: bool,
    /// Allow remote streaming and caching over a metered connection.
    #[serde(default)]
    pub remote_cellular_enabled: bool,
    /// The name other devices see; `None` uses "Wave on <device>".
    #[serde(default)]
    pub share_name: Option<String>,
    /// Keep sharing with the window closed: on desktop closing hides Wave
    /// instead of quitting, on Android a foreground service keeps it running.
    #[serde(default)]
    pub share_in_background: bool,
    /// This Wave's id, made once so other Waves keep recognising it across
    /// launches.
    #[serde(default)]
    pub share_device_id: Option<String>,
    /// Other Waves this one is paired with, and what it shares with each.
    #[serde(default)]
    pub wave_peers: Vec<crate::peers::PeerRecord>,
    /// Plex client identifier, generated once and then kept so the server
    /// sees the same device across launches.
    #[serde(default)]
    pub plex_client_identifier: Option<String>,
    /// Keep coupled songs back to back in the queue.
    #[serde(default)]
    pub song_coupling_enabled: bool,
    /// Coupled songs, kept even while coupling is off so turning it back on
    /// brings them back.
    #[serde(default)]
    pub song_couples: Vec<SongCouple>,
    /// Offer to mash albums and playlists together into one shuffled queue.
    #[serde(default)]
    pub playlist_mashing_enabled: bool,
    /// Look for a newer release when Wave starts. On Android the new APK is
    /// also downloaded, ready to install. Off leaves a manual check button.
    #[serde(default = "default_auto_update_enabled")]
    pub auto_update_enabled: bool,
}

/// The playlist a saved queue was started from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQueueSource {
    pub playlist_id: String,
    /// The queue kept the playlist's own order rather than a sorted view's.
    pub in_playlist_order: bool,
}

/// Two songs, by path, that always play back to back: `first`, then
/// `second`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongCouple {
    pub first: String,
    pub second: String,
}

/// The old default, replaced because remote-library tracks share this cache.
const LEGACY_SOURCE_CACHE_LIMIT_MB: u64 = 512;

fn default_source_cache_limit_mb() -> u64 {
    if cfg!(target_os = "android") {
        1024
    } else {
        2048
    }
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

fn default_auto_update_enabled() -> bool {
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
            last_queue_mashup: false,
            last_queue_source: None,
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
            theme_mode: ThemeMode::System,
            outside_sourcing_enabled: default_outside_sourcing_enabled(),
            spotify_client_id: None,
            jamendo_client_id: None,
            source_cache_limit_mb: default_source_cache_limit_mb(),
            remote_sources_enabled: false,
            remote_cellular_enabled: false,
            share_name: None,
            share_in_background: false,
            share_device_id: None,
            wave_peers: Vec::new(),
            plex_client_identifier: None,
            song_coupling_enabled: false,
            song_couples: Vec::new(),
            playlist_mashing_enabled: false,
            auto_update_enabled: default_auto_update_enabled(),
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
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(settings) => settings,
                Err(e) => {
                    // The next save overwrites the file, so keep a copy of
                    // what could not be read before it is gone.
                    tracing::warn!("Settings file is not fully readable: {e}");
                    let mut backup = path.as_os_str().to_owned();
                    backup.push(".bak");
                    let _ = fs::copy(path, PathBuf::from(backup));
                    Self::salvage(&contents)
                }
            },
            Err(_) => Self::default(),
        };
        settings.normalize();
        settings
    }

    /// Keep every field that still reads and default the rest. One value
    /// this build does not know, such as a theme added by a newer version,
    /// used to throw away the media folders and the queue along with it.
    fn salvage(contents: &str) -> Self {
        let Ok(serde_json::Value::Object(fields)) = serde_json::from_str(contents) else {
            return Self::default();
        };
        let mut kept = serde_json::Map::new();
        for (key, value) in fields {
            kept.insert(key.clone(), value);
            if serde_json::from_value::<Self>(serde_json::Value::Object(kept.clone())).is_err() {
                kept.remove(&key);
            }
        }
        serde_json::from_value(serde_json::Value::Object(kept)).unwrap_or_default()
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
        let mut value =
            serde_json::to_value(self).map_err(|e| format!("Failed to serialize settings: {e}"))?;
        // Another version of Wave may share this file. Whatever it saved that
        // this one does not know is written back as it was, so running an
        // older build does not wipe a newer one's pairings and the like.
        if let (Some(ours), Ok(Ok(serde_json::Value::Object(theirs)))) = (
            value.as_object_mut(),
            fs::read_to_string(path).map(|s| serde_json::from_str(&s)),
        ) {
            for (key, field) in theirs {
                ours.entry(key).or_insert(field);
            }
        }
        let json = serde_json::to_string_pretty(&value)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?;
        // Saved every few seconds during playback, and the process can be
        // killed at any point (Android does so freely). Written beside the
        // file and renamed over it, so a cut-off write never leaves JSON that
        // loads as all defaults. The data is synced before the rename, or a
        // power cut can leave the renamed file empty. The temp name carries
        // the process id because the CLI and the daemon both save here.
        let mut temp = path.as_os_str().to_owned();
        temp.push(format!(".{}.tmp", std::process::id()));
        let temp = PathBuf::from(temp);
        let written = fs::File::create(&temp).and_then(|mut file| {
            file.write_all(json.as_bytes())?;
            file.sync_all()
        });
        if let Err(e) = written {
            let _ = fs::remove_file(&temp);
            return Err(format!("Failed to write settings: {e}"));
        }
        fs::rename(&temp, path).map_err(|e| {
            let _ = fs::remove_file(&temp);
            format!("Failed to write settings: {e}")
        })
    }

    /// The pairs the player should keep together: none while coupling is off.
    pub fn active_song_couples(&self) -> Vec<(String, String)> {
        if !self.song_coupling_enabled {
            return Vec::new();
        }
        self.song_couples
            .iter()
            .map(|c| (c.first.clone(), c.second.clone()))
            .collect()
    }

    /// Couple two songs. A song belongs to one couple at a time, so any
    /// couple either one was already in is dropped.
    pub fn couple_songs(&mut self, first: String, second: String) {
        self.uncouple_song(&first);
        self.uncouple_song(&second);
        if first != second {
            self.song_couples.push(SongCouple { first, second });
        }
    }

    /// Drop the couple `path` belongs to, if any.
    pub fn uncouple_song(&mut self, path: &str) {
        self.song_couples
            .retain(|c| c.first != path && c.second != path);
    }

    pub fn toggle_close_action(&mut self) {
        self.close_action = match self.close_action {
            CloseAction::Quit => CloseAction::HideWindow,
            CloseAction::HideWindow | CloseAction::Minimize => CloseAction::Quit,
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
            *gain = gain.clamp(-12.0, 12.0);
        }
        if !self.equalizer.crossfade_duration.is_finite() {
            self.equalizer.crossfade_duration = 0.0;
        }
        self.equalizer.crossfade_duration = self.equalizer.crossfade_duration.clamp(0.0, 8.0);
        if !self.last_position_seconds.is_finite() {
            self.last_position_seconds = 0.0;
        }
        self.last_position_seconds = self.last_position_seconds.max(0.0);
        if self.source_cache_limit_mb == LEGACY_SOURCE_CACHE_LIMIT_MB {
            self.source_cache_limit_mb = default_source_cache_limit_mb();
        }
    }
}

fn settings_path(app: &tauri::AppHandle) -> PathBuf {
    super::paths::app_data_dir(app)
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

    #[test]
    fn settings_saved_before_the_theme_choice_follow_the_system() {
        let mut saved = serde_json::to_value(AppSettings::default()).unwrap();
        saved.as_object_mut().unwrap().remove("theme_mode");
        let loaded: AppSettings = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded.theme_mode, ThemeMode::System);
    }

    #[test]
    fn a_chosen_theme_survives_a_save() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let settings = AppSettings {
                theme_mode: mode,
                ..AppSettings::default()
            };
            let json = serde_json::to_string(&settings).unwrap();
            let loaded: AppSettings = serde_json::from_str(&json).unwrap();
            assert_eq!(loaded.theme_mode, mode);
        }
    }

    #[test]
    fn a_song_is_in_one_couple_at_a_time() {
        let mut settings = AppSettings::default();
        settings.couple_songs("a".into(), "b".into());
        settings.couple_songs("c".into(), "a".into());
        assert_eq!(
            settings.song_couples,
            [SongCouple {
                first: "c".into(),
                second: "a".into()
            }]
        );
        settings.couple_songs("d".into(), "d".into());
        assert_eq!(settings.song_couples.len(), 1);
    }

    #[test]
    fn couples_only_reach_the_player_while_coupling_is_on() {
        let mut settings = AppSettings::default();
        settings.couple_songs("a".into(), "b".into());
        assert!(settings.active_song_couples().is_empty());
        settings.song_coupling_enabled = true;
        assert_eq!(
            settings.active_song_couples(),
            [("a".to_string(), "b".to_string())]
        );
    }

    #[test]
    fn settings_another_version_saved_are_kept() {
        let dir = std::env::temp_dir().join(format!("wave-test-settings-{}", uuid::Uuid::new_v4()));
        let path = dir.join(SETTINGS_FILE);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"volume": 0.5, "from_a_newer_wave": [1, 2]}"#).unwrap();

        let settings = AppSettings::load_from_path(&path);
        assert_eq!(settings.volume, 0.5);
        AppSettings {
            volume: 0.25,
            ..settings
        }
        .save_to_path(&path)
        .unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["volume"], 0.25);
        assert_eq!(saved["from_a_newer_wave"], serde_json::json!([1, 2]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_replaces_the_file_whole() {
        let dir = std::env::temp_dir().join(format!("wave-test-settings-{}", uuid::Uuid::new_v4()));
        let path = dir.join(SETTINGS_FILE);
        let settings = AppSettings {
            volume: 0.35,
            media_folders: vec!["/music".into()],
            ..AppSettings::default()
        };
        settings.save_to_path(&path).unwrap();
        settings.save_to_path(&path).unwrap();

        let loaded = AppSettings::load_from_path(&path);
        assert_eq!(loaded.volume, 0.35);
        assert_eq!(loaded.media_folders, ["/music"]);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy() != SETTINGS_FILE)
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn one_unreadable_field_keeps_the_rest() {
        let dir = std::env::temp_dir().join(format!("wave-test-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILE);
        std::fs::write(
            &path,
            r#"{"volume":0.4,"media_folders":["/music"],"theme_mode":"sepia"}"#,
        )
        .unwrap();

        let loaded = AppSettings::load_from_path(&path);
        assert_eq!(loaded.volume, 0.4);
        assert_eq!(loaded.media_folders, ["/music"]);
        assert_eq!(loaded.theme_mode, ThemeMode::default());
        let mut backup = path.clone().into_os_string();
        backup.push(".bak");
        assert!(PathBuf::from(backup).is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_cache_limit_migrates_to_new_default() {
        let load = |json: &str| {
            let mut settings: AppSettings = serde_json::from_str(json).unwrap();
            settings.normalize();
            settings
        };
        assert_eq!(
            load(r#"{"source_cache_limit_mb":512}"#).source_cache_limit_mb,
            if cfg!(target_os = "android") {
                1024
            } else {
                2048
            }
        );
        assert_eq!(
            load(r#"{"source_cache_limit_mb":4096}"#).source_cache_limit_mb,
            4096
        );
    }
}
