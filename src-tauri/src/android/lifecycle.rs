// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Keep playing after Android destroys the UI, and close properly after.
//!
//! Swiping Wave out of recents (or backing out of it on older Android)
//! destroys the Activity. Tauri treats its last window going away as a request
//! to exit and would end the process, and the music with it. While something
//! is playing we keep the process; the media service holds it in the
//! foreground, which also stops Android from killing it with the task. The
//! same goes while the library is shared in the background, held by the
//! sharing service.
//!
//! The process then runs "headless": engine, media session and notification,
//! no window. From there:
//! - Opening Wave again starts a new Activity, which gets a new main window.
//! - Pausing keeps the media service in the foreground, since it is all that
//!   holds the process now; without it Android ends the process soon after.
//! - Dismissing the paused notification, or a stop from Bluetooth, ends the
//!   process, since nothing is left for it to do.
//!
//! Headless code must never call anything that dispatches through the
//! Activity (Tauri plugin commands, `app.path()`): with no Activity those
//! panic inside Tauri and abort the process. See `app_paths::app_data_dir`.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager, RunEvent};

use crate::commands;

/// Set once the UI was destroyed while playback kept the process alive.
static HEADLESS: AtomicBool = AtomicBool::new(false);

/// When the last window rebuild was requested, so the tick and the Activity
/// start signal do not both build one while the first is still coming up.
static REOPEN_REQUESTED_MS: AtomicU64 = AtomicU64::new(0);
const REOPEN_RETRY_MS: u64 = 3000;

/// The Activity the previous tick saw, so the tick only acts on one that has
/// been around for a full tick and is surely done with onCreate.
static TICK_SEEN_ACTIVITY: AtomicUsize = AtomicUsize::new(0);

pub fn on_run_event(app: &AppHandle, event: &RunEvent) {
    match event {
        RunEvent::ExitRequested { code, api, .. } => {
            commands::listen_flush_partial(app);
            commands::persist_playback_state(app);
            // `code` is only set for a deliberate exit (exit button, dismissed
            // controls); those always go through.
            // Sharing in the background keeps the process the same way; its
            // own foreground service holds it then.
            let sharing = crate::share::in_background(app);
            if code.is_none() && (commands::is_playback_active(app) || sharing) {
                tracing::info!("UI closed while playing or sharing, keeping the process");
                HEADLESS.store(true, Ordering::Release);
                crate::android::media_bridge::session_set_headless(true);
                api.prevent_exit();
            }
        }
        RunEvent::Exit => {
            // The process ends right after this. Take the notification down
            // synchronously, or a paused one would outlive us with dead
            // buttons.
            crate::android::media_bridge::session_shutdown_now();
        }
        _ => {}
    }
}

/// True while the engine runs without any window.
pub fn is_headless(app: &AppHandle) -> bool {
    HEADLESS.load(Ordering::Acquire) && app.webview_windows().is_empty()
}

/// Called from the playback tick. A fallback: normally the Activity start
/// signal from Java ("ui-started") rebuilds the window first.
pub fn tick(app: &AppHandle) {
    let current = crate::android::jni::live_activity()
        .map(|(_, activity)| activity as usize)
        .unwrap_or(0);
    // Tao registers an Activity a moment before wry can host a webview in it;
    // building in that gap panics. Wait until it has been seen twice.
    if TICK_SEEN_ACTIVITY.swap(current, Ordering::AcqRel) == current {
        ensure_main_window(app);
    }
}

/// Give a freshly started Activity its main window.
///
/// Called when Android reports an Activity start (onStart, so onCreate has
/// fully run) and from the tick. Tauri
/// only creates windows from the config at launch, and its own resume event
/// is per window, so with no window left nothing else would ever build one
/// and the reopened app would stay blank.
pub fn ensure_main_window(app: &AppHandle) {
    if !HEADLESS.load(Ordering::Acquire) {
        return;
    }
    if !app.webview_windows().is_empty() {
        // The rebuilt window is up; back to normal.
        HEADLESS.store(false, Ordering::Release);
        crate::android::media_bridge::session_set_headless(false);
        return;
    }
    // The Activity has to exist before a window can attach to it.
    if crate::android::jni::live_activity().is_none() {
        return;
    }
    // The media worker and the tick can both get here at once; only one of
    // them may build.
    let now = now_ms();
    let last = REOPEN_REQUESTED_MS.load(Ordering::Acquire);
    if now.saturating_sub(last) < REOPEN_RETRY_MS
        || REOPEN_REQUESTED_MS
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }

    // wry tears down a destroyed Activity's webview later, on the UI thread,
    // and finds what to remove by webview id. A rebuilt window's webview has
    // the same id, `main`, so a teardown still queued there would take the new
    // webview's asset and IPC handlers with it and leave the page blank. That
    // happens when Android destroys the old Activity only after starting the
    // new one. This round trip through the same queue waits it out.
    if let Err(error) = tauri::webview_version() {
        tracing::debug!("Waiting for the old webview to be torn down: {error}");
    }
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
        // Stay headless until the window shows up in the manager; if the
        // build fails, the next tick tries again.
        Ok(_) => tracing::info!("Recreated the main window for a new Activity"),
        Err(error) => tracing::error!("Could not recreate the main window: {error}"),
    }
}

/// End the process if it is only running for playback that just ended (the
/// paused notification was dismissed, or playback was stopped).
pub fn exit_if_headless(app: &AppHandle) {
    if is_headless(app) && !crate::share::in_background(app) {
        tracing::info!("Playback ended with no UI open, exiting");
        app.exit(0);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
