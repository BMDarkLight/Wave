// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The shared library as UPnP ContentDirectory objects.
//!
//! The tree is small and fixed: the root holds Artists, Albums, All songs and
//! Playlists; an artist holds their albums, an album or playlist holds songs.
//! Object ids are stable across rebuilds (`track:<id>`, `album:<hash>`) so a
//! client that remembered one still finds it after the library changes.
//!
//! Only files Wave can read itself are shared: local files, downloads, and
//! `content://` documents on Android. Tracks of someone else's server are
//! left out.

use crate::library::{Library, LIBRARY_TRACK_FROM, TRACK_SELECT_COLUMNS};
use crate::metadata::Track;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Ids of the fixed containers.
pub const ROOT: &str = "0";
const ARTISTS: &str = "artists";
const ALBUMS: &str = "albums";
const SONGS: &str = "songs";
const PLAYLISTS: &str = "playlists";

const CLASS_TRACK: &str = "object.item.audioItem.musicTrack";
const CLASS_FOLDER: &str = "object.container.storageFolder";
const CLASS_ALBUM: &str = "object.container.album.musicAlbum";
const CLASS_ARTIST: &str = "object.container.person.musicArtist";
const CLASS_PLAYLIST: &str = "object.container.playlistContainer";

/// UPnP error codes the ContentDirectory spec defines for Browse and Search.
pub const NO_SUCH_OBJECT: u16 = 701;
pub const BAD_SEARCH_CRITERIA: u16 = 708;
pub const NO_SUCH_CONTAINER: u16 = 710;
pub const INVALID_ARGS: u16 = 402;

#[derive(Debug, Clone, PartialEq)]
pub struct Fault {
    pub code: u16,
    pub description: &'static str,
}

impl Fault {
    pub fn new(code: u16, description: &'static str) -> Self {
        Self { code, description }
    }
}

impl Library {
    /// Every track the library shows whose file Wave can open itself.
    pub fn shareable_tracks(&self) -> Result<Vec<Track>, String> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(&format!(
                "SELECT {TRACK_SELECT_COLUMNS} FROM {LIBRARY_TRACK_FROM}
                 WHERE t.path NOT LIKE 'wave-remote://%'"
            ))
            .map_err(|e| format!("Failed to list shared tracks: {e}"))?;
        let cover_root = self.cover_root().to_path_buf();
        let rows = statement
            .query_map([], |row| crate::library::row_to_track(row, &cover_root))
            .map_err(|e| format!("Failed to list shared tracks: {e}"))?;
        rows.collect::<Result<_, _>>()
            .map_err(|e| format!("Failed to read a shared track: {e}"))
    }

    /// Changes whenever the shared library does: a song added, removed or
    /// retagged, or a playlist edited. Clients compare it to know when to
    /// browse again (`SystemUpdateID`).
    pub fn share_version(&self) -> Result<u32, String> {
        let connection = self.read_connection();
        let (count, indexed, modified): (i64, i64, i64) = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(indexed_at), 0), COALESCE(MAX(modified_at), 0)
                 FROM library_tracks WHERE path NOT LIKE 'wave-remote://%'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| format!("Failed to read the library version: {e}"))?;
        let playlists: (i64, i64) = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM playlist_tracks),
                        COALESCE((SELECT MAX(updated_at) FROM playlists), 0)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((0, 0));
        let digest = Sha256::digest(
            format!(
                "{count}:{indexed}:{modified}:{}:{}",
                playlists.0, playlists.1
            )
            .as_bytes(),
        );
        Ok(u32::from_be_bytes([
            digest[0], digest[1], digest[2], digest[3],
        ]))
    }
}

/// A playlist and the paths of its songs, in order.
pub struct PlaylistSource {
    pub id: String,
    pub name: String,
    pub paths: Vec<String>,
}

struct Album {
    id: String,
    title: String,
    artist: String,
    year: Option<i32>,
    tracks: Vec<usize>,
}

struct Artist {
    id: String,
    name: String,
    albums: Vec<usize>,
}

struct Playlist {
    id: String,
    name: String,
    tracks: Vec<usize>,
}

/// One object of the tree.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Object {
    Root,
    Folder(&'static str),
    Artist(usize),
    Album(usize),
    Playlist(usize),
    Track(usize),
}

/// The shared library at one moment, ready to browse and search.
pub struct Snapshot {
    pub version: u32,
    tracks: Vec<Track>,
    track_index: HashMap<String, usize>,
    albums: Vec<Album>,
    album_index: HashMap<String, usize>,
    artists: Vec<Artist>,
    artist_index: HashMap<String, usize>,
    playlists: Vec<Playlist>,
    playlist_index: HashMap<String, usize>,
}

fn short_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn fold(text: &str) -> String {
    text.trim().to_lowercase()
}

/// The artist an album is filed under: its album artist, else its artist.
fn album_artist(track: &Track) -> String {
    track
        .album_artist
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .unwrap_or(track.artist.trim())
        .to_string()
}

impl Snapshot {
    pub fn build(version: u32, mut tracks: Vec<Track>, playlists: Vec<PlaylistSource>) -> Self {
        let order = |t: &Track| {
            (
                fold(&album_artist(t)),
                fold(&t.album),
                t.disc_number.unwrap_or(0),
                t.track_number.unwrap_or(0),
                fold(&t.title),
            )
        };
        tracks.sort_by_cached_key(order);
        let track_index: HashMap<String, usize> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.clone(), i))
            .collect();

        let mut albums: Vec<Album> = Vec::new();
        let mut album_index: HashMap<String, usize> = HashMap::new();
        for (i, track) in tracks.iter().enumerate() {
            let artist = album_artist(track);
            let key = format!("{}\u{0}{}", fold(&track.album), fold(&artist));
            let id = format!("album:{}", short_hash(&key));
            let slot = *album_index.entry(id.clone()).or_insert_with(|| {
                albums.push(Album {
                    id,
                    title: track.album.clone(),
                    artist,
                    year: track.year,
                    tracks: Vec::new(),
                });
                albums.len() - 1
            });
            albums[slot].tracks.push(i);
        }

        let mut artists: Vec<Artist> = Vec::new();
        let mut artist_index: HashMap<String, usize> = HashMap::new();
        for (i, album) in albums.iter().enumerate() {
            let id = format!("artist:{}", short_hash(&fold(&album.artist)));
            let slot = *artist_index.entry(id.clone()).or_insert_with(|| {
                artists.push(Artist {
                    id,
                    name: album.artist.clone(),
                    albums: Vec::new(),
                });
                artists.len() - 1
            });
            artists[slot].albums.push(i);
        }
        artists.sort_by_cached_key(|a| fold(&a.name));
        let artist_index = artists
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id.clone(), i))
            .collect();
        // Albums list alphabetically; artists keep their albums by year.
        let mut by_title: Vec<usize> = (0..albums.len()).collect();
        by_title.sort_by_cached_key(|&i| fold(&albums[i].title));
        let remap: HashMap<usize, usize> = by_title
            .iter()
            .enumerate()
            .map(|(new, &old)| (old, new))
            .collect();
        let mut sorted_albums: Vec<Option<Album>> = albums.into_iter().map(Some).collect();
        let albums: Vec<Album> = by_title
            .iter()
            .map(|&old| sorted_albums[old].take().expect("each album moves once"))
            .collect();
        for artist in &mut artists {
            for album in &mut artist.albums {
                *album = remap[album];
            }
            artist
                .albums
                .sort_by_key(|&i| (albums[i].year.unwrap_or(i32::MAX), fold(&albums[i].title)));
        }
        let album_index = albums
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id.clone(), i))
            .collect();

        let by_path: HashMap<&str, usize> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.path.as_str(), i))
            .collect();
        let playlists: Vec<Playlist> = playlists
            .into_iter()
            .map(|p| Playlist {
                id: format!("playlist:{}", p.id),
                name: p.name,
                tracks: p
                    .paths
                    .iter()
                    .filter_map(|path| by_path.get(path.as_str()).copied())
                    .collect(),
            })
            .filter(|p| !p.tracks.is_empty())
            .collect();
        let playlist_index = playlists
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();

        Self {
            version,
            tracks,
            track_index,
            albums,
            album_index,
            artists,
            artist_index,
            playlists,
            playlist_index,
        }
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// A shared track by its library id.
    pub fn track(&self, id: &str) -> Option<&Track> {
        self.track_index.get(id).map(|&i| &self.tracks[i])
    }

    fn object(&self, id: &str) -> Option<Object> {
        Some(match id {
            ROOT => Object::Root,
            ARTISTS => Object::Folder(ARTISTS),
            ALBUMS => Object::Folder(ALBUMS),
            SONGS => Object::Folder(SONGS),
            PLAYLISTS => Object::Folder(PLAYLISTS),
            _ => {
                let (kind, rest) = id.split_once(':')?;
                match kind {
                    "track" => Object::Track(*self.track_index.get(rest)?),
                    "album" => Object::Album(*self.album_index.get(id)?),
                    "artist" => Object::Artist(*self.artist_index.get(id)?),
                    "playlist" => Object::Playlist(*self.playlist_index.get(id)?),
                    _ => return None,
                }
            }
        })
    }

    fn children(&self, object: Object) -> Vec<Object> {
        match object {
            Object::Root => [ARTISTS, ALBUMS, SONGS, PLAYLISTS]
                .into_iter()
                .map(Object::Folder)
                .collect(),
            Object::Folder(ARTISTS) => (0..self.artists.len()).map(Object::Artist).collect(),
            Object::Folder(ALBUMS) => (0..self.albums.len()).map(Object::Album).collect(),
            Object::Folder(SONGS) => (0..self.tracks.len()).map(Object::Track).collect(),
            Object::Folder(_) => (0..self.playlists.len()).map(Object::Playlist).collect(),
            Object::Artist(i) => self.artists[i]
                .albums
                .iter()
                .map(|&a| Object::Album(a))
                .collect(),
            Object::Album(i) => self.albums[i]
                .tracks
                .iter()
                .map(|&t| Object::Track(t))
                .collect(),
            Object::Playlist(i) => self.playlists[i]
                .tracks
                .iter()
                .map(|&t| Object::Track(t))
                .collect(),
            Object::Track(_) => Vec::new(),
        }
    }

    fn id_of(&self, object: Object) -> String {
        match object {
            Object::Root => ROOT.to_string(),
            Object::Folder(id) => id.to_string(),
            Object::Artist(i) => self.artists[i].id.clone(),
            Object::Album(i) => self.albums[i].id.clone(),
            Object::Playlist(i) => self.playlists[i].id.clone(),
            Object::Track(i) => format!("track:{}", self.tracks[i].id),
        }
    }

    /// The parent a lone object reports: the place it lives in the tree.
    fn home_of(&self, object: Object) -> String {
        match object {
            Object::Root => "-1".to_string(),
            Object::Folder(_) => ROOT.to_string(),
            Object::Artist(_) => ARTISTS.to_string(),
            Object::Album(_) => ALBUMS.to_string(),
            Object::Playlist(_) => PLAYLISTS.to_string(),
            Object::Track(i) => {
                let track = &self.tracks[i];
                let key = format!("{}\u{0}{}", fold(&track.album), fold(&album_artist(track)));
                format!("album:{}", short_hash(&key))
            }
        }
    }

    fn class_of(object: Object) -> &'static str {
        match object {
            Object::Root | Object::Folder(_) => CLASS_FOLDER,
            Object::Artist(_) => CLASS_ARTIST,
            Object::Album(_) => CLASS_ALBUM,
            Object::Playlist(_) => CLASS_PLAYLIST,
            Object::Track(_) => CLASS_TRACK,
        }
    }

    /// `Browse`: one object's own metadata, or a page of its children.
    pub fn browse(
        &self,
        object_id: &str,
        direct_children: bool,
        start: usize,
        count: usize,
        base: &str,
    ) -> Result<Page, Fault> {
        let object = self
            .object(object_id)
            .ok_or(Fault::new(NO_SUCH_OBJECT, "No such object"))?;
        if !direct_children {
            let parent = self.home_of(object);
            return Ok(Page {
                didl: didl(&[self.render(object, &parent, base)]),
                returned: 1,
                total: 1,
            });
        }
        let children = self.children(object);
        let parent = self.id_of(object);
        Ok(self.page(&children, Some(&parent), start, count, base))
    }

    /// `Search`: every object under `container_id` that `criteria` matches.
    pub fn search(
        &self,
        container_id: &str,
        criteria: &str,
        start: usize,
        count: usize,
        base: &str,
    ) -> Result<Page, Fault> {
        let container = self
            .object(container_id)
            .ok_or(Fault::new(NO_SUCH_CONTAINER, "No such container"))?;
        let expression = parse_criteria(criteria)
            .map_err(|_| Fault::new(BAD_SEARCH_CRITERIA, "Unsupported search criteria"))?;
        let mut found = Vec::new();
        let mut stack = vec![container];
        let mut seen = std::collections::HashSet::new();
        while let Some(object) = stack.pop() {
            for child in self.children(object).into_iter().rev() {
                if seen.insert(self.id_of(child)) {
                    stack.push(child);
                }
            }
            if object != container && expression.matches(&|p| self.property(object, p)) {
                found.push(object);
            }
        }
        // Depth-first order with tracks after their containers reads oddly;
        // containers first, then songs, each in tree order, is what clients
        // expect from a flat result list.
        found.sort_by_key(|o| matches!(o, Object::Track(_)));
        // A search result reports where it lives, not the container searched.
        Ok(self.page(&found, None, start, count, base))
    }

    /// A page of `objects`, all children of `parent`, or each reporting its
    /// own home when `parent` is `None`. `count` 0 means all of them.
    fn page(
        &self,
        objects: &[Object],
        parent: Option<&str>,
        start: usize,
        count: usize,
        base: &str,
    ) -> Page {
        let total = objects.len();
        let end = if count == 0 {
            total
        } else {
            start.saturating_add(count).min(total)
        };
        let slice = objects.get(start.min(total)..end).unwrap_or_default();
        let rendered: Vec<String> = slice
            .iter()
            .map(|&o| {
                let parent = parent.map_or_else(|| self.home_of(o), str::to_string);
                self.render(o, &parent, base)
            })
            .collect();
        Page {
            didl: didl(&rendered),
            returned: slice.len(),
            total,
        }
    }

    fn property(&self, object: Object, name: &str) -> Option<String> {
        let class = Self::class_of(object);
        match name {
            "upnp:class" => Some(class.to_string()),
            "@id" => Some(self.id_of(object)),
            "@parentID" => Some(self.home_of(object)),
            _ => match object {
                Object::Track(i) => {
                    let t = &self.tracks[i];
                    match name {
                        "dc:title" => Some(t.title.clone()),
                        "dc:creator" | "upnp:artist" => Some(t.artist.clone()),
                        "upnp:album" => Some(t.album.clone()),
                        "upnp:albumArtist" => Some(album_artist(t)),
                        "upnp:genre" => t.genre.clone(),
                        "upnp:originalTrackNumber" => t.track_number.map(|n| n.to_string()),
                        "dc:date" => t.year.map(|y| format!("{y:04}-01-01")),
                        _ => None,
                    }
                }
                Object::Album(i) => {
                    let a = &self.albums[i];
                    match name {
                        "dc:title" | "upnp:album" => Some(a.title.clone()),
                        "dc:creator" | "upnp:artist" | "upnp:albumArtist" => Some(a.artist.clone()),
                        "dc:date" => a.year.map(|y| format!("{y:04}-01-01")),
                        _ => None,
                    }
                }
                Object::Artist(i) => match name {
                    "dc:title" | "upnp:artist" => Some(self.artists[i].name.clone()),
                    _ => None,
                },
                Object::Playlist(i) => match name {
                    "dc:title" => Some(self.playlists[i].name.clone()),
                    _ => None,
                },
                Object::Root | Object::Folder(_) => match name {
                    "dc:title" => Some(self.folder_title(object).to_string()),
                    _ => None,
                },
            },
        }
    }

    fn folder_title(&self, object: Object) -> &'static str {
        match object {
            Object::Folder(ARTISTS) => "Artists",
            Object::Folder(ALBUMS) => "Albums",
            Object::Folder(SONGS) => "All songs",
            Object::Folder(_) => "Playlists",
            _ => "Music",
        }
    }

    fn render(&self, object: Object, parent: &str, base: &str) -> String {
        let id = escape(&self.id_of(object));
        let parent = escape(parent);
        let class = Self::class_of(object);
        let child_count = self.children(object).len();
        let container = |title: &str, extra: String| {
            format!(
                r#"<container id="{id}" parentID="{parent}" childCount="{child_count}" restricted="1" searchable="1"><dc:title>{}</dc:title><upnp:class>{class}</upnp:class>{extra}</container>"#,
                escape(title)
            )
        };
        match object {
            Object::Root | Object::Folder(_) => container(self.folder_title(object), String::new()),
            Object::Artist(i) => container(&self.artists[i].name, String::new()),
            Object::Playlist(i) => container(&self.playlists[i].name, String::new()),
            Object::Album(i) => {
                let album = &self.albums[i];
                let mut extra = format!(
                    "<upnp:artist>{0}</upnp:artist><dc:creator>{0}</dc:creator>",
                    escape(&album.artist)
                );
                if let Some(year) = album.year {
                    extra.push_str(&format!("<dc:date>{year:04}-01-01</dc:date>"));
                }
                if let Some(&first) = album
                    .tracks
                    .iter()
                    .find(|&&t| self.tracks[t].album_art_id.is_some())
                {
                    extra.push_str(&art_uri(base, &self.tracks[first]));
                }
                container(&album.title, extra)
            }
            Object::Track(i) => render_track(&self.tracks[i], &id, &parent, base),
        }
    }
}

/// One `Browse` or `Search` reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub didl: String,
    pub returned: usize,
    pub total: usize,
}

fn didl(objects: &[String]) -> String {
    format!(
        r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/" xmlns:dlna="urn:schemas-dlna-org:metadata-1-0/">{}</DIDL-Lite>"#,
        objects.concat()
    )
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Characters XML 1.0 can't carry at all.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// The MIME type a file of this format is served as.
pub fn mime_for(format: &str) -> &'static str {
    match format.to_ascii_lowercase().as_str() {
        "mp3" | "mp2" | "mp1" => "audio/mpeg",
        "flac" => "audio/flac",
        "m4a" | "m4b" | "m4p" | "mp4" | "alac" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" | "wave" => "audio/wav",
        "aiff" => "audio/aiff",
        "mka" | "mkv" => "audio/x-matroska",
        "weba" => "audio/webm",
        "caf" => "audio/x-caf",
        _ => UNKNOWN_MIME,
    }
}

/// The DLNA fourth field: seekable by byte range, streamed, a profile name
/// where DLNA defines one for the format.
pub fn dlna_features(format: &str) -> String {
    let profile = match format.to_ascii_lowercase().as_str() {
        "mp3" => "DLNA.ORG_PN=MP3;",
        "m4a" | "aac" | "mp4" => "DLNA.ORG_PN=AAC_ISO_320;",
        _ => "",
    };
    format!("{profile}DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000")
}

fn duration_text(seconds: f64) -> String {
    let millis = (seconds * 1000.0).round() as u64;
    format!(
        "{}:{:02}:{:02}.{:03}",
        millis / 3_600_000,
        millis / 60_000 % 60,
        millis / 1000 % 60,
        millis % 1000
    )
}

/// The file extension a shared track's URL ends in.
///
/// Android stores a document's MIME subtype as its format ("MPEG",
/// "X-FLAC", "X-M4A"), so those are mapped to the extension they stand for;
/// failing that, the extension of the file's own name is used. Serving them
/// as `application/octet-stream` made players, Wave's own DLNA client
/// included, skip most of a phone's library.
pub fn extension_of(track: &Track) -> String {
    let format = track.format.trim().to_ascii_lowercase();
    let known = match format.as_str() {
        "mpeg" | "mpga" | "x-mpeg" | "mp3" => "mp3",
        "x-flac" | "flac" => "flac",
        "mp4" | "x-m4a" | "m4a" | "x-mp4" | "mp4a-latm" => "m4a",
        "aac" | "x-aac" | "aacp" => "aac",
        "x-wav" | "wav" | "wave" | "vnd.wave" => "wav",
        "x-aiff" | "aiff" | "aif" => "aiff",
        "x-ogg" | "ogg" | "oga" | "vorbis" => "ogg",
        "x-matroska" | "mka" => "mka",
        "webm" | "weba" => "weba",
        "x-caf" | "caf" => "caf",
        other => other,
    };
    if mime_for(known) != UNKNOWN_MIME {
        return known.to_string();
    }
    // `content://` paths end in an encoded document id, `.../Song.mp3`.
    let decoded = percent_encoding::percent_decode_str(&track.path).decode_utf8_lossy();
    let from_name = decoded
        .rsplit(['/', ':'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| mime_for(ext) != UNKNOWN_MIME);
    from_name.unwrap_or_else(|| {
        if known.is_empty() {
            "bin".to_string()
        } else {
            known.to_string()
        }
    })
}

const UNKNOWN_MIME: &str = "application/octet-stream";

fn art_uri(base: &str, track: &Track) -> String {
    format!(
        r#"<upnp:albumArtURI dlna:profileID="JPEG_TN">{base}/art/{}.jpg</upnp:albumArtURI>"#,
        escape(&crate::remote::uri::percent_encode(&track.id))
    )
}

fn render_track(track: &Track, id: &str, parent: &str, base: &str) -> String {
    let mut out = format!(
        r#"<item id="{id}" parentID="{parent}" restricted="1"><dc:title>{}</dc:title><upnp:class>{CLASS_TRACK}</upnp:class>"#,
        escape(&track.title)
    );
    out.push_str(&format!(
        "<upnp:artist>{0}</upnp:artist><dc:creator>{0}</dc:creator><upnp:album>{1}</upnp:album>",
        escape(&track.artist),
        escape(&track.album)
    ));
    if let Some(artist) = track
        .album_artist
        .as_deref()
        .filter(|a| !a.trim().is_empty())
    {
        out.push_str(&format!(
            r#"<upnp:artist role="AlbumArtist">{}</upnp:artist>"#,
            escape(artist)
        ));
    }
    if let Some(genre) = track.genre.as_deref().filter(|g| !g.trim().is_empty()) {
        out.push_str(&format!("<upnp:genre>{}</upnp:genre>", escape(genre)));
    }
    if let Some(number) = track.track_number.filter(|n| *n > 0) {
        out.push_str(&format!(
            "<upnp:originalTrackNumber>{number}</upnp:originalTrackNumber>"
        ));
    }
    if let Some(year) = track.year.filter(|y| *y > 0) {
        out.push_str(&format!("<dc:date>{year:04}-01-01</dc:date>"));
    }
    if track.album_art_id.is_some() {
        out.push_str(&art_uri(base, track));
    }
    let extension = extension_of(track);
    let mut attributes = format!(
        r#"protocolInfo="http-get:*:{}:{}""#,
        mime_for(&extension),
        dlna_features(&extension)
    );
    if track.file_size > 0 {
        attributes.push_str(&format!(r#" size="{}""#, track.file_size));
    }
    if let Some(seconds) = track.duration_seconds.filter(|d| *d > 0.0) {
        attributes.push_str(&format!(r#" duration="{}""#, duration_text(seconds)));
    }
    if let Some(rate) = track.sample_rate.filter(|r| *r > 0) {
        attributes.push_str(&format!(r#" sampleFrequency="{rate}""#));
    }
    if let Some(channels) = track.channels.filter(|c| *c > 0) {
        attributes.push_str(&format!(r#" nrAudioChannels="{channels}""#));
    }
    out.push_str(&format!(
        "<res {attributes}>{base}/media/{}.{}</res></item>",
        escape(&crate::remote::uri::percent_encode(&track.id)),
        escape(&extension)
    ));
    out
}

// ── Search criteria ────────────────────────────────────────────────────────
//
// The grammar of ContentDirectory:1, section 2.5.5:
//
//   searchCrit  ::= searchExp | '*'
//   searchExp   ::= relExp | searchExp logOp searchExp | '(' searchExp ')'
//   relExp      ::= property binOp quotedVal | property existsOp boolVal
//   binOp       ::= relOp | stringOp
//   relOp       ::= '=' | '!=' | '<' | '<=' | '>' | '>='
//   stringOp    ::= 'contains' | 'doesNotContain' | 'derivedfrom'
//
// with 'and' binding tighter than 'or'.

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Open,
    Close,
    Word(String),
    Quoted(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Criteria {
    All,
    And(Box<Criteria>, Box<Criteria>),
    Or(Box<Criteria>, Box<Criteria>),
    Relation {
        property: String,
        operator: String,
        value: String,
    },
    Exists {
        property: String,
        present: bool,
    },
}

fn tokenize(text: &str) -> Result<Vec<Token>, ()> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' => {
                chars.next();
                tokens.push(Token::Open);
            }
            ')' => {
                chars.next();
                tokens.push(Token::Close);
            }
            '"' => {
                chars.next();
                let mut value = String::new();
                loop {
                    match chars.next().ok_or(())? {
                        '\\' => value.push(chars.next().ok_or(())?),
                        '"' => break,
                        c => value.push(c),
                    }
                }
                tokens.push(Token::Quoted(value));
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '(' || c == ')' || c == '"' {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                tokens.push(Token::Word(word));
            }
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    fn peek_word(&self, word: &str) -> bool {
        matches!(self.tokens.get(self.at), Some(Token::Word(w)) if w.eq_ignore_ascii_case(word))
    }

    fn or(&mut self) -> Result<Criteria, ()> {
        let mut left = self.and()?;
        while self.peek_word("or") {
            self.at += 1;
            left = Criteria::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Criteria, ()> {
        let mut left = self.primary()?;
        while self.peek_word("and") {
            self.at += 1;
            left = Criteria::And(Box::new(left), Box::new(self.primary()?));
        }
        Ok(left)
    }

    fn primary(&mut self) -> Result<Criteria, ()> {
        match self.next().ok_or(())? {
            Token::Open => {
                let inner = self.or()?;
                match self.next() {
                    Some(Token::Close) => Ok(inner),
                    _ => Err(()),
                }
            }
            Token::Word(property) => {
                let Some(Token::Word(operator)) = self.next() else {
                    return Err(());
                };
                if operator.eq_ignore_ascii_case("exists") {
                    let Some(Token::Word(value)) = self.next() else {
                        return Err(());
                    };
                    let present = match value.to_ascii_lowercase().as_str() {
                        "true" => true,
                        "false" => false,
                        _ => return Err(()),
                    };
                    return Ok(Criteria::Exists { property, present });
                }
                let known = [
                    "=",
                    "!=",
                    "<",
                    "<=",
                    ">",
                    ">=",
                    "contains",
                    "doesnotcontain",
                    "derivedfrom",
                ];
                if !known.contains(&operator.to_ascii_lowercase().as_str()) {
                    return Err(());
                }
                let Some(Token::Quoted(value)) = self.next() else {
                    return Err(());
                };
                Ok(Criteria::Relation {
                    property,
                    operator: operator.to_ascii_lowercase(),
                    value,
                })
            }
            _ => Err(()),
        }
    }
}

pub(crate) fn parse_criteria(text: &str) -> Result<Criteria, ()> {
    let text = text.trim();
    if text.is_empty() || text == "*" {
        return Ok(Criteria::All);
    }
    let mut parser = Parser {
        tokens: tokenize(text)?,
        at: 0,
    };
    let criteria = parser.or()?;
    if parser.at != parser.tokens.len() {
        return Err(());
    }
    Ok(criteria)
}

impl Criteria {
    fn matches(&self, property: &dyn Fn(&str) -> Option<String>) -> bool {
        match self {
            Criteria::All => true,
            Criteria::And(a, b) => a.matches(property) && b.matches(property),
            Criteria::Or(a, b) => a.matches(property) || b.matches(property),
            Criteria::Exists {
                property: name,
                present,
            } => property(name).is_some() == *present,
            Criteria::Relation {
                property: name,
                operator,
                value,
            } => {
                let Some(actual) = property(name) else {
                    return operator == "!=" || operator == "doesnotcontain";
                };
                let (a, v) = (actual.to_lowercase(), value.to_lowercase());
                let ordering = || match (a.parse::<f64>(), v.parse::<f64>()) {
                    (Ok(x), Ok(y)) => x.partial_cmp(&y),
                    _ => Some(a.cmp(&v)),
                };
                use std::cmp::Ordering::*;
                match operator.as_str() {
                    "=" => a == v,
                    "!=" => a != v,
                    "contains" => a.contains(&v),
                    "doesnotcontain" => !a.contains(&v),
                    "derivedfrom" => a == v || a.starts_with(&format!("{v}.")),
                    "<" => ordering() == Some(Less),
                    "<=" => matches!(ordering(), Some(Less | Equal)),
                    ">" => ordering() == Some(Greater),
                    ">=" => matches!(ordering(), Some(Greater | Equal)),
                    _ => false,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str, title: &str, artist: &str, album: &str, number: i32) -> Track {
        Track {
            id: id.into(),
            path: format!("/music/{id}.flac"),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            format: "flac".into(),
            track_number: Some(number),
            duration_seconds: Some(225.5),
            file_size: 1234,
            year: Some(2001),
            ..Track::default()
        }
    }

    fn snapshot() -> Snapshot {
        let mut feat = track("t3", "Harder & Better", "Daft Punk", "Discovery", 4);
        feat.album_art_id = Some("art1".into());
        Snapshot::build(
            7,
            vec![
                feat,
                track("t1", "One More Time", "Daft Punk", "Discovery", 1),
                track("t2", "Teardrop", "Massive Attack", "Mezzanine", 3),
            ],
            vec![PlaylistSource {
                id: "p1".into(),
                name: "Favs".into(),
                paths: vec!["/music/t2.flac".into(), "/elsewhere/gone.mp3".into()],
            }],
        )
    }

    const BASE: &str = "http://192.168.1.5:50800";

    #[test]
    fn the_root_holds_the_four_views() {
        let page = snapshot().browse(ROOT, true, 0, 0, BASE).unwrap();
        assert_eq!((page.returned, page.total), (4, 4));
        for title in ["Artists", "Albums", "All songs", "Playlists"] {
            assert!(page.didl.contains(&format!("<dc:title>{title}</dc:title>")));
        }
    }

    #[test]
    fn albums_hold_songs_in_track_order_with_media_urls() {
        let snap = snapshot();
        let albums = snap.browse(ALBUMS, true, 0, 0, BASE).unwrap();
        assert_eq!(albums.total, 2);
        let discovery = &snap.albums[0];
        assert_eq!(discovery.title, "Discovery");
        let page = snap.browse(&discovery.id, true, 0, 0, BASE).unwrap();
        assert_eq!(page.total, 2);
        let first = page.didl.find("One More Time").unwrap();
        let second = page.didl.find("Harder &amp; Better").unwrap();
        assert!(first < second, "track 1 before track 4");
        assert!(page.didl.contains(r#"size="1234" duration="0:03:45.500""#));
        assert!(page.didl.contains(&format!("{BASE}/media/t1.flac</res>")));
        assert!(page.didl.contains("http-get:*:audio/flac:DLNA.ORG_OP=01"));
        // The album container carries its cover from a track that has one.
        assert!(albums.didl.contains(&format!("{BASE}/art/t3.jpg")));
    }

    #[test]
    fn paging_follows_starting_index_and_requested_count() {
        let snap = snapshot();
        let page = snap.browse(SONGS, true, 1, 1, BASE).unwrap();
        assert_eq!((page.returned, page.total), (1, 3));
        let past = snap.browse(SONGS, true, 9, 5, BASE).unwrap();
        assert_eq!((past.returned, past.total), (0, 3));
    }

    #[test]
    fn metadata_of_a_track_names_its_album_as_parent() {
        let snap = snapshot();
        let page = snap.browse("track:t2", false, 0, 0, BASE).unwrap();
        let album =
            &snap.albums[snap.album_index[&snap.home_of(Object::Track(snap.track_index["t2"]))]];
        assert_eq!(album.title, "Mezzanine");
        assert!(page.didl.contains(&format!(r#"parentID="{}""#, album.id)));
        assert_eq!(
            snap.browse("nope", true, 0, 0, BASE).unwrap_err().code,
            NO_SUCH_OBJECT
        );
    }

    #[test]
    fn playlists_keep_only_shared_songs() {
        let snap = snapshot();
        let page = snap.browse("playlist:p1", true, 0, 0, BASE).unwrap();
        assert_eq!(page.total, 1);
        assert!(page.didl.contains("Teardrop"));
    }

    #[test]
    fn artists_hold_their_albums() {
        let snap = snapshot();
        let artists = snap.browse(ARTISTS, true, 0, 0, BASE).unwrap();
        assert_eq!(artists.total, 2);
        let daft = snap.artists.iter().find(|a| a.name == "Daft Punk").unwrap();
        let page = snap.browse(&daft.id, true, 0, 0, BASE).unwrap();
        assert_eq!(page.total, 1);
        assert!(page.didl.contains(CLASS_ALBUM));
    }

    #[test]
    fn search_follows_the_spec_grammar() {
        let snap = snapshot();
        let all_audio = snap
            .search(
                ROOT,
                r#"upnp:class derivedfrom "object.item.audioItem""#,
                0,
                0,
                BASE,
            )
            .unwrap();
        assert_eq!(
            all_audio.total, 3,
            "each song once, though it sits in several views"
        );

        let titled = snap
            .search(
                ROOT,
                r#"(upnp:class = "object.item.audioItem.musicTrack" and dc:title contains "TEAR") or upnp:artist = "nobody""#,
                0,
                0,
                BASE,
            )
            .unwrap();
        assert_eq!(titled.total, 1);
        assert!(titled.didl.contains("Teardrop"));

        let albums = snap
            .search(
                ROOT,
                r#"upnp:class derivedfrom "object.container.album""#,
                0,
                0,
                BASE,
            )
            .unwrap();
        assert_eq!(albums.total, 2);

        let escaped = parse_criteria(r#"dc:title = "say \"hi\"""#).unwrap();
        assert_eq!(
            escaped,
            Criteria::Relation {
                property: "dc:title".into(),
                operator: "=".into(),
                value: r#"say "hi""#.into()
            }
        );
        assert!(parse_criteria("*").is_ok());
        assert!(parse_criteria(r#"upnp:genre exists true"#).is_ok());
        assert_eq!(
            snap.search(ROOT, "dc:title ~ \"x\"", 0, 0, BASE)
                .unwrap_err()
                .code,
            BAD_SEARCH_CRITERIA
        );
        assert!(parse_criteria(r#"(dc:title = "x""#).is_err());
    }

    #[test]
    fn android_formats_are_served_as_audio() {
        let with = |format: &str, path: &str| {
            extension_of(&Track {
                format: format.into(),
                path: path.into(),
                ..Track::default()
            })
        };
        assert_eq!(with("MPEG", "content://x/a"), "mp3");
        assert_eq!(with("X-FLAC", "content://x/a"), "flac");
        assert_eq!(with("X-M4A", "content://x/a"), "m4a");
        assert_eq!(with("AAC", "content://x/a"), "aac");
        assert_eq!(mime_for("aac"), "audio/aac");
        // Nothing to go on in the format: the document's own name decides.
        assert_eq!(
            with(
                "AUDIO",
                "content://com.android.externalstorage.documents/tree/primary%3AMusic/document/primary%3AMusic%2FSong.MP3"
            ),
            "mp3"
        );
        assert_eq!(with("flac", "/music/a.flac"), "flac");
    }

    #[test]
    fn text_is_escaped_for_xml() {
        assert_eq!(
            escape(r#"A & B <"x"> 'y'"#),
            "A &amp; B &lt;&quot;x&quot;&gt; &apos;y&apos;"
        );
        assert_eq!(escape("bell\u{7}"), "bell");
        assert_eq!(duration_text(3725.25), "1:02:05.250");
    }
}
