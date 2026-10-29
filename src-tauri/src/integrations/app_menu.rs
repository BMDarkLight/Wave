// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The macOS menu bar. Its playback and view commands go to the frontend as
//! `menu-command` events, which run the same code as the keyboard shortcuts
//! on Windows and Linux (`useAppCommands`). Those two have no menu bar: a
//! native one would sit above Wave's own dark window chrome.

use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Wry};

/// Menu ids that are frontend commands carry this prefix.
const COMMAND: &str = "cmd:";
/// Commands that show something in the main window, which has to be up.
/// Playback commands work from the mini player too.
const NEEDS_MAIN_WINDOW: &[&str] = &[
    "search",
    "queue",
    "lyrics",
    "settings",
    "check_updates",
    "add_folder",
    "new_playlist",
];

fn command(
    app: &AppHandle,
    name: &str,
    label: &str,
    accelerator: Option<&str>,
) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(app, format!("{COMMAND}{name}"), label, true, accelerator)
}

pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let sep = || PredefinedMenuItem::separator(app);
    let about = AboutMetadata {
        name: Some("Wave".into()),
        version: Some(app.package_info().version.to_string()),
        copyright: Some("Copyright (C) 2025 BMDarkLight".into()),
        license: Some("AGPL-3.0-or-later".into()),
        website: Some("https://github.com/BMDarkLight/Wave".into()),
        ..AboutMetadata::default()
    };

    let wave = Submenu::with_items(
        app,
        "Wave",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Wave"), Some(about))?,
            &command(app, "check_updates", "Check for Updates...", None)?,
            &sep()?,
            &command(app, "settings", "Settings...", Some("CmdOrCtrl+,"))?,
            &sep()?,
            &PredefinedMenuItem::services(app, None)?,
            &sep()?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &sep()?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &command(app, "open_files", "Open...", Some("CmdOrCtrl+O"))?,
            &command(
                app,
                "add_folder",
                "Add Folder as Playlist...",
                Some("CmdOrCtrl+Shift+O"),
            )?,
            &command(app, "new_playlist", "New Playlist", Some("CmdOrCtrl+N"))?,
            &sep()?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    // Text fields need these for copy and paste to work at all on macOS.
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &sep()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let playback = Submenu::with_items(
        app,
        "Controls",
        true,
        &[
            &command(app, "play_pause", "Play / Pause", None)?,
            &command(app, "next", "Next", Some("CmdOrCtrl+Right"))?,
            &command(app, "previous", "Previous", Some("CmdOrCtrl+Left"))?,
            &sep()?,
            &command(app, "volume_up", "Increase Volume", Some("CmdOrCtrl+Up"))?,
            &command(
                app,
                "volume_down",
                "Decrease Volume",
                Some("CmdOrCtrl+Down"),
            )?,
            &sep()?,
            &command(app, "shuffle", "Shuffle", Some("CmdOrCtrl+S"))?,
            &command(app, "repeat", "Repeat", Some("CmdOrCtrl+R"))?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &command(app, "search", "Search", Some("CmdOrCtrl+F"))?,
            &command(app, "queue", "Queue", Some("CmdOrCtrl+U"))?,
            &command(app, "lyrics", "Lyrics", Some("CmdOrCtrl+L"))?,
            &sep()?,
            &command(app, "mini_player", "Mini Player", Some("CmdOrCtrl+Shift+M"))?,
            &sep()?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &sep()?,
            &MenuItem::with_id(app, "show_main", "Wave", true, Some("CmdOrCtrl+0"))?,
        ],
    )?;
    let help = Submenu::with_items(
        app,
        "Help",
        true,
        &[&MenuItem::with_id(
            app,
            "open_repository",
            "Wave on GitHub",
            true,
            None::<&str>,
        )?],
    )?;

    Menu::with_items(
        app,
        &[&wave, &file, &edit, &playback, &view, &window, &help],
    )
}

pub fn handle(app: &AppHandle, id: &str) {
    if let Some(name) = id.strip_prefix(COMMAND) {
        if NEEDS_MAIN_WINDOW.contains(&name) {
            let _ = super::mini_player::expand(app);
        }
        let _ = app.emit_to(super::mini_player::MAIN, "menu-command", name);
        return;
    }
    match id {
        "show_main" => {
            let _ = super::mini_player::expand(app);
        }
        "open_repository" => {
            let _ = crate::app::updater::open_repository();
        }
        _ => {}
    }
}
