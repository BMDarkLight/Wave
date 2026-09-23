// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Bars and meters.
//!
//! Everything here returns a string of a known width, because these get
//! redrawn in place by the now-playing dashboard and a stray column would
//! leave litter on the line.

use crate::cli::ui::Ui;

/// Highest gain the EQ bars are drawn against, matching the dial range.
const EQ_RANGE_DB: f32 = 12.0;

fn clamp01(fraction: f64) -> f64 {
    if fraction.is_nan() {
        0.0
    } else {
        fraction.clamp(0.0, 1.0)
    }
}

/// How many of `cells` a fraction fills, rounded and never over the top.
fn filled(fraction: f64, cells: usize) -> usize {
    ((clamp01(fraction) * cells as f64).round() as usize).min(cells)
}

pub fn progress(ui: &Ui, fraction: f64, width: usize) -> String {
    let on = filled(fraction, width);
    format!(
        "{}{}",
        ui.glyphs.bar_full.repeat(on),
        ui.glyphs.bar_empty.repeat(width - on)
    )
}

pub fn meter(ui: &Ui, fraction: f64, cells: usize) -> String {
    let on = filled(fraction, cells);
    format!(
        "{}{}",
        ui.glyphs.meter_on.repeat(on),
        ui.glyphs.meter_off.repeat(cells - on)
    )
}

/// One EQ band drawn against a centered axis, so cut and boost read at a
/// glance rather than by checking a sign. Always `half * 2 + 1` columns wide.
pub fn eq_band(ui: &Ui, gain_db: f32, half: usize) -> String {
    let gain = if gain_db.is_nan() {
        0.0
    } else {
        gain_db.clamp(-EQ_RANGE_DB, EQ_RANGE_DB)
    };
    let cells = filled((gain.abs() / EQ_RANGE_DB) as f64, half);
    let (left, right) = if gain < 0.0 { (cells, 0) } else { (0, cells) };

    format!(
        "{}{}{}{}{}",
        " ".repeat(half - left),
        ui.glyphs.bar_full.repeat(left),
        ui.glyphs.axis,
        ui.glyphs.bar_full.repeat(right),
        " ".repeat(half - right)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ui::{display_width, Ui};

    #[test]
    fn progress_is_always_exactly_the_requested_width() {
        let ui = Ui::plain();
        for f in [-1.0, 0.0, 0.25, 0.5, 1.0, 2.0, f64::NAN] {
            assert_eq!(display_width(&progress(&ui, f, 20)), 20, "fraction {f}");
        }
    }

    #[test]
    fn progress_fills_from_the_left() {
        let ui = Ui::plain();
        let half = progress(&ui, 0.5, 10);
        assert!(half.starts_with(ui.glyphs.bar_full));
        assert!(half.ends_with(ui.glyphs.bar_empty));
        assert_eq!(progress(&ui, 0.0, 10), ui.glyphs.bar_empty.repeat(10));
        assert_eq!(progress(&ui, 1.0, 10), ui.glyphs.bar_full.repeat(10));
    }

    #[test]
    fn progress_of_zero_width_is_empty() {
        assert_eq!(progress(&Ui::plain(), 0.5, 0), "");
    }

    #[test]
    fn meter_counts_cells_not_columns() {
        let ui = Ui::plain();
        let m = meter(&ui, 0.6, 10);
        assert_eq!(m.chars().count(), 10);
        assert_eq!(m.matches(ui.glyphs.meter_on).count(), 6);
    }

    #[test]
    fn eq_band_puts_the_axis_in_the_middle() {
        let ui = Ui::plain();
        let flat = eq_band(&ui, 0.0, 6);
        assert_eq!(display_width(&flat), 13, "6 left, axis, 6 right");
        assert_eq!(flat.chars().nth(6).unwrap().to_string(), ui.glyphs.axis);
    }

    #[test]
    fn eq_band_boost_goes_right_and_cut_goes_left() {
        let ui = Ui::plain();
        let boost = eq_band(&ui, 12.0, 6);
        let cut = eq_band(&ui, -12.0, 6);
        // A full boost fills every cell right of the axis, a full cut every
        // cell left of it.
        assert!(boost.ends_with(&ui.glyphs.bar_full.repeat(6)));
        assert!(boost.starts_with(&" ".repeat(6)));
        assert!(cut.starts_with(&ui.glyphs.bar_full.repeat(6)));
        assert!(cut.ends_with(&" ".repeat(6)));
        assert_eq!(display_width(&boost), 13);
        assert_eq!(display_width(&cut), 13);
    }

    #[test]
    fn eq_band_clamps_out_of_range_gain() {
        let ui = Ui::plain();
        assert_eq!(eq_band(&ui, 99.0, 6), eq_band(&ui, 12.0, 6));
        assert_eq!(eq_band(&ui, -99.0, 6), eq_band(&ui, -12.0, 6));
        assert_eq!(eq_band(&ui, f32::NAN, 6), eq_band(&ui, 0.0, 6));
    }
}
