// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The background worker that fills one track's sparse cache file.

use super::ranges::RangeSet;
use crate::remote::backend::{ByteStream, RemoteBackend, RemoteError};
use crate::remote::gate::{GateState, NetworkGate};
use crate::remote::store::RemoteStreamInfo;
use crate::remote::uri::RemoteUri;
use crate::remote::{RemoteEvent, RemoteRuntime, SourceState};
use crate::sources::cache::remote_cache_path;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

const CHUNK: usize = 64 * 1024;
/// A wanted byte this close ahead of the cursor is reached by reading on
/// rather than by opening a new request.
const SEEK_AHEAD: u64 = 256 * 1024;
const SAVE_EVERY: u64 = 1024 * 1024;
const LOST_CONNECTION: &str = "Lost the connection to the server";

pub struct Fill {
    pub uri: RemoteUri,
    pub path: PathBuf,
    pub len: u64,
    state: Mutex<FillState>,
    cond: Condvar,
    gate: &'static NetworkGate,
}

struct FillState {
    ranges: RangeSet,
    /// The first missing byte a reader is waiting on.
    wanted: Option<u64>,
    done: bool,
    error: Option<String>,
    readers: usize,
}

impl std::fmt::Debug for Fill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fill")
            .field("uri", &self.uri)
            .field("path", &self.path)
            .field("len", &self.len)
            .finish()
    }
}

pub(super) enum WaitError {
    TimedOut,
    Failed(String),
}

impl From<WaitError> for String {
    fn from(error: WaitError) -> String {
        match error {
            WaitError::TimedOut => "The server is taking too long to send this song".to_string(),
            WaitError::Failed(message) => message,
        }
    }
}

enum FillError {
    Remote(RemoteError),
    Local(String),
}

impl From<RemoteError> for FillError {
    fn from(error: RemoteError) -> Self {
        FillError::Remote(error)
    }
}

impl FillError {
    fn message(&self) -> String {
        match self {
            FillError::Remote(error) => error.to_string(),
            FillError::Local(_) => "Couldn't save this song for playback".to_string(),
        }
    }
}

pub(super) fn sidecar_path(path: &Path) -> PathBuf {
    let mut sidecar = path.as_os_str().to_owned();
    sidecar.push(".ranges");
    PathBuf::from(sidecar)
}

#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match std::os::windows::fs::FileExt::seek_write(file, buf, offset) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Opens the cache file and works out which of its bytes are real. The
/// sidecar is the record of a partial file; a file without one is trusted
/// only when the store says it is complete.
fn open_cache(
    path: &Path,
    sidecar: &Path,
    len: u64,
    marked_complete: bool,
) -> io::Result<(File, RangeSet)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    let known = if file.metadata()?.len() != len {
        None
    } else if let Some(ranges) = RangeSet::load(sidecar) {
        (ranges.end() <= len).then_some(ranges)
    } else if marked_complete {
        let mut full = RangeSet::default();
        full.insert(0, len);
        Some(full)
    } else {
        None
    };
    match known {
        Some(ranges) => Ok((file, ranges)),
        None => {
            file.set_len(len)?;
            let _ = std::fs::remove_file(sidecar);
            Ok((file, RangeSet::default()))
        }
    }
}

/// A broken connection is the server's problem, not the disk's.
fn read_some(stream: &mut dyn ByteStream, buf: &mut [u8]) -> Result<usize, FillError> {
    loop {
        match stream.read(buf) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                tracing::debug!("Stream read failed: {e}");
                return Err(RemoteError::Unreachable(LOST_CONNECTION.to_string()).into());
            }
        }
    }
}

/// How a failed fill reflects on its whole source. A missing or unreadable
/// track is that track's problem, so it leaves the source alone.
fn source_state_for(error: &RemoteError) -> Option<SourceState> {
    match error {
        RemoteError::Unauthorized | RemoteError::Http(401) => Some(SourceState::SignedOut),
        RemoteError::Blocked(_) => Some(SourceState::Blocked),
        RemoteError::Http(400..=499) | RemoteError::Parse(_) => None,
        RemoteError::Http(_) | RemoteError::Unreachable(_) => Some(SourceState::Unreachable),
    }
}

impl Fill {
    /// Starts filling `info`'s cache file, resuming from its sidecar. A file
    /// that is already whole comes back complete without a worker.
    #[allow(dead_code)]
    pub fn spawn(
        rt: &'static RemoteRuntime,
        info: RemoteStreamInfo,
        backend: Arc<dyn RemoteBackend>,
    ) -> Arc<Fill> {
        Self::spawn_with(rt, NetworkGate::global(), info, backend)
    }

    /// [`Fill::spawn`] with the gate the worker checks before every read.
    pub(crate) fn spawn_with(
        rt: &'static RemoteRuntime,
        gate: &'static NetworkGate,
        info: RemoteStreamInfo,
        backend: Arc<dyn RemoteBackend>,
    ) -> Arc<Fill> {
        let path = remote_cache_path(&info.uri.source_id, &info.uri.remote_id, &info.extension);
        let sidecar = sidecar_path(&path);
        let len = info.size;
        let marked_complete = info.cache_state.as_deref() == Some("complete");

        let (file, ranges, error) = match open_cache(&path, &sidecar, len, marked_complete) {
            Ok((file, ranges)) => (Some(file), ranges, None),
            Err(e) => {
                let error = FillError::Local(e.to_string());
                tracing::warn!("Could not open {}: {e}", path.display());
                (None, RangeSet::default(), Some(error.message()))
            }
        };
        let whole = file.is_some() && marked_complete && ranges.covers(len) && !sidecar.exists();
        let fill = Arc::new(Fill {
            uri: info.uri.clone(),
            path,
            len,
            state: Mutex::new(FillState {
                ranges,
                wanted: None,
                done: whole,
                error,
                readers: 0,
            }),
            cond: Condvar::new(),
            gate,
        });
        let Some(file) = file.filter(|_| !whole) else {
            return fill;
        };

        if info.cache_state.as_deref() != Some("partial") {
            // Recorded up front so the orphan sweep keeps the file if the app
            // stops before the fill does, and so a stale `complete` never
            // vouches for a file that is mostly holes.
            let marked = rt.with_library(|l| l.set_remote_cache_state(&info.uri, Some("partial")));
            if let Err(e) = marked {
                tracing::warn!("Could not mark a stream as partial: {e}");
            }
        }

        let worker = fill.clone();
        let part_key = info.part_key;
        let spawned = std::thread::Builder::new()
            .name("wave-stream-fill".into())
            .spawn(move || worker.run(rt, backend.as_ref(), &part_key, &file, &sidecar));
        if let Err(e) = spawned {
            let error = FillError::Local(e.to_string());
            tracing::warn!("Could not start a stream fill: {e}");
            fill.lock().error = Some(error.message());
        }
        fill
    }

    fn lock(&self) -> MutexGuard<'_, FillState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Blocks until `min` bytes (fewer at the end of the file) are available
    /// at `pos`, and returns how many are.
    pub fn wait_for(&self, pos: u64, min: u64, timeout: Duration) -> Result<u64, String> {
        self.wait(pos, min, timeout).map_err(String::from)
    }

    pub(super) fn wait(&self, pos: u64, min: u64, timeout: Duration) -> Result<u64, WaitError> {
        if pos >= self.len {
            return Ok(0);
        }
        let need = min.min(self.len - pos);
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            let available = state.ranges.contiguous_from(pos);
            if available >= need {
                return Ok(available);
            }
            if let Some(error) = &state.error {
                return Err(WaitError::Failed(error.clone()));
            }
            state.wanted = Some(pos + available);
            let now = Instant::now();
            if now >= deadline {
                return Err(WaitError::TimedOut);
            }
            state = self
                .cond
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    pub fn is_complete(&self) -> bool {
        self.lock().done
    }

    pub fn contiguous_from(&self, pos: u64) -> u64 {
        self.lock().ranges.contiguous_from(pos)
    }

    pub fn error(&self) -> Option<String> {
        self.lock().error.clone()
    }

    #[allow(dead_code)]
    pub fn readers(&self) -> usize {
        self.lock().readers
    }

    pub(super) fn add_reader(&self) {
        self.lock().readers += 1;
    }

    #[allow(dead_code)]
    pub(super) fn remove_reader(&self) {
        let mut state = self.lock();
        state.readers = state.readers.saturating_sub(1);
    }

    /// Still downloading, or held open by a reader.
    pub(super) fn is_active(&self) -> bool {
        let state = self.lock();
        state.readers > 0 || (!state.done && state.error.is_none())
    }

    fn run(
        &self,
        rt: &RemoteRuntime,
        backend: &dyn RemoteBackend,
        part_key: &str,
        file: &File,
        sidecar: &Path,
    ) {
        match self.work(rt, backend, part_key, file, sidecar) {
            Ok(()) => self.complete(rt, file, sidecar),
            Err(error) => self.fail(rt, file, sidecar, error),
        }
    }

    /// Fills gaps from the first one onward, jumping to a reader's wanted byte
    /// when it is behind the cursor or too far ahead to read on to. The state
    /// lock is never held across network or disk I/O.
    fn work(
        &self,
        rt: &RemoteRuntime,
        backend: &dyn RemoteBackend,
        part_key: &str,
        file: &File,
        sidecar: &Path,
    ) -> Result<(), FillError> {
        let Some(mut cursor) = self.first_gap() else {
            return Ok(());
        };
        let mut response: Option<Box<dyn ByteStream>> = None;
        let mut buf = vec![0u8; CHUNK];
        let mut unsaved = 0u64;
        loop {
            self.gate.check()?;
            let limit = {
                let mut state = self.lock();
                if let Some(wanted) = state.wanted {
                    if wanted >= self.len || state.ranges.contiguous_from(wanted) > 0 {
                        state.wanted = None;
                    } else if wanted < cursor || wanted > cursor + SEEK_AHEAD {
                        response = None;
                        cursor = wanted;
                    }
                }
                state.ranges.next_start_after(cursor).unwrap_or(self.len)
            };

            let fresh = response.is_none();
            let stream = match &mut response {
                Some(stream) => stream,
                None => response.insert(backend.open_bytes(part_key, cursor)?),
            };
            let want = (limit - cursor).min(CHUNK as u64) as usize;
            let n = read_some(stream.as_mut(), &mut buf[..want])?;
            if n == 0 {
                if fresh {
                    let early = RemoteError::Unreachable("The server ended this song early".into());
                    return Err(early.into());
                }
                response = None;
                match self.first_gap() {
                    Some(gap) => {
                        cursor = gap;
                        continue;
                    }
                    None => return Ok(()),
                }
            }

            write_all_at(file, &buf[..n], cursor).map_err(|e| FillError::Local(e.to_string()))?;
            let reached_data = {
                let mut state = self.lock();
                state.ranges.insert(cursor, cursor + n as u64);
                cursor += n as u64;
                cursor >= self.len || state.ranges.contiguous_from(cursor) > 0
            };
            self.cond.notify_all();
            if fresh {
                self.mark_reachable(rt);
            }

            unsaved += n as u64;
            if unsaved >= SAVE_EVERY {
                unsaved = 0;
                self.save_sidecar(file, sidecar);
            }
            if reached_data {
                response = None;
                match self.first_gap() {
                    Some(gap) => cursor = gap,
                    None => return Ok(()),
                }
            }
        }
    }

    fn first_gap(&self) -> Option<u64> {
        self.lock().ranges.first_gap(0, self.len)
    }

    /// A source greyed out by an earlier failure is back once it sends audio.
    /// Syncing and SignedOut belong to other flows and are left alone.
    fn mark_reachable(&self, rt: &RemoteRuntime) {
        let source = &self.uri.source_id;
        if matches!(
            rt.status(source),
            SourceState::Unreachable | SourceState::Blocked
        ) {
            rt.set_status(source, SourceState::Ok);
        }
    }

    /// The data is flushed first, so the sidecar never claims bytes that a
    /// power cut could still lose.
    fn save_sidecar(&self, file: &File, sidecar: &Path) {
        if let Err(e) = file.sync_data() {
            tracing::warn!("Could not flush stream progress: {e}");
            return;
        }
        let ranges = self.lock().ranges.clone();
        if let Err(e) = ranges.save(sidecar) {
            tracing::warn!("Could not save stream progress: {e}");
        }
    }

    fn complete(&self, rt: &RemoteRuntime, file: &File, sidecar: &Path) {
        if let Err(e) = file.sync_all() {
            tracing::warn!("Could not flush a stream cache file: {e}");
        }
        let _ = std::fs::remove_file(sidecar);
        let enabled = self.gate.state() != GateState::Disabled;
        let stored = rt.with_library(|l| {
            l.set_remote_cache_state(&self.uri, Some("complete"))?;
            l.recompute_remote_visibility(enabled)
        });
        if let Err(e) = stored {
            tracing::warn!("Could not record a finished stream: {e}");
        }
        self.lock().done = true;
        self.cond.notify_all();
    }

    /// The file and sidecar stay so the next fill resumes where this one
    /// stopped.
    fn fail(&self, rt: &RemoteRuntime, file: &File, sidecar: &Path, error: FillError) {
        self.save_sidecar(file, sidecar);
        let marked = rt.with_library(|l| l.set_remote_cache_state(&self.uri, Some("partial")));
        if let Err(e) = marked {
            tracing::warn!("Could not mark a stream as partial: {e}");
        }
        match &error {
            FillError::Remote(remote) => {
                if let Some(state) = source_state_for(remote) {
                    rt.set_status(&self.uri.source_id, state);
                }
            }
            FillError::Local(detail) => {
                tracing::warn!("Could not write {}: {detail}", self.path.display());
            }
        }
        let message = error.message();
        let readers = {
            let mut state = self.lock();
            state.error = Some(message.clone());
            state.readers
        };
        self.cond.notify_all();
        if readers > 0 {
            rt.emit(RemoteEvent::StreamFailed {
                path: self.uri.to_path(),
                message,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::reader::ProgressiveReader;
    use super::super::testkit::*;
    use crate::remote::backend::RemoteError;
    use crate::remote::{RemoteEvent, SourceState};
    use std::sync::Arc;
    use std::time::Duration;

    const LONG: Duration = Duration::from_secs(20);

    #[test]
    fn interrupted_fill_resumes_from_sidecar() {
        let h = Harness::new(3 * MIB as usize);
        let first = Arc::new(FakeBackend::new(h.data.clone(), FAST).failing_at(MIB));
        let fill = h.spawn(first);
        assert!(wait_until(LONG, || fill.error().is_some()));
        assert_eq!(fill.contiguous_from(0), MIB);
        assert!(!fill.is_complete());
        assert!(h.sidecar().exists());
        assert_eq!(h.info().cache_state.as_deref(), Some("partial"));
        assert_eq!(h.rt.status(&h.uri.source_id), SourceState::Unreachable);
        drop(fill);

        let second = Arc::new(FakeBackend::new(h.data.clone(), FAST));
        let fill = h.spawn(second.clone());
        assert!(wait_until(LONG, || fill.is_complete()));
        assert_eq!(second.opens().first(), Some(&MIB));
        assert!(std::fs::read(h.cache_path()).unwrap() == *h.data);
    }

    #[test]
    fn completion_removes_sidecar_and_marks_complete() {
        let h = Harness::new(3 * MIB as usize);
        let fill = h.spawn(Arc::new(FakeBackend::new(h.data.clone(), FAST)));
        assert!(wait_until(LONG, || fill.is_complete()));
        assert_eq!(h.info().cache_state.as_deref(), Some("complete"));
        assert!(!h.sidecar().exists());
        assert!(std::fs::read(h.cache_path()).unwrap() == *h.data);
        assert_eq!(fill.wait_for(0, u64::MAX, Duration::ZERO), Ok(3 * MIB));

        let idle = Arc::new(FakeBackend::new(h.data.clone(), FAST));
        let again = h.spawn(idle.clone());
        assert!(again.is_complete());
        assert!(idle.opens().is_empty());
    }

    #[test]
    fn a_right_length_file_without_a_sidecar_starts_over() {
        for state in [None, Some("partial")] {
            let h = Harness::new(MIB as usize);
            std::fs::create_dir_all(h.cache_path().parent().unwrap()).unwrap();
            std::fs::write(h.cache_path(), vec![0xAA; MIB as usize]).unwrap();
            h.rt.with_library(|l| l.set_remote_cache_state(&h.uri, state))
                .unwrap();

            let backend = Arc::new(FakeBackend::new(h.data.clone(), FAST));
            let fill = h.spawn(backend.clone());
            assert_eq!(fill.contiguous_from(0), 0, "state {state:?}");
            assert!(wait_until(LONG, || fill.is_complete()));
            assert_eq!(backend.opens().first(), Some(&0));
            assert!(std::fs::read(h.cache_path()).unwrap() == *h.data);
        }
    }

    #[test]
    fn a_working_stream_clears_an_unreachable_or_blocked_source() {
        for state in [SourceState::Unreachable, SourceState::Blocked] {
            let h = Harness::new(MIB as usize);
            h.rt.set_status(&h.uri.source_id, state);
            let fill = h.spawn(Arc::new(FakeBackend::new(h.data.clone(), FAST)));
            assert!(wait_until(LONG, || h.rt.status(&h.uri.source_id) == SourceState::Ok));
            assert!(wait_until(LONG, || fill.is_complete()));
        }
    }

    #[test]
    fn a_working_stream_leaves_syncing_and_signed_out_alone() {
        for state in [SourceState::Syncing, SourceState::SignedOut] {
            let h = Harness::new(MIB as usize);
            h.rt.set_status(&h.uri.source_id, state);
            let fill = h.spawn(Arc::new(FakeBackend::new(h.data.clone(), FAST)));
            assert!(wait_until(LONG, || fill.is_complete()));
            assert_eq!(h.rt.status(&h.uri.source_id), state);
        }
    }

    #[test]
    fn a_track_level_failure_leaves_the_source_alone() {
        let cases = [
            (
                RemoteError::Http(404),
                "The server answered with an error (404)",
            ),
            (
                RemoteError::Parse("bad".into()),
                "Unexpected response from the server",
            ),
        ];
        for (error, message) in cases {
            let h = Harness::new(MIB as usize);
            let fill = h.spawn(Arc::new(
                FakeBackend::new(h.data.clone(), Duration::from_millis(300)).refusing(error),
            ));
            let reader = ProgressiveReader::new(fill.clone(), LONG).unwrap();
            assert_eq!(fill.wait_for(0, 1, LONG).unwrap_err(), message);
            assert_eq!(h.rt.status(&h.uri.source_id), SourceState::Ok);
            assert_eq!(h.info().cache_state.as_deref(), Some("partial"));
            assert!(wait_until(LONG, || h.events().contains(
                &RemoteEvent::StreamFailed {
                    path: h.uri.to_path(),
                    message: message.into(),
                }
            )));
            drop(reader);
        }

        let h = Harness::new(MIB as usize);
        let fill = h.spawn(Arc::new(
            FakeBackend::new(h.data.clone(), FAST).refusing(RemoteError::Http(503)),
        ));
        assert!(fill.wait_for(0, 1, LONG).is_err());
        assert_eq!(h.rt.status(&h.uri.source_id), SourceState::Unreachable);
    }

    #[test]
    fn closing_the_gate_stops_the_fill() {
        let h = Harness::new(3 * MIB as usize);
        let backend = Arc::new(FakeBackend::new(h.data.clone(), SLOW));
        let fill = h.spawn(backend.clone());
        assert!(fill.wait_for(0, 64 * 1024, LONG).is_ok());

        h.gate.set_policy(false, true);
        assert!(wait_until(LONG, || fill.error().is_some()));
        assert_eq!(fill.error().as_deref(), Some("Outside Sourcing is off"));
        let reads = backend.reads();
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(backend.reads(), reads);
        assert!(!fill.is_complete());
        assert!(fill.contiguous_from(0) < 3 * MIB);
        assert!(h.sidecar().exists());
        assert_eq!(h.info().cache_state.as_deref(), Some("partial"));
        assert_eq!(h.rt.status(&h.uri.source_id), SourceState::Blocked);
    }

    #[test]
    fn wait_for_reports_a_fill_error() {
        let h = Harness::new(MIB as usize);
        let fill = h.spawn(Arc::new(
            FakeBackend::new(h.data.clone(), FAST).failing_at(64 * 1024),
        ));
        let err = fill.wait_for(512 * 1024, 1, LONG).unwrap_err();
        assert_eq!(err, "Lost the connection to the server");
    }
}
