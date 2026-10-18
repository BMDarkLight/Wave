# Remote Library (Plex)

A remote source is a music library that lives on another machine. Its tracks
appear in Wave's library next to local files, stream on demand, and are kept in
a cache so a second play is instant and works offline. Plex is the first
backend. The pieces below are deliberately backend-agnostic: a later backend,
such as a Wave peer reached through Radii, plugs into the same contract without
touching the library merge, the stream cache, or the player.

Everything here is off until the user turns on **Outside Sourcing** in Settings.
With it off, Wave makes no remote request of any kind, including at startup.

---

## Layout

| Path | Role |
|------|------|
| `remote/uri.rs` | `wave-remote://<source>/<remote id>` paths |
| `remote/backend.rs` | `RemoteBackend` trait, `RemoteTrackMeta`, `RemoteError` |
| `remote/store.rs` | Remote sources and remote rows in the library database |
| `remote/gate.rs` | Network policy: off, Wi-Fi only, or allowed |
| `remote/refresh.rs` | The 30 second refresh cooldown |
| `remote/sync.rs` | Catalog walks and the album artwork pass |
| `remote/plex/` | Plex sign-in, server discovery, catalog paging, ranged audio |
| `remote/stream/` | Stream cache, progressive reader, Android loopback server |
| `remote/commands.rs` | Tauri commands, events and startup |
| `audio/buffered_source.rs` | Decodes a remote track on a worker thread |

`RemoteRuntime` (in `remote/mod.rs`) owns the live backends, each source's
status and its own database handle. It has no Tauri types, so the playback
daemon uses it too.

---

## Remote tracks in the library

A remote track is an ordinary `tracks` row with `source_state = 'remote'`.

| Column | Meaning |
|--------|---------|
| `path` | `wave-remote://<source>/<remote id>`. Never changes, so queues, playlists and history stay valid |
| `remote_source_id` | The source it belongs to |
| `source_id` | The backend's id (Plex `ratingKey`) |
| `source_url` | Where the bytes come from (Plex part key) |
| `cache_state` | `NULL`, `partial` or `complete` |
| `remote_visible` | Whether browse, search and counts show it |
| `remote_missing` | The server stopped listing it |
| `remote_art_ref` | Artwork reference fetched after a sync |

Source ids are derived from the server, `<kind>-<server id>`, so removing a
server and adding it again gives it the same id.

**Rows are never hard-deleted.** Playlists, favorites, play counts and
transition history all cascade on the track id. A track the server stops
listing becomes a tombstone (`remote_missing = 1`): hidden from browse and
search, still listed in playlists, playable only if its file is fully cached.
When Plex re-matches a library and every id changes, a sync re-matches each new
id to its old row by artist, album, title and duration (within 2 seconds), so
the row keeps its id and everything attached to it.

**One visible copy per song.** Rows with the same normalized artist, album and
title collapse to one in browse, search and counts. Order of preference: a local
file (downloads included), then a fully cached remote copy, then the source that
was added first. `recompute_remote_visibility` stores the result and runs after
syncs, local imports, downloads, cache completion and eviction. With Outside
Sourcing off, every remote row is hidden from browse and search but stays in
playlists.

A sync that fails partway changes nothing beyond the rows it already upserted;
tombstoning only happens at the end of a complete walk.

---

## Plex

Sign-in uses Plex's PIN flow: Wave opens the Plex sign-in page in the browser
and polls the PIN until it returns a token. Pending sign-ins live in memory for
15 minutes and the token never reaches the frontend. The user then picks a
server (their own first, shared ones marked) and one or more music libraries.

**Without plex.tv.** A server can also be added by its address, for networks
where plex.tv can't be reached. Wave reads `/identity` (which answers without a
token) for the server's id, tries `/` for its name, and lists its music
libraries. The token is optional: a server that allows the local network in
without signing in (Settings, Network, "List of IP addresses and networks that
are allowed without auth") needs none, and Wave then sends no token at all,
since some servers turn away an empty one. A bare address such as
`192.168.1.10` becomes `http://192.168.1.10:32400`; an address with a scheme is
kept as typed, so a reverse proxy works. A directly added server is stored with
`direct: true` and gets the same source id it would get through plex.tv, so
adding it both ways is refused as a duplicate. Signing in again means entering
a new token, which is checked against the same server id.

A server lists several addresses. Wave tries local HTTPS, then the plain local
address (some routers block `plex.direct` names), then remote, then relay, and
remembers the first that answers. The working address is written back after a
sync without touching the rest of the stored config.

Catalogs are paged 500 tracks at a time. Audio is the original file, never the
transcoder, fetched with HTTP Range requests. Error messages never include a URL,
since Plex URLs carry the token.

---

## Stream cache and progressive playback

Playing a remote track starts a **fill**: a background download into a sparse
cache file, with a `.ranges` sidecar recording which byte ranges have arrived.
Each track's bytes are fetched once.

- A read past the downloaded region (a seek, or a format that keeps its index at
  the end) restarts the download at that offset; the gap is filled afterwards.
- An interrupted fill resumes from the sidecar. A file is marked `complete` only
  when every byte is present.
- A failed fill keeps its partial file and reports `remote-stream-failed` if
  something was playing it. A dropped connection marks the source unreachable
  until the next chunk arrives.
- The fill checks the network gate between chunks, so turning Outside Sourcing
  off or losing Wi-Fi stops it.

**Desktop.** The decoder reads the cache file through `ProgressiveReader`, which
waits for missing bytes. That wait must never happen on the audio callback, so a
remote track is decoded on a worker thread into a two-second buffer
(`BufferedSource`). When the buffer runs dry the output plays silence, raises the
buffering flag and counts the stalled time, and the position clock leaves that
time out. Starting an uncached track waits up to 10 seconds for its first bytes
before the player lock is taken, on every play and skip path.

**Android.** ExoPlayer cannot read a half-written file, so a loopback HTTP
server on `127.0.0.1` serves remote tracks from the cache as they fill, with
Range support and a per-session secret in the path. Rust hands ExoPlayer the
`wave-remote://` path unchanged; the Java side rewrites it to the loopback
address and keeps the original as the media id.

**Next track.** An uncached remote track is not preloaded eagerly. Its fill
starts about 30 seconds before the current track ends, and gapless or crossfade
pick it up once it can start. This is the hook a future predictive prefetch
plugs into.

**Eviction.** Remote files share the source cache and its size cap (2 GB on
desktop, 1 GB on Android, set in Settings). Evicting a remote file clears its
`cache_state` but keeps the row. The playing track, the queue and active fills
are never evicted. A startup sweep removes cache files no row accounts for.

---

## Network policy

`NetworkGate` answers one question before every remote request:

| State | When |
|-------|------|
| `disabled` | Outside Sourcing is off |
| `cellular` | Android on mobile data (or a metered non-Wi-Fi network) with **Use cellular data** off |
| `allowed` | Otherwise |

The Android probe fails closed: if Wave cannot tell what network it is on, it
treats it as mobile data.

---

## Refresh

Pull to refresh on mobile Home and Library, and the refresh button on desktop
Home, rescan local folders and sync every remote source. The backend refuses a
second refresh within 30 seconds, or while one is running, and returns the
seconds left so the UI can say so.

---

## Commands and events

| Command | Purpose |
|---------|---------|
| `plex_begin_sign_in` | Start the PIN sign-in and open the browser |
| `plex_poll_sign_in` | `pending`, `done` or `expired` |
| `plex_list_servers` / `plex_list_libraries` | Choices for setup |
| `plex_add_source` | Add a server and start its first sync |
| `plex_reauthorize_source` | Sign a source in again after Plex rejected its token |
| `plex_probe_direct` / `plex_add_direct_source` | Add a server by address and optional token, without plex.tv |
| `plex_set_direct_token` | Give a directly added server a new token |
| `list_remote_sources` / `remove_remote_source` | The settings list |
| `get_remote_status` | Gate state and per-source state |
| `request_library_refresh` | Start a refresh or report the cooldown |
| `clear_source_cache` / `get_source_cache_usage` | The cache controls |
| `download_remote_track` | Keep a remote track as a local file |

Events: `remote-sync-progress`, `remote-sync-finished`, `remote-status-changed`,
`remote-stream-failed`, `remote-track-cached`, `library-refresh-finished`,
`library-changed`.

---

## Testing

The suite is offline. Plex responses are inline JSON fixtures and a loopback
fake server; the stream cache is tested against a throttled fake backend,
including seek-ahead, resume, gate closure and decoding while the file is still
downloading.

One live check talks to a real server and decodes a full track through the
playback decoder:

```bash
WAVE_PLEX_URL=http://192.168.1.10:32400 WAVE_PLEX_TOKEN=... WAVE_PLEX_SECTION=3 \
  cargo test --lib -- --ignored live_plex --nocapture
```
