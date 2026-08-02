//! Shared album-art thumbs — one small JPEG on disk per unique image.
//!
//! Full-resolution covers are never persisted; call
//! [`extract_full_cover_data_url`] when the lyrics panel needs a large image.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose, Engine as _};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

const COVER_ART_DIR: &str = "cover_art";
const THUMBS_DIR: &str = "thumbs";
const THUMB_MAX_EDGE: u32 = 96;
const THUMB_MAX_BYTES: usize = 12 * 1024;
const THUMB_JPEG_QUALITY: u8 = 70;

/// Cover art extracted from an audio file or downloaded from the network.
pub struct ExtractedCoverArt {
    pub data: Vec<u8>,
    pub mime: String,
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
/// `cover_art/thumbs/{hash}.jpg`. Returns art id + absolute path for the UI / OS.
pub fn save_album_art_thumb(
    app: &AppHandle,
    cover_art: ExtractedCoverArt,
) -> Result<SavedAlbumArt, String> {
    let thumb = make_thumb_jpeg(&cover_art.data)?;
    let id = hex_sha256(&thumb);
    let relative = format!("{THUMBS_DIR}/{id}.jpg");
    let abs = thumb_abs_path(app, &id)?;

    if !abs.is_file() {
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create cover art thumbs dir: {e}"))?;
        }
        fs::write(&abs, &thumb).map_err(|e| format!("Failed to write cover art thumb: {e}"))?;
    }

    Ok(SavedAlbumArt {
        id,
        thumb_abs_path: abs,
        relative_path: relative,
        mime: "image/jpeg".into(),
        byte_size: thumb.len() as i64,
    })
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

/// Resolve an absolute thumb path from a relative DB path or art id.
pub fn resolve_thumb_abs(app: &AppHandle, relative_or_id: &str) -> Option<PathBuf> {
    if relative_or_id.is_empty() {
        return None;
    }
    let app_dir = app.path().app_data_dir().ok()?;
    let cover_root = app_dir.join(COVER_ART_DIR);

    let candidate = if relative_or_id.contains('/') || relative_or_id.contains('\\') {
        cover_root.join(relative_or_id)
    } else if relative_or_id.ends_with(".jpg")
        || relative_or_id.ends_with(".png")
        || relative_or_id.ends_with(".webp")
    {
        cover_root.join(relative_or_id)
    } else {
        cover_root.join(THUMBS_DIR).join(format!("{relative_or_id}.jpg"))
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
    let Ok(app_dir) = app.path().app_data_dir() else {
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

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn thumb_abs_path(app: &AppHandle, art_id: &str) -> Result<PathBuf, String> {
    let app_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(app_dir
        .join(COVER_ART_DIR)
        .join(THUMBS_DIR)
        .join(format!("{art_id}.jpg")))
}

fn make_thumb_jpeg(data: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(data).map_err(|e| format!("Failed to load image: {e}"))?;
    let resized = img.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE);
    let rgb = resized.to_rgb8();

    let mut quality = THUMB_JPEG_QUALITY;
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
            .map_err(|e| format!("Failed to encode cover thumb: {e}"))?;

        if buf.len() <= THUMB_MAX_BYTES || quality <= 35 {
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
    let thumb = make_thumb_jpeg(&bytes)?;
    let id = hex_sha256(&thumb);
    let relative = format!("{THUMBS_DIR}/{id}.jpg");
    let abs = cover_root.join(THUMBS_DIR).join(format!("{id}.jpg"));
    if !abs.is_file() {
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create thumbs dir: {e}"))?;
        }
        fs::write(&abs, &thumb).map_err(|e| format!("Failed to write thumb: {e}"))?;
    }
    Ok(SavedAlbumArt {
        id,
        thumb_abs_path: abs,
        relative_path: relative,
        mime: "image/jpeg".into(),
        byte_size: thumb.len() as i64,
    })
}

/// Encode arbitrary bytes as a data URL for one-shot full-cover responses.
pub fn full_cover_data_url(data: Vec<u8>, mime: &str) -> String {
    let mime = normalize_mime(mime);
    // Cap absurd payloads for IPC — downscale if > 1.5 MiB.
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
    let scale = (max_bytes as f64 / data.len() as f64).sqrt().clamp(0.2, 0.9);
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
