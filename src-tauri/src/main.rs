// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Release builds are GUI-subsystem on Windows and therefore start with no
    // stdio at all. Anything with arguments is a terminal invocation, so borrow
    // the parent's console before a single line is printed. No-op elsewhere.
    if args.len() > 1 {
        wave_lib::win_console::attach_parent_terminal();
    }

    if args.iter().any(|a| a == "--playback-daemon") {
        let _instance =
            wave_lib::single_instance::try_acquire(wave_lib::single_instance::InstanceMode::Daemon)
                .unwrap_or_else(|e| {
                    eprintln!("{e}");
                    std::process::exit(1);
                });
        wave_lib::playback_daemon::run_daemon();
        return;
    }

    if args.len() > 1 {
        // Playback daemon is running: allow all CLI commands (library + IPC).
        if wave_lib::playback_daemon::daemon_is_running() {
            wave_lib::cli::run();
            return;
        }
        if wave_lib::cli::is_daemon_ipc_client(&args) {
            wave_lib::cli::run();
            return;
        }

        // GUI is running: allow library/metadata CLI, block playback daemon spawn.
        if wave_lib::single_instance::gui_is_running() {
            if wave_lib::cli::conflicts_with_gui(&args) {
                eprintln!(
                    "Wave desktop app is already running. Quit it before starting CLI playback, \
                     or manage playback from the app window."
                );
                std::process::exit(1);
            }
            wave_lib::cli::run();
            return;
        }
        if wave_lib::single_instance::primary_is_running() {
            eprintln!("{}", wave_lib::single_instance::already_running_message());
            std::process::exit(1);
        }
        wave_lib::cli::run();
    } else {
        wave_lib::run();
    }
}
