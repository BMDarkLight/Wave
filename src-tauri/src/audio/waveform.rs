// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Track waveforms for the seek bar.
//!
//! A track is decoded once and reduced to the RMS level of every 20 ms of
//! audio. Those levels are folded into a fixed [`WAVEFORM_BINS`] values, so
//! the shape is the same size for a jingle and an audiobook and the UI can
//! draw it at any width.
//!
//! RMS follows how loud a passage sounds. Peaks sit near full scale for most
//! of any modern master, which made every bar the same height.

use crate::error::AudioError;

/// Values in a stored waveform.
pub const WAVEFORM_BINS: usize = 400;

/// Bumped when the analysis changes, so saved waveforms are recomputed.
/// 1 was per-block peaks on a square-root curve.
pub const WAVEFORM_VERSION: i64 = 2;

/// Length of audio each decoded level covers. Android's `PeakAnalyzer` uses
/// the same block length.
#[cfg(not(target_os = "android"))]
const BLOCK_SECS: f32 = 0.02;

/// Fold per-block RMS levels (0.0 to 1.0) into [`WAVEFORM_BINS`] bytes.
///
/// Each bin is the RMS of the blocks it covers, so it shows how loud that
/// stretch is overall rather than its single loudest moment. Values are
/// scaled linearly so the loudest bin is 255. Empty or silent input gives all
/// zeros.
pub fn bins_from_levels(levels: &[f32]) -> Vec<u8> {
    let mut bins = vec![0.0f32; WAVEFORM_BINS];
    if levels.is_empty() {
        return vec![0; WAVEFORM_BINS];
    }
    for (bin, value) in bins.iter_mut().enumerate() {
        let start = (bin * levels.len() / WAVEFORM_BINS).min(levels.len() - 1);
        let end = ((bin + 1) * levels.len() / WAVEFORM_BINS)
            .max(start + 1)
            .min(levels.len());
        let covered = &levels[start..end];
        let squares: f32 = covered.iter().map(|&l| l * l).sum();
        *value = (squares / covered.len() as f32).sqrt();
    }
    let loudest = bins.iter().copied().fold(0.0f32, f32::max);
    if loudest <= f32::EPSILON || !loudest.is_finite() {
        return vec![0; WAVEFORM_BINS];
    }
    bins.iter()
        .map(|&v| (v / loudest * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// RMS of every `block` interleaved samples, from 16-bit samples.
#[cfg(not(target_os = "android"))]
fn block_levels(samples: impl Iterator<Item = i16>, block: usize) -> Vec<f32> {
    let block = block.max(1);
    let mut levels = Vec::new();
    let mut squares = 0.0f32;
    let mut count = 0;
    for sample in samples {
        let value = sample as f32 / i16::MAX as f32;
        squares += value * value;
        count += 1;
        if count == block {
            levels.push((squares / count as f32).sqrt().min(1.0));
            squares = 0.0;
            count = 0;
        }
    }
    if count > 0 {
        levels.push((squares / count as f32).sqrt().min(1.0));
    }
    levels
}

/// Decode `path` and return its waveform.
#[cfg(not(target_os = "android"))]
pub fn compute_waveform(path: &str) -> Result<Vec<u8>, AudioError> {
    use crate::audio::symphonia_source::SymphoniaSource;
    use rodio::Source;

    let source = SymphoniaSource::new(path)?;
    let block = (source.sample_rate() as f32 * BLOCK_SECS) as usize * source.channels() as usize;
    Ok(bins_from_levels(&block_levels(source, block)))
}

/// Decode `path` (a file path or a `content://` URI) and return its waveform.
#[cfg(target_os = "android")]
pub fn compute_waveform(path: &str) -> Result<Vec<u8>, AudioError> {
    crate::android::audio::exo_analyze_waveform(path)
        .map(|levels| bins_from_levels(&levels))
        .map_err(|e| AudioError::Decode(format!("Waveform analysis failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono 16-bit WAV: one second of silence, then one second of a tone.
    #[cfg(not(target_os = "android"))]
    fn silence_then_tone_wav() -> std::path::PathBuf {
        let rate = 22_050u32;
        let mut samples: Vec<i16> = vec![0; rate as usize];
        samples.extend((0..rate).map(|i| {
            let t = i as f32 / rate as f32;
            (16_000.0 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) as i16
        }));
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!("wave-waveform-{}.wav", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_decoded_file_shows_its_quiet_and_loud_halves() {
        let path = silence_then_tone_wav();
        let bins = compute_waveform(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(bins.len(), WAVEFORM_BINS);
        assert!(
            bins[..195].iter().all(|&b| b == 0),
            "silent half should be flat"
        );
        assert!(
            bins[205..].iter().all(|&b| b > 240),
            "tone half should be full"
        );
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_missing_file_is_an_error() {
        assert!(compute_waveform("/definitely/not/here.wav").is_err());
    }

    #[test]
    fn always_gives_the_fixed_number_of_bins() {
        for len in [0usize, 1, 7, 399, 400, 401, 100_000] {
            assert_eq!(
                bins_from_levels(&vec![0.5; len]).len(),
                WAVEFORM_BINS,
                "{len}"
            );
        }
    }

    #[test]
    fn silence_is_flat_zero() {
        assert!(bins_from_levels(&[0.0; 1000]).iter().all(|&b| b == 0));
        assert!(bins_from_levels(&[]).iter().all(|&b| b == 0));
    }

    #[test]
    fn the_loudest_part_reaches_full_height() {
        let mut levels = vec![0.1f32; 800];
        levels[600] = 0.4;
        let bins = bins_from_levels(&levels);
        assert_eq!(bins.iter().copied().max(), Some(255));
        assert_eq!(bins[300], 255);
    }

    #[test]
    fn quiet_then_loud_shows_in_order() {
        let mut levels = vec![0.05f32; 1000];
        levels[500..].fill(0.8);
        let bins = bins_from_levels(&levels);
        assert!(
            bins[..190].iter().all(|&b| b < 80),
            "quiet half should be low"
        );
        assert!(
            bins[210..].iter().all(|&b| b == 255),
            "loud half should be full"
        );
    }

    #[test]
    fn short_input_spreads_across_all_bins() {
        let bins = bins_from_levels(&[0.2, 1.0]);
        assert_eq!(bins[0], bins[199]);
        assert_eq!(bins[399], 255);
        assert!(bins[0] < 255);
    }

    #[test]
    fn a_bin_shows_how_loud_it_is_overall_not_its_loudest_moment() {
        // Bins are 10 blocks wide. The first is steady at half level; the
        // second is quiet apart from one full-scale hit.
        let mut levels = vec![0.1f32; 4000];
        levels[..10].fill(0.5);
        levels[15] = 1.0;
        let bins = bins_from_levels(&levels);
        assert!(bins[0] > bins[1], "{} vs {}", bins[0], bins[1]);
        assert!(bins[1] > bins[2]);
    }

    #[test]
    fn half_the_level_is_half_the_height() {
        let mut levels = vec![1.0f32; 800];
        levels[..400].fill(0.5);
        let bins = bins_from_levels(&levels);
        assert_eq!(bins[399], 255);
        assert!((bins[0] as i32 - 128).abs() <= 1, "{}", bins[0]);
    }

    #[test]
    fn block_levels_cover_every_sample() {
        let samples = [0i16, 16_384, -32_767, 0, 100];
        let levels = block_levels(samples.into_iter(), 2);
        assert_eq!(levels.len(), 3);
        assert!((levels[0] - 0.5f32 / 2f32.sqrt()).abs() < 1e-3);
        assert!((levels[1] - 1.0f32 / 2f32.sqrt()).abs() < 1e-3);
        assert!(levels[2] > 0.0);
    }
}
