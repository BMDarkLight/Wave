// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! A small Subsonic-compatible API for third-party players.
//!
//! Only the read-only parts needed to browse and stream a library are
//! implemented: ping, artists/albums/songs, search, playlists, stream and
//! cover art. Both plain passwords and Subsonic's salted MD5 tokens work.

use super::catalog::{extension_of, mime_for};
use super::http::{self, Request, Server};
use crate::metadata::Track;
use crate::remote::subsonic::md5::md5_hex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::TcpStream;

const VERSION: &str = "1.16.1";

/// Credentials for Subsonic clients. Deliberately has no `Debug`.
#[derive(Clone)]
pub struct Auth {
    user: String,
    password: String,
}

impl Auth {
    pub fn new(user: String, password: String) -> Self {
        Self { user, password }
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    fn authorized(&self, params: &HashMap<String, String>) -> bool {
        let user = params.get("u").map(String::as_str).unwrap_or("");
        if !same(user.as_bytes(), self.user.as_bytes()) {
            return false;
        }
        if let Some(token) = params.get("t") {
            let salt = params.get("s").map(String::as_str).unwrap_or("");
            let expected = md5_hex(format!("{}{}", self.password, salt).as_bytes());
            return same(token.to_ascii_lowercase().as_bytes(), expected.as_bytes());
        }
        params
            .get("p")
            .is_some_and(|password| match password.strip_prefix("enc:") {
                Some(hex) => {
                    hex_decode(hex).is_some_and(|bytes| same(&bytes, self.password.as_bytes()))
                }
                None => same(password.as_bytes(), self.password.as_bytes()),
            })
    }
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .fold(0u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

/// Subsonic's optional `enc:` password form is uppercase hex.
fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = value.as_bytes();
    (0..bytes.len())
        .step_by(2)
        .map(|i| {
            let high = (bytes[i] as char).to_digit(16)?;
            let low = (bytes[i + 1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (raw_key, raw_value) = part.split_once('=').unwrap_or((part, ""));
            // Query values are form-encoded: a literal '+' means a space,
            // while an encoded plus arrives as `%2B` and stays a plus.
            let key = raw_key.replace('+', " ");
            let key = crate::remote::uri::percent_decode(&key).unwrap_or_else(|| key.clone());
            let value = raw_value.replace('+', " ");
            let value = crate::remote::uri::percent_decode(&value).unwrap_or_else(|| value.clone());
            (key, value)
        })
        .collect()
}

fn envelope(ok: bool, mut extra: Map<String, Value>) -> Value {
    let mut response = json!({
        "status": if ok { "ok" } else { "failed" },
        "version": VERSION,
        "type": "Wave",
        "openSubsonic": true,
    });
    if let Some(object) = response.as_object_mut() {
        object.append(&mut extra);
    }
    json!({ "subsonic-response": response })
}

fn ok(extra: Map<String, Value>) -> Value {
    envelope(true, extra)
}

fn fail(code: i32, message: &str) -> Value {
    let mut extra = Map::new();
    extra.insert(
        "error".to_string(),
        json!({ "code": code, "message": message }),
    );
    envelope(false, extra)
}

fn json_response(stream: &mut TcpStream, body: &Value, head_only: bool) -> io::Result<()> {
    http::respond(
        stream,
        "200 OK",
        &[("Content-Type", "application/json".to_string())],
        serde_json::to_string(body).unwrap_or_default().as_bytes(),
        head_only,
    )
}

/// Stable, readable Subsonic ids derived from the same keys Wave groups by.
fn stable_id(prefix: &str, key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let number = u64::from_be_bytes(digest[..8].try_into().unwrap_or_default());
    format!("{prefix}{number:016x}")
}

fn artist_name(track: &Track) -> String {
    track
        .album_artist
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(track.artist.trim())
        .to_string()
}

fn artist_id(track: &Track) -> String {
    stable_id(
        "ar-",
        &format!("\u{1f}{}", artist_name(track).to_lowercase()),
    )
}

fn album_id(track: &Track) -> String {
    stable_id(
        "al-",
        &format!(
            "\u{1f}{}\u{1f}{}",
            artist_name(track).to_lowercase(),
            track.album.trim().to_lowercase()
        ),
    )
}

fn has_art(track: &Track) -> bool {
    track.cover_art_data_url.is_some() || track.album_art_id.is_some()
}

fn song(track: &Track) -> Value {
    let extension = extension_of(track);
    let mut value = json!({
        "id": track.id,
        "parent": album_id(track),
        "title": track.title,
        "album": track.album,
        "artist": track.artist,
        "albumArtist": artist_name(track),
        "isDir": false,
        "contentType": mime_for(&extension),
        "suffix": extension,
        "duration": track.duration_seconds.unwrap_or(0.0) as u64,
        "track": track.track_number,
        "disc": track.disc_number,
        "year": track.year,
        "genre": track.genre,
        "size": track.file_size.max(0),
        "path": track.path,
        "albumId": album_id(track),
        "artistId": artist_id(track),
        "type": "music",
    });
    if has_art(track) {
        if let Some(object) = value.as_object_mut() {
            object.insert("coverArt".to_string(), json!(track.id));
        }
    }
    value
}

#[derive(Clone)]
struct Album {
    id: String,
    name: String,
    artist: String,
    artist_id: String,
    tracks: Vec<usize>,
    /// When a song of this album was last added to the library.
    added: i64,
    /// The newest release year among this album's songs.
    released: i32,
}

struct Artist {
    id: String,
    name: String,
    albums: Vec<String>,
    tracks: Vec<usize>,
}

fn groups(tracks: &[Track]) -> (Vec<Artist>, Vec<Album>) {
    let mut artists: HashMap<String, Artist> = HashMap::new();
    let mut albums: HashMap<String, Album> = HashMap::new();
    for (index, track) in tracks.iter().enumerate() {
        let aname = artist_name(track);
        let aid = artist_id(track);
        let alid = album_id(track);
        artists
            .entry(aid.clone())
            .or_insert_with(|| Artist {
                id: aid.clone(),
                name: aname.clone(),
                albums: Vec::new(),
                tracks: Vec::new(),
            })
            .tracks
            .push(index);
        let album = albums.entry(alid.clone()).or_insert_with(|| Album {
            id: alid.clone(),
            name: track.album.clone(),
            artist: aname.clone(),
            artist_id: aid.clone(),
            tracks: Vec::new(),
            added: track.indexed_at,
            released: track.year.unwrap_or(0),
        });
        album.tracks.push(index);
        album.added = album.added.max(track.indexed_at);
        album.released = album.released.max(track.year.unwrap_or(0));
        if !album.tracks.iter().any(|&i| i != index) {
            artists
                .entry(aid)
                .and_modify(|a| a.albums.push(alid.clone()));
        }
    }
    let mut artists: Vec<_> = artists.into_values().collect();
    let mut albums: Vec<_> = albums.into_values().collect();
    artists.sort_by_key(|a| a.name.to_lowercase());
    albums.sort_by(|a, b| {
        a.artist
            .to_lowercase()
            .cmp(&b.artist.to_lowercase())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    (artists, albums)
}

fn cover_of<'a>(tracks: &'a [Track], indices: &[usize]) -> Option<&'a str> {
    indices
        .iter()
        .map(|&i| &tracks[i])
        .find(|track| has_art(track))
        .map(|track| track.id.as_str())
}

fn duration_of(tracks: &[Track], indices: &[usize]) -> u64 {
    indices
        .iter()
        .map(|&i| tracks[i].duration_seconds.unwrap_or(0.0) as u64)
        .sum()
}

fn artist_summary(artist: &Artist) -> Value {
    json!({
        "id": artist.id,
        "name": artist.name,
        "albumCount": artist.albums.len(),
        "isDir": true,
    })
}

fn album_summary(album: &Album, tracks: &[Track]) -> Value {
    let mut value = json!({
        "id": album.id,
        "parent": album.artist_id,
        "name": album.name,
        "title": album.name,
        "artist": album.artist,
        "artistId": album.artist_id,
        "songCount": album.tracks.len(),
        "duration": duration_of(tracks, &album.tracks),
        "isDir": true,
    });
    if let Some(cover) = cover_of(tracks, &album.tracks) {
        if let Some(object) = value.as_object_mut() {
            object.insert("coverArt".to_string(), json!(cover));
        }
    }
    value
}

fn artist_detail(artist: &Artist, albums: &[Album], tracks: &[Track]) -> Value {
    let children: Vec<Value> = albums
        .iter()
        .filter(|album| album.artist_id == artist.id)
        .map(|album| album_summary(album, tracks))
        .collect();
    json!({
        "id": artist.id,
        "name": artist.name,
        "albumCount": children.len(),
        "album": children,
    })
}

fn album_detail(album: &Album, tracks: &[Track]) -> Value {
    let songs: Vec<Value> = album.tracks.iter().map(|&i| song(&tracks[i])).collect();
    let mut value = album_summary(album, tracks);
    if let Some(object) = value.as_object_mut() {
        object.insert("song".to_string(), json!(songs));
    }
    value
}

fn artists_index(artists: &[Artist]) -> Value {
    let mut by_letter: HashMap<String, Vec<Value>> = HashMap::new();
    for artist in artists {
        let letter = artist
            .name
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase().to_string())
            .filter(|c| c.chars().all(|c| c.is_ascii_alphabetic()))
            .unwrap_or_else(|| "#".to_string());
        by_letter
            .entry(letter)
            .or_default()
            .push(artist_summary(artist));
    }
    let mut index: Vec<Value> = by_letter
        .into_iter()
        .map(|(name, artist)| json!({ "name": name, "artist": artist }))
        .collect();
    index.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({ "index": index })
}

/// Subsonic's `search2`/`search3` give each result category its own count.
fn category_limit(params: &HashMap<String, String>, name: &str) -> usize {
    params
        .get(name)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 500)
}

fn limit(params: &HashMap<String, String>, default: usize, max: usize) -> (usize, usize) {
    let size = params
        .get("size")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(1, max);
    let offset = params
        .get("offset")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    (size, offset)
}

fn playlists(server: &Server) -> Value {
    let Some(library) = &server.library else {
        return json!([]);
    };
    match library.lock().list_playlists(None) {
        Ok(list) => json!(list
            .into_iter()
            .filter(|p| p.peer_source_id.is_none())
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "songCount": p.track_count,
                    "owner": "wave",
                    "public": true,
                })
            })
            .collect::<Vec<_>>()),
        Err(e) => {
            tracing::debug!("Could not list playlists for a Subsonic client: {e}");
            json!([])
        }
    }
}

/// Answer one `/rest/` request.
pub(crate) fn handle(
    stream: &mut TcpStream,
    auth: &Auth,
    server: &Server,
    request: &Request,
    path: &str,
    query: &str,
    head_only: bool,
) -> io::Result<()> {
    let params = parse_query(query);
    if !auth.authorized(&params) {
        return json_response(stream, &fail(40, "Invalid username or password"), head_only);
    }
    let endpoint = path.trim_start_matches("/rest/").trim_end_matches(".view");
    let id = params.get("id").map(String::as_str).unwrap_or("");

    match endpoint {
        "ping" => return json_response(stream, &ok(Map::new()), head_only),
        "getLicense" => {
            let mut extra = Map::new();
            extra.insert(
                "license".to_string(),
                json!({ "valid": true, "email": "wave@localhost", "licenseExpires": "never" }),
            );
            return json_response(stream, &ok(extra), head_only);
        }
        "stream" | "download" => {
            if id.is_empty() {
                return json_response(stream, &fail(70, "Missing id"), head_only);
            }
            return http::media(stream, server, request, id, head_only);
        }
        "getCoverArt" => {
            if id.is_empty() {
                return json_response(stream, &fail(70, "Missing id"), head_only);
            }
            return http::art(stream, server, id, head_only);
        }
        _ => {}
    }

    let Ok(snapshot) = server.catalog.snapshot() else {
        return json_response(stream, &fail(0, "The library isn't available"), head_only);
    };
    let tracks = snapshot.tracks();
    let (artists, albums) = groups(tracks);

    let body = match endpoint {
        "getSong" => match tracks.iter().find(|track| track.id == id) {
            Some(track) => {
                let mut extra = Map::new();
                extra.insert("song".to_string(), song(track));
                ok(extra)
            }
            None => fail(70, "No such song"),
        },
        "getArtists" | "getIndexes" => {
            let mut extra = Map::new();
            extra.insert(
                if endpoint == "getArtists" {
                    "artists"
                } else {
                    "indexes"
                }
                .to_string(),
                artists_index(&artists),
            );
            ok(extra)
        }
        "getArtist" => match artists.iter().find(|artist| artist.id == id) {
            Some(artist) => {
                let mut extra = Map::new();
                extra.insert("artist".to_string(), artist_detail(artist, &albums, tracks));
                ok(extra)
            }
            None => fail(70, "No such artist"),
        },
        "getAlbum" => match albums.iter().find(|album| album.id == id) {
            Some(album) => {
                let mut extra = Map::new();
                extra.insert("album".to_string(), album_detail(album, tracks));
                ok(extra)
            }
            None => fail(70, "No such album"),
        },
        "getMusicDirectory" => {
            let children: Vec<Value> = if let Some(album) = albums.iter().find(|a| a.id == id) {
                album.tracks.iter().map(|&i| song(&tracks[i])).collect()
            } else if let Some(artist) = artists.iter().find(|a| a.id == id) {
                albums
                    .iter()
                    .filter(|album| album.artist_id == artist.id)
                    .map(|album| album_summary(album, tracks))
                    .collect()
            } else {
                artists.iter().map(artist_summary).collect()
            };
            let (name, parent) = if let Some(album) = albums.iter().find(|a| a.id == id) {
                (album.name.clone(), album.artist_id.clone())
            } else if let Some(artist) = artists.iter().find(|a| a.id == id) {
                (artist.name.clone(), String::new())
            } else {
                ("Artists".to_string(), String::new())
            };
            let mut extra = Map::new();
            extra.insert(
                "directory".to_string(),
                json!({
                    "id": if id.is_empty() { "root" } else { id },
                    "parent": parent,
                    "name": name,
                    "child": children,
                }),
            );
            ok(extra)
        }
        "getAlbumList" | "getAlbumList2" => {
            let mut sorted = albums.clone();
            match params.get("type").map(String::as_str).unwrap_or("") {
                "alphabetical_by_album" | "alphabetical" => {
                    sorted.sort_by_key(|a| a.name.to_lowercase())
                }
                "recent" => sorted.sort_by(|a, b| {
                    b.added
                        .cmp(&a.added)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                }),
                "newest" => sorted.sort_by(|a, b| {
                    b.released
                        .cmp(&a.released)
                        .then_with(|| b.added.cmp(&a.added))
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                }),
                "random" => {
                    let seed = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .subsec_nanos() as usize;
                    sorted.sort_by_key(|album| stable_id("", &format!("{}{seed}", album.id)));
                }
                _ => {}
            }
            let (size, offset) = limit(&params, 50, 500);
            let page: Vec<Value> = sorted
                .into_iter()
                .skip(offset)
                .take(size)
                .map(|album| album_summary(&album, tracks))
                .collect();
            let mut extra = Map::new();
            let key = if endpoint == "getAlbumList2" {
                "albumList2"
            } else {
                "albumList"
            };
            extra.insert(key.to_string(), json!({ "album": page }));
            ok(extra)
        }
        "search" | "search2" | "search3" => {
            let query = params
                .get("query")
                .or_else(|| params.get("q"))
                .map(|q| q.trim().to_lowercase())
                .unwrap_or_default();
            let matches: Vec<usize> = tracks
                .iter()
                .enumerate()
                .filter(|(_, track)| {
                    query.is_empty()
                        || [&track.title, &track.artist, &track.album]
                            .iter()
                            .any(|field| field.to_lowercase().contains(&query))
                })
                .map(|(index, _)| index)
                .collect();
            let matched_albums: Vec<&Album> = albums
                .iter()
                .filter(|album| matches.iter().any(|&i| album.tracks.contains(&i)))
                .collect();
            let matched_artists: Vec<&Artist> = artists
                .iter()
                .filter(|artist| matches.iter().any(|&i| artist.tracks.contains(&i)))
                .collect();
            let mut extra = Map::new();
            if endpoint == "search" {
                let (size, offset) = limit(&params, 100, 500);
                // Subsonic 1.0 used a flat `match` list of songs.
                let found: Vec<Value> = matches
                    .into_iter()
                    .skip(offset)
                    .take(size)
                    .map(|i| song(&tracks[i]))
                    .collect();
                extra.insert("searchResult".to_string(), json!({ "match": found }));
            } else {
                let (_, offset) = limit(&params, 100, 500);
                let key = if endpoint == "search2" {
                    "searchResult2"
                } else {
                    "searchResult3"
                };
                extra.insert(
                    key.to_string(),
                    json!({
                        "artist": matched_artists.into_iter().skip(offset).take(category_limit(&params, "artistCount")).map(artist_summary).collect::<Vec<_>>(),
                        "album": matched_albums.into_iter().skip(offset).take(category_limit(&params, "albumCount")).map(|a| album_summary(a, tracks)).collect::<Vec<_>>(),
                        "song": matches.into_iter().skip(offset).take(category_limit(&params, "songCount")).map(|i| song(&tracks[i])).collect::<Vec<_>>(),
                    }),
                );
            }
            ok(extra)
        }
        "getPlaylists" => {
            let mut extra = Map::new();
            extra.insert(
                "playlists".to_string(),
                json!({ "playlist": playlists(server) }),
            );
            ok(extra)
        }
        "getPlaylist" => {
            let Some(library) = &server.library else {
                return json_response(stream, &fail(0, "Playlists aren't available"), head_only);
            };
            let playlist = library
                .lock()
                .list_playlists(None)
                .ok()
                .and_then(|list| list.into_iter().find(|playlist| playlist.id == id));
            let Some(playlist) = playlist.filter(|p| p.peer_source_id.is_none()) else {
                return json_response(stream, &fail(70, "No such playlist"), head_only);
            };

            // `library_tracks` includes paired-device rows that sharing never
            // publishes. Only return songs present in the same snapshot used
            // by browse/search/stream, so a playlist can't leak remote tracks
            // or ids that can't be played.
            let shareable: HashSet<&str> = tracks.iter().map(|track| track.id.as_str()).collect();
            let found: Vec<Track> = library
                .lock()
                .get_playlist_tracks(id)
                .unwrap_or_default()
                .into_iter()
                .filter(|track| shareable.contains(track.id.as_str()))
                .collect();
            let mut extra = Map::new();
            extra.insert(
                "playlist".to_string(),
                json!({
                    "id": id,
                    "name": playlist.name,
                    "songCount": found.len(),
                    "entry": found.iter().map(song).collect::<Vec<_>>(),
                }),
            );
            ok(extra)
        }
        _ => fail(0, "Unsupported endpoint"),
    };
    json_response(stream, &body, head_only)
}

#[cfg(test)]
mod tests {
    use super::{album_id, artist_id, fail, hex_decode, ok, parse_query, stable_id, Auth};

    #[test]
    fn query_parameters_are_decoded() {
        let params = parse_query("u=wave&p=pass%20word&v=1.16.1&x=a+b&y=a%2Bb");
        assert_eq!(params.get("u").map(String::as_str), Some("wave"));
        assert_eq!(params.get("p").map(String::as_str), Some("pass word"));
        assert_eq!(params.get("x").map(String::as_str), Some("a b"));
        assert_eq!(params.get("y").map(String::as_str), Some("a+b"));
    }

    #[test]
    fn response_has_the_subsonic_envelope() {
        let body = ok(serde_json::Map::new());
        assert_eq!(body["subsonic-response"]["status"], "ok");
        assert_eq!(body["subsonic-response"]["version"], "1.16.1");
        let failed = fail(40, "nope");
        assert_eq!(failed["subsonic-response"]["status"], "failed");
        assert_eq!(failed["subsonic-response"]["error"]["code"], 40);
    }

    #[test]
    fn authenticates_plain_salted_and_hex_passwords() {
        let auth = Auth::new("wave".into(), "s3cret".into());
        assert!(auth.authorized(&parse_query("u=wave&p=s3cret")));
        assert!(!auth.authorized(&parse_query("u=wave&p=wrong")));
        assert!(!auth.authorized(&parse_query("u=other&p=s3cret")));

        let salt = "abc123";
        let token = crate::remote::subsonic::md5::md5_hex(format!("s3cret{salt}").as_bytes());
        assert!(auth.authorized(&parse_query(&format!("u=wave&s={salt}&t={token}"))));

        // "s3cret" in hex, the optional `enc:` form some clients use.
        assert!(auth.authorized(&parse_query("u=wave&p=enc:733363726574")));
        assert_eq!(hex_decode("733363726574").unwrap(), b"s3cret");
        assert!(hex_decode("abc").is_none());
    }

    #[test]
    fn ids_are_stable_and_grouped() {
        assert_eq!(stable_id("al-", "a"), stable_id("al-", "a"));
        assert_ne!(stable_id("al-", "a"), stable_id("al-", "b"));
        assert!(artist_id(&crate::metadata::Track {
            id: "t".into(),
            path: "p".into(),
            name: String::new(),
            title: String::new(),
            artist: "Artist".into(),
            album: "Album".into(),
            album_artist: None,
            genre: None,
            year: None,
            track_number: None,
            disc_number: None,
            format: "mp3".into(),
            duration_seconds: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            lyrics: None,
            lyrics_source: None,
            has_synced_lyrics: false,
            cover_art_data_url: None,
            cover_art_mime: None,
            cover_art_source: None,
            album_art_id: None,
            fingerprint_sha256: None,
            acoustid_fingerprint: None,
            musicbrainz_recording_id: None,
            file_size: 0,
            modified_at: 0,
            indexed_at: 0,
            is_saf_uri: false,
            source_provider: None,
            source_state: None,
            remote_source_id: None,
            cache_state: None,
            remote_missing: false,
        })
        .starts_with("ar-"));
        let mut track = crate::metadata::Track {
            id: "t".into(),
            path: "p".into(),
            name: String::new(),
            title: String::new(),
            artist: "Artist".into(),
            album: "Album".into(),
            album_artist: None,
            genre: None,
            year: None,
            track_number: None,
            disc_number: None,
            format: "mp3".into(),
            duration_seconds: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            lyrics: None,
            lyrics_source: None,
            has_synced_lyrics: false,
            cover_art_data_url: None,
            cover_art_mime: None,
            cover_art_source: None,
            album_art_id: None,
            fingerprint_sha256: None,
            acoustid_fingerprint: None,
            musicbrainz_recording_id: None,
            file_size: 0,
            modified_at: 0,
            indexed_at: 0,
            is_saf_uri: false,
            source_provider: None,
            source_state: None,
            remote_source_id: None,
            cache_state: None,
            remote_missing: false,
        };
        let first = album_id(&track);
        track.album = "Other".into();
        assert_ne!(first, album_id(&track));
    }
}
