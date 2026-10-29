// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The mini player: a small window that stays on top, for files opened from
//! the file manager and for anyone who wants Wave out of the way. It loads the
//! same frontend as the main window, which draws the mini layout for `#mini`.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::app_settings::AppSettingsState;
use crate::dto::CloseAction;

pub const LABEL: &str = "mini";
pub const MAIN: &str = "main";

const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 124.0;
/// Gap between the window and the screen edge when it first appears.
const EDGE_GAP: f64 = 24.0;

/// Show the mini player, making it the first time.
pub fn show(app: &AppHandle) -> Result<(), String> {
    let window = match app.get_webview_window(LABEL) {
        Some(window) => window,
        None => build(app)?,
    };
    let _ = window.unminimize();
    window.show().map_err(|e| e.to_string())?;
    let _ = window.set_focus();
    Ok(())
}

fn build(app: &AppHandle) -> Result<WebviewWindow, String> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html#mini".into()))
        .title("Wave")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .decorations(false)
        .shadow(true)
        .always_on_top(true)
        .visible(false)
        .build()
        .map_err(|e| format!("Could not open the mini player: {e}"))?;
    place_in_corner(&window);
    Ok(window)
}

/// Bottom right of the screen the main window is on, clear of the dock or
/// taskbar on most setups.
fn place_in_corner(window: &WebviewWindow) {
    let monitor = window
        .app_handle()
        .get_webview_window(MAIN)
        .and_then(|main| main.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let x = area.position.x as f64 / scale + area.size.width as f64 / scale - WIDTH - EDGE_GAP;
    let y = area.position.y as f64 / scale + area.size.height as f64 / scale - HEIGHT - EDGE_GAP;
    let _ = window.set_position(tauri::LogicalPosition::new(x, y));
}

/// Bring back the full window, putting the mini player away.
pub fn expand(app: &AppHandle) -> Result<(), String> {
    if let Some(main) = app.get_webview_window(MAIN) {
        let _ = main.unminimize();
        main.show().map_err(|e| e.to_string())?;
        let _ = main.set_focus();
    }
    if let Some(mini) = app.get_webview_window(LABEL) {
        let _ = mini.hide();
    }
    Ok(())
}

/// Swap the full window for the mini player.
pub fn collapse(app: &AppHandle) -> Result<(), String> {
    show(app)?;
    if let Some(main) = app.get_webview_window(MAIN) {
        let _ = main.hide();
    }
    Ok(())
}

fn main_is_visible(app: &AppHandle) -> bool {
    app.get_webview_window(MAIN)
        .and_then(|main| main.is_visible().ok())
        .unwrap_or(false)
}

/// Closing the mini player only puts it away while the full window is up.
/// When it is the only window, it does what closing the main window does.
pub fn close(app: &AppHandle) {
    let Some(mini) = app.get_webview_window(LABEL) else {
        return;
    };
    if main_is_visible(app) {
        let _ = mini.hide();
        return;
    }
    crate::commands::listen_flush_partial(app);
    crate::commands::persist_playback_state(app);
    let action = app
        .try_state::<AppSettingsState>()
        .and_then(|state| state.0.lock().ok().map(|s| s.close_action))
        .unwrap_or(CloseAction::Quit);
    match action {
        CloseAction::Minimize => {
            let _ = mini.minimize();
        }
        CloseAction::Quit if !crate::share::in_background(app) => app.exit(0),
        _ => {
            let _ = mini.hide();
        }
    }
}
