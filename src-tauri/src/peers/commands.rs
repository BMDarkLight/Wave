// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! What the window can do with other Waves: see them, ask one to share,
//! answer a request, change what is shared, and forget one.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::api::{AnswerBody, AskBody};
use super::{client, service, ShareScope};
use crate::commands::LibraryState;

/// A Wave as the window lists it.
#[derive(Debug, Clone, Serialize)]
pub struct WavePeerDto {
    pub id: String,
    pub name: String,
    pub online: bool,
    pub paired: bool,
    /// "everything", "songs" or "nothing": what this side shares with it.
    pub share: String,
    pub share_count: usize,
    /// It asked to share and waits for an answer.
    pub asking: bool,
    /// This side asked and waits for its answer.
    pub asked: bool,
    pub playlist_id: Option<String>,
    /// It streamed a song from this Wave in the last few seconds.
    pub playing_from_me: bool,
}

/// What to share with a device, as the window says it.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShareChoice {
    Everything,
    /// Songs picked in the song picker, by path.
    Songs {
        paths: Vec<String>,
    },
    Nothing,
}

/// A device streaming within this long counts as playing from this one.
const PLAYING_WITHIN: std::time::Duration = std::time::Duration::from_secs(20);

fn scope_from(app: &AppHandle, choice: ShareChoice) -> Result<ShareScope, String> {
    Ok(match choice {
        ShareChoice::Everything => ShareScope::Everything,
        ShareChoice::Nothing => ShareScope::Nothing,
        ShareChoice::Songs { paths } => {
            let library = app
                .try_state::<LibraryState>()
                .ok_or("The library isn't open")?;
            let tracks = library
                .0
                .lock()
                .map_err(|_| "The library is busy")?
                .get_tracks_by_paths(&paths)?;
            ShareScope::Tracks(tracks.into_iter().flatten().map(|t| t.id).collect())
        }
    })
}

fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| format!("Background task failed: {e}"))?
}

#[tauri::command]
pub async fn list_wave_peers(app: AppHandle) -> Result<Vec<WavePeerDto>, String> {
    blocking(move || {
        let service = service(&app);
        let host = service.host().clone();
        let peers = host.peers();
        let playlists = app
            .try_state::<LibraryState>()
            .and_then(|l| l.0.lock().ok().and_then(|l| l.list_playlists(None).ok()))
            .unwrap_or_default();
        let registry = service.registry.lock();
        let mut list: Vec<WavePeerDto> = peers
            .iter()
            .map(|peer| {
                let source = crate::remote::commands::wave_source_id(&peer.id);
                let (share, share_count) = match &peer.share {
                    ShareScope::Everything => ("everything", 0),
                    ShareScope::Tracks(ids) => ("songs", ids.len()),
                    ShareScope::Nothing => ("nothing", 0),
                };
                WavePeerDto {
                    id: peer.id.clone(),
                    name: peer.name.clone(),
                    online: registry.found.contains_key(&peer.id),
                    paired: true,
                    share: share.to_string(),
                    share_count,
                    asking: false,
                    asked: false,
                    playlist_id: playlists
                        .iter()
                        .find(|p| p.peer_source_id.as_deref() == Some(source.as_str()))
                        .map(|p| p.id.clone()),
                    playing_from_me: registry
                        .streaming
                        .get(&peer.id)
                        .is_some_and(|at| at.elapsed() < PLAYING_WITHIN),
                }
            })
            .collect();
        for (id, found) in &registry.found {
            if peers.iter().any(|p| &p.id == id) {
                continue;
            }
            let pending = registry.pending.get(id);
            list.push(WavePeerDto {
                id: id.clone(),
                name: found.name.clone(),
                online: true,
                paired: false,
                share: "nothing".into(),
                share_count: 0,
                asking: pending.is_some_and(|p| p.incoming),
                asked: pending.is_some_and(|p| !p.incoming),
                playlist_id: None,
                playing_from_me: false,
            });
        }
        list.sort_by(|a, b| b.paired.cmp(&a.paired).then(a.name.cmp(&b.name)));
        Ok(list)
    })
    .await
}

/// Ask a Wave on the network to share. It answers through `/wave/answer`.
#[tauri::command]
pub async fn connect_wave_peer(id: String, app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let service = service(&app);
        let found = service
            .registry
            .lock()
            .found
            .get(&id)
            .cloned()
            .ok_or("That device isn't on the network right now")?;
        let port = crate::share::port().ok_or("Turn on Outside Sourcing first")?;
        let (own_id, own_name) = service.host().identity();
        let secret = secret();
        service.asked(&id, &found.name, &found.address, &secret);
        client::ask(
            &found.address,
            &AskBody {
                id: own_id,
                name: own_name,
                port,
                secret,
            },
        )
        .map_err(|e| e.to_string())?;
        service
            .host()
            .emit("wave-peers-changed", serde_json::json!({}));
        Ok(())
    })
    .await
}

/// Answer a Wave that asked to share: allow it with `share`, or turn it down.
#[tauri::command]
pub async fn answer_wave_request(
    id: String,
    accept: bool,
    share: ShareChoice,
    app: AppHandle,
) -> Result<(), String> {
    blocking(move || {
        let service = service(&app);
        let port = crate::share::port().ok_or("Turn on Outside Sourcing first")?;
        let (own_id, own_name) = service.host().identity();
        if !accept {
            if let Some(pending) = service.decline(&id) {
                let _ = client::answer(
                    &pending.address,
                    &AnswerBody {
                        id: own_id,
                        name: own_name,
                        port,
                        secret: pending.secret,
                        accepted: false,
                    },
                );
            }
            return Ok(());
        }
        let scope = scope_from(&app, share)?;
        let record = service.accept(&id, scope)?;
        client::answer(
            &record.address,
            &AnswerBody {
                id: own_id,
                name: own_name,
                port,
                secret: record.secret.clone(),
                accepted: true,
            },
        )
        .map_err(|e| e.to_string())?;
        service
            .host()
            .emit("wave-peers-changed", serde_json::json!({}));
        Ok(())
    })
    .await
}

/// Change what this Wave shares with a paired one.
#[tauri::command]
pub async fn set_wave_share(id: String, share: ShareChoice, app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let scope = scope_from(&app, share)?;
        let service = service(&app);
        service.host().update_peers(&mut |peers| {
            if let Some(peer) = peers.iter_mut().find(|p| p.id == id) {
                peer.share = scope.clone();
            }
        });
        service
            .host()
            .emit("wave-peers-changed", serde_json::json!({}));
        Ok(())
    })
    .await
}

/// The paths of the songs shared with a device, to start the picker from.
#[tauri::command]
pub async fn get_wave_share_paths(id: String, app: AppHandle) -> Result<Vec<String>, String> {
    blocking(move || {
        let peer = service(&app)
            .host()
            .peers()
            .into_iter()
            .find(|p| p.id == id)
            .ok_or("That device isn't paired")?;
        let ShareScope::Tracks(ids) = peer.share else {
            return Ok(Vec::new());
        };
        let library = app
            .try_state::<LibraryState>()
            .ok_or("The library isn't open")?;
        let library = library.0.lock().map_err(|_| "The library is busy")?;
        Ok(ids
            .iter()
            .filter_map(|id| library.get_track_by_id(id).ok().flatten())
            .map(|t| t.path)
            .collect())
    })
    .await
}

/// Don't offer this device again until Wave restarts.
#[tauri::command]
pub async fn dismiss_wave_peer(id: String, app: AppHandle) -> Result<(), String> {
    service(&app).registry.lock().dismissed.insert(id);
    Ok(())
}

/// Look for other Waves now, instead of at the next round.
#[tauri::command]
pub async fn search_wave_peers() -> Result<(), String> {
    super::discovery::search_now();
    Ok(())
}

/// Play on a paired Wave from now on, starting with what is loaded here.
#[tauri::command]
pub async fn cast_to_wave(id: String, app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let service = service(&app);
        let peer = service
            .host()
            .peers()
            .into_iter()
            .find(|peer| peer.id == id)
            .ok_or("That device isn't paired with this one")?;
        if !service.registry.lock().found.contains_key(&peer.id) {
            return Err(format!("{} isn't on the network right now", peer.name));
        }
        let output = super::cast::CastSender::start(service.clone(), &peer);
        crate::commands::with_app_player(&app, |player| {
            player.cast_to(Box::new(output)).map_err(|e| e.to_string())
        })?;
        let _ = app.emit("wave-cast-changed", ());
        Ok(())
    })
    .await
}

/// Play on this device again, from where the other one was.
#[tauri::command]
pub async fn stop_casting(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        crate::commands::with_app_player(&app, |player| {
            player.end_cast(true).map_err(|e| e.to_string())
        })?;
        let _ = app.emit("wave-cast-changed", ());
        Ok(())
    })
    .await
}

/// Forget a paired device on both sides.
#[tauri::command]
pub async fn forget_wave_peer(id: String, app: AppHandle) -> Result<(), String> {
    blocking(move || {
        super::forget_by_source(&app, &crate::remote::commands::wave_source_id(&id));
        Ok(())
    })
    .await
}
