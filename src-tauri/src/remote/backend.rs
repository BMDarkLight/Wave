// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The contract a remote library backend fulfils.

use super::gate::GateState;

/// One track of a remote catalog, normalized so the store never needs to know
/// which backend produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTrackMeta {
    pub remote_id: String,
    pub title: String,
    pub artist: String,
    pub album_artist: Option<String>,
    pub album: String,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub duration_seconds: Option<f64>,
    pub size: u64,
    pub extension: String,
    pub part_key: String,
    pub art_ref: Option<String>,
    pub updated_at: Option<i64>,
}

/// One page of a remote catalog. `next` is the cursor for the following page,
/// `None` once the catalog is exhausted.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogPage {
    pub tracks: Vec<RemoteTrackMeta>,
    pub total: Option<usize>,
    pub next: Option<usize>,
}

/// Unreachable messages are short sentences fit for a toast and never carry a
/// URL that contains a token.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteError {
    Unreachable(String),
    Unauthorized,
    Blocked(GateState),
    Parse(String),
    Http(u16),
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::Unreachable(msg) => f.write_str(msg),
            RemoteError::Unauthorized => f.write_str("Sign in to Plex again"),
            RemoteError::Blocked(GateState::Disabled) => f.write_str("Outside Sourcing is off"),
            RemoteError::Blocked(GateState::Cellular) => {
                f.write_str("Remote music is set to Wi-Fi only")
            }
            RemoteError::Blocked(GateState::Allowed) => f.write_str("Remote music is unavailable"),
            RemoteError::Parse(_) => f.write_str("Unexpected response from the server"),
            RemoteError::Http(code) => write!(f, "The server answered with an error ({code})"),
        }
    }
}

pub trait ByteStream: std::io::Read + Send {}
impl<T: std::io::Read + Send> ByteStream for T {}

pub trait RemoteBackend: Send + Sync {
    fn kind(&self) -> &'static str;
    fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError>;
    fn open_bytes(&self, part_key: &str, offset: u64) -> Result<Box<dyn ByteStream>, RemoteError>;
    #[allow(dead_code)]
    fn check_reachable(&self) -> Result<(), RemoteError>;
    fn fetch_artwork(&self, art_ref: &str) -> Result<Vec<u8>, RemoteError>;

    /// The connection last known to work, for the caller to store as the
    /// source's `preferred_uri`.
    fn working_uri(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_user_facing() {
        assert_eq!(
            RemoteError::Unreachable("Can't reach Home".into()).to_string(),
            "Can't reach Home"
        );
        assert_eq!(
            RemoteError::Unauthorized.to_string(),
            "Sign in to Plex again"
        );
        assert_eq!(
            RemoteError::Blocked(GateState::Disabled).to_string(),
            "Outside Sourcing is off"
        );
        assert_eq!(
            RemoteError::Blocked(GateState::Cellular).to_string(),
            "Remote music is set to Wi-Fi only"
        );
        assert_eq!(
            RemoteError::Blocked(GateState::Allowed).to_string(),
            "Remote music is unavailable"
        );
        assert_eq!(
            RemoteError::Parse("token abc".into()).to_string(),
            "Unexpected response from the server"
        );
        assert_eq!(
            RemoteError::Http(503).to_string(),
            "The server answered with an error (503)"
        );
    }
}
