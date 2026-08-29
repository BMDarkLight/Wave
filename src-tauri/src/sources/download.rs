//! Turning a streamed track into a kept one.
//!
//! Destination differs by platform, by design:
//!
//! - **Desktop** writes to a dedicated Wave downloads folder, registered as a
//!   media folder on first use so a download is browsable immediately even if
//!   the user never configured one.
//! - **Android** writes into the primary existing media folder, so downloads
//!   sit with the rest of the user's music. The folder picker asks for write
//!   access but deliberately falls back to read-only when the system refuses
//!   (see `FolderPickerCallback.java`), so a read-only grant is a *normal*
//!   outcome here, not an error — we fall back to app-private storage and tell
//!   the user where the file went.
//!
//! Downloads **copy** rather than move. Promoting a cached file while that
//! track is playing would pull the file out from under an open decoder:
//! survivable on Unix, a hard failure on Windows. The cache copy is left for
//! the next eviction pass instead.

use std::path::{Path, PathBuf};

use super::SourceTrack;

/// Where a download ended up, and whether we had to fall back.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadDestination {
    pub path: PathBuf,
    /// Folder to register as a media folder so the file is browsable.
    pub media_folder: PathBuf,
    /// Set when the intended destination was unusable. Surfaced to the user as
    /// a toast rather than swallowed.
    pub fallback_reason: Option<String>,
}

/// Filesystem-safe component from arbitrary tag text.
fn safe_component(raw: &str, fallback: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() {
        fallback.to_string()
    } else if cleaned.len() > 120 {
        cleaned.chars().take(120).collect()
    } else {
        cleaned
    }
}

/// `<root>/<Artist>/<Album>/<Title>.<ext>`, every component sanitised.
pub fn relative_layout(track: &SourceTrack) -> PathBuf {
    PathBuf::from(safe_component(&track.artist, "Unknown Artist"))
        .join(safe_component(
            track.album.as_deref().unwrap_or("Singles"),
            "Singles",
        ))
        .join(format!(
            "{}.{}",
            safe_component(&track.title, "Untitled"),
            track.extension()
        ))
}

/// Add `-1`, `-2`, ... before the extension until the name is free, so a
/// second download of the same title never silently overwrites the first.
pub fn dedupe_filename(desired: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    if !exists(desired) {
        return desired.to_path_buf();
    }
    let parent = desired.parent().unwrap_or_else(|| Path::new(""));
    let stem = desired
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("track");
    let ext = desired
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3");
    for n in 1..1000 {
        let candidate = parent.join(format!("{stem}-{n}.{ext}"));
        if !exists(&candidate) {
            return candidate;
        }
    }
    parent.join(format!("{stem}-{}.{ext}", std::process::id()))
}

/// Pick the download destination for this platform.
///
/// `media_folders` is the user's configured list; on Android the first usable
/// local one wins. `is_writable` decides whether a candidate root can actually
/// be written to — injected so the policy is testable without a device.
pub fn choose_destination(
    track: &SourceTrack,
    media_folders: &[String],
    is_writable: &dyn Fn(&Path) -> bool,
    exists: &dyn Fn(&Path) -> bool,
) -> DownloadDestination {
    let relative = relative_layout(track);
    let private_root = crate::app::paths::downloads_dir();

    if cfg!(target_os = "android") {
        // SAF trees are `content://` URIs, which are not filesystem paths; only
        // a plain path can be written through std::fs. A user whose primary
        // folder is a SAF tree falls back to app-private storage.
        let primary = media_folders
            .iter()
            .find(|folder| !folder.starts_with("content://"))
            .map(PathBuf::from)
            .filter(|root| is_writable(root));

        return match primary {
            Some(root) => DownloadDestination {
                path: dedupe_filename(&root.join(&relative), exists),
                media_folder: root,
                fallback_reason: None,
            },
            None => DownloadDestination {
                path: dedupe_filename(&private_root.join(&relative), exists),
                media_folder: private_root,
                fallback_reason: Some(
                    "Your music folder is read-only, so this was saved to Wave's own downloads folder"
                        .to_string(),
                ),
            },
        };
    }

    DownloadDestination {
        path: dedupe_filename(&private_root.join(&relative), exists),
        media_folder: private_root,
        fallback_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> SourceTrack {
        SourceTrack {
            provider: "jamendo".into(),
            id: "1".into(),
            title: "Sunrise".into(),
            artist: "Bensound".into(),
            album: Some("Royalty Free".into()),
            duration_seconds: Some(185.0),
            artwork_url: None,
            audio_url: Some("https://example.com/a.mp3".into()),
            is_full_length: true,
            downloadable: true,
            attribution: None,
            already_in_library: None,
        }
    }

    #[test]
    fn layout_is_artist_album_title() {
        let path = relative_layout(&track());
        assert_eq!(path, PathBuf::from("Bensound/Royalty Free/Sunrise.mp3"));
    }

    #[test]
    fn layout_sanitises_separators_out_of_tags() {
        let mut t = track();
        t.artist = "AC/DC".into();
        t.album = None;
        t.title = "Who Made Who?".into();
        let path = relative_layout(&t);
        assert_eq!(path, PathBuf::from("AC_DC/Singles/Who Made Who_.mp3"));
    }

    #[test]
    fn layout_falls_back_on_empty_tags() {
        let mut t = track();
        t.artist = "   ".into();
        t.title = "...".into();
        t.album = Some(String::new());
        let path = relative_layout(&t);
        assert_eq!(path, PathBuf::from("Unknown Artist/Singles/Untitled.mp3"));
    }

    #[test]
    fn dedupe_never_overwrites_an_existing_file() {
        let taken = ["/m/A/B/S.mp3", "/m/A/B/S-1.mp3"];
        let exists = |p: &Path| taken.contains(&p.to_string_lossy().as_ref());
        let result = dedupe_filename(Path::new("/m/A/B/S.mp3"), &exists);
        assert_eq!(result, PathBuf::from("/m/A/B/S-2.mp3"));
    }

    #[test]
    fn dedupe_leaves_a_free_name_alone() {
        let exists = |_: &Path| false;
        let p = Path::new("/m/A/B/S.mp3");
        assert_eq!(dedupe_filename(p, &exists), p.to_path_buf());
    }

    #[test]
    fn desktop_uses_the_dedicated_downloads_folder() {
        if cfg!(target_os = "android") {
            return;
        }
        let dest = choose_destination(&track(), &["/music".into()], &|_| true, &|_| false);
        assert!(dest.path.starts_with(crate::app::paths::downloads_dir()));
        assert_eq!(dest.fallback_reason, None);
    }

    #[test]
    fn android_prefers_the_primary_media_folder() {
        if !cfg!(target_os = "android") {
            return;
        }
        let folders = vec!["/storage/emulated/0/Music".to_string()];
        let dest = choose_destination(&track(), &folders, &|_| true, &|_| false);
        assert!(dest.path.starts_with("/storage/emulated/0/Music"));
        assert_eq!(dest.fallback_reason, None);
    }

    #[test]
    fn android_falls_back_when_the_grant_is_read_only() {
        if !cfg!(target_os = "android") {
            return;
        }
        let folders = vec!["/storage/emulated/0/Music".to_string()];
        // is_writable = false models the READ-only SAF grant fallback.
        let dest = choose_destination(&track(), &folders, &|_| false, &|_| false);
        assert!(dest.path.starts_with(crate::app::paths::downloads_dir()));
        assert!(dest.fallback_reason.is_some());
    }

    #[test]
    fn android_falls_back_for_a_saf_uri_folder() {
        if !cfg!(target_os = "android") {
            return;
        }
        // content:// trees can't be written through std::fs.
        let folders =
            vec!["content://com.android.externalstorage/tree/primary%3AMusic".to_string()];
        let dest = choose_destination(&track(), &folders, &|_| true, &|_| false);
        assert!(dest.fallback_reason.is_some());
    }
}
