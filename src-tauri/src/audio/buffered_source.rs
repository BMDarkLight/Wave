// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Decodes on a worker thread into a bounded buffer, so a decoder that waits
//! on the network never holds up the audio callback.

use super::symphonia_source::SymphoniaSource;
use rodio::source::SeekError;
use rodio::Source;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// Output time spent on silence while waiting for audio, in microseconds.
pub type SharedStall = Arc<AtomicU64>;
/// Set while the output plays silence because the decoder fell behind.
pub type SharedBuffering = Arc<AtomicBool>;

const BUFFER_SECONDS: usize = 2;
/// Frames the worker decodes between looks at the shared state.
const CHUNK_FRAMES: usize = 1024;

#[derive(Default)]
struct State {
    samples: VecDeque<i16>,
    /// Bumped by every seek, so a chunk decoded before it is dropped.
    generation: u64,
    seek: Option<Duration>,
    /// The decoder reached the end, or failed, at the current position.
    finished: bool,
    /// The source was dropped.
    closed: bool,
    /// The worker thread is gone and will decode nothing more.
    gone: bool,
    worker_waiting: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    capacity: usize,
    chunk: usize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct BufferedSource {
    shared: Arc<Shared>,
    /// Samples taken from the shared buffer a chunk at a time, so the
    /// output locks it rarely.
    ready: VecDeque<i16>,
    stall: SharedStall,
    buffering: SharedBuffering,
    channels: u16,
    sample_rate: u32,
    total_duration: Option<Duration>,
    micros_per_sample: f64,
    /// Stall not yet added to `stall`, under a microsecond.
    stall_carry: f64,
    /// Silent samples still owed so real audio resumes on a frame boundary.
    silence_left: usize,
    /// Samples handed out in the current frame.
    phase: usize,
}

impl BufferedSource {
    pub fn spawn(source: SymphoniaSource, stall: SharedStall, buffering: SharedBuffering) -> Self {
        let channels = source.channels().max(1);
        let sample_rate = source.sample_rate().max(1);
        let total_duration = source.total_duration();
        let per_second = sample_rate as usize * channels as usize;
        let shared = Arc::new(Shared {
            state: Mutex::default(),
            changed: Condvar::new(),
            capacity: per_second * BUFFER_SECONDS,
            chunk: CHUNK_FRAMES * channels as usize,
        });

        let worker = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("wave-buffered-decode".into())
            .spawn(move || {
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    decode(source, &worker)
                }));
                if run.is_err() {
                    tracing::warn!("The stream decoder crashed");
                }
                worker.lock().gone = true;
            });
        if let Err(e) = spawned {
            tracing::warn!("Could not start the stream decoder: {e}");
            shared.lock().gone = true;
        }

        let chunk = shared.chunk;
        Self {
            shared,
            ready: VecDeque::with_capacity(chunk),
            stall,
            buffering,
            channels,
            sample_rate,
            total_duration,
            micros_per_sample: 1e6 / per_second as f64,
            stall_carry: 0.0,
            silence_left: 0,
            phase: 0,
        }
    }

    fn silence(&mut self) -> i16 {
        self.buffering.store(true, Ordering::Relaxed);
        self.stall_carry += self.micros_per_sample;
        let whole = self.stall_carry.floor();
        if whole >= 1.0 {
            self.stall.fetch_add(whole as u64, Ordering::Relaxed);
            self.stall_carry -= whole;
        }
        0
    }
}

/// The worker: decode a chunk with no lock held, then hand it over unless a
/// seek made it stale. It stays until the source is dropped, so a seek after
/// the end still gets served.
fn decode(mut source: SymphoniaSource, shared: &Shared) {
    let mut chunk = Vec::with_capacity(shared.chunk);
    loop {
        let (generation, seek) = {
            let mut state = shared.lock();
            loop {
                if state.closed {
                    return;
                }
                let room = state.samples.len() + shared.chunk <= shared.capacity;
                if state.seek.is_some() || (!state.finished && room) {
                    break;
                }
                state.worker_waiting = true;
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            state.worker_waiting = false;
            (state.generation, state.seek.take())
        };

        if let Some(position) = seek {
            if let Err(e) = source.try_seek(position) {
                tracing::warn!("Seeking a stream failed: {e}");
            }
        }

        chunk.clear();
        let mut ended = false;
        while chunk.len() < shared.chunk {
            match source.next() {
                Some(sample) => chunk.push(sample),
                None => {
                    ended = true;
                    break;
                }
            }
        }

        let mut state = shared.lock();
        if state.closed {
            return;
        }
        if state.generation != generation {
            continue;
        }
        state.samples.extend(chunk.drain(..));
        if ended {
            state.finished = true;
            if let Some(failure) = source.failure() {
                tracing::warn!("A stream stopped decoding: {failure}");
            }
        }
    }
}

impl Iterator for BufferedSource {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        let channels = self.channels as usize;
        let sample = if self.silence_left > 0 {
            self.silence_left -= 1;
            self.silence()
        } else if let Some(sample) = self.ready.pop_front() {
            sample
        } else {
            let mut state = self.shared.lock();
            let take = state.samples.len().min(self.shared.chunk);
            self.ready.extend(state.samples.drain(..take));
            if state.worker_waiting
                && state.samples.len() + self.shared.chunk <= self.shared.capacity
            {
                state.worker_waiting = false;
                self.shared.changed.notify_one();
            }
            let ended = state.gone || (state.finished && state.seek.is_none());
            drop(state);
            match self.ready.pop_front() {
                Some(sample) => {
                    self.buffering.store(false, Ordering::Relaxed);
                    sample
                }
                None if ended => {
                    self.buffering.store(false, Ordering::Relaxed);
                    return None;
                }
                None => {
                    // A whole frame of silence, so the channels stay in place.
                    self.silence_left = channels - 1;
                    self.silence()
                }
            }
        };
        self.phase = (self.phase + 1) % channels;
        Some(sample)
    }
}

impl Source for BufferedSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        {
            let mut state = self.shared.lock();
            state.samples.clear();
            state.generation += 1;
            state.seek = Some(position);
            state.finished = false;
        }
        self.shared.changed.notify_one();
        self.ready.clear();
        let channels = self.channels as usize;
        self.silence_left = (channels - self.phase) % channels;
        Ok(())
    }
}

impl Drop for BufferedSource {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.changed.notify_one();
        self.buffering.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::symphonia_source::SymphoniaSource;
    use crate::remote::stream::testkit::wav_bytes;
    use rodio::Source;
    use std::io::{self, Cursor, Read, Seek, SeekFrom};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use symphonia::core::io::MediaSource;

    const FRAMES: usize = 3 * 44_100;
    const RATE: usize = 44_100;

    /// A WAV in memory whose reads stop once, for `pause`, halfway through.
    struct StallingReader {
        inner: Cursor<Vec<u8>>,
        len: u64,
        stall_at: u64,
        pause: Duration,
        stalled: bool,
    }

    impl StallingReader {
        fn new(pause: Duration) -> Self {
            let bytes = wav_bytes(FRAMES);
            let len = bytes.len() as u64;
            Self {
                inner: Cursor::new(bytes),
                len,
                stall_at: len / 2,
                pause,
                stalled: false,
            }
        }
    }

    impl Read for StallingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let pos = self.inner.position();
            if !self.stalled && pos >= self.stall_at {
                std::thread::sleep(self.pause);
                self.stalled = true;
            }
            let limit = if self.stalled {
                buf.len()
            } else {
                buf.len().min((self.stall_at - pos) as usize)
            };
            self.inner.read(&mut buf[..limit])
        }
    }

    impl Seek for StallingReader {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    impl MediaSource for StallingReader {
        fn is_seekable(&self) -> bool {
            true
        }

        fn byte_len(&self) -> Option<u64> {
            Some(self.len)
        }
    }

    fn spawn(pause: Duration) -> (BufferedSource, SharedStall, SharedBuffering) {
        let source = SymphoniaSource::from_media_source(
            Box::new(StallingReader::new(pause)),
            Some("wav"),
            "test.wav",
        )
        .unwrap();
        let stall: SharedStall = Arc::new(AtomicU64::new(0));
        let buffering: SharedBuffering = Arc::new(AtomicBool::new(false));
        let buffered = BufferedSource::spawn(source, stall.clone(), buffering.clone());
        (buffered, stall, buffering)
    }

    fn decoded() -> Vec<i16> {
        SymphoniaSource::from_media_source(
            Box::new(Cursor::new(wav_bytes(FRAMES))),
            Some("wav"),
            "test.wav",
        )
        .unwrap()
        .collect()
    }

    /// Pulls everything, 1000 samples at a time, and checks no batch blocks.
    /// The pause between batches still pulls about ten times faster than
    /// playback, so the buffer runs dry during the stall. Batches must fit in
    /// 5 ms, but a busy machine can preempt the test thread itself, so a few
    /// may run over as long as none comes near the reader's 300 ms stall.
    fn drain(source: &mut BufferedSource, buffering: &SharedBuffering) -> (Vec<i16>, bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut out = Vec::new();
        let mut buffered_mid_stream = false;
        let mut batches = 0usize;
        let mut over_budget = 0usize;
        loop {
            assert!(Instant::now() < deadline, "never finished");
            let started = Instant::now();
            for _ in 0..1000 {
                match source.next() {
                    None => {
                        assert!(
                            over_budget * 50 <= batches,
                            "{over_budget} of {batches} batches took over 5 ms"
                        );
                        return (out, buffered_mid_stream);
                    }
                    Some(0) => {
                        if !out.is_empty() && buffering.load(Ordering::SeqCst) {
                            buffered_mid_stream = true;
                        }
                    }
                    Some(sample) => {
                        assert!(!buffering.load(Ordering::SeqCst), "still buffering");
                        out.push(sample);
                    }
                }
            }
            let elapsed = started.elapsed();
            assert!(
                elapsed < Duration::from_millis(100),
                "a batch took {elapsed:?}"
            );
            batches += 1;
            if elapsed >= Duration::from_millis(5) {
                over_budget += 1;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn underrun_yields_silence_without_blocking() {
        let (mut source, _, buffering) = spawn(Duration::from_millis(300));
        assert_eq!(source.channels(), 2);
        assert_eq!(source.sample_rate(), RATE as u32);
        assert_eq!(source.current_frame_len(), None);
        let (_, buffered_mid_stream) = drain(&mut source, &buffering);
        assert!(buffered_mid_stream, "no silence while the reader stalled");
        assert!(!buffering.load(Ordering::SeqCst));
    }

    #[test]
    fn stall_counter_grows_during_underrun() {
        let (mut source, stall, _) = spawn(Duration::from_millis(300));
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut real = 0;
        while real < FRAMES {
            assert!(Instant::now() < deadline, "never got past the stall");
            match source.next() {
                Some(0) | None => {}
                Some(_) => real += 1,
            }
        }
        assert!(stall.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn yields_all_real_samples_in_order() {
        let (mut source, _, buffering) = spawn(Duration::from_millis(300));
        let expected: Vec<i16> = decoded().into_iter().filter(|&s| s != 0).collect();
        let (got, _) = drain(&mut source, &buffering);
        assert_eq!(got.len(), expected.len());
        assert!(got == expected, "samples differ from the decoded file");
    }

    #[test]
    fn seek_drops_stale_samples() {
        let (mut source, _, _) = spawn(Duration::ZERO);
        let all = decoded();
        for _ in 0..500 {
            source.next();
        }
        std::thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        source.try_seek(Duration::from_secs(1)).unwrap();
        assert!(started.elapsed() < Duration::from_millis(50));

        let at = RATE * 2;
        assert_ne!(all[at], 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        let first = loop {
            assert!(Instant::now() < deadline, "nothing after the seek");
            match source.next() {
                Some(0) => {}
                other => break other,
            }
        };
        assert_eq!(first, Some(all[at]));
        let rest: Vec<i16> = (0..8).filter_map(|_| next_real(&mut source)).collect();
        assert_eq!(rest, all[at + 1..at + 9].to_vec());
    }

    fn next_real(source: &mut BufferedSource) -> Option<i16> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match source.next() {
                Some(0) => {}
                other => return other,
            }
        }
        None
    }
}
