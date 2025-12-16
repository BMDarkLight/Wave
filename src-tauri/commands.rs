use std::sync::Mutex;
use crate::audio::player::AudioPlayer;

pub struct PlayerState(pub Mutex<AudioPlayer>);

#[tauri::command]
pub fn play_track(path: String, state: tauri::State<PlayerState>) {
    state.0.lock().unwrap().play(&path);
}