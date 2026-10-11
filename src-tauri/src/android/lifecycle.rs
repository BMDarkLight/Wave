// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Keep playing after Android destroys the UI.
//!
//! Swiping Wave out of recents (or backing out of it on older Android)
//! destroys the Activity. Tauri treats its last window going away as a request
//! to exit and would end the process, and the music with it. While something
//! is playing we keep the process; the media service holds it in the
//! foreground. When the user comes back, Android starts a fresh Activity and
//! we give it a new main window.

use tauri::{AppHandle, Manager, RunEvent};

use crate::commands;

pub fn on_run_event(app: &AppHandle, event: &RunEvent) {
    match event {
        RunEvent::ExitRequested { code, api, .. } => {
            commands::listen_flush_partial(app);
            commands::persist_playback_state(app);
            // `code` is only set for a deliberate exit (exit button, dismissed
            // controls); those always go through.
            if code.is_none() && commands::is_playback_active(app) {
                tracing::info!("UI closed while playing, keeping playback alive");
                api.prevent_exit();
            }
        }
        RunEvent::Resumed => reopen_main_window(app),
        _ => {}
    }
}

/// Recreate the main window for an Activity that started after the previous
/// one was destroyed. The new Activity waits for a window to attach to.
fn reopen_main_window(app: &AppHandle) {
    if !app.webview_windows().is_empty() {
        return;
    }
    let Some(config) = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "main")
        .cloned()
    else {
        return;
    };
    match tauri::WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build()) {
        Ok(_) => tracing::info!("Recreated the main window for a new Activity"),
        Err(error) => tracing::error!("Could not recreate the main window: {error}"),
    }
}
