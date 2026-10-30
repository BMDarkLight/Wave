// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The server paired Waves talk to: the `/wave/` API, and the songs and
//! covers themselves, with byte ranges so the other side can seek.
//!
//! One thread per connection, one request per connection (`Connection:
//! close`). Only clients on the local network are answered.

use super::catalog::{mime_for, Snapshot};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const CHUNK: usize = 64 * 1024;
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 64 * 1024;
const IDLE: Duration = Duration::from_secs(15);

/// Opens a shared file for reading. Android `content://` documents need the
/// app's ContentResolver; everything else is a plain path.
pub type Opener = Arc<dyn Fn(&str) -> io::Result<std::fs::File> + Send + Sync>;

/// What the request handlers need from the server around them.
pub trait Catalog: Send + Sync {
    fn snapshot(&self) -> Result<Arc<Snapshot>, String>;
}

pub struct Server {
    pub catalog: Arc<dyn Catalog>,
    pub opener: Opener,
    pub peers: Arc<crate::peers::api::PeerService>,
}

/// Whether `ip` can only be on this machine or its local network.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_local(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// Accepts connections until `stop` is set.
pub fn serve(listener: TcpListener, server: Arc<Server>, stop: Arc<AtomicBool>) {
    let _ = listener.set_nonblocking(true);
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if !is_local(peer.ip()) {
                    tracing::debug!("Refused a request from {peer}");
                    continue;
                }
                let server = server.clone();
                let spawned = std::thread::Builder::new()
                    .name("wave-share-http".into())
                    .spawn(move || {
                        let _ = stream.set_nonblocking(false);
                        if let Err(e) = handle(stream, &server) {
                            tracing::debug!("A request from another Wave ended: {e}");
                        }
                    });
                if let Err(e) = spawned {
                    tracing::warn!("Could not answer another Wave: {e}");
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                tracing::debug!("Accepting a connection failed: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[derive(Debug, Default)]
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn read_request(stream: &TcpStream) -> io::Result<Request> {
    let mut reader = BufReader::new(stream.take((MAX_HEAD + MAX_BODY) as u64));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let mut request = Request {
        method: parts.next().unwrap_or_default().to_ascii_uppercase(),
        path: parts.next().unwrap_or_default().to_string(),
        ..Request::default()
    };
    let mut head = line.len();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        head += line.len();
        if head > MAX_HEAD {
            return Err(io::Error::other("request head too large"));
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            request
                .headers
                .push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    let length: usize = request
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if length > MAX_BODY {
        return Err(io::Error::other("request body too large"));
    }
    request.body = vec![0; length];
    reader.read_exact(&mut request.body)?;
    Ok(request)
}

fn server_header() -> String {
    format!(
        "{}/1 Wave/{}",
        std::env::consts::OS,
        env!("CARGO_PKG_VERSION")
    )
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, String)],
    body: &[u8],
    head_only: bool,
) -> io::Result<()> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nServer: {}\r\nConnection: close\r\n",
        server_header()
    );
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    stream.write_all(out.as_bytes())?;
    if !head_only {
        stream.write_all(body)?;
    }
    stream.flush()
}

fn handle(mut stream: TcpStream, server: &Server) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE))?;
    stream.set_write_timeout(Some(IDLE))?;
    let request = read_request(&stream)?;
    let head_only = request.method == "HEAD";
    let (path, query) = request
        .path
        .split_once('?')
        .unwrap_or((request.path.as_str(), ""));
    let path = path.to_string();
    if path.starts_with("/wave/") {
        return wave(&mut stream, server, &request, &path, query, head_only);
    }
    respond(&mut stream, "404 Not Found", &[], b"", head_only)
}

/// A request for the API paired Waves use.
fn wave(
    stream: &mut TcpStream,
    server: &Server,
    request: &Request,
    path: &str,
    query: &str,
    head_only: bool,
) -> io::Result<()> {
    let peers = &server.peers;
    let from = stream.peer_addr()?.ip();
    let snapshot = server.catalog.snapshot().ok();
    let reply = peers.handle(
        &crate::peers::api::PeerRequest {
            method: &request.method,
            path,
            query,
            peer: request.header("X-Wave-Peer"),
            secret: request.header("X-Wave-Secret"),
            body: &request.body,
            from,
        },
        snapshot.as_deref(),
    );
    use crate::peers::api::PeerReply;
    match reply {
        Some(PeerReply::Json(status, body)) => respond(
            stream,
            status,
            &[("Content-Type", "application/json".to_string())],
            body.as_bytes(),
            head_only,
        ),
        Some(PeerReply::Media(id)) => media(stream, server, request, &id, head_only),
        Some(PeerReply::Art(id)) => art(stream, server, &id, head_only),
        None => respond(stream, "404 Not Found", &[], b"", head_only),
    }
}

/// What a `Range` header asks for.
#[derive(Debug, PartialEq)]
enum Wanted {
    /// No usable byte range: the whole file goes, as RFC 9110 says for a
    /// range in another unit or one that does not parse.
    Whole,
    Part(u64, u64),
    /// A byte range that starts past the end.
    Unsatisfiable,
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n`, resolved against `len`. Only the
/// first of several ranges is served.
fn wanted_range(value: &str, len: u64) -> Wanted {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return Wanted::Whole;
    };
    let Some((start, end)) = spec.split(',').next().unwrap_or("").trim().split_once('-') else {
        return Wanted::Whole;
    };
    if start.is_empty() {
        return match end.parse::<u64>() {
            Ok(0) => Wanted::Unsatisfiable,
            Ok(_) if len == 0 => Wanted::Unsatisfiable,
            Ok(suffix) => Wanted::Part(len.saturating_sub(suffix), len - 1),
            Err(_) => Wanted::Whole,
        };
    }
    let Ok(start) = start.parse::<u64>() else {
        return Wanted::Whole;
    };
    let end = match end {
        "" => None,
        e => match e.parse::<u64>() {
            Ok(end) if end >= start => Some(end),
            _ => return Wanted::Whole,
        },
    };
    if start >= len {
        return Wanted::Unsatisfiable;
    }
    Wanted::Part(start, end.map_or(len - 1, |end| end.min(len - 1)))
}

fn media(
    stream: &mut TcpStream,
    server: &Server,
    request: &Request,
    id: &str,
    head_only: bool,
) -> io::Result<()> {
    let found = server
        .catalog
        .snapshot()
        .ok()
        .and_then(|snapshot| snapshot.track(id).cloned());
    let Some(track) = found else {
        return respond(stream, "404 Not Found", &[], b"", head_only);
    };
    let mut file = match (server.opener)(&track.path) {
        Ok(file) => file,
        Err(e) => {
            tracing::debug!("Could not open a shared track: {e}");
            return respond(stream, "404 Not Found", &[], b"", head_only);
        }
    };
    let len = file.metadata().map(|m| m.len())?;
    let extension = super::catalog::extension_of(&track);
    let mut headers = vec![
        ("Content-Type", mime_for(&extension).to_string()),
        ("Accept-Ranges", "bytes".to_string()),
    ];
    let wanted = request
        .header("Range")
        .map_or(Wanted::Whole, |range| wanted_range(range, len));
    let (status, start, end) = match wanted {
        Wanted::Whole => ("200 OK", 0, len.saturating_sub(1)),
        Wanted::Part(start, end) => {
            headers.push(("Content-Range", format!("bytes {start}-{end}/{len}")));
            ("206 Partial Content", start, end)
        }
        Wanted::Unsatisfiable => {
            headers.push(("Content-Range", format!("bytes */{len}")));
            return respond(
                stream,
                "416 Range Not Satisfiable",
                &headers,
                b"",
                head_only,
            );
        }
    };
    let length = if len == 0 { 0 } else { end - start + 1 };
    let mut out = format!(
        "HTTP/1.1 {status}\r\nServer: {}\r\nConnection: close\r\n",
        server_header()
    );
    for (name, value) in &headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!("Content-Length: {length}\r\n\r\n"));
    stream.write_all(out.as_bytes())?;
    if head_only || length == 0 {
        return stream.flush();
    }
    file.seek(SeekFrom::Start(start))?;
    let mut remaining = length;
    let mut buf = vec![0u8; CHUNK];
    while remaining > 0 {
        let want = remaining.min(CHUNK as u64) as usize;
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        stream.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    stream.flush()
}

/// The best cover there is for a track: the one inside its file at full
/// size, else the 512px copy saved with the album art, else the 96px
/// thumbnail. The other Wave keeps what it is sent and shows it large.
fn best_cover(server: &Server, track: &crate::metadata::Track) -> Option<(Vec<u8>, String)> {
    if let Some(found) = (server.opener)(&track.path)
        .ok()
        .and_then(crate::remote::tags::embedded_picture)
    {
        return Some(found);
    }
    // The list query fills this with the absolute thumbnail path.
    let thumb = track
        .cover_art_data_url
        .as_deref()
        .filter(|path| !path.starts_with("data:"))?;
    let best = crate::cover_art::prefer_media_artwork_url(Some(thumb))?;
    std::fs::read(best)
        .ok()
        .map(|bytes| (bytes, "image/jpeg".to_string()))
}

fn art(stream: &mut TcpStream, server: &Server, id: &str, head_only: bool) -> io::Result<()> {
    let cover = server
        .catalog
        .snapshot()
        .ok()
        .and_then(|snapshot| best_cover(server, snapshot.track(id)?));
    match cover {
        Some((bytes, mime)) => respond(
            stream,
            "200 OK",
            &[("Content-Type", mime)],
            &bytes,
            head_only,
        ),
        None => respond(stream, "404 Not Found", &[], b"", head_only),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::Track;
    use std::net::Ipv4Addr;

    struct Fixed(Arc<Snapshot>);
    impl Catalog for Fixed {
        fn snapshot(&self) -> Result<Arc<Snapshot>, String> {
            Ok(self.0.clone())
        }
    }

    /// A paired Wave with id "phone" and secret `PEER_SECRET`, shared `share`.
    struct OnePeer(crate::peers::ShareScope);
    const PEER_SECRET: &str = "0123456789abcdef0123";
    impl crate::peers::api::Host for OnePeer {
        fn identity(&self) -> (String, String) {
            ("1234".into(), "Wave on test".into())
        }
        fn peers(&self) -> Vec<crate::peers::PeerRecord> {
            vec![crate::peers::PeerRecord {
                id: "phone".into(),
                name: "Wave on Android".into(),
                address: "http://127.0.0.1:1".into(),
                secret: PEER_SECRET.into(),
                share: self.0.clone(),
                source_id: None,
                paired_at: 0,
            }]
        }
        fn update_peers(&self, _change: &mut dyn FnMut(&mut Vec<crate::peers::PeerRecord>)) {}
        fn fingerprints(&self) -> std::collections::HashMap<String, String> {
            Default::default()
        }
        fn emit(&self, _event: &str, _payload: serde_json::Value) {}
        fn paired(&self, _peer: &crate::peers::PeerRecord) {}
        fn forgotten(&self, _peer_id: &str) {}
        fn reachable(&self, _peer_id: &str, _online: bool) {}
    }

    /// A running server over one 1000-byte song, and its port.
    fn running() -> (u16, Arc<AtomicBool>, std::path::PathBuf) {
        running_with(crate::peers::ShareScope::Everything)
    }

    fn running_with(share: crate::peers::ShareScope) -> (u16, Arc<AtomicBool>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("wave-share-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let song = dir.join("song.mp3");
        let bytes: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&song, &bytes).unwrap();
        let thumb = dir.join("thumb.jpg");
        std::fs::write(&thumb, b"\xFF\xD8 a tiny jpeg").unwrap();
        let track = Track {
            id: "t 1".into(),
            path: song.to_string_lossy().into_owned(),
            title: "Song".into(),
            artist: "Band".into(),
            album: "Album".into(),
            format: "mp3".into(),
            file_size: 1000,
            album_art_id: Some("art".into()),
            cover_art_data_url: Some(thumb.to_string_lossy().into_owned()),
            ..Track::default()
        };
        let snapshot = Arc::new(Snapshot::build(42, vec![track]));
        let server = Arc::new(Server {
            catalog: Arc::new(Fixed(snapshot)),
            opener: Arc::new(|path: &str| std::fs::File::open(path)),
            peers: Arc::new(crate::peers::api::PeerService::new(Arc::new(OnePeer(
                share,
            )))),
        });
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || serve(listener, server, flag));
        (port, stop, song)
    }

    fn exchange(port: u16, request: &str) -> (String, Vec<u8>) {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).unwrap();
        let split = reply.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (
            String::from_utf8_lossy(&reply[..split]).into_owned(),
            reply[split + 4..].to_vec(),
        )
    }

    #[test]
    fn local_addresses_only() {
        assert!(is_local("192.168.1.9".parse().unwrap()));
        assert!(is_local("127.0.0.1".parse().unwrap()));
        assert!(is_local("fe80::1".parse().unwrap()));
        assert!(is_local("::ffff:10.0.0.2".parse().unwrap()));
        assert!(!is_local("8.8.8.8".parse().unwrap()));
        assert!(!is_local("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn ranges_follow_rfc_7233() {
        assert_eq!(wanted_range("bytes=0-499", 1000), Wanted::Part(0, 499));
        assert_eq!(wanted_range("bytes=500-", 1000), Wanted::Part(500, 999));
        assert_eq!(wanted_range("bytes=-100", 1000), Wanted::Part(900, 999));
        assert_eq!(wanted_range("bytes=900-5000", 1000), Wanted::Part(900, 999));
        assert_eq!(wanted_range("bytes=0-9,20-29", 1000), Wanted::Part(0, 9));
        assert_eq!(wanted_range("bytes=1000-", 1000), Wanted::Unsatisfiable);
        assert_eq!(wanted_range("bytes=-0", 1000), Wanted::Unsatisfiable);
        // Not a byte range Wave can read: the whole file, not an error.
        assert_eq!(wanted_range("bytes=5-2", 1000), Wanted::Whole);
        assert_eq!(wanted_range("items=0-1", 1000), Wanted::Whole);
        assert_eq!(wanted_range("bytes=abc-", 1000), Wanted::Whole);
        assert_eq!(wanted_range("bytes=", 1000), Wanted::Whole);
    }

    #[test]
    fn only_the_wave_api_is_served() {
        let (port, stop, _) = running();
        let (head, _) = exchange(port, "GET /rootDesc.xml HTTP/1.1\r\n\r\n");
        assert!(head.starts_with("HTTP/1.1 404"), "{head}");
        let (head, body) = exchange(port, "GET /wave/hello HTTP/1.1\r\n\r\n");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(String::from_utf8_lossy(&body).contains("\"id\":\"1234\""));
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn a_paired_wave_streams_what_it_was_given() {
        let (port, stop, _) = running_with(crate::peers::ShareScope::Tracks(vec!["t 1".into()]));
        let auth = format!("X-Wave-Peer: phone\r\nX-Wave-Secret: {PEER_SECRET}\r\n");
        let (head, body) = exchange(
            port,
            &format!("GET /wave/media/t%201.mp3 HTTP/1.1\r\n{auth}Range: bytes=10-19\r\n\r\n"),
        );
        assert!(head.starts_with("HTTP/1.1 206"), "{head}");
        assert_eq!(
            body,
            (10..20u32).map(|i| (i % 251) as u8).collect::<Vec<_>>()
        );
        let (head, _) = exchange(port, "GET /wave/media/t%201.mp3 HTTP/1.1\r\n\r\n");
        assert!(head.starts_with("HTTP/1.1 403"), "{head}");
        let (head, body) = exchange(
            port,
            &format!("GET /wave/catalog?cursor=0 HTTP/1.1\r\n{auth}\r\n"),
        );
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(String::from_utf8_lossy(&body).contains("\"total\":1"));
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn a_paired_wave_gets_nothing_outside_its_share() {
        let (port, stop, _) = running_with(crate::peers::ShareScope::Nothing);
        let auth = format!("X-Wave-Peer: phone\r\nX-Wave-Secret: {PEER_SECRET}\r\n");
        let (head, _) = exchange(
            port,
            &format!("GET /wave/media/t%201.mp3 HTTP/1.1\r\n{auth}\r\n"),
        );
        assert!(head.starts_with("HTTP/1.1 404"), "{head}");
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn media_is_served_whole_or_in_ranges() {
        let (port, stop, song) = running();
        let bytes = std::fs::read(&song).unwrap();
        let get = |method: &str, path: &str, extra: &str| {
            exchange(
                port,
                &format!(
                    "{method} /wave/{path} HTTP/1.1\r\nX-Wave-Peer: phone\r\nX-Wave-Secret: {PEER_SECRET}\r\n{extra}\r\n"
                ),
            )
        };

        let (head, body) = get("GET", "media/t%201.mp3", "");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("Content-Type: audio/mpeg"));
        assert!(head.contains("Accept-Ranges: bytes"));
        assert_eq!(body, bytes);

        let (head, body) = get("GET", "media/t%201.mp3", "Range: bytes=100-199\r\n");
        assert!(head.starts_with("HTTP/1.1 206"));
        assert!(head.contains("Content-Range: bytes 100-199/1000"));
        assert_eq!(body, bytes[100..200]);

        let (head, body) = get("HEAD", "media/t%201.mp3", "");
        assert!(head.contains("Content-Length: 1000"));
        assert!(body.is_empty());

        let (head, _) = get("GET", "media/t%201.mp3", "Range: bytes=2000-\r\n");
        assert!(head.starts_with("HTTP/1.1 416"));
        assert!(head.contains("Content-Range: bytes */1000"));

        let (head, _) = get("GET", "media/unknown.mp3", "");
        assert!(head.starts_with("HTTP/1.1 404"));
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn covers_are_served_from_the_thumbnail() {
        let (port, stop, _) = running();
        let auth = format!("X-Wave-Peer: phone\r\nX-Wave-Secret: {PEER_SECRET}\r\n");
        let (head, body) = exchange(
            port,
            &format!("GET /wave/art/t%201.jpg HTTP/1.1\r\n{auth}\r\n"),
        );
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("Content-Type: image/jpeg"));
        assert_eq!(body, b"\xFF\xD8 a tiny jpeg");
        let (head, _) = exchange(
            port,
            &format!("GET /wave/art/unknown.jpg HTTP/1.1\r\n{auth}\r\n"),
        );
        assert!(head.starts_with("HTTP/1.1 404"));
        stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod pairing_tests {
    use super::*;
    use crate::metadata::Track;
    use crate::peers::api::{AnswerBody, AskBody, Host, PeerService, SharedTrack, UnpairBody};
    use crate::peers::cast::{
        CastControl, CastControlBody, CastItem, CastPhase, CastPlay, CastState,
    };
    use crate::peers::{client, PeerRecord, ShareScope};
    use std::collections::HashMap;
    use std::net::Ipv4Addr;
    use std::sync::Mutex;

    struct Fixed(Arc<Snapshot>);
    impl Catalog for Fixed {
        fn snapshot(&self) -> Result<Arc<Snapshot>, String> {
            Ok(self.0.clone())
        }
    }

    /// A Wave's settings and events, kept in memory.
    struct MemHost {
        id: String,
        peers: Mutex<Vec<PeerRecord>>,
        events: Mutex<Vec<String>>,
        /// What paired devices asked this one to play, last first.
        casts: Mutex<Vec<CastPlay>>,
    }

    impl Host for MemHost {
        fn identity(&self) -> (String, String) {
            (self.id.clone(), format!("Wave on {}", self.id))
        }
        fn peers(&self) -> Vec<PeerRecord> {
            self.peers.lock().unwrap().clone()
        }
        fn update_peers(&self, change: &mut dyn FnMut(&mut Vec<PeerRecord>)) {
            change(&mut self.peers.lock().unwrap());
        }
        fn fingerprints(&self) -> HashMap<String, String> {
            HashMap::new()
        }
        fn emit(&self, event: &str, _payload: serde_json::Value) {
            self.events.lock().unwrap().push(event.to_string());
        }
        fn paired(&self, _peer: &PeerRecord) {}
        fn forgotten(&self, _peer_id: &str) {}
        fn reachable(&self, _peer_id: &str, _online: bool) {}
        fn cast_play(&self, _from: &PeerRecord, play: CastPlay) -> Result<(), String> {
            self.casts.lock().unwrap().insert(0, play);
            Ok(())
        }
        fn cast_control(&self, _from: &PeerRecord, control: CastControlBody) -> Result<(), String> {
            let mut casts = self.casts.lock().unwrap();
            let current = casts.first_mut().filter(|c| c.session == control.session);
            let current = current.ok_or("nothing of yours plays here")?;
            if let CastControl::Pause = control.control {
                current.playing = false;
            }
            Ok(())
        }
        fn cast_state(&self, _from: &PeerRecord) -> CastState {
            let casts = self.casts.lock().unwrap();
            let current = casts.first();
            CastState {
                session: current.map_or(0, |c| c.session),
                phase: match current {
                    Some(c) if c.playing => CastPhase::Playing,
                    Some(_) => CastPhase::Paused,
                    None => CastPhase::Idle,
                },
                position: current.map_or(0.0, |c| c.position),
                duration: None,
            }
        }
    }

    /// A Wave's settings and events, its peer service and port.
    type Running = (Arc<MemHost>, Arc<PeerService>, u16);

    /// A Wave serving `songs` (1000-byte files).
    fn wave(id: &str, songs: &[&str]) -> Running {
        let dir = std::env::temp_dir().join(format!("wave-pair-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let tracks = songs
            .iter()
            .map(|name| {
                let path = dir.join(format!("{name}.mp3"));
                std::fs::write(&path, vec![name.as_bytes()[0]; 1000]).unwrap();
                Track {
                    id: name.to_string(),
                    path: path.to_string_lossy().into_owned(),
                    title: name.to_string(),
                    format: "mp3".into(),
                    file_size: 1000,
                    ..Track::default()
                }
            })
            .collect();
        let host = Arc::new(MemHost {
            id: id.into(),
            peers: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            casts: Mutex::new(Vec::new()),
        });
        let peers = Arc::new(PeerService::new(host.clone()));
        let server = Arc::new(Server {
            catalog: Arc::new(Fixed(Arc::new(Snapshot::build(1, tracks)))),
            opener: Arc::new(|path: &str| std::fs::File::open(path)),
            peers: peers.clone(),
        });
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || serve(listener, server, Arc::new(AtomicBool::new(false))));
        (host, peers, port)
    }

    #[test]
    fn two_waves_pair_share_and_forget() {
        let (pc_host, pc, pc_port) = wave("pc", &["pc-song"]);
        let (phone_host, phone, phone_port) = wave("phone", &["a-song", "b-song", "c-song"]);
        let phone_address = format!("http://127.0.0.1:{phone_port}");
        let secret = "0123456789abcdef0123456789abcdef".to_string();

        // The PC asks.
        pc.asked("phone", "Wave on phone", &phone_address, &secret);
        client::ask(
            &phone_address,
            &AskBody {
                id: "pc".into(),
                name: "Wave on pc".into(),
                port: pc_port,
                secret: secret.clone(),
            },
        )
        .unwrap();
        assert!(phone_host
            .events
            .lock()
            .unwrap()
            .contains(&"wave-peer-request".to_string()));

        // The phone shares two of its songs, and says so.
        let record = phone
            .accept(
                "pc",
                ShareScope::Tracks(vec!["a-song".into(), "c-song".into()]),
            )
            .unwrap();
        assert_eq!(record.address, format!("http://127.0.0.1:{pc_port}"));
        client::answer(
            &record.address,
            &AnswerBody {
                id: "phone".into(),
                name: "Wave on phone".into(),
                port: phone_port,
                secret: record.secret.clone(),
                accepted: true,
            },
        )
        .unwrap();
        let on_pc = pc_host.peers();
        assert_eq!(on_pc.len(), 1);
        assert_eq!(on_pc[0].address, phone_address);
        assert!(pc_host
            .events
            .lock()
            .unwrap()
            .contains(&"wave-peer-answered".to_string()));

        // The PC reads what it was given, and can play it.
        let page = client::catalog(&on_pc[0], "pc", 0).unwrap();
        let ids: Vec<_> = page.tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a-song", "c-song"]);
        let response = client::signed(
            reqwest::blocking::Client::new().get(client::media_url(
                &phone_address,
                "c-song",
                "mp3",
            )),
            "pc",
            &on_pc[0],
        )
        .header("Range", "bytes=0-9")
        .send()
        .unwrap();
        assert_eq!(response.status().as_u16(), 206);
        assert_eq!(response.bytes().unwrap().as_ref(), &[b'c'; 10]);

        // The PC forgets the phone; the phone drops the pairing too.
        client::unpair(
            &phone_address,
            &UnpairBody {
                id: "pc".into(),
                secret: on_pc[0].secret.clone(),
            },
        )
        .unwrap();
        pc.forget("phone");
        assert!(phone_host.peers().is_empty());
        assert!(pc_host.peers().is_empty());
        assert_eq!(
            client::catalog(&on_pc[0], "pc", 0).unwrap_err(),
            client::CallError::Forgotten
        );
    }

    /// Pairs "pc" with "phone"; the phone shares nothing with the pc and the
    /// pc nothing with the phone.
    fn paired_pair() -> (Running, Running) {
        let pc = wave("pc", &["pc-song"]);
        let phone = wave("phone", &["a-song"]);
        let secret = "0123456789abcdef0123456789abcdef".to_string();
        let record = |id: &str, port: u16| PeerRecord {
            id: id.into(),
            name: format!("Wave on {id}"),
            address: format!("http://127.0.0.1:{port}"),
            secret: secret.clone(),
            share: ShareScope::Nothing,
            source_id: None,
            paired_at: 0,
        };
        pc.0.peers.lock().unwrap().push(record("phone", phone.2));
        phone.0.peers.lock().unwrap().push(record("pc", pc.2));
        (pc, phone)
    }

    #[test]
    fn a_wave_plays_to_another_and_lends_it_the_song() {
        let ((pc_host, pc, _), (phone_host, _, _)) = paired_pair();
        let phone = pc_host.peers()[0].clone();
        let pc_song = Track {
            id: "pc-song".into(),
            title: "pc-song".into(),
            format: "mp3".into(),
            file_size: 1000,
            ..Track::default()
        };
        let play = CastPlay {
            session: 1,
            item: CastItem::Sender {
                track: Box::new(SharedTrack::of(&pc_song, None)),
            },
            position: 30.0,
            playing: true,
        };

        // Not shared with the phone, so it can't stream it until the pc
        // plays it there.
        let stream = || {
            client::signed(
                reqwest::blocking::Client::new().get(client::media_url(
                    &pc_host_address(&phone_host),
                    "pc-song",
                    "mp3",
                )),
                "phone",
                &phone_host.peers()[0],
            )
            .header("Range", "bytes=0-3")
            .send()
            .unwrap()
            .status()
            .as_u16()
        };
        assert_eq!(stream(), 404);
        pc.grant_cast("phone", "pc-song");
        client::cast_play(&phone, "pc", &play).unwrap();
        assert_eq!(stream(), 206);
        assert_eq!(phone_host.casts.lock().unwrap()[0], play);

        let state = client::cast_state(&phone, "pc").unwrap();
        assert_eq!((state.session, state.phase), (1, CastPhase::Playing));
        client::cast_control(
            &phone,
            "pc",
            &CastControlBody {
                session: 1,
                control: CastControl::Pause,
            },
        )
        .unwrap();
        assert_eq!(
            client::cast_state(&phone, "pc").unwrap().phase,
            CastPhase::Paused
        );
        // A control for a track that is no longer playing is turned down.
        assert!(matches!(
            client::cast_control(
                &phone,
                "pc",
                &CastControlBody {
                    session: 9,
                    control: CastControl::Resume,
                },
            ),
            Err(client::CallError::Refused(400))
        ));

        // Only paired devices can play here.
        let stranger = PeerRecord {
            id: "stranger".into(),
            secret: "ffffffffffffffffffffffffffffffff".into(),
            ..phone.clone()
        };
        assert_eq!(
            client::cast_play(&stranger, "stranger", &play),
            Err(client::CallError::Forgotten)
        );
    }

    /// Where the pc answers, from the phone's record of it.
    fn pc_host_address(phone_host: &MemHost) -> String {
        phone_host.peers()[0].address.clone()
    }
}
