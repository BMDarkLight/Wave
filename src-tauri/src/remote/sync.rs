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
#[allow(dead_code)]
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
    let recorded = rt.with_library(|library| {
        let message = result.as_ref().err().map(|e| e.to_string());
        let recorded = library.record_remote_sync(source_id, now_timestamp(), message.as_deref());
        // A failed walk may still have upserted pages that need a visibility pass.
        recorded.and(library.recompute_remote_visibility(remote_enabled))
    });
    if let Err(detail) = recorded {
        tracing::warn!("Remote sync could not record its result: {detail}");
    }

    match &result {
        Ok(_) => {
            if let Some(config) = backend.updated_config() {
                let stored = rt.with_library(|library| {
                    library.update_remote_source_config(source_id, &config)
                });
                if let Err(detail) = stored {
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
#[allow(dead_code)]
pub fn sync_all_in_background(
    rt: &'static RemoteRuntime,
    remote_enabled: bool,
    done: Box<dyn FnOnce() + Send>,
) {
    let spawned = std::thread::Builder::new()
        .name("remote-sync".into())
        .spawn(move || {
            let workers: Vec<_> = rt
                .source_ids()
                .into_iter()
                .filter_map(|id| {
                    std::thread::Builder::new()
                        .name(format!("remote-sync-{id}"))
                        .spawn(move || {
                            let _ = sync_source(rt, &id, remote_enabled);
                        })
                        .ok()
                })
                .collect();
            for worker in workers {
                let _ = worker.join();
            }
            done();
        });
    if let Err(e) = spawned {
        tracing::warn!("Could not start the remote sync thread: {e}");
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
        config: Option<String>,
    }

    impl FakeBackend {
        fn new(pages: Vec<Result<CatalogPage, RemoteError>>) -> Self {
            Self {
                pages,
                art: Ok(png_bytes()),
                art_calls: Mutex::new(Vec::new()),
                config: None,
            }
        }
    }

    impl RemoteBackend for FakeBackend {
        fn kind(&self) -> &'static str {
            "plex"
        }

        fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
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

        fn updated_config(&self) -> Option<String> {
            self.config.clone()
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
    fn a_changed_connection_is_stored_with_the_source() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv", "Home", "{}")
            .unwrap();
        let mut backend = FakeBackend::new(vec![page(vec![meta("1", "Album", None)], None, None)]);
        backend.config = Some(r#"{"preferred":"http://nas"}"#.to_string());
        let (rt, _) = runtime(library, &source, backend);
        sync_source(&rt, &source, true).unwrap();
        let row = rt.with_library(|l| l.list_remote_sources().unwrap().remove(0));
        assert_eq!(row.config_json, r#"{"preferred":"http://nas"}"#);
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
