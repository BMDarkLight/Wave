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
use crate::playback_daemon::{daemon_request_if_running, DaemonRequest};

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
        ui.heading(&ui::count(devices.len(), "output device", "output devices"))
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

/// A volume the user typed, before any step is applied to the current level.
#[derive(Debug, PartialEq)]
enum VolumeInput {
    /// 0.0 to 1.0.
    Level(f32),
    /// Percentage points to move by.
    Step(f32),
}

/// "50%" and "50" are percentages, "0.5" is a fraction (so the older
/// 0.0 to 1.0 form keeps working), and a leading sign makes it a step.
fn parse_volume(raw: &str) -> Result<VolumeInput, String> {
    let text = raw.trim();
    let bad = || format!("Can't read \"{raw}\" as a volume; try 50%, 0.5, or +10.");
    let signed = text.starts_with(['+', '-']);
    let number = text.trim_end_matches('%').trim();
    let value: f32 = number.parse().map_err(|_| bad())?;
    if !value.is_finite() {
        return Err(bad());
    }
    if signed {
        return Ok(VolumeInput::Step(value));
    }
    let level = if text.ends_with('%') || value > 1.0 {
        value / 100.0
    } else {
        value
    };
    if !(0.0..=1.0).contains(&level) {
        return Err("Volume must be between 0% and 100%.".to_string());
    }
    Ok(VolumeInput::Level(level))
}

fn current_volume() -> f32 {
    match daemon_request_if_running(DaemonRequest::Status) {
        Ok(Some(resp)) => resp.status.map_or(0.0, |s| s.volume),
        Ok(None) => ui::no_daemon(),
        Err(e) => ui::fail(e, None, ui::EXIT_GENERAL),
    }
}

fn cmd_devices_volume(level: Option<String>) {
    let Some(raw) = level else {
        let volume = current_volume();
        json::maybe_emit(&serde_json::json!({ "volume": volume }));
        let ui = ui::current();
        println!(
            "{} {} {}%",
            ui.dim("Volume"),
            crate::cli::bar::meter(ui, volume as f64, 10),
            (volume * 100.0).round() as i32
        );
        return;
    };
    let level = match parse_volume(&raw) {
        Ok(VolumeInput::Level(level)) => level,
        Ok(VolumeInput::Step(points)) => (current_volume() + points / 100.0).clamp(0.0, 1.0),
        Err(e) => ui::fail(e, None, ui::EXIT_GENERAL),
    };
    daemon_cmd(DaemonRequest::Volume { level });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_and_fractions_both_set_a_level() {
        assert_eq!(parse_volume("50%"), Ok(VolumeInput::Level(0.5)));
        assert_eq!(parse_volume("50"), Ok(VolumeInput::Level(0.5)));
        assert_eq!(parse_volume("0.5"), Ok(VolumeInput::Level(0.5)));
        assert_eq!(parse_volume("1"), Ok(VolumeInput::Level(1.0)));
        assert_eq!(parse_volume("0"), Ok(VolumeInput::Level(0.0)));
        assert_eq!(parse_volume("100%"), Ok(VolumeInput::Level(1.0)));
    }

    #[test]
    fn a_sign_makes_it_a_step_in_percentage_points() {
        assert_eq!(parse_volume("+10"), Ok(VolumeInput::Step(10.0)));
        assert_eq!(parse_volume("-5%"), Ok(VolumeInput::Step(-5.0)));
    }

    #[test]
    fn nonsense_and_out_of_range_volumes_are_refused() {
        assert!(parse_volume("loud").is_err());
        assert!(parse_volume("150").is_err());
        assert!(parse_volume("").is_err());
        assert!(parse_volume("nan").is_err());
    }
}
