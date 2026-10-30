// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! A fingerprint of a file's audio that leaves its tags out, so the same song
//! on two devices matches even after its tags were edited on one of them.
//!
//! Reading every byte of every song would take minutes on a phone, so only
//! three stretches of the audio are hashed (start, middle and end), together
//! with the audio's length. Two different recordings agreeing on all of that
//! does not happen in practice.

use std::io::{Read, Seek, SeekFrom};

/// Bytes hashed at each of the three points.
const SAMPLE: u64 = 64 * 1024;

/// Where the audio sits in a file: from `start` up to, not including, `end`.
/// Formats Wave does not know are hashed whole.
pub fn audio_payload<R: Read + Seek>(
    reader: &mut R,
    len: u64,
    extension: &str,
) -> Option<(u64, u64)> {
    match extension.to_ascii_lowercase().as_str() {
        "flac" => flac_payload(reader, len),
        "m4a" | "m4b" | "m4p" | "mp4" | "aac" | "alac" => mp4_payload(reader, len),
        "mp3" | "mp2" | "mp1" => {
            let (start, end) = id3_payload(reader, len);
            Some((skip_info_frame(reader, start, end), end))
        }
        _ => Some(id3_payload(reader, len)),
    }
}

/// Past an MP3's Xing, Info or VBRI frame. It holds the encoder's notes and
/// a seek table, not audio, and a tag editor may rewrite it.
fn skip_info_frame<R: Read + Seek>(reader: &mut R, start: u64, end: u64) -> u64 {
    let mut frame = [0u8; 200];
    if end < start + frame.len() as u64 || read_at(reader, start, &mut frame).is_none() {
        return start;
    }
    let Some(length) = mpeg_frame_length(&frame[..4]) else {
        return start;
    };
    let notes = frame
        .windows(4)
        .take(60)
        .any(|w| w == b"Xing" || w == b"Info" || w == b"VBRI");
    if notes && start + length < end {
        start + length
    } else {
        start
    }
}

/// Length in bytes of an MPEG audio Layer III frame from its header.
fn mpeg_frame_length(header: &[u8]) -> Option<u64> {
    if header[0] != 0xff || header[1] & 0xe0 != 0xe0 {
        return None;
    }
    let version = (header[1] >> 3) & 0b11;
    let layer = (header[1] >> 1) & 0b11;
    if version == 0b01 || layer != 0b01 {
        return None;
    }
    let mpeg1 = version == 0b11;
    let bitrates: [u64; 15] = if mpeg1 {
        [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ]
    } else {
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]
    };
    let bitrate = *bitrates.get(usize::from(header[2] >> 4))?;
    let rates: [u64; 3] = match version {
        0b11 => [44100, 48000, 32000],
        0b10 => [22050, 24000, 16000],
        _ => [11025, 12000, 8000],
    };
    let rate = *rates.get(usize::from((header[2] >> 2) & 0b11))?;
    if bitrate == 0 {
        return None;
    }
    let padding = u64::from((header[2] >> 1) & 1);
    let coefficient = if mpeg1 { 144 } else { 72 };
    Some(coefficient * bitrate * 1000 / rate + padding)
}

fn read_at<R: Read + Seek>(reader: &mut R, at: u64, buf: &mut [u8]) -> Option<()> {
    reader.seek(SeekFrom::Start(at)).ok()?;
    reader.read_exact(buf).ok()
}

/// Everything between an ID3v2 tag at the front and ID3v1 or APEv2 tags at
/// the back. MP3, and anything else that might carry ID3 tags.
fn id3_payload<R: Read + Seek>(reader: &mut R, len: u64) -> (u64, u64) {
    let mut start = 0;
    let mut head = [0u8; 10];
    if len >= 10 && read_at(reader, 0, &mut head).is_some() && &head[..3] == b"ID3" {
        let size = head[6..10]
            .iter()
            .fold(0u64, |acc, &b| (acc << 7) | u64::from(b & 0x7f));
        // A footer repeats the header at the end of the tag.
        let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
        start = (10 + size + footer).min(len);
    }
    let mut end = len;
    let mut tag = [0u8; 3];
    if end >= start + 128 && read_at(reader, end - 128, &mut tag).is_some() && &tag == b"TAG" {
        end -= 128;
    }
    let mut ape = [0u8; 32];
    if end >= start + 32
        && read_at(reader, end - 32, &mut ape).is_some()
        && &ape[..8] == b"APETAGEX"
    {
        let size = u64::from(u32::from_le_bytes([ape[12], ape[13], ape[14], ape[15]]));
        // The size counts the footer; a header, when there is one, comes on top.
        let header = if ape[23] & 0x80 != 0 { 32 } else { 0 };
        end = end.saturating_sub(size + header).max(start);
    }
    (start, end)
}

/// The frames after the last metadata block.
fn flac_payload<R: Read + Seek>(reader: &mut R, len: u64) -> Option<(u64, u64)> {
    let mut magic = [0u8; 4];
    read_at(reader, 0, &mut magic)?;
    if &magic != b"fLaC" {
        return Some(id3_payload(reader, len));
    }
    let mut at = 4;
    loop {
        let mut header = [0u8; 4];
        read_at(reader, at, &mut header)?;
        let size = u64::from(u32::from_be_bytes([0, header[1], header[2], header[3]]));
        at += 4 + size;
        if header[0] & 0x80 != 0 || at >= len {
            return Some((at.min(len), len));
        }
    }
}

/// The body of the `mdat` box, where an MP4 file keeps its audio.
fn mp4_payload<R: Read + Seek>(reader: &mut R, len: u64) -> Option<(u64, u64)> {
    let mut at = 0;
    while at + 8 <= len {
        let mut header = [0u8; 8];
        read_at(reader, at, &mut header)?;
        let mut size = u64::from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]));
        let mut body = at + 8;
        if size == 1 {
            let mut large = [0u8; 8];
            read_at(reader, at + 8, &mut large)?;
            size = u64::from_be_bytes(large);
            body += 8;
        } else if size == 0 {
            size = len - at;
        }
        if &header[4..8] == b"mdat" {
            return Some((body.min(len), (at + size).min(len)));
        }
        if size < 8 {
            break;
        }
        at += size;
    }
    Some((0, len))
}

/// `a1:` and 32 hex digits, or `None` when the file can't be read or has no
/// audio in it.
pub fn audio_fingerprint<R: Read + Seek>(
    reader: &mut R,
    len: u64,
    extension: &str,
) -> Option<String> {
    use sha2::{Digest, Sha256};
    let (start, end) = audio_payload(reader, len, extension)?;
    if end <= start {
        return None;
    }
    let payload = end - start;
    let mut hasher = Sha256::new();
    hasher.update(payload.to_le_bytes());
    let spans: Vec<(u64, u64)> = if payload <= 3 * SAMPLE {
        vec![(start, payload)]
    } else {
        let middle = start + (payload - SAMPLE) / 2;
        vec![(start, SAMPLE), (middle, SAMPLE), (end - SAMPLE, SAMPLE)]
    };
    let mut buf = Vec::new();
    for (at, size) in spans {
        buf.resize(size as usize, 0);
        read_at(reader, at, &mut buf)?;
        hasher.update(&buf);
    }
    let digest = hasher.finalize();
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("a1:{hex}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fingerprint(bytes: &[u8], extension: &str) -> String {
        audio_fingerprint(&mut Cursor::new(bytes), bytes.len() as u64, extension).unwrap()
    }

    /// Audio-like bytes that differ with `seed`, long enough to sample.
    fn frames(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    fn id3v2(body_len: usize) -> Vec<u8> {
        let mut tag = b"ID3\x04\x00\x00".to_vec();
        // Syncsafe size: seven bits per byte.
        let n = body_len as u32;
        tag.extend([
            (n >> 21) as u8 & 0x7f,
            (n >> 14) as u8 & 0x7f,
            (n >> 7) as u8 & 0x7f,
            n as u8 & 0x7f,
        ]);
        tag.extend(vec![b'T'; body_len]);
        tag
    }

    fn id3v1(title: &str) -> Vec<u8> {
        let mut tag = b"TAG".to_vec();
        tag.extend(title.bytes());
        tag.resize(128, 0);
        tag
    }

    #[test]
    fn same_mp3_audio_with_different_id3_tags_matches() {
        let audio = frames(7, 400_000);
        let a = [id3v2(300), audio.clone()].concat();
        let b = [id3v2(9000), audio.clone(), id3v1("Edited title")].concat();
        assert_eq!(fingerprint(&a, "mp3"), fingerprint(&b, "mp3"));
        assert!(fingerprint(&a, "mp3").starts_with("a1:"));
    }

    /// An MPEG-1 Layer III frame at 128 kbps and 44.1 kHz: 417 bytes.
    fn info_frame(note: &[u8]) -> Vec<u8> {
        let mut frame = vec![0xff, 0xfb, 0x90, 0x00];
        frame.resize(36, 0);
        frame.extend(note);
        frame.resize(417, 0x55);
        frame
    }

    #[test]
    fn a_rewritten_xing_frame_does_not_count() {
        let audio = frames(8, 400_000);
        let a = [id3v2(100), info_frame(b"Xing"), audio.clone()].concat();
        let b = [id3v2(100), info_frame(b"Info"), audio.clone()].concat();
        assert_eq!(fingerprint(&a, "mp3"), fingerprint(&b, "mp3"));
    }

    #[test]
    fn different_audio_differs() {
        let a = [id3v2(100), frames(1, 400_000)].concat();
        let b = [id3v2(100), frames(2, 400_000)].concat();
        assert_ne!(fingerprint(&a, "mp3"), fingerprint(&b, "mp3"));
    }

    fn flac(comment_len: usize, picture_len: usize, audio: &[u8]) -> Vec<u8> {
        let mut file = b"fLaC".to_vec();
        let block = |kind: u8, last: bool, len: usize| {
            let mut b = vec![kind | if last { 0x80 } else { 0 }];
            b.extend([(len >> 16) as u8, (len >> 8) as u8, len as u8]);
            b.extend(vec![kind; len]);
            b
        };
        file.extend(block(0, false, 34));
        file.extend(block(4, false, comment_len));
        file.extend(block(6, true, picture_len));
        file.extend(audio);
        file
    }

    #[test]
    fn flac_metadata_blocks_are_skipped() {
        let audio = frames(3, 300_000);
        let a = flac(40, 1000, &audio);
        let b = flac(900, 50_000, &audio);
        assert_eq!(fingerprint(&a, "flac"), fingerprint(&b, "flac"));
        assert_eq!(
            audio_payload(&mut Cursor::new(&a), a.len() as u64, "flac"),
            Some(((a.len() - audio.len()) as u64, a.len() as u64))
        );
    }

    fn mp4(meta_len: usize, audio: &[u8]) -> Vec<u8> {
        let boxed = |kind: &[u8], body: &[u8]| {
            let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            b.extend(kind);
            b.extend(body);
            b
        };
        [
            boxed(b"ftyp", b"M4A \x00\x00\x00\x00"),
            boxed(b"moov", &vec![1u8; meta_len]),
            boxed(b"mdat", audio),
        ]
        .concat()
    }

    #[test]
    fn mp4_uses_mdat() {
        let audio = frames(9, 250_000);
        assert_eq!(
            fingerprint(&mp4(100, &audio), "m4a"),
            fingerprint(&mp4(7000, &audio), "m4a")
        );
        assert_ne!(
            fingerprint(&mp4(100, &audio), "m4a"),
            fingerprint(&mp4(100, &frames(10, 250_000)), "m4a")
        );
    }

    #[test]
    fn tiny_files_hash_whole_payload() {
        let a = [id3v2(10), frames(4, 5000)].concat();
        let mut b = a.clone();
        let last = b.len() - 1;
        b[last] ^= 1;
        assert_ne!(fingerprint(&a, "mp3"), fingerprint(&b, "mp3"));
    }

    #[test]
    fn unknown_extension_hashes_whole_file() {
        let bytes = frames(5, 1000);
        assert_eq!(
            audio_payload(&mut Cursor::new(&bytes), 1000, "xyz"),
            Some((0, 1000))
        );
        assert!(audio_fingerprint(&mut Cursor::new(Vec::new()), 0, "mp3").is_none());
    }
}

// ── Stored fingerprints ────────────────────────────────────────────────────

impl crate::library::Library {
    /// Local songs with no fingerprint yet, or whose file size changed since
    /// theirs was taken: `(id, path, format)`.
    pub fn tracks_needing_fingerprint(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>, String> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(
                "SELECT id, path, format FROM tracks
                 WHERE remote_source_id IS NULL
                   AND (source_state IS NULL OR source_state = 'downloaded')
                   AND (audio_fingerprint_key IS NULL
                        OR audio_fingerprint_key != CAST(file_size AS TEXT))
                 LIMIT ?1",
            )
            .map_err(|e| format!("Failed to list songs to fingerprint: {e}"))?;
        let rows = statement
            .query_map([limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| format!("Failed to list songs to fingerprint: {e}"))?;
        rows.collect::<Result<_, _>>()
            .map_err(|e| format!("Failed to read songs to fingerprint: {e}"))
    }

    /// Store a fingerprint, or `None` for a file that could not be read, so
    /// it is not tried again until it changes. `key` is the file size.
    pub fn set_audio_fingerprint(
        &self,
        track_id: &str,
        fingerprint: Option<&str>,
        key: &str,
    ) -> Result<(), String> {
        self.write_connection()
            .execute(
                "UPDATE tracks SET audio_fingerprint = ?2, audio_fingerprint_key = ?3 WHERE id = ?1",
                rusqlite::params![track_id, fingerprint, key],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to save a fingerprint: {e}"))
    }

    /// Every local song's fingerprint, mapped to the song's id.
    pub fn local_fingerprints(&self) -> Result<std::collections::HashMap<String, String>, String> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(
                "SELECT audio_fingerprint, id FROM tracks
                 WHERE remote_source_id IS NULL AND audio_fingerprint IS NOT NULL
                   AND (source_state IS NULL OR source_state = 'downloaded')",
            )
            .map_err(|e| format!("Failed to read fingerprints: {e}"))?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("Failed to read fingerprints: {e}"))?;
        rows.collect::<Result<_, _>>()
            .map_err(|e| format!("Failed to read fingerprints: {e}"))
    }
}

#[cfg(test)]
mod store_tests {
    use crate::library::Library;

    fn library() -> Library {
        let path = std::env::temp_dir().join(format!("wave-fp-{}.sqlite", uuid::Uuid::new_v4()));
        Library::new_with_path(&path).unwrap()
    }

    fn insert(library: &Library, id: &str, size: i64, remote: bool) {
        library
            .write_connection()
            .execute(
                "INSERT INTO tracks (id, path, name, title, artist, album, format, file_size,
                                     modified_at, indexed_at, remote_source_id, source_state)
                 VALUES (?1, ?2, ?1, ?1, 'A', 'B', 'mp3', ?3, 0, 0, ?4, ?5)",
                rusqlite::params![
                    id,
                    format!("/music/{id}.mp3"),
                    size,
                    remote.then_some("peer"),
                    remote.then_some("remote"),
                ],
            )
            .unwrap();
    }

    #[test]
    fn local_songs_without_a_fingerprint_are_listed() {
        let library = library();
        insert(&library, "local", 100, false);
        insert(&library, "remote", 100, true);
        let needing = library.tracks_needing_fingerprint(10).unwrap();
        assert_eq!(
            needing,
            vec![(
                "local".to_string(),
                "/music/local.mp3".to_string(),
                "mp3".to_string()
            )]
        );
    }

    #[test]
    fn a_stored_fingerprint_round_trips_and_is_not_asked_for_again() {
        let library = library();
        insert(&library, "song", 100, false);
        library
            .set_audio_fingerprint("song", Some("a1:abc"), "100")
            .unwrap();
        assert!(library.tracks_needing_fingerprint(10).unwrap().is_empty());
        assert_eq!(
            library
                .local_fingerprints()
                .unwrap()
                .get("a1:abc")
                .map(String::as_str),
            Some("song")
        );
    }

    #[test]
    fn a_changed_file_is_fingerprinted_again() {
        let library = library();
        insert(&library, "song", 100, false);
        library
            .set_audio_fingerprint("song", Some("a1:abc"), "100")
            .unwrap();
        library
            .write_connection()
            .execute("UPDATE tracks SET file_size = 200 WHERE id = 'song'", [])
            .unwrap();
        assert_eq!(library.tracks_needing_fingerprint(10).unwrap().len(), 1);
    }

    #[test]
    fn an_unreadable_file_is_not_retried() {
        let library = library();
        insert(&library, "song", 100, false);
        library.set_audio_fingerprint("song", None, "100").unwrap();
        assert!(library.tracks_needing_fingerprint(10).unwrap().is_empty());
        assert!(library.local_fingerprints().unwrap().is_empty());
    }
}

// ── Filling them in ────────────────────────────────────────────────────────

/// Songs fingerprinted per pass, and the pause between passes. Small passes
/// keep the library lock free for everything else.
const BATCH: usize = 50;
const PASS_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

/// Fingerprint local songs on a background thread, a few at a time, for as
/// long as Wave runs. New and changed files are picked up on later passes.
pub fn fill_in_background(app: tauri::AppHandle) {
    use tauri::Manager;
    let spawned = std::thread::Builder::new()
        .name("wave-fingerprints".into())
        .spawn(move || {
            let opener = crate::share::file_opener(Some(app.clone()));
            loop {
                std::thread::sleep(PASS_EVERY);
                let Some(library) = app.try_state::<crate::commands::LibraryState>() else {
                    continue;
                };
                let pending = match library.0.lock() {
                    Ok(lib) => lib.tracks_needing_fingerprint(BATCH).unwrap_or_default(),
                    Err(_) => continue,
                };
                for (id, path, format) in pending {
                    let taken = opener(&path).ok().and_then(|mut file| {
                        let len = file.metadata().ok()?.len();
                        Some((audio_fingerprint(&mut file, len, &format), len))
                    });
                    let (fingerprint, len) = match taken {
                        Some((fingerprint, len)) => (fingerprint, len.to_string()),
                        None => (None, String::new()),
                    };
                    if let Ok(lib) = library.0.lock() {
                        // The key is the size the library holds, so the row
                        // is only revisited once a rescan changes it.
                        let key = lib
                            .track_file_size(&id)
                            .map(|size| size.to_string())
                            .unwrap_or(len);
                        if let Err(e) = lib.set_audio_fingerprint(&id, fingerprint.as_deref(), &key)
                        {
                            tracing::debug!("{e}");
                        }
                    }
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("Couldn't start fingerprinting: {e}");
    }
}

impl crate::library::Library {
    fn track_file_size(&self, track_id: &str) -> Option<i64> {
        self.read_connection()
            .query_row(
                "SELECT file_size FROM tracks WHERE id = ?1",
                [track_id],
                |row| row.get(0),
            )
            .ok()
    }
}
