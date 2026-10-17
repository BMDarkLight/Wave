// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Catalog sync and the artwork pass that follows it.

use super::backend::{RemoteBackend, RemoteError};
use super::store::WalkSummary;
use super::{RemoteEvent, RemoteRuntime, SourceState};
use crate::library::now_timestamp;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const ARTWORK_PACING: Duration = Duration::from_millis(50);
const SLOT_POLL: Duration = Duration::from_millis(25);

/// Clears the source's sync flag when the sync ends, however it ends.
struct SlotGuard(Arc<AtomicBool>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Failures of the runtime's own database are not the server's fault, but the
/// user can only retry, so they surface as a short toast-sized message.
fn local_failure(detail: String) -> RemoteError {
    tracing::warn!("Remote sync could not use the library: {detail}");
    RemoteError::Unreachable("Couldn't save the server's music".to_string())
}

fn state_for(error: &RemoteError) -> SourceState {
    match error {
        RemoteError::Unauthorized => SourceState::SignedOut,
        RemoteError::Blocked(_) => SourceState::Blocked,
        _ => SourceState::Unreachable,
    }
}

/// Walks the whole catalog into one walk and tombstones only when every page
/// arrived. A source already syncing is waited on instead of synced twice, and
/// the wait returns an empty summary.
pub fn sync_source(
    rt: &RemoteRuntime,
    source_id: &str,
    remote_enabled: bool,
) -> Result<WalkSummary, RemoteError> {
    let Some(backend) = rt.backend(source_id) else {
        return Err(RemoteError::Unreachable("This server is not set up".into()));
    };
    let slot = rt.sync_slot(source_id);
    if slot
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        while slot.load(Ordering::Acquire) {
            std::thread::sleep(SLOT_POLL);
        }
        return Ok(WalkSummary::default());
    }
    let _guard = SlotGuard(slot);

    rt.set_status(source_id, SourceState::Syncing);
    let result = walk_catalog(rt, source_id, backend.as_ref());
    let message = result.as_ref().err().map(|e| e.to_string());
    let recorded = rt.with_library(|library| {
        library.record_remote_sync(source_id, now_timestamp(), message.as_deref())
    });
    if let Err(detail) = recorded {
        tracing::warn!("Remote sync could not record its result: {detail}");
    }
    // A failed walk may still have upserted pages that need a visibility pass.
    let visibility = rt.with_library(|library| library.recompute_remote_visibility(remote_enabled));
    if let Err(detail) = visibility {
        tracing::warn!("Remote sync could not refresh visibility: {detail}");
    }

    match &result {
        Ok(_) => {
            if let Some(uri) = backend.working_uri() {
                if let Err(detail) = store_working_uri(rt, source_id, &uri) {
                    tracing::warn!("Could not store the working connection: {detail}");
                }
            }
            rt.set_status(source_id, SourceState::Ok);
        }
        Err(e) => rt.set_status(source_id, state_for(e)),
    }
    rt.emit(RemoteEvent::SyncFinished {
        source_id: source_id.to_string(),
        error: result.as_ref().err().map(|e| e.to_string()),
    });

    if result.is_ok() {
        artwork_pass(rt, source_id);
    }
    result
}

/// Sets `preferred_uri` in the stored config and nothing else. The row is read
/// again here, because a sign-in during a long sync may have rewritten it, and
/// it is written only when the value changes.
fn store_working_uri(rt: &RemoteRuntime, source_id: &str, uri: &str) -> Result<(), String> {
    rt.with_library(|library| {
        let Some(row) = library
            .list_remote_sources()?
            .into_iter()
            .find(|row| row.id == source_id)
        else {
            return Ok(());
        };
        let mut config: serde_json::Value =
            serde_json::from_str(&row.config_json).map_err(|e| e.to_string())?;
        let Some(object) = config.as_object_mut() else {
            return Ok(());
        };
        let preferred = serde_json::Value::String(uri.to_string());
        if object.get("preferred_uri") == Some(&preferred) {
            return Ok(());
        }
        object.insert("preferred_uri".to_string(), preferred);
        library.update_remote_source_config(source_id, &config.to_string())
    })
}

fn walk_catalog(
    rt: &RemoteRuntime,
    source_id: &str,
    backend: &dyn RemoteBackend,
) -> Result<WalkSummary, RemoteError> {
    let mut walk = rt
        .with_library(|library| library.begin_remote_walk(source_id))
        .map_err(local_failure)?;
    let mut cursor = 0;
    let mut synced = 0;
    loop {
        let page = backend.catalog_page(cursor)?;
        rt.with_library(|library| {
            library.upsert_remote_page(&mut walk, backend.kind(), &page.tracks)
        })
        .map_err(local_failure)?;
        synced += page.tracks.len();
        rt.emit(RemoteEvent::SyncProgress {
            source_id: source_id.to_string(),
            synced,
            total: page.total,
        });
        match page.next {
            Some(next) if next > cursor => cursor = next,
            Some(_) => {
                return Err(RemoteError::Parse("catalog cursor did not advance".into()));
            }
            None => break,
        }
    }
    rt.with_library(|library| library.finish_remote_walk(walk))
        .map_err(local_failure)
}

/// Fetches one cover per album that has none. Stops at the first sign the
/// server is out of reach or the gate is closed, since the rest would fail too.
pub fn artwork_pass(rt: &RemoteRuntime, source_id: &str) {
    let Some(backend) = rt.backend(source_id) else {
        return;
    };
    let albums = match rt.with_library(|library| library.remote_albums_missing_art(source_id)) {
        Ok(albums) => albums,
        Err(detail) => {
            tracing::warn!("Could not list albums missing artwork: {detail}");
            return;
        }
    };
    for (index, (album, album_artist, art_ref)) in albums.iter().enumerate() {
        if index > 0 {
            std::thread::sleep(ARTWORK_PACING);
        }
        match backend.fetch_artwork(art_ref) {
            Ok(image) => {
                let saved = rt.with_library(|library| {
                    library.set_album_art_for_remote_album(source_id, album, album_artist, &image)
                });
                if let Err(detail) = saved {
                    tracing::warn!("Could not store artwork for {album}: {detail}");
                }
            }
            Err(RemoteError::Blocked(_) | RemoteError::Unreachable(_)) => break,
            Err(_) => {}
        }
    }
}

/// Syncs every source on its own thread and calls `done` once all have
/// finished. Returns at once.
pub fn sync_all_in_background(
    rt: &'static RemoteRuntime,
    remote_enabled: bool,
    done: Box<dyn FnOnce() + Send>,
) {
    spawn_then_done(
        |job| {
            std::thread::Builder::new()
                .name("remote-sync".into())
                .spawn(job)
                .map(|_| ())
        },
        move || {
            let workers: Vec<_> = rt
                .source_ids()
                .into_iter()
                .filter_map(|id| {
                    let spawned = std::thread::Builder::new()
                        .name(format!("remote-sync-{id}"))
                        .spawn(move || {
                            let _ = sync_source(rt, &id, remote_enabled);
                        });
                    spawned
                        .map_err(|e| tracing::warn!("Could not start a remote sync thread: {e}"))
                        .ok()
                })
                .collect();
            for worker in workers {
                let _ = worker.join();
            }
        },
        done,
    );
}

/// Runs `work` through `spawner` and then `done`. If the spawner cannot start
/// the job, `done` runs right away so whoever waits on it never hangs.
fn spawn_then_done(
    spawner: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
    work: impl FnOnce() + Send + 'static,
    done: Box<dyn FnOnce() + Send>,
) {
    let done = Arc::new(parking_lot::Mutex::new(Some(done)));
    let in_thread = done.clone();
    let job: Box<dyn FnOnce() + Send> = Box::new(move || {
        work();
        if let Some(done) = in_thread.lock().take() {
            done();
        }
    });
    if let Err(e) = spawner(job) {
        tracing::warn!("Could not start the remote sync thread: {e}");
        if let Some(done) = done.lock().take() {
            done();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;
    use crate::remote::backend::{ByteStream, CatalogPage, RemoteBackend, RemoteTrackMeta};
    use crate::remote::gate::GateState;
    use crate::remote::{RemoteEvent, SourceState};
    use parking_lot::Mutex;
    use std::path::PathBuf;
    use std::sync::Arc;
    use uuid::Uuid;

    struct FakeBackend {
        pages: Vec<Result<CatalogPage, RemoteError>>,
        art: Result<Vec<u8>, RemoteError>,
        art_calls: Mutex<Vec<String>>,
        working: Option<String>,
        during_walk: Option<Box<dyn Fn() + Send + Sync>>,
    }

    impl FakeBackend {
        fn new(pages: Vec<Result<CatalogPage, RemoteError>>) -> Self {
            Self {
                pages,
                art: Ok(png_bytes()),
                art_calls: Mutex::new(Vec::new()),
                working: None,
                during_walk: None,
            }
        }
    }

    impl RemoteBackend for FakeBackend {
        fn kind(&self) -> &'static str {
            "plex"
        }

        fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
            if let Some(hook) = &self.during_walk {
                hook();
            }
            self.pages[cursor].clone()
        }

        fn open_bytes(&self, _: &str, _: u64) -> Result<Box<dyn ByteStream>, RemoteError> {
            Err(RemoteError::Http(404))
        }

        fn check_reachable(&self) -> Result<(), RemoteError> {
            Ok(())
        }

        fn fetch_artwork(&self, art_ref: &str) -> Result<Vec<u8>, RemoteError> {
            self.art_calls.lock().push(art_ref.to_string());
            self.art.clone()
        }

        fn working_uri(&self) -> Option<String> {
            self.working.clone()
        }
    }

    fn png_bytes() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            8,
            image::Rgb([200, 30, 30]),
        ))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
        out.into_inner()
    }

    fn meta(id: &str, album: &str, art: Option<&str>) -> RemoteTrackMeta {
        RemoteTrackMeta {
            remote_id: id.to_string(),
            title: format!("Song {id}"),
            artist: "Band".to_string(),
            album_artist: None,
            album: album.to_string(),
            track_number: Some(1),
            disc_number: None,
            year: None,
            genre: None,
            duration_seconds: Some(100.0),
            size: 1000,
            extension: "flac".to_string(),
            part_key: format!("/library/parts/{id}/file.flac"),
            art_ref: art.map(str::to_string),
            updated_at: Some(5),
        }
    }

    fn page(
        tracks: Vec<RemoteTrackMeta>,
        total: Option<usize>,
        next: Option<usize>,
    ) -> Result<CatalogPage, RemoteError> {
        Ok(CatalogPage {
            tracks,
            total,
            next,
        })
    }

    fn test_library() -> Library {
        let dir: PathBuf = std::env::temp_dir().join(format!("wave-test-sync-{}", Uuid::new_v4()));
        Library::new_with_path(&dir.join("library.db")).unwrap()
    }

    type Events = Arc<std::sync::Mutex<Vec<RemoteEvent>>>;

    /// A runtime over `library` with `backend` registered for `source`.
    fn runtime(library: Library, source: &str, backend: FakeBackend) -> (RemoteRuntime, Events) {
        let seen: Events = Arc::default();
        let sink = seen.clone();
        let rt =
            RemoteRuntime::new_for_test(library, Box::new(move |e| sink.lock().unwrap().push(e)));
        rt.insert_backend_for_test(source, Arc::new(backend));
        (rt, seen)
    }

    fn count(rt: &RemoteRuntime, sql: &str) -> i64 {
        rt.with_library(|l| {
            l.read_connection()
                .query_row(sql, [], |row| row.get(0))
                .unwrap()
        })
    }

    fn without_status_changes(seen: &Events) -> Vec<RemoteEvent> {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|e| !matches!(e, RemoteEvent::StatusChanged))
            .cloned()
            .collect()
    }

    #[test]
    fn failed_page_mid_walk_deletes_nothing_and_marks_unreachable() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv", "Home", "{}")
            .unwrap();
        let mut walk = library.begin_remote_walk(&source).unwrap();
        library
            .upsert_remote_page(
                &mut walk,
                "plex",
                &[meta("1", "Album", None), meta("2", "Album", None)],
            )
            .unwrap();
        library.finish_remote_walk(walk).unwrap();

        let backend = FakeBackend::new(vec![
            page(vec![meta("1", "Album", None)], Some(2), Some(1)),
            Err(RemoteError::Unreachable("Can't reach Home".into())),
        ]);
        let (rt, seen) = runtime(library, &source, backend);

        let err = sync_source(&rt, &source, true).unwrap_err();
        assert_eq!(err, RemoteError::Unreachable("Can't reach Home".into()));
        assert_eq!(rt.status(&source), SourceState::Unreachable);
        assert_eq!(
            count(
                &rt,
                "SELECT COUNT(*) FROM tracks WHERE remote_missing = 0 AND source_state = 'remote'"
            ),
            2
        );
        let row = rt.with_library(|l| l.list_remote_sources().unwrap().remove(0));
        assert_eq!(row.last_sync_error.as_deref(), Some("Can't reach Home"));
        assert!(row.last_sync_at.is_some());
        assert_eq!(
            without_status_changes(&seen).last(),
            Some(&RemoteEvent::SyncFinished {
                source_id: source,
                error: Some("Can't reach Home".into()),
            })
        );
    }

    #[test]
    fn unauthorized_marks_signed_out() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv", "Home", "{}")
            .unwrap();
        let (rt, _) = runtime(
            library,
            &source,
            FakeBackend::new(vec![Err(RemoteError::Unauthorized)]),
        );
        assert_eq!(
            sync_source(&rt, &source, true).unwrap_err(),
            RemoteError::Unauthorized
        );
        assert_eq!(rt.status(&source), SourceState::SignedOut);
    }

    #[test]
    fn blocked_marks_blocked() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv", "Home", "{}")
            .unwrap();
        let (rt, _) = runtime(
            library,
            &source,
            FakeBackend::new(vec![Err(RemoteError::Blocked(GateState::Disabled))]),
        );
        assert!(sync_source(&rt, &source, true).is_err());
        assert_eq!(rt.status(&source), SourceState::Blocked);
    }

    #[test]
    fn successful_walk_records_sync_and_emits_finished() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv", "Home", "{}")
            .unwrap();
        let backend = FakeBackend::new(vec![
            page(
                vec![meta("1", "Album", None), meta("2", "Album", None)],
                Some(3),
                Some(1),
            ),
            page(vec![meta("3", "Album", None)], Some(3), None),
        ]);
        let (rt, seen) = runtime(library, &source, backend);

        let summary = sync_source(&rt, &source, true).unwrap();
        assert_eq!(summary.upserted, 3);
        assert_eq!(summary.tombstoned, 0);
        assert_eq!(rt.status(&source), SourceState::Ok);

        let row = rt.with_library(|l| l.list_remote_sources().unwrap().remove(0));
        assert!(row.last_sync_at.is_some());
        assert_eq!(row.last_sync_error, None);
        assert_eq!(row.track_count, 3);
        assert_eq!(count(&rt, "SELECT COUNT(*) FROM library_tracks"), 3);

        let progress = |synced| RemoteEvent::SyncProgress {
            source_id: source.clone(),
            synced,
            total: Some(3),
        };
        assert_eq!(
            without_status_changes(&seen),
            vec![
                progress(2),
                progress(3),
                RemoteEvent::SyncFinished {
                    source_id: source.clone(),
                    error: None,
                },
            ]
        );
    }

    #[test]
    fn a_full_walk_tombstones_what_the_server_dropped() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv", "Home", "{}")
            .unwrap();
        let mut walk = library.begin_remote_walk(&source).unwrap();
        library
            .upsert_remote_page(
                &mut walk,
                "plex",
                &[meta("1", "Album", None), meta("2", "Album", None)],
            )
            .unwrap();
        library.finish_remote_walk(walk).unwrap();

        let backend = FakeBackend::new(vec![page(vec![meta("1", "Album", None)], Some(1), None)]);
        let (rt, _) = runtime(library, &source, backend);
        let summary = sync_source(&rt, &source, true).unwrap();
        assert_eq!(summary.tombstoned, 1);
        assert_eq!(count(&rt, "SELECT COUNT(*) FROM library_tracks"), 1);
    }

    #[test]
    fn a_second_sync_waits_for_the_first_instead_of_walking_again() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv", "Home", "{}")
            .unwrap();
        // No pages: any attempt to walk would panic on the empty list.
        let (rt, _) = runtime(library, &source, FakeBackend::new(Vec::new()));
        let slot = rt.sync_slot(&source);
        slot.store(true, Ordering::Release);
        let release = slot.clone();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            release.store(false, Ordering::Release);
        });

        let summary = sync_source(&rt, &source, true).unwrap();
        releaser.join().unwrap();
        assert_eq!(summary, WalkSummary::default());
        assert!(!slot.load(Ordering::Acquire));
    }

    #[test]
    fn sync_stores_only_the_working_uri_in_the_current_config() {
        let library = test_library();
        let source = library
            .insert_remote_source(
                "plex",
                "srv",
                "Home",
                r#"{"server_token":"old","preferred_uri":null}"#,
            )
            .unwrap();
        // Another connection rewrites the config while the walk is running,
        // as a re-sign-in would.
        let db_path = library.db_path();
        let id = source.clone();
        let mut backend = FakeBackend::new(vec![page(vec![meta("1", "Album", None)], None, None)]);
        backend.working = Some("http://nas:32400".to_string());
        backend.during_walk = Some(Box::new(move || {
            let other = Library::new_with_path(std::path::Path::new(&db_path)).unwrap();
            other
                .update_remote_source_config(
                    &id,
                    r#"{"server_token":"fresh","preferred_uri":null}"#,
                )
                .unwrap();
        }));
        let (rt, _) = runtime(library, &source, backend);

        sync_source(&rt, &source, true).unwrap();
        let row = rt.with_library(|l| l.list_remote_sources().unwrap().remove(0));
        let stored: serde_json::Value = serde_json::from_str(&row.config_json).unwrap();
        assert_eq!(
            stored,
            serde_json::json!({"server_token": "fresh", "preferred_uri": "http://nas:32400"})
        );
    }

    #[test]
    fn an_unchanged_working_uri_leaves_the_stored_config_alone() {
        let library = test_library();
        let config = r#"{"b":1,"preferred_uri":"http://nas:32400"}"#;
        let source = library
            .insert_remote_source("plex", "srv", "Home", config)
            .unwrap();
        let mut backend = FakeBackend::new(vec![page(vec![meta("1", "Album", None)], None, None)]);
        backend.working = Some("http://nas:32400".to_string());
        let (rt, _) = runtime(library, &source, backend);
        sync_source(&rt, &source, true).unwrap();
        let row = rt.with_library(|l| l.list_remote_sources().unwrap().remove(0));
        assert_eq!(row.config_json, config);
    }

    #[test]
    fn a_failed_record_does_not_skip_the_visibility_pass() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv", "Home", "{}")
            .unwrap();
        let db_path = library.db_path();
        let id = source.clone();
        let mut backend = FakeBackend::new(vec![page(vec![meta("1", "Album", None)], None, None)]);
        // Break only the sync bookkeeping: the record update then fails.
        backend.during_walk = Some(Box::new(move || {
            let other = Library::new_with_path(std::path::Path::new(&db_path)).unwrap();
            other
                .write_connection()
                .execute_batch(&format!(
                    "CREATE TRIGGER fail_record BEFORE UPDATE OF last_sync_at ON remote_sources
                     WHEN NEW.id = '{id}' BEGIN SELECT RAISE(ABORT, 'no'); END"
                ))
                .unwrap();
        }));
        let (rt, _) = runtime(library, &source, backend);
        sync_source(&rt, &source, true).unwrap();
        assert_eq!(count(&rt, "SELECT COUNT(*) FROM library_tracks"), 1);
    }

    #[test]
    fn the_done_callback_runs_when_the_thread_cannot_start() {
        let called = Arc::new(AtomicBool::new(false));
        let flag = called.clone();
        spawn_then_done(
            |_| Err(std::io::Error::other("no threads")),
            || {},
            Box::new(move || flag.store(true, Ordering::Release)),
        );
        assert!(called.load(Ordering::Acquire));
    }

    #[test]
    fn the_done_callback_runs_once_after_the_work() {
        let (tx, rx) = std::sync::mpsc::channel();
        let order = tx.clone();
        spawn_then_done(
            |job| std::thread::Builder::new().spawn(job).map(|_| ()),
            move || order.send("work").unwrap(),
            Box::new(move || tx.send("done").unwrap()),
        );
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "work");
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "done");
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn artwork_pass_sets_album_art_for_every_track_of_album() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv", "Home", "{}")
            .unwrap();
        let backend = FakeBackend::new(vec![page(
            vec![
                meta("1", "Album", Some("/thumb/9")),
                meta("2", "Album", Some("/thumb/9")),
                meta("3", "Bare", None),
            ],
            Some(3),
            None,
        )]);
        let (rt, _) = runtime(library, &source, backend);

        sync_source(&rt, &source, true).unwrap();
        assert_eq!(
            count(
                &rt,
                "SELECT COUNT(*) FROM tracks WHERE album = 'Album' AND album_art_id IS NOT NULL"
            ),
            2
        );
        assert_eq!(
            count(
                &rt,
                "SELECT COUNT(*) FROM tracks WHERE album = 'Bare' AND album_art_id IS NOT NULL"
            ),
            0
        );
    }

    #[test]
    fn artwork_pass_stops_at_the_first_unreachable_fetch() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv", "Home", "{}")
            .unwrap();
        let mut walk = library.begin_remote_walk(&source).unwrap();
        library
            .upsert_remote_page(
                &mut walk,
                "plex",
                &[
                    meta("1", "A", Some("/thumb/1")),
                    meta("2", "B", Some("/thumb/2")),
                ],
            )
            .unwrap();
        library.finish_remote_walk(walk).unwrap();

        let mut backend = FakeBackend::new(Vec::new());
        backend.art = Err(RemoteError::Unreachable("Can't reach Home".into()));
        let backend = Arc::new(backend);
        let (rt, _) = runtime(library, &source, FakeBackend::new(Vec::new()));
        rt.insert_backend_for_test(&source, backend.clone());

        artwork_pass(&rt, &source);
        assert_eq!(backend.art_calls.lock().len(), 1);
    }
}
