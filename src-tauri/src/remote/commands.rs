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
use super::plex::direct as plex_direct;
use super::plex::{self, PlexHeaders};
use super::refresh::RefreshCooldown;
use super::store::RemoteSourceRow;
use super::uri::RemoteUri;
use super::{
    audius, dlna, jellyfin, kodi, stream, subsonic, sync, webdav, RemoteEvent, RemoteRuntime,
    SourceState,
};
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
    /// How to sign in again: `plex_tv`, `plex_token`, `password` or `none`.
    pub reauth: String,
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
        reauth: reauth_for(row).to_string(),
    }
}

/// How a source signs in again: through plex.tv, with a typed Plex token,
/// with its password, or not at all (DLNA and Audius have no sign-in).
fn reauth_for(row: &RemoteSourceRow) -> &'static str {
    if matches!(row.kind.as_str(), "dlna" | "audius") {
        return "none";
    }
    if matches!(
        row.kind.as_str(),
        "subsonic" | "jellyfin" | "webdav" | "kodi"
    ) {
        return "password";
    }
    let direct = serde_json::from_str::<serde_json::Value>(&row.config_json)
        .ok()
        .and_then(|config| config.get("direct").and_then(serde_json::Value::as_bool))
        .unwrap_or(false);
    if direct {
        "plex_token"
    } else {
        "plex_tv"
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
            direct: false,
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

#[derive(Debug, Clone, Serialize)]
pub struct PlexDirectServerDto {
    pub server_id: String,
    pub name: String,
    pub libraries: Vec<PlexLibraryDto>,
}

/// Probing a server by address, with the error a person can act on.
fn probe_direct_server(
    url: &str,
    token: &str,
    headers: &PlexHeaders,
) -> Result<plex_direct::DirectProbe, String> {
    plex_direct::probe_direct(url, token, headers).map_err(|e| match e {
        RemoteError::Unauthorized if token.is_empty() => {
            "This server needs a token. Add the server's X-Plex-Token.".to_string()
        }
        RemoteError::Unauthorized => "The server turned down this token".to_string(),
        RemoteError::Unreachable(_) => {
            format!("Can't reach a Plex server at {url}. Check the address and port.")
        }
        other => remote_error(other),
    })
}

/// Looks for a Plex server at an address, without plex.tv, and lists its
/// music libraries.
#[tauri::command]
pub async fn plex_probe_direct(
    url: String,
    token: Option<String>,
    app: AppHandle,
) -> Result<PlexDirectServerDto, String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let url = plex_direct::normalize_server_url(&url)?;
    let token = token.unwrap_or_default().trim().to_string();
    let headers = plex_headers(&app)?;
    blocking(move || {
        let probe = probe_direct_server(&url, &token, &headers)?;
        Ok(PlexDirectServerDto {
            server_id: probe.server_id,
            name: probe.name,
            libraries: probe
                .sections
                .into_iter()
                .map(|section| PlexLibraryDto {
                    key: section.key,
                    title: section.title,
                })
                .collect(),
        })
    })
    .await
}

/// Adds a Plex server by its address, with the chosen libraries, and starts
/// its first sync. No plex.tv account is involved.
#[tauri::command]
pub async fn plex_add_direct_source(
    url: String,
    token: Option<String>,
    library_keys: Vec<String>,
    app: AppHandle,
) -> Result<RemoteSourceDto, String> {
    NetworkGate::global().check().map_err(remote_error)?;
    if library_keys.is_empty() {
        return Err("Choose at least one music library".to_string());
    }
    let url = plex_direct::normalize_server_url(&url)?;
    let token = token.unwrap_or_default().trim().to_string();
    let headers = plex_headers(&app)?;
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        let probe = probe_direct_server(&url, &token, &headers)?;
        let config = plex_direct::direct_config(&url, &token, &probe, &library_keys);
        if config.sections.is_empty() {
            return Err("Those libraries are no longer on the server".to_string());
        }
        let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        let id = rt.with_library(|library| {
            let id = library.insert_remote_source("plex", &probe.server_id, &probe.name, &json)?;
            rt.reload_sources(library);
            Ok::<_, String>(id)
        })?;
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

/// Gives a directly connected Plex server a new token, after it stopped
/// accepting the old one.
#[tauri::command]
pub async fn plex_set_direct_token(
    source_id: String,
    token: String,
    app: AppHandle,
) -> Result<(), String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let token = token.trim().to_string();
    let headers = plex_headers(&app)?;
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        let row = rt
            .with_library(|library| library.list_remote_sources())?
            .into_iter()
            .find(|row| row.id == source_id)
            .ok_or_else(|| "That source was removed".to_string())?;
        let mut config: PlexSourceConfig =
            serde_json::from_str(&row.config_json).map_err(|e| e.to_string())?;
        let url = config
            .preferred_uri
            .clone()
            .ok_or_else(|| "This source has no address".to_string())?;
        let probe = probe_direct_server(&url, &token, &headers)?;
        if probe.server_id != config.server_id {
            return Err("There's a different server at that address now".to_string());
        }
        config.server_token = token;
        let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        rt.with_library(|library| {
            library.update_remote_source_config(&source_id, &json)?;
            rt.reload_sources(library);
            Ok::<_, String>(())
        })?;
        rt.set_status(&source_id, SourceState::Ok);
        sync_one_in_background(&task_app, rt, source_id);
        Ok(())
    })
    .await
}

/// A Subsonic check's failure, said the way a person can act on.
fn subsonic_error(error: RemoteError, url: &str) -> String {
    match error {
        RemoteError::Unauthorized => "Wrong username or password".to_string(),
        RemoteError::Unreachable(_) => {
            format!("Can't reach a server at {url}. Check the address and port.")
        }
        RemoteError::Parse(_) | RemoteError::Http(_) => {
            "There's no Subsonic server at that address".to_string()
        }
        other => remote_error(other),
    }
}

/// Signs in to a Subsonic-compatible server (Navidrome, Airsonic, Gonic and
/// others), adds it as a remote source and starts its first sync.
#[tauri::command]
pub async fn subsonic_add_source(
    url: String,
    username: String,
    password: String,
    app: AppHandle,
) -> Result<RemoteSourceDto, String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let url = subsonic::normalize_url(&url)?;
    let username = username.trim().to_string();
    if username.is_empty() {
        return Err("Enter your username".to_string());
    }
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        let config = subsonic::SubsonicConfig {
            url: url.clone(),
            username,
            password,
            plain_password: false,
            server_type: None,
        };
        let probed = subsonic::SubsonicBackend::new(config)
            .probe()
            .map_err(|e| subsonic_error(e, &url))?;
        let name = subsonic::display_name(probed.server_type.as_deref(), &probed.url);
        let key = subsonic::stable_key(&probed);
        let json = serde_json::to_string(&probed).map_err(|e| e.to_string())?;
        store_new_source(&task_app, rt, "subsonic", &key, &name, &json)
    })
    .await
}

/// Saves a checked source, builds its backend and starts its first sync.
fn store_new_source(
    app: &AppHandle,
    rt: &'static RemoteRuntime,
    kind: &str,
    key: &str,
    name: &str,
    config_json: &str,
) -> Result<RemoteSourceDto, String> {
    let id = rt.with_library(|library| {
        let id = library.insert_remote_source(kind, key, name, config_json)?;
        rt.reload_sources(library);
        Ok::<_, String>(id)
    })?;
    sync_one_in_background(app, rt, id.clone());
    let row = rt
        .with_library(|library| library.list_remote_sources())?
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(|| "The new source went missing".to_string())?;
    Ok(source_dto(&row, rt.status(&id)))
}

/// A WebDAV check's failure, said the way a person can act on.
fn webdav_error(error: RemoteError, url: &str) -> String {
    match error {
        RemoteError::Unauthorized => "Wrong username or password".to_string(),
        RemoteError::Unreachable(_) => {
            format!("Can't reach {url}. Check the address and port.")
        }
        RemoteError::Http(404) => "There's no folder at that address".to_string(),
        RemoteError::Http(_) | RemoteError::Parse(_) => {
            "That address isn't a WebDAV folder".to_string()
        }
        RemoteError::Insecure => {
            "Use an https:// address to sign in to a server outside your network".to_string()
        }
        other => remote_error(other),
    }
}

/// Adds a WebDAV folder (Nextcloud, ownCloud, a NAS share, rclone) as a
/// remote source and starts its first sync. The username is optional.
#[tauri::command]
pub async fn webdav_add_source(
    url: String,
    username: String,
    password: String,
    app: AppHandle,
) -> Result<RemoteSourceDto, String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let url = webdav::normalize_url(&url)?;
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        let config = webdav::WebDavConfig {
            url: url.clone(),
            username: username.trim().to_string(),
            password,
        };
        webdav::WebDavBackend::new(config.clone())
            .and_then(|backend| backend.probe())
            .map_err(|e| webdav_error(e, &url))?;
        let name = webdav::display_name(&config.url);
        let key = webdav::stable_key(&config);
        let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        store_new_source(&task_app, rt, "webdav", &key, &name, &json)
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
pub struct DlnaServerDto {
    pub location: String,
    pub name: String,
}

/// Looks for DLNA and UPnP media servers on the local network. Each one found
/// is listed by name with the address its description lives at.
#[tauri::command]
pub async fn dlna_discover() -> Result<Vec<DlnaServerDto>, String> {
    let gate = NetworkGate::global();
    gate.check().map_err(remote_error)?;
    blocking(move || {
        let locations = dlna::discover(gate, Duration::from_secs(4)).map_err(remote_error)?;
        // Descriptions are read side by side so one slow box doesn't hold up
        // the list.
        let handles: Vec<_> = locations
            .into_iter()
            .map(|location| std::thread::spawn(move || dlna::describe(gate, &location).ok()))
            .collect();
        let mut seen = std::collections::HashSet::new();
        let mut servers = Vec::new();
        for config in handles.into_iter().filter_map(|h| h.join().ok().flatten()) {
            if seen.insert(dlna::stable_key(&config)) {
                servers.push(DlnaServerDto {
                    location: config.location,
                    name: config.name,
                });
            }
        }
        servers.sort_by_key(|server| server.name.to_lowercase());
        Ok(servers)
    })
    .await
}

/// Adds a DLNA or UPnP media server, found by [`dlna_discover`] or typed as an
/// address, and starts its first sync.
#[tauri::command]
pub async fn dlna_add_source(address: String, app: AppHandle) -> Result<RemoteSourceDto, String> {
    let gate = NetworkGate::global();
    gate.check().map_err(remote_error)?;
    let candidates = dlna::candidate_locations(&address)?;
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        let config = candidates
            .iter()
            .find_map(|location| dlna::describe(gate, location).ok())
            .ok_or_else(|| "There's no media server at that address".to_string())?;
        dlna::DlnaBackend::new(config.clone())
            .and_then(|backend| backend.probe())
            .map_err(|_| format!("{} didn't answer a request for its music", config.name))?;
        let key = dlna::stable_key(&config);
        let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        store_new_source(&task_app, rt, "dlna", &key, &config.name, &json)
    })
    .await
}

/// Adds an Audius account by its handle: its uploads, favorites and reposts
/// join the library. Nothing about the account is private, so there is no
/// sign-in.
#[tauri::command]
pub async fn audius_add_source(handle: String, app: AppHandle) -> Result<RemoteSourceDto, String> {
    let gate = NetworkGate::global();
    gate.check().map_err(remote_error)?;
    let handle = audius::parse_handle(&handle)?;
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        let config = audius::resolve(gate, &handle).map_err(|e| match e {
            RemoteError::Http(404) => format!("There's no Audius account called @{handle}"),
            other => remote_error(other),
        })?;
        let name = audius::display_name(&config);
        let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        store_new_source(&task_app, rt, "audius", &config.user_id, &name, &json)
    })
    .await
}

/// A Kodi check's failure, said the way a person can act on.
fn kodi_error(error: RemoteError, url: &str) -> String {
    match error {
        RemoteError::Unauthorized => "Wrong username or password".to_string(),
        RemoteError::Unreachable(_) => format!(
            "Can't reach Kodi at {url}. Check that remote control over HTTP is on in Kodi's settings."
        ),
        RemoteError::Http(_) | RemoteError::Parse(_) => {
            "There's no Kodi web server at that address".to_string()
        }
        RemoteError::Insecure => {
            "Use an https:// address to sign in to Kodi outside your network".to_string()
        }
        other => remote_error(other),
    }
}

/// Adds a Kodi device's music library and starts its first sync.
#[tauri::command]
pub async fn kodi_add_source(
    url: String,
    username: String,
    password: String,
    app: AppHandle,
) -> Result<RemoteSourceDto, String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let url = kodi::normalize_url(&url)?;
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        let config = kodi::KodiConfig {
            url: url.clone(),
            username: username.trim().to_string(),
            password,
        };
        kodi::KodiBackend::new(config.clone())
            .probe()
            .map_err(|e| kodi_error(e, &url))?;
        let name = kodi::display_name(&config.url);
        let key = kodi::stable_key(&config);
        let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        store_new_source(&task_app, rt, "kodi", &key, &name, &json)
    })
    .await
}

/// A Kodi config with a new password, checked with the device.
fn kodi_with_password(config_json: &str, password: String) -> Result<String, String> {
    let mut config: kodi::KodiConfig =
        serde_json::from_str(config_json).map_err(|e| e.to_string())?;
    config.password = password;
    let url = config.url.clone();
    kodi::KodiBackend::new(config.clone())
        .probe()
        .map_err(|e| kodi_error(e, &url))?;
    serde_json::to_string(&config).map_err(|e| e.to_string())
}

/// A WebDAV config with a new password, checked against the folder.
fn webdav_with_password(config_json: &str, password: String) -> Result<String, String> {
    let mut config: webdav::WebDavConfig =
        serde_json::from_str(config_json).map_err(|e| e.to_string())?;
    config.password = password;
    let url = config.url.clone();
    webdav::WebDavBackend::new(config.clone())
        .and_then(|backend| backend.probe())
        .map_err(|e| webdav_error(e, &url))?;
    serde_json::to_string(&config).map_err(|e| e.to_string())
}

/// A Subsonic config signed in with a new password, checked with the server.
fn subsonic_with_password(config_json: &str, password: String) -> Result<String, String> {
    let mut config: subsonic::SubsonicConfig =
        serde_json::from_str(config_json).map_err(|e| e.to_string())?;
    config.password = password;
    config.plain_password = false;
    let url = config.url.clone();
    let probed = subsonic::SubsonicBackend::new(config)
        .probe()
        .map_err(|e| subsonic_error(e, &url))?;
    serde_json::to_string(&probed).map_err(|e| e.to_string())
}

/// A Jellyfin or Emby config with a new token, from signing in again as the
/// same user on the same device id.
fn jellyfin_with_password(config_json: &str, password: &str) -> Result<String, String> {
    let mut config: jellyfin::JellyfinConfig =
        serde_json::from_str(config_json).map_err(|e| e.to_string())?;
    let auth = jellyfin::authenticate(
        NetworkGate::global(),
        &config.url,
        &config.device_id,
        &config.username,
        password,
        jellyfin::is_emby(&config.product),
    )
    .map_err(|e| jellyfin_error(e, &config.url))?;
    if auth.user.id != config.user_id {
        return Err("That password is for a different account".to_string());
    }
    config.token = auth.access_token;
    serde_json::to_string(&config).map_err(|e| e.to_string())
}

/// Gives a source that signs in with a password (Subsonic, Jellyfin, Emby) a
/// new one, after the server stopped accepting the old sign-in.
#[tauri::command]
pub async fn set_source_password(
    source_id: String,
    password: String,
    app: AppHandle,
) -> Result<(), String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        let row = rt
            .with_library(|library| library.list_remote_sources())?
            .into_iter()
            .find(|row| row.id == source_id)
            .ok_or_else(|| "That source was removed".to_string())?;
        let json = match row.kind.as_str() {
            "subsonic" => subsonic_with_password(&row.config_json, password)?,
            "jellyfin" => jellyfin_with_password(&row.config_json, &password)?,
            "webdav" => webdav_with_password(&row.config_json, password)?,
            "kodi" => kodi_with_password(&row.config_json, password)?,
            _ => return Err("This source doesn't sign in with a password".to_string()),
        };
        rt.with_library(|library| {
            library.update_remote_source_config(&source_id, &json)?;
            rt.reload_sources(library);
            Ok::<_, String>(())
        })?;
        rt.set_status(&source_id, SourceState::Ok);
        sync_one_in_background(&task_app, rt, source_id);
        Ok(())
    })
    .await
}

/// A Jellyfin or Emby check's failure, said the way a person can act on.
fn jellyfin_error(error: RemoteError, url: &str) -> String {
    match error {
        RemoteError::Unauthorized => "Wrong username or password".to_string(),
        RemoteError::Unreachable(_) => {
            format!("Can't reach a server at {url}. Check the address and port.")
        }
        RemoteError::Parse(_) | RemoteError::Http(_) => {
            "There's no Jellyfin or Emby server at that address".to_string()
        }
        other => remote_error(other),
    }
}

struct PendingJellyfin {
    config: jellyfin::JellyfinConfig,
    libraries: Vec<jellyfin::JellyfinLibrary>,
    created: Instant,
}

/// Jellyfin and Emby sign-ins waiting for the user to pick libraries, keyed
/// by an id the frontend holds. The token never leaves the backend.
fn pending_jellyfin() -> std::sync::MutexGuard<'static, HashMap<String, PendingJellyfin>> {
    static PENDING: OnceLock<Mutex<HashMap<String, PendingJellyfin>>> = OnceLock::new();
    let mut pending = PENDING
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();
    pending.retain(|_, p| now.saturating_duration_since(p.created) < SESSION_TTL);
    pending
}

#[derive(Debug, Clone, Serialize)]
pub struct JellyfinServerDto {
    pub session_id: String,
    pub name: String,
    pub libraries: Vec<PlexLibraryDto>,
}

/// Signs in to a Jellyfin or Emby server and lists the user's music
/// libraries. Nothing is stored until [`jellyfin_add_source`].
#[tauri::command]
pub async fn jellyfin_sign_in(
    url: String,
    username: String,
    password: String,
) -> Result<JellyfinServerDto, String> {
    NetworkGate::global().check().map_err(remote_error)?;
    let url = jellyfin::normalize_url(&url)?;
    let username = username.trim().to_string();
    if username.is_empty() {
        return Err("Enter your username".to_string());
    }
    blocking(move || {
        let gate = NetworkGate::global();
        let info = jellyfin::public_info(gate, &url).map_err(|e| jellyfin_error(e, &url))?;
        let device_id = uuid::Uuid::new_v4().simple().to_string();
        let emby = jellyfin::is_emby(info.product());
        let auth = jellyfin::authenticate(gate, &url, &device_id, &username, &password, emby)
            .map_err(|e| jellyfin_error(e, &url))?;
        let config = jellyfin::JellyfinConfig {
            url: url.clone(),
            server_name: info
                .server_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| url.split("://").nth(1).unwrap_or(&url).to_string()),
            product: info.product().to_string(),
            server_id: info.id,
            user_id: auth.user.id,
            username,
            token: auth.access_token,
            device_id,
            libraries: Vec::new(),
        };
        let libraries: Vec<jellyfin::JellyfinLibrary> =
            jellyfin::JellyfinBackend::new(config.clone())
                .music_libraries()
                .map_err(|e| jellyfin_error(e, &url))?
                .into_iter()
                .map(|view| jellyfin::JellyfinLibrary {
                    name: view.name.unwrap_or_else(|| "Music".to_string()),
                    id: view.id,
                })
                .collect();
        let name = jellyfin::display_name(&config);
        let dto_libraries = libraries
            .iter()
            .map(|library| PlexLibraryDto {
                key: library.id.clone(),
                title: library.name.clone(),
            })
            .collect();
        let session_id = uuid::Uuid::new_v4().simple().to_string();
        pending_jellyfin().insert(
            session_id.clone(),
            PendingJellyfin {
                config,
                libraries,
                created: Instant::now(),
            },
        );
        Ok(JellyfinServerDto {
            session_id,
            name,
            libraries: dto_libraries,
        })
    })
    .await
}

/// Adds a signed-in Jellyfin or Emby server with the chosen libraries and
/// starts its first sync.
#[tauri::command]
pub async fn jellyfin_add_source(
    session_id: String,
    library_ids: Vec<String>,
    app: AppHandle,
) -> Result<RemoteSourceDto, String> {
    let rt = runtime()?;
    let task_app = app.clone();
    blocking(move || {
        // Kept until the source is stored, so a failed add can be tried again.
        let (mut config, libraries) = pending_jellyfin()
            .get(&session_id)
            .map(|pending| (pending.config.clone(), pending.libraries.clone()))
            .ok_or_else(|| "The sign-in expired. Sign in again.".to_string())?;
        config.libraries = libraries
            .into_iter()
            .filter(|library| library_ids.contains(&library.id))
            .collect();
        if config.libraries.is_empty() {
            return Err("Choose at least one library".to_string());
        }
        let name = jellyfin::display_name(&config);
        let key = jellyfin::stable_key(&config);
        let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
        let id = rt.with_library(|library| {
            let id = library.insert_remote_source("jellyfin", &key, &name, &json)?;
            rt.reload_sources(library);
            Ok::<_, String>(id)
        })?;
        pending_jellyfin().remove(&session_id);
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
            let row = library
                .list_remote_sources()?
                .into_iter()
                .find(|row| row.id == source_id);
            if let Some(row) = row.filter(|row| row.kind == "jellyfin") {
                sign_out_in_background(&row.config_json);
            }
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

/// Revokes a removed Jellyfin or Emby source's token, as Emby's docs ask of a
/// client that signs out. Best effort: removal never waits on the server.
fn sign_out_in_background(config_json: &str) {
    let Ok(config) = serde_json::from_str::<jellyfin::JellyfinConfig>(config_json) else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("wave-remote-sign-out".into())
        .spawn(move || {
            if let Err(e) = jellyfin::sign_out(NetworkGate::global(), &config) {
                tracing::debug!("Signing out of a removed source failed: {e}");
            }
        });
    if let Err(e) = spawned {
        tracing::debug!("Could not start signing out: {e}");
    }
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
    let (enabled, cellular, searching) = app
        .try_state::<AppSettingsState>()
        .and_then(|state| {
            state.0.lock().ok().map(|s| {
                (
                    s.remote_sources_enabled,
                    s.remote_cellular_enabled,
                    s.outside_sourcing_enabled,
                )
            })
        })
        .unwrap_or((false, false, false));
    NetworkGate::global().set_policy(enabled, cellular);
    // Search results have always streamed over any network.
    NetworkGate::search().set_policy(searching, true);
    apply_sharing(app);
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

/// Starts or stops sharing with other Waves to match the settings: it runs
/// while Outside Sourcing is on. Starting binds
/// sockets and stopping joins threads, so the work runs on a thread of its
/// own, which reads the settings as they are by then; a quick on-off-on lands
/// on the last of them.
pub fn apply_sharing(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("wave-share-apply".into())
        .spawn(move || {
            let config = sharing_config(&app);
            let background = config.is_some()
                && app
                    .try_state::<AppSettingsState>()
                    .and_then(|s| s.0.lock().ok().map(|s| s.share_in_background))
                    .unwrap_or(false);
            #[cfg(target_os = "android")]
            let name = config.as_ref().map(|c| c.name.clone());
            crate::share::apply(config);
            // Android ends a process with no window unless a foreground
            // service holds it; desktop hides the window on close instead.
            #[cfg(target_os = "android")]
            match name.filter(|_| background && crate::share::is_running()) {
                Some(name) => crate::android::share_service::start(&name),
                None => crate::android::share_service::stop(),
            }
            #[cfg(not(target_os = "android"))]
            let _ = background;
        });
    if let Err(e) = spawned {
        tracing::warn!("Couldn't change library sharing: {e}");
    }
}

/// The server sharing should run with, or `None` when it should be off.
fn sharing_config(app: &AppHandle) -> Option<crate::share::ShareConfig> {
    // Read before the settings lock is taken, so the two are never held
    // together.
    let db_path = app
        .try_state::<LibraryState>()
        .and_then(|state| {
            state
                .0
                .lock()
                .ok()
                .map(|l| std::path::PathBuf::from(l.db_path()))
        })
        .unwrap_or_else(crate::app::paths::library_db_path);
    let state = app.try_state::<AppSettingsState>()?;
    let mut settings = state.0.lock().ok()?;
    if !settings.remote_sources_enabled {
        return None;
    }
    let uuid = match settings.share_device_id.clone() {
        Some(id) => id,
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            settings.share_device_id = Some(id.clone());
            if let Err(e) = settings.save(app) {
                tracing::warn!("Couldn't save the shared library's id: {e}");
            }
            id
        }
    };
    Some(crate::share::ShareConfig {
        name: settings
            .share_name
            .clone()
            .unwrap_or_else(crate::share::default_name),
        uuid,
        db_path,
        opener: crate::share::file_opener(Some(app.clone())),
        peers: crate::peers::service(app),
    })
}

/// Whether the library is being shared, under what name and where.
#[tauri::command]
pub async fn get_share_status() -> Result<crate::share::ShareStatus, String> {
    blocking(|| Ok(crate::share::status())).await
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
        RemoteEvent::LibraryChanged => {
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
    let paired: Vec<String> = app
        .try_state::<AppSettingsState>()
        .and_then(|state| {
            state
                .0
                .lock()
                .ok()
                .map(|s| s.wave_peers.iter().map(|p| p.id.clone()).collect())
        })
        .unwrap_or_default();
    let app_for_start = app.clone();
    let spawned = std::thread::Builder::new()
        .name("wave-remote-start".into())
        .spawn(move || {
            drop_dlna_copies(&app_for_start, &paired);
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

// ── Paired Waves ───────────────────────────────────────────────────────────

/// The remote source a paired Wave's songs live under.
pub(crate) fn wave_source_id(peer_id: &str) -> String {
    format!("wave-{}", crate::remote::uri::percent_encode(peer_id))
}

/// Start reading what a newly paired Wave shares. Pairing again with a Wave
/// that was paired before picks its old songs back up.
pub(crate) fn add_wave_source(app: &AppHandle, peer: &crate::peers::PeerRecord) {
    let Ok(rt) = runtime() else {
        return;
    };
    let config = crate::remote::wave::WaveConfig {
        peer_id: peer.id.clone(),
    };
    let Ok(json) = serde_json::to_string(&config) else {
        return;
    };
    let id = wave_source_id(&peer.id);
    drop_dlna_copies(app, std::slice::from_ref(&peer.id));
    let added = rt.with_library(|library| {
        match library.insert_remote_source("wave", &peer.id, &peer.name, &json) {
            Ok(_) => {}
            Err(_) => library.update_remote_source_config(&id, &json)?,
        }
        rt.reload_sources(library);
        Ok::<_, String>(())
    });
    if let Err(e) = added {
        tracing::warn!("Couldn't add {}'s music: {e}", peer.name);
        return;
    }
    sync_one_in_background(app, rt, id);
    let _ = app.emit("remote-status-changed", ());
}

/// The DLNA sources that are paired Waves, added back when Wave also served
/// DLNA. A Wave answers only paired Waves now, so they would stay offline.
fn dlna_copies_of(rows: &[super::store::RemoteSourceRow], peer_ids: &[String]) -> Vec<String> {
    rows.iter()
        .filter(|row| row.kind == "dlna")
        .filter(|row| {
            serde_json::from_str::<super::dlna::DlnaConfig>(&row.config_json)
                .is_ok_and(|config| peer_ids.iter().any(|id| config.udn == format!("uuid:{id}")))
        })
        .map(|row| row.id.clone())
        .collect()
}

/// Removes the DLNA sources that are these Waves, songs and all.
pub(crate) fn drop_dlna_copies(app: &AppHandle, peer_ids: &[String]) {
    let Ok(rt) = runtime() else {
        return;
    };
    let copies = rt
        .with_library(|library| library.list_remote_sources())
        .map(|rows| dlna_copies_of(&rows, peer_ids))
        .unwrap_or_default();
    for id in copies {
        tracing::info!("Removing {id}: that Wave is paired now");
        remove_wave_source_by_id(app, &id);
    }
}

/// Drop a forgotten Wave's songs and its playlist.
pub(crate) fn remove_wave_source(app: &AppHandle, peer_id: &str) {
    remove_wave_source_by_id(app, &wave_source_id(peer_id));
}

pub(crate) fn remove_wave_source_by_id(app: &AppHandle, id: &str) {
    let Ok(rt) = runtime() else {
        return;
    };
    let id = id.to_string();
    let enabled = remote_enabled(app);
    let removed = rt.with_library(|library| {
        library.delete_device_playlist(&id)?;
        let cached = library.tombstone_remote_source(&id)?;
        for uri in cached {
            delete_cache_files(library, &uri);
        }
        rt.reload_sources(library);
        library.recompute_remote_visibility(enabled)
    });
    if let Err(e) = removed {
        tracing::warn!("Couldn't remove a forgotten device's music: {e}");
    }
    let _ = app.emit("remote-status-changed", ());
    let _ = app.emit("library-changed", ());
}

/// A paired Wave came into reach or left it: its songs show or hide, and on
/// return its catalog is read again.
pub(crate) fn set_wave_reachable(app: &AppHandle, peer_id: &str, online: bool) {
    let Ok(rt) = runtime() else {
        return;
    };
    let id = wave_source_id(peer_id);
    if rt.backend(&id).is_none() {
        return;
    }
    rt.set_status(
        &id,
        if online {
            SourceState::Ok
        } else {
            SourceState::Unreachable
        },
    );
    super::store::set_source_offline(&id, !online);
    let enabled = remote_enabled(app);
    if let Err(e) = rt.with_library(|library| library.recompute_remote_visibility(enabled)) {
        tracing::warn!("Couldn't update which songs show: {e}");
    }
    if online {
        sync_one_in_background(app, rt, id);
    }
    let _ = app.emit("remote-status-changed", ());
    let _ = app.emit("library-changed", ());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::store::RemoteSourceRow;
    use std::time::{Duration, Instant};

    #[test]
    fn dlna_sources_that_are_paired_waves_are_picked_out() {
        let row = |id: &str, kind: &str, config_json: &str| RemoteSourceRow {
            id: id.into(),
            kind: kind.into(),
            display_name: id.into(),
            config_json: config_json.into(),
            created_at: 0,
            last_sync_at: None,
            last_sync_error: None,
            track_count: 0,
        };
        let dlna = |udn: &str| {
            serde_json::to_string(&super::super::dlna::DlnaConfig {
                location: "http://10.0.0.2:50800/rootDesc.xml".into(),
                udn: udn.into(),
                name: "Wave on Android".into(),
                control_url: "http://10.0.0.2:50800/ctl/ContentDirectory".into(),
                service_type: "urn:schemas-upnp-org:service:ContentDirectory:1".into(),
            })
            .unwrap()
        };
        let rows = [
            row("dlna-phone", "dlna", &dlna("uuid:phone")),
            row("dlna-tv", "dlna", &dlna("uuid:tv")),
            row("wave-phone", "wave", r#"{"peer_id":"phone"}"#),
        ];
        assert_eq!(dlna_copies_of(&rows, &["phone".into()]), ["dlna-phone"]);
        assert!(dlna_copies_of(&rows, &[]).is_empty());
    }

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
    fn reauth_follows_how_the_source_was_added() {
        let row = |kind: &str, config: &str| RemoteSourceRow {
            id: "x".into(),
            kind: kind.into(),
            display_name: "X".into(),
            config_json: config.into(),
            created_at: 1,
            last_sync_at: None,
            last_sync_error: None,
            track_count: 0,
        };
        assert_eq!(
            source_dto(&row("plex", "{\"direct\":true}"), SourceState::Ok).reauth,
            "plex_token"
        );
        assert_eq!(
            source_dto(&row("plex", "{\"direct\":false}"), SourceState::Ok).reauth,
            "plex_tv"
        );
        assert_eq!(
            source_dto(&row("plex", "{}"), SourceState::Ok).reauth,
            "plex_tv"
        );
        assert_eq!(
            source_dto(&row("subsonic", "{}"), SourceState::Ok).reauth,
            "password"
        );
        assert_eq!(
            source_dto(&row("jellyfin", "{}"), SourceState::Ok).reauth,
            "password"
        );
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
