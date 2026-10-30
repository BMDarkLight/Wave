// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The playlist each paired Wave gets: every song it shares, in its order,
//! kept while it is offline. Where this library already has a song, the
//! playlist holds that copy, so it plays from here.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};

use crate::library::{now_timestamp, Library};

type SongKey = (String, String, String);

fn sql_error(what: &'static str) -> impl Fn(rusqlite::Error) -> String {
    move |error| format!("Failed to {what}: {error}")
}

impl Library {
    /// Make or refresh the playlist for the Wave behind `source_id`. Returns
    /// the playlist's id.
    pub fn sync_device_playlist(&self, source_id: &str, name: &str) -> Result<String, String> {
        let existing: Option<String> = self
            .read_connection()
            .query_row(
                "SELECT id FROM playlists WHERE peer_source_id = ?1",
                [source_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error("find a device playlist"))?;
        let playlist_id = match existing {
            Some(id) => id,
            None => {
                let created = self.create_playlist_for_import(name)?;
                self.write_connection()
                    .execute(
                        "UPDATE playlists SET peer_source_id = ?2 WHERE id = ?1",
                        params![created.id, source_id],
                    )
                    .map_err(sql_error("mark a device playlist"))?;
                created.id
            }
        };
        let entries = self.device_entries(source_id)?;
        let mut connection = self.write_connection();
        let tx = connection
            .transaction()
            .map_err(sql_error("begin a device playlist update"))?;
        tx.execute(
            "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
            [&playlist_id],
        )
        .map_err(sql_error("clear a device playlist"))?;
        let now = now_timestamp();
        let mut added = std::collections::HashSet::new();
        for track_id in entries {
            // A song on the device twice, or two of its songs that are one
            // song here, is listed once.
            if !added.insert(track_id.clone()) {
                continue;
            }
            tx.execute(
                "INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![playlist_id, track_id, added.len() as i64 - 1, now],
            )
            .map_err(sql_error("fill a device playlist"))?;
        }
        tx.execute(
            "UPDATE playlists SET updated_at = ?2 WHERE id = ?1",
            params![playlist_id, now],
        )
        .map_err(sql_error("touch a device playlist"))?;
        tx.commit().map_err(sql_error("save a device playlist"))?;
        Ok(playlist_id)
    }

    /// The device's songs in its order, each as the copy that plays: this
    /// library's own where it has the song, the device's otherwise. The same
    /// rule decides which copy the library shows.
    fn device_entries(&self, source_id: &str) -> Result<Vec<String>, String> {
        let connection = self.read_connection();
        let mut by_print: HashMap<String, String> = HashMap::new();
        let mut by_key: HashMap<SongKey, Vec<(String, Option<String>)>> = HashMap::new();
        let mut statement = connection
            .prepare(
                "SELECT id, lower(trim(artist)), lower(trim(album)), lower(trim(title)),
                        audio_fingerprint
                 FROM tracks WHERE source_state IS NULL OR source_state = 'downloaded'",
            )
            .map_err(sql_error("read local songs"))?;
        let locals = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (row.get(1)?, row.get(2)?, row.get(3)?),
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(sql_error("read local songs"))?;
        for local in locals {
            let (id, key, print): (String, SongKey, Option<String>) =
                local.map_err(sql_error("read a local song"))?;
            if let Some(print) = &print {
                by_print.entry(print.clone()).or_insert_with(|| id.clone());
            }
            by_key.entry(key).or_default().push((id, print));
        }

        let mut statement = connection
            .prepare(
                "SELECT id, lower(trim(artist)), lower(trim(album)), lower(trim(title)),
                        audio_fingerprint
                 FROM tracks
                 WHERE remote_source_id = ?1 AND remote_missing = 0
                 ORDER BY rowid",
            )
            .map_err(sql_error("read a device's songs"))?;
        let rows = statement
            .query_map([source_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (row.get(1)?, row.get(2)?, row.get(3)?),
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(sql_error("read a device's songs"))?;
        let mut entries = Vec::new();
        for row in rows {
            let (id, key, print): (String, SongKey, Option<String>) =
                row.map_err(sql_error("read a device's song"))?;
            let here = print
                .as_ref()
                .and_then(|p| by_print.get(p).cloned())
                .or_else(|| {
                    // Without the audio to compare, a song of the same name
                    // counts as this one, as it does in the library.
                    let same_name = by_key.get(&key)?;
                    let unsure = print.is_none() || same_name.iter().any(|(_, p)| p.is_none());
                    unsure.then(|| same_name[0].0.clone())
                });
            entries.push(here.unwrap_or(id));
        }
        Ok(entries)
    }

    /// Remove a forgotten Wave's playlist.
    pub fn delete_device_playlist(&self, source_id: &str) -> Result<(), String> {
        let connection = self.write_connection();
        connection
            .execute(
                "DELETE FROM playlist_tracks WHERE playlist_id IN
                    (SELECT id FROM playlists WHERE peer_source_id = ?1)",
                [source_id],
            )
            .map_err(sql_error("empty a device playlist"))?;
        connection
            .execute(
                "DELETE FROM playlists WHERE peer_source_id = ?1",
                [source_id],
            )
            .map(drop)
            .map_err(sql_error("remove a device playlist"))
    }

    /// The device a playlist belongs to, by its remote source id.
    pub fn device_of_playlist(&self, playlist_id: &str) -> Result<Option<String>, String> {
        self.read_connection()
            .query_row(
                "SELECT peer_source_id FROM playlists WHERE id = ?1",
                [playlist_id],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(sql_error("look up a playlist's device"))
    }
}

#[cfg(test)]
mod tests {
    use crate::library::Library;
    use crate::remote::backend::RemoteTrackMeta;
    use rusqlite::params;

    fn library() -> Library {
        let dir = std::env::temp_dir().join(format!("wave-devices-{}", uuid::Uuid::new_v4()));
        Library::new_with_path(&dir.join("library.db")).unwrap()
    }

    fn song(id: &str, title: &str, fingerprint: Option<&str>) -> RemoteTrackMeta {
        RemoteTrackMeta {
            remote_id: id.into(),
            title: title.into(),
            artist: "Band".into(),
            album_artist: None,
            album: "Album".into(),
            track_number: None,
            disc_number: None,
            year: None,
            genre: None,
            duration_seconds: Some(100.0),
            size: 10,
            extension: "mp3".into(),
            part_key: format!("{id}.mp3"),
            art_ref: None,
            updated_at: None,
            audio_fingerprint: fingerprint.map(str::to_string),
        }
    }

    fn sync(library: &Library, source: &str, songs: &[RemoteTrackMeta]) {
        let mut walk = library.begin_remote_walk(source).unwrap();
        library
            .upsert_remote_page(&mut walk, "wave", songs)
            .unwrap();
        library.finish_remote_walk(walk).unwrap();
        library.recompute_remote_visibility(true).unwrap();
    }

    fn titles(library: &Library, playlist: &str) -> Vec<(String, bool)> {
        let connection = library.read_connection();
        let mut statement = connection
            .prepare(
                "SELECT t.title, t.remote_source_id IS NULL FROM playlist_tracks pt
                 JOIN tracks t ON t.id = pt.track_id
                 WHERE pt.playlist_id = ?1 ORDER BY pt.position",
            )
            .unwrap();
        let rows = statement
            .query_map([playlist], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    fn local(library: &Library, id: &str, title: &str, fingerprint: &str) {
        library
            .write_connection()
            .execute(
                "INSERT INTO tracks (id, path, name, title, artist, album, format, file_size,
                                     modified_at, indexed_at, audio_fingerprint)
                 VALUES (?1, ?2, ?1, ?3, 'Band', 'Album', 'MP3', 1, 1, 1, ?4)",
                params![id, format!("/music/{id}.mp3"), title, fingerprint],
            )
            .unwrap();
    }

    #[test]
    fn the_playlist_lists_what_the_device_shares_and_plays_what_is_here_from_here() {
        let library = library();
        local(&library, "mine", "Shared Song", "a1:same");
        let source = library
            .insert_remote_source("wave", "phone", "Wave on Android", "{}")
            .unwrap();
        sync(
            &library,
            &source,
            &[
                song("1", "First", None),
                song("2", "Shared Song (phone tags)", Some("a1:same")),
                song("3", "Third", None),
            ],
        );
        let playlist = library
            .sync_device_playlist(&source, "Wave on Android")
            .unwrap();
        assert_eq!(
            titles(&library, &playlist),
            [
                ("First".to_string(), false),
                ("Shared Song".to_string(), true),
                ("Third".to_string(), false),
            ]
        );
        let info = library
            .list_playlists(None)
            .unwrap()
            .into_iter()
            .find(|p| p.id == playlist)
            .unwrap();
        assert_eq!(info.peer_source_id.as_deref(), Some(source.as_str()));
        assert_eq!(info.name, "Wave on Android");
    }

    #[test]
    fn a_later_sync_updates_the_same_playlist() {
        let library = library();
        let source = library
            .insert_remote_source("wave", "phone", "Wave on Android", "{}")
            .unwrap();
        sync(
            &library,
            &source,
            &[song("1", "One", None), song("2", "Two", None)],
        );
        let first = library
            .sync_device_playlist(&source, "Wave on Android")
            .unwrap();
        sync(&library, &source, &[song("2", "Two", None)]);
        let second = library
            .sync_device_playlist(&source, "Wave on Android")
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(titles(&library, &second), [("Two".to_string(), false)]);
    }

    #[test]
    fn deleting_removes_the_playlist() {
        let library = library();
        let source = library
            .insert_remote_source("wave", "phone", "Wave on Android", "{}")
            .unwrap();
        sync(&library, &source, &[song("1", "One", None)]);
        let playlist = library
            .sync_device_playlist(&source, "Wave on Android")
            .unwrap();
        library.delete_device_playlist(&source).unwrap();
        assert!(library
            .list_playlists(None)
            .unwrap()
            .iter()
            .all(|p| p.id != playlist));
    }
}
