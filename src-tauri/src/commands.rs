use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app_settings::{AppSettings, AppSettingsState};
use crate::audio::player::AudioPlayer;
use crate::dto::{
    AlbumSummaryDto, ArtistSummaryDto, CloseAction, EqSettingsDto, HomeSuggestionsDto,
    ImportResultDto, ListeningStatsDto, PlaybackModeDto, PlaybackStateDto, QueueDto,
    QueueStateDto, SearchHitDto,
};
use crate::library::{Library, PlaylistInfo};
use crate::listen::{ListenEndReason, ListenFlush, ListenTracker};
use crate::media_controls::TrackMetadata;
use crate::metadata::{enrich_lyrics_online, is_supported_audio_file, supported_audio_extensions, Track};
use crate::path_validation::{validate_audio_path, validate_safe_output_path};
use tauri::{Emitter, Manager};
use walkdir::WalkDir;

/// Lazily-initialized audio engine. Creation is deferred until first use so
/// Android can finish wiring JNI / ndk_context before cpal/oboe opens a stream.
pub struct PlayerState(pub Mutex<Option<AudioPlayer>>);
pub struct LibraryState(pub Mutex<Library>);
pub struct MediaBridgeState(pub crate::media_controls::MediaBridgeState);
pub struct ListenState(pub Mutex<ListenTracker>);

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
        self.0.as_ref().expect("player must be initialized before deref")
    }
}

impl DerefMut for PlayerGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
            .as_mut()
            .expect("player must be initialized before deref_mut")
    }
}

fn lock_player<'a>(
    state: &'a tauri::State<'a, PlayerState>,
) -> Result<PlayerGuard<'a>, String> {
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
    settings.shuffle = player.queue.is_shuffled();
    settings.repeat = player.repeat.clone();
    let _ = settings.save(app);
}

fn apply_listen_flush(app: &tauri::AppHandle, flush: ListenFlush) {
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
                // Cold start (restore / external play) — use engine path for both.
                tracker.start(path.clone(), path.clone(), duration, None);
                drop(tracker);
                touch_recently_played(app, &path);
            }
        }
        Some(_) => {
            // Right after listen_switch_track the engine may still report the
            // previous file for a tick or two — don't treat that as a new song.
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
        (player_path, player.position_seconds(), player.duration_seconds())
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
    // get_current_path is usually already the *new* file at ~0s — applying
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
        return;
    }

    let track = match app.state::<LibraryState>().0.lock() {
        Ok(lib) => Some(resolve_track(&lib, &path)),
        Err(_) => None,
    };
    if let Some(track) = track {
        let bridge = app.state::<MediaBridgeState>();
        sync_bridge_now_playing_at(app, &track, position);
        bridge.0.set_paused(position);
    }
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

fn sync_queue_from_tracks(player: &mut AudioPlayer, tracks: &[Track], index: usize) {
    let new_paths: Vec<String> = tracks.iter().map(|track| track.path.clone()).collect();
    let old_paths: Vec<String> = player.queue.tracks().to_vec();

    // Preserve any manually-added queue items (those not in the new playlist).
    let manual: Vec<String> = old_paths
        .into_iter()
        .filter(|p| !new_paths.contains(p))
        .collect();

    player.queue.set_tracks(new_paths);
    if player.queue.jump(index).is_none() {
        tracing::warn!("Failed to align playback queue with playlist index {index}");
    }
    // Re-append manual items so they play after the playlist finishes.
    for path in manual {
        player.queue.enqueue(path);
    }
}

/// Build a minimal `Track` for a path that isn't in the library (e.g. a file
/// that was deleted or moved after being added to the queue).
fn placeholder_track(path: &str) -> Track {
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown")
        .to_string();
    Track {
        id: String::new(),
        path: path.to_string(),
        name: name.clone(),
        title: name,
        artist: "Unknown Artist".to_string(),
        album: "Local Files".to_string(),
        album_artist: None,
        genre: None,
        year: None,
        track_number: None,
        disc_number: None,
        format: Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("Audio")
            .to_uppercase(),
        duration_seconds: None,
        sample_rate: None,
        channels: None,
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
        file_size: 0,
        modified_at: 0,
        indexed_at: 0,
        is_saf_uri: false,
    }
}

/// Look up a track by path in the library, falling back to a placeholder.
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
        if let Ok(Some(full)) =
            crate::metadata::extract_full_cover_data_url(Some(app), &track.path)
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
/// the queue when a track ends — without relying on frontend polling alone.
pub(crate) fn tick_auto_advance(app: &tauri::AppHandle) {
    // Crossfade handoff can happen while the sink is still playing — check
    // independently of should_auto_advance so UI/queue/media stay in sync.
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
        if !player.should_auto_advance() {
            return;
        }

        // Skip past unreadable files instead of stopping — a single bad track
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

/// Apply a media-session action from the Android native JNI bridge.
/// Used when the WebView is frozen in the background and JS handlers cannot run.
#[cfg(target_os = "android")]
pub(crate) fn handle_native_media_action(app: &tauri::AppHandle, action: &str) -> Result<(), String> {
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
            let position = with_app_player(app, |player| Ok(player.position_seconds())).unwrap_or(0.0);
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
            let position = with_app_player(app, |player| Ok(player.position_seconds())).unwrap_or(0.0);
            app.state::<MediaBridgeState>().0.set_paused(position);
            let position = with_app_player(app, |player| {
                let position = player.position_seconds();
                player.pause().map_err(|e| e.to_string())?;
                Ok(position)
            })?;
            app.state::<MediaBridgeState>().0.set_paused(position);
            Ok(())
        }
        "stop" => {
            with_app_player(app, |player| player.stop().map_err(|e| e.to_string()))?;
            app.state::<MediaBridgeState>().0.set_stopped();
            Ok(())
        }
        "next" => {
            let path = with_app_player(app, |player| {
                player.play_next().map_err(|e| e.to_string())
            })?;
            if let Some(path) = path {
                let track = match app.state::<LibraryState>().0.lock() {
                    Ok(lib) => resolve_track(&lib, &path),
                    Err(_) => placeholder_track(&path),
                };
                listen_switch_track(app, &track.path, ListenEndReason::Skipped);
                sync_bridge_now_playing(app, &track);
            }
            Ok(())
        }
        "previous" => {
            let path = with_app_player(app, |player| {
                player.play_previous().map_err(|e| e.to_string())
            })?;
            if let Some(path) = path {
                let track = match app.state::<LibraryState>().0.lock() {
                    Ok(lib) => resolve_track(&lib, &path),
                    Err(_) => placeholder_track(&path),
                };
                listen_switch_track(app, &track.path, ListenEndReason::Skipped);
                sync_bridge_now_playing(app, &track);
            }
            Ok(())
        }
        "shuffle" => {
            with_app_player(app, |player| {
                let next = !player.queue.is_shuffled();
                player.queue.set_shuffle(next);
                Ok(())
            })?;
            let bridge = app.state::<MediaBridgeState>();
            sync_bridge_playback_mode(app, &bridge);
            Ok(())
        }
        "repeat" => {
            with_app_player(app, |player| {
                player.repeat = match player.repeat {
                    RepeatMode::Off => RepeatMode::All,
                    RepeatMode::All => RepeatMode::One,
                    RepeatMode::One => RepeatMode::Off,
                };
                Ok(())
            })?;
            let bridge = app.state::<MediaBridgeState>();
            sync_bridge_playback_mode(app, &bridge);
            Ok(())
        }
        other => {
            tracing::debug!("Ignoring unknown Android media action: {other}");
            Ok(())
        }
    }
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
    bridge.0.set_playback_mode(shuffle, repeat_mode_str(&repeat).to_string());
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

/// Exit the process (Android double-back-to-exit confirm).
#[tauri::command]
pub fn exit_app(app: tauri::AppHandle) {
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
    // Block off the async runtime — the JNI side waits on the system picker.
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
pub async fn scan_saf_folder(
    uri: String,
    app: tauri::AppHandle,
) -> Result<Vec<String>, String> {
    blocking(move || crate::android::saf_scan::list_audio_files(&app, &uri)).await
}

// ── Playback commands ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn play_track(
    path: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
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
        with_app_player(&app_clone, |player| {
            player.play(&play_path).map_err(|e| format!("Playback failed: {e}"))
        })
    })
    .await?;

    let app_clone = app.clone();
    let lookup_a = original_path;
    let lookup_b = local_path;
    let track = blocking(move || {
        let lib = app_clone.state::<LibraryState>();
        let lib = lib.0.lock().map_err(|e| e.to_string())?;
        match lib.get_tracks_by_paths(&[lookup_a.clone()]) {
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

    let play_path = local_path.clone();
    let queue_paths = materialized.clone();
    let app_clone = app.clone();
    blocking(move || {
        with_app_player(&app_clone, |player| {
            player.queue.set_tracks(queue_paths);
            if player.queue.jump(index).is_none() {
                tracing::warn!("Failed to align queue with play_tracks index {index}");
            }
            player
                .play(&play_path)
                .map_err(|e| format!("Playback failed: {e}"))
        })
    })
    .await?;

    let app_clone = app.clone();
    let lookup = local_path.clone();
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

#[tauri::command]
pub async fn get_playback_state(
    state: tauri::State<'_, PlayerState>,
) -> Result<PlaybackStateDto, String> {
    let guard = lock_player_state(&state);
    let Some(player) = guard.as_ref() else {
        return Ok(PlaybackStateDto {
            is_playing: false,
            is_paused: false,
            current_path: None,
            position_seconds: 0.0,
            duration_seconds: None,
            volume: 0.8,
            output_device_name: AudioPlayer::current_output_name(),
        });
    };
    Ok(PlaybackStateDto {
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
    })
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
        if gain.abs() > 24.0 {
            return Err(format!("EQ band {index} gain must be between -24 and +24 dB"));
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
pub async fn get_crossfade_duration(
    state: tauri::State<'_, PlayerState>,
) -> Result<f32, String> {
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

// ── Library / playlist commands ───────────────────────────────────────────────

#[tauri::command]
pub async fn add_track_to_playlist(
    path: String,
    app: tauri::AppHandle,
) -> Result<Track, String> {
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
pub async fn get_playlist(
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<Track>, String> {
    lock_library(&library)?.get_default_playlist_tracks()
}

#[tauri::command]
pub async fn clear_playlist(
    library: tauri::State<'_, LibraryState>,
) -> Result<(), String> {
    lock_library(&library)?.clear_default_playlist()
}

// ── Favorites ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn add_track_to_favorites(
    path: String,
    app: tauri::AppHandle,
) -> Result<Track, String> {
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
pub async fn get_favorites(
    library: tauri::State<'_, LibraryState>,
) -> Result<Vec<Track>, String> {
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
pub async fn toggle_favorite(
    path: String,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let app = app.clone();
    blocking(move || {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        lib.toggle_favorite(&path)
    })
    .await
}

#[tauri::command]
pub async fn clear_favorites(
    library: tauri::State<'_, LibraryState>,
) -> Result<(), String> {
    lock_library(&library)?.clear_favorites()
}

#[tauri::command]
pub async fn play_track_from_playlist(
    index: usize,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let app_clone = app.clone();
    let (raw_tracks, track) = blocking(move || {
        let library = app_clone.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        let tracks = lib.get_default_playlist_tracks()?;
        let track = tracks
            .get(index)
            .ok_or_else(|| format!("Track not found at index {index}"))?
            .clone();
        Ok((tracks, track))
    })
    .await?;

    let app_clone = app.clone();
    let raw_track_paths: Vec<String> = raw_tracks.iter().map(|t| t.path.clone()).collect();
    let materialized_paths = blocking(move || {
        Ok::<_, String>(raw_track_paths
            .into_iter()
            .map(|path| {
                crate::android::import::resolve_playback_source(&app_clone, &path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|e| {
                        tracing::warn!("Failed to resolve track {path}: {e}");
                        path
                    })
            })
            .collect::<Vec<_>>())
    })
    .await?;

    let local_path = materialized_paths
        .get(index)
        .cloned()
        .unwrap_or_default();

    if local_path.is_empty() {
        return Err(format!("Audio file not found for track at index {index}"));
    }

    let tracks: Vec<Track> = raw_tracks
        .into_iter()
        .zip(materialized_paths.into_iter())
        .map(|(mut t, p)| { t.path = p; t })
        .collect();

    let app_clone = app.clone();
    let tracks_clone = tracks.clone();
    blocking(move || {
        with_app_player(&app_clone, |player| {
            sync_queue_from_tracks(player, &tracks_clone, index);
            player.play(&local_path).map_err(|e| format!("Playback failed: {e}"))
        })
    })
    .await?;

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

        let paths: Vec<String> = WalkDir::new(dir_path)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter(|e| is_supported_audio_file(e.path()))
            .filter_map(|e| e.path().to_str().map(str::to_string))
            .collect();

        Ok(paths)
    })
    .await
}

#[tauri::command]
pub async fn index_music_library(
    directory: String,
    profile_id: Option<String>,
    playlist_name: Option<String>,
    app: tauri::AppHandle,
) -> Result<Vec<Track>, String> {
    let app = app.clone();
    blocking(move || {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        lib.index_directory(profile_id, playlist_name, directory)
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
pub async fn get_queue(
    state: tauri::State<'_, PlayerState>,
) -> Result<QueueStateDto, String> {
    let guard = lock_player_state(&state);
    let Some(player) = guard.as_ref() else {
        return Ok(QueueStateDto {
            tracks: Vec::new(),
            current_index: None,
            is_shuffled: false,
        });
    };
    Ok(QueueStateDto {
        tracks: player.queue.tracks().to_vec(),
        current_index: player.queue.current_index(),
        is_shuffled: player.queue.is_shuffled(),
    })
}

#[tauri::command]
pub async fn play_next(
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    let app_clone = app.clone();
    let path = blocking(move || {
        with_app_player(&app_clone, |guard| {
            guard.play_next().map_err(|e| e.to_string())
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
        listen_switch_track(&app, &track.path, ListenEndReason::Skipped);
        sync_bridge_now_playing(&app, &track);
    }

    Ok(path)
}

#[tauri::command]
pub async fn play_previous(
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    let app_clone = app.clone();
    let path = blocking(move || {
        with_app_player(&app_clone, |guard| {
            guard.play_previous().map_err(|e| e.to_string())
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
    lock_player(&state)?.queue.set_shuffle(enabled);
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
    lock_player(&state)?.repeat = repeat;
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
            lib.get_tracks_by_paths(&[resolved.clone()])?
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
        let (_, _) = lib.apply_playlist_sync(&id, &[], &[track.clone()], &[])?;
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
) -> Result<(), String> {
    lock_library(&library)?.remove_track_from_library(&path)
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

/// Extract full embedded cover art as a one-shot data URL (not persisted).
/// Used when the lyrics sidebar needs a large album image.
#[tauri::command]
pub async fn get_track_full_cover(
    path: String,
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    validate_audio_path(&path)?;
    let app_clone = app.clone();
    blocking(move || {
        crate::metadata::extract_full_cover_data_url(Some(&app_clone), &path)
    })
    .await
}

#[tauri::command]
pub async fn fetch_lyrics_for_track(
    path: String,
    app: tauri::AppHandle,
) -> Result<Track, String> {
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
    let raw_tracks = blocking(move || {
        let library = app_clone.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        lib.get_playlist_tracks(&playlist_id)
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

    let play_path = local_path.clone();
    let queue_for_sync = materialized;
    let originals_for_filter = original_paths;
    let app_clone = app.clone();
    blocking(move || {
        with_app_player(&app_clone, |player| {
            let playlist_set: std::collections::HashSet<&str> =
                queue_for_sync.iter().map(String::as_str).collect();
            let original_set: std::collections::HashSet<&str> =
                originals_for_filter.iter().map(String::as_str).collect();
            let manual: Vec<String> = player
                .queue
                .tracks()
                .iter()
                .filter(|p| {
                    !playlist_set.contains(p.as_str()) && !original_set.contains(p.as_str())
                })
                .cloned()
                .collect();

            player.queue.set_tracks(queue_for_sync);
            if player.queue.jump(index).is_none() {
                tracing::warn!("Failed to align playback queue with playlist index {index}");
            }
            for path in manual {
                player.queue.enqueue(path);
            }
            player
                .play(&play_path)
                .map_err(|e| format!("Playback failed: {e}"))
        })
    })
    .await?;

    let mut played_track = track;
    played_track.path = local_path;
    sync_bridge_now_playing(&app, &played_track);
    listen_switch_track(&app, &original_path, ListenEndReason::Partial);
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

#[tauri::command]
pub async fn remove_from_queue(
    index: usize,
    state: tauri::State<'_, PlayerState>,
) -> Result<Option<String>, String> {
    Ok(lock_player(&state)?.remove_from_queue(index))
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
pub async fn clear_queue(
    state: tauri::State<'_, PlayerState>,
) -> Result<(), String> {
    lock_player(&state)?.clear_upcoming();
    Ok(())
}

#[tauri::command]
pub async fn get_queue_tracks(
    state: tauri::State<'_, PlayerState>,
    library: tauri::State<'_, LibraryState>,
) -> Result<QueueDto, String> {
    let (paths, current_index, is_shuffled) = {
        let guard = lock_player_state(&state);
        match guard.as_ref() {
            Some(player) => (
                player.queue.tracks().to_vec(),
                player.queue.current_index(),
                player.queue.is_shuffled(),
            ),
            None => (Vec::new(), None, false),
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
    })
}

#[tauri::command]
pub async fn play_track_from_queue(
    index: usize,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let app_clone = app.clone();
    let path = blocking(move || {
        with_app_player(&app_clone, |guard| {
            guard.jump_to_queue_index(index).map_err(|e| e.to_string())
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
    let app = app.clone();
    let extension = Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let (playlist_id, tracks) = match extension.as_str() {
        "json" => {
            blocking({
                let app = app.clone();
                move || {
                    let library = app.state::<LibraryState>();
                    let lib = library.0.lock().map_err(|e| e.to_string())?;
                    lib.import_playlist_json(&path, name.as_deref())
                }
            })
            .await?
        }
        "m3u" | "m3u8" => {
            let app = app.clone();
            blocking(move || {
                let library = app.state::<LibraryState>();
                let lib = library.0.lock().map_err(|e| e.to_string())?;
                lib.import_playlist_m3u(&path, name.as_deref())
            })
            .await?
        }
        _ => return Err(format!("Unsupported playlist file format: .{extension}")),
    };

    let pid = playlist_id.clone();
    let info = blocking(move || {
        let library = app.state::<LibraryState>();
        let lib = library.0.lock().map_err(|e| e.to_string())?;
        lib.get_playlist_info(&pid)?
            .ok_or_else(|| "Imported playlist not found".to_string())
    })
    .await?;

    Ok(ImportResultDto {
        playlist_id,
        playlist_name: info.name,
        track_count: tracks.len(),
    })
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

    // Save state from the current player before replacing it.
    let was_playing = guard.is_playing();
    let was_paused = guard.is_paused();
    let current_path = guard.get_current_path().and_then(|p| p.to_str().map(String::from));
    let position = guard.position_seconds();
    let volume = guard.volume();
    let queue = std::mem::take(&mut guard.queue);
    let repeat = guard.repeat.clone();
    let eq_config = guard.eq_config.lock().unwrap().clone();
    let eq_version = *guard.eq_version.lock().unwrap();

    // Build a new player on the requested device.
    let mut new_player = AudioPlayer::new_with_device(&device_name)?;
    new_player.queue = queue;
    new_player.repeat = repeat;
    new_player.set_volume(volume)?;
    *new_player.eq_config.lock().unwrap() = eq_config;
    *new_player.eq_version.lock().unwrap() = eq_version;

    // Resume playback if something was playing.
    if let Some(ref path) = current_path {
        if was_playing || was_paused {
            new_player.play(path)?;
            if position > 0.0 {
                new_player.seek(position)?;
            }
            if was_paused {
                new_player.pause()?;
            }
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
/// When the frontend omits `cover_url` (intentional — so the 96px list thumb
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
pub async fn clear_media_session(
    bridge: tauri::State<'_, MediaBridgeState>,
) -> Result<(), String> {
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
pub fn get_close_action(
    state: tauri::State<'_, AppSettingsState>,
) -> Result<CloseAction, String> {
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
    blocking(move || {
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

            let batch_added = {
                let lib = library.0.lock().map_err(|e| e.to_string())?;
                let (added, _) =
                    lib.apply_playlist_sync(&playlist_id, &[], &extracted, &link_ids)?;
                added + link_ids.len() as u32
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
    .await
}

#[derive(serde::Serialize)]
pub struct SyncPlaylistResult {
    pub added: u32,
    pub removed: u32,
    pub errors: Vec<String>,
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
    blocking(move || {
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

        let raw_paths = if let Some(paths) = scanned_paths {
            paths
        } else {
            let dir_path = Path::new(&sync_folder);
            if !dir_path.is_dir() {
                return Err(format!(
                    "Sync folder is missing or not a directory: {sync_folder}"
                ));
            }
            WalkDir::new(dir_path)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
                .filter(|e| is_supported_audio_file(e.path()))
                .filter_map(|e| e.path().to_str().map(str::to_string))
                .collect()
        };

        let mut errors = Vec::new();
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

        for chunk in to_add.chunks(BATCH_SIZE) {
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
                processed += 1;
            }

            let batch_added = {
                let lib = library.0.lock().map_err(|e| e.to_string())?;
                let (a, _) =
                    lib.apply_playlist_sync(&playlist_id_clone, &[], &extracted, &link_ids)?;
                a + link_ids.len() as u32
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
                let _ = enrich_app.emit(
                    "sync-progress",
                    serde_json::json!({ "phase": "art_done" }),
                );
            });
        }

        Ok(SyncPlaylistResult {
            added,
            removed,
            errors,
        })
    })
    .await
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
    let imports_dir = app
        .path()
        .app_data_dir()
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
            player.clear_upcoming();
            player.queue = Default::default();
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
    if let Ok(app_dir) = app.path().app_data_dir() {
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
pub async fn get_home_suggestions(
    app: tauri::AppHandle,
) -> Result<HomeSuggestionsDto, String> {
    let seed = {
        let state = app.state::<PlayerState>();
        let guard = lock_player_state(&state);
        guard
            .as_ref()
            .and_then(|p| p.get_current_path().map(|p| p.to_string_lossy().into_owned()))
    };
    let library = app.state::<LibraryState>();
    let lib = library.0.lock().map_err(lock_poisoned)?;
    lib.get_home_suggestions(seed.as_deref())
}
