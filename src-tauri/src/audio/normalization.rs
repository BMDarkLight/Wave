use std::collections::HashMap;

use crate::error::AudioError;

/// Minimum peak amplitude treated as valid (avoids divide-by-near-zero).
pub const MIN_PEAK: f32 = 0.001;
/// Maximum linear boost for quiet tracks (+12 dB).
pub const MAX_GAIN: f32 = 4.0;
/// How many recently analyzed peaks feed the session median.
const SESSION_PEAK_LIMIT: usize = 50;
/// Ceiling on simultaneous background peak scans (each opens a decoder — a
/// MediaCodec instance on Android, a decode thread on desktop). Bounds
/// resource use when the user skips through tracks faster than analysis
/// finishes; a request over the cap is simply skipped rather than queued —
/// it just stays un-normalized until it's requested again.
const MAX_CONCURRENT_ANALYSIS: usize = 3;

/// Tracks per-file peaks and computes median-relative boost gains.
#[derive(Debug, Clone)]
pub struct VolumeNormalizer {
    enabled: bool,
    peak_cache: HashMap<String, f32>,
    session_peaks: Vec<f32>,
    in_flight_analysis: usize,
}

impl Default for VolumeNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl VolumeNormalizer {
    pub fn new() -> Self {
        Self {
            enabled: false,
            peak_cache: HashMap::new(),
            session_peaks: Vec::new(),
            in_flight_analysis: 0,
        }
    }

    /// Reserve a background-analysis slot. Returns `false` (reserving
    /// nothing) when [`MAX_CONCURRENT_ANALYSIS`] scans are already running —
    /// the caller should skip spawning and leave the track at neutral gain
    /// for now. Pair every `true` result with [`Self::end_analysis`].
    pub fn try_begin_analysis(&mut self) -> bool {
        if self.in_flight_analysis >= MAX_CONCURRENT_ANALYSIS {
            return false;
        }
        self.in_flight_analysis += 1;
        true
    }

    pub fn end_analysis(&mut self) {
        self.in_flight_analysis = self.in_flight_analysis.saturating_sub(1);
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.session_peaks.clear();
        }
    }

    pub fn cached_peak(&self, path: &str) -> Option<f32> {
        self.peak_cache.get(path).copied()
    }

    /// Cache a peak value without counting it toward the running session
    /// median. Used for a peeked/upcoming track that was analyzed in the
    /// background but hasn't actually started playing yet.
    pub fn cache_peak(&mut self, path: &str, peak: f32) {
        self.peak_cache.insert(path.to_string(), peak.clamp(MIN_PEAK, 1.0));
    }

    /// Median peak of tracks analyzed this session.
    pub fn median_peak(&self) -> Option<f32> {
        if self.session_peaks.is_empty() {
            return None;
        }
        let mut sorted = self.session_peaks.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = sorted.len() / 2;
        Some(if sorted.len() % 2 == 0 {
            (sorted[mid - 1] + sorted[mid]) * 0.5
        } else {
            sorted[mid]
        })
    }

    /// Linear gain for a track peak relative to the session median.
    ///
    /// Loud tracks are never attenuated; quiet tracks are boosted up to [`MAX_GAIN`]
    /// while keeping `peak * gain <= 1.0` to avoid clipping.
    pub fn compute_gain(track_peak: f32, median: f32) -> f32 {
        let peak = track_peak.clamp(MIN_PEAK, 1.0);
        let target = median.clamp(MIN_PEAK, 1.0);
        if peak >= target {
            return 1.0;
        }
        let raw = target / peak;
        let clip_limit = 1.0 / peak;
        raw.min(MAX_GAIN).min(clip_limit)
    }

    /// Record a track peak and return the gain to apply (1.0 when disabled).
    pub fn register_peak(&mut self, path: &str, peak: f32) -> f32 {
        let peak = peak.clamp(MIN_PEAK, 1.0);
        let is_new = !self.peak_cache.contains_key(path);
        self.peak_cache.insert(path.to_string(), peak);
        if !self.enabled {
            return 1.0;
        }
        if is_new {
            self.session_peaks.push(peak);
            if self.session_peaks.len() > SESSION_PEAK_LIMIT {
                self.session_peaks.remove(0);
            }
        }
        let median = self.median_peak().unwrap_or(peak);
        Self::compute_gain(peak, median)
    }
}

/// Scan an audio file and return its sample peak in 0.0–1.0 (desktop).
#[cfg(not(target_os = "android"))]
pub fn analyze_peak_amplitude(path: &str) -> Result<f32, AudioError> {
    use crate::audio::symphonia_source::SymphoniaSource;

    let mut source = SymphoniaSource::new(path)?;
    let mut peak = 0.0f32;
    while let Some(sample) = source.next() {
        let abs = (sample as f32 / i16::MAX as f32).abs();
        if abs > peak {
            peak = abs;
        }
    }
    Ok(peak.max(MIN_PEAK))
}

#[cfg(target_os = "android")]
pub fn analyze_peak_amplitude(path: &str) -> Result<f32, AudioError> {
    crate::android::audio::exo_analyze_peak(path)
        .map(|p| p.max(MIN_PEAK))
        .map_err(|e| AudioError::Decode(format!("Peak analysis failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loud_track_is_not_attenuated() {
        assert_eq!(VolumeNormalizer::compute_gain(0.9, 0.5), 1.0);
    }

    #[test]
    fn quiet_track_is_boosted_toward_median() {
        let gain = VolumeNormalizer::compute_gain(0.25, 0.5);
        assert!((gain - 2.0).abs() < 1e-4);
    }

    #[test]
    fn gain_is_capped_to_prevent_clipping() {
        let gain = VolumeNormalizer::compute_gain(0.05, 0.9);
        assert!((gain - MAX_GAIN).abs() < 1e-4 || 0.05 * gain <= 1.0 + 1e-4);
    }

    #[test]
    fn median_of_session_peaks() {
        let mut n = VolumeNormalizer::new();
        n.set_enabled(true);
        n.register_peak("a", 0.2);
        n.register_peak("b", 0.8);
        assert!((n.median_peak().unwrap() - 0.5).abs() < 1e-4);
    }
}
