use crate::dto::{
    AlbumSummaryDto, ArtistSummaryDto, HomeSuggestionsDto, ListenRankDto, ListeningStatsDto,
    SearchHitDto,
};
use crate::metadata::{extract_track, is_supported_audio_file, Track};
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Manager;
use uuid::Uuid;
use walkdir::WalkDir;

/// Lean list/playlist select: no lyrics / acoustid blobs; cover is album_art thumb path.
pub(crate) const TRACK_SELECT_COLUMNS: &str = "t.id, t.path, t.name, t.title, t.artist, t.album, t.album_artist, t.genre,
                        t.year, t.track_number, t.disc_number, t.format, t.duration_seconds,
                        t.sample_rate, t.channels, t.bit_depth,
                        NULL AS lyrics, NULL AS lyrics_source,
                        aa.thumb_path, COALESCE(aa.mime, t.cover_art_mime), t.cover_art_source,
                        t.fingerprint_sha256, NULL AS acoustid_fingerprint, t.musicbrainz_recording_id,
                        t.file_size, t.modified_at, t.indexed_at, t.is_saf_uri, t.album_art_id";

/// Full track row including lyrics (lyrics panel / detail).
pub(crate) const TRACK_DETAIL_COLUMNS: &str = "t.id, t.path, t.name, t.title, t.artist, t.album, t.album_artist, t.genre,
                        t.year, t.track_number, t.disc_number, t.format, t.duration_seconds,
                        t.sample_rate, t.channels, t.bit_depth, t.lyrics, t.lyrics_source,
                        aa.thumb_path, COALESCE(aa.mime, t.cover_art_mime), t.cover_art_source,
                        t.fingerprint_sha256, t.acoustid_fingerprint, t.musicbrainz_recording_id,
                        t.file_size, t.modified_at, t.indexed_at, t.is_saf_uri, t.album_art_id";

pub(crate) const TRACK_FROM: &str = "tracks t LEFT JOIN album_art aa ON aa.id = t.album_art_id";

/// Default virtual playlist that mirrors the full track table.
pub const LIBRARY_PLAYLIST_NAME: &str = "Library";
const LEGACY_LIBRARY_PLAYLIST_NAME: &str = "All Local Files";

fn is_library_playlist_name(name: &str) -> bool {
    name == LIBRARY_PLAYLIST_NAME || name == LEGACY_LIBRARY_PLAYLIST_NAME
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistInfo {
    pub id: String,
    pub profile_id: String,
    pub name: String,
    pub track_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
    /// Optional folder path/URI this playlist stays synced with.
    /// Desktop: filesystem path. Android: SAF `content://…/tree/…` URI.
    pub sync_folder: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlaylistExportJson {
    format: String,
    version: u32,
    name: String,
    exported_at: i64,
    tracks: Vec<TrackExportJson>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TrackExportJson {
    path: String,
    title: String,
    artist: String,
    album: String,
    duration_seconds: Option<f64>,
}

/// Cached ID for the default Library playlist, so we don't need to hit the
/// database on every single read operation.
pub struct Library {
    db_path: PathBuf,
    connection: RwLock<Connection>,
    cover_root: PathBuf,
    default_playlist_id_cache: OnceLock<String>,
    favorites_playlist_id_cache: OnceLock<String>,
    app_handle: Option<tauri::AppHandle>,
}

impl Library {
    pub fn new(app_handle: &tauri::AppHandle) -> Result<Self, String> {
        let app_dir = app_handle
            .path()
            .app_data_dir()
            .map_err(|error| format!("Failed to resolve application data directory: {error}"))?;
        std::fs::create_dir_all(&app_dir)
            .map_err(|error| format!("Failed to create application data directory: {error}"))?;

        let db_path = app_dir.join("wave-library.sqlite");
        let cover_root = app_dir.join("cover_art");
        std::fs::create_dir_all(cover_root.join("thumbs"))
            .map_err(|error| format!("Failed to create cover art directory: {error}"))?;
        let connection = Connection::open(&db_path)
            .map_err(|error| format!("Failed to open library database: {error}"))?;
        let library = Self {
            db_path,
            connection: RwLock::new(connection),
            cover_root,
            default_playlist_id_cache: OnceLock::new(),
            favorites_playlist_id_cache: OnceLock::new(),
            app_handle: Some(app_handle.clone()),
        };
        library.initialize()?;
        Ok(library)
    }

    /// Create a library from a direct database path (for CLI, no Tauri dependency).
    pub fn new_with_path(db_path: &std::path::Path) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create database directory: {e}"))?;
        }
        let cover_root = db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("cover_art");
        let _ = std::fs::create_dir_all(cover_root.join("thumbs"));
        let connection = Connection::open(db_path)
            .map_err(|e| format!("Failed to open library database: {e}"))?;
        let library = Self {
            db_path: db_path.to_path_buf(),
            connection: RwLock::new(connection),
            cover_root,
            default_playlist_id_cache: OnceLock::new(),
            favorites_playlist_id_cache: OnceLock::new(),
            app_handle: None,
        };
        library.initialize()?;
        Ok(library)
    }

    pub fn db_path(&self) -> String {
        self.db_path.to_string_lossy().to_string()
    }

    pub fn cover_root(&self) -> &Path {
        &self.cover_root
    }

    pub(crate) fn read_connection(&self) -> RwLockReadGuard<'_, Connection> {
        self.connection.read()
    }

    pub(crate) fn write_connection(&self) -> RwLockWriteGuard<'_, Connection> {
        self.connection.write()
    }

    /// Write lock (mutates). Prefer [`read_connection`] for SELECT-only paths.
    pub(crate) fn lock_connection(&self) -> Result<RwLockWriteGuard<'_, Connection>, String> {
        Ok(self.write_connection())
    }

    fn initialize(&self) -> Result<(), String> {
        let connection = self.lock_connection()?;
        connection
            .execute_batch(
                "
                PRAGMA journal_mode = WAL;
                PRAGMA foreign_keys = ON;
                PRAGMA synchronous = NORMAL;

                CREATE TABLE IF NOT EXISTS profiles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS tracks (
                    id TEXT PRIMARY KEY,
                    path TEXT NOT NULL UNIQUE,
                    name TEXT NOT NULL,
                    title TEXT NOT NULL,
                    artist TEXT NOT NULL,
                    album TEXT NOT NULL,
                    album_artist TEXT,
                    genre TEXT,
                    year INTEGER,
                    track_number INTEGER,
                    disc_number INTEGER,
                    format TEXT NOT NULL,
                    duration_seconds REAL,
                    sample_rate INTEGER,
                    channels INTEGER,
                    bit_depth INTEGER,
                    lyrics TEXT,
                    lyrics_source TEXT,
                    cover_art_data_url TEXT,
                    cover_art_mime TEXT,
                    cover_art_source TEXT,
                    fingerprint_sha256 TEXT,
                    acoustid_fingerprint TEXT,
                    musicbrainz_recording_id TEXT,
                    file_size INTEGER NOT NULL,
                    modified_at INTEGER NOT NULL,
                    indexed_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS playlists (
                    id TEXT PRIMARY KEY,
                    profile_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    UNIQUE(profile_id, name),
                    FOREIGN KEY(profile_id) REFERENCES profiles(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS playlist_tracks (
                    playlist_id TEXT NOT NULL,
                    track_id TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    added_at INTEGER NOT NULL,
                    PRIMARY KEY(playlist_id, track_id),
                    UNIQUE(playlist_id, position),
                    FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE,
                    FOREIGN KEY(track_id) REFERENCES tracks(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_tracks_artist_album ON tracks(artist, album);
                CREATE INDEX IF NOT EXISTS idx_tracks_title ON tracks(title);
                CREATE INDEX IF NOT EXISTS idx_playlist_tracks_position
                    ON playlist_tracks(playlist_id, position);

                CREATE TABLE IF NOT EXISTS album_art (
                    id TEXT PRIMARY KEY,
                    thumb_path TEXT NOT NULL,
                    mime TEXT NOT NULL,
                    byte_size INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS listen_stats (
                    track_id TEXT PRIMARY KEY,
                    play_count INTEGER NOT NULL DEFAULT 0,
                    skip_count INTEGER NOT NULL DEFAULT 0,
                    listen_seconds REAL NOT NULL DEFAULT 0,
                    last_played_at INTEGER NOT NULL DEFAULT 0,
                    FOREIGN KEY(track_id) REFERENCES tracks(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS track_transitions (
                    from_track_id TEXT NOT NULL,
                    to_track_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    count INTEGER NOT NULL DEFAULT 0,
                    last_at INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY(from_track_id, to_track_id, kind),
                    FOREIGN KEY(from_track_id) REFERENCES tracks(id) ON DELETE CASCADE,
                    FOREIGN KEY(to_track_id) REFERENCES tracks(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_listen_stats_last_played
                    ON listen_stats(last_played_at DESC);
                CREATE INDEX IF NOT EXISTS idx_listen_stats_play_count
                    ON listen_stats(play_count DESC, listen_seconds DESC);
                CREATE INDEX IF NOT EXISTS idx_track_transitions_from
                    ON track_transitions(from_track_id, kind, count DESC);
                ",
            )
            .map_err(|error| format!("Failed to initialize library database: {error}"))?;

        ensure_track_column(&connection, "lyrics", "TEXT")?;
        ensure_track_column(&connection, "lyrics_source", "TEXT")?;
        ensure_track_column(&connection, "cover_art_data_url", "TEXT")?;
        ensure_track_column(&connection, "cover_art_mime", "TEXT")?;
        ensure_track_column(&connection, "cover_art_source", "TEXT")?;
        ensure_track_column(&connection, "fingerprint_sha256", "TEXT")?;
        ensure_track_column(&connection, "acoustid_fingerprint", "TEXT")?;
        ensure_track_column(&connection, "musicbrainz_recording_id", "TEXT")?;
        ensure_track_column(&connection, "is_saf_uri", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_track_column(&connection, "album_art_id", "TEXT")?;
        ensure_playlist_column(&connection, "sync_folder", "TEXT")?;

        if let Err(error) = ensure_tracks_fts(&connection) {
            tracing::warn!("Failed to initialize FTS search index: {error}");
        }

        // Seed the default profile and playlist. We do this once at startup.
        let profile_id = ensure_profile_with_connection(&connection, "default", "Default")?;
        // Migrate legacy default playlist name before ensuring the current one.
        let _ = connection.execute(
            "UPDATE playlists SET name = ?1 WHERE name = ?2",
            params![LIBRARY_PLAYLIST_NAME, LEGACY_LIBRARY_PLAYLIST_NAME],
        );
        let playlist_id =
            ensure_playlist_with_connection(&connection, &profile_id, LIBRARY_PLAYLIST_NAME)?;
        let favorites_id =
            ensure_playlist_with_connection(&connection, &profile_id, "Favorites")?;

        // Warm the caches.
        let _ = self.default_playlist_id_cache.set(playlist_id.clone());
        let _ = self.favorites_playlist_id_cache.set(favorites_id);

        if let Err(error) = repair_all_playlist_positions(&connection) {
            tracing::warn!("Failed to repair playlist positions on startup: {error}");
        }

        // Remove duplicate tracks (same artist + album + title), keeping the
        // earliest indexed copy.
        if let Err(error) = deduplicate_tracks(&connection) {
            tracing::warn!("Failed to deduplicate tracks on startup: {error}");
        }

        drop(connection);
        if let Err(error) = self.migrate_cover_art_blobs() {
            tracing::warn!("Cover art migration: {error}");
        }

        Ok(())
    }

    /// Returns the cached default playlist id, seeding if necessary.
    pub fn default_playlist_id(&self) -> Result<String, String> {
        if let Some(id) = self.default_playlist_id_cache.get() {
            return Ok(id.clone());
        }
        let connection = self.lock_connection()?;
        let profile_id = ensure_profile_with_connection(&connection, "default", "Default")?;
        let playlist_id =
            ensure_playlist_with_connection(&connection, &profile_id, LIBRARY_PLAYLIST_NAME)?;
        let _ = self.default_playlist_id_cache.set(playlist_id.clone());
        Ok(playlist_id)
    }

    /// Migrate legacy per-track `cover_art_data_url` blobs into shared `album_art` thumbs.
    fn migrate_cover_art_blobs(&self) -> Result<(), String> {
        let connection = self.write_connection();
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM tracks
                 WHERE cover_art_data_url IS NOT NULL
                   AND length(cover_art_data_url) > 0
                   AND album_art_id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if count == 0 {
            // Still share album_art_id among siblings that already migrated.
            drop(connection);
            self.share_album_art_among_siblings()?;
            return Ok(());
        }

        tracing::info!("Migrating {count} track cover blobs to shared album art thumbs");
        let rows: Vec<(String, String, Option<String>)> = {
            let mut stmt = connection
                .prepare(
                    "SELECT id, cover_art_data_url, cover_art_mime FROM tracks
                     WHERE cover_art_data_url IS NOT NULL
                       AND length(cover_art_data_url) > 0
                       AND album_art_id IS NULL",
                )
                .map_err(|e| format!("Failed to prepare cover migration: {e}"))?;
            let mapped = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|e| format!("Failed to query cover migration: {e}"))?;
            mapped
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to read cover migration rows: {e}"))?
        };

        let now = now_timestamp();
        for (track_id, data_url, mime) in rows {
            let saved = match crate::cover_art::migrate_data_url_to_thumb(&self.cover_root, &data_url)
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!("Skip cover migrate for {track_id}: {e}");
                    let _ = connection.execute(
                        "UPDATE tracks SET cover_art_data_url = NULL WHERE id = ?1",
                        params![track_id],
                    );
                    continue;
                }
            };
            let mime = mime.unwrap_or_else(|| saved.mime.clone());
            connection
                .execute(
                    "INSERT OR IGNORE INTO album_art (id, thumb_path, mime, byte_size, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        saved.id,
                        saved.relative_path,
                        mime,
                        saved.byte_size,
                        now
                    ],
                )
                .map_err(|e| format!("Failed to insert album_art: {e}"))?;
            connection
                .execute(
                    "UPDATE tracks SET album_art_id = ?1, cover_art_mime = ?2,
                            cover_art_data_url = NULL WHERE id = ?3",
                    params![saved.id, mime, track_id],
                )
                .map_err(|e| format!("Failed to link album_art: {e}"))?;
        }

        drop(connection);
        self.share_album_art_among_siblings()?;

        // Best-effort VACUUM to reclaim blob space.
        {
            let connection = self.write_connection();
            if let Err(e) = connection.execute_batch("VACUUM;") {
                tracing::warn!("VACUUM after cover migration skipped: {e}");
            }
        }

        if let Some(app) = &self.app_handle {
            crate::cover_art::cleanup_legacy_track_covers(app);
        }
        Ok(())
    }

    /// Copy album_art_id to sibling tracks that share (album_artist, album).
    fn share_album_art_among_siblings(&self) -> Result<(), String> {
        let connection = self.write_connection();
        connection
            .execute_batch(
                "
                UPDATE tracks
                SET album_art_id = (
                    SELECT t2.album_art_id FROM tracks t2
                    WHERE t2.album_art_id IS NOT NULL
                      AND t2.album = tracks.album
                      AND COALESCE(NULLIF(t2.album_artist, ''), t2.artist)
                          = COALESCE(NULLIF(tracks.album_artist, ''), tracks.artist)
                    LIMIT 1
                )
                WHERE album_art_id IS NULL
                  AND album IS NOT NULL
                  AND length(album) > 0
                  AND EXISTS (
                    SELECT 1 FROM tracks t2
                    WHERE t2.album_art_id IS NOT NULL
                      AND t2.album = tracks.album
                      AND COALESCE(NULLIF(t2.album_artist, ''), t2.artist)
                          = COALESCE(NULLIF(tracks.album_artist, ''), tracks.artist)
                  );
                ",
            )
            .map_err(|e| format!("Failed to share album art among siblings: {e}"))?;
        Ok(())
    }

    /// Returns the cached favorites playlist id, seeding if necessary.
    pub fn favorites_playlist_id(&self) -> Result<String, String> {
        if let Some(id) = self.favorites_playlist_id_cache.get() {
            return Ok(id.clone());
        }
        let connection = self.lock_connection()?;
        let profile_id = ensure_profile_with_connection(&connection, "default", "Default")?;
        let playlist_id =
            ensure_playlist_with_connection(&connection, &profile_id, "Favorites")?;
        let _ = self.favorites_playlist_id_cache.set(playlist_id.clone());
        Ok(playlist_id)
    }

    pub fn add_track_to_default_playlist(&self, path: String) -> Result<Track, String> {
        let playlist_id = self.default_playlist_id()?;
        self.add_track_to_playlist(&playlist_id, path)
    }

    pub fn add_track_to_playlist(&self, playlist_id: &str, path: String) -> Result<Track, String> {
        let is_default = self
            .default_playlist_id_cache
            .get()
            .map_or(false, |id| id == playlist_id);

        let existing = {
            let connection = self.lock_connection()?;
            connection
                .query_row(
                    &format!(
                        "SELECT {TRACK_SELECT_COLUMNS}
                         FROM {TRACK_FROM}
                         WHERE t.path = ?1"
                    ),
                    params![path],
                    |row| row_to_track(row, &self.cover_root),
                )
                .optional()
                .map_err(|error| format!("Failed to look up track: {error}"))?
        };

        let mut track = match existing {
            Some(track) => track,
            None => extract_track(self.app_handle.as_ref(), &path)?,
        };

        // "Library" is virtual — just ensure the track exists in the
        // tracks table. No playlist_tracks entry is needed.
        if is_default {
            let mut connection = self.lock_connection()?;
            let tx = connection
                .transaction()
                .map_err(|e| format!("Failed to begin transaction: {e}"))?;
            let track_id = upsert_track(&tx, &track)?;
            track.id = track_id;
            tx.commit()
                .map_err(|e| format!("Failed to commit transaction: {e}"))?;
            return Ok(track);
        }

        let now = now_timestamp();
        let mut connection = self.lock_connection()?;
        let tx = connection
            .transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;

        let track_id = upsert_track(&tx, &track)?;
        track.id = track_id.clone();

        let already_in_playlist = tx
            .query_row(
                "SELECT 1 FROM playlist_tracks
                 WHERE playlist_id = ?1 AND track_id = ?2",
                params![playlist_id, track_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("Failed to check playlist membership: {error}"))?
            .is_some();
        if already_in_playlist {
            return Err("Track is already in the playlist".to_string());
        }

        let position = next_playlist_position(&tx, playlist_id)?;
        tx.execute(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![playlist_id, track_id, position, now],
        )
        .map_err(|error| format!("Failed to add track to playlist: {error}"))?;

        tx.commit()
            .map_err(|e| format!("Failed to commit transaction: {e}"))?;

        Ok(track)
    }

    pub fn remove_track_from_playlist_by_path(
        &self,
        playlist_id: &str,
        path: &str,
    ) -> Result<(), String> {
        let is_default = self
            .default_playlist_id_cache
            .get()
            .map_or(false, |id| id == playlist_id);

        let mut connection = self.lock_connection()?;
        let tx = connection
            .transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;

        if is_default {
            // Library is virtual — remove the track entirely
            return Self::delete_track_by_path_in_tx(&tx, path).and_then(|_| {
                tx.commit()
                    .map_err(|e| format!("Failed to commit transaction: {e}"))
            });
        }

        let deleted = tx
            .execute(
                "DELETE FROM playlist_tracks
                 WHERE playlist_id = ?1
                   AND track_id = (SELECT id FROM tracks WHERE path = ?2)",
                params![playlist_id, path],
            )
            .map_err(|error| format!("Failed to remove playlist track: {error}"))?;
        if deleted == 0 {
            return Err("Track is not in the playlist".to_string());
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit transaction: {e}"))?;

        Ok(())
    }

    /// Remove a track from the library (and every playlist). Desktop and Android.
    pub fn remove_track_from_library(&self, path: &str) -> Result<(), String> {
        let mut connection = self.lock_connection()?;
        let tx = connection
            .transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;
        Self::delete_track_by_path_in_tx(&tx, path)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit transaction: {e}"))?;
        Ok(())
    }

    fn delete_track_by_path_in_tx(tx: &Transaction<'_>, path: &str) -> Result<(), String> {
        let track_id: Option<String> = tx
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .ok();

        let track_id = match track_id {
            Some(id) => id,
            None => return Err("Track not found".to_string()),
        };

        tx.execute(
            "DELETE FROM playlist_tracks WHERE track_id = ?1",
            params![track_id],
        )
        .map_err(|e| format!("Failed to remove from playlist_tracks: {e}"))?;

        let _ = tx.execute(
            "DELETE FROM tracks_fts WHERE track_id = ?1",
            params![track_id],
        );

        tx.execute("DELETE FROM tracks WHERE id = ?1", params![track_id])
            .map_err(|e| format!("Failed to remove track: {e}"))?;
        Ok(())
    }

    pub fn get_default_playlist_tracks(&self) -> Result<Vec<Track>, String> {
        let playlist_id = self.default_playlist_id()?;
        self.get_playlist_tracks(&playlist_id)
    }

    pub fn get_playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>, String> {
        let connection = self.lock_connection()?;

        // Library returns every track in the library
        if self.default_playlist_id_cache.get().map_or(false, |id| id == playlist_id) {
            let mut statement = connection
                .prepare(
                    &format!(
                        "SELECT {TRACK_SELECT_COLUMNS}
                     FROM {TRACK_FROM}
                     ORDER BY t.name"
                    ),
                )
                .map_err(|error| format!("Failed to prepare all-local-files query: {error}"))?;

            let tracks = statement
                .query_map([], |row| row_to_track(row, &self.cover_root))
                .map_err(|error| format!("Failed to query all-local-files: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("Failed to read all-local-files track: {error}"))?;
            return Ok(tracks);
        }

        let mut statement = connection
            .prepare(
                &format!(
                    "SELECT {TRACK_SELECT_COLUMNS}
                 FROM playlist_tracks pt
                 JOIN tracks t ON t.id = pt.track_id
                 LEFT JOIN album_art aa ON aa.id = t.album_art_id
                 WHERE pt.playlist_id = ?1
                 ORDER BY pt.position"
                ),
            )
            .map_err(|error| format!("Failed to prepare playlist query: {error}"))?;

        let tracks = statement
            .query_map(params![playlist_id], |row| row_to_track(row, &self.cover_root))
            .map_err(|error| format!("Failed to query playlist: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read playlist track: {error}"))?;
        Ok(tracks)
    }

    pub fn clear_default_playlist(&self) -> Result<(), String> {
        let playlist_id = self.default_playlist_id()?;
        let connection = self.lock_connection()?;
        connection
            .execute(
                "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
                params![playlist_id],
            )
            .map_err(|error| format!("Failed to clear playlist: {error}"))?;
        Ok(())
    }

    // ── Favorites ────────────────────────────────────────────────────────────

    /// Add a track to the Favorites playlist. Extracts metadata and upserts the
    /// track into the library first (so favorites work for any file, not just
    /// already-indexed ones).
    pub fn add_track_to_favorites(&self, path: String) -> Result<Track, String> {
        let playlist_id = self.favorites_playlist_id()?;
        self.add_track_to_playlist(&playlist_id, path)
    }

    /// Remove a track from the Favorites playlist by file path.
    pub fn remove_track_from_favorites(&self, path: &str) -> Result<(), String> {
        let playlist_id = self.favorites_playlist_id()?;
        self.remove_track_from_playlist_by_path(&playlist_id, path)
    }

    /// List every track in the Favorites playlist, ordered by position.
    pub fn get_favorites(&self) -> Result<Vec<Track>, String> {
        let playlist_id = self.favorites_playlist_id()?;
        self.get_playlist_tracks(&playlist_id)
    }

    /// Whether a track (by file path) is in the Favorites playlist.
    pub fn is_track_in_favorites(&self, path: &str) -> Result<bool, String> {
        let playlist_id = self.favorites_playlist_id()?;
        let connection = self.lock_connection()?;
        let in_favorites = connection
            .query_row(
                "SELECT 1 FROM playlist_tracks
                 WHERE playlist_id = ?1
                   AND track_id = (SELECT id FROM tracks WHERE path = ?2)",
                params![playlist_id, path],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("Failed to check favorites: {error}"))?
            .is_some();
        Ok(in_favorites)
    }

    /// Whether a track is registered in the library and belongs to at least one playlist.
    pub fn is_track_in_any_playlist(&self, path: &str) -> Result<bool, String> {
        let connection = self.lock_connection()?;
        let in_playlist = connection
            .query_row(
                "SELECT 1
                 FROM tracks t
                 INNER JOIN playlist_tracks pt ON pt.track_id = t.id
                 WHERE t.path = ?1
                 LIMIT 1",
                params![path],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("Failed to check playlist membership: {error}"))?
            .is_some();
        Ok(in_playlist)
    }

    /// Toggle the favorite state of a track. Returns the new state
    /// (`true` = now favorited, `false` = now unfavorited).
    pub fn toggle_favorite(&self, path: &str) -> Result<bool, String> {
        if self.is_track_in_favorites(path)? {
            self.remove_track_from_favorites(path)?;
            Ok(false)
        } else {
            self.add_track_to_favorites(path.to_string())?;
            Ok(true)
        }
    }

    /// Remove every track from the Favorites playlist.
    pub fn clear_favorites(&self) -> Result<(), String> {
        let playlist_id = self.favorites_playlist_id()?;
        let connection = self.lock_connection()?;
        connection
            .execute(
                "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
                params![playlist_id],
            )
            .map_err(|error| format!("Failed to clear favorites: {error}"))?;
        Ok(())
    }

    pub fn index_directory(
        &self,
        profile_id: Option<String>,
        playlist_name: Option<String>,
        directory: String,
    ) -> Result<Vec<Track>, String> {
        let profile_id_str = profile_id.unwrap_or_else(|| "default".to_string());
        let playlist_name_str =
            playlist_name.unwrap_or_else(|| LIBRARY_PLAYLIST_NAME.to_string());

        // Resolve / create the profile and playlist outside the connection lock.
        let playlist_id = {
            let connection = self.lock_connection()?;
            ensure_profile_with_connection(&connection, &profile_id_str, &profile_id_str)?;
            ensure_playlist_with_connection(&connection, &profile_id_str, &playlist_name_str)?
        };

        let directory_path = Path::new(&directory);
        if !directory_path.is_dir() {
            return Err("Library path is not a directory".to_string());
        }

        // Collect all audio file paths first so the WalkDir iterator isn't
        // held across the DB lock acquisition.
        let audio_paths: Vec<String> = WalkDir::new(directory_path)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter(|e| is_supported_audio_file(e.path()))
            .filter_map(|e| e.path().to_str().map(str::to_string))
            .collect();

        let mut tracks = Vec::with_capacity(audio_paths.len());
        let mut failed: Vec<String> = Vec::new();

        // Import everything in a single transaction for much better performance.
        let mut connection = self.lock_connection()?;
        let tx = connection
            .transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;

        let now = now_timestamp();
        for path in &audio_paths {
            match extract_track(self.app_handle.as_ref(), path) {
                Ok(mut track) => {
                    let track_id = match upsert_track(&tx, &track) {
                        Ok(id) => id,
                        Err(e) => {
                            failed.push(format!("{path}: {e}"));
                            continue;
                        }
                    };
                    track.id = track_id.clone();
                    let position = match next_playlist_position(&tx, &playlist_id) {
                        Ok(p) => p,
                        Err(e) => {
                            failed.push(format!("{path}: {e}"));
                            continue;
                        }
                    };
                    match tx.execute(
                        "INSERT OR IGNORE INTO playlist_tracks
                         (playlist_id, track_id, position, added_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![playlist_id, track_id, position, now],
                    ) {
                        Ok(0) => {}
                        Ok(_) => tracks.push(track),
                        Err(e) => failed.push(format!("{path}: {e}")),
                    }
                }
                Err(e) => {
                    failed.push(format!("{path}: {e}"));
                }
            }
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit import transaction: {e}"))?;

        if !failed.is_empty() {
            tracing::warn!(
                "Skipped {} file(s) during library scan:\n{}",
                failed.len(),
                failed.join("\n")
            );
        }

        Ok(tracks)
    }

    pub fn list_playlists(&self, profile_id: Option<String>) -> Result<Vec<PlaylistInfo>, String> {
        let connection = self.lock_connection()?;

        // For the default Library playlist, count from tracks table
        // since it's virtual and doesn't rely on playlist_tracks.
        let default_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
            .unwrap_or(0);

        let default_id = self.default_playlist_id_cache.get().cloned();

        let mut sql = "
            SELECT p.id, p.profile_id, p.name, COUNT(pt.track_id), p.created_at, p.updated_at,
                   p.sync_folder
            FROM playlists p
            LEFT JOIN playlist_tracks pt ON pt.playlist_id = p.id
        "
        .to_string();

        if profile_id.is_some() {
            sql.push_str(" WHERE p.profile_id = ?1");
        }

        sql.push_str(" GROUP BY p.id ORDER BY p.updated_at DESC, p.name");

        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| format!("Failed to prepare playlists query: {error}"))?;

        let rows: Vec<PlaylistInfo> = if let Some(profile_id) = profile_id {
            statement.query_map(params![profile_id], row_to_playlist)
        } else {
            statement.query_map([], row_to_playlist)
        }
        .map_err(|error| format!("Failed to query playlists: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to read playlists: {error}"))?;

        // Patch the default playlist count
        let rows = rows
            .into_iter()
            .map(|mut p| {
                if Some(&p.id) == default_id.as_ref() {
                    p.track_count = default_count;
                }
                p
            })
            .collect();

        Ok(rows)
    }

    // ── Playlist CRUD ────────────────────────────────────────────────────────

    pub fn create_playlist(
        &self,
        name: &str,
        sync_folder: Option<&str>,
    ) -> Result<PlaylistInfo, String> {
        self.insert_playlist(name, false, sync_folder)
    }

    /// Create a playlist for import flows, auto-suffixing duplicate names.
    pub fn create_playlist_for_import(&self, name: &str) -> Result<PlaylistInfo, String> {
        self.insert_playlist(name, true, None)
    }

    fn insert_playlist(
        &self,
        name: &str,
        allow_duplicate_suffix: bool,
        sync_folder: Option<&str>,
    ) -> Result<PlaylistInfo, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Playlist name cannot be empty".to_string());
        }

        let sync_folder = sync_folder
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let connection = self.lock_connection()?;
        let profile_id = ensure_profile_with_connection(&connection, "default", "Default")?;
        let final_name = if allow_duplicate_suffix {
            self.resolve_unique_playlist_name(&connection, &profile_id, name)?
        } else if self.playlist_name_exists(&connection, &profile_id, name)? {
            return Err(format!("A playlist named \"{name}\" already exists"));
        } else {
            name.to_string()
        };

        let id = Uuid::new_v4().to_string();
        let now = now_timestamp();
        connection
            .execute(
                "INSERT INTO playlists (id, profile_id, name, created_at, updated_at, sync_folder)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                params![id, profile_id, final_name, now, sync_folder],
            )
            .map_err(|error| format!("Failed to create playlist: {error}"))?;

        self.playlist_info(&connection, &id)?
            .ok_or_else(|| "Playlist vanished immediately after creation".to_string())
    }

    /// Bind (or clear) the folder a playlist stays synced with.
    pub fn set_playlist_sync_folder(
        &self,
        playlist_id: &str,
        sync_folder: Option<&str>,
    ) -> Result<PlaylistInfo, String> {
        let sync_folder = sync_folder
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let connection = self.lock_connection()?;
        let now = now_timestamp();
        let updated = connection
            .execute(
                "UPDATE playlists SET sync_folder = ?1, updated_at = ?2 WHERE id = ?3",
                params![sync_folder, now, playlist_id],
            )
            .map_err(|error| format!("Failed to update playlist sync folder: {error}"))?;
        if updated == 0 {
            return Err("Playlist not found".to_string());
        }
        self.playlist_info(&connection, playlist_id)?
            .ok_or_else(|| "Playlist not found".to_string())
    }

    fn playlist_name_exists(
        &self,
        connection: &Connection,
        profile_id: &str,
        name: &str,
    ) -> Result<bool, String> {
        connection
            .query_row(
                "SELECT 1 FROM playlists WHERE profile_id = ?1 AND name = ?2",
                params![profile_id, name],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("Failed to check playlist name: {error}"))
            .map(|row| row.is_some())
    }

    fn resolve_unique_playlist_name(
        &self,
        connection: &Connection,
        profile_id: &str,
        base: &str,
    ) -> Result<String, String> {
        if !self.playlist_name_exists(connection, profile_id, base)? {
            return Ok(base.to_string());
        }

        for index in 2..1000 {
            let candidate = format!("{base} ({index})");
            if !self.playlist_name_exists(connection, profile_id, &candidate)? {
                return Ok(candidate);
            }
        }

        Err(format!("Could not find a unique name for playlist \"{base}\""))
    }

    pub fn delete_playlist(&self, playlist_id: &str) -> Result<(), String> {
        let connection = self.lock_connection()?;
        let name: Option<String> = connection
            .query_row(
                "SELECT name FROM playlists WHERE id = ?1",
                params![playlist_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("Failed to look up playlist: {error}"))?;

        match name {
            None => return Err("Playlist not found".to_string()),
            Some(name) if is_library_playlist_name(&name) => {
                return Err("The default Library playlist cannot be deleted".to_string());
            }
            Some(name) if name == "Favorites" => {
                return Err("The \"Favorites\" playlist cannot be deleted".to_string());
            }
            Some(_) => {}
        }

        let deleted = connection
            .execute(
                "DELETE FROM playlists WHERE id = ?1",
                params![playlist_id],
            )
            .map_err(|error| format!("Failed to delete playlist: {error}"))?;
        if deleted == 0 {
            return Err("Playlist not found".to_string());
        }
        Ok(())
    }

    pub fn rename_playlist(&self, playlist_id: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Playlist name cannot be empty".to_string());
        }

        let connection = self.lock_connection()?;
        let (current_name, profile_id): (String, String) = connection
            .query_row(
                "SELECT name, profile_id FROM playlists WHERE id = ?1",
                params![playlist_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| format!("Failed to look up playlist: {error}"))?
            .ok_or_else(|| "Playlist not found".to_string())?;

        if is_library_playlist_name(&current_name) {
            return Err("The default Library playlist cannot be renamed".to_string());
        }

        if current_name == "Favorites" {
            return Err("The \"Favorites\" playlist cannot be renamed".to_string());
        }

        if name != current_name && self.playlist_name_exists(&connection, &profile_id, name)? {
            return Err(format!("A playlist named \"{name}\" already exists"));
        }

        let now = now_timestamp();
        connection
            .execute(
                "UPDATE playlists SET name = ?1, updated_at = ?2 WHERE id = ?3",
                params![name, now, playlist_id],
            )
            .map_err(|error| format!("Failed to rename playlist: {error}"))?;
        Ok(())
    }

    pub fn clear_playlist(&self, playlist_id: &str) -> Result<(), String> {
        let is_default = self
            .default_playlist_id_cache
            .get()
            .map_or(false, |id| id == playlist_id);
        let favorites_id = self.favorites_playlist_id()?;
        if favorites_id == playlist_id {
            return Err("Favorites cannot be cleared with clear playlist".to_string());
        }

        let mut connection = self.lock_connection()?;

        let sync_folder: Option<String> = connection
            .query_row(
                "SELECT sync_folder FROM playlists WHERE id = ?1",
                params![playlist_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|e| format!("Failed to look up playlist: {e}"))?;
        if sync_folder.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            return Err(
                "Synced playlists cannot be cleared. Unlink the sync folder first.".to_string(),
            );
        }

        let tx = connection
            .transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;

        if is_default {
            tx.execute("DELETE FROM playlist_tracks", [])
                .map_err(|e| format!("Failed to clear playlist_tracks: {e}"))?;
            let _ = tx.execute("DELETE FROM tracks_fts", []);
            tx.execute("DELETE FROM tracks", [])
                .map_err(|e| format!("Failed to clear tracks: {e}"))?;
        } else {
            tx.execute(
                "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
                params![playlist_id],
            )
            .map_err(|error| format!("Failed to clear playlist: {error}"))?;
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit transaction: {e}"))?;
        Ok(())
    }

    /// Wipe every track and delete all user playlists. Keeps Library and Favorites
    /// (both empty). Also clears Library's sync folder link.
    pub fn reset_library(&self) -> Result<(u32, u32), String> {
        let library_id = self.default_playlist_id()?;
        let favorites_id = self.favorites_playlist_id()?;
        let playlists = self.list_playlists(None)?;

        let mut deleted_playlists = 0u32;
        for pl in &playlists {
            if pl.id == library_id || pl.id == favorites_id {
                continue;
            }
            self.delete_playlist(&pl.id)?;
            deleted_playlists += 1;
        }

        let mut connection = self.lock_connection()?;
        let tx = connection
            .transaction()
            .map_err(|e| format!("Failed to begin reset transaction: {e}"))?;

        let track_count: i64 = tx
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
            .map_err(|e| format!("Failed to count tracks: {e}"))?;

        tx.execute("DELETE FROM playlist_tracks", [])
            .map_err(|e| format!("Failed to clear playlist membership: {e}"))?;
        let _ = tx.execute("DELETE FROM tracks_fts", []);
        let _ = tx.execute("DELETE FROM track_transitions", []);
        let _ = tx.execute("DELETE FROM listen_stats", []);
        tx.execute("DELETE FROM tracks", [])
            .map_err(|e| format!("Failed to clear tracks: {e}"))?;
        // Drop cached album art rows; files on disk are cleaned by reset_app.
        let _ = tx.execute("DELETE FROM album_art", []);
        tx.execute(
            "UPDATE playlists SET sync_folder = NULL, updated_at = ?1 WHERE id = ?2",
            params![now_timestamp(), library_id],
        )
        .map_err(|e| format!("Failed to clear Library sync folder: {e}"))?;
        tx.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![now_timestamp(), favorites_id],
        )
        .map_err(|e| format!("Failed to bump Favorites updated_at: {e}"))?;

        tx.commit()
            .map_err(|e| format!("Failed to commit reset: {e}"))?;

        Ok((track_count as u32, deleted_playlists))
    }

    /// Make playlist membership match `desired_paths` exactly.
    ///
    /// Optimized for launch sync: only loads paths (not full track rows), uses
    /// one DB transaction, and only probes metadata for brand-new files.
    pub fn sync_playlist_to_paths(
        &self,
        playlist_id: &str,
        desired_paths: &[String],
    ) -> Result<(u32, u32), String> {
        let (to_remove, to_add) = self.diff_playlist_paths(playlist_id, desired_paths)?;
        if to_remove.is_empty() && to_add.is_empty() {
            return Ok((0, 0));
        }

        let existing_ids = self.track_ids_by_paths(&to_add)?;
        let mut extracted = Vec::new();
        for path in &to_add {
            if existing_ids.contains_key(&normalize_path_key(path)) {
                continue;
            }
            match extract_track(self.app_handle.as_ref(), path) {
                Ok(track) => extracted.push(track),
                Err(e) => tracing::warn!("Sync skip (metadata): {path}: {e}"),
            }
        }

        let link_ids: Vec<String> = to_add
            .iter()
            .filter_map(|path| existing_ids.get(&normalize_path_key(path)).cloned())
            .collect();

        self.apply_playlist_sync(playlist_id, &to_remove, &extracted, &link_ids)
    }

    /// Diff current playlist paths against `desired_paths` (normalized).
    pub fn diff_playlist_paths(
        &self,
        playlist_id: &str,
        desired_paths: &[String],
    ) -> Result<(Vec<String>, Vec<String>), String> {
        use std::collections::{HashMap, HashSet};

        let current_raw = self.playlist_track_paths(playlist_id)?;
        let mut current_by_key: HashMap<String, String> = HashMap::new();
        for path in current_raw {
            current_by_key
                .entry(normalize_path_key(&path))
                .or_insert(path);
        }

        let mut desired_set: HashSet<String> = HashSet::new();
        let mut desired_ordered: Vec<String> = Vec::new();
        for path in desired_paths {
            let key = normalize_path_key(path);
            if desired_set.insert(key) {
                desired_ordered.push(path.clone());
            }
        }

        let to_remove: Vec<String> = current_by_key
            .iter()
            .filter(|(key, _)| !desired_set.contains(key.as_str()))
            .map(|(_, raw)| raw.clone())
            .collect();
        let to_add: Vec<String> = desired_ordered
            .into_iter()
            .filter(|path| !current_by_key.contains_key(&normalize_path_key(path)))
            .collect();

        Ok((to_remove, to_add))
    }

    /// Apply a precomputed sync diff in a single write transaction.
    pub fn apply_playlist_sync(
        &self,
        playlist_id: &str,
        to_remove: &[String],
        extracted: &[Track],
        link_track_ids: &[String],
    ) -> Result<(u32, u32), String> {
        if to_remove.is_empty() && extracted.is_empty() && link_track_ids.is_empty() {
            return Ok((0, 0));
        }

        let is_default = self
            .default_playlist_id_cache
            .get()
            .map_or(false, |id| id == playlist_id);

        let now = now_timestamp();
        let mut connection = self.lock_connection()?;
        let tx = connection
            .transaction()
            .map_err(|e| format!("Failed to begin sync transaction: {e}"))?;

        // Upsert/link first so path rewrites land before removals. Otherwise a
        // path-normalization mismatch deletes the real row, then inserts a dupe.
        let mut kept_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut added = 0u32;
        let mut position = next_playlist_position(&tx, playlist_id)?;

        for track in extracted {
            let track_id = upsert_track_deduped(&tx, track)?;
            kept_ids.insert(track_id.clone());
            if is_default {
                added += 1;
                continue;
            }
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO playlist_tracks (playlist_id, track_id, position, added_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![playlist_id, track_id, position, now],
                )
                .map_err(|e| format!("Failed to add track to playlist: {e}"))?;
            if inserted > 0 {
                added += 1;
                position += 1;
            }
        }

        if !is_default {
            for track_id in link_track_ids {
                kept_ids.insert(track_id.clone());
                let inserted = tx
                    .execute(
                        "INSERT OR IGNORE INTO playlist_tracks (playlist_id, track_id, position, added_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![playlist_id, track_id, position, now],
                    )
                    .map_err(|e| format!("Failed to link track to playlist: {e}"))?;
                if inserted > 0 {
                    added += 1;
                    position += 1;
                }
            }
        } else {
            for track_id in link_track_ids {
                kept_ids.insert(track_id.clone());
            }
        }

        let mut removed = 0u32;
        if is_default {
            for path in to_remove {
                let track_id = resolve_track_id_by_path(&tx, path)?;
                let Some(track_id) = track_id else {
                    continue;
                };
                if kept_ids.contains(&track_id) {
                    continue;
                }
                let _ = tx.execute(
                    "DELETE FROM playlist_tracks WHERE track_id = ?1",
                    params![track_id],
                );
                if tx
                    .execute("DELETE FROM tracks WHERE id = ?1", params![track_id])
                    .map_err(|e| format!("Failed to remove track: {e}"))?
                    > 0
                {
                    removed += 1;
                }
            }
        } else {
            for path in to_remove {
                let track_id = resolve_track_id_by_path(&tx, path)?;
                let Some(track_id) = track_id else {
                    continue;
                };
                if kept_ids.contains(&track_id) {
                    continue;
                }
                let deleted = tx
                    .execute(
                        "DELETE FROM playlist_tracks
                         WHERE playlist_id = ?1 AND track_id = ?2",
                        params![playlist_id, track_id],
                    )
                    .map_err(|e| format!("Failed to remove playlist track: {e}"))?;
                if deleted > 0 {
                    removed += 1;
                }
            }
        }

        tx.execute(
            "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
            params![now, playlist_id],
        )
        .map_err(|e| format!("Failed to bump playlist updated_at: {e}"))?;

        tx.commit()
            .map_err(|e| format!("Failed to commit sync transaction: {e}"))?;
        drop(connection);

        // Collapse any artist/album/title duplicates left over from path mismatches.
        {
            let connection = self.lock_connection()?;
            if let Err(e) = deduplicate_tracks(&connection) {
                tracing::warn!("Post-sync dedupe failed: {e}");
            }
        }

        Ok((added, removed))
    }

    /// Paths only — avoids loading cover art / full rows during sync.
    pub fn playlist_track_paths(&self, playlist_id: &str) -> Result<Vec<String>, String> {
        let connection = self.lock_connection()?;
        if self
            .default_playlist_id_cache
            .get()
            .map_or(false, |id| id == playlist_id)
        {
            let mut statement = connection
                .prepare("SELECT path FROM tracks ORDER BY path")
                .map_err(|e| format!("Failed to prepare path query: {e}"))?;
            let paths = statement
                .query_map([], |row| row.get(0))
                .map_err(|e| format!("Failed to query paths: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to read paths: {e}"))?;
            return Ok(paths);
        }

        let mut statement = connection
            .prepare(
                "SELECT t.path FROM playlist_tracks pt
                 JOIN tracks t ON t.id = pt.track_id
                 WHERE pt.playlist_id = ?1
                 ORDER BY pt.position",
            )
            .map_err(|e| format!("Failed to prepare playlist path query: {e}"))?;
        let paths = statement
            .query_map(params![playlist_id], |row| row.get(0))
            .map_err(|e| format!("Failed to query playlist paths: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read playlist paths: {e}"))?;
        Ok(paths)
    }

    pub fn track_ids_by_paths(
        &self,
        paths: &[String],
    ) -> Result<std::collections::HashMap<String, String>, String> {
        use std::collections::{HashMap, HashSet};
        let mut map = HashMap::new();
        if paths.is_empty() {
            return Ok(map);
        }
        let wanted: HashSet<String> = paths.iter().map(|p| normalize_path_key(p)).collect();
        let connection = self.lock_connection()?;
        let mut statement = connection
            .prepare("SELECT id, path FROM tracks")
            .map_err(|e| format!("Failed to prepare track lookup: {e}"))?;
        let rows = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| format!("Failed to query tracks: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read tracks: {e}"))?;
        for (id, stored) in rows {
            let key = normalize_path_key(&stored);
            if wanted.contains(&key) {
                map.insert(key, id);
            }
        }
        Ok(map)
    }

    /// Create a playlist from all tracks matching the given album name.
    /// Uses the album name as the playlist name unless `playlist_name` is provided.
    /// Returns an error if no tracks are found for the given album.
    pub fn create_album_playlist(
        &self,
        album: &str,
        playlist_name: Option<&str>,
    ) -> Result<PlaylistInfo, String> {
        let album = album.trim();
        if album.is_empty() {
            return Err("Album name cannot be empty".to_string());
        }

        let mut connection = self.lock_connection()?;
        let tracks = Self::get_tracks_by_column(&connection, &self.cover_root, "album", album)?;
        if tracks.is_empty() {
            return Err(format!("No tracks found for album \"{album}\""));
        }

        let name = playlist_name.unwrap_or(album);
        let playlist_name_str = name.to_string();

        // Resolve profile and create the playlist outside the transaction.
        let profile_id =
            ensure_profile_with_connection(&connection, "default", "Default")?;
        // Allow duplicate suffixes for auto-generated playlists.
        let final_name =
            self.resolve_unique_playlist_name(&connection, &profile_id, &playlist_name_str)?;

        let playlist_id = Uuid::new_v4().to_string();
        let now = now_timestamp();
        connection
            .execute(
                "INSERT INTO playlists (id, profile_id, name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![playlist_id, profile_id, final_name, now],
            )
            .map_err(|error| format!("Failed to create playlist: {error}"))?;

        let tx = connection
            .transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;

        for (index, track) in tracks.iter().enumerate() {
            tx.execute(
                "INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![playlist_id, track.id, index as i64, now],
            )
            .map_err(|error| {
                format!("Failed to add track to album playlist: {error}")
            })?;
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit album playlist transaction: {e}"))?;

        self.playlist_info(&connection, &playlist_id)?
            .ok_or_else(|| "Playlist vanished immediately after creation".to_string())
    }

    /// Create a playlist from all tracks matching the given artist name.
    /// Uses the artist name as the playlist name unless `playlist_name` is provided.
    /// Returns an error if no tracks are found for the given artist.
    pub fn create_artist_playlist(
        &self,
        artist: &str,
        playlist_name: Option<&str>,
    ) -> Result<PlaylistInfo, String> {
        let artist = artist.trim();
        if artist.is_empty() {
            return Err("Artist name cannot be empty".to_string());
        }

        let mut connection = self.lock_connection()?;
        let tracks = Self::get_tracks_by_column(&connection, &self.cover_root, "artist", artist)?;
        if tracks.is_empty() {
            return Err(format!("No tracks found for artist \"{artist}\""));
        }

        let name = playlist_name.unwrap_or(artist);
        let playlist_name_str = name.to_string();

        let profile_id =
            ensure_profile_with_connection(&connection, "default", "Default")?;
        let final_name =
            self.resolve_unique_playlist_name(&connection, &profile_id, &playlist_name_str)?;

        let playlist_id = Uuid::new_v4().to_string();
        let now = now_timestamp();
        connection
            .execute(
                "INSERT INTO playlists (id, profile_id, name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![playlist_id, profile_id, final_name, now],
            )
            .map_err(|error| format!("Failed to create playlist: {error}"))?;

        let tx = connection
            .transaction()
            .map_err(|e| format!("Failed to begin transaction: {e}"))?;

        for (index, track) in tracks.iter().enumerate() {
            tx.execute(
                "INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![playlist_id, track.id, index as i64, now],
            )
            .map_err(|error| {
                format!("Failed to add track to artist playlist: {error}")
            })?;
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit artist playlist transaction: {e}"))?;

        self.playlist_info(&connection, &playlist_id)?
            .ok_or_else(|| "Playlist vanished immediately after creation".to_string())
    }

    /// Query tracks where a given column equals a value, ordered by
    /// album → disc_number → track_number for sensible ordering.
    fn get_tracks_by_column(
        connection: &Connection,
        cover_root: &Path,
        column: &str,
        value: &str,
    ) -> Result<Vec<Track>, String> {
        let sql = format!(
            "SELECT {TRACK_SELECT_COLUMNS}
             FROM {TRACK_FROM}
             WHERE t.{column} = ?1
             ORDER BY t.album, COALESCE(t.disc_number, 1), COALESCE(t.track_number, 0)"
        );
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| format!("Failed to prepare query by {column}: {error}"))?;
        let tracks = statement
            .query_map(params![value], |row| row_to_track(row, cover_root))
            .map_err(|error| format!("Failed to query by {column}: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read tracks by {column}: {error}"))?;
        Ok(tracks)
    }

    // ── Album / artist browsing & querying ───────────────────────────────────

    /// List every distinct album in the library grouped by
    /// `(album, COALESCE(album_artist, artist))`, ordered by album artist then
    /// album name. Each entry carries aggregate info (track count, year,
    /// representative cover art) suitable for a browse grid.
    pub fn list_albums(&self) -> Result<Vec<AlbumSummaryDto>, String> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(
                &format!(
                    "SELECT
                    t.album,
                    COALESCE(NULLIF(t.album_artist, ''), t.artist) AS album_artist,
                    MIN(t.artist) AS artist,
                    COUNT(*) AS track_count,
                    MIN(t.year) AS year,
                    MIN(aa.thumb_path) AS cover_art_data_url,
                    MIN(COALESCE(aa.mime, t.cover_art_mime)) AS cover_art_mime,
                    MIN(t.path) AS cover_track_path
                 FROM {TRACK_FROM}
                 GROUP BY t.album, COALESCE(NULLIF(t.album_artist, ''), t.artist)
                 ORDER BY album_artist, t.album"
                ),
            )
            .map_err(|error| format!("Failed to prepare albums query: {error}"))?;
        let albums = statement
            .query_map([], |row| row_to_album_summary(row, &self.cover_root))
            .map_err(|error| format!("Failed to query albums: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read albums: {error}"))?;
        Ok(albums)
    }

    /// List every distinct artist in the library (by the track `artist` tag),
    /// with aggregate track and album counts, ordered by artist name.
    pub fn list_artists(&self) -> Result<Vec<ArtistSummaryDto>, String> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(
                "SELECT
                    t.artist,
                    COUNT(*) AS track_count,
                    COUNT(DISTINCT t.album) AS album_count
                 FROM tracks t
                 GROUP BY t.artist
                 ORDER BY t.artist",
            )
            .map_err(|error| format!("Failed to prepare artists query: {error}"))?;
        let artists = statement
            .query_map([], row_to_artist_summary)
            .map_err(|error| format!("Failed to query artists: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read artists: {error}"))?;
        Ok(artists)
    }

    /// Return every track belonging to an album.
    ///
    /// When `album_artist` is provided, tracks are matched on both `album` and
    /// the resolved album artist (`COALESCE(album_artist, artist)`). This keeps
    /// same-named albums by different artists apart — pass the value from an
    /// [`AlbumSummaryDto`] (or a clicked `Track`'s `album_artist` falling back to
    /// `artist`) for a precise, Spotify-style "go to album" result.
    ///
    /// When `album_artist` is `None`, only the `album` name is matched (which may
    /// merge same-named albums across artists).
    ///
    /// Tracks are ordered by disc number then track number.
    pub fn get_tracks_by_album(
        &self,
        album: &str,
        album_artist: Option<&str>,
    ) -> Result<Vec<Track>, String> {
        let album = album.trim();
        if album.is_empty() {
            return Err("Album name cannot be empty".to_string());
        }
        let album_artist = album_artist.map(str::trim).filter(|a| !a.is_empty());

        let connection = self.lock_connection()?;
        let sql = if album_artist.is_some() {
            format!(
                "SELECT {TRACK_SELECT_COLUMNS}
                 FROM {TRACK_FROM}
                 WHERE t.album = ?1
                   AND COALESCE(NULLIF(t.album_artist, ''), t.artist) = ?2
                 ORDER BY COALESCE(t.disc_number, 1), COALESCE(t.track_number, 0)"
            )
        } else {
            format!(
                "SELECT {TRACK_SELECT_COLUMNS}
                 FROM {TRACK_FROM}
                 WHERE t.album = ?1
                 ORDER BY COALESCE(t.disc_number, 1), COALESCE(t.track_number, 0)"
            )
        };

        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| format!("Failed to prepare album tracks query: {error}"))?;

        let rows = if let Some(album_artist) = album_artist {
            statement
                .query_map(params![album, album_artist], |row| {
                    row_to_track(row, &self.cover_root)
                })
                .map_err(|error| format!("Failed to query album tracks: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("Failed to read album tracks: {error}"))?
        } else {
            statement
                .query_map(params![album], |row| row_to_track(row, &self.cover_root))
                .map_err(|error| format!("Failed to query album tracks: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("Failed to read album tracks: {error}"))?
        };

        Ok(rows)
    }

    /// Return every track by an artist (a discography), ordered by album then
    /// disc number then track number. Matches the track `artist` tag.
    pub fn get_tracks_by_artist(&self, artist: &str) -> Result<Vec<Track>, String> {
        let artist = artist.trim();
        if artist.is_empty() {
            return Err("Artist name cannot be empty".to_string());
        }
        let connection = self.lock_connection()?;
        Self::get_tracks_by_column(&connection, &self.cover_root, "artist", artist)
    }

    /// Return distinct albums by an artist, with aggregate info suitable for
    /// an artist page (album grid / discography list).
    pub fn get_artist_albums(&self, artist: &str) -> Result<Vec<AlbumSummaryDto>, String> {
        let artist = artist.trim();
        if artist.is_empty() {
            return Err("Artist name cannot be empty".to_string());
        }
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(
                &format!(
                    "SELECT
                    t.album,
                    COALESCE(NULLIF(t.album_artist, ''), t.artist) AS album_artist,
                    MIN(t.artist) AS artist,
                    COUNT(*) AS track_count,
                    MIN(t.year) AS year,
                    MIN(aa.thumb_path) AS cover_art_data_url,
                    MIN(COALESCE(aa.mime, t.cover_art_mime)) AS cover_art_mime,
                    MIN(t.path) AS cover_track_path
                 FROM {TRACK_FROM}
                 WHERE t.artist = ?1
                 GROUP BY t.album, COALESCE(NULLIF(t.album_artist, ''), t.artist)
                 ORDER BY MIN(COALESCE(t.year, 9999)), t.album"
                ),
            )
            .map_err(|error| format!("Failed to prepare artist albums query: {error}"))?;
        let albums = statement
            .query_map(params![artist], |row| row_to_album_summary(row, &self.cover_root))
            .map_err(|error| format!("Failed to query artist albums: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read artist albums: {error}"))?;
        Ok(albums)
    }

    pub fn get_playlist_info(&self, playlist_id: &str) -> Result<Option<PlaylistInfo>, String> {
        let connection = self.lock_connection()?;
        self.playlist_info(&connection, playlist_id)
    }

    fn playlist_info(
        &self,
        connection: &Connection,
        playlist_id: &str,
    ) -> Result<Option<PlaylistInfo>, String> {
        connection
            .query_row(
                "SELECT p.id, p.profile_id, p.name, COUNT(pt.track_id), p.created_at, p.updated_at,
                        p.sync_folder
                 FROM playlists p
                 LEFT JOIN playlist_tracks pt ON pt.playlist_id = p.id
                 WHERE p.id = ?1
                 GROUP BY p.id",
                params![playlist_id],
                row_to_playlist,
            )
            .optional()
            .map_err(|error| format!("Failed to query playlist: {error}"))
    }

    /// Look up full `Track` records for a list of file paths (used by the queue).
    /// Returns `Some(track)` for found tracks and `None` for paths not in the
    /// library, preserving the input order.
    /// Look up a single track by its UUID.
    pub fn get_track_by_id(&self, track_id: &str) -> Result<Option<Track>, String> {
        let connection = self.lock_connection()?;
        connection
            .query_row(
                &format!("SELECT {TRACK_SELECT_COLUMNS} FROM {TRACK_FROM} WHERE t.id = ?1"),
                params![track_id],
                |row| row_to_track(row, &self.cover_root),
            )
            .optional()
            .map_err(|e| format!("Failed to query track by id: {e}"))
    }

    /// Search tracks by a query string matching title, artist, or album.
    pub fn search_tracks(&self, query: &str) -> Result<Vec<Track>, String> {
        self.search_tracks_limited(query, None)
    }

    /// Search tracks with an optional result limit.
    pub fn search_tracks_limited(
        &self,
        query: &str,
        limit: Option<u32>,
    ) -> Result<Vec<Track>, String> {
        Ok(self
            .search_tracks_rich(query, limit)?
            .into_iter()
            .map(|hit| hit.track)
            .collect())
    }

    /// Fast realtime search across title, artist, album, filename, and lyrics.
    /// Uses FTS5 when available, with a LIKE fallback.
    pub fn search_tracks_rich(
        &self,
        query: &str,
        limit: Option<u32>,
    ) -> Result<Vec<SearchHitDto>, String> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.unwrap_or(80).min(200) as i64;
        let connection = self.read_connection();

        let fts_hits = search_tracks_fts(&connection, &self.cover_root, query, limit);
        match fts_hits {
            Ok(hits) if !hits.is_empty() || fts_table_ready(&connection) => Ok(hits),
            Ok(_) | Err(_) => search_tracks_like(&connection, &self.cover_root, query, limit),
        }
    }

    /// Search playlists by name.
    pub fn search_playlists(&self, query: &str) -> Result<Vec<PlaylistInfo>, String> {
        let pattern = format!("%{}%", query);
        let connection = self.lock_connection()?;
        let mut stmt = connection
            .prepare(
                "SELECT p.id, p.profile_id, p.name, COUNT(pt.track_id), p.created_at, p.updated_at,
                        p.sync_folder
                 FROM playlists p
                 LEFT JOIN playlist_tracks pt ON pt.playlist_id = p.id
                 WHERE p.name LIKE ?1
                 GROUP BY p.id ORDER BY p.updated_at DESC",
            )
            .map_err(|e| format!("Failed to prepare playlist search query: {e}"))?;
        let rows = stmt
            .query_map(params![pattern], row_to_playlist)
            .map_err(|e| format!("Failed to execute playlist search: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read search results: {e}"))
    }

    /// Update a track's cover art from raw image bytes (stored as shared thumb).
    pub fn set_track_cover(
        &self,
        track_id: &str,
        image_data: &[u8],
        mime_type: &str,
    ) -> Result<(), String> {
        let Some(app) = &self.app_handle else {
            return Err("Cover update requires app handle".into());
        };
        let saved = crate::cover_art::save_album_art_thumb(
            app,
            crate::cover_art::ExtractedCoverArt {
                data: image_data.to_vec(),
                mime: mime_type.to_string(),
            },
        )?;
        let connection = self.write_connection();
        let now = now_timestamp();
        connection
            .execute(
                "INSERT OR IGNORE INTO album_art (id, thumb_path, mime, byte_size, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    saved.id,
                    saved.relative_path,
                    saved.mime,
                    saved.byte_size,
                    now
                ],
            )
            .map_err(|e| format!("Failed to upsert album_art: {e}"))?;
        connection
            .execute(
                "UPDATE tracks SET album_art_id = ?1, cover_art_mime = ?2,
                        cover_art_source = 'user', cover_art_data_url = NULL
                 WHERE id = ?3",
                params![saved.id, saved.mime, track_id],
            )
            .map_err(|e| format!("Failed to update track cover: {e}"))?;
        Ok(())
    }

    /// Persist album_art_id / mime after deferred online enrich (no full re-upsert).
    pub fn apply_track_art_update(&self, track: &Track) -> Result<(), String> {
        let Some(ref art_id) = track.album_art_id else {
            return Ok(());
        };
        let connection = self.write_connection();
        let rel = format!("thumbs/{art_id}.jpg");
        let mime = track
            .cover_art_mime
            .clone()
            .unwrap_or_else(|| "image/jpeg".into());
        connection
            .execute(
                "INSERT OR IGNORE INTO album_art (id, thumb_path, mime, byte_size, created_at)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                params![art_id, rel, mime, now_timestamp()],
            )
            .map_err(|e| format!("Failed to insert album_art: {e}"))?;
        connection
            .execute(
                "UPDATE tracks SET album_art_id = ?1, cover_art_mime = ?2,
                        cover_art_source = COALESCE(?3, cover_art_source),
                        cover_art_data_url = NULL
                 WHERE path = ?4",
                params![art_id, mime, track.cover_art_source, track.path],
            )
            .map_err(|e| format!("Failed to update track art: {e}"))?;
        Ok(())
    }

    /// Full track detail including lyrics (for lyrics panel).
    pub fn get_track_details(&self, path: &str) -> Result<Option<Track>, String> {
        let connection = self.read_connection();
        connection
            .query_row(
                &format!(
                    "SELECT {TRACK_DETAIL_COLUMNS}
                     FROM {TRACK_FROM}
                     WHERE t.path = ?1"
                ),
                params![path],
                |row| row_to_track(row, &self.cover_root),
            )
            .optional()
            .map_err(|error| format!("Failed to query track details: {error}"))
    }

    pub fn set_track_lyrics(
        &self,
        track_id: &str,
        lyrics: &str,
        source: &str,
    ) -> Result<(), String> {
        let connection = self.write_connection();
        connection
            .execute(
                "UPDATE tracks SET lyrics = ?1, lyrics_source = ?2 WHERE id = ?3",
                params![lyrics, source, track_id],
            )
            .map_err(|e| format!("Failed to update track lyrics: {e}"))?;
        // Refresh FTS row for lyrics search.
        if let Ok(Some(track)) = connection.query_row(
            &format!("SELECT {TRACK_DETAIL_COLUMNS} FROM {TRACK_FROM} WHERE t.id = ?1"),
            params![track_id],
            |row| row_to_track(row, &self.cover_root),
        ).optional() {
            let _ = sync_track_fts(&connection, &track);
        }
        Ok(())
    }

    pub fn get_tracks_by_paths(&self, paths: &[String]) -> Result<Vec<Option<Track>>, String> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.lock_connection()?;
        let mut tracks = Vec::with_capacity(paths.len());
        for path in paths {
            let track = connection
                .query_row(
                    &format!(
                        "SELECT {TRACK_SELECT_COLUMNS}
                         FROM {TRACK_FROM}
                         WHERE t.path = ?1"
                    ),
                    params![path],
                    |row| row_to_track(row, &self.cover_root),
                )
                .optional()
                .map_err(|error| format!("Failed to query track by path: {error}"))?;
            tracks.push(track);
        }
        Ok(tracks)
    }

    // ── Export / Import ──────────────────────────────────────────────────────

    /// Export a playlist as an M3U8 file (a plain-text list of file paths).
    pub fn export_playlist_m3u(
        &self,
        playlist_id: &str,
        output_path: &str,
    ) -> Result<(), String> {
        let tracks = self.get_playlist_tracks(playlist_id)?;
        let mut content = String::from("#EXTM3U\n");
        for track in &tracks {
            let duration = track.duration_seconds.map(|d| d as i64).unwrap_or(-1);
            content.push_str(&format!(
                "#EXTINF:{},{} - {}\n",
                duration, track.artist, track.title
            ));
            content.push_str(&track.path);
            content.push('\n');
        }
        std::fs::write(output_path, content)
            .map_err(|error| format!("Failed to write M3U file: {error}"))?;
        Ok(())
    }

    /// Import an M3U/M3U8 file, creating a new playlist and adding all
    /// referenced files to it. Returns the new playlist id and imported tracks.
    pub fn import_playlist_m3u(
        &self,
        m3u_path: &str,
        playlist_name: Option<&str>,
    ) -> Result<(String, Vec<Track>), String> {
        let content =
            std::fs::read_to_string(m3u_path).map_err(|error| format!("Failed to read M3U file: {error}"))?;
        let name = playlist_name.unwrap_or_else(|| {
            Path::new(m3u_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Imported Playlist")
        });

        let playlist_info = self.create_playlist_for_import(name)?;
        let playlist_id = playlist_info.id;

        let mut tracks = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match self.add_track_to_playlist(&playlist_id, line.to_string()) {
                Ok(track) => tracks.push(track),
                Err(error) => tracing::warn!("Skipped during M3U import — {line}: {error}"),
            }
        }
        Ok((playlist_id, tracks))
    }

    /// Export a playlist as a Wave JSON file (paths + metadata).
    pub fn export_playlist_json(
        &self,
        playlist_id: &str,
        output_path: &str,
    ) -> Result<(), String> {
        let tracks = self.get_playlist_tracks(playlist_id)?;
        let info = self
            .get_playlist_info(playlist_id)?
            .ok_or("Playlist not found")?;

        let export = PlaylistExportJson {
            format: "wave-playlist".to_string(),
            version: 1,
            name: info.name,
            exported_at: now_timestamp(),
            tracks: tracks
                .iter()
                .map(|t| TrackExportJson {
                    path: t.path.clone(),
                    title: t.title.clone(),
                    artist: t.artist.clone(),
                    album: t.album.clone(),
                    duration_seconds: t.duration_seconds,
                })
                .collect(),
        };

        let json = serde_json::to_string_pretty(&export)
            .map_err(|error| format!("Failed to serialize playlist JSON: {error}"))?;
        std::fs::write(output_path, json)
            .map_err(|error| format!("Failed to write JSON file: {error}"))?;
        Ok(())
    }

    /// Import a Wave JSON playlist file, creating a new playlist.
    pub fn import_playlist_json(
        &self,
        json_path: &str,
        playlist_name: Option<&str>,
    ) -> Result<(String, Vec<Track>), String> {
        let content = std::fs::read_to_string(json_path)
            .map_err(|error| format!("Failed to read JSON file: {error}"))?;
        let export: PlaylistExportJson = serde_json::from_str(&content)
            .map_err(|error| format!("Failed to parse playlist JSON: {error}"))?;
        if export.format != "wave-playlist" {
            return Err(format!(
                "Unsupported playlist format: {} (expected wave-playlist)",
                export.format
            ));
        }
        if export.tracks.len() > 10_000 {
            return Err(format!(
                "Playlist has too many tracks ({}; max 10000)",
                export.tracks.len()
            ));
        }

        let name = playlist_name.unwrap_or(&export.name);
        let playlist_info = self.create_playlist_for_import(name)?;
        let playlist_id = playlist_info.id;

        let mut tracks = Vec::new();
        for track in &export.tracks {
            match self.add_track_to_playlist(&playlist_id, track.path.clone()) {
                Ok(t) => tracks.push(t),
                Err(error) => {
                    tracing::warn!("Skipped during JSON import — {}: {error}", track.path)
                }
            }
        }
        Ok((playlist_id, tracks))
    }

    // ── Listen stats / recommendations ────────────────────────────────────────

    /// Persist a finished listen session for a track path.
    pub fn record_listen(
        &self,
        path: &str,
        seconds: f64,
        completed: bool,
        skipped: bool,
        from_path: Option<&str>,
    ) -> Result<(), String> {
        if seconds < 1.0 && !completed && !skipped {
            return Ok(());
        }
        let connection = self.write_connection();
        let track_id = match resolve_track_id_by_path(&connection, path)? {
            Some(id) => id,
            None => return Ok(()),
        };
        let now = now_timestamp();
        let play_inc: i64 = if completed { 1 } else { 0 };
        let skip_inc: i64 = if skipped { 1 } else { 0 };
        let secs = seconds.max(0.0);

        connection
            .execute(
                "INSERT INTO listen_stats (track_id, play_count, skip_count, listen_seconds, last_played_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(track_id) DO UPDATE SET
                   play_count = play_count + excluded.play_count,
                   skip_count = skip_count + excluded.skip_count,
                   listen_seconds = listen_seconds + excluded.listen_seconds,
                   last_played_at = excluded.last_played_at",
                params![track_id, play_inc, skip_inc, secs, now],
            )
            .map_err(|e| format!("Failed to update listen stats: {e}"))?;

        // Transitions are recorded when the *next* track starts (see touch), or
        // here when closing a session that knows its predecessor.
        if let Some(from) = from_path.filter(|p| !p.is_empty() && *p != path) {
            if let Some(from_id) = resolve_track_id_by_path(&connection, from)? {
                if from_id != track_id {
                    let kind = if skipped { "skip" } else { "complete" };
                    connection
                        .execute(
                            "INSERT INTO track_transitions (from_track_id, to_track_id, kind, count, last_at)
                             VALUES (?1, ?2, ?3, 1, ?4)
                             ON CONFLICT(from_track_id, to_track_id, kind) DO UPDATE SET
                               count = count + 1,
                               last_at = excluded.last_at",
                            params![from_id, track_id, kind, now],
                        )
                        .map_err(|e| format!("Failed to update track transition: {e}"))?;
                }
            }
        }
        Ok(())
    }

    /// Mark a track as recently played immediately when playback starts.
    /// Does not bump play_count / listen_seconds.
    pub fn touch_last_played(&self, path: &str) -> Result<(), String> {
        let connection = self.write_connection();
        let track_id = match resolve_track_id_by_path(&connection, path)? {
            Some(id) => id,
            None => return Ok(()),
        };
        let now = now_timestamp();
        connection
            .execute(
                "INSERT INTO listen_stats (track_id, play_count, skip_count, listen_seconds, last_played_at)
                 VALUES (?1, 0, 0, 0, ?2)
                 ON CONFLICT(track_id) DO UPDATE SET
                   last_played_at = excluded.last_played_at",
                params![track_id, now],
            )
            .map_err(|e| format!("Failed to touch last played: {e}"))?;
        Ok(())
    }

    /// Top recently played tracks (by `last_played_at`).
    pub fn get_recently_played(&self, limit: u32) -> Result<Vec<Track>, String> {
        let limit = limit.clamp(1, 200) as i64;
        let connection = self.read_connection();
        let mut stmt = connection
            .prepare(&format!(
                "SELECT {TRACK_SELECT_COLUMNS}
                 FROM {TRACK_FROM}
                 INNER JOIN listen_stats ls ON ls.track_id = t.id
                 WHERE ls.last_played_at > 0
                 ORDER BY ls.last_played_at DESC
                 LIMIT ?1"
            ))
            .map_err(|e| format!("Failed to prepare recently played query: {e}"))?;
        let tracks = stmt
            .query_map(params![limit], |row| row_to_track(row, &self.cover_root))
            .map_err(|e| format!("Failed to query recently played: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read recently played: {e}"))?;
        Ok(tracks)
    }

    /// Top most-played tracks (by play_count, then listen_seconds).
    pub fn get_most_played(&self, limit: u32) -> Result<Vec<Track>, String> {
        let limit = limit.clamp(1, 200) as i64;
        let connection = self.read_connection();
        let mut stmt = connection
            .prepare(&format!(
                "SELECT {TRACK_SELECT_COLUMNS}
                 FROM {TRACK_FROM}
                 INNER JOIN listen_stats ls ON ls.track_id = t.id
                 WHERE ls.play_count > 0 OR ls.listen_seconds >= 30
                 ORDER BY ls.play_count DESC, ls.listen_seconds DESC, ls.last_played_at DESC
                 LIMIT ?1"
            ))
            .map_err(|e| format!("Failed to prepare most played query: {e}"))?;
        let tracks = stmt
            .query_map(params![limit], |row| row_to_track(row, &self.cover_root))
            .map_err(|e| format!("Failed to query most played: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read most played: {e}"))?;
        Ok(tracks)
    }

    /// Favorite song: highest play_count, then listen time.
    pub fn get_favorite_track(&self) -> Result<Option<Track>, String> {
        Ok(self.get_most_played(1)?.into_iter().next())
    }

    /// Favorite album by aggregated listen seconds across its tracks.
    pub fn get_favorite_album(&self) -> Result<Option<AlbumSummaryDto>, String> {
        let connection = self.read_connection();
        connection
            .query_row(
                &format!(
                    "SELECT
                        t.album,
                        COALESCE(NULLIF(t.album_artist, ''), t.artist) AS album_artist,
                        MIN(t.artist) AS artist,
                        COUNT(*) AS track_count,
                        MIN(t.year) AS year,
                        MIN(aa.thumb_path) AS cover_art_data_url,
                        MIN(COALESCE(aa.mime, t.cover_art_mime)) AS cover_art_mime,
                        MIN(t.path) AS cover_track_path
                     FROM {TRACK_FROM}
                     INNER JOIN listen_stats ls ON ls.track_id = t.id
                     WHERE t.album IS NOT NULL AND TRIM(t.album) != ''
                       AND LOWER(TRIM(t.album)) NOT IN ('unknown album', 'unknown', 'untitled')
                     GROUP BY t.album, COALESCE(NULLIF(t.album_artist, ''), t.artist)
                     ORDER BY SUM(ls.listen_seconds) DESC, SUM(ls.play_count) DESC
                     LIMIT 1"
                ),
                [],
                |row| row_to_album_summary(row, &self.cover_root),
            )
            .optional()
            .map_err(|e| format!("Failed to query favorite album: {e}"))
    }

    /// Favorite artist by aggregated listen seconds.
    pub fn get_favorite_artist(&self) -> Result<Option<ArtistSummaryDto>, String> {
        let connection = self.read_connection();
        connection
            .query_row(
                "SELECT
                    t.artist,
                    COUNT(*) AS track_count,
                    COUNT(DISTINCT t.album) AS album_count
                 FROM tracks t
                 INNER JOIN listen_stats ls ON ls.track_id = t.id
                 WHERE t.artist IS NOT NULL AND TRIM(t.artist) != ''
                   AND LOWER(TRIM(t.artist)) NOT IN ('unknown artist', 'unknown', 'various', 'various artists')
                 GROUP BY t.artist
                 ORDER BY SUM(ls.listen_seconds) DESC, SUM(ls.play_count) DESC
                 LIMIT 1",
                [],
                row_to_artist_summary,
            )
            .optional()
            .map_err(|e| format!("Failed to query favorite artist: {e}"))
    }

    /// Compact listening overview for Settings.
    pub fn get_listening_stats(&self, limit: u32) -> Result<ListeningStatsDto, String> {
        let limit = limit.clamp(1, 20) as i64;
        let top_tracks = self.get_most_played(limit as u32)?;
        let connection = self.read_connection();

        let (total_listen_seconds, total_plays, tracks_played): (f64, i64, i64) = connection
            .query_row(
                "SELECT
                    COALESCE(SUM(listen_seconds), 0),
                    COALESCE(SUM(play_count), 0),
                    COUNT(*)
                 FROM listen_stats
                 WHERE last_played_at > 0 OR listen_seconds > 0 OR play_count > 0",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Failed to query listen totals: {e}"))?;

        let mut artist_stmt = connection
            .prepare(
                "SELECT
                    t.artist AS name,
                    COALESCE(SUM(ls.listen_seconds), 0) AS listen_seconds,
                    COALESCE(SUM(ls.play_count), 0) AS play_count
                 FROM tracks t
                 INNER JOIN listen_stats ls ON ls.track_id = t.id
                 WHERE t.artist IS NOT NULL AND TRIM(t.artist) != ''
                   AND LOWER(TRIM(t.artist)) NOT IN ('unknown artist', 'unknown', 'various', 'various artists')
                 GROUP BY t.artist
                 ORDER BY listen_seconds DESC, play_count DESC
                 LIMIT ?1",
            )
            .map_err(|e| format!("Failed to prepare top artists: {e}"))?;
        let top_artists = artist_stmt
            .query_map(params![limit], row_to_listen_rank)
            .map_err(|e| format!("Failed to query top artists: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read top artists: {e}"))?;

        let mut album_stmt = connection
            .prepare(
                "SELECT
                    t.album AS name,
                    COALESCE(SUM(ls.listen_seconds), 0) AS listen_seconds,
                    COALESCE(SUM(ls.play_count), 0) AS play_count
                 FROM tracks t
                 INNER JOIN listen_stats ls ON ls.track_id = t.id
                 WHERE t.album IS NOT NULL AND TRIM(t.album) != ''
                   AND LOWER(TRIM(t.album)) NOT IN ('unknown album', 'unknown', 'untitled')
                 GROUP BY t.album, COALESCE(NULLIF(t.album_artist, ''), t.artist)
                 ORDER BY listen_seconds DESC, play_count DESC
                 LIMIT ?1",
            )
            .map_err(|e| format!("Failed to prepare top albums: {e}"))?;
        let top_albums = album_stmt
            .query_map(params![limit], row_to_listen_rank)
            .map_err(|e| format!("Failed to query top albums: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read top albums: {e}"))?;

        let mut genre_stmt = connection
            .prepare(
                "SELECT
                    TRIM(t.genre) AS name,
                    COALESCE(SUM(ls.listen_seconds), 0) AS listen_seconds,
                    COALESCE(SUM(ls.play_count), 0) AS play_count
                 FROM tracks t
                 INNER JOIN listen_stats ls ON ls.track_id = t.id
                 WHERE t.genre IS NOT NULL AND TRIM(t.genre) != ''
                   AND LOWER(TRIM(t.genre)) NOT IN ('unknown', 'unknown genre')
                 GROUP BY LOWER(TRIM(t.genre))
                 ORDER BY listen_seconds DESC, play_count DESC
                 LIMIT ?1",
            )
            .map_err(|e| format!("Failed to prepare top genres: {e}"))?;
        let top_genres = genre_stmt
            .query_map(params![limit], row_to_listen_rank)
            .map_err(|e| format!("Failed to query top genres: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read top genres: {e}"))?;

        Ok(ListeningStatsDto {
            total_listen_seconds,
            total_plays,
            tracks_played,
            top_tracks,
            top_artists,
            top_albums,
            top_genres,
        })
    }

    /// Build Home suggestions using listen history and track-to-track edges.
    pub fn get_home_suggestions(
        &self,
        seed_path: Option<&str>,
    ) -> Result<HomeSuggestionsDto, String> {
        let favorite_track = self.get_favorite_track()?;
        let favorite_album = self.get_favorite_album()?;
        let favorite_artist = self.get_favorite_artist()?;
        let recent = self.get_recently_played(40)?;
        let most = self.get_most_played(40)?;
        let curated = !recent.is_empty() || !most.is_empty();

        let connection = self.read_connection();
        let mut candidates: Vec<Track> = Vec::new();
        let mut seen = std::collections::HashSet::<String>::new();

        let push_track = |track: Track, candidates: &mut Vec<Track>, seen: &mut std::collections::HashSet<String>| {
            if seen.insert(track.path.clone()) {
                candidates.push(track);
            }
        };

        // Transition neighbors from seed / current / favorite.
        let seed_ids: Vec<String> = {
            let mut ids = Vec::new();
            for path in seed_path
                .into_iter()
                .chain(favorite_track.as_ref().map(|t| t.path.as_str()))
                .chain(recent.first().map(|t| t.path.as_str()))
            {
                if let Ok(Some(id)) = resolve_track_id_by_path(&connection, path) {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
            ids
        };

        for seed_id in &seed_ids {
            let mut stmt = connection
                .prepare(&format!(
                    "SELECT {TRACK_SELECT_COLUMNS}
                     FROM {TRACK_FROM}
                     INNER JOIN track_transitions tr ON tr.to_track_id = t.id
                     WHERE tr.from_track_id = ?1 AND tr.kind = 'complete'
                     ORDER BY tr.count DESC, tr.last_at DESC
                     LIMIT 24"
                ))
                .map_err(|e| format!("Failed to prepare transition query: {e}"))?;
            let rows = stmt
                .query_map(params![seed_id], |row| row_to_track(row, &self.cover_root))
                .map_err(|e| format!("Failed to query transitions: {e}"))?;
            for row in rows {
                push_track(row.map_err(|e| format!("Failed to read transition: {e}"))?, &mut candidates, &mut seen);
            }
        }
        drop(connection);

        for track in most.iter().chain(recent.iter()) {
            push_track(track.clone(), &mut candidates, &mut seen);
        }

        // Same-artist / same-album enrichment from top listens.
        if let Some(fav) = favorite_track.as_ref() {
            if !fav.artist.trim().is_empty() {
                for track in self.get_tracks_by_artist(&fav.artist)?.into_iter().take(20) {
                    push_track(track, &mut candidates, &mut seen);
                }
            }
            if !fav.album.trim().is_empty() {
                let aa = fav
                    .album_artist
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .or(Some(fav.artist.as_str()));
                for track in self
                    .get_tracks_by_album(&fav.album, aa)?
                    .into_iter()
                    .take(16)
                {
                    push_track(track, &mut candidates, &mut seen);
                }
            }
        }

        // Cold-start / fill from library.
        if candidates.len() < 40 {
            let connection = self.read_connection();
            let mut stmt = connection
                .prepare(&format!(
                    "SELECT {TRACK_SELECT_COLUMNS}
                     FROM {TRACK_FROM}
                     ORDER BY t.indexed_at DESC
                     LIMIT 200"
                ))
                .map_err(|e| format!("Failed to prepare library fill: {e}"))?;
            let rows = stmt
                .query_map([], |row| row_to_track(row, &self.cover_root))
                .map_err(|e| format!("Failed to fill suggestions: {e}"))?;
            for row in rows {
                push_track(row.map_err(|e| format!("Failed to read fill track: {e}"))?, &mut candidates, &mut seen);
                if candidates.len() >= 120 {
                    break;
                }
            }
        }

        shuffle_tracks(&mut candidates);

        let featured = candidates.first().cloned().or_else(|| favorite_track.clone());
        let mix: Vec<Track> = candidates.iter().skip(1).take(8).cloned().collect();
        let more: Vec<Track> = candidates
            .iter()
            .skip(1 + mix.len())
            .take(10)
            .cloned()
            .collect();

        let albums = self.suggest_albums(8, favorite_album.as_ref())?;

        Ok(HomeSuggestionsDto {
            featured,
            mix,
            more,
            albums,
            favorite_track,
            favorite_album,
            favorite_artist,
            curated,
        })
    }

    fn suggest_albums(
        &self,
        limit: usize,
        favorite: Option<&AlbumSummaryDto>,
    ) -> Result<Vec<AlbumSummaryDto>, String> {
        let connection = self.read_connection();
        let mut stmt = connection
            .prepare(&format!(
                "SELECT
                    t.album,
                    COALESCE(NULLIF(t.album_artist, ''), t.artist) AS album_artist,
                    MIN(t.artist) AS artist,
                    COUNT(*) AS track_count,
                    MIN(t.year) AS year,
                    MIN(aa.thumb_path) AS cover_art_data_url,
                    MIN(COALESCE(aa.mime, t.cover_art_mime)) AS cover_art_mime,
                    MIN(t.path) AS cover_track_path,
                    COALESCE(SUM(ls.listen_seconds), 0) AS listen_score
                 FROM {TRACK_FROM}
                 LEFT JOIN listen_stats ls ON ls.track_id = t.id
                 WHERE t.album IS NOT NULL AND TRIM(t.album) != ''
                 GROUP BY t.album, COALESCE(NULLIF(t.album_artist, ''), t.artist)
                 ORDER BY listen_score DESC, track_count DESC
                 LIMIT 40"
            ))
            .map_err(|e| format!("Failed to prepare album suggestions: {e}"))?;

        let mut albums = stmt
            .query_map([], |row| {
                let summary = row_to_album_summary(row, &self.cover_root)?;
                let score: f64 = row.get(8)?;
                Ok((summary, score))
            })
            .map_err(|e| format!("Failed to query album suggestions: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read album suggestions: {e}"))?;

        // Light shuffle among the top tier so refresh still feels alive.
        let top_tier: Vec<_> = albums
            .iter()
            .filter(|(_, score)| *score > 0.0)
            .cloned()
            .collect();
        let rest: Vec<_> = albums
            .iter()
            .filter(|(_, score)| *score <= 0.0)
            .cloned()
            .collect();
        let mut ranked = top_tier;
        shuffle_pairs(&mut ranked);
        let mut cold = rest;
        shuffle_pairs(&mut cold);
        ranked.extend(cold);
        albums = ranked;

        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::<String>::new();
        if let Some(fav) = favorite {
            let key = format!("{}::{}", fav.name, fav.album_artist.clone().unwrap_or_default());
            seen.insert(key);
            out.push(fav.clone());
        }
        for (album, _) in albums {
            let key = format!(
                "{}::{}",
                album.name,
                album.album_artist.clone().unwrap_or_default()
            );
            if seen.insert(key) {
                out.push(album);
            }
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }
}

fn shuffle_tracks(items: &mut [Track]) {
    let mut seed = now_timestamp() as u64
        ^ (items.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for i in (1..items.len()).rev() {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        let j = (seed as usize) % (i + 1);
        items.swap(i, j);
    }
}

fn shuffle_pairs<T: Clone>(items: &mut [(T, f64)]) {
    let mut seed = now_timestamp() as u64 ^ (items.len() as u64);
    for i in (1..items.len()).rev() {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        let j = (seed as usize) % (i + 1);
        items.swap(i, j);
    }
}

pub(crate) fn row_to_track(
    row: &rusqlite::Row<'_>,
    cover_root: &Path,
) -> rusqlite::Result<Track> {
    let thumb_rel: Option<String> = row.get(18)?;
    let album_art_id: Option<String> = row.get(28)?;
    let cover_art_data_url = resolve_cover_display_path(cover_root, thumb_rel.as_deref(), album_art_id.as_deref());
    Ok(Track {
        id: row.get(0)?,
        path: row.get(1)?,
        name: row.get(2)?,
        title: row.get(3)?,
        artist: row.get(4)?,
        album: row.get(5)?,
        album_artist: row.get(6)?,
        genre: row.get(7)?,
        year: row.get(8)?,
        track_number: row.get(9)?,
        disc_number: row.get(10)?,
        format: row.get(11)?,
        duration_seconds: row.get(12)?,
        sample_rate: row.get(13)?,
        channels: row.get(14)?,
        bit_depth: row.get(15)?,
        lyrics: row.get(16)?,
        lyrics_source: row.get(17)?,
        cover_art_data_url,
        cover_art_mime: row.get(19)?,
        cover_art_source: row.get(20)?,
        fingerprint_sha256: row.get(21)?,
        acoustid_fingerprint: row.get(22)?,
        musicbrainz_recording_id: row.get(23)?,
        file_size: row.get(24)?,
        modified_at: row.get(25)?,
        indexed_at: row.get(26)?,
        is_saf_uri: row.get(27)?,
        album_art_id,
    })
}

fn resolve_cover_display_path(
    cover_root: &Path,
    thumb_rel: Option<&str>,
    album_art_id: Option<&str>,
) -> Option<String> {
    if let Some(rel) = thumb_rel.filter(|s| !s.is_empty()) {
        let abs = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            cover_root.join(rel)
        };
        if abs.is_file() {
            return Some(abs.to_string_lossy().into_owned());
        }
    }
    if let Some(id) = album_art_id.filter(|s| !s.is_empty()) {
        let abs = cover_root.join("thumbs").join(format!("{id}.jpg"));
        if abs.is_file() {
            return Some(abs.to_string_lossy().into_owned());
        }
    }
    None
}

fn row_to_playlist(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlaylistInfo> {
    Ok(PlaylistInfo {
        id: row.get(0)?,
        profile_id: row.get(1)?,
        name: row.get(2)?,
        track_count: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        sync_folder: row.get(6)?,
    })
}

fn row_to_album_summary(
    row: &rusqlite::Row<'_>,
    cover_root: &Path,
) -> rusqlite::Result<AlbumSummaryDto> {
    let thumb_rel: Option<String> = row.get(5)?;
    Ok(AlbumSummaryDto {
        name: row.get(0)?,
        album_artist: row.get(1)?,
        artist: row.get(2)?,
        track_count: row.get(3)?,
        year: row.get(4)?,
        cover_art_data_url: resolve_cover_display_path(cover_root, thumb_rel.as_deref(), None),
        cover_art_mime: row.get(6)?,
        cover_track_path: row.get(7)?,
    })
}

fn row_to_artist_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtistSummaryDto> {
    Ok(ArtistSummaryDto {
        name: row.get(0)?,
        track_count: row.get(1)?,
        album_count: row.get(2)?,
    })
}

fn row_to_listen_rank(row: &rusqlite::Row<'_>) -> rusqlite::Result<ListenRankDto> {
    Ok(ListenRankDto {
        name: row.get(0)?,
        listen_seconds: row.get(1)?,
        play_count: row.get(2)?,
    })
}

/// A trait that abstracts over `Connection` and `Transaction` so that our
/// helpers can be called in both contexts without duplicating code.
trait Queryable {
    fn exec<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize>;
    fn query_opt<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<Option<T>>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>;
    fn query_vec<T, F>(&self, sql: &str, f: F) -> rusqlite::Result<Vec<T>>
    where
        F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>;
}

impl Queryable for Connection {
    fn exec<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.execute(sql, params)
    }
    fn query_opt<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<Option<T>>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        self.query_row(sql, params, f).optional()
    }
    fn query_vec<T, F>(&self, sql: &str, f: F) -> rusqlite::Result<Vec<T>>
    where
        F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        let mut statement = self.prepare(sql)?;
        let rows = statement.query_map([], f)?;
        rows.collect()
    }
}

impl Queryable for Transaction<'_> {
    fn exec<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.execute(sql, params)
    }
    fn query_opt<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<Option<T>>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        self.query_row(sql, params, f).optional()
    }
    fn query_vec<T, F>(&self, sql: &str, f: F) -> rusqlite::Result<Vec<T>>
    where
        F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        let mut statement = self.prepare(sql)?;
        let rows = statement.query_map([], f)?;
        rows.collect()
    }
}

impl Queryable for RwLockWriteGuard<'_, Connection> {
    fn exec<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.execute(sql, params)
    }
    fn query_opt<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<Option<T>>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        self.query_row(sql, params, f).optional()
    }
    fn query_vec<T, F>(&self, sql: &str, f: F) -> rusqlite::Result<Vec<T>>
    where
        F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        let mut statement = self.prepare(sql)?;
        let rows = statement.query_map([], f)?;
        rows.collect()
    }
}

impl Queryable for RwLockReadGuard<'_, Connection> {
    fn exec<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.execute(sql, params)
    }
    fn query_opt<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<Option<T>>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        self.query_row(sql, params, f).optional()
    }
    fn query_vec<T, F>(&self, sql: &str, f: F) -> rusqlite::Result<Vec<T>>
    where
        F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        let mut statement = self.prepare(sql)?;
        let rows = statement.query_map([], f)?;
        rows.collect()
    }
}

fn ensure_profile_with_connection(
    conn: &impl Queryable,
    id: &str,
    name: &str,
) -> Result<String, String> {
    let now = now_timestamp();
    conn.exec(
        "INSERT INTO profiles (id, name, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at",
        params![id, name, now],
    )
    .map_err(|error| format!("Failed to ensure profile: {error}"))?;
    Ok(id.to_string())
}

fn ensure_playlist_with_connection(
    conn: &impl Queryable,
    profile_id: &str,
    name: &str,
) -> Result<String, String> {
    if let Some(id) = conn
        .query_opt(
            "SELECT id FROM playlists WHERE profile_id = ?1 AND name = ?2",
            params![profile_id, name],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| format!("Failed to find playlist: {error}"))?
    {
        return Ok(id);
    }

    let now = now_timestamp();
    let id = Uuid::new_v4().to_string();
    conn.exec(
        "INSERT INTO playlists (id, profile_id, name, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, profile_id, name, now],
    )
    .map_err(|error| format!("Failed to create playlist: {error}"))?;
    Ok(id)
}

fn lookup_track_id(conn: &impl Queryable, path: &str) -> Result<String, String> {
    conn.query_opt(
        "SELECT id FROM tracks WHERE path = ?1",
        params![path],
        |row| row.get(0),
    )
    .map_err(|error| format!("Failed to look up track id: {error}"))?
    .ok_or_else(|| format!("Track not found in library: {path}"))
}

/// Resolve a track id from a stored or scanned path (exact, then normalized).
fn resolve_track_id_by_path(
    conn: &impl Queryable,
    path: &str,
) -> Result<Option<String>, String> {
    if let Some(id) = conn
        .query_opt(
            "SELECT id FROM tracks WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to look up track by path: {e}"))?
    {
        return Ok(Some(id));
    }

    let canon = normalize_path_key(path);
    if canon != path {
        if let Some(id) = conn
            .query_opt(
                "SELECT id FROM tracks WHERE path = ?1",
                params![canon],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to look up track by canonical path: {e}"))?
        {
            return Ok(Some(id));
        }
    }

    let rows = conn
        .query_vec("SELECT id, path FROM tracks", |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("Failed to scan tracks for path match: {e}"))?;
    for (id, stored) in rows {
        if normalize_path_key(&stored) == canon {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Find an existing library row for this file: path → fingerprint → tags.
fn find_existing_track_id(conn: &impl Queryable, track: &Track) -> Result<Option<String>, String> {
    if let Some(id) = resolve_track_id_by_path(conn, &track.path)? {
        return Ok(Some(id));
    }

    if let Some(ref fp) = track.fingerprint_sha256 {
        if !fp.is_empty() {
            if let Some(id) = conn
                .query_opt(
                    "SELECT id FROM tracks WHERE fingerprint_sha256 = ?1 LIMIT 1",
                    params![fp],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to look up track by fingerprint: {e}"))?
            {
                return Ok(Some(id));
            }
        }
    }

    // Tag match (same heuristic as startup dedupe).
    if !track.title.is_empty() && track.title != "Unknown" {
        if let Some(id) = conn
            .query_opt(
                "SELECT id FROM tracks
                 WHERE lower(artist) = lower(?1)
                   AND lower(album) = lower(?2)
                   AND lower(title) = lower(?3)
                 LIMIT 1",
                params![track.artist, track.album, track.title],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to look up track by tags: {e}"))?
        {
            return Ok(Some(id));
        }
    }

    Ok(None)
}

/// Upsert for sync: reuse fingerprint/tag matches and rewrite `path` to the
/// canonical scanned location so later syncs stop seeing false "new" files.
fn upsert_track_deduped(conn: &impl Queryable, track: &Track) -> Result<String, String> {
    let mut track = track.clone();
    track.path = normalize_path_key(&track.path);

    if let Some(existing_id) = find_existing_track_id(conn, &track)? {
        // Point the existing row at the canonical path (ignore unique conflict
        // if another row already owns that path — then prefer that row).
        let path_owner: Option<String> = conn
            .query_opt(
                "SELECT id FROM tracks WHERE path = ?1",
                params![track.path],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check path owner: {e}"))?;

        let id = match path_owner {
            Some(owner_id) if owner_id != existing_id => owner_id,
            _ => {
                conn.exec(
                    "UPDATE tracks SET
                        path = ?1,
                        name = ?2,
                        title = ?3,
                        artist = ?4,
                        album = ?5,
                        album_artist = ?6,
                        genre = ?7,
                        year = ?8,
                        track_number = ?9,
                        disc_number = ?10,
                        format = ?11,
                        duration_seconds = ?12,
                        sample_rate = ?13,
                        channels = ?14,
                        bit_depth = ?15,
                        fingerprint_sha256 = COALESCE(?16, fingerprint_sha256),
                        file_size = ?17,
                        modified_at = ?18,
                        indexed_at = ?19,
                        is_saf_uri = ?20,
                        album_art_id = COALESCE(?21, album_art_id),
                        cover_art_mime = COALESCE(?22, cover_art_mime),
                        cover_art_source = COALESCE(?23, cover_art_source)
                     WHERE id = ?24",
                    params![
                        track.path,
                        track.name,
                        track.title,
                        track.artist,
                        track.album,
                        track.album_artist,
                        track.genre,
                        track.year,
                        track.track_number,
                        track.disc_number,
                        track.format,
                        track.duration_seconds,
                        track.sample_rate,
                        track.channels,
                        track.bit_depth,
                        track.fingerprint_sha256,
                        track.file_size,
                        track.modified_at,
                        track.indexed_at,
                        track.is_saf_uri as i32,
                        track.album_art_id,
                        track.cover_art_mime,
                        track.cover_art_source,
                        existing_id,
                    ],
                )
                .map_err(|e| format!("Failed to update existing track: {e}"))?;
                existing_id
            }
        };
        track.id = id.clone();
        let _ = sync_track_fts(conn, &track);
        return Ok(id);
    }

    upsert_track(conn, &track)
}

fn upsert_track(conn: &impl Queryable, track: &Track) -> Result<String, String> {
    if let Some(ref art_id) = track.album_art_id {
        let rel = format!("thumbs/{art_id}.jpg");
        let mime = track
            .cover_art_mime
            .clone()
            .unwrap_or_else(|| "image/jpeg".into());
        let _ = conn.exec(
            "INSERT OR IGNORE INTO album_art (id, thumb_path, mime, byte_size, created_at)
             VALUES (?1, ?2, ?3, 0, ?4)",
            params![art_id, rel, mime, now_timestamp()],
        );
    }

    conn.exec(
        "INSERT INTO tracks (
            id, path, name, title, artist, album, album_artist, genre, year, track_number,
            disc_number, format, duration_seconds, sample_rate, channels, bit_depth,
            lyrics, lyrics_source, cover_art_data_url, cover_art_mime, cover_art_source,
            fingerprint_sha256, acoustid_fingerprint, musicbrainz_recording_id, file_size,
            modified_at, indexed_at, is_saf_uri, album_art_id
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, NULL, ?19,
            ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28
        )
        ON CONFLICT(path) DO UPDATE SET
            name = excluded.name,
            title = excluded.title,
            artist = excluded.artist,
            album = excluded.album,
            album_artist = excluded.album_artist,
            genre = excluded.genre,
            year = excluded.year,
            track_number = excluded.track_number,
            disc_number = excluded.disc_number,
            format = excluded.format,
            duration_seconds = excluded.duration_seconds,
            sample_rate = excluded.sample_rate,
            channels = excluded.channels,
            bit_depth = excluded.bit_depth,
            lyrics = excluded.lyrics,
            lyrics_source = excluded.lyrics_source,
            cover_art_data_url = NULL,
            cover_art_mime = COALESCE(excluded.cover_art_mime, tracks.cover_art_mime),
            cover_art_source = COALESCE(excluded.cover_art_source, tracks.cover_art_source),
            fingerprint_sha256 = excluded.fingerprint_sha256,
            acoustid_fingerprint = excluded.acoustid_fingerprint,
            musicbrainz_recording_id = excluded.musicbrainz_recording_id,
            file_size = excluded.file_size,
            modified_at = excluded.modified_at,
            indexed_at = excluded.indexed_at,
            is_saf_uri = excluded.is_saf_uri,
            album_art_id = COALESCE(excluded.album_art_id, tracks.album_art_id)",
        params![
            track.id,
            track.path,
            track.name,
            track.title,
            track.artist,
            track.album,
            track.album_artist,
            track.genre,
            track.year,
            track.track_number,
            track.disc_number,
            track.format,
            track.duration_seconds,
            track.sample_rate,
            track.channels,
            track.bit_depth,
            track.lyrics,
            track.lyrics_source,
            track.cover_art_mime,
            track.cover_art_source,
            track.fingerprint_sha256,
            track.acoustid_fingerprint,
            track.musicbrainz_recording_id,
            track.file_size,
            track.modified_at,
            track.indexed_at,
            track.is_saf_uri as i32,
            track.album_art_id,
        ],
    )
    .map_err(|error| format!("Failed to upsert track: {error}"))?;
    let id = lookup_track_id(conn, &track.path)?;
    let mut synced = track.clone();
    synced.id = id.clone();
    let _ = sync_track_fts(conn, &synced);
    Ok(id)
}

fn ensure_track_column(
    connection: &Connection,
    column_name: &str,
    column_type: &str,
) -> Result<(), String> {
    ensure_table_column(connection, "tracks", column_name, column_type)
}

fn ensure_playlist_column(
    connection: &Connection,
    column_name: &str,
    column_type: &str,
) -> Result<(), String> {
    ensure_table_column(connection, "playlists", column_name, column_type)
}

fn ensure_table_column(
    connection: &Connection,
    table_name: &str,
    column_name: &str,
    column_type: &str,
) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table_name})"))
        .map_err(|error| format!("Failed to inspect {table_name} schema: {error}"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("Failed to inspect {table_name} columns: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to read {table_name} columns: {error}"))?;

    if columns.iter().any(|column| column == column_name) {
        return Ok(());
    }

    connection
        .execute(
            &format!("ALTER TABLE {table_name} ADD COLUMN {column_name} {column_type}"),
            [],
        )
        .map_err(|error| format!("Failed to add {table_name}.{column_name}: {error}"))?;
    Ok(())
}

fn next_playlist_position(conn: &impl Queryable, playlist_id: &str) -> Result<i64, String> {
    conn.query_opt(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM playlist_tracks WHERE playlist_id = ?1",
        params![playlist_id],
        |row| row.get(0),
    )
    .map_err(|error| format!("Failed to calculate playlist position: {error}"))?
    .ok_or_else(|| "Failed to compute next playlist position".to_string())
}

/// Re-numbers all positions in a playlist so they are contiguous starting at 0.
/// Uses a two-phase update so UNIQUE(playlist_id, position) is never violated mid-flight.
fn repair_all_playlist_positions(connection: &Connection) -> Result<(), String> {
    let mut statement = connection
        .prepare("SELECT id FROM playlists")
        .map_err(|error| format!("Failed to prepare playlist repair query: {error}"))?;
    let playlist_ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Failed to query playlists for repair: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to read playlist ids for repair: {error}"))?;

    for playlist_id in playlist_ids {
        let tx = connection
            .unchecked_transaction()
            .map_err(|error| format!("Failed to begin playlist repair transaction: {error}"))?;
        compact_playlist_positions(&tx, &playlist_id)?;
        tx.commit()
            .map_err(|error| format!("Failed to commit playlist repair transaction: {error}"))?;
    }

    Ok(())
}

/// Remove duplicate tracks that share the same artist, album, and title,
/// keeping the earliest indexed copy. Untitled / "Unknown" rows are left alone
/// so distinct untagged files are not collapsed together.
fn deduplicate_tracks(connection: &Connection) -> Result<(), String> {
    let keep_ids: Vec<String> = {
        let mut stmt = connection
            .prepare(
                "SELECT id FROM (
                     SELECT id,
                            ROW_NUMBER() OVER (
                                PARTITION BY lower(artist), lower(album), lower(title)
                                ORDER BY indexed_at ASC, id ASC
                            ) AS rn
                     FROM tracks
                     WHERE trim(title) != '' AND lower(trim(title)) != 'unknown'
                 )
                 WHERE rn = 1
                 UNION ALL
                 SELECT id FROM tracks
                 WHERE trim(title) = '' OR lower(trim(title)) = 'unknown'",
            )
            .map_err(|e| format!("Failed to prepare dedup query: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("Failed to query dedup keepers: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read dedup keepers: {e}"))?;
        rows
    };

    if keep_ids.is_empty() {
        return Ok(());
    }

    let tx = connection
        .unchecked_transaction()
        .map_err(|e| format!("Failed to begin dedup transaction: {e}"))?;

    // Build a temporary table of ids to keep for efficient NOT IN filtering
    tx.execute_batch("CREATE TEMPORARY TABLE IF NOT EXISTS _dedup_keep (id TEXT PRIMARY KEY)")
        .map_err(|e| format!("Failed to create dedup temp table: {e}"))?;
    tx.execute("DELETE FROM _dedup_keep", [])
        .map_err(|e| format!("Failed to clear dedup temp table: {e}"))?;
    for id in &keep_ids {
        tx.execute("INSERT INTO _dedup_keep (id) VALUES (?1)", params![id])
            .map_err(|e| format!("Failed to insert dedup keeper: {e}"))?;
    }

    // Remove orphaned playlist_tracks entries first
    tx.execute(
        "DELETE FROM playlist_tracks
         WHERE track_id NOT IN (SELECT id FROM _dedup_keep)",
        [],
    )
    .map_err(|e| format!("Failed to remove duplicate playlist tracks: {e}"))?;

    // Remove the duplicate tracks themselves
    let removed = tx
        .execute(
            "DELETE FROM tracks
             WHERE id NOT IN (SELECT id FROM _dedup_keep)",
            [],
        )
        .map_err(|e| format!("Failed to remove duplicate tracks: {e}"))?;

    let _ = tx.execute(
        "DELETE FROM tracks_fts
         WHERE track_id NOT IN (SELECT id FROM _dedup_keep)",
        [],
    );

    tx.execute("DROP TABLE IF EXISTS _dedup_keep", [])
        .map_err(|e| format!("Failed to drop dedup temp table: {e}"))?;

    tx.commit()
        .map_err(|e| format!("Failed to commit dedup transaction: {e}"))?;

    if removed > 0 {
        tracing::info!("Removed {removed} duplicate track(s) on startup");
    }

    Ok(())
}

fn compact_playlist_positions(
    tx: &Transaction<'_>,
    playlist_id: &str,
) -> Result<(), String> {
    let mut statement = tx
        .prepare(
            "SELECT track_id FROM playlist_tracks
             WHERE playlist_id = ?1
             ORDER BY position, added_at",
        )
        .map_err(|error| format!("Failed to prepare playlist compaction query: {error}"))?;

    let track_ids = statement
        .query_map(params![playlist_id], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Failed to read playlist tracks for compaction: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to read playlist track ids: {error}"))?;

    for (index, track_id) in track_ids.iter().enumerate() {
        tx.execute(
            "UPDATE playlist_tracks
             SET position = ?1
             WHERE playlist_id = ?2 AND track_id = ?3",
            params![-(index as i64 + 1), playlist_id, track_id],
        )
        .map_err(|error| format!("Failed to stage playlist positions: {error}"))?;
    }

    for (index, track_id) in track_ids.iter().enumerate() {
        tx.execute(
            "UPDATE playlist_tracks
             SET position = ?1
             WHERE playlist_id = ?2 AND track_id = ?3",
            params![index as i64, playlist_id, track_id],
        )
        .map_err(|error| format!("Failed to compact playlist positions: {error}"))?;
    }

    Ok(())
}

/// Stable path key for membership diffs (resolves symlinks when possible).
fn normalize_path_key(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with("content://") {
        return trimmed.to_string();
    }
    Path::new(trimmed)
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| trimmed.to_string())
}

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn ensure_tracks_fts(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
                title,
                artist,
                album,
                name,
                lyrics,
                track_id UNINDEXED,
                tokenize = 'unicode61 remove_diacritics 2'
            );",
        )
        .map_err(|e| format!("Failed to create tracks_fts: {e}"))?;

    let fts_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM tracks_fts", [], |row| row.get(0))
        .unwrap_or(0);
    let track_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
        .unwrap_or(0);

    if fts_count != track_count {
        rebuild_tracks_fts(connection)?;
    }
    Ok(())
}

fn rebuild_tracks_fts(connection: &Connection) -> Result<(), String> {
    connection
        .execute("DELETE FROM tracks_fts", [])
        .map_err(|e| format!("Failed to clear tracks_fts: {e}"))?;
    connection
        .execute(
            "INSERT INTO tracks_fts(title, artist, album, name, lyrics, track_id)
             SELECT title, artist, album, name, COALESCE(lyrics, ''), id FROM tracks",
            [],
        )
        .map_err(|e| format!("Failed to rebuild tracks_fts: {e}"))?;
    Ok(())
}

fn fts_table_ready(connection: &Connection) -> bool {
    connection
        .prepare("SELECT 1 FROM tracks_fts LIMIT 1")
        .is_ok()
}

fn sync_track_fts(conn: &impl Queryable, track: &Track) -> Result<(), String> {
    let _ = conn.exec(
        "DELETE FROM tracks_fts WHERE track_id = ?1",
        params![track.id],
    );
    conn.exec(
        "INSERT INTO tracks_fts(title, artist, album, name, lyrics, track_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            track.title,
            track.artist,
            track.album,
            track.name,
            track.lyrics.clone().unwrap_or_default(),
            track.id,
        ],
    )
    .map_err(|e| format!("Failed to sync tracks_fts: {e}"))?;
    Ok(())
}

/// Build an FTS5 MATCH query: each token becomes a prefix term (`foo*`).
fn build_fts_match_query(raw: &str) -> Option<String> {
    let tokens: Vec<String> = raw
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
        .filter(|t| !t.is_empty())
        .map(|t| {
            let escaped = t.replace('"', "\"\"");
            format!("\"{escaped}\"*")
        })
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    }
}

fn field_contains(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn matched_fields_for(track: &Track, query: &str) -> Vec<String> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = q
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    let check = |value: &str| {
        if tokens.is_empty() {
            field_contains(value, q)
        } else {
            tokens.iter().any(|t| field_contains(value, t))
        }
    };

    let mut fields = Vec::new();
    if check(&track.title) {
        fields.push("title".into());
    }
    if check(&track.artist) {
        fields.push("artist".into());
    }
    if check(&track.album) {
        fields.push("album".into());
    }
    if check(&track.name) && !fields.iter().any(|f| f == "title") {
        fields.push("name".into());
    }
    if track
        .lyrics
        .as_deref()
        .is_some_and(|lyrics| check(lyrics))
    {
        fields.push("lyrics".into());
    }
    if fields.is_empty() {
        // FTS may stem/match differently — still mark title as a soft hit.
        fields.push("title".into());
    }
    fields
}

/// Lower is better: title/name → artist → album → lyrics.
fn match_field_priority(fields: &[String]) -> u8 {
    if fields.iter().any(|f| f == "title" || f == "name") {
        0
    } else if fields.iter().any(|f| f == "artist") {
        1
    } else if fields.iter().any(|f| f == "album") {
        2
    } else if fields.iter().any(|f| f == "lyrics") {
        3
    } else {
        4
    }
}

fn sort_search_hits(hits: &mut [SearchHitDto]) {
    hits.sort_by(|a, b| {
        match_field_priority(&a.matched_fields)
            .cmp(&match_field_priority(&b.matched_fields))
            .then_with(|| {
                a.track
                    .artist
                    .to_lowercase()
                    .cmp(&b.track.artist.to_lowercase())
            })
            .then_with(|| {
                a.track
                    .album
                    .to_lowercase()
                    .cmp(&b.track.album.to_lowercase())
            })
            .then_with(|| {
                a.track
                    .title
                    .to_lowercase()
                    .cmp(&b.track.title.to_lowercase())
            })
    });
}

fn lyrics_snippet(lyrics: &str, query: &str) -> Option<String> {
    let lower = lyrics.to_lowercase();
    let needle = query
        .split_whitespace()
        .find(|t| !t.is_empty())
        .unwrap_or(query)
        .to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let idx = lower.find(&needle)?;
    let start = lyrics[..idx]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or_else(|| idx.saturating_sub(40));
    let end = lyrics[idx..]
        .find('\n')
        .map(|i| idx + i)
        .unwrap_or_else(|| (idx + needle.len() + 60).min(lyrics.len()));
    let mut snip = lyrics[start..end].trim().to_string();
    if start > 0 {
        snip = format!("…{snip}");
    }
    if end < lyrics.len() {
        snip.push('…');
    }
    Some(snip)
}

fn search_tracks_fts(
    connection: &Connection,
    cover_root: &Path,
    query: &str,
    limit: i64,
) -> Result<Vec<SearchHitDto>, String> {
    let match_query = build_fts_match_query(query)
        .ok_or_else(|| "Empty search query".to_string())?;

    // Over-fetch then re-rank so title hits aren't truncated by bm25 alone.
    let fetch_limit = limit.saturating_mul(3).min(600);
    let mut stmt = connection
        .prepare(
            // Column weights: title, artist, album, name, lyrics (higher = more important).
            "SELECT f.track_id,
                    snippet(tracks_fts, 4, '', '', '…', 10) AS lyrics_snip
             FROM tracks_fts f
             WHERE tracks_fts MATCH ?1
             ORDER BY bm25(tracks_fts, 12.0, 6.0, 3.0, 10.0, 0.4)
             LIMIT ?2",
        )
        .map_err(|e| format!("Failed to prepare FTS search: {e}"))?;

    let rows = stmt
        .query_map(params![match_query, fetch_limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .map_err(|e| format!("Failed to execute FTS search: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read FTS hits: {e}"))?;

    let mut hits = Vec::with_capacity(rows.len());
    for (track_id, fts_snip) in rows {
        let track = connection
            .query_row(
                &format!("SELECT {TRACK_DETAIL_COLUMNS} FROM {TRACK_FROM} WHERE t.id = ?1"),
                params![track_id],
                |row| row_to_track(row, cover_root),
            )
            .map_err(|e| format!("Failed to load FTS hit track: {e}"))?;
        let matched_fields = matched_fields_for(&track, query);
        let lyrics_snippet = if matched_fields.iter().any(|f| f == "lyrics") {
            track
                .lyrics
                .as_deref()
                .and_then(|l| lyrics_snippet(l, query))
                .or(fts_snip.filter(|s| !s.trim().is_empty()))
        } else {
            None
        };
        hits.push(SearchHitDto {
            track,
            matched_fields,
            lyrics_snippet,
        });
    }
    sort_search_hits(&mut hits);
    hits.truncate(limit as usize);
    Ok(hits)
}

fn search_tracks_like(
    connection: &Connection,
    cover_root: &Path,
    query: &str,
    limit: i64,
) -> Result<Vec<SearchHitDto>, String> {
    let pattern = format!("%{}%", query.trim());
    let mut stmt = connection
        .prepare(&format!(
            "SELECT {TRACK_DETAIL_COLUMNS} FROM {TRACK_FROM}
             WHERE t.title LIKE ?1 COLLATE NOCASE
                OR t.artist LIKE ?1 COLLATE NOCASE
                OR t.album LIKE ?1 COLLATE NOCASE
                OR t.name LIKE ?1 COLLATE NOCASE
                OR IFNULL(t.lyrics, '') LIKE ?1 COLLATE NOCASE
             ORDER BY
                CASE
                  WHEN t.title LIKE ?1 COLLATE NOCASE
                    OR t.name LIKE ?1 COLLATE NOCASE THEN 0
                  WHEN t.artist LIKE ?1 COLLATE NOCASE THEN 1
                  WHEN t.album LIKE ?1 COLLATE NOCASE THEN 2
                  ELSE 3
                END,
                t.artist, t.album, t.track_number
             LIMIT ?2"
        ))
        .map_err(|e| format!("Failed to prepare LIKE search: {e}"))?;
    let tracks = stmt
        .query_map(params![pattern, limit], |row| row_to_track(row, cover_root))
        .map_err(|e| format!("Failed to execute LIKE search: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read LIKE search results: {e}"))?;

    let mut hits: Vec<SearchHitDto> = tracks
        .into_iter()
        .map(|track| {
            let matched_fields = matched_fields_for(&track, query);
            let lyrics_snippet = if matched_fields.iter().any(|f| f == "lyrics") {
                track
                    .lyrics
                    .as_deref()
                    .and_then(|l| lyrics_snippet(l, query))
            } else {
                None
            };
            SearchHitDto {
                track,
                matched_fields,
                lyrics_snippet,
            }
        })
        .collect();
    sort_search_hits(&mut hits);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn open_test_library() -> Result<Library, String> {
        let connection = Connection::open_in_memory()
            .map_err(|error| format!("Failed to open in-memory database: {error}"))?;
        let cover_root = std::env::temp_dir().join(format!("wave-test-covers-{}", Uuid::new_v4()));
        std::fs::create_dir_all(cover_root.join("thumbs"))
            .map_err(|e| format!("Failed to create test cover dir: {e}"))?;
        let library = Library {
            db_path: PathBuf::from(":memory:"),
            connection: RwLock::new(connection),
            cover_root,
            default_playlist_id_cache: OnceLock::new(),
            favorites_playlist_id_cache: OnceLock::new(),
            app_handle: None,
        };
        library.initialize()?;
        Ok(library)
    }

    fn sample_track(id: &str, path: &str) -> Track {
        Track {
            id: id.to_string(),
            path: path.to_string(),
            name: "song.mp3".to_string(),
            title: "Song".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            album_artist: None,
            genre: None,
            year: None,
            track_number: None,
            disc_number: None,
            format: "MP3".to_string(),
            duration_seconds: Some(180.0),
            sample_rate: Some(44_100),
            channels: Some(2),
            bit_depth: None,
            lyrics: None,
            lyrics_source: None,
            cover_art_data_url: None,
            cover_art_mime: None,
            cover_art_source: None,
            album_art_id: None,
            fingerprint_sha256: None,
            acoustid_fingerprint: None,
            musicbrainz_recording_id: None,
            file_size: 1,
            modified_at: 1,
            indexed_at: 1,
            is_saf_uri: false,
        }
    }

    fn insert_playlist_track_with_connection(
        connection: &Connection,
        playlist_id: &str,
        track_id: &str,
        position: i64,
    ) -> Result<(), String> {
        connection
            .execute(
                "INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![playlist_id, track_id, position, now_timestamp()],
            )
            .map_err(|error| format!("Failed to insert playlist track: {error}"))?;
        Ok(())
    }

    #[test]
    fn upsert_track_preserves_existing_id_for_same_path() {
        let library = open_test_library().expect("library");
        let connection = library.lock_connection().expect("connection");
        let first = sample_track("stable-id", "/music/song.mp3");
        let first_id = upsert_track(&*connection, &first).expect("first upsert");
        assert_eq!(first_id, "stable-id");

        let second = sample_track("new-random-id", "/music/song.mp3");
        let second_id = upsert_track(&*connection, &second).expect("second upsert");
        assert_eq!(second_id, "stable-id");
    }

    #[test]
    fn playlist_track_can_be_removed_and_readded_by_path() {
        let library = open_test_library().expect("library");
        let playlist_id = library.default_playlist_id().expect("playlist");
        let track_path = "/music/replay.mp3";

        let track = sample_track("track-a", track_path);
        {
            let connection = library.lock_connection().expect("connection");
            let track_id = upsert_track(&*connection, &track).expect("upsert");
            insert_playlist_track_with_connection(&connection, &playlist_id, &track_id, 0)
                .expect("insert");
        }

        library
            .remove_track_from_playlist_by_path(&playlist_id, track_path)
            .expect("remove");

        // After removing from Library, the track row is deleted entirely.
        // Re-upserting creates a new track with the new id.
        {
            let connection = library.lock_connection().expect("connection");
            let refreshed = sample_track("track-b", track_path);
            let canonical_id = upsert_track(&*connection, &refreshed).expect("re-upsert");
            insert_playlist_track_with_connection(&connection, &playlist_id, &canonical_id, 0)
                .expect("reinsert");
        }

        let tracks = library
            .get_playlist_tracks(&playlist_id)
            .expect("playlist tracks");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "track-b");
        assert_eq!(tracks[0].path, track_path);
    }

    #[test]
    fn compact_playlist_positions_renumbers_without_unique_conflicts() {
        let library = open_test_library().expect("library");
        let playlist_id = library.default_playlist_id().expect("playlist");

        {
            let mut connection = library.lock_connection().expect("connection");
            let track_a = sample_track("a", "/music/a.mp3");
            let track_b = sample_track("b", "/music/b.mp3");
            let track_c = sample_track("c", "/music/c.mp3");
            let id_a = upsert_track(&*connection, &track_a).expect("upsert a");
            let id_b = upsert_track(&*connection, &track_b).expect("upsert b");
            let id_c = upsert_track(&*connection, &track_c).expect("upsert c");

            insert_playlist_track_with_connection(&connection, &playlist_id, &id_a, 0)
                .expect("insert a");
            insert_playlist_track_with_connection(&connection, &playlist_id, &id_b, 2)
                .expect("insert b");
            insert_playlist_track_with_connection(&connection, &playlist_id, &id_c, 4)
                .expect("insert c");

            let tx = connection.transaction().expect("transaction");
            compact_playlist_positions(&tx, &playlist_id).expect("compact");
            tx.commit().expect("commit");
        }

        let connection = library.lock_connection().expect("connection");
        let positions: Vec<i64> = connection
            .prepare(
                "SELECT position FROM playlist_tracks
                 WHERE playlist_id = ?1
                 ORDER BY position",
            )
            .expect("prepare")
            .query_map(params![playlist_id], |row| row.get(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows");

        assert_eq!(positions, vec![0, 1, 2]);
    }

    #[test]
    fn remove_track_by_path_fails_for_missing_entries() {
        let library = open_test_library().expect("library");
        let playlist_id = library.default_playlist_id().expect("playlist");

        let err = library
            .remove_track_from_playlist_by_path(&playlist_id, "/music/missing.mp3")
            .expect_err("missing track should fail");
        assert!(err.contains("Track not found"));
    }

    #[test]
    fn remove_track_leaves_position_gaps_without_error() {
        let library = open_test_library().expect("library");
        let playlist_id = library.default_playlist_id().expect("playlist");

        {
            let connection = library.lock_connection().expect("connection");
            let track_a = sample_track("a", "/music/a.mp3");
            let track_b = sample_track("b", "/music/b.mp3");
            let id_a = upsert_track(&*connection, &track_a).expect("upsert a");
            let id_b = upsert_track(&*connection, &track_b).expect("upsert b");
            insert_playlist_track_with_connection(&connection, &playlist_id, &id_a, 0)
                .expect("insert a");
            insert_playlist_track_with_connection(&connection, &playlist_id, &id_b, 1)
                .expect("insert b");
        }

        library
            .remove_track_from_playlist_by_path(&playlist_id, "/music/a.mp3")
            .expect("remove first track");

        let tracks = library
            .get_playlist_tracks(&playlist_id)
            .expect("playlist tracks");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].path, "/music/b.mp3");
    }

    // ── Album / artist browsing & querying ──────────────────────────────────

    /// Build a `Track` with customizable album/artist metadata for browse tests.
    fn track_with(
        id: &str,
        path: &str,
        artist: &str,
        album: &str,
        album_artist: Option<&str>,
        track_number: Option<i32>,
        disc_number: Option<i32>,
        year: Option<i32>,
        cover_art_id: Option<&str>,
    ) -> Track {
        let mut t = sample_track(id, path);
        t.artist = artist.to_string();
        t.album = album.to_string();
        t.album_artist = album_artist.map(String::from);
        t.track_number = track_number;
        t.disc_number = disc_number;
        t.year = year;
        if let Some(art_id) = cover_art_id {
            t.album_art_id = Some(art_id.to_string());
            t.cover_art_mime = Some("image/jpeg".to_string());
            t.cover_art_source = Some("test".to_string());
        }
        t
    }

    fn upsert_many(library: &Library, tracks: &[Track]) {
        let connection = library.lock_connection().expect("connection");
        for track in tracks {
            if let Some(ref art_id) = track.album_art_id {
                let thumb = library.cover_root.join("thumbs").join(format!("{art_id}.jpg"));
                if !thumb.exists() {
                    std::fs::write(&thumb, b"fake").expect("thumb");
                }
            }
            upsert_track(&*connection, track).expect("upsert");
        }
    }

    fn seed_library_for_browse_tests(library: &Library) {
        upsert_many(
            library,
            &[
                // "Abbey Road" by The Beatles (album_artist set, 3 tracks).
                track_with("b1", "/m/abbey-1.flac", "The Beatles", "Abbey Road", Some("The Beatles"), Some(1), Some(1), Some(1969), Some("art_abbey")),
                track_with("b2", "/m/abbey-2.flac", "The Beatles", "Abbey Road", Some("The Beatles"), Some(2), Some(1), Some(1969), Some("art_abbey")),
                track_with("b3", "/m/abbey-3.flac", "The Beatles", "Abbey Road", Some("The Beatles"), Some(3), Some(1), Some(1969), Some("art_abbey")),
                // A second Beatles album so album_count > 1.
                track_with("b4", "/m/letitbe-1.flac", "The Beatles", "Let It Be", Some("The Beatles"), Some(1), Some(1), Some(1970), Some("art_letitbe")),
                // "Greatest Hits" collision: Queen (2 tracks) vs ABBA (1 track, no cover).
                track_with("q1", "/m/queen-1.flac", "Queen", "Greatest Hits", Some("Queen"), Some(1), Some(1), Some(1981), Some("art_queen")),
                track_with("q2", "/m/queen-2.flac", "Queen", "Greatest Hits", Some("Queen"), Some(2), Some(1), Some(1981), Some("art_queen")),
                track_with("a1", "/m/abba-1.flac", "ABBA", "Greatest Hits", Some("ABBA"), Some(1), Some(1), Some(1975), None),
                // Album with no album_artist tag → resolved album_artist falls back to artist.
                track_with("s1", "/m/solo-1.flac", "Solo", "No Album Artist", None, Some(1), Some(1), Some(2020), None),
            ],
        );
    }

    #[test]
    fn list_albums_groups_by_album_and_resolved_album_artist() {
        let library = open_test_library().expect("library");
        seed_library_for_browse_tests(&library);

        let albums = library.list_albums().expect("albums");

        // 5 distinct (album, album_artist) groups — "Greatest Hits" appears twice
        // and The Beatles have two separate albums.
        assert_eq!(albums.len(), 5);

        // Ordered by album_artist then album.
        let names: Vec<(&str, &str, i64)> = albums
            .iter()
            .map(|a| (a.name.as_str(), a.album_artist.as_deref().unwrap_or(""), a.track_count))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Greatest Hits", "ABBA", 1),
                ("Greatest Hits", "Queen", 2),
                ("No Album Artist", "Solo", 1),
                ("Abbey Road", "The Beatles", 3),
                ("Let It Be", "The Beatles", 1),
            ]
        );

        let abbey = albums.iter().find(|a| a.name == "Abbey Road").unwrap();
        assert_eq!(abbey.artist, "The Beatles");
        assert_eq!(abbey.year, Some(1969));
        assert!(abbey.cover_art_data_url.as_ref().is_some_and(|p| p.ends_with("art_abbey.jpg")));
        assert_eq!(abbey.cover_art_mime.as_deref(), Some("image/jpeg"));

        let abba = albums.iter().find(|a| a.name == "Greatest Hits" && a.album_artist.as_deref() == Some("ABBA")).unwrap();
        assert_eq!(abba.year, Some(1975));
        assert!(abba.cover_art_data_url.is_none());

        // An album with a NULL album_artist tag resolves to the track artist.
        let solo = albums.iter().find(|a| a.name == "No Album Artist").unwrap();
        assert_eq!(solo.album_artist.as_deref(), Some("Solo"));
    }

    #[test]
    fn list_artists_aggregates_track_and_album_counts() {
        let library = open_test_library().expect("library");
        seed_library_for_browse_tests(&library);

        let artists = library.list_artists().expect("artists");

        // Ordered by artist name.
        let by_name: Vec<(&str, i64, i64)> = artists
            .iter()
            .map(|a| (a.name.as_str(), a.track_count, a.album_count))
            .collect();
        assert_eq!(
            by_name,
            vec![
                ("ABBA", 1, 1),
                ("Queen", 2, 1),
                ("Solo", 1, 1),
                ("The Beatles", 4, 2), // 4 tracks across 2 albums
            ]
        );
    }

    #[test]
    fn get_tracks_by_album_disambiguates_same_named_albums() {
        let library = open_test_library().expect("library");
        seed_library_for_browse_tests(&library);

        // Precise match using album_artist keeps the two "Greatest Hits" apart.
        let queen = library
            .get_tracks_by_album("Greatest Hits", Some("Queen"))
            .expect("queen");
        assert_eq!(queen.len(), 2);
        assert!(queen.iter().all(|t| t.artist == "Queen"));
        // Ordered by track_number.
        assert_eq!(queen[0].track_number, Some(1));
        assert_eq!(queen[1].track_number, Some(2));

        let abba = library
            .get_tracks_by_album("Greatest Hits", Some("ABBA"))
            .expect("abba");
        assert_eq!(abba.len(), 1);

        // Without album_artist, both merge into one result set.
        let merged = library
            .get_tracks_by_album("Greatest Hits", None)
            .expect("merged");
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn get_tracks_by_album_matches_resolved_album_artist_when_tag_is_null() {
        let library = open_test_library().expect("library");
        seed_library_for_browse_tests(&library);

        // "No Album Artist" has a NULL album_artist tag; resolved value is "Solo".
        let tracks = library
            .get_tracks_by_album("No Album Artist", Some("Solo"))
            .expect("tracks");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].artist, "Solo");
    }

    #[test]
    fn get_tracks_by_album_orders_by_disc_then_track() {
        let library = open_test_library().expect("library");
        upsert_many(
            &library,
            &[
                track_with("d1", "/m/d1.flac", "X", "Double", Some("X"), Some(1), Some(2), None, None),
                track_with("d2", "/m/d2.flac", "X", "Double", Some("X"), Some(2), Some(1), None, None),
                track_with("d3", "/m/d3.flac", "X", "Double", Some("X"), Some(1), Some(1), None, None),
            ],
        );

        let tracks = library
            .get_tracks_by_album("Double", Some("X"))
            .expect("tracks");
        let order: Vec<(Option<i32>, Option<i32>)> = tracks
            .iter()
            .map(|t| (t.disc_number, t.track_number))
            .collect();
        assert_eq!(
            order,
            vec![(Some(1), Some(1)), (Some(1), Some(2)), (Some(2), Some(1))]
        );
    }

    #[test]
    fn get_tracks_by_artist_returns_discography_ordered_by_album_disc_track() {
        let library = open_test_library().expect("library");
        seed_library_for_browse_tests(&library);

        let tracks = library.get_tracks_by_artist("The Beatles").expect("tracks");
        assert_eq!(tracks.len(), 4);
        // Ordered by album name: "Abbey Road" before "Let It Be".
        assert_eq!(tracks[0].album, "Abbey Road");
        assert_eq!(tracks[3].album, "Let It Be");
        // Within Abbey Road: disc 1, tracks 1..3.
        assert_eq!(tracks[0].track_number, Some(1));
        assert_eq!(tracks[1].track_number, Some(2));
        assert_eq!(tracks[2].track_number, Some(3));
    }

    #[test]
    fn get_tracks_by_album_rejects_empty_name() {
        let library = open_test_library().expect("library");
        let err = library
            .get_tracks_by_album("   ", None)
            .expect_err("empty album should fail");
        assert!(err.contains("cannot be empty"));
    }

    #[test]
    fn get_tracks_by_artist_rejects_empty_name() {
        let library = open_test_library().expect("library");
        let err = library
            .get_tracks_by_artist("")
            .expect_err("empty artist should fail");
        assert!(err.contains("cannot be empty"));
    }

    // ── Favorites ───────────────────────────────────────────────────────────

    #[test]
    fn favorites_playlist_is_seeded_and_listed() {
        let library = open_test_library().expect("library");
        let playlists = library.list_playlists(None).expect("playlists");
        let names: Vec<&str> = playlists.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Favorites"));
        assert!(names.contains(&"Library"));
    }

    #[test]
    fn favorites_id_is_stable_across_calls() {
        let library = open_test_library().expect("library");
        let a = library.favorites_playlist_id().expect("id a");
        let b = library.favorites_playlist_id().expect("id b");
        assert_eq!(a, b);
    }

    #[test]
    fn is_track_in_favorites_reflects_membership() {
        let library = open_test_library().expect("library");
        let favorites_id = library.favorites_playlist_id().expect("favorites");
        let track_path = "/music/fav.mp3";

        assert!(!library.is_track_in_favorites(track_path).expect("absent"));

        {
            let connection = library.lock_connection().expect("connection");
            let track = sample_track("fav-1", track_path);
            let id = upsert_track(&*connection, &track).expect("upsert");
            insert_playlist_track_with_connection(&connection, &favorites_id, &id, 0)
                .expect("insert");
        }

        assert!(library.is_track_in_favorites(track_path).expect("present"));

        library
            .remove_track_from_favorites(track_path)
            .expect("remove");

        assert!(!library.is_track_in_favorites(track_path).expect("removed"));
    }

    #[test]
    fn is_track_in_any_playlist_requires_registered_membership() {
        let library = open_test_library().expect("library");
        let playlist_id = library.default_playlist_id().expect("playlist");
        let track_path = "/music/registered.mp3";

        assert!(!library
            .is_track_in_any_playlist(track_path)
            .expect("absent before insert"));

        {
            let connection = library.lock_connection().expect("connection");
            let track = sample_track("reg-1", track_path);
            let id = upsert_track(&*connection, &track).expect("upsert");
            insert_playlist_track_with_connection(&connection, &playlist_id, &id, 0)
                .expect("insert");
        }

        assert!(library
            .is_track_in_any_playlist(track_path)
            .expect("present after insert"));

        library
            .remove_track_from_playlist_by_path(&playlist_id, track_path)
            .expect("remove");

        assert!(!library
            .is_track_in_any_playlist(track_path)
            .expect("absent after remove"));
    }

    #[test]
    fn toggle_favorite_removes_when_present() {
        let library = open_test_library().expect("library");
        let favorites_id = library.favorites_playlist_id().expect("favorites");
        let track_path = "/music/toggle.mp3";

        {
            let connection = library.lock_connection().expect("connection");
            let track = sample_track("tog-1", track_path);
            let id = upsert_track(&*connection, &track).expect("upsert");
            insert_playlist_track_with_connection(&connection, &favorites_id, &id, 0)
                .expect("insert");
        }

        let now_favorited = library.toggle_favorite(track_path).expect("toggle off");
        assert!(!now_favorited);
        assert!(library.get_favorites().expect("favorites").is_empty());
    }

    #[test]
    fn get_favorites_returns_tracks_in_position_order() {
        let library = open_test_library().expect("library");
        let favorites_id = library.favorites_playlist_id().expect("favorites");

        {
            let connection = library.lock_connection().expect("connection");
            let t1 = sample_track("f1", "/music/f1.mp3");
            let t2 = sample_track("f2", "/music/f2.mp3");
            let id1 = upsert_track(&*connection, &t1).expect("upsert 1");
            let id2 = upsert_track(&*connection, &t2).expect("upsert 2");
            insert_playlist_track_with_connection(&connection, &favorites_id, &id1, 0)
                .expect("insert 1");
            insert_playlist_track_with_connection(&connection, &favorites_id, &id2, 1)
                .expect("insert 2");
        }

        let favorites = library.get_favorites().expect("favorites");
        assert_eq!(favorites.len(), 2);
        assert_eq!(favorites[0].path, "/music/f1.mp3");
        assert_eq!(favorites[1].path, "/music/f2.mp3");
    }

    #[test]
    fn clear_favorites_removes_all() {
        let library = open_test_library().expect("library");
        let favorites_id = library.favorites_playlist_id().expect("favorites");

        {
            let connection = library.lock_connection().expect("connection");
            let t = sample_track("cf-1", "/music/cf.mp3");
            let id = upsert_track(&*connection, &t).expect("upsert");
            insert_playlist_track_with_connection(&connection, &favorites_id, &id, 0)
                .expect("insert");
        }

        assert_eq!(library.get_favorites().expect("favorites").len(), 1);
        library.clear_favorites().expect("clear");
        assert!(library.get_favorites().expect("favorites").is_empty());
    }

    #[test]
    fn delete_playlist_rejects_favorites() {
        let library = open_test_library().expect("library");
        let favorites_id = library.favorites_playlist_id().expect("favorites");
        let err = library
            .delete_playlist(&favorites_id)
            .expect_err("should not delete favorites");
        assert!(err.contains("cannot be deleted"));
    }

    #[test]
    fn rename_playlist_rejects_favorites() {
        let library = open_test_library().expect("library");
        let favorites_id = library.favorites_playlist_id().expect("favorites");
        let err = library
            .rename_playlist(&favorites_id, "My Songs")
            .expect_err("should not rename favorites");
        assert!(err.contains("cannot be renamed"));
    }

    #[test]
    fn add_track_to_playlist_reuses_existing_track_without_extraction() {
        let library = open_test_library().expect("library");
        let favorites_id = library.favorites_playlist_id().expect("favorites");
        let default_id = library.default_playlist_id().expect("default");
        let track_path = "/music/already-indexed.mp3";

        let original_track = sample_track("seeded-id", track_path);
        {
            let connection = library.lock_connection().expect("connection");
            let id = upsert_track(&*connection, &original_track).expect("upsert");
            assert_eq!(id, "seeded-id");
            // Add it to the default playlist first.
            insert_playlist_track_with_connection(&connection, &default_id, &id, 0)
                .expect("insert to default");
        }

        let added = library
            .add_track_to_playlist(&favorites_id, track_path.to_string())
            .expect("add existing track to favorites");

        assert_eq!(added.id, "seeded-id");
        assert_eq!(added.title, original_track.title);
        assert_eq!(added.artist, original_track.artist);

        let favorites = library.get_favorites().expect("favorites");
        assert_eq!(favorites.len(), 1);
        assert_eq!(favorites[0].path, track_path);
    }

    #[test]
    fn apply_playlist_sync_reuses_fingerprint_instead_of_duplicating() {
        let library = open_test_library().expect("library");
        let favorites_id = library.favorites_playlist_id().expect("favorites");

        let mut first = sample_track("track-a", "/music/album/song.mp3");
        first.fingerprint_sha256 = Some("fp-same-file".to_string());
        first.title = "Real Title".to_string();

        library
            .apply_playlist_sync(&favorites_id, &[], &[first], &[])
            .expect("first sync");

        let mut second = sample_track("track-b", "/other/mount/song.mp3");
        second.fingerprint_sha256 = Some("fp-same-file".to_string());
        second.title = "Real Title".to_string();
        second.artist = "Artist".to_string();
        second.album = "Album".to_string();

        // Same file, new path string — must update the existing row, not insert.
        library
            .apply_playlist_sync(
                &favorites_id,
                &["/music/album/song.mp3".to_string()],
                &[second],
                &[],
            )
            .expect("second sync");

        let favorites = library.get_playlist_tracks(&favorites_id).expect("tracks");
        assert_eq!(favorites.len(), 1, "playlist must not grow on path-variant sync");
        assert_eq!(favorites[0].id, "track-a");
        assert_eq!(favorites[0].path, "/other/mount/song.mp3");

        let connection = library.lock_connection().expect("connection");
        let track_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
            .expect("count");
        assert_eq!(track_count, 1, "library must not keep a leftover duplicate row");
    }

    #[test]
    fn sync_playlist_to_paths_is_idempotent() {
        let library = open_test_library().expect("library");
        let favorites_id = library.favorites_playlist_id().expect("favorites");

        let track = sample_track("stable-id", "/library/track.flac");
        library
            .apply_playlist_sync(&favorites_id, &[], &[track], &[])
            .expect("seed");

        let desired = vec!["/library/track.flac".to_string()];
        let (added, removed) = library
            .sync_playlist_to_paths(&favorites_id, &desired)
            .expect("first reconcile");
        assert_eq!((added, removed), (0, 0));

        let (added2, removed2) = library
            .sync_playlist_to_paths(&favorites_id, &desired)
            .expect("second reconcile");
        assert_eq!((added2, removed2), (0, 0));

        let favorites = library.get_playlist_tracks(&favorites_id).expect("tracks");
        assert_eq!(favorites.len(), 1);
    }

    #[test]
    fn clear_playlist_rejects_synced_folder() {
        let library = open_test_library().expect("library");
        let info = library
            .create_playlist("Synced Mix", Some("/music/synced"))
            .expect("create");
        let err = library
            .clear_playlist(&info.id)
            .expect_err("synced clear should fail");
        assert!(err.to_lowercase().contains("synced"));
    }

    #[test]
    fn remove_from_user_playlist_keeps_library_track() {
        let library = open_test_library().expect("library");
        let playlist = library
            .create_playlist("Workout", None)
            .expect("create");
        let path = "/music/only-here.mp3";

        {
            let connection = library.lock_connection().expect("connection");
            let id = upsert_track(&*connection, &sample_track("t1", path)).expect("upsert");
            insert_playlist_track_with_connection(&connection, &playlist.id, &id, 0)
                .expect("playlist");
        }

        library
            .remove_track_from_playlist_by_path(&playlist.id, path)
            .expect("remove");

        let default_id = library.default_playlist_id().expect("default");
        assert_eq!(
            library
                .get_playlist_tracks(&default_id)
                .expect("library")
                .len(),
            1,
            "playlist remove must not delete the library row"
        );
        assert!(library.get_playlist_tracks(&playlist.id).expect("pl").is_empty());
    }

    #[test]
    fn remove_from_library_deletes_everywhere() {
        let library = open_test_library().expect("library");
        let playlist = library.create_playlist("Mix", None).expect("create");
        let favorites_id = library.favorites_playlist_id().expect("favorites");
        let path = "/music/gone.mp3";

        {
            let connection = library.lock_connection().expect("connection");
            let id = upsert_track(&*connection, &sample_track("gone", path)).expect("upsert");
            insert_playlist_track_with_connection(&connection, &playlist.id, &id, 0)
                .expect("playlist");
            insert_playlist_track_with_connection(&connection, &favorites_id, &id, 0)
                .expect("favorite");
        }

        library
            .remove_track_from_library(path)
            .expect("remove from library");

        let default_id = library.default_playlist_id().expect("default");
        assert!(library.get_playlist_tracks(&default_id).expect("lib").is_empty());
        assert!(library.get_playlist_tracks(&playlist.id).expect("pl").is_empty());
        assert!(library.get_favorites().expect("fav").is_empty());
    }

    #[test]
    fn remove_from_user_playlist_keeps_track_if_in_another_playlist() {
        let library = open_test_library().expect("library");
        let a = library.create_playlist("A", None).expect("a");
        let b = library.create_playlist("B", None).expect("b");
        let path = "/music/shared.mp3";

        {
            let connection = library.lock_connection().expect("connection");
            let id = upsert_track(&*connection, &sample_track("t-shared", path)).expect("upsert");
            insert_playlist_track_with_connection(&connection, &a.id, &id, 0).expect("a");
            insert_playlist_track_with_connection(&connection, &b.id, &id, 0).expect("b");
        }

        library
            .remove_track_from_playlist_by_path(&a.id, path)
            .expect("remove from a");

        let default_id = library.default_playlist_id().expect("default");
        let all = library.get_playlist_tracks(&default_id).expect("library");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].path, path);
        assert_eq!(library.get_playlist_tracks(&b.id).expect("b").len(), 1);
    }

    #[test]
    fn remove_from_favorites_does_not_purge_library_track() {
        let library = open_test_library().expect("library");
        let favorites_id = library.favorites_playlist_id().expect("favorites");
        let path = "/music/keep-me.mp3";

        {
            let connection = library.lock_connection().expect("connection");
            let id = upsert_track(&*connection, &sample_track("keep", path)).expect("upsert");
            insert_playlist_track_with_connection(&connection, &favorites_id, &id, 0)
                .expect("favorite");
        }

        library
            .remove_track_from_favorites(path)
            .expect("unfavorite");

        let default_id = library.default_playlist_id().expect("default");
        assert_eq!(
            library.get_playlist_tracks(&default_id).expect("all").len(),
            1,
            "unfavoriting must not delete the library row"
        );
    }

    #[test]
    fn clear_user_playlist_keeps_library_tracks() {
        let library = open_test_library().expect("library");
        let playlist = library.create_playlist("Temp", None).expect("create");
        let path = "/music/temp-only.mp3";

        {
            let connection = library.lock_connection().expect("connection");
            let id = upsert_track(&*connection, &sample_track("temp", path)).expect("upsert");
            insert_playlist_track_with_connection(&connection, &playlist.id, &id, 0)
                .expect("insert");
        }

        library.clear_playlist(&playlist.id).expect("clear");

        let default_id = library.default_playlist_id().expect("default");
        assert_eq!(
            library.get_playlist_tracks(&default_id).expect("all").len(),
            1,
            "clearing a playlist must not wipe the library"
        );
    }

    #[test]
    fn reset_library_wipes_tracks_and_user_playlists() {
        let library = open_test_library().expect("library");
        let mix = library.create_playlist("Mix", Some("/music/mix")).expect("mix");
        let favorites_id = library.favorites_playlist_id().expect("favorites");
        let library_id = library.default_playlist_id().expect("library");
        let path = "/music/wipe-me.mp3";

        {
            let connection = library.lock_connection().expect("connection");
            let id = upsert_track(&*connection, &sample_track("wipe", path)).expect("upsert");
            insert_playlist_track_with_connection(&connection, &mix.id, &id, 0).expect("mix");
            insert_playlist_track_with_connection(&connection, &favorites_id, &id, 0)
                .expect("fav");
        }

        let (tracks, playlists) = library.reset_library().expect("reset");
        assert_eq!(tracks, 1);
        assert_eq!(playlists, 1);

        let remaining = library.list_playlists(None).expect("list");
        let names: Vec<_> = remaining.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Library"));
        assert!(names.contains(&"Favorites"));
        assert!(!names.contains(&"Mix"));
        assert!(library.get_playlist_tracks(&library_id).expect("lib").is_empty());
        assert!(library.get_favorites().expect("fav").is_empty());

        let sync: Option<String> = {
            let connection = library.lock_connection().expect("connection");
            connection
                .query_row(
                    "SELECT sync_folder FROM playlists WHERE id = ?1",
                    params![library_id],
                    |row| row.get(0),
                )
                .expect("sync")
        };
        assert!(sync.is_none());
    }

    #[test]
    fn search_tracks_rich_matches_title_artist_album_and_lyrics() {
        let library = open_test_library().expect("library");
        {
            let connection = library.lock_connection().expect("connection");
            let mut rainy = sample_track("s1", "/m/rainy.flac");
            rainy.title = "Rainy Day".into();
            rainy.artist = "Storm Band".into();
            rainy.album = "Cloud Nine".into();
            rainy.lyrics = Some("walking through the thunder and rain tonight".into());
            upsert_track(&*connection, &rainy).expect("upsert rainy");

            let mut sunny = sample_track("s2", "/m/sunny.flac");
            sunny.title = "Sunny Side".into();
            sunny.artist = "Blue Sky".into();
            sunny.album = "Clear Skies".into();
            sunny.lyrics = Some("nothing but sunshine".into());
            upsert_track(&*connection, &sunny).expect("upsert sunny");
        }

        let by_title = library.search_tracks_rich("Rainy", Some(20)).expect("title");
        assert_eq!(by_title.len(), 1);
        assert_eq!(by_title[0].track.title, "Rainy Day");
        assert!(by_title[0].matched_fields.iter().any(|f| f == "title"));

        let by_artist = library.search_tracks_rich("Storm", Some(20)).expect("artist");
        assert_eq!(by_artist.len(), 1);
        assert!(by_artist[0].matched_fields.iter().any(|f| f == "artist"));

        let by_album = library.search_tracks_rich("Cloud", Some(20)).expect("album");
        assert!(by_album.iter().any(|h| h.track.album == "Cloud Nine"));

        let by_lyrics = library
            .search_tracks_rich("thunder", Some(20))
            .expect("lyrics");
        assert_eq!(by_lyrics.len(), 1);
        assert!(by_lyrics[0].matched_fields.iter().any(|f| f == "lyrics"));
        assert!(by_lyrics[0]
            .lyrics_snippet
            .as_deref()
            .is_some_and(|s| s.to_lowercase().contains("thunder")));
    }

    #[test]
    fn search_tracks_orders_by_title_artist_album_lyrics() {
        let library = open_test_library().expect("library");
        {
            let connection = library.lock_connection().expect("connection");

            let mut lyrics_hit = sample_track("o1", "/m/lyrics.flac");
            lyrics_hit.title = "Something Else".into();
            lyrics_hit.artist = "Zulu".into();
            lyrics_hit.album = "Zed".into();
            lyrics_hit.lyrics = Some("echo echo echo in the night".into());
            upsert_track(&*connection, &lyrics_hit).expect("lyrics");

            let mut album_hit = sample_track("o2", "/m/album.flac");
            album_hit.title = "Other Song".into();
            album_hit.artist = "Yankee".into();
            album_hit.album = "Echo Chamber".into();
            album_hit.lyrics = None;
            upsert_track(&*connection, &album_hit).expect("album");

            let mut artist_hit = sample_track("o3", "/m/artist.flac");
            artist_hit.title = "Different".into();
            artist_hit.artist = "Echo Park".into();
            artist_hit.album = "West".into();
            artist_hit.lyrics = None;
            upsert_track(&*connection, &artist_hit).expect("artist");

            let mut title_hit = sample_track("o4", "/m/title.flac");
            title_hit.title = "Echo".into();
            title_hit.artist = "Alpha".into();
            title_hit.album = "Beta".into();
            title_hit.lyrics = None;
            upsert_track(&*connection, &title_hit).expect("title");
        }

        let hits = library.search_tracks_rich("echo", Some(20)).expect("search");
        assert!(hits.len() >= 4, "expected all four field matches, got {hits:?}");
        assert_eq!(hits[0].track.title, "Echo");
        assert!(hits[0].matched_fields.iter().any(|f| f == "title"));
        assert_eq!(hits[1].track.artist, "Echo Park");
        assert!(hits[1].matched_fields.iter().any(|f| f == "artist"));
        assert_eq!(hits[2].track.album, "Echo Chamber");
        assert!(hits[2].matched_fields.iter().any(|f| f == "album"));
        assert!(hits[3].matched_fields.iter().any(|f| f == "lyrics"));
        assert_eq!(hits[3].track.title, "Something Else");
    }
}
