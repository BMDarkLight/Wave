// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The app's commands for remote sources: Plex sign-in and setup, the source
//! list, refresh, the shared cache, and keeping a remote track for good.

use super::backend::RemoteError;
use super::gate::{GateState, NetworkGate};
use super::plex::auth::{self, PlexServer};
use super::plex::connection::{self as plex_connection, PlexSourceConfig};
use super::plex::{self, PlexHeaders};
use super::refresh::RefreshCooldown;
use super::store::RemoteSourceRow;
use super::uri::RemoteUri;
use super::{stream, sync, RemoteEvent, RemoteRuntime, SourceState};
use crate::app::settings::AppSettingsState;
use crate::commands::LibraryState;
use crate::metadata::Track;
use crate::sources::download as source_download;
use crate::sources::SourceTrack;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// How long a Plex sign-in waits for the browser before it is forgotten.
const SESSION_TTL: Duration = Duration::from_secs(15 * 60);
/// How long keeping a remote track waits for the rest of its file.
const DOWNLOAD_WAIT: Duration = Duration::from_secs(10 * 60);

struct Session {
    pin_id: i64,
    account_token: Option<String>,
    created: Instant,
}

/// Plex sign-ins in progress, keyed by an id the frontend holds. The account
/// token never leaves the backend.
#[derive(Default)]
pub(crate) struct SignInSessions {
    sessions: HashMap<String, Session>,
}

impl SignInSessions {
    fn insert(&mut self, pin_id: i64, now: Instant) -> String {
        self.purge(now);
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.sessions.insert(
            id.clone(),
            Session {
                pin_id,
                account_token: None,
                created: now,
            },
        );
        id
    }

    fn purge(&mut self, now: Instant) {
        self.sessions
            .retain(|_, s| now.saturating_duration_since(s.created) < SESSION_TTL);
    }

    fn pin(&mut self, id: &str, now: Instant) -> Option<i64> {
        self.purge(now);
        self.sessions.get(id).map(|s| s.pin_id)
    }

    fn token(&mut self, id: &str, now: Instant) -> Option<String> {
        self.purge(now);
        self.sessions.get(id).and_then(|s| s.account_token.clone())
    }

    fn set_token(&mut self, id: &str, token: String) {
        if let Some(session) = self.sessions.get_mut(id) {
            session.account_token = Some(token);
        }
    }

    fn remove(&mut self, id: &str) {
        self.sessions.remove(id);
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

fn sessions() -> std::sync::MutexGuard<'static, SignInSessions> {
    static SESSIONS: OnceLock<Mutex<SignInSessions>> = OnceLock::new();
    SESSIONS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone, Serialize)]
pub struct PlexSignInDto {
    pub session_id: String,
    pub auth_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlexServerDto {
    pub id: String,
    pub name: String,
    pub owned: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlexLibraryDto {
    pub key: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteSourceDto {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub state: SourceState,
    pub track_count: i64,
    pub last_sync_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteSourceStateDto {
    pub id: String,
    pub state: SourceState,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteStatusDto {
    pub gate: GateState,
    pub sources: Vec<RemoteSourceStateDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefreshTicketDto {
    pub started: bool,
    pub retry_after_seconds: u32,
}

/// A stored source as the settings list shows it. The config, which holds
/// tokens, is left out.
fn source_dto(row: &RemoteSourceRow, state: SourceState) -> RemoteSourceDto {
    RemoteSourceDto {
        id: row.id.clone(),
        kind: row.kind.clone(),
        display_name: row.display_name.clone(),
        state,
        track_count: row.track_count,
        last_sync_at: row.last_sync_at,
        last_error: row.last_sync_error.clone(),
    }
}

fn runtime() -> Result<&'static RemoteRuntime, String> {
    RemoteRuntime::global().ok_or_else(|| "Remote music isn't available".to_string())
}

fn remote_enabled(app: &AppHandle) -> bool {
    app.try_state::<AppSettingsState>()
        .and_then(|state| state.0.lock().ok().map(|s| s.remote_sources_enabled))
        .unwrap_or(false)
}

/// The headers every Plex request carries, creating and saving Wave's client
/// id the first time.
fn plex_headers(app: &AppHandle) -> Result<PlexHeaders, String> {
    let state = app.state::<AppSettingsState>();
    let mut settings = state.0.lock().map_err(|e| e.to_string())?;
    let had_id = settings.plex_client_identifier.is_some();
    let id = plex::client_identifier(&mut settings);
    if !had_id {
        settings.save(app)?;
    }
    Ok(PlexHeaders::new(id))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("Background task failed: {e}"))?
}

fn remote_error(error: RemoteError) -> String {
    error.to_string()
}

/// The account token of a finished sign-in.
fn signed_in_token(session_id: &str) -> Result<String, String> {
    sessions()
        .token(session_id, Instant::now())
        .ok_or_else(|| "The Plex sign-in has expired. Sign in again.".to_string())
}

fn find_server(headers: &PlexHeaders, token: &str, server_id: &str) -> Result<PlexServer, String> {
    auth::list_servers(headers, token)
        .map_err(remote_error)?
        .into_iter()
        .find(|server| server.client_identifier == server_id)
        .ok_or_else(|| "That Plex server is no longer available".to_string())
}

/// The first of the server's addresses that answers.
fn reachable_uri(headers: &PlexHeaders, server: &PlexServer) -> Result<String, String> {
    let candidates = plex_connection::ordered(&server.connections);
    plex_connection::pick(&candidates, &|uri| {
        plex_connection::probe_identity(headers, uri)
    })
    .ok_or_else(|| format!("Can't reach {}", server.name))
}

/// Starts signing in to Plex: opens the Plex sign-in page in the browser.
#[tauri::command]
pub async fn plex_begin_sign_in(app: AppHandle) -> Result<PlexSignInDto, String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let headers = plex_headers(&app)?;
    blocking(move || {
        let pin = auth::create_pin(&headers).map_err(remote_error)?;
        let auth_url = auth::auth_url(&headers, &pin);
        let session_id = sessions().insert(pin.id, Instant::now());
        if let Err(e) = crate::app::updater::open_url(&auth_url) {
            tracing::warn!("Could not open the Plex sign-in page: {e}");
        }
        Ok(PlexSignInDto {
            session_id,
            auth_url,
        })
    })
    .await
}

/// `"pending"`, `"done"` or `"expired"`.
#[tauri::command]
pub async fn plex_poll_sign_in(session_id: String, app: AppHandle) -> Result<String, String> {
    let headers = plex_headers(&app)?;
    blocking(move || {
        let now = Instant::now();
        if sessions().token(&session_id, now).is_some() {
            return Ok("done".to_string());
        }
        let Some(pin_id) = sessions().pin(&session_id, now) else {
            return Ok("expired".to_string());
        };
        match auth::poll_pin(&headers, pin_id) {
            Ok(Some(token)) => {
                sessions().set_token(&session_id, token);
                Ok("done".to_string())
            }
            Ok(None) => Ok("pending".to_string()),
            Err(RemoteError::Unauthorized) => {
                sessions().remove(&session_id);
                Ok("expired".to_string())
            }
            Err(e) => Err(remote_error(e)),
        }
    })
    .await
}

/// The signed-in account's servers, the user's own first.
#[tauri::command]
pub async fn plex_list_servers(
    session_id: String,
    app: AppHandle,
) -> Result<Vec<PlexServerDto>, String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let headers = plex_headers(&app)?;
    blocking(move || {
        let token = signed_in_token(&session_id)?;
        let mut servers: Vec<PlexServerDto> = auth::list_servers(&headers, &token)
            .map_err(remote_error)?
            .into_iter()
            .map(|server| PlexServerDto {
                id: server.client_identifier,
                name: server.name,
                owned: server.owned,
            })
            .collect();
        servers.sort_by(|a, b| b.owned.cmp(&a.owned).then_with(|| a.name.cmp(&b.name)));
        Ok(servers)
    })
    .await
}

/// A server's music libraries.
#[tauri::command]
pub async fn plex_list_libraries(
    session_id: String,
    server_id: String,
    app: AppHandle,
) -> Result<Vec<PlexLibraryDto>, String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let headers = plex_headers(&app)?;
    blocking(move || {
        let token = signed_in_token(&session_id)?;
        let server = find_server(&headers, &token, &server_id)?;
        let uri = reachable_uri(&headers, &server)?;
        Ok(
            auth::list_music_sections(&uri, &server.access_token, &headers)
                .map_err(remote_error)?
                .into_iter()
                .map(|section| PlexLibraryDto {
                    key: section.key,
                    title: section.title,
                })
                .collect(),
        )
    })
    .await
}

/// Adds a Plex server, with the chosen libraries, as a remote source and
/// starts its first sync.
#[tauri::command]
pub async fn plex_add_source(
    session_id: String,
    server_id: String,
    library_keys: Vec<String>,
    app: AppHandle,
) -> Result<RemoteSourceDto, String> {
    NetworkGate::global().check().map_err(remote_error)?;
    if library_keys.is_empty() {
        return Err("Choose at least one music library".to_string());
    }
    let headers = plex_headers(&app)?;
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        let token = signed_in_token(&session_id)?;
        let server = find_server(&headers, &token, &server_id)?;
        let uri = reachable_uri(&headers, &server)?;
        let sections = auth::list_music_sections(&uri, &server.access_token, &headers)
            .map_err(remote_error)?
            .into_iter()
            .filter(|section| library_keys.contains(&section.key))
            .collect::<Vec<_>>();
        if sections.is_empty() {
            return Err("Those libraries are no longer on the server".to_string());
        }
        let config = PlexSourceConfig {
            server_id: server.client_identifier.clone(),
            server_name: server.name.clone(),
            server_token: server.access_token.clone(),
            account_token: token,
            sections,
            connections: server.connections.clone(),
            preferred_uri: Some(uri),
        };
        let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        let id = rt.with_library(|library| {
            let id = library.insert_remote_source(
                "plex",
                &server.client_identifier,
                &server.name,
                &json,
            )?;
            rt.reload_sources(library);
            Ok::<_, String>(id)
        })?;
        sessions().remove(&session_id);
        sync_one_in_background(&task_app, rt, id.clone());
        let row = rt
            .with_library(|library| library.list_remote_sources())?
            .into_iter()
            .find(|row| row.id == id)
            .ok_or_else(|| "The new source went missing".to_string())?;
        Ok(source_dto(&row, rt.status(&id)))
    })
    .await
}

/// Signs a source in again after Plex stopped accepting its token.
#[tauri::command]
pub async fn plex_reauthorize_source(
    source_id: String,
    session_id: String,
    app: AppHandle,
) -> Result<(), String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let headers = plex_headers(&app)?;
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        let token = signed_in_token(&session_id)?;
        let row = rt
            .with_library(|library| library.list_remote_sources())?
            .into_iter()
            .find(|row| row.id == source_id)
            .ok_or_else(|| "That source was removed".to_string())?;
        let mut config: PlexSourceConfig =
            serde_json::from_str(&row.config_json).map_err(|e| e.to_string())?;
        let server = find_server(&headers, &token, &config.server_id)?;
        config.account_token = token;
        config.server_token = server.access_token;
        config.connections = server.connections;
        let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        rt.with_library(|library| {
            library.update_remote_source_config(&source_id, &json)?;
            rt.reload_sources(library);
            Ok::<_, String>(())
        })?;
        sessions().remove(&session_id);
        rt.set_status(&source_id, SourceState::Ok);
        sync_one_in_background(&task_app, rt, source_id);
        Ok(())
    })
    .await
}

fn sync_one_in_background(app: &AppHandle, rt: &'static RemoteRuntime, source_id: String) {
    let enabled = remote_enabled(app);
    let spawned = std::thread::Builder::new()
        .name("wave-remote-sync".into())
        .spawn(move || {
            if let Err(e) = sync::sync_source(rt, &source_id, enabled) {
                tracing::warn!("Syncing a remote source failed: {e}");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("Could not start a remote sync: {e}");
    }
}

#[tauri::command]
pub async fn list_remote_sources() -> Result<Vec<RemoteSourceDto>, String> {
    let rt = runtime()?;
    blocking(move || {
        let rows = rt.with_library(|library| library.list_remote_sources())?;
        Ok(rows
            .iter()
            .map(|row| source_dto(row, rt.status(&row.id)))
            .collect())
    })
    .await
}

/// Removes a source. Its tracks are kept as hidden rows so playlists and
/// history survive, and come back if the same server is added again. Its
/// cached audio is deleted; downloads stay.
#[tauri::command]
pub async fn remove_remote_source(source_id: String, app: AppHandle) -> Result<(), String> {
    let rt = runtime()?;
    let enabled = remote_enabled(&app);
    blocking(move || {
        rt.with_library(|library| {
            let cached = library.tombstone_remote_source(&source_id)?;
            for uri in cached {
                delete_cache_files(library, &uri);
            }
            rt.reload_sources(library);
            library.recompute_remote_visibility(enabled)
        })
    })
    .await?;
    let _ = app.emit("remote-status-changed", ());
    let _ = app.emit("library-changed", ());
    Ok(())
}

fn delete_cache_files(library: &crate::library::Library, uri: &RemoteUri) {
    let extension = match library.remote_stream_info(uri) {
        Ok(Some(info)) => info.extension,
        _ => return,
    };
    for file in stream::remote_cache_files_for(uri, &extension) {
        if let Err(e) = std::fs::remove_file(&file) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("Could not delete {}: {e}", file.display());
            }
        }
    }
}

#[tauri::command]
pub async fn get_remote_status() -> Result<RemoteStatusDto, String> {
    let gate = NetworkGate::global().state();
    let Some(rt) = RemoteRuntime::global() else {
        return Ok(RemoteStatusDto {
            gate,
            sources: Vec::new(),
        });
    };
    let sources = blocking(move || {
        let rows = rt.with_library(|library| library.list_remote_sources())?;
        Ok(rows
            .into_iter()
            .map(|row| RemoteSourceStateDto {
                state: rt.status(&row.id),
                id: row.id,
            })
            .collect())
    })
    .await?;
    Ok(RemoteStatusDto { gate, sources })
}

/// Syncs every remote source, unless a refresh ran in the last 30 seconds or
/// is still running. Local folders are rescanned by the frontend when this
/// says it started.
#[tauri::command]
pub async fn request_library_refresh(app: AppHandle) -> Result<RefreshTicketDto, String> {
    if let Err(retry_after_seconds) = RefreshCooldown::global().try_begin(Instant::now()) {
        return Ok(RefreshTicketDto {
            started: false,
            retry_after_seconds,
        });
    }
    let finish = move |app: AppHandle| {
        RefreshCooldown::global().finish();
        let _ = app.emit("library-refresh-finished", ());
    };
    match RemoteRuntime::global() {
        Some(rt) if NetworkGate::global().state() == GateState::Allowed => {
            let done_app = app.clone();
            sync::sync_all_in_background(rt, true, Box::new(move || finish(done_app)));
        }
        _ => finish(app),
    }
    Ok(RefreshTicketDto {
        started: true,
        retry_after_seconds: 0,
    })
}

/// Deletes every cached track that is not playing, queued or downloading.
/// Downloads are never touched. Returns the bytes freed.
#[tauri::command]
pub async fn clear_source_cache(app: AppHandle) -> Result<u64, String> {
    blocking(move || {
        let protected = crate::commands::queued_paths(&app);
        let state = app.state::<LibraryState>();
        let library = state.0.lock().map_err(|e| e.to_string())?;
        Ok(crate::commands::evict_source_cache_to(
            &app,
            &library,
            protected,
            Some(0),
        ))
    })
    .await
}

/// Bytes the shared audio cache takes, previews aside.
#[tauri::command]
pub async fn get_source_cache_usage() -> Result<u64, String> {
    blocking(|| {
        let root = crate::app::paths::source_cache_dir();
        let previews = root.join("previews");
        Ok(walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && !entry.path().starts_with(&previews))
            .filter_map(|entry| entry.metadata().ok())
            .map(|metadata| metadata.len())
            .sum())
    })
    .await
}

/// Keeps a remote track for good: finishes downloading it, copies it to the
/// download folder, and turns it into a local track.
#[tauri::command]
pub async fn download_remote_track(path: String, app: AppHandle) -> Result<Track, String> {
    let task_app = app.clone();
    let (stored, fallback) = blocking(move || {
        let track = {
            let state = task_app.state::<LibraryState>();
            let library = state.0.lock().map_err(|e| e.to_string())?;
            library
                .get_track_details(&path)?
                .ok_or_else(|| "Track is not in the library".to_string())?
        };
        if track.source_state.as_deref() == Some("downloaded") {
            return Ok((track, None));
        }
        let cached = match stream::resolve_play_target(&path)? {
            stream::PlayTarget::File(file) => file,
            stream::PlayTarget::Progressive(fill, extension) => {
                fill.wait_for(0, fill.len, DOWNLOAD_WAIT)?;
                let uri = RemoteUri::parse(&path).ok_or("Not a remote track")?;
                let [file, _] = stream::remote_cache_files_for(&uri, &extension);
                file.to_string_lossy().into_owned()
            }
        };

        let as_source = SourceTrack {
            provider: track.source_provider.clone().unwrap_or_default(),
            id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: Some(track.album.clone()),
            duration_seconds: track.duration_seconds,
            artwork_url: None,
            audio_url: Some(format!("remote.{}", track.format)),
            is_full_length: true,
            downloadable: true,
            attribution: None,
            already_in_library: None,
        };
        let settings_state = task_app.state::<AppSettingsState>();
        let media_folders = settings_state
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .media_folders
            .clone();
        let destination = source_download::choose_destination(
            &as_source,
            &media_folders,
            &|root| {
                std::fs::create_dir_all(root).is_ok()
                    && !std::fs::metadata(root)
                        .map(|m| m.permissions().readonly())
                        .unwrap_or(true)
            },
            &|path| path.exists(),
        );
        if let Some(parent) = destination.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create download folder: {e}"))?;
        }
        std::fs::copy(&cached, &destination.path).map_err(|e| format!("Download failed: {e}"))?;

        {
            let mut settings = settings_state.0.lock().map_err(|e| e.to_string())?;
            let folder = destination.media_folder.to_string_lossy().into_owned();
            if !settings.media_folders.contains(&folder) {
                settings.media_folders.push(folder);
                settings.save(&task_app)?;
            }
        }

        let stored = {
            let state = task_app.state::<LibraryState>();
            let library = state.0.lock().map_err(|e| e.to_string())?;
            let stored =
                library.promote_source_track(&track.id, &destination.path.to_string_lossy())?;
            library.recompute_remote_visibility(remote_enabled(&task_app))?;
            stored
        };
        Ok((stored, destination.fallback_reason))
    })
    .await?;
    if let Some(reason) = fallback {
        let _ = app.emit("source-download-fallback", reason);
    }
    let _ = app.emit("library-changed", ());
    Ok(stored)
}

/// Re-checks which remote tracks show after local files came or went, so a
/// local copy hides its remote twin. Runs in the background.
pub fn after_local_library_change(app: &AppHandle) {
    let enabled = remote_enabled(app);
    let Some(rt) = RemoteRuntime::global() else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("wave-remote-visibility".into())
        .spawn(move || {
            if let Err(e) = rt.with_library(|library| library.recompute_remote_visibility(enabled))
            {
                tracing::warn!("Could not update which remote tracks show: {e}");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("Could not start a visibility check: {e}");
    }
}

/// Applies Outside Sourcing settings to the network gate and the library.
/// Called after the settings change; `was_enabled` is the value before.
pub fn apply_settings(app: &AppHandle, was_enabled: bool) {
    let (enabled, cellular) = app
        .try_state::<AppSettingsState>()
        .and_then(|state| {
            state
                .0
                .lock()
                .ok()
                .map(|s| (s.remote_sources_enabled, s.remote_cellular_enabled))
        })
        .unwrap_or((false, false));
    NetworkGate::global().set_policy(enabled, cellular);
    let _ = app.emit("remote-status-changed", ());
    if enabled == was_enabled {
        return;
    }
    let Some(rt) = RemoteRuntime::global() else {
        return;
    };
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("wave-remote-toggle".into())
        .spawn(move || {
            if let Err(e) = rt.with_library(|library| library.recompute_remote_visibility(enabled))
            {
                tracing::warn!("Could not update which remote tracks show: {e}");
            }
            let _ = app.emit("library-changed", ());
            if enabled && NetworkGate::global().state() == GateState::Allowed {
                sync::sync_all_in_background(rt, true, Box::new(|| {}));
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("Could not apply Outside Sourcing: {e}");
    }
}

/// Delivers runtime events to the frontend.
pub fn event_sink(app: AppHandle) -> Box<dyn Fn(RemoteEvent) + Send + Sync> {
    Box::new(move |event| match event {
        RemoteEvent::SyncProgress {
            source_id,
            synced,
            total,
        } => {
            let _ = app.emit(
                "remote-sync-progress",
                serde_json::json!({ "source_id": source_id, "synced": synced, "total": total }),
            );
        }
        RemoteEvent::SyncFinished { source_id, error } => {
            let _ = app.emit(
                "remote-sync-finished",
                serde_json::json!({ "source_id": source_id, "error": error }),
            );
            let _ = app.emit("library-changed", ());
        }
        RemoteEvent::StatusChanged => {
            let _ = app.emit("remote-status-changed", ());
        }
        RemoteEvent::StreamFailed { path, message } => {
            let _ = app.emit(
                "remote-stream-failed",
                serde_json::json!({ "path": path, "message": message }),
            );
        }
        RemoteEvent::FillComplete { path } => {
            let _ = app.emit("remote-track-cached", serde_json::json!({ "path": path }));
            trim_cache_in_background(&app);
        }
    })
}

/// A finished download may have pushed the cache past its cap.
fn trim_cache_in_background(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("wave-cache-trim".into())
        .spawn(move || {
            let protected = crate::commands::queued_paths(&app);
            let state = app.state::<LibraryState>();
            let Ok(library) = state.0.lock() else {
                return;
            };
            crate::commands::evict_source_cache_to(&app, &library, protected, None);
        });
    if let Err(e) = spawned {
        tracing::warn!("Could not start trimming the cache: {e}");
    }
}

/// Brings remote sources up at launch: network policy, Plex identity, the
/// stored sources, an orphan sweep, and a first sync when the network allows.
pub fn start(app: &AppHandle, rt: &'static RemoteRuntime) {
    #[cfg(target_os = "android")]
    NetworkGate::global().set_cellular_probe(crate::android::network::is_cellular);
    apply_settings(app, remote_enabled(app));
    match plex_headers(app) {
        Ok(headers) => rt.set_plex_headers(headers),
        Err(e) => tracing::warn!("Plex is unavailable: {e}"),
    }
    let enabled = remote_enabled(app);
    let spawned = std::thread::Builder::new()
        .name("wave-remote-start".into())
        .spawn(move || {
            rt.with_library(|library| {
                rt.reload_sources(library);
                if let Err(e) = library.recompute_remote_visibility(enabled) {
                    tracing::warn!("Could not update which remote tracks show: {e}");
                }
            });
            match stream::sweep_orphans(rt) {
                Ok(0) => {}
                Ok(removed) => tracing::info!("Removed {removed} stray remote cache files"),
                Err(e) => tracing::warn!("Could not sweep the remote cache: {e}"),
            }
            if NetworkGate::global().state() == GateState::Allowed {
                sync::sync_all_in_background(rt, true, Box::new(|| {}));
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("Could not start remote sources: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::store::RemoteSourceRow;
    use std::time::{Duration, Instant};

    #[test]
    fn sign_in_sessions_expire_after_15_minutes() {
        let start = Instant::now();
        let mut sessions = SignInSessions::default();
        let id = sessions.insert(7, start);
        assert!(sessions
            .pin(&id, start + Duration::from_secs(14 * 60))
            .is_some());
        assert!(sessions
            .pin(&id, start + Duration::from_secs(15 * 60))
            .is_none());
        assert!(sessions.is_empty());
    }

    #[test]
    fn source_dto_maps_state_and_counts() {
        let row = RemoteSourceRow {
            id: "plex-abc".into(),
            kind: "plex".into(),
            display_name: "Home".into(),
            config_json: "{\"server_token\":\"secret\"}".into(),
            created_at: 1,
            last_sync_at: Some(42),
            last_sync_error: Some("Can't reach Home".into()),
            track_count: 120,
        };
        let dto = source_dto(&row, SourceState::Unreachable);
        assert_eq!(dto.id, "plex-abc");
        assert_eq!(dto.kind, "plex");
        assert_eq!(dto.display_name, "Home");
        assert_eq!(dto.state, SourceState::Unreachable);
        assert_eq!(dto.track_count, 120);
        assert_eq!(dto.last_sync_at, Some(42));
        assert_eq!(dto.last_error.as_deref(), Some("Can't reach Home"));
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("secret"), "{json}");
    }
}
