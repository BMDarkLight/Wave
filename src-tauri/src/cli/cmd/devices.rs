// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Audio output devices and volume.

use crate::audio::player::AudioPlayer;
use crate::cli::{daemon_cmd, json, ui, DevicesCmd};
use crate::playback_daemon::DaemonRequest;

pub fn run(cmd: DevicesCmd) {
    match cmd {
        DevicesCmd::List => cmd_devices_list(),
        DevicesCmd::Switch { name } => cmd_devices_switch(name),
        DevicesCmd::Volume { level } => cmd_devices_volume(level),
    }
}

fn cmd_devices_list() {
    let devices = AudioPlayer::list_output_devices();
    let current = AudioPlayer::current_output_name();
    json::maybe_emit(&serde_json::json!({ "devices": devices, "default": current }));

    let ui = ui::current();
    if devices.is_empty() {
        println!("No audio output devices found.");
        return;
    }
    println!(
        "{}\n",
        ui.heading(&format!("{} output devices", devices.len()))
    );
    for device in &devices {
        if *device == current {
            let line = format!("{} {device}", ui.glyphs.playing);
            println!(
                "  {}  {}",
                ui.paint(ui::style::OK, &line),
                ui.dim("default")
            );
        } else {
            println!("    {device}");
        }
    }
}

fn cmd_devices_switch(name: String) {
    daemon_cmd(DaemonRequest::SetDevice { name });
}

fn cmd_devices_volume(level: f32) {
    daemon_cmd(DaemonRequest::Volume { level });
}
