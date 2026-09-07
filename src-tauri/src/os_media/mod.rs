// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! OS-level media integration (SMTC flyout, media keys, taskbar controls).
//!
//! Platform code lives in one place per OS:
//! - Windows → `os_media/windows.rs` (everything: AppUserModelID, SMTC, taskbar)
//! - Linux/macOS → souvlaki via `integrations/media_controls.rs`
//! - Android → `tauri-plugin-media-session` (MediaSession + MediaStyle notification)

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::WindowsMedia;

/// Set the Windows AppUserModelID so the shell shows the correct app name.
pub fn set_app_user_model_id(app_id: &str) {
    #[cfg(target_os = "windows")]
    windows::set_app_user_model_id(app_id);
    #[cfg(not(target_os = "windows"))]
    let _ = app_id;
}
