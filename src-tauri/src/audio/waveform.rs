// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Track waveforms for the seek bar.
//!
//! A track is decoded once and reduced to the peak level of every 20 ms of
//! audio. Those peaks are folded into a fixed
//! [`WAVEFORM_BINS`] values, so the shape is the same size for a jingle and an
//! audiobook and the UI can draw it at any width.

use crate::error::AudioError;

/// Values in a stored waveform.
pub const WAVEFORM_BINS: usize = 400;

/// Length of audio each decoded peak covers. Android's `PeakAnalyzer` uses
/// the same block length.
#[cfg(not(target_os = "android"))]
const BLOCK_SECS: f32 = 0.02;

/// Fold per-block peaks (0.0 to 1.0) into [`WAVEFORM_BINS`] bytes.
///
/// Each bin keeps the loudest block it covers. Values are scaled so the
/// loudest bin is 255, on a square-root curve so quiet passages stay visible.
/// Empty or silent input gives all zeros.
pub fn bins_from_peaks(peaks: &[f32]) -> Vec<u8> {
    let mut bins = vec![0.0f32; WAVEFORM_BINS];
    if peaks.is_empty() {
        return vec![0; WAVEFORM_BINS];
    }
    for (bin, value) in bins.iter_mut().enumerate() {
        let start = bin * peaks.len() / WAVEFORM_BINS;
        let end = ((bin + 1) * peaks.len() / WAVEFORM_BINS).max(start + 1);
        *value = peaks[start.min(peaks.len() - 1)..end.min(peaks.len())]
            .iter()
            .fold(0.0f32, |loudest, &p| loudest.max(p.abs()));
    }
    let loudest = bins.iter().copied().fold(0.0f32, f32::max);
    if loudest <= f32::EPSILON || !loudest.is_finite() {
        return vec![0; WAVEFORM_BINS];
    }
    bins.iter()
        .map(|&v| ((v / loudest).sqrt() * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// Peak of every `block` interleaved samples, from 16-bit samples.
#[cfg(not(target_os = "android"))]
fn block_peaks(samples: impl Iterator<Item = i16>, block: usize) -> Vec<f32> {
    let block = block.max(1);
    let mut peaks = Vec::new();
    let mut loudest = 0.0f32;
    let mut count = 0;
    for sample in samples {
        loudest = loudest.max((sample as f32 / i16::MAX as f32).abs());
        count += 1;
        if count == block {
            peaks.push(loudest);
            loudest = 0.0;
            count = 0;
        }
    }
    if count > 0 {
        peaks.push(loudest);
    }
    peaks
}

/// Decode `path` and return its waveform.
#[cfg(not(target_os = "android"))]
pub fn compute_waveform(path: &str) -> Result<Vec<u8>, AudioError> {
    use crate::audio::symphonia_source::SymphoniaSource;
    use rodio::Source;

    let source = SymphoniaSource::new(path)?;
    let block = (source.sample_rate() as f32 * BLOCK_SECS) as usize * source.channels() as usize;
    Ok(bins_from_peaks(&block_peaks(source, block)))
}

/// Decode `path` (a file path or a `content://` URI) and return its waveform.
#[cfg(target_os = "android")]
pub fn compute_waveform(path: &str) -> Result<Vec<u8>, AudioError> {
    crate::android::audio::exo_analyze_waveform(path)
        .map(|peaks| bins_from_peaks(&peaks))
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
                bins_from_peaks(&vec![0.5; len]).len(),
                WAVEFORM_BINS,
                "{len}"
            );
        }
    }

    #[test]
    fn silence_is_flat_zero() {
        assert!(bins_from_peaks(&[0.0; 1000]).iter().all(|&b| b == 0));
        assert!(bins_from_peaks(&[]).iter().all(|&b| b == 0));
    }

    #[test]
    fn the_loudest_part_reaches_full_height() {
        let mut peaks = vec![0.1f32; 800];
        peaks[600] = 0.4;
        let bins = bins_from_peaks(&peaks);
        assert_eq!(bins.iter().copied().max(), Some(255));
        assert_eq!(bins[300], 255);
    }

    #[test]
    fn quiet_then_loud_shows_in_order() {
        let mut peaks = vec![0.05f32; 1000];
        peaks[500..].fill(0.8);
        let bins = bins_from_peaks(&peaks);
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
        let bins = bins_from_peaks(&[0.2, 1.0]);
        assert_eq!(bins[0], bins[199]);
        assert_eq!(bins[399], 255);
        assert!(bins[0] < 255);
    }

    #[test]
    fn a_bin_keeps_its_loudest_block() {
        let mut peaks = vec![0.0f32; 4000];
        peaks[15] = 1.0; // bins are 10 blocks wide, so this is in the second
        assert_eq!(bins_from_peaks(&peaks)[1], 255);
        assert_eq!(bins_from_peaks(&peaks)[0], 0);
    }

    #[test]
    fn block_peaks_cover_every_sample() {
        let samples = [0i16, 16_384, -32_767, 0, 100];
        let peaks = block_peaks(samples.into_iter(), 2);
        assert_eq!(peaks.len(), 3);
        assert!((peaks[0] - 0.5).abs() < 1e-3);
        assert!((peaks[1] - 1.0).abs() < 1e-3);
        assert!(peaks[2] > 0.0);
    }
}
