// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Import external audio sources into app-private storage.
//!
//! On Android, dialog pickers often return `content://` URIs that cannot be
//! opened with plain `std::fs`. Copying them into the app data directory gives
//! the library and player stable local paths.

use std::fs::{self, File};
use std::io::{copy, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sha2::{Digest, Sha256};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tauri::{AppHandle, Manager};
use tauri_plugin_fs::{FilePath, FsExt, OpenOptions};
use uuid::Uuid;

use crate::metadata::is_supported_audio_file;
use crate::path_validation::is_android_content_uri;

fn imports_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?
        .join("imports");
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create imports dir: {e}"))?;
    Ok(dir)
}

fn guess_extension(path: &str) -> String {
    let candidate = path
        .rsplit(['/', '\\', '?'])
        .next()
        .unwrap_or(path)
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_lowercase())
        .unwrap_or_default();
    // Android content URIs often encode the name as `...document/audio%3A1234`
    // without a real extension.
    if !candidate.is_empty()
        && candidate.len() <= 8
        && candidate.chars().all(|c| c.is_ascii_alphanumeric())
        && candidate != "bin"
        && !candidate.chars().all(|c| c.is_ascii_digit())
    {
        candidate
    } else {
        "bin".to_string()
    }
}

fn sniff_extension(path: &Path) -> String {
    let Ok(bytes) = fs::read(path) else {
        return "bin".to_string();
    };
    if bytes.starts_with(b"ID3") || matches!(bytes.first(), Some(0xFF)) {
        return "mp3".to_string();
    }
    if bytes.starts_with(b"fLaC") {
        return "flac".to_string();
    }
    if bytes.starts_with(b"OggS") {
        return "ogg".to_string();
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return "wav".to_string();
    }
    if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        return "m4a".to_string();
    }
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return "mka".to_string();
    }
    "bin".to_string()
}

fn is_playable_audio_file(path: &Path) -> bool {
    if is_supported_audio_file(path) {
        return true;
    }
    // Content-based probe for Android imports that lack a real extension.
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .is_ok()
}

fn copy_via_fs_plugin(app: &AppHandle, source: &str, dest: &Path) -> Result<(), String> {
    let file_path = FilePath::from_str(source)
        .map_err(|e| format!("Invalid audio source URI {source}: {e}"))?;
    let mut opts = OpenOptions::new();
    opts.read(true);
    let mut reader = app
        .fs()
        .open(file_path, opts)
        .map_err(|e| format!("Failed to open audio source {source}: {e}"))?;

    let mut writer =
        File::create(dest).map_err(|e| format!("Failed to create {}: {e}", dest.display()))?;
    copy(&mut reader, &mut writer)
        .map_err(|e| format!("Failed to copy audio source {source}: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("Failed to flush {}: {e}", dest.display()))?;
    Ok(())
}

fn content_hash(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize())[..16].to_string())
}

/// Resolve a source for playback.
///
/// On Android, `content://` URIs are returned as-is so ExoPlayer can stream
/// them without copying into app storage.
pub fn resolve_playback_source(app: &AppHandle, source: &str) -> Result<PathBuf, String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("Audio path is empty".to_string());
    }
    #[cfg(target_os = "android")]
    if is_android_content_uri(trimmed) {
        return Ok(PathBuf::from(trimmed));
    }
    materialize_audio_source(app, trimmed)
}

/// Resolve a source for library indexing (import / sync / add-to-playlist).
///
/// On Android, `content://` URIs are stored as-is (zero-copy). Desktop and
/// plain files still resolve to real filesystem paths.
pub fn resolve_library_source(app: &AppHandle, source: &str) -> Result<String, String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("Audio path is empty".to_string());
    }
    #[cfg(target_os = "android")]
    if is_android_content_uri(trimmed) {
        return Ok(trimmed.to_string());
    }
    Ok(materialize_audio_source(app, trimmed)?
        .to_string_lossy()
        .into_owned())
}

/// Resolve a picked path/URI into a local filesystem path suitable for Wave.
///
/// Regular files are returned unchanged. Content URIs (and unresolved `file:`
/// URLs) are copied into the app imports directory. Prefer
/// [`resolve_library_source`] for indexing and [`resolve_playback_source`] for
/// playback so Android stays zero-copy.
pub fn materialize_audio_source(app: &AppHandle, source: &str) -> Result<PathBuf, String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("Audio path is empty".to_string());
    }

    let as_path = PathBuf::from(trimmed);
    if as_path.is_file() {
        if !is_playable_audio_file(&as_path) {
            return Err(format!("Unsupported audio format: {trimmed}"));
        }
        return Ok(as_path);
    }

    if let Some(local) = trimmed.strip_prefix("file://") {
        let local_path = PathBuf::from(local);
        if local_path.is_file() {
            if !is_playable_audio_file(&local_path) {
                return Err(format!("Unsupported audio format: {trimmed}"));
            }
            return Ok(local_path);
        }
        return Err(format!("Audio file not found: {trimmed}"));
    }

    if !is_android_content_uri(trimmed) && !trimmed.starts_with("file:") {
        return Err(format!("Audio file does not exist: {trimmed}"));
    }

    let imports = imports_dir(app)?;
    let mut ext = guess_extension(trimmed);
    let staging = imports.join(format!("staging-{}.{}", Uuid::new_v4(), ext));
    copy_via_fs_plugin(app, trimmed, &staging)?;

    if !is_supported_audio_file(&staging) {
        let sniffed = sniff_extension(&staging);
        if sniffed != "bin" {
            ext = sniffed;
        }
    }

    if !is_playable_audio_file(&staging) {
        let _ = fs::remove_file(&staging);
        return Err(format!("Could not read audio from source: {trimmed}"));
    }

    if ext == "bin" {
        ext = sniff_extension(&staging);
        if ext == "bin" {
            ext = "mp3".to_string(); // last resort; file already probed as playable
        }
    }

    let hash = content_hash(&staging)?;
    let final_path = imports.join(format!("{hash}.{ext}"));
    if final_path.exists() {
        let _ = fs::remove_file(&staging);
        return Ok(final_path);
    }

    fs::rename(&staging, &final_path)
        .or_else(|_| {
            fs::copy(&staging, &final_path)
                .map(|_| ())
                .and_then(|_| fs::remove_file(&staging))
        })
        .map_err(|e| format!("Failed to finalize import {}: {e}", final_path.display()))?;

    Ok(final_path)
}
