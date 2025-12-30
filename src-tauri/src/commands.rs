use std::sync::Mutex;
use crate::audio::player::AudioPlayer;
use crate::playlist::{Playlist, Track};

pub struct PlayerState(pub Mutex<AudioPlayer>);
pub struct PlaylistState(pub Mutex<Playlist>);

#[tauri::command]
pub fn play_track(path: String, state: tauri::State<PlayerState>) -> Result<(), String> {
    state.0.lock().unwrap().play(&path)
}

#[tauri::command]
pub fn pause_track(state: tauri::State<PlayerState>) -> Result<(), String> {
    state.0.lock().unwrap().pause()
}

#[tauri::command]
pub fn resume_track(state: tauri::State<PlayerState>) -> Result<(), String> {
    state.0.lock().unwrap().resume()
}

#[tauri::command]
pub fn stop_track(state: tauri::State<PlayerState>) -> Result<(), String> {
    state.0.lock().unwrap().stop()
}

#[tauri::command]
pub fn get_playback_state(state: tauri::State<PlayerState>) -> Result<serde_json::Value, String> {
    let player = state.0.lock().unwrap();
    Ok(serde_json::json!({
        "is_playing": player.is_playing(),
        "is_paused": player.is_paused(),
        "current_path": player.get_current_path()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
    }))
}

#[tauri::command]
pub fn add_track_to_playlist(
    path: String,
    playlist: tauri::State<PlaylistState>,
) -> Result<Track, String> {
    Ok(playlist.0.lock().unwrap().add_track(path))
}

#[tauri::command]
pub fn remove_track_from_playlist(
    index: usize,
    playlist: tauri::State<PlaylistState>,
) -> Result<(), String> {
    playlist.0.lock().unwrap().remove_track(index)
}

#[tauri::command]
pub fn get_playlist(playlist: tauri::State<PlaylistState>) -> Result<Vec<Track>, String> {
    Ok(playlist.0.lock().unwrap().get_tracks())
}

#[tauri::command]
pub fn clear_playlist(playlist: tauri::State<PlaylistState>) -> Result<(), String> {
    playlist.0.lock().unwrap().clear();
    Ok(())
}

#[tauri::command]
pub fn play_track_from_playlist(
    index: usize,
    player: tauri::State<PlayerState>,
    playlist: tauri::State<PlaylistState>,
) -> Result<(), String> {
    let track = playlist
        .0
        .lock()
        .unwrap()
        .get_track(index)
        .ok_or("Track not found")?;
    player.0.lock().unwrap().play(&track.path)
}

