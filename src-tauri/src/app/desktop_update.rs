// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Desktop updates through tauri-plugin-updater. The release workflow signs
//! each installer and publishes `latest.json`; the plugin checks that
//! signature before anything is installed. Windows runs the new installer
//! with a progress bar only, macOS and AppImage swap the app in place, and
//! .deb and .rpm installs ask for the admin password once.

use std::time::Instant;

use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

use super::updater::{PROGRESS_EVENT, PROGRESS_INTERVAL};
use crate::dto::UpdateProgressDto;

/// The signed package `latest.json` lists for this computer, if there is one.
/// Errors are left to the caller, which falls back to the release page.
pub fn find(app: &AppHandle) -> Result<Option<Update>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    tauri::async_runtime::block_on(updater.check()).map_err(|e| e.to_string())
}

/// Download `update` and check its signature. `size` comes from the GitHub
/// release, for when the server sends no length.
pub fn download(app: &AppHandle, update: &Update, size: Option<u64>) -> Result<Vec<u8>, String> {
    let mut received: u64 = 0;
    let mut last_report = Instant::now();
    let report = |received: u64, total: u64| {
        let _ = app.emit(PROGRESS_EVENT, UpdateProgressDto { received, total });
    };
    report(0, size.unwrap_or(0));
    let bytes = tauri::async_runtime::block_on(update.download(
        |chunk, length| {
            received += chunk as u64;
            if last_report.elapsed() >= PROGRESS_INTERVAL {
                last_report = Instant::now();
                report(received, length.or(size).unwrap_or(0));
            }
        },
        || {},
    ))
    .map_err(|e| format!("Could not download the update: {e}"))?;
    let total = bytes.len() as u64;
    report(total, total);
    Ok(bytes)
}

/// Install the downloaded package and start the new version. On Windows the
/// installer takes over and Wave exits right away.
pub fn install_and_restart(app: &AppHandle, update: &Update, bytes: &[u8]) -> Result<(), String> {
    update
        .install(bytes)
        .map_err(|e| format!("Could not install the update: {e}"))?;
    std::env::set_var(super::single_instance::RELAUNCH_ENV, "1");
    app.restart()
}

/// Why this copy of Wave should not update itself, said as what to do
/// instead. `None` when the plugin can install the update.
pub fn managed_elsewhere() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        linux::managed_elsewhere()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;
    use std::process::{Command, Stdio};

    use tauri::utils::config::BundleType;
    use tauri::utils::platform::bundle_type;

    /// Whether the package database of `tool` lists Wave's own binary.
    fn owned_by(tool: &str, query: &str) -> bool {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        Command::new(tool)
            .arg(query)
            .arg(exe)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    pub fn managed_elsewhere() -> Option<String> {
        if std::env::var_os("FLATPAK_ID").is_some() {
            return Some("Update it with: flatpak update app.bmdarklight.wave".to_string());
        }
        match bundle_type() {
            Some(BundleType::AppImage) if std::env::var_os("APPIMAGE").is_some() => None,
            // The AUR package unpacks the .deb, so the binary still says Deb.
            Some(BundleType::Deb) if owned_by("dpkg", "-S") => None,
            Some(BundleType::Rpm) if owned_by("rpm", "-qf") => None,
            _ if Path::new("/usr/bin/pacman").exists() => {
                Some("Update it from the AUR, for example with: yay -Syu".to_string())
            }
            _ => Some("Update it with the package manager you installed it from.".to_string()),
        }
    }
}
