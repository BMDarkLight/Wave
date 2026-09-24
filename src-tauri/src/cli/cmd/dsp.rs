// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Equalizer, bass and treble, gapless playback and crossfade.

use serde_json::json;

use crate::cli::{json, render, ui, DspCmd};
use crate::playback_daemon::{daemon_request_if_running, DaemonRequest, DspStatus};

pub fn run(cmd: DspCmd) {
    use crate::audio::dsp::EqConfig;

    match cmd {
        DspCmd::Presets => {
            let presets: Vec<_> = EqConfig::list_presets()
                .map(|(name, description)| json!({ "name": name, "description": description }))
                .collect();
            json::maybe_emit(&presets);
            let ui = ui::current();
            println!("{}\n", ui.heading(&format!("{} EQ presets", presets.len())));
            for (name, desc) in EqConfig::list_presets() {
                println!("  {}  {}", ui::pad(name, 12, ui::Align::Left), ui.dim(desc));
            }
            println!("\n  {}", ui.dim("apply one with: wave dsp preset <name>"));
        }
        DspCmd::EqShow => {
            let dsp = load_dsp_status();
            json::maybe_emit(&dsp);
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::EqSet { bands } => {
            if bands.len() != 10 {
                ui::fail(
                    format!("Expected exactly 10 EQ band values, got {}.", bands.len()),
                    Some("to change a single band: wave dsp eq-band <band> <dB>"),
                    ui::EXIT_GENERAL,
                );
            }
            let mut arr = [0.0f32; 10];
            arr.copy_from_slice(&bands);
            let dsp = apply_dsp_request(DaemonRequest::SetEqBands { bands: arr });
            ui::done(
                dsp_message_or("EQ bands set and enabled."),
                json!({ "dsp": dsp }),
            );
            println!();
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::EqBand { band, db } => {
            let Some(index) = parse_band(&band) else {
                ui::fail(
                    format!("No EQ band \"{band}\"."),
                    Some("use 1 to 10, or one of 31, 62, 125, 250, 500, 1k, 2k, 4k, 8k, 16k"),
                    ui::EXIT_GENERAL,
                );
            };
            if !db.is_finite() || !(-12.0..=12.0).contains(&db) {
                ui::fail(
                    "Band gain must be between -12 and +12 dB.",
                    None,
                    ui::EXIT_GENERAL,
                );
            }
            let mut bands = load_dsp_status().bands;
            bands[index] = db;
            let dsp = apply_dsp_request(DaemonRequest::SetEqBands { bands });
            let freq = crate::audio::dsp::EQ_BANDS_HZ[index];
            ui::done(
                format!(
                    "Band {} ({}) set to {db:+.1} dB.",
                    index + 1,
                    render::band_frequency(freq)
                ),
                json!({ "dsp": dsp }),
            );
            println!();
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::EqEnable => {
            let dsp = apply_dsp_request(DaemonRequest::SetEqEnabled { enabled: true });
            ui::done(dsp_message_or("Equalizer enabled."), json!({ "dsp": dsp }));
            println!();
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::EqDisable => {
            let dsp = apply_dsp_request(DaemonRequest::SetEqEnabled { enabled: false });
            ui::done(dsp_message_or("Equalizer disabled."), json!({ "dsp": dsp }));
            println!();
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::EqReset => {
            let dsp = apply_dsp_request(DaemonRequest::ResetEq);
            ui::done(
                dsp_message_or("Equalizer reset to flat and enabled."),
                json!({ "dsp": dsp }),
            );
            println!();
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::Preset { name } => {
            let dsp = apply_dsp_request(DaemonRequest::ApplyEqPreset { name: name.clone() });
            ui::done(format!("Applied EQ preset {name}."), json!({ "dsp": dsp }));
            println!();
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
            ui::done(
                format!("EQ settings exported to {output}"),
                json!({ "output": output }),
            );
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
            ui::done(
                format!("EQ settings imported from {input}"),
                json!({ "dsp": dsp }),
            );
            println!();
            print!("{}", render::dsp_status(ui::current(), &dsp));
        }
        DspCmd::Gapless { state } => match state {
            None => {
                let dsp = load_dsp_status();
                json::maybe_emit(&json!({ "gapless": dsp.gapless_enabled }));
                println!(
                    "{}",
                    ui::current().kv("Gapless", on_off(dsp.gapless_enabled), 10)
                );
            }
            Some(raw) => {
                let enabled = parse_on_off(&raw, "gapless");
                let dsp = apply_dsp_request(DaemonRequest::SetGapless { enabled });
                ui::done(
                    format!("Gapless playback {}.", on_off(dsp.gapless_enabled)),
                    json!({ "gapless": dsp.gapless_enabled }),
                );
            }
        },
        DspCmd::Crossfade { seconds } => match seconds {
            None => {
                let dsp = load_dsp_status();
                json::maybe_emit(&json!({ "crossfade_seconds": dsp.crossfade_duration }));
                println!("{}", ui::current().kv("Crossfade", &crossfade(&dsp), 10));
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
                ui::done(
                    format!("Crossfade {}.", crossfade(&dsp)),
                    json!({ "crossfade_seconds": dsp.crossfade_duration }),
                );
            }
        },
        DspCmd::Bass { db } => match db {
            None => {
                let dsp = load_dsp_status();
                json::maybe_emit(&json!({ "bass_db": dsp.bass }));
                println!(
                    "{}",
                    ui::current().kv("Bass", &format!("{:+.1} dB", dsp.bass), 10)
                );
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
                ui::done(
                    format!("Bass {:+.1} dB, treble {:+.1} dB.", dsp.bass, dsp.treble),
                    json!({ "dsp": dsp }),
                );
                println!("  {}", render::eq_bands_brief(ui::current(), &dsp));
            }
        },
        DspCmd::Treble { db } => match db {
            None => {
                let dsp = load_dsp_status();
                json::maybe_emit(&json!({ "treble_db": dsp.treble }));
                println!(
                    "{}",
                    ui::current().kv("Treble", &format!("{:+.1} dB", dsp.treble), 10)
                );
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
                ui::done(
                    format!("Treble {:+.1} dB, bass {:+.1} dB.", dsp.treble, dsp.bass),
                    json!({ "dsp": dsp }),
                );
                println!("  {}", render::eq_bands_brief(ui::current(), &dsp));
            }
        },
    }
}

fn on_off(enabled: bool) -> &'static str {
    if enabled {
        "on"
    } else {
        "off"
    }
}

fn crossfade(dsp: &DspStatus) -> String {
    if dsp.crossfade_duration <= 0.0 {
        "off".to_string()
    } else {
        format!("{:.1}s", dsp.crossfade_duration)
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

    settings_dsp_status(&crate::app_settings::AppSettings::load_from_disk())
}

fn settings_dsp_status(settings: &crate::app_settings::AppSettings) -> DspStatus {
    let (bass, treble) = settings.equalizer.tone_dials();
    DspStatus {
        eq_enabled: settings.equalizer.enabled,
        bands: settings.equalizer.bands,
        crossfade_duration: settings.equalizer.crossfade_duration,
        gapless_enabled: settings.gapless_enabled,
        bass,
        treble,
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
            let (_, treble) = settings.equalizer.tone_dials();
            settings.equalizer.apply_bass_treble(db, treble);
        }
        DaemonRequest::SetTreble { db } => {
            let (bass, _) = settings.equalizer.tone_dials();
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

    settings_dsp_status(&settings)
}

/// A band given as its number (1 to 10) or its frequency ("125", "1k",
/// "16khz"), returned as a 0-based index.
fn parse_band(raw: &str) -> Option<usize> {
    let raw = raw.trim().to_ascii_lowercase();
    let raw = raw.strip_suffix("hz").unwrap_or(&raw).trim_end();
    let (digits, scale) = match raw.strip_suffix('k') {
        Some(digits) => (digits, 1000.0),
        None => (raw, 1.0),
    };
    let value: f32 = digits.trim().parse().ok()?;
    if scale == 1.0 && value.fract() == 0.0 && (1.0..=10.0).contains(&value) {
        return Some(value as usize - 1);
    }
    let hz = value * scale;
    crate::audio::dsp::EQ_BANDS_HZ
        .iter()
        .position(|&band| (band - hz).abs() < 1.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_band_is_named_by_number_or_frequency() {
        assert_eq!(parse_band("1"), Some(0));
        assert_eq!(parse_band("10"), Some(9));
        assert_eq!(parse_band("31"), Some(0));
        assert_eq!(parse_band("125"), Some(2));
        assert_eq!(parse_band("125Hz"), Some(2));
        assert_eq!(parse_band("1k"), Some(5));
        assert_eq!(parse_band("16kHz"), Some(9));
        assert_eq!(parse_band("16000"), Some(9));
    }

    #[test]
    fn an_unknown_band_is_rejected() {
        assert_eq!(parse_band("0"), None);
        assert_eq!(parse_band("11"), None);
        assert_eq!(parse_band("440"), None);
        assert_eq!(parse_band("bass"), None);
        assert_eq!(parse_band(""), None);
    }
}
