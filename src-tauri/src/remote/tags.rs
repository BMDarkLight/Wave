// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Tags for backends that serve plain files with no metadata API (WebDAV).
//!
//! Most formats keep their tags at the front: ID3v2 for MP3, the metadata
//! blocks for FLAC, the comment header for Ogg. Reading the first
//! [`HEADER_BYTES`] of a file is enough for those, and a file whose tags are
//! elsewhere (an MP4 with `moov` at the end) falls back to its path.
//!
//! What was read is kept in a [`TagCache`] on disk, keyed by the file's
//! fingerprint, so a sync only reads headers of new or changed files.

use lofty::config::ParseOptions;
use lofty::file::{FileType, TaggedFileExt};
use lofty::prelude::{Accessor, ItemKey};
use lofty::probe::Probe;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;

/// Enough for ID3v2 and FLAC metadata with a modest embedded cover.
pub const HEADER_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HeaderTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
}

impl HeaderTags {
    fn is_empty(&self) -> bool {
        *self == HeaderTags::default()
    }
}

/// Tags from the start of a file. `None` when the bytes hold no tags we can
/// read, including a tag cut off by the end of the header.
pub fn read_header_tags(bytes: Vec<u8>, extension: &str) -> Option<HeaderTags> {
    let options = ParseOptions::new()
        .read_properties(false)
        .read_cover_art(false);
    let probe = match FileType::from_ext(extension) {
        Some(file_type) => Probe::with_file_type(Cursor::new(bytes), file_type),
        None => Probe::new(Cursor::new(bytes)).guess_file_type().ok()?,
    };
    let file = probe.options(options).read().ok()?;

    let mut out = HeaderTags::default();
    let text = |value: Option<String>| {
        value
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    for tag in file.tags() {
        out.title = out
            .title
            .or_else(|| text(tag.title().map(|v| v.into_owned())));
        out.artist = out
            .artist
            .or_else(|| text(tag.artist().map(|v| v.into_owned())));
        out.album = out
            .album
            .or_else(|| text(tag.album().map(|v| v.into_owned())));
        out.genre = out
            .genre
            .or_else(|| text(tag.genre().map(|v| v.into_owned())));
        out.album_artist = out
            .album_artist
            .or_else(|| text(tag.get_string(ItemKey::AlbumArtist).map(str::to_string)));
        out.year = out.year.or_else(|| {
            tag.get_string(ItemKey::RecordingDate)
                .or_else(|| tag.get_string(ItemKey::Year))
                .and_then(|date| date.trim().get(..4)?.parse().ok())
        });
        out.track_number = out.track_number.or_else(|| tag.track().map(|n| n as i32));
        out.disc_number = out.disc_number.or_else(|| tag.disk().map(|n| n as i32));
    }
    (!out.is_empty()).then_some(out)
}

/// The front cover embedded in an audio file, with its MIME type, read from
/// any seekable source (a local file or an Android document).
pub fn embedded_picture(reader: impl std::io::Read + std::io::Seek) -> Option<(Vec<u8>, String)> {
    let file = Probe::new(std::io::BufReader::new(reader))
        .options(ParseOptions::new().read_properties(false))
        .guess_file_type()
        .ok()?
        .read()
        .ok()?;
    file.tags().iter().find_map(|tag| {
        let picture = tag
            .pictures()
            .iter()
            .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
            .or_else(|| tag.pictures().first())?;
        (!picture.data().is_empty()).then(|| {
            (
                picture.data().to_vec(),
                picture
                    .mime_type()
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| "image/jpeg".to_string()),
            )
        })
    })
}

/// The playing length of a complete audio file, from its headers.
pub fn measure_duration(path: &std::path::Path) -> Option<f64> {
    use lofty::file::AudioFile;
    let file = Probe::open(path)
        .ok()?
        .options(ParseOptions::new().read_tags(false).read_cover_art(false))
        .guess_file_type()
        .ok()?
        .read()
        .ok()?;
    let seconds = file.properties().duration().as_secs_f64();
    (seconds > 0.0).then_some(seconds)
}

#[derive(Default, Serialize, Deserialize)]
struct CacheEntry {
    fingerprint: String,
    /// `None` records that the header had nothing readable, so it is not
    /// fetched again until the file changes.
    tags: Option<HeaderTags>,
}

/// Header tags already read for one source, by file path.
pub struct TagCache {
    path: PathBuf,
    entries: HashMap<String, CacheEntry>,
    dirty: bool,
}

impl TagCache {
    /// The cache for `key`, empty when there is none yet or it can't be read.
    pub fn open(key: &str) -> Self {
        let path = cache_dir().join(format!("{}.json", crate::sources::cache::sanitize(key)));
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            entries,
            dirty: false,
        }
    }

    /// `Some(tags)` when this exact version of the file was read before.
    pub fn get(&self, file: &str, fingerprint: &str) -> Option<Option<HeaderTags>> {
        self.entries
            .get(file)
            .filter(|entry| entry.fingerprint == fingerprint)
            .map(|entry| entry.tags.clone())
    }

    pub fn insert(&mut self, file: &str, fingerprint: &str, tags: Option<HeaderTags>) {
        self.entries.insert(
            file.to_string(),
            CacheEntry {
                fingerprint: fingerprint.to_string(),
                tags,
            },
        );
        self.dirty = true;
    }

    /// Writes the cache if anything changed. A failure only costs re-reading
    /// headers on the next sync.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let Ok(json) = serde_json::to_vec(&self.entries) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&self.path, json).is_ok() {
            self.dirty = false;
        }
    }
}

fn cache_dir() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join("wave-test-remote-tags")
    } else {
        crate::app::paths::source_cache_dir().join("remote-tags")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ID3v2.3 tag with a title and artist, followed by nothing.
    fn id3(title: &str, artist: &str) -> Vec<u8> {
        let frame = |id: &[u8], text: &str| {
            let mut body = vec![0u8];
            body.extend_from_slice(text.as_bytes());
            let mut out = id.to_vec();
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&body);
            out
        };
        let mut frames = frame(b"TIT2", title);
        frames.extend(frame(b"TPE1", artist));
        frames.extend(frame(b"TRCK", "7/12"));
        let size = frames.len() as u32;
        let syncsafe = [
            ((size >> 21) & 0x7f) as u8,
            ((size >> 14) & 0x7f) as u8,
            ((size >> 7) & 0x7f) as u8,
            (size & 0x7f) as u8,
        ];
        let mut out = b"ID3\x03\x00\x00".to_vec();
        out.extend_from_slice(&syncsafe);
        out.extend(frames);
        // A frame sync so the file reads as MPEG audio.
        out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        out.extend(vec![0u8; 1024]);
        out
    }

    #[test]
    fn reads_id3_tags_from_a_header() {
        let tags = read_header_tags(id3("Song", "Band"), "mp3").unwrap();
        assert_eq!(tags.title.as_deref(), Some("Song"));
        assert_eq!(tags.artist.as_deref(), Some("Band"));
        assert_eq!(tags.track_number, Some(7));
    }

    #[test]
    fn finds_the_front_cover_inside_a_file() {
        let mut body = vec![0u8];
        body.extend_from_slice(b"image/jpeg\0");
        body.push(3); // front cover
        body.push(0); // empty description
        body.extend_from_slice(b"\xFF\xD8\xFF\xE0 cover bytes");
        let mut frame = b"APIC".to_vec();
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend(body);
        let size = frame.len() as u32;
        let mut file = b"ID3\x03\x00\x00".to_vec();
        file.extend_from_slice(&[
            ((size >> 21) & 0x7f) as u8,
            ((size >> 14) & 0x7f) as u8,
            ((size >> 7) & 0x7f) as u8,
            (size & 0x7f) as u8,
        ]);
        file.extend(frame);
        file.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        file.extend(vec![0u8; 1024]);
        let (data, mime) = embedded_picture(Cursor::new(file)).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(data.ends_with(b"cover bytes"));
        assert_eq!(embedded_picture(Cursor::new(vec![0u8; 2048])), None);
    }

    #[test]
    fn bytes_without_tags_read_as_none() {
        assert_eq!(read_header_tags(vec![0u8; 4096], "mp3"), None);
        assert_eq!(read_header_tags(b"not audio".to_vec(), "xyz"), None);
    }

    #[test]
    fn the_cache_matches_on_fingerprint_and_survives_a_reopen() {
        let key = format!("test-{}", uuid::Uuid::new_v4());
        let mut cache = TagCache::open(&key);
        let tags = HeaderTags {
            title: Some("T".into()),
            ..HeaderTags::default()
        };
        cache.insert("a/b.mp3", "etag-1", Some(tags.clone()));
        cache.insert("a/c.mp3", "etag-2", None);
        cache.save();

        let reopened = TagCache::open(&key);
        assert_eq!(reopened.get("a/b.mp3", "etag-1"), Some(Some(tags)));
        assert_eq!(reopened.get("a/c.mp3", "etag-2"), Some(None));
        assert_eq!(reopened.get("a/b.mp3", "etag-changed"), None);
        let _ = std::fs::remove_file(&reopened.path);
    }
}
