// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! DLNA and UPnP media servers: MiniDLNA (ReadyMedia), Serviio, Universal
//! Media Server, Gerbera, and the servers built into most NAS boxes and many
//! routers.
//!
//! Servers are found with an SSDP search on the local network, or added by
//! the address of their device description. The catalog comes from the
//! ContentDirectory service: one `Search` for every audio item when the server
//! supports it, otherwise a `Browse` walk of its containers. A walk sees the
//! same song under Artists, Albums, Genres and Folders, so items are kept once
//! by their file URL. Audio is the item's `res` URL, fetched with Range.

use super::backend::{
    total_size, ByteStream, CatalogPage, RemoteBackend, RemoteError, RemoteTrackMeta,
};
use super::gate::NetworkGate;
use super::http::{api_client, stream_client};
use reqwest::blocking::Response;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// Items asked for per request. Most servers cap a reply below this anyway.
const PAGE: usize = 500;
/// Cursors at or above this walk containers instead of searching.
const BROWSE_MODE: usize = 1 << 30;
/// Containers this deep are almost certainly a loop.
const MAX_DEPTH: usize = 16;
const MAX_ART_BYTES: u64 = 10 * 1024 * 1024;

const SSDP_ADDR: &str = "239.255.255.250:1900";
const MEDIA_SERVER: &str = "urn:schemas-upnp-org:device:MediaServer:1";
const AUDIO_CLASS: &str = "object.item.audioItem";

/// Where description documents usually live, for an address typed without a path.
const DESCRIPTION_PATHS: &[&str] = &[
    "/rootDesc.xml",
    "/description.xml",
    "/DeviceDescription.xml",
    "/description/fetch",
    "/dms/description.xml",
];

/// Everything needed to reach one media server, stored as `config_json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DlnaConfig {
    /// The device description URL the server announced.
    pub location: String,
    pub udn: String,
    pub name: String,
    /// Absolute URL of the ContentDirectory control endpoint.
    pub control_url: String,
    /// The exact service type, which SOAP actions are addressed to.
    pub service_type: String,
}

/// Searches the local network for media servers and returns the description
/// URL of each that answers within `wait`.
pub fn discover(gate: &NetworkGate, wait: Duration) -> Result<Vec<String>, RemoteError> {
    gate.check()?;
    let unreachable = || RemoteError::Unreachable("Can't search the local network".into());
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .map_err(|_| unreachable())?;
    socket
        .bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)).into())
        .map_err(|_| unreachable())?;
    // The TTL the UPnP Device Architecture asks for.
    let _ = socket.set_multicast_ttl_v4(2);
    let target: SocketAddr = SSDP_ADDR.parse().expect("the SSDP address is valid");
    let search = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_ADDR}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {MEDIA_SERVER}\r\nUSER-AGENT: {}/1 UPnP/1.1 Wave/{}\r\n\r\n",
        std::env::consts::OS,
        env!("CARGO_PKG_VERSION")
    );
    // The system sends multicast out of one interface, which is the tunnel
    // while a VPN is up, so a server on the LAN would never hear the search.
    // Each interface gets its own copy, and each copy goes twice because UDP
    // can drop a datagram.
    let mut sent = false;
    for interface in multicast_interfaces() {
        if !interface.is_unspecified() && socket.set_multicast_if_v4(&interface).is_err() {
            continue;
        }
        for _ in 0..2 {
            sent |= socket.send_to(search.as_bytes(), &target.into()).is_ok();
        }
    }
    if !sent {
        return Err(unreachable());
    }
    let socket: UdpSocket = socket.into();
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|_| unreachable())?;
    let deadline = Instant::now() + wait;
    let mut found = Vec::new();
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        let Ok((n, _)) = socket.recv_from(&mut buf) else {
            continue;
        };
        if let Some(location) = ssdp_location(&String::from_utf8_lossy(&buf[..n])) {
            if !found.contains(&location) {
                found.push(location);
            }
        }
    }
    Ok(found)
}

/// The IPv4 addresses of the interfaces a search goes out of, after the
/// system's own choice (`0.0.0.0`).
fn multicast_interfaces() -> Vec<Ipv4Addr> {
    let mut found = vec![Ipv4Addr::UNSPECIFIED];
    let mut add = |ip: Ipv4Addr| {
        if !found.contains(&ip) {
            found.push(ip);
        }
    };
    #[cfg(unix)]
    if let Ok(interfaces) = nix::ifaddrs::getifaddrs() {
        use nix::net::if_::InterfaceFlags;
        for interface in interfaces {
            let usable = interface.flags.contains(InterfaceFlags::IFF_UP)
                && interface.flags.contains(InterfaceFlags::IFF_MULTICAST);
            if let Some(ip) = interface
                .address
                .as_ref()
                .and_then(|a| a.as_sockaddr_in())
                .map(|a| a.ip())
                .filter(|_| usable)
            {
                add(ip);
            }
        }
    }
    // Without an interface list, the route to each private range names the
    // address the system would send from.
    #[cfg(not(unix))]
    for probe in [
        "192.168.1.1:9",
        "10.0.0.1:9",
        "172.16.0.1:9",
        "169.254.1.1:9",
    ] {
        let address = UdpSocket::bind("0.0.0.0:0")
            .and_then(|s| s.connect(probe).and_then(|_| s.local_addr()));
        if let Ok(SocketAddr::V4(address)) = address {
            if !address.ip().is_unspecified() {
                add(*address.ip());
            }
        }
    }
    add(Ipv4Addr::LOCALHOST);
    found
}

/// The `LOCATION` header of an SSDP reply.
fn ssdp_location(reply: &str) -> Option<String> {
    reply.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("location")
            .then(|| value.trim().to_string())
            .filter(|v| v.starts_with("http://") || v.starts_with("https://"))
    })
}

/// The description URLs worth trying for what the user typed: the address
/// itself when it has a path, else the usual locations on that host.
pub fn candidate_locations(input: &str) -> Result<Vec<String>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter the server's address".to_string());
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("http://{input}")
    };
    let url = url::Url::parse(&with_scheme).map_err(|_| "That address isn't valid".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Use an http:// address".to_string());
    }
    if url.path() != "/" {
        return Ok(vec![url.to_string()]);
    }
    Ok(DESCRIPTION_PATHS
        .iter()
        .filter_map(|path| url.join(path).ok().map(|u| u.to_string()))
        .collect())
}

fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name)
        .and_then(|n| n.text())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// Reads a device description and finds its ContentDirectory service.
fn parse_description(xml: &str, location: &str) -> Result<DlnaConfig, RemoteError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| RemoteError::Parse(e.to_string()))?;
    let root = doc.root_element();
    let location_url =
        url::Url::parse(location).map_err(|_| RemoteError::Parse("bad location".into()))?;
    let base = child_text(root, "URLBase")
        .and_then(|b| url::Url::parse(&b).ok())
        .unwrap_or(location_url);

    // The media server may be the root device or one embedded in it.
    for device in root
        .descendants()
        .filter(|n| n.tag_name().name() == "device")
    {
        let is_server =
            child_text(device, "deviceType").is_some_and(|t| t.contains(":device:MediaServer:"));
        if !is_server {
            continue;
        }
        let Some(service_list) = child(device, "serviceList") else {
            continue;
        };
        let directory = service_list.children().find(|s| {
            s.is_element()
                && child_text(*s, "serviceType")
                    .is_some_and(|t| t.contains(":service:ContentDirectory:"))
        });
        let Some(directory) = directory else {
            continue;
        };
        let service_type = child_text(directory, "serviceType").unwrap_or_default();
        let control = child_text(directory, "controlURL")
            .ok_or_else(|| RemoteError::Parse("no control URL".into()))?;
        let control_url = base
            .join(&control)
            .map_err(|_| RemoteError::Parse("bad control URL".into()))?;
        let udn = child_text(device, "UDN").unwrap_or_else(|| location.to_string());
        let name = child_text(device, "friendlyName")
            .unwrap_or_else(|| base.host_str().unwrap_or("Media server").to_string());
        return Ok(DlnaConfig {
            location: location.to_string(),
            udn,
            name,
            control_url: control_url.to_string(),
            service_type,
        });
    }
    Err(RemoteError::Parse("not a media server".into()))
}

/// Fetches and reads a device description.
pub fn describe(gate: &NetworkGate, location: &str) -> Result<DlnaConfig, RemoteError> {
    gate.check()?;
    let host = url::Url::parse(location)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let resp = api_client()
        .get(location)
        .timeout(Duration::from_secs(5))
        .send()
        .map_err(|_| RemoteError::Unreachable(format!("Can't reach {host}")))?;
    if !resp.status().is_success() {
        return Err(RemoteError::Http(resp.status().as_u16()));
    }
    let body = resp
        .text()
        .map_err(|_| RemoteError::Unreachable(format!("Can't reach {host}")))?;
    parse_description(&body, location)
}

/// Identifies the same server across address changes.
pub fn stable_key(config: &DlnaConfig) -> String {
    config.udn.trim_start_matches("uuid:").to_ascii_lowercase()
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn soap_envelope(service_type: &str, action: &str, args: &[(&str, &str)]) -> String {
    let args: String = args
        .iter()
        .map(|(name, value)| format!("<{name}>{}</{name}>", escape_xml(value)))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action} xmlns:u="{service_type}">{args}</u:{action}></s:Body></s:Envelope>"#
    )
}

/// One reply to `Browse` or `Search`.
#[derive(Debug, Default, PartialEq)]
struct DirectoryPage {
    items: Vec<DidlItem>,
    containers: Vec<String>,
    returned: usize,
    total: Option<usize>,
}

/// An audio item from DIDL-Lite, before it becomes a track.
#[derive(Debug, Clone, PartialEq)]
struct DidlItem {
    title: String,
    artist: Option<String>,
    album_artist: Option<String>,
    album: Option<String>,
    genre: Option<String>,
    track_number: Option<i32>,
    year: Option<i32>,
    res_url: String,
    mime: Option<String>,
    size: u64,
    duration_seconds: Option<f64>,
    art_url: Option<String>,
}

/// "0:03:45.120" or "3:45" to seconds.
/// The ContentDirectory spec allows the fraction as decimals, `.F+`, or as a
/// ratio, `.F0/F1`, so "0:03:45.1/2" is 225.5 seconds.
fn parse_duration(text: &str) -> Option<f64> {
    let mut seconds = 0.0;
    for part in text.trim().split(':') {
        let value = match part.split_once('.') {
            Some((whole, fraction)) if fraction.contains('/') => {
                let (top, bottom) = fraction.split_once('/')?;
                let (top, bottom) = (top.parse::<f64>().ok()?, bottom.parse::<f64>().ok()?);
                if bottom <= 0.0 {
                    return None;
                }
                whole.parse::<f64>().ok()? + top / bottom
            }
            _ => part.parse::<f64>().ok()?,
        };
        seconds = seconds * 60.0 + value;
    }
    (seconds > 0.0).then_some(seconds)
}

/// The file extension a `res` element's MIME type stands for.
fn extension_for_mime(mime: &str) -> Option<&'static str> {
    Some(
        match mime.split(';').next()?.trim().to_ascii_lowercase().as_str() {
            "audio/mpeg" | "audio/mp3" | "audio/x-mpeg" => "mp3",
            "audio/flac" | "audio/x-flac" => "flac",
            "audio/mp4" | "audio/x-m4a" | "audio/m4a" | "audio/aac" | "audio/x-aac" => "m4a",
            "audio/ogg" | "audio/x-ogg" | "application/ogg" | "audio/vorbis" => "ogg",
            "audio/opus" => "opus",
            "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => "wav",
            "audio/aiff" | "audio/x-aiff" => "aiff",
            _ => return None,
        },
    )
}

fn parse_didl(didl: &str, base: &url::Url) -> Result<(Vec<DidlItem>, Vec<String>), RemoteError> {
    let doc = roxmltree::Document::parse(didl).map_err(|e| RemoteError::Parse(e.to_string()))?;
    let mut items = Vec::new();
    let mut containers = Vec::new();
    for node in doc.root_element().children().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "container" => {
                if let Some(id) = node.attribute("id") {
                    containers.push(id.to_string());
                }
            }
            "item" => {
                if let Some(item) = parse_item(node, base) {
                    items.push(item);
                }
            }
            _ => {}
        }
    }
    Ok((items, containers))
}

fn parse_item(node: roxmltree::Node<'_, '_>, base: &url::Url) -> Option<DidlItem> {
    let class = child_text(node, "class").unwrap_or_default();
    if !class.starts_with(AUDIO_CLASS) {
        return None;
    }
    // A plain HTTP resource in a format we can play. Servers list transcodes
    // beside the original; the original is the one that states its size.
    let playable: Vec<_> = node
        .children()
        .filter(|n| {
            n.is_element()
                && n.tag_name().name() == "res"
                && n.attribute("protocolInfo").is_some_and(|info| {
                    let mut parts = info.split(':');
                    parts.next() == Some("http-get")
                        && parts
                            .nth(1)
                            .is_some_and(|mime| extension_for_mime(mime).is_some())
                })
        })
        .collect();
    let res = playable
        .iter()
        .find(|n| n.attribute("size").is_some())
        .or_else(|| playable.first())
        .copied()?;
    let res_url = base.join(res.text()?.trim()).ok()?.to_string();
    let mime = res
        .attribute("protocolInfo")
        .and_then(|info| info.split(':').nth(2))
        .map(str::to_string);

    let mut artist = None;
    let mut album_artist = None;
    for credit in node
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "artist")
    {
        let Some(name) = credit.text().map(str::trim).filter(|t| !t.is_empty()) else {
            continue;
        };
        match credit.attribute("role") {
            Some(role) if role.eq_ignore_ascii_case("AlbumArtist") => {
                album_artist.get_or_insert_with(|| name.to_string());
            }
            None => {
                artist.get_or_insert_with(|| name.to_string());
            }
            Some(_) => {}
        }
    }
    let artist = artist.or_else(|| child_text(node, "creator"));

    Some(DidlItem {
        title: child_text(node, "title").unwrap_or_else(|| "Untitled".to_string()),
        artist,
        album_artist,
        album: child_text(node, "album"),
        genre: child_text(node, "genre"),
        track_number: child_text(node, "originalTrackNumber").and_then(|n| n.parse().ok()),
        year: child_text(node, "date").and_then(|d| d.get(..4)?.parse().ok()),
        size: res
            .attribute("size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        duration_seconds: res.attribute("duration").and_then(parse_duration),
        art_url: child_text(node, "albumArtURI")
            .and_then(|u| base.join(&u).ok().map(|u| u.to_string())),
        res_url,
        mime,
    })
}

/// Reads a SOAP reply to `Browse` or `Search`. A fault means the server
/// refused the action.
fn parse_directory_reply(xml: &str, base: &url::Url) -> Result<DirectoryPage, RemoteError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| RemoteError::Parse(e.to_string()))?;
    if doc.descendants().any(|n| n.tag_name().name() == "Fault") {
        return Err(RemoteError::Parse("the server refused the request".into()));
    }
    let reply = doc
        .descendants()
        .find(|n| matches!(n.tag_name().name(), "BrowseResponse" | "SearchResponse"))
        .ok_or_else(|| RemoteError::Parse("no directory reply".into()))?;
    let number = |name: &str| child_text(reply, name).and_then(|n| n.parse::<usize>().ok());
    let didl = child_text(reply, "Result").unwrap_or_default();
    let (items, containers) = if didl.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        parse_didl(&didl, base)?
    };
    Ok(DirectoryPage {
        returned: number("NumberReturned").unwrap_or(items.len() + containers.len()),
        total: number("TotalMatches").filter(|t| *t > 0),
        items,
        containers,
    })
}

/// The path and query of a URL, which stay the same when the server's
/// address changes.
fn path_of(url: &str) -> String {
    url::Url::parse(url)
        .map(|u| match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        })
        .unwrap_or_else(|_| url.to_string())
}

fn remote_id_for(res_url: &str) -> String {
    let digest = Sha256::digest(path_of(res_url).as_bytes());
    digest[..10].iter().map(|b| format!("{b:02x}")).collect()
}

fn item_to_meta(item: &DidlItem) -> RemoteTrackMeta {
    let extension = item
        .mime
        .as_deref()
        .and_then(extension_for_mime)
        .map(str::to_string)
        .unwrap_or_else(|| "mp3".to_string());
    // A server that only shares folders (rclone, some NAS boxes) sends the
    // file name as the title and no tags. Without an artist, the name is read
    // the way a WebDAV file name is: "07 - Title", "Artist - Title".
    let named = match &item.artist {
        Some(_) => None,
        None => Some(super::webdav::name_meta(&item.title, None)),
    };
    RemoteTrackMeta {
        remote_id: remote_id_for(&item.res_url),
        title: named
            .as_ref()
            .map_or_else(|| item.title.clone(), |n| n.title.clone()),
        artist: item
            .artist
            .clone()
            .or_else(|| named.as_ref().and_then(|n| n.artist.clone()))
            .unwrap_or_else(|| "Unknown Artist".to_string()),
        album_artist: item.album_artist.clone(),
        album: item
            .album
            .clone()
            .unwrap_or_else(|| "Unknown Album".to_string()),
        track_number: item
            .track_number
            .or_else(|| named.as_ref().and_then(|n| n.track_number)),
        disc_number: named.as_ref().and_then(|n| n.disc_number),
        year: item.year,
        genre: item.genre.clone(),
        duration_seconds: item.duration_seconds,
        size: item.size,
        extension,
        part_key: item.res_url.clone(),
        art_ref: item.art_url.clone(),
        updated_at: None,
    }
}

struct BrowseWalk {
    queue: VecDeque<(String, usize)>,
    visited: HashSet<String>,
    seen: HashSet<String>,
}

pub struct DlnaBackend {
    config: DlnaConfig,
    control: url::Url,
    gate: &'static NetworkGate,
    walk: parking_lot::Mutex<Option<BrowseWalk>>,
}

impl DlnaBackend {
    pub fn new(config: DlnaConfig) -> Result<Self, RemoteError> {
        Self::with_gate(config, NetworkGate::global())
    }

    pub fn with_gate(config: DlnaConfig, gate: &'static NetworkGate) -> Result<Self, RemoteError> {
        let control = url::Url::parse(&config.control_url)
            .map_err(|_| RemoteError::Parse("bad control URL".into()))?;
        Ok(Self {
            config,
            control,
            gate,
            walk: parking_lot::Mutex::new(None),
        })
    }

    fn unreachable(&self) -> RemoteError {
        RemoteError::Unreachable(format!("Can't reach {}", self.config.name))
    }

    fn action(&self, action: &str, args: &[(&str, &str)]) -> Result<DirectoryPage, RemoteError> {
        self.gate.check()?;
        let resp = api_client()
            .post(self.control.clone())
            .header("Content-Type", "text/xml; charset=\"utf-8\"")
            .header(
                "SOAPAction",
                format!("\"{}#{action}\"", self.config.service_type),
            )
            .body(soap_envelope(&self.config.service_type, action, args))
            .send()
            .map_err(|_| self.unreachable())?;
        // A SOAP fault arrives as a 500 with a body worth reading.
        let status = resp.status().as_u16();
        let body = resp.text().map_err(|_| self.unreachable())?;
        if status != 200 && status != 500 {
            return Err(RemoteError::Http(status));
        }
        parse_directory_reply(&body, &self.control)
    }

    fn search(&self, offset: usize) -> Result<DirectoryPage, RemoteError> {
        let start = offset.to_string();
        let count = PAGE.to_string();
        let criteria = format!("upnp:class derivedfrom \"{AUDIO_CLASS}\"");
        self.action(
            "Search",
            &[
                ("ContainerID", "0"),
                ("SearchCriteria", &criteria),
                ("Filter", "*"),
                ("StartingIndex", &start),
                ("RequestedCount", &count),
                ("SortCriteria", ""),
            ],
        )
    }

    fn browse(&self, object_id: &str, offset: usize) -> Result<DirectoryPage, RemoteError> {
        let start = offset.to_string();
        let count = PAGE.to_string();
        self.action(
            "Browse",
            &[
                ("ObjectID", object_id),
                ("BrowseFlag", "BrowseDirectChildren"),
                ("Filter", "*"),
                ("StartingIndex", &start),
                ("RequestedCount", &count),
                ("SortCriteria", ""),
            ],
        )
    }

    /// Checks the server answers before it is stored.
    pub fn probe(&self) -> Result<(), RemoteError> {
        self.browse("0", 0).map(|_| ())
    }

    /// Walks containers until a page's worth of new items is found.
    fn browse_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
        let mut slot = self.walk.lock();
        if cursor == BROWSE_MODE || slot.is_none() {
            *slot = Some(BrowseWalk {
                queue: VecDeque::from([("0".to_string(), 0)]),
                visited: HashSet::from(["0".to_string()]),
                seen: HashSet::new(),
            });
        }
        let walk = slot.as_mut().expect("the walk was just set");
        let mut tracks = Vec::new();
        while tracks.len() < PAGE {
            let Some((container, depth)) = walk.queue.pop_front() else {
                break;
            };
            let mut offset = 0;
            loop {
                let page = self.browse(&container, offset)?;
                for item in &page.items {
                    if walk.seen.insert(path_of(&item.res_url)) {
                        tracks.push(item_to_meta(item));
                    }
                }
                if depth < MAX_DEPTH {
                    for id in page.containers {
                        if walk.visited.insert(id.clone()) {
                            walk.queue.push_back((id, depth + 1));
                        }
                    }
                }
                offset += page.returned;
                // A TotalMatches of 0 with items returned means the server
                // can't count, so paging goes on until a page comes back empty.
                let more = page.total.is_none_or(|total| offset < total);
                if page.returned == 0 || !more {
                    break;
                }
            }
        }
        let next = (!walk.queue.is_empty()).then_some(cursor + 1);
        if next.is_none() {
            *slot = None;
        }
        Ok(CatalogPage {
            tracks,
            total: None,
            next,
        })
    }

    fn get(&self, url: &str, offset: u64, last: Option<u64>) -> Result<Response, RemoteError> {
        self.gate.check()?;
        let range = match last {
            Some(last) => format!("bytes={offset}-{last}"),
            None => format!("bytes={offset}-"),
        };
        let resp = stream_client()
            .get(url)
            .header("Range", range)
            .send()
            .map_err(|_| self.unreachable())?;
        match resp.status().as_u16() {
            206 => Ok(resp),
            200 if offset == 0 => Ok(resp),
            code => Err(RemoteError::Http(code)),
        }
    }
}

impl RemoteBackend for DlnaBackend {
    fn kind(&self) -> &'static str {
        "dlna"
    }

    fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
        self.gate.check()?;
        if cursor >= BROWSE_MODE {
            return self.browse_page(cursor);
        }
        let page = match self.search(cursor) {
            Ok(page) => page,
            // MiniDLNA searches; many smaller servers only browse.
            Err(RemoteError::Parse(_) | RemoteError::Http(_)) if cursor == 0 => {
                return self.browse_page(BROWSE_MODE);
            }
            Err(e) => return Err(e),
        };
        let offset = cursor + page.returned;
        let more = page.returned > 0 && page.total.is_none_or(|total| offset < total);
        Ok(CatalogPage {
            tracks: page.items.iter().map(item_to_meta).collect(),
            total: page.total,
            next: more.then_some(offset),
        })
    }

    fn open_bytes(&self, part_key: &str, offset: u64) -> Result<Box<dyn ByteStream>, RemoteError> {
        Ok(Box::new(self.get(part_key, offset, None)?))
    }

    fn check_reachable(&self) -> Result<(), RemoteError> {
        self.probe()
    }

    fn fetch_artwork(&self, art_ref: &str) -> Result<Vec<u8>, RemoteError> {
        let resp = self.get(art_ref, 0, None)?;
        let mut bytes = Vec::new();
        resp.take(MAX_ART_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|_| self.unreachable())?;
        Ok(bytes)
    }

    fn content_length(&self, part_key: &str) -> Result<u64, RemoteError> {
        let resp = self.get(part_key, 0, Some(0))?;
        total_size(&resp).ok_or_else(|| RemoteError::Parse("no size".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::gate::GateState;
    use crate::remote::testkit::{fake_server, http};

    fn open_gate() -> &'static NetworkGate {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(true, true);
        gate
    }

    const DESCRIPTION: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
    <friendlyName>NAS: minidlna</friendlyName>
    <UDN>uuid:4d696e69-444c-164e-9d41-b827eb7a1c2f</UDN>
    <serviceList>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType>
        <controlURL>/ctl/ConnectionMgr</controlURL>
      </service>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>
        <controlURL>/ctl/ContentDir</controlURL>
      </service>
    </serviceList>
  </device>
</root>"#;

    fn didl(body: &str) -> String {
        let didl = format!(
            r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">{body}</DIDL-Lite>"#
        );
        escape_xml(&didl)
    }

    fn reply(action: &str, didl_body: &str, returned: usize, total: usize) -> String {
        let xml = format!(
            r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:{action}Response xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1"><Result>{}</Result><NumberReturned>{returned}</NumberReturned><TotalMatches>{total}</TotalMatches><UpdateID>1</UpdateID></u:{action}Response></s:Body></s:Envelope>"#,
            didl(didl_body)
        );
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
            xml.len()
        )
    }

    fn fault() -> String {
        let xml = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring></s:Fault></s:Body></s:Envelope>"#;
        format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
            xml.len()
        )
    }

    fn item(id: &str, title: &str, res: &str) -> String {
        format!(
            r#"<item id="{id}" parentID="1" restricted="1"><dc:title>{title}</dc:title>
               <upnp:class>object.item.audioItem.musicTrack</upnp:class>
               <upnp:artist>Band</upnp:artist><upnp:artist role="AlbumArtist">Band &amp; Co</upnp:artist>
               <upnp:album>Album</upnp:album><upnp:genre>Rock</upnp:genre>
               <upnp:originalTrackNumber>4</upnp:originalTrackNumber><dc:date>2003-01-01</dc:date>
               <upnp:albumArtURI>/AlbumArt/9.jpg</upnp:albumArtURI>
               <res protocolInfo="http-get:*:audio/x-wav:*">/Transcode/{id}.wav</res>
               <res size="4321" duration="0:03:45.500" protocolInfo="http-get:*:audio/flac:*">{res}</res>
               </item>"#
        )
    }

    fn backend_for(port: u16) -> DlnaBackend {
        DlnaBackend::with_gate(
            DlnaConfig {
                location: format!("http://127.0.0.1:{port}/rootDesc.xml"),
                udn: "uuid:x".into(),
                name: "NAS".into(),
                control_url: format!("http://127.0.0.1:{port}/ctl/ContentDir"),
                service_type: "urn:schemas-upnp-org:service:ContentDirectory:1".into(),
            },
            open_gate(),
        )
        .unwrap()
    }

    #[test]
    fn reads_the_ssdp_location_header() {
        let reply = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nLocation: http://192.168.1.2:8200/rootDesc.xml\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n";
        assert_eq!(
            ssdp_location(reply).as_deref(),
            Some("http://192.168.1.2:8200/rootDesc.xml")
        );
        assert_eq!(ssdp_location("HTTP/1.1 200 OK\r\n\r\n"), None);
    }

    #[test]
    fn a_bare_address_tries_the_usual_description_paths() {
        let found = candidate_locations("192.168.1.2:8200").unwrap();
        assert_eq!(found[0], "http://192.168.1.2:8200/rootDesc.xml");
        assert_eq!(found.len(), DESCRIPTION_PATHS.len());
        assert_eq!(
            candidate_locations("http://nas:8895/deviceDescription/abc").unwrap(),
            vec!["http://nas:8895/deviceDescription/abc".to_string()]
        );
        assert!(candidate_locations(" ").is_err());
    }

    #[test]
    fn the_description_names_the_server_and_its_directory() {
        let config =
            parse_description(DESCRIPTION, "http://192.168.1.2:8200/rootDesc.xml").unwrap();
        assert_eq!(config.name, "NAS: minidlna");
        assert_eq!(config.control_url, "http://192.168.1.2:8200/ctl/ContentDir");
        assert_eq!(
            config.service_type,
            "urn:schemas-upnp-org:service:ContentDirectory:1"
        );
        assert_eq!(stable_key(&config), "4d696e69-444c-164e-9d41-b827eb7a1c2f");
        assert!(parse_description("<root><device><deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType></device></root>", "http://a/").is_err());
    }

    #[test]
    fn items_keep_the_playable_resource_and_their_tags() {
        let base = url::Url::parse("http://192.168.1.2:8200/ctl/ContentDir").unwrap();
        let body = format!(
            "{}{}",
            item("64$1", "Song", "/MediaItems/22.flac"),
            r#"<item id="v"><dc:title>Clip</dc:title><upnp:class>object.item.videoItem</upnp:class><res protocolInfo="http-get:*:video/mp4:*">/v.mp4</res></item>"#
        );
        let didl = format!(
            r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">{body}<container id="1$4"/></DIDL-Lite>"#
        );
        let (items, containers) = parse_didl(&didl, &base).unwrap();
        assert_eq!(containers, ["1$4"]);
        assert_eq!(items.len(), 1);
        let meta = item_to_meta(&items[0]);
        assert_eq!(meta.title, "Song");
        assert_eq!(meta.artist, "Band");
        assert_eq!(meta.album_artist.as_deref(), Some("Band & Co"));
        assert_eq!(meta.track_number, Some(4));
        assert_eq!(meta.year, Some(2003));
        assert_eq!(meta.size, 4321);
        assert_eq!(meta.duration_seconds, Some(225.5));
        assert_eq!(meta.extension, "flac");
        assert_eq!(meta.part_key, "http://192.168.1.2:8200/MediaItems/22.flac");
        assert_eq!(
            meta.art_ref.as_deref(),
            Some("http://192.168.1.2:8200/AlbumArt/9.jpg")
        );
        // The id survives the server moving to another address.
        assert_eq!(
            meta.remote_id,
            remote_id_for("http://10.0.0.9:8200/MediaItems/22.flac")
        );
    }

    #[test]
    fn the_catalog_is_one_search_for_audio() {
        let (port, seen) = fake_server(vec![reply(
            "Search",
            &format!(
                "{}{}",
                item("a", "One", "/m/1.flac"),
                item("b", "Two", "/m/2.flac")
            ),
            2,
            2,
        )]);
        let page = backend_for(port).catalog_page(0).unwrap();
        assert_eq!(page.tracks.len(), 2);
        assert_eq!(page.total, Some(2));
        assert_eq!(page.next, None);
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("POST /ctl/ContentDir "));
        assert!(log[0].contains("urn:schemas-upnp-org:service:ContentDirectory:1#Search"));
    }

    #[test]
    fn untagged_items_are_read_from_their_file_names() {
        let base = url::Url::parse("http://nas:8200/ctl").unwrap();
        let untagged = |title: &str| {
            let didl = format!(
                r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/"><item id="1"><dc:title>{title}</dc:title><upnp:class>object.item.audioItem.musicTrack</upnp:class><res size="9" protocolInfo="http-get:*:audio/mpeg:*">/a.mp3</res></item></DIDL-Lite>"#
            );
            item_to_meta(&parse_didl(&didl, &base).unwrap().0[0])
        };
        let meta = untagged("Nirvana - Drain You");
        assert_eq!(
            (meta.artist.as_str(), meta.title.as_str()),
            ("Nirvana", "Drain You")
        );
        let meta = untagged("07 - Creeping Death");
        assert_eq!(meta.title, "Creeping Death");
        assert_eq!(meta.track_number, Some(7));
        assert_eq!(meta.artist, "Unknown Artist");
        // A tagged item keeps its title as the server sent it.
        let base = url::Url::parse("http://192.168.1.2:8200/ctl/ContentDir").unwrap();
        let didl = format!(
            r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">{}</DIDL-Lite>"#,
            item("x", "AC - DC Live", "/m/9.flac")
        );
        assert_eq!(
            item_to_meta(&parse_didl(&didl, &base).unwrap().0[0]).title,
            "AC - DC Live"
        );
    }

    #[test]
    fn durations_take_every_form_the_spec_allows() {
        assert_eq!(parse_duration("0:03:45"), Some(225.0));
        assert_eq!(parse_duration("0:03:45.500"), Some(225.5));
        assert_eq!(parse_duration(":03:45"), None);
        assert_eq!(parse_duration("3:45"), Some(225.0));
        assert_eq!(parse_duration("0:03:45.1/2"), Some(225.5));
        assert_eq!(parse_duration("0:03:45.1/0"), None);
        assert_eq!(parse_duration("garbage"), None);
    }

    #[test]
    fn a_server_that_cannot_count_is_paged_until_a_page_is_empty() {
        // TotalMatches = 0 with items returned means "unknown" (CDS v1 2.7.4.2).
        let (port, _) = fake_server(vec![
            fault(),
            reply("Browse", r#"<container id="c"/>"#, 1, 0),
            reply("Browse", "", 0, 0),
            reply("Browse", &item("a", "One", "/m/1.flac"), 1, 0),
            reply("Browse", &item("b", "Two", "/m/2.flac"), 1, 0),
            reply("Browse", "", 0, 0),
        ]);
        let page = backend_for(port).catalog_page(0).unwrap();
        let titles: Vec<_> = page.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["One", "Two"]);
    }

    #[test]
    fn a_server_without_search_is_browsed_once_per_song() {
        let (port, _) = fake_server(vec![
            fault(),
            // Root: two views that hold the same song.
            reply(
                "Browse",
                r#"<container id="artists"/><container id="folders"/>"#,
                2,
                2,
            ),
            reply("Browse", &item("a1", "One", "/m/1.flac"), 1, 1),
            reply("Browse", &item("f1", "One", "/m/1.flac"), 1, 1),
        ]);
        let page = backend_for(port).catalog_page(0).unwrap();
        assert_eq!(page.tracks.len(), 1);
        assert_eq!(page.tracks[0].title, "One");
        assert_eq!(page.next, None);
    }

    #[test]
    fn audio_is_fetched_with_a_range_and_its_size_can_be_asked() {
        let (port, seen) = fake_server(vec![
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/98765\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx".to_string(),
            http("206 Partial Content", "abc"),
        ]);
        let backend = backend_for(port);
        let url = format!("http://127.0.0.1:{port}/m/1.flac");
        assert_eq!(backend.content_length(&url).unwrap(), 98765);
        let mut body = Vec::new();
        backend
            .open_bytes(&url, 7)
            .unwrap()
            .read_to_end(&mut body)
            .unwrap();
        assert_eq!(body, b"abc");
        let log = seen.lock().unwrap();
        assert!(log[0].to_ascii_lowercase().contains("range: bytes=0-0\r\n"));
        assert!(log[1].to_ascii_lowercase().contains("range: bytes=7-\r\n"));
    }

    #[test]
    fn a_closed_gate_blocks_every_call() {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        assert_eq!(
            discover(gate, Duration::from_millis(10)).err(),
            Some(RemoteError::Blocked(GateState::Disabled))
        );
    }
}
