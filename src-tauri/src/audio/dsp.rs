use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::Source;

/// Shared state for crossfade track switching.
///
/// Written by the audio thread inside [`Crossfade`] and read by the player on
/// the auto-advance tick so UI / queue / media session can follow the handoff.
#[derive(Debug, Clone, Default)]
pub struct CrossfadeState {
    /// Path of the track the UI / player should treat as current.
    /// Updated when the fade *starts* (not when the outgoing track ends).
    pub current_path: Option<String>,
    /// Playback position within [`Self::current_path`].
    pub position: Duration,
    /// Total duration of [`Self::current_path`].
    pub duration: Option<Duration>,
    /// `true` while the outgoing track still has a pending next source attached.
    pub pending_next: bool,
    /// Latched when the logical track changes (fade start); cleared by the player.
    pub track_switched: bool,
}

/// Standard ISO frequency bands for a 10-band graphic equalizer.
pub const EQ_BANDS_HZ: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

/// Named EQ presets indexed by the band labels below.
pub const EQ_PRESETS: &[(&str, &str, [f32; 10])] = &[
    ("flat",       "Flat (all 0 dB)",             [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("bass-boost", "Bass boost",                  [4.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("bass-cut",   "Bass cut",                    [-4.0, -4.0, -2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("rock",       "Rock (smile curve)",          [3.0, 2.0, 0.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0, 2.0]),
    ("pop",        "Pop (boosted mids)",           [1.0, 1.0, 2.0, 3.0, 3.0, 2.0, 1.0, 1.0, 1.0, 1.0]),
    ("jazz",       "Jazz (warm, gentle highs)",    [2.0, 2.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
    ("classical",  "Classical (flat, slight air)", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 2.0]),
    ("vocal",      "Vocal (cut lows, boost mids)", [-2.0, -2.0, -1.0, 1.0, 3.0, 4.0, 3.0, 1.0, -1.0, -2.0]),
    ("loudness",   "Loudness (low-volume curve)", [5.0, 4.0, 2.0, 0.0, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0]),
    ("headphones", "Headphones (subtle crossfeed)", [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, -1.0, -1.0, 0.0, 0.0]),
];

/// Serializable EQ preset file format.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EqPresetFile {
    /// Optional user-facing name for the preset.
    pub name: Option<String>,
    pub enabled: bool,
    /// 10 gain values in dB.
    pub bands: [f32; 10],
    /// The frequency labels for reference.
    pub frequencies: [f32; 10],
    /// Crossfade duration in seconds (0.0 = off, max 8.0).
    #[serde(default)]
    pub crossfade_duration: f32,
}

impl EqPresetFile {
    pub fn from_config(config: &EqConfig, name: Option<String>) -> Self {
        Self {
            name,
            enabled: config.enabled,
            bands: config.bands,
            frequencies: EQ_BANDS_HZ,
            crossfade_duration: config.crossfade_duration,
        }
    }

    pub fn save_to(path: &str, config: &EqConfig, name: Option<String>) -> Result<(), String> {
        let pf = Self::from_config(config, name);
        let json = serde_json::to_string_pretty(&pf).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn load_from(path: &str) -> Result<EqConfig, String> {
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let pf: Self = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        Ok(EqConfig {
            bands: pf.bands,
            enabled: pf.enabled,
            crossfade_duration: pf.crossfade_duration,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EqConfig {
    /// Gain per band in dB.
    pub bands: [f32; 10],
    /// Master enable/disable for the EQ chain.
    pub enabled: bool,
    /// Crossfade duration in seconds (0.0 = off, max 8.0).
    pub crossfade_duration: f32,
}

impl Default for EqConfig {
    fn default() -> Self {
        Self {
            bands: [0.0; 10],
            enabled: false,
            crossfade_duration: 0.0,
        }
    }
}

impl EqConfig {
    const TONE_WEIGHT_DECAY: f32 = 0.7;

    /// Apply a named preset, returning `true` on success.
    pub fn apply_preset(&mut self, name: &str) -> Option<()> {
        let (_key, _desc, bands) = EQ_PRESETS.iter().find(|(key, _, _)| *key == name)?;
        self.bands = *bands;
        self.enabled = true;
        Some(())
    }

    /// Iterate available preset names and descriptions.
    pub fn list_presets() -> impl Iterator<Item = (&'static str, &'static str)> {
        EQ_PRESETS.iter().map(|(key, desc, _)| (*key, *desc))
    }

    /// Bass dial gain (lowest band), matching the mobile tone control.
    pub fn bass_gain(&self) -> f32 {
        self.bands[0].clamp(-12.0, 12.0)
    }

    /// Treble dial gain (highest band), matching the mobile tone control.
    pub fn treble_gain(&self) -> f32 {
        self.bands[9].clamp(-12.0, 12.0)
    }

    /// Rebuild the 10-band curve from bass + treble dials (GUI tone control).
    pub fn apply_bass_treble(&mut self, bass: f32, treble: f32) {
        let bass = bass.clamp(-12.0, 12.0);
        let treble = treble.clamp(-12.0, 12.0);
        for (i, band) in self.bands.iter_mut().enumerate() {
            let left = Self::TONE_WEIGHT_DECAY.powi(i as i32);
            let right = Self::TONE_WEIGHT_DECAY.powi((9 - i) as i32);
            *band = (bass * left + treble * right).clamp(-12.0, 12.0);
        }
        self.enabled = true;
    }
}

/// Transparent bi-quad filter (Direct Form 1).
///
/// Used as the atomic building block for peaking EQ, low/high-shelf,
/// and low/high-pass filters.
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    /// Peaking (bell) EQ filter centred at `freq` Hz with gain `gain_db` dB
    /// and quality factor `q`.
    fn peaking_eq(sample_rate: f32, freq: f32, gain_db: f32, q: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;

        let inv_a0 = 1.0 / a0;
        Self {
            b0: b0 * inv_a0,
            b1: b1 * inv_a0,
            b2: b2 * inv_a0,
            a1: a1 * inv_a0,
            a2: a2 * inv_a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// `Source` wrapper that applies a 10-band graphic equalizer in real-time.
///
/// EQ bands are configured via the shared `EqConfig` so the frontend can
/// adjust gains while a track is playing. The filter coefficients are
/// lazily rebuilt whenever the config version changes.
pub struct Equalizer<S> {
    inner: S,
    filters: [Biquad; 10],
    config: Arc<Mutex<EqConfig>>,
    sr: f32,
    /// Cached enabled flag so we avoid locking on every sample when EQ is off.
    enabled: bool,
    /// Monotonically increasing version — bumped on every config write so the
    /// audio thread can cheaply detect changes.
    version: Arc<Mutex<u64>>,
    last_version: u64,
}

impl<S: Source<Item = f32>> Equalizer<S> {
    pub fn new(source: S, config: Arc<Mutex<EqConfig>>, version: Arc<Mutex<u64>>) -> Self {
        let sr = source.sample_rate() as f32;
        let cfg = config.lock().unwrap_or_else(|e| e.into_inner());
        let enabled = cfg.enabled;
        let mut filters = core::array::from_fn(|_| Biquad::peaking_eq(sr, 1000.0, 0.0, 1.41));
        for (i, (freq, gain)) in EQ_BANDS_HZ.iter().zip(cfg.bands.iter()).enumerate() {
            filters[i] = Biquad::peaking_eq(sr, *freq, *gain, 1.41);
        }
        drop(cfg);

        Self {
            inner: source,
            filters,
            config,
            sr,
            enabled,
            version,
            last_version: 0,
        }
    }

    fn sync_config(&mut self) {
        let v = *self.version.lock().unwrap_or_else(|e| e.into_inner());
        if v == self.last_version {
            return;
        }
        self.last_version = v;
        let cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
        self.enabled = cfg.enabled;
        for (i, (freq, gain)) in EQ_BANDS_HZ.iter().zip(cfg.bands.iter()).enumerate() {
            self.filters[i] = Biquad::peaking_eq(self.sr, *freq, *gain, 1.41);
        }
    }
}

impl<S: Source<Item = f32> + Send> Iterator for Equalizer<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;
        self.sync_config();
        if self.enabled {
            Some(self.filters.iter_mut().fold(sample, |s, f| f.process(s)))
        } else {
            Some(sample)
        }
    }
}

impl<S: Source<Item = f32> + Send> Source for Equalizer<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        for f in self.filters.iter_mut() {
            f.reset();
        }
        self.inner.try_seek(pos)
    }
}

/// `Source` wrapper that crossfades between two sources.
///
/// The `current` source plays first. When it's within `crossfade_duration`
/// of ending, the `next` source begins playing and both are mixed with
/// volume ramps. UI/queue handoff is signaled at **fade start** (not when the
/// outgoing track fully ends) so metadata follows the incoming track immediately.
pub struct Crossfade {
    current: Box<dyn Source<Item = f32> + Send>,
    next: Option<Box<dyn Source<Item = f32> + Send>>,
    next_duration: Option<Duration>,
    crossfade_duration: f32,
    sr: u32,
    channels: u16,
    /// Position within the outgoing track (before promote).
    position: Duration,
    /// How far the next source has advanced during an active fade.
    next_position: Duration,
    /// Duration of the outgoing track.
    current_duration: Option<Duration>,
    /// Whether we're in the crossfade region.
    in_crossfade: bool,
    /// Fade progress (0.0 to 1.0) during crossfade.
    fade_progress: f32,
    /// When set, the fade window starts here instead of at
    /// `duration - crossfade_duration`. Used after a seek that lands inside
    /// the configured fade region so the next track begins at 0 over the
    /// remaining time — not mid-song at the "would-have-been" fade offset.
    fade_from: Option<Duration>,
    /// After fade starts, shared state / UI follow the incoming track.
    ui_on_next: bool,
    /// Shared state for track switching notifications.
    state: Arc<Mutex<CrossfadeState>>,
    /// Path of the outgoing track.
    current_path: Option<String>,
    /// Path of the next track (if any).
    next_path: Option<String>,
}

impl Crossfade {
    pub fn new(
        current: Box<dyn Source<Item = f32> + Send>,
        next: Option<Box<dyn Source<Item = f32> + Send>>,
        crossfade_duration: f32,
        current_path: Option<String>,
        next_path: Option<String>,
    ) -> (Self, Arc<Mutex<CrossfadeState>>) {
        let sr = current.sample_rate();
        let channels = current.channels().max(1);
        let current_duration = current.total_duration();
        let next_duration = next.as_ref().and_then(|n| n.total_duration());
        let pending_next = next.is_some();
        let state = Arc::new(Mutex::new(CrossfadeState {
            current_path: current_path.clone(),
            position: Duration::ZERO,
            duration: current_duration,
            pending_next,
            track_switched: false,
        }));
        let crossfade = Self {
            current,
            next,
            next_duration,
            crossfade_duration: crossfade_duration.clamp(0.0, 8.0),
            sr,
            channels,
            position: Duration::ZERO,
            next_position: Duration::ZERO,
            current_duration,
            in_crossfade: false,
            fade_progress: 0.0,
            fade_from: None,
            ui_on_next: false,
            state: state.clone(),
            current_path,
            next_path,
        };
        (crossfade, state)
    }

    fn sample_period(&self) -> Duration {
        let denom = self.sr as f64 * self.channels as f64;
        if denom <= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(1.0 / denom)
        }
    }

    /// Configured fade length, capped at half the outgoing track.
    fn configured_fade_secs(&self) -> f32 {
        match self.current_duration {
            Some(dur) => self
                .crossfade_duration
                .min((dur.as_secs_f32() * 0.5).max(0.0)),
            None => 0.0,
        }
    }

    /// Active fade window: `(fade_start, fade_secs)`.
    fn fade_window(&self) -> Option<(Duration, f32)> {
        let dur = self.current_duration?;
        if let Some(from) = self.fade_from {
            let secs = dur.saturating_sub(from).as_secs_f32();
            if secs <= 0.0 {
                return None;
            }
            return Some((from, secs));
        }
        let fade_secs = self.configured_fade_secs();
        if fade_secs <= 0.0 {
            return None;
        }
        let start = dur.saturating_sub(Duration::from_secs_f32(fade_secs));
        Some((start, fade_secs))
    }

    fn update_crossfade_region(&mut self) {
        if self.next.is_none() {
            self.in_crossfade = false;
            self.fade_progress = 0.0;
            return;
        }
        let Some((fade_start, fade_secs)) = self.fade_window() else {
            self.in_crossfade = false;
            self.fade_progress = 0.0;
            return;
        };
        let now_in = self.position >= fade_start;
        if now_in && !self.in_crossfade {
            // Just entered the fade window — hand UI/queue to the next track.
            self.signal_fade_start();
        }
        self.in_crossfade = now_in;
        if self.in_crossfade {
            let into_fade = self.position.saturating_sub(fade_start).as_secs_f32();
            self.fade_progress = (into_fade / fade_secs).clamp(0.0, 1.0);
        } else {
            self.fade_progress = 0.0;
        }
    }

    /// Latch shared state onto the incoming track as soon as the fade begins.
    fn signal_fade_start(&mut self) {
        if self.ui_on_next || self.next_path.is_none() {
            return;
        }
        self.ui_on_next = true;
        if let Ok(mut state) = self.state.lock() {
            state.current_path = self.next_path.clone();
            state.position = self.next_position;
            state.duration = self.next_duration;
            state.pending_next = true;
            state.track_switched = true;
        }
    }

    fn sync_shared_state(&self) {
        if let Ok(mut state) = self.state.lock() {
            if self.ui_on_next {
                // Keep reporting the incoming track once the fade has started.
                if self.next.is_some() {
                    state.current_path = self.next_path.clone();
                    state.position = self.next_position;
                    state.duration = self.next_duration;
                    state.pending_next = true;
                } else {
                    state.current_path = self.current_path.clone();
                    state.position = self.position;
                    state.duration = self.current_duration;
                    state.pending_next = false;
                }
            } else {
                state.current_path = self.current_path.clone();
                state.position = self.position;
                state.duration = self.current_duration;
                state.pending_next = self.next.is_some();
            }
        }
    }

    /// Promote the next source to current after the outgoing track ends.
    fn promote_next(&mut self) -> Option<f32> {
        let next = self.next.take()?;
        let next_path = self.next_path.take();
        let next_duration = self.next_duration.take().or_else(|| next.total_duration());
        let promoted_position = self.next_position;

        self.current = next;
        self.current_path = next_path.clone();
        self.position = promoted_position;
        self.current_duration = next_duration;
        self.next_position = Duration::ZERO;
        self.in_crossfade = false;
        self.fade_progress = 0.0;
        self.fade_from = None;
        self.ui_on_next = true;

        if let Ok(mut state) = self.state.lock() {
            // Only latch a switch if fade-start didn't already (e.g. unknown duration).
            let already = state.current_path.as_deref() == next_path.as_deref();
            state.current_path = next_path;
            state.position = promoted_position;
            state.duration = next_duration;
            state.pending_next = false;
            if !already {
                state.track_switched = true;
            }
        }

        None
    }
}

impl Iterator for Crossfade {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let dt = self.sample_period();
        self.update_crossfade_region();

        let current_sample = self.current.next();

        if self.in_crossfade && self.next.is_some() {
            let next_sample = self.next.as_mut().and_then(|n| n.next());
            if next_sample.is_some() {
                self.next_position += dt;
            }

            match (current_sample, next_sample) {
                (Some(cur), Some(nxt)) => {
                    self.position += dt;
                    self.sync_shared_state();
                    let fade_out = 1.0 - self.fade_progress;
                    let fade_in = self.fade_progress;
                    Some(cur * fade_out + nxt * fade_in)
                }
                (Some(cur), None) => {
                    // Next ended early — finish the outgoing track alone.
                    self.position += dt;
                    self.sync_shared_state();
                    Some(cur)
                }
                (None, Some(nxt)) => {
                    // Outgoing finished mid-fade — promote and keep the sample.
                    // `next_position` already includes this sample's period.
                    let _ = self.promote_next();
                    self.sync_shared_state();
                    Some(nxt)
                }
                (None, None) => None,
            }
        } else if current_sample.is_none() && self.next.is_some() {
            // Current ended outside the fade window (e.g. unknown duration).
            let next_sample = self.next.as_mut().and_then(|n| n.next());
            if next_sample.is_some() {
                self.next_position += dt;
            }
            let _ = self.promote_next();
            if let Some(sample) = next_sample {
                // Position already accounts for this sample via `next_position`.
                self.sync_shared_state();
                Some(sample)
            } else {
                self.current.next().inspect(|_| {
                    self.position += dt;
                    self.sync_shared_state();
                })
            }
        } else {
            if current_sample.is_some() {
                self.position += dt;
                self.sync_shared_state();
            }
            current_sample
        }
    }
}

impl Source for Crossfade {
    fn current_frame_len(&self) -> Option<usize> {
        self.current.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sr
    }

    fn total_duration(&self) -> Option<Duration> {
        // Report the logical current track only — the player clock tracks one
        // track at a time and updates on promote.
        self.current_duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        // UI / AudioPlayer hand off to the incoming track at *fade start*
        // (`signal_fade_start`), while `self.current` is still the outgoing
        // source until `promote_next`. Seeking must follow the logical track
        // the UI already shows — otherwise the scrubber jumps the previous song.
        if self.ui_on_next && self.next.is_some() {
            let _ = self.promote_next();
        }

        self.current.try_seek(pos)?;
        self.position = pos;
        self.in_crossfade = false;
        self.fade_progress = 0.0;
        self.next_position = Duration::ZERO;
        self.fade_from = None;

        // If we already handed the UI to this track (or just promoted into it),
        // keep reporting it. Only clear the flag when seeking the outgoing track
        // before fade-start handoff.
        if !self.ui_on_next {
            // Seeking the outgoing track — rewind any attached next source and
            // optionally start a shortened fade if we landed in the fade window.
            if let Some(ref mut n) = self.next {
                n.try_seek(Duration::ZERO)?;
            }
            if let Some(dur) = self.current_duration {
                let fade_secs = self.configured_fade_secs();
                if fade_secs > 0.0 {
                    let natural_start = dur.saturating_sub(Duration::from_secs_f32(fade_secs));
                    if pos >= natural_start && pos < dur {
                        self.fade_from = Some(pos);
                    }
                }
            }
        } else {
            // Logical current is the (promoted) incoming track — no outgoing
            // partner left to fade with from this seek.
            self.next = None;
            self.next_path = None;
            self.next_duration = None;
        }

        self.update_crossfade_region();
        self.sync_shared_state();
        Ok(())
    }
}

/// Live-adjustable gain shared between a running [`VolumeGain`] source and
/// whoever computes the actual normalization value for its track.
///
/// Peak analysis can take a while (a full-file decode), so playback starts
/// at neutral gain (1.0) and a background analysis thread swaps in the real
/// value once it's ready — this cell is how it reaches the audio thread
/// without touching the player-wide lock. Each track load gets its own cell,
/// so a slow analysis that finishes after the track has already changed just
/// writes into an orphaned cell nobody reads anymore instead of needing
/// explicit cancellation.
pub type SharedGain = Arc<std::sync::atomic::AtomicU32>;

pub fn shared_gain(initial: f32) -> SharedGain {
    Arc::new(std::sync::atomic::AtomicU32::new(initial.to_bits()))
}

pub fn set_shared_gain(cell: &SharedGain, value: f32) {
    cell.store(value.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// Per-track linear gain applied after decode (volume normalization).
pub struct VolumeGain<S> {
    inner: S,
    gain: SharedGain,
    channels: u16,
    sr: u32,
}

impl<S: Source<Item = f32>> VolumeGain<S> {
    pub fn new(inner: S, gain: SharedGain) -> Self {
        let channels = inner.channels();
        let sr = inner.sample_rate();
        Self {
            inner,
            gain,
            channels,
            sr,
        }
    }
}

use super::normalization::MAX_GAIN as MAX_NORMALIZATION_GAIN;

impl<S: Source<Item = f32>> Source for VolumeGain<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sr
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)
    }
}

impl<S: Source<Item = f32>> Iterator for VolumeGain<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let gain = f32::from_bits(self.gain.load(std::sync::atomic::Ordering::Relaxed))
            .clamp(0.0, MAX_NORMALIZATION_GAIN);
        self.inner.next().map(|sample| (sample * gain).clamp(-1.0, 1.0))
    }
}

/// Soft transport fade — tiny gain ramp for play / pause / seek / stop.
///
/// Multiplies samples by a locally smoothed gain that tracks a shared
/// [`SoftFadeState::target`]. New instances always start at gain `0` so every
/// source fades in when it first becomes audible.
pub const SOFT_FADE_SECS: f32 = 0.028;

#[derive(Debug)]
pub struct SoftFadeState {
    /// 0.0 = silence, 1.0 = full level (on top of the sink volume).
    pub target: f32,
}

impl Default for SoftFadeState {
    fn default() -> Self {
        Self { target: 1.0 }
    }
}

pub struct SoftFade {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<SoftFadeState>>,
    gain: f32,
    step: f32,
    channels: u16,
    sr: u32,
}

impl SoftFade {
    pub fn new(
        inner: Box<dyn Source<Item = f32> + Send>,
        state: Arc<Mutex<SoftFadeState>>,
    ) -> Self {
        let sr = inner.sample_rate().max(1);
        let channels = inner.channels().max(1);
        let step = 1.0 / (sr as f32 * channels as f32 * SOFT_FADE_SECS);
        Self {
            inner,
            state,
            gain: 0.0,
            step,
            channels,
            sr,
        }
    }
}

impl Iterator for SoftFade {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;
        let target = self
            .state
            .lock()
            .map(|s| s.target.clamp(0.0, 1.0))
            .unwrap_or(1.0);
        if (self.gain - target).abs() <= self.step {
            self.gain = target;
        } else if self.gain < target {
            self.gain += self.step;
        } else {
            self.gain -= self.step;
        }
        Some(sample * self.gain)
    }
}

impl Source for SoftFade {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sr
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)?;
        // Start silent after a seek so the player can fade back in.
        self.gain = 0.0;
        Ok(())
    }
}

// Note: `Crossfade::configured_fade_secs` and `Crossfade::fade_window` are not
// covered here — `Crossfade` holds `Box<dyn Source<Item = f32> + Send>` fields
// per its struct definition, so constructing a real instance in a unit test
// would require a full `rodio::Source` implementation for a fake source. That's
// more scaffolding than is justified for two pure-math helper methods; the
// `EqConfig`/`Biquad` tests below are the clear wins for this file.

#[cfg(test)]
mod tests {
    use super::*;

    // ── EqConfig::apply_preset / list_presets ───────────────────────────────

    #[test]
    fn apply_preset_sets_bands_and_enables() {
        let mut config = EqConfig::default();
        let (key, _desc, bands) = EQ_PRESETS
            .iter()
            .find(|(key, _, _)| *key == "bass-boost")
            .expect("bass-boost preset exists");
        let result = config.apply_preset(key);
        assert_eq!(result, Some(()));
        assert_eq!(config.bands, *bands);
        assert!(config.enabled);
    }

    #[test]
    fn apply_preset_unknown_name_returns_none_and_leaves_state_unchanged() {
        let mut config = EqConfig::default();
        config.bands[0] = 3.5;
        config.enabled = false;
        let before = config.clone();
        let result = config.apply_preset("not-a-real-preset");
        assert_eq!(result, None);
        assert_eq!(config.bands, before.bands);
        assert_eq!(config.enabled, before.enabled);
    }

    #[test]
    fn list_presets_matches_eq_presets_const() {
        let listed: Vec<(&str, &str)> = EqConfig::list_presets().collect();
        let expected: Vec<(&str, &str)> = EQ_PRESETS.iter().map(|(k, d, _)| (*k, *d)).collect();
        assert_eq!(listed, expected);
        assert_eq!(listed.len(), EQ_PRESETS.len());
    }

    // ── EqConfig::bass_gain / treble_gain ───────────────────────────────────

    #[test]
    fn bass_gain_reads_first_band() {
        let mut config = EqConfig::default();
        config.bands[0] = 5.0;
        assert_eq!(config.bass_gain(), 5.0);
    }

    #[test]
    fn treble_gain_reads_last_band() {
        let mut config = EqConfig::default();
        config.bands[9] = -3.0;
        assert_eq!(config.treble_gain(), -3.0);
    }

    #[test]
    fn bass_and_treble_gain_are_clamped() {
        let mut config = EqConfig::default();
        config.bands[0] = 100.0;
        config.bands[9] = -100.0;
        assert_eq!(config.bass_gain(), 12.0);
        assert_eq!(config.treble_gain(), -12.0);
    }

    // ── EqConfig::apply_bass_treble ──────────────────────────────────────────

    #[test]
    fn apply_bass_treble_sets_edge_bands_per_decay_formula() {
        let mut config = EqConfig::default();
        let decay = EqConfig::TONE_WEIGHT_DECAY;
        let bass = 6.0f32;
        let treble = -3.0f32;
        config.apply_bass_treble(bass, treble);

        let expected_band0 = (bass * decay.powi(0) + treble * decay.powi(9)).clamp(-12.0, 12.0);
        let expected_band9 = (bass * decay.powi(9) + treble * decay.powi(0)).clamp(-12.0, 12.0);

        assert!((config.bands[0] - expected_band0).abs() < 1e-4);
        assert!((config.bands[9] - expected_band9).abs() < 1e-4);
        assert!(config.enabled);
    }

    #[test]
    fn apply_bass_treble_clamps_inputs_and_outputs() {
        let mut config = EqConfig::default();
        config.apply_bass_treble(1000.0, -1000.0);
        for band in config.bands {
            assert!((-12.0..=12.0).contains(&band));
        }
    }

    // ── Biquad ───────────────────────────────────────────────────────────────

    #[test]
    fn peaking_eq_zero_gain_is_transparent() {
        let mut filter = Biquad::peaking_eq(44_100.0, 1_000.0, 0.0, 1.41);
        let samples = [-0.5f32, 0.3, 0.9, -0.2, 0.0, 1.0, -1.0, 0.42];
        for &sample in &samples {
            let output = filter.process(sample);
            assert!(
                (output - sample).abs() < 1e-4,
                "expected {sample}, got {output}"
            );
        }
    }

    #[test]
    fn reset_after_zero_gain_processing_matches_fresh_filter() {
        let mut filter = Biquad::peaking_eq(44_100.0, 1_000.0, 0.0, 1.41);
        for &sample in &[0.8f32, -0.6, 0.4, -0.2] {
            filter.process(sample);
        }
        filter.reset();

        let mut fresh = Biquad::peaking_eq(44_100.0, 1_000.0, 0.0, 1.41);
        let first_input = 0.37f32;
        let after_reset_output = filter.process(first_input);
        let fresh_output = fresh.process(first_input);
        assert!((after_reset_output - fresh_output).abs() < 1e-4);
    }

    #[test]
    fn reset_actually_clears_internal_state_for_nonzero_gain() {
        // Unlike the 0 dB case, a filter with real gain is *not* an all-pass,
        // so its output depends on prior state -- this exercises reset()
        // meaningfully by comparing two runs of the same impulse sequence.
        let sequence = [1.0f32, 0.0, 0.0, 0.0, 0.5, -0.5];

        let mut filter = Biquad::peaking_eq(44_100.0, 1_000.0, 6.0, 1.41);
        let first_run: Vec<f32> = sequence.iter().map(|&s| filter.process(s)).collect();

        // Perturb state with unrelated samples, then reset.
        filter.process(0.9);
        filter.process(-0.9);
        filter.reset();

        let second_run: Vec<f32> = sequence.iter().map(|&s| filter.process(s)).collect();

        for (a, b) in first_run.iter().zip(second_run.iter()) {
            assert!((a - b).abs() < 1e-5, "expected {a}, got {b}");
        }
    }
}
