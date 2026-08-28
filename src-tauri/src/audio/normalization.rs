use std::collections::{HashMap, HashSet};

use crate::error::AudioError;

/// Minimum amplitude (peak or RMS) treated as valid (avoids divide-by-near-zero).
pub const MIN_LEVEL: f32 = 0.001;
/// Maximum linear boost for quiet tracks (+12 dB).
pub const MAX_GAIN: f32 = 4.0;
/// Maximum linear cut for loud tracks (-12 dB), mirroring [`MAX_GAIN`].
pub const MIN_GAIN: f32 = 0.25;
/// How many recently analyzed tracks feed the session median.
const SESSION_RMS_LIMIT: usize = 50;
/// Ceiling on simultaneous background level scans (each opens a decoder — a
/// MediaCodec instance on Android, a decode thread on desktop). Bounds
/// resource use when the user skips through tracks faster than analysis
/// finishes; a request over the cap is simply skipped rather than queued —
/// it just stays un-normalized until it's requested again.
const MAX_CONCURRENT_ANALYSIS: usize = 3;

/// Sample peak and RMS (root-mean-square) amplitude for one file, both in
/// 0.0–1.0. Peak alone doesn't track perceived loudness — a sparse mix with
/// one loud transient can have a high peak while sounding quiet throughout —
/// so gain is driven by RMS; peak is kept only as a clip-safety guard on
/// boosts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioLevels {
    pub peak: f32,
    pub rms: f32,
}

/// Tracks per-file loudness and computes median-relative gains.
#[derive(Debug, Clone)]
pub struct VolumeNormalizer {
    enabled: bool,
    levels_cache: HashMap<String, AudioLevels>,
    session_rms: Vec<f32>,
    /// Paths that have actually contributed a sample to `session_rms`.
    /// Kept separate from `levels_cache`: a track can get cached levels from
    /// a non-counting peek (crossfade/gapless prefetch of a track that
    /// hasn't played yet) without that peek counting toward the median —
    /// this set is what makes `register_levels` idempotent per-path
    /// regardless of whether a peek already populated the cache first.
    contributed: HashSet<String>,
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
            levels_cache: HashMap::new(),
            session_rms: Vec::new(),
            contributed: HashSet::new(),
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
            self.session_rms.clear();
            self.contributed.clear();
        }
    }

    pub fn cached_levels(&self, path: &str) -> Option<AudioLevels> {
        self.levels_cache.get(path).copied()
    }

    /// Cache levels without counting them toward the running session median.
    /// Used for a peeked/upcoming track that was analyzed in the background
    /// but hasn't actually started playing yet.
    pub fn cache_levels(&mut self, path: &str, levels: AudioLevels) {
        self.levels_cache.insert(path.to_string(), clamp_levels(levels));
    }

    /// Median RMS of tracks analyzed this session.
    pub fn median_rms(&self) -> Option<f32> {
        if self.session_rms.is_empty() {
            return None;
        }
        let mut sorted = self.session_rms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = sorted.len() / 2;
        Some(if sorted.len() % 2 == 0 {
            (sorted[mid - 1] + sorted[mid]) * 0.5
        } else {
            sorted[mid]
        })
    }

    /// Linear gain for a track's loudness relative to the session median.
    ///
    /// Driven by RMS, not peak: quiet tracks are boosted and loud tracks are
    /// attenuated toward the median, clamped to [`MIN_GAIN`]–[`MAX_GAIN`].
    /// The track's peak still caps any boost so `peak * gain <= 1.0` — it
    /// only guards against clipping, it never drives the gain itself.
    pub fn compute_gain(track_rms: f32, median_rms: f32, track_peak: f32) -> f32 {
        let rms = track_rms.clamp(MIN_LEVEL, 1.0);
        let target = median_rms.clamp(MIN_LEVEL, 1.0);
        let peak = track_peak.clamp(MIN_LEVEL, 1.0);
        let raw = target / rms;
        let gain = raw.clamp(MIN_GAIN, MAX_GAIN);
        if gain > 1.0 {
            gain.min(1.0 / peak)
        } else {
            gain
        }
    }

    /// Record a track's levels and return the gain to apply (1.0 when disabled).
    pub fn register_levels(&mut self, path: &str, levels: AudioLevels) -> f32 {
        let levels = clamp_levels(levels);
        self.levels_cache.insert(path.to_string(), levels);
        if !self.enabled {
            return 1.0;
        }
        // `contributed` (not `levels_cache`) gates the median update: a
        // track can already be cache-only (from a non-counting peek) the
        // first time it's actually registered, and that first real play
        // must still count once.
        if self.contributed.insert(path.to_string()) {
            self.session_rms.push(levels.rms);
            if self.session_rms.len() > SESSION_RMS_LIMIT {
                self.session_rms.remove(0);
            }
        }
        let median = self.median_rms().unwrap_or(levels.rms);
        Self::compute_gain(levels.rms, median, levels.peak)
    }
}

fn clamp_levels(levels: AudioLevels) -> AudioLevels {
    AudioLevels {
        peak: levels.peak.clamp(MIN_LEVEL, 1.0),
        rms: levels.rms.clamp(MIN_LEVEL, 1.0),
    }
}

/// Scan an audio file and return its sample peak and RMS, both in 0.0–1.0
/// (desktop).
#[cfg(not(target_os = "android"))]
pub fn analyze_track_levels(path: &str) -> Result<AudioLevels, AudioError> {
    use crate::audio::symphonia_source::SymphoniaSource;

    let mut source = SymphoniaSource::new(path)?;
    let mut peak = 0.0f32;
    let mut sum_squares = 0.0f64;
    let mut count = 0u64;
    while let Some(sample) = source.next() {
        let abs = (sample as f32 / i16::MAX as f32).abs();
        if abs > peak {
            peak = abs;
        }
        sum_squares += (abs as f64) * (abs as f64);
        count += 1;
    }
    let rms = if count > 0 {
        (sum_squares / count as f64).sqrt() as f32
    } else {
        0.0
    };
    Ok(AudioLevels {
        peak: peak.max(MIN_LEVEL),
        rms: rms.max(MIN_LEVEL),
    })
}

#[cfg(target_os = "android")]
pub fn analyze_track_levels(path: &str) -> Result<AudioLevels, AudioError> {
    crate::android::audio::exo_analyze_levels(path)
        .map(|(peak, rms)| AudioLevels {
            peak: peak.max(MIN_LEVEL),
            rms: rms.max(MIN_LEVEL),
        })
        .map_err(|e| AudioError::Decode(format!("Level analysis failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_track_is_boosted_toward_median() {
        let gain = VolumeNormalizer::compute_gain(0.25, 0.5, 0.25);
        assert!((gain - 2.0).abs() < 1e-4);
    }

    #[test]
    fn loud_track_is_attenuated_toward_median() {
        let gain = VolumeNormalizer::compute_gain(0.8, 0.4, 0.8);
        assert!((gain - 0.5).abs() < 1e-4);
    }

    #[test]
    fn boost_is_capped_to_prevent_clipping() {
        // Peak is much higher than RMS (a track with a sharp transient), so
        // the raw RMS-matching boost would push the peak past 1.0 — the clip
        // guard must cap it below the requested boost.
        let gain = VolumeNormalizer::compute_gain(0.05, 0.9, 0.9);
        assert!(gain < MAX_GAIN);
        assert!(0.9 * gain <= 1.0 + 1e-4);
    }

    #[test]
    fn attenuation_is_clamped_symmetrically() {
        let gain = VolumeNormalizer::compute_gain(1.0, 0.05, 1.0);
        assert!((gain - MIN_GAIN).abs() < 1e-4);
    }

    #[test]
    fn median_of_session_rms() {
        let mut n = VolumeNormalizer::new();
        n.set_enabled(true);
        n.register_levels("a", AudioLevels { peak: 0.2, rms: 0.2 });
        n.register_levels("b", AudioLevels { peak: 0.8, rms: 0.8 });
        assert!((n.median_rms().unwrap() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn quiet_by_loudness_track_gets_boosted_despite_matching_peak() {
        // Old (peak-based) behavior: this track's peak matches the session
        // reference, so peak-based gain would land on 1.0 - no boost - even
        // though the track is much quieter to the ear (e.g. a sparse mix
        // with one loud transient pulling the peak up). RMS-based gain must
        // still recognize it as quiet and boost it.
        let track_peak = 0.9;
        let track_rms = 0.1;
        let median_rms = 0.4;
        let gain = VolumeNormalizer::compute_gain(track_rms, median_rms, track_peak);
        assert!(gain > 1.0, "expected a boost, got gain={gain}");
    }
}
