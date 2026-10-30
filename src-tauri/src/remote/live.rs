// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! End-to-end checks against real servers, run on request:
//!
//! ```text
//! WAVE_WEBDAV_URL=http://127.0.0.1:8090/ WAVE_WEBDAV_USER=u WAVE_WEBDAV_PASS=p \
//!   cargo test --lib -- --ignored live_webdav --nocapture
//! WAVE_DLNA_ADDRESS=127.0.0.1:7879 cargo test --lib -- --ignored live_dlna --nocapture
//! WAVE_KODI_URL=192.168.1.30:8080 WAVE_KODI_USER=kodi WAVE_KODI_PASS=... \
//!   cargo test --lib -- --ignored live_kodi --nocapture
//! cargo test --lib -- --ignored live_audius_account_end_to_end --nocapture
//! ```
//!
//! Each one adds the source to a fresh library, syncs it the way the app does,
//! fetches artwork, then plays tracks through the stream cache: a read far
//! ahead of the download first, which only a working ranged request can
//! answer quickly, then the whole file, which must decode and have its length
//! recorded.

use super::gate::NetworkGate;
use super::stream::{self, PlayTarget};
use super::uri::RemoteUri;
use super::{dlna, sync, RemoteRuntime};
use crate::library::Library;
use std::time::{Duration, Instant};

const FILL_WAIT: Duration = Duration::from_secs(180);

struct Row {
    path: String,
    title: String,
    artist: String,
    album: String,
    format: String,
    size: i64,
    duration: Option<f64>,
    art: Option<String>,
}

fn rows(rt: &RemoteRuntime, source_id: &str) -> Vec<Row> {
    rt.with_library(|l| {
        let connection = l.read_connection();
        let mut statement = connection
            .prepare(
                "SELECT path, title, artist, album, format, file_size, duration_seconds,
                        album_art_id
                 FROM tracks WHERE remote_source_id = ?1 ORDER BY path",
            )
            .unwrap();
        statement
            .query_map([source_id], |r| {
                Ok(Row {
                    path: r.get(0)?,
                    title: r.get(1)?,
                    artist: r.get(2)?,
                    album: r.get(3)?,
                    format: r.get(4)?,
                    size: r.get(5)?,
                    duration: r.get(6)?,
                    art: r.get(7)?,
                })
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    })
}

fn runtime() -> &'static RemoteRuntime {
    NetworkGate::global().set_policy(true, true);
    NetworkGate::search().set_policy(true, true);
    let dir = std::env::temp_dir().join(format!("wave-live-{}", uuid::Uuid::new_v4()));
    let library = Library::new_with_path(&dir.join("library.db")).unwrap();
    Box::leak(Box::new(RemoteRuntime::new_for_test(
        library,
        Box::new(|_| {}),
    )))
}

/// Syncs a source and returns its rows.
fn add_and_sync(kind: &str, key: &str, config_json: &str) -> (&'static RemoteRuntime, String) {
    let rt = runtime();
    let id = rt.with_library(|l| {
        let id = l
            .insert_remote_source(kind, key, kind, config_json)
            .unwrap();
        rt.reload_sources(l);
        id
    });
    let started = Instant::now();
    let summary = sync::sync_source(rt, &id, true).expect("sync failed");
    println!(
        "synced {} tracks from {kind} in {:.1}s",
        summary.upserted,
        started.elapsed().as_secs_f64()
    );
    sync::artwork_pass(rt, &id);
    (rt, id)
}

/// Plays one track through the stream cache from start to finish.
fn play_through(rt: &'static RemoteRuntime, path: &str) {
    let gate = NetworkGate::for_path(path);
    let target = stream::resolve_play_target_with(rt, gate, path).expect("resolve");
    let PlayTarget::Progressive(fill, extension) = target else {
        panic!("{path} was already cached before its first play");
    };
    assert!(fill.len > 0, "the size was never learned");

    // A read three quarters in, before anything else has downloaded.
    let ahead = fill.len * 3 / 4;
    let started = Instant::now();
    fill.wait_for(ahead, 4096.min(fill.len - ahead), Duration::from_secs(30))
        .expect("reading ahead of the download failed");
    let seek = started.elapsed();

    fill.wait_for(0, fill.len, FILL_WAIT)
        .expect("the whole file never arrived");
    let uri = RemoteUri::parse(path).unwrap();
    let [file, _] = stream::remote_cache_files_for(&uri, &extension);
    assert_eq!(
        std::fs::metadata(&file).unwrap().len(),
        fill.len,
        "cache file size"
    );
    crate::audio::symphonia_source::SymphoniaSource::new(file.to_string_lossy().as_ref())
        .expect("the cached file must decode");

    // Completion is recorded on the fill's thread; give it a moment.
    let deadline = Instant::now() + Duration::from_secs(10);
    let state = loop {
        let state: Option<String> = rt.with_library(|l| {
            l.read_connection()
                .query_row(
                    "SELECT cache_state FROM tracks WHERE path = ?1",
                    [path],
                    |r| r.get(0),
                )
                .unwrap()
        });
        if state.as_deref() == Some("complete") || Instant::now() > deadline {
            break state;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(state.as_deref(), Some("complete"));
    println!(
        "  played {path}: {} bytes, read ahead in {:.2}s",
        fill.len,
        seek.as_secs_f64()
    );
}

fn report_and_play(rt: &'static RemoteRuntime, id: &str, plays: usize) -> Vec<Row> {
    let found = rows(rt, id);
    assert!(!found.is_empty(), "the sync found no tracks");
    for row in &found {
        println!(
            "  {} | {} | {} | {} | {} bytes | {:?}s | art {}",
            row.title,
            row.artist,
            row.album,
            row.format,
            row.size,
            row.duration,
            row.art.is_some()
        );
    }
    for row in found.iter().take(plays) {
        play_through(rt, &row.path);
    }
    // Every played track now has a length, measured if the catalog had none.
    let played = rows(rt, id);
    for row in played.iter().take(plays) {
        assert!(
            row.duration.is_some_and(|d| d > 0.0),
            "{} has no length",
            row.title
        );
    }
    played
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
}

#[test]
#[ignore = "needs a WebDAV server"]
fn live_webdav_end_to_end() {
    NetworkGate::global().set_policy(true, true);
    let config = super::webdav::WebDavConfig {
        url: super::webdav::normalize_url(&env("WAVE_WEBDAV_URL")).unwrap(),
        username: std::env::var("WAVE_WEBDAV_USER").unwrap_or_default(),
        password: std::env::var("WAVE_WEBDAV_PASS").unwrap_or_default(),
    };
    super::webdav::WebDavBackend::new(config.clone())
        .unwrap()
        .probe()
        .expect("probe");
    let key = super::webdav::stable_key(&config);
    let (rt, id) = add_and_sync("webdav", &key, &serde_json::to_string(&config).unwrap());
    let plays = std::env::var("WAVE_LIVE_PLAYS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(usize::MAX);
    report_and_play(rt, &id, plays);

    // A second sync reads no headers again and changes nothing.
    let before: Vec<_> = rows(rt, &id)
        .into_iter()
        .map(|r| (r.path, r.title))
        .collect();
    let started = Instant::now();
    sync::sync_source(rt, &id, true).expect("second sync");
    let after: Vec<_> = rows(rt, &id)
        .into_iter()
        .map(|r| (r.path, r.title))
        .collect();
    assert_eq!(before, after);
    println!("second sync in {:.2}s", started.elapsed().as_secs_f64());
}

#[test]
#[ignore = "needs a DLNA server"]
fn live_dlna_end_to_end() {
    let gate = NetworkGate::global();
    gate.set_policy(true, true);
    let found = dlna::discover(gate, Duration::from_secs(3)).expect("discover");
    println!("discovered: {found:?}");
    let config = match std::env::var("WAVE_DLNA_ADDRESS") {
        Ok(address) => dlna::candidate_locations(&address)
            .unwrap()
            .iter()
            .find_map(|location| dlna::describe(gate, location).ok())
            .expect("no media server at that address"),
        Err(_) => found
            .iter()
            .find_map(|location| dlna::describe(gate, location).ok())
            .expect("no media server found"),
    };
    println!("server: {} at {}", config.name, config.location);
    // The server announces its own addresses, which need not be the one
    // typed, but its port and description path are the same.
    let announced = url::Url::parse(&config.location).unwrap();
    assert!(
        found
            .iter()
            .filter_map(|l| url::Url::parse(l).ok())
            .any(|l| { l.port() == announced.port() && l.path() == announced.path() }),
        "discovery missed {}",
        config.location
    );
    let key = dlna::stable_key(&config);
    let (rt, id) = add_and_sync("dlna", &key, &serde_json::to_string(&config).unwrap());
    report_and_play(rt, &id, usize::MAX);
}

#[test]
#[ignore = "needs a Kodi device"]
fn live_kodi_end_to_end() {
    NetworkGate::global().set_policy(true, true);
    let config = super::kodi::KodiConfig {
        url: super::kodi::normalize_url(&env("WAVE_KODI_URL")).unwrap(),
        username: std::env::var("WAVE_KODI_USER").unwrap_or_default(),
        password: std::env::var("WAVE_KODI_PASS").unwrap_or_default(),
    };
    super::kodi::KodiBackend::new(config.clone())
        .probe()
        .expect("probe");
    let key = super::kodi::stable_key(&config);
    let (rt, id) = add_and_sync("kodi", &key, &serde_json::to_string(&config).unwrap());
    report_and_play(rt, &id, 3);
}

#[test]
#[ignore = "hits the network"]
fn live_audius_account_end_to_end() {
    let gate = NetworkGate::global();
    gate.set_policy(true, true);
    // An account of ordinary-length songs; the point is the pipeline, not
    // the bandwidth an hour-long mix needs.
    let config = super::audius::resolve(gate, "chillhopmusic").expect("resolve");
    let (rt, id) = add_and_sync(
        "audius",
        &config.user_id,
        &serde_json::to_string(&config).unwrap(),
    );
    let mut found = rows(rt, &id);
    found.sort_by(|a, b| a.duration.partial_cmp(&b.duration).unwrap());
    for row in found.iter().take(2) {
        play_through(rt, &row.path);
    }
}

#[test]
#[ignore = "hits the network"]
fn live_search_results_stream_through_the_cache() {
    use super::search::{part_key, remote_id, SEARCH_SOURCE};
    use crate::sources::{self, SourceProvider};

    let rt = runtime();
    rt.with_library(|l| rt.reload_sources(l));
    let providers: Vec<Box<dyn SourceProvider>> = vec![
        Box::new(sources::audius::Audius),
        Box::new(sources::ccmixter::CcMixter),
        Box::new(sources::wikimedia::WikimediaCommons),
    ];
    for (provider, query) in providers.iter().zip(["lofi", "piano", "bach cello suite"]) {
        let client = sources::source_client();
        let tracks = provider.search(client, query, 10).expect("search");
        let track = tracks
            .iter()
            .filter(|t| t.duration_seconds.is_none_or(|d| d < 400.0))
            .find(|t| provider.resolve_audio(client, t).is_ok())
            .expect("nothing playable");
        let url = provider.resolve_audio(client, track).unwrap();
        let rid = remote_id(provider.id(), &track.id);
        let path = RemoteUri::new(SEARCH_SOURCE, &rid).to_path();
        let row = crate::metadata::Track {
            id: uuid::Uuid::new_v4().to_string(),
            path: path.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            format: {
                let mut resolved = track.clone();
                resolved.audio_url = Some(url.clone());
                resolved.extension()
            },
            ..crate::metadata::Track::default()
        };
        rt.with_library(|l| {
            l.upsert_streamed_source_track(
                &row,
                provider.id(),
                &rid,
                &part_key(provider.id(), &url),
            )
        })
        .unwrap();
        println!("{}: {}", provider.id(), track.title);
        play_through(rt, &path);
    }
}

/// Shares a folder of real files with Wave's own media server, then finds it
/// over SSDP, syncs it and plays every song with Wave's own DLNA client:
///
/// ```text
/// WAVE_SHARE_DIR=/path/to/music cargo test --lib -- --ignored live_shared_library --nocapture
/// ```
///
/// `WAVE_SHARE_HOLD=<seconds>` keeps the server up that long afterwards, to
/// try it from a TV, VLC or another player.
#[test]
#[ignore = "shares a folder on the local network"]
fn live_shared_library_round_trip() {
    let folder = env("WAVE_SHARE_DIR");
    let dir = std::env::temp_dir().join(format!("wave-share-live-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("shared.db");
    let library = Library::new_with_path(&db_path).unwrap();
    let (indexed, failed) = library
        .index_directory_with_progress(None, None, folder, &mut |_, _, _| {})
        .unwrap();
    assert!(failed.is_empty(), "could not index: {failed:?}");
    assert!(!indexed.is_empty());
    let playlist = library.create_playlist("Road trip", None).unwrap();
    library
        .add_track_to_playlist(&playlist.id, indexed[0].path.clone())
        .unwrap();
    drop(library);

    let uuid = uuid::Uuid::new_v4().to_string();
    crate::share::apply(Some(crate::share::ShareConfig {
        name: format!("Wave live test {}", &uuid[..8]),
        uuid: uuid.clone(),
        db_path,
        opener: crate::share::file_opener(None),
        dlna: true,
        peers: None,
    }));
    let status = crate::share::status();
    assert!(status.running, "{:?}", status.error);
    println!("sharing: {status:?}");

    let gate = NetworkGate::global();
    gate.set_policy(true, true);
    let config = (0..3)
        .find_map(|_| {
            dlna::discover(gate, Duration::from_secs(3))
                .unwrap()
                .iter()
                .filter_map(|location| dlna::describe(gate, location).ok())
                .find(|c| c.udn == format!("uuid:{uuid}"))
        })
        .expect("Wave's client never discovered Wave's server");
    println!("discovered {} at {}", config.name, config.location);

    let key = dlna::stable_key(&config);
    let (rt, id) = add_and_sync("dlna", &key, &serde_json::to_string(&config).unwrap());
    let played = report_and_play(rt, &id, usize::MAX);
    assert_eq!(played.len(), indexed.len(), "each shared song once");
    let mut shared: Vec<_> = played.iter().map(|r| r.title.clone()).collect();
    let mut local: Vec<_> = indexed.iter().map(|t| t.title.clone()).collect();
    shared.sort();
    local.sort();
    assert_eq!(shared, local, "titles survive the trip");
    for track in &indexed {
        let row = played.iter().find(|r| r.title == track.title).unwrap();
        assert_eq!(row.artist, track.artist, "{}", track.title);
        assert_eq!(row.album, track.album, "{}", track.title);
        assert_eq!(row.size, track.file_size, "{}", track.title);
    }

    if let Some(hold) = std::env::var("WAVE_SHARE_HOLD")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        println!("holding the server for {hold}s");
        std::thread::sleep(Duration::from_secs(hold));
    }
    crate::share::apply(None);
    assert!(!crate::share::status().running);
}
