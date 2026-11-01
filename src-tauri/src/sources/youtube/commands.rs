// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Settings-page commands for Online play: whether its programs are
//! installed, installing them once the user agrees, and removing them.

use serde::Serialize;
use tauri::AppHandle;

/// What the settings page needs to show the Online play switch.
#[derive(Debug, Clone, Serialize)]
pub struct OnlinePlayStatusDto {
    /// False on Android and on machines yt-dlp or Deno have no build for.
    pub supported: bool,
    pub installed: bool,
    /// Release tags of the installed programs.
    pub ytdlp_version: Option<String>,
    pub deno_version: Option<String>,
}

#[cfg(not(target_os = "android"))]
mod desktop {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use serde::Serialize;
    use tauri::{AppHandle, Emitter};

    use super::super::tools::{self, Progress, Tool, TOOLS_DIR};
    use super::OnlinePlayStatusDto;

    pub const PROGRESS_EVENT: &str = "online-play-install-progress";
    const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

    /// One install at a time, however many times the switch is flipped.
    static INSTALLING: AtomicBool = AtomicBool::new(false);

    #[derive(Debug, Clone, Serialize)]
    struct ProgressDto {
        tool: &'static str,
        received: u64,
        total: u64,
    }

    pub fn tools_dir(app: &AppHandle) -> Result<PathBuf, String> {
        crate::app_paths::app_data_dir(app).map(|dir| dir.join(TOOLS_DIR))
    }

    pub fn status(app: &AppHandle) -> Result<OnlinePlayStatusDto, String> {
        let dir = tools_dir(app)?;
        let version = |tool| tools::installed(&dir, tool).map(|i| i.tag);
        Ok(OnlinePlayStatusDto {
            supported: tools::supported(),
            installed: tools::all_installed(&dir),
            ytdlp_version: version(Tool::YtDlp),
            deno_version: version(Tool::Deno),
        })
    }

    pub fn install(app: &AppHandle) -> Result<OnlinePlayStatusDto, String> {
        if INSTALLING.swap(true, Ordering::SeqCst) {
            return Err("Online play is already being installed".into());
        }
        let result = (|| {
            let dir = tools_dir(app)?;
            let mut last_report: Option<Instant> = None;
            tools::install_all(&dir, &mut |p: Progress| {
                let done = p.received == p.total;
                if done || last_report.is_none_or(|t| t.elapsed() >= PROGRESS_INTERVAL) {
                    last_report = Some(Instant::now());
                    let _ = app.emit(
                        PROGRESS_EVENT,
                        ProgressDto {
                            tool: p.tool.key(),
                            received: p.received,
                            total: p.total,
                        },
                    );
                }
            })?;
            status(app)
        })();
        INSTALLING.store(false, Ordering::SeqCst);
        result
    }

    pub fn refresh(app: &AppHandle) {
        // An install in progress fetches the latest anyway.
        if INSTALLING.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(dir) = tools_dir(app) {
            tools::refresh_if_stale(&dir);
        }
        INSTALLING.store(false, Ordering::SeqCst);
    }

    pub fn remove(app: &AppHandle) -> Result<OnlinePlayStatusDto, String> {
        if INSTALLING.load(Ordering::SeqCst) {
            return Err("Wait for the install to finish first".into());
        }
        tools::remove_all(&tools_dir(app)?)?;
        status(app)
    }
}

/// Where yt-dlp and Deno live.
#[cfg(not(target_os = "android"))]
pub fn tools_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    desktop::tools_dir(app)
}

/// Whether both programs are on disk, so Online play may be switched on.
pub fn tools_installed(app: &AppHandle) -> bool {
    #[cfg(not(target_os = "android"))]
    {
        desktop::tools_dir(app).is_ok_and(|dir| super::tools::all_installed(&dir))
    }
    #[cfg(target_os = "android")]
    {
        let _ = app;
        false
    }
}

/// Update the installed programs on a background thread when a day has passed
/// since the last check.
pub fn refresh_in_background(app: &AppHandle) {
    #[cfg(not(target_os = "android"))]
    {
        let app = app.clone();
        std::thread::spawn(move || desktop::refresh(&app));
    }
    #[cfg(target_os = "android")]
    let _ = app;
}

#[cfg(target_os = "android")]
fn unsupported() -> OnlinePlayStatusDto {
    OnlinePlayStatusDto {
        supported: false,
        installed: false,
        ytdlp_version: None,
        deno_version: None,
    }
}

#[tauri::command]
pub async fn online_play_status(app: AppHandle) -> Result<OnlinePlayStatusDto, String> {
    #[cfg(not(target_os = "android"))]
    return crate::commands::blocking(move || desktop::status(&app)).await;
    #[cfg(target_os = "android")]
    {
        let _ = app;
        Ok(unsupported())
    }
}

/// Download yt-dlp and Deno. Only called after the user agreed to it; reports
/// progress on `online-play-install-progress`.
#[tauri::command]
pub async fn online_play_install(app: AppHandle) -> Result<OnlinePlayStatusDto, String> {
    #[cfg(not(target_os = "android"))]
    return crate::commands::blocking(move || desktop::install(&app)).await;
    #[cfg(target_os = "android")]
    {
        let _ = app;
        Err("Online play is not available on Android".into())
    }
}

/// Delete both programs. The caller turns the setting off.
#[tauri::command]
pub async fn online_play_remove(app: AppHandle) -> Result<OnlinePlayStatusDto, String> {
    #[cfg(not(target_os = "android"))]
    return crate::commands::blocking(move || desktop::remove(&app)).await;
    #[cfg(target_os = "android")]
    {
        let _ = app;
        Ok(unsupported())
    }
}
