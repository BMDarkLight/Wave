//! Remote song sourcing: the third search tier.
//!
//! Search escalates scope -> library -> here. Providers are queried only when
//! the user explicitly asks (see the `search_sources` command), never on a
//! keystroke.
//!
//! Every rule from [`crate::enrichment`] applies: calls are blocking and MUST
//! run off the command response path, and every failure degrades to "this
//! provider found nothing" rather than failing the whole search. One provider
//! being down, slow, or unconfigured never blocks another.

pub mod archive;
pub mod cache;
pub mod deezer;
pub mod download;
pub mod jamendo;

use reqwest::blocking::Client;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT_SECONDS: u64 = 8;
const USER_AGENT: &str = "Wave/0.1.0 (music sourcing)";

/// Shared blocking client for every provider. Separate from the enrichment
/// client so a slow MusicBrainz call can't consume a sourcing connection.
pub fn source_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
            .user_agent(USER_AGENT)
            .build()
            .expect("failed to build source HTTP client")
    })
}

// ── Errors ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SourceError {
    /// Provider needs credentials the user hasn't supplied (Jamendo client id).
    /// Surfaced to the UI as a setup hint, not as a failure.
    NotConfigured(String),
    Network(String),
    Parse(String),
    /// Provider had a result but no playable audio behind it.
    NoAudio,
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured(what) => write!(f, "{what}"),
            Self::Network(e) => write!(f, "Network error: {e}"),
            Self::Parse(e) => write!(f, "Unexpected response: {e}"),
            Self::NoAudio => write!(f, "No playable audio for this result"),
        }
    }
}

// ── DTOs ──────────────────────────────────────────────────────────────────

/// One remote search result, shaped for direct use by the UI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SourceTrack {
    pub provider: String,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_seconds: Option<f64>,
    pub artwork_url: Option<String>,
    /// `None` when the provider needs a second call to resolve playable audio
    /// (Internet Archive). Always resolved through [`SourceProvider::resolve_audio`].
    pub audio_url: Option<String>,
    /// False for Deezer, whose public API only exposes 30-second previews.
    pub is_full_length: bool,
    /// False when the provider's terms don't allow keeping a copy. Drives
    /// whether the UI offers a download button at all.
    pub downloadable: bool,
    /// Licence line to display alongside the result (CC sources).
    pub attribution: Option<String>,
    /// Path of the local track this result duplicates, filled in by the command
    /// layer — providers always leave it `None`. Lets the UI mark a result as
    /// already owned and play the local copy instead of re-fetching.
    #[serde(default)]
    pub already_in_library: Option<String>,
}

impl SourceTrack {
    /// Filename extension for the cached/downloaded file, guessed from the
    /// audio URL and defaulted to mp3 — every provider here serves MP3.
    pub fn extension(&self) -> String {
        self.audio_url
            .as_deref()
            .and_then(|url| url.split('?').next())
            .and_then(|url| url.rsplit('.').next())
            .filter(|ext| ext.len() <= 4 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_else(|| "mp3".to_string())
    }
}

/// One provider's slice of a search. `error` being `Some` while `tracks` is
/// empty is the normal degraded case — the UI renders the section as
/// unavailable and the other sections still show results.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderResults {
    pub provider: String,
    pub display_name: String,
    pub tracks: Vec<SourceTrack>,
    pub error: Option<String>,
}

// ── Provider trait ────────────────────────────────────────────────────────

pub trait SourceProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;

    /// Minimum spacing between calls to this provider.
    fn rate_limit(&self) -> Duration {
        Duration::from_millis(0)
    }

    fn search(
        &self,
        client: &Client,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>, SourceError>;

    /// Turn a search result into a fetchable audio URL. Providers that already
    /// filled `audio_url` just hand it back; Internet Archive does its file
    /// listing here so search stays one request per provider.
    fn resolve_audio(&self, client: &Client, track: &SourceTrack) -> Result<String, SourceError>;
}

// ── Configuration ─────────────────────────────────────────────────────────

/// What the registry needs to decide which providers are usable.
#[derive(Debug, Clone, Default)]
pub struct SourceConfig {
    /// Free Jamendo API client id. Without it, Jamendo reports itself as
    /// not configured rather than silently disappearing.
    pub jamendo_client_id: Option<String>,
}

/// Every provider, configured or not. Unconfigured ones still appear so the UI
/// can tell the user what to set up instead of hiding the option.
pub fn providers(config: &SourceConfig) -> Vec<Box<dyn SourceProvider>> {
    vec![
        Box::new(deezer::Deezer),
        Box::new(jamendo::Jamendo::new(config.jamendo_client_id.clone())),
        Box::new(archive::InternetArchive),
    ]
}

pub fn provider_by_id(config: &SourceConfig, id: &str) -> Option<Box<dyn SourceProvider>> {
    providers(config).into_iter().find(|p| p.id() == id)
}

// ── Rate limiting ─────────────────────────────────────────────────────────

/// Sleeps until `min_gap` has passed since this provider's previous call.
/// Per-provider so a slow, heavily rate-limited source never delays a fast one.
pub fn throttle(provider_id: &str, min_gap: Duration) {
    if min_gap.is_zero() {
        return;
    }
    static LAST_CALL: OnceLock<Mutex<std::collections::HashMap<String, Instant>>> = OnceLock::new();
    let map = LAST_CALL.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let sleep_for = {
        let Ok(mut guard) = map.lock() else {
            return;
        };
        let now = Instant::now();
        let wait = guard
            .get(provider_id)
            .map(|last| min_gap.saturating_sub(now.duration_since(*last)))
            .unwrap_or_default();
        guard.insert(provider_id.to_string(), now + wait);
        wait
    };
    if !sleep_for.is_zero() {
        std::thread::sleep(sleep_for);
    }
}

// ── Fan-out ───────────────────────────────────────────────────────────────

/// Search every provider concurrently, one thread each, and return their
/// results in registry order. A provider that panics, errors, or times out
/// yields an errored section rather than taking the search down with it.
pub fn search_all(config: &SourceConfig, query: &str, limit: usize) -> Vec<ProviderResults> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Vec::new();
    }

    let handles: Vec<_> = providers(config)
        .into_iter()
        .map(|provider| {
            let query = query.clone();
            std::thread::spawn(move || {
                let id = provider.id().to_string();
                let display_name = provider.display_name().to_string();
                throttle(&id, provider.rate_limit());
                match provider.search(source_client(), &query, limit) {
                    Ok(tracks) => ProviderResults {
                        provider: id,
                        display_name,
                        tracks,
                        error: None,
                    },
                    Err(error) => ProviderResults {
                        provider: id,
                        display_name,
                        tracks: Vec::new(),
                        error: Some(error.to_string()),
                    },
                }
            })
        })
        .collect();

    handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .collect()
}

// ── Shared parsing helpers ────────────────────────────────────────────────

pub(crate) fn get_json(client: &Client, url: &str) -> Result<serde_json::Value, SourceError> {
    let body = client
        .get(url)
        .send()
        .map_err(|e| SourceError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| SourceError::Network(e.to_string()))?
        .text()
        .map_err(|e| SourceError::Network(e.to_string()))?;
    serde_json::from_str(&body).map_err(|e| SourceError::Parse(e.to_string()))
}

pub(crate) fn str_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Providers report ids as strings or numbers depending on the endpoint.
pub(crate) fn id_field(value: &serde_json::Value, key: &str) -> Option<String> {
    match value.get(key) {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

pub(crate) fn duration_field(value: &serde_json::Value, key: &str) -> Option<f64> {
    match value.get(key) {
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|d| *d > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_falls_back_to_mp3() {
        let mut track = SourceTrack {
            provider: "deezer".into(),
            id: "1".into(),
            title: "t".into(),
            artist: "a".into(),
            album: None,
            duration_seconds: None,
            artwork_url: None,
            audio_url: None,
            is_full_length: false,
            downloadable: false,
            attribution: None,
            already_in_library: None,
        };
        assert_eq!(track.extension(), "mp3");

        track.audio_url = Some("https://example.com/a/b.flac".into());
        assert_eq!(track.extension(), "flac");

        // Query strings must not leak into the extension.
        track.audio_url = Some("https://example.com/a/b.ogg?token=abc.def".into());
        assert_eq!(track.extension(), "ogg");

        // A path with no extension at all stays mp3 rather than becoming junk.
        track.audio_url = Some("https://example.com/stream/12345".into());
        assert_eq!(track.extension(), "mp3");
    }

    #[test]
    fn id_field_accepts_strings_and_numbers() {
        let v = serde_json::json!({ "a": 42, "b": "x1", "c": "", "d": null });
        assert_eq!(id_field(&v, "a").as_deref(), Some("42"));
        assert_eq!(id_field(&v, "b").as_deref(), Some("x1"));
        assert_eq!(id_field(&v, "c"), None);
        assert_eq!(id_field(&v, "d"), None);
    }

    #[test]
    fn duration_field_rejects_zero_and_garbage() {
        let v = serde_json::json!({ "a": 210, "b": "185", "c": 0, "d": "nope" });
        assert_eq!(duration_field(&v, "a"), Some(210.0));
        assert_eq!(duration_field(&v, "b"), Some(185.0));
        assert_eq!(duration_field(&v, "c"), None);
        assert_eq!(duration_field(&v, "d"), None);
    }
}

/// Live network checks. Ignored by default so the normal suite stays offline
/// and deterministic; run with `cargo test -- --ignored --nocapture` to verify
/// a provider's real API still matches its parser.
#[cfg(test)]
mod live_tests {
    use super::*;

    #[test]
    #[ignore = "hits the network"]
    fn deezer_search_resolve_and_fetch_round_trip() {
        let client = source_client();
        let provider = deezer::Deezer;

        let tracks = provider
            .search(client, "daft punk", 3)
            .expect("Deezer search failed");
        assert!(!tracks.is_empty(), "Deezer returned no parseable results");

        let track = &tracks[0];
        assert!(!track.id.is_empty());
        assert!(!track.title.is_empty());
        assert!(!track.artist.is_empty());
        assert!(!track.is_full_length, "Deezer must never claim full length");
        assert!(!track.downloadable, "Deezer previews are not downloadable");

        let url = provider
            .resolve_audio(client, track)
            .expect("Deezer preview URL missing");

        let dest = std::env::temp_dir().join(format!("wave-live-{}.mp3", track.id));
        let _ = std::fs::remove_file(&dest);
        let written = cache::fetch_to(client, &url, &dest).expect("preview fetch failed");

        assert!(
            written > 10_000,
            "preview suspiciously small: {written} bytes"
        );
        let header = std::fs::read(&dest).expect("cached file unreadable");
        // ID3 tag or a raw MPEG frame sync — either way, decodable audio.
        let is_audio =
            header.starts_with(b"ID3") || (header[0] == 0xFF && header[1] & 0xE0 == 0xE0);
        assert!(is_audio, "cached bytes are not MP3: {:?}", &header[..4]);

        // The temp `.part` file must never survive a successful fetch.
        assert!(!dest.with_extension("mp3.part").exists());

        // Bytes that look like MP3 are not enough: every Deezer preview ships
        // an empty ID3v2.4 tag that used to fail Symphonia's probe outright.
        // Decoding is the assertion that actually catches that.
        crate::audio::symphonia_source::SymphoniaSource::new(dest.to_string_lossy().as_ref())
            .expect("cached preview must be decodable by the playback engine");

        // Bytes that look like MP3 are not enough: every Deezer preview ships
        // an empty ID3v2.4 tag that used to fail Symphonia's probe outright.
        // Decoding is the assertion that actually catches that.
        crate::audio::symphonia_source::SymphoniaSource::new(dest.to_string_lossy().as_ref())
            .expect("cached preview must be decodable by the playback engine");

        println!(
            "OK  {} — {} ({} bytes cached to {})",
            track.artist,
            track.title,
            written,
            dest.display()
        );
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    #[ignore = "hits the network"]
    fn archive_search_resolves_to_a_real_audio_url() {
        let client = source_client();
        let provider = archive::InternetArchive;

        let tracks = provider
            .search(client, "grateful dead 1977", 3)
            .expect("Archive search failed");
        assert!(!tracks.is_empty(), "Archive returned no parseable results");

        // Search is item-level and deliberately defers audio resolution.
        assert!(tracks.iter().all(|t| t.audio_url.is_none()));

        let (track, resolved) = tracks
            .iter()
            .find_map(|t| provider.resolve_audio(client, t).ok().map(|url| (t, url)))
            .expect("no Archive item resolved to audio");
        assert!(resolved.starts_with("https://archive.org/download/"));

        // Asserting the URL's shape is not enough — that is exactly how a 401
        // on an access-restricted item reached the user. Fetch it and decode.
        // Derive the extension from the resolved URL exactly as
        // `stream_source_track` does — Archive tracks carry no audio_url until
        // resolve, so a cache path built before that would have no extension
        // and `validate_audio_path` would reject it.
        let mut resolved_track = track.clone();
        resolved_track.audio_url = Some(resolved.clone());
        let dest = std::env::temp_dir().join(format!(
            "wave-live-archive-{}.{}",
            track.id,
            resolved_track.extension()
        ));
        let _ = std::fs::remove_file(&dest);
        let written = cache::fetch_to(client, &resolved, &dest)
            .unwrap_or_else(|e| panic!("resolved URL was not fetchable: {e}"));
        assert!(written > 10_000, "suspiciously small: {written} bytes");

        crate::audio::symphonia_source::SymphoniaSource::new(dest.to_string_lossy().as_ref())
            .expect("fetched Archive audio must be decodable");

        println!("OK  {} — {written} bytes from {resolved}", track.title);
        let _ = std::fs::remove_file(&dest);
    }
}
