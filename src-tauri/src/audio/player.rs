// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::error::AudioError;
use crate::remote::stream::{self, reader::ProgressiveReader, PlayTarget};

use super::buffered_source::{BufferedSource, SharedBuffering, SharedStall};
use super::dsp::{
    load_shared_speed, set_shared_gain, set_shared_speed, shared_gain, shared_level, shared_speed,
    Crossfade, CrossfadeState, EqConfig, Equalizer, LevelMeter, NextTrack, NextTrackSlot,
    Revocable, SharedGain, SharedLevel, SharedSpeed, SoftFade, SoftFadeState, TimeStretch,
    VolumeGain, SOFT_FADE_SECS,
};
use super::normalization::{analyze_track_levels, VolumeNormalizer};
use super::sleep_timer::{SleepRequest, SleepStep, SleepTimer, SleepTimerStatus};
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
    /// Track seconds per wall second.
    speed: f32,
    /// The player's stall counter, in microseconds, when the clock started.
    stall_at_start: u64,
}

impl PlaybackClock {
    fn stopped() -> Self {
        Self {
            started_at: None,
            elapsed_before_start: Duration::ZERO,
            duration: None,
            speed: 1.0,
            stall_at_start: 0,
        }
    }

    /// Track time played so far, not capped at the duration. `stall_now` is
    /// the player's stall counter: silence played while a stream caught up
    /// is not track time. It is counted in track time already, upstream of
    /// the time stretch, so it is not scaled by the speed.
    fn raw_elapsed(&self, stall_now: u64) -> Duration {
        self.started_at
            .map(|started_at| {
                let stalled = Duration::from_micros(stall_now.saturating_sub(self.stall_at_start));
                self.elapsed_before_start
                    + started_at
                        .elapsed()
                        .mul_f32(self.speed)
                        .saturating_sub(stalled)
            })
            .unwrap_or(self.elapsed_before_start)
    }

    fn position(&self, stall_now: u64) -> Duration {
        let elapsed = self.raw_elapsed(stall_now);
        self.duration
            .map(|duration| elapsed.min(duration))
            .unwrap_or(elapsed)
    }

    /// Switch speed without moving the position.
    fn set_speed(&mut self, speed: f32, stall_now: u64) {
        self.elapsed_before_start = self.raw_elapsed(stall_now);
        if self.started_at.is_some() {
            self.started_at = Some(Instant::now());
        }
        self.stall_at_start = stall_now;
        self.speed = speed;
    }
}

#[cfg(target_os = "android")]
fn android_duration_of(path: &str) -> Option<Duration> {
    crate::android::audio::exo_duration_for(path)
        .ok()
        .flatten()
        .map(|ms| Duration::from_millis(ms as u64))
}

/// How long auto-advance waits for a remote next track to be able to start.
const ADVANCE_WAIT: Duration = Duration::from_secs(10);

/// Lets auto-advance go once `next` is ready, or once it has waited
/// [`ADVANCE_WAIT`] for it. `wait` holds the track waited on and since when.
fn advance_gate(
    wait: &mut Option<(String, Instant)>,
    next: &str,
    ready: bool,
    now: Instant,
) -> bool {
    if !ready {
        let since = match wait {
            Some((path, since)) if path == next => *since,
            _ => {
                *wait = Some((next.to_string(), now));
                now
            }
        };
        if now.duration_since(since) < ADVANCE_WAIT {
            return false;
        }
    }
    *wait = None;
    true
}

// ── Queue ─────────────────────────────────────────────────────────────────────

/// In-memory playback queue (separate from the library's persisted playlist).
///
/// Shuffle reorders `tracks` itself, so the queue always lists tracks in the
/// order they play. Turning shuffle off puts them back in their old order.
#[derive(Debug, Clone, Default)]
pub struct Queue {
    tracks: Vec<String>,
    current_index: Option<usize>,
    shuffle: bool,
    /// Each entry's slot in the unshuffled queue, kept while `tracks` is
    /// shuffled. `None` with shuffle on is a new queue that gets shuffled once
    /// the first track to play is picked.
    unshuffled_rank: Option<Vec<usize>>,
    /// Song coupling: pairs of paths kept back to back, the first always
    /// right before the second.
    couples: Vec<(String, String)>,
    /// A new queue nothing has played from yet. It is shuffled and its
    /// couples are put together once the first track is picked.
    fresh: bool,
}

impl Queue {
    /// Replace the queue. Shuffle stays on if it was on, and the new tracks
    /// are shuffled around whichever one starts playing.
    pub fn set_tracks(&mut self, tracks: Vec<String>) {
        self.tracks = tracks;
        self.current_index = None;
        self.unshuffled_rank = None;
        self.fresh = true;
    }

    /// Fisher-Yates over the whole queue. The current track moves to the
    /// front so every other track is still ahead of it.
    fn shuffle_tracks(&mut self) {
        let len = self.tracks.len();
        let mut order: Vec<usize> = (0..len).collect();
        // splitmix64. Each swap needs its own random draw; deriving every j
        // from one fixed seed reaches only a sliver of the possible orders.
        let mut state = uuid::Uuid::new_v4().as_u64_pair().0;
        let mut next_random = move || {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        for i in (1..len).rev() {
            let j = (next_random() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        if let Some(idx) = self.current_index {
            if let Some(pos) = order.iter().position(|&v| v == idx) {
                order.swap(0, pos);
            }
            self.current_index = Some(0);
        }
        let ranks = self
            .unshuffled_rank
            .take()
            .unwrap_or_else(|| (0..len).collect());
        self.unshuffled_rank = Some(order.iter().map(|&i| ranks[i]).collect());
        self.reorder(&order);
        self.keep_couples_together();
    }

    /// Replace the coupled pairs. The queue is rearranged right away so the
    /// new pairs already play back to back.
    pub fn set_couples(&mut self, couples: Vec<(String, String)>) {
        self.couples = couples;
        if !self.fresh {
            self.keep_couples_together();
        }
    }

    /// Move each couple's second song right after its first, among the tracks
    /// still to play. A pair the current track splits is left alone, since
    /// half of it has already played.
    fn keep_couples_together(&mut self) {
        for (first, second) in self.couples.clone() {
            let floor = self.current_index.unwrap_or(0);
            let Some(a) = self.tracks.iter().position(|p| *p == first) else {
                continue;
            };
            let Some(b) = self.tracks.iter().position(|p| *p == second) else {
                continue;
            };
            if a < floor || b < floor || Some(b) == self.current_index || b == a + 1 {
                continue;
            }
            // Taking `b` out from before `a` shifts `a` back one slot.
            let to = if b > a { a + 1 } else { a };
            self.move_track(b, to);
        }
    }

    /// Put the tracks back in the order they had before shuffling.
    fn unshuffle_tracks(&mut self) {
        let Some(ranks) = self.unshuffled_rank.take() else {
            return;
        };
        let mut order: Vec<usize> = (0..self.tracks.len()).collect();
        order.sort_by_key(|&i| ranks[i]);
        self.current_index = self
            .current_index
            .and_then(|current| order.iter().position(|&i| i == current));
        self.reorder(&order);
        self.keep_couples_together();
    }

    /// Rebuild `tracks` so slot `n` holds the entry that was at `order[n]`.
    fn reorder(&mut self, order: &[usize]) {
        let mut old = std::mem::take(&mut self.tracks);
        self.tracks = order.iter().map(|&i| std::mem::take(&mut old[i])).collect();
    }

    /// Get a new queue ready the first time a track is picked from it:
    /// shuffled if shuffle is on, with its couples back to back either way.
    fn settle_new_queue(&mut self) {
        if !std::mem::take(&mut self.fresh) {
            return;
        }
        if self.shuffle {
            self.shuffle_tracks();
        } else {
            self.keep_couples_together();
        }
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current_index
    }

    pub fn tracks(&self) -> &[String] {
        &self.tracks
    }

    /// Jump to a specific index and return its path. On a new shuffled queue
    /// this also shuffles it, with the picked track first.
    pub fn jump(&mut self, index: usize) -> Option<&str> {
        if index >= self.tracks.len() {
            return None;
        }
        self.current_index = Some(index);
        self.settle_new_queue();
        let current = self.current_index?;
        self.tracks.get(current).map(String::as_str)
    }

    /// Jump to a track the user picked. Picking the second song of a couple
    /// starts from its first, which is moved, or queued, right before it.
    pub fn pick(&mut self, index: usize) -> Option<&str> {
        let picked = self.tracks.get(index)?;
        let first = self
            .couples
            .iter()
            .find(|(_, second)| second == picked)
            .map(|(first, _)| first.clone());
        let index = match first {
            None => index,
            Some(first) => match self.tracks.iter().position(|p| *p == first) {
                Some(at) if at + 1 == index => at,
                // Taking the first out from before the second shifts it back.
                Some(at) => {
                    let to = if at < index { index - 1 } else { index };
                    self.move_track(at, to);
                    to
                }
                None => {
                    self.insert_at(index, first);
                    index
                }
            },
        };
        self.jump(index)
    }

    /// Put `path` at `index`, keeping it there in the unshuffled order too.
    fn insert_at(&mut self, index: usize, path: String) {
        self.tracks.insert(index, path);
        if let Some(current) = self.current_index.filter(|&current| current >= index) {
            self.current_index = Some(current + 1);
        }
        if let Some(ref mut ranks) = self.unshuffled_rank {
            let rank = ranks.get(index).copied().unwrap_or(ranks.len());
            for r in ranks.iter_mut() {
                if *r >= rank {
                    *r += 1;
                }
            }
            ranks.insert(index, rank);
        }
    }

    /// Index that `next` would select, without mutating queue state.
    fn peek_next_index(&self, repeat: &RepeatMode) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        match self.current_index {
            None => Some(0),
            Some(current) => {
                let next = current + 1;
                match repeat {
                    RepeatMode::All => Some(next % self.tracks.len()),
                    RepeatMode::Off | RepeatMode::One => (next < self.tracks.len()).then_some(next),
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
        self.settle_new_queue();
        let next_idx = self.peek_next_index(repeat)?;
        self.current_index = Some(next_idx);
        self.tracks.get(next_idx).map(String::as_str)
    }

    /// Paths from the current queue position through the end of playback order.
    // Only reached from the Android ExoPlayer gapless path; the unit tests
    // below cover it on every target.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn paths_from_current_forward(&self, repeat: &RepeatMode) -> Vec<String> {
        let Some(start) = self.current_index else {
            return Vec::new();
        };
        let mut paths = self.tracks[start..].to_vec();
        if *repeat == RepeatMode::All {
            paths.extend_from_slice(&self.tracks[..start]);
        }
        paths
    }

    /// Go back to the previous track.
    pub fn previous(&mut self, repeat: &RepeatMode) -> Option<&str> {
        let prev_idx = self.peek_previous_index(repeat)?;
        self.current_index = Some(prev_idx);
        self.tracks.get(prev_idx).map(String::as_str)
    }

    /// Index that `previous` would select, without mutating queue state.
    fn peek_previous_index(&self, repeat: &RepeatMode) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        let current = self.current_index.unwrap_or(0);
        // At the start of the queue there is nothing to go back to, and the
        // current track keeps playing, so its index has to stay put.
        match (current, repeat) {
            (0, RepeatMode::All) => Some(self.tracks.len() - 1),
            (0, _) => None,
            (idx, _) => Some(idx - 1),
        }
    }

    /// Peek at the previous track path without moving back.
    pub fn peek_previous(&self, repeat: &RepeatMode) -> Option<&str> {
        self.peek_previous_index(repeat)
            .and_then(|i| self.tracks.get(i).map(String::as_str))
    }

    /// Turning shuffle on reshuffles the queue around the current track;
    /// turning it off restores the order the queue had before.
    pub fn set_shuffle(&mut self, enabled: bool) {
        self.shuffle = enabled;
        if enabled {
            self.fresh = false;
            self.shuffle_tracks();
        } else {
            self.unshuffle_tracks();
        }
    }

    /// Turn shuffle on for a queue saved while shuffled, keeping its order.
    pub fn resume_shuffle(&mut self) {
        self.shuffle = true;
        self.unshuffled_rank = Some((0..self.tracks.len()).collect());
    }

    pub fn is_shuffled(&self) -> bool {
        self.shuffle
    }

    /// Append a track to the end of the queue.
    pub fn enqueue(&mut self, path: String) {
        self.tracks.push(path);
        if let Some(ref mut ranks) = self.unshuffled_rank {
            ranks.push(ranks.len());
        }
    }

    /// Insert a track so it plays immediately after the current track.
    /// If nothing is playing, it is appended.
    pub fn insert_next(&mut self, path: String) {
        let insert_at = match self.current_index {
            Some(idx) => (idx + 1).min(self.tracks.len()),
            None => self.tracks.len(),
        };
        self.tracks.insert(insert_at, path);
        if let Some(ref mut ranks) = self.unshuffled_rank {
            // Unshuffled, it still follows the track it was queued behind.
            let rank = match self.current_index {
                Some(idx) => ranks[idx] + 1,
                None => ranks.len(),
            };
            for r in ranks.iter_mut() {
                if *r >= rank {
                    *r += 1;
                }
            }
            ranks.insert(insert_at, rank);
        }
    }

    /// Remove the track at `index`, adjusting `current_index` accordingly.
    /// Returns the removed path.
    pub fn remove_at(&mut self, index: usize) -> Option<String> {
        if index >= self.tracks.len() {
            return None;
        }

        let removed = self.tracks.remove(index);

        if let Some(current) = self.current_index {
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

        if let Some(ref mut ranks) = self.unshuffled_rank {
            let rank = ranks.remove(index);
            for r in ranks.iter_mut() {
                if *r > rank {
                    *r -= 1;
                }
            }
        }

        Some(removed)
    }

    /// Move a track from `from` to `to` within the queue, keeping
    /// `current_index` consistent.
    pub fn move_track(&mut self, from: usize, to: usize) -> bool {
        let len = self.tracks.len();
        if from >= len || to >= len || from == to {
            return false;
        }

        let item = self.tracks.remove(from);
        self.tracks.insert(to, item);

        if let Some(ref mut ranks) = self.unshuffled_rank {
            let rank = ranks.remove(from);
            ranks.insert(to, rank);
        }

        if let Some(current) = self.current_index {
            let moved = if current == from {
                to
            } else if from < current && current <= to {
                current - 1
            } else if to <= current && current < from {
                current + 1
            } else {
                current
            };
            self.current_index = Some(moved);
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
        self.unshuffled_rank = self.shuffle.then(|| (0..self.tracks.len()).collect());
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

/// The crossfade wrapped around the playing track while crossfade is on.
/// ExoPlayer crossfades by itself on Android, so this stays unused there.
#[cfg_attr(target_os = "android", allow(dead_code))]
struct CrossfadeParts {
    /// Shared with the mixer, which reports the handoff and what comes next.
    state: Arc<Mutex<CrossfadeState>>,
    /// Sends the mixer a different track to fade into.
    slot: Arc<NextTrackSlot>,
    /// The track last sent through `slot`, so it is not reopened every tick
    /// while the mixer has yet to pick it up.
    requested: Option<Option<String>>,
}

/// The next queue track, appended to the sink behind the playing one.
struct Prefetched {
    path: String,
    duration: Option<Duration>,
    /// Drops it before it starts, once the queue no longer plays it next.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    revoked: Arc<AtomicBool>,
}

/// A decoded, fully-wrapped playback source: the boxed sample stream, the
/// track duration when the decoder could report one, and its crossfade
/// (present only while crossfade is on).
type BuiltSource = (
    Box<dyn Source<Item = f32> + Send + 'static>,
    Option<Duration>,
    Option<CrossfadeParts>,
);

pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 2.0;

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
    /// The crossfade around the playing track, for track switching.
    crossfade: Option<CrossfadeParts>,
    /// Soft gain ramp for play/pause/seek/stop (shared with the active SoftFade).
    soft_fade: Arc<Mutex<SoftFadeState>>,
    /// Next queue path already appended to the sink so playback can continue
    /// when the current source ends. Critical on Android while backgrounded,
    /// where a delayed tick alone can miss the transition.
    prefetched_next: Option<Prefetched>,
    /// Seamless queue transitions when crossfade is off.
    gapless_enabled: bool,
    /// Boost quieter tracks toward the session median peak (off by default).
    volume_normalization_enabled: bool,
    /// Shared separately from the rest of `AudioPlayer` so a background peak
    /// analysis thread can update it without holding the player-wide lock for
    /// the duration of a full-file decode.
    normalizer: Arc<Mutex<VolumeNormalizer>>,
    sleep: SleepTimer,
    /// Fraction of `volume` actually sent to the output while the sleep
    /// timer fades out. 1.0 the rest of the time.
    sleep_level: f32,
    /// Set while the sleep timer waits for the current track to end. Nothing
    /// after it is preloaded (no sink prefetch, crossfade or ExoPlayer
    /// playlist), so the track can end without the next one starting.
    stop_after_current: bool,
    /// Playback speed, shared with the time stretch in every source.
    speed: SharedSpeed,
    /// Loudness of what is playing, measured at the end of the chain for the
    /// live waveform.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    level: SharedLevel,
    /// Silence played while a stream waited for the network, shared with
    /// every buffered source, so the clock can leave it out.
    stall: SharedStall,
    /// Set while a stream plays silence waiting for the network.
    buffering: SharedBuffering,
    /// When `tick_remote_prefetch` last looked, so it looks once a second.
    prefetch_checked_at: Option<Instant>,
    /// The remote next track last asked to warm, so it is asked once.
    warmed: Option<String>,
    /// The remote next track auto-advance is waiting on, and since when.
    advance_wait: Option<(String, Instant)>,
    #[cfg(test)]
    prefetch_checks: usize,
    #[cfg(test)]
    warm_requests: usize,
    /// Bumped on every track change; a background analysis result that
    /// arrives for a stale generation is dropped instead of being applied to
    /// whatever now-current track it no longer matches. Only Android defers
    /// analysis to a background thread, so this is unused elsewhere.
    #[cfg(target_os = "android")]
    normalization_generation: Arc<AtomicU64>,
    /// Active ExoPlayer playlist URIs (Android gapless mode).
    #[cfg(target_os = "android")]
    android_gapless_playlist: Vec<String>,
    /// ExoPlayer is holding output for a transient audio focus loss and the
    /// transport clock is frozen until it lets go (see `set_output_suspended`).
    #[cfg(target_os = "android")]
    output_suspended: bool,
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
            crossfade: None,
            soft_fade: Arc::new(Mutex::new(SoftFadeState::default())),
            prefetched_next: None,
            gapless_enabled: true,
            volume_normalization_enabled: false,
            normalizer: Arc::new(Mutex::new(VolumeNormalizer::new())),
            sleep: SleepTimer::default(),
            sleep_level: 1.0,
            stop_after_current: false,
            speed: shared_speed(1.0),
            level: shared_level(),
            stall: Arc::new(AtomicU64::new(0)),
            buffering: Arc::new(AtomicBool::new(false)),
            prefetch_checked_at: None,
            warmed: None,
            advance_wait: None,
            #[cfg(test)]
            prefetch_checks: 0,
            #[cfg(test)]
            warm_requests: 0,
            #[cfg(target_os = "android")]
            normalization_generation: Arc::new(AtomicU64::new(0)),
            #[cfg(target_os = "android")]
            android_gapless_playlist: Vec::new(),
            #[cfg(target_os = "android")]
            output_suspended: false,
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
            return crate::android::audio::ensure_initialized().map_err(AudioError::StreamCreation);
        }

        #[cfg(not(target_os = "android"))]
        {
            if self.output.is_some() {
                return Ok(());
            }

            crate::android::jni::ensure_jni_thread_attached();
            if !crate::android::jni::android_audio_ready() {
                return Err(AudioError::StreamCreation(
                    "Android audio context is not ready yet, try playing again in a moment"
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
                    "Android audio context is not ready yet, try again in a moment".to_string(),
                ));
            }

            let host = cpal::default_host();
            let device = host
                .output_devices()
                .map_err(|e| AudioError::DeviceUnavailable(e.to_string()))?
                .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
                .ok_or_else(|| {
                    AudioError::DeviceUnavailable(format!("Device not found: {device_name}"))
                })?;

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
                crossfade: None,
                soft_fade: Arc::new(Mutex::new(SoftFadeState::default())),
                prefetched_next: None,
                gapless_enabled: true,
                volume_normalization_enabled: false,
                normalizer: Arc::new(Mutex::new(VolumeNormalizer::new())),
                sleep: SleepTimer::default(),
                sleep_level: 1.0,
                stop_after_current: false,
                speed: shared_speed(1.0),
                level: shared_level(),
                stall: Arc::new(AtomicU64::new(0)),
                buffering: Arc::new(AtomicBool::new(false)),
                prefetch_checked_at: None,
                warmed: None,
                advance_wait: None,
                #[cfg(test)]
                prefetch_checks: 0,
                #[cfg(test)]
                warm_requests: 0,
                #[cfg(target_os = "android")]
                normalization_generation: Arc::new(AtomicU64::new(0)),
                #[cfg(target_os = "android")]
                android_gapless_playlist: Vec::new(),
                #[cfg(target_os = "android")]
                output_suspended: false,
            })
        }
    }

    /// Gain cell for the desktop (rodio) playback path.
    ///
    /// A full-file peak scan can take a while, and unlike Android there's no
    /// separate "set gain on the running player" call to defer to; gain is
    /// baked into the source chain via [`VolumeGain`]. So instead of
    /// resolving a final `f32` synchronously (which used to block `play()`
    /// under the player lock for the length of the decode, the same
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
                .cached_levels(path)
                .map(|levels| normalizer.register_levels(path, levels))
        };
        if let Some(gain) = cached_gain {
            return shared_gain(gain);
        }
        let cell = shared_gain(1.0);
        self.spawn_desktop_peak_analysis(path.to_string(), cell.clone(), false);
        cell
    }

    /// Counts already-cached levels toward the session median, for a track
    /// that was only ever *peeked* (gapless sink-prefetch) and has now
    /// actually become the playing track via [`Self::adopt_prefetched`].
    ///
    /// The prefetch's gain cell is already wired into the running source, so
    /// this doesn't need to build or return a new one. It only needs the
    /// `register_levels` side effect (median contribution), which a peek
    /// never triggers on its own. If analysis hasn't finished caching levels
    /// yet, this is a no-op; the track just doesn't contribute this time.
    fn register_now_playing_for_normalization(&self, path: &str) {
        if !self.volume_normalization_enabled {
            return;
        }
        let mut normalizer = self.normalizer.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(levels) = normalizer.cached_levels(path) {
            normalizer.register_levels(path, levels);
        }
    }

    /// Same as [`Self::normalization_gain_cell_for_path`] but for a
    /// peeked/upcoming track (crossfade or gapless prefetch) that may never
    /// actually play. Doesn't count toward the running session median.
    fn peek_normalization_gain_cell_for_path(&self, path: &str) -> SharedGain {
        if !self.volume_normalization_enabled {
            return shared_gain(1.0);
        }
        let cached_gain = {
            let normalizer = self.normalizer.lock().unwrap_or_else(|e| e.into_inner());
            normalizer.cached_levels(path).map(|levels| {
                let median = normalizer.median_rms().unwrap_or(levels.rms);
                VolumeNormalizer::compute_gain(levels.rms, median, levels.peak)
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
        if !normalizer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_begin_analysis()
        {
            // Already at the concurrent-scan cap; leave this cell at neutral
            // gain. It'll be retried (and likely cached by then) next time
            // this path is loaded.
            return;
        }
        std::thread::spawn(move || {
            // A remote URI is not a file the decoder can open. Analyze the
            // finished cache file, or keep the neutral gain until there is one,
            // rather than caching made-up levels for the track.
            let source = if crate::remote::uri::is_remote(&path) {
                match stream::cache_file_if_complete(&path) {
                    Some(file) => file,
                    None => {
                        normalizer
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .end_analysis();
                        return;
                    }
                }
            } else {
                path.clone()
            };
            let levels = analyze_track_levels(&source).unwrap_or_else(|error| {
                tracing::warn!("Level analysis failed for \"{path}\": {error}");
                super::normalization::AudioLevels {
                    peak: 0.5,
                    rms: 0.5,
                }
            });
            let gain = {
                let mut normalizer = normalizer.lock().unwrap_or_else(|e| e.into_inner());
                let gain = if incoming {
                    normalizer.cache_levels(&path, levels);
                    let median = normalizer.median_rms().unwrap_or(levels.rms);
                    VolumeNormalizer::compute_gain(levels.rms, median, levels.peak)
                } else {
                    normalizer.register_levels(&path, levels)
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
                .cached_levels(path)
                .map(|levels| normalizer.register_levels(path, levels))
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
        if !normalizer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_begin_analysis()
        {
            // Already at the concurrent-scan cap (e.g. the user is skipping
            // through tracks faster than analysis finishes). Leave gain at
            // neutral for now; the next play/sync of this path retries.
            return;
        }
        let generation = self.normalization_generation.clone();
        std::thread::spawn(move || {
            // A remote track is analyzed from its finished cache file. Until
            // there is one it keeps the neutral gain it started with.
            let source = if crate::remote::uri::is_remote(&path) {
                match stream::cache_file_if_complete(&path) {
                    Some(file) => file,
                    None => {
                        normalizer
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .end_analysis();
                        return;
                    }
                }
            } else {
                path.clone()
            };
            let levels = analyze_track_levels(&source).unwrap_or_else(|error| {
                tracing::warn!("Level analysis failed for \"{path}\": {error}");
                super::normalization::AudioLevels {
                    peak: 0.5,
                    rms: 0.5,
                }
            });
            let gain = {
                let mut normalizer = normalizer.lock().unwrap_or_else(|e| e.into_inner());
                let gain = if incoming {
                    normalizer.cache_levels(&path, levels);
                    let median = normalizer.median_rms().unwrap_or(levels.rms);
                    VolumeNormalizer::compute_gain(levels.rms, median, levels.peak)
                } else {
                    normalizer.register_levels(&path, levels)
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
        let Some(next) = self.upcoming_for_preload() else {
            let _ = crate::android::audio::exo_set_incoming_normalization_gain(1.0);
            return;
        };
        let cached_gain = {
            let normalizer = self.normalizer.lock().unwrap_or_else(|e| e.into_inner());
            normalizer.cached_levels(&next).map(|levels| {
                let median = normalizer.median_rms().unwrap_or(levels.rms);
                VolumeNormalizer::compute_gain(levels.rms, median, levels.peak)
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
            let listed =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || match cpal::default_host().output_devices() {
                        Ok(devices) => devices
                            .filter_map(|d| d.name().ok())
                            .filter(|n| !n.is_empty())
                            .collect(),
                        Err(_) => vec![],
                    },
                ));
            listed.unwrap_or_default()
        }
    }

    /// The decoder for `path`. A remote track still downloading is decoded
    /// on a worker thread, so waiting on the network never blocks output.
    fn open_decoder(
        path: &str,
        stall: &SharedStall,
        buffering: &SharedBuffering,
    ) -> Result<Box<dyn Source<Item = i16> + Send>, AudioError> {
        if !crate::remote::uri::is_remote(path) {
            return Ok(Box::new(SymphoniaSource::new(path)?));
        }
        match stream::resolve_play_target(path).map_err(AudioError::FileOpen)? {
            PlayTarget::File(file) => Ok(Box::new(SymphoniaSource::new(&file)?)),
            PlayTarget::Progressive(fill, extension) => {
                let reader = ProgressiveReader::new(fill, Duration::from_secs(10))
                    .map_err(|e| AudioError::FileOpen(format!("Cannot open \"{path}\": {e}")))?;
                let source =
                    SymphoniaSource::from_media_source(Box::new(reader), Some(&extension), path)?;
                Ok(Box::new(BufferedSource::spawn(
                    source,
                    stall.clone(),
                    buffering.clone(),
                )))
            }
        }
    }

    // Many parameters, but they are the full set of knobs the decode chain
    // needs (gain, EQ, crossfade, prefetch) and are always passed together.
    #[allow(clippy::too_many_arguments)]
    fn build_source(
        path: &str,
        track_gain: SharedGain,
        eq_config: Arc<Mutex<EqConfig>>,
        eq_version: Arc<Mutex<u64>>,
        crossfade_duration: f32,
        next_path: Option<&str>,
        next_gain: SharedGain,
        soft_fade: Arc<Mutex<SoftFadeState>>,
        speed: SharedSpeed,
        level: SharedLevel,
        stall: &SharedStall,
        buffering: &SharedBuffering,
    ) -> Result<BuiltSource, AudioError> {
        let source = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Self::open_decoder(path, stall, buffering)
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

        // While crossfade is on every track is wrapped, even with nothing to
        // fade into yet, so a track queued later can still be faded into.
        let (chain, crossfade): (Box<dyn Source<Item = f32> + Send>, _) =
            if crossfade_duration > 0.0 {
                let next = next_path.and_then(|next_path| {
                    Self::open_crossfade_next(
                        next_path,
                        next_gain,
                        eq_config_for_next,
                        eq_version_for_next,
                        stall,
                        buffering,
                    )
                });
                let opened = next.as_ref().map(|next| next.path.clone());
                let (crossfade, state) = Crossfade::new(
                    Box::new(eq),
                    next.map(|next| next.source),
                    crossfade_duration,
                    Some(path.to_string()),
                    opened,
                );
                let parts = CrossfadeParts {
                    state,
                    slot: crossfade.next_slot(),
                    requested: None,
                };
                (Box::new(crossfade), Some(parts))
            } else {
                (Box::new(eq), None)
            };

        let stretched = TimeStretch::new(chain, speed);
        let faded = SoftFade::new(Box::new(stretched), soft_fade);
        let metered = LevelMeter::new(Box::new(faded), level);
        Ok((Box::new(metered), duration, crossfade))
    }

    /// Open the track a crossfade fades into, with its own gain and the
    /// shared EQ. A file that will not open just leaves nothing to fade into.
    fn open_crossfade_next(
        path: &str,
        gain: SharedGain,
        eq_config: Arc<Mutex<EqConfig>>,
        eq_version: Arc<Mutex<u64>>,
        stall: &SharedStall,
        buffering: &SharedBuffering,
    ) -> Option<NextTrack> {
        let source = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Self::open_decoder(path, stall, buffering)
        })) {
            Ok(Ok(source)) => source,
            Ok(Err(error)) => {
                tracing::warn!("Crossfade preload failed for \"{path}\": {error}");
                return None;
            }
            Err(_) => {
                tracing::warn!("Crossfade preload panicked for \"{path}\"");
                return None;
            }
        };
        let gained = VolumeGain::new(source.convert_samples(), gain);
        Some(NextTrack {
            source: Box::new(Equalizer::new(gained, eq_config, eq_version)),
            path: path.to_string(),
        })
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
        self.advance_wait = None;
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

            let next_path = self.upcoming_for_preload();
            let track_gain = self.normalization_gain_cell_for_path(path);
            let next_gain = next_path
                .as_deref()
                .map(|next| self.peek_normalization_gain_cell_for_path(next))
                .unwrap_or_else(|| shared_gain(1.0));
            let (source, duration, crossfade) = Self::build_source(
                path,
                track_gain,
                self.eq_config.clone(),
                self.eq_version.clone(),
                self.crossfade_duration,
                next_path.as_deref(),
                next_gain,
                self.soft_fade.clone(),
                self.speed.clone(),
                self.level.clone(),
                &self.stall,
                &self.buffering,
            )?;

            let handle = &self.output.as_ref().expect("output ensured").handle;

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

            sink.set_volume(self.output_volume());
            sink.append(source);
            sink.play();

            self.sink = Some(sink);
            self.current_path = Some(PathBuf::from(path));
            self.clock = PlaybackClock {
                started_at: Some(Instant::now()),
                elapsed_before_start: Duration::ZERO,
                duration,
                speed: self.speed(),
                stall_at_start: self.stall_now(),
            };

            self.crossfade = crossfade;
            self.prefetch_next_into_sink();

            Ok(())
        }
    }

    #[cfg(target_os = "android")]
    fn play_via_exo(&mut self, path: &str) -> Result<(), AudioError> {
        self.ensure_output()?;
        // Repeat-One must not hand ExoPlayer a multi-item playlist: gapless
        // transitions happen entirely inside ExoPlayer's own timeline, so it
        // would seamlessly auto-advance to the next queued track the moment
        // this one ends, before `play_next()`'s repeat-one branch (which
        // replays the current track) ever gets a chance to run. Desktop's
        // `prefetch_next_into_sink` already refuses to prefetch under
        // repeat-one for the same reason; this mirrors that.
        let use_gapless = self.gapless_enabled
            && self.crossfade_duration <= 0.0
            && self.repeat != RepeatMode::One
            && !self.stop_after_current;
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
        let _ = crate::android::audio::exo_set_volume(self.output_volume());
        self.sync_android_eq();

        // Only this track's own length: the cache can still hold the last
        // one's, and the clock would stop at it.
        let duration = android_duration_of(path);

        self.sink = None;
        self.prefetched_next = None;
        self.crossfade = None;
        self.current_path = Some(PathBuf::from(path));
        self.clock = PlaybackClock {
            started_at: Some(Instant::now()),
            elapsed_before_start: Duration::ZERO,
            duration,
            speed: self.speed(),
            stall_at_start: self.stall_now(),
        };
        self.sync_android_playback_extras();
        Ok(())
    }

    #[cfg(target_os = "android")]
    fn sync_android_playback_extras(&self) {
        let _ = crate::android::audio::exo_set_crossfade_duration(self.crossfade_duration);
        let _ = crate::android::audio::exo_set_gapless_enabled(self.gapless_enabled);
        self.sync_android_upcoming();
    }

    /// Tell ExoPlayer which track to crossfade into next.
    #[cfg(target_os = "android")]
    fn sync_android_upcoming(&self) {
        self.sync_android_crossfade_normalization();
        let next = if self.crossfade_duration > 0.0 {
            self.upcoming_for_preload()
        } else {
            None
        };
        let _ = crate::android::audio::exo_set_upcoming_uri(next.as_deref());
    }

    /// ExoPlayer holds the gapless playlist it was given at play time. Hand it
    /// the queue's current upcoming order so shuffle toggles and queue edits
    /// change what plays next.
    #[cfg(target_os = "android")]
    fn resync_android_gapless(&mut self) {
        if self.android_gapless_playlist.len() <= 1 {
            return;
        }
        // Catch up on an auto-advance first so the queue is on the track
        // ExoPlayer is actually playing.
        self.check_android_gapless_handoff();
        let mut upcoming = self.queue.paths_from_current_forward(&self.repeat);
        if upcoming.is_empty() {
            return;
        }
        upcoming.remove(0);
        // Repeat one plays the current track again, so nothing may follow it
        // inside ExoPlayer's own timeline.
        if self.upcoming_for_preload().is_none() {
            upcoming.clear();
        }
        match crate::android::audio::exo_replace_upcoming_media_items(&upcoming) {
            Ok(Some(current)) => {
                self.android_gapless_playlist.truncate(current + 1);
                self.android_gapless_playlist.extend(upcoming);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("Could not update the gapless playlist: {e}"),
        }
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
        // Only this track's own length: the cache can still hold the last
        // one's, and the clock would stop at it.
        let duration = android_duration_of(&path);
        // ExoPlayer auto-advanced within a playlist built entirely up front
        // (see `play_via_exo`), which only ever set gain for the first item.
        // Without this, every later track in a gapless run keeps whatever
        // gain the first track got, right through crossfade/skip handling.
        self.apply_android_normalization_for_path(&path);
        self.current_path = Some(new_path_buf);
        self.clock = PlaybackClock {
            started_at: Some(Instant::now()),
            elapsed_before_start: Duration::ZERO,
            duration,
            speed: self.speed(),
            stall_at_start: self.stall_now(),
        };
        true
    }

    /// ExoPlayer's duration of the current track, once it knows it.
    #[cfg(target_os = "android")]
    fn android_exo_duration(&self) -> Option<i64> {
        let path = self.current_path.as_deref()?.to_str()?;
        crate::android::audio::exo_duration_for(path).ok().flatten()
    }

    /// Move the transport clock to where ExoPlayer is. The clock still says
    /// whether Wave counts as playing and feeds the media session, listening
    /// stats and the saved position, so it must not drift from what is heard.
    #[cfg(target_os = "android")]
    fn sync_android_clock_to_exo(&mut self) {
        let Some(path) = self
            .current_path
            .as_deref()
            .and_then(|p| p.to_str())
            .map(str::to_string)
        else {
            return;
        };
        if let Ok(Some(ms)) = crate::android::audio::exo_position_for(&path) {
            self.clock.elapsed_before_start = Duration::from_millis(ms as u64);
            if self.clock.started_at.is_some() {
                self.clock.started_at = Some(Instant::now());
                self.clock.stall_at_start = self.stall_now();
            }
        }
        if let Some(duration) = android_duration_of(&path) {
            self.clock.duration = Some(duration);
        }
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

        // Only this track's own length: the cache can still hold the last
        // one's, and the clock would stop at it.
        let duration = android_duration_of(&next_path);

        self.current_path = Some(new_path_buf);
        self.clock = PlaybackClock {
            started_at: Some(Instant::now()),
            elapsed_before_start: Duration::ZERO,
            duration,
            speed: self.speed(),
            stall_at_start: self.stall_now(),
        };
        self.apply_android_normalization_for_path(&next_path);
        self.sync_android_playback_extras();
        true
    }

    /// Append the upcoming queue track to the active sink (best-effort).
    fn prefetch_next_into_sink(&mut self) {
        if !self.gapless_enabled || self.prefetched_next.is_some() {
            return;
        }
        if self.repeat == RepeatMode::One {
            return;
        }
        // Skip sink prefetch when crossfade is enabled. The current source
        // already contains the next track internally for the transition.
        if self.crossfade_duration > 0.0 {
            return;
        }
        let Some(next_path) = self.upcoming_for_preload() else {
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
            self.speed.clone(),
            self.level.clone(),
            &self.stall,
            &self.buffering,
        ) else {
            return;
        };
        if let Some(sink) = self.sink.as_ref() {
            let (source, revoked) = Revocable::new(source);
            sink.append(source);
            self.prefetched_next = Some(Prefetched {
                path: next_path,
                duration,
                revoked,
            });
        }
    }

    /// Line the preloaded next track back up with the queue. Gapless and
    /// crossfade both preload the following track as soon as one starts, so
    /// after a queue edit, shuffle or repeat change it has to be swapped out,
    /// or it plays anyway and the queue jumps to wherever it sits.
    #[cfg(not(target_os = "android"))]
    fn refresh_preload(&mut self) {
        let wanted = self.upcoming_for_preload();

        if let Some(crossfade) = self.crossfade.as_mut() {
            let (queued, fading) = {
                let state = crossfade.state.lock().unwrap_or_else(|e| e.into_inner());
                (state.next_path.clone(), state.fading)
            };
            if fading {
                // The next track is already fading in. Anything sent before
                // the fade began was dropped, so send it again afterwards.
                crossfade.requested = None;
                return;
            }
            if queued == wanted || crossfade.requested.as_ref() == Some(&wanted) {
                return;
            }
            let next = wanted.as_deref().and_then(|path| {
                let gain = self.peek_normalization_gain_cell_for_path(path);
                Self::open_crossfade_next(
                    path,
                    gain,
                    self.eq_config.clone(),
                    self.eq_version.clone(),
                    &self.stall,
                    &self.buffering,
                )
            });
            if let Some(crossfade) = self.crossfade.as_mut() {
                crossfade.slot.replace(next);
                crossfade.requested = Some(wanted);
            }
            return;
        }

        let Some(sink) = self.sink.as_ref() else {
            return;
        };
        if self.prefetched_next.as_ref().map(|next| &next.path) == wanted.as_ref() {
            return;
        }
        match self.prefetched_next.take() {
            // Nothing else is left in the sink, so the prefetched track is
            // the one playing and the next tick adopts it.
            Some(stale) if sink.len() <= 1 => self.prefetched_next = Some(stale),
            Some(stale) => {
                stale.revoked.store(true, Ordering::Relaxed);
                self.prefetch_next_into_sink();
            }
            None if !sink.empty() => self.prefetch_next_into_sink(),
            None => {}
        }
    }

    /// The queue or repeat mode changed: have what plays next follow it.
    fn sync_preload(&mut self) {
        #[cfg(target_os = "android")]
        {
            self.resync_android_gapless();
            self.sync_android_upcoming();
        }
        #[cfg(not(target_os = "android"))]
        self.refresh_preload();
    }

    /// Adopt a track that is already playing via sink prefetch (no restart).
    fn adopt_prefetched(&mut self, path: &str, duration: Option<Duration>) {
        // This track was only ever gain-*peeked* during prefetch (its gain
        // cell is already live in the sink), which deliberately doesn't
        // count toward the session median. Now that it's actually the
        // playing track, let it contribute, otherwise gapless playback
        // (the common case with normalization + prefetch both on) never
        // advances the median past whatever `play()` was first called with.
        self.register_now_playing_for_normalization(path);
        self.current_path = Some(PathBuf::from(path));
        // If the prefetched source has already been audible for a bit (tick
        // latency), keep the clock near zero. We can't know the exact sink
        // offset cheaply, and restarting would be worse.
        self.clock = PlaybackClock {
            started_at: Some(Instant::now()),
            elapsed_before_start: Duration::ZERO,
            duration,
            speed: self.speed(),
            stall_at_start: self.stall_now(),
        };
        self.prefetched_next = None;
        self.crossfade = None;
        self.prefetch_next_into_sink();
    }

    /// Load a track at `position_secs` without starting playback (session restore).
    pub fn load_paused_at(&mut self, path: &str, position_secs: f64) -> Result<(), AudioError> {
        // `position_secs` is deserialized from the persisted settings JSON, so
        // a hand-edited or corrupted file could contain a huge or infinite
        // value, which `Duration::from_secs_f64` below panics on outright.
        // Same cap as `seek()`.
        const MAX_SEEK_SECONDS: f64 = 1e9;
        // Not `clamp`: `max(0.0)` folds NaN to 0.0, while `f64::clamp`
        // propagates NaN straight into the panicking `from_secs_f64`.
        #[allow(clippy::manual_clamp)]
        let position_secs = position_secs.max(0.0).min(MAX_SEEK_SECONDS);
        #[cfg(target_os = "android")]
        {
            return self.load_paused_at_exo(path, position_secs);
        }

        #[cfg(not(target_os = "android"))]
        {
            self.load_paused_at_desktop(path, position_secs)
        }
    }

    #[cfg(target_os = "android")]
    fn load_paused_at_exo(&mut self, path: &str, position_secs: f64) -> Result<(), AudioError> {
        self.ensure_output()?;
        let position_ms = (position_secs.max(0.0) * 1000.0) as i64;
        crate::android::audio::exo_prepare_uri_at(path, position_ms).map_err(AudioError::Decode)?;
        let _ = crate::android::audio::exo_set_volume(self.output_volume());
        self.sync_android_eq();

        // Only this track's own length: the cache can still hold the last
        // one's, and the clock would stop at it.
        let duration = android_duration_of(path);

        self.sink = None;
        self.prefetched_next = None;
        self.crossfade = None;
        self.current_path = Some(PathBuf::from(path));
        self.clock = PlaybackClock {
            started_at: None,
            elapsed_before_start: Duration::from_secs_f64(position_secs.max(0.0)),
            duration,
            speed: self.speed(),
            stall_at_start: self.stall_now(),
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
        self.crossfade = None;
        self.set_soft_fade_target(1.0);

        let offset = Duration::from_secs_f64(position_secs.max(0.0));
        let next_path = self.upcoming_for_preload();
        let track_gain = self.normalization_gain_cell_for_path(path);
        let next_gain = next_path
            .as_deref()
            .map(|next| self.peek_normalization_gain_cell_for_path(next))
            .unwrap_or_else(|| shared_gain(1.0));
        let (source, duration, crossfade) = Self::build_source(
            path,
            track_gain,
            self.eq_config.clone(),
            self.eq_version.clone(),
            self.crossfade_duration,
            next_path.as_deref(),
            next_gain,
            self.soft_fade.clone(),
            self.speed.clone(),
            self.level.clone(),
            &self.stall,
            &self.buffering,
        )?;

        let handle = &self.output.as_ref().expect("output ensured").handle;

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

        sink.set_volume(self.output_volume());
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
            speed: self.speed(),
            stall_at_start: self.stall_now(),
        };
        self.crossfade = crossfade;
        self.prefetch_next_into_sink();
        Ok(())
    }

    /// True when the sink still has the prefetched follow-up buffered or playing.
    fn has_sink_prefetch(&self) -> bool {
        self.prefetched_next.is_some() || self.sink.as_ref().is_some_and(|sink| sink.len() > 1)
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
            self.output_suspended = false;
            return Ok(());
        }

        #[cfg(not(target_os = "android"))]
        match &self.sink {
            Some(sink) => {
                if !sink.is_paused() {
                    self.fade_out_blocking();
                }
                sink.pause();
                self.clock.elapsed_before_start = self.clock.position(self.stall_now());
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
            self.start_clock();
            self.output_suspended = false;
            return Ok(());
        }

        #[cfg(not(target_os = "android"))]
        match &self.sink {
            Some(sink) => {
                self.set_soft_fade_target(0.0);
                sink.play();
                self.set_soft_fade_target(1.0);
                self.start_clock();
                Ok(())
            }
            None => Err(AudioError::NoTrackLoaded),
        }
    }

    /// Set the clock running from where it stopped. Resuming what already
    /// plays leaves it alone: restarting it there would throw away the time
    /// played since the last start.
    fn start_clock(&mut self) {
        if self.clock.started_at.is_none() {
            self.clock.started_at = Some(Instant::now());
            self.clock.stall_at_start = self.stall_now();
        }
    }

    /// ExoPlayer holds output during a transient audio focus loss (a call, a
    /// navigation prompt) and resumes by itself afterwards. Freeze the
    /// transport clock meanwhile so the position stays true and the media
    /// session shows the track as paused.
    #[cfg(target_os = "android")]
    pub fn set_output_suspended(&mut self, suspended: bool) {
        if suspended {
            if self.clock.started_at.is_some() {
                self.clock.elapsed_before_start = self.clock.position(self.stall_now());
                self.clock.started_at = None;
                self.output_suspended = true;
            }
        } else if std::mem::take(&mut self.output_suspended)
            && self.current_path.is_some()
            && self.clock.started_at.is_none()
        {
            self.clock.started_at = Some(Instant::now());
            self.clock.stall_at_start = self.stall_now();
        }
    }

    /// True while output is held for a transient focus loss.
    #[cfg(target_os = "android")]
    pub fn is_output_suspended(&self) -> bool {
        self.output_suspended && self.current_path.is_some()
    }

    pub fn stop(&mut self) -> Result<(), AudioError> {
        #[cfg(target_os = "android")]
        {
            let _ = crate::android::audio::exo_stop();
            self.sink = None;
            self.current_path = None;
            self.prefetched_next = None;
            self.crossfade = None;
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
            self.crossfade = None;
            self.set_soft_fade_target(1.0);
            self.clock = PlaybackClock::stopped();
            Ok(())
        }
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), AudioError> {
        // `seconds` comes straight from a Tauri IPC argument. NaN/negative
        // collapse to 0 via `.max`; the `.min` cap keeps huge or infinite
        // values (a malformed payload, or a frontend div-by-zero producing
        // Infinity) from reaching `Duration::from_secs_f64` below, which
        // panics outright on non-finite or overflowing input.
        const MAX_SEEK_SECONDS: f64 = 1e9;
        // Not `clamp`: see `load_paused_at`, `clamp` would propagate NaN.
        #[allow(clippy::manual_clamp)]
        let seconds = seconds.max(0.0).min(MAX_SEEK_SECONDS);
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
            self.clock.stall_at_start = self.stall_now();
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
            // A paused track is rebuilt paused: starting it to pause it again
            // lets a moment of audio out.
            if self.has_sink_prefetch() {
                if !was_playing {
                    return self.load_paused_at(&path, seconds);
                }
                self.play(&path)?;
                if let Some(sink) = self.sink.as_ref() {
                    sink.try_seek(offset)
                        .map_err(|error| AudioError::Decode(format!("Seek failed: {error}")))?;
                }
                self.clock.elapsed_before_start = offset;
                self.clock.started_at = Some(Instant::now());
                self.clock.stall_at_start = self.stall_now();
                // play() already faded in; SoftFade::try_seek reset gain to 0,
                // so nudge target to restart the post-seek ramp.
                self.set_soft_fade_target(1.0);
                return Ok(());
            }

            match &self.sink {
                Some(sink) => {
                    sink.try_seek(offset)
                        .map_err(|error| AudioError::Decode(format!("Seek failed: {error}")))?;

                    self.clock.elapsed_before_start = offset;
                    self.clock.started_at = was_playing.then(Instant::now);
                    self.clock.stall_at_start = self.stall_now();
                    self.prefetched_next = None;
                    // Crossfade may promote the incoming track inside try_seek when
                    // the UI already handed off at fade-start. Adopt that path so
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
        self.push_output_volume();
        Ok(())
    }

    /// The level actually sent to the output: the user's volume, scaled down
    /// while the sleep timer fades out.
    fn output_volume(&self) -> f32 {
        self.volume * self.sleep_level
    }

    fn push_output_volume(&self) {
        let volume = self.output_volume();
        #[cfg(target_os = "android")]
        {
            let _ = crate::android::audio::exo_set_volume(volume);
        }
        #[cfg(not(target_os = "android"))]
        if let Some(ref sink) = self.sink {
            sink.set_volume(volume);
        }
    }

    fn set_sleep_level(&mut self, level: f32) {
        let level = level.clamp(0.0, 1.0);
        if (self.sleep_level - level).abs() < f32::EPSILON {
            return;
        }
        self.sleep_level = level;
        self.push_output_volume();
    }

    /// The next queue track worth preloading for gapless or crossfade, or
    /// `None` while the sleep timer is waiting for the current track to end
    /// and under repeat one, where the current track plays again instead.
    fn upcoming_for_preload(&self) -> Option<String> {
        if self.stop_after_current || self.repeat == RepeatMode::One {
            return None;
        }
        let next = self.queue.peek_next(&self.repeat);
        // A remote track that can't start yet would block whoever opens it.
        // ExoPlayer streams it through the loopback instead, so Android keeps it.
        #[cfg(not(target_os = "android"))]
        let next = next.filter(|next| stream::is_ready(next));
        next.map(str::to_string)
    }

    fn stall_now(&self) -> u64 {
        self.stall.load(Ordering::Relaxed)
    }

    /// True while playback is waiting on the network.
    pub fn is_buffering(&self) -> bool {
        #[cfg(target_os = "android")]
        {
            crate::android::audio::exo_is_buffering().unwrap_or(false)
        }
        #[cfg(not(target_os = "android"))]
        {
            self.buffering.load(Ordering::Relaxed)
        }
    }

    /// Gets a remote next track ready before the current one ends: start
    /// buffering it near the end, and preload it once it can start. Looks at
    /// most once a second, since readiness can mean a database lookup.
    pub fn tick_remote_prefetch(&mut self) {
        let now = Instant::now();
        if self
            .prefetch_checked_at
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(1))
        {
            return;
        }
        self.prefetch_checked_at = Some(now);
        #[cfg(test)]
        {
            self.prefetch_checks += 1;
        }
        if self.stop_after_current || self.repeat == RepeatMode::One {
            return;
        }
        let Some(next) = self.queue.peek_next(&self.repeat).map(str::to_string) else {
            return;
        };
        if !crate::remote::uri::is_remote(&next) {
            return;
        }
        if !stream::is_ready(&next) {
            const LEAD: Duration = Duration::from_secs(30);
            let near_end = match self.clock.duration {
                Some(duration) if duration >= LEAD => {
                    duration.saturating_sub(self.clock.position(self.stall_now())) <= LEAD
                }
                _ => true,
            };
            if near_end {
                self.warm_once(&next);
            }
            return;
        }
        // ExoPlayer keeps its own playlist on Android.
        #[cfg(not(target_os = "android"))]
        {
            let prefetched = self
                .prefetched_next
                .as_ref()
                .is_some_and(|prefetched| prefetched.path == next);
            let in_crossfade = self.crossfade.as_ref().is_some_and(|crossfade| {
                let state = crossfade.state.lock().unwrap_or_else(|e| e.into_inner());
                state.next_path.as_deref() == Some(next.as_str())
            });
            if !prefetched && !in_crossfade {
                self.sync_preload();
            }
        }
    }

    /// Starts buffering a remote track, unless it was the last one asked.
    fn warm_once(&mut self, path: &str) {
        if self.warmed.as_deref() == Some(path) {
            return;
        }
        self.warmed = Some(path.to_string());
        #[cfg(test)]
        {
            self.warm_requests += 1;
        }
        stream::warm(path);
    }

    /// The track `play_next` would start, without changing anything.
    pub fn peek_next_path(&self) -> Option<String> {
        if self.repeat == RepeatMode::One {
            if let Some(current) = self.current_path.as_ref().and_then(|p| p.to_str()) {
                return Some(current.to_string());
            }
        }
        self.queue.peek_next(&self.repeat).map(str::to_string)
    }

    /// The track `play_previous` would start, without changing anything.
    pub fn peek_previous_path(&self) -> Option<String> {
        if self.position_seconds() > 3.0 {
            return self
                .current_path
                .as_ref()
                .and_then(|p| p.to_str())
                .map(str::to_string);
        }
        self.queue.peek_previous(&self.repeat).map(str::to_string)
    }

    /// Whether auto-advance can go ahead. Opening a remote track that can't
    /// start yet would wait on the network under the player lock, so this
    /// warms it and holds off, for at most [`ADVANCE_WAIT`], after which the
    /// usual error path gets its turn.
    pub fn ready_to_auto_advance(&mut self) -> bool {
        let Some(next) = self.peek_next_path() else {
            self.advance_wait = None;
            return true;
        };
        let ready = !crate::remote::uri::is_remote(&next) || stream::is_ready(&next);
        if !ready {
            self.warm_once(&next);
        }
        advance_gate(&mut self.advance_wait, &next, ready, Instant::now())
    }
    /// Loudness of what is playing right now, 0.0 to 1.0. Zero while paused or
    /// stopped.
    pub fn output_level(&self) -> f32 {
        if !self.is_playing() {
            return 0.0;
        }
        #[cfg(target_os = "android")]
        {
            return crate::android::audio::exo_get_level().unwrap_or(0.0);
        }
        #[cfg(not(target_os = "android"))]
        super::dsp::load_level(&self.level)
    }

    pub fn speed(&self) -> f32 {
        load_shared_speed(&self.speed)
    }

    /// Set the playback speed. Returns the speed in effect, which is exactly
    /// 1.0 for anything within 0.01 of it.
    pub fn set_speed(&mut self, speed: f32) -> Result<f32, String> {
        if !speed.is_finite() || !(MIN_SPEED..=MAX_SPEED).contains(&speed) {
            return Err(format!(
                "Speed must be between {MIN_SPEED}x and {MAX_SPEED}x"
            ));
        }
        let speed = if (speed - 1.0).abs() < 0.01 {
            1.0
        } else {
            speed
        };
        let stall = self.stall_now();
        self.clock.set_speed(speed, stall);
        set_shared_speed(&self.speed, speed);
        #[cfg(target_os = "android")]
        {
            let _ = crate::android::audio::exo_set_speed(speed);
        }
        Ok(speed)
    }

    pub fn set_sleep_timer(&mut self, request: SleepRequest) -> Result<(), String> {
        self.sleep.set(request, Instant::now())?;
        if request == SleepRequest::Off {
            self.set_sleep_level(1.0);
        }
        self.set_stop_after_current(self.sleep.is_end_of_track());
        Ok(())
    }

    /// Copy a running sleep timer onto a player built to replace `other`, such
    /// as after an output device switch. Call before the new player starts
    /// the track, so it is loaded without a preloaded follow-up.
    pub fn carry_sleep_timer_from(&mut self, other: &AudioPlayer) {
        self.sleep = other.sleep.clone();
        self.sleep_level = other.sleep_level;
        self.stop_after_current = other.stop_after_current;
    }

    pub fn sleep_timer_status(&self) -> SleepTimerStatus {
        self.sleep.status(Instant::now())
    }

    /// True when the next natural track end should pause instead of advancing.
    /// Auto-advance loops check this and call [`Self::finish_held_track`].
    pub fn sleep_after_current_track(&self) -> bool {
        self.stop_after_current
    }

    /// Advance the sleep timer by one tick: fade the output while it runs
    /// down, and pause when a countdown expires.
    ///
    /// Returns `true` when this call paused playback.
    pub fn tick_sleep_timer(&mut self) -> bool {
        let speed = f64::from(self.speed());
        let track_left = self.duration_seconds().map(|duration| {
            Duration::from_secs_f64(((duration - self.position_seconds()) / speed).max(0.0))
        });
        match self.sleep.step(Instant::now(), track_left) {
            SleepStep::Idle => false,
            SleepStep::Level(level) => {
                if self.is_playing() {
                    self.set_sleep_level(level);
                }
                false
            }
            SleepStep::Expired => {
                self.sleep.cancel();
                let paused = self.is_playing() && self.pause().is_ok();
                self.set_sleep_level(1.0);
                paused
            }
        }
    }

    /// The track the sleep timer was waiting on has ended: turn the timer off
    /// and load what would have played next, paused at its start, so resuming
    /// carries on with the queue. Stops when the queue has nothing left.
    ///
    /// Returns the path that is now loaded.
    pub fn finish_held_track(&mut self) -> Result<Option<String>, AudioError> {
        self.sleep.cancel();
        self.stop_after_current = false;
        self.sleep_level = 1.0;

        let next = if self.repeat == RepeatMode::One {
            self.current_path
                .as_ref()
                .and_then(|path| path.to_str())
                .map(str::to_string)
        } else {
            self.queue.next(&self.repeat).map(str::to_string)
        };
        let Some(path) = next else {
            self.stop()?;
            return Ok(None);
        };
        self.load_paused_at(&path, 0.0)?;
        Ok(Some(path))
    }

    fn set_stop_after_current(&mut self, enabled: bool) {
        if self.stop_after_current == enabled {
            return;
        }
        self.stop_after_current = enabled;
        let Some(path) = self
            .current_path
            .as_ref()
            .and_then(|path| path.to_str())
            .map(str::to_string)
        else {
            return;
        };

        if !enabled {
            // Preloading is allowed again. Gapless picks the next track back
            // up now; a crossfade resumes from the following track.
            #[cfg(not(target_os = "android"))]
            if self.crossfade_duration <= 0.0 {
                self.prefetch_next_into_sink();
            }
            #[cfg(target_os = "android")]
            self.sync_android_playback_extras();
            return;
        }

        // The next track may already be lined up behind this one. Reload the
        // current track at the same spot without it.
        #[cfg(not(target_os = "android"))]
        let preloaded = self.has_sink_prefetch() || self.crossfade_has_next();
        #[cfg(target_os = "android")]
        let preloaded = self.android_gapless_playlist.len() > 1;
        if !preloaded {
            #[cfg(target_os = "android")]
            self.sync_android_playback_extras();
            return;
        }

        let position = self.position_seconds();
        let was_playing = self.is_playing();
        #[cfg(not(target_os = "android"))]
        if was_playing {
            self.fade_out_blocking();
        }
        if let Err(error) = self.load_paused_at(&path, position) {
            tracing::warn!("Could not reload the track for the sleep timer: {error}");
            return;
        }
        #[cfg(target_os = "android")]
        self.android_gapless_playlist.clear();
        if was_playing {
            let _ = self.resume();
        }
    }

    /// True when the crossfade has a track lined up to fade into.
    #[cfg(not(target_os = "android"))]
    fn crossfade_has_next(&self) -> bool {
        self.crossfade.as_ref().is_some_and(|crossfade| {
            crossfade
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .next_path
                .is_some()
        })
    }

    pub fn is_playing(&self) -> bool {
        #[cfg(target_os = "android")]
        {
            // Rust transport clock is authoritative. The Exo JNI cache is only
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
                self.clock.raw_elapsed(self.stall_now()) >= duration.saturating_add(grace)
                    && !self.is_playing()
            });
        }

        #[cfg(not(target_os = "android"))]
        {
            // A crossfade source keeps playing the *next* track in the same sink after
            // the outgoing track ends. Wall-clock / pending_next alone are not enough:
            // once the mixer promotes, pending_next flips false while audio is still
            // mid-song, and a premature play_next() restarts that track from 0.
            // Only advance when this sink is truly exhausted (then start the following track).
            if self.crossfade.is_some() && !self.sink_exhausted() {
                return false;
            }

            let at_duration_end = self.clock.duration.is_some_and(|duration| {
                let grace = Duration::from_millis(350);
                self.clock.raw_elapsed(self.stall_now()) >= duration.saturating_add(grace)
            });

            // Prefetched next is already in the sink, so only adopt once the current
            // source has actually finished (sink has drained down to the follow-up).
            // Adopting earlier and calling play() on a path mismatch was restarting
            // the next track after a few milliseconds of audio.
            if self.prefetched_next.is_some() {
                let sink_len = self.sink.as_ref().map(|s| s.len()).unwrap_or(0);
                let past_start =
                    self.clock.raw_elapsed(self.stall_now()) >= Duration::from_millis(500);
                if sink_len <= 1 && past_start {
                    return true;
                }
                if at_duration_end && sink_len <= 1 {
                    return true;
                }
                // Still playing the outgoing source (len >= 2), so wait.
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

    pub fn get_current_path(&self) -> Option<&PathBuf> {
        self.current_path.as_ref()
    }

    pub fn position_seconds(&self) -> f64 {
        #[cfg(target_os = "android")]
        {
            if let Some(path) = self.current_path.as_deref().and_then(|p| p.to_str()) {
                // ExoPlayer knows where playback really is. A wall clock runs
                // on through buffering, a stalled read or a late start, and
                // one restarted on a handoff tick starts behind. Its position
                // only counts for the track Rust thinks is playing; until a
                // handoff is picked up, the clock stands in.
                if let Ok(Some(ms)) = crate::android::audio::exo_position_for(path) {
                    return ms as f64 / 1000.0;
                }
                return self.clock.position(self.stall_now()).as_secs_f64();
            }
        }
        self.clock.position(self.stall_now()).as_secs_f64()
    }

    pub fn duration_seconds(&self) -> Option<f64> {
        #[cfg(target_os = "android")]
        {
            if let Some(ms) = self.android_exo_duration() {
                return Some(ms as f64 / 1000.0);
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
            let switched =
                self.check_android_gapless_handoff() || self.check_android_crossfade_handoff();
            self.sync_android_clock_to_exo();
            return switched;
        }

        #[cfg(not(target_os = "android"))]
        {
            let switched = self.adopt_crossfade_switch();
            // Once a fade finishes, the track it faded into has nothing after
            // it until the following queue track is sent along.
            if self.crossfade.is_some() {
                self.refresh_preload();
            }
            switched
        }
    }

    /// Follow the crossfade mixer onto the track it started fading into.
    #[cfg(not(target_os = "android"))]
    fn adopt_crossfade_switch(&mut self) -> bool {
        let Some(state) = self.crossfade.as_ref().map(|c| c.state.clone()) else {
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
            speed: self.speed(),
            stall_at_start: self.stall_now(),
        };
        true
    }

    /// Align `current_path` / duration with the crossfade mixer after a seek.
    ///
    /// Unlike [`Self::check_crossfade_track_switch`], this does not require the
    /// `track_switched` latch; seek may promote the incoming source without
    /// re-signaling a switch.
    fn adopt_crossfade_logical_track(&mut self) {
        let Some(state) = self.crossfade.as_ref().map(|c| c.state.clone()) else {
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
        self.eq_config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
        cfg.apply_preset(name).ok_or_else(|| {
            let names: Vec<&str> = EqConfig::list_presets().map(|(n, _)| n).collect();
            format!(
                "Unknown EQ preset \"{name}\". Available: {}",
                names.join(", ")
            )
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
        let cfg = self
            .eq_config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
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
        // previous source keeps playing, so only adopt when that source
        // has actually finished. Manual Next must tear down and restart.
        #[cfg(target_os = "android")]
        if self.try_android_gapless_skip_forward()? {
            return Ok(self
                .current_path
                .as_ref()
                .and_then(|p| p.to_str().map(str::to_string)));
        }

        if let Some(Prefetched {
            path: prefetched,
            duration,
            ..
        }) = self.prefetched_next.take()
        {
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
                // Prefetch no longer matches the queue, so fall through.
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
        let past_start = self.clock.raw_elapsed(self.stall_now()) >= Duration::from_millis(500);
        let at_duration_end = self.clock.duration.is_some_and(|duration| {
            let grace = Duration::from_millis(350);
            self.clock.raw_elapsed(self.stall_now()) >= duration.saturating_add(grace)
        });
        // len <= 1 means only the follow-up (or an empty sink) is left.
        ((past_start || at_duration_end) && sink_len <= 1) || self.sink_exhausted()
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

    pub fn set_shuffle(&mut self, enabled: bool) {
        self.queue.set_shuffle(enabled);
        self.sync_preload();
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
        self.sync_preload();
    }

    pub fn set_song_couples(&mut self, couples: Vec<(String, String)>) {
        self.queue.set_couples(couples);
        self.sync_preload();
    }

    pub fn enqueue(&mut self, path: &str) {
        self.queue.enqueue(path.to_string());
        self.sync_preload();
    }

    pub fn insert_next(&mut self, path: &str) {
        self.queue.insert_next(path.to_string());
        self.sync_preload();
    }

    /// Remove a queue entry. Removing the track that is playing moves on to
    /// the one after it, or stops at the end of the queue: otherwise the
    /// removed track plays out and the queue then skips its successor.
    pub fn remove_from_queue(&mut self, index: usize) -> Option<String> {
        let was_current = self.queue.current_index() == Some(index);
        let removed = self.queue.remove_at(index)?;
        if was_current && self.current_path.is_some() {
            let paused = self.is_paused();
            if index < self.queue.tracks().len() {
                // A paused player stays paused on the next track, loaded
                // without being started first.
                if paused {
                    let next = self.queue.jump_to(index).map(str::to_string);
                    if let Some(next) = next {
                        if let Err(error) = self.load_paused_at(&next, 0.0) {
                            tracing::warn!("Could not load \"{next}\": {error}");
                        }
                    }
                } else {
                    let _ = self.jump_to_queue_index(index);
                }
            } else {
                let _ = self.stop();
            }
        } else {
            self.sync_preload();
        }
        Some(removed)
    }

    pub fn move_queue_track(&mut self, from: usize, to: usize) -> bool {
        let moved = self.queue.move_track(from, to);
        if moved {
            self.sync_preload();
        }
        moved
    }

    pub fn clear_upcoming(&mut self) {
        self.queue.clear_upcoming();
        self.sync_preload();
    }

    /// Play the queue's current track, such as one just picked with
    /// [`Queue::pick`]. Returns its path.
    pub fn play_current(&mut self) -> Result<Option<String>, AudioError> {
        let path = self
            .queue
            .current_index()
            .and_then(|index| self.queue.tracks().get(index).cloned());
        if let Some(ref path) = path {
            self.play(path)?;
        }
        Ok(path)
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

    fn coupled(paths: &[&str], first: &str, second: &str) -> Queue {
        let mut queue = queue_of(paths);
        queue.set_couples(vec![(first.to_string(), second.to_string())]);
        queue
    }

    #[test]
    fn picking_the_second_of_a_couple_starts_from_the_first() {
        let mut queue = coupled(&["/a", "/x", "/b"], "/a", "/b");
        assert_eq!(queue.pick(2), Some("/a"));
        assert_eq!(queue.next(&RepeatMode::Off), Some("/b"));
    }

    #[test]
    fn picking_the_second_of_a_couple_queues_a_first_that_was_missing() {
        let mut queue = coupled(&["/x", "/b", "/y"], "/a", "/b");
        assert_eq!(queue.pick(1), Some("/a"));
        assert_eq!(queue.tracks(), ["/x", "/a", "/b", "/y"]);
        assert_eq!(queue.next(&RepeatMode::Off), Some("/b"));
    }

    #[test]
    fn picking_the_second_of_a_couple_moves_a_first_that_comes_later() {
        let mut queue = coupled(&["/b", "/x", "/a"], "/a", "/b");
        assert_eq!(queue.pick(0), Some("/a"));
        assert_eq!(queue.tracks(), ["/a", "/b", "/x"]);
    }

    #[test]
    fn picking_the_second_of_a_couple_works_under_shuffle() {
        let mut queue = Queue::default();
        queue.set_shuffle(true);
        queue.set_couples(vec![("/a".to_string(), "/b".to_string())]);
        queue.set_tracks(["/x", "/a", "/y", "/b", "/z"].map(String::from).to_vec());
        assert_eq!(queue.pick(3), Some("/a"));
        assert_eq!(queue.next(&RepeatMode::Off), Some("/b"));
        assert_eq!(queue.tracks().len(), 5);
    }

    #[test]
    fn picking_anything_else_plays_what_was_picked() {
        let mut queue = coupled(&["/a", "/x", "/b"], "/a", "/b");
        assert_eq!(queue.pick(0), Some("/a"));
        let x = queue.tracks().iter().position(|p| p == "/x").unwrap();
        assert_eq!(queue.pick(x), Some("/x"));
    }

    // ── set_tracks ───────────────────────────────────────────────────────

    #[test]
    fn set_tracks_resets_current_index_and_keeps_shuffle_on() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(2);
        queue.set_shuffle(true);
        assert!(queue.current_index().is_some());
        assert!(queue.is_shuffled());

        queue.set_tracks(vec!["x".to_string(), "y".to_string()]);
        assert_eq!(queue.current_index(), None);
        assert!(queue.is_shuffled());
        assert_eq!(queue.tracks(), &["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn set_tracks_leaves_an_unshuffled_queue_unshuffled() {
        let mut queue = queue_of(&["a", "b"]);
        queue.set_tracks(vec!["x".to_string(), "y".to_string()]);
        assert!(!queue.is_shuffled());
    }

    #[test]
    fn a_new_shuffled_queue_plays_every_track_from_the_one_picked() {
        let paths: Vec<String> = (0..20).map(|i| i.to_string()).collect();
        let mut queue = queue_of(&["a"]);
        queue.set_shuffle(true);
        queue.set_tracks(paths);
        assert_eq!(queue.jump(7), Some("7"));
        assert_eq!(queue.current_index(), Some(0));

        let mut played = vec!["7".to_string()];
        while let Some(path) = queue.next(&RepeatMode::Off) {
            played.push(path.to_string());
        }
        assert_eq!(played, queue.tracks());
        played.sort_unstable_by_key(|p| p.parse::<usize>().unwrap());
        assert_eq!(played, (0..20).map(|i| i.to_string()).collect::<Vec<_>>());
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
    fn next_wraps_to_the_shuffled_start_under_repeat_all() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(2);
        queue.set_shuffle(true);
        // The track playing when shuffle went on opens the shuffled queue.
        assert_eq!(queue.tracks()[0], "c");

        queue.jump(2);
        let result = queue.next(&RepeatMode::All);
        assert_eq!(result, Some("c"));
        assert_eq!(queue.current_index(), Some(0));
    }

    // ── previous ─────────────────────────────────────────────────────────

    #[test]
    fn previous_returns_none_before_first_track_when_repeat_off_or_one() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        assert_eq!(queue.previous(&RepeatMode::Off), None);
        assert_eq!(queue.current_index(), Some(0));
        assert_eq!(queue.previous(&RepeatMode::One), None);
        assert_eq!(queue.current_index(), Some(0));
        // The track after the first is still next.
        assert_eq!(queue.next(&RepeatMode::Off), Some("b"));
    }

    #[test]
    fn a_fresh_shuffled_queue_starts_at_the_first_shuffled_track() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.set_shuffle(true);
        let order = queue.tracks().to_vec();
        let played: Vec<String> = (0..3)
            .map(|_| queue.next(&RepeatMode::Off).unwrap().to_string())
            .collect();
        assert_eq!(played, order);
        assert_eq!(queue.next(&RepeatMode::Off), None);
    }

    #[test]
    fn previous_at_the_start_of_a_shuffled_queue_keeps_the_current_track() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.set_shuffle(true);
        queue.jump(0);
        assert_eq!(queue.previous(&RepeatMode::Off), None);
        assert_eq!(queue.current_index(), Some(0));
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
    fn set_shuffle_true_reorders_the_queue_with_the_current_track_first() {
        let mut queue = queue_of(&["a", "b", "c", "d", "e"]);
        queue.jump(2);
        queue.set_shuffle(true);
        assert!(queue.is_shuffled());

        let mut sorted = queue.tracks().to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, ["a", "b", "c", "d", "e"]);
        assert_eq!(queue.tracks()[0], "c");
        assert_eq!(queue.current_index(), Some(0));
    }

    #[test]
    fn set_shuffle_false_restores_the_order_and_keeps_the_current_track() {
        let paths: Vec<String> = (0..20).map(|i| i.to_string()).collect();
        let mut queue = Queue::default();
        queue.set_tracks(paths.clone());
        queue.jump(5);
        queue.set_shuffle(true);
        queue.next(&RepeatMode::Off);
        let playing = queue.tracks()[queue.current_index().unwrap()].clone();

        queue.set_shuffle(false);
        assert!(!queue.is_shuffled());
        assert_eq!(queue.tracks(), paths);
        assert_eq!(queue.tracks()[queue.current_index().unwrap()], playing);
    }

    // ── enqueue ──────────────────────────────────────────────────────────

    #[test]
    fn enqueue_appends_to_tracks() {
        let mut queue = queue_of(&["a", "b"]);
        queue.enqueue("c".to_string());
        assert_eq!(
            queue.tracks(),
            &["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn enqueue_while_shuffled_appends_and_stays_last_when_unshuffled() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.set_shuffle(true);
        queue.enqueue("d".to_string());
        assert_eq!(queue.tracks()[3], "d");
        queue.set_shuffle(false);
        assert_eq!(queue.tracks(), ["a", "b", "c", "d"]);
    }

    // ── insert_next ──────────────────────────────────────────────────────

    #[test]
    fn insert_next_inserts_right_after_current() {
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        queue.insert_next("x".to_string());
        assert_eq!(
            queue.tracks(),
            &[
                "a".to_string(),
                "x".to_string(),
                "b".to_string(),
                "c".to_string()
            ]
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
    fn insert_next_while_shuffled_follows_the_current_track_both_ways() {
        let mut queue = queue_of(&["a", "b", "c", "d"]);
        queue.jump(1);
        queue.set_shuffle(true);
        assert_eq!(queue.tracks()[0], "b");

        queue.insert_next("x".to_string());
        assert_eq!(queue.tracks()[1], "x");
        assert_eq!(queue.next(&RepeatMode::Off), Some("x"));

        queue.set_shuffle(false);
        assert_eq!(queue.tracks(), ["a", "b", "x", "c", "d"]);
        assert_eq!(queue.current_index(), Some(2));
    }

    #[test]
    fn removing_and_moving_while_shuffled_still_unshuffles_cleanly() {
        let mut queue = queue_of(&["a", "b", "c", "d", "e"]);
        queue.jump(0);
        queue.set_shuffle(true);
        let gone = queue.tracks()[2].clone();
        queue.remove_at(2);
        queue.move_track(3, 1);
        queue.set_shuffle(false);
        let expected: Vec<&str> = ["a", "b", "c", "d", "e"]
            .into_iter()
            .filter(|p| *p != gone)
            .collect();
        assert_eq!(queue.tracks(), expected);
        assert_eq!(queue.tracks()[queue.current_index().unwrap()], "a");
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
            &[
                "b".to_string(),
                "c".to_string(),
                "a".to_string(),
                "d".to_string()
            ]
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
            &[
                "d".to_string(),
                "a".to_string(),
                "b".to_string(),
                "c".to_string()
            ]
        );
        let idx = queue.current_index().unwrap();
        assert_eq!(queue.tracks()[idx], "c");
    }

    // ── song coupling ────────────────────────────────────────────────────

    fn couple(first: &str, second: &str) -> Vec<(String, String)> {
        vec![(first.to_string(), second.to_string())]
    }

    fn assert_back_to_back(queue: &Queue, first: &str, second: &str) {
        let tracks = queue.tracks();
        let a = tracks.iter().position(|p| p == first).unwrap();
        assert_eq!(
            tracks.get(a + 1).map(String::as_str),
            Some(second),
            "{tracks:?}"
        );
    }

    #[test]
    fn a_shuffle_always_plays_a_couple_back_to_back_in_order() {
        let paths: Vec<String> = (0..12).map(|i| i.to_string()).collect();
        for _ in 0..200 {
            let mut queue = Queue::default();
            queue.set_couples(couple("7", "3"));
            queue.set_tracks(paths.clone());
            queue.set_shuffle(true);
            assert_back_to_back(&queue, "7", "3");
            assert_eq!(queue.tracks().len(), 12);
        }
    }

    #[test]
    fn picking_either_half_of_a_couple_to_start_still_keeps_them_together() {
        let paths: Vec<String> = (0..8).map(|i| i.to_string()).collect();
        let mut queue = Queue::default();
        queue.set_couples(couple("2", "5"));
        queue.set_shuffle(true);
        queue.set_tracks(paths.clone());
        queue.jump(2);
        assert_eq!(&queue.tracks()[..2], ["2", "5"]);
    }

    #[test]
    fn coupling_while_shuffled_rearranges_only_what_is_still_to_play() {
        let mut queue = queue_of(&["a", "b", "c", "d", "e"]);
        queue.jump(0);
        queue.set_shuffle(true);
        queue.set_couples(couple("a", "e"));
        assert_eq!(&queue.tracks()[..2], ["a", "e"]);
        assert_eq!(queue.current_index(), Some(0));

        // The second song is playing, so its first has nowhere to go.
        let mut queue = queue_of(&["a", "b", "c"]);
        queue.jump(0);
        queue.set_shuffle(true);
        let before = queue.tracks().to_vec();
        queue.set_couples(couple("b", "a"));
        assert_eq!(queue.tracks(), before);
    }

    #[test]
    fn a_couple_plays_back_to_back_without_shuffle_too() {
        let mut queue = queue_of(&["a", "b", "c", "d", "e"]);
        queue.set_couples(couple("b", "e"));
        // Nothing has played yet, so the new queue waits for its first pick.
        assert_eq!(queue.tracks(), ["a", "b", "c", "d", "e"]);
        assert_eq!(queue.jump(0), Some("a"));
        assert_eq!(queue.tracks(), ["a", "b", "e", "c", "d"]);

        // Coupling a queue that is already playing rearranges what is left.
        queue.set_couples(couple("d", "c"));
        assert_eq!(queue.tracks(), ["a", "b", "e", "d", "c"]);
    }

    #[test]
    fn turning_shuffle_off_keeps_couples_together() {
        let mut queue = queue_of(&["a", "b", "c", "d"]);
        queue.set_couples(couple("d", "b"));
        queue.jump(0);
        assert_eq!(queue.tracks(), ["a", "c", "d", "b"]);
        queue.set_shuffle(true);
        assert_back_to_back(&queue, "d", "b");
        queue.set_shuffle(false);
        assert_back_to_back(&queue, "d", "b");
        assert_eq!(queue.tracks()[queue.current_index().unwrap()], "a");
    }

    #[test]
    fn shuffling_a_new_queue_does_not_reshuffle_on_the_first_pick() {
        let paths: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let mut queue = Queue::default();
        queue.set_tracks(paths);
        queue.set_shuffle(true);
        let shuffled = queue.tracks().to_vec();
        queue.jump(3);
        assert_eq!(queue.tracks(), shuffled);
        assert_eq!(queue.current_index(), Some(3));
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
        assert!(queue.is_shuffled());
        queue.set_shuffle(false);
        assert_eq!(queue.tracks(), &["b".to_string()]);
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
        assert_eq!(clock.raw_elapsed(0), Duration::ZERO);
        assert_eq!(clock.position(0), Duration::ZERO);
    }

    #[test]
    fn clock_runs_at_the_playback_speed() {
        let clock = PlaybackClock {
            started_at: Some(Instant::now() - Duration::from_secs(1)),
            elapsed_before_start: Duration::ZERO,
            duration: None,
            speed: 2.0,
            stall_at_start: 0,
        };
        let position = clock.position(0).as_secs_f64();
        assert!((1.95..2.2).contains(&position), "{position}");
    }

    #[test]
    fn changing_speed_keeps_the_position() {
        let mut player = AudioPlayer::new_deferred();
        player.clock = PlaybackClock {
            started_at: Some(Instant::now() - Duration::from_secs(1)),
            elapsed_before_start: Duration::ZERO,
            duration: None,
            speed: 1.0,
            stall_at_start: 0,
        };
        assert_eq!(player.set_speed(2.0), Ok(2.0));
        let position = player.clock.position(0).as_secs_f64();
        assert!((0.95..1.1).contains(&position), "{position}");
        assert_eq!(player.speed(), 2.0);
    }

    #[test]
    fn speed_is_range_checked_and_snaps_to_normal() {
        let mut player = AudioPlayer::new_deferred();
        assert!(player.set_speed(0.4).is_err());
        assert!(player.set_speed(2.01).is_err());
        assert!(player.set_speed(f32::NAN).is_err());
        assert_eq!(player.set_speed(1.004), Ok(1.0));
        assert_eq!(player.set_speed(0.5), Ok(0.5));
        assert_eq!(player.speed(), 0.5);
    }

    /// Two seconds of a 440 Hz tone at half scale, as a mono WAV.
    fn tone_wav() -> std::path::PathBuf {
        let rate = 22_050u32;
        let samples: Vec<i16> = (0..rate * 2)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (16_000.0 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) as i16
            })
            .collect();
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!("wave-tone-{}.wav", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// Plays through the real audio output (at zero volume), so CI skips it.
    /// Run with `cargo test --lib -- --ignored live_level`.
    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore]
    fn live_level_follows_what_is_playing() {
        let path = tone_wav();
        let mut player = AudioPlayer::new().unwrap();
        player.set_volume(0.0).unwrap();
        assert_eq!(player.output_level(), 0.0);
        player.play(path.to_str().unwrap()).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let playing = player.output_level();
        player.pause().unwrap();
        let paused = player.output_level();
        player.stop().unwrap();
        std::fs::remove_file(&path).ok();
        // A half-scale sine has an RMS of about 0.35, measured before volume.
        assert!((0.25..0.45).contains(&playing), "{playing}");
        assert_eq!(paused, 0.0);
    }

    #[test]
    fn a_track_ends_sooner_at_double_speed() {
        let clock = PlaybackClock {
            started_at: Some(Instant::now() - Duration::from_secs(6)),
            elapsed_before_start: Duration::ZERO,
            duration: Some(Duration::from_secs(10)),
            speed: 2.0,
            stall_at_start: 0,
        };
        // Six wall seconds at 2x is twelve seconds of a ten second track.
        assert!(clock.raw_elapsed(0) >= Duration::from_secs(10));
    }

    #[test]
    fn clock_elapsed_and_position_advance_after_start() {
        let mut clock = PlaybackClock::stopped();
        clock.started_at = Some(Instant::now());
        std::thread::sleep(Duration::from_millis(5));
        assert!(clock.raw_elapsed(0) >= Duration::from_millis(5));
        assert!(clock.position(0) >= Duration::from_millis(5));
    }

    #[test]
    fn clock_position_clamps_to_duration() {
        let mut clock = PlaybackClock::stopped();
        clock.duration = Some(Duration::from_millis(1));
        clock.started_at = Some(Instant::now());
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(clock.position(0), Duration::from_millis(1));
        assert!(clock.raw_elapsed(0) > clock.position(0));
    }

    #[test]
    fn clock_subtracts_stall_time() {
        let clock = PlaybackClock {
            started_at: Some(Instant::now() - Duration::from_secs(10)),
            elapsed_before_start: Duration::ZERO,
            duration: None,
            speed: 1.0,
            stall_at_start: 1_000_000,
        };
        let position = clock.position(5_000_000).as_secs_f64();
        assert!((5.95..6.2).contains(&position), "{position}");
        assert_eq!(clock.position(60_000_000), Duration::ZERO);

        let mut player = AudioPlayer::new_deferred();
        player.clock = PlaybackClock {
            stall_at_start: 0,
            ..clock
        };
        player.stall.store(4_000_000, Ordering::Relaxed);
        let position = player.position_seconds();
        assert!((5.95..6.2).contains(&position), "{position}");
    }

    #[test]
    #[cfg(not(target_os = "android"))]
    fn uncached_remote_next_is_not_preloaded() {
        let mut player = AudioPlayer::new_deferred();
        player
            .queue
            .set_tracks(vec!["/music/a.flac".into(), "wave-remote://s/1".into()]);
        player.queue.jump(0);
        assert_eq!(player.upcoming_for_preload(), None);

        player
            .queue
            .set_tracks(vec!["/music/a.flac".into(), "/music/b.flac".into()]);
        player.queue.jump(0);
        assert_eq!(
            player.upcoming_for_preload().as_deref(),
            Some("/music/b.flac")
        );
    }

    #[test]
    fn peeks_match_what_next_and_previous_pick() {
        let paths: Vec<String> = (0..5).map(|i| format!("/music/{i}.flac")).collect();
        for repeat in [RepeatMode::Off, RepeatMode::All] {
            for shuffle in [false, true] {
                for start in [0, 2, 4] {
                    let mut player = AudioPlayer::new_deferred();
                    player.repeat = repeat.clone();
                    player.queue.set_tracks(paths.clone());
                    player.queue.jump(start);
                    player.set_shuffle(shuffle);

                    let mut forward = player.queue.clone();
                    let mut back = player.queue.clone();
                    let case = format!("{repeat:?} shuffle={shuffle} start={start}");
                    assert_eq!(
                        player.peek_next_path().as_deref(),
                        forward.next(&repeat),
                        "{case}"
                    );
                    assert_eq!(
                        player.peek_previous_path().as_deref(),
                        back.previous(&repeat),
                        "{case}"
                    );
                }
            }
        }

        let mut player = AudioPlayer::new_deferred();
        player.queue.set_tracks(paths.clone());
        player.queue.jump(2);
        player.current_path = Some(PathBuf::from("/music/2.flac"));
        player.repeat = RepeatMode::One;
        assert_eq!(player.peek_next_path().as_deref(), Some("/music/2.flac"));
        player.repeat = RepeatMode::Off;
        player.clock.elapsed_before_start = Duration::from_secs(10);
        assert_eq!(
            player.peek_previous_path().as_deref(),
            Some("/music/2.flac")
        );
    }

    #[test]
    fn auto_advance_waits_for_a_remote_next_then_gives_up() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let remote = "wave-remote://s/1";
        let mut wait = None;
        assert!(!advance_gate(&mut wait, remote, false, start));
        assert!(!advance_gate(&mut wait, remote, false, at(9)));
        assert!(advance_gate(&mut wait, remote, false, at(10)));
        assert!(wait.is_none());

        assert!(!advance_gate(&mut wait, remote, false, at(20)));
        assert!(advance_gate(&mut wait, remote, true, at(21)));
        assert!(wait.is_none());

        // Another next track waits from scratch.
        assert!(!advance_gate(&mut wait, remote, false, at(30)));
        assert!(!advance_gate(&mut wait, "wave-remote://s/2", false, at(39)));
        assert!(!advance_gate(&mut wait, "wave-remote://s/2", false, at(48)));
        assert!(advance_gate(&mut wait, "wave-remote://s/2", false, at(49)));

        let mut player = AudioPlayer::new_deferred();
        player
            .queue
            .set_tracks(vec!["/music/a.flac".into(), remote.into()]);
        player.queue.jump(0);
        assert!(!player.ready_to_auto_advance());
        assert!(!player.ready_to_auto_advance());
        assert_eq!(player.warm_requests, 1);
        player.queue.jump(1);
        assert!(player.ready_to_auto_advance());
    }

    #[test]
    fn remote_prefetch_looks_once_a_second_and_warms_each_track_once() {
        let mut player = AudioPlayer::new_deferred();
        player.queue.set_tracks(vec![
            "/music/a.flac".into(),
            "wave-remote://s/1".into(),
            "wave-remote://s/2".into(),
        ]);
        player.queue.jump(0);
        player.tick_remote_prefetch();
        player.tick_remote_prefetch();
        assert_eq!(player.prefetch_checks, 1);
        assert_eq!(player.warm_requests, 1);

        player.prefetch_checked_at = Some(Instant::now() - Duration::from_secs(2));
        player.tick_remote_prefetch();
        assert_eq!(player.prefetch_checks, 2);
        assert_eq!(player.warm_requests, 1);

        player.queue.jump(1);
        player.prefetch_checked_at = None;
        player.tick_remote_prefetch();
        assert_eq!(player.prefetch_checks, 3);
        assert_eq!(player.warm_requests, 2);
        assert_eq!(player.warmed.as_deref(), Some("wave-remote://s/2"));
    }

    /// `seconds` of a 440 Hz tone, as a mono WAV.
    fn tone_of(seconds: f32) -> String {
        let rate = 22_050u32;
        let samples: Vec<i16> = (0..(rate as f32 * seconds) as u32)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (12_000.0 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) as i16
            })
            .collect();
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!("wave-tone-{}.wav", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// A muted player on the real output with a queue of `count` tones.
    #[cfg(not(target_os = "android"))]
    fn playing_queue(count: usize, seconds: f32) -> (AudioPlayer, Vec<String>) {
        let paths: Vec<String> = (0..count).map(|_| tone_of(seconds)).collect();
        let mut player = AudioPlayer::new().unwrap();
        player.set_volume(0.0).unwrap();
        player.queue.set_tracks(paths.clone());
        (player, paths)
    }

    /// Tick the player the way the app does until the queue runs out, and
    /// return each track that became current with the time it did.
    #[cfg(not(target_os = "android"))]
    fn play_out(player: &mut AudioPlayer, limit: Duration) -> Vec<(String, Duration)> {
        let start = Instant::now();
        let mut order = Vec::new();
        let mut last = None;
        while start.elapsed() < limit {
            player.check_crossfade_track_switch();
            if player.should_auto_advance() && !matches!(player.play_next(), Ok(Some(_))) {
                let _ = player.stop();
            }
            let current = player
                .get_current_path()
                .map(|path| path.to_string_lossy().into_owned());
            if current != last {
                if let Some(path) = &current {
                    order.push((path.clone(), start.elapsed()));
                }
                last = current.clone();
            }
            if current.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        order
    }

    #[cfg(not(target_os = "android"))]
    fn paths_of(order: &[(String, Duration)]) -> Vec<&str> {
        order.iter().map(|(path, _)| path.as_str()).collect()
    }

    // The tests below play through the real audio output (muted), so CI
    // skips them. Run with `cargo test --lib -- --ignored --test-threads=1`.

    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore = "needs an audio output device"]
    fn a_track_queued_next_mid_song_plays_right_after_it() {
        let (mut player, paths) = playing_queue(3, 0.8);
        player.jump_to_queue_index(0).unwrap();
        let next = tone_of(0.8);
        player.insert_next(&next);
        let order = play_out(&mut player, Duration::from_secs(6));
        assert_eq!(paths_of(&order), [&paths[0], &next, &paths[1], &paths[2]]);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore = "needs an audio output device"]
    fn moving_a_track_mid_song_changes_what_plays_next() {
        let (mut player, paths) = playing_queue(3, 0.8);
        player.jump_to_queue_index(0).unwrap();
        player.move_queue_track(2, 1);
        let order = play_out(&mut player, Duration::from_secs(5));
        assert_eq!(paths_of(&order), [&paths[0], &paths[2], &paths[1]]);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore = "needs an audio output device"]
    fn shuffling_mid_song_still_plays_every_track_once() {
        let (mut player, _) = playing_queue(6, 0.6);
        player.jump_to_queue_index(0).unwrap();
        player.set_shuffle(true);
        let shuffled = player.queue.tracks().to_vec();
        let order = play_out(&mut player, Duration::from_secs(8));
        assert_eq!(paths_of(&order), shuffled);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore = "needs an audio output device"]
    fn repeat_one_set_mid_song_drops_the_preloaded_track() {
        let (mut player, paths) = playing_queue(2, 0.8);
        player.jump_to_queue_index(0).unwrap();
        assert!(player.prefetched_next.is_some());
        player.set_repeat(RepeatMode::One);
        assert!(player.prefetched_next.is_none());
        let order = play_out(&mut player, Duration::from_secs(2));
        assert_eq!(paths_of(&order), [&paths[0]]);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore = "needs an audio output device"]
    fn a_crossfade_fades_into_a_track_queued_next_mid_song() {
        let (mut player, paths) = playing_queue(3, 1.6);
        player.set_crossfade_duration(0.5);
        player.jump_to_queue_index(0).unwrap();
        let next = tone_of(1.6);
        player.insert_next(&next);
        let order = play_out(&mut player, Duration::from_secs(9));
        assert_eq!(paths_of(&order), [&paths[0], &next, &paths[1], &paths[2]]);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore = "needs an audio output device"]
    fn repeat_one_repeats_with_crossfade_on() {
        let (mut player, paths) = playing_queue(3, 1.6);
        player.set_crossfade_duration(0.5);
        player.set_repeat(RepeatMode::One);
        player.jump_to_queue_index(0).unwrap();
        let order = play_out(&mut player, Duration::from_secs(4));
        assert_eq!(paths_of(&order), [&paths[0]]);
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore = "needs an audio output device"]
    fn every_track_change_crossfades() {
        let (mut player, paths) = playing_queue(4, 1.6);
        player.set_crossfade_duration(0.5);
        player.jump_to_queue_index(0).unwrap();
        let order = play_out(&mut player, Duration::from_secs(8));
        assert_eq!(paths_of(&order), paths);
        // Each handoff comes at the fade, half a second before the outgoing
        // track ends. One that waits for the end lands 1.6 s after the last.
        for pair in order.windows(2) {
            let gap = pair[1].1 - pair[0].1;
            assert!(gap < Duration::from_millis(1400), "{gap:?}");
        }
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore = "needs an audio output device"]
    fn a_restored_track_lines_up_the_next_one() {
        let (mut player, paths) = playing_queue(2, 1.6);
        player.queue.jump(0);
        player.load_paused_at(&paths[0], 0.5).unwrap();
        assert!(player.prefetched_next.is_some(), "gapless");

        player.set_crossfade_duration(0.5);
        player.load_paused_at(&paths[0], 0.5).unwrap();
        assert!(player.crossfade_has_next(), "crossfade");
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    #[ignore = "needs an audio output device"]
    fn a_track_added_to_the_end_mid_song_is_lined_up() {
        let (mut player, paths) = playing_queue(1, 0.8);
        player.jump_to_queue_index(0).unwrap();
        assert!(player.prefetched_next.is_none());
        let added = tone_of(0.8);
        player.enqueue(&added);
        let order = play_out(&mut player, Duration::from_secs(4));
        assert_eq!(paths_of(&order), [&paths[0], &added]);
    }
}
