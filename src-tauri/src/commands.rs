// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app::updater::UpdaterState;
use crate::app_settings::{AppSettings, AppSettingsState, SavedQueueSource, SongCouple};
use crate::audio::player::{AudioPlayer, QueueSource};
use crate::audio::sleep_timer::{SleepRequest, SleepTimerStatus};
use crate::dto::{
    AlbumSummaryDto, ArtistSummaryDto, CloseAction, EqSettingsDto, HomeSuggestionsDto,
    ImportResultDto, ListeningStatsDto, LyricsImportResultDto, PlaybackModeDto, PlaybackStateDto,
    QueueDto, QueueStateDto, SearchCollectionsDto, SearchHitDto, SongCoupleDto, SongCouplingDto,
    ThemeMode, UpdateCheckDto,
};
use crate::library::{Library, PlaylistInfo};
use crate::listen::{ListenEndReason, ListenFlush, ListenTracker};
use crate::media_controls::TrackMetadata;
use crate::metadata::{enrich_lyrics_online, supported_audio_extensions, Track};
use crate::path_validation::{
    is_android_content_uri, validate_audio_path, validate_playlist_import_path,
    validate_safe_output_path,
};
use crate::tag_edit::{Change, ResolvedEdit, TagEdit};
use tauri::{Emitter, Manager};

/// Lazily-initialized audio engine. Creation is deferred until first use so
/// Android can finish wiring JNI / ndk_context before cpal/oboe opens a stream.
pub struct PlayerState(pub Mutex<Option<AudioPlayer>>);
pub struct LibraryState(pub Mutex<Library>);
pub struct MediaBridgeState(pub crate::media_controls::MediaBridgeState);
pub struct ListenState(pub Mutex<ListenTracker>);
/// Guards against overlapping artist-enrichment background jobs (e.g. from
/// rapid Home page refreshes). Only one runs at a time.
pub struct EnrichmentState(pub std::sync::atomic::AtomicBool);
pub struct WaveformState(pub crate::waveform_service::WaveformService);

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns true if the lyrics text contains LRC-style timestamps like [01:23.45].
fn has_lrc_timestamps(lyrics: &str) -> bool {
    lyrics
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .any(|line| {
            let trimmed = line.trim();
            trimmed.starts_with('[')
                && trimmed.len() > 5
                && trimmed.as_bytes()[1].is_ascii_digit()
                && trimmed.as_bytes()[2] == b':'
        })
}

fn lock_poisoned<T>(e: std::sync::PoisonError<T>) -> String {
    tracing::warn!("Mutex was poisoned, recovering: {e}");
    "State lock poisoned".to_string()
}

fn lock_player_state<'a>(
    state: &'a tauri::State<'a, PlayerState>,
) -> std::sync::MutexGuard<'a, Option<AudioPlayer>> {
    match state.0.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            tracing::warn!("Player mutex was poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

fn create_audio_player() -> Result<AudioPlayer, String> {
    // Never open the OS audio device during construction. On Android, cpal/oboe
    // can abort the process via JNI; queue/EQ/UI must stay usable without a stream.
    Ok(AudioPlayer::new_deferred())
}

pub(crate) fn ensure_player(slot: &mut Option<AudioPlayer>) -> Result<&mut AudioPlayer, String> {
    if slot.is_none() {
        *slot = Some(create_audio_player()?);
    }
    Ok(slot.as_mut().expect("player just initialized"))
}

pub struct PlayerGuard<'a>(MutexGuard<'a, Option<AudioPlayer>>);

impl Deref for PlayerGuard<'_> {
    type Target = AudioPlayer;

    fn deref(&self) -> &Self::Target {
        self.0
            .as_ref()
            .expect("player must be initialized before deref")
    }
}

impl DerefMut for PlayerGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
            .as_mut()
            .expect("player must be initialized before deref_mut")
    }
}

fn lock_player<'a>(state: &'a tauri::State<'a, PlayerState>) -> Result<PlayerGuard<'a>, String> {
    // Recover from poisoned mutex: a previous panic may have left the lock in a
    // poisoned state, but the player data is still usable.
    let mut guard = match state.0.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            tracing::warn!("Player mutex was poisoned, recovering: {poisoned}");
            poisoned.into_inner()
        }
    };
    ensure_player(&mut guard)?;
    Ok(PlayerGuard(guard))
}

fn with_app_player<R>(
    app: &tauri::AppHandle,
    f: impl FnOnce(&mut AudioPlayer) -> Result<R, String>,
) -> Result<R, String> {
    let state = app.state::<PlayerState>();
    let mut slot = match state.0.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            tracing::warn!("Player mutex was poisoned, recovering: {poisoned}");
            poisoned.into_inner()
        }
    };
    let player = ensure_player(&mut slot)?;
    f(player)
}

const PLAYBACK_PERSIST_INTERVAL_MS: i64 = 3000;
static LAST_PLAYBACK_PERSIST_MS: AtomicI64 = AtomicI64::new(0);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn path_is_restorable(path: &str) -> bool {
    if path.starts_with("content://") {
        return true;
    }
    // Restoring happens while the app starts, under the player lock: a remote
    // track comes back only when its whole file is cached, so nothing waits
    // on the network. It stays in the queue either way.
    if crate::remote::uri::is_remote(path) {
        return crate::remote::stream::cache_file_if_complete(path).is_some();
    }
    Path::new(path).exists()
}

/// Write the current queue, track, and scrubber position to disk.
pub(crate) fn persist_playback_state(app: &tauri::AppHandle) {
    let player_state = match app.try_state::<PlayerState>() {
        Some(s) => s,
        None => return,
    };
    let settings_state = match app.try_state::<AppSettingsState>() {
        Some(s) => s,
        None => return,
    };
    let player = match player_state.0.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(player) = player.as_ref() else {
        return;
    };
    let mut settings = match settings_state.0.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    settings.last_track_path = player
        .get_current_path()
        .map(|p| p.to_string_lossy().into_owned());
    settings.last_position_seconds = player.position_seconds();
    settings.last_queue = player.queue.tracks().to_vec();
    settings.last_queue_index = player.queue.current_index();
    settings.last_queue_mashup = player.queue.is_mashup();
    settings.last_queue_source = player.queue.source().map(|source| SavedQueueSource {
        playlist_id: source.playlist_id().to_string(),
        in_playlist_order: source.in_playlist_order(),
    });
    settings.shuffle = player.queue.is_shuffled();
    settings.repeat = player.repeat.clone();
    let _ = settings.save(app);
}

fn apply_listen_flush(app: &tauri::AppHandle, flush: ListenFlush) {
    // Previewing 30 seconds of a track is not listening to it; letting it reach
    // listen_stats would skew top artists, recently played, and Home suggestions.
    if is_preview_path(&flush.path) {
        return;
    }
    let library_state = app.state::<LibraryState>();
    let Ok(lib) = library_state.0.lock() else {
        return;
    };
    if let Err(e) = lib.record_listen(
        &flush.path,
        flush.seconds,
        flush.completed,
        flush.skipped,
        flush.from_path.as_deref(),
    ) {
        tracing::warn!("Failed to record listen: {e}");
    }
}

fn touch_recently_played(app: &tauri::AppHandle, path: &str) {
    if is_preview_path(path) {
        return;
    }
    let library_state = app.state::<LibraryState>();
    let Ok(lib) = library_state.0.lock() else {
        return;
    };
    if let Err(e) = lib.touch_last_played(path) {
        tracing::warn!("Failed to touch last played: {e}");
    }
}

/// Observe the active track while it plays; sync when the path changes unexpectedly.
pub(crate) fn tick_listen_progress(app: &tauri::AppHandle) {
    let snapshot = {
        let state = match app.try_state::<PlayerState>() {
            Some(s) => s,
            None => return,
        };
        let guard = match state.0.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(player) = guard.as_ref() else {
            return;
        };
        let path = player
            .get_current_path()
            .map(|p| p.to_string_lossy().into_owned());
        let Some(path) = path else {
            return;
        };
        let playing = player.is_playing();
        let position = player.position_seconds();
        let duration = player.duration_seconds();
        (path, playing, position, duration)
    };

    let (path, playing, position, duration) = snapshot;
    let listen_state = match app.try_state::<ListenState>() {
        Some(s) => s,
        None => return,
    };
    let mut tracker = match listen_state.0.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    if tracker.matches_player_path(&path) {
        // Player may report a materialized path while we keyed on the library URI.
        tracker.set_player_path(path);
        tracker.set_duration(duration);
        if playing {
            tracker.observe_position(position);
        }
        return;
    }

    match tracker.current_path() {
        None => {
            if playing || position > 0.5 {
                // Cold start (restore / external play), so use engine path for both.
                tracker.start(path.clone(), path.clone(), duration, None);
                drop(tracker);
                touch_recently_played(app, &path);
            }
        }
        Some(_) => {
            // Right after listen_switch_track the engine may still report the
            // previous file for a tick or two, so don't treat that as a new song.
            if tracker.peek_seconds().unwrap_or(0.0) < 1.0 {
                return;
            }
            if playing {
                let flush = tracker.switch_track(
                    path.clone(),
                    path.clone(),
                    duration,
                    ListenEndReason::Completed,
                );
                drop(tracker);
                if let Some(flush) = flush {
                    apply_listen_flush(app, flush);
                }
                touch_recently_played(app, &path);
            }
        }
    }
}

/// Keep the Android media session in step with the engine.
///
/// The frontend reports position while it runs, but Android freezes the
/// WebView in the background and destroys it when the task is swiped away.
/// This keeps play/pause state and position right from Rust either way. The
/// bridge drops reports that match what the session already extrapolates, so
/// this costs nothing while playback runs normally.
#[cfg(target_os = "android")]
pub(crate) fn tick_media_session(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<PlayerState>() else {
        return;
    };
    let snapshot = {
        // Never wait on the player from the tick; a command holding it will
        // push its own update.
        let Ok(guard) = state.0.try_lock() else {
            return;
        };
        guard.as_ref().and_then(|player| {
            player
                .get_current_path()
                .map(|_| (player.position_seconds(), player.is_playing()))
        })
    };
    let Some((position, playing)) = snapshot else {
        return;
    };
    if let Some(bridge) = app.try_state::<MediaBridgeState>() {
        bridge.0.update_position(position, playing);
    }
}

/// Whether a track is playing right now, or only held for a call and about
/// to resume. Decides if the process stays alive when Android destroys the UI.
#[cfg(target_os = "android")]
pub(crate) fn is_playback_active(app: &tauri::AppHandle) -> bool {
    let Some(state) = app.try_state::<PlayerState>() else {
        return false;
    };
    let guard = match state.0.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .as_ref()
        .is_some_and(|player| player.is_playing() || player.is_output_suspended())
}

/// End the current listen session and start one for `record_path`.
///
/// `record_path` must be the library identity (original path / content URI).
/// Never seeds the new session with the previous track's scrubber position.
pub(crate) fn listen_switch_track(
    app: &tauri::AppHandle,
    record_path: &str,
    reason: ListenEndReason,
) {
    let (player_path, position, duration) = {
        let state = match app.try_state::<PlayerState>() {
            Some(s) => s,
            None => return,
        };
        let guard = match state.0.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(player) = guard.as_ref() else {
            return;
        };
        let player_path = player
            .get_current_path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| record_path.to_string());
        (
            player_path,
            player.position_seconds(),
            player.duration_seconds(),
        )
    };

    let listen_state = match app.try_state::<ListenState>() {
        Some(s) => s,
        None => return,
    };
    let mut tracker = match listen_state.0.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    // Only credit the outgoing track with this scrubber position when the
    // engine is still reporting that same track. After play_next()/play(),
    // get_current_path is usually already the *new* file at ~0s, so applying
    // that to the old session (or worse, seeding the new session with the
    // old track's high position) mis-attributes listens.
    let engine_still_on_outgoing = tracker.matches_player_path(&player_path)
        && tracker.current_record_path() != Some(record_path);
    if engine_still_on_outgoing {
        tracker.observe_position(position);
        tracker.set_duration(duration);
    }

    // Prefer the engine path for tick matching when it has already moved to
    // the new file; otherwise key on the library path until the engine catches up.
    let new_match_path = if !engine_still_on_outgoing {
        player_path
    } else {
        record_path.to_string()
    };

    let flush = tracker.switch_track(
        new_match_path,
        record_path.to_string(),
        if !engine_still_on_outgoing {
            duration
        } else {
            None
        },
        reason,
    );
    drop(tracker);

    if let Some(flush) = flush {
        apply_listen_flush(app, flush);
    }
    // Mark recently played immediately so the sidebar updates without waiting
    // for the song to end / be skipped.
    touch_recently_played(app, record_path);
}

/// Flush the active listen session without starting a new one (stop / app exit).
pub(crate) fn listen_flush_partial(app: &tauri::AppHandle) {
    let (player_path, position) = {
        let state = match app.try_state::<PlayerState>() {
            Some(s) => s,
            None => return,
        };
        let guard = match state.0.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(player) = guard.as_ref() else {
            return;
        };
        (
            player
                .get_current_path()
                .map(|p| p.to_string_lossy().into_owned()),
            player.position_seconds(),
        )
    };

    let listen_state = match app.try_state::<ListenState>() {
        Some(s) => s,
        None => return,
    };
    let mut tracker = match listen_state.0.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(path) = player_path.as_deref() {
        if tracker.matches_player_path(path) {
            tracker.observe_position(position);
        }
    }
    let flush = tracker.end(ListenEndReason::Partial);
    drop(tracker);
    if let Some(flush) = flush {
        apply_listen_flush(app, flush);
    }
}

pub(crate) fn persist_playback_state_throttled(app: &tauri::AppHandle) {
    let now = now_ms();
    let last = LAST_PLAYBACK_PERSIST_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < PLAYBACK_PERSIST_INTERVAL_MS {
        return;
    }
    LAST_PLAYBACK_PERSIST_MS.store(now, Ordering::Relaxed);
    persist_playback_state(app);
}

/// Load the last session's track paused at the saved scrubber position.
pub(crate) fn restore_saved_playback(app: &tauri::AppHandle) {
    let (path, position) = {
        let settings_state = match app.try_state::<AppSettingsState>() {
            Some(s) => s,
            None => return,
        };
        let settings = match settings_state.0.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let path = match settings.last_track_path.clone() {
            Some(p) if path_is_restorable(&p) => p,
            _ => return,
        };
        (path, settings.last_position_seconds)
    };

    if let Err(e) = with_app_player(app, |player| {
        if player.get_current_path().is_some() {
            return Ok(());
        }
        player
            .load_paused_at(&path, position)
            .map_err(|e| format!("Restore playback: {e}"))
    }) {
        tracing::warn!("{e}");
    }

    // ExoPlayer holds the paused track for instant UI resume. Do not publish
    // to the OS media session here. That starts the foreground service and
    // leaves a stale notification after the app is closed.
}

fn lock_library<'a>(
    state: &'a tauri::State<'a, LibraryState>,
) -> Result<std::sync::MutexGuard<'a, Library>, String> {
    state.0.lock().map_err(lock_poisoned)
}

fn sync_bridge_playing(bridge: &tauri::State<MediaBridgeState>, position_secs: f64) {
    bridge.0.set_playing(position_secs);
}

/// Run a blocking operation on a background thread pool so the UI stays
/// responsive.  Returns the inner `Result` directly.
async fn blocking<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("Background task failed: {e}"))?
}

/// Replace the queue with a playlist's songs and pick `index` to play. Songs
/// queued by hand that had yet to play go on after the playlist; whatever
/// else the old queue held, such as the rest of an album, is dropped.
fn start_playlist_queue(
    player: &mut AudioPlayer,
    queue_paths: Vec<String>,
    source: QueueSource,
    index: usize,
) {
    let manual = player.queue.upcoming_manual();
    player.queue.set_tracks(queue_paths);
    player.queue.set_source(source);
    if player.queue.pick(index).is_none() {
        tracing::warn!("Failed to align playback queue with playlist index {index}");
    }
    for path in manual {
        if !player.queue.tracks().contains(&path) {
            player.queue.enqueue(path);
        }
    }
}

/// The library revision last acted on; `i64::MIN` until the first check.
static SEEN_LIBRARY_REVISION: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(i64::MIN);

/// Notice library changes made anywhere (a folder sync, a remote source, an
/// Android scan, the CLI), tell the window, and have a queue that plays a
/// playlist pick up the songs added to it or taken out of it.
pub(crate) fn tick_library_changes(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;

    let source = {
        let state = app.state::<PlayerState>();
        let Ok(slot) = state.0.lock() else {
            return;
        };
        slot.as_ref()
            .and_then(|player| player.queue.source())
            .map(|source| source.playlist_id().to_string())
    };

    // A sync holds the library between batches; try again next tick rather
    // than hold up playback waiting for it.
    let (revision, library_paths) = {
        let state = app.state::<LibraryState>();
        let Ok(lib) = state.0.try_lock() else {
            return;
        };
        let Ok(revision) = lib.revision() else {
            return;
        };
        let seen = SEEN_LIBRARY_REVISION.load(Ordering::Relaxed);
        // The first check also reads a queue restored from the last session.
        if revision == seen {
            return;
        }
        let paths = match &source {
            Some(id) => match lib.queue_source_paths(id) {
                Ok(paths) => Some(paths),
                Err(error) => {
                    tracing::warn!("Could not read the playing playlist: {error}");
                    return;
                }
            },
            None => None,
        };
        (revision, paths)
    };
    let first_check = SEEN_LIBRARY_REVISION.swap(revision, Ordering::Relaxed) == i64::MIN;
    if !first_check {
        let _ = app.emit("library-changed", ());
    }

    let (Some(playlist_id), Some(library_paths)) = (source, library_paths) else {
        return;
    };
    let unseen: Vec<String> = {
        let state = app.state::<PlayerState>();
        let Ok(slot) = state.0.lock() else {
            return;
        };
        let Some(player) = slot.as_ref() else {
            return;
        };
        player
            .queue
            .unseen_source_paths(&library_paths)
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    let resolved: std::collections::HashMap<String, String> = unseen
        .into_iter()
        .filter_map(|path| {
            let playable = crate::android::import::resolve_playback_source(app, &path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to resolve track {path}: {e}");
                    path.clone()
                });
            (playable != path).then_some((path, playable))
        })
        .collect();

    let changed = {
        let state = app.state::<PlayerState>();
        let Ok(mut slot) = state.0.lock() else {
            return;
        };
        let Some(player) = slot.as_mut() else {
            return;
        };
        player.follow_queue_source(&playlist_id, &library_paths, &resolved)
    };
    if changed {
        persist_playback_state(app);
        let _ = app.emit("queue-changed", ());
    }
}

/// Build a minimal `Track` for a path that isn't in the library (e.g. a file
/// that was deleted or moved after being added to the queue).
/// Metadata for previews cached during this session, keyed by file path.
///
/// Previews are not library content, so they get no `tracks` row. That is what
/// keeps a 30-second clip out of browse, search, counts, listening stats, and
/// recently-played. But the queue and the player bar still need a title and an
/// artist to show, and every one of those surfaces already falls back to
/// [`placeholder_track`] for a path with no row. Registering previews here
/// means all of them display correctly with no further changes.
///
/// In-memory on purpose: previews do not survive a restart, and neither should
/// this.
fn preview_registry() -> &'static std::sync::Mutex<std::collections::HashMap<String, Track>> {
    static REGISTRY: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Track>>,
    > = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn register_preview(track: &Track) {
    if let Ok(mut map) = preview_registry().lock() {
        map.insert(track.path.clone(), track.clone());
    }
}

/// Whether this path is a preview clip rather than real library content.
pub(crate) fn is_preview_path(path: &str) -> bool {
    preview_registry()
        .lock()
        .map(|map| map.contains_key(path))
        .unwrap_or(false)
}

/// Files opened from the file manager, with the tags read from them. They
/// are not in the library, and this lets the queue and the mini player show
/// their real title and cover instead of a name guessed from the file.
fn opened_registry() -> &'static std::sync::Mutex<std::collections::HashMap<String, Track>> {
    static REGISTRY: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Track>>,
    > = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn placeholder_track(path: &str) -> Track {
    // A cached preview has real metadata even though it has no library row.
    if let Ok(map) = preview_registry().lock() {
        if let Some(track) = map.get(path) {
            return track.clone();
        }
    }
    if let Ok(map) = opened_registry().lock() {
        if let Some(track) = map.get(path) {
            return track.clone();
        }
    }
    // Guess from the file and folder names, as an import of an untagged file
    // would. A content URI ends in its encoded document id, so it is decoded
    // first rather than shown as `primary%3AMusic%2F…`.
    let is_saf_uri = path.starts_with("content://");
    let (name, fallback) = if is_saf_uri {
        crate::metadata::content_uri_fallback(path, None)
    } else {
        let name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string();
        (name, crate::metadata::fallback_fields(Path::new(path)))
    };
    let format = Path::new(&name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("Audio")
        .to_uppercase();
    Track {
        id: String::new(),
        path: path.to_string(),
        name,
        title: fallback.title,
        artist: fallback.artist,
        album: fallback.album,
        album_artist: None,
        genre: None,
        year: None,
        track_number: None,
        disc_number: None,
        format,
        duration_seconds: None,
        sample_rate: None,
        channels: None,
        bit_depth: None,
        lyrics: None,
        lyrics_source: None,
        has_synced_lyrics: false,
        cover_art_data_url: None,
        cover_art_mime: None,
        cover_art_source: None,
        album_art_id: None,
        fingerprint_sha256: None,
        acoustid_fingerprint: None,
        musicbrainz_recording_id: None,
        file_size: 0,
        modified_at: 0,
        indexed_at: 0,
        source_provider: None,
        source_state: None,
        remote_source_id: None,
        cache_state: None,
        remote_missing: false,
        is_saf_uri,
    }
}

/// Look up a track by path in the library, falling back to a placeholder.
/// The track at `path` as the queue shows it, for the tray.
#[cfg(not(target_os = "android"))]
pub(crate) fn track_for_path(app: &tauri::AppHandle, path: &str) -> Option<Track> {
    let library = app.try_state::<LibraryState>()?;
    let library = library.0.lock().ok()?;
    Some(resolve_track(&library, path))
}

fn resolve_track(library: &Library, path: &str) -> Track {
    match library.get_tracks_by_paths(&[path.to_string()]) {
        Ok(results) if results.first().is_some_and(Option::is_some) => {
            results.into_iter().next().flatten().unwrap()
        }
        _ => placeholder_track(path),
    }
}

/// Push track metadata (and current shuffle/repeat mode) to the OS media bridge.
fn sync_bridge_now_playing(app: &tauri::AppHandle, track: &Track) {
    sync_bridge_now_playing_at(app, track, 0.0);
}

/// Like [`sync_bridge_now_playing`], but anchors the media session scrubber at
/// `position_secs` (needed after a crossfade handoff mid-track).
fn sync_bridge_now_playing_at(app: &tauri::AppHandle, track: &Track, position_secs: f64) {
    let bridge = app.state::<MediaBridgeState>();
    bridge.0.now_playing_at(
        TrackMetadata {
            title: Some(track.title.clone()),
            artist: Some(track.artist.clone()),
            album: Some(track.album.clone()),
            duration_seconds: track.duration_seconds,
            cover_url: resolve_os_cover_url(app, track),
        },
        position_secs,
    );
    sync_bridge_playback_mode(app, &bridge);
}

/// Prefer 512px media-session art over the 96px UI list thumb.
fn resolve_os_cover_url(app: &tauri::AppHandle, track: &Track) -> Option<String> {
    if let Some(id) = track.album_art_id.as_deref().filter(|s| !s.is_empty()) {
        if let Some(path) = crate::cover_art::media_path_for_id(app, id) {
            return Some(path.to_string_lossy().into_owned());
        }
        // Lazily build media art from the embedded cover once per album.
        if let Ok(Some(full)) = crate::metadata::extract_full_cover_data_url(Some(app), &track.path)
        {
            if let Ok((bytes, _)) = crate::cover_art::decode_data_url(&full) {
                if let Some(path) = crate::cover_art::ensure_media_art(app, id, &bytes) {
                    return Some(path.to_string_lossy().into_owned());
                }
            }
        }
        // Last resort: derive media JPEG from the existing 96px thumb bytes.
        if let Some(thumb) = crate::cover_art::thumb_path_for_id(app, id) {
            if let Ok(bytes) = std::fs::read(&thumb) {
                if let Some(path) = crate::cover_art::ensure_media_art(app, id, &bytes) {
                    return Some(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    crate::cover_art::prefer_media_artwork_url(track.cover_art_data_url.as_deref())
}

/// GUI-side auto-advance (matches the playback daemon tick).
/// Call periodically from a background thread so Android/desktop keep playing
/// the queue when a track ends, without relying on frontend polling alone.
pub(crate) fn tick_auto_advance(app: &tauri::AppHandle) {
    // Crossfade handoff can happen while the sink is still playing, so check
    // independently of should_auto_advance to keep UI/queue/media in sync.
    let handoff = {
        let state = app.state::<PlayerState>();
        let mut slot = match state.0.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!("Player mutex was poisoned during auto-advance, recovering");
                poisoned.into_inner()
            }
        };
        let Some(player) = slot.as_mut() else {
            return;
        };
        player.tick_remote_prefetch();
        if player.check_crossfade_track_switch() {
            let path = player
                .get_current_path()
                .map(|p| p.to_string_lossy().into_owned());
            let position = player.position_seconds();
            path.map(|p| (p, position))
        } else {
            None
        }
    };

    if let Some((path, position)) = handoff {
        let track = match app.state::<LibraryState>().0.lock() {
            Ok(lib) => resolve_track(&lib, &path),
            Err(_) => placeholder_track(&path),
        };
        listen_switch_track(app, &track.path, ListenEndReason::Completed);
        sync_bridge_now_playing_at(app, &track, position);
    }

    let advanced = {
        let state = app.state::<PlayerState>();
        let mut slot = match state.0.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!("Player mutex was poisoned during auto-advance, recovering");
                poisoned.into_inner()
            }
        };
        let Some(player) = slot.as_mut() else {
            return;
        };
        if !player.should_auto_advance() || !player.ready_to_auto_advance() {
            return;
        }

        // The sleep timer was waiting for this track to end.
        if player.sleep_after_current_track() {
            let loaded = player.finish_held_track().unwrap_or_else(|error| {
                tracing::warn!("Sleep timer could not load the next track: {error}");
                let _ = player.stop();
                None
            });
            drop(slot);
            finish_sleep_track_end(app, loaded);
            return;
        }

        // Skip past unreadable files instead of stopping. A single bad track
        // must not halt background queue playback on Android.
        let mut result = None;
        for _ in 0..8 {
            match player.play_next() {
                Ok(Some(path)) => {
                    result = Some(path);
                    break;
                }
                Ok(None) => {
                    let _ = player.stop();
                    result = None;
                    break;
                }
                Err(error) => {
                    tracing::warn!("Auto-advance failed, skipping track: {error}");
                }
            }
        }
        if result.is_none() && player.get_current_path().is_some() && player.should_auto_advance() {
            let _ = player.stop();
        }
        result
    };

    match advanced {
        Some(path) => {
            let track = match app.state::<LibraryState>().0.lock() {
                Ok(lib) => resolve_track(&lib, &path),
                Err(_) => placeholder_track(&path),
            };
            listen_switch_track(app, &track.path, ListenEndReason::Completed);
            sync_bridge_now_playing(app, &track);
        }
        None => {
            // Only clear the media session when nothing is playing anymore.
            let still_playing = app
                .state::<PlayerState>()
                .0
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|p| p.is_playing() || p.is_paused()))
                .unwrap_or(false);
            if !still_playing {
                listen_flush_partial(app);
                let bridge = app.state::<MediaBridgeState>();
                bridge.0.set_stopped();
            }
        }
    }
}

/// The sleep timer's end-of-track wait is over. `loaded` is the track now
/// sitting paused at its start, or `None` when the queue ran out.
fn finish_sleep_track_end(app: &tauri::AppHandle, loaded: Option<String>) {
    match loaded {
        Some(path) => {
            let track = match app.state::<LibraryState>().0.lock() {
                Ok(lib) => resolve_track(&lib, &path),
                Err(_) => placeholder_track(&path),
            };
            listen_switch_track(app, &track.path, ListenEndReason::Completed);
            sync_bridge_now_playing(app, &track);
            app.state::<MediaBridgeState>().0.set_paused(0.0);
        }
        None => {
            listen_flush_partial(app);
            app.state::<MediaBridgeState>().0.set_stopped();
        }
    }
    persist_playback_state(app);
}

/// Fade and pause for the sleep timer. Runs on the same tick as auto-advance.
pub(crate) fn tick_sleep_timer(app: &tauri::AppHandle) {
    let paused_at = {
        let Some(state) = app.try_state::<PlayerState>() else {
            return;
        };
        let mut slot = match state.0.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(player) = slot.as_mut() else {
            return;
        };
        player.tick_sleep_timer().then(|| player.position_seconds())
    };
    if let Some(position) = paused_at {
        app.state::<MediaBridgeState>().0.set_paused(position);
        persist_playback_state(app);
    }
}

#[tauri::command]
pub async fn set_sleep_timer(
    request: SleepRequest,
    state: tauri::State<'_, PlayerState>,
) -> Result<SleepTimerStatus, String> {
    let mut slot = lock_player_state(&state);
    let player = ensure_player(&mut slot)?;
    player.set_sleep_timer(request)?;
    Ok(player.sleep_timer_status())
}

/// Save a computed waveform to the library. Returns whether `path` is a
/// library track, so the worker can keep other files in memory instead.
pub(crate) fn save_waveform(app: &tauri::AppHandle, path: &str, bins: &[u8]) -> bool {
    let Some(library) = app.try_state::<LibraryState>() else {
        return false;
    };
    let library = match library.0.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Err(error) = library.save_waveform(path, bins) {
        tracing::warn!("{error}");
        return false;
    }
    library.get_waveform(path).ok().flatten().is_some()
}

/// The seek bar waveform for `path`, or `None` while it is being computed (a
/// `waveform-ready` event follows) or when the track cannot be analysed.
#[tauri::command]
pub async fn get_waveform(
    path: String,
    library: tauri::State<'_, LibraryState>,
    waveforms: tauri::State<'_, WaveformState>,
) -> Result<Option<Vec<u8>>, String> {
    let saved = lock_library(&library)?.get_waveform(&path)?;
    Ok(waveforms.0.lookup(&path, saved))
}

/// Loudness of what is playing right now, 0.0 to 1.0, for the live waveform.
#[tauri::command]
pub async fn get_playback_level(state: tauri::State<'_, PlayerState>) -> Result<f32, String> {
    let guard = lock_player_state(&state);
    Ok(guard.as_ref().map_or(0.0, AudioPlayer::output_level))
}

#[tauri::command]
pub async fn set_playback_speed(
    speed: f32,
    state: tauri::State<'_, PlayerState>,
    bridge: tauri::State<'_, MediaBridgeState>,
) -> Result<f32, String> {
    let (speed, position, playing) = {
        let mut slot = lock_player_state(&state);
        let player = ensure_player(&mut slot)?;
        let speed = player.set_speed(speed)?;
        (speed, player.position_seconds(), player.is_playing())
    };
    bridge.0.set_speed(f64::from(speed), position, playing);
    Ok(speed)
}

/// Apply a media-session action from the Android native JNI bridge.
/// Used when the WebView is frozen in the background and JS handlers cannot run.
#[cfg(target_os = "android")]
pub(crate) fn handle_native_media_action(
    app: &tauri::AppHandle,
    action: &str,
) -> Result<(), String> {
    use crate::audio::player::RepeatMode;

    if let Some(seconds) = action.strip_prefix("seek:") {
        let seconds: f64 = seconds
            .parse()
            .map_err(|e| format!("invalid seek payload: {e}"))?;
        let playing = with_app_player(app, |player| {
            player.seek(seconds).map_err(|e| e.to_string())?;
            Ok(player.is_playing())
        })?;
        app.state::<MediaBridgeState>()
            .0
            .update_position(seconds, playing);
        return Ok(());
    }

    match action {
        "play" => {
            // Optimistic MediaSession update so the notification doesn't lag ExoPlayer.
            let position =
                with_app_player(app, |player| Ok(player.position_seconds())).unwrap_or(0.0);
            app.state::<MediaBridgeState>().0.set_playing(position);
            let position = with_app_player(app, |player| {
                if player.get_current_path().is_none() {
                    return Ok(None);
                }
                if !player.is_playing() {
                    player.resume().map_err(|e| e.to_string())?;
                }
                Ok(Some(player.position_seconds()))
            })?;
            if let Some(position) = position {
                app.state::<MediaBridgeState>().0.set_playing(position);
            }
            Ok(())
        }
        "pause" => {
            let position =
                with_app_player(app, |player| Ok(player.position_seconds())).unwrap_or(0.0);
            app.state::<MediaBridgeState>().0.set_paused(position);
            let position = with_app_player(app, |player| {
                let position = player.position_seconds();
                player.pause().map_err(|e| e.to_string())?;
                Ok(position)
            })?;
            app.state::<MediaBridgeState>().0.set_paused(position);
            Ok(())
        }
        "next" | "previous" => {
            spawn_skip(app, action == "next", |app, path| {
                let track = match app.state::<LibraryState>().0.lock() {
                    Ok(lib) => resolve_track(&lib, &path),
                    Err(_) => placeholder_track(&path),
                };
                listen_switch_track(app, &track.path, ListenEndReason::Skipped);
                sync_bridge_now_playing(app, &track);
            });
            Ok(())
        }
        "shuffle" => {
            with_app_player(app, |player| {
                let next = !player.queue.is_shuffled();
                player.set_shuffle(next);
                Ok(())
            })?;
            let bridge = app.state::<MediaBridgeState>();
            sync_bridge_playback_mode(app, &bridge);
            Ok(())
        }
        "repeat" => {
            with_app_player(app, |player| {
                let repeat = match player.repeat {
                    RepeatMode::Off => RepeatMode::All,
                    RepeatMode::All => RepeatMode::One,
                    RepeatMode::One => RepeatMode::Off,
                };
                player.set_repeat(repeat);
                Ok(())
            })?;
            let bridge = app.state::<MediaBridgeState>();
            sync_bridge_playback_mode(app, &bridge);
            Ok(())
        }
        "dismiss" | "stop" => {
            // The paused notification was swiped away, or a car / headset /
            // lock screen sent stop. Drop the controls but keep the queue, so
            // the app reopens where it left off.
            let paused = with_app_player(app, |player| {
                if player.is_playing() {
                    player.pause().map_err(|e| e.to_string())?;
                }
                Ok(())
            });
            if let Err(error) = paused {
                tracing::warn!("dismiss: pause failed: {error}");
            }
            persist_playback_state(app);
            app.state::<MediaBridgeState>().0.set_stopped();
            // Closed app, paused music, controls swiped away: close for good.
            crate::android::lifecycle::exit_if_headless(app);
            Ok(())
        }
        "theme-changed" => {
            // The phone switched between light and dark.
            let _ = app.emit("system-theme-changed", ());
            Ok(())
        }
        "ui-started" => {
            // An Activity started; if the UI had been destroyed, rebuild it.
            crate::android::lifecycle::ensure_main_window(app);
            Ok(())
        }
        "open-now-playing" => {
            // Wave was opened from the media notification or the lock screen.
            // The page may still be loading, so the request waits for it.
            OPEN_NOW_PLAYING.store(true, Ordering::Release);
            let _ = app.emit("open-now-playing", ());
            Ok(())
        }
        "suspend" | "unsuspend" => {
            // ExoPlayer held or released output for a transient focus loss.
            let (position, playing) = with_app_player(app, |player| {
                player.set_output_suspended(action == "suspend");
                Ok((player.position_seconds(), player.is_playing()))
            })?;
            app.state::<MediaBridgeState>()
                .0
                .update_position(position, playing);
            Ok(())
        }
        other => {
            tracing::debug!("Ignoring unknown Android media action: {other}");
            Ok(())
        }
    }
}

/// Set when the media session opened Wave and Now Playing should show. Held
/// here because the page may not exist yet when the tap arrives.
static OPEN_NOW_PLAYING: AtomicBool = AtomicBool::new(false);

/// Whether the media session asked for Now Playing since the page last
/// checked. Asking clears it.
#[tauri::command]
pub fn take_open_now_playing() -> bool {
    OPEN_NOW_PLAYING.swap(false, Ordering::AcqRel)
}

/// Push the current shuffle/repeat mode to the OS media bridge (e.g. so the
/// Android notification's shuffle/repeat buttons reflect the right state).
fn sync_bridge_playback_mode(app: &tauri::AppHandle, bridge: &tauri::State<MediaBridgeState>) {
    let state = app.state::<PlayerState>();
    let (shuffle, repeat) = {
        let guard = lock_player_state(&state);
        match guard.as_ref() {
            Some(player) => (player.queue.is_shuffled(), player.repeat.clone()),
            None => (false, crate::audio::player::RepeatMode::default()),
        }
    };
    bridge
        .0
        .set_playback_mode(shuffle, repeat_mode_str(&repeat).to_string());
}

fn repeat_mode_str(mode: &crate::audio::player::RepeatMode) -> &'static str {
    use crate::audio::player::RepeatMode;
    match mode {
        RepeatMode::Off => "off",
        RepeatMode::One => "one",
        RepeatMode::All => "all",
    }
}

// ── Platform / import helpers ─────────────────────────────────────────────────

#[tauri::command]
pub fn host_os() -> String {
    std::env::consts::OS.to_string()
}

/// Last Android crash / soft-failure report (for devices without adb).
#[tauri::command]
pub fn take_android_crash_report() -> Option<String> {
    #[cfg(target_os = "android")]
    {
        return crate::android::crash_report::take_last();
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

#[tauri::command]
pub fn clear_android_crash_report() {
    #[cfg(target_os = "android")]
    {
        crate::android::crash_report::clear_last();
    }
}

/// Exit the process (Android double-back-to-exit confirm).
#[tauri::command]
pub fn exit_app(app: tauri::AppHandle) {
    #[cfg(target_os = "android")]
    if let Some(bridge) = app.try_state::<MediaBridgeState>() {
        bridge.0.set_stopped();
    }
    app.exit(0);
}

#[derive(serde::Serialize)]
pub struct ImportAudioResult {
    pub paths: Vec<String>,
    pub errors: Vec<String>,
}

/// Resolve picked files/content URIs for library use (zero-copy on Android).
#[tauri::command]
pub async fn import_audio_sources(
    paths: Vec<String>,
    app: tauri::AppHandle,
) -> Result<ImportAudioResult, String> {
    let app = app.clone();
    blocking(move || {
        let mut ok = Vec::new();
        let mut errors = Vec::new();
        for source in &paths {
            match crate::android::import::resolve_library_source(&app, source) {
                Ok(path) => ok.push(path),
                Err(err) => errors.push(format!("{source}: {err}")),
            }
        }
        Ok(ImportAudioResult { paths: ok, errors })
    })
    .await
}

/// Pick a folder using Android Storage Access Framework (SAF).
/// Returns a content:// URI with persistable URI permission.
#[tauri::command]
#[cfg(target_os = "android")]
pub async fn pick_media_folder(
    app: tauri::AppHandle,
) -> Result<crate::android::folder_picker::FolderPickerResult, String> {
    // Block off the async runtime; the JNI side waits on the system picker.
    blocking(move || crate::android::folder_picker::pick_folder(&app)).await
}

/// Pick a folder using Android Storage Access Framework (SAF).
/// Returns a content:// URI with persistable URI permission.
#[tauri::command]
#[cfg(not(target_os = "android"))]
pub async fn pick_media_folder(
    _app: tauri::AppHandle,
) -> Result<crate::android::folder_picker::FolderPickerResult, String> {
    Err("Folder picker is only available on Android".to_string())
}

/// Recursively list audio files under a SAF `content://…/tree/…` URI.
/// Used on Android because `plugin-fs` `readDir` cannot walk content URIs.
#[tauri::command]
pub async fn scan_saf_folder(uri: String, app: tauri::AppHandle) -> Result<Vec<String>, String> {
    blocking(move || crate::android::saf_scan::list_audio_files(&app, &uri)).await
}

// ── Playback commands ─────────────────────────────────────────────────────────

/// Buffers the start of a remote track before the player is locked, so the
/// lock is not held while the network catches up.
fn prepare_remote_for_play(path: &str) -> Result<(), String> {
    crate::remote::stream::prepare_for_play(path, std::time::Duration::from_secs(10))
        .map_err(|e| format!("Playback failed: {e}"))
}

/// Skips to the next or previous track. A remote target is buffered first,
/// outside the player lock, so other commands don't wait on the network.
pub(crate) fn skip_track(app: &tauri::AppHandle, forward: bool) -> Result<Option<String>, String> {
    skip_via(
        |step| with_app_player(app, |player| step(player)),
        prepare_remote_for_play,
        forward,
    )
}

/// Runs a skip and the caller's follow-up on a thread of its own, so the
/// thread a tray or media event arrives on never waits for a remote track
/// to buffer.
pub(crate) fn spawn_skip(
    app: &tauri::AppHandle,
    forward: bool,
    then: impl FnOnce(&tauri::AppHandle, String) + Send + 'static,
) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("wave-skip".into())
        .spawn(move || match skip_track(&app, forward) {
            Ok(Some(path)) => then(&app, path),
            Ok(None) => {}
            Err(e) => tracing::warn!("Could not skip: {e}"),
        });
    if let Err(e) = spawned {
        tracing::warn!("Could not start a skip: {e}");
    }
}

type SkipStep<'a> = &'a mut dyn FnMut(&mut AudioPlayer) -> Result<Option<String>, String>;

/// The skip apart from Tauri: look under one lock, buffer outside it, then
/// advance under another.
fn skip_via(
    mut with_player: impl FnMut(SkipStep<'_>) -> Result<Option<String>, String>,
    prepare: impl FnOnce(&str) -> Result<(), String>,
    forward: bool,
) -> Result<Option<String>, String> {
    let target = with_player(&mut |player| {
        Ok(if forward {
            player.peek_next_path()
        } else {
            player.peek_previous_path()
        })
    })?;
    if let Some(target) = target {
        prepare(&target)?;
    }
    with_player(&mut |player| {
        if forward {
            player.play_next()
        } else {
            player.play_previous()
        }
        .map_err(|e| e.to_string())
    })
}

#[tauri::command]
pub async fn play_track(path: String, app: tauri::AppHandle) -> Result<(), String> {
    validate_audio_path(&path)?;
    let app_clone = app.clone();
    let path_clone = path.clone();
    let local_path = blocking(move || {
        crate::android::import::resolve_playback_source(&app_clone, &path_clone)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| format!("Could not access audio file: {e}"))
    })
    .await?;

    let play_path = local_path.clone();
    let original_path = path.clone();
    let app_clone = app.clone();
    blocking(move || {
        prepare_remote_for_play(&play_path)?;
        with_app_player(&app_clone, |player| {
            player
                .play(&play_path)
                .map_err(|e| format!("Playback failed: {e}"))
        })
    })
    .await?;

    let app_clone = app.clone();
    let lookup_a = original_path;
    let lookup_b = local_path;
    let track = blocking(move || {
        let lib = app_clone.state::<LibraryState>();
        let lib = lib.0.lock().map_err(|e| e.to_string())?;
        match lib.get_tracks_by_paths(std::slice::from_ref(&lookup_a)) {
            Ok(results) if results.first().is_some_and(Option::is_some) => {
                Ok::<_, String>(results.into_iter().next().flatten().unwrap())
            }
            _ => Ok(resolve_track(&lib, &lookup_b)),
        }
    })
    .await?;
    sync_bridge_now_playing(&app, &track);
    listen_switch_track(&app, &track.path, ListenEndReason::Partial);
    persist_playback_state(&app);

    Ok(())
}

/// Play `paths[index]` and replace the playback queue with `paths`.
/// Used by album/artist views so Next/auto-advance follows that list.
#[tauri::command]
pub async fn play_tracks(
    paths: Vec<String>,
    index: usize,
    app: tauri::AppHandle,
) -> Result<(), String> {
    start_queue(paths, index, false, app).await
}

/// Shuffle `paths`, the tracks of several albums or playlists, into one
/// queue marked as a mashup, and play it from the top.
#[tauri::command]
pub async fn play_mashup(paths: Vec<String>, app: tauri::AppHandle) -> Result<(), String> {
    let mut paths = paths;
    let order = crate::audio::player::random_order(paths.len());
    let shuffled = order
        .into_iter()
        .map(|i| std::mem::take(&mut paths[i]))
        .collect();
    start_queue(shuffled, 0, true, app).await
}

/// Shuffle the mashup that is playing again, keeping the current track.
#[tauri::command]
pub async fn reshuffle_mashup(
    state: tauri::State<'_, PlayerState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if !lock_player(&state)?.reshuffle_mashup() {
        return Err("No mashup is playing".to_string());
    }
    persist_playback_state(&app);
    Ok(())
}

/// Replace the queue with `paths` and play `paths[index]`.
/// When the last file-manager open arrived, to tell a multi-file selection
/// apart from a new choice.
#[cfg(not(target_os = "android"))]
static LAST_OPEN: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
/// Opens run one at a time, so a later batch never lands in a queue that an
/// earlier one is about to replace.
#[cfg(not(target_os = "android"))]
static OPENING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[cfg(not(target_os = "android"))]
const OPEN_BATCH_GAP: std::time::Duration = std::time::Duration::from_secs(1);

/// Play audio files opened from outside Wave, in order, replacing the queue.
/// With `mini`, they play in the mini player.
#[cfg(not(target_os = "android"))]
pub(crate) fn open_audio_files(app: &tauri::AppHandle, files: Vec<String>, mini: bool) {
    if files.is_empty() {
        return;
    }
    // Explorer starts Wave once per selected file, so a selection arrives as
    // several opens in quick succession: those join the queue. Drops and the
    // Open dialog hand over the whole selection at once and always replace it.
    let joins_queue = mini && {
        let mut last = LAST_OPEN.lock().unwrap_or_else(|e| e.into_inner());
        let recent = last.is_some_and(|at| at.elapsed() < OPEN_BATCH_GAP);
        *last = Some(std::time::Instant::now());
        recent
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _turn = OPENING.lock().await;
        let reader = app.clone();
        let paths = files.clone();
        let _ = blocking(move || {
            let known = reader
                .state::<LibraryState>()
                .0
                .lock()
                .ok()
                .and_then(|lib| lib.get_tracks_by_paths(&paths).ok())
                .unwrap_or_default();
            for (i, path) in paths.iter().enumerate() {
                if known.get(i).is_some_and(Option::is_some) {
                    continue;
                }
                match crate::metadata::extract_track_for_bulk(Some(&reader), path) {
                    Ok(track) => {
                        if let Ok(mut map) = opened_registry().lock() {
                            map.insert(path.clone(), track);
                        }
                    }
                    Err(e) => tracing::warn!("Could not read the tags of {path}: {e}"),
                }
            }
            Ok(())
        })
        .await;
        if joins_queue {
            let queued = with_app_player(&app, |player| {
                for file in files {
                    player.queue.enqueue(file);
                }
                Ok(())
            });
            if let Err(e) = queued {
                tracing::warn!("Could not queue the opened files: {e}");
            }
        } else if let Err(e) = start_queue(files, 0, false, app.clone()).await {
            tracing::warn!("Could not play the opened files: {e}");
            return;
        }
        persist_playback_state(&app);
        let _ = app.emit("queue-changed", ());
        if mini {
            if let Err(e) = crate::integrations::mini_player::show(&app) {
                tracing::warn!("{e}");
            }
        }
    });
}

/// Play the audio files dropped on a window, and return the folders in the
/// drop, which the frontend turns into playlists. Anything else is ignored.
#[cfg(not(target_os = "android"))]
#[tauri::command]
pub async fn open_dropped(
    paths: Vec<String>,
    app: tauri::AppHandle,
) -> Result<crate::dto::DroppedDto, String> {
    let folders: Vec<String> = paths
        .iter()
        .filter(|path| Path::new(path).is_dir())
        .cloned()
        .collect();
    let files =
        crate::app::open_files::audio_files_only(paths.into_iter().map(std::path::PathBuf::from));
    let played = files.len();
    open_audio_files(&app, files, false);
    Ok(crate::dto::DroppedDto { played, folders })
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
pub async fn show_mini_player(app: tauri::AppHandle) -> Result<(), String> {
    crate::integrations::mini_player::collapse(&app)
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
pub async fn expand_mini_player(app: tauri::AppHandle) -> Result<(), String> {
    crate::integrations::mini_player::expand(&app)
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
pub async fn close_mini_player(app: tauri::AppHandle) -> Result<(), String> {
    crate::integrations::mini_player::close(&app);
    Ok(())
}

async fn start_queue(
    paths: Vec<String>,
    index: usize,
    mashup: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if paths.is_empty() {
        return Err("No tracks to play".to_string());
    }
    if index >= paths.len() {
        return Err(format!("Track not found at index {index}"));
    }

    let app_clone = app.clone();
    let paths_clone = paths.clone();
    let materialized = blocking(move || {
        Ok::<_, String>(
            paths_clone
                .into_iter()
                .map(|path| {
                    crate::android::import::resolve_playback_source(&app_clone, &path)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|e| {
                            tracing::warn!("Failed to resolve track {path}: {e}");
                            path
                        })
                })
                .collect::<Vec<_>>(),
        )
    })
    .await?;

    let local_path = materialized
        .get(index)
        .cloned()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| format!("Audio file not found for track at index {index}"))?;

    let queue_paths = materialized.clone();
    let app_clone = app.clone();
    let first = local_path.clone();
    let played = blocking(move || {
        prepare_remote_for_play(&first)?;
        with_app_player(&app_clone, |player| {
            player.queue.set_tracks(queue_paths);
            if mashup {
                player.queue.mark_mashup();
            }
            if player.queue.pick(index).is_none() {
                tracing::warn!("Failed to align queue with play_tracks index {index}");
            }
            player
                .play_current()
                .map_err(|e| format!("Playback failed: {e}"))
        })
    })
    .await?;

    let app_clone = app.clone();
    let lookup = played.unwrap_or(local_path);
    let track = blocking(move || {
        let lib = app_clone.state::<LibraryState>();
        let lib = lib.0.lock().map_err(|e| e.to_string())?;
        Ok::<_, String>(resolve_track(&lib, &lookup))
    })
    .await?;
    sync_bridge_now_playing(&app, &track);
    persist_playback_state(&app);
    listen_switch_track(&app, &track.path, ListenEndReason::Partial);
    Ok(())
}

#[tauri::command]
pub async fn pause_track(
    state: tauri::State<'_, PlayerState>,
    bridge: tauri::State<'_, MediaBridgeState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let position = {
        let mut player = lock_player(&state)?;
        let position = player.position_seconds();
        player.pause()?;
        position
    };
    bridge.0.set_paused(position);
    persist_playback_state(&app);
    Ok(())
}

#[tauri::command]
pub async fn resume_track(
    state: tauri::State<'_, PlayerState>,
    bridge: tauri::State<'_, MediaBridgeState>,
) -> Result<(), String> {
    let position = {
        let mut player = lock_player(&state)?;
        player.resume()?;
        player.position_seconds()
    };
    sync_bridge_playing(&bridge, position);
    Ok(())
}

#[tauri::command]
pub async fn stop_track(
    state: tauri::State<'_, PlayerState>,
    bridge: tauri::State<'_, MediaBridgeState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    lock_player(&state)?.stop()?;
    bridge.0.set_stopped();
    listen_flush_partial(&app);
    persist_playback_state(&app);
    Ok(())
}

/// The state the last poll returned and when, for polls that land while
/// another command holds the player.
fn last_playback_state() -> &'static Mutex<Option<(PlaybackStateDto, std::time::Instant)>> {
    static LAST: Mutex<Option<(PlaybackStateDto, std::time::Instant)>> = Mutex::new(None);
    &LAST
}

/// `state` moved on by `elapsed` of playback, as the player would report it.
fn carried_forward(mut state: PlaybackStateDto, elapsed: std::time::Duration) -> PlaybackStateDto {
    if state.is_playing && !state.is_buffering {
        let mut position = state.position_seconds + elapsed.as_secs_f64() * f64::from(state.speed);
        if let Some(duration) = state.duration_seconds.filter(|d| *d > 0.0) {
            position = position.min(duration);
        }
        state.position_seconds = position;
    }
    state
}

#[tauri::command]
pub async fn get_playback_state(
    state: tauri::State<'_, PlayerState>,
) -> Result<PlaybackStateDto, String> {
    // Loading a track or skipping past unreadable ones can hold the player for
    // seconds. A poll that waited for it froze the seek bar and, every half
    // second, tied up one more runtime thread. While the player is busy, the
    // last answer carried forward is as good as a fresh one.
    let guard = match state.0.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            let last = last_playback_state()
                .lock()
                .ok()
                .and_then(|last| last.clone());
            // An old answer (polls pause while the app is in the background)
            // says nothing about now, so wait for the player instead.
            match last.filter(|(_, at)| at.elapsed() < std::time::Duration::from_secs(5)) {
                Some((last, at)) => return Ok(carried_forward(last, at.elapsed())),
                None => lock_player_state(&state),
            }
        }
    };
    let fresh = playback_state_of(guard.as_ref());
    drop(guard);
    if let Ok(mut last) = last_playback_state().lock() {
        *last = Some((fresh.clone(), std::time::Instant::now()));
    }
    Ok(fresh)
}

fn playback_state_of(player: Option<&AudioPlayer>) -> PlaybackStateDto {
    let Some(player) = player else {
        return PlaybackStateDto {
            is_playing: false,
            is_paused: false,
            current_path: None,
            position_seconds: 0.0,
            duration_seconds: None,
            volume: 0.8,
            output_device_name: AudioPlayer::current_output_name(),
            sleep_timer: SleepTimerStatus::default(),
            speed: 1.0,
            is_buffering: false,
        };
    };
    PlaybackStateDto {
        is_playing: player.is_playing(),
        is_paused: player.is_paused(),
        current_path: player
            .get_current_path()
            .and_then(|path| path.to_str())
            .map(str::to_string),
        position_seconds: player.position_seconds(),
        duration_seconds: player.duration_seconds(),
        volume: player.volume(),
        output_device_name: AudioPlayer::current_output_name(),
        sleep_timer: player.sleep_timer_status(),
        speed: player.speed(),
        is_buffering: player.is_buffering(),
    }
}

#[tauri::command]
pub async fn seek_track(
    seconds: f64,
    state: tauri::State<'_, PlayerState>,
    bridge: tauri::State<'_, MediaBridgeState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let playing = {
        let mut player = lock_player(&state)?;
        player.seek(seconds)?;
        player.is_playing()
    };
    bridge.0.update_position(seconds, playing);
    persist_playback_state(&app);
    Ok(())
}

#[tauri::command]
pub async fn set_volume(
    volume: f32,
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    lock_player(&state)?.set_volume(volume)?;
    let mut settings = lock_settings(&settings_state)?;
    settings.volume = volume;
    settings.save(&app)?;
    Ok(())
}

// ── Equalizer ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_eq_settings(
    state: tauri::State<'_, PlayerState>,
) -> Result<EqSettingsDto, String> {
    let guard = lock_player_state(&state);
    let eq = match guard.as_ref() {
        Some(player) => player.eq_settings(),
        None => crate::audio::dsp::EqConfig::default(),
    };
    Ok(EqSettingsDto {
        bands: eq.bands,
        enabled: eq.enabled,
    })
}

#[tauri::command]
pub async fn set_eq_bands(
    bands: Vec<f32>,
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if bands.len() != 10 {
        return Err("Expected exactly 10 EQ band values".to_string());
    }
    for (index, gain) in bands.iter().enumerate() {
        if !gain.is_finite() {
            return Err(format!("EQ band {index} must be a finite number"));
        }
        if gain.abs() > 12.0 {
            return Err(format!(
                "EQ band {index} gain must be between -12 and +12 dB"
            ));
        }
    }
    let mut arr = [0.0f32; 10];
    arr.copy_from_slice(&bands);
    lock_player(&state)?.set_eq_bands(arr);
    let mut settings = lock_settings(&settings_state)?;
    settings.equalizer.bands = arr;
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn set_eq_enabled(
    enabled: bool,
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    lock_player(&state)?.set_eq_enabled(enabled);
    let mut settings = lock_settings(&settings_state)?;
    settings.equalizer.enabled = enabled;
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn export_eq_settings(
    path: String,
    name: Option<String>,
    state: tauri::State<'_, PlayerState>,
) -> Result<(), String> {
    validate_safe_output_path(&path, "json")?;
    let player = lock_player(&state)?;
    let eq = player.eq_settings();
    crate::audio::dsp::EqPresetFile::save_to(&path, &eq, name)
}

#[tauri::command]
pub async fn import_eq_settings(
    path: String,
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let eq = crate::audio::dsp::EqPresetFile::load_from(&path)?;
    {
        let mut player = lock_player(&state)?;
        player.set_eq_bands(eq.bands);
        player.set_eq_enabled(eq.enabled);
        // Preserve the live crossfade unless the preset file explicitly set one.
        if eq.crossfade_duration > 0.0 {
            player.set_crossfade_duration(eq.crossfade_duration);
        }
    }
    let mut settings = lock_settings(&settings_state)?;
    let keep_crossfade = settings.equalizer.crossfade_duration;
    settings.equalizer = eq;
    if settings.equalizer.crossfade_duration <= 0.0 {
        settings.equalizer.crossfade_duration = keep_crossfade;
    }
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn get_crossfade_duration(state: tauri::State<'_, PlayerState>) -> Result<f32, String> {
    let guard = lock_player_state(&state);
    let duration = match guard.as_ref() {
        Some(player) => player.crossfade_duration(),
        None => 0.0,
    };
    Ok(duration)
}

#[tauri::command]
pub async fn set_crossfade_duration(
    duration: f32,
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if !duration.is_finite() {
        return Err("Crossfade duration must be a finite number".to_string());
    }
    let duration = duration.clamp(0.0, 8.0);
    lock_player(&state)?.set_crossfade_duration(duration);
    let mut settings = lock_settings(&settings_state)?;
    settings.equalizer.crossfade_duration = duration;
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn get_gapless_enabled(
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<bool, String> {
    let guard = lock_player_state(&state);
    if let Some(player) = guard.as_ref() {
        return Ok(player.gapless_enabled());
    }
    let settings = lock_settings(&settings_state)?;
    Ok(settings.gapless_enabled)
}

#[tauri::command]
pub async fn set_gapless_enabled(
    enabled: bool,
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    lock_player(&state)?.set_gapless_enabled(enabled);
    let mut settings = lock_settings(&settings_state)?;
    settings.gapless_enabled = enabled;
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn get_volume_normalization_enabled(
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<bool, String> {
    let guard = lock_player_state(&state);
    if let Some(player) = guard.as_ref() {
        return Ok(player.volume_normalization_enabled());
    }
    let settings = lock_settings(&settings_state)?;
    Ok(settings.volume_normalization_enabled)
}

#[tauri::command]
pub async fn set_volume_normalization_enabled(
    enabled: bool,
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    lock_player(&state)?.set_volume_normalization_enabled(enabled);
    let mut settings = lock_settings(&settings_state)?;
    settings.volume_normalization_enabled = enabled;
    settings.save(&app)?;
    Ok(())
}

/// Parse lyrics text into a structured sheet.
///
/// Detection is by content, so this handles plain text, LRC, Enhanced LRC, and
/// TTML without the caller needing to know which it has. Parsing lives in Rust
/// rather than the UI so it is covered by the test suite and so any future
/// consumer (a mobile notification, a fullscreen view) shares one
/// implementation.
#[tauri::command]
pub async fn parse_lyrics_sheet(text: String) -> Result<crate::lyrics::LyricsSheet, String> {
    Ok(crate::lyrics::parse_sheet(&text))
}

/// Everything the Settings page needs to configure remote sourcing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceSettingsDto {
    pub outside_sourcing_enabled: bool,
    pub jamendo_client_id: String,
    pub spotify_client_id: String,
    /// "Outside Sourcing": music from the user's own servers, such as Plex.
    pub remote_sources_enabled: bool,
    /// Android only: let remote music use mobile data.
    pub remote_cellular_enabled: bool,
    pub cache_limit_mb: u64,
    /// Share the library as a DLNA / UPnP media server while Outside
    /// Sourcing is on.
    #[serde(default)]
    pub library_sharing_enabled: bool,
    /// The shared library's name; "" uses the default.
    #[serde(default)]
    pub share_name: String,
    /// Keep sharing after the window is closed.
    #[serde(default)]
    pub share_in_background: bool,
}

#[tauri::command]
pub async fn get_source_settings(
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<SourceSettingsDto, String> {
    let settings = lock_settings(&settings_state)?;
    Ok(SourceSettingsDto {
        outside_sourcing_enabled: settings.outside_sourcing_enabled,
        jamendo_client_id: settings.jamendo_client_id.clone().unwrap_or_default(),
        spotify_client_id: settings.spotify_client_id.clone().unwrap_or_default(),
        remote_sources_enabled: settings.remote_sources_enabled,
        remote_cellular_enabled: settings.remote_cellular_enabled,
        cache_limit_mb: settings.source_cache_limit_mb,
        library_sharing_enabled: settings.library_sharing_enabled,
        share_name: settings.share_name.clone().unwrap_or_default(),
        share_in_background: settings.share_in_background,
    })
}

/// Persist source settings. Blank credentials are stored as `None`, which makes
/// the affected provider report itself as needing setup rather than failing.
#[tauri::command]
pub async fn set_source_settings(
    settings: SourceSettingsDto,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    fn normalise(raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    let (was_enabled, cap_shrank) = {
        let mut stored = lock_settings(&settings_state)?;
        let was_enabled = stored.remote_sources_enabled;
        let cap_shrank = settings.cache_limit_mb < stored.source_cache_limit_mb;
        stored.outside_sourcing_enabled = settings.outside_sourcing_enabled;
        stored.jamendo_client_id = normalise(&settings.jamendo_client_id);
        stored.spotify_client_id = normalise(&settings.spotify_client_id);
        stored.remote_sources_enabled = settings.remote_sources_enabled;
        stored.remote_cellular_enabled = settings.remote_cellular_enabled;
        stored.library_sharing_enabled = settings.library_sharing_enabled;
        stored.share_name = normalise(&settings.share_name);
        stored.share_in_background = settings.share_in_background;
        stored.source_cache_limit_mb = settings.cache_limit_mb.max(MIN_CACHE_LIMIT_MB);
        stored.save(&app)?;
        (was_enabled, cap_shrank)
    };
    crate::remote::commands::apply_settings(&app, was_enabled);
    if cap_shrank {
        let protected = queued_paths(&app);
        let task_app = app.clone();
        blocking(move || {
            let state = task_app.state::<LibraryState>();
            let library = state.0.lock().map_err(lock_poisoned)?;
            evict_source_cache_to(&task_app, &library, protected, None);
            Ok(())
        })
        .await?;
    }
    Ok(())
}

/// The smallest cache size a setting can ask for; below it a single long
/// track would not fit.
const MIN_CACHE_LIMIT_MB: u64 = 64;

#[tauri::command]
pub async fn get_auto_lyrics_download(
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<bool, String> {
    let settings = lock_settings(&settings_state)?;
    Ok(settings.auto_lyrics_download)
}

#[tauri::command]
pub async fn set_auto_lyrics_download(
    enabled: bool,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut settings = lock_settings(&settings_state)?;
    settings.auto_lyrics_download = enabled;
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn get_playlist_mashing_enabled(
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<bool, String> {
    let settings = lock_settings(&settings_state)?;
    Ok(settings.playlist_mashing_enabled)
}

#[tauri::command]
pub async fn set_playlist_mashing_enabled(
    enabled: bool,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut settings = lock_settings(&settings_state)?;
    settings.playlist_mashing_enabled = enabled;
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn get_waveform_seek_enabled(
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<bool, String> {
    let settings = lock_settings(&settings_state)?;
    Ok(settings.waveform_seek_enabled)
}

#[tauri::command]
pub async fn set_waveform_seek_enabled(
    enabled: bool,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut settings = lock_settings(&settings_state)?;
    settings.waveform_seek_enabled = enabled;
    settings.save(&app)?;
    Ok(())
}

/// Couples with their songs' titles, looked up after the settings lock is
/// released so the two locks are never held together.
fn song_coupling_dto(
    enabled: bool,
    couples: Vec<SongCouple>,
    library: &tauri::State<'_, LibraryState>,
) -> Result<SongCouplingDto, String> {
    let library = lock_library(library)?;
    let title = |path: &str| {
        let track = resolve_track(&library, path);
        if track.title.is_empty() {
            track.name
        } else {
            track.title
        }
    };
    let couples = couples
        .into_iter()
        .map(|c| SongCoupleDto {
            first_title: title(&c.first),
            second_title: title(&c.second),
            first: c.first,
            second: c.second,
        })
        .collect();
    Ok(SongCouplingDto { enabled, couples })
}

/// Apply a coupling change, save it and hand the pairs to the player, which
/// rearranges the queue straight away.
fn change_song_coupling(
    change: impl FnOnce(&mut AppSettings),
    state: &tauri::State<'_, PlayerState>,
    settings_state: &tauri::State<'_, AppSettingsState>,
    library: &tauri::State<'_, LibraryState>,
    app: &tauri::AppHandle,
) -> Result<SongCouplingDto, String> {
    let (enabled, couples, active) = {
        let mut settings = lock_settings(settings_state)?;
        change(&mut settings);
        settings.save(app)?;
        (
            settings.song_coupling_enabled,
            settings.song_couples.clone(),
            settings.active_song_couples(),
        )
    };
    if let Some(player) = lock_player_state(state).as_mut() {
        player.set_song_couples(active);
    }
    song_coupling_dto(enabled, couples, library)
}

#[tauri::command]
pub async fn get_song_coupling(
    settings_state: tauri::State<'_, AppSettingsState>,
    library: tauri::State<'_, LibraryState>,
) -> Result<SongCouplingDto, String> {
    let (enabled, couples) = {
        let settings = lock_settings(&settings_state)?;
        (
            settings.song_coupling_enabled,
            settings.song_couples.clone(),
        )
    };
    song_coupling_dto(enabled, couples, &library)
}

#[tauri::command]
pub async fn set_song_coupling_enabled(
    enabled: bool,
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
    library: tauri::State<'_, LibraryState>,
    app: tauri::AppHandle,
) -> Result<SongCouplingDto, String> {
    change_song_coupling(
        |settings| settings.song_coupling_enabled = enabled,
        &state,
        &settings_state,
        &library,
        &app,
    )
}

#[tauri::command]
pub async fn couple_songs(
    first: String,
    second: String,
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
    library: tauri::State<'_, LibraryState>,
    app: tauri::AppHandle,
) -> Result<SongCouplingDto, String> {
    change_song_coupling(
        |settings| settings.couple_songs(first, second),
        &state,
        &settings_state,
        &library,
        &app,
    )
}

#[tauri::command]
pub async fn uncouple_song(
    path: String,
    state: tauri::State<'_, PlayerState>,
    settings_state: tauri::State<'_, AppSettingsState>,
    library: tauri::State<'_, LibraryState>,
    app: tauri::AppHandle,
) -> Result<SongCouplingDto, String> {
    change_song_coupling(
        |settings| settings.uncouple_song(&path),
        &state,
        &settings_state,
        &library,
        &app,
    )
}

#[tauri::command]
pub async fn get_glass_effects_enabled(
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<bool, String> {
    let settings = lock_settings(&settings_state)?;
    Ok(settings.glass_effects_enabled)
}

#[tauri::command]
pub async fn set_glass_effects_enabled(
    enabled: bool,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut settings = lock_settings(&settings_state)?;
    settings.glass_effects_enabled = enabled;
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn get_animations_enabled(
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<bool, String> {
    let settings = lock_settings(&settings_state)?;
    Ok(settings.animations_enabled)
}

#[tauri::command]
pub async fn set_animations_enabled(
    enabled: bool,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut settings = lock_settings(&settings_state)?;
    settings.animations_enabled = enabled;
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn get_reduce_motion_enabled(
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<bool, String> {
    let settings = lock_settings(&settings_state)?;
    Ok(settings.reduce_motion_enabled)
}

#[tauri::command]
pub async fn set_reduce_motion_enabled(
    enabled: bool,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut settings = lock_settings(&settings_state)?;
    settings.reduce_motion_enabled = enabled;
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn get_theme_mode(
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<ThemeMode, String> {
    let settings = lock_settings(&settings_state)?;
    Ok(settings.theme_mode)
}

#[tauri::command]
pub async fn set_theme_mode(
    mode: ThemeMode,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    {
        let mut settings = lock_settings(&settings_state)?;
        settings.theme_mode = mode;
        settings.save(&app)?;
    }
    crate::app::theme::apply(&app, mode);
    Ok(())
}

/// The phone's light or dark setting. Desktop pages read the system theme
/// from their window, so this is `None` there.
#[tauri::command]
pub fn get_system_theme() -> Option<tauri::Theme> {
    #[cfg(target_os = "android")]
    {
        return crate::android::system_theme::is_dark().map(|dark| {
            if dark {
                tauri::Theme::Dark
            } else {
                tauri::Theme::Light
            }
        });
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// Match the status and navigation bar icons to the theme the page shows.
/// Only Android draws them over the page; elsewhere this does nothing.
#[tauri::command]
pub fn set_system_bar_theme(theme: tauri::Theme) {
    #[cfg(target_os = "android")]
    crate::android::system_theme::set_light_bars(theme == tauri::Theme::Light);
    #[cfg(not(target_os = "android"))]
    let _ = theme;
}

// ── Updates ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_auto_update_enabled(
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<bool, String> {
    let settings = lock_settings(&settings_state)?;
    Ok(settings.auto_update_enabled)
}

#[tauri::command]
pub async fn set_auto_update_enabled(
    enabled: bool,
    settings_state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut settings = lock_settings(&settings_state)?;
    settings.auto_update_enabled = enabled;
    settings.save(&app)?;
    Ok(())
}

#[tauri::command]
pub async fn check_for_update(app: tauri::AppHandle) -> Result<UpdateCheckDto, String> {
    blocking(move || crate::app::updater::check(&app, &app.state::<UpdaterState>())).await
}

/// Android only: download the APK the last check found.
#[tauri::command]
pub async fn download_update(app: tauri::AppHandle) -> Result<(), String> {
    blocking(move || crate::app::updater::download(&app, &app.state::<UpdaterState>())).await
}

/// Android only: open the system installer on the downloaded APK.
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    blocking(move || crate::app::updater::install(&app, &app.state::<UpdaterState>())).await
}

#[tauri::command]
pub async fn open_release_page(app: tauri::AppHandle) -> Result<(), String> {
    blocking(move || crate::app::updater::open_release_page(&app.state::<UpdaterState>())).await
}

#[tauri::command]
pub async fn open_repository() -> Result<(), String> {
    blocking(crate::app::updater::open_repository).await
}

/// Open a GitHub issue prefilled with an Android crash report.
#[tauri::command]
pub async fn open_crash_issue(report: String) -> Result<(), String> {
    blocking(move || crate::app::updater::open_crash_issue(&report)).await
}

// ── Library / playlist commands ───────────────────────────────────────────────

#[tauri::command]
pub async fn add_track_to_playlist(path: String, app: tauri::AppHandle) -> Result<Track, String> {
    let app = app.clone();
    blocking(move || {
        let resolved = crate::android::import::resolve_library_source(&app, &path)?;
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        lib.add_track_to_default_playlist(resolved)
    })
    .await
}

#[tauri::command]
pub async fn remove_track_from_playlist(
    path: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<(), String> {
    let lib = lock_library(&library)?;
    let playlist_id = lib.default_playlist_id()?;
    lib.remove_track_from_playlist_by_path(&playlist_id, &path)
}

#[tauri::command]
pub async fn get_playlist(library: tauri::State<'_, LibraryState>) -> Result<Vec<Track>, String> {
    lock_library(&library)?.get_default_playlist_tracks()
}

#[tauri::command]
pub async fn clear_playlist(library: tauri::State<'_, LibraryState>) -> Result<(), String> {
    lock_library(&library)?.clear_default_playlist()
}

// ── Favorites ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn add_track_to_favorites(path: String, app: tauri::AppHandle) -> Result<Track, String> {
    let app = app.clone();
    blocking(move || {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        lib.add_track_to_favorites(path)
    })
    .await
}

#[tauri::command]
pub async fn remove_track_from_favorites(
    path: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<(), String> {
    lock_library(&library)?.remove_track_from_favorites(&path)
}

#[tauri::command]
pub async fn get_favorites(library: tauri::State<'_, LibraryState>) -> Result<Vec<Track>, String> {
    lock_library(&library)?.get_favorites()
}

#[tauri::command]
pub async fn is_track_in_favorites(
    path: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<bool, String> {
    lock_library(&library)?.is_track_in_favorites(&path)
}

#[tauri::command]
pub async fn is_track_in_playlist(
    path: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<bool, String> {
    lock_library(&library)?.is_track_in_any_playlist(&path)
}

#[tauri::command]
pub async fn toggle_favorite(path: String, app: tauri::AppHandle) -> Result<bool, String> {
    let app = app.clone();
    blocking(move || {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        lib.toggle_favorite(&path)
    })
    .await
}

#[tauri::command]
pub async fn clear_favorites(library: tauri::State<'_, LibraryState>) -> Result<(), String> {
    lock_library(&library)?.clear_favorites()
}

#[tauri::command]
pub async fn play_track_from_playlist(index: usize, app: tauri::AppHandle) -> Result<(), String> {
    let app_clone = app.clone();
    let (raw_tracks, track, library_id) = blocking(move || {
        let library = app_clone.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        let tracks = lib.get_default_playlist_tracks()?;
        let track = tracks
            .get(index)
            .ok_or_else(|| format!("Track not found at index {index}"))?
            .clone();
        Ok((tracks, track, lib.default_playlist_id()?))
    })
    .await?;

    let app_clone = app.clone();
    let raw_track_paths: Vec<String> = raw_tracks.iter().map(|t| t.path.clone()).collect();
    let materialized_paths = blocking(move || {
        Ok::<_, String>(
            raw_track_paths
                .into_iter()
                .map(|path| {
                    crate::android::import::resolve_playback_source(&app_clone, &path)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|e| {
                            tracing::warn!("Failed to resolve track {path}: {e}");
                            path
                        })
                })
                .collect::<Vec<_>>(),
        )
    })
    .await?;

    let local_path = materialized_paths.get(index).cloned().unwrap_or_default();

    if local_path.is_empty() {
        return Err(format!("Audio file not found for track at index {index}"));
    }

    let source = QueueSource::new(
        library_id,
        true,
        raw_tracks
            .iter()
            .map(|t| t.path.clone())
            .zip(materialized_paths.iter().cloned()),
    );
    let tracks: Vec<Track> = raw_tracks
        .into_iter()
        .zip(materialized_paths)
        .map(|(mut t, p)| {
            t.path = p;
            t
        })
        .collect();

    let app_clone = app.clone();
    let queue_paths: Vec<String> = tracks.iter().map(|t| t.path.clone()).collect();
    let first = local_path.clone();
    let played = blocking(move || {
        prepare_remote_for_play(&first)?;
        with_app_player(&app_clone, |player| {
            start_playlist_queue(player, queue_paths, source, index);
            player
                .play_current()
                .map_err(|e| format!("Playback failed: {e}"))
        })
    })
    .await?;
    // A coupled song's first plays before it, so that may be what started.
    let track = match played {
        Some(path) if path != local_path => tracks
            .iter()
            .find(|t| t.path == path)
            .cloned()
            .unwrap_or_else(|| placeholder_track(&path)),
        _ => track,
    };

    sync_bridge_now_playing(&app, &track);
    listen_switch_track(&app, &track.path, ListenEndReason::Partial);
    Ok(())
}

#[tauri::command]
pub async fn scan_directory(directory: String) -> Result<Vec<String>, String> {
    blocking(move || {
        let dir_path = Path::new(&directory);
        if !dir_path.is_dir() {
            return Err("Path is not a directory".to_string());
        }

        let (paths, unreadable) = crate::library::audio_files_under(dir_path);
        for line in unreadable {
            tracing::warn!("Skipped {line}");
        }
        Ok(paths)
    })
    .await
}

#[tauri::command]
pub async fn list_playlists(
    profile_id: Option<String>,
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<PlaylistInfo>, String> {
    lock_library(&library)?.list_playlists(profile_id)
}

#[tauri::command]
pub async fn get_library_database_path(
    library: tauri::State<'_, LibraryState>,
) -> Result<String, String> {
    Ok(lock_library(&library)?.db_path())
}

#[tauri::command]
pub async fn get_supported_audio_extensions() -> Result<Vec<String>, String> {
    Ok(supported_audio_extensions())
}

#[tauri::command]
pub async fn get_queue(state: tauri::State<'_, PlayerState>) -> Result<QueueStateDto, String> {
    let guard = lock_player_state(&state);
    let Some(player) = guard.as_ref() else {
        return Ok(QueueStateDto {
            tracks: Vec::new(),
            current_index: None,
            is_shuffled: false,
            is_mashup: false,
        });
    };
    Ok(QueueStateDto {
        tracks: player.queue.tracks().to_vec(),
        current_index: player.queue.current_index(),
        is_shuffled: player.queue.is_shuffled(),
        is_mashup: player.queue.is_mashup(),
    })
}

#[tauri::command]
pub async fn play_next(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let app_clone = app.clone();
    let path = blocking(move || skip_track(&app_clone, true)).await?;

    if let Some(ref p) = path {
        let p = p.clone();
        let app_clone = app.clone();
        let track = blocking(move || {
            let lib = app_clone.state::<LibraryState>();
            let lib = lib.0.lock().map_err(|e| e.to_string())?;
            Ok::<_, String>(resolve_track(&lib, &p))
        })
        .await?;
        listen_switch_track(&app, &track.path, ListenEndReason::Skipped);
        sync_bridge_now_playing(&app, &track);
    }

    Ok(path)
}

#[tauri::command]
pub async fn play_previous(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let app_clone = app.clone();
    let path = blocking(move || skip_track(&app_clone, false)).await?;

    if let Some(ref p) = path {
        let p = p.clone();
        let app_clone = app.clone();
        let track = blocking(move || {
            let lib = app_clone.state::<LibraryState>();
            let lib = lib.0.lock().map_err(|e| e.to_string())?;
            Ok::<_, String>(resolve_track(&lib, &p))
        })
        .await?;
        listen_switch_track(&app, &track.path, ListenEndReason::Skipped);
        sync_bridge_now_playing(&app, &track);
    }

    Ok(path)
}

#[tauri::command]
pub async fn set_shuffle(
    enabled: bool,
    state: tauri::State<'_, PlayerState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    lock_player(&state)?.set_shuffle(enabled);
    let bridge = app.state::<MediaBridgeState>();
    sync_bridge_playback_mode(&app, &bridge);
    Ok(())
}

#[tauri::command]
pub async fn set_repeat(
    mode: String,
    state: tauri::State<'_, PlayerState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    use crate::audio::player::RepeatMode;

    let repeat = match mode.as_str() {
        "off" => RepeatMode::Off,
        "one" => RepeatMode::One,
        "all" => RepeatMode::All,
        _ => return Err(format!("Invalid repeat mode: {mode}")),
    };
    lock_player(&state)?.set_repeat(repeat);
    let bridge = app.state::<MediaBridgeState>();
    sync_bridge_playback_mode(&app, &bridge);
    Ok(())
}

#[tauri::command]
pub async fn get_playback_mode(
    state: tauri::State<'_, PlayerState>,
) -> Result<PlaybackModeDto, String> {
    use crate::audio::player::RepeatMode;

    let guard = lock_player_state(&state);
    let Some(player) = guard.as_ref() else {
        return Ok(PlaybackModeDto {
            repeat: RepeatMode::default(),
            shuffle: false,
        });
    };
    Ok(PlaybackModeDto {
        repeat: player.repeat.clone(),
        shuffle: player.queue.is_shuffled(),
    })
}

// ── Playlist CRUD ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_playlist(
    name: String,
    sync_folder: Option<String>,
    library: tauri::State<'_, LibraryState>,
) -> Result<PlaylistInfo, String> {
    lock_library(&library)?.create_playlist(&name, sync_folder.as_deref())
}

#[tauri::command]
pub async fn set_playlist_sync_folder(
    id: String,
    sync_folder: Option<String>,
    library: tauri::State<'_, LibraryState>,
) -> Result<PlaylistInfo, String> {
    lock_library(&library)?.set_playlist_sync_folder(&id, sync_folder.as_deref())
}

#[tauri::command]
pub async fn delete_playlist(
    id: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<(), String> {
    lock_library(&library)?.delete_playlist(&id)
}

#[tauri::command]
pub async fn rename_playlist(
    id: String,
    name: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<(), String> {
    lock_library(&library)?.rename_playlist(&id, &name)
}

#[tauri::command]
pub async fn get_playlist_tracks_by_id(
    id: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<Track>, String> {
    lock_library(&library)?.get_playlist_tracks(&id)
}

#[tauri::command]
pub async fn search_library_tracks(
    query: String,
    limit: Option<u32>,
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<Track>, String> {
    let capped = limit.unwrap_or(50).min(200);
    lock_library(&library)?.search_tracks_limited(&query, Some(capped))
}

/// Realtime library search with matched-field metadata and lyrics snippets.
#[tauri::command]
pub async fn search_library(
    query: String,
    limit: Option<u32>,
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<SearchHitDto>, String> {
    let capped = limit.unwrap_or(80).min(200);
    lock_library(&library)?.search_tracks_rich(&query, Some(capped))
}

/// Album and artist matches for the same query, so the search view can show
/// collections above the track hits.
#[tauri::command]
pub async fn search_library_collections(
    query: String,
    limit: Option<u32>,
    library: tauri::State<'_, LibraryState>,
) -> Result<SearchCollectionsDto, String> {
    let capped = limit.unwrap_or(12).min(50);
    let library = lock_library(&library)?;
    Ok(SearchCollectionsDto {
        albums: library.search_albums(&query, Some(capped))?,
        artists: library.search_artists(&query, Some(capped))?,
    })
}

/// Add many songs to a playlist in the order given and return how many were
/// added. A song the playlist already holds is skipped instead of failing the
/// rest, so picking a whole album that overlaps the playlist still works.
#[tauri::command]
pub async fn add_tracks_to_playlist_by_id(
    id: String,
    paths: Vec<String>,
    app: tauri::AppHandle,
) -> Result<usize, String> {
    blocking(move || {
        let library = app.state::<LibraryState>();
        let known = {
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            // Library lists every song already, and has no rows of its own.
            if lib.default_playlist_id()? == id {
                return Err("Songs in your library are already in Library".to_string());
            }
            lib.track_ids_by_paths(&paths)?
        };

        // Songs from the library go in by id. Anything else, such as a file
        // picked from disk, is resolved and read first, like a single add.
        let sources: Vec<String> = paths
            .iter()
            .filter_map(|path| {
                if known.contains_key(&crate::library::normalize_path_key(path)) {
                    return Some(path.clone());
                }
                match crate::android::import::resolve_library_source(&app, path) {
                    Ok(resolved) => Some(resolved),
                    Err(error) => {
                        tracing::warn!("Skipped adding {path} to a playlist: {error}");
                        None
                    }
                }
            })
            .collect();

        // Tags for new files are read with the library unlocked, as an
        // import does, so browsing stays live through a long add.
        const BATCH_SIZE: usize = 50;
        let mut added = 0;
        for chunk in sources.chunks(BATCH_SIZE) {
            let entries = crate::library::import_entries(Some(&app), chunk, &known);
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            added += lib.append_import_entries(&id, &entries)?;
        }
        Ok(added)
    })
    .await
}

#[tauri::command]
pub async fn add_track_to_playlist_by_id(
    id: String,
    path: String,
    app: tauri::AppHandle,
) -> Result<Track, String> {
    let app = app.clone();
    blocking(move || {
        let resolved = crate::android::import::resolve_library_source(&app, &path)?;
        let existing = {
            let library = app.state::<LibraryState>();
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            lib.get_tracks_by_paths(std::slice::from_ref(&resolved))?
                .into_iter()
                .next()
                .flatten()
        };
        if let Some(track) = existing {
            let library = app.state::<LibraryState>();
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            // Link into playlist if needed (no re-extract).
            return lib.add_track_to_playlist(&id, track.path);
        }

        // Extract with the library unlocked so browsing stays live.
        let track = crate::metadata::extract_track(Some(&app), &resolved)?;
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        let (_, _) = lib.apply_playlist_sync(&id, &[], std::slice::from_ref(&track), &[])?;
        // Re-read so callers get the upserted id / cover paths.
        Ok(lib
            .get_tracks_by_paths(&[resolved])?
            .into_iter()
            .next()
            .flatten()
            .unwrap_or(track))
    })
    .await
}

#[tauri::command]
pub async fn remove_track_from_playlist_by_id(
    id: String,
    path: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<(), String> {
    lock_library(&library)?.remove_track_from_playlist_by_path(&id, &path)
}

#[tauri::command]
pub async fn remove_track_from_library(
    path: String,
    library: tauri::State<'_, LibraryState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    lock_library(&library)?.remove_track_from_library(&path)?;
    crate::remote::commands::after_local_library_change(&app);
    Ok(())
}

#[tauri::command]
pub async fn clear_playlist_by_id(
    id: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<(), String> {
    lock_library(&library)?.clear_playlist(&id)
}

#[tauri::command]
pub async fn create_album_playlist(
    album: String,
    name: Option<String>,
    app: tauri::AppHandle,
) -> Result<PlaylistInfo, String> {
    let app = app.clone();
    blocking(move || {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        lib.create_album_playlist(&album, name.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn create_artist_playlist(
    artist: String,
    name: Option<String>,
    app: tauri::AppHandle,
) -> Result<PlaylistInfo, String> {
    let app = app.clone();
    blocking(move || {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        lib.create_artist_playlist(&artist, name.as_deref())
    })
    .await
}

// ── Album & artist browsing / querying ────────────────────────────────────────

/// List every distinct album in the library (grouped by album + album artist).
#[tauri::command]
pub async fn list_albums(
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<AlbumSummaryDto>, String> {
    lock_library(&library)?.list_albums()
}

/// List every distinct artist in the library with track and album counts.
#[tauri::command]
pub async fn list_artists(
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<ArtistSummaryDto>, String> {
    lock_library(&library)?.list_artists()
}

/// Return every track in an album. Pass `albumArtist` (from an
/// [`AlbumSummaryDto`] or a clicked `Track`'s `album_artist` falling back to
/// `artist`) to keep same-named albums by different artists apart.
#[tauri::command]
pub async fn get_album_tracks(
    album: String,
    album_artist: Option<String>,
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<Track>, String> {
    lock_library(&library)?.get_tracks_by_album(&album, album_artist.as_deref())
}

/// Return every track by an artist (a discography).
#[tauri::command]
pub async fn get_artist_tracks(
    artist: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<Track>, String> {
    lock_library(&library)?.get_tracks_by_artist(&artist)
}

/// Return distinct albums by an artist, with aggregate info for an artist page.
#[tauri::command]
pub async fn get_artist_albums(
    artist: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<AlbumSummaryDto>, String> {
    lock_library(&library)?.get_artist_albums(&artist)
}

#[tauri::command]
pub async fn get_track_details(
    path: String,
    library: tauri::State<'_, LibraryState>,
) -> Result<Option<Track>, String> {
    validate_audio_path(&path)?;
    lock_library(&library)?.get_track_details(&path)
}

/// Outcome of a metadata edit. Files are reported one by one: a single
/// read-only track should not abandon the rest of an album.
#[derive(Debug, serde::Serialize)]
pub struct MetadataEditResultDto {
    pub updated: usize,
    pub failed: Vec<MetadataEditFailureDto>,
}

#[derive(Debug, serde::Serialize)]
pub struct MetadataEditFailureDto {
    pub path: String,
    pub reason: String,
}

/// Write tag changes into the given files, then bring their library rows in
/// line with what was written.
#[tauri::command]
pub async fn update_track_metadata(
    paths: Vec<String>,
    edit: TagEdit,
    app: tauri::AppHandle,
) -> Result<MetadataEditResultDto, String> {
    if paths.is_empty() {
        return Err("No tracks to edit".into());
    }

    // Validate before touching anything, so a bad year fails the whole edit
    // instead of half an album.
    let cover_app = app.clone();
    let edit = edit.resolve_with(&move |path: &str| read_picked_image(&cover_app, path))?;
    if edit.is_empty() {
        return Ok(MetadataEditResultDto {
            updated: 0,
            failed: Vec::new(),
        });
    }

    blocking(move || {
        let state = app.state::<LibraryState>();
        let library = state.0.lock().map_err(lock_poisoned)?;

        let mut updated = 0;
        let mut failed = Vec::new();
        for path in paths {
            match write_track_metadata(&app, &library, &path, &edit) {
                Ok(()) => updated += 1,
                Err(reason) => failed.push(MetadataEditFailureDto { path, reason }),
            }
        }
        Ok(MetadataEditResultDto { updated, failed })
    })
    .await
}

/// Read a cover image the user picked, wherever the picker put it.
///
/// Android's dialog returns a `content://` URI, so the filesystem reader would
/// only ever report the file as missing.
fn read_picked_image(app: &tauri::AppHandle, path: &str) -> Result<Vec<u8>, String> {
    if is_android_content_uri(path) {
        crate::android::saf_io::read_uri_bytes(app, path, crate::tag_edit::MAX_COVER_BYTES)
    } else {
        crate::tag_edit::read_image_file(path)
    }
}

/// Retag a SAF document by copying it out, tagging the copy, then copying it
/// back. lofty needs a real file and a document URI is not one.
///
/// The copy back is the only stretch where losing power can damage the track.
/// Handing lofty the document descriptor directly would stretch that window
/// across the whole rewrite, and replacing the document instead of overwriting
/// it would mint a new URI, orphaning the library row and every playlist entry
/// that points at the old one.
fn write_tags_over_saf(
    app: &tauri::AppHandle,
    uri: &str,
    edit: &ResolvedEdit,
) -> Result<(), String> {
    let staging = crate::app_paths::app_cache_dir(app)
        .map_err(|e| format!("Failed to resolve cache dir: {e}"))?
        .join("retag");
    std::fs::create_dir_all(&staging).map_err(|e| format!("Failed to create retag dir: {e}"))?;

    // lofty sniffs the container from the bytes, so the extension is only a
    // hint. Carrying the real one over keeps it an accurate hint.
    let temp = staging.join(format!(
        "{}.{}",
        uuid::Uuid::new_v4(),
        crate::android::import::guess_extension(uri)
    ));

    let result = (|| {
        crate::android::saf_io::copy_uri_to_file(app, uri, &temp)?;
        crate::tag_edit::write_to_file(&temp, edit)?;
        crate::android::saf_io::copy_file_to_uri(app, &temp, uri)
    })();

    let _ = std::fs::remove_file(&temp);
    result
}

fn write_track_metadata(
    app: &tauri::AppHandle,
    library: &Library,
    path: &str,
    edit: &ResolvedEdit,
) -> Result<(), String> {
    crate::remote::ensure_editable(path)?;
    let track = library
        .get_track_details(path)?
        .ok_or_else(|| "Track is not in the library".to_string())?;
    if track.source_state.as_deref() == Some("cached") {
        return Err("Previews are not library files and cannot be edited".into());
    }

    if is_android_content_uri(path) {
        write_tags_over_saf(app, path, edit)?;
    } else {
        let file = validate_audio_path(path)?;
        crate::tag_edit::write_to_file(&file, edit)?;
    }

    library.update_track_tags(path, edit)?;

    match &edit.cover {
        Some(Change::Set(jpeg)) => library.set_track_cover(&track.id, jpeg)?,
        Some(Change::Clear) => library.clear_track_cover(&track.id)?,
        None => {}
    }
    Ok(())
}

/// Render an image file the user picked as a data URL, so the metadata editor
/// can show it before the edit is saved. The asset protocol is scoped to
/// Wave's own data directories, which is why this does not go through it.
#[tauri::command]
pub async fn read_cover_preview(path: String, app: tauri::AppHandle) -> Result<String, String> {
    blocking(move || {
        let data = read_picked_image(&app, &path)?;
        crate::cover_art::preview_cover_data_url(&data)
    })
    .await
}

/// Report whether these tracks can actually be retagged.
///
/// Only Android can answer anything but yes. Its folder picker falls back to a
/// read-only grant when the system turns down read + write, and a library added
/// that way cannot be written to no matter what the editor offers. Checking up
/// front lets the dialog say so before the user fills it in.
#[tauri::command]
pub async fn check_metadata_write_access(
    paths: Vec<String>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    blocking(move || {
        Ok(paths_writable(&paths, |uri| {
            crate::android::saf_io::can_write_uri(&app, uri)
        }))
    })
    .await
}

/// Whether every track in an edit can be written: no remote track, and every
/// Android document URI granted for writing.
fn paths_writable(paths: &[String], can_write_uri: impl Fn(&str) -> bool) -> bool {
    !paths.iter().any(|path| crate::remote::uri::is_remote(path))
        && paths
            .iter()
            .filter(|path| is_android_content_uri(path))
            .all(|uri| can_write_uri(uri))
}

/// Extract full embedded cover art as a one-shot data URL (not persisted).
/// Used when the lyrics sidebar needs a large album image.
#[tauri::command]
pub async fn get_track_full_cover(
    path: String,
    app: tauri::AppHandle,
    library: tauri::State<'_, LibraryState>,
) -> Result<Option<String>, String> {
    validate_audio_path(&path)?;
    let art_id = lock_library(&library)?
        .get_track_details(&path)?
        .and_then(|track| track.album_art_id);
    let app_clone = app.clone();
    blocking(move || {
        if let Ok(Some(full)) =
            crate::metadata::extract_full_cover_data_url(Some(&app_clone), &path)
        {
            return Ok(Some(full));
        }
        // A remote song has no file to read a cover from until it is cached
        // whole. The 512px copy saved with its album art is the next best;
        // the 96px list thumbnail looked blurred at full size.
        Ok(art_id
            .and_then(|id| crate::cover_art::media_path_for_id(&app_clone, &id))
            .and_then(|media| std::fs::read(media).ok())
            .map(|bytes| crate::cover_art::full_cover_data_url(bytes, "image/jpeg")))
    })
    .await
}

#[tauri::command]
pub async fn fetch_lyrics_for_track(path: String, app: tauri::AppHandle) -> Result<Track, String> {
    validate_audio_path(&path)?;
    let p = path.clone();
    let app_clone = app.clone();

    // Resolve under the library lock, then release it before any network I/O
    // so playback controls stay responsive while lyrics are fetched.
    let mut track = blocking(move || {
        let library = app_clone.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        if let Ok(Some(detailed)) = lib.get_track_details(&p) {
            return Ok(detailed);
        }
        Ok(resolve_track(&lib, &p))
    })
    .await?;

    if track.lyrics.is_some()
        && (track.lyrics_source.as_deref() == Some("lrclib")
            || has_lrc_timestamps(track.lyrics.as_deref().unwrap_or("")))
    {
        return Ok(track);
    }

    let enriched = blocking(move || {
        let mut track = track;
        enrich_lyrics_online(&mut track);
        Ok(track)
    })
    .await?;
    track = enriched;

    if let (Some(lyrics), Some(source)) = (&track.lyrics.clone(), &track.lyrics_source.clone()) {
        let track_id = track.id.clone();
        let lyrics = lyrics.clone();
        let source = source.clone();
        let app_clone = app.clone();
        let _ = blocking(move || {
            let library = app_clone.state::<LibraryState>();
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            lib.set_track_lyrics(&track_id, &lyrics, &source)
        })
        .await;
    }

    Ok(track)
}

#[tauri::command]
pub async fn play_track_from_specific_playlist(
    playlist_id: String,
    index: usize,
    ordered_paths: Option<Vec<String>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let app_clone = app.clone();
    let lookup_id = playlist_id.clone();
    let raw_tracks = blocking(move || {
        let library = app_clone.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        lib.get_playlist_tracks(&lookup_id)
    })
    .await?;

    let original_paths: Vec<String> = if let Some(ref paths) = ordered_paths {
        paths.clone()
    } else {
        raw_tracks.iter().map(|t| t.path.clone()).collect()
    };

    if index >= original_paths.len() {
        return Err(format!("Track not found at index {index}"));
    }

    // Resolve every queue entry (Android keeps content:// for ExoPlayer).
    let app_clone = app.clone();
    let paths_to_materialize = original_paths.clone();
    let materialized = blocking(move || {
        Ok::<_, String>(
            paths_to_materialize
                .into_iter()
                .map(|path| {
                    crate::android::import::resolve_playback_source(&app_clone, &path)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|e| {
                            tracing::warn!("Failed to resolve track {path}: {e}");
                            path
                        })
                })
                .collect::<Vec<_>>(),
        )
    })
    .await?;

    let local_path = materialized
        .get(index)
        .cloned()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| format!("Audio file not found for track at index {index}"))?;

    let original_path = original_paths[index].clone();
    let track = raw_tracks
        .iter()
        .find(|t| t.path == original_path)
        .cloned()
        .unwrap_or_else(|| placeholder_track(&original_path));

    let source = QueueSource::new(
        playlist_id,
        ordered_paths.is_none(),
        original_paths.into_iter().zip(materialized.iter().cloned()),
    );
    let queue_for_sync = materialized;
    let app_clone = app.clone();
    let first = local_path.clone();
    let played = blocking(move || {
        prepare_remote_for_play(&first)?;
        with_app_player(&app_clone, |player| {
            start_playlist_queue(player, queue_for_sync, source, index);
            player
                .play_current()
                .map_err(|e| format!("Playback failed: {e}"))
        })
    })
    .await?;

    // A coupled song's first plays before it, so that may be what started.
    let (played_track, listened_path) = match played {
        Some(path) if path != local_path => {
            let app_clone = app.clone();
            let lookup = path.clone();
            let track = blocking(move || {
                let lib = app_clone.state::<LibraryState>();
                let lib = lib.0.lock().map_err(|e| e.to_string())?;
                Ok::<_, String>(resolve_track(&lib, &lookup))
            })
            .await?;
            (track, path)
        }
        _ => {
            let mut played_track = track;
            played_track.path = local_path;
            (played_track, original_path)
        }
    };
    sync_bridge_now_playing(&app, &played_track);
    listen_switch_track(&app, &listened_path, ListenEndReason::Partial);
    Ok(())
}

// ── Queue manipulation ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn add_to_queue(
    path: String,
    state: tauri::State<'_, PlayerState>,
) -> Result<(), String> {
    validate_audio_path(&path)?;
    lock_player(&state)?.enqueue(&path);
    Ok(())
}

#[tauri::command]
pub async fn queue_insert_next(
    path: String,
    state: tauri::State<'_, PlayerState>,
) -> Result<(), String> {
    validate_audio_path(&path)?;
    lock_player(&state)?.insert_next(&path);
    Ok(())
}

/// Removing the playing track moves playback on to the one after it, so the
/// media session and listening stats have to follow, as they do for a skip.
#[tauri::command]
pub async fn remove_from_queue(
    index: usize,
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    let task_app = app.clone();
    let (removed, before, after, paused) = blocking(move || {
        let following = with_app_player(&task_app, |player| {
            let current = player.queue.current_index() == Some(index);
            Ok(current
                .then(|| player.queue.tracks().get(index + 1).cloned())
                .flatten())
        })?;
        if let Some(following) = following {
            if let Err(e) = prepare_remote_for_play(&following) {
                tracing::warn!("{e}");
            }
        }
        with_app_player(&task_app, |player| {
            let path_of = |player: &AudioPlayer| {
                player
                    .get_current_path()
                    .map(|p| p.to_string_lossy().into_owned())
            };
            let before = path_of(player);
            let removed = player.remove_from_queue(index);
            Ok((removed, before, path_of(player), player.is_paused()))
        })
    })
    .await?;

    if before != after {
        match after {
            Some(path) => {
                let lookup_app = app.clone();
                let track = blocking(move || {
                    let lib = lookup_app.state::<LibraryState>();
                    let lib = lib.0.lock().map_err(|e| e.to_string())?;
                    Ok::<_, String>(resolve_track(&lib, &path))
                })
                .await?;
                listen_switch_track(&app, &track.path, ListenEndReason::Skipped);
                sync_bridge_now_playing(&app, &track);
                if paused {
                    app.state::<MediaBridgeState>().0.set_paused(0.0);
                }
            }
            None => {
                listen_flush_partial(&app);
                app.state::<MediaBridgeState>().0.set_stopped();
            }
        }
        persist_playback_state(&app);
    }
    Ok(removed)
}

#[tauri::command]
pub async fn move_queue_track(
    from: usize,
    to: usize,
    state: tauri::State<'_, PlayerState>,
) -> Result<(), String> {
    let moved = lock_player(&state)?.move_queue_track(from, to);
    if moved {
        Ok(())
    } else {
        Err("Invalid queue move".into())
    }
}

#[tauri::command]
pub async fn clear_queue(state: tauri::State<'_, PlayerState>) -> Result<(), String> {
    lock_player(&state)?.clear_upcoming();
    Ok(())
}

#[tauri::command]
pub async fn get_queue_tracks(
    state: tauri::State<'_, PlayerState>,
    library: tauri::State<'_, LibraryState>,
) -> Result<QueueDto, String> {
    let (paths, current_index, is_shuffled, is_mashup) = {
        let guard = lock_player_state(&state);
        match guard.as_ref() {
            Some(player) => (
                player.queue.tracks().to_vec(),
                player.queue.current_index(),
                player.queue.is_shuffled(),
                player.queue.is_mashup(),
            ),
            None => (Vec::new(), None, false, false),
        }
    };

    let lookup = lock_library(&library)?.get_tracks_by_paths(&paths)?;
    let tracks: Vec<Track> = paths
        .iter()
        .enumerate()
        .map(|(i, path)| match lookup.get(i).and_then(|o| o.as_ref()) {
            Some(track) => track.clone(),
            None => placeholder_track(path),
        })
        .collect();

    Ok(QueueDto {
        tracks,
        current_index,
        is_shuffled,
        is_mashup,
    })
}

#[tauri::command]
pub async fn play_track_from_queue(index: usize, app: tauri::AppHandle) -> Result<(), String> {
    let app_clone = app.clone();
    let path = blocking(move || {
        let picked = with_app_player(&app_clone, |guard| {
            Ok(guard.queue.tracks().get(index).cloned())
        })?;
        if let Some(picked) = picked {
            prepare_remote_for_play(&picked)?;
        }
        with_app_player(&app_clone, |guard| {
            if guard.queue.pick(index).is_none() {
                return Ok(None);
            }
            guard.play_current().map_err(|e| e.to_string())
        })
    })
    .await?;

    if let Some(ref p) = path {
        let p = p.clone();
        let app_clone = app.clone();
        let track = blocking(move || {
            let lib = app_clone.state::<LibraryState>();
            let lib = lib.0.lock().map_err(|e| e.to_string())?;
            Ok::<_, String>(resolve_track(&lib, &p))
        })
        .await?;
        sync_bridge_now_playing(&app, &track);
        listen_switch_track(&app, &track.path, ListenEndReason::Partial);
    }

    Ok(())
}

// ── Playlist export / import ─────────────────────────────────────────────────

#[tauri::command]
pub async fn export_playlist(
    playlist_id: String,
    path: String,
    export_format: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let app = app.clone();
    blocking(move || {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        let expected_ext = match export_format.as_str() {
            "m3u" => "m3u",
            "json" => "json",
            _ => return Err(format!("Unknown export format: {export_format}")),
        };
        validate_safe_output_path(&path, expected_ext)?;
        match export_format.as_str() {
            "m3u" => lib.export_playlist_m3u(&playlist_id, &path),
            "json" => lib.export_playlist_json(&playlist_id, &path),
            _ => unreachable!(),
        }
    })
    .await
}

#[tauri::command]
pub async fn import_playlist(
    path: String,
    name: Option<String>,
    app: tauri::AppHandle,
) -> Result<ImportResultDto, String> {
    validate_playlist_import_path(&path)?;

    // Reading tags for every new file is the slow part, so it runs with the
    // library unlocked; the lock is only taken for each short write. Holding
    // it throughout froze every other library call until the import ended.
    blocking(move || {
        let file = crate::library::read_playlist_file(&path)?;
        let library = app.state::<LibraryState>();
        let (playlist, known) = {
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            let playlist = lib.create_playlist_for_import(name.as_deref().unwrap_or(&file.name))?;
            let known = lib.track_ids_by_paths(&file.paths)?;
            (playlist, known)
        };

        const BATCH_SIZE: usize = 50;
        let mut track_count = 0;
        for chunk in file.paths.chunks(BATCH_SIZE) {
            let entries = crate::library::import_entries(Some(&app), chunk, &known);
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            track_count += lib.append_import_entries(&playlist.id, &entries)?;
        }

        Ok(ImportResultDto {
            playlist_id: playlist.id,
            playlist_name: playlist.name,
            track_count,
        })
    })
    .await
}

#[tauri::command]
pub async fn export_lyrics(path: String, app: tauri::AppHandle) -> Result<usize, String> {
    let app = app.clone();
    blocking(move || {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        validate_safe_output_path(&path, "json")?;
        lib.export_lyrics_json(&path)
    })
    .await
}

#[tauri::command]
pub async fn import_lyrics(
    path: String,
    app: tauri::AppHandle,
) -> Result<LyricsImportResultDto, String> {
    let app = app.clone();
    blocking(move || {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        let (imported, skipped, missing) = lib.import_lyrics_json(&path)?;
        Ok(LyricsImportResultDto {
            imported,
            skipped,
            missing,
        })
    })
    .await
}

// ── Audio output devices ─────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_output_devices() -> Result<Vec<String>, String> {
    Ok(AudioPlayer::list_output_devices())
}

#[tauri::command]
pub async fn set_output_device(
    device_name: String,
    state: tauri::State<'_, PlayerState>,
) -> Result<(), String> {
    let mut slot = lock_player_state(&state);
    let guard = ensure_player(&mut slot)?;

    // Read state from the current player without mutating it. If building
    // the new player or resuming on it fails below, the old player must be
    // left completely intact (queue included) rather than half-torn-down.
    let was_playing = guard.is_playing();
    let was_paused = guard.is_paused();
    let current_path = guard
        .get_current_path()
        .and_then(|p| p.to_str().map(String::from));
    let position = guard.position_seconds();
    let volume = guard.volume();
    let queue = guard.queue.clone();
    let repeat = guard.repeat.clone();
    let eq_config = guard
        .eq_config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let eq_version = *guard.eq_version.lock().unwrap_or_else(|e| e.into_inner());
    let crossfade = guard.crossfade_duration();
    let gapless = guard.gapless_enabled();
    let normalization = guard.volume_normalization_enabled();

    // Build a new player on the requested device.
    let mut new_player = AudioPlayer::new_with_device(&device_name)?;
    new_player.queue = queue;
    new_player.repeat = repeat;
    new_player.set_volume(volume)?;
    *new_player
        .eq_config
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = eq_config;
    *new_player
        .eq_version
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = eq_version;
    new_player.set_crossfade_duration(crossfade);
    new_player.set_gapless_enabled(gapless);
    new_player.set_volume_normalization_enabled(normalization);
    new_player.carry_sleep_timer_from(guard);
    let _ = new_player.set_speed(guard.speed());

    // The old device keeps playing until its player is replaced below, so
    // silence it before the new one starts rather than let both play.
    if was_playing {
        let _ = guard.pause();
    }

    // Resume playback best-effort: a failure here (e.g. the file that was
    // playing has since been deleted) shouldn't discard an otherwise-
    // successful device switch and the queue/EQ we just carried over. A
    // paused track is loaded paused, so none of it is heard on the way.
    if let Some(ref path) = current_path {
        let resumed = if was_paused {
            new_player.load_paused_at(path, position)
        } else if was_playing {
            new_player.play(path).map(|()| {
                if position > 0.0 {
                    let _ = new_player.seek(position);
                }
            })
        } else {
            Ok(())
        };
        if let Err(e) = resumed {
            tracing::warn!("Resume after output device switch failed: {e}");
        }
    }

    *slot = Some(new_player);
    Ok(())
}

// ── OS media controls ─────────────────────────────────────────────────────────

/// Called by the frontend whenever the currently playing track changes.
/// Pushes rich metadata (title, artist, album, duration, cover art URL) to the
/// OS media interface so it shows up in the system media overlay / Control Center.
///
/// When the frontend omits `cover_url` (intentional, so the 96px list thumb
/// never overwrites OS art), we resolve the current track's 512px media-session
/// art here so macOS/Windows keep showing high-quality artwork.
#[tauri::command]
pub async fn update_media_metadata(
    app: tauri::AppHandle,
    mut metadata: TrackMetadata,
    bridge: tauri::State<'_, MediaBridgeState>,
) -> Result<(), String> {
    let needs_cover = metadata
        .cover_url
        .as_ref()
        .map(|s| s.trim().is_empty())
        .unwrap_or(true);
    if needs_cover {
        let path = {
            let state = app.state::<PlayerState>();
            let mut slot = state.0.lock().map_err(|e| e.to_string())?;
            slot.as_mut()
                .and_then(|player| player.get_current_path())
                .and_then(|p| p.to_str().map(String::from))
        };
        if let Some(path) = path {
            let track = {
                let lib = app.state::<LibraryState>();
                let lib = lib.0.lock().map_err(|e| e.to_string())?;
                resolve_track(&lib, &path)
            };
            metadata.cover_url = resolve_os_cover_url(&app, &track);
        }
    }
    bridge.0.set_metadata(metadata);
    Ok(())
}

/// Called periodically (every 500 ms) by the frontend to keep the OS media
/// interface playback position in sync with the actual audio clock.
#[tauri::command]
pub async fn update_media_position(
    position_seconds: f64,
    is_playing: bool,
    bridge: tauri::State<'_, MediaBridgeState>,
) -> Result<(), String> {
    bridge.0.update_position(position_seconds, is_playing);
    Ok(())
}

/// Clear the OS media session when nothing is loaded (Stopped, no metadata).
#[tauri::command]
pub async fn clear_media_session(bridge: tauri::State<'_, MediaBridgeState>) -> Result<(), String> {
    bridge.0.set_stopped();
    Ok(())
}

// ── Window / app settings ─────────────────────────────────────────────────────

fn lock_settings<'a>(
    state: &'a tauri::State<'a, AppSettingsState>,
) -> Result<std::sync::MutexGuard<'a, AppSettings>, String> {
    state.0.lock().map_err(lock_poisoned)
}

/// Return what the window close button currently does.
#[tauri::command]
pub fn get_close_action(state: tauri::State<'_, AppSettingsState>) -> Result<CloseAction, String> {
    Ok(lock_settings(&state)?.close_action)
}

/// Set what the window close button does.
#[tauri::command]
pub fn set_close_action(
    action: CloseAction,
    state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<CloseAction, String> {
    let mut settings = lock_settings(&state)?;
    settings.close_action = action;
    settings.save(&app)?;
    Ok(settings.close_action)
}

/// Toggle the window close button between quitting and hiding the window.
#[tauri::command]
pub fn toggle_close_action(
    state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<CloseAction, String> {
    let mut settings = lock_settings(&state)?;
    settings.toggle_close_action();
    settings.save(&app)?;
    Ok(settings.close_action)
}

// ── Media folders ─────────────────────────────────────────────────────────────

/// Return the list of saved media folder paths/URIs.
#[tauri::command]
pub fn list_media_folders(
    state: tauri::State<'_, AppSettingsState>,
) -> Result<Vec<String>, String> {
    let settings = lock_settings(&state)?;
    Ok(settings.media_folders.clone())
}

/// Persist a new media folder URI (e.g. content://… on Android) to settings.
#[tauri::command]
pub fn save_media_folder(
    path: String,
    state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut settings = lock_settings(&state)?;
    if !settings.media_folders.contains(&path) {
        settings.media_folders.push(path);
        settings.save(&app)?;
    }
    Ok(())
}

/// Remove a media folder URI from settings.
#[tauri::command]
pub fn remove_media_folder(
    path: String,
    state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut settings = lock_settings(&state)?;
    settings.media_folders.retain(|f| f != &path);
    settings.save(&app)?;
    Ok(())
}

/// Scan a local directory for audio files and return their paths.
/// Works on desktop where paths are real filesystem paths.
#[tauri::command]
pub async fn scan_media_folder(folder: String) -> Result<Vec<String>, String> {
    scan_directory(folder).await
}

/// Import audio files found by a scan into a playlist.
/// On Android, `content://` URIs are indexed in place (zero-copy).
#[derive(serde::Serialize)]
pub struct ScanImportResult {
    pub imported: u32,
    pub errors: Vec<String>,
}

/// Process large folders in chunks so metadata extract + DB writes stay bounded.
const IMPORT_BATCH_SIZE: usize = 40;

fn normalize_import_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with("content://") {
        return trimmed.to_string();
    }
    Path::new(trimmed)
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| trimmed.to_string())
}

fn push_import_error(errors: &mut Vec<String>, detail: String) {
    if errors.len() < 40 {
        errors.push(detail);
    }
}

#[tauri::command]
pub async fn import_scanned_audio(
    paths: Vec<String>,
    playlist_id: String,
    app: tauri::AppHandle,
) -> Result<ScanImportResult, String> {
    let app_clone = app.clone();
    let result = blocking(move || {
        let library = app_clone.state::<LibraryState>();
        let mut errors = Vec::new();
        let mut imported = 0u32;
        let total = paths.len();

        // Resolve paths first (cheap), then extract outside the library lock.
        let mut desired = Vec::with_capacity(paths.len());
        let mut seen = std::collections::HashSet::new();
        for path in &paths {
            match crate::android::import::resolve_library_source(&app_clone, path) {
                Ok(resolved) => {
                    let key = normalize_import_path(&resolved);
                    if seen.insert(key.clone()) {
                        desired.push(key);
                    }
                }
                Err(e) => push_import_error(&mut errors, format!("{path}: {e}")),
            }
        }

        let existing_ids = {
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            lib.track_ids_by_paths(&desired)?
        };

        let mut processed = 0usize;
        for chunk in desired.chunks(IMPORT_BATCH_SIZE) {
            let mut extracted = Vec::new();
            let mut link_ids = Vec::new();
            for path in chunk {
                let key = normalize_import_path(path);
                if let Some(id) = existing_ids.get(&key).cloned() {
                    link_ids.push(id);
                    processed += 1;
                    continue;
                }
                match crate::metadata::extract_track_for_bulk(Some(&app_clone), path) {
                    Ok(track) => extracted.push(track),
                    Err(e) => {
                        tracing::warn!("Import skip (metadata): {path}: {e}");
                        push_import_error(&mut errors, format!("{path}: {e}"));
                    }
                }
                processed += 1;
            }

            // `added` already counts the known tracks it linked into the
            // playlist; ones the playlist held before were not imported.
            let batch_added = {
                let lib = library.0.lock().map_err(|e| e.to_string())?;
                let (added, _) =
                    lib.apply_playlist_sync(&playlist_id, &[], &extracted, &link_ids)?;
                added
            };
            imported += batch_added;

            let _ = app_clone.emit(
                "sync-progress",
                serde_json::json!({
                    "playlist_id": playlist_id,
                    "phase": "extract",
                    "processed": processed,
                    "to_add": total,
                    "extracted": imported,
                }),
            );

            // Yield so UI / audio ticks stay responsive on large libraries.
            std::thread::sleep(std::time::Duration::from_millis(8));
        }

        let _ = app_clone.emit(
            "sync-progress",
            serde_json::json!({
                "playlist_id": playlist_id,
                "phase": "done",
                "added": imported,
            }),
        );

        Ok(ScanImportResult { imported, errors })
    })
    .await;
    crate::remote::commands::after_local_library_change(&app);
    result
}

#[derive(serde::Serialize)]
pub struct SyncPlaylistResult {
    pub added: u32,
    pub removed: u32,
    pub errors: Vec<String>,
}

/// Read tags and covers for `paths` on a few threads, one result per path in
/// the same order. Each file is mostly waiting on storage and image decoding,
/// so this is where a large import spends its time.
fn extract_tracks_parallel(app: &tauri::AppHandle, paths: &[&str]) -> Vec<Result<Track, String>> {
    use std::sync::atomic::AtomicUsize;

    // A few threads is where phones stop getting faster; each one also holds
    // a decoded cover in memory.
    let workers = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 4)
        .min(paths.len());
    if workers <= 1 {
        return paths
            .iter()
            .map(|path| crate::metadata::extract_track_for_bulk(Some(app), path))
            .collect();
    }

    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Result<Track, String>>>> =
        paths.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = paths.get(index) else {
                    break;
                };
                let result = crate::metadata::extract_track_for_bulk(Some(app), path);
                if let Ok(mut slot) = slots[index].lock() {
                    *slot = Some(result);
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .ok()
                .flatten()
                .unwrap_or_else(|| Err("Reading the file stopped unexpectedly".into()))
        })
        .collect()
}

/// Reconcile a synced playlist with its folder contents.
///
/// `scanned_paths`: optional pre-scanned file list (required on Android SAF).
/// When `None`, the playlist's `sync_folder` is walked on disk (desktop).
///
/// Locks the library only in short bursts so the UI can keep loading / switching
/// playlists while a large sync runs. Metadata extraction for new files happens
/// with the library unlocked. Processes in batches to avoid OOM on mobile.
#[tauri::command]
pub async fn sync_playlist_folder(
    playlist_id: String,
    scanned_paths: Option<Vec<String>>,
    app: tauri::AppHandle,
) -> Result<SyncPlaylistResult, String> {
    const BATCH_SIZE: usize = 40;

    let app_clone = app.clone();
    let playlist_id_clone = playlist_id.clone();
    let result = blocking(move || {
        let library = app_clone.state::<LibraryState>();

        let sync_folder = {
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            let playlists = lib.list_playlists(None)?;
            playlists
                .into_iter()
                .find(|p| p.id == playlist_id_clone)
                .and_then(|p| p.sync_folder)
                .ok_or_else(|| "Playlist is not linked to a sync folder".to_string())?
        };

        let mut errors = Vec::new();
        let raw_paths = if let Some(paths) = scanned_paths {
            paths
        } else {
            let dir_path = Path::new(&sync_folder);
            if !dir_path.is_dir() {
                return Err(format!(
                    "Sync folder is missing or not a directory: {sync_folder}"
                ));
            }
            let (paths, unreadable) = crate::library::audio_files_under(dir_path);
            for line in unreadable {
                push_import_error(&mut errors, line);
            }
            paths
        };

        let mut desired = Vec::with_capacity(raw_paths.len());
        let mut seen = std::collections::HashSet::new();

        // Resolve in chunks (Android: keep content://; desktop: real paths).
        for chunk in raw_paths.chunks(BATCH_SIZE) {
            for path in chunk {
                match crate::android::import::resolve_library_source(&app_clone, path) {
                    Ok(resolved) => {
                        let key = normalize_import_path(&resolved);
                        if seen.insert(key.clone()) {
                            desired.push(key);
                        }
                    }
                    Err(e) => push_import_error(&mut errors, format!("{path}: {e}")),
                }
            }
        }

        let (to_remove, to_add) = {
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            lib.diff_playlist_paths(&playlist_id_clone, &desired)?
        };

        let _ = app_clone.emit(
            "sync-progress",
            serde_json::json!({
                "playlist_id": playlist_id_clone,
                "phase": "diff",
                "scanned": desired.len(),
                "to_add": to_add.len(),
                "to_remove": to_remove.len(),
            }),
        );

        if to_remove.is_empty() && to_add.is_empty() {
            return Ok(SyncPlaylistResult {
                added: 0,
                removed: 0,
                errors,
            });
        }

        // Removals first in one short write, then add in memory-bounded batches.
        let mut removed = 0u32;
        if !to_remove.is_empty() {
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            let (_, r) = lib.apply_playlist_sync(&playlist_id_clone, &to_remove, &[], &[])?;
            removed = r;
        }

        let existing_ids = {
            let lib = library.0.lock().map_err(|e| e.to_string())?;
            lib.track_ids_by_paths(&to_add)?
        };

        let mut added = 0u32;
        let mut processed = 0usize;
        let mut need_online_art: Vec<Track> = Vec::new();

        let started = std::time::Instant::now();
        for chunk in to_add.chunks(BATCH_SIZE) {
            let mut extracted = Vec::new();
            let mut link_ids = Vec::new();
            let mut new_paths = Vec::new();
            for path in chunk {
                let key = normalize_import_path(path);
                if let Some(id) = existing_ids.get(&key).cloned() {
                    link_ids.push(id);
                } else {
                    new_paths.push(path.as_str());
                }
                processed += 1;
            }
            let results = extract_tracks_parallel(&app_clone, &new_paths);
            for (path, result) in new_paths.iter().zip(results) {
                match result {
                    Ok(track) => {
                        if track.album_art_id.is_none() {
                            need_online_art.push(track.clone());
                        }
                        extracted.push(track);
                    }
                    Err(e) => {
                        tracing::warn!("Sync skip (metadata): {path}: {e}");
                        push_import_error(&mut errors, format!("{path}: {e}"));
                    }
                }
            }

            // `apply_playlist_sync` counts the known tracks it links in too.
            let batch_added = {
                let lib = library.0.lock().map_err(|e| e.to_string())?;
                let (a, _) =
                    lib.apply_playlist_sync(&playlist_id_clone, &[], &extracted, &link_ids)?;
                a
            };
            added += batch_added;

            let _ = app_clone.emit(
                "sync-progress",
                serde_json::json!({
                    "playlist_id": playlist_id_clone,
                    "phase": "extract",
                    "processed": processed,
                    "to_add": to_add.len(),
                    "extracted": added,
                }),
            );

            std::thread::sleep(std::time::Duration::from_millis(8));
        }

        tracing::info!(
            "Sync added {added} of {} track(s) in {:.1}s",
            to_add.len(),
            started.elapsed().as_secs_f64()
        );
        let _ = app_clone.emit(
            "sync-progress",
            serde_json::json!({
                "playlist_id": playlist_id_clone,
                "phase": "done",
                "added": added,
                "removed": removed,
            }),
        );

        if !need_online_art.is_empty() {
            let enrich_app = app_clone.clone();
            std::thread::spawn(move || {
                for mut track in need_online_art {
                    crate::metadata::enrich_cover_art_online(&enrich_app, &mut track);
                    if track.album_art_id.is_none() {
                        continue;
                    }
                    if let Ok(lib) = enrich_app.state::<LibraryState>().0.lock() {
                        let _ = lib.apply_track_art_update(&track);
                    }
                }
                let _ =
                    enrich_app.emit("sync-progress", serde_json::json!({ "phase": "art_done" }));
            });
        }

        Ok(SyncPlaylistResult {
            added,
            removed,
            errors,
        })
    })
    .await;
    crate::remote::commands::after_local_library_change(&app);
    result
}

#[tauri::command]
pub fn is_folder_setup_dismissed(
    state: tauri::State<'_, AppSettingsState>,
) -> Result<bool, String> {
    Ok(lock_settings(&state)?.folder_setup_dismissed)
}

#[tauri::command]
pub fn dismiss_folder_setup(
    state: tauri::State<'_, AppSettingsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut settings = lock_settings(&state)?;
    settings.folder_setup_dismissed = true;
    settings.save(&app)?;
    Ok(())
}

/// Remove all cached audio imports from app-private storage.
/// Frees up space used by materialized content:// URI copies.
#[tauri::command]
pub fn clear_audio_imports(app: tauri::AppHandle) -> Result<u64, String> {
    let imports_dir = crate::app_paths::app_data_dir(&app)
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?
        .join("imports");

    if !imports_dir.exists() {
        return Ok(0);
    }

    let mut freed = 0u64;
    for entry in std::fs::read_dir(&imports_dir)
        .map_err(|e| format!("Failed to read imports dir: {e}"))?
        .filter_map(Result::ok)
    {
        if let Ok(metadata) = entry.metadata() {
            freed += metadata.len();
        }
        let _ = std::fs::remove_file(entry.path());
    }

    Ok(freed)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResetAppResult {
    pub tracks_removed: u32,
    pub playlists_deleted: u32,
}

/// Wipe library data, media folder prefs, and cached cover art / imports.
/// Keeps empty Library + Favorites playlists. Does not delete audio files on disk.
#[tauri::command]
pub async fn reset_app(app: tauri::AppHandle) -> Result<ResetAppResult, String> {
    // Stop playback first so the UI isn't left pointing at deleted tracks.
    {
        let state = app.state::<PlayerState>();
        let mut slot = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(player) = slot.as_mut() {
            let _ = player.stop();
            // Emptied in place: shuffle and song couples are settings, and
            // the reset leaves those alone.
            player.queue.set_tracks(Vec::new());
        }
    }
    {
        let bridge = app.state::<MediaBridgeState>();
        bridge.0.set_stopped();
    }

    let (tracks_removed, playlists_deleted) = {
        let library = app.state::<LibraryState>();
        let library = library.0.lock().map_err(|e| e.to_string())?;
        library.reset_library()?
    };

    {
        let settings_state = app.state::<AppSettingsState>();
        let mut settings = lock_settings(&settings_state)?;
        settings.media_folders.clear();
        settings.folder_setup_dismissed = false;
        settings.save(&app)?;
    }

    // Best-effort cleanup of on-disk caches.
    if let Ok(app_dir) = crate::app_paths::app_data_dir(&app) {
        let cover_dir = app_dir.join("cover_art");
        if cover_dir.is_dir() {
            let _ = std::fs::remove_dir_all(&cover_dir);
            let _ = std::fs::create_dir_all(&cover_dir);
        }
    }
    let _ = clear_audio_imports(app.clone());

    Ok(ResetAppResult {
        tracks_removed,
        playlists_deleted,
    })
}

// ── Listen stats / recommendations ───────────────────────────────────────────

#[tauri::command]
pub async fn get_recently_played(
    limit: Option<u32>,
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<Track>, String> {
    lock_library(&library)?.get_recently_played(limit.unwrap_or(100))
}

#[tauri::command]
pub async fn get_most_played(
    limit: Option<u32>,
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<Track>, String> {
    lock_library(&library)?.get_most_played(limit.unwrap_or(100))
}

#[tauri::command]
pub async fn get_favorite_track(
    library: tauri::State<'_, LibraryState>,
) -> Result<Option<Track>, String> {
    lock_library(&library)?.get_favorite_track()
}

#[tauri::command]
pub async fn get_favorite_album(
    library: tauri::State<'_, LibraryState>,
) -> Result<Option<AlbumSummaryDto>, String> {
    lock_library(&library)?.get_favorite_album()
}

#[tauri::command]
pub async fn get_favorite_artist(
    library: tauri::State<'_, LibraryState>,
) -> Result<Option<ArtistSummaryDto>, String> {
    lock_library(&library)?.get_favorite_artist()
}

#[tauri::command]
pub async fn get_listening_stats(
    limit: Option<u32>,
    library: tauri::State<'_, LibraryState>,
) -> Result<ListeningStatsDto, String> {
    lock_library(&library)?.get_listening_stats(limit.unwrap_or(5))
}

#[tauri::command]
pub async fn get_home_suggestions(app: tauri::AppHandle) -> Result<HomeSuggestionsDto, String> {
    let seed = {
        let state = app.state::<PlayerState>();
        let guard = lock_player_state(&state);
        guard.as_ref().and_then(|p| {
            p.get_current_path()
                .map(|p| p.to_string_lossy().into_owned())
        })
    };
    let suggestions = {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(lock_poisoned)?;
        lib.get_home_suggestions(seed.as_deref())?
    };

    maybe_spawn_artist_enrichment(&app);

    Ok(suggestions)
}

/// Kick off a background genre/similar-artist enrichment pass (see
/// `enrichment.rs`) if one isn't already running. Fire-and-forget: the
/// suggestions already returned above used whatever was cached going in,
/// and this only affects what a *future* request will see. Skips entirely
/// once the cache is warm, so steady-state Home visits make zero network
/// calls.
fn maybe_spawn_artist_enrichment(app: &tauri::AppHandle) {
    let enrichment_state = app.state::<EnrichmentState>();
    if enrichment_state
        .0
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }

    let app_clone = app.clone();
    std::thread::spawn(move || {
        run_artist_enrichment_job(&app_clone);
        app_clone
            .state::<EnrichmentState>()
            .0
            .store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// Refreshes cached genre tags / similar artists for whichever top artists
/// are missing or stale. Every network call lives here, off the command
/// path. The library lock is only ever held for the brief local reads and
/// writes around each call, never across a network request, so playback,
/// scanning, and metadata lookups are never blocked by this.
fn run_artist_enrichment_job(app: &tauri::AppHandle) {
    let stale = {
        let library = app.state::<LibraryState>();
        let Ok(lib) = library.0.lock() else {
            return;
        };
        let seed_artists = match lib.diversity_seed_artists() {
            Ok(names) => names,
            Err(e) => {
                tracing::warn!("Failed to pick diversity seed artists: {e}");
                return;
            }
        };
        match lib.artists_needing_enrichment(&seed_artists) {
            Ok(names) => names,
            Err(e) => {
                tracing::warn!("Failed to check artist enrichment cache: {e}");
                return;
            }
        }
    };

    if stale.is_empty() {
        return;
    }

    let client = crate::enrichment::enrichment_client();
    for (index, artist_name) in stale.iter().enumerate() {
        if index > 0 {
            std::thread::sleep(crate::enrichment::RATE_LIMIT_DELAY);
        }
        let profile = crate::enrichment::fetch_artist_profile(client, artist_name);
        let status = if profile.mbid.is_some() {
            "ok"
        } else {
            "not_found"
        };

        let library = app.state::<LibraryState>();
        let save_result = library.0.lock().map(|lib| {
            lib.save_artist_enrichment(
                artist_name,
                profile.mbid.as_deref(),
                &profile.tags,
                &profile.similar,
                status,
            )
        });
        if let Ok(Err(e)) = save_result {
            tracing::warn!("Failed to save artist enrichment for {artist_name}: {e}");
        }
    }
}

// ── Remote song sourcing ──────────────────────────────────────────────────────
//
// The third search tier. Every command here does blocking network work, so each
// runs on `spawn_blocking`, never on the async command path. Failures degrade:
// a provider that is down yields an errored section, not a failed search.

use crate::remote::search as search_stream;
use crate::remote::uri::RemoteUri;
use crate::sources::{self, cache as source_cache, download as source_download, SourceTrack};

/// Providers currently configured, read from settings.
fn source_config(app: &tauri::AppHandle) -> sources::SourceConfig {
    let jamendo_client_id = app
        .try_state::<AppSettingsState>()
        .and_then(|state| state.0.lock().ok().map(|s| s.jamendo_client_id.clone()))
        .flatten();
    sources::SourceConfig { jamendo_client_id }
}

/// Tier 3: search every remote provider concurrently.
///
/// Results are annotated with the local path of any track the user already
/// owns, so the UI can mark a hit as "in your library" and play the local copy
/// rather than re-fetching something already on disk.
#[tauri::command]
pub async fn search_sources(
    query: String,
    limit: Option<u32>,
    app: tauri::AppHandle,
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<sources::ProviderResults>, String> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    // Fail closed: if settings can't be read, assume the user has not opted in.
    if !app
        .try_state::<AppSettingsState>()
        .and_then(|state| state.0.lock().ok().map(|s| s.outside_sourcing_enabled))
        .unwrap_or(false)
    {
        // Master switch is off: no provider is contacted at all.
        return Ok(Vec::new());
    }
    let capped = limit.unwrap_or(20).min(50) as usize;
    let config = source_config(&app);

    let mut results =
        tokio::task::spawn_blocking(move || sources::search_all(&config, &query, capped))
            .await
            .map_err(|e| format!("Source search failed: {e}"))?;

    // Annotate against the library while we hold the lock exactly once.
    {
        let library = lock_library(&library)?;
        for section in &mut results {
            for track in &mut section.tracks {
                track.already_in_library = library
                    .find_local_match(&track.title, &track.artist)
                    .unwrap_or(None);
            }
        }
    }
    Ok(results)
}

/// Build the library row for a freshly fetched remote file.
fn source_track_to_track(
    source: &SourceTrack,
    path: &str,
    album_art_id: Option<String>,
    file_size: i64,
) -> Track {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    Track {
        // Fresh, as for a local file; re-streaming the same result keeps the
        // id it was first stored with, since the upsert matches on path.
        id: uuid::Uuid::new_v4().to_string(),
        path: path.to_string(),
        name: Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.title.clone()),
        title: source.title.clone(),
        artist: source.artist.clone(),
        album: source
            .album
            .clone()
            .unwrap_or_else(|| "Singles".to_string()),
        album_artist: None,
        genre: None,
        year: None,
        track_number: None,
        disc_number: None,
        format: source.extension(),
        duration_seconds: source.duration_seconds,
        sample_rate: None,
        channels: None,
        bit_depth: None,
        lyrics: None,
        lyrics_source: None,
        has_synced_lyrics: false,
        cover_art_data_url: None,
        cover_art_mime: album_art_id.as_ref().map(|_| "image/jpeg".to_string()),
        cover_art_source: Some(source.provider.clone()),
        album_art_id,
        fingerprint_sha256: None,
        acoustid_fingerprint: None,
        musicbrainz_recording_id: None,
        file_size,
        modified_at: now,
        indexed_at: now,
        is_saf_uri: false,
        source_provider: Some(source.provider.clone()),
        source_state: None,
        remote_source_id: None,
        cache_state: None,
        remote_missing: false,
    }
}

/// Fetch provider artwork into the shared album-art store. Best-effort: a
/// missing or broken image costs the cover, never the track.
fn fetch_source_artwork(app: &tauri::AppHandle, source: &SourceTrack) -> Option<String> {
    let url = source.artwork_url.as_deref()?;
    let bytes = sources::source_client()
        .get(url)
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .ok()?;
    let saved = crate::cover_art::save_album_art_thumb(
        app,
        crate::cover_art::ExtractedCoverArt {
            data: bytes.to_vec(),
        },
    )
    .ok()?;
    Some(saved.id)
}

/// Trim the stream cache down to `cap_bytes`, or the configured cap when
/// `None`. Returns the bytes freed.
///
/// `protected` is the currently playing track plus the live queue. Deleting
/// one of those would pull a file out from under an open decoder. Remote
/// tracks share the cap: evicting one deletes its cache file but keeps its row.
pub(crate) fn evict_source_cache_to(
    app: &tauri::AppHandle,
    library: &Library,
    mut protected: Vec<String>,
    cap_bytes: Option<u64>,
) -> u64 {
    use crate::remote::stream::{active_paths, remote_cache_files_for};
    use crate::remote::uri::RemoteUri;

    let (cap_mb, remote_enabled) = app
        .try_state::<AppSettingsState>()
        .and_then(|state| {
            state
                .0
                .lock()
                .ok()
                .map(|s| (s.source_cache_limit_mb, s.remote_sources_enabled))
        })
        .unwrap_or((512, false));
    let Ok(mut candidates) = library.cached_source_tracks() else {
        return 0;
    };

    // Remote candidates are keyed by their URI, which no track id can be.
    let remote_rows = library.remote_eviction_candidates().unwrap_or_else(|e| {
        tracing::warn!("Could not list cached remote tracks: {e}");
        Vec::new()
    });
    let mut remote: std::collections::HashMap<String, (RemoteUri, String, String)> =
        std::collections::HashMap::new();
    for (uri, extension, fetched_at) in remote_rows {
        let [file, _] = remote_cache_files_for(&uri, &extension);
        let file = file.to_string_lossy().into_owned();
        let key = uri.to_path();
        candidates.push(source_cache::EvictionCandidate {
            track_id: key.clone(),
            path: file.clone(),
            fetched_at,
        });
        remote.insert(key, (uri, extension, file));
    }
    let queued_files: Vec<String> = protected
        .iter()
        .filter_map(|path| RemoteUri::parse(path))
        .filter_map(|uri| remote.get(&uri.to_path()).map(|(_, _, file)| file.clone()))
        .collect();
    protected.extend(queued_files);
    protected.extend(
        active_paths()
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned()),
    );

    let sizes = |path: &str| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let doomed = source_cache::plan_eviction(
        &candidates,
        &sizes,
        cap_bytes.unwrap_or_else(|| cap_mb.saturating_mul(1024 * 1024)),
        &protected,
    );
    let freed = doomed.iter().map(|c| sizes(&c.path)).sum();
    let mut evicted_remote = false;
    for candidate in doomed {
        let Some((uri, extension, _)) = remote.get(&candidate.track_id) else {
            let _ = library.forget_cached_source_track(&candidate.track_id);
            continue;
        };
        for file in remote_cache_files_for(uri, extension) {
            if let Err(e) = std::fs::remove_file(&file) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("Could not delete {}: {e}", file.display());
                }
            }
        }
        if let Err(e) = library.set_remote_cache_state(uri, None) {
            tracing::warn!("Could not forget the cache of {}: {e}", uri.to_path());
        }
        evicted_remote = true;
    }
    if evicted_remote {
        if let Err(e) = library.recompute_remote_visibility(remote_enabled) {
            tracing::warn!("Could not update which remote tracks show: {e}");
        }
    }
    freed
}

/// [`protected_cache_paths`] from an app handle.
pub(crate) fn queued_paths(app: &tauri::AppHandle) -> Vec<String> {
    app.try_state::<PlayerState>()
        .map(|player| protected_cache_paths(&player))
        .unwrap_or_default()
}

/// Paths that must survive eviction: what is playing, and what is queued.
fn protected_cache_paths(player: &tauri::State<'_, PlayerState>) -> Vec<String> {
    let guard = lock_player_state(player);
    let Some(player) = guard.as_ref() else {
        return Vec::new();
    };
    let mut paths = player.queue.tracks().to_vec();
    // A track started on its own, outside the queue, plays all the same.
    if let Some(current) = player.get_current_path() {
        paths.push(current.to_string_lossy().into_owned());
    }
    paths
}

/// Registers a full-length result as a hidden row of the search source. Only
/// resolving its audio URL touches the network; the bytes come when it plays.
async fn register_streamed_source_track(
    track: SourceTrack,
    app: tauri::AppHandle,
) -> Result<Track, String> {
    crate::remote::gate::NetworkGate::search()
        .check()
        .map_err(|e| e.to_string())?;
    let config = source_config(&app);
    tokio::task::spawn_blocking(move || {
        let provider = sources::provider_by_id(&config, &track.provider)
            .ok_or_else(|| format!("Unknown source: {}", track.provider))?;
        let audio_url = provider
            .resolve_audio(sources::source_client(), &track)
            .map_err(|e| e.to_string())?;
        let mut resolved = track.clone();
        resolved.audio_url = Some(audio_url.clone());

        let remote_id = search_stream::remote_id(&track.provider, &track.id);
        let path = RemoteUri::new(search_stream::SEARCH_SOURCE, &remote_id).to_path();
        let art_id = fetch_source_artwork(&app, &track);
        let mut row = source_track_to_track(&resolved, &path, art_id, 0);
        row.name = format!("{}.{}", track.title, resolved.extension());

        let state = app.state::<LibraryState>();
        let library = state.0.lock().map_err(lock_poisoned)?;
        library.upsert_streamed_source_track(
            &row,
            &track.provider,
            &remote_id,
            &search_stream::part_key(&track.provider, &audio_url),
        )
    })
    .await
    .map_err(|e| format!("Stream failed: {e}"))?
}

/// Stream a remote result. A preview is fetched whole into the session's
/// preview cache. A full-length track becomes a hidden row of the built-in
/// search source and plays progressively through the remote stream cache, so
/// it starts as soon as the first bytes arrive; nothing is fetched here.
#[tauri::command]
pub async fn stream_source_track(
    track: SourceTrack,
    app: tauri::AppHandle,
    library: tauri::State<'_, LibraryState>,
) -> Result<Track, String> {
    if track.is_full_length {
        let existing = {
            let library = lock_library(&library)?;
            let remote_id = search_stream::remote_id(&track.provider, &track.id);
            // A streamed row resumes from whatever its cache holds. Rows from
            // before streaming point at a whole file in the cache.
            match library.find_source_track(&track.provider, &remote_id)? {
                Some(streamed) => Some(streamed),
                None => library
                    .find_source_track(&track.provider, &track.id)?
                    .filter(|old| Path::new(&old.path).is_file()),
            }
        };
        return match existing {
            Some(existing) => Ok(existing),
            None => register_streamed_source_track(track, app).await,
        };
    }

    // A preview is not something the user chose to own, so it never becomes a
    // library row. Reuse the cached clip if it is still on disk.
    let existing = preview_registry().lock().ok().and_then(|map| {
        map.values()
            .find(|t| {
                t.source_provider.as_deref() == Some(track.provider.as_str())
                    && Path::new(&t.path).is_file()
                    && t.path
                        == source_cache::preview_path(
                            &track.provider,
                            &track.id,
                            &track.extension(),
                        )
                        .to_string_lossy()
            })
            .cloned()
    });
    if let Some(existing) = existing {
        return Ok(existing);
    }

    let config = source_config(&app);
    let app_for_fetch = app.clone();
    let source = track.clone();
    let (path, size, art_id) = tokio::task::spawn_blocking(move || {
        let provider = sources::provider_by_id(&config, &source.provider)
            .ok_or_else(|| format!("Unknown source: {}", source.provider))?;
        let client = sources::source_client();
        let audio_url = provider
            .resolve_audio(client, &source)
            .map_err(|e| e.to_string())?;

        let mut resolved = source.clone();
        resolved.audio_url = Some(audio_url.clone());
        let dest = source_cache::preview_path(&source.provider, &source.id, &resolved.extension());
        let size = source_cache::fetch_to(client, &audio_url, provider.fetch_referer(), &dest)
            .map_err(|e| e.to_string())?;
        let art_id = fetch_source_artwork(&app_for_fetch, &source);
        Ok::<_, String>((dest, size as i64, art_id))
    })
    .await
    .map_err(|e| format!("Stream failed: {e}"))??;

    // Playable, displayable, and entirely absent from the library.
    let path_string = path.to_string_lossy().into_owned();
    let mut row = source_track_to_track(&track, &path_string, art_id, size);
    row.id = format!("preview:{}:{}", track.provider, track.id);
    row.source_state = Some("preview".to_string());
    register_preview(&row);
    Ok(row)
}

/// Keep a remote track: copy it into the download destination and promote its
/// row into the library proper.
///
/// Copies rather than moves; see `sources::download` for why moving would
/// break a track that is playing from the cache on Windows.
#[tauri::command]
pub async fn download_source_track(
    track: SourceTrack,
    app: tauri::AppHandle,
    library: tauri::State<'_, LibraryState>,
    settings_state: tauri::State<'_, AppSettingsState>,
) -> Result<Track, String> {
    if !track.downloadable {
        return Err(format!(
            "{} does not allow saving this track; it can only be streamed",
            track.provider
        ));
    }

    // Make sure the bytes exist locally, reusing the cached copy when present.
    let cached = stream_source_track(track.clone(), app.clone(), library.clone()).await?;
    // A streamed row waits for its cache to finish, then is kept the way a
    // remote library track is.
    if crate::remote::uri::is_remote(&cached.path) {
        return crate::remote::commands::download_remote_track(cached.path, app).await;
    }

    let media_folders = lock_settings(&settings_state)?.media_folders.clone();
    let destination = source_download::choose_destination(
        &track,
        &media_folders,
        &|root| {
            std::fs::create_dir_all(root).is_ok()
                && !std::fs::metadata(root)
                    .map(|m| m.permissions().readonly())
                    .unwrap_or(true)
        },
        &|path| path.exists(),
    );

    let source_path = cached.path.clone();
    let dest_path = destination.path.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create download folder: {e}"))?;
        }
        std::fs::copy(&source_path, &dest_path).map_err(|e| format!("Download failed: {e}"))?;
        Ok::<_, String>(())
    })
    .await
    .map_err(|e| format!("Download failed: {e}"))??;

    // Register the destination so the file is browsable without a rescan.
    {
        let mut settings = lock_settings(&settings_state)?;
        let folder = destination.media_folder.to_string_lossy().into_owned();
        if !settings.media_folders.contains(&folder) {
            settings.media_folders.push(folder);
            settings.save(&app)?;
        }
    }

    let stored = {
        let library = lock_library(&library)?;
        library.promote_source_track(&cached.id, &destination.path.to_string_lossy())?
    };

    if let Some(reason) = destination.fallback_reason {
        let _ = app.emit("source-download-fallback", reason);
    }
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_streamed_result_gets_a_row_id_of_its_own() {
        // An empty id made the second full-length result streamed in a
        // session fail with a UNIQUE constraint on `tracks.id`.
        let source = SourceTrack {
            provider: "audius".into(),
            id: "YmJWK".into(),
            title: "Late Night".into(),
            artist: "kirby".into(),
            album: None,
            duration_seconds: Some(190.0),
            artwork_url: None,
            audio_url: Some("https://api.audius.co/v1/tracks/YmJWK/stream".into()),
            is_full_length: true,
            downloadable: false,
            attribution: None,
            already_in_library: None,
        };
        let a = source_track_to_track(&source, "wave-remote://search/audius%3Aa", None, 0);
        let b = source_track_to_track(&source, "wave-remote://search/audius%3Ab", None, 0);
        assert!(!a.id.is_empty());
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn remote_tracks_restore_only_from_a_finished_cache() {
        // Without a finished cache file, restoring would wait on the network
        // while the app starts.
        assert!(!path_is_restorable("wave-remote://s1/1"));
        assert!(!path_is_restorable("/definitely/not/here.flac"));
    }

    #[test]
    fn a_remote_track_makes_an_edit_unwritable() {
        let always = |_: &str| true;
        assert!(paths_writable(&["/music/a.flac".to_string()], always));
        assert!(!paths_writable(
            &[
                "/music/a.flac".to_string(),
                "wave-remote://s1/1".to_string()
            ],
            always
        ));
        assert!(!paths_writable(
            &["content://doc/1".to_string()],
            |_: &str| false
        ));
    }

    #[test]
    fn remote_tracks_are_never_editable() {
        assert_eq!(
            crate::remote::ensure_editable("wave-remote://s1/1").unwrap_err(),
            crate::remote::REMOTE_EDIT_REFUSED
        );
        assert!(crate::remote::ensure_editable("/music/a.flac").is_ok());
    }

    #[test]
    fn placeholder_for_a_content_uri_shows_the_file_name() {
        let track = placeholder_track(
            "content://com.android.externalstorage.documents/tree/primary%3AMusic/document/primary%3AMusic%2FTelegram%2FArtist%20-%20Song.mp3",
        );
        assert_eq!(track.name, "Artist - Song.mp3");
        assert_eq!(track.title, "Song");
        assert_eq!(track.artist, "Artist");
        assert_eq!(track.album, "Telegram");
        assert_eq!(track.format, "MP3");
        assert!(track.is_saf_uri);
    }

    fn playing_at(position: f64, duration: Option<f64>, speed: f32) -> PlaybackStateDto {
        PlaybackStateDto {
            is_playing: true,
            is_paused: false,
            current_path: Some("/music/a.mp3".into()),
            position_seconds: position,
            duration_seconds: duration,
            volume: 1.0,
            output_device_name: String::new(),
            sleep_timer: SleepTimerStatus::default(),
            speed,
            is_buffering: false,
        }
    }

    #[test]
    fn a_busy_poll_carries_the_position_forward_at_the_playback_speed() {
        let state = carried_forward(
            playing_at(10.0, Some(200.0), 1.5),
            std::time::Duration::from_secs(2),
        );
        assert!((state.position_seconds - 13.0).abs() < 1e-9);
    }

    #[test]
    fn a_skip_buffers_the_track_a_direct_skip_would_play() {
        use crate::audio::player::RepeatMode;

        // Missing files: playing fails, but only after the queue has moved,
        // so where it moved to says what each skip picked.
        let paths: Vec<String> = (0..4)
            .map(|i| format!("/nonexistent/wave-skip-{i}.flac"))
            .collect();
        for repeat in [RepeatMode::Off, RepeatMode::All] {
            for shuffle in [false, true] {
                for forward in [true, false] {
                    for start in [0, 3] {
                        let case =
                            format!("{repeat:?} shuffle={shuffle} forward={forward} {start}");
                        let mut player = AudioPlayer::new_deferred();
                        player.repeat = repeat.clone();
                        player.queue.set_tracks(paths.clone());
                        player.queue.jump(start);
                        player.set_shuffle(shuffle);

                        let mut twin = AudioPlayer::new_deferred();
                        twin.repeat = repeat.clone();
                        twin.queue = player.queue.clone();
                        let before = twin.queue.current_index();
                        let _ = if forward {
                            twin.play_next()
                        } else {
                            twin.play_previous()
                        };
                        let after = twin.queue.current_index();
                        let expected = (after != before)
                            .then(|| after.map(|i| twin.queue.tracks()[i].clone()))
                            .flatten();

                        let player = Mutex::new(player);
                        let mut prepared = None;
                        let _ = skip_via(
                            |step| step(&mut player.lock().unwrap()),
                            |path| {
                                prepared = Some(path.to_string());
                                Ok(())
                            },
                            forward,
                        );
                        let player = player.into_inner().unwrap();
                        assert_eq!(prepared, expected, "{case}");
                        assert_eq!(player.queue.current_index(), after, "{case}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_skip_that_cannot_buffer_does_not_move() {
        let mut player = AudioPlayer::new_deferred();
        player
            .queue
            .set_tracks(vec!["/music/a.flac".into(), "wave-remote://s/1".into()]);
        player.queue.jump(0);
        let player = Mutex::new(player);
        let result = skip_via(
            |step| step(&mut player.lock().unwrap()),
            |_| Err("Playback failed: offline".to_string()),
            true,
        );
        assert_eq!(result, Err("Playback failed: offline".to_string()));
        assert_eq!(player.into_inner().unwrap().queue.current_index(), Some(0));
    }

    #[test]
    fn a_busy_poll_holds_the_position_while_buffering() {
        let mut buffering = playing_at(10.0, Some(200.0), 1.0);
        buffering.is_buffering = true;
        let state = carried_forward(buffering, std::time::Duration::from_secs(2));
        assert_eq!(state.position_seconds, 10.0);
    }

    #[test]
    fn a_busy_poll_stops_at_the_end_of_the_track() {
        let state = carried_forward(
            playing_at(199.0, Some(200.0), 1.0),
            std::time::Duration::from_secs(5),
        );
        assert_eq!(state.position_seconds, 200.0);
    }

    #[test]
    fn a_busy_poll_leaves_a_paused_position_alone() {
        let mut paused = playing_at(42.0, Some(200.0), 1.0);
        paused.is_playing = false;
        paused.is_paused = true;
        let state = carried_forward(paused, std::time::Duration::from_secs(5));
        assert_eq!(state.position_seconds, 42.0);
    }

    #[test]
    fn placeholder_for_a_file_guesses_from_its_name() {
        let track = placeholder_track("/music/Album/Song.flac");
        assert_eq!(track.name, "Song.flac");
        assert_eq!(track.title, "Song");
        assert_eq!(track.album, "Album");
        assert_eq!(track.format, "FLAC");
        assert!(!track.is_saf_uri);
    }
}
