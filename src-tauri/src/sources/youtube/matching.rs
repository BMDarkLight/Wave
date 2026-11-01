// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Picks the YouTube video that is a catalog song, not a live take, a cover
//! or an hour-long loop of it.
//!
//! Length decides most of it: the label's own upload runs within a second or
//! two of the catalog's. Then the title's words, the artist, and YouTube's
//! auto-generated "Topic" channels, which carry the label's audio.

use super::Candidate;

/// A candidate further than this from the catalog length is another
/// recording, or the song with an intro or outro bolted on.
const MAX_LENGTH_GAP_SECONDS: f64 = 10.0;
/// Below this nothing is close enough to play in the song's place.
const MIN_SCORE: f64 = 45.0;
/// Words that mark another recording, unless the catalog title has them too.
const OTHER_VERSIONS: [&str; 14] = [
    "live",
    "cover",
    "remix",
    "karaoke",
    "instrumental",
    "acoustic",
    "nightcore",
    "slowed",
    "sped",
    "reverb",
    "8d",
    "hour",
    "acapella",
    "reaction",
];

/// What the catalog says the song is.
#[derive(Debug, Clone, Copy)]
pub struct Wanted<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub duration_seconds: Option<f64>,
}

/// A song to find later, on another thread.
#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    pub title: String,
    pub artist: String,
    pub duration_seconds: Option<f64>,
}

impl Song {
    pub fn wanted(&self) -> Wanted<'_> {
        Wanted {
            title: &self.title,
            artist: &self.artist,
            duration_seconds: self.duration_seconds,
        }
    }
}

/// The search query that finds `wanted` best.
pub fn query(wanted: &Wanted) -> String {
    format!("{} {}", wanted.artist, wanted.title)
}

/// The candidate that is the song, if any is.
#[cfg(test)]
fn best<'a>(wanted: &Wanted, candidates: &'a [Candidate]) -> Option<&'a Candidate> {
    ranked(wanted, candidates).into_iter().next()
}

/// Every candidate that could be the song, most likely first.
pub fn ranked<'a>(wanted: &Wanted, candidates: &'a [Candidate]) -> Vec<&'a Candidate> {
    let mut scored: Vec<(&Candidate, f64)> = candidates
        .iter()
        .filter_map(|c| score(wanted, c).map(|s| (c, s)))
        .filter(|&(_, s)| s >= MIN_SCORE)
        .collect();
    // Stable, so ties keep search's own order of relevance.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.into_iter().map(|(c, _)| c).collect()
}

/// Whether a stream of `seconds` can be the song: the last check, for
/// results whose length search didn't report.
pub fn length_fits(wanted: &Wanted, seconds: Option<f64>) -> bool {
    match (wanted.duration_seconds, seconds) {
        (Some(want), Some(have)) => (want - have).abs() <= MAX_LENGTH_GAP_SECONDS,
        _ => true,
    }
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// How well `candidate` fits, or `None` when it plainly is something else.
fn score(wanted: &Wanted, candidate: &Candidate) -> Option<f64> {
    let mut score = 0.0;

    match (wanted.duration_seconds, candidate.duration_seconds) {
        (Some(want), Some(have)) => {
            let gap = (want - have).abs();
            if gap > MAX_LENGTH_GAP_SECONDS {
                return None;
            }
            score += 30.0 - 3.0 * gap;
        }
        // Without a length to compare, the words have to carry it.
        _ => score += 5.0,
    }

    let title_words = words(&candidate.title);
    let wanted_words = words(wanted.title);
    if !wanted_words.is_empty() {
        let found = wanted_words
            .iter()
            .filter(|w| title_words.contains(w))
            .count();
        score += 40.0 * found as f64 / wanted_words.len() as f64;
    }

    let channel = candidate.channel.as_deref().unwrap_or_default();
    let artist_words = words(wanted.artist);
    let channel_words = words(channel);
    let names_artist = !artist_words.is_empty()
        && artist_words
            .iter()
            .all(|w| channel_words.contains(w) || title_words.contains(w));
    if names_artist {
        score += 20.0;
    }
    if channel.ends_with(" - Topic") {
        score += 15.0;
    }

    for marker in OTHER_VERSIONS {
        if title_words.iter().any(|w| w == marker) && !wanted_words.iter().any(|w| w == marker) {
            score -= 40.0;
        }
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, title: &str, channel: &str, duration: Option<f64>) -> Candidate {
        Candidate {
            id: id.into(),
            title: title.into(),
            channel: Some(channel.into()),
            duration_seconds: duration,
        }
    }

    const GET_LUCKY: Wanted = Wanted {
        title: "Get Lucky (feat. Pharrell Williams and Nile Rodgers)",
        artist: "Daft Punk",
        duration_seconds: Some(248.0),
    };

    #[test]
    fn takes_the_topic_upload_over_a_live_take_and_a_loop() {
        let found = [
            candidate(
                "live0000000",
                "Daft Punk - Get Lucky (Live at the Grammys)",
                "Daft Punk",
                Some(251.0),
            ),
            candidate("loop0000000", "Get Lucky 1 hour", "Loops", Some(3600.0)),
            candidate(
                "topic000000",
                "Get Lucky (feat. Pharrell Williams and Nile Rodgers)",
                "Daft Punk - Topic",
                Some(248.0),
            ),
        ];
        assert_eq!(
            best(&GET_LUCKY, &found).map(|c| c.id.as_str()),
            Some("topic000000")
        );
    }

    #[test]
    fn a_video_with_a_long_intro_is_not_the_song() {
        let found = [candidate(
            "video000000",
            "Daft Punk - Get Lucky (Official Video)",
            "Daft Punk",
            Some(369.0),
        )];
        assert_eq!(best(&GET_LUCKY, &found), None);
    }

    #[test]
    fn a_remix_counts_when_the_catalog_asked_for_one() {
        let remix = Wanted {
            title: "Get Lucky (Remix)",
            artist: "Daft Punk",
            duration_seconds: Some(300.0),
        };
        let found = [candidate(
            "remix000000",
            "Get Lucky (Remix)",
            "Daft Punk - Topic",
            Some(301.0),
        )];
        assert!(best(&remix, &found).is_some());
        assert!(best(&GET_LUCKY, &found).is_none());
    }

    #[test]
    fn nothing_close_means_nothing_played() {
        let found = [candidate(
            "other000000",
            "Something Else Entirely",
            "Someone",
            Some(248.0),
        )];
        assert_eq!(best(&GET_LUCKY, &found), None);
        assert_eq!(best(&GET_LUCKY, &[]), None);
    }

    #[test]
    fn without_lengths_the_title_and_artist_decide() {
        let wanted = Wanted {
            duration_seconds: None,
            ..GET_LUCKY
        };
        let found = [candidate(
            "nolength000",
            "Get Lucky (feat. Pharrell Williams and Nile Rodgers)",
            "Daft Punk",
            None,
        )];
        assert!(best(&wanted, &found).is_some());
    }
}
