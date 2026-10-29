// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

#[cfg(not(target_os = "android"))]
mod inner {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    #[cfg(target_os = "macos")]
    use tauri::image::Image;
    use tauri::{
        menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
        tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
        AppHandle, Manager, Wry,
    };

    use crate::audio::player::AudioPlayer;
    use crate::audio::sleep_timer::{request_for_menu_choice, SLEEP_MENU};
    use crate::commands::{ensure_player, LibraryState, PlayerState};

    static TRAY_CLICK_STATE: Mutex<Option<Instant>> = Mutex::new(None);
    static TRAY_CLICK_GENERATION: AtomicU64 = AtomicU64::new(0);
    const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);

    /// How often the menu catches up with playback and the playlists.
    const REFRESH: Duration = Duration::from_secs(1);
    /// Longest song line in the menu before it is cut short.
    const MAX_LABEL_CHARS: usize = 48;

    /// The parts of the menu that change while Wave runs.
    struct LiveMenu {
        tray: TrayIcon<Wry>,
        now_playing: MenuItem<Wry>,
        play_pause: MenuItem<Wry>,
        playlists: Submenu<Wry>,
    }

    static LIVE: OnceLock<LiveMenu> = OnceLock::new();

    fn with_player<R>(
        app: &AppHandle,
        f: impl FnOnce(&mut AudioPlayer) -> Result<R, String>,
    ) -> Option<R> {
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
        let playlists = Submenu::with_id(app, "tray_playlists", "Play Playlist", true)?;
        fill_playlists(app.handle(), &playlists, &playlist_entries(app.handle()))?;
        let sleep_sub = Submenu::with_id(app, "tray_sleep", "Sleep Timer", true)?;
        for (choice, label) in SLEEP_MENU {
            let item = MenuItem::with_id(
                app,
                format!("tray_sleep:{choice}"),
                label,
                true,
                None::<&str>,
            )?;
            sleep_sub.append(&item)?;
        }
        let now_playing = MenuItem::with_id(
            app,
            "tray_now_playing",
            "Nothing playing",
            false,
            None::<&str>,
        )?;
        let play_pause = MenuItem::with_id(app, "tray_play_pause", "Play", true, None::<&str>)?;
        let prev = MenuItem::with_id(app, "tray_prev", "Previous", true, None::<&str>)?;
        let next = MenuItem::with_id(app, "tray_next", "Next", true, None::<&str>)?;
        let stop = MenuItem::with_id(app, "tray_stop", "Stop", true, None::<&str>)?;
        let mini = MenuItem::with_id(app, "tray_mini", "Mini Player", true, None::<&str>)?;
        let show = MenuItem::with_id(app, "tray_show", "Show Wave", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "tray_quit", "Quit Wave", true, None::<&str>)?;

        let menu = Menu::with_items(
            app,
            &[
                &now_playing,
                &PredefinedMenuItem::separator(app)?,
                &play_pause,
                &prev,
                &next,
                &stop,
                &PredefinedMenuItem::separator(app)?,
                &playlists,
                &sleep_sub,
                &PredefinedMenuItem::separator(app)?,
                &mini,
                &show,
                &PredefinedMenuItem::separator(app)?,
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

        // A click plays or pauses, a double click brings Wave up, and the
        // menu is on the right click.
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
                    bring_up(tray.app_handle());
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

        let _ = LIVE.set(LiveMenu {
            tray,
            now_playing,
            play_pause,
            playlists,
        });
        let handle = app.handle().clone();
        std::thread::spawn(move || keep_menu_current(&handle));

        Ok(())
    }

    /// What the menu shows, so it is only touched when something changed.
    #[derive(PartialEq, Default)]
    struct Shown {
        song: Option<String>,
        playing: bool,
        playlists: Vec<(String, String)>,
    }

    fn keep_menu_current(app: &AppHandle) {
        let Some(live) = LIVE.get() else {
            return;
        };
        let mut shown = Shown {
            playlists: playlist_entries(app),
            ..Shown::default()
        };
        loop {
            std::thread::sleep(REFRESH);
            let (path, playing) = with_player(app, |p| {
                let path = p
                    .get_current_path()
                    .and_then(|path| path.to_str().map(str::to_string));
                Ok((path, p.is_playing()))
            })
            .unwrap_or((None, false));

            let song = path.and_then(|path| {
                crate::commands::track_for_path(app, &path).map(|track| {
                    let title = if track.title.trim().is_empty() {
                        track.name.clone()
                    } else {
                        track.title.clone()
                    };
                    if track.artist.trim().is_empty() {
                        title
                    } else {
                        format!("{title} - {}", track.artist)
                    }
                })
            });
            if song != shown.song {
                let label = song.as_deref().map_or("Nothing playing".to_string(), short);
                let _ = live.now_playing.set_text(&label);
                let tooltip = song
                    .as_deref()
                    .map_or("Wave".to_string(), |s| format!("Wave: {s}"));
                let _ = live.tray.set_tooltip(Some(&tooltip));
                shown.song = song;
            }
            if playing != shown.playing {
                let _ = live
                    .play_pause
                    .set_text(if playing { "Pause" } else { "Play" });
                shown.playing = playing;
            }
            let playlists = playlist_entries(app);
            if playlists != shown.playlists {
                if let Err(e) = fill_playlists(app, &live.playlists, &playlists) {
                    tracing::warn!("Could not update the tray playlists: {e}");
                }
                shown.playlists = playlists;
            }
        }
    }

    fn short(text: &str) -> String {
        if text.chars().count() <= MAX_LABEL_CHARS {
            return text.to_string();
        }
        let cut: String = text.chars().take(MAX_LABEL_CHARS - 3).collect();
        format!("{}...", cut.trim_end())
    }

    /// Ids and names of the playlists the menu offers, at most 30.
    fn playlist_entries(app: &AppHandle) -> Vec<(String, String)> {
        let Some(library_state) = app.try_state::<LibraryState>() else {
            return Vec::new();
        };
        let Ok(lib) = library_state.0.lock() else {
            return Vec::new();
        };
        lib.list_playlists(None)
            .map(|playlists| {
                playlists
                    .into_iter()
                    .take(30)
                    .map(|pl| (pl.id, pl.name))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn fill_playlists<M: Manager<Wry>>(
        manager: &M,
        sub: &Submenu<Wry>,
        entries: &[(String, String)],
    ) -> tauri::Result<()> {
        for item in sub.items()? {
            sub.remove(&item as &dyn IsMenuItem<Wry>)?;
        }
        if entries.is_empty() {
            let none = MenuItem::new(manager, "No playlists yet", false, None::<&str>)?;
            return sub.append(&none);
        }
        for (id, name) in entries {
            let item = MenuItem::with_id(
                manager,
                format!("tray_playlist:{id}"),
                short(name),
                true,
                None::<&str>,
            )?;
            sub.append(&item)?;
        }
        Ok(())
    }

    /// Show whichever window was last in use: the mini player when it is up
    /// and the main window is not, otherwise the main window.
    fn bring_up(app: &AppHandle) {
        let mini_visible = app
            .get_webview_window(crate::integrations::mini_player::LABEL)
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false);
        if mini_visible {
            let _ = crate::integrations::mini_player::show(app);
        } else {
            show_main_window(app);
        }
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
                for chunk in buf.as_chunks::<3>().0 {
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
        if id == "tray_prev" || id == "tray_next" {
            crate::commands::spawn_skip(app, id == "tray_next", |app, path| {
                crate::commands::listen_switch_track(
                    app,
                    &path,
                    crate::listen::ListenEndReason::Skipped,
                );
            });
            return;
        }
        if id == "tray_stop" {
            let _ = with_player(app, |p| p.stop().map_err(|e| e.to_string()));
            crate::commands::listen_flush_partial(app);
            return;
        }
        if id == "tray_show" {
            let _ = crate::integrations::mini_player::expand(app);
            return;
        }
        if id == "tray_mini" {
            let _ = crate::integrations::mini_player::collapse(app);
            return;
        }
        if id == "tray_quit" {
            app.exit(0);
            return;
        }
        if let Some(playlist_id) = id.strip_prefix("tray_playlist:") {
            play_playlist(app, playlist_id);
            return;
        }
        if let Some(request) = id
            .strip_prefix("tray_sleep:")
            .and_then(request_for_menu_choice)
        {
            let _ = with_player(app, |p| p.set_sleep_timer(request));
        }
    }

    fn handle_tray_left_click(app: &AppHandle) {
        let now = Instant::now();
        let mut state = TRAY_CLICK_STATE.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(prev) = *state {
            if now.duration_since(prev) <= DOUBLE_CLICK_WINDOW {
                *state = None;
                TRAY_CLICK_GENERATION.fetch_add(1, Ordering::SeqCst);
                bring_up(app);
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

        // Stopped, so resume the last loaded track if we still have one.
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
