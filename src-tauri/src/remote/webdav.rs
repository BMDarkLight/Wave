// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! WebDAV folders: Nextcloud, ownCloud, Synology and QNAP NAS shares, rclone,
//! Apache and nginx with DAV, and anything else that answers `PROPFIND`.
//!
//! There is no music API here, only files. The folder is walked one directory
//! at a time (`Depth: 1`, since many servers refuse `infinity`), audio files
//! become tracks, and an image such as `cover.jpg` next to them becomes their
//! artwork. Tags come from the first bytes of each file, read once and kept in
//! a [`TagCache`]; a file with no readable tags is described by its path,
//! `Artist/Album/01 - Title.flac`.

use super::backend::{
    unix_seconds, ByteStream, CatalogPage, RemoteBackend, RemoteError, RemoteTrackMeta,
};
use super::gate::NetworkGate;
use super::http::{api_client, basic_auth_travels_safely, stream_client};
use super::tags::{read_header_tags, HeaderTags, TagCache, HEADER_BYTES};
use super::uri::percent_encode;
use reqwest::blocking::{RequestBuilder, Response};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
use std::io::Read;

/// Tracks per catalog page. A directory is never split, so a page can run over.
const PAGE: usize = 500;
/// Deep enough for Artist/Album/Disc layouts with room to spare, shallow enough
/// that a symlink loop ends.
const MAX_DEPTH: usize = 12;
/// Artwork bigger than this is not a cover.
const MAX_ART_BYTES: u64 = 10 * 1024 * 1024;

const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop>
<d:resourcetype/><d:getcontentlength/><d:getlastmodified/><d:getetag/>
</d:prop></d:propfind>"#;

/// Everything needed to reach one WebDAV folder, stored as `config_json`.
/// Holds the password, so it deliberately has no `Debug`.
#[derive(Clone, Serialize, Deserialize)]
pub struct WebDavConfig {
    /// The music folder, always ending in `/`.
    pub url: String,
    /// Empty when the folder needs no sign-in.
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

impl WebDavConfig {
    pub fn signs_in(&self) -> bool {
        !self.username.is_empty()
    }
}

/// The folder address as typed, made usable: a scheme is added when missing,
/// a query or fragment is dropped, and the path ends in `/` so relative paths
/// join under it rather than beside it.
pub fn normalize_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter the folder's address".to_string());
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("http://{input}")
    };
    let mut url =
        url::Url::parse(&with_scheme).map_err(|_| "That address isn't valid".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Use an http:// or https:// address".to_string());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("Enter the folder's address".to_string());
    }
    url.set_query(None);
    url.set_fragment(None);
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url.to_string())
}

/// Identifies the same account on the same folder, so adding it twice is
/// refused and re-adding it revives its tracks. Paths are case-sensitive on
/// most servers, so only the host is folded.
pub fn stable_key(config: &WebDavConfig) -> String {
    let place = url::Url::parse(&config.url)
        .map(|url| {
            let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
            let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
            format!("{host}{port}{}", url.path())
        })
        .unwrap_or_else(|_| config.url.clone());
    format!("{}@{place}", config.username)
}

/// "Music (nas.local)": the folder's own name and the host.
pub fn display_name(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return "WebDAV".to_string();
    };
    let host = parsed.host_str().unwrap_or_default();
    let folder = parsed
        .path_segments()
        .and_then(|mut segments| segments.rfind(|s| !s.is_empty()))
        .map(decode)
        .filter(|name| !name.is_empty());
    match folder {
        Some(folder) => format!("{folder} ({host})"),
        None => format!("WebDAV ({host})"),
    }
}

fn decode(segment: &str) -> String {
    percent_encoding::percent_decode_str(segment)
        .decode_utf8_lossy()
        .into_owned()
}

/// One entry of a `PROPFIND` answer, with its path decoded.
#[derive(Debug, Clone, PartialEq)]
struct DavEntry {
    path: String,
    is_dir: bool,
    size: u64,
    etag: Option<String>,
    modified: Option<i64>,
}

/// The `DAV:` element named `name` among `node`'s children.
fn dav_child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children().find(|n| {
        n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some("DAV:")
    })
}

fn parse_multistatus(xml: &str, base: &url::Url) -> Result<Vec<DavEntry>, RemoteError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| RemoteError::Parse(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "multistatus" {
        return Err(RemoteError::Parse("not a WebDAV listing".into()));
    }
    let mut entries = Vec::new();
    for response in root
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "response")
    {
        let Some(href) = dav_child(response, "href").and_then(|n| n.text()) else {
            continue;
        };
        let Ok(resolved) = base.join(href.trim()) else {
            continue;
        };
        // RFC 4918 lets a member answer with a bare status in place of its
        // properties, such as a 403 for a folder this login can't read.
        let refused = dav_child(response, "status")
            .and_then(|s| s.text())
            .is_some_and(|status| !status.contains(" 2"));
        if refused {
            continue;
        }
        // Only the properties the server could read; a 404 propstat lists the
        // ones it doesn't have.
        let prop = response
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "propstat")
            .find(|propstat| {
                dav_child(*propstat, "status")
                    .and_then(|s| s.text())
                    .is_none_or(|status| status.contains(" 200"))
            })
            .and_then(|propstat| dav_child(propstat, "prop"));
        let text = |name: &str| {
            prop.and_then(|p| dav_child(p, name))
                .and_then(|n| n.text())
                .map(str::trim)
                .filter(|t| !t.is_empty())
        };
        let is_dir = prop
            .and_then(|p| dav_child(p, "resourcetype"))
            .is_some_and(|rt| dav_child(rt, "collection").is_some())
            || href.trim().ends_with('/');
        entries.push(DavEntry {
            path: decode(resolved.path()),
            is_dir,
            size: text("getcontentlength")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            // A weak tag, W/"abc", names the same version as "abc" here.
            etag: text("getetag").map(|e| e.trim_start_matches("W/").trim_matches('"').to_string()),
            modified: text("getlastmodified").and_then(http_date_seconds),
        });
    }
    Ok(entries)
}

/// Seconds since 1970 for an HTTP date, "Tue, 15 Nov 1994 12:45:26 GMT".
fn http_date_seconds(date: &str) -> Option<i64> {
    let parts: Vec<&str> = date.split_whitespace().collect();
    let [_, day, month, year, time, ..] = parts.as_slice() else {
        return None;
    };
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let month = MONTHS
        .iter()
        .position(|m| month.to_ascii_lowercase().starts_with(m))?
        + 1;
    let day: u32 = day.parse().ok()?;
    unix_seconds(&format!("{year}-{month:02}-{day:02}T{time}Z"))
}

/// A path's segments, each percent-encoded, joined back with `/`.
fn encode_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode)
        .collect::<Vec<_>>()
        .join("/")
}

/// The file name's extension, lowercased.
fn extension_of(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// Folders a NAS or desktop leaves behind that never hold music.
fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with('@')
        || name.eq_ignore_ascii_case("#recycle")
        || name.eq_ignore_ascii_case("$recycle.bin")
}

/// What a file's path says about it when its tags don't.
#[derive(Debug, Clone, Default, PartialEq)]
struct PathMeta {
    title: String,
    artist: Option<String>,
    album: Option<String>,
    track_number: Option<i32>,
    disc_number: Option<i32>,
    year: Option<i32>,
}

/// "CD1", "Disc 2", "disk03".
fn disc_folder_number(name: &str) -> Option<i32> {
    let lower = name.trim().to_ascii_lowercase();
    let rest = ["disc", "disk", "cd"]
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix))?;
    rest.trim_start_matches([' ', '-', '_', '.']).parse().ok()
}

/// "2001 - Discovery" gives the year and the album name.
fn split_year(folder: &str) -> (Option<i32>, String) {
    let year = folder.get(..4).and_then(|y| y.parse::<i32>().ok());
    if let Some(year) = year.filter(|y| (1900..=2100).contains(y)) {
        let rest = folder[4..].trim_start_matches([' ', '-', '_', '.', ')', ']']);
        if !rest.is_empty() && rest.len() < folder.len() - 4 {
            return (Some(year), rest.to_string());
        }
    }
    (None, folder.to_string())
}

/// "01 - Title", "1-03 Title", "07. Title": the leading number and the rest.
fn split_track_number(stem: &str) -> (Option<i32>, Option<i32>, &str) {
    let digits = stem.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 3 {
        return (None, None, stem);
    }
    let (number, rest) = stem.split_at(digits);
    // "1-03 Title" is disc 1, track 3.
    if let Some(after) = rest.strip_prefix('-') {
        let track_digits = after.chars().take_while(|c| c.is_ascii_digit()).count();
        if (1..=3).contains(&track_digits) {
            let (track, title) = after.split_at(track_digits);
            let title = title.trim_start_matches([' ', '-', '_', '.']);
            if !title.is_empty() {
                return (track.parse().ok(), number.parse().ok(), title);
            }
        }
    }
    let title = rest.trim_start_matches([' ', '-', '_', '.']);
    if title.is_empty() || title.len() == rest.len() {
        return (None, None, stem);
    }
    (number.parse().ok(), None, title)
}

fn meta_from_path(relative: &str) -> PathMeta {
    let mut segments: Vec<&str> = relative.split('/').filter(|s| !s.is_empty()).collect();
    let file = segments.pop().unwrap_or_default();
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);

    let disc_number = segments.last().and_then(|name| disc_folder_number(name));
    if disc_number.is_some() {
        segments.pop();
    }
    let (year, album) = match segments.last() {
        Some(folder) => {
            let (year, album) = split_year(folder);
            (year, Some(album))
        }
        None => (None, None),
    };
    let artist = (segments.len() >= 2).then(|| segments[segments.len() - 2].to_string());
    let name = name_meta(stem, artist);
    PathMeta {
        title: name.title,
        artist: name.artist,
        album,
        track_number: name.track_number,
        disc_number: disc_number.or(name.disc_number),
        year,
    }
}

/// What a file name without its extension says about a song.
pub(crate) struct NameMeta {
    pub title: String,
    pub artist: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
}

/// Reads "07 - Title", "1-03 Title" and "Artist - Title", given the artist
/// the folders named, if any. Also used for DLNA servers that send file
/// names as titles and no tags.
pub(crate) fn name_meta(stem: &str, mut artist: Option<String>) -> NameMeta {
    let (track_number, disc_number, rest) = split_track_number(stem);
    let mut title = rest.to_string();
    // "Artist - Title" names the artist when the folders don't, and repeats it
    // when they do.
    if let Some((left, right)) = title.split_once(" - ") {
        let left = left.trim();
        let right = right.trim();
        match &artist {
            Some(known) if known.eq_ignore_ascii_case(left) && !right.is_empty() => {
                title = right.to_string();
            }
            None if !left.is_empty() && !right.is_empty() => {
                artist = Some(left.to_string());
                title = right.to_string();
            }
            _ => {}
        }
    }
    NameMeta {
        title,
        artist,
        track_number,
        disc_number,
    }
}

/// A short stable id for a file path. The path itself can be long and full of
/// characters a cache file name can't hold.
fn remote_id_for(relative: &str) -> String {
    let digest = Sha256::digest(relative.as_bytes());
    digest[..10].iter().map(|b| format!("{b:02x}")).collect()
}

/// The picture in a folder most likely to be its cover.
fn pick_cover<'a>(images: &[&'a str]) -> Option<&'a str> {
    let stem = |path: &str| {
        let name = path.rsplit('/').next().unwrap_or(path);
        name.rsplit_once('.')
            .map_or(name, |(stem, _)| stem)
            .to_ascii_lowercase()
    };
    for wanted in ["cover", "folder", "front", "album", "albumart"] {
        if let Some(found) = images.iter().find(|path| stem(path) == wanted) {
            return Some(found);
        }
    }
    let mut sorted = images.to_vec();
    sorted.sort_unstable();
    sorted.first().copied()
}

struct Walk {
    queue: VecDeque<(String, usize)>,
    visited: HashSet<String>,
    tags: TagCache,
}

pub struct WebDavBackend {
    config: WebDavConfig,
    base: url::Url,
    gate: &'static NetworkGate,
    walk: parking_lot::Mutex<Option<Walk>>,
}

impl WebDavBackend {
    pub fn new(config: WebDavConfig) -> Result<Self, RemoteError> {
        Self::with_gate(config, NetworkGate::global())
    }

    pub fn with_gate(
        config: WebDavConfig,
        gate: &'static NetworkGate,
    ) -> Result<Self, RemoteError> {
        let base = url::Url::parse(&config.url)
            .map_err(|_| RemoteError::Parse("The folder address isn't valid".into()))?;
        Ok(Self {
            config,
            base,
            gate,
            walk: parking_lot::Mutex::new(None),
        })
    }

    fn unreachable(&self) -> RemoteError {
        let host = self.base.host_str().unwrap_or_default();
        RemoteError::Unreachable(format!("Can't reach {host}"))
    }

    fn url_for(&self, relative: &str) -> Result<url::Url, RemoteError> {
        self.base
            .join(&encode_path(relative))
            .map_err(|_| RemoteError::Parse("Bad file path".into()))
    }

    /// The path below the music folder, or `None` for anything outside it.
    fn relative(&self, path: &str) -> Option<String> {
        let root = decode(self.base.path());
        path.strip_prefix(&root).map(str::to_string)
    }

    fn authorized(&self, request: RequestBuilder) -> Result<RequestBuilder, RemoteError> {
        self.gate.check()?;
        if !self.config.signs_in() {
            return Ok(request);
        }
        if !basic_auth_travels_safely(&self.config.url) {
            return Err(RemoteError::Insecure);
        }
        Ok(request.basic_auth(&self.config.username, Some(&self.config.password)))
    }

    fn propfind(&self, relative: &str, depth: &str) -> Result<Vec<DavEntry>, RemoteError> {
        let url = self.url_for(relative)?;
        let method = Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid method");
        let resp = self
            .authorized(api_client().request(method, url.clone()))?
            .header("Depth", depth)
            // Nextcloud's public share endpoint refuses a PROPFIND without it.
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(PROPFIND_BODY)
            .send()
            .map_err(|_| self.unreachable())?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(RemoteError::Unauthorized);
        }
        if !status.is_success() {
            return Err(RemoteError::Http(status.as_u16()));
        }
        let body = resp.text().map_err(|_| self.unreachable())?;
        parse_multistatus(&body, &url)
    }

    /// Bytes of a file from `offset`, up to `last` when given.
    fn get_range(
        &self,
        relative: &str,
        offset: u64,
        last: Option<u64>,
    ) -> Result<Response, RemoteError> {
        let range = match last {
            Some(last) => format!("bytes={offset}-{last}"),
            None => format!("bytes={offset}-"),
        };
        let resp = self
            .authorized(stream_client().get(self.url_for(relative)?))?
            .header("Range", range)
            .send()
            .map_err(|_| self.unreachable())?;
        match resp.status().as_u16() {
            206 => Ok(resp),
            200 if offset == 0 => Ok(resp),
            401 | 403 => Err(RemoteError::Unauthorized),
            code => Err(RemoteError::Http(code)),
        }
    }

    /// Checks the folder and the login before the source is stored.
    pub fn probe(&self) -> Result<(), RemoteError> {
        let entries = self.propfind("", "0")?;
        if entries.first().is_some_and(|entry| entry.is_dir) {
            Ok(())
        } else {
            Err(RemoteError::Parse(
                "That address is a file, not a folder".into(),
            ))
        }
    }

    /// Header tags for one file, from the cache when the file hasn't changed.
    fn tags_for(
        &self,
        tags: &mut TagCache,
        entry: &DavEntry,
        relative: &str,
        extension: &str,
    ) -> Option<HeaderTags> {
        let fingerprint = entry
            .etag
            .clone()
            .unwrap_or_else(|| format!("{}-{}", entry.size, entry.modified.unwrap_or_default()));
        if let Some(known) = tags.get(relative, &fingerprint) {
            return known;
        }
        let last = HEADER_BYTES.min(entry.size.max(1)) - 1;
        let read = self
            .get_range(relative, 0, Some(last))
            .ok()
            .and_then(|resp| {
                let mut bytes = Vec::new();
                resp.take(HEADER_BYTES).read_to_end(&mut bytes).ok()?;
                Some(bytes)
            });
        // A failed read is not recorded, so the next sync tries again.
        let bytes = read?;
        let found = read_header_tags(bytes, extension);
        tags.insert(relative, &fingerprint, found.clone());
        found
    }

    fn track_meta(
        &self,
        tags: &mut TagCache,
        entry: &DavEntry,
        relative: &str,
        extension: String,
        art: Option<&str>,
    ) -> RemoteTrackMeta {
        let path = meta_from_path(relative);
        let read = self
            .tags_for(tags, entry, relative, &extension)
            .unwrap_or_default();
        RemoteTrackMeta {
            remote_id: remote_id_for(relative),
            title: read.title.unwrap_or(path.title),
            artist: read
                .artist
                .or(path.artist)
                .unwrap_or_else(|| "Unknown Artist".to_string()),
            album_artist: read.album_artist,
            album: read
                .album
                .or(path.album)
                .unwrap_or_else(|| "Unknown Album".to_string()),
            track_number: read.track_number.or(path.track_number),
            disc_number: read.disc_number.or(path.disc_number),
            year: read.year.or(path.year),
            genre: read.genre,
            duration_seconds: None,
            size: entry.size,
            extension,
            part_key: relative.to_string(),
            art_ref: art.map(str::to_string),
            updated_at: entry.modified,
        }
    }

    /// Lists one directory: its tracks, and its subdirectories onto the queue.
    fn walk_dir(
        &self,
        walk: &mut Walk,
        dir: &str,
        depth: usize,
    ) -> Result<Vec<RemoteTrackMeta>, RemoteError> {
        let entries = self.propfind(dir, "1")?;
        let audio_extensions = crate::metadata::supported_audio_extensions();
        let mut files = Vec::new();
        let mut images = Vec::new();
        for entry in &entries {
            let Some(relative) = self.relative(&entry.path) else {
                continue;
            };
            let relative = relative.trim_end_matches('/').to_string();
            // The directory lists itself first.
            if relative == dir.trim_end_matches('/') {
                continue;
            }
            let name = relative.rsplit('/').next().unwrap_or_default();
            if is_hidden(name) {
                continue;
            }
            if entry.is_dir {
                let child = format!("{relative}/");
                if depth < MAX_DEPTH && walk.visited.insert(child.clone()) {
                    walk.queue.push_back((child, depth + 1));
                }
                continue;
            }
            match extension_of(&relative) {
                Some(ext) if audio_extensions.contains(&ext) => files.push((entry, relative, ext)),
                Some(ext)
                    if matches!(ext.as_str(), "jpg" | "jpeg" | "png")
                        && entry.size <= MAX_ART_BYTES =>
                {
                    images.push(relative)
                }
                _ => {}
            }
        }
        let image_refs: Vec<&str> = images.iter().map(String::as_str).collect();
        let cover = pick_cover(&image_refs);
        let mut tracks = Vec::with_capacity(files.len());
        for (entry, relative, ext) in files {
            tracks.push(self.track_meta(&mut walk.tags, entry, &relative, ext, cover));
        }
        Ok(tracks)
    }
}

impl RemoteBackend for WebDavBackend {
    fn kind(&self) -> &'static str {
        "webdav"
    }

    fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
        self.gate.check()?;
        let mut slot = self.walk.lock();
        if cursor == 0 || slot.is_none() {
            *slot = Some(Walk {
                queue: VecDeque::from([(String::new(), 0)]),
                visited: HashSet::from([String::new()]),
                tags: TagCache::open(&stable_key(&self.config)),
            });
        }
        let walk = slot.as_mut().expect("the walk was just set");
        let mut tracks = Vec::new();
        while tracks.len() < PAGE {
            let Some((dir, depth)) = walk.queue.pop_front() else {
                break;
            };
            match self.walk_dir(walk, &dir, depth) {
                Ok(found) => tracks.extend(found),
                // A folder that vanished or can't be read mid-walk is skipped;
                // anything else ends the sync, and tombstones nothing.
                Err(RemoteError::Http(_)) if !dir.is_empty() => {}
                Err(e) => {
                    walk.tags.save();
                    return Err(e);
                }
            }
        }
        walk.tags.save();
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

    fn open_bytes(&self, part_key: &str, offset: u64) -> Result<Box<dyn ByteStream>, RemoteError> {
        Ok(Box::new(self.get_range(part_key, offset, None)?))
    }

    fn check_reachable(&self) -> Result<(), RemoteError> {
        self.propfind("", "0").map(|_| ())
    }

    fn fetch_artwork(&self, art_ref: &str) -> Result<Vec<u8>, RemoteError> {
        let resp = self.get_range(art_ref, 0, None)?;
        let mut bytes = Vec::new();
        resp.take(MAX_ART_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|_| self.unreachable())?;
        Ok(bytes)
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

    fn multistatus(body: &str) -> String {
        let xml =
            format!(r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">{body}</d:multistatus>"#);
        format!(
            "HTTP/1.1 207 Multi-Status\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
            xml.len()
        )
    }

    fn dir(href: &str) -> String {
        format!(
            r#"<d:response><d:href>{href}</d:href><d:propstat><d:prop>
               <d:resourcetype><d:collection/></d:resourcetype></d:prop>
               <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#
        )
    }

    fn file(href: &str, size: u64, etag: &str) -> String {
        format!(
            r#"<d:response><d:href>{href}</d:href><d:propstat><d:prop>
               <d:resourcetype/><d:getcontentlength>{size}</d:getcontentlength>
               <d:getetag>"{etag}"</d:getetag>
               <d:getlastmodified>Tue, 15 Nov 1994 12:45:26 GMT</d:getlastmodified>
               </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#
        )
    }

    fn backend_for(port: u16, path: &str, user: &str) -> WebDavBackend {
        WebDavBackend::with_gate(
            WebDavConfig {
                url: format!("http://127.0.0.1:{port}{path}"),
                username: user.into(),
                password: "sesame".into(),
            },
            open_gate(),
        )
        .unwrap()
    }

    #[test]
    fn addresses_get_a_scheme_and_a_trailing_slash() {
        assert_eq!(
            normalize_url("nas:5005/music").unwrap(),
            "http://nas:5005/music/"
        );
        assert_eq!(
            normalize_url("https://cloud.example.com/remote.php/dav/files/alice/Music?x=1#y")
                .unwrap(),
            "https://cloud.example.com/remote.php/dav/files/alice/Music/"
        );
        assert!(normalize_url("  ").is_err());
        assert!(normalize_url("ftp://nas/music").is_err());
    }

    #[test]
    fn names_and_keys_come_from_the_folder_and_account() {
        assert_eq!(
            display_name("https://cloud.example.com/remote.php/dav/files/alice/My%20Music/"),
            "My Music (cloud.example.com)"
        );
        assert_eq!(display_name("http://nas:8080/"), "WebDAV (nas)");
        let config = WebDavConfig {
            url: "http://NAS.local:5005/Music/".into(),
            username: "alice".into(),
            password: "x".into(),
        };
        assert_eq!(stable_key(&config), "alice@nas.local:5005/Music/");
    }

    #[test]
    fn paths_describe_untagged_files() {
        let meta = meta_from_path("Daft Punk/2001 - Discovery/03 - Digital Love.flac");
        assert_eq!(meta.artist.as_deref(), Some("Daft Punk"));
        assert_eq!(meta.album.as_deref(), Some("Discovery"));
        assert_eq!(meta.year, Some(2001));
        assert_eq!(meta.track_number, Some(3));
        assert_eq!(meta.title, "Digital Love");

        let meta = meta_from_path("Band/Live/CD2/1-07 Encore.mp3");
        assert_eq!(meta.album.as_deref(), Some("Live"));
        assert_eq!(meta.disc_number, Some(2));
        assert_eq!(meta.track_number, Some(7));
        assert_eq!(meta.title, "Encore");

        let meta = meta_from_path("Singles/Muse - Uprising.mp3");
        assert_eq!(meta.artist.as_deref(), Some("Muse"));
        assert_eq!(meta.title, "Uprising");
        assert_eq!(meta.album.as_deref(), Some("Singles"));

        let meta = meta_from_path("Band/Album/Band - 2 Minutes.mp3");
        assert_eq!(meta.title, "2 Minutes");
        // A title that only starts with a number keeps it.
        let meta = meta_from_path("1979.mp3");
        assert_eq!(meta.title, "1979");
        assert_eq!(meta.track_number, None);
    }

    #[test]
    fn listings_parse_dirs_files_and_dates() {
        let base = url::Url::parse("http://nas/dav/Music/").unwrap();
        let xml = format!(
            r#"<d:multistatus xmlns:d="DAV:">{}{}</d:multistatus>"#,
            dir("/dav/Music/"),
            file("/dav/Music/A%20B/song.mp3", 1234, "e1")
        );
        let entries = parse_multistatus(&xml, &base).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].path, "/dav/Music/A B/song.mp3");
        assert_eq!(entries[1].size, 1234);
        assert_eq!(entries[1].etag.as_deref(), Some("e1"));
        assert_eq!(entries[1].modified, Some(784_903_526));
        assert!(parse_multistatus("<html/>", &base).is_err());
    }

    /// The shapes RFC 4918 itself uses: an absolute URI as `href`, a member
    /// refused with a bare status (section 9.1 example), a weak ETag, a prefix
    /// other than `d:`, and a property the server reports as 404 in its own
    /// propstat beside the 200 one.
    #[test]
    fn listings_follow_rfc_4918_examples() {
        let base = url::Url::parse("http://www.example.com/container/").unwrap();
        let xml = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>http://www.example.com/container/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>http://www.example.com/container/secret/</D:href>
    <D:status>HTTP/1.1 403 Forbidden</D:status>
  </D:response>
  <D:response>
    <D:href>front.mp3</D:href>
    <D:propstat><D:prop><D:getcontenttype/></D:prop>
      <D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>
    <D:propstat><D:prop>
      <D:resourcetype/><D:getcontentlength>4525</D:getcontentlength>
      <D:getetag>W/"zzyzx"</D:getetag>
      <D:getlastmodified>Mon, 12 Jan 1998 09:25:56 GMT</D:getlastmodified>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
</D:multistatus>"#;
        let entries = parse_multistatus(xml, &base).unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["/container/", "/container/front.mp3"]);
        let file = &entries[1];
        assert!(!file.is_dir);
        assert_eq!(file.size, 4525);
        assert_eq!(file.etag.as_deref(), Some("zzyzx"));
        assert_eq!(file.modified, unix_seconds("1998-01-12T09:25:56Z"));
    }

    #[test]
    fn a_walk_lists_audio_reads_headers_and_finds_covers() {
        let etag = format!("e-{}", uuid::Uuid::new_v4());
        let (port, seen) = fake_server(vec![
            multistatus(&format!(
                "{}{}{}",
                dir("/Music/"),
                dir("/Music/Band/"),
                dir("/Music/.Trash/")
            )),
            multistatus(&format!(
                "{}{}",
                dir("/Music/Band/"),
                dir("/Music/Band/Album/")
            )),
            multistatus(&format!(
                "{}{}{}{}",
                dir("/Music/Band/Album/"),
                file("/Music/Band/Album/01%20-%20One.mp3", 5000, &etag),
                file("/Music/Band/Album/cover.jpg", 300, "c"),
                file("/Music/Band/Album/notes.txt", 10, "n")
            )),
            // The header read: no tags, so the path describes the track.
            http("206 Partial Content", "plain bytes"),
        ]);
        let backend = backend_for(port, "/Music/", "");
        let page = backend.catalog_page(0).unwrap();
        assert_eq!(page.next, None);
        assert_eq!(page.tracks.len(), 1);
        let track = &page.tracks[0];
        assert_eq!(track.title, "One");
        assert_eq!(track.artist, "Band");
        assert_eq!(track.album, "Album");
        assert_eq!(track.track_number, Some(1));
        assert_eq!(track.size, 5000);
        assert_eq!(track.extension, "mp3");
        assert_eq!(track.part_key, "Band/Album/01 - One.mp3");
        assert_eq!(track.art_ref.as_deref(), Some("Band/Album/cover.jpg"));

        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("PROPFIND /Music/ "), "{}", log[0]);
        assert!(log[0].to_ascii_lowercase().contains("depth: 1\r\n"));
        assert!(log[2].starts_with("PROPFIND /Music/Band/Album/ "));
        assert!(log[3].starts_with("GET /Music/Band/Album/01%20-%20One.mp3 "));
        assert!(log[3]
            .to_ascii_lowercase()
            .contains("range: bytes=0-4999\r\n"));
        assert!(!log.iter().any(|r| r.contains(".Trash")));
    }

    #[test]
    fn audio_is_read_with_a_range_and_basic_auth() {
        let (port, seen) = fake_server(vec![http("206 Partial Content", "abc")]);
        let mut stream = backend_for(port, "/dav/", "alice")
            .open_bytes("A/b c.flac", 10)
            .unwrap();
        let mut body = Vec::new();
        stream.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"abc");
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("GET /dav/A/b%20c.flac "), "{}", log[0]);
        assert!(log[0].to_ascii_lowercase().contains("range: bytes=10-\r\n"));
        assert!(
            log[0].contains("authorization: Basic YWxpY2U6c2VzYW1l"),
            "{}",
            log[0]
        );
    }

    #[test]
    fn a_whole_file_for_a_seek_is_reported_so_the_fill_can_restart() {
        let (port, _) = fake_server(vec![http("200 OK", "everything")]);
        assert_eq!(
            backend_for(port, "/", "").open_bytes("a.mp3", 5).err(),
            Some(RemoteError::Http(200))
        );
    }

    #[test]
    fn a_refused_login_is_unauthorized() {
        let (port, _) = fake_server(vec![http("401 Unauthorized", "")]);
        assert_eq!(
            backend_for(port, "/", "alice").probe().err(),
            Some(RemoteError::Unauthorized)
        );
    }

    #[test]
    fn the_password_never_crosses_the_internet_in_the_clear() {
        let backend = WebDavBackend::with_gate(
            WebDavConfig {
                url: "http://203.0.113.9/dav/".into(),
                username: "alice".into(),
                password: "x".into(),
            },
            open_gate(),
        )
        .unwrap();
        assert_eq!(backend.probe().err(), Some(RemoteError::Insecure));
    }

    #[test]
    fn a_closed_gate_blocks_every_call() {
        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        let (port, seen) = fake_server(vec![http("207 Multi-Status", "")]);
        let backend = WebDavBackend::with_gate(
            WebDavConfig {
                url: format!("http://127.0.0.1:{port}/"),
                username: String::new(),
                password: String::new(),
            },
            gate,
        )
        .unwrap();
        assert_eq!(
            backend.catalog_page(0).err(),
            Some(RemoteError::Blocked(GateState::Disabled))
        );
        assert!(backend.open_bytes("a.mp3", 0).is_err());
        assert!(seen.lock().unwrap().is_empty());
    }
}
