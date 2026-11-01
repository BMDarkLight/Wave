// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Gets to YouTube by whatever way works right now, without the listener
//! noticing which.
//!
//! The ways are tried in order: yt-dlp on this computer, then each paired
//! desktop Wave on the network that can run it, then public Invidious and
//! Piped servers. A way that fails rests for a while so the next song doesn't
//! wait on it again, and the last public server that worked is tried first.
//!
//! A song keeps the audio format it started with, whichever way later
//! fetches it. The same format is byte for byte the same file everywhere, so
//! what is already cached stays good when playback moves to another way in
//! mid-song, which is what happens when a stream URL expires or a server
//! drops out.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Response;

use super::matching::{self, Wanted};
use super::{Candidate, Stream};

/// What the listener is told when no way in works.
pub const UNREACHABLE: &str = "YouTube couldn't be reached, not directly, through your other \
     Waves, or through public servers. Try again in a little while.";
/// How long a failed way is left alone.
const REST: Duration = Duration::from_secs(10 * 60);
/// Public servers of one kind tried per request before moving on, so a run of
/// dead ones can't hold a song up for long.
const PUBLIC_TRIES: usize = 3;
/// A stream URL this close to expiring is fetched afresh first.
const EXPIRY_MARGIN_SECONDS: u64 = 120;
/// Ways a single fetch may go through before giving up.
const FETCH_ATTEMPTS: usize = 4;
/// Results asked for when searching for a song.
const SEARCH_RESULTS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// yt-dlp on this computer. Never on Android, which can't run it.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    Direct,
    /// A paired desktop Wave, relaying through its own yt-dlp.
    Wave,
    Invidious,
    Piped,
}

/// One way in to YouTube.
pub trait Way: Send + Sync {
    /// Names this way among the others, for resting and remembering it.
    fn key(&self) -> String;
    fn kind(&self) -> Kind;
    fn search(&self, query: &str) -> Result<Vec<Candidate>, String>;
    /// The audio for `video_id`; with `itag`, in that format only.
    fn stream(&self, video_id: &str, itag: Option<&str>) -> Result<Stream, String>;
}

/// The ways open right now, in the order to try them.
pub trait Ways: Send + Sync {
    fn ways(&self) -> Vec<Box<dyn Way>>;
}

/// The format a song started in, kept for as long as Wave runs.
#[derive(Debug, Clone, PartialEq)]
struct Pin {
    itag: String,
    size: Option<u64>,
}

#[derive(Default)]
struct State {
    resting: HashMap<String, Instant>,
    last_good: HashMap<Kind, String>,
    pins: HashMap<String, Pin>,
    /// The stream each song is playing from, and the way it came by.
    current: HashMap<String, (Stream, String)>,
}

pub struct Router {
    ways: Box<dyn Ways>,
    state: Mutex<State>,
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fresh(stream: &Stream) -> bool {
    stream
        .expires_at
        .is_none_or(|at| at > now_seconds() + EXPIRY_MARGIN_SECONDS)
}

impl Router {
    pub fn new(ways: Box<dyn Ways>) -> Self {
        Self {
            ways,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The ways to try, resting ones and `skip` left out, with the public
    /// server that last worked first among its kind.
    fn open_ways(&self, skip: &[String]) -> Vec<Box<dyn Way>> {
        let mut ways = self.ways.ways();
        let state = self.state();
        let now = Instant::now();
        ways.retain(|way| {
            let key = way.key();
            !skip.contains(&key) && state.resting.get(&key).is_none_or(|&until| until <= now)
        });
        // A stable sort keeps the order between kinds and moves each kind's
        // last good way to its front.
        ways.sort_by_key(|way| {
            let kind_rank = match way.kind() {
                Kind::Direct => 0,
                Kind::Wave => 1,
                Kind::Invidious => 2,
                Kind::Piped => 3,
            };
            let favourite = state.last_good.get(&way.kind()) == Some(&way.key());
            (kind_rank, !favourite)
        });
        ways
    }

    fn rest(&self, key: &str, why: &str) {
        tracing::debug!("Online play: resting {key}: {why}");
        self.state()
            .resting
            .insert(key.to_string(), Instant::now() + REST);
    }

    fn worked(&self, way: &dyn Way) {
        self.state().last_good.insert(way.kind(), way.key());
    }

    /// Runs `attempt` on each open way until one answers.
    fn first_answer<T>(
        &self,
        skip: &[String],
        mut attempt: impl FnMut(&dyn Way) -> Result<T, String>,
    ) -> Result<(T, String), String> {
        let mut public_tries: HashMap<Kind, usize> = HashMap::new();
        for way in self.open_ways(skip) {
            if matches!(way.kind(), Kind::Invidious | Kind::Piped) {
                let tries = public_tries.entry(way.kind()).or_default();
                if *tries >= PUBLIC_TRIES {
                    continue;
                }
                *tries += 1;
            }
            match attempt(way.as_ref()) {
                Ok(found) => {
                    self.worked(way.as_ref());
                    return Ok((found, way.key()));
                }
                Err(e) => self.rest(&way.key(), &e),
            }
        }
        Err(UNREACHABLE.to_string())
    }

    pub fn search(&self, query: &str) -> Result<Vec<Candidate>, String> {
        self.first_answer(&[], |way| way.search(query))
            .map(|(found, _)| found)
    }

    /// The video that is `wanted`, as an audio reference ready to play: its
    /// stream is already found, so playback starts at once.
    pub fn find(&self, wanted: &Wanted) -> Result<String, FindError> {
        let candidates = self
            .search(&matching::query(wanted))
            .map_err(FindError::Unreachable)?;
        let top: Vec<Candidate> = candidates.into_iter().take(SEARCH_RESULTS).collect();
        let video = matching::best(wanted, &top).ok_or(FindError::NoMatch)?;
        let stream = self.stream(&video.id).map_err(FindError::Unreachable)?;
        Ok(super::audio_ref(&video.id, &stream.ext))
    }

    /// The stream to play `video_id` from now.
    pub fn stream(&self, video_id: &str) -> Result<Stream, String> {
        if let Some((stream, _)) = self.state().current.get(video_id) {
            if fresh(stream) {
                return Ok(stream.clone());
            }
        }
        self.resolve(video_id, &[]).map(|(stream, _)| stream)
    }

    fn resolve(&self, video_id: &str, skip: &[String]) -> Result<(Stream, String), String> {
        let pin = self.state().pins.get(video_id).cloned();
        let (stream, key) = self.first_answer(skip, |way| {
            let stream = way.stream(video_id, pin.as_ref().map(|p| p.itag.as_str()))?;
            match &pin {
                // A different file under the same format would garble what
                // is already cached.
                Some(Pin {
                    size: Some(want), ..
                }) if stream.filesize.is_some_and(|have| have != *want) => {
                    Err("served a different file for this song".to_string())
                }
                _ => Ok(stream),
            }
        })?;
        let mut state = self.state();
        if pin.is_none() {
            if let Some(itag) = &stream.itag {
                state.pins.insert(
                    video_id.to_string(),
                    Pin {
                        itag: itag.clone(),
                        size: stream.filesize,
                    },
                );
            }
        }
        state
            .current
            .insert(video_id.to_string(), (stream.clone(), key.clone()));
        Ok((stream, key))
    }

    /// Bytes of `video_id` for an HTTP `range`, moving to the next way in
    /// whenever the current one stops answering.
    pub fn fetch(&self, video_id: &str, range: &str) -> Result<Response, String> {
        let mut failed: Vec<String> = Vec::new();
        for _ in 0..FETCH_ATTEMPTS {
            let current = self.state().current.get(video_id).cloned();
            let (stream, key) = match current {
                Some((stream, key)) if fresh(&stream) && !failed.contains(&key) => (stream, key),
                _ => self.resolve(video_id, &failed)?,
            };
            let mut request = crate::remote::http::stream_client()
                .get(&stream.url)
                .header("Range", range);
            for (name, value) in &stream.http_headers {
                request = request.header(name.as_str(), value.as_str());
            }
            let why = match request.send() {
                Ok(resp) if resp.status().as_u16() == 206 => return Ok(resp),
                Ok(resp) if resp.status().as_u16() == 200 && range.starts_with("bytes=0-") => {
                    return Ok(resp)
                }
                Ok(resp) => format!("answered {}", resp.status().as_u16()),
                Err(e) => e.to_string(),
            };
            self.rest(&key, &why);
            self.state().current.remove(video_id);
            failed.push(key);
        }
        Err(UNREACHABLE.to_string())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FindError {
    /// No way in answered.
    Unreachable(String),
    /// YouTube answered, but nothing it had was this song.
    NoMatch,
}

static ROUTER: OnceLock<Router> = OnceLock::new();

/// Set up the router the app plays through. Later calls change nothing.
pub fn init(app: &tauri::AppHandle) {
    let _ = ROUTER.set(Router::new(Box::new(app_ways::AppWays::new(app.clone()))));
}

/// The app's router, once [`init`] has run.
pub fn global() -> Option<&'static Router> {
    ROUTER.get()
}

/// Answers a paired Wave from this computer's own yt-dlp. Never passes the
/// request on, so two Waves can't send it round in circles.
#[cfg(not(target_os = "android"))]
pub mod direct {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    use super::{fresh, Candidate, Stream};
    use crate::sources::youtube::ytdlp::YtDlp;

    fn ytdlp(app: &tauri::AppHandle) -> Result<YtDlp, String> {
        let dir = crate::sources::youtube::commands::tools_dir(app)?;
        YtDlp::in_dir(&dir).ok_or_else(|| "Online play isn't installed on this computer".into())
    }

    /// Streams by video id and format.
    type Streams = HashMap<(String, Option<String>), Stream>;

    pub fn search(app: &tauri::AppHandle, query: &str) -> Result<Vec<Candidate>, String> {
        ytdlp(app)?.search(query, super::SEARCH_RESULTS)
    }

    /// Streams are looked up once and reused until they near expiry: the
    /// other Wave asks again for every range it reads.
    pub fn stream(
        app: &tauri::AppHandle,
        video_id: &str,
        itag: Option<&str>,
    ) -> Result<Stream, String> {
        static CACHE: OnceLock<Mutex<Streams>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        let key = (video_id.to_string(), itag.map(String::from));
        if let Some(stream) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
            if fresh(&stream) {
                return Ok(stream);
            }
        }
        let stream = ytdlp(app)?.stream_format(video_id, itag)?;
        if let Ok(mut cache) = cache.lock() {
            cache.retain(|_, s| fresh(s));
            cache.insert(key, stream.clone());
        }
        Ok(stream)
    }
}

mod app_ways {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use super::{Kind, Way, Ways};
    use crate::peers::PeerRecord;
    use crate::sources::youtube::{invidious, piped, public_client, Candidate, Stream};

    /// Public server lists are looked up again after this long.
    const LIST_AGE: Duration = Duration::from_secs(6 * 60 * 60);

    type Listing = Mutex<Option<(Instant, Vec<String>)>>;

    pub struct AppWays {
        #[cfg_attr(target_os = "android", allow(dead_code))]
        app: tauri::AppHandle,
        invidious: Listing,
        piped: Listing,
    }

    impl AppWays {
        pub fn new(app: tauri::AppHandle) -> Self {
            Self {
                app,
                invidious: Mutex::new(None),
                piped: Mutex::new(None),
            }
        }
    }

    /// The servers in `listing`, looked up with `fetch` when missing or old.
    /// A failed lookup keeps the old list rather than none.
    fn servers(
        listing: &Listing,
        fetch: impl FnOnce() -> Result<Vec<String>, String>,
    ) -> Vec<String> {
        let mut listing = listing.lock().unwrap_or_else(|e| e.into_inner());
        let stale = listing
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= LIST_AGE);
        if stale {
            match fetch() {
                Ok(found) => *listing = Some((Instant::now(), found)),
                Err(e) => {
                    tracing::debug!("Online play: server list unavailable: {e}");
                    // Try again in a few minutes, not on every song.
                    let kept = listing.take().map(|(_, l)| l).unwrap_or_default();
                    *listing = Some((Instant::now() - LIST_AGE + Duration::from_secs(300), kept));
                }
            }
        }
        listing.as_ref().map(|(_, l)| l.clone()).unwrap_or_default()
    }

    impl Ways for AppWays {
        fn ways(&self) -> Vec<Box<dyn Way>> {
            let mut ways: Vec<Box<dyn Way>> = Vec::new();
            #[cfg(not(target_os = "android"))]
            if crate::sources::youtube::commands::tools_installed(&self.app) {
                ways.push(Box::new(Direct(self.app.clone())));
            }
            for (peer, own_id) in crate::peers::online_paired() {
                ways.push(Box::new(WaveWay { peer, own_id }));
            }
            let client = public_client();
            for base in servers(&self.invidious, || invidious::instances(client)) {
                ways.push(Box::new(Public {
                    kind: Kind::Invidious,
                    base,
                }));
            }
            for base in servers(&self.piped, || piped::instances(client)) {
                ways.push(Box::new(Public {
                    kind: Kind::Piped,
                    base,
                }));
            }
            ways
        }
    }

    #[cfg(not(target_os = "android"))]
    struct Direct(tauri::AppHandle);

    #[cfg(not(target_os = "android"))]
    impl Way for Direct {
        fn key(&self) -> String {
            "direct".into()
        }
        fn kind(&self) -> Kind {
            Kind::Direct
        }
        fn search(&self, query: &str) -> Result<Vec<Candidate>, String> {
            super::direct::search(&self.0, query)
        }
        fn stream(&self, video_id: &str, itag: Option<&str>) -> Result<Stream, String> {
            super::direct::stream(&self.0, video_id, itag)
        }
    }

    struct WaveWay {
        peer: PeerRecord,
        own_id: String,
    }

    impl Way for WaveWay {
        fn key(&self) -> String {
            format!("wave:{}", self.peer.id)
        }
        fn kind(&self) -> Kind {
            Kind::Wave
        }
        fn search(&self, query: &str) -> Result<Vec<Candidate>, String> {
            crate::peers::client::youtube_search(&self.own_id, &self.peer, query)
                .map_err(|e| e.to_string())
        }
        fn stream(&self, video_id: &str, itag: Option<&str>) -> Result<Stream, String> {
            crate::peers::client::youtube_stream(&self.own_id, &self.peer, video_id, itag)
                .map_err(|e| e.to_string())
        }
    }

    struct Public {
        kind: Kind,
        base: String,
    }

    impl Way for Public {
        fn key(&self) -> String {
            self.base.clone()
        }
        fn kind(&self) -> Kind {
            self.kind
        }
        fn search(&self, query: &str) -> Result<Vec<Candidate>, String> {
            match self.kind {
                Kind::Invidious => invidious::search(public_client(), &self.base, query),
                _ => piped::search(public_client(), &self.base, query),
            }
        }
        fn stream(&self, video_id: &str, itag: Option<&str>) -> Result<Stream, String> {
            let stream = match self.kind {
                Kind::Invidious => invidious::stream(public_client(), &self.base, video_id, itag)?,
                _ => piped::stream(public_client(), &self.base, video_id, itag)?,
            };
            // Many public servers list streams they then refuse to serve.
            // One byte proves this one does, before the song depends on it.
            let probe = public_client()
                .get(&stream.url)
                .header("Range", "bytes=0-0")
                .send()
                .map_err(|e| e.to_string())?;
            match probe.status().as_u16() {
                200 | 206 => Ok(stream),
                code => Err(format!("its stream answered {code}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone)]
    struct Fake {
        key: &'static str,
        kind: Kind,
        works: bool,
        itags: Vec<(&'static str, u64)>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl Way for Fake {
        fn key(&self) -> String {
            self.key.into()
        }
        fn kind(&self) -> Kind {
            self.kind
        }
        fn search(&self, query: &str) -> Result<Vec<Candidate>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}:search", self.key));
            if !self.works {
                return Err("down".into());
            }
            Ok(vec![Candidate {
                id: "aaaaaaaaaaa".into(),
                title: query.into(),
                channel: Some("Artist - Topic".into()),
                duration_seconds: Some(200.0),
            }])
        }
        fn stream(&self, _id: &str, itag: Option<&str>) -> Result<Stream, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}:stream", self.key));
            if !self.works {
                return Err("down".into());
            }
            let (found, size) = self
                .itags
                .iter()
                .find(|(i, _)| itag.is_none_or(|want| want == *i))
                .ok_or("no such format")?;
            Ok(Stream {
                url: format!("https://{}/{found}", self.key),
                http_headers: Vec::new(),
                itag: Some(found.to_string()),
                ext: "webm".into(),
                codec: None,
                duration_seconds: Some(200.0),
                filesize: Some(*size),
                expires_at: Some(now_seconds() + 3600),
            })
        }
    }

    struct FakeWays(Vec<Fake>);

    impl Ways for FakeWays {
        fn ways(&self) -> Vec<Box<dyn Way>> {
            self.0
                .iter()
                .cloned()
                .map(|f| Box::new(f) as Box<dyn Way>)
                .collect()
        }
    }

    fn fake(key: &'static str, kind: Kind, works: bool, calls: &Arc<Mutex<Vec<String>>>) -> Fake {
        Fake {
            key,
            kind,
            works,
            itags: vec![("251", 1000), ("140", 900)],
            calls: calls.clone(),
        }
    }

    #[test]
    fn falls_through_to_the_first_way_that_answers() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new(Box::new(FakeWays(vec![
            fake("direct", Kind::Direct, false, &calls),
            fake("wave:pc", Kind::Wave, false, &calls),
            fake("inv", Kind::Invidious, true, &calls),
            fake("piped", Kind::Piped, true, &calls),
        ])));
        let found = router.search("song").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["direct:search", "wave:pc:search", "inv:search"]
        );

        // The failed ways rest: the next request goes straight to Invidious.
        calls.lock().unwrap().clear();
        router.stream("aaaaaaaaaaa").unwrap();
        assert_eq!(*calls.lock().unwrap(), vec!["inv:stream"]);
    }

    #[test]
    fn says_youtube_was_unreachable_when_nothing_answers() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new(Box::new(FakeWays(vec![
            fake("direct", Kind::Direct, false, &calls),
            fake("inv", Kind::Invidious, false, &calls),
        ])));
        assert_eq!(router.search("song").unwrap_err(), UNREACHABLE);
        assert_eq!(
            router.find(&Wanted {
                title: "song",
                artist: "Artist",
                duration_seconds: Some(200.0)
            }),
            Err(FindError::Unreachable(UNREACHABLE.to_string()))
        );
    }

    #[test]
    fn tries_only_a_few_public_servers_of_each_kind() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new(Box::new(FakeWays(vec![
            fake("inv1", Kind::Invidious, false, &calls),
            fake("inv2", Kind::Invidious, false, &calls),
            fake("inv3", Kind::Invidious, false, &calls),
            fake("inv4", Kind::Invidious, true, &calls),
            fake("piped", Kind::Piped, true, &calls),
        ])));
        router.search("song").unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["inv1:search", "inv2:search", "inv3:search", "piped:search"]
        );
    }

    #[test]
    fn the_public_server_that_worked_goes_first_next_time() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new(Box::new(FakeWays(vec![
            fake("inv1", Kind::Invidious, true, &calls),
            fake("inv2", Kind::Invidious, true, &calls),
        ])));
        router
            .state()
            .last_good
            .insert(Kind::Invidious, "inv2".into());
        router.search("song").unwrap();
        assert_eq!(*calls.lock().unwrap(), vec!["inv2:search"]);
    }

    #[test]
    fn a_song_keeps_its_format_on_every_way() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut aac_only = fake("inv", Kind::Invidious, true, &calls);
        aac_only.itags = vec![("140", 900)];
        let mut same = fake("piped", Kind::Piped, true, &calls);
        same.itags = vec![("251", 1000)];
        let router = Router::new(Box::new(FakeWays(vec![
            fake("direct", Kind::Direct, true, &calls),
            aac_only,
            same,
        ])));
        let first = router.stream("aaaaaaaaaaa").unwrap();
        assert_eq!(first.itag.as_deref(), Some("251"));

        // Direct goes away; Invidious only has AAC, so Piped serves the song.
        let (stream, key) = router.resolve("aaaaaaaaaaa", &["direct".into()]).unwrap();
        assert_eq!(key, "piped");
        assert_eq!(stream.itag.as_deref(), Some("251"));
    }

    #[test]
    fn a_different_file_in_the_same_format_is_refused() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut other_cut = fake("inv", Kind::Invidious, true, &calls);
        other_cut.itags = vec![("251", 1234)];
        let router = Router::new(Box::new(FakeWays(vec![
            fake("direct", Kind::Direct, true, &calls),
            other_cut,
        ])));
        router.stream("aaaaaaaaaaa").unwrap();
        assert_eq!(
            router
                .resolve("aaaaaaaaaaa", &["direct".into()])
                .unwrap_err(),
            UNREACHABLE
        );
    }

    #[test]
    fn finds_the_song_and_hands_back_a_playable_reference() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new(Box::new(FakeWays(vec![fake(
            "inv",
            Kind::Invidious,
            true,
            &calls,
        )])));
        let wanted = Wanted {
            title: "Artist song",
            artist: "Artist",
            duration_seconds: Some(200.0),
        };
        assert_eq!(router.find(&wanted).unwrap(), "youtube:aaaaaaaaaaa.webm");
        let elsewhere = Wanted {
            duration_seconds: Some(400.0),
            ..wanted
        };
        assert_eq!(router.find(&elsewhere), Err(FindError::NoMatch));
    }
}
