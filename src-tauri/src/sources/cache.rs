//! On-disk cache for streamed audio.
//!
//! Playback is path-keyed all the way down (`AudioPlayer::play` ->
//! `SymphoniaSource::new` -> `File::open`), so remote audio becomes playable by
//! becoming a file. Fetching to a cache file and handing the existing engine a
//! path leaves seeking, gapless, DSP, normalization, and the Android ExoPlayer
//! route completely untouched.
//!
//! The layout here is deliberately compatible with progressive playback: a
//! future reader that waits at the write frontier can use these same files
//! without changing the eviction or naming rules.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use reqwest::blocking::Client;

use super::SourceError;
use crate::app::paths;

/// Refuse absurd payloads outright rather than filling the disk on a bad URL.
const MAX_FETCH_BYTES: u64 = 512 * 1024 * 1024;

/// Cache file for a provider's track. Ids are sanitised because Internet
/// Archive identifiers can contain path separators.
pub fn cached_path(provider: &str, id: &str, extension: &str) -> PathBuf {
    paths::source_cache_dir()
        .join(sanitize(provider))
        .join(format!("{}.{}", sanitize(id), sanitize(extension)))
}

/// Replace anything that could escape the cache directory or upset a
/// filesystem. Long ids are truncated with a hash suffix so they stay unique.
fn sanitize(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = if cleaned.is_empty() {
        "unnamed".to_string()
    } else {
        cleaned
    };
    if cleaned.len() <= 96 {
        return cleaned;
    }
    // Deterministic suffix keeps truncated ids from colliding.
    let mut hash: u64 = 1469598103934665603;
    for byte in raw.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{}_{hash:x}", &cleaned[..79])
}

/// Download `url` to `dest`, writing through a temp file so an interrupted
/// fetch can never leave a half-written file that Symphonia would fail to
/// probe. Returns the byte count.
pub fn fetch_to(client: &Client, url: &str, dest: &Path) -> Result<u64, SourceError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| SourceError::Network(format!("Cannot create cache directory: {e}")))?;
    }

    let mut response = client
        .get(url)
        .send()
        .map_err(|e| SourceError::Network(e.to_string()))?;

    // `error_for_status` renders as a wall of URL, which is useless in a toast.
    // Say what actually happened instead.
    let status = response.status();
    if !status.is_success() {
        return Err(SourceError::Network(describe_http_failure(status.as_u16())));
    }

    if let Some(len) = response.content_length() {
        if len > MAX_FETCH_BYTES {
            return Err(SourceError::Network(format!(
                "Refusing to download {len} bytes"
            )));
        }
    }

    let temp = dest.with_extension(format!(
        "{}.part",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("tmp")
    ));
    let written = {
        let mut file = fs::File::create(&temp)
            .map_err(|e| SourceError::Network(format!("Cannot open cache file: {e}")))?;
        // Cap while streaming too — a server can omit or lie about content-length.
        let mut limited = (&mut response).take(MAX_FETCH_BYTES);
        std::io::copy(&mut limited, &mut file)
            .map_err(|e| SourceError::Network(format!("Download failed: {e}")))?
    };

    if written == 0 {
        let _ = fs::remove_file(&temp);
        return Err(SourceError::NoAudio);
    }

    fs::rename(&temp, dest).map_err(|e| {
        let _ = fs::remove_file(&temp);
        SourceError::Network(format!("Cannot finalise cache file: {e}"))
    })?;
    Ok(written)
}

/// Plain-language reason a download failed, for display to the user.
///
/// Archive.org in particular serves 401/403 for access-restricted items, which
/// are indistinguishable from normal ones until you try to fetch them.
fn describe_http_failure(status: u16) -> String {
    match status {
        401 | 403 => "This recording isn't available for download".to_string(),
        404 => "This recording is no longer available at the source".to_string(),
        429 => "The source is rate limiting — try again in a moment".to_string(),
        500..=599 => format!("The source is having problems (HTTP {status})"),
        other => format!("The source refused the download (HTTP {other})"),
    }
}

/// Total bytes held in the cache directory.
pub fn cache_size_bytes() -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| match entry.file_type() {
                Ok(t) if t.is_dir() => walk(&entry.path()),
                Ok(_) => entry.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => 0,
            })
            .sum()
    }
    walk(&paths::source_cache_dir())
}

/// One cached row's eviction candidacy, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct EvictionCandidate {
    pub track_id: String,
    pub path: String,
    pub fetched_at: i64,
}

/// Choose which cached tracks to drop so the cache fits under `cap_bytes`.
///
/// `protected` holds paths that must survive regardless of age — the currently
/// playing track and everything in the live queue. Evicting one of those would
/// delete a file out from under an open decoder.
///
/// Pure so the policy can be tested without touching the disk; the caller
/// applies the result.
pub fn plan_eviction(
    candidates: &[EvictionCandidate],
    sizes: &dyn Fn(&str) -> u64,
    cap_bytes: u64,
    protected: &[String],
) -> Vec<EvictionCandidate> {
    let total: u64 = candidates.iter().map(|c| sizes(&c.path)).sum();
    if total <= cap_bytes {
        return Vec::new();
    }

    let mut ordered: Vec<&EvictionCandidate> = candidates
        .iter()
        .filter(|c| !protected.iter().any(|p| p == &c.path))
        .collect();
    // Oldest fetch first — least recently useful.
    ordered.sort_by_key(|c| (c.fetched_at, c.track_id.clone()));

    let mut freed = 0u64;
    let mut doomed = Vec::new();
    for candidate in ordered {
        if total.saturating_sub(freed) <= cap_bytes {
            break;
        }
        freed += sizes(&candidate.path);
        doomed.push(candidate.clone());
    }
    doomed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, at: i64) -> EvictionCandidate {
        EvictionCandidate {
            track_id: id.to_string(),
            path: format!("/cache/{id}.mp3"),
            fetched_at: at,
        }
    }

    // Every candidate is 100 bytes, so caps read as "how many files fit".
    fn flat_sizes(_: &str) -> u64 {
        100
    }

    #[test]
    fn http_failures_read_as_sentences_not_urls() {
        // The raw reqwest error dumped a full CDN URL into the error toast.
        assert_eq!(
            describe_http_failure(401),
            "This recording isn't available for download"
        );
        assert_eq!(describe_http_failure(403), describe_http_failure(401));
        assert!(describe_http_failure(503).contains("having problems"));
        assert!(describe_http_failure(429).contains("rate limiting"));
        assert!(!describe_http_failure(418).contains("http"));
    }

    #[test]
    fn under_cap_evicts_nothing() {
        let c = vec![candidate("a", 1), candidate("b", 2)];
        assert!(plan_eviction(&c, &flat_sizes, 500, &[]).is_empty());
    }

    #[test]
    fn evicts_oldest_first_until_under_cap() {
        let c = vec![
            candidate("new", 30),
            candidate("old", 10),
            candidate("mid", 20),
        ];
        let doomed = plan_eviction(&c, &flat_sizes, 100, &[]);
        // 300 bytes held, 100 allowed -> drop the two oldest.
        assert_eq!(doomed.len(), 2);
        assert_eq!(doomed[0].track_id, "old");
        assert_eq!(doomed[1].track_id, "mid");
    }

    #[test]
    fn never_evicts_a_protected_path() {
        let c = vec![candidate("old", 10), candidate("new", 20)];
        let protected = vec!["/cache/old.mp3".to_string()];
        let doomed = plan_eviction(&c, &flat_sizes, 100, &protected);
        // The oldest is playing, so the newer one goes instead.
        assert_eq!(doomed.len(), 1);
        assert_eq!(doomed[0].track_id, "new");
    }

    #[test]
    fn protected_files_alone_can_exceed_the_cap() {
        // Nothing is evictable; the cap is exceeded rather than deleting a
        // file the decoder currently holds open.
        let c = vec![candidate("a", 1), candidate("b", 2)];
        let protected = vec!["/cache/a.mp3".into(), "/cache/b.mp3".into()];
        assert!(plan_eviction(&c, &flat_sizes, 50, &protected).is_empty());
    }

    #[test]
    fn sanitize_blocks_path_traversal() {
        assert_eq!(sanitize("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize("gd1977-05-08.sbd"), "gd1977-05-08_sbd");
        assert_eq!(sanitize(""), "unnamed");
    }

    #[test]
    fn sanitize_truncates_long_ids_without_colliding() {
        let a = sanitize(&"x".repeat(300));
        let b = sanitize(&format!("{}y", "x".repeat(299)));
        assert!(a.len() <= 96 && b.len() <= 96);
        assert_ne!(a, b);
    }

    #[test]
    fn cached_path_stays_inside_the_cache_dir() {
        let path = cached_path("archive", "../escape", "mp3");
        assert!(path.starts_with(paths::source_cache_dir()));
        assert!(!path.to_string_lossy().contains(".."));
    }
}
