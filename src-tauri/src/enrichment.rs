//! Best-effort external enrichment: genre tags + similar artists for the
//! Home page diversity feature (see `Library::get_home_suggestions`).
//!
//! Every network call here is blocking and MUST run on a dedicated
//! background thread, never on a command's response path or while holding
//! the library lock — see `commands::run_artist_enrichment_job`. Every
//! failure (network, parse, rate limit, unknown artist) degrades to "no
//! data" rather than an error: Home suggestions must keep working exactly
//! as before if this is offline, slow, or blocked.

use reqwest::blocking::Client;
use serde::Deserialize;
use std::time::Duration;

const REQUEST_TIMEOUT_SECONDS: u64 = 6;
const USER_AGENT: &str = "Wave/0.1.0 (local artist enrichment)";

/// Spacing between MusicBrainz calls — their usage policy asks for ~1 req/sec
/// from unauthenticated clients.
pub const RATE_LIMIT_DELAY: Duration = Duration::from_millis(1100);

/// A broad ListenBrainz Labs similar-artists model (5-year listening
/// window). Picked for coverage over precision — the app already ranks
/// results locally by score and filters to what fits the request.
const LISTENBRAINZ_ALGORITHM: &str =
    "session_based_days_1825_session_300_contribution_3_threshold_10_limit_100_filter_True_skip_30";

pub fn enrichment_client() -> &'static Client {
    static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
            .user_agent(USER_AGENT)
            .build()
            .expect("failed to build enrichment HTTP client")
    })
}

/// How many similar artists we keep per seed artist (ListenBrainz can return
/// up to 100 — Home suggestions only ever reads the top 8, so anything past
/// this is pure DB bloat).
const MAX_SIMILAR_ARTISTS_STORED: usize = 20;

/// How many of the (already score-sorted) similar artists get an album-cover
/// lookup. Bounded separately from the above because each lookup is its own
/// MusicBrainz call — this matches the 8 Home suggestions can ever display
/// per seed, so no lookup is wasted on an artist that won't be shown.
const MAX_COVER_LOOKUPS_PER_SEED: usize = 8;

/// One similar artist plus what's needed to render it: an optional
/// representative album (release-group) to source cover art from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SimilarArtistEntry {
    pub name: String,
    pub mbid: Option<String>,
    pub score: f64,
    pub cover_release_group_mbid: Option<String>,
}

/// Genre tags + similar artists resolved for one artist name. Any field can
/// come back empty if that stage failed or found nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtistProfile {
    pub mbid: Option<String>,
    /// Genre tags, most-used first.
    pub tags: Vec<String>,
    /// Highest similarity score first.
    pub similar: Vec<SimilarArtistEntry>,
}

/// Resolve genre tags + similar artists (each with a best-effort album cover)
/// for `artist_name`. Sleeps between each network call to respect
/// MusicBrainz's rate-limit policy — callers must invoke this from a
/// background thread, never inline on a command.
pub fn fetch_artist_profile(client: &Client, artist_name: &str) -> ArtistProfile {
    let mut profile = ArtistProfile::default();
    let Some(mbid) = resolve_artist_mbid(client, artist_name) else {
        return profile;
    };
    profile.mbid = Some(mbid.clone());

    std::thread::sleep(RATE_LIMIT_DELAY);
    profile.tags = fetch_artist_genres(client, &mbid);

    std::thread::sleep(RATE_LIMIT_DELAY);
    let mut similar = fetch_similar_artists(client, &mbid);

    for entry in similar.iter_mut().take(MAX_COVER_LOOKUPS_PER_SEED) {
        let Some(similar_mbid) = entry.mbid.clone() else {
            continue;
        };
        std::thread::sleep(RATE_LIMIT_DELAY);
        entry.cover_release_group_mbid = fetch_representative_release_group(client, &similar_mbid);
    }
    profile.similar = similar;

    profile
}

/// Cover Art Archive URL for a release-group's front cover, sized for a Home
/// page card. Missing coverage (common — not every release is archived) is
/// left to the caller's `<img onerror>` fallback.
pub fn cover_art_url(release_group_mbid: &str) -> String {
    format!("https://coverartarchive.org/release-group/{release_group_mbid}/front-250")
}

fn resolve_artist_mbid(client: &Client, artist_name: &str) -> Option<String> {
    let query = format!(
        "artist:\"{}\"",
        crate::metadata::escape_musicbrainz_query(artist_name)
    );
    let body = client
        .get("https://musicbrainz.org/ws/2/artist/")
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "1")])
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .ok()?;
    parse_mb_artist_search(&body)
}

fn fetch_artist_genres(client: &Client, mbid: &str) -> Vec<String> {
    let url = format!("https://musicbrainz.org/ws/2/artist/{mbid}");
    let body = client
        .get(&url)
        .query(&[("fmt", "json"), ("inc", "genres")])
        .send()
        .and_then(|r| r.error_for_status())
        .ok()
        .and_then(|r| r.text().ok());
    match body {
        Some(text) => parse_mb_artist_genres(&text),
        None => Vec::new(),
    }
}

fn fetch_similar_artists(client: &Client, mbid: &str) -> Vec<SimilarArtistEntry> {
    let body = client
        .get("https://labs.api.listenbrainz.org/similar-artists/json")
        .query(&[("artist_mbids", mbid), ("algorithm", LISTENBRAINZ_ALGORITHM)])
        .send()
        .and_then(|r| r.error_for_status())
        .ok()
        .and_then(|r| r.text().ok());
    let mut hits = match body {
        Some(text) => parse_lb_similar_artists(&text),
        None => Vec::new(),
    };
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(MAX_SIMILAR_ARTISTS_STORED);
    hits
}

fn fetch_representative_release_group(client: &Client, artist_mbid: &str) -> Option<String> {
    let body = client
        .get("https://musicbrainz.org/ws/2/release-group")
        .query(&[("artist", artist_mbid), ("type", "album"), ("fmt", "json"), ("limit", "1")])
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .ok()?;
    parse_mb_release_group_search(&body)
}

// ── Response parsing (pure — unit tested against real fixture payloads) ────

#[derive(Debug, Deserialize)]
struct MbArtistSearchResponse {
    artists: Option<Vec<MbArtistHit>>,
}

#[derive(Debug, Deserialize)]
struct MbArtistHit {
    id: String,
}

fn parse_mb_artist_search(body: &str) -> Option<String> {
    let parsed: MbArtistSearchResponse = serde_json::from_str(body).ok()?;
    parsed.artists?.into_iter().next().map(|hit| hit.id)
}

#[derive(Debug, Deserialize)]
struct MbArtistGenresResponse {
    genres: Option<Vec<MbGenre>>,
}

#[derive(Debug, Deserialize)]
struct MbGenre {
    name: String,
    #[serde(default)]
    count: i64,
}

fn parse_mb_artist_genres(body: &str) -> Vec<String> {
    let Ok(parsed) = serde_json::from_str::<MbArtistGenresResponse>(body) else {
        return Vec::new();
    };
    let mut genres = parsed.genres.unwrap_or_default();
    genres.sort_by(|a, b| b.count.cmp(&a.count));
    genres.into_iter().map(|g| g.name).collect()
}

#[derive(Debug, Deserialize)]
struct LbSimilarArtist {
    name: String,
    score: f64,
    #[serde(default)]
    artist_mbid: Option<String>,
}

fn parse_lb_similar_artists(body: &str) -> Vec<SimilarArtistEntry> {
    let Ok(parsed) = serde_json::from_str::<Vec<LbSimilarArtist>>(body) else {
        return Vec::new();
    };
    parsed
        .into_iter()
        .map(|a| SimilarArtistEntry {
            name: a.name,
            mbid: a.artist_mbid,
            score: a.score,
            cover_release_group_mbid: None,
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct MbReleaseGroupSearchResponse {
    #[serde(rename = "release-groups")]
    release_groups: Option<Vec<MbReleaseGroupHit>>,
}

#[derive(Debug, Deserialize)]
struct MbReleaseGroupHit {
    id: String,
}

fn parse_mb_release_group_search(body: &str) -> Option<String> {
    let parsed: MbReleaseGroupSearchResponse = serde_json::from_str(body).ok()?;
    parsed.release_groups?.into_iter().next().map(|rg| rg.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_mb_artist_search ───────────────────────────────────────────

    #[test]
    fn parse_mb_artist_search_extracts_first_mbid() {
        let body = r#"{"created":"2024-01-01","count":1,"offset":0,"artists":[{"id":"65f4f0c5-ef9e-490c-aee3-909e7ae6b2ab","name":"Metallica","score":100}]}"#;
        assert_eq!(
            parse_mb_artist_search(body),
            Some("65f4f0c5-ef9e-490c-aee3-909e7ae6b2ab".to_string())
        );
    }

    #[test]
    fn parse_mb_artist_search_handles_busy_error_payload() {
        let body = r#"{"error": "The MusicBrainz web server is currently busy. Please try again later."}"#;
        assert_eq!(parse_mb_artist_search(body), None);
    }

    #[test]
    fn parse_mb_artist_search_handles_empty_results() {
        let body = r#"{"created":"2024-01-01","count":0,"offset":0,"artists":[]}"#;
        assert_eq!(parse_mb_artist_search(body), None);
    }

    // ── parse_mb_artist_genres ───────────────────────────────────────────

    #[test]
    fn parse_mb_artist_genres_sorts_by_count_descending() {
        let body = r#"{"id":"x","genres":[{"name":"hard rock","count":20,"id":"a"},{"name":"heavy metal","count":41,"id":"b"},{"name":"thrash metal","count":5,"id":"c"}]}"#;
        assert_eq!(
            parse_mb_artist_genres(body),
            vec!["heavy metal".to_string(), "hard rock".to_string(), "thrash metal".to_string()]
        );
    }

    #[test]
    fn parse_mb_artist_genres_handles_missing_field() {
        let body = r#"{"id":"x"}"#;
        assert!(parse_mb_artist_genres(body).is_empty());
    }

    #[test]
    fn parse_mb_artist_genres_handles_invalid_mbid_error() {
        let body = r#"{"error":"Invalid mbid.","help":"For usage, please see: https://musicbrainz.org/development/mmd"}"#;
        assert!(parse_mb_artist_genres(body).is_empty());
    }

    // ── parse_lb_similar_artists ─────────────────────────────────────────

    #[test]
    fn parse_lb_similar_artists_extracts_name_mbid_and_score() {
        let body = r#"[{"artist_mbid":"5b11f4ce-a62d-471e-81fc-a69a8278c7da","name":"Nirvana","comment":"1980s-1990s US grunge band","type":"Group","gender":null,"score":2886,"reference_mbid":"65f4f0c5-ef9e-490c-aee3-909e7ae6b2ab"},{"artist_mbid":"ca891d65-d9b0-4258-89f7-e6ba29d83767","name":"Iron Maiden","comment":"","type":"Group","gender":null,"score":2742,"reference_mbid":"65f4f0c5-ef9e-490c-aee3-909e7ae6b2ab"}]"#;
        assert_eq!(
            parse_lb_similar_artists(body),
            vec![
                SimilarArtistEntry {
                    name: "Nirvana".to_string(),
                    mbid: Some("5b11f4ce-a62d-471e-81fc-a69a8278c7da".to_string()),
                    score: 2886.0,
                    cover_release_group_mbid: None,
                },
                SimilarArtistEntry {
                    name: "Iron Maiden".to_string(),
                    mbid: Some("ca891d65-d9b0-4258-89f7-e6ba29d83767".to_string()),
                    score: 2742.0,
                    cover_release_group_mbid: None,
                },
            ]
        );
    }

    #[test]
    fn parse_lb_similar_artists_handles_html_error_body() {
        let body = "<!doctype html>\n<html lang=en>\n<title>400 Bad Request</title>\n<h1>Bad Request</h1>";
        assert!(parse_lb_similar_artists(body).is_empty());
    }

    #[test]
    fn parse_lb_similar_artists_handles_empty_array() {
        assert!(parse_lb_similar_artists("[]").is_empty());
    }

    // ── parse_mb_release_group_search ────────────────────────────────────

    #[test]
    fn parse_mb_release_group_search_extracts_first_release_group_id() {
        let body = r#"{"release-group-count":433,"release-groups":[{"primary-type":"Album","disambiguation":"","first-release-date":"1989-08-08","primary-type-id":"f529b476-6e62-324f-b0aa-1f3e33d313fc","secondary-types":[],"id":"f1afec0b-26dd-3db5-9aa1-c91229a74a24","secondary-type-ids":[],"title":"Bleach"}],"release-group-offset":0}"#;
        assert_eq!(
            parse_mb_release_group_search(body),
            Some("f1afec0b-26dd-3db5-9aa1-c91229a74a24".to_string())
        );
    }

    #[test]
    fn parse_mb_release_group_search_handles_empty_results() {
        let body = r#"{"release-group-count":0,"release-groups":[],"release-group-offset":0}"#;
        assert_eq!(parse_mb_release_group_search(body), None);
    }

    #[test]
    fn cover_art_url_points_at_release_group_front_image() {
        assert_eq!(
            cover_art_url("f1afec0b-26dd-3db5-9aa1-c91229a74a24"),
            "https://coverartarchive.org/release-group/f1afec0b-26dd-3db5-9aa1-c91229a74a24/front-250"
        );
    }
}
