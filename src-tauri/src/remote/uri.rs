// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! `wave-remote://<source_id>/<remote_id>` paths. A remote track's `path` is
//! this URI, so it can sit in `tracks.path`, playlists and the queue like any
//! local path while never being mistaken for a file.

pub const REMOTE_SCHEME: &str = "wave-remote://";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteUri {
    pub source_id: String,
    pub remote_id: String,
}

impl RemoteUri {
    pub fn new(source_id: &str, remote_id: &str) -> Self {
        Self {
            source_id: source_id.to_string(),
            remote_id: remote_id.to_string(),
        }
    }

    /// Both parts are non-empty and separated by exactly one `/`; anything
    /// else is not one of ours.
    pub fn parse(path: &str) -> Option<Self> {
        let rest = path.strip_prefix(REMOTE_SCHEME)?;
        let (source, remote) = rest.split_once('/')?;
        if source.is_empty() || remote.is_empty() || remote.contains('/') {
            return None;
        }
        Some(Self {
            source_id: percent_decode(source)?,
            remote_id: percent_decode(remote)?,
        })
    }

    pub fn to_path(&self) -> String {
        format!(
            "{REMOTE_SCHEME}{}/{}",
            percent_encode(&self.source_id),
            percent_encode(&self.remote_id)
        )
    }
}

pub fn is_remote(path: &str) -> bool {
    path.starts_with(REMOTE_SCHEME)
}

/// Leaves only the RFC 3986 unreserved set (`A-Z a-z 0-9 - _ . ~`) as is.
pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `None` on a truncated or non-hex escape, or when the decoded bytes are not
/// valid UTF-8.
pub(crate) fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_encodes_slashes() {
        let uri = RemoteUri::new("src1", "lib/123?x");
        let path = uri.to_path();
        assert_eq!(path, "wave-remote://src1/lib%2F123%3Fx");
        assert_eq!(RemoteUri::parse(&path), Some(uri));
    }

    #[test]
    fn rejects_non_remote_and_malformed() {
        assert!(RemoteUri::parse("/music/a.flac").is_none());
        assert!(RemoteUri::parse("wave-remote://only-source").is_none());
        assert!(RemoteUri::parse("wave-remote:///123").is_none());
        assert!(!is_remote("content://x"));
    }

    #[test]
    fn percent_decode_rejects_bad_escapes_and_invalid_utf8() {
        assert_eq!(percent_decode("a%2Fb").as_deref(), Some("a/b"));
        assert!(percent_decode("a%2").is_none());
        assert!(percent_decode("a%zz").is_none());
        assert!(percent_decode("a%+1").is_none());
        assert!(percent_decode("%FF").is_none());
    }

    #[test]
    fn percent_encode_keeps_only_unreserved_bytes() {
        assert_eq!(percent_encode("aZ09-_.~"), "aZ09-_.~");
        assert_eq!(percent_encode("a b/é"), "a%20b%2F%C3%A9");
    }
}
