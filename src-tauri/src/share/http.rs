// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The media server's HTTP side: the device and service descriptions, SOAP
//! control for ContentDirectory and ConnectionManager, and the songs and
//! covers themselves, with byte ranges so a player can seek.
//!
//! One thread per connection, one request per connection (`Connection:
//! close`), which every UPnP control point handles. Only clients on the local
//! network are answered.

use super::catalog::{dlna_features, escape, mime_for, Fault, Page, Snapshot, INVALID_ARGS};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const CHUNK: usize = 64 * 1024;
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 64 * 1024;
const IDLE: Duration = Duration::from_secs(15);

pub const CONTENT_DIRECTORY: &str = "urn:schemas-upnp-org:service:ContentDirectory:1";
pub const CONNECTION_MANAGER: &str = "urn:schemas-upnp-org:service:ConnectionManager:1";
pub const MEDIA_SERVER: &str = "urn:schemas-upnp-org:device:MediaServer:1";

/// Opens a shared file for reading. Android `content://` documents need the
/// app's ContentResolver; everything else is a plain path.
pub type Opener = Arc<dyn Fn(&str) -> io::Result<std::fs::File> + Send + Sync>;

/// What the request handlers need from the server around them.
pub trait Catalog: Send + Sync {
    fn snapshot(&self) -> Result<Arc<Snapshot>, String>;
}

pub struct Server {
    pub name: String,
    pub uuid: String,
    pub catalog: Arc<dyn Catalog>,
    pub opener: Opener,
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
                    tracing::debug!("Refused a media server request from {peer}");
                    continue;
                }
                let server = server.clone();
                let spawned = std::thread::Builder::new()
                    .name("wave-share-http".into())
                    .spawn(move || {
                        let _ = stream.set_nonblocking(false);
                        if let Err(e) = handle(stream, &server) {
                            tracing::debug!("Media server request ended: {e}");
                        }
                    });
                if let Err(e) = spawned {
                    tracing::warn!("Could not answer a media server request: {e}");
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                tracing::debug!("Media server accept failed: {e}");
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
        "{}/1 UPnP/1.0 Wave/{}",
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

fn xml(stream: &mut TcpStream, status: &str, body: &str, head_only: bool) -> io::Result<()> {
    respond(
        stream,
        status,
        &[("Content-Type", r#"text/xml; charset="utf-8""#.to_string())],
        body.as_bytes(),
        head_only,
    )
}

fn handle(mut stream: TcpStream, server: &Server) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE))?;
    stream.set_write_timeout(Some(IDLE))?;
    let request = read_request(&stream)?;
    // The address the client reached us on is the one our URLs must use.
    let local = stream.local_addr()?;
    let base = match local.ip() {
        IpAddr::V6(v6) if v6.to_ipv4_mapped().is_none() => {
            format!("http://[{v6}]:{}", local.port())
        }
        IpAddr::V6(v6) => format!("http://{}:{}", v6.to_ipv4_mapped().unwrap(), local.port()),
        IpAddr::V4(v4) => format!("http://{v4}:{}", local.port()),
    };
    let head_only = request.method == "HEAD";
    let path = request
        .path
        .split('?')
        .next()
        .unwrap_or_default()
        .to_string();
    match (request.method.as_str(), path.as_str()) {
        ("GET" | "HEAD", "/rootDesc.xml") => xml(
            &mut stream,
            "200 OK",
            &device_description(server),
            head_only,
        ),
        ("GET" | "HEAD", "/ContentDirectory.xml") => {
            xml(&mut stream, "200 OK", CONTENT_DIRECTORY_SCPD, head_only)
        }
        ("GET" | "HEAD", "/ConnectionManager.xml") => {
            xml(&mut stream, "200 OK", CONNECTION_MANAGER_SCPD, head_only)
        }
        ("POST", "/ctl/ContentDirectory") => {
            let reply = content_directory(server, &request, &base);
            soap_reply(&mut stream, CONTENT_DIRECTORY, reply)
        }
        ("POST", "/ctl/ConnectionManager") => {
            let reply = connection_manager(&request);
            soap_reply(&mut stream, CONNECTION_MANAGER, reply)
        }
        ("SUBSCRIBE", "/evt/ContentDirectory" | "/evt/ConnectionManager") => {
            // Subscriptions are acknowledged so control points that insist on
            // one carry on; changes are found through SystemUpdateID.
            let sid = request
                .header("SID")
                .map(str::to_string)
                .unwrap_or_else(|| format!("uuid:{}", uuid::Uuid::new_v4()));
            respond(
                &mut stream,
                "200 OK",
                &[("SID", sid), ("TIMEOUT", "Second-1800".to_string())],
                b"",
                false,
            )
        }
        ("UNSUBSCRIBE", "/evt/ContentDirectory" | "/evt/ConnectionManager") => {
            respond(&mut stream, "200 OK", &[], b"", false)
        }
        ("GET" | "HEAD", p) if p.starts_with("/media/") => media(
            &mut stream,
            server,
            &request,
            &p["/media/".len()..],
            head_only,
        ),
        // Players show this beside the server's name.
        ("GET" | "HEAD", "/icon/128.png") => respond(
            &mut stream,
            "200 OK",
            &[("Content-Type", "image/png".to_string())],
            include_bytes!("../../icons/128x128.png"),
            head_only,
        ),
        ("GET" | "HEAD", "/icon/64.png") => respond(
            &mut stream,
            "200 OK",
            &[("Content-Type", "image/png".to_string())],
            include_bytes!("../../icons/64x64.png"),
            head_only,
        ),
        ("GET" | "HEAD", p) if p.starts_with("/art/") => {
            art(&mut stream, server, &p["/art/".len()..], head_only)
        }
        _ => respond(&mut stream, "404 Not Found", &[], b"", head_only),
    }
}

/// The track id in `<id>.<extension>`, percent-decoded.
fn track_id_from(name: &str) -> Option<String> {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    crate::remote::uri::percent_decode(stem).filter(|id| !id.is_empty())
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
    name: &str,
    head_only: bool,
) -> io::Result<()> {
    let found = track_id_from(name).and_then(|id| {
        let snapshot = server.catalog.snapshot().ok()?;
        snapshot.track(&id).cloned()
    });
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
        ("transferMode.dlna.org", "Streaming".to_string()),
        ("contentFeatures.dlna.org", dlna_features(&extension)),
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
/// thumbnail. Players show covers large, and so does a copy of Wave that
/// syncs this library, which keeps what it is sent.
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

fn art(stream: &mut TcpStream, server: &Server, name: &str, head_only: bool) -> io::Result<()> {
    let cover = track_id_from(name).and_then(|id| {
        let snapshot = server.catalog.snapshot().ok()?;
        best_cover(server, snapshot.track(&id)?)
    });
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

// ── SOAP ───────────────────────────────────────────────────────────────────

/// An action's name and arguments, from the `SOAPACTION` header and body.
fn soap_action(request: &Request) -> Option<(String, Vec<(String, String)>)> {
    let body = std::str::from_utf8(&request.body).ok()?;
    let doc = roxmltree::Document::parse(body).ok()?;
    let from_header = request.header("SOAPACTION").and_then(|v| {
        v.trim_matches('"')
            .rsplit_once('#')
            .map(|(_, a)| a.to_string())
    });
    let body_el = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "Body")?;
    let action_el = body_el.children().find(|n| n.is_element())?;
    let name = from_header.unwrap_or_else(|| action_el.tag_name().name().to_string());
    let args = action_el
        .children()
        .filter(|n| n.is_element())
        .map(|n| {
            (
                n.tag_name().name().to_string(),
                n.text().unwrap_or_default().to_string(),
            )
        })
        .collect();
    Some((name, args))
}

type SoapReply = Result<(String, Vec<(&'static str, String)>), Fault>;

fn soap_reply(stream: &mut TcpStream, service: &str, reply: SoapReply) -> io::Result<()> {
    match reply {
        Ok((action, values)) => {
            let args: String = values
                .iter()
                .map(|(name, value)| format!("<{name}>{}</{name}>", escape(value)))
                .collect();
            let body = format!(
                r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action}Response xmlns:u="{service}">{args}</u:{action}Response></s:Body></s:Envelope>"#
            );
            xml(stream, "200 OK", &body, false)
        }
        Err(fault) => {
            let body = format!(
                r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>{}</errorCode><errorDescription>{}</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#,
                fault.code,
                escape(fault.description)
            );
            xml(stream, "500 Internal Server Error", &body, false)
        }
    }
}

const INVALID_ACTION: u16 = 401;

fn arg<'a>(args: &'a [(String, String)], name: &str) -> Option<&'a str> {
    args.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn number(args: &[(String, String)], name: &str) -> Result<usize, Fault> {
    arg(args, name)
        .unwrap_or("0")
        .trim()
        .parse()
        .map_err(|_| Fault::new(INVALID_ARGS, "Invalid Args"))
}

fn page_reply(action: &str, page: Page, version: u32) -> (String, Vec<(&'static str, String)>) {
    (
        action.to_string(),
        vec![
            ("Result", page.didl),
            ("NumberReturned", page.returned.to_string()),
            ("TotalMatches", page.total.to_string()),
            ("UpdateID", version.to_string()),
        ],
    )
}

fn content_directory(server: &Server, request: &Request, base: &str) -> SoapReply {
    let (action, args) =
        soap_action(request).ok_or(Fault::new(INVALID_ACTION, "Invalid Action"))?;
    let snapshot = || {
        server
            .catalog
            .snapshot()
            .map_err(|_| Fault::new(501, "Action Failed"))
    };
    match action.as_str() {
        "Browse" => {
            let snap = snapshot()?;
            let object = arg(&args, "ObjectID").unwrap_or("0");
            let children = match arg(&args, "BrowseFlag") {
                Some("BrowseDirectChildren") => true,
                Some("BrowseMetadata") => false,
                _ => return Err(Fault::new(INVALID_ARGS, "Invalid Args")),
            };
            let start = number(&args, "StartingIndex")?;
            let count = number(&args, "RequestedCount")?;
            let page = snap.browse(object, children, start, count, base)?;
            Ok(page_reply("Browse", page, snap.version))
        }
        "Search" => {
            let snap = snapshot()?;
            let container = arg(&args, "ContainerID").unwrap_or("0");
            let criteria = arg(&args, "SearchCriteria").unwrap_or("*");
            let start = number(&args, "StartingIndex")?;
            let count = number(&args, "RequestedCount")?;
            let page = snap.search(container, criteria, start, count, base)?;
            Ok(page_reply("Search", page, snap.version))
        }
        "GetSearchCapabilities" => Ok((
            action,
            vec![(
                "SearchCaps",
                "@id,@parentID,upnp:class,dc:title,dc:creator,upnp:artist,upnp:album,upnp:genre,dc:date"
                    .to_string(),
            )],
        )),
        // Sorting is not offered: results come in library order.
        "GetSortCapabilities" => Ok((action, vec![("SortCaps", String::new())])),
        "GetSystemUpdateID" => Ok((action, vec![("Id", snapshot()?.version.to_string())])),
        _ => Err(Fault::new(INVALID_ACTION, "Invalid Action")),
    }
}

/// Every format the server can send, for `GetProtocolInfo`.
fn source_protocols() -> String {
    ["mp3", "flac", "m4a", "ogg", "opus", "wav", "aiff"]
        .iter()
        .map(|ext| format!("http-get:*:{}:*", mime_for(ext)))
        .collect::<Vec<_>>()
        .join(",")
}

fn connection_manager(request: &Request) -> SoapReply {
    let (action, args) =
        soap_action(request).ok_or(Fault::new(INVALID_ACTION, "Invalid Action"))?;
    match action.as_str() {
        "GetProtocolInfo" => Ok((
            action,
            vec![("Source", source_protocols()), ("Sink", String::new())],
        )),
        "GetCurrentConnectionIDs" => Ok((action, vec![("ConnectionIDs", "0".to_string())])),
        "GetCurrentConnectionInfo" => {
            if arg(&args, "ConnectionID").map(str::trim) != Some("0") {
                // 706: Invalid connection reference.
                return Err(Fault::new(706, "Invalid connection reference"));
            }
            Ok((
                action,
                vec![
                    ("RcsID", "-1".to_string()),
                    ("AVTransportID", "-1".to_string()),
                    ("ProtocolInfo", String::new()),
                    ("PeerConnectionManager", String::new()),
                    ("PeerConnectionID", "-1".to_string()),
                    ("Direction", "Output".to_string()),
                    ("Status", "OK".to_string()),
                ],
            ))
        }
        _ => Err(Fault::new(INVALID_ACTION, "Invalid Action")),
    }
}

// ── Descriptions ───────────────────────────────────────────────────────────

pub fn device_description(server: &Server) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0" xmlns:dlna="urn:schemas-dlna-org:device-1-0">
<specVersion><major>1</major><minor>0</minor></specVersion>
<device>
<deviceType>{MEDIA_SERVER}</deviceType>
<friendlyName>{name}</friendlyName>
<manufacturer>Wave</manufacturer>
<manufacturerURL>https://github.com/BMDarkLight/Wave</manufacturerURL>
<modelDescription>Wave music library</modelDescription>
<modelName>Wave</modelName>
<modelNumber>{version}</modelNumber>
<modelURL>https://github.com/BMDarkLight/Wave</modelURL>
<UDN>uuid:{uuid}</UDN>
<dlna:X_DLNADOC>DMS-1.50</dlna:X_DLNADOC>
<iconList>
<icon><mimetype>image/png</mimetype><width>128</width><height>128</height><depth>24</depth><url>/icon/128.png</url></icon>
<icon><mimetype>image/png</mimetype><width>64</width><height>64</height><depth>24</depth><url>/icon/64.png</url></icon>
</iconList>
<serviceList>
<service>
<serviceType>{CONTENT_DIRECTORY}</serviceType>
<serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId>
<SCPDURL>/ContentDirectory.xml</SCPDURL>
<controlURL>/ctl/ContentDirectory</controlURL>
<eventSubURL>/evt/ContentDirectory</eventSubURL>
</service>
<service>
<serviceType>{CONNECTION_MANAGER}</serviceType>
<serviceId>urn:upnp-org:serviceId:ConnectionManager</serviceId>
<SCPDURL>/ConnectionManager.xml</SCPDURL>
<controlURL>/ctl/ConnectionManager</controlURL>
<eventSubURL>/evt/ConnectionManager</eventSubURL>
</service>
</serviceList>
</device>
</root>"#,
        name = escape(&server.name),
        version = env!("CARGO_PKG_VERSION"),
        uuid = escape(&server.uuid),
    )
}

/// The actions and state variables ContentDirectory:1 requires, plus Search.
const CONTENT_DIRECTORY_SCPD: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
<specVersion><major>1</major><minor>0</minor></specVersion>
<actionList>
<action><name>GetSearchCapabilities</name><argumentList>
<argument><name>SearchCaps</name><direction>out</direction><relatedStateVariable>SearchCapabilities</relatedStateVariable></argument>
</argumentList></action>
<action><name>GetSortCapabilities</name><argumentList>
<argument><name>SortCaps</name><direction>out</direction><relatedStateVariable>SortCapabilities</relatedStateVariable></argument>
</argumentList></action>
<action><name>GetSystemUpdateID</name><argumentList>
<argument><name>Id</name><direction>out</direction><relatedStateVariable>SystemUpdateID</relatedStateVariable></argument>
</argumentList></action>
<action><name>Browse</name><argumentList>
<argument><name>ObjectID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ObjectID</relatedStateVariable></argument>
<argument><name>BrowseFlag</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_BrowseFlag</relatedStateVariable></argument>
<argument><name>Filter</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Filter</relatedStateVariable></argument>
<argument><name>StartingIndex</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Index</relatedStateVariable></argument>
<argument><name>RequestedCount</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
<argument><name>SortCriteria</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_SortCriteria</relatedStateVariable></argument>
<argument><name>Result</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Result</relatedStateVariable></argument>
<argument><name>NumberReturned</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
<argument><name>TotalMatches</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
<argument><name>UpdateID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_UpdateID</relatedStateVariable></argument>
</argumentList></action>
<action><name>Search</name><argumentList>
<argument><name>ContainerID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ObjectID</relatedStateVariable></argument>
<argument><name>SearchCriteria</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_SearchCriteria</relatedStateVariable></argument>
<argument><name>Filter</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Filter</relatedStateVariable></argument>
<argument><name>StartingIndex</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Index</relatedStateVariable></argument>
<argument><name>RequestedCount</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
<argument><name>SortCriteria</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_SortCriteria</relatedStateVariable></argument>
<argument><name>Result</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Result</relatedStateVariable></argument>
<argument><name>NumberReturned</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
<argument><name>TotalMatches</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
<argument><name>UpdateID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_UpdateID</relatedStateVariable></argument>
</argumentList></action>
</actionList>
<serviceStateTable>
<stateVariable sendEvents="no"><name>SearchCapabilities</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="no"><name>SortCapabilities</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="yes"><name>SystemUpdateID</name><dataType>ui4</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_ObjectID</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_Result</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_SearchCriteria</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_BrowseFlag</name><dataType>string</dataType>
<allowedValueList><allowedValue>BrowseMetadata</allowedValue><allowedValue>BrowseDirectChildren</allowedValue></allowedValueList></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_Filter</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_SortCriteria</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_Index</name><dataType>ui4</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_Count</name><dataType>ui4</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_UpdateID</name><dataType>ui4</dataType></stateVariable>
</serviceStateTable>
</scpd>"#;

const CONNECTION_MANAGER_SCPD: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
<specVersion><major>1</major><minor>0</minor></specVersion>
<actionList>
<action><name>GetProtocolInfo</name><argumentList>
<argument><name>Source</name><direction>out</direction><relatedStateVariable>SourceProtocolInfo</relatedStateVariable></argument>
<argument><name>Sink</name><direction>out</direction><relatedStateVariable>SinkProtocolInfo</relatedStateVariable></argument>
</argumentList></action>
<action><name>GetCurrentConnectionIDs</name><argumentList>
<argument><name>ConnectionIDs</name><direction>out</direction><relatedStateVariable>CurrentConnectionIDs</relatedStateVariable></argument>
</argumentList></action>
<action><name>GetCurrentConnectionInfo</name><argumentList>
<argument><name>ConnectionID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ConnectionID</relatedStateVariable></argument>
<argument><name>RcsID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_RcsID</relatedStateVariable></argument>
<argument><name>AVTransportID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_AVTransportID</relatedStateVariable></argument>
<argument><name>ProtocolInfo</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ProtocolInfo</relatedStateVariable></argument>
<argument><name>PeerConnectionManager</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionManager</relatedStateVariable></argument>
<argument><name>PeerConnectionID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionID</relatedStateVariable></argument>
<argument><name>Direction</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Direction</relatedStateVariable></argument>
<argument><name>Status</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionStatus</relatedStateVariable></argument>
</argumentList></action>
</actionList>
<serviceStateTable>
<stateVariable sendEvents="yes"><name>SourceProtocolInfo</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="yes"><name>SinkProtocolInfo</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="yes"><name>CurrentConnectionIDs</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionStatus</name><dataType>string</dataType>
<allowedValueList><allowedValue>OK</allowedValue><allowedValue>ContentFormatMismatch</allowedValue><allowedValue>InsufficientBandwidth</allowedValue><allowedValue>UnreliableChannel</allowedValue><allowedValue>Unknown</allowedValue></allowedValueList></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionManager</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_Direction</name><dataType>string</dataType>
<allowedValueList><allowedValue>Input</allowedValue><allowedValue>Output</allowedValue></allowedValueList></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_ProtocolInfo</name><dataType>string</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionID</name><dataType>i4</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_AVTransportID</name><dataType>i4</dataType></stateVariable>
<stateVariable sendEvents="no"><name>A_ARG_TYPE_RcsID</name><dataType>i4</dataType></stateVariable>
</serviceStateTable>
</scpd>"#;

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

    /// A running server over one 1000-byte song, and its port.
    fn running() -> (u16, Arc<AtomicBool>, std::path::PathBuf) {
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
        let snapshot = Arc::new(Snapshot::build(42, vec![track], Vec::new()));
        let server = Arc::new(Server {
            name: "Wave on test & co".into(),
            uuid: "1234".into(),
            catalog: Arc::new(Fixed(snapshot)),
            opener: Arc::new(|path: &str| std::fs::File::open(path)),
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

    fn soap(port: u16, service: &str, action: &str, args: &str) -> (String, String) {
        let body = format!(
            r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:{action} xmlns:u="{service}">{args}</u:{action}></s:Body></s:Envelope>"#
        );
        let path = if service == CONTENT_DIRECTORY {
            "/ctl/ContentDirectory"
        } else {
            "/ctl/ConnectionManager"
        };
        let (head, body) = exchange(
            port,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Type: text/xml\r\nSOAPACTION: \"{service}#{action}\"\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        (head, String::from_utf8(body).unwrap())
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
    fn the_description_names_both_services() {
        let (port, stop, _) = running();
        let (head, body) = exchange(port, "GET /rootDesc.xml HTTP/1.1\r\nHost: x\r\n\r\n");
        let body = String::from_utf8(body).unwrap();
        assert!(head.starts_with("HTTP/1.1 200"));
        assert!(body.contains("<friendlyName>Wave on test &amp; co</friendlyName>"));
        assert!(body.contains("<UDN>uuid:1234</UDN>"));
        assert!(body.contains(CONTENT_DIRECTORY) && body.contains(CONNECTION_MANAGER));
        // Our own DLNA client must accept it.
        let config = crate::remote::dlna::parse_description(
            &body,
            &format!("http://127.0.0.1:{port}/rootDesc.xml"),
        )
        .unwrap();
        assert_eq!(
            config.control_url,
            format!("http://127.0.0.1:{port}/ctl/ContentDirectory")
        );
        for scpd in ["/ContentDirectory.xml", "/ConnectionManager.xml"] {
            let (_, body) = exchange(port, &format!("GET {scpd} HTTP/1.1\r\n\r\n"));
            assert!(roxmltree::Document::parse(std::str::from_utf8(&body).unwrap()).is_ok());
        }
        // Every icon the description lists is there.
        let description = roxmltree::Document::parse(&body).unwrap();
        let icons: Vec<_> = description
            .descendants()
            .filter(|n| n.has_tag_name("url"))
            .filter_map(|n| n.text())
            .collect();
        assert_eq!(icons.len(), 2);
        for icon in icons {
            let (head, body) = exchange(port, &format!("GET {icon} HTTP/1.1\r\n\r\n"));
            assert!(head.starts_with("HTTP/1.1 200"), "{icon}: {head}");
            assert!(body.starts_with(b"\x89PNG"));
        }
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn browse_and_search_answer_as_the_spec_shows() {
        let (port, stop, _) = running();
        let (head, body) = soap(
            port,
            CONTENT_DIRECTORY,
            "Browse",
            "<ObjectID>songs</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter><StartingIndex>0</StartingIndex><RequestedCount>10</RequestedCount><SortCriteria></SortCriteria>",
        );
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let doc = roxmltree::Document::parse(&body).unwrap();
        let text = |name: &str| {
            doc.descendants()
                .find(|n| n.tag_name().name() == name)
                .and_then(|n| n.text())
                .unwrap_or_default()
                .to_string()
        };
        assert_eq!(text("NumberReturned"), "1");
        assert_eq!(text("TotalMatches"), "1");
        assert_eq!(text("UpdateID"), "42");
        // Result is escaped DIDL-Lite that parses on its own.
        let didl = text("Result");
        let parsed = roxmltree::Document::parse(&didl).unwrap();
        let res = parsed
            .descendants()
            .find(|n| n.tag_name().name() == "res")
            .unwrap();
        assert_eq!(
            res.text().unwrap(),
            format!("http://127.0.0.1:{port}/media/t%201.mp3")
        );

        let (_, body) = soap(
            port,
            CONTENT_DIRECTORY,
            "Search",
            r#"<ContainerID>0</ContainerID><SearchCriteria>upnp:class derivedfrom "object.item.audioItem"</SearchCriteria><Filter>*</Filter><StartingIndex>0</StartingIndex><RequestedCount>0</RequestedCount><SortCriteria></SortCriteria>"#,
        );
        assert!(body.contains("<TotalMatches>1</TotalMatches>"));

        let (head, body) = soap(
            port,
            CONTENT_DIRECTORY,
            "Browse",
            "<ObjectID>nope</ObjectID><BrowseFlag>BrowseMetadata</BrowseFlag>",
        );
        assert!(head.starts_with("HTTP/1.1 500"));
        assert!(body.contains("<errorCode>701</errorCode>"));

        let (_, body) = soap(port, CONNECTION_MANAGER, "GetProtocolInfo", "");
        assert!(body.contains("http-get:*:audio/flac:*"));
        let (_, body) = soap(port, CONTENT_DIRECTORY, "GetSystemUpdateID", "");
        assert!(body.contains("<Id>42</Id>"));
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn media_is_served_whole_or_in_ranges() {
        let (port, stop, song) = running();
        let bytes = std::fs::read(&song).unwrap();

        let (head, body) = exchange(port, "GET /media/t%201.mp3 HTTP/1.1\r\n\r\n");
        assert!(head.starts_with("HTTP/1.1 200"));
        assert!(head.contains("Content-Type: audio/mpeg"));
        assert!(head.contains("Accept-Ranges: bytes"));
        assert!(head.contains("contentFeatures.dlna.org: DLNA.ORG_PN=MP3;DLNA.ORG_OP=01"));
        assert_eq!(body, bytes);

        let (head, body) = exchange(
            port,
            "GET /media/t%201.mp3 HTTP/1.1\r\nRange: bytes=100-199\r\n\r\n",
        );
        assert!(head.starts_with("HTTP/1.1 206"));
        assert!(head.contains("Content-Range: bytes 100-199/1000"));
        assert_eq!(body, bytes[100..200]);

        let (head, body) = exchange(port, "HEAD /media/t%201.mp3 HTTP/1.1\r\n\r\n");
        assert!(head.contains("Content-Length: 1000"));
        assert!(body.is_empty());

        let (head, _) = exchange(
            port,
            "GET /media/t%201.mp3 HTTP/1.1\r\nRange: bytes=2000-\r\n\r\n",
        );
        assert!(head.starts_with("HTTP/1.1 416"));
        assert!(head.contains("Content-Range: bytes */1000"));

        let (head, _) = exchange(port, "GET /media/unknown.mp3 HTTP/1.1\r\n\r\n");
        assert!(head.starts_with("HTTP/1.1 404"));
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn covers_are_served_from_the_thumbnail() {
        let (port, stop, _) = running();
        let (head, body) = exchange(port, "GET /art/t%201.jpg HTTP/1.1\r\n\r\n");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("Content-Type: image/jpeg"));
        assert_eq!(body, b"\xFF\xD8 a tiny jpeg");
        let (head, _) = exchange(port, "GET /art/unknown.jpg HTTP/1.1\r\n\r\n");
        assert!(head.starts_with("HTTP/1.1 404"));
        stop.store(true, Ordering::Relaxed);
    }
}
