// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! An HTTP server on 127.0.0.1 that hands remote tracks to ExoPlayer, which
//! can only stream from a URL, straight from the cache file as it fills.

use super::reader::ProgressiveReader;
use super::{resolve_play_target_with, PlayTarget};
use crate::remote::gate::NetworkGate;
use crate::remote::uri::{percent_decode, RemoteUri};
use crate::remote::RemoteRuntime;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

const CHUNK: usize = 64 * 1024;
const MAX_HEAD: u64 = 8 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one read waits on the fill before the connection gives up. The
/// player reconnects from where it stopped.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Loopback {
    /// `http://127.0.0.1:{port}/{secret}/`. Holds the secret, so never log it.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub base: String,
}

enum Refusal {
    NotFound,
    Unavailable(String),
}

type Resolve = dyn Fn(&str) -> Result<PlayTarget, Refusal> + Send + Sync;

/// Starts the server once and returns it on every later call.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn start_loopback() -> io::Result<&'static Loopback> {
    static LOOPBACK: Mutex<Option<&'static Loopback>> = Mutex::new(None);
    let mut slot = LOOPBACK.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(loopback) = *slot {
        return Ok(loopback);
    }
    let loopback: &'static Loopback = Box::leak(Box::new(bind(Arc::new(resolve))?));
    *slot = Some(loopback);
    Ok(loopback)
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn resolve(path: &str) -> Result<PlayTarget, Refusal> {
    let rt = RemoteRuntime::global()
        .ok_or_else(|| Refusal::Unavailable("Remote music isn't set up".into()))?;
    resolve_with(rt, NetworkGate::for_path(path), path)
}

fn resolve_with(
    rt: &'static RemoteRuntime,
    gate: &'static NetworkGate,
    path: &str,
) -> Result<PlayTarget, Refusal> {
    // Only library tracks, never an arbitrary path on the device.
    let uri = RemoteUri::parse(path).ok_or(Refusal::NotFound)?;
    match rt.with_library(|l| l.remote_stream_info(&uri)) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(Refusal::NotFound),
        Err(e) => return Err(Refusal::Unavailable(e)),
    }
    resolve_play_target_with(rt, gate, path).map_err(Refusal::Unavailable)
}

fn bind(resolve: Arc<Resolve>) -> io::Result<Loopback> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let secret = uuid::Uuid::new_v4().simple().to_string();
    let prefix: Arc<str> = format!("/{secret}/").into();
    std::thread::Builder::new()
        .name("wave-loopback".into())
        .spawn(move || accept(listener, prefix, resolve))?;
    Ok(Loopback {
        base: format!("http://127.0.0.1:{port}/{secret}/"),
    })
}

fn accept(listener: TcpListener, prefix: Arc<str>, resolve: Arc<Resolve>) {
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                tracing::debug!("Loopback accept failed: {e}");
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let prefix = prefix.clone();
        let resolve = resolve.clone();
        let spawned = std::thread::Builder::new()
            .name("wave-loopback-conn".into())
            .spawn(move || serve(stream, &prefix, resolve.as_ref()));
        if let Err(e) = spawned {
            tracing::warn!("Could not start a loopback connection: {e}");
        }
    }
}

fn serve(mut stream: TcpStream, prefix: &str, resolve: &Resolve) {
    if let Err(e) = respond(&mut stream, prefix, resolve) {
        // The player hanging up mid-body is routine.
        tracing::debug!("Loopback connection ended: {e}");
    }
}

fn respond(stream: &mut TcpStream, prefix: &str, resolve: &Resolve) -> io::Result<()> {
    stream.set_read_timeout(Some(HEAD_TIMEOUT))?;
    let Some(request) = read_request(stream)? else {
        return status(stream, 400, "Bad Request");
    };
    if !matches!(request.method.as_str(), "GET" | "HEAD") {
        return status(stream, 405, "Method Not Allowed");
    }
    let Some(encoded) = strip_secret(&request.target, prefix) else {
        return status(stream, 403, "Forbidden");
    };
    let encoded = encoded.split('?').next().unwrap_or_default();
    let Some(path) = percent_decode(encoded) else {
        return status(stream, 404, "Not Found");
    };
    let target = match resolve(&path) {
        Ok(target) => target,
        Err(Refusal::NotFound) => return status(stream, 404, "Not Found"),
        Err(Refusal::Unavailable(e)) => {
            tracing::debug!("Loopback could not open a remote track: {e}");
            return status(stream, 503, "Service Unavailable");
        }
    };

    let (mut body, len, extension): (Box<dyn Body>, u64, String) = match target {
        PlayTarget::File(path) => {
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(e) => {
                    tracing::debug!("Loopback could not open a cached track: {e}");
                    return status(stream, 503, "Service Unavailable");
                }
            };
            let len = file.metadata()?.len();
            let extension = Path::new(&path)
                .extension()
                .map(|ext| ext.to_string_lossy().into_owned())
                .unwrap_or_default();
            (Box::new(file), len, extension)
        }
        PlayTarget::Progressive(fill, extension) => {
            let len = fill.len;
            let start = request.range.map_or(0, |(start, _)| start);
            // Refused up front rather than after headers promising a body.
            if start < len {
                if let Err(e) = fill.wait_for(start, 1, READ_TIMEOUT) {
                    tracing::debug!("Loopback stream stalled: {e}");
                    return status(stream, 503, "Service Unavailable");
                }
            }
            let reader = ProgressiveReader::new(fill, READ_TIMEOUT)?;
            (Box::new(reader), len, extension)
        }
    };

    let (start, end) = match request.range {
        None => (0, len),
        Some((start, end)) => {
            let last = end.unwrap_or(u64::MAX).min(len.saturating_sub(1));
            if start >= len || start > last {
                let head = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
                );
                return stream.write_all(head.as_bytes());
            }
            (start, last + 1)
        }
    };
    let mut head = match request.range {
        None => "HTTP/1.1 200 OK\r\n".to_string(),
        Some(_) => format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{len}\r\n",
            end - 1
        ),
    };
    head.push_str(&format!(
        "Content-Type: {}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n\
         Connection: close\r\n\r\n",
        content_type(&extension),
        end - start
    ));
    stream.write_all(head.as_bytes())?;
    if request.method == "HEAD" {
        return Ok(());
    }

    body.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; CHUNK];
    let mut left = end - start;
    while left > 0 {
        let want = usize::try_from(left).unwrap_or(CHUNK).min(CHUNK);
        let n = body.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        stream.write_all(&buf[..n])?;
        left -= n as u64;
    }
    stream.flush()
}

trait Body: Read + Seek {}
impl<T: Read + Seek> Body for T {}

struct Request {
    method: String,
    target: String,
    range: Option<(u64, Option<u64>)>,
}

/// The request line and the `Range` header. `None` for a malformed or
/// oversized head.
fn read_request(stream: &TcpStream) -> io::Result<Option<Request>> {
    let mut reader = BufReader::new(stream).take(MAX_HEAD);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
        return Ok(None);
    };
    let mut request = Request {
        method: method.to_string(),
        target: target.to_string(),
        range: None,
    };
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            return Ok(Some(request));
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("range") {
                request.range = parse_range(value);
            }
        }
    }
}

/// `bytes=a-` or `bytes=a-b`. Any other form is ignored and the whole file
/// is sent, which HTTP allows.
fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let end = end.trim();
    if end.is_empty() {
        return Some((start, None));
    }
    Some((start, Some(end.parse().ok()?)))
}

/// What follows the secret, compared in constant time.
fn strip_secret<'a>(target: &'a str, prefix: &str) -> Option<&'a str> {
    let head = target.as_bytes().get(..prefix.len())?;
    let diff = head
        .iter()
        .zip(prefix.as_bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    (diff == 0).then(|| &target[prefix.len()..])
}

fn status(stream: &mut TcpStream, code: u16, reason: &str) -> io::Result<()> {
    let head =
        format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    stream.write_all(head.as_bytes())
}

fn content_type(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        "ogg" | "oga" | "opus" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::super::fill_for_with;
    use super::super::testkit::*;
    use super::*;
    use crate::remote::uri::percent_encode;

    const LONG: Duration = Duration::from_secs(20);

    struct Response {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Response {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }
    }

    fn serve_harness(h: &Harness, gate: &'static NetworkGate) -> Loopback {
        let rt = h.rt;
        bind(Arc::new(move |path: &str| resolve_with(rt, gate, path))).unwrap()
    }

    fn split_base(base: &str) -> (&str, &str) {
        let rest = base.strip_prefix("http://").unwrap();
        let slash = rest.find('/').unwrap();
        (&rest[..slash], &rest[slash..])
    }

    fn target_for(loopback: &Loopback, path: &str) -> String {
        format!("{}{}", split_base(&loopback.base).1, percent_encode(path))
    }

    fn request(loopback: &Loopback, method: &str, target: &str, range: Option<&str>) -> Response {
        let (addr, _) = split_base(&loopback.base);
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(LONG)).unwrap();
        let mut head = format!("{method} {target} HTTP/1.1\r\nHost: {addr}\r\n");
        if let Some(range) = range {
            head.push_str(&format!("Range: {range}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();

        let end = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a complete response head");
        let head = String::from_utf8(raw[..end].to_vec()).unwrap();
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap().split(' ').nth(1).unwrap();
        let headers = lines
            .map(|line| {
                let (key, value) = line.split_once(':').unwrap();
                (key.trim().to_string(), value.trim().to_string())
            })
            .collect();
        Response {
            status: status.parse().unwrap(),
            headers,
            body: raw[end + 4..].to_vec(),
        }
    }

    fn get(loopback: &Loopback, path: &str, range: Option<&str>) -> Response {
        request(loopback, "GET", &target_for(loopback, path), range)
    }

    #[test]
    fn wrong_secret_is_forbidden() {
        let h = Harness::new(64 * 1024);
        let backend = Arc::new(FakeBackend::new(h.data.clone(), FAST));
        h.rt.insert_backend_for_test(&h.uri.source_id, backend.clone());
        let loopback = serve_harness(&h, h.gate);
        let path = percent_encode(&h.uri.to_path());

        let wrong = format!("/{}/{path}", "0".repeat(32));
        assert_eq!(request(&loopback, "GET", &wrong, None).status, 403);
        assert_eq!(
            request(&loopback, "GET", &format!("/{path}"), None).status,
            403
        );
        assert_eq!(request(&loopback, "GET", "/", None).status, 403);
        assert!(backend.opens().is_empty());
    }

    #[test]
    fn range_request_returns_206_with_content_range() {
        let h = Harness::new(MIB as usize);
        h.rt.insert_backend_for_test(
            &h.uri.source_id,
            Arc::new(FakeBackend::new(h.data.clone(), FAST)),
        );
        let loopback = serve_harness(&h, h.gate);
        let path = h.uri.to_path();

        let response = get(&loopback, &path, Some("bytes=100-199"));
        assert_eq!(response.status, 206);
        assert_eq!(
            response.header("Content-Range"),
            Some(format!("bytes 100-199/{MIB}").as_str())
        );
        assert_eq!(response.header("Content-Length"), Some("100"));
        assert_eq!(response.header("Accept-Ranges"), Some("bytes"));
        assert_eq!(response.header("Content-Type"), Some("audio/flac"));
        assert!(response.body == h.data[100..200], "body differs");

        let past = get(&loopback, &path, Some(&format!("bytes={MIB}-")));
        assert_eq!(past.status, 416);
        assert_eq!(
            past.header("Content-Range"),
            Some(format!("bytes */{MIB}").as_str())
        );
    }

    #[test]
    fn open_ended_range_streams_to_end_while_filling() {
        let len = 512 * 1024;
        let h = Harness::new(len);
        h.rt.insert_backend_for_test(
            &h.uri.source_id,
            Arc::new(FakeBackend::new(h.data.clone(), SLOW)),
        );
        let loopback = serve_harness(&h, h.gate);
        let fill = fill_for_with(h.rt, h.gate, &h.uri).unwrap();
        assert!(!fill.is_complete());

        let response = get(&loopback, &h.uri.to_path(), Some("bytes=4096-"));
        assert_eq!(response.status, 206);
        assert_eq!(
            response.header("Content-Range"),
            Some(format!("bytes 4096-{}/{len}", len - 1).as_str())
        );
        assert_eq!(
            response.header("Content-Length"),
            Some((len - 4096).to_string().as_str())
        );
        assert!(response.body == h.data[4096..], "body differs");
    }

    #[test]
    fn no_range_returns_200_full_length() {
        let len = 256 * 1024;
        let h = Harness::new(len);
        h.rt.insert_backend_for_test(
            &h.uri.source_id,
            Arc::new(FakeBackend::new(h.data.clone(), FAST)),
        );
        let loopback = serve_harness(&h, h.gate);
        let target = target_for(&loopback, &h.uri.to_path());

        let response = request(&loopback, "GET", &target, None);
        assert_eq!(response.status, 200);
        assert_eq!(
            response.header("Content-Length"),
            Some(len.to_string().as_str())
        );
        assert_eq!(response.header("Accept-Ranges"), Some("bytes"));
        assert_eq!(response.header("Content-Range"), None);
        assert_eq!(response.header("Connection"), Some("close"));
        assert!(response.body == *h.data, "body differs");

        let head = request(&loopback, "HEAD", &target, None);
        assert_eq!(head.status, 200);
        assert_eq!(
            head.header("Content-Length"),
            Some(len.to_string().as_str())
        );
        assert!(head.body.is_empty());
    }

    #[test]
    fn gate_closed_uncached_returns_503() {
        let h = Harness::new(64 * 1024);
        let backend = Arc::new(FakeBackend::new(h.data.clone(), FAST));
        h.rt.insert_backend_for_test(&h.uri.source_id, backend.clone());
        let loopback = serve_harness(&h, gate(false));

        assert_eq!(get(&loopback, &h.uri.to_path(), None).status, 503);
        assert!(backend.opens().is_empty());
        assert!(!h.cache_path().exists());
    }

    #[test]
    fn unknown_tracks_and_local_paths_are_not_found() {
        let h = Harness::new(64 * 1024);
        h.rt.insert_backend_for_test(
            &h.uri.source_id,
            Arc::new(FakeBackend::new(h.data.clone(), FAST)),
        );
        let loopback = serve_harness(&h, h.gate);
        let unknown = RemoteUri::new(&h.uri.source_id, "999").to_path();

        assert_eq!(get(&loopback, &unknown, None).status, 404);
        assert_eq!(get(&loopback, "/etc/hosts", None).status, 404);
        assert_eq!(get(&loopback, "wave-remote://nothing", None).status, 404);
        let bad_escape = format!("{}%zz", split_base(&loopback.base).1);
        assert_eq!(request(&loopback, "GET", &bad_escape, None).status, 404);
        let post = request(
            &loopback,
            "POST",
            &target_for(&loopback, &h.uri.to_path()),
            None,
        );
        assert_eq!(post.status, 405);
    }
}
