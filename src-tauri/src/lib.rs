mod audio;
mod commands;
mod playlist;

use commands::{PlayerState, PlaylistState};
use audio::player::AudioPlayer;
use playlist::Playlist;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let player = AudioPlayer::new()
        .expect("Failed to initialize audio player");
    
    let player_state = PlayerState(std::sync::Mutex::new(player));
    let playlist_state = PlaylistState(std::sync::Mutex::new(Playlist::new()));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(player_state)
        .manage(playlist_state)
        .invoke_handler(tauri::generate_handler![
            commands::play_track,
            commands::pause_track,
            commands::resume_track,
            commands::stop_track,
            commands::get_playback_state,
            commands::add_track_to_playlist,
            commands::remove_track_from_playlist,
            commands::get_playlist,
            commands::clear_playlist,
            commands::play_track_from_playlist,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
