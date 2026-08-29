mod app;
mod audio;
pub mod cli;
mod commands;
mod cover_art;
mod dto;
mod enrichment;
mod error;
mod integrations;
mod library;
mod listen;
mod metadata;
mod path_validation;
mod sources;
pub mod playback_daemon;
mod os_media;
mod android;

pub use app::paths as app_paths;
pub use app::settings as app_settings;
pub use app::single_instance;
pub use integrations::gui_tray;
pub use integrations::media_controls;

use app_settings::AppSettingsState;
use commands::{EnrichmentState, LibraryState, ListenState, MediaBridgeState, PlayerState};
use dto::CloseAction;
use listen::ListenTracker;
use tauri::{Manager, WindowEvent};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Single-instance locking is a desktop concern. On Android, HOME/cwd-based
    // lock paths are unreliable and can abort launch before the UI starts.
    #[cfg(not(target_os = "android"))]
    let _instance = single_instance::try_acquire(single_instance::InstanceMode::Gui)
        .unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .init();

    os_media::set_app_user_model_id("app.bmdarklight.wave");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        // No-op on desktop; on Android/iOS owns MediaSession + media notification.
        .plugin(tauri_plugin_media_session::init())
        .setup(|app| {
            let settings = app_settings::AppSettings::load(app.handle());

            // Previews are session-scoped clips, never library content. Drop
            // any left by the last run rather than accumulating audio the user
            // never chose to keep.
            sources::cache::clear_previews();

            // Defer audio device creation until first playback command. Opening
            // cpal/oboe during setup can panic on Android before JNI is ready.
            // The in-memory player itself is safe to initialize now, allowing
            // persisted volume and EQ values to be restored before first play.
            let mut player = audio::player::AudioPlayer::new_deferred();
            player.set_volume(settings.volume)?;
            player.set_eq_bands(settings.equalizer.bands);
            player.set_eq_enabled(settings.equalizer.enabled);
            player.set_crossfade_duration(settings.equalizer.crossfade_duration);
            player.set_gapless_enabled(settings.gapless_enabled);
            player.set_volume_normalization_enabled(settings.volume_normalization_enabled);

            // Restore playback state from last session.
            if !settings.last_queue.is_empty() {
                player.queue.set_tracks(settings.last_queue.clone());
                if let Some(idx) = settings.last_queue_index {
                    if idx < settings.last_queue.len() {
                        let _ = player.queue.jump(idx);
                    }
                }
            }
            player.queue.set_shuffle(settings.shuffle);
            player.repeat = settings.repeat.clone();

            app.manage(PlayerState(std::sync::Mutex::new(Some(player))));

            app.manage(AppSettingsState(std::sync::Mutex::new(settings)));
            app.manage(ListenState(std::sync::Mutex::new(ListenTracker::new())));

            let library = library::Library::new(app.handle())?;
            app.manage(LibraryState(std::sync::Mutex::new(library)));
            app.manage(EnrichmentState(std::sync::atomic::AtomicBool::new(false)));

            let app_handle = app.handle().clone();
            app.manage(MediaBridgeState(media_controls::MediaBridgeState::new(
                app_handle.clone(),
            )));

            // SMTC requires a valid HWND — initialize on the UI thread once the window exists.
            if let Some(window) = app.get_webview_window("main") {
                let init_handle = app_handle.clone();
                window.on_window_event(move |event| {
                    if matches!(
                        event,
                        WindowEvent::Focused(true)
                            | WindowEvent::Resized(_)
                            | WindowEvent::ScaleFactorChanged { .. }
                    ) {
                        if let Some(state) = init_handle.try_state::<MediaBridgeState>() {
                            state.0.ensure_initialized_main();
                        }
                    }
                });

                // Schedule init without blocking setup (blocking here deadlocks the UI thread).
                let init_handle = app_handle.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Some(state) = init_handle.try_state::<MediaBridgeState>() {
                        state.0.ensure_initialized_main();
                    }
                });
            } else {
                tracing::warn!("Main window not found — OS media controls will init on first use");
            }

            if let Err(e) = gui_tray::setup(app) {
                tracing::warn!("System tray unavailable: {e}");
            }

            // Auto-advance when the current track ends. The playback daemon
            // does this for headless mode; the GUI needs its own tick —
            // especially on Android, where sink-empty detection via frontend
            // polling alone is unreliable.
            #[cfg(target_os = "android")]
            android::media_bridge::install(app.handle());

            #[cfg(target_os = "android")]
            {
                // Best-effort ExoPlayer warm-up once the Activity exists.
                let exo_app = app_handle.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Err(e) = crate::android::audio::ensure_initialized() {
                        tracing::warn!("ExoPlayer init deferred: {e}");
                    }
                    commands::restore_saved_playback(&exo_app);
                });
            }

            #[cfg(not(target_os = "android"))]
            {
                commands::restore_saved_playback(app.handle());
            }

            let tick_app = app_handle.clone();
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    // Media actions are handled by the dedicated worker; tick
                    // only retries JNI install and auto-advances the queue.
                    #[cfg(target_os = "android")]
                    {
                        android::media_bridge::drain_actions(&tick_app);
                    }
                    commands::tick_listen_progress(&tick_app);
                    #[cfg(target_os = "android")]
                    commands::tick_media_session(&tick_app);
                    commands::tick_auto_advance(&tick_app);
                    commands::persist_playback_state_throttled(&tick_app);
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();

                // Persist playback state before closing.
                commands::listen_flush_partial(app);
                commands::persist_playback_state(app);

                let action = app
                    .try_state::<AppSettingsState>()
                    .and_then(|state| state.0.lock().ok().map(|s| s.close_action))
                    .unwrap_or(CloseAction::Quit);

                if action == CloseAction::HideWindow {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::play_track,
            commands::play_tracks,
            commands::pause_track,
            commands::resume_track,
            commands::stop_track,
            commands::get_playback_state,
            commands::seek_track,
            commands::set_volume,
            commands::add_track_to_playlist,
            commands::remove_track_from_playlist,
            commands::get_playlist,
            commands::clear_playlist,
            commands::add_track_to_favorites,
            commands::remove_track_from_favorites,
            commands::get_favorites,
            commands::is_track_in_favorites,
            commands::is_track_in_playlist,
            commands::toggle_favorite,
            commands::clear_favorites,
            commands::play_track_from_playlist,
            commands::scan_directory,
            commands::index_music_library,
            commands::list_playlists,
            commands::get_library_database_path,
            commands::get_supported_audio_extensions,
            commands::get_queue,
            commands::play_next,
            commands::play_previous,
            commands::set_shuffle,
            commands::set_repeat,
            commands::get_playback_mode,
            commands::update_media_metadata,
            commands::update_media_position,
            commands::clear_media_session,
            commands::create_playlist,
            commands::set_playlist_sync_folder,
            commands::delete_playlist,
            commands::rename_playlist,
            commands::get_playlist_tracks_by_id,
            commands::search_library_tracks,
            commands::search_library,
            commands::search_sources,
            commands::stream_source_track,
            commands::download_source_track,
            commands::get_source_settings,
            commands::set_source_settings,
            commands::add_track_to_playlist_by_id,
            commands::remove_track_from_playlist_by_id,
            commands::remove_track_from_library,
            commands::clear_playlist_by_id,
            commands::fetch_lyrics_for_track,
            commands::play_track_from_specific_playlist,
            commands::create_album_playlist,
            commands::create_artist_playlist,
            commands::list_albums,
            commands::list_artists,
            commands::get_album_tracks,
            commands::get_artist_tracks,
            commands::get_artist_albums,
            commands::get_track_details,
            commands::get_track_full_cover,
            commands::add_to_queue,
            commands::queue_insert_next,
            commands::remove_from_queue,
            commands::move_queue_track,
            commands::clear_queue,
            commands::get_queue_tracks,
            commands::play_track_from_queue,
            commands::export_playlist,
            commands::import_playlist,
            commands::export_lyrics,
            commands::import_lyrics,
            commands::list_output_devices,
            commands::set_output_device,
            commands::get_eq_settings,
            commands::set_eq_bands,
            commands::set_eq_enabled,
            commands::export_eq_settings,
            commands::import_eq_settings,
            commands::get_crossfade_duration,
            commands::set_crossfade_duration,
            commands::get_gapless_enabled,
            commands::set_gapless_enabled,
            commands::get_volume_normalization_enabled,
            commands::set_volume_normalization_enabled,
            commands::get_auto_lyrics_download,
            commands::set_auto_lyrics_download,
            commands::get_close_action,
            commands::set_close_action,
            commands::toggle_close_action,
            commands::host_os,
            commands::take_android_crash_report,
            commands::clear_android_crash_report,
            commands::exit_app,
            commands::import_audio_sources,
            commands::list_media_folders,
            commands::save_media_folder,
            commands::remove_media_folder,
            commands::scan_media_folder,
            commands::import_scanned_audio,
            commands::sync_playlist_folder,
            commands::is_folder_setup_dismissed,
            commands::dismiss_folder_setup,
            commands::pick_media_folder,
            commands::scan_saf_folder,
            commands::clear_audio_imports,
            commands::reset_app,
            commands::get_recently_played,
            commands::get_most_played,
            commands::get_favorite_track,
            commands::get_favorite_album,
            commands::get_favorite_artist,
            commands::get_listening_stats,
            commands::get_home_suggestions,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
