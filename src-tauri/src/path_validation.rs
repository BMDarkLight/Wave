use std::path::{Component, Path, PathBuf};

use crate::metadata::is_supported_audio_file;

/// Maximum bytes hashed when fingerprinting a track during library indexing.
pub const MAX_FILE_HASH_BYTES: u64 = 500 * 1024 * 1024;

/// Maximum playlist JSON import size (10 MiB).
pub const MAX_PLAYLIST_IMPORT_BYTES: u64 = 10 * 1024 * 1024;

/// Android Storage Access Framework / MediaStore content URI.
pub fn is_android_content_uri(path: &str) -> bool {
    path.starts_with("content://")
}

/// Validate that `path` points to a readable supported audio file.
///
/// Content URIs are accepted here so callers can materialize them into local
/// storage before playback/indexing. Plain filesystem paths must exist.
pub fn validate_audio_path(path: &str) -> Result<PathBuf, String> {
    if is_android_content_uri(path) || path.starts_with("file://") {
        return Ok(PathBuf::from(path));
    }
    let path_buf = PathBuf::from(path);
    if !path_buf.is_file() {
        return Err(format!("Audio file does not exist: {path}"));
    }
    if !is_supported_audio_file(&path_buf) {
        return Err(format!("Unsupported audio file extension: {path}"));
    }
    Ok(path_buf)
}

/// Reject paths that traverse outside the filesystem root or contain parent refs.
pub fn validate_safe_output_path(path: &str, expected_extension: &str) -> Result<PathBuf, String> {
    let path_buf = PathBuf::from(path);
    if path.is_empty() {
        return Err("Output path cannot be empty".to_string());
    }
    for component in path_buf.components() {
        if matches!(component, Component::ParentDir) {
            return Err("Output path cannot contain '..'".to_string());
        }
    }
    let ext = path_buf
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if ext != expected_extension {
        return Err(format!(
            "Output path must have .{expected_extension} extension, got .{ext}"
        ));
    }
    if let Some(parent) = path_buf.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(format!(
                "Parent directory does not exist: {}",
                parent.display()
            ));
        }
    }
    Ok(path_buf)
}

/// Validate playlist import file before reading into memory.
pub fn validate_playlist_import_path(path: &str) -> Result<(PathBuf, u64), String> {
    let path_buf = Path::new(path);
    if !path_buf.is_file() {
        return Err(format!("Playlist file not found: {path}"));
    }
    let size = std::fs::metadata(path_buf)
        .map_err(|e| format!("Failed to read playlist file metadata: {e}"))?
        .len();
    if size > MAX_PLAYLIST_IMPORT_BYTES {
        return Err(format!(
            "Playlist file too large ({size} bytes, max {MAX_PLAYLIST_IMPORT_BYTES})"
        ));
    }
    Ok((path_buf.to_path_buf(), size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        // uuid goes before `name` so any extension in `name` stays at the end
        // of the filename (PathBuf::extension() only looks at the final `.`).
        std::env::temp_dir().join(format!("wave-test-{}-{}", uuid::Uuid::new_v4(), name))
    }

    // ── is_android_content_uri ──────────────────────────────────────────────

    #[test]
    fn content_uri_prefix_is_detected() {
        assert!(is_android_content_uri("content://foo"));
    }

    #[test]
    fn empty_string_is_not_a_content_uri() {
        assert!(!is_android_content_uri(""));
    }

    #[test]
    fn content_scheme_without_slashes_is_not_a_content_uri() {
        assert!(!is_android_content_uri("content:"));
    }

    #[test]
    fn content_uri_check_is_case_sensitive() {
        assert!(!is_android_content_uri("CONTENT://foo"));
    }

    #[test]
    fn file_uri_is_not_a_content_uri() {
        assert!(!is_android_content_uri("file://x"));
    }

    // ── validate_audio_path ─────────────────────────────────────────────────

    #[test]
    fn validate_audio_path_short_circuits_for_content_uri_without_touching_disk() {
        let result = validate_audio_path("content://nonexistent/foo.mp3");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_audio_path_short_circuits_for_file_uri_without_touching_disk() {
        let result = validate_audio_path("file://nonexistent/foo.mp3");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_audio_path_rejects_nonexistent_path() {
        let path = temp_path("nonexistent.mp3");
        let result = validate_audio_path(path.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn validate_audio_path_rejects_directory() {
        let dir = temp_path("dir");
        std::fs::create_dir_all(&dir).expect("create test dir");
        let result = validate_audio_path(dir.to_str().unwrap());
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_audio_path_rejects_unsupported_extension() {
        let file = temp_path("unsupported.txt");
        std::fs::write(&file, b"not audio").expect("write test file");
        let result = validate_audio_path(file.to_str().unwrap());
        assert!(result.is_err());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn validate_audio_path_accepts_supported_extension() {
        let file = temp_path("supported.mp3");
        std::fs::write(&file, b"not really audio").expect("write test file");
        let result = validate_audio_path(file.to_str().unwrap());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), file);
        let _ = std::fs::remove_file(&file);
    }

    // ── validate_safe_output_path ───────────────────────────────────────────

    #[test]
    fn validate_safe_output_path_rejects_empty_path() {
        let result = validate_safe_output_path("", "m3u");
        assert!(result.is_err());
    }

    #[test]
    fn validate_safe_output_path_rejects_leading_parent_dir() {
        let result = validate_safe_output_path("../foo.m3u", "m3u");
        assert!(result.is_err());
    }

    #[test]
    fn validate_safe_output_path_rejects_mid_path_parent_dir() {
        let result = validate_safe_output_path("foo/../bar.m3u", "m3u");
        assert!(result.is_err());
    }

    #[test]
    fn validate_safe_output_path_rejects_extension_mismatch() {
        let result = validate_safe_output_path("out.txt", "m3u");
        assert!(result.is_err());
    }

    #[test]
    fn validate_safe_output_path_extension_match_is_case_insensitive() {
        let result = validate_safe_output_path("out.M3U", "m3u");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_safe_output_path_accepts_bare_filename() {
        // No directory component -> parent() is empty, so the exists() check
        // is skipped entirely regardless of cwd.
        let result = validate_safe_output_path("out.m3u", "m3u");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_safe_output_path_rejects_nonexistent_parent_directory() {
        let result = validate_safe_output_path("/definitely/does/not/exist/out.m3u", "m3u");
        assert!(result.is_err());
    }

    // ── validate_playlist_import_path ───────────────────────────────────────

    #[test]
    fn validate_playlist_import_path_rejects_nonexistent_path() {
        let path = temp_path("missing.m3u");
        let result = validate_playlist_import_path(path.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn validate_playlist_import_path_returns_size_for_small_file() {
        let file = temp_path("small.m3u");
        let contents = b"#EXTM3U\n/some/path.mp3\n";
        std::fs::write(&file, contents).expect("write test file");
        let result = validate_playlist_import_path(file.to_str().unwrap());
        assert!(result.is_ok());
        let (returned_path, size) = result.unwrap();
        assert_eq!(returned_path, file);
        assert_eq!(size, contents.len() as u64);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn validate_playlist_import_path_rejects_oversized_file() {
        let file = temp_path("huge.m3u");
        let f = std::fs::File::create(&file).expect("create test file");
        f.set_len(MAX_PLAYLIST_IMPORT_BYTES + 1).expect("set sparse len");
        drop(f);
        let result = validate_playlist_import_path(file.to_str().unwrap());
        assert!(result.is_err());
        let _ = std::fs::remove_file(&file);
    }
}
