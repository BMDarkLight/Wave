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
    /// The server only takes the password itself, and the connection is
    /// not HTTPS.
    Insecure,
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::Unreachable(msg) => f.write_str(msg),
            RemoteError::Unauthorized => f.write_str("Sign in to this server again"),
            RemoteError::Blocked(GateState::Disabled) => f.write_str("Outside Sourcing is off"),
            RemoteError::Blocked(GateState::Cellular) => {
                f.write_str("Your servers are set to Wi-Fi only")
            }
            RemoteError::Blocked(GateState::Allowed) => {
                f.write_str("Can't stream from your servers right now")
            }
            RemoteError::Parse(_) => f.write_str("Unexpected response from the server"),
            RemoteError::Http(code) => write!(f, "The server returned an error ({code})"),
            RemoteError::Insecure => f.write_str(
                "This server only accepts the password itself. Connect to it over HTTPS",
            ),
        }
    }
}

/// Seconds since 1970 for an ISO 8601 date such as
/// "2024-03-01T18:22:05.1234567Z", read as UTC.
pub fn unix_seconds(date: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| -> Option<i64> { date.get(range)?.parse().ok() };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from the civil date, after Howard Hinnant's algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
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

    /// The file's size, for backends whose catalog doesn't list it. The
    /// stream cache needs it before the first byte is written.
    fn content_length(&self, _part_key: &str) -> Result<u64, RemoteError> {
        Err(RemoteError::Parse(
            "The server didn't say how big this song is".into(),
        ))
    }
}

/// The total size a ranged answer reports: from `Content-Range` on a 206,
/// from `Content-Length` on a 200 that ignored the range.
pub fn total_size(resp: &reqwest::blocking::Response) -> Option<u64> {
    let header = |name| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    if resp.status().as_u16() == 206 {
        return header(reqwest::header::CONTENT_RANGE)?
            .rsplit('/')
            .next()?
            .trim()
            .parse()
            .ok()
            .filter(|size| *size > 0);
    }
    resp.content_length().filter(|size| *size > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_seconds_reads_iso_dates() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            unix_seconds("2000-02-29T12:00:00.0000000Z"),
            Some(951_825_600)
        );
        assert_eq!(unix_seconds("garbage"), None);
        assert_eq!(unix_seconds("2024-13-01T00:00:00Z"), None);
    }

    #[test]
    fn display_messages_are_user_facing() {
        assert_eq!(
            RemoteError::Unreachable("Can't reach Home".into()).to_string(),
            "Can't reach Home"
        );
        assert_eq!(
            RemoteError::Unauthorized.to_string(),
            "Sign in to this server again"
        );
        assert_eq!(
            RemoteError::Blocked(GateState::Disabled).to_string(),
            "Outside Sourcing is off"
        );
        assert_eq!(
            RemoteError::Blocked(GateState::Cellular).to_string(),
            "Your servers are set to Wi-Fi only"
        );
        assert_eq!(
            RemoteError::Blocked(GateState::Allowed).to_string(),
            "Can't stream from your servers right now"
        );
        assert_eq!(
            RemoteError::Parse("token abc".into()).to_string(),
            "Unexpected response from the server"
        );
        assert_eq!(
            RemoteError::Http(503).to_string(),
            "The server returned an error (503)"
        );
    }
}
