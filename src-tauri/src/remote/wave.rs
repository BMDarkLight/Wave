// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Another Wave, paired with this one, as a source of music. It reads only
//! what that Wave chose to share, through its `/wave/` API, and sends each
//! song's audio fingerprint so songs this library already has are not
//! brought in twice.
//!
//! The address and secret are looked up for every request rather than kept,
//! so a device that moved to another address keeps working as soon as
//! discovery sees it there.

use std::io::Read;

use reqwest::blocking::Response;
use serde::{Deserialize, Serialize};

use super::backend::{total_size, CatalogPage, RemoteBackend, RemoteError, RemoteTrackMeta};
use super::gate::NetworkGate;
use super::http::stream_client;
use crate::peers::api::SharedTrack;
use crate::peers::client::{self, CallError};
use crate::peers::PeerRecord;

const MAX_ART_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WaveConfig {
    pub peer_id: String,
}

pub struct WaveBackend {
    config: WaveConfig,
    gate: &'static NetworkGate,
}

/// `<id>.<extension>`, which is all a song needs to be found again.
fn part_key(track: &SharedTrack) -> String {
    format!("{}.{}", track.id, track.extension)
}

fn split_part_key(part_key: &str) -> (&str, &str) {
    part_key.rsplit_once('.').unwrap_or((part_key, "mp3"))
}

pub fn to_meta(track: SharedTrack) -> RemoteTrackMeta {
    RemoteTrackMeta {
        part_key: part_key(&track),
        art_ref: track.art.then(|| track.id.clone()),
        remote_id: track.id,
        title: track.title,
        artist: track.artist,
        album_artist: track.album_artist,
        album: track.album,
        track_number: track.track_number,
        disc_number: track.disc_number,
        year: track.year,
        genre: track.genre,
        duration_seconds: track.duration,
        size: track.size,
        extension: track.extension,
        updated_at: None,
        audio_fingerprint: track.fingerprint,
    }
}

impl WaveBackend {
    pub fn new(config: WaveConfig) -> Self {
        Self {
            config,
            gate: NetworkGate::global(),
        }
    }

    /// The pairing as it stands now, and this Wave's own id.
    fn peer(&self) -> Result<(PeerRecord, String), RemoteError> {
        crate::peers::lookup(&self.config.peer_id)
            .ok_or_else(|| RemoteError::Unreachable("That device is no longer paired".into()))
    }

    fn unreachable(&self) -> RemoteError {
        RemoteError::Unreachable("The other device isn't on the network".into())
    }

    /// The other side forgot this one: forget it here too.
    fn forgotten(&self) -> RemoteError {
        crate::peers::forget_locally(&self.config.peer_id);
        RemoteError::Unauthorized
    }

    fn get(&self, url: String, range: Option<String>) -> Result<Response, RemoteError> {
        self.gate.check()?;
        let (peer, own_id) = self.peer()?;
        let mut request = client::signed(stream_client().get(url), &own_id, &peer);
        if let Some(range) = range {
            request = request.header("Range", range);
        }
        let response = request.send().map_err(|_| self.unreachable())?;
        match response.status().as_u16() {
            200 | 206 => Ok(response),
            403 => Err(self.forgotten()),
            code => Err(RemoteError::Http(code)),
        }
    }
}

impl RemoteBackend for WaveBackend {
    fn kind(&self) -> &'static str {
        "wave"
    }

    fn catalog_page(&self, cursor: usize) -> Result<CatalogPage, RemoteError> {
        self.gate.check()?;
        let (peer, own_id) = self.peer()?;
        let page = client::catalog(&peer, &own_id, cursor).map_err(|e| match e {
            CallError::Forgotten => self.forgotten(),
            CallError::Refused(code) => RemoteError::Http(code),
            CallError::Unreachable => self.unreachable(),
        })?;
        Ok(CatalogPage {
            tracks: page.tracks.into_iter().map(to_meta).collect(),
            total: Some(page.total),
            next: page.next,
        })
    }

    fn open_bytes(
        &self,
        part_key: &str,
        offset: u64,
    ) -> Result<Box<dyn super::backend::ByteStream>, RemoteError> {
        let (peer, _) = self.peer()?;
        let (id, extension) = split_part_key(part_key);
        let url = client::media_url(&peer.address, id, extension);
        Ok(Box::new(self.get(url, Some(format!("bytes={offset}-")))?))
    }

    fn check_reachable(&self) -> Result<(), RemoteError> {
        self.gate.check()?;
        let (peer, _) = self.peer()?;
        match client::hello(&peer.address) {
            Ok(hello) if hello.id == peer.id => Ok(()),
            _ => Err(self.unreachable()),
        }
    }

    fn fetch_artwork(&self, art_ref: &str) -> Result<Vec<u8>, RemoteError> {
        let (peer, _) = self.peer()?;
        let response = self.get(client::art_url(&peer.address, art_ref), None)?;
        let mut bytes = Vec::new();
        response
            .take(MAX_ART_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|_| self.unreachable())?;
        Ok(bytes)
    }

    fn content_length(&self, part_key: &str) -> Result<u64, RemoteError> {
        let (peer, _) = self.peer()?;
        let (id, extension) = split_part_key(part_key);
        let response = self.get(
            client::media_url(&peer.address, id, extension),
            Some("bytes=0-0".to_string()),
        )?;
        total_size(&response).ok_or_else(|| RemoteError::Parse("no size".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_songs_become_remote_rows_with_their_fingerprint() {
        let meta = to_meta(SharedTrack {
            id: "t.1".into(),
            title: "Song".into(),
            artist: "Band".into(),
            album_artist: None,
            album: "Album".into(),
            track_number: Some(3),
            disc_number: None,
            year: Some(1999),
            genre: None,
            duration: Some(201.5),
            size: 4000,
            extension: "flac".into(),
            fingerprint: Some("a1:abc".into()),
            art: true,
        });
        assert_eq!(meta.remote_id, "t.1");
        assert_eq!(meta.part_key, "t.1.flac");
        assert_eq!(split_part_key(&meta.part_key), ("t.1", "flac"));
        assert_eq!(meta.art_ref.as_deref(), Some("t.1"));
        assert_eq!(meta.audio_fingerprint.as_deref(), Some("a1:abc"));
    }
}
