mod audio;
mod commands;

use commands::{play_track, PlayerState};
use audio::player::AudioPlayer;

fn main() {
    tauri::Builder::default()
        .manage(PlayerState(std::sync::Mutex::new(AudioPlayer::new())))
        .invoke_handler(tauri::generate_handler![play_track])
        .run(tauri::generate_context!())
        .expect("error running app");
}