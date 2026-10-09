// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Finds or computes the waveform the seek bar draws for a track.
//!
//! Saved waveforms come straight from the library. A missing one is queued
//! for a single background worker, which keeps only the latest request: when
//! someone skips through ten tracks, only the one they land on is decoded.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

/// Results kept in memory for files that are not library tracks, and failures
/// so a file that cannot be decoded is not retried on every poll.
const MEMORY_LIMIT: usize = 512;

#[derive(Default)]
struct Queue {
    pending: Mutex<Option<String>>,
    wake: Condvar,
    /// `None` marks a path whose analysis failed.
    memory: Mutex<HashMap<String, Option<Vec<u8>>>>,
}

#[derive(Clone, Default)]
pub struct WaveformService {
    queue: Arc<Queue>,
}

/// Only local files and Android document URIs can be decoded; streams and
/// previews keep the plain slider.
pub fn can_analyze(path: &str) -> bool {
    if crate::path_validation::is_android_content_uri(path) {
        return true;
    }
    if path.starts_with("http://") || path.starts_with("https://") {
        return false;
    }
    std::path::Path::new(path).is_file()
}

impl WaveformService {
    /// The waveform for `path` if one is known. Otherwise queues it for the
    /// worker (unless it already failed or cannot be decoded) and returns
    /// `None`; the worker reports back through its `on_ready` callback.
    pub fn lookup(&self, path: &str, saved: Option<Vec<u8>>) -> Option<Vec<u8>> {
        if saved.is_some() {
            return saved;
        }
        if let Some(known) = self.memory().get(path) {
            return known.clone();
        }
        if can_analyze(path) {
            self.request(path);
        }
        None
    }

    fn request(&self, path: &str) {
        let mut pending = self.queue.pending.lock().unwrap_or_else(|e| e.into_inner());
        *pending = Some(path.to_string());
        self.queue.wake.notify_one();
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, HashMap<String, Option<Vec<u8>>>> {
        self.queue.memory.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn remember(&self, path: &str, result: Option<Vec<u8>>) {
        let mut memory = self.memory();
        if memory.len() >= MEMORY_LIMIT {
            memory.clear();
        }
        memory.insert(path.to_string(), result);
    }

    /// Block until a request is pending, then take it.
    fn next_request(&self) -> String {
        let mut pending = self.queue.pending.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(path) = pending.take() {
                return path;
            }
            pending = self
                .queue
                .wake
                .wait(pending)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Handle one request: decode, save, remember, report.
    ///
    /// `save` returns whether the waveform went into the library; one that
    /// did not (a file outside the library) is kept in memory instead.
    fn process(
        &self,
        path: &str,
        analyze: &impl Fn(&str) -> Result<Vec<u8>, String>,
        save: &impl Fn(&str, &[u8]) -> bool,
        on_ready: &impl Fn(&str),
    ) {
        match analyze(path) {
            Ok(bins) => {
                if !save(path, &bins) {
                    self.remember(path, Some(bins));
                }
                on_ready(path);
            }
            Err(error) => {
                tracing::warn!("Waveform analysis failed for \"{path}\": {error}");
                self.remember(path, None);
            }
        }
    }

    /// Run the worker on its own thread for the life of the app.
    pub fn spawn_worker(
        &self,
        analyze: impl Fn(&str) -> Result<Vec<u8>, String> + Send + 'static,
        save: impl Fn(&str, &[u8]) -> bool + Send + 'static,
        on_ready: impl Fn(&str) + Send + 'static,
    ) {
        let service = self.clone();
        std::thread::Builder::new()
            .name("waveform".to_string())
            .spawn(move || loop {
                let path = service.next_request();
                service.process(&path, &analyze, &save, &on_ready);
            })
            .map(|_| ())
            .unwrap_or_else(|e| tracing::warn!("Could not start the waveform worker: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn temp_file() -> String {
        let path = std::env::temp_dir().join(format!("wave-waveform-{}.mp3", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"x").unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_saved_waveform_is_returned_without_queueing() {
        let service = WaveformService::default();
        assert_eq!(service.lookup("/x.mp3", Some(vec![7])), Some(vec![7]));
        assert!(service.queue.pending.lock().unwrap().is_none());
    }

    #[test]
    fn a_missing_waveform_is_queued_and_the_latest_request_wins() {
        let service = WaveformService::default();
        let (a, b) = (temp_file(), temp_file());
        assert_eq!(service.lookup(&a, None), None);
        assert_eq!(service.lookup(&b, None), None);
        assert_eq!(service.next_request(), b);
        assert!(service.queue.pending.lock().unwrap().is_none());
    }

    #[test]
    fn streams_and_missing_files_are_never_queued() {
        let service = WaveformService::default();
        service.lookup("https://example.com/a.mp3", None);
        service.lookup("/definitely/not/here.flac", None);
        assert!(service.queue.pending.lock().unwrap().is_none());
    }

    #[test]
    fn android_documents_can_be_analyzed() {
        assert!(can_analyze(
            "content://com.android.externalstorage.documents/document/a"
        ));
    }

    #[test]
    fn a_library_result_is_saved_and_reported() {
        let service = WaveformService::default();
        let saved = RefCell::new(Vec::new());
        let ready = RefCell::new(Vec::new());
        service.process(
            "/a.mp3",
            &|_| Ok(vec![1, 2]),
            &|p, bins| {
                saved.borrow_mut().push((p.to_string(), bins.to_vec()));
                true
            },
            &|p| ready.borrow_mut().push(p.to_string()),
        );
        assert_eq!(
            saved.borrow().as_slice(),
            &[("/a.mp3".to_string(), vec![1, 2])]
        );
        assert_eq!(ready.borrow().as_slice(), &["/a.mp3".to_string()]);
        assert!(service.memory().is_empty());
    }

    #[test]
    fn a_file_outside_the_library_is_kept_in_memory() {
        let service = WaveformService::default();
        let path = temp_file();
        service.process(&path, &|_| Ok(vec![5]), &|_, _| false, &|_| {});
        assert_eq!(service.lookup(&path, None), Some(vec![5]));
        assert!(service.queue.pending.lock().unwrap().is_none());
    }

    #[test]
    fn a_failed_file_is_not_retried() {
        let service = WaveformService::default();
        let path = temp_file();
        let ready = RefCell::new(0);
        service.process(
            &path,
            &|_| Err("bad file".to_string()),
            &|_, _| true,
            &|_| *ready.borrow_mut() += 1,
        );
        assert_eq!(*ready.borrow(), 0);
        assert_eq!(service.lookup(&path, None), None);
        assert!(service.queue.pending.lock().unwrap().is_none());
    }
}
