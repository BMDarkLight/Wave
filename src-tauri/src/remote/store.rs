// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Remote catalogs in the `tracks` table.
//!
//! A remote row is never deleted: playlists, favorites and listen history hang
//! off its id. A song that leaves the server becomes a tombstone
//! (`remote_missing = 1`), and one that comes back under a new server id takes
//! its old row over. Which copy of a song shows in the library is stored in
//! `remote_visible`, recomputed here rather than in every browse query.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, params_from_iter, OptionalExtension, Transaction};
use uuid::Uuid;

use crate::library::{now_timestamp, Library};
use crate::remote::backend::RemoteTrackMeta;
use crate::remote::uri::RemoteUri;

/// Rows per `IN (...)` list, well under SQLite's variable limit.
const BATCH: usize = 500;

/// Album artist as albums are grouped: the tag, or the track artist without one.
const ALBUM_ARTIST_SQL: &str = "COALESCE(NULLIF(TRIM(album_artist), ''), artist)";

/// Normalized (artist, album, title), compared the way `find_local_match` does.
type SongKey = (String, String, String);

#[derive(Debug, Clone)]
pub struct RemoteSourceRow {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub config_json: String,
    #[allow(dead_code)]
    pub created_at: i64,
    pub last_sync_at: Option<i64>,
    pub last_sync_error: Option<String>,
    pub track_count: i64,
}

/// A catalog walk in progress. Rows are upserted as pages arrive; only
/// [`Library::finish_remote_walk`] tombstones, so dropping a walk early
/// changes nothing beyond the upserts.
#[derive(Debug)]
pub struct RemoteWalk {
    source_id: String,
    seen: HashSet<String>,
    upserted: usize,
    revived: usize,
    rekeyed: Vec<RemoteUri>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WalkSummary {
    pub upserted: usize,
    pub tombstoned: usize,
    pub revived: usize,
    /// Old URIs of rows that were re-keyed while holding cached audio. Their
    /// cache state is cleared; the caller deletes the orphaned files.
    pub rekeyed: Vec<RemoteUri>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RemoteStreamInfo {
    pub uri: RemoteUri,
    pub provider: String,
    pub part_key: String,
    pub size: u64,
    pub extension: String,
    pub cache_state: Option<String>,
    pub source_state: String,
    pub path_if_downloaded: Option<String>,
}

enum Outcome {
    Inserted,
    Updated,
    Revived,
}

/// A row taken over under a new server id.
struct Rekey {
    old_remote_id: String,
    cache_state: Option<String>,
    downloaded: bool,
}

struct RowMatch {
    id: String,
    outcome: Outcome,
    rekey: Option<Rekey>,
}

fn sql_error(what: &str) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |error| format!("Failed to {what}: {error}")
}

fn in_list(len: usize) -> String {
    vec!["?"; len].join(",")
}

fn durations_match(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).abs() <= 2.0,
        _ => false,
    }
}

fn set_visible(tx: &Transaction<'_>, ids: &[String], visible: bool) -> Result<(), String> {
    for chunk in ids.chunks(BATCH) {
        tx.execute(
            &format!(
                "UPDATE tracks SET remote_visible = {} WHERE id IN ({})",
                visible as i32,
                in_list(chunk.len())
            ),
            params_from_iter(chunk),
        )
        .map_err(sql_error("update remote visibility"))?;
    }
    Ok(())
}

/// The row already holding this server id, or a tombstone (or a row this walk
/// has not reached yet) that carries the same song under an old id.
fn find_row_to_update(
    tx: &Transaction<'_>,
    source_id: &str,
    meta: &RemoteTrackMeta,
    seen: &HashSet<String>,
) -> Result<Option<RowMatch>, String> {
    let exact: Option<(String, bool)> = tx
        .query_row(
            "SELECT id, remote_missing FROM tracks
             WHERE remote_source_id = ?1 AND source_id = ?2",
            params![source_id, meta.remote_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_error("look up remote track"))?;
    if let Some((id, missing)) = exact {
        let outcome = if missing {
            Outcome::Revived
        } else {
            Outcome::Updated
        };
        return Ok(Some(RowMatch {
            id,
            outcome,
            rekey: None,
        }));
    }

    // Missing rows first. A row that is still listed but not yet seen in this
    // walk counts too: the server re-keyed it, and the old id will not show up.
    let mut statement = tx
        .prepare(
            "SELECT id, source_id, remote_missing, duration_seconds, cache_state,
                    source_state = 'downloaded'
             FROM tracks
             WHERE remote_source_id = ?1
               AND lower(trim(artist)) = lower(trim(?2))
               AND lower(trim(album)) = lower(trim(?3))
               AND lower(trim(title)) = lower(trim(?4))
             ORDER BY remote_missing DESC, id",
        )
        .map_err(sql_error("prepare remote re-match"))?;
    let candidates = statement
        .query_map(
            params![source_id, meta.artist, meta.album, meta.title],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<f64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<bool>>(5)?.unwrap_or(false),
                ))
            },
        )
        .map_err(sql_error("query remote re-match"))?;
    for candidate in candidates {
        let (id, old_remote_id, missing, duration, cache_state, downloaded) =
            candidate.map_err(sql_error("read remote re-match"))?;
        if !missing && seen.contains(&old_remote_id) {
            continue;
        }
        if durations_match(duration, meta.duration_seconds) {
            return Ok(Some(RowMatch {
                id,
                outcome: Outcome::Revived,
                rekey: Some(Rekey {
                    old_remote_id,
                    cache_state,
                    downloaded,
                }),
            }));
        }
    }
    Ok(None)
}

fn upsert_one(
    tx: &Transaction<'_>,
    source_id: &str,
    provider: &str,
    meta: &RemoteTrackMeta,
    seen: &HashSet<String>,
    now: i64,
) -> Result<(Outcome, Option<RemoteUri>), String> {
    let path = RemoteUri::new(source_id, &meta.remote_id).to_path();
    let name = format!("{}.{}", meta.title, meta.extension);
    let file_size = i64::try_from(meta.size).unwrap_or(i64::MAX);

    if let Some(RowMatch { id, outcome, rekey }) = find_row_to_update(tx, source_id, meta, seen)? {
        // The cache file is named by the old id, so a re-keyed row starts
        // uncached; its old file is reported for the caller to delete.
        let reset_cache = rekey.as_ref().is_some_and(|r| !r.downloaded);
        let orphaned = rekey
            .filter(|r| !r.downloaded && r.cache_state.is_some())
            .map(|r| RemoteUri::new(source_id, &r.old_remote_id));
        // A download is the user's own file now: its path, name, format, size
        // and tags describe that file and stay as they are, so a sync never
        // undoes their edits. Otherwise a new part or size means new bytes
        // behind the same URI, so the cache starts over. A size of 0 or no
        // duration means the catalog doesn't say (Audius, Kodi, WebDAV), so the
        // value learned when the track first played is kept.
        tx.execute(
            "UPDATE tracks SET
                path = CASE WHEN source_state = 'downloaded' THEN path ELSE ?2 END,
                name = CASE WHEN source_state = 'downloaded' THEN name ELSE ?3 END,
                title = CASE WHEN source_state = 'downloaded' THEN title ELSE ?4 END,
                artist = CASE WHEN source_state = 'downloaded' THEN artist ELSE ?5 END,
                album = CASE WHEN source_state = 'downloaded' THEN album ELSE ?6 END,
                album_artist = CASE WHEN source_state = 'downloaded'
                                    THEN album_artist ELSE ?7 END,
                genre = CASE WHEN source_state = 'downloaded' THEN genre ELSE ?8 END,
                year = CASE WHEN source_state = 'downloaded' THEN year ELSE ?9 END,
                track_number = CASE WHEN source_state = 'downloaded'
                                    THEN track_number ELSE ?10 END,
                disc_number = CASE WHEN source_state = 'downloaded'
                                   THEN disc_number ELSE ?11 END,
                format = CASE WHEN source_state = 'downloaded' THEN format ELSE ?12 END,
                duration_seconds = CASE WHEN source_state = 'downloaded' OR ?13 IS NULL
                                        THEN duration_seconds ELSE ?13 END,
                file_size = CASE WHEN source_state = 'downloaded' OR ?14 = 0
                                 THEN file_size ELSE ?14 END,
                source_provider = ?15, source_id = ?16, source_url = ?17,
                source_state = CASE WHEN source_state = 'downloaded'
                                    THEN source_state ELSE 'remote' END,
                remote_updated_at = ?18, remote_art_ref = ?19, remote_missing = 0,
                cache_state = CASE WHEN ?20 OR (source_state IS NOT 'downloaded'
                        AND (source_url IS NOT ?17 OR (?14 <> 0 AND file_size IS NOT ?14)))
                    THEN NULL ELSE cache_state END,
                source_fetched_at = CASE WHEN ?20 OR (source_state IS NOT 'downloaded'
                        AND (source_url IS NOT ?17 OR (?14 <> 0 AND file_size IS NOT ?14)))
                    THEN NULL ELSE source_fetched_at END
             WHERE id = ?1",
            params![
                id,
                path,
                name,
                meta.title,
                meta.artist,
                meta.album,
                meta.album_artist,
                meta.genre,
                meta.year,
                meta.track_number,
                meta.disc_number,
                meta.extension,
                meta.duration_seconds,
                file_size,
                provider,
                meta.remote_id,
                meta.part_key,
                meta.updated_at,
                meta.art_ref,
                reset_cache,
            ],
        )
        .map_err(sql_error("update remote track"))?;
        return Ok((outcome, orphaned));
    }

    tx.execute(
        "INSERT INTO tracks (
            id, path, name, title, artist, album, album_artist, genre, year, track_number,
            disc_number, format, duration_seconds, file_size, modified_at, indexed_at,
            source_provider, source_id, source_url, source_state, remote_source_id,
            remote_visible, remote_missing, remote_updated_at, remote_art_ref
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, 'remote', ?20,
            0, 0, ?21, ?22
        )",
        params![
            Uuid::new_v4().to_string(),
            path,
            name,
            meta.title,
            meta.artist,
            meta.album,
            meta.album_artist,
            meta.genre,
            meta.year,
            meta.track_number,
            meta.disc_number,
            meta.extension,
            meta.duration_seconds,
            file_size,
            meta.updated_at.unwrap_or(0),
            now,
            provider,
            meta.remote_id,
            meta.part_key,
            source_id,
            meta.updated_at,
            meta.art_ref,
        ],
    )
    .map_err(sql_error("insert remote track"))?;
    Ok((Outcome::Inserted, None))
}

/// Remote rows are only searchable while they are visible, so after any
/// visibility or metadata change their index entries are rebuilt from the view.
fn refresh_remote_fts(tx: &Transaction<'_>) -> Result<(), String> {
    tx.execute(
        "DELETE FROM tracks_fts WHERE track_id IN
            (SELECT id FROM tracks WHERE source_state = 'remote')",
        [],
    )
    .map_err(sql_error("clear remote search entries"))?;
    tx.execute(
        "INSERT INTO tracks_fts(title, artist, album, name, lyrics, track_id)
         SELECT title, artist, album, name, COALESCE(lyrics, ''), id
         FROM library_tracks WHERE source_state = 'remote'",
        [],
    )
    .map_err(sql_error("rebuild remote search entries"))?;
    Ok(())
}

struct Candidate {
    id: String,
    key: SongKey,
    complete: bool,
    created_at: i64,
    source_order: i64,
    missing: bool,
    visible: bool,
}

impl Library {
    /// The id comes from the server's own stable key, so removing a server and
    /// adding it again gives the same id and its tombstones re-match.
    pub fn insert_remote_source(
        &self,
        kind: &str,
        stable_key: &str,
        display_name: &str,
        config_json: &str,
    ) -> Result<String, String> {
        let id = format!("{kind}-{}", crate::remote::uri::percent_encode(stable_key));
        let added = self
            .write_connection()
            .execute(
                "INSERT OR IGNORE INTO remote_sources
                    (id, kind, display_name, config_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, kind, display_name, config_json, now_timestamp()],
            )
            .map_err(sql_error("add remote source"))?;
        if added == 0 {
            return Err("This server is already added".to_string());
        }
        Ok(id)
    }

    pub fn update_remote_source_config(&self, id: &str, config_json: &str) -> Result<(), String> {
        let changed = self
            .write_connection()
            .execute(
                "UPDATE remote_sources SET config_json = ?2 WHERE id = ?1",
                params![id, config_json],
            )
            .map_err(sql_error("update remote source"))?;
        if changed == 0 {
            return Err(format!("Unknown remote source: {id}"));
        }
        Ok(())
    }

    pub fn list_remote_sources(&self) -> Result<Vec<RemoteSourceRow>, String> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(
                "SELECT rs.id, rs.kind, rs.display_name, rs.config_json, rs.created_at,
                        rs.last_sync_at, rs.last_sync_error,
                        (SELECT COUNT(*) FROM tracks t
                         WHERE t.remote_source_id = rs.id AND t.remote_missing = 0)
                 FROM remote_sources rs
                 ORDER BY rs.created_at, rs.rowid",
            )
            .map_err(sql_error("prepare remote source list"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(RemoteSourceRow {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    display_name: row.get(2)?,
                    config_json: row.get(3)?,
                    created_at: row.get(4)?,
                    last_sync_at: row.get(5)?,
                    last_sync_error: row.get(6)?,
                    track_count: row.get(7)?,
                })
            })
            .map_err(sql_error("list remote sources"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error("read remote sources"))?;
        Ok(rows)
    }

    pub fn record_remote_sync(&self, id: &str, at: i64, error: Option<&str>) -> Result<(), String> {
        self.write_connection()
            .execute(
                "UPDATE remote_sources SET last_sync_at = ?2, last_sync_error = ?3 WHERE id = ?1",
                params![id, at, error],
            )
            .map_err(sql_error("record remote sync"))?;
        Ok(())
    }

    pub fn begin_remote_walk(&self, source_id: &str) -> Result<RemoteWalk, String> {
        let known = self
            .read_connection()
            .query_row(
                "SELECT 1 FROM remote_sources WHERE id = ?1",
                params![source_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(sql_error("look up remote source"))?
            .is_some();
        if !known {
            return Err(format!("Unknown remote source: {source_id}"));
        }
        Ok(RemoteWalk {
            source_id: source_id.to_string(),
            seen: HashSet::new(),
            upserted: 0,
            revived: 0,
            rekeyed: Vec::new(),
        })
    }

    /// One page in one transaction. After an error the walk must be dropped,
    /// not finished: finishing would tombstone everything the failed page
    /// would have listed.
    pub fn upsert_remote_page(
        &self,
        walk: &mut RemoteWalk,
        provider: &str,
        page: &[RemoteTrackMeta],
    ) -> Result<(), String> {
        let now = now_timestamp();
        let mut connection = self.write_connection();
        let tx = connection
            .transaction()
            .map_err(sql_error("begin remote page"))?;
        let mut revived = 0;
        let mut rekeyed = Vec::new();
        for meta in page {
            let (outcome, orphaned) =
                upsert_one(&tx, &walk.source_id, provider, meta, &walk.seen, now)?;
            if matches!(outcome, Outcome::Revived) {
                revived += 1;
            }
            rekeyed.extend(orphaned);
            walk.seen.insert(meta.remote_id.clone());
        }
        tx.commit().map_err(sql_error("commit remote page"))?;
        walk.upserted += page.len();
        walk.revived += revived;
        walk.rekeyed.extend(rekeyed);
        Ok(())
    }

    /// Tombstone every row of the source this walk did not see. Call only for
    /// a walk that listed the whole catalog.
    pub fn finish_remote_walk(&self, walk: RemoteWalk) -> Result<WalkSummary, String> {
        let mut connection = self.write_connection();
        let tx = connection
            .transaction()
            .map_err(sql_error("begin remote walk finish"))?;
        let stale: Vec<String> = {
            let mut statement = tx
                .prepare(
                    "SELECT id, source_id FROM tracks
                     WHERE remote_source_id = ?1 AND remote_missing = 0
                       AND source_state = 'remote'",
                )
                .map_err(sql_error("prepare remote walk finish"))?;
            let rows = statement
                .query_map(params![walk.source_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(sql_error("list remote rows"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error("read remote rows"))?;
            rows.into_iter()
                .filter(|(_, remote_id)| !walk.seen.contains(remote_id))
                .map(|(id, _)| id)
                .collect()
        };
        for chunk in stale.chunks(BATCH) {
            let marks = in_list(chunk.len());
            tx.execute(
                &format!(
                    "UPDATE tracks SET remote_missing = 1, remote_visible = 0
                     WHERE id IN ({marks})"
                ),
                params_from_iter(chunk),
            )
            .map_err(sql_error("tombstone remote tracks"))?;
            tx.execute(
                &format!("DELETE FROM tracks_fts WHERE track_id IN ({marks})"),
                params_from_iter(chunk),
            )
            .map_err(sql_error("drop tombstoned search entries"))?;
        }
        tx.commit()
            .map_err(sql_error("commit remote walk finish"))?;
        Ok(WalkSummary {
            upserted: walk.upserted,
            tombstoned: stale.len(),
            revived: walk.revived,
            rekeyed: walk.rekeyed,
        })
    }

    /// Hide one remote track the user removed. It stays a row, so playlists
    /// and history keep it, and the next sync brings it back. Returns whether
    /// `path` named a remote track. Visibility is not recomputed here.
    pub fn tombstone_remote_track(&self, path: &str) -> Result<bool, String> {
        let connection = self.write_connection();
        let id: Option<String> = connection
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1 AND source_state = 'remote'",
                params![path],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error("look up remote track"))?;
        let Some(id) = id else {
            return Ok(false);
        };
        connection
            .execute(
                "UPDATE tracks SET remote_missing = 1, remote_visible = 0 WHERE id = ?1",
                params![id],
            )
            .map_err(sql_error("hide remote track"))?;
        let _ = connection.execute("DELETE FROM tracks_fts WHERE track_id = ?1", params![id]);
        Ok(true)
    }

    /// Remove a source: its rows become tombstones, except downloads, which
    /// stay as local files. Returns the rows that had cached audio so the
    /// caller can delete those files. Visibility is not recomputed here, so a
    /// copy from another source may still need to take over.
    pub fn tombstone_remote_source(&self, source_id: &str) -> Result<Vec<RemoteUri>, String> {
        let mut connection = self.write_connection();
        let tx = connection
            .transaction()
            .map_err(sql_error("begin remote source removal"))?;
        let cached = {
            let mut statement = tx
                .prepare(
                    "SELECT source_id FROM tracks
                     WHERE remote_source_id = ?1 AND source_state = 'remote'
                       AND cache_state IS NOT NULL",
                )
                .map_err(sql_error("prepare cached remote rows"))?;
            let ids = statement
                .query_map(params![source_id], |row| row.get::<_, String>(0))
                .map_err(sql_error("list cached remote rows"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error("read cached remote rows"))?;
            ids.into_iter()
                .map(|remote_id| RemoteUri::new(source_id, &remote_id))
                .collect::<Vec<_>>()
        };
        tx.execute(
            "DELETE FROM tracks_fts WHERE track_id IN
                (SELECT id FROM tracks
                 WHERE remote_source_id = ?1 AND source_state = 'remote')",
            params![source_id],
        )
        .map_err(sql_error("drop removed search entries"))?;
        tx.execute(
            "UPDATE tracks SET remote_missing = 1, remote_visible = 0, cache_state = NULL
             WHERE remote_source_id = ?1 AND source_state = 'remote'",
            params![source_id],
        )
        .map_err(sql_error("tombstone removed source"))?;
        tx.execute(
            "DELETE FROM remote_sources WHERE id = ?1",
            params![source_id],
        )
        .map_err(sql_error("delete remote source"))?;
        tx.commit()
            .map_err(sql_error("commit remote source removal"))?;
        Ok(cached)
    }

    /// Decide which copy of each song the library shows. A local file always
    /// wins; between remote copies, one with complete cached audio comes first,
    /// then the source added earliest. Missing rows are never shown.
    pub fn recompute_remote_visibility(&self, remote_enabled: bool) -> Result<(), String> {
        let mut connection = self.write_connection();
        let has_remote = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM tracks WHERE source_state = 'remote')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(sql_error("check for remote rows"))?;
        if !has_remote {
            return Ok(());
        }
        let tx = connection
            .transaction()
            .map_err(sql_error("begin visibility update"))?;

        if !remote_enabled {
            tx.execute(
                "UPDATE tracks SET remote_visible = 0 WHERE source_state = 'remote'",
                [],
            )
            .map_err(sql_error("hide remote tracks"))?;
        } else {
            let candidates = remote_candidates(&tx)?;
            let local_keys = local_keys_among(&tx, &candidates)?;

            let rank = |c: &Candidate| (!c.complete, c.created_at, c.source_order, c.id.clone());
            let mut best: HashMap<&SongKey, &Candidate> = HashMap::new();
            for candidate in candidates.iter().filter(|c| !c.missing) {
                if local_keys.contains(&candidate.key) {
                    continue;
                }
                best.entry(&candidate.key)
                    .and_modify(|current| {
                        if rank(candidate) < rank(current) {
                            *current = candidate;
                        }
                    })
                    .or_insert(candidate);
            }
            let winners: HashSet<&str> = best.values().map(|c| c.id.as_str()).collect();

            let mut show = Vec::new();
            let mut hide = Vec::new();
            for candidate in &candidates {
                let wanted = winners.contains(candidate.id.as_str());
                if wanted && !candidate.visible {
                    show.push(candidate.id.clone());
                } else if !wanted && candidate.visible {
                    hide.push(candidate.id.clone());
                }
            }
            set_visible(&tx, &show, true)?;
            set_visible(&tx, &hide, false)?;
        }

        refresh_remote_fts(&tx)?;
        tx.commit().map_err(sql_error("commit visibility update"))
    }

    pub fn remote_stream_info(&self, uri: &RemoteUri) -> Result<Option<RemoteStreamInfo>, String> {
        let connection = self.read_connection();
        connection
            .query_row(
                "SELECT source_provider, source_url, file_size, format, cache_state,
                        source_state, path
                 FROM tracks WHERE remote_source_id = ?1 AND source_id = ?2",
                params![uri.source_id, uri.remote_id],
                |row| {
                    let source_state = row.get::<_, Option<String>>(5)?.unwrap_or_default();
                    let path: String = row.get(6)?;
                    let downloaded = source_state == "downloaded";
                    Ok(RemoteStreamInfo {
                        uri: uri.clone(),
                        provider: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        part_key: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        size: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                        extension: row.get::<_, String>(3)?.to_lowercase(),
                        cache_state: row.get(4)?,
                        source_state,
                        path_if_downloaded: downloaded.then_some(path),
                    })
                },
            )
            .optional()
            .map_err(sql_error("look up remote stream"))
    }

    pub fn set_remote_cache_state(
        &self,
        uri: &RemoteUri,
        state: Option<&str>,
    ) -> Result<(), String> {
        self.write_connection()
            .execute(
                "UPDATE tracks SET cache_state = ?3,
                        source_fetched_at = CASE WHEN ?3 = 'complete' THEN ?4
                                                 ELSE source_fetched_at END
                 WHERE remote_source_id = ?1 AND source_id = ?2",
                params![uri.source_id, uri.remote_id, state, now_timestamp()],
            )
            .map_err(sql_error("set remote cache state"))?;
        Ok(())
    }

    /// Records a size the server only revealed when asked for the file,
    /// for backends whose catalog leaves it out.
    pub fn set_remote_track_size(&self, uri: &RemoteUri, size: u64) -> Result<(), String> {
        self.write_connection()
            .execute(
                "UPDATE tracks SET file_size = ?3
                 WHERE remote_source_id = ?1 AND source_id = ?2
                   AND source_state IS NOT 'downloaded'",
                params![
                    uri.source_id,
                    uri.remote_id,
                    i64::try_from(size).unwrap_or(i64::MAX)
                ],
            )
            .map_err(sql_error("set remote track size"))?;
        Ok(())
    }

    /// Fills in a length the catalog didn't give, once the whole file is
    /// cached and can be measured. A known length is never replaced.
    pub fn set_remote_track_duration_if_missing(
        &self,
        uri: &RemoteUri,
        seconds: f64,
    ) -> Result<(), String> {
        self.write_connection()
            .execute(
                "UPDATE tracks SET duration_seconds = ?3
                 WHERE remote_source_id = ?1 AND source_id = ?2
                   AND (duration_seconds IS NULL OR duration_seconds <= 0)",
                params![uri.source_id, uri.remote_id, seconds],
            )
            .map_err(sql_error("set remote track duration"))?;
        Ok(())
    }

    /// Cached remote rows with the extension that names their file, least
    /// recently fetched first. Downloads are not listed: they are library
    /// content, not cache.
    pub fn remote_eviction_candidates(&self) -> Result<Vec<(RemoteUri, String, i64)>, String> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(
                "SELECT remote_source_id, source_id, format, COALESCE(source_fetched_at, 0)
                 FROM tracks
                 WHERE cache_state IS NOT NULL AND source_state = 'remote'
                   AND remote_source_id IS NOT NULL
                 ORDER BY COALESCE(source_fetched_at, 0), id",
            )
            .map_err(sql_error("prepare remote eviction query"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    RemoteUri::new(&row.get::<_, String>(0)?, &row.get::<_, String>(1)?),
                    row.get::<_, String>(2)?.to_lowercase(),
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(sql_error("query remote eviction candidates"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error("read remote eviction candidates"))?;
        Ok(rows)
    }

    /// Every row that should have a cache file, with the extension that names it.
    pub fn remote_cached_files(&self) -> Result<Vec<(RemoteUri, String)>, String> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(
                "SELECT remote_source_id, source_id, format FROM tracks
                 WHERE cache_state IS NOT NULL AND remote_source_id IS NOT NULL",
            )
            .map_err(sql_error("prepare remote cache file query"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    RemoteUri::new(&row.get::<_, String>(0)?, &row.get::<_, String>(1)?),
                    row.get::<_, String>(2)?.to_lowercase(),
                ))
            })
            .map_err(sql_error("query remote cache files"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error("read remote cache files"))?;
        Ok(rows)
    }

    /// Give every row of one album in one source the same cover. A cover the
    /// user chose themselves is left alone.
    pub fn set_album_art_for_remote_album(
        &self,
        source_id: &str,
        album: &str,
        album_artist: &str,
        image: &[u8],
    ) -> Result<(), String> {
        let saved = crate::cover_art::save_album_art_thumb_in(self.cover_root(), image)?;
        let mut connection = self.write_connection();
        let tx = connection
            .transaction()
            .map_err(sql_error("begin album art update"))?;
        tx.execute(
            "INSERT OR IGNORE INTO album_art (id, thumb_path, mime, byte_size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                saved.id,
                saved.relative_path,
                saved.mime,
                saved.byte_size,
                now_timestamp()
            ],
        )
        .map_err(sql_error("upsert album_art"))?;
        tx.execute(
            &format!(
                "UPDATE tracks SET album_art_id = ?1, cover_art_mime = ?2,
                        cover_art_source = 'remote', cover_art_data_url = NULL
                 WHERE remote_source_id = ?3 AND album = ?4 AND {ALBUM_ARTIST_SQL} = ?5
                   AND (cover_art_source IS NULL OR cover_art_source = 'remote')"
            ),
            params![saved.id, saved.mime, source_id, album, album_artist],
        )
        .map_err(sql_error("update remote album art"))?;
        tx.commit().map_err(sql_error("commit album art update"))
    }

    /// `(album, album_artist, art_ref)` for each album of the source that has
    /// artwork on the server but none stored here.
    pub fn remote_albums_missing_art(
        &self,
        source_id: &str,
    ) -> Result<Vec<(String, String, String)>, String> {
        let connection = self.read_connection();
        let mut statement = connection
            .prepare(&format!(
                "SELECT album, {ALBUM_ARTIST_SQL} AS grouped_artist, MIN(remote_art_ref)
                 FROM tracks
                 WHERE remote_source_id = ?1 AND remote_missing = 0
                   AND album_art_id IS NULL AND remote_art_ref IS NOT NULL
                 GROUP BY album, grouped_artist
                 ORDER BY album, grouped_artist"
            ))
            .map_err(sql_error("prepare missing art query"))?;
        let rows = statement
            .query_map(params![source_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(sql_error("query albums missing art"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error("read albums missing art"))?;
        Ok(rows)
    }
}

/// Every remote row with the key it is compared by.
fn remote_candidates(tx: &Transaction<'_>) -> Result<Vec<Candidate>, String> {
    let mut statement = tx
        .prepare(
            "SELECT t.id, lower(trim(t.artist)), lower(trim(t.album)), lower(trim(t.title)),
                    t.cache_state, t.remote_missing, t.remote_visible,
                    COALESCE(rs.created_at, 9223372036854775807),
                    COALESCE(rs.rowid, 9223372036854775807)
             FROM tracks t
             LEFT JOIN remote_sources rs ON rs.id = t.remote_source_id
             WHERE t.source_state = 'remote' AND t.remote_source_id IS NOT 'search'",
        )
        .map_err(sql_error("prepare visibility query"))?;
    let rows = statement
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(3)?;
            // A row with no title matches nothing but itself.
            let key = if title.is_empty() {
                (format!("\0{id}"), String::new(), String::new())
            } else {
                (row.get(1)?, row.get(2)?, title)
            };
            Ok(Candidate {
                id,
                key,
                complete: row.get::<_, Option<String>>(4)?.as_deref() == Some("complete"),
                missing: row.get(5)?,
                visible: row.get(6)?,
                created_at: row.get(7)?,
                source_order: row.get(8)?,
            })
        })
        .map_err(sql_error("query visibility"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error("read visibility rows"))?;
    Ok(rows)
}

/// The keys of local files (including downloads) that some remote row shares.
fn local_keys_among(
    tx: &Transaction<'_>,
    candidates: &[Candidate],
) -> Result<HashSet<SongKey>, String> {
    let remote_keys: HashSet<&SongKey> = candidates.iter().map(|c| &c.key).collect();
    let mut statement = tx
        .prepare(
            "SELECT lower(trim(artist)), lower(trim(album)), lower(trim(title))
             FROM tracks WHERE source_state IS NULL OR source_state = 'downloaded'",
        )
        .map_err(sql_error("prepare local key query"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(sql_error("query local keys"))?;
    let mut shared = HashSet::new();
    for row in rows {
        let key = row.map_err(sql_error("read local key"))?;
        if remote_keys.contains(&key) {
            shared.insert(key);
        }
    }
    Ok(shared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::Track;
    use rusqlite::params;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn test_library() -> Library {
        let dir: PathBuf =
            std::env::temp_dir().join(format!("wave-test-remote-{}", Uuid::new_v4()));
        Library::new_with_path(&dir.join("library.db")).unwrap()
    }

    fn meta(id: &str, title: &str, artist: &str, album: &str, dur: f64) -> RemoteTrackMeta {
        RemoteTrackMeta {
            remote_id: id.to_string(),
            title: title.to_string(),
            artist: artist.to_string(),
            album_artist: None,
            album: album.to_string(),
            track_number: Some(1),
            disc_number: None,
            year: Some(2001),
            genre: None,
            duration_seconds: Some(dur),
            size: 1000,
            extension: "flac".to_string(),
            part_key: format!("/library/parts/{id}/file.flac"),
            art_ref: None,
            updated_at: Some(5),
        }
    }

    fn walk(library: &Library, source: &str, page: &[RemoteTrackMeta]) -> WalkSummary {
        let mut walk = library.begin_remote_walk(source).unwrap();
        library.upsert_remote_page(&mut walk, "plex", page).unwrap();
        library.finish_remote_walk(walk).unwrap()
    }

    fn library_titles(library: &Library) -> Vec<String> {
        let connection = library.read_connection();
        let mut statement = connection
            .prepare("SELECT title FROM library_tracks ORDER BY title")
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    fn track_id(library: &Library, source: &str, remote_id: &str) -> String {
        library
            .read_connection()
            .query_row(
                "SELECT id FROM tracks WHERE remote_source_id = ?1 AND source_id = ?2",
                params![source, remote_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn count(library: &Library, sql: &str) -> i64 {
        library
            .read_connection()
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    }

    fn insert_local(library: &Library, id: &str, title: &str, artist: &str, album: &str) {
        library
            .write_connection()
            .execute(
                "INSERT INTO tracks (id, path, name, title, artist, album, format, file_size,
                                     modified_at, indexed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'MP3', 1, 1, 1)",
                params![
                    id,
                    format!("/music/{id}.mp3"),
                    format!("{id}.mp3"),
                    title,
                    artist,
                    album
                ],
            )
            .unwrap();
    }

    fn add_to_playlist(library: &Library, playlist_id: &str, track_id: &str) {
        library
            .write_connection()
            .execute(
                "INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at)
                 VALUES (?1, ?2, 0, 1)",
                params![playlist_id, track_id],
            )
            .unwrap();
    }

    #[test]
    fn walk_upserts_and_finish_tombstones_unseen() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        let three = [
            meta("1", "One", "Band", "Album", 100.0),
            meta("2", "Two", "Band", "Album", 110.0),
            meta("3", "Three", "Band", "Album", 120.0),
        ];
        let first = walk(&library, &source, &three);
        assert_eq!(first.upserted, 3);
        assert_eq!(first.tombstoned, 0);
        assert_eq!(first.revived, 0);
        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(library_titles(&library), ["One", "Three", "Two"]);

        let second = walk(&library, &source, &three[..2]);
        assert_eq!(second.tombstoned, 1);
        // Hidden at once, before any visibility pass.
        assert_eq!(library_titles(&library), ["One", "Two"]);
        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(library_titles(&library), ["One", "Two"]);
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks WHERE remote_missing = 1"
            ),
            1
        );
        assert_eq!(count(&library, "SELECT COUNT(*) FROM tracks"), 3);
        let sources = library.list_remote_sources().unwrap();
        assert_eq!(sources[0].track_count, 2);
    }

    #[test]
    fn unfinished_walk_tombstones_nothing() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[
                meta("1", "One", "Band", "Album", 100.0),
                meta("2", "Two", "Band", "Album", 110.0),
                meta("3", "Three", "Band", "Album", 120.0),
            ],
        );

        let mut partial = library.begin_remote_walk(&source).unwrap();
        library
            .upsert_remote_page(
                &mut partial,
                "plex",
                &[meta("1", "One", "Band", "Album", 100.0)],
            )
            .unwrap();
        drop(partial);

        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks WHERE remote_missing = 0"
            ),
            3
        );
    }

    #[test]
    fn changed_rating_keys_revive_rows_and_keep_ids() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[
                meta("1", "One", "Band", "Album", 100.0),
                meta("2", "Two", "Band", "Album", 110.0),
                meta("3", "Three", "Band", "Album", 120.0),
            ],
        );
        let before: Vec<String> = ["1", "2", "3"]
            .iter()
            .map(|id| track_id(&library, &source, id))
            .collect();

        let playlist = library.create_playlist("Mix", None).unwrap();
        add_to_playlist(&library, &playlist.id, &before[0]);
        let old_path = RemoteUri::new(&source, "1").to_path();
        library
            .record_listen(&old_path, 30.0, true, false, None)
            .unwrap();

        let summary = walk(
            &library,
            &source,
            &[
                meta("10", "One", "Band", "Album", 100.0),
                meta("20", "Two", "Band", "Album", 110.0),
                meta("30", "Three", "Band", "Album", 120.0),
            ],
        );
        assert_eq!(summary.revived, 3);
        assert_eq!(summary.tombstoned, 0);

        let after: Vec<String> = ["10", "20", "30"]
            .iter()
            .map(|id| track_id(&library, &source, id))
            .collect();
        assert_eq!(before, after);
        assert_eq!(count(&library, "SELECT COUNT(*) FROM tracks"), 3);

        let in_playlist = library.get_playlist_tracks(&playlist.id).unwrap();
        assert_eq!(in_playlist.len(), 1);
        assert_eq!(in_playlist[0].id, before[0]);
        assert_eq!(in_playlist[0].path, RemoteUri::new(&source, "10").to_path());
        let plays: i64 = library
            .read_connection()
            .query_row(
                "SELECT play_count FROM listen_stats WHERE track_id = ?1",
                params![before[0]],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(plays, 1);
    }

    #[test]
    fn tombstoned_rows_revive_only_on_a_matching_duration() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[meta("1", "One", "Band", "Album", 100.0)],
        );
        let original = track_id(&library, &source, "1");
        let gone = walk(&library, &source, &[]);
        assert_eq!(gone.tombstoned, 1);

        // Same tags but a different length is another recording.
        let other = walk(
            &library,
            &source,
            &[meta("2", " one ", "BAND", "album", 140.0)],
        );
        assert_eq!(other.revived, 0);
        assert_ne!(track_id(&library, &source, "2"), original);

        walk(&library, &source, &[]);
        let back = walk(
            &library,
            &source,
            &[meta("3", " one ", "BAND", "album", 101.5)],
        );
        assert_eq!(back.revived, 1);
        assert_eq!(track_id(&library, &source, "3"), original);
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks WHERE remote_missing = 0"
            ),
            1
        );
    }

    #[test]
    fn local_copy_hides_remote_copy() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        insert_local(&library, "local", "Song", "Band", "Album");
        walk(
            &library,
            &source,
            &[
                meta("1", " song ", "BAND", "album", 100.0),
                meta("2", "Other", "Band", "Album", 100.0),
            ],
        );
        library.recompute_remote_visibility(true).unwrap();

        assert_eq!(library_titles(&library), ["Other", "Song"]);
        let visible_remote: i64 = count(
            &library,
            "SELECT COUNT(*) FROM library_tracks WHERE source_state = 'remote'",
        );
        assert_eq!(visible_remote, 1);
    }

    #[test]
    fn cached_remote_beats_uncached_and_older_source_beats_newer() {
        let library = test_library();
        let s1 = library
            .insert_remote_source("plex", "Old", "Old", "{}")
            .unwrap();
        let s2 = library
            .insert_remote_source("plex", "New", "New", "{}")
            .unwrap();
        walk(&library, &s1, &[meta("1", "Song", "Band", "Album", 100.0)]);
        walk(&library, &s2, &[meta("1", "Song", "Band", "Album", 100.0)]);

        let visible_source = |library: &Library| -> String {
            library
                .read_connection()
                .query_row(
                    "SELECT remote_source_id FROM library_tracks WHERE source_state = 'remote'",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };

        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(visible_source(&library), s1);

        let uri = RemoteUri::new(&s2, "1");
        library
            .set_remote_cache_state(&uri, Some("complete"))
            .unwrap();
        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(visible_source(&library), s2);
        assert_eq!(count(&library, "SELECT COUNT(*) FROM library_tracks"), 1);

        library.set_remote_cache_state(&uri, None).unwrap();
        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(visible_source(&library), s1);
    }

    #[test]
    fn disabled_outside_sourcing_hides_all_remote() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[meta("1", "Song", "Band", "Album", 100.0)],
        );
        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(library_titles(&library), ["Song"]);

        let playlist = library.create_playlist("Mix", None).unwrap();
        add_to_playlist(&library, &playlist.id, &track_id(&library, &source, "1"));

        library.recompute_remote_visibility(false).unwrap();
        assert!(library_titles(&library).is_empty());
        let in_playlist = library.get_playlist_tracks(&playlist.id).unwrap();
        assert_eq!(in_playlist.len(), 1);
        assert_eq!(in_playlist[0].title, "Song");

        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(library_titles(&library), ["Song"]);
    }

    #[test]
    fn remote_rows_in_search_only_when_visible() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[meta("1", "Zanzibar", "Band", "Album", 100.0)],
        );

        assert!(library.search_tracks("Zanzibar").unwrap().is_empty());
        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(library.search_tracks("Zanzibar").unwrap().len(), 1);
        library.recompute_remote_visibility(false).unwrap();
        assert!(library.search_tracks("Zanzibar").unwrap().is_empty());
    }

    #[test]
    fn tombstone_source_keeps_downloads() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[
                meta("1", "Kept", "Band", "Album", 100.0),
                meta("2", "Cached", "Band", "Album", 110.0),
                meta("3", "Bare", "Band", "Album", 120.0),
            ],
        );
        library.recompute_remote_visibility(true).unwrap();
        library
            .set_remote_cache_state(&RemoteUri::new(&source, "2"), Some("complete"))
            .unwrap();
        library
            .promote_source_track(&track_id(&library, &source, "1"), "/downloads/kept.flac")
            .unwrap();

        let to_delete = library.tombstone_remote_source(&source).unwrap();
        assert_eq!(to_delete, vec![RemoteUri::new(&source, "2")]);
        assert!(library.list_remote_sources().unwrap().is_empty());

        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(library_titles(&library), ["Kept"]);
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks WHERE remote_missing = 1 AND source_state = 'remote'"
            ),
            2
        );
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks WHERE cache_state IS NOT NULL"
            ),
            0
        );
    }

    #[test]
    fn local_indexing_never_touches_remote_rows() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[meta("1", "Song", "Band", "Album", 100.0)],
        );
        let snapshot = |library: &Library| -> (String, String, String, String, Option<String>) {
            library
                .read_connection()
                .query_row(
                    "SELECT id, path, title, source_state, remote_source_id
                     FROM tracks WHERE source_provider = 'plex'",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .unwrap()
        };
        let before = snapshot(&library);

        let local = Track {
            id: "local".to_string(),
            path: "/music/song.mp3".to_string(),
            name: "song.mp3".to_string(),
            title: "Song".to_string(),
            artist: "Band".to_string(),
            album: "Album".to_string(),
            format: "MP3".to_string(),
            duration_seconds: Some(100.0),
            file_size: 1,
            modified_at: 1,
            indexed_at: 1,
            ..Track::default()
        };
        let default_playlist = library.default_playlist_id().unwrap();
        library
            .apply_playlist_sync(&default_playlist, &[], &[local], &[])
            .unwrap();

        assert_eq!(snapshot(&library), before);
        assert_eq!(count(&library, "SELECT COUNT(*) FROM tracks"), 2);
    }

    #[test]
    fn recompute_with_no_remote_rows_changes_nothing() {
        let library = test_library();
        insert_local(&library, "a", "Song", "Band", "Album");
        library.search_tracks("Song").unwrap();
        let before = count(&library, "SELECT COUNT(*) FROM tracks_fts");
        library.recompute_remote_visibility(true).unwrap();
        library.recompute_remote_visibility(false).unwrap();
        assert_eq!(count(&library, "SELECT COUNT(*) FROM tracks_fts"), before);
        assert_eq!(library_titles(&library), ["Song"]);
    }

    #[test]
    fn upsert_updates_metadata_and_keeps_downloads_local() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[meta("1", "Song", "Band", "Album", 100.0)],
        );
        let id = track_id(&library, &source, "1");
        library
            .promote_source_track(&id, "/downloads/song.flac")
            .unwrap();
        // The user retitles the downloaded file.
        library
            .write_connection()
            .execute(
                "UPDATE tracks SET title = 'My Title' WHERE id = ?1",
                params![id],
            )
            .unwrap();

        let mut changed = meta("1", "Song", "Band", "Album", 100.0);
        changed.genre = Some("Rock".to_string());
        changed.part_key = "/library/parts/9/file.flac".to_string();
        walk(&library, &source, &[changed]);

        let (path, state, title, genre, url): (String, String, String, Option<String>, String) =
            library
                .read_connection()
                .query_row(
                    "SELECT path, source_state, title, genre, source_url FROM tracks WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .unwrap();
        assert_eq!(path, "/downloads/song.flac");
        assert_eq!(state, "downloaded");
        // A download is the user's own file now: a sync never rewrites its tags.
        assert_eq!(title, "My Title");
        assert_eq!(genre, None);
        assert_eq!(url, "/library/parts/9/file.flac");
    }

    #[test]
    fn a_changed_part_or_size_resets_the_cache_state() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        let uri = RemoteUri::new(&source, "1");
        let state = |library: &Library| {
            library
                .read_connection()
                .query_row(
                    "SELECT cache_state, source_fetched_at FROM tracks WHERE source_id = '1'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                        ))
                    },
                )
                .unwrap()
        };
        let original = meta("1", "One", "Band", "Album", 100.0);
        walk(&library, &source, std::slice::from_ref(&original));

        library
            .set_remote_cache_state(&uri, Some("complete"))
            .unwrap();
        walk(&library, &source, std::slice::from_ref(&original));
        let (cache, fetched) = state(&library);
        assert_eq!(cache.as_deref(), Some("complete"));
        assert!(fetched.is_some());

        let mut resized = original.clone();
        resized.size = 2000;
        walk(&library, &source, std::slice::from_ref(&resized));
        assert_eq!(state(&library), (None, None));

        library
            .set_remote_cache_state(&uri, Some("partial"))
            .unwrap();
        let mut moved = resized.clone();
        moved.part_key = "/library/parts/9/file.flac".into();
        walk(&library, &source, std::slice::from_ref(&moved));
        assert_eq!(state(&library), (None, None));

        // A download keeps its own size, so a different server size is normal.
        library
            .write_connection()
            .execute(
                "UPDATE tracks SET source_state = 'downloaded', cache_state = 'complete',
                        file_size = 1 WHERE source_id = '1'",
                [],
            )
            .unwrap();
        walk(&library, &source, std::slice::from_ref(&moved));
        assert_eq!(state(&library).0.as_deref(), Some("complete"));
    }

    #[test]
    fn stream_info_cache_state_and_eviction_order() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "Home", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[
                meta("1", "One", "Band", "Album", 100.0),
                meta("2", "Two", "Band", "Album", 110.0),
            ],
        );
        let one = RemoteUri::new(&source, "1");
        let two = RemoteUri::new(&source, "2");

        let info = library.remote_stream_info(&one).unwrap().unwrap();
        assert_eq!(info.uri, one);
        assert_eq!(info.provider, "plex");
        assert_eq!(info.part_key, "/library/parts/1/file.flac");
        assert_eq!(info.size, 1000);
        assert_eq!(info.extension, "flac");
        assert_eq!(info.cache_state, None);
        assert_eq!(info.source_state, "remote");
        assert_eq!(info.path_if_downloaded, None);
        assert!(library
            .remote_stream_info(&RemoteUri::new(&source, "nope"))
            .unwrap()
            .is_none());

        assert!(library.remote_eviction_candidates().unwrap().is_empty());
        library
            .set_remote_cache_state(&one, Some("complete"))
            .unwrap();
        library
            .set_remote_cache_state(&two, Some("partial"))
            .unwrap();
        library
            .write_connection()
            .execute(
                "UPDATE tracks SET source_fetched_at = 500 WHERE source_id = '1'",
                [],
            )
            .unwrap();
        library
            .write_connection()
            .execute(
                "UPDATE tracks SET source_fetched_at = 100 WHERE source_id = '2'",
                [],
            )
            .unwrap();
        let candidates = library.remote_eviction_candidates().unwrap();
        assert_eq!(
            candidates
                .iter()
                .map(|(uri, _, _)| uri.clone())
                .collect::<Vec<_>>(),
            vec![two.clone(), one.clone()]
        );
        assert!(candidates
            .iter()
            .all(|(_, extension, _)| extension == "flac"));

        library
            .set_remote_cache_state(&two, Some("complete"))
            .unwrap();
        let fetched: i64 = library
            .read_connection()
            .query_row(
                "SELECT source_fetched_at FROM tracks WHERE source_id = '2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(fetched > 100);

        library
            .promote_source_track(&track_id(&library, &source, "1"), "/downloads/one.flac")
            .unwrap();
        let info = library.remote_stream_info(&one).unwrap().unwrap();
        assert_eq!(info.source_state, "downloaded");
        assert_eq!(
            info.path_if_downloaded.as_deref(),
            Some("/downloads/one.flac")
        );
        // A download is never an eviction candidate.
        assert_eq!(library.remote_eviction_candidates().unwrap().len(), 1);
    }

    fn png_bytes() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            8,
            image::Rgb([200, 30, 30]),
        ))
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
        out.into_inner()
    }

    #[test]
    fn album_art_applies_to_the_whole_album_in_one_source() {
        let library = test_library();
        let s1 = library
            .insert_remote_source("plex", "A", "A", "{}")
            .unwrap();
        let s2 = library
            .insert_remote_source("plex", "B", "B", "{}")
            .unwrap();
        let with_art = |id: &str, title: &str, album: &str, art: Option<&str>| {
            let mut m = meta(id, title, "Band", album, 100.0);
            m.art_ref = art.map(str::to_string);
            m
        };
        walk(
            &library,
            &s1,
            &[
                with_art("1", "One", "Album", Some("/thumb/9")),
                with_art("2", "Two", "Album", Some("/thumb/9")),
                with_art("3", "Three", "Bare", None),
            ],
        );
        walk(
            &library,
            &s2,
            &[with_art("1", "One", "Album", Some("/thumb/4"))],
        );

        let missing = library.remote_albums_missing_art(&s1).unwrap();
        assert_eq!(
            missing,
            vec![(
                "Album".to_string(),
                "Band".to_string(),
                "/thumb/9".to_string()
            )]
        );

        library
            .set_album_art_for_remote_album(&s1, "Album", "Band", &png_bytes())
            .unwrap();
        assert!(library.remote_albums_missing_art(&s1).unwrap().is_empty());
        assert_eq!(library.remote_albums_missing_art(&s2).unwrap().len(), 1);
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks WHERE album_art_id IS NOT NULL
                 AND cover_art_source = 'remote'"
            ),
            2
        );
    }

    #[test]
    fn a_download_missing_from_the_server_stays_a_searchable_local_file() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv1", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[
                meta("1", "Zanzibar", "Band", "Album", 100.0),
                meta("2", "Other", "Band", "Album", 110.0),
            ],
        );
        library
            .promote_source_track(&track_id(&library, &source, "1"), "/downloads/z.flac")
            .unwrap();
        library.recompute_remote_visibility(true).unwrap();

        let summary = walk(
            &library,
            &source,
            &[meta("2", "Other", "Band", "Album", 110.0)],
        );
        assert_eq!(summary.tombstoned, 0);
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks WHERE remote_missing = 1"
            ),
            0
        );
        assert_eq!(library.search_tracks("Zanzibar").unwrap().len(), 1);
        assert_eq!(library_titles(&library), ["Other", "Zanzibar"]);
        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(library.search_tracks("Zanzibar").unwrap().len(), 1);
    }

    #[test]
    fn rekeying_a_cached_row_clears_its_cache_and_reports_the_old_file() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv1", "Home", "{}")
            .unwrap();
        walk(
            &library,
            &source,
            &[
                meta("1", "One", "Band", "Album", 100.0),
                meta("2", "Two", "Band", "Album", 110.0),
            ],
        );
        library
            .set_remote_cache_state(&RemoteUri::new(&source, "1"), Some("complete"))
            .unwrap();
        let id = track_id(&library, &source, "1");

        let summary = walk(
            &library,
            &source,
            &[
                meta("10", "One", "Band", "Album", 100.0),
                meta("20", "Two", "Band", "Album", 110.0),
            ],
        );
        assert_eq!(summary.revived, 2);
        // Only the row that held cached audio has an orphaned file.
        assert_eq!(summary.rekeyed, vec![RemoteUri::new(&source, "1")]);
        assert_eq!(track_id(&library, &source, "10"), id);

        let info = library
            .remote_stream_info(&RemoteUri::new(&source, "10"))
            .unwrap()
            .unwrap();
        assert_eq!(info.cache_state, None);
        let fetched: Option<i64> = library
            .read_connection()
            .query_row(
                "SELECT source_fetched_at FROM tracks WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fetched, None);
        assert!(library.remote_eviction_candidates().unwrap().is_empty());
    }

    #[test]
    fn readding_a_removed_source_revives_its_rows() {
        let library = test_library();
        let source = library
            .insert_remote_source("plex", "srv1", "Home", "{}")
            .unwrap();
        let two = [
            meta("1", "One", "Band", "Album", 100.0),
            meta("2", "Two", "Band", "Album", 110.0),
        ];
        walk(&library, &source, &two);
        let before = [
            track_id(&library, &source, "1"),
            track_id(&library, &source, "2"),
        ];
        let playlist = library.create_playlist("Mix", None).unwrap();
        add_to_playlist(&library, &playlist.id, &before[0]);
        let path = RemoteUri::new(&source, "1").to_path();
        library
            .record_listen(&path, 30.0, true, false, None)
            .unwrap();

        library.tombstone_remote_source(&source).unwrap();
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks WHERE remote_missing = 1"
            ),
            2
        );

        let again = library
            .insert_remote_source("plex", "srv1", "Home", "{}")
            .unwrap();
        assert_eq!(again, source);
        let summary = walk(&library, &again, &two);
        assert_eq!(summary.revived, 2);

        assert_eq!(track_id(&library, &again, "1"), before[0]);
        assert_eq!(track_id(&library, &again, "2"), before[1]);
        assert_eq!(count(&library, "SELECT COUNT(*) FROM tracks"), 2);
        assert_eq!(
            count(
                &library,
                "SELECT COUNT(*) FROM tracks WHERE remote_missing = 0"
            ),
            2
        );
        let in_playlist = library.get_playlist_tracks(&playlist.id).unwrap();
        assert_eq!(in_playlist.len(), 1);
        assert_eq!(in_playlist[0].id, before[0]);
        let plays: i64 = library
            .read_connection()
            .query_row(
                "SELECT play_count FROM listen_stats WHERE track_id = ?1",
                params![before[0]],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(plays, 1);
    }

    #[test]
    fn adding_the_same_server_twice_is_refused() {
        let library = test_library();
        let id = library
            .insert_remote_source("plex", "srv1", "Home", "{}")
            .unwrap();
        let error = library
            .insert_remote_source("plex", "srv1", "Other name", "{}")
            .unwrap_err();
        assert_eq!(error, "This server is already added");
        assert_eq!(library.list_remote_sources().unwrap().len(), 1);
        let other = library
            .insert_remote_source("plex", "srv2", "Office", "{}")
            .unwrap();
        assert_ne!(other, id);
    }

    #[test]
    fn source_rows_round_trip() {
        let library = test_library();
        let id = library
            .insert_remote_source("plex", "Home", "Home", "{\"a\":1}")
            .unwrap();
        library
            .update_remote_source_config(&id, "{\"a\":2}")
            .unwrap();
        assert!(library
            .update_remote_source_config("missing", "{}")
            .is_err());
        library
            .record_remote_sync(&id, 1234, Some("offline"))
            .unwrap();
        let rows = library.list_remote_sources().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].config_json, "{\"a\":2}");
        assert_eq!(rows[0].last_sync_at, Some(1234));
        assert_eq!(rows[0].last_sync_error.as_deref(), Some("offline"));
        library.record_remote_sync(&id, 2000, None).unwrap();
        assert_eq!(
            library.list_remote_sources().unwrap()[0].last_sync_error,
            None
        );
    }

    fn remote_path(library: &Library, source: &str, remote_id: &str) -> String {
        library
            .read_connection()
            .query_row(
                "SELECT path FROM tracks WHERE remote_source_id = ?1 AND source_id = ?2",
                params![source, remote_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn metadata_edit_rejects_remote_path() {
        let library = test_library();
        let s1 = library
            .insert_remote_source("plex", "s1", "s1", "{}")
            .unwrap();
        walk(
            &library,
            &s1,
            &[meta("1", "Song", "Artist", "Album", 200.0)],
        );
        let path = remote_path(&library, &s1, "1");
        let edit = crate::tag_edit::ResolvedEdit {
            title: Some("Renamed".into()),
            ..Default::default()
        };
        assert_eq!(
            library.update_track_tags(&path, &edit).unwrap_err(),
            crate::remote::REMOTE_EDIT_REFUSED
        );
        library.recompute_remote_visibility(true).unwrap();
        assert_eq!(library_titles(&library), vec!["Song".to_string()]);
    }

    #[test]
    fn removing_remote_track_tombstones_instead_of_deleting() {
        let library = test_library();
        let s1 = library
            .insert_remote_source("plex", "s1", "s1", "{}")
            .unwrap();
        walk(
            &library,
            &s1,
            &[meta("1", "Song", "Artist", "Album", 200.0)],
        );
        let id = track_id(&library, &s1, "1");
        let path = remote_path(&library, &s1, "1");
        let playlist = library.create_playlist("Mix", None).unwrap();
        library
            .add_track_to_playlist(&playlist.id, path.clone())
            .unwrap();

        library.remove_track_from_library(&path).unwrap();
        library.recompute_remote_visibility(true).unwrap();

        let missing: i64 = library
            .read_connection()
            .query_row(
                "SELECT remote_missing FROM tracks WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(missing, 1);
        assert!(library_titles(&library).is_empty());
        let members = library.get_playlist_tracks(&playlist.id).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id, id);
    }

    #[test]
    fn playlist_export_import_round_trips_remote_uri() {
        let library = test_library();
        let s1 = library
            .insert_remote_source("plex", "s1", "s1", "{}")
            .unwrap();
        walk(
            &library,
            &s1,
            &[meta("1", "Song", "Artist", "Album", 200.0)],
        );
        let id = track_id(&library, &s1, "1");
        let path = remote_path(&library, &s1, "1");
        let playlist = library.create_playlist("Mix", None).unwrap();
        library.add_track_to_playlist(&playlist.id, path).unwrap();

        let dir = std::env::temp_dir().join(format!("wave-test-m3u-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("mix.m3u");
        library
            .export_playlist_m3u(&playlist.id, file.to_str().unwrap())
            .unwrap();
        let (imported, added) = library
            .import_playlist_file(file.to_str().unwrap(), Some("Mix again"))
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(added, 1);
        let members = library.get_playlist_tracks(&imported).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].id, id);
    }

    #[test]
    fn folder_sync_keeps_remote_members_of_a_user_playlist() {
        let library = test_library();
        let s1 = library
            .insert_remote_source("plex", "s1", "s1", "{}")
            .unwrap();
        walk(
            &library,
            &s1,
            &[meta("1", "Song", "Artist", "Album", 200.0)],
        );
        let remote = remote_path(&library, &s1, "1");
        let folder = std::env::temp_dir().join(format!("wave-test-sync-{}", Uuid::new_v4()));
        let playlist = library
            .create_playlist("Synced", Some(folder.to_str().unwrap()))
            .unwrap();
        library
            .add_track_to_playlist(&playlist.id, remote.clone())
            .unwrap();

        let (to_remove, _) = library.diff_playlist_paths(&playlist.id, &[]).unwrap();
        assert!(!to_remove.contains(&remote), "{to_remove:?}");
        library
            .apply_playlist_sync(&playlist.id, &to_remove, &[], &[])
            .unwrap();
        let members = library.get_playlist_tracks(&playlist.id).unwrap();
        assert_eq!(members.len(), 1);
    }
}
