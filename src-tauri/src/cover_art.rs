// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Shared album-art thumbs. One small JPEG on disk per unique image.
//!
//! Full-resolution covers are never persisted; call
//! [`extract_full_cover_data_url`] when the lyrics panel needs a large image.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose, Engine as _};
use sha2::{Digest, Sha256};
use tauri::AppHandle;

const COVER_ART_DIR: &str = "cover_art";
const THUMBS_DIR: &str = "thumbs";
const MEDIA_DIR: &str = "media";
/// One small file per cover image already saved, named by a hash of the
/// image as it came out of the audio file and holding the art id it became.
/// Every track of an album usually carries the same bytes, so this lets an
/// import skip decoding and resizing all but the first of them.
const SOURCES_DIR: &str = "sources";
const THUMB_MAX_EDGE: u32 = 96;
const THUMB_MAX_BYTES: usize = 12 * 1024;
const THUMB_JPEG_QUALITY: u8 = 70;
/// Higher-res art for OS media session / lock-screen notifications.
const MEDIA_MAX_EDGE: u32 = 512;
const MEDIA_MAX_BYTES: usize = 180 * 1024;
const MEDIA_JPEG_QUALITY: u8 = 85;
/// Artwork written back into the audio file itself.
const EMBED_MAX_EDGE: u32 = 1000;
const EMBED_MAX_BYTES: usize = 500 * 1024;
const EMBED_JPEG_QUALITY: u8 = 88;

/// Cover art extracted from an audio file or downloaded from the network.
pub struct ExtractedCoverArt {
    pub data: Vec<u8>,
}

/// Result of writing (or reusing) a shared album-art thumb.
#[derive(Debug, Clone)]
pub struct SavedAlbumArt {
    pub id: String,
    pub thumb_abs_path: PathBuf,
    pub relative_path: String,
    pub mime: String,
    pub byte_size: i64,
}

/// Resize to a tiny JPEG thumb, dedupe by content hash, write once under
/// `cover_art/thumbs/{hash}.jpg`. Also writes a 512px media-session JPEG under
/// `cover_art/media/{hash}.jpg` for lock-screen / notification artwork.
pub fn save_album_art_thumb(
    app: &AppHandle,
    cover_art: ExtractedCoverArt,
) -> Result<SavedAlbumArt, String> {
    save_album_art_thumb_in(&cover_root(app)?, &cover_art.data)
}

/// Same, for callers that already know the cover root and have no app handle:
/// the CLI and the playback daemon both open the library that way.
pub fn save_album_art_thumb_in(cover_root: &Path, data: &[u8]) -> Result<SavedAlbumArt, String> {
    let source_key = hex_sha256(data);
    if let Some(saved) = reuse_saved_source(cover_root, &source_key) {
        return Ok(saved);
    }

    // Both sizes come from one decode. The output matches what decoding
    // twice gave, so art ids stay the same as in earlier versions.
    let img = image::load_from_memory(data).map_err(|e| format!("Failed to load image: {e}"))?;
    let thumb = encode_sized_jpeg(&img, THUMB_MAX_EDGE, THUMB_MAX_BYTES, THUMB_JPEG_QUALITY)?;
    let media = encode_sized_jpeg(&img, MEDIA_MAX_EDGE, MEDIA_MAX_BYTES, MEDIA_JPEG_QUALITY)?;
    let id = hex_sha256(&thumb);
    let abs = cover_root.join(THUMBS_DIR).join(format!("{id}.jpg"));
    let media_abs = cover_root.join(MEDIA_DIR).join(format!("{id}.jpg"));

    if !abs.is_file() {
        write_new_file(&abs, &thumb)
            .map_err(|e| format!("Failed to write cover art thumb: {e}"))?;
    }
    if !media_abs.is_file() {
        write_new_file(&media_abs, &media)
            .map_err(|e| format!("Failed to write cover art media: {e}"))?;
    }
    // Only a speed-up: if it is not written, the next import decodes again.
    let _ = write_new_file(
        &cover_root.join(SOURCES_DIR).join(&source_key),
        id.as_bytes(),
    );

    Ok(saved_album_art(cover_root, id, thumb.len() as i64))
}

/// The art a cover with this source hash was saved as, while both its files
/// are still on disk.
fn reuse_saved_source(cover_root: &Path, source_key: &str) -> Option<SavedAlbumArt> {
    let id = fs::read_to_string(cover_root.join(SOURCES_DIR).join(source_key)).ok()?;
    let id = id.trim();
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let thumb = fs::metadata(cover_root.join(THUMBS_DIR).join(format!("{id}.jpg"))).ok()?;
    if !cover_root
        .join(MEDIA_DIR)
        .join(format!("{id}.jpg"))
        .is_file()
    {
        return None;
    }
    Some(saved_album_art(
        cover_root,
        id.to_string(),
        thumb.len() as i64,
    ))
}

fn saved_album_art(cover_root: &Path, id: String, byte_size: i64) -> SavedAlbumArt {
    SavedAlbumArt {
        thumb_abs_path: cover_root.join(THUMBS_DIR).join(format!("{id}.jpg")),
        relative_path: format!("{THUMBS_DIR}/{id}.jpg"),
        id,
        mime: "image/jpeg".into(),
        byte_size,
    }
}

/// Write through a temporary file and rename it into place. Imports save
/// covers from several threads, and two tracks of one album can finish
/// together; a reader must never see a half-written file.
fn write_new_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("{}-{n}.tmp", std::process::id()));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Re-encode picked artwork for embedding in an audio file by the metadata
/// editor. Bounded so the same image can go into every track of an album
/// without adding tens of megabytes to the batch.
pub fn make_embeddable_jpeg(data: &[u8]) -> Result<Vec<u8>, String> {
    make_sized_jpeg(data, EMBED_MAX_EDGE, EMBED_MAX_BYTES, EMBED_JPEG_QUALITY)
}

/// Show the user the image they just picked, before anything is written. Sized
/// for a dialog rather than for fidelity, so the data URL stays small.
pub fn preview_cover_data_url(data: &[u8]) -> Result<String, String> {
    let jpeg = make_sized_jpeg(data, MEDIA_MAX_EDGE, MEDIA_MAX_BYTES, MEDIA_JPEG_QUALITY)?;
    Ok(to_data_url("image/jpeg", &jpeg))
}

/// CLI / no-app path: encode a tiny thumb as a data URL (not persisted).
pub fn encode_cover_data_url(data: Vec<u8>, _mime: &str) -> Result<String, String> {
    let thumb = make_thumb_jpeg(&data)?;
    Ok(to_data_url("image/jpeg", &thumb))
}

/// Absolute path for a previously saved thumb by art id, if the file exists.
pub fn thumb_path_for_id(app: &AppHandle, art_id: &str) -> Option<PathBuf> {
    let path = thumb_abs_path(app, art_id).ok()?;
    path.is_file().then_some(path)
}

/// Absolute path for media-session (512px) art by id, if the file exists.
pub fn media_path_for_id(app: &AppHandle, art_id: &str) -> Option<PathBuf> {
    let path = media_abs_path(app, art_id).ok()?;
    path.is_file().then_some(path)
}

/// Ensure a 512px media-session JPEG exists for `art_id`, writing from `source`
/// bytes when missing. Returns the absolute path when available.
pub fn ensure_media_art(app: &AppHandle, art_id: &str, source: &[u8]) -> Option<PathBuf> {
    if let Some(existing) = media_path_for_id(app, art_id) {
        return Some(existing);
    }
    let media =
        make_sized_jpeg(source, MEDIA_MAX_EDGE, MEDIA_MAX_BYTES, MEDIA_JPEG_QUALITY).ok()?;
    let abs = media_abs_path(app, art_id).ok()?;
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).ok()?;
    }
    fs::write(&abs, &media).ok()?;
    abs.is_file().then_some(abs)
}

/// If `cover_url` points at a UI thumb (`…/thumbs/{id}.jpg`), prefer the
/// matching media-session file (`…/media/{id}.jpg`) when it exists.
pub fn prefer_media_artwork_url(cover_url: Option<&str>) -> Option<String> {
    let url = cover_url?.trim();
    if url.is_empty() {
        return None;
    }

    let path_str = url.strip_prefix("file://").unwrap_or(url);
    let path = Path::new(path_str);
    if let Some(parent) = path.parent() {
        if parent.file_name().and_then(|n| n.to_str()) == Some(THUMBS_DIR) {
            if let Some(name) = path.file_name() {
                let media = parent.parent().unwrap_or(parent).join(MEDIA_DIR).join(name);
                if media.is_file() {
                    return Some(media.to_string_lossy().into_owned());
                }
            }
        }
    }
    Some(url.to_string())
}

/// Resolve an absolute thumb path from a relative DB path or art id.
pub fn resolve_thumb_abs(app: &AppHandle, relative_or_id: &str) -> Option<PathBuf> {
    if relative_or_id.is_empty() {
        return None;
    }
    let app_dir = crate::app_paths::app_data_dir(app).ok()?;
    let cover_root = app_dir.join(COVER_ART_DIR);

    // Already a relative path, or a bare filename with a known image
    // extension. Either way it is resolved straight under the cover root.
    let candidate = if relative_or_id.contains('/')
        || relative_or_id.contains('\\')
        || relative_or_id.ends_with(".jpg")
        || relative_or_id.ends_with(".png")
        || relative_or_id.ends_with(".webp")
    {
        cover_root.join(relative_or_id)
    } else {
        cover_root
            .join(THUMBS_DIR)
            .join(format!("{relative_or_id}.jpg"))
    };

    if candidate.is_file() {
        return Some(candidate);
    }

    // Legacy per-track cache: cover_art/{track_id}.{ext}
    for ext in ["jpg", "png", "webp"] {
        let legacy = cover_root.join(format!("{relative_or_id}.{ext}"));
        if legacy.is_file() {
            return Some(legacy);
        }
    }
    None
}

/// Absolute path to a previously cached cover file (legacy track-id or art-id).
#[allow(dead_code)]
pub fn cached_cover_path(app: &AppHandle, track_or_art_id: &str) -> Option<PathBuf> {
    resolve_thumb_abs(app, track_or_art_id)
}

/// Build a data URL from raw image bytes without resizing (lyrics-panel full art).
pub fn to_data_url(mime: &str, data: &[u8]) -> String {
    let encoded = general_purpose::STANDARD.encode(data);
    format!("data:{mime};base64,{encoded}")
}

/// Decode a `data:` URL into raw bytes + mime.
pub fn decode_data_url(data_url: &str) -> Result<(Vec<u8>, String), String> {
    let (header, data) = data_url
        .split_once(',')
        .ok_or_else(|| "Invalid data URL".to_string())?;
    let mime = header
        .strip_prefix("data:")
        .and_then(|h| h.split(';').next())
        .unwrap_or("image/jpeg")
        .to_string();
    let bytes = general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("Failed to decode data URL: {e}"))?;
    Ok((bytes, mime))
}

/// Delete legacy per-track cover files under `cover_art/` (not under `thumbs/`).
pub fn cleanup_legacy_track_covers(app: &AppHandle) {
    let Ok(app_dir) = crate::app_paths::app_data_dir(app) else {
        return;
    };
    let cover_dir = app_dir.join(COVER_ART_DIR);
    let Ok(entries) = fs::read_dir(&cover_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Keep thumbs/ contents; remove flat `{uuid}.jpg` style files.
        if name.contains('.')
            && !name.starts_with('.')
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e, "jpg" | "jpeg" | "png" | "webp"))
        {
            let _ = fs::remove_file(&path);
        }
    }
}

fn cover_root(app: &AppHandle) -> Result<PathBuf, String> {
    let app_dir = crate::app_paths::app_data_dir(app)
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(app_dir.join(COVER_ART_DIR))
}

fn thumb_abs_path(app: &AppHandle, art_id: &str) -> Result<PathBuf, String> {
    Ok(cover_root(app)?
        .join(THUMBS_DIR)
        .join(format!("{art_id}.jpg")))
}

fn media_abs_path(app: &AppHandle, art_id: &str) -> Result<PathBuf, String> {
    Ok(cover_root(app)?
        .join(MEDIA_DIR)
        .join(format!("{art_id}.jpg")))
}

fn make_thumb_jpeg(data: &[u8]) -> Result<Vec<u8>, String> {
    make_sized_jpeg(data, THUMB_MAX_EDGE, THUMB_MAX_BYTES, THUMB_JPEG_QUALITY)
}

fn make_sized_jpeg(
    data: &[u8],
    max_edge: u32,
    max_bytes: usize,
    start_quality: u8,
) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(data).map_err(|e| format!("Failed to load image: {e}"))?;
    encode_sized_jpeg(&img, max_edge, max_bytes, start_quality)
}

fn encode_sized_jpeg(
    img: &image::DynamicImage,
    max_edge: u32,
    max_bytes: usize,
    start_quality: u8,
) -> Result<Vec<u8>, String> {
    let resized = img.thumbnail(max_edge, max_edge);
    let rgb = resized.to_rgb8();

    let mut quality = start_quality;
    loop {
        let mut buf = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        encoder
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("Failed to encode cover art: {e}"))?;

        if buf.len() <= max_bytes || quality <= 35 {
            return Ok(buf);
        }
        quality = quality.saturating_sub(15).max(35);
    }
}

fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Write a thumb from an existing data URL (migration helper). No AppHandle needed
/// when `cover_root` is already known.
pub fn migrate_data_url_to_thumb(
    cover_root: &Path,
    data_url: &str,
) -> Result<SavedAlbumArt, String> {
    let (bytes, _mime) = decode_data_url(data_url)?;
    save_album_art_thumb_in(cover_root, &bytes)
}

/// Encode arbitrary bytes as a data URL for one-shot full-cover responses.
pub fn full_cover_data_url(data: Vec<u8>, mime: &str) -> String {
    let mime = normalize_mime(mime);
    // Cap absurd payloads for IPC: downscale if > 1.5 MiB.
    const MAX_FULL: usize = 1536 * 1024;
    if data.len() <= MAX_FULL {
        return to_data_url(&mime, &data);
    }
    match downscale_for_ipc(data, &mime, MAX_FULL) {
        Ok((bytes, out_mime)) => to_data_url(&out_mime, &bytes),
        Err(_) => to_data_url(&mime, &[]),
    }
}

fn downscale_for_ipc(
    data: Vec<u8>,
    mime: &str,
    max_bytes: usize,
) -> Result<(Vec<u8>, String), String> {
    let img = image::load_from_memory(&data).map_err(|e| e.to_string())?;
    let scale = (max_bytes as f64 / data.len() as f64)
        .sqrt()
        .clamp(0.2, 0.9);
    let w = (img.width() as f64 * scale).round().max(128.0) as u32;
    let h = (img.height() as f64 * scale).round().max(128.0) as u32;
    let resized = img.resize(w, h, image::imageops::FilterType::Triangle);
    let mut buf = Vec::new();
    resized
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .map_err(|e| e.to_string())?;
    let _ = mime;
    Ok((buf, "image/jpeg".into()))
}

fn normalize_mime(mime: &str) -> String {
    match mime.to_ascii_lowercase().as_str() {
        "image/jpg" | "image/jpeg" => "image/jpeg".into(),
        "image/png" => "image/png".into(),
        "image/webp" => "image/webp".into(),
        other if other.starts_with("image/") => other.to_string(),
        _ => "image/jpeg".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!("wave-test-art-{}", uuid::Uuid::new_v4()))
    }

    fn sample_cover() -> Vec<u8> {
        let img = image::RgbImage::from_fn(640, 640, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8])
        });
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode(img.as_raw(), 640, 640, image::ExtendedColorType::Rgb8)
            .unwrap();
        jpeg
    }

    #[test]
    fn keeps_the_art_id_of_earlier_versions() {
        let root = test_root();
        let cover = sample_cover();
        let saved = save_album_art_thumb_in(&root, &cover).unwrap();
        let thumb =
            make_sized_jpeg(&cover, THUMB_MAX_EDGE, THUMB_MAX_BYTES, THUMB_JPEG_QUALITY).unwrap();
        assert_eq!(saved.id, hex_sha256(&thumb));
        assert!(saved.thumb_abs_path.is_file());
        assert!(root
            .join(MEDIA_DIR)
            .join(format!("{}.jpg", saved.id))
            .is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reuses_a_cover_it_has_seen_without_decoding_it() {
        let root = test_root();
        let cover = sample_cover();
        let first = save_album_art_thumb_in(&root, &cover).unwrap();
        let second = reuse_saved_source(&root, &hex_sha256(&cover)).unwrap();
        assert_eq!(second.id, first.id);
        assert_eq!(second.byte_size, first.byte_size);
        let again = save_album_art_thumb_in(&root, &cover).unwrap();
        assert_eq!(again.id, first.id);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn writes_a_deleted_thumb_again() {
        let root = test_root();
        let cover = sample_cover();
        let first = save_album_art_thumb_in(&root, &cover).unwrap();
        fs::remove_file(&first.thumb_abs_path).unwrap();
        let second = save_album_art_thumb_in(&root, &cover).unwrap();
        assert_eq!(second.id, first.id);
        assert!(second.thumb_abs_path.is_file());
        let _ = fs::remove_dir_all(root);
    }
}
