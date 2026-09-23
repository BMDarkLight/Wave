// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Equalizer, bass and treble, gapless playback and crossfade.

use crate::cli::{render, ui, DspCmd};
use crate::playback_daemon::{daemon_request_if_running, DaemonRequest, DspStatus};

pub fn run(cmd: DspCmd) {
    use crate::audio::dsp::EqConfig;

    match cmd {
        DspCmd::Presets => {
            println!("Available EQ presets:");
            for (name, desc) in EqConfig::list_presets() {
                println!("  {name:16}  {desc}");
            }
        }
        DspCmd::EqShow => {
            let dsp = load_dsp_status();
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::EqSet { bands } => {
            if bands.len() != 10 {
                ui::fail(
                    format!("Expected exactly 10 EQ band values, got {}.", bands.len()),
                    Some("bands run 31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000 Hz"),
                    ui::EXIT_GENERAL,
                );
            }
            let mut arr = [0.0f32; 10];
            arr.copy_from_slice(&bands);
            let dsp = apply_dsp_request(DaemonRequest::SetEqBands { bands: arr });
            println!("{}", dsp_message_or("EQ bands set and enabled."));
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::EqEnable => {
            let dsp = apply_dsp_request(DaemonRequest::SetEqEnabled { enabled: true });
            println!("{}", dsp_message_or("Equalizer enabled."));
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::EqDisable => {
            let dsp = apply_dsp_request(DaemonRequest::SetEqEnabled { enabled: false });
            println!("{}", dsp_message_or("Equalizer disabled."));
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::EqReset => {
            let dsp = apply_dsp_request(DaemonRequest::ResetEq);
            println!("{}", dsp_message_or("Equalizer reset to flat and enabled."));
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::Preset { name } => {
            let dsp = apply_dsp_request(DaemonRequest::ApplyEqPreset { name: name.clone() });
            println!("Applied EQ preset: {name}");
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::Export { output, name } => {
            let dsp = load_dsp_status();
            let eq = EqConfig {
                bands: dsp.bands,
                enabled: dsp.eq_enabled,
                crossfade_duration: dsp.crossfade_duration,
            };
            crate::audio::dsp::EqPresetFile::save_to(&output, &eq, name).unwrap_or_else(|e| {
                ui::fail(format!("Failed to export EQ: {e}"), None, ui::EXIT_GENERAL);
            });
            println!("EQ settings exported to {output}");
        }
        DspCmd::Import { input } => {
            let eq = crate::audio::dsp::EqPresetFile::load_from(&input).unwrap_or_else(|e| {
                ui::fail(format!("Failed to import EQ: {e}"), None, ui::EXIT_GENERAL);
            });
            let mut dsp = apply_dsp_request(DaemonRequest::SetEqBands { bands: eq.bands });
            if !eq.enabled {
                dsp = apply_dsp_request(DaemonRequest::SetEqEnabled { enabled: false });
            }
            // Match GUI: only apply crossfade from the file when it is explicitly > 0.
            if eq.crossfade_duration > 0.0 {
                dsp = apply_dsp_request(DaemonRequest::SetCrossfade {
                    seconds: eq.crossfade_duration,
                });
            }
            println!("EQ settings imported from {input} and applied.");
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::Gapless { state } => match state {
            None => {
                let dsp = load_dsp_status();
                println!(
                    "Gapless playback: {}",
                    if dsp.gapless_enabled { "ON" } else { "OFF" }
                );
            }
            Some(raw) => {
                let enabled = parse_on_off(&raw, "gapless");
                let dsp = apply_dsp_request(DaemonRequest::SetGapless { enabled });
                println!(
                    "Gapless playback: {}",
                    if dsp.gapless_enabled { "ON" } else { "OFF" }
                );
            }
        },
        DspCmd::Crossfade { seconds } => match seconds {
            None => {
                let dsp = load_dsp_status();
                if dsp.crossfade_duration <= 0.0 {
                    println!("Crossfade: OFF");
                } else {
                    println!("Crossfade: {:.1}s", dsp.crossfade_duration);
                }
            }
            Some(secs) => {
                if !secs.is_finite() || !(0.0..=8.0).contains(&secs) {
                    ui::fail(
                        "Crossfade must be between 0 and 8 seconds.",
                        None,
                        ui::EXIT_GENERAL,
                    );
                }
                let dsp = apply_dsp_request(DaemonRequest::SetCrossfade { seconds: secs });
                if dsp.crossfade_duration <= 0.0 {
                    println!("Crossfade: OFF");
                } else {
                    println!("Crossfade: {:.1}s", dsp.crossfade_duration);
                }
            }
        },
        DspCmd::Bass { db } => match db {
            None => {
                let dsp = load_dsp_status();
                println!("Bass: {:+.1} dB", dsp.bass);
            }
            Some(value) => {
                if !value.is_finite() || !(-12.0..=12.0).contains(&value) {
                    ui::fail(
                        "Bass must be between -12 and +12 dB.",
                        None,
                        ui::EXIT_GENERAL,
                    );
                }
                let dsp = apply_dsp_request(DaemonRequest::SetBass { db: value });
                println!("Bass: {:+.1} dB  (treble {:+.1} dB)", dsp.bass, dsp.treble);
                println!("{}", render::eq_bands_brief(ui::current(), &dsp));
            }
        },
        DspCmd::Treble { db } => match db {
            None => {
                let dsp = load_dsp_status();
                println!("Treble: {:+.1} dB", dsp.treble);
            }
            Some(value) => {
                if !value.is_finite() || !(-12.0..=12.0).contains(&value) {
                    ui::fail(
                        "Treble must be between -12 and +12 dB.",
                        None,
                        ui::EXIT_GENERAL,
                    );
                }
                let dsp = apply_dsp_request(DaemonRequest::SetTreble { db: value });
                println!("Treble: {:+.1} dB  (bass {:+.1} dB)", dsp.treble, dsp.bass);
                println!("{}", render::eq_bands_brief(ui::current(), &dsp));
            }
        },
    }
}

thread_local! {
    static LAST_DSP_MESSAGE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn dsp_message_or(fallback: &str) -> String {
    LAST_DSP_MESSAGE.with(|cell| {
        cell.borrow_mut()
            .take()
            .unwrap_or_else(|| fallback.to_string())
    })
}

fn load_dsp_status() -> DspStatus {
    match daemon_request_if_running(DaemonRequest::DspStatus) {
        Ok(Some(resp)) if resp.ok => {
            if let Some(dsp) = resp.dsp {
                return dsp;
            }
        }
        Ok(Some(resp)) => {
            if let Some(err) = resp.error {
                ui::fail(err, None, ui::EXIT_GENERAL);
            }
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
        _ => {}
    }

    let settings = crate::app_settings::AppSettings::load_from_disk();
    DspStatus {
        eq_enabled: settings.equalizer.enabled,
        bands: settings.equalizer.bands,
        crossfade_duration: settings.equalizer.crossfade_duration,
        gapless_enabled: settings.gapless_enabled,
        bass: settings.equalizer.bass_gain(),
        treble: settings.equalizer.treble_gain(),
    }
}

fn apply_dsp_request(request: DaemonRequest) -> DspStatus {
    match daemon_request_if_running(request.clone()) {
        Ok(Some(resp)) => {
            if !resp.ok {
                ui::fail(
                    resp.error.unwrap_or_else(|| "DSP request failed".into()),
                    None,
                    ui::EXIT_GENERAL,
                );
            }
            LAST_DSP_MESSAGE.with(|cell| {
                *cell.borrow_mut() = resp.message.clone();
            });
            if let Some(dsp) = resp.dsp {
                return dsp;
            }
            return load_dsp_status();
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
        Ok(None) => {}
    }

    // No daemon: mutate persisted settings so the next GUI/daemon launch picks them up.
    let mut settings = crate::app_settings::AppSettings::load_from_disk();
    match request {
        DaemonRequest::SetEqBands { bands } => {
            settings.equalizer.bands = bands;
            settings.equalizer.enabled = true;
        }
        DaemonRequest::SetEqEnabled { enabled } => {
            settings.equalizer.enabled = enabled;
        }
        DaemonRequest::ResetEq => {
            settings.equalizer.bands = [0.0; 10];
            settings.equalizer.enabled = true;
        }
        DaemonRequest::ApplyEqPreset { name } => {
            if settings.equalizer.apply_preset(&name).is_none() {
                let names: Vec<&str> = crate::audio::dsp::EqConfig::list_presets()
                    .map(|(n, _)| n)
                    .collect();
                ui::fail(
                    format!("Unknown EQ preset \"{name}\""),
                    Some(&format!("available: {}", names.join(", "))),
                    ui::EXIT_NOT_FOUND,
                );
            }
        }
        DaemonRequest::SetCrossfade { seconds } => {
            settings.equalizer.crossfade_duration = seconds.clamp(0.0, 8.0);
        }
        DaemonRequest::SetGapless { enabled } => {
            settings.gapless_enabled = enabled;
        }
        DaemonRequest::SetBass { db } => {
            let treble = settings.equalizer.treble_gain();
            settings.equalizer.apply_bass_treble(db, treble);
        }
        DaemonRequest::SetTreble { db } => {
            let bass = settings.equalizer.bass_gain();
            settings.equalizer.apply_bass_treble(bass, db);
        }
        _ => {}
    }
    settings.save_to_disk().unwrap_or_else(|e| {
        ui::fail(
            format!("Failed to save settings: {e}"),
            None,
            ui::EXIT_GENERAL,
        );
    });

    DspStatus {
        eq_enabled: settings.equalizer.enabled,
        bands: settings.equalizer.bands,
        crossfade_duration: settings.equalizer.crossfade_duration,
        gapless_enabled: settings.gapless_enabled,
        bass: settings.equalizer.bass_gain(),
        treble: settings.equalizer.treble_gain(),
    }
}

fn parse_on_off(raw: &str, label: &str) -> bool {
    match raw.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "enable" | "enabled" => true,
        "off" | "false" | "0" | "disable" | "disabled" => false,
        _ => {
            ui::fail(
                format!("{label} expects on/off, got \"{raw}\""),
                None,
                ui::EXIT_GENERAL,
            );
        }
    }
}
