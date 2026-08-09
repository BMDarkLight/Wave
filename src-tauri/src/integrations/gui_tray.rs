#[cfg(not(target_os = "android"))]
mod inner {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use tauri::{
        image::Image,
        menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
        tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
        AppHandle, Manager,
    };

    use crate::audio::player::AudioPlayer;
    use crate::commands::{ensure_player, LibraryState, PlayerState};

    static TRAY_CLICK_STATE: Mutex<Option<Instant>> = Mutex::new(None);
    static TRAY_CLICK_GENERATION: AtomicU64 = AtomicU64::new(0);
    const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);

    fn with_player<R>(app: &AppHandle, f: impl FnOnce(&mut AudioPlayer) -> Result<R, String>) -> Option<R> {
        let player_state = app.state::<PlayerState>();
        let mut slot = match player_state.0.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!("Player mutex was poisoned, recovering");
                poisoned.into_inner()
            }
        };
        let Ok(player) = ensure_player(&mut slot) else {
            return None;
        };
        f(player).ok()
    }

    pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
        let playlists_sub = build_playlists_submenu(app)?;
        let play_pause = MenuItem::with_id(app, "tray_play_pause", "Play / Pause", true, None::<&str>)?;
        let prev = MenuItem::with_id(app, "tray_prev", "Previous", true, None::<&str>)?;
        let next = MenuItem::with_id(app, "tray_next", "Next", true, None::<&str>)?;
        let stop = MenuItem::with_id(app, "tray_stop", "Stop", true, None::<&str>)?;
        let show = MenuItem::with_id(app, "tray_show", "Show Window", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "tray_quit", "Quit", true, None::<&str>)?;

        let menu = Menu::with_items(
            app,
            &[
                &playlists_sub,
                &PredefinedMenuItem::separator(app)?,
                &play_pause,
                &prev,
                &next,
                &stop,
                &PredefinedMenuItem::separator(app)?,
                &show,
                &quit,
            ],
        )?;

        #[cfg(target_os = "macos")]
        let icon = load_tray_image()?;
        #[cfg(not(target_os = "macos"))]
        let icon = app
            .default_window_icon()
            .cloned()
            .ok_or("No default window icon")?;

        let tray = TrayIconBuilder::with_id("wave-tray")
            .icon(icon)
            .icon_as_template(cfg!(target_os = "macos"))
            .menu(&menu)
            .tooltip("Wave")
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| handle_menu_event(app, event.id.as_ref()))
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } = event
                {
                    show_main_window(tray.app_handle());
                    return;
                }

                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    handle_tray_left_click(tray.app_handle());
                }
            })
            .build(app)?;

        let _ = tray.set_show_menu_on_left_click(false);

        Ok(())
    }

    fn build_playlists_submenu(app: &mut tauri::App) -> Result<Submenu<tauri::Wry>, tauri::Error> {
        let sub = Submenu::with_id(app, "tray_playlists", "Playlists", true)?;
        if let Some(library_state) = app.try_state::<LibraryState>() {
            if let Ok(lib) = library_state.0.lock() {
                if let Ok(playlists) = lib.list_playlists(None) {
                    for pl in playlists.iter().take(30) {
                        let item = MenuItem::with_id(
                            app,
                            format!("tray_playlist:{}", pl.id),
                            &pl.name,
                            true,
                            None::<&str>,
                        )?;
                        sub.append(&item)?;
                    }
                }
            }
        }
        Ok(sub)
    }

    #[cfg(target_os = "macos")]
    fn load_tray_image() -> tauri::Result<Image<'static>> {
        let bytes = include_bytes!("../../icons/tray-template.png");
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder
            .read_info()
            .map_err(|e| std::io::Error::other(format!("PNG decode: {e}")))?;
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|e| std::io::Error::other(format!("PNG frame: {e}")))?;
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::Rgb => {
                let mut rgba = Vec::with_capacity((buf.len() / 3) * 4);
                for chunk in buf.chunks_exact(3) {
                    rgba.extend_from_slice(chunk);
                    rgba.push(255);
                }
                rgba
            }
            other => {
                return Err(std::io::Error::other(format!(
                    "Unsupported tray PNG color type: {other:?}"
                ))
                .into());
            }
        };
        Ok(Image::new_owned(rgba, info.width, info.height))
    }

    fn handle_menu_event(app: &AppHandle, id: &str) {
        if id == "tray_play_pause" {
            toggle_play_pause(app);
            return;
        }
        if id == "tray_prev" {
            if let Some(path) = with_player(app, |p| {
                p.play_previous().map_err(|e| e.to_string())
            })
            .flatten()
            {
                crate::commands::listen_switch_track(
                    app,
                    &path,
                    crate::listen::ListenEndReason::Skipped,
                );
            }
            return;
        }
        if id == "tray_next" {
            if let Some(path) = with_player(app, |p| {
                p.play_next().map_err(|e| e.to_string())
            })
            .flatten()
            {
                crate::commands::listen_switch_track(
                    app,
                    &path,
                    crate::listen::ListenEndReason::Skipped,
                );
            }
            return;
        }
        if id == "tray_stop" {
            let _ = with_player(app, |p| p.stop().map_err(|e| e.to_string()));
            crate::commands::listen_flush_partial(app);
            return;
        }
        if id == "tray_show" {
            show_main_window(app);
            return;
        }
        if id == "tray_quit" {
            app.exit(0);
            return;
        }
        if let Some(playlist_id) = id.strip_prefix("tray_playlist:") {
            play_playlist(app, playlist_id);
        }
    }

    fn handle_tray_left_click(app: &AppHandle) {
        let now = Instant::now();
        let mut state = TRAY_CLICK_STATE.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(prev) = *state {
            if now.duration_since(prev) <= DOUBLE_CLICK_WINDOW {
                *state = None;
                TRAY_CLICK_GENERATION.fetch_add(1, Ordering::SeqCst);
                show_main_window(app);
                return;
            }
        }

        *state = Some(now);
        let generation = TRAY_CLICK_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(DOUBLE_CLICK_WINDOW);
            if TRAY_CLICK_GENERATION.load(Ordering::SeqCst) == generation {
                toggle_play_pause(&app);
            }
        });
    }

    fn show_main_window(app: &AppHandle) {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }

    fn toggle_play_pause(app: &AppHandle) {
        let player_state = app.state::<PlayerState>();
        let mut slot = match player_state.0.lock() {
            Ok(p) => p,
            Err(poisoned) => {
                tracing::warn!("Player mutex was poisoned, recovering");
                poisoned.into_inner()
            }
        };
        let player = match ensure_player(&mut slot) {
            Ok(p) => p,
            Err(_) => return,
        };

        if player.is_playing() {
            let _ = player.pause();
            return;
        }
        if player.is_paused() {
            let _ = player.resume();
            return;
        }

        // Stopped — resume the last loaded track if we still have one.
        if let Some(path) = player
            .get_current_path()
            .and_then(|p| p.to_str().map(str::to_string))
        {
            let _ = player.play(&path);
            return;
        }

        // Otherwise start from the in-memory queue.
        if !player.queue.tracks().is_empty() {
            let index = player.queue.current_index().unwrap_or(0);
            if let Some(path) = player.queue.jump(index) {
                let path = path.to_string();
                let _ = player.play(&path);
                return;
            }
        }

        drop(slot);
        try_play_default_playlist(app);
    }

    fn try_play_default_playlist(app: &AppHandle) {
        let tracks = {
            let library_state = app.state::<LibraryState>();
            let lib = match library_state.0.lock() {
                Ok(l) => l,
                Err(_) => return,
            };
            match lib.get_default_playlist_tracks() {
                Ok(t) if !t.is_empty() => t,
                _ => return,
            }
        };

        let paths: Vec<String> = tracks.iter().map(|t| t.path.clone()).collect();
        let first_path = paths[0].clone();

        with_player(app, |player| {
            player.queue.set_tracks(paths);
            player.queue.jump(0);
            player.play(&first_path).map_err(|e| e.to_string())
        });
        crate::commands::listen_switch_track(
            app,
            &first_path,
            crate::listen::ListenEndReason::Partial,
        );
    }

    fn play_playlist(app: &AppHandle, playlist_id: &str) {
        let tracks = {
            let library_state = app.state::<LibraryState>();
            let lib = match library_state.0.lock() {
                Ok(l) => l,
                Err(_) => return,
            };
            match lib.get_playlist_tracks(playlist_id) {
                Ok(t) if !t.is_empty() => t,
                _ => return,
            }
        };

        let first_path = tracks[0].path.clone();
        let paths: Vec<String> = tracks.iter().map(|t| t.path.clone()).collect();

        with_player(app, |player| {
            player.queue.set_tracks(paths);
            player.queue.jump(0);
            player.play(&first_path).map_err(|e| e.to_string())
        });
        crate::commands::listen_switch_track(
            app,
            &first_path,
            crate::listen::ListenEndReason::Partial,
        );
    }
}

#[cfg(not(target_os = "android"))]
pub use inner::setup;

#[cfg(target_os = "android")]
pub fn setup(_app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
