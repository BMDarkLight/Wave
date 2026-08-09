use crate::audio::player::RepeatMode;
use crate::metadata::Track;
use serde::{Deserialize, Serialize};

/// What happens when the user clicks the window close button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CloseAction {
    /// Exit the application.
    #[default]
    Quit,
    /// Hide the main window; playback and the tray icon keep running.
    HideWindow,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaybackStateDto {
    pub is_playing: bool,
    pub is_paused: bool,
    pub current_path: Option<String>,
    pub position_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub volume: f32,
    pub output_device_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueStateDto {
    pub tracks: Vec<String>,
    pub current_index: Option<usize>,
    pub is_shuffled: bool,
}

/// Queue with full track metadata (for UI display).
#[derive(Debug, Clone, Serialize)]
pub struct QueueDto {
    pub tracks: Vec<Track>,
    pub current_index: Option<usize>,
    pub is_shuffled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaybackModeDto {
    pub repeat: RepeatMode,
    pub shuffle: bool,
}

/// Result of importing a playlist — the new playlist id and its tracks.
#[derive(Debug, Clone, Serialize)]
pub struct ImportResultDto {
    pub playlist_id: String,
    pub playlist_name: String,
    pub track_count: usize,
}

/// Result of importing a Wave lyrics backup into matching library tracks.
#[derive(Debug, Clone, Serialize)]
pub struct LyricsImportResultDto {
    pub imported: usize,
    pub skipped: usize,
    pub missing: usize,
}

/// Summary of a distinct album in the library, used for browse/grid views.
///
/// Albums are grouped by `(album, album_artist)` — falling back to the track
/// `artist` when `album_artist` is missing — so that unrelated albums which
/// happen to share a name (e.g. several "Greatest Hits") are kept separate.
#[derive(Debug, Clone, Serialize)]
pub struct AlbumSummaryDto {
    pub name: String,
    /// Resolved album artist: the tag `album_artist` when present, else `artist`.
    pub album_artist: Option<String>,
    /// A representative track artist for the album (first found).
    pub artist: String,
    pub track_count: i64,
    pub year: Option<i32>,
    /// Representative cover art for the album (may be a `data:` URL or HTTPS URL).
    pub cover_art_data_url: Option<String>,
    pub cover_art_mime: Option<String>,
    /// Path of a track that carries this album's cover (for full-res extraction).
    pub cover_track_path: Option<String>,
}

/// Summary of a distinct artist in the library, used for browse/discography views.
#[derive(Debug, Clone, Serialize)]
pub struct ArtistSummaryDto {
    pub name: String,
    pub track_count: i64,
    pub album_count: i64,
}

/// One realtime search hit with which fields matched and an optional lyrics snippet.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHitDto {
    pub track: Track,
    /// Fields that matched: `title`, `artist`, `album`, `name`, `lyrics`.
    pub matched_fields: Vec<String>,
    pub lyrics_snippet: Option<String>,
}

/// Equalizer settings returned by `get_eq_settings`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct EqSettingsDto {
    /// Gain in dB for each of the 10 ISO bands.
    pub bands: [f32; 10],
    /// Whether the EQ is currently applied.
    pub enabled: bool,
}

/// Curated Home page payload built from listen stats + transitions.
#[derive(Debug, Clone, Serialize)]
pub struct HomeSuggestionsDto {
    pub featured: Option<Track>,
    pub mix: Vec<Track>,
    pub more: Vec<Track>,
    pub albums: Vec<AlbumSummaryDto>,
    pub favorite_track: Option<Track>,
    pub favorite_album: Option<AlbumSummaryDto>,
    pub favorite_artist: Option<ArtistSummaryDto>,
    /// True when listen history influenced the picks.
    pub curated: bool,
}

/// One named listen rollup (artist / album / genre).
#[derive(Debug, Clone, Serialize)]
pub struct ListenRankDto {
    pub name: String,
    pub listen_seconds: f64,
    pub play_count: i64,
}

/// Compact listening overview for Settings.
#[derive(Debug, Clone, Serialize)]
pub struct ListeningStatsDto {
    pub total_listen_seconds: f64,
    pub total_plays: i64,
    pub tracks_played: i64,
    pub top_tracks: Vec<Track>,
    pub top_artists: Vec<ListenRankDto>,
    pub top_albums: Vec<ListenRankDto>,
    pub top_genres: Vec<ListenRankDto>,
}
