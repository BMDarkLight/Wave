// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Remote audio as a sparse cache file per track. A [`Fill`] downloads into
//! the file in the background, seeking ahead when a reader asks for bytes far
//! from where it is, and a [`reader::ProgressiveReader`] reads it while it fills.

pub mod fill;
pub mod loopback;
pub mod ranges;
pub mod reader;

pub use fill::Fill;

use crate::remote::gate::NetworkGate;
use crate::remote::store::RemoteStreamInfo;
use crate::remote::uri::{is_remote, RemoteUri};
use crate::remote::RemoteRuntime;
use crate::sources::cache::{remote_cache_path, remote_cache_path_in, remote_cache_root};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

/// Bytes buffered at the start of a track before playback begins.
pub const READY_BYTES: u64 = 512 * 1024;

const NOT_SET_UP: &str = "Remote music isn't set up";

#[derive(Debug)]
pub enum PlayTarget {
    File(String),
    Progressive(Arc<Fill>, String /* extension */),
}

/// Live fills by track. A fill lives as long as its worker or a holder does.
fn registry() -> MutexGuard<'static, HashMap<RemoteUri, Weak<Fill>>> {
    static FILLS: OnceLock<Mutex<HashMap<RemoteUri, Weak<Fill>>>> = OnceLock::new();
    FILLS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn live_fill(uri: &RemoteUri) -> Option<Arc<Fill>> {
    registry().get(uri).and_then(Weak::upgrade)
}

fn lookup(rt: &RemoteRuntime, uri: &RemoteUri) -> Result<RemoteStreamInfo, String> {
    rt.with_library(|l| l.remote_stream_info(uri))?
        .ok_or_else(|| "This song isn't in your library anymore".to_string())
}

/// The cache file, when the store calls it complete and it is all there.
fn complete_file(info: &RemoteStreamInfo) -> Option<PathBuf> {
    if info.cache_state.as_deref() != Some("complete") {
        return None;
    }
    let path = remote_cache_path(&info.uri.source_id, &info.uri.remote_id, &info.extension);
    let len = std::fs::metadata(&path).ok()?.len();
    (len == info.size).then_some(path)
}

/// Joins the track's running fill or starts one. Without network access only
/// a complete file can be had.
#[allow(dead_code)]
pub fn fill_for(uri: &RemoteUri) -> Result<Arc<Fill>, String> {
    let rt = RemoteRuntime::global().ok_or(NOT_SET_UP)?;
    fill_for_with(rt, NetworkGate::global(), uri)
}

pub(crate) fn fill_for_with(
    rt: &'static RemoteRuntime,
    gate: &'static NetworkGate,
    uri: &RemoteUri,
) -> Result<Arc<Fill>, String> {
    let mut fills = registry();
    fills.retain(|_, fill| fill.strong_count() > 0);
    // A fill that failed is replaced, and the new one resumes from its sidecar.
    if let Some(fill) = fills.get(uri).and_then(Weak::upgrade) {
        if fill.error().is_none() {
            return Ok(fill);
        }
    }
    let info = lookup(rt, uri)?;
    if info.size == 0 {
        return Err("The server didn't say how big this song is".to_string());
    }
    if complete_file(&info).is_none() {
        gate.check().map_err(|e| e.to_string())?;
    }
    let backend = rt
        .backend(&uri.source_id)
        .ok_or("This server is not set up")?;
    let fill = Fill::spawn_with(rt, gate, info, backend);
    if let Some(error) = fill.error() {
        return Err(error);
    }
    fills.insert(uri.clone(), Arc::downgrade(&fill));
    Ok(fill)
}

/// What the player should open for `path`: a downloaded or fully cached file,
/// or a fill to read progressively. Paths that are not remote are files.
pub fn resolve_play_target(path: &str) -> Result<PlayTarget, String> {
    let rt = RemoteRuntime::global().ok_or(NOT_SET_UP)?;
    resolve_play_target_with(rt, NetworkGate::global(), path)
}

pub(crate) fn resolve_play_target_with(
    rt: &'static RemoteRuntime,
    gate: &'static NetworkGate,
    path: &str,
) -> Result<PlayTarget, String> {
    if !is_remote(path) {
        return Ok(PlayTarget::File(path.to_string()));
    }
    let uri = RemoteUri::parse(path).ok_or("This song's address is not valid")?;
    let info = lookup(rt, &uri)?;
    if let Some(local) = info.path_if_downloaded {
        return Ok(PlayTarget::File(local));
    }
    if let Some(cached) = complete_file(&info) {
        return Ok(PlayTarget::File(cached.to_string_lossy().into_owned()));
    }
    let fill = fill_for_with(rt, gate, &uri)?;
    Ok(PlayTarget::Progressive(fill, info.extension))
}

/// Blocks until a remote track has enough bytes buffered to start.
pub fn prepare_for_play(path: &str, timeout: Duration) -> Result<(), String> {
    if !is_remote(path) {
        return Ok(());
    }
    let rt = RemoteRuntime::global().ok_or(NOT_SET_UP)?;
    prepare_for_play_with(rt, NetworkGate::global(), path, timeout)
}

fn prepare_for_play_with(
    rt: &'static RemoteRuntime,
    gate: &'static NetworkGate,
    path: &str,
    timeout: Duration,
) -> Result<(), String> {
    if !is_remote(path) {
        return Ok(());
    }
    match resolve_play_target_with(rt, gate, path)? {
        PlayTarget::File(_) => Ok(()),
        PlayTarget::Progressive(fill, _) => fill.wait_for(0, READY_BYTES, timeout).map(|_| ()),
    }
}

/// Whether `path` could start playing now. Never starts a fill.
pub fn is_ready(path: &str) -> bool {
    if !is_remote(path) {
        return true;
    }
    RemoteRuntime::global().is_some_and(|rt| is_ready_with(rt, path))
}

fn is_ready_with(rt: &RemoteRuntime, path: &str) -> bool {
    if !is_remote(path) {
        return true;
    }
    let Some(uri) = RemoteUri::parse(path) else {
        return false;
    };
    if let Some(fill) = live_fill(&uri) {
        if fill.is_complete() || fill.contiguous_from(0) >= READY_BYTES.min(fill.len) {
            return true;
        }
    }
    lookup(rt, &uri)
        .is_ok_and(|info| info.path_if_downloaded.is_some() || complete_file(&info).is_some())
}

/// Starts buffering a remote track in the background, when the network allows.
pub fn warm(path: &str) {
    if !is_remote(path) {
        return;
    }
    let path = path.to_string();
    let spawned = std::thread::Builder::new()
        .name("wave-stream-warm".into())
        .spawn(move || {
            if let Err(e) = resolve_play_target(&path) {
                tracing::debug!("Could not warm {path}: {e}");
            }
        });
    if let Err(e) = spawned {
        tracing::debug!("Could not start warming a stream: {e}");
    }
}

/// Cache files in use, which eviction must leave alone.
pub fn active_paths() -> Vec<PathBuf> {
    registry()
        .values()
        .filter_map(Weak::upgrade)
        .filter(|fill| fill.is_active())
        .map(|fill| fill.path.clone())
        .collect()
}

/// A file that can be read for `path`: the path itself for a local track, or
/// a remote track's finished cache file. `None` while a remote track is not
/// fully downloaded.
pub fn local_file_for(path: &str) -> Option<String> {
    if !is_remote(path) {
        return Some(path.to_string());
    }
    cache_file_if_complete(path)
}

/// The finished cache file behind a remote path, for code that needs a real
/// file rather than a stream.
pub fn cache_file_if_complete(path: &str) -> Option<String> {
    let rt = RemoteRuntime::global()?;
    cache_file_if_complete_with(rt, path)
}

fn cache_file_if_complete_with(rt: &RemoteRuntime, path: &str) -> Option<String> {
    let uri = RemoteUri::parse(path)?;
    let info = lookup(rt, &uri).ok()?;
    complete_file(&info).map(|file| file.to_string_lossy().into_owned())
}

/// A track's cache file and its `.ranges` sidecar.
pub fn remote_cache_files_for(uri: &RemoteUri, extension: &str) -> [PathBuf; 2] {
    remote_cache_files_in(&remote_cache_root(), uri, extension)
}

fn remote_cache_files_in(root: &Path, uri: &RemoteUri, extension: &str) -> [PathBuf; 2] {
    let file = remote_cache_path_in(root, &uri.source_id, &uri.remote_id, extension);
    let sidecar = fill::sidecar_path(&file);
    [file, sidecar]
}

/// Deletes every file under the remote cache that no cached row accounts for,
/// and returns how many went. The expected names are rebuilt from the rows
/// because sanitized names cannot be mapped back to tracks.
pub fn sweep_orphans(rt: &RemoteRuntime) -> std::io::Result<usize> {
    sweep_orphans_in(rt, &remote_cache_root())
}

fn sweep_orphans_in(rt: &RemoteRuntime, root: &Path) -> std::io::Result<usize> {
    if !root.is_dir() {
        return Ok(0);
    }
    let rows = rt
        .with_library(|l| l.remote_cached_files())
        .map_err(std::io::Error::other)?;
    let mut keep: HashSet<PathBuf> = rows
        .iter()
        .flat_map(|(uri, extension)| remote_cache_files_in(root, uri, extension))
        .collect();
    for path in active_paths() {
        keep.insert(fill::sidecar_path(&path));
        keep.insert(path);
    }
    let mut removed = 0;
    sweep_dir(root, &keep, &mut removed)?;
    Ok(removed)
}

fn sweep_dir(dir: &Path, keep: &HashSet<PathBuf>, removed: &mut usize) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            sweep_dir(&path, keep, removed)?;
            // Fails, harmlessly, while the directory still holds kept files.
            let _ = std::fs::remove_dir(&path);
        } else if !keep.contains(&path) {
            std::fs::remove_file(&path)?;
            *removed += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod testkit {
    use super::*;
    use crate::library::Library;
    use crate::remote::backend::{
        ByteStream, CatalogPage, RemoteBackend, RemoteError, RemoteTrackMeta,
    };
    use crate::remote::store::RemoteStreamInfo;
    use crate::remote::RemoteEvent;
    use crate::sources::cache::{remote_cache_root, sanitize};
    use std::io::{self, Read};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Instant;
    use uuid::Uuid;

    pub const MIB: u64 = 1024 * 1024;
    const PIECE: usize = 16 * 1024;
    /// About 256 KiB/s.
    pub const SLOW: Duration = Duration::from_millis(62);
    pub const FAST: Duration = Duration::from_millis(2);

    pub fn sample(len: usize) -> Arc<Vec<u8>> {
        let mut x: u32 = 0x9e37_79b9;
        Arc::new(
            (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect(),
        )
    }

    /// Stereo 16-bit 44.1 kHz PCM.
    pub fn wav_bytes(frames: usize) -> Vec<u8> {
        let data_len = (frames * 4) as u32;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&44_100u32.to_le_bytes());
        out.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            let sample = ((i as f32 * 0.05).sin() * 8000.0) as i16;
            out.extend_from_slice(&sample.to_le_bytes());
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }

    /// Serves `data` in 16 KiB pieces with a pause before each.
    pub struct FakeBackend {
        data: Arc<Vec<u8>>,
        delay: Duration,
        fail_at: Option<u64>,
        silent: bool,
        refuse: Option<RemoteError>,
        opens: Mutex<Vec<u64>>,
        reads: Arc<AtomicUsize>,
    }

    impl FakeBackend {
        pub fn new(data: Arc<Vec<u8>>, delay: Duration) -> Self {
            Self {
                data,
                delay,
                fail_at: None,
                silent: false,
                refuse: None,
                opens: Mutex::new(Vec::new()),
                reads: Arc::default(),
            }
        }

        /// Every stream breaks once it reaches `at`.
        pub fn failing_at(mut self, at: u64) -> Self {
            self.fail_at = Some(at);
            self
        }

        /// Streams open but send nothing for seconds, then break.
        pub fn silent(mut self) -> Self {
            self.silent = true;
            self
        }

        /// Every `open_bytes` fails with `error`.
        pub fn refusing(mut self, error: RemoteError) -> Self {
            self.refuse = Some(error);
            self
        }

        pub fn opens(&self) -> Vec<u64> {
            self.opens.lock().unwrap().clone()
        }

        /// Reads that delivered bytes, across every stream.
        pub fn reads(&self) -> usize {
            self.reads.load(Ordering::SeqCst)
        }
    }

    struct FakeStream {
        data: Arc<Vec<u8>>,
        pos: usize,
        delay: Duration,
        fail_at: Option<usize>,
        silent: bool,
        reads: Arc<AtomicUsize>,
    }

    impl Read for FakeStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.silent {
                std::thread::sleep(Duration::from_secs(3));
                return Err(io::Error::new(io::ErrorKind::TimedOut, "silent"));
            }
            let stop = self.fail_at.unwrap_or(usize::MAX).min(self.data.len());
            if self.fail_at.is_some_and(|at| self.pos >= at) {
                return Err(io::Error::new(io::ErrorKind::ConnectionReset, "reset"));
            }
            std::thread::sleep(self.delay);
            let n = buf.len().min(PIECE).min(stop.saturating_sub(self.pos));
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(n)
        }
    }

    impl RemoteBackend for FakeBackend {
        fn kind(&self) -> &'static str {
            "plex"
        }

        fn catalog_page(&self, _: usize) -> Result<CatalogPage, RemoteError> {
            Err(RemoteError::Http(404))
        }

        fn open_bytes(&self, _: &str, offset: u64) -> Result<Box<dyn ByteStream>, RemoteError> {
            self.opens.lock().unwrap().push(offset);
            if let Some(error) = &self.refuse {
                std::thread::sleep(self.delay);
                return Err(error.clone());
            }
            Ok(Box::new(FakeStream {
                data: self.data.clone(),
                pos: offset as usize,
                delay: self.delay,
                fail_at: self.fail_at.map(|at| at as usize),
                silent: self.silent,
                reads: self.reads.clone(),
            }))
        }

        fn check_reachable(&self) -> Result<(), RemoteError> {
            Ok(())
        }

        fn fetch_artwork(&self, _: &str) -> Result<Vec<u8>, RemoteError> {
            Err(RemoteError::Http(404))
        }
    }

    /// One remote track of `len` bytes in its own source and library, behind
    /// its own open gate. Its cache directory is removed on drop.
    pub struct Harness {
        pub rt: &'static RemoteRuntime,
        pub gate: &'static NetworkGate,
        pub uri: RemoteUri,
        pub data: Arc<Vec<u8>>,
        events: Arc<Mutex<Vec<RemoteEvent>>>,
    }

    impl Harness {
        pub fn new(len: usize) -> Self {
            let dir = std::env::temp_dir().join(format!("wave-test-stream-{}", Uuid::new_v4()));
            let library = Library::new_with_path(&dir.join("library.db")).unwrap();
            let source = library
                .insert_remote_source("plex", &Uuid::new_v4().to_string(), "Home", "{}")
                .unwrap();
            let meta = RemoteTrackMeta {
                remote_id: "1".into(),
                title: "Song".into(),
                artist: "Band".into(),
                album_artist: None,
                album: "Album".into(),
                track_number: Some(1),
                disc_number: None,
                year: None,
                genre: None,
                duration_seconds: Some(10.0),
                size: len as u64,
                extension: "flac".into(),
                part_key: "/library/parts/1/file.flac".into(),
                art_ref: None,
                updated_at: Some(1),
            };
            let mut walk = library.begin_remote_walk(&source).unwrap();
            library
                .upsert_remote_page(&mut walk, "plex", &[meta])
                .unwrap();
            library.finish_remote_walk(walk).unwrap();

            let events: Arc<Mutex<Vec<RemoteEvent>>> = Arc::default();
            let sink = events.clone();
            let rt: &'static RemoteRuntime = Box::leak(Box::new(RemoteRuntime::new_for_test(
                library,
                Box::new(move |e| sink.lock().unwrap().push(e)),
            )));
            Self {
                rt,
                gate: gate(true),
                uri: RemoteUri::new(&source, "1"),
                data: sample(len),
                events,
            }
        }

        pub fn info(&self) -> RemoteStreamInfo {
            self.rt
                .with_library(|l| l.remote_stream_info(&self.uri))
                .unwrap()
                .unwrap()
        }

        pub fn cache_path(&self) -> PathBuf {
            remote_cache_files_for(&self.uri, "flac")[0].clone()
        }

        pub fn sidecar(&self) -> PathBuf {
            remote_cache_files_for(&self.uri, "flac")[1].clone()
        }

        pub fn events(&self) -> Vec<RemoteEvent> {
            self.events.lock().unwrap().clone()
        }

        pub fn spawn(&self, backend: Arc<FakeBackend>) -> Arc<Fill> {
            self.rt
                .insert_backend_for_test(&self.uri.source_id, backend.clone());
            Fill::spawn_with(self.rt, self.gate, self.info(), backend)
        }
    }

    impl Drop for Harness {
        /// Closing the gate stops this harness's workers within one read; a
        /// sidecar one of them saves while the directory goes is removed on
        /// the next pass.
        fn drop(&mut self) {
            self.gate.set_policy(false, true);
            let dir = remote_cache_root().join(sanitize(&self.uri.source_id));
            for _ in 0..20 {
                let _ = std::fs::remove_dir_all(&dir);
                std::thread::sleep(Duration::from_millis(25));
                if !dir.exists() {
                    break;
                }
            }
        }
    }

    pub fn gate(open: bool) -> &'static NetworkGate {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(open, true);
        gate
    }

    pub fn wait_until(timeout: Duration, done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        done()
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use crate::sources::cache::{remote_cache_path, sanitize};
    use uuid::Uuid;

    const LONG: Duration = Duration::from_secs(20);

    #[test]
    fn local_file_for_passes_local_paths_and_waits_for_remote_files() {
        assert_eq!(
            local_file_for("/music/a.flac").as_deref(),
            Some("/music/a.flac")
        );
        assert_eq!(local_file_for("wave-remote://nowhere/1"), None);
    }

    #[test]
    fn closed_gate_refuses_uncached_but_allows_complete() {
        let h = Harness::new(MIB as usize);
        let backend = Arc::new(FakeBackend::new(h.data.clone(), FAST));
        h.rt.insert_backend_for_test(&h.uri.source_id, backend.clone());
        let closed = gate(false);
        let path = h.uri.to_path();

        assert_eq!(
            fill_for_with(h.rt, closed, &h.uri).unwrap_err(),
            "Outside Sourcing is off"
        );
        assert!(resolve_play_target_with(h.rt, closed, &path).is_err());
        assert!(!h.cache_path().exists());
        assert_eq!(cache_file_if_complete_with(h.rt, &path), None);

        std::fs::create_dir_all(h.cache_path().parent().unwrap()).unwrap();
        std::fs::write(h.cache_path(), &*h.data).unwrap();
        h.rt.with_library(|l| l.set_remote_cache_state(&h.uri, Some("complete")))
            .unwrap();

        match resolve_play_target_with(h.rt, closed, &path).unwrap() {
            PlayTarget::File(file) => assert_eq!(PathBuf::from(file), h.cache_path()),
            other => panic!("expected the cache file, got {other:?}"),
        }
        assert_eq!(
            cache_file_if_complete_with(h.rt, &path).map(PathBuf::from),
            Some(h.cache_path())
        );
        let fill = fill_for_with(h.rt, closed, &h.uri).unwrap();
        assert!(fill.is_complete());
        assert!(backend.opens().is_empty());
    }

    #[test]
    fn uncached_track_resolves_to_a_progressive_fill() {
        let h = Harness::new(MIB as usize);
        h.rt.insert_backend_for_test(
            &h.uri.source_id,
            Arc::new(FakeBackend::new(h.data.clone(), FAST)),
        );
        match resolve_play_target_with(h.rt, h.gate, &h.uri.to_path()).unwrap() {
            PlayTarget::Progressive(fill, extension) => {
                assert_eq!(extension, "flac");
                assert_eq!(fill.path, h.cache_path());
                assert!(wait_until(LONG, || fill.is_complete()));
            }
            other => panic!("expected a fill, got {other:?}"),
        }
        assert!(matches!(
            resolve_play_target_with(h.rt, h.gate, "/music/a.flac"),
            Ok(PlayTarget::File(p)) if p == "/music/a.flac"
        ));
    }

    #[test]
    fn a_complete_row_without_its_file_streams_again_as_partial() {
        let h = Harness::new(MIB as usize);
        h.rt.insert_backend_for_test(
            &h.uri.source_id,
            Arc::new(FakeBackend::new(h.data.clone(), SLOW).failing_at(64 * 1024)),
        );
        h.rt.with_library(|l| l.set_remote_cache_state(&h.uri, Some("complete")))
            .unwrap();

        let target = resolve_play_target_with(h.rt, h.gate, &h.uri.to_path()).unwrap();
        let PlayTarget::Progressive(fill, _) = target else {
            panic!("a missing file cannot play from disk");
        };
        assert_eq!(h.info().cache_state.as_deref(), Some("partial"));
        assert!(!is_ready_with(h.rt, &h.uri.to_path()));
        assert_eq!(cache_file_if_complete_with(h.rt, &h.uri.to_path()), None);
        assert!(wait_until(LONG, || fill.error().is_some()));
    }

    #[test]
    fn fill_for_joins_a_live_fill_and_replaces_a_failed_one() {
        let h = Harness::new(MIB as usize);
        h.rt.insert_backend_for_test(
            &h.uri.source_id,
            Arc::new(FakeBackend::new(h.data.clone(), SLOW).failing_at(256 * 1024)),
        );
        let open = h.gate;
        let first = fill_for_with(h.rt, open, &h.uri).unwrap();
        let joined = fill_for_with(h.rt, open, &h.uri).unwrap();
        assert!(Arc::ptr_eq(&first, &joined));
        assert!(active_paths().contains(&h.cache_path()));

        assert!(wait_until(LONG, || first.error().is_some()));
        assert!(!active_paths().contains(&h.cache_path()));
        let replaced = fill_for_with(h.rt, open, &h.uri).unwrap();
        assert!(!Arc::ptr_eq(&first, &replaced));
        assert!(wait_until(LONG, || replaced.error().is_some()));
    }

    #[test]
    fn prepare_waits_for_ready_bytes() {
        let h = Harness::new(2 * MIB as usize);
        h.rt.insert_backend_for_test(
            &h.uri.source_id,
            Arc::new(FakeBackend::new(h.data.clone(), FAST)),
        );
        let path = h.uri.to_path();
        assert!(!is_ready_with(h.rt, &path));
        prepare_for_play_with(h.rt, h.gate, &path, LONG).unwrap();
        assert!(is_ready_with(h.rt, &path));
        assert!(prepare_for_play_with(h.rt, gate(false), "/music/a.flac", LONG).is_ok());
        assert!(is_ready_with(h.rt, "/music/a.flac"));
    }

    #[test]
    fn sweep_removes_orphans_and_keeps_cached_rows() {
        let h = Harness::new(1024);
        let root = std::env::temp_dir().join(format!("wave-test-sweep-{}", Uuid::new_v4()));
        h.rt.with_library(|l| l.set_remote_cache_state(&h.uri, Some("partial")))
            .unwrap();
        let [file, sidecar] = remote_cache_files_in(&root, &h.uri, "flac");
        let orphan = root.join("plex-gone").join("9.flac");
        let stray = root.join(sanitize(&h.uri.source_id)).join("2.mp3");
        for path in [&file, &sidecar, &orphan, &stray] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }

        assert_eq!(sweep_orphans_in(h.rt, &root).unwrap(), 2);
        assert!(file.exists() && sidecar.exists());
        assert!(!orphan.exists() && !stray.exists());
        assert!(!root.join("plex-gone").exists());
        assert_eq!(sweep_orphans_in(h.rt, &root.join("missing")).unwrap(), 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cache_files_are_the_file_and_its_sidecar() {
        let uri = RemoteUri::new("plex-a", "7");
        let [file, sidecar] = remote_cache_files_for(&uri, "mp3");
        assert_eq!(file, remote_cache_path("plex-a", "7", "mp3"));
        assert_eq!(
            sidecar.file_name().unwrap().to_string_lossy(),
            "7.mp3.ranges"
        );
    }
}
