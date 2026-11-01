// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Online play against the real services: Deezer for the song, then YouTube
//! by each way in, down to the audio decoding and seeking in Wave's own
//! decoder. They download yt-dlp and Deno (about 90 MB) and talk to YouTube
//! and public servers, so none runs unless asked:
//!
//! `cargo test --lib youtube::live -- --ignored --nocapture --test-threads=1`
//!
//! `WAVE_TOOLS_DIR` keeps the install between runs.

use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rodio::Source;

use super::matching::Wanted;
use super::router::{self, Kind, Router, Way, Ways};
use super::tools;
use super::ytdlp::YtDlp;
use super::{video_id_of, Candidate, Stream};
use crate::audio::symphonia_source::SymphoniaSource;
use crate::sources::{deezer::Deezer, source_client, SourceProvider, SourceTrack};

/// Songs whose catalog cut differs from the most viewed upload, so a sloppy
/// match shows up as a length that's off.
const SONGS: [&str; 4] = [
    "Daft Punk Get Lucky",
    "Queen Bohemian Rhapsody",
    "Billie Eilish bad guy",
    "Adele Hello",
];

fn tools_dir() -> PathBuf {
    std::env::var("WAVE_TOOLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("wave-online-play-live"))
}

fn installed() -> YtDlp {
    let dir = tools_dir();
    let started = Instant::now();
    tools::install_all(&dir, &mut |_| {}).unwrap();
    println!("install or update check: {:?}", started.elapsed());
    YtDlp::in_dir(&dir).unwrap()
}

/// What Deezer lists for `query`, as Online play shows it.
fn deezer(query: &str) -> SourceTrack {
    let found = Deezer { online_play: true }
        .search(source_client(), query, 5)
        .unwrap();
    found.into_iter().next().expect("Deezer found nothing")
}

fn wanted(track: &SourceTrack) -> Wanted<'_> {
    Wanted {
        title: &track.title,
        artist: &track.artist,
        duration_seconds: track.duration_seconds,
    }
}

struct Direct(YtDlp);

impl Way for Direct {
    fn key(&self) -> String {
        "direct".into()
    }
    fn kind(&self) -> Kind {
        Kind::Direct
    }
    fn search(&self, query: &str) -> Result<Vec<Candidate>, String> {
        self.0.search(query, 8)
    }
    fn stream(&self, video_id: &str, itag: Option<&str>) -> Result<Stream, String> {
        self.0.stream_format(video_id, itag)
    }
}

struct Only(Vec<Box<dyn Fn() -> Box<dyn Way> + Send + Sync>>);

impl Ways for Only {
    fn ways(&self) -> Vec<Box<dyn Way>> {
        self.0.iter().map(|make| make()).collect()
    }
}

/// Finds `track`, downloads all of it the way playback does, and decodes and
/// seeks it. Returns the decoded length.
fn play(router: &Router, track: &SourceTrack) -> f64 {
    let started = Instant::now();
    let reference = router.find(&wanted(track)).unwrap();
    let found_in = started.elapsed();
    let video_id = video_id_of(&reference).unwrap();

    // A range from the middle, as a seek asks for.
    let mut part = Vec::new();
    let opened = router.fetch(video_id, "bytes=1000-1999").unwrap();
    assert_eq!((opened.start, opened.end), (1000, 1999));
    { opened.response }.read_to_end(&mut part).unwrap();
    assert_eq!(part.len(), 1000, "a mid-file range");

    // All of it, asking again from wherever an answer stops, as the reader
    // does.
    let started = Instant::now();
    let mut bytes = Vec::new();
    loop {
        let opened = router
            .fetch(video_id, &format!("bytes={}-", bytes.len()))
            .unwrap();
        let total = opened.total as usize;
        { opened.response }.read_to_end(&mut bytes).unwrap();
        if bytes.len() >= total {
            break;
        }
    }
    let fetched_in = started.elapsed();
    let ext = reference.rsplit('.').next().unwrap();
    let path = std::env::temp_dir().join(format!("wave-live-{video_id}.{ext}"));
    std::fs::write(&path, &bytes).unwrap();

    let open = || {
        SymphoniaSource::from_media_source(
            Box::new(std::fs::File::open(&path).unwrap()),
            Some(ext),
            "live",
        )
        .unwrap()
    };
    let source = open();
    let (rate, channels) = (source.sample_rate() as f64, source.channels() as f64);
    let samples = source.count() as f64;
    let seconds = samples / rate / channels;

    let mut seeking = open();
    seeking
        .try_seek(Duration::from_secs_f64(seconds / 2.0))
        .unwrap();
    assert!(seeking.next().is_some(), "plays on after a seek");
    let _ = std::fs::remove_file(&path);

    println!(
        "  {} - {} ({:?}s on Deezer): {reference}, found in {found_in:?}, \
         {} KB in {fetched_in:?}, decodes to {seconds:.1}s",
        track.artist,
        track.title,
        track.duration_seconds,
        bytes.len() / 1024
    );
    seconds
}

fn assert_same_song(track: &SourceTrack, seconds: f64) {
    let listed = track.duration_seconds.unwrap();
    assert!(
        (listed - seconds).abs() <= 10.0,
        "{} plays {seconds:.0}s but Deezer lists {listed:.0}s",
        track.title
    );
}

#[test]
#[ignore]
fn yt_dlp_finds_and_plays_catalog_songs() {
    let ytdlp = installed();
    let started = Instant::now();
    let found = ytdlp.search("Daft Punk Get Lucky", 8).unwrap();
    println!("a search: {:?}, {} results", started.elapsed(), found.len());
    assert!(
        found.iter().all(|c| c.duration_seconds.is_some()),
        "lengths reported"
    );

    let dir = tools_dir();
    let router = Router::new(Box::new(Only(vec![Box::new(move || {
        Box::new(Direct(YtDlp::in_dir(&dir).unwrap())) as Box<dyn Way>
    })])));
    for song in SONGS {
        let track = deezer(song);
        let seconds = play(&router, &track);
        assert_same_song(&track, seconds);
    }

    // Found once, a song plays again without asking YouTube.
    let track = deezer(SONGS[0]);
    let started = Instant::now();
    router.find(&wanted(&track)).unwrap();
    assert!(started.elapsed() < Duration::from_millis(50));
}

/// Plays a song through public servers of `kind` alone.
fn through_public(kind: Kind, servers: Vec<String>) {
    println!("{} {kind:?} servers listed: {servers:?}", servers.len());
    assert!(!servers.is_empty(), "no {kind:?} servers listed");
    let ways: Vec<Box<dyn Fn() -> Box<dyn Way> + Send + Sync>> = servers
        .into_iter()
        .map(|base| {
            Box::new(move || router::public_way(kind, &base))
                as Box<dyn Fn() -> Box<dyn Way> + Send + Sync>
        })
        .collect();
    // Every listed server gets its chance, not just the first three.
    let router = Router::new(Box::new(Only(ways)));
    let track = deezer(SONGS[0]);
    let mut last = None;
    for _ in 0..5 {
        match router.find(&wanted(&track)) {
            Ok(_) => {
                let seconds = play(&router, &track);
                assert_same_song(&track, seconds);
                return;
            }
            Err(e) => last = Some(e),
        }
    }
    panic!("no {kind:?} server played the song: {last:?}");
}

#[test]
#[ignore]
fn invidious_plays_a_catalog_song() {
    through_public(
        Kind::Invidious,
        super::invidious::instances(super::public_client()).unwrap(),
    );
}

#[test]
#[ignore]
fn piped_plays_a_catalog_song() {
    through_public(
        Kind::Piped,
        super::piped::instances(super::public_client()).unwrap(),
    );
}

/// yt-dlp is gone, so the song comes through the public servers instead.
#[test]
#[ignore]
fn a_broken_yt_dlp_falls_through_to_public_servers() {
    struct Broken;
    impl Way for Broken {
        fn key(&self) -> String {
            "direct".into()
        }
        fn kind(&self) -> Kind {
            Kind::Direct
        }
        fn search(&self, _query: &str) -> Result<Vec<Candidate>, String> {
            Err("yt-dlp is missing".into())
        }
        fn stream(&self, _id: &str, _itag: Option<&str>) -> Result<Stream, String> {
            Err("yt-dlp is missing".into())
        }
    }
    let client = super::public_client();
    let mut ways: Vec<Box<dyn Fn() -> Box<dyn Way> + Send + Sync>> =
        vec![Box::new(|| Box::new(Broken) as Box<dyn Way>)];
    for base in super::invidious::instances(client).unwrap_or_default() {
        ways.push(Box::new(move || router::public_way(Kind::Invidious, &base)));
    }
    for base in super::piped::instances(client).unwrap_or_default() {
        ways.push(Box::new(move || router::public_way(Kind::Piped, &base)));
    }
    let router = Router::new(Box::new(Only(ways)));
    let track = deezer(SONGS[0]);
    let seconds = play(&router, &track);
    assert_same_song(&track, seconds);
}
