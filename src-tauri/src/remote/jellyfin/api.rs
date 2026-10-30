// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Jellyfin and Emby JSON, and how an audio item becomes a remote track.

use crate::remote::backend::{unix_seconds, RemoteError, RemoteTrackMeta};
use serde::Deserialize;

/// Run times are in 100 nanosecond ticks.
const TICKS_PER_SECOND: f64 = 10_000_000.0;

/// `/System/Info/Public`, which answers without signing in.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PublicInfo {
    pub id: String,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub product_name: Option<String>,
}

impl PublicInfo {
    /// Jellyfin names itself; Emby leaves the product out.
    pub fn product(&self) -> &'static str {
        match self.product_name.as_deref() {
            Some(name) if name.to_ascii_lowercase().contains("jellyfin") => "Jellyfin",
            _ => "Emby",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthUser {
    pub id: String,
}

/// `/Users/AuthenticateByName`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthResult {
    pub user: AuthUser,
    pub access_token: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct View {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub collection_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ViewList {
    #[serde(default)]
    items: Vec<View>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct MediaSource {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    container: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Item {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    album_id: Option<String>,
    #[serde(default)]
    album_artist: Option<String>,
    #[serde(default)]
    artists: Vec<String>,
    #[serde(default)]
    index_number: Option<i32>,
    #[serde(default)]
    parent_index_number: Option<i32>,
    #[serde(default)]
    production_year: Option<i32>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    run_time_ticks: Option<i64>,
    #[serde(default)]
    container: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    media_sources: Vec<MediaSource>,
    #[serde(default)]
    image_tags: std::collections::HashMap<String, String>,
    #[serde(default)]
    album_primary_image_tag: Option<String>,
    #[serde(default)]
    date_created: Option<String>,
}

/// One page of `/Items`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemPage {
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub total_record_count: Option<usize>,
}

fn parse<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, RemoteError> {
    serde_json::from_str(body).map_err(|e| RemoteError::Parse(e.to_string()))
}

pub fn parse_public_info(body: &str) -> Result<PublicInfo, RemoteError> {
    parse(body)
}

pub fn parse_auth(body: &str) -> Result<AuthResult, RemoteError> {
    parse(body)
}

/// The user's music libraries, in the server's order.
pub fn parse_music_views(body: &str) -> Result<Vec<View>, RemoteError> {
    let list: ViewList = parse(body)?;
    Ok(list
        .items
        .into_iter()
        .filter(|view| view.collection_type.as_deref() == Some("music"))
        .collect())
}

pub fn parse_items(body: &str) -> Result<ItemPage, RemoteError> {
    parse(body)
}

/// The file's extension, from its path when the server shows it, else from
/// the container ("flac", or the first of "mov,mp4,m4a").
fn extension(item: &Item, source: Option<&MediaSource>) -> String {
    let from_path = source
        .and_then(|s| s.path.as_deref())
        .or(item.path.as_deref())
        .and_then(|path| path.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| {
            (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())
        });
    if let Some(ext) = from_path {
        return ext;
    }
    let container = source
        .and_then(|s| s.container.as_deref())
        .or(item.container.as_deref())
        .and_then(|c| c.split(',').next())
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty());
    match container.as_deref() {
        Some("mpeg") | None => "mp3".to_string(),
        Some("mov") | Some("mp4") => "m4a".to_string(),
        Some(other) => other.to_string(),
    }
}

/// The track an audio item becomes, or `None` for one with no id.
pub fn item_to_meta(item: &Item) -> Option<RemoteTrackMeta> {
    if item.id.trim().is_empty() {
        return None;
    }
    let source = item.media_sources.first();
    let artist = item
        .artists
        .first()
        .cloned()
        .or_else(|| item.album_artist.clone())
        .filter(|a| !a.trim().is_empty())
        .unwrap_or_else(|| "Unknown Artist".to_string());
    // The item's own picture, else its album's.
    let art_ref = if item.image_tags.contains_key("Primary") {
        Some(item.id.clone())
    } else if item.album_primary_image_tag.is_some() {
        item.album_id.clone()
    } else {
        None
    };
    Some(RemoteTrackMeta {
        audio_fingerprint: None,
        remote_id: item.id.clone(),
        title: item
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Unknown Title".to_string()),
        artist,
        album_artist: item.album_artist.clone().filter(|a| !a.trim().is_empty()),
        album: item
            .album
            .clone()
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| "Unknown Album".to_string()),
        track_number: item.index_number,
        disc_number: item.parent_index_number,
        year: item.production_year,
        genre: item.genres.first().cloned(),
        duration_seconds: item
            .run_time_ticks
            .filter(|ticks| *ticks > 0)
            .map(|ticks| ticks as f64 / TICKS_PER_SECOND),
        size: source.and_then(|s| s.size).unwrap_or(0),
        extension: extension(item, source),
        part_key: item.id.clone(),
        art_ref,
        updated_at: item.date_created.as_deref().and_then(unix_seconds),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_info_tells_jellyfin_from_emby() {
        let jellyfin = parse_public_info(
            r#"{"Id":"abc","ServerName":"Home","ProductName":"Jellyfin Server","Version":"10.10.3"}"#,
        )
        .unwrap();
        assert_eq!(jellyfin.id, "abc");
        assert_eq!(jellyfin.server_name.as_deref(), Some("Home"));
        assert_eq!(jellyfin.product(), "Jellyfin");
        let emby = parse_public_info(r#"{"Id":"def","ServerName":"Den","Version":"4.8"}"#).unwrap();
        assert_eq!(emby.product(), "Emby");
    }

    #[test]
    fn auth_result_reads_user_and_token() {
        let auth = parse_auth(
            r#"{"User":{"Id":"u1","Name":"alice"},"AccessToken":"tok","ServerId":"abc","SessionInfo":{}}"#,
        )
        .unwrap();
        assert_eq!(auth.user.id, "u1");
        assert_eq!(auth.access_token, "tok");
    }

    #[test]
    fn only_music_views_are_kept() {
        let views = parse_music_views(
            r#"{"Items":[
              {"Id":"m","Name":"Music","CollectionType":"music"},
              {"Id":"f","Name":"Films","CollectionType":"movies"},
              {"Id":"x","Name":"Mixed"}
            ],"TotalRecordCount":3}"#,
        )
        .unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].id, "m");
        assert_eq!(views[0].name.as_deref(), Some("Music"));
    }

    #[test]
    fn an_audio_item_becomes_a_track() {
        let page = parse_items(
            r#"{"Items":[{
              "Id":"i1","Name":"Song","Album":"Record","AlbumId":"al1",
              "AlbumArtist":"Band","Artists":["Singer","Band"],
              "IndexNumber":3,"ParentIndexNumber":1,"ProductionYear":2001,
              "Genres":["Rock","Pop"],"RunTimeTicks":2450000000,
              "Container":"flac","DateCreated":"2024-03-01T18:22:05.1234567Z",
              "MediaSources":[{"Path":"/music/Band/Record/03 Song.FLAC","Container":"flac","Size":31415}],
              "ImageTags":{},"AlbumPrimaryImageTag":"t"
            }],"TotalRecordCount":40}"#,
        )
        .unwrap();
        assert_eq!(page.total_record_count, Some(40));
        let meta = item_to_meta(&page.items[0]).unwrap();
        assert_eq!(meta.remote_id, "i1");
        assert_eq!(meta.title, "Song");
        assert_eq!(meta.artist, "Singer");
        assert_eq!(meta.album_artist.as_deref(), Some("Band"));
        assert_eq!(meta.album, "Record");
        assert_eq!(meta.track_number, Some(3));
        assert_eq!(meta.disc_number, Some(1));
        assert_eq!(meta.year, Some(2001));
        assert_eq!(meta.genre.as_deref(), Some("Rock"));
        assert_eq!(meta.duration_seconds, Some(245.0));
        assert_eq!(meta.size, 31415);
        assert_eq!(meta.extension, "flac");
        assert_eq!(meta.part_key, "i1");
        assert_eq!(
            meta.art_ref.as_deref(),
            Some("al1"),
            "falls back to the album"
        );
        assert_eq!(meta.updated_at, Some(1_709_317_325));
    }

    #[test]
    fn sparse_items_get_fallbacks() {
        let page = parse_items(
            r#"{"Items":[
              {"Id":"i2","Container":"mov,mp4,m4a,3gp","ImageTags":{"Primary":"x"}},
              {"Id":"","Name":"No id"}
            ]}"#,
        )
        .unwrap();
        let meta = item_to_meta(&page.items[0]).unwrap();
        assert_eq!(meta.title, "Unknown Title");
        assert_eq!(meta.artist, "Unknown Artist");
        assert_eq!(meta.album, "Unknown Album");
        assert_eq!(meta.extension, "m4a");
        assert_eq!(meta.art_ref.as_deref(), Some("i2"));
        assert_eq!(meta.duration_seconds, None);
        assert!(item_to_meta(&page.items[1]).is_none());
    }
}
