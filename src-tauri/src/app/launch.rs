// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Which window Wave opens with. The main window starts hidden (see the
//! platform `tauri.*.conf.json` files) so that a launch to open files can show
//! only the mini player.

use tauri::{AppHandle, Manager};

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use std::sync::OnceLock;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use crate::integrations::mini_player;

/// macOS sends the files to open as an Apple event after launch, not on the
/// command line. Waiting this long for one is not noticeable next to the
/// webview starting up.
#[cfg(target_os = "macos")]
const OPEN_EVENT_WAIT: Duration = Duration::from_millis(350);
/// An open event this soon after launch is what launched Wave.
#[cfg(target_os = "macos")]
const LAUNCH_WINDOW: Duration = Duration::from_secs(4);

#[cfg(target_os = "macos")]
static OPENED_AT_LAUNCH: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "macos")]
static LAUNCHED: OnceLock<Instant> = OnceLock::new();

fn show_main(app: &AppHandle) {
    if let Some(main) = app.get_webview_window(mini_player::MAIN) {
        let _ = main.show();
        let _ = main.set_focus();
    }
}

/// Show the main window, unless files opened in the mini player first.
pub fn show_main_window_soon(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        LAUNCHED.get_or_init(Instant::now);
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(OPEN_EVENT_WAIT);
            if !OPENED_AT_LAUNCH.load(Ordering::SeqCst) {
                let main_app = app.clone();
                let _ = app.run_on_main_thread(move || show_main(&main_app));
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    show_main(app);
}

/// Files from Finder: a double click, "Open With", or a drop on the Dock icon.
#[cfg(target_os = "macos")]
pub fn opened(app: &AppHandle, urls: &[tauri::Url]) {
    let files = crate::app::open_files::audio_files_only(
        urls.iter().filter_map(|url| url.to_file_path().ok()),
    );
    if files.is_empty() {
        return;
    }
    let at_launch = LAUNCHED
        .get()
        .is_some_and(|launched| launched.elapsed() < LAUNCH_WINDOW);
    if at_launch && !OPENED_AT_LAUNCH.swap(true, Ordering::SeqCst) {
        // Slow to arrive, after the main window went up: take it down again.
        if let Some(main) = app.get_webview_window(mini_player::MAIN) {
            let _ = main.hide();
        }
    }
    crate::commands::open_audio_files(app, files, true);
}
