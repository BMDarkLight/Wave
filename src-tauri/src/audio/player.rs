#[cfg(target_os = "android")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rodio::source::UniformSourceIterator;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::error::AudioError;

use super::dsp::{
    set_shared_gain, shared_gain, Crossfade, CrossfadeState, EqConfig, Equalizer, SharedGain,
    SoftFade, SoftFadeState, VolumeGain, SOFT_FADE_SECS,
};
use super::normalization::{analyze_peak_amplitude, VolumeNormalizer};
use super::symphonia_source::SymphoniaSource;

// ── Playback modes ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
}

// ── Playback clock ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct PlaybackClock {
    started_at: Option<Instant>,
    elapsed_before_start: Duration,
    duration: Option<Duration>,
}

impl PlaybackClock {
    fn stopped() -> Self {
        Self {
            started_at: None,
            elapsed_before_start: Duration::ZERO,
            duration: None,
        }
    }

    fn raw_elapsed(&self) -> Duration {
        self.started_at
            .map(|started_at| self.elapsed_before_start + started_at.elapsed())
            .unwrap_or(self.elapsed_before_start)
    }

    fn position(&self) -> Duration {
        let elapsed = self.raw_elapsed();
        self.duration
            .map(|duration| elapsed.min(duration))
            .unwrap_or(elapsed)
    }
}

// ── Queue ─────────────────────────────────────────────────────────────────────

/// In-memory playback queue (separate from the library's persisted playlist).
#[derive(Debug, Clone, Default)]
pub struct Queue {
    tracks: Vec<String>,
    current_index: Option<usize>,
    shuffle_order: Option<Vec<usize>>,
    shuffle_pos: usize,
}

impl Queue {
    pub fn set_tracks(&mut self, tracks: Vec<String>) {
        self.tracks = tracks;
        self.current_index = None;
        self.shuffle_order = None;
        self.shuffle_pos = 0;
    }

    /// Rebuild the Fisher-Yates shuffle order when shuffle is enabled.
    fn rebuild_shuffle_order(&mut self) {
        if self.tracks.is_empty() {
            self.shuffle_order = None;
            return;
        }
        let mut order: Vec<usize> = (0..self.tracks.len()).collect();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(42) as usize;
        for i in (1..order.len()).rev() {
            let j = (seed.wrapping_mul(i + 1).wrapping_add(seed)) % (i + 1);
            order.swap(i, j);
        }
        if let Some(idx) = self.current_index {
            if let Some(pos) = order.iter().position(|&v| v == idx) {
                order.swap(0, pos);
            }
        }
        self.shuffle_pos = 0;
        self.shuffle_order = Some(order);
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current_index
    }

    pub fn tracks(&self) -> &[String] {
        &self.tracks
    }

    /// Jump to a specific raw index and return its path.
    pub fn jump(&mut self, index: usize) -> Option<&str> {
        if index >= self.tracks.len() {
            return None;
        }
        self.current_index = Some(index);
        if let Some(ref mut order) = self.shuffle_order {
            if let Some(pos) = order.iter().position(|&v| v == index) {
                self.shuffle_pos = pos;
            }
        }
        self.tracks.get(index).map(String::as_str)
    }

    /// Index that `next` would select, without mutating queue state.
    fn peek_next_index(&self, repeat: &RepeatMode) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        if let Some(ref order) = self.shuffle_order {
            let next_pos = self.shuffle_pos + 1;
            match repeat {
                RepeatMode::All => Some(order[next_pos % order.len()]),
                RepeatMode::Off | RepeatMode::One => {
                    if next_pos < order.len() {
                        Some(order[next_pos])
                    } else {
                        None
                    }
                }
            }
        } else {
            match self.current_index {
                None if !self.tracks.is_empty() => Some(0),
                None => None,
                Some(current) => {
                    let next = current + 1;
                    match repeat {
                        RepeatMode::All => Some(next % self.tracks.len()),
                        RepeatMode::Off | RepeatMode::One => {
                            if next < self.tracks.len() {
                                Some(next)
                            } else {
                                None
                            }
                        }
                    }
                }
            }
        }
    }

    /// Peek at the next track path without advancing the queue.
    pub fn peek_next(&self, repeat: &RepeatMode) -> Option<&str> {
        self.peek_next_index(repeat)
            .and_then(|i| self.tracks.get(i).map(String::as_str))
    }

    /// Advance to the next track. Returns `None` when the queue is exhausted
    /// and `repeat == Off`. With `repeat == All` it wraps around.
    pub fn next(&mut self, repeat: &RepeatMode) -> Option<&str> {
        let next_idx = self.peek_next_index(repeat)?;
        if let Some(ref order) = self.shuffle_order.clone() {
            let next_pos = self.shuffle_pos + 1;
            match repeat {
                RepeatMode::All => self.shuffle_pos = next_pos % order.len(),
                RepeatMode::Off | RepeatMode::One => self.shuffle_pos = next_pos,
            }
        }
        self.current_index = Some(next_idx);
        self.tracks.get(next_idx).map(String::as_str)
    }

    /// Paths from the current queue position through the end of playback order.
    pub fn paths_from_current_forward(&self, repeat: &RepeatMode) -> Vec<String> {
        let mut sim = self.clone();
        let mut paths = Vec::new();
        let Some(start_idx) = sim.current_index else {
            return paths;
        };
        paths.push(sim.tracks[start_idx].clone());
        let extra_limit = match repeat {
            RepeatMode::All => self.tracks.len().saturating_sub(1),
            _ => self.tracks.len().saturating_sub(1),
        };
        let mut added = 0usize;
        while added < extra_limit {
            let Some(next) = sim.peek_next(repeat) else {
                break;
            };
            paths.push(next.to_string());
            sim.next(repeat);
            added += 1;
            if *repeat != RepeatMode::All && sim.peek_next(repeat).is_none() {
                break;
            }
            if *repeat == RepeatMode::All && paths.len() >= self.tracks.len() {
                break;
            }
        }
        paths
    }

    /// Go back to the previous track.
    pub fn previous(&mut self, repeat: &RepeatMode) -> Option<&str> {
        if self.tracks.is_empty() {
            return None;
        }
        let prev_idx = if let Some(ref order) = self.shuffle_order.clone() {
            match (self.shuffle_pos, repeat) {
                (0, RepeatMode::All) => {
                    self.shuffle_pos = order.len() - 1;
                    Some(order[self.shuffle_pos])
                }
                (0, _) => None,
                (pos, _) => {
                    self.shuffle_pos = pos - 1;
                    Some(order[self.shuffle_pos])
                }
            }
        } else {
            let current = self.current_index.unwrap_or(0);
            match (current, repeat) {
                (0, RepeatMode::All) => Some(self.tracks.len() - 1),
                (0, _) => None,
                (idx, _) => Some(idx - 1),
            }
        };

        self.current_index = prev_idx;
        prev_idx.and_then(|i| self.tracks.get(i).map(String::as_str))
    }

    pub fn set_shuffle(&mut self, enabled: bool) {
        if enabled {
            self.rebuild_shuffle_order();
        } else {
            self.shuffle_order = None;
        }
    }

    pub fn is_shuffled(&self) -> bool {
        self.shuffle_order.is_some()
    }

    /// Append a track to the end of the queue.
    pub fn enqueue(&mut self, path: String) {
        let new_idx = self.tracks.len();
        self.tracks.push(path);
        if let Some(ref mut order) = self.shuffle_order {
            order.push(new_idx);
        }
    }

    /// Insert a track so it plays immediately after the current track.
    /// If nothing is playing, it is appended.
    pub fn insert_next(&mut self, path: String) {
        let insert_at = match self.current_index {
            Some(idx) => (idx + 1).min(self.tracks.len()),
            None => self.tracks.len(),
        };
        self.tracks.insert(insert_at, path.clone());
        if let Some(ref mut order) = self.shuffle_order {
            for idx in order.iter_mut() {
                if *idx >= insert_at {
                    *idx += 1;
                }
            }
            let pos = (self.shuffle_pos + 1).min(order.len());
            order.insert(pos, insert_at);
        }
    }

    /// Remove the track at `index` from the raw track list, adjusting
    /// `current_index` and shuffle order accordingly. Returns the removed path.
    pub fn remove_at(&mut self, index: usize) -> Option<String> {
        if index >= self.tracks.len() {
            return None;
        }

        let removed = self.tracks.remove(index);

        match self.current_index {
            Some(current) => {
                if current == index {
                    if self.tracks.is_empty() {
                        self.current_index = None;
                    } else if current >= self.tracks.len() {
                        self.current_index = Some(self.tracks.len() - 1);
                    }
                } else if current > index {
                    self.current_index = Some(current - 1);
                }
            }
            None => {}
        }

        if let Some(ref mut order) = self.shuffle_order {
            let mut new_order = Vec::with_capacity(order.len().saturating_sub(1));
            let mut removed_pos = None;
            for (i, &idx) in order.iter().enumerate() {
                if idx == index {
                    removed_pos = Some(i);
                    continue;
                }
                new_order.push(if idx > index { idx - 1 } else { idx });
            }
            *order = new_order;
            if let Some(rp) = removed_pos {
                if rp <= self.shuffle_pos {
                    self.shuffle_pos = self.shuffle_pos.saturating_sub(1);
                }
            }
            if order.is_empty() {
                self.shuffle_order = None;
                self.shuffle_pos = 0;
            }
        }

        Some(removed)
    }

    /// Move a track from `from` to `to` within the queue, keeping
    /// `current_index` and shuffle indices consistent.
    pub fn move_track(&mut self, from: usize, to: usize) -> bool {
        let len = self.tracks.len();
        if from >= len || to >= len || from == to {
            return false;
        }

        let item = self.tracks.remove(from);
        self.tracks.insert(to, item);

        let remap = |idx: usize| -> usize {
            if idx == from {
                to
            } else if from < to {
                if idx > from && idx <= to {
                    idx - 1
                } else {
                    idx
                }
            } else if idx >= to && idx < from {
                idx + 1
            } else {
                idx
            }
        };

        if let Some(current) = self.current_index {
            self.current_index = Some(remap(current));
        }

        if let Some(ref mut order) = self.shuffle_order {
            for idx in order.iter_mut() {
                *idx = remap(*idx);
            }
        }

        true
    }

    /// Remove all tracks from the queue except the one currently playing.
    pub fn clear_upcoming(&mut self) {
        if let Some(current_idx) = self.current_index {
            if let Some(path) = self.tracks.get(current_idx).cloned() {
                self.tracks = vec![path];
                self.current_index = Some(0);
            } else {
                self.tracks.clear();
                self.current_index = None;
            }
        } else {
            self.tracks.clear();
        }
        self.shuffle_order = None;
        self.shuffle_pos = 0;
    }

    /// Jump to a specific index in the queue and return its path.
    pub fn jump_to(&mut self, index: usize) -> Option<&str> {
        self.jump(index)
    }
}

// ── AudioPlayer ───────────────────────────────────────────────────────────────

/// Wrapper that makes `OutputStream` safe to put in a `Mutex<AudioPlayer>`.
///
/// `rodio::OutputStream` (and the underlying `cpal::Stream`) is `!Send` because
/// CoreAudio's stream handle contains a raw pointer to a callback. In practice it
/// is only ever accessed through the `Mutex` guard – never concurrently – so the
/// unsafety is sound as long as we never send the player to another thread without
/// the lock.
/// Held for its drop side-effect (silences audio when the stream is dropped).
#[allow(dead_code)]
struct SendableStream(OutputStream);

// SAFETY: AudioPlayer is always accessed through a Mutex; the OutputStream is
// never accessed concurrently from multiple threads.
unsafe impl Send for SendableStream {}

struct AudioOutput {
    _stream: SendableStream,
    handle: OutputStreamHandle,
}

pub struct AudioPlayer {
    /// Lazily opened so Android can finish JNI setup before cpal/oboe runs.
    output: Option<AudioOutput>,
    sink: Option<Sink>,
    current_path: Option<PathBuf>,
    clock: PlaybackClock,
    volume: f32,
    pub queue: Queue,
    pub repeat: RepeatMode,
    /// Shared equalizer configuration (may be referenced by a running
    /// `Equalizer` source inside the current sink).
    pub eq_config: Arc<Mutex<EqConfig>>,
    pub(crate) eq_version: Arc<Mutex<u64>>,
    /// Crossfade duration in seconds (0.0 = off).
    crossfade_duration: f32,
    /// Shared state for crossfade track switching.
    crossfade_state: Option<Arc<Mutex<CrossfadeState>>>,
    /// Soft gain ramp for play/pause/seek/stop (shared with the active SoftFade).
    soft_fade: Arc<Mutex<SoftFadeState>>,
    /// Next queue path already appended to the sink so playback can continue
    /// when the current source ends — critical on Android while backgrounded,
    /// where a delayed tick alone can miss the transition.
    prefetched_next: Option<(String, Option<Duration>)>,
    /// Seamless queue transitions when crossfade is off.
    gapless_enabled: bool,
    /// Boost quieter tracks toward the session median peak (off by default).
    volume_normalization_enabled: bool,
    /// Shared separately from the rest of `AudioPlayer` so a background peak
    /// analysis thread can update it without holding the player-wide lock for
    /// the duration of a full-file decode.
    normalizer: Arc<Mutex<VolumeNormalizer>>,
    /// Bumped on every track change; a background analysis result that
    /// arrives for a stale generation is dropped instead of being applied to
    /// whatever now-current track it no longer matches. Only Android defers
    /// analysis to a background thread, so this is unused elsewhere.
    #[cfg(target_os = "android")]
    normalization_generation: Arc<AtomicU64>,
    /// Active ExoPlayer playlist URIs (Android gapless mode).
    #[cfg(target_os = "android")]
    android_gapless_playlist: Vec<String>,
}

impl AudioPlayer {
    /// Create a player without opening an OS audio device.
    ///
    /// Queue / EQ / volume state work immediately; the output stream is opened
    /// on first `play` (or explicit `ensure_output`).
    pub fn new_deferred() -> Self {
        Self {
            output: None,
            sink: None,
            current_path: None,
            clock: PlaybackClock::stopped(),
            volume: 0.8,
            queue: Queue::default(),
            repeat: RepeatMode::default(),
            eq_config: Arc::new(Mutex::new(EqConfig::default())),
            eq_version: Arc::new(Mutex::new(0)),
            crossfade_duration: 0.0,
            crossfade_state: None,
            soft_fade: Arc::new(Mutex::new(SoftFadeState::default())),
            prefetched_next: None,
            gapless_enabled: true,
            volume_normalization_enabled: false,
            normalizer: Arc::new(Mutex::new(VolumeNormalizer::new())),
            #[cfg(target_os = "android")]
            normalization_generation: Arc::new(AtomicU64::new(0)),
            #[cfg(target_os = "android")]
            android_gapless_playlist: Vec::new(),
        }
    }

    pub fn new() -> Result<Self, AudioError> {
        let mut player = Self::new_deferred();
        player.ensure_output()?;
        Ok(player)
    }

    /// Open the default output device if it is not open yet.
    pub fn ensure_output(&mut self) -> Result<(), AudioError> {
        #[cfg(target_os = "android")]
        {
            // ExoPlayer replaces cpal/rodio on Android.
            crate::android::jni::ensure_jni_thread_attached();
            return crate::android::audio::ensure_initialized()
                .map_err(AudioError::StreamCreation);
        }

        #[cfg(not(target_os = "android"))]
        {
            if self.output.is_some() {
                return Ok(());
            }

            crate::android::jni::ensure_jni_thread_attached();
            if !crate::android::jni::android_audio_ready() {
                return Err(AudioError::StreamCreation(
                    "Android audio context is not ready yet — try playing again in a moment"
                        .to_string(),
                ));
            }

            let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                OutputStream::try_default()
            }));

            let (stream, handle) = match opened {
                Ok(Ok(pair)) => pair,
                Ok(Err(error)) => {
                    return Err(AudioError::StreamCreation(format!(
                        "Could not open audio output device: {error}"
                    )));
                }
                Err(payload) => {
                    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    tracing::error!("OutputStream::try_default panicked: {msg}");
                    return Err(AudioError::StreamCreation(format!(
                        "Audio system crash: {msg}"
                    )));
                }
            };

            self.output = Some(AudioOutput {
                _stream: SendableStream(stream),
                handle,
            });
            Ok(())
        }
    }

    /// Create a player that outputs to a specific audio device (by name).
    pub fn new_with_device(device_name: &str) -> Result<Self, AudioError> {
        #[cfg(target_os = "android")]
        {
            let _ = device_name;
            return Ok(Self::new_deferred());
        }

        #[cfg(not(target_os = "android"))]
        {
        use cpal::traits::{DeviceTrait, HostTrait};

        crate::android::jni::ensure_jni_thread_attached();
        if !crate::android::jni::android_audio_ready() {
            return Err(AudioError::StreamCreation(
                "Android audio context is not ready yet — try again in a moment".to_string(),
            ));
        }

        let host = cpal::default_host();
        let device = host
            .output_devices()
            .map_err(|e| AudioError::DeviceUnavailable(e.to_string()))?
            .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
            .ok_or_else(|| AudioError::DeviceUnavailable(format!("Device not found: {device_name}")))?;

        let (stream, handle) = OutputStream::try_from_device(&device)
            .map_err(|error| AudioError::StreamCreation(error.to_string()))?;

        Ok(Self {
            output: Some(AudioOutput {
                _stream: SendableStream(stream),
                handle,
            }),
            sink: None,
            current_path: None,
            clock: PlaybackClock::stopped(),
            volume: 0.8,
            queue: Queue::default(),
            repeat: RepeatMode::default(),
            eq_config: Arc::new(Mutex::new(EqConfig::default())),
            eq_version: Arc::new(Mutex::new(0)),
            crossfade_duration: 0.0,
            crossfade_state: None,
            soft_fade: Arc::new(Mutex::new(SoftFadeState::default())),
            prefetched_next: None,
            gapless_enabled: true,
            volume_normalization_enabled: false,
            normalizer: Arc::new(Mutex::new(VolumeNormalizer::new())),
            #[cfg(target_os = "android")]
            normalization_generation: Arc::new(AtomicU64::new(0)),
            #[cfg(target_os = "android")]
            android_gapless_playlist: Vec::new(),
        })
        }
    }

    /// Gain cell for the desktop (rodio) playback path.
    ///
    /// A full-file peak scan can take a while, and unlike Android there's no
    /// separate "set gain on the running player" call to defer to — gain is
    /// baked into the source chain via [`VolumeGain`]. So instead of
    /// resolving a final `f32` synchronously (which used to block `play()`
    /// under the player lock for the length of the decode — the same
    /// "backend freezes" symptom Android had), this returns a
    /// [`SharedGain`] cell seeded at neutral (1.0) and, on a cache miss,
    /// kicks off a background analysis thread that writes the real gain
    /// into the cell once ready. `VolumeGain` reads the cell live, so the
    /// audible gain updates in place without restarting playback.
    ///
    /// Not itself `cfg`-gated: it's reached only through `build_source`'s
    /// desktop-only call sites, but `build_source` compiles for every
    /// target (Android's `play()` short-circuits to `play_via_exo` before
    /// ever calling it), so this has to stay available everywhere too.
    fn normalization_gain_cell_for_path(&mut self, path: &str) -> SharedGain {
        if !self.volume_normalization_enabled {
            return shared_gain(1.0);
        }
        let cached_gain = {
            let mut normalizer = self.normalizer.lock().unwrap_or_else(|e| e.into_inner());
            normalizer
                .cached_peak(path)
                .map(|peak| normalizer.register_peak(path, peak))
        };
        if let Some(gain) = cached_gain {
            return shared_gain(gain);
        }
        let cell = shared_gain(1.0);
        self.spawn_desktop_peak_analysis(path.to_string(), cell.clone(), false);
        cell
    }

    /// Same as [`Self::normalization_gain_cell_for_path`] but for a
    /// peeked/upcoming track (crossfade or gapless prefetch) that may never
    /// actually play — doesn't count toward the running session median.
    fn peek_normalization_gain_cell_for_path(&self, path: &str) -> SharedGain {
        if !self.volume_normalization_enabled {
            return shared_gain(1.0);
        }
        let cached_gain = {
            let normalizer = self.normalizer.lock().unwrap_or_else(|e| e.into_inner());
            normalizer.cached_peak(path).map(|peak| {
                let median = normalizer.median_peak().unwrap_or(peak);
                VolumeNormalizer::compute_gain(peak, median)
            })
        };
        if let Some(gain) = cached_gain {
            return shared_gain(gain);
        }
        let cell = shared_gain(1.0);
        self.spawn_desktop_peak_analysis(path.to_string(), cell.clone(), true);
        cell
    }

    fn spawn_desktop_peak_analysis(&self, path: String, cell: SharedGain, incoming: bool) {
        let normalizer = self.normalizer.clone();
        if !normalizer.lock().unwrap_or_else(|e| e.into_inner()).try_begin_analysis() {
            // Already at the concurrent-scan cap; leave this cell at neutral
            // gain. It'll be retried (and likely cached by then) next time
            // this path is loaded.
            return;
        }
        std::thread::spawn(move || {
            let peak = analyze_peak_amplitude(&path).unwrap_or_else(|error| {
                tracing::warn!("Peak analysis failed for \"{path}\": {error}");
                0.5
            });
            let gain = {
                let mut normalizer = normalizer.lock().unwrap_or_else(|e| e.into_inner());
                let gain = if incoming {
                    normalizer.cache_peak(&path, peak);
                    let median = normalizer.median_peak().unwrap_or(peak);
                    VolumeNormalizer::compute_gain(peak, median)
                } else {
                    normalizer.register_peak(&path, peak)
                };
                normalizer.end_analysis();
                gain
            };
            set_shared_gain(&cell, gain);
        });
    }

    /// Apply normalization gain for the track that just started playing.
    ///
    /// A full-file peak scan (`PeakAnalyzer` on the Java side) can take
    /// seconds. Blocking here would hold the player lock for that long and
    /// stall every other command (queue navigation, playback-state polling,
    /// etc. all serialize on the same lock), which is exactly the "backend
    /// freezes" symptom normalization used to cause. So: apply a cached gain
    /// immediately if we have one, otherwise start at neutral gain and
    /// refine in the background once analysis finishes.
    #[cfg(target_os = "android")]
    fn apply_android_normalization_for_path(&mut self, path: &str) {
        if !self.volume_normalization_enabled {
            let _ = crate::android::audio::exo_set_track_normalization_gain(1.0);
            return;
        }
        let gen = self.normalization_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let cached_gain = {
            let mut normalizer = self.normalizer.lock().unwrap_or_else(|e| e.into_inner());
            normalizer
                .cached_peak(path)
                .map(|peak| normalizer.register_peak(path, peak))
        };
        if let Some(gain) = cached_gain {
            let _ = crate::android::audio::exo_set_track_normalization_gain(gain);
            return;
        }
        let _ = crate::android::audio::exo_set_track_normalization_gain(1.0);
        self.spawn_android_peak_analysis(path.to_string(), gen, false);
    }

    /// Analyze `path` off the player lock and push the resulting gain once
    /// ready. `incoming` selects the crossfade "next track" gain slot instead
    /// of the active track's, and skips updating the session median (the
    /// track may never actually play, e.g. if crossfade settings change).
    #[cfg(target_os = "android")]
    fn spawn_android_peak_analysis(&self, path: String, gen: u64, incoming: bool) {
        let normalizer = self.normalizer.clone();
        if !normalizer.lock().unwrap_or_else(|e| e.into_inner()).try_begin_analysis() {
            // Already at the concurrent-scan cap (e.g. the user is skipping
            // through tracks faster than analysis finishes). Leave gain at
            // neutral for now; the next play/sync of this path retries.
            return;
        }
        let generation = self.normalization_generation.clone();
        std::thread::spawn(move || {
            let peak = analyze_peak_amplitude(&path).unwrap_or_else(|error| {
                tracing::warn!("Peak analysis failed for \"{path}\": {error}");
                0.5
            });
            let gain = {
                let mut normalizer = normalizer.lock().unwrap_or_else(|e| e.into_inner());
                let gain = if incoming {
                    normalizer.cache_peak(&path, peak);
                    let median = normalizer.median_peak().unwrap_or(peak);
                    VolumeNormalizer::compute_gain(peak, median)
                } else {
                    normalizer.register_peak(&path, peak)
                };
                normalizer.end_analysis();
                gain
            };
            // The active/upcoming track changed while this ran; applying the
            // gain now would land on the wrong track.
            if generation.load(Ordering::SeqCst) != gen {
                return;
            }
            let _ = if incoming {
                crate::android::audio::exo_set_incoming_normalization_gain(gain)
            } else {
                crate::android::audio::exo_set_track_normalization_gain(gain)
            };
        });
    }

    #[cfg(target_os = "android")]
    fn sync_android_crossfade_normalization(&self) {
        if self.crossfade_duration <= 0.0 || !self.volume_normalization_enabled {
            let _ = crate::android::audio::exo_set_incoming_normalization_gain(1.0);
            return;
        }
        let Some(next) = self.queue.peek_next(&self.repeat).map(str::to_string) else {
            let _ = crate::android::audio::exo_set_incoming_normalization_gain(1.0);
            return;
        };
        let cached_gain = {
            let normalizer = self.normalizer.lock().unwrap_or_else(|e| e.into_inner());
            normalizer.cached_peak(&next).map(|peak| {
                let median = normalizer.median_peak().unwrap_or(peak);
                VolumeNormalizer::compute_gain(peak, median)
            })
        };
        if let Some(gain) = cached_gain {
            let _ = crate::android::audio::exo_set_incoming_normalization_gain(gain);
            return;
        }
        let _ = crate::android::audio::exo_set_incoming_normalization_gain(1.0);
        let gen = self.normalization_generation.load(Ordering::SeqCst);
        self.spawn_android_peak_analysis(next, gen, true);
    }

    /// List all available audio output device names.
    pub fn list_output_devices() -> Vec<String> {
        #[cfg(target_os = "android")]
        {
            return vec!["ExoPlayer (system default)".to_string()];
        }
        #[cfg(not(target_os = "android"))]
        {
        use cpal::traits::{DeviceTrait, HostTrait};
        crate::android::jni::ensure_jni_thread_attached();
        let listed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match cpal::default_host().output_devices() {
                Ok(devices) => devices
                    .filter_map(|d| d.name().ok())
                    .filter(|n| !n.is_empty())
                    .collect(),
                Err(_) => vec![],
            }
        }));
        listed.unwrap_or_default()
        }
    }

    fn build_source(
        path: &str,
        track_gain: SharedGain,
        eq_config: Arc<Mutex<EqConfig>>,
        eq_version: Arc<Mutex<u64>>,
        crossfade_duration: f32,
        next_path: Option<&str>,
        next_gain: SharedGain,
        soft_fade: Arc<Mutex<SoftFadeState>>,
    ) -> Result<(Box<dyn Source<Item = f32> + Send + 'static>, Option<Duration>, Option<Arc<Mutex<CrossfadeState>>>), AudioError> {
        let source = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            SymphoniaSource::new(path)
        })) {
            Ok(result) => result?,
            Err(_) => {
                return Err(AudioError::Decode(format!(
                    "Audio decoding crashed while opening \"{path}\". \
                     The file may be corrupted."
                )));
            }
        };
        let duration = source.total_duration();
        let converted = source.convert_samples();
        let gained = VolumeGain::new(converted, track_gain);
        let eq_config_for_next = eq_config.clone();
        let eq_version_for_next = eq_version.clone();
        let eq = Equalizer::new(gained, eq_config, eq_version);

        // Wrap in Crossfade if enabled and we have a next track.
        let chain: Box<dyn Source<Item = f32> + Send> = if crossfade_duration > 0.0 {
            if let Some(next_path) = next_path {
                let next_source = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    SymphoniaSource::new(next_path)
                })) {
                    Ok(Ok(source)) => Some(source.convert_samples()),
                    Ok(Err(error)) => {
                        tracing::warn!("Crossfade preload failed for \"{next_path}\": {error}");
                        None
                    }
                    Err(_) => {
                        tracing::warn!("Crossfade preload panicked for \"{next_path}\"");
                        None
                    }
                };

                if let Some(next_converted) = next_source {
                    let next_gained = VolumeGain::new(next_converted, next_gain);
                    let next_eq =
                        Equalizer::new(next_gained, eq_config_for_next, eq_version_for_next);
                    // Match channel count / sample rate so per-sample mixing is valid.
                    let target_channels = eq.channels();
                    let target_sr = eq.sample_rate();
                    let next_matched: Box<dyn Source<Item = f32> + Send> =
                        if next_eq.channels() != target_channels || next_eq.sample_rate() != target_sr
                        {
                            Box::new(UniformSourceIterator::new(
                                next_eq,
                                target_channels,
                                target_sr,
                            ))
                        } else {
                            Box::new(next_eq)
                        };
                    let (crossfade, state) = Crossfade::new(
                        Box::new(eq),
                        Some(next_matched),
                        crossfade_duration,
                        Some(path.to_string()),
                        Some(next_path.to_string()),
                    );
                    let faded = SoftFade::new(Box::new(crossfade), soft_fade);
                    return Ok((Box::new(faded), duration, Some(state)));
                }
            }
            Box::new(eq)
        } else {
            Box::new(eq)
        };

        let faded = SoftFade::new(chain, soft_fade);
        Ok((Box::new(faded), duration, None))
    }

    fn set_soft_fade_target(&self, target: f32) {
        if let Ok(mut state) = self.soft_fade.lock() {
            state.target = target.clamp(0.0, 1.0);
        }
    }

    fn wait_soft_fade(&self) {
        let ms = ((SOFT_FADE_SECS * 1000.0) as u64).saturating_add(8);
        std::thread::sleep(Duration::from_millis(ms));
    }

    fn fade_out_blocking(&self) {
        self.set_soft_fade_target(0.0);
        self.wait_soft_fade();
    }

    pub fn play(&mut self, path: &str) -> Result<(), AudioError> {
        #[cfg(target_os = "android")]
        {
            return self.play_via_exo(path);
        }

        #[cfg(not(target_os = "android"))]
        {
        self.ensure_output()?;
        if self.sink.is_some() && self.is_playing() {
            self.fade_out_blocking();
        }

        if let Some(sink) = self.sink.take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sink.stop();
            }));
        }
        self.prefetched_next = None;

        // SoftFade instances start at gain 0 and ramp toward this target.
        self.set_soft_fade_target(1.0);

        let next_path = self
            .queue
            .peek_next(&self.repeat)
            .map(|s| s.to_string());
        let track_gain = self.normalization_gain_cell_for_path(path);
        let next_gain = next_path
            .as_deref()
            .map(|next| self.peek_normalization_gain_cell_for_path(next))
            .unwrap_or_else(|| shared_gain(1.0));
        let (source, duration, crossfade_state) = Self::build_source(
            path,
            track_gain,
            self.eq_config.clone(),
            self.eq_version.clone(),
            self.crossfade_duration,
            next_path.as_deref(),
            next_gain,
            self.soft_fade.clone(),
        )?;

        let handle = &self
            .output
            .as_ref()
            .expect("output ensured")
            .handle;

        let sink = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Sink::try_new(handle)
        })) {
            Ok(Ok(sink)) => sink,
            Ok(Err(error)) => {
                return Err(AudioError::SinkCreation(format!(
                    "Could not initialise audio playback: {error}"
                )));
            }
            Err(_) => {
                return Err(AudioError::SinkCreation(
                    "Audio playback initialisation crashed. \
                     Another app may be using the speaker exclusively."
                        .to_string(),
                ));
            }
        };

        sink.set_volume(self.volume);
        sink.append(source);
        sink.play();

        self.sink = Some(sink);
        self.current_path = Some(PathBuf::from(path));
        self.clock = PlaybackClock {
            started_at: Some(Instant::now()),
            elapsed_before_start: Duration::ZERO,
            duration,
        };

        self.crossfade_state = crossfade_state;
        self.prefetch_next_into_sink();

        Ok(())
        }
    }

    #[cfg(target_os = "android")]
    fn play_via_exo(&mut self, path: &str) -> Result<(), AudioError> {
        self.ensure_output()?;
        let use_gapless = self.gapless_enabled && self.crossfade_duration <= 0.0;
        if use_gapless {
            let paths = self.queue.paths_from_current_forward(&self.repeat);
            let paths = if paths.is_empty() {
                vec![path.to_string()]
            } else {
                paths
            };
            self.android_gapless_playlist = paths.clone();
            if paths.len() > 1 {
                crate::android::audio::exo_play_media_items(&paths, 0)
                    .map_err(AudioError::Decode)?;
            } else {
                crate::android::audio::exo_play_uri(path).map_err(AudioError::Decode)?;
            }
        } else {
            self.android_gapless_playlist.clear();
            crate::android::audio::exo_play_uri(path).map_err(AudioError::Decode)?;
        }
        self.apply_android_normalization_for_path(path);
        let _ = crate::android::audio::exo_set_volume(self.volume);
        self.sync_android_eq();

        let duration = crate::android::audio::exo_get_duration()
            .ok()
            .filter(|&ms| ms > 0)
            .map(|ms| Duration::from_millis(ms as u64));

        self.sink = None;
        self.prefetched_next = None;
        self.crossfade_state = None;
        self.current_path = Some(PathBuf::from(path));
        self.clock = PlaybackClock {
            started_at: Some(Instant::now()),
            elapsed_before_start: Duration::ZERO,
            duration,
        };
        self.sync_android_playback_extras();
        Ok(())
    }

    #[cfg(target_os = "android")]
    fn sync_android_playback_extras(&self) {
        let _ = crate::android::audio::exo_set_crossfade_duration(self.crossfade_duration);
        let _ = crate::android::audio::exo_set_gapless_enabled(self.gapless_enabled);
        self.sync_android_crossfade_normalization();
        if self.crossfade_duration > 0.0 {
            let next = self.queue.peek_next(&self.repeat).map(str::to_string);
            let _ = crate::android::audio::exo_set_upcoming_uri(next.as_deref());
            return;
        }
        let _ = crate::android::audio::exo_set_upcoming_uri(None);
    }

    #[cfg(target_os = "android")]
    fn apply_android_gapless_index(&mut self, index: usize) -> bool {
        let Some(path) = self.android_gapless_playlist.get(index).cloned() else {
            return false;
        };
        let new_path_buf = PathBuf::from(&path);
        if self.current_path.as_deref() == Some(new_path_buf.as_path()) {
            return false;
        }
        if let Some(idx) = self.queue.tracks().iter().position(|p| p == &path) {
            let _ = self.queue.jump(idx);
        }
        let duration = crate::android::audio::exo_get_duration()
            .ok()
            .filter(|&ms| ms > 0)
            .map(|ms| Duration::from_millis(ms as u64));
        self.current_path = Some(new_path_buf);
        self.clock = PlaybackClock {
            started_at: Some(Instant::now()),
            elapsed_before_start: Duration::ZERO,
            duration,
        };
        true
    }

    #[cfg(target_os = "android")]
    fn check_android_gapless_handoff(&mut self) -> bool {
        if !self.gapless_enabled || self.crossfade_duration > 0.0 {
            return false;
        }
        let Ok(Some(index)) = crate::android::audio::exo_consume_media_index_change() else {
            return false;
        };
        self.apply_android_gapless_index(index as usize)
    }

    #[cfg(target_os = "android")]
    fn try_android_gapless_skip_forward(&mut self) -> Result<bool, AudioError> {
        if !self.gapless_enabled || self.crossfade_duration > 0.0 {
            return Ok(false);
        }
        if !crate::android::audio::exo_seek_to_next_media_item().unwrap_or(false) {
            return Ok(false);
        }
        if let Ok(index) = crate::android::audio::exo_get_current_media_index() {
            let _ = self.apply_android_gapless_index(index as usize);
        }
        Ok(true)
    }

    #[cfg(target_os = "android")]
    fn check_android_crossfade_handoff(&mut self) -> bool {
        let Ok(Some(next_path)) = crate::android::audio::exo_consume_crossfade_handoff() else {
            return false;
        };

        let new_path_buf = PathBuf::from(&next_path);
        if self.current_path.as_deref() == Some(new_path_buf.as_path()) {
            return false;
        }

        let peeked = self.queue.peek_next(&self.repeat).map(str::to_string);
        if peeked.as_deref() == Some(next_path.as_str()) {
            let _ = self.queue.next(&self.repeat);
        } else if let Some(idx) = self.queue.tracks().iter().position(|p| p == &next_path) {
            let _ = self.queue.jump(idx);
        }

        let duration = crate::android::audio::exo_get_duration()
            .ok()
            .filter(|&ms| ms > 0)
            .map(|ms| Duration::from_millis(ms as u64));

        self.current_path = Some(new_path_buf);
        self.clock = PlaybackClock {
            started_at: Some(Instant::now()),
            elapsed_before_start: Duration::ZERO,
            duration,
        };
        self.apply_android_normalization_for_path(&next_path);
        self.sync_android_playback_extras();
        true
    }

    /// Append the upcoming queue track to the active sink (best-effort).
    fn prefetch_next_into_sink(&mut self) {
        if !self.gapless_enabled {
            return;
        }
        if self.repeat == RepeatMode::One {
            return;
        }
        // Skip sink prefetch when crossfade is enabled — the current source
        // already contains the next track internally for the transition.
        if self.crossfade_duration > 0.0 {
            return;
        }
        let Some(next_path) = self.queue.peek_next(&self.repeat).map(str::to_string) else {
            return;
        };
        let next_gain = self.peek_normalization_gain_cell_for_path(&next_path);
        let Ok((source, duration, _state)) = Self::build_source(
            &next_path,
            next_gain,
            self.eq_config.clone(),
            self.eq_version.clone(),
            0.0,
            None,
            shared_gain(1.0),
            self.soft_fade.clone(),
        ) else {
            return;
        };
        if let Some(sink) = self.sink.as_ref() {
            sink.append(source);
            self.prefetched_next = Some((next_path, duration));
        }
    }

    /// Adopt a track that is already playing via sink prefetch (no restart).
    fn adopt_prefetched(&mut self, path: &str, duration: Option<Duration>) {
        self.current_path = Some(PathBuf::from(path));
        // If the prefetched source has already been audible for a bit (tick
        // latency), keep the clock near zero — we can't know exact sink offset
        // cheaply, and restarting would be worse.
        self.clock = PlaybackClock {
            started_at: Some(Instant::now()),
            elapsed_before_start: Duration::ZERO,
            duration,
        };
        self.prefetched_next = None;
        self.crossfade_state = None;
        self.prefetch_next_into_sink();
    }

    /// Load a track at `position_secs` without starting playback (session restore).
    pub fn load_paused_at(&mut self, path: &str, position_secs: f64) -> Result<(), AudioError> {
        #[cfg(target_os = "android")]
        {
            return self.load_paused_at_exo(path, position_secs);
        }

        #[cfg(not(target_os = "android"))]
        {
            return self.load_paused_at_desktop(path, position_secs);
        }
    }

    #[cfg(target_os = "android")]
    fn load_paused_at_exo(&mut self, path: &str, position_secs: f64) -> Result<(), AudioError> {
        self.ensure_output()?;
        let position_ms = (position_secs.max(0.0) * 1000.0) as i64;
        crate::android::audio::exo_prepare_uri_at(path, position_ms).map_err(AudioError::Decode)?;
        let _ = crate::android::audio::exo_set_volume(self.volume);
        self.sync_android_eq();

        let duration = crate::android::audio::exo_get_duration()
            .ok()
            .filter(|&ms| ms > 0)
            .map(|ms| Duration::from_millis(ms as u64));

        self.sink = None;
        self.prefetched_next = None;
        self.crossfade_state = None;
        self.current_path = Some(PathBuf::from(path));
        self.clock = PlaybackClock {
            started_at: None,
            elapsed_before_start: Duration::from_secs_f64(position_secs.max(0.0)),
            duration,
        };
        self.sync_android_playback_extras();
        Ok(())
    }

    #[cfg(not(target_os = "android"))]
    fn load_paused_at_desktop(&mut self, path: &str, position_secs: f64) -> Result<(), AudioError> {
        self.ensure_output()?;

        if self.sink.is_some() {
            if let Some(sink) = self.sink.take() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    sink.stop();
                }));
            }
        }
        self.prefetched_next = None;
        self.crossfade_state = None;
        self.set_soft_fade_target(1.0);

        let offset = Duration::from_secs_f64(position_secs.max(0.0));
        let track_gain = self.normalization_gain_cell_for_path(path);
        let (source, duration, crossfade_state) = Self::build_source(
            path,
            track_gain,
            self.eq_config.clone(),
            self.eq_version.clone(),
            0.0,
            None,
            shared_gain(1.0),
            self.soft_fade.clone(),
        )?;

        let handle = &self
            .output
            .as_ref()
            .expect("output ensured")
            .handle;

        let sink = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Sink::try_new(handle)
        })) {
            Ok(Ok(sink)) => sink,
            Ok(Err(error)) => {
                return Err(AudioError::SinkCreation(format!(
                    "Could not initialise audio playback: {error}"
                )));
            }
            Err(_) => {
                return Err(AudioError::SinkCreation(
                    "Audio playback initialisation crashed.".to_string(),
                ));
            }
        };

        sink.set_volume(self.volume);
        sink.append(source);
        sink.pause();
        sink.try_seek(offset)
            .map_err(|error| AudioError::Decode(format!("Seek failed: {error}")))?;

        self.sink = Some(sink);
        self.current_path = Some(PathBuf::from(path));
        self.clock = PlaybackClock {
            started_at: None,
            elapsed_before_start: offset,
            duration,
        };
        self.crossfade_state = crossfade_state;
        Ok(())
    }

    /// True when the sink still has the prefetched follow-up buffered or playing.
    fn has_sink_prefetch(&self) -> bool {
        self.prefetched_next.is_some()
            || self
                .sink
                .as_ref()
                .is_some_and(|sink| sink.len() > 1)
    }

    pub fn pause(&mut self) -> Result<(), AudioError> {
        #[cfg(target_os = "android")]
        {
            if self.current_path.is_none() {
                return Err(AudioError::NoTrackLoaded);
            }
            let position = self.position_seconds();
            crate::android::audio::exo_pause().map_err(AudioError::Decode)?;
            self.clock.elapsed_before_start = Duration::from_secs_f64(position.max(0.0));
            self.clock.started_at = None;
            return Ok(());
        }

        #[cfg(not(target_os = "android"))]
        match &self.sink {
            Some(sink) => {
                if !sink.is_paused() {
                    self.fade_out_blocking();
                }
                sink.pause();
                self.clock.elapsed_before_start = self.clock.position();
                self.clock.started_at = None;
                Ok(())
            }
            None => Err(AudioError::NoTrackLoaded),
        }
    }

    pub fn resume(&mut self) -> Result<(), AudioError> {
        #[cfg(target_os = "android")]
        {
            if self.current_path.is_none() {
                return Err(AudioError::NoTrackLoaded);
            }
            crate::android::audio::exo_play().map_err(AudioError::Decode)?;
            self.clock.started_at = Some(Instant::now());
            return Ok(());
        }

        #[cfg(not(target_os = "android"))]
        match &self.sink {
            Some(sink) => {
                self.set_soft_fade_target(0.0);
                sink.play();
                self.set_soft_fade_target(1.0);
                self.clock.started_at = Some(Instant::now());
                Ok(())
            }
            None => Err(AudioError::NoTrackLoaded),
        }
    }

    pub fn stop(&mut self) -> Result<(), AudioError> {
        #[cfg(target_os = "android")]
        {
            let _ = crate::android::audio::exo_stop();
            self.sink = None;
            self.current_path = None;
            self.prefetched_next = None;
            self.crossfade_state = None;
            self.clock = PlaybackClock::stopped();
            return Ok(());
        }

        #[cfg(not(target_os = "android"))]
        {
        if self.sink.is_some() && self.is_playing() {
            self.fade_out_blocking();
        }
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.current_path = None;
        self.prefetched_next = None;
        self.crossfade_state = None;
        self.set_soft_fade_target(1.0);
        self.clock = PlaybackClock::stopped();
        Ok(())
        }
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), AudioError> {
        #[cfg(target_os = "android")]
        {
            if self.current_path.is_none() {
                return Err(AudioError::NoTrackLoaded);
            }
            let position_ms = (seconds.max(0.0) * 1000.0) as i64;
            let was_playing = self.clock.started_at.is_some();
            crate::android::audio::exo_seek(position_ms).map_err(AudioError::Decode)?;
            self.clock.elapsed_before_start = Duration::from_millis(position_ms as u64);
            self.clock.started_at = was_playing.then(Instant::now);
            return Ok(());
        }

        #[cfg(not(target_os = "android"))]
        {
        let offset = Duration::from_secs_f64(seconds.max(0.0));
        let path = self
            .current_path
            .as_ref()
            .and_then(|p| p.to_str())
            .map(str::to_string)
            .ok_or(AudioError::NoTrackLoaded)?;

        let was_playing = self.is_playing();
        if was_playing {
            self.fade_out_blocking();
        }

        // Seeking leaves any sink-appended follow-up track in place. Rebuild so
        // we don't later hear a few ms of the old next and then restart it.
        if self.has_sink_prefetch() {
            self.play(&path)?;
            if let Some(sink) = self.sink.as_ref() {
                sink.try_seek(offset)
                    .map_err(|error| AudioError::Decode(format!("Seek failed: {error}")))?;
            }
            self.clock.elapsed_before_start = offset;
            self.clock.started_at = was_playing.then(Instant::now);
            if !was_playing {
                let _ = self.pause();
            } else {
                // play() already faded in; SoftFade::try_seek reset gain to 0 —
                // nudge target so the post-seek ramp restarts.
                self.set_soft_fade_target(1.0);
            }
            return Ok(());
        }

        match &self.sink {
            Some(sink) => {
                sink.try_seek(offset)
                    .map_err(|error| AudioError::Decode(format!("Seek failed: {error}")))?;

                self.clock.elapsed_before_start = offset;
                self.clock.started_at = was_playing.then(Instant::now);
                self.prefetched_next = None;
                // Crossfade may promote the incoming track inside try_seek when
                // the UI already handed off at fade-start — adopt that path so
                // transport and metadata stay on the song the scrubber controls.
                self.adopt_crossfade_logical_track();
                if was_playing {
                    self.set_soft_fade_target(1.0);
                }

                Ok(())
            }
            None => Err(AudioError::NoTrackLoaded),
        }
        }
    }

    pub fn set_volume(&mut self, volume: f32) -> Result<(), AudioError> {
        if !(0.0..=1.0).contains(&volume) {
            return Err(AudioError::InvalidVolume);
        }
        self.volume = volume;
        #[cfg(target_os = "android")]
        {
            let _ = crate::android::audio::exo_set_volume(volume);
        }
        #[cfg(not(target_os = "android"))]
        if let Some(ref sink) = self.sink {
            sink.set_volume(volume);
        }
        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        #[cfg(target_os = "android")]
        {
            // Rust transport clock is authoritative — Exo JNI cache is only
            // refreshed on the main looper, so worker-thread polls lie.
            return self.current_path.is_some() && self.clock.started_at.is_some();
        }
        #[cfg(not(target_os = "android"))]
        match &self.sink {
            Some(sink) => !sink.is_paused() && sink.len() > 0,
            None => false,
        }
    }

    pub fn is_paused(&self) -> bool {
        #[cfg(target_os = "android")]
        {
            return self.current_path.is_some() && !self.is_playing();
        }
        #[cfg(not(target_os = "android"))]
        match &self.sink {
            Some(sink) => sink.is_paused(),
            None => false,
        }
    }

    /// True when the sink has drained (natural end-of-track).
    pub fn sink_exhausted(&self) -> bool {
        #[cfg(target_os = "android")]
        {
            if self.current_path.is_none() {
                return false;
            }
            return crate::android::audio::exo_playback_ended().unwrap_or(false);
        }
        #[cfg(not(target_os = "android"))]
        match &self.sink {
            Some(sink) => sink.empty(),
            None => self.current_path.is_some(),
        }
    }

    /// True when the current track has reached end-of-stream and should advance.
    ///
    /// On some Android/cpal backends the sink never reports empty (or pauses
    /// instead), so we also treat wall-clock past known duration as finished.
    /// When the next track was prefetched into the sink, duration-end means the
    /// prefetched source is already audible and we only need to adopt it.
    pub fn should_auto_advance(&self) -> bool {
        if self.current_path.is_none() {
            return false;
        }

        #[cfg(target_os = "android")]
        {
            if crate::android::audio::exo_playback_ended().unwrap_or(false) {
                return true;
            }
            if self.gapless_enabled
                && self.crossfade_duration <= 0.0
                && crate::android::audio::exo_has_next_media_item().unwrap_or(false)
            {
                return false;
            }
            // Fallback: wall-clock past known duration (ExoPlayer duration may
            // arrive late after prepare).
            return self.clock.duration.is_some_and(|duration| {
                let grace = Duration::from_millis(350);
                self.clock.raw_elapsed() >= duration.saturating_add(grace)
                    && !self.is_playing()
            });
        }

        #[cfg(not(target_os = "android"))]
        {
        // A crossfade source keeps playing the *next* track in the same sink after
        // the outgoing track ends. Wall-clock / pending_next alone are not enough:
        // once the mixer promotes, pending_next flips false while audio is still
        // mid-song — and a premature play_next() restarts that track from 0.
        // Only advance when this sink is truly exhausted (then start the following track).
        if self.crossfade_state.is_some() && !self.sink_exhausted() {
            return false;
        }

        let at_duration_end = self.clock.duration.is_some_and(|duration| {
            let grace = Duration::from_millis(350);
            self.clock.raw_elapsed() >= duration.saturating_add(grace)
        });

        // Prefetched next is already in the sink — only adopt once the current
        // source has actually finished (sink has drained down to the follow-up).
        // Adopting earlier and calling play() on a path mismatch was restarting
        // the next track after a few milliseconds of audio.
        if self.prefetched_next.is_some() {
            let sink_len = self.sink.as_ref().map(|s| s.len()).unwrap_or(0);
            let past_start = self.clock.raw_elapsed() >= Duration::from_millis(500);
            if sink_len <= 1 && past_start {
                return true;
            }
            if at_duration_end && sink_len <= 1 {
                return true;
            }
            // Still playing the outgoing source (len >= 2) — wait.
            return false;
        }

        if self.is_paused() {
            // Some Android backends pause when the source ends rather than
            // leaving an idle non-paused empty sink.
            return self.sink_exhausted() || at_duration_end;
        }

        if !self.is_playing() {
            return true;
        }

        at_duration_end
        }
    }

    /// Back-compat alias used by older call sites.
    pub fn has_finished_naturally(&self) -> bool {
        self.should_auto_advance()
    }

    pub fn get_current_path(&self) -> Option<&PathBuf> {
        self.current_path.as_ref()
    }

    pub fn position_seconds(&self) -> f64 {
        #[cfg(target_os = "android")]
        {
            if self.current_path.is_some() {
                // Always prefer the Rust clock for loaded tracks. Exo position
                // JNI reads a cache that is only refreshed on the main looper,
                // so polls from Rust worker / Tauri command threads freeze after
                // seek while the MediaSession notification keeps extrapolating.
                return self.clock.position().as_secs_f64();
            }
        }
        self.clock.position().as_secs_f64()
    }

    pub fn duration_seconds(&self) -> Option<f64> {
        #[cfg(target_os = "android")]
        {
            if let Ok(ms) = crate::android::audio::exo_get_duration() {
                if ms > 0 {
                    return Some(ms as f64 / 1000.0);
                }
            }
        }
        self.clock.duration.map(|duration| duration.as_secs_f64())
    }

    /// Apply a crossfade UI/queue handoff (signaled at fade start).
    ///
    /// Returns `true` when the logical current track changed.
    pub fn check_crossfade_track_switch(&mut self) -> bool {
        #[cfg(target_os = "android")]
        {
            if self.check_android_gapless_handoff() {
                return true;
            }
            return self.check_android_crossfade_handoff();
        }

        #[cfg(not(target_os = "android"))]
        {
        let Some(state) = self.crossfade_state.clone() else {
            return false;
        };
        let Ok(mut guard) = state.lock() else {
            return false;
        };
        if !guard.track_switched {
            return false;
        }
        guard.track_switched = false;

        let Some(new_path) = guard.current_path.clone() else {
            return false;
        };
        let duration = guard.duration;
        let position = guard.position;
        drop(guard);

        let new_path_buf = PathBuf::from(&new_path);
        if self.current_path.as_deref() == Some(new_path_buf.as_path()) {
            return false;
        }

        // Keep the in-memory queue index aligned with the track now audible.
        let peeked = self.queue.peek_next(&self.repeat).map(str::to_string);
        if peeked.as_deref() == Some(new_path.as_str()) {
            let _ = self.queue.next(&self.repeat);
        } else if let Some(idx) = self.queue.tracks().iter().position(|p| p == &new_path) {
            let _ = self.queue.jump(idx);
        }

        let was_playing = self.is_playing();
        self.current_path = Some(new_path_buf);
        self.clock = PlaybackClock {
            started_at: was_playing.then(Instant::now),
            elapsed_before_start: position,
            duration,
        };
        true
        }
    }

    /// Align `current_path` / duration with the crossfade mixer after a seek.
    ///
    /// Unlike [`Self::check_crossfade_track_switch`], this does not require the
    /// `track_switched` latch — seek may promote the incoming source without
    /// re-signaling a switch.
    fn adopt_crossfade_logical_track(&mut self) {
        let Some(state) = self.crossfade_state.clone() else {
            return;
        };
        let Ok(guard) = state.lock() else {
            return;
        };
        let Some(new_path) = guard.current_path.clone() else {
            return;
        };
        let duration = guard.duration;
        drop(guard);

        let new_path_buf = PathBuf::from(&new_path);
        if self.current_path.as_deref() != Some(new_path_buf.as_path()) {
            let peeked = self.queue.peek_next(&self.repeat).map(str::to_string);
            if peeked.as_deref() == Some(new_path.as_str()) {
                let _ = self.queue.next(&self.repeat);
            } else if let Some(idx) = self.queue.tracks().iter().position(|p| p == &new_path) {
                let _ = self.queue.jump(idx);
            }
            self.current_path = Some(new_path_buf);
        }
        if duration.is_some() {
            self.clock.duration = duration;
        }
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    // ── Equalizer ─────────────────────────────────────────────────────────────

    pub fn eq_settings(&self) -> EqConfig {
        self.eq_config.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_eq_bands(&mut self, bands: [f32; 10]) {
        let mut cfg = self.eq_config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.bands = bands;
        *self.eq_version.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        #[cfg(target_os = "android")]
        {
            drop(cfg);
            self.sync_android_eq();
        }
    }

    pub fn set_eq_enabled(&mut self, enabled: bool) {
        let mut cfg = self.eq_config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.enabled = enabled;
        *self.eq_version.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        #[cfg(target_os = "android")]
        {
            drop(cfg);
            self.sync_android_eq();
        }
    }

    pub fn apply_eq_preset(&mut self, name: &str) -> Result<(), String> {
        let mut cfg = self.eq_config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.apply_preset(name)
            .ok_or_else(|| {
                let names: Vec<&str> = EqConfig::list_presets().map(|(n, _)| n).collect();
                format!("Unknown EQ preset \"{name}\". Available: {}", names.join(", "))
            })?;
        *self.eq_version.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        #[cfg(target_os = "android")]
        {
            drop(cfg);
            self.sync_android_eq();
        }
        Ok(())
    }

    #[cfg(target_os = "android")]
    fn sync_android_eq(&self) {
        let cfg = self.eq_config.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let _ = crate::android::audio::exo_set_eq_bands(&cfg.bands);
        let _ = crate::android::audio::exo_set_eq_enabled(cfg.enabled);
    }

    // ── Crossfade ───────────────────────────────────────────────────────────────

    pub fn crossfade_duration(&self) -> f32 {
        self.crossfade_duration
    }

    pub fn set_crossfade_duration(&mut self, duration: f32) {
        self.crossfade_duration = duration.clamp(0.0, 8.0);
        // Keep settings/export EqConfig in sync with the playback field.
        if let Ok(mut cfg) = self.eq_config.lock() {
            cfg.crossfade_duration = self.crossfade_duration;
        }
        #[cfg(target_os = "android")]
        self.sync_android_playback_extras();
    }

    pub fn gapless_enabled(&self) -> bool {
        self.gapless_enabled
    }

    pub fn set_gapless_enabled(&mut self, enabled: bool) {
        self.gapless_enabled = enabled;
        #[cfg(target_os = "android")]
        self.sync_android_playback_extras();
    }

    pub fn volume_normalization_enabled(&self) -> bool {
        self.volume_normalization_enabled
    }

    pub fn set_volume_normalization_enabled(&mut self, enabled: bool) {
        self.volume_normalization_enabled = enabled;
        self.normalizer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_enabled(enabled);
        #[cfg(target_os = "android")]
        {
            if !enabled {
                let _ = crate::android::audio::exo_set_track_normalization_gain(1.0);
                let _ = crate::android::audio::exo_set_incoming_normalization_gain(1.0);
            } else if let Some(path) = self
                .current_path
                .as_ref()
                .and_then(|p| p.to_str())
                .map(str::to_string)
            {
                self.apply_android_normalization_for_path(&path);
                self.sync_android_playback_extras();
            }
        }
    }

    /// Query the current default audio output device name (live, every call).
    pub fn current_output_name() -> String {
        #[cfg(target_os = "android")]
        {
            return "ExoPlayer".to_string();
        }
        #[cfg(not(target_os = "android"))]
        {
        use cpal::traits::{DeviceTrait, HostTrait};
        let name = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cpal::default_host()
                .default_output_device()
                .and_then(|d| d.name().ok())
                .unwrap_or_else(|| "Unknown".to_string())
        }));
        name.unwrap_or_else(|_| "Unknown".to_string())
        }
    }

    pub fn play_next(&mut self) -> Result<Option<String>, AudioError> {
        if self.repeat == RepeatMode::One {
            if let Some(path) = self.current_path.clone() {
                let path_str = path
                    .to_str()
                    .ok_or_else(|| AudioError::Decode("Invalid path encoding".to_string()))?
                    .to_string();
                self.play(&path_str)?;
                return Ok(Some(path_str));
            }
        }

        // Sink prefetch appends the next track *after* the current source.
        // Adopting it mid-song only updates current_path (UI) while the
        // previous source keeps playing — so only adopt when that source
        // has actually finished. Manual Next must tear down and restart.
        #[cfg(target_os = "android")]
        if self.try_android_gapless_skip_forward()? {
            return Ok(
                self.current_path
                    .as_ref()
                    .and_then(|p| p.to_str().map(str::to_string)),
            );
        }

        if let Some((prefetched, duration)) = self.prefetched_next.take() {
            if self.gapless_enabled && self.prefetched_is_now_audible() {
                let peeked = self.queue.peek_next(&self.repeat).map(str::to_string);
                if peeked.as_deref() == Some(prefetched.as_str()) {
                    let _ = self.queue.next(&self.repeat);
                    self.adopt_prefetched(&prefetched, duration);
                    return Ok(Some(prefetched));
                }
                if let Some(idx) = self.queue.tracks().iter().position(|p| p == &prefetched) {
                    let _ = self.queue.jump(idx);
                    self.adopt_prefetched(&prefetched, duration);
                    return Ok(Some(prefetched));
                }
                // Prefetch no longer matches the queue — fall through.
            }
            // Mid-song skip (or stale buffer): rebuild from the real next track.
            // prefetched_next was cleared by take(); play() also clears the sink.
        }

        let path = self.queue.next(&self.repeat).map(str::to_string);
        if let Some(ref next_path) = path {
            self.play(next_path)?;
        }
        Ok(path)
    }

    /// True when sink-prefetched audio is what speakers are hearing now
    /// (outgoing source drained), so adopting it is safe.
    fn prefetched_is_now_audible(&self) -> bool {
        let sink_len = self.sink.as_ref().map(|s| s.len()).unwrap_or(0);
        let past_start = self.clock.raw_elapsed() >= Duration::from_millis(500);
        let at_duration_end = self.clock.duration.is_some_and(|duration| {
            let grace = Duration::from_millis(350);
            self.clock.raw_elapsed() >= duration.saturating_add(grace)
        });
        // len <= 1 ⇒ only the follow-up (or empty) remains in the sink.
        (sink_len <= 1 && past_start) || (at_duration_end && sink_len <= 1) || self.sink_exhausted()
    }

    pub fn play_previous(&mut self) -> Result<Option<String>, AudioError> {
        if self.position_seconds() > 3.0 {
            self.seek(0.0)?;
            return Ok(self
                .current_path
                .as_ref()
                .and_then(|path| path.to_str().map(str::to_string)));
        }
        let path = self.queue.previous(&self.repeat).map(str::to_string);
        if let Some(ref previous_path) = path {
            self.play(previous_path)?;
        }
        Ok(path)
    }

    // ── Queue manipulation ───────────────────────────────────────────────────

    pub fn enqueue(&mut self, path: &str) {
        self.queue.enqueue(path.to_string());
    }

    pub fn insert_next(&mut self, path: &str) {
        self.queue.insert_next(path.to_string());
    }

    pub fn remove_from_queue(&mut self, index: usize) -> Option<String> {
        self.queue.remove_at(index)
    }

    pub fn move_queue_track(&mut self, from: usize, to: usize) -> bool {
        self.queue.move_track(from, to)
    }

    pub fn clear_upcoming(&mut self) {
        self.queue.clear_upcoming();
    }

    pub fn jump_to_queue_index(&mut self, index: usize) -> Result<Option<String>, AudioError> {
        let path = self.queue.jump_to(index).map(str::to_string);
        if let Some(ref path) = path {
            self.play(path)?;
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_of(paths: &[&str]) -> Queue {
        let mut queue = Queue::default();
        queue.set_tracks(paths.iter().map(|p| p.to_string()).collect());
        queue
    }

    // ── set_tracks ───────────────────────────────────────────────────────

    #[test]
    fn set_tracks_resets_current_index_and_shuffle_state() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(2);
        queue.set_shuffle(true);
        assert!(queue.current_index().is_some());
        assert!(queue.is_shuffled());

        queue.set_tracks(vec!["x".to_string(), "y".to_string()]);
        assert_eq!(queue.current_index(), None);
        assert!(!queue.is_shuffled());
        assert_eq!(queue.shuffle_pos, 0);
        assert_eq!(queue.tracks(), &["x".to_string(), "y".to_string()]);
    }

    // ── jump ─────────────────────────────────────────────────────────────

    #[test]
    fn jump_to_valid_index_sets_current_and_returns_path() {
        let mut queue = queue_of(&["a", "b", "c"]);
        let result = queue.jump(1);
        assert_eq!(result, Some("b"));
        assert_eq!(queue.current_index(), Some(1));
    }

    #[test]
    fn jump_out_of_bounds_returns_none() {
        let mut queue = queue_of(&["a", "b", "c"]);
        assert_eq!(queue.jump(5), None);
    }

    // ── next ─────────────────────────────────────────────────────────────

    #[test]
    fn next_returns_none_past_last_track_when_repeat_off_or_one() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(2);
        assert_eq!(queue.next(&RepeatMode::Off), None);
        assert_eq!(queue.current_index(), Some(2));
        assert_eq!(queue.next(&RepeatMode::One), None);
        assert_eq!(queue.current_index(), Some(2));
    }

    #[test]
    fn next_wraps_to_first_track_under_repeat_all() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(2);
        let result = queue.next(&RepeatMode::All);
        assert_eq!(result, Some("a"));
        assert_eq!(queue.current_index(), Some(0));
    }

    #[test]
    fn next_wraps_to_shuffle_order_start_under_repeat_all() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(2);
        queue.set_shuffle(true);
        // Pinned: shuffle_order[0] must equal the index that was current at
        // shuffle time (2, "c").
        assert_eq!(queue.shuffle_order.as_ref().unwrap()[0], 2);

        let last_pos = queue.shuffle_order.as_ref().unwrap().len() - 1;
        queue.shuffle_pos = last_pos;
        queue.current_index = Some(queue.shuffle_order.as_ref().unwrap()[last_pos]);

        let result = queue.next(&RepeatMode::All);
        assert_eq!(result, Some("c"));
        assert_eq!(queue.current_index(), Some(2));
    }

    // ── previous ─────────────────────────────────────────────────────────

    #[test]
    fn previous_returns_none_before_first_track_when_repeat_off_or_one() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        assert_eq!(queue.previous(&RepeatMode::Off), None);
        // Unlike `next`, `previous` unconditionally writes
        // `current_index = prev_idx` (no early-return `?`), so it becomes
        // None here rather than staying at 0.
        assert_eq!(queue.current_index(), None);
        assert_eq!(queue.previous(&RepeatMode::One), None);
        assert_eq!(queue.current_index(), None);
    }

    #[test]
    fn previous_wraps_to_last_track_under_repeat_all() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        let result = queue.previous(&RepeatMode::All);
        assert_eq!(result, Some("c"));
        assert_eq!(queue.current_index(), Some(2));
    }

    // ── peek_next ────────────────────────────────────────────────────────

    #[test]
    fn peek_next_does_not_mutate_state() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        let first = queue.peek_next(&RepeatMode::Off);
        let index_after_first_peek = queue.current_index();
        let second = queue.peek_next(&RepeatMode::Off);
        assert_eq!(first, second);
        assert_eq!(index_after_first_peek, queue.current_index());
        assert_eq!(queue.current_index(), Some(0));
    }

    // ── set_shuffle / is_shuffled ────────────────────────────────────────

    #[test]
    fn set_shuffle_true_builds_valid_permutation_pinned_to_current() {
        let mut queue = queue_of(&["a", "b", "c", "d", "e"]);
        queue.jump(2);
        queue.set_shuffle(true);
        assert!(queue.is_shuffled());

        let order = queue.shuffle_order.clone().unwrap();
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
        // Current track is pinned to the front of the shuffle order.
        assert_eq!(order[0], 2);
    }

    #[test]
    fn set_shuffle_false_clears_order() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.set_shuffle(true);
        assert!(queue.is_shuffled());
        queue.set_shuffle(false);
        assert!(!queue.is_shuffled());
        assert!(queue.shuffle_order.is_none());
    }

    // ── enqueue ──────────────────────────────────────────────────────────

    #[test]
    fn enqueue_appends_to_tracks() {
        let mut queue = queue_of(&["a", "b"]);
        queue.enqueue("c".to_string());
        assert_eq!(queue.tracks(), &["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn enqueue_while_shuffled_appends_new_index_to_shuffle_order() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.set_shuffle(true);
        queue.enqueue("d".to_string());
        let order = queue.shuffle_order.clone().unwrap();
        assert_eq!(order.len(), 4);
        assert!(order.contains(&3));
    }

    // ── insert_next ──────────────────────────────────────────────────────

    #[test]
    fn insert_next_inserts_right_after_current() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        queue.insert_next("x".to_string());
        assert_eq!(
            queue.tracks(),
            &["a".to_string(), "x".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(queue.current_index(), Some(0));
    }

    #[test]
    fn insert_next_appends_when_nothing_current() {
        let mut queue = queue_of(&["a", "b"]);
        queue.insert_next("z".to_string());
        assert_eq!(
            queue.tracks(),
            &["a".to_string(), "b".to_string(), "z".to_string()]
        );
    }

    #[test]
    fn insert_next_while_shuffled_updates_shuffle_order() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        queue.set_shuffle(true);
        assert_eq!(queue.shuffle_order.as_ref().unwrap()[0], 0);

        queue.insert_next("x".to_string());

        let order = queue.shuffle_order.clone().unwrap();
        assert_eq!(order.len(), 4);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3]);
        // Pin at position 0 is untouched, and the newly inserted index (1)
        // lands right after the shuffle cursor.
        assert_eq!(order[0], 0);
        assert_eq!(order[1], 1);
    }

    // ── remove_at ────────────────────────────────────────────────────────

    #[test]
    fn remove_at_out_of_bounds_returns_none() {
        let mut queue = queue_of(&["a", "b", "c"]);
        assert_eq!(queue.remove_at(10), None);
    }

    #[test]
    fn remove_at_current_last_track_clamps_current_index() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(2);
        let removed = queue.remove_at(2);
        assert_eq!(removed, Some("c".to_string()));
        assert_eq!(queue.tracks(), &["a".to_string(), "b".to_string()]);
        assert_eq!(queue.current_index(), Some(1));
    }

    #[test]
    fn remove_at_current_non_last_track_keeps_index() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        let removed = queue.remove_at(0);
        assert_eq!(removed, Some("a".to_string()));
        assert_eq!(queue.tracks(), &["b".to_string(), "c".to_string()]);
        assert_eq!(queue.current_index(), Some(0));
    }

    #[test]
    fn remove_at_before_current_decrements_current_index() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(2);
        let removed = queue.remove_at(0);
        assert_eq!(removed, Some("a".to_string()));
        assert_eq!(queue.tracks(), &["b".to_string(), "c".to_string()]);
        assert_eq!(queue.current_index(), Some(1));
    }

    #[test]
    fn remove_at_after_current_leaves_current_index_unchanged() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        let removed = queue.remove_at(2);
        assert_eq!(removed, Some("c".to_string()));
        assert_eq!(queue.current_index(), Some(0));
    }

    #[test]
    fn remove_at_last_remaining_track_clears_current_index() {
        let mut queue = queue_of(&["only"]);
        queue.jump(0);
        let removed = queue.remove_at(0);
        assert_eq!(removed, Some("only".to_string()));
        assert!(queue.tracks().is_empty());
        assert_eq!(queue.current_index(), None);
    }

    // ── move_track ───────────────────────────────────────────────────────

    #[test]
    fn move_track_same_index_returns_false() {
        let mut queue = queue_of(&["a", "b", "c"]);
        assert!(!queue.move_track(1, 1));
    }

    #[test]
    fn move_track_out_of_bounds_returns_false() {
        let mut queue = queue_of(&["a", "b", "c"]);
        assert!(!queue.move_track(0, 10));
        assert!(!queue.move_track(10, 0));
    }

    #[test]
    fn move_track_from_before_current_to_after_keeps_same_track_current() {
        let mut queue = queue_of(&["a", "b", "c", "d"]);
        queue.jump(1); // "b"
        let moved = queue.move_track(0, 2);
        assert!(moved);
        assert_eq!(
            queue.tracks(),
            &["b".to_string(), "c".to_string(), "a".to_string(), "d".to_string()]
        );
        // current_index must still point at "b".
        let idx = queue.current_index().unwrap();
        assert_eq!(queue.tracks()[idx], "b");
    }

    #[test]
    fn move_track_from_after_current_to_before_keeps_same_track_current() {
        let mut queue = queue_of(&["a", "b", "c", "d"]);
        queue.jump(2); // "c"
        let moved = queue.move_track(3, 0);
        assert!(moved);
        assert_eq!(
            queue.tracks(),
            &["d".to_string(), "a".to_string(), "b".to_string(), "c".to_string()]
        );
        let idx = queue.current_index().unwrap();
        assert_eq!(queue.tracks()[idx], "c");
    }

    // ── clear_upcoming ───────────────────────────────────────────────────

    #[test]
    fn clear_upcoming_keeps_only_current_track() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(1);
        queue.set_shuffle(true);
        queue.clear_upcoming();
        assert_eq!(queue.tracks(), &["b".to_string()]);
        assert_eq!(queue.current_index(), Some(0));
        assert!(!queue.is_shuffled());
    }

    #[test]
    fn clear_upcoming_with_no_current_clears_all() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.clear_upcoming();
        assert!(queue.tracks().is_empty());
        assert_eq!(queue.current_index(), None);
    }

    // ── paths_from_current_forward ───────────────────────────────────────

    #[test]
    fn paths_from_current_forward_never_exceeds_track_count_under_repeat_all() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        let paths = queue.paths_from_current_forward(&RepeatMode::All);
        assert!(!paths.is_empty());
        assert!(paths.len() <= queue.tracks().len());
    }

    #[test]
    fn paths_from_current_forward_empty_when_nothing_current() {
        let queue = queue_of(&["a", "b", "c"]);
        let paths = queue.paths_from_current_forward(&RepeatMode::Off);
        assert!(paths.is_empty());
    }

    // ── PlaybackClock ────────────────────────────────────────────────────

    #[test]
    fn stopped_clock_is_zeroed() {
        let clock = PlaybackClock::stopped();
        assert!(clock.started_at.is_none());
        assert_eq!(clock.elapsed_before_start, Duration::ZERO);
        assert_eq!(clock.duration, None);
        assert_eq!(clock.raw_elapsed(), Duration::ZERO);
        assert_eq!(clock.position(), Duration::ZERO);
    }

    #[test]
    fn clock_elapsed_and_position_advance_after_start() {
        let mut clock = PlaybackClock::stopped();
        clock.started_at = Some(Instant::now());
        std::thread::sleep(Duration::from_millis(5));
        assert!(clock.raw_elapsed() >= Duration::from_millis(5));
        assert!(clock.position() >= Duration::from_millis(5));
    }

    #[test]
    fn clock_position_clamps_to_duration() {
        let mut clock = PlaybackClock::stopped();
        clock.duration = Some(Duration::from_millis(1));
        clock.started_at = Some(Instant::now());
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(clock.position(), Duration::from_millis(1));
        assert!(clock.raw_elapsed() > clock.position());
    }
}
