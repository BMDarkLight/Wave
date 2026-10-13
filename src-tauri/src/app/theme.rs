// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The theme choice from Settings, applied to the native window.
//!
//! The page draws its own colors from the setting. This keeps what the page
//! cannot draw in step with it: the title bar and window frame, and the
//! webview's scrollbars and form controls.

use tauri::{AppHandle, Manager, Runtime, Theme, WebviewWindow};

use crate::dto::ThemeMode;

/// Apply `mode` to every open window.
pub fn apply<R: Runtime>(app: &AppHandle<R>, mode: ThemeMode) {
    for window in app.webview_windows().values() {
        apply_to(window, mode);
    }
}

/// Apply `mode` to one window. The system choice needs nothing at launch,
/// since a new window already follows the system.
pub fn apply_at_launch<R: Runtime>(window: &WebviewWindow<R>, mode: ThemeMode) {
    if mode != ThemeMode::System {
        apply_to(window, mode);
    }
}

/// Put a chosen theme back after the system changed it underneath.
///
/// Only Linux needs this: there a change of the desktop's color scheme
/// reaches every window, whatever theme it was given.
pub fn keep_chosen<R: Runtime>(window: &WebviewWindow<R>, mode: ThemeMode, shown: Theme) {
    if let Some(chosen) = forced(mode) {
        if chosen != shown {
            let _ = window.set_theme(Some(chosen));
        }
    }
}

fn forced(mode: ThemeMode) -> Option<Theme> {
    match mode {
        ThemeMode::System => None,
        ThemeMode::Light => Some(Theme::Light),
        ThemeMode::Dark => Some(Theme::Dark),
    }
}

fn apply_to<R: Runtime>(window: &WebviewWindow<R>, mode: ThemeMode) {
    let _ = window.set_theme(forced(mode));
    // On Linux, clearing the theme switches GTK to light rather than back to
    // the desktop's color scheme. With no theme set the window does report
    // that scheme, so read it and set it.
    #[cfg(target_os = "linux")]
    if mode == ThemeMode::System {
        if let Ok(system) = window.theme() {
            let _ = window.set_theme(Some(system));
        }
    }
}
