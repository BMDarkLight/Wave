// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The contract a remote library backend fulfils.

/// One track of a remote catalog, normalized so the store never needs to know
/// which backend produced it.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
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
