// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Move bytes between SAF `content://` documents and local files.
//!
//! Android hands Wave document URIs rather than paths, and `std::fs` cannot
//! open one. `tauri-plugin-fs` can: it asks the ContentResolver for a file
//! descriptor and returns it as a real `std::fs::File`. That covers every copy
//! in and out the app needs, so none of this has to go through JNI.
//!
//! On desktop these functions still work for ordinary paths. A `content://`
//! URI simply fails to open, which is the right answer there.

use std::fs::File;
use std::io::{copy, Read, Write};
use std::path::Path;
use std::str::FromStr;

use tauri::AppHandle;
use tauri_plugin_fs::{FilePath, FsExt, OpenOptions};

fn open(app: &AppHandle, uri: &str, opts: OpenOptions) -> Result<File, String> {
    let file_path =
        FilePath::from_str(uri).map_err(|e| format!("Invalid source URI {uri}: {e}"))?;
    app.fs()
        .open(file_path, opts)
        .map_err(|e| format!("Failed to open {uri}: {e}"))
}

/// Copy a document into a local file.
pub fn copy_uri_to_file(app: &AppHandle, uri: &str, dest: &Path) -> Result<(), String> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    let mut reader = open(app, uri, opts)?;

    let mut writer =
        File::create(dest).map_err(|e| format!("Failed to create {}: {e}", dest.display()))?;
    copy(&mut reader, &mut writer).map_err(|e| format!("Failed to copy {uri}: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("Failed to flush {}: {e}", dest.display()))?;
    Ok(())
}

/// Copy a local file back over a document, replacing its contents.
///
/// Truncation matters here. Without it a shorter file leaves the tail of the
/// old one in place, which for an audio file means a valid header followed by
/// garbage. The plugin maps read + write + truncate to the SAF mode string
/// `"rwt"`, which is what a DocumentsProvider expects for a full rewrite.
pub fn copy_file_to_uri(app: &AppHandle, src: &Path, uri: &str) -> Result<(), String> {
    let mut reader =
        File::open(src).map_err(|e| format!("Failed to open {}: {e}", src.display()))?;

    let mut opts = OpenOptions::new();
    opts.read(true).write(true).truncate(true);
    let mut writer = open(app, uri, opts)?;

    copy(&mut reader, &mut writer).map_err(|e| format!("Failed to write back to {uri}: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("Failed to flush {uri}: {e}"))?;
    Ok(())
}

/// Read a document into memory, refusing anything over `max_bytes`.
///
/// Reads one byte past the ceiling so an oversized file is rejected on its
/// size rather than silently truncated to the limit.
pub fn read_uri_bytes(app: &AppHandle, uri: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    let reader = open(app, uri, opts)?;

    let mut buf = Vec::new();
    reader
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| format!("Failed to read {uri}: {e}"))?;

    if buf.len() as u64 > max_bytes {
        return Err(format!(
            "File is too large (over {max_bytes} bytes, from {uri})"
        ));
    }
    Ok(buf)
}

/// Report whether the persisted grant behind `uri` allows writing.
///
/// Opens read + write with truncate off, which leaves the document exactly as
/// it was, then closes it again. A folder added under the picker's read-only
/// fallback makes the resolver refuse the open, and that refusal is the signal.
pub fn can_write_uri(app: &AppHandle, uri: &str) -> bool {
    let mut opts = OpenOptions::new();
    opts.read(true).write(true);
    open(app, uri, opts).is_ok()
}
