# Remote Library

A remote source is a music library that lives on another machine. Its tracks
appear in Wave's library next to local files, stream on demand, and are kept in
a cache so a second play is instant and works offline. The backends are Plex,
Jellyfin and Emby, Subsonic-compatible servers (Navidrome, Airsonic, Gonic),
WebDAV folders (Nextcloud, ownCloud, NAS shares), DLNA and UPnP media servers,
Kodi, and Audius accounts. Full-length search results stream through the same
cache; see [Song sourcing](./sources.md#streamed-results-are-hidden-remote-rows).
The pieces below are deliberately backend-agnostic: a later backend,
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
| `remote/subsonic/` | Subsonic API: signed requests, catalog paging, ranged audio |
| `remote/jellyfin/` | Jellyfin and Emby: sign-in, music libraries, catalog paging, ranged audio |
| `remote/webdav.rs` | WebDAV: `PROPFIND` walk, tags from file headers, folder covers |
| `remote/dlna.rs` | DLNA and UPnP: SSDP discovery, ContentDirectory search and browse |
| `remote/kodi.rs` | Kodi: JSON-RPC catalog, `/vfs/` audio |
| `remote/audius.rs` | Audius accounts: uploads, favorites and reposts |
| `remote/search.rs` | The built-in source behind streamed search results |
| `remote/tags.rs` | Tags read from the first bytes of a file, and their cache |
| `remote/http.rs` | The HTTP clients every backend shares |
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
transcoder, fetched with HTTP Range requests. Plex can turn a part request away
when it has no playback decision for it (503, or 509 for bandwidth); its docs
say a download needs none, so Wave asks again with `download=1`. Error messages
never include a URL, since Plex URLs carry the token.

---

## Subsonic

Navidrome, Airsonic, Gonic, Ampache and other servers that speak the Subsonic
API are added with an address, a username and a password. A bare address gets
`http://`; a pasted `/rest` or Navidrome `/app/#/...` path is dropped. Every
request is signed with a fresh salt and `t = md5(password + salt)`, so the
password itself never goes over the wire. A server that refuses token auth
(error 41, as LDAP-backed ones do, or 42 for any unsupported method) gets the
hex-encoded password instead, and the source remembers that choice. Hex is not
encryption, so that fallback only happens over HTTPS or to a server on this
machine; anywhere else the source reports that it needs HTTPS. The password is stored in the
source's config, because each signature needs it.

`ping` checks the account when the source is added and reports the server type
(OpenSubsonic servers name themselves), which goes into the display name such as
"Navidrome (nas:4533)". The source id is built from the username and host, so
the same account on the same server can't be added twice.

The catalog is read with `search3` and an empty query, 500 songs a page, which
OpenSubsonic requires servers to answer with every song. An older server that
returns nothing that way, or refuses the empty query, is walked album by album
instead (`getAlbumList2` in batches of 50, then `getAlbum` for each). A song's
`created` date becomes its date added. `getScanStatus` gives the song count for the
progress bar when the server reports one. Audio comes from `stream` with
`format=raw`, so the server sends the original file, with HTTP Range requests;
an error answered as JSON or XML in place of audio is reported as an error, not
cached. Cover art comes from `getCoverArt`. Signing in again means entering the
password, which is checked with `ping` before it is saved.

---

## Jellyfin and Emby

Both servers share one API and one backend (`kind = "jellyfin"`). A bare
address gets `http://` and port 8096, and a pasted web UI path (`/web/#/home`)
is dropped. `/System/Info/Public`, which needs no sign-in, gives the server's id
and name, and tells the two apart: Jellyfin names itself in `ProductName`, Emby
leaves it out.

Signing in posts the username and password to `/Users/AuthenticateByName`
once. Wave stores the access token and user id it gets back, never the
password. Every request carries the `MediaBrowser` scheme in the
`Authorization` header, values quoted and URL-encoded, with a device id made
for that source. Emby also gets `X-Emby-Authorization` and `X-Emby-Token`, which
it documents; Jellyfin doesn't, since 12.0 removed them and its docs warn
against sending a token twice. The server ties a
sign-in to its device id, so signing in again replaces the old token rather
than leaving sessions behind. Signing in again checks that the password signs
in the same user. Until libraries are picked, the sign-in waits in memory for 15
minutes and its token never reaches the frontend. Removing the source posts
`/Sessions/Logout`, so the token stops working.

The user's music libraries come from `/UserViews` on Jellyfin (the old route is
gone in 12) and `/Users/{id}/Views` on Emby, each falling back to the other on a
404, keeping views whose `CollectionType` is `music`. The catalog is
`/Items?userId=` on Jellyfin and `/Users/{id}/Items` on Emby, with
`IncludeItemTypes=Audio` and `Recursive`,
500 tracks a page, one library after another; each library's
`TotalRecordCount` gives the total for the progress bar. Audio is
`/Audio/{id}/stream?static=true`, the original file, never a transcode, with
HTTP Range requests. Artwork is the item's primary image, or its album's.
The source id is built from the server id and user id.

---

## WebDAV

A WebDAV folder is added by its address, with a username and password if it
asks for one. A bare address gets `http://`, and the path always ends in `/` so
files join under it. Nextcloud and ownCloud folders live under
`/remote.php/dav/files/<user>/`; a Nextcloud public link share is
`/public.php/dav/files/<share token>/`, with `anonymous` as the username when
the share has a password. Every `PROPFIND` carries
`X-Requested-With: XMLHttpRequest`, without which Nextcloud refuses one on a
public share. The password is stored in the source's config,
because every request carries it as Basic auth. Over plain `http://` it is only
sent to this machine or an address that can only be on the local network
(private and link-local IPs, single-label names, `.local`, `.lan`,
`.home.arpa`, `.internal`); anywhere else the source asks for HTTPS.

There is no music API, only files. The catalog walks the folder one directory
at a time with `PROPFIND` and `Depth: 1`, since many servers refuse
`Depth: infinity`. Folders whose names start with `.` or `@`, and recycle bins,
are skipped (Synology keeps thumbnails in `@eaDir`); the walk stops 12 levels
down. A member listed with a bare error status and no properties, as RFC 4918
allows for a folder the login can't read, is skipped too, and a weak ETag
(`W/"..."`) counts as the same version as its strong form. A directory that vanishes or can't be read mid-walk is skipped; any other
failure ends the sync without tombstoning anything.

Each audio file becomes a track whose id is a hash of its path below the
folder, so the id survives a change of server address. Its tags come from the
first 256 KB of the file, enough for ID3v2, FLAC and Ogg headers. What was read
is kept in `<data>/source-cache/remote-tags/`, keyed by the file's ETag (or its
size and modification time), so a sync only reads headers of new or changed
files. A file with no tags there, such as an MP4 with its index at the end, is
described by its path: `Artist/Album/01 - Title.flac`, with `2001 - Album`
giving the year, `CD2` or `Disc 2` folders giving the disc, and `Artist - Title`
file names giving the artist when the folders don't. A `cover.jpg`,
`folder.jpg` or `front.jpg` next to the files becomes the album's artwork. The
length is measured once the whole file is cached.

---

## DLNA and UPnP

Media servers are found with an SSDP `M-SEARCH` for
`urn:schemas-upnp-org:device:MediaServer:1`, sent twice and answered for 3
seconds, or added by address. The search goes out of every multicast-capable
interface, not only the one the system picks: while a VPN is up the system
sends multicast into the tunnel, and a server on the LAN would never hear it. A bare `host:port` tries the description paths
servers commonly use (`/rootDesc.xml` for MiniDLNA, `/description.xml`, and
others). The device description gives the name, the UDN, which becomes the
source id so a server that changes address is still the same source, and the
ContentDirectory control URL.

The catalog is one `Search` for `upnp:class derivedfrom
"object.item.audioItem"`, 500 items a page, which MiniDLNA and most full
servers answer. A server that faults is walked with `Browse` instead,
container by container, 16 levels at most. A walk sees the same song under
Artists, Albums, Genres and Folders, so items are kept once by their file URL.
An item's audio is its first plain `http-get` resource in a format Wave plays,
preferring one that states its size, since servers list transcodes (often
`audio/L16`) beside the original. Tags, length and artwork come from the
DIDL-Lite item; the length may be `H:MM:SS.fff` or the spec's fraction form
`H:MM:SS.F0/F1`. A server that only shares folders (rclone, some NAS boxes)
sends the file name as the title and no tags, so an item without an artist has
its title read the way a WebDAV file name is. A `TotalMatches` of 0 means the
server can't count, so paging continues until a page comes back empty. No
sign-in exists, so there is never a password to ask for.

---

## Kodi

Kodi is added by the address of its web server (Settings, Services, Control,
"Allow remote control via HTTP"), port 8080 unless one is given, with its
username and password, which travel as Basic auth under the same rule as
WebDAV. The catalog is `AudioLibrary.GetSongs`, 500 songs a page in Kodi's own
order: a sort with ties, such as by track number across albums, could move a
song between pages. The artist is Kodi's `displayartist` ("A feat. B"). A song's
audio is its original file: `Files.PrepareDownload` names a `/vfs/` path,
which also works for files Kodi reads over NFS or SMB. Artwork is the song's
thumbnail through `/image/`. The catalog has no file sizes; see
[Files without a size](#files-without-a-size).

---

## Audius

An Audius account is added by its handle, typed as `@name`, `name` or a
profile link. Its uploads, favorites and reposts are public, so there is no
sign-in. The catalog walks the three in turn, 100 items a request; favorites
list only track ids, which are looked up in one request per page. A track in
more than one list is kept once, and tracks gated behind a purchase or a follow
are left out. Favorites are matched to their tracks by either id form, since
the API spec gives a string where the server sends a number. A later page that
repeats the one before means the server isn't paging that list, and ends it. Audio is the `/stream` endpoint, which redirects to a freshly
signed copy and honours Range.

---

## Files without a size

The stream cache sizes its sparse file before the first byte is written.
WebDAV, DLNA, Plex, Jellyfin and Subsonic list sizes; Kodi, Audius and search
results don't. For those, the first play asks for one byte
(`Range: bytes=0-0`), reads the total from `Content-Range`, and stores it on the
row. A sync that lists a size of 0 or no length keeps the stored values, so the
cache isn't reset by a catalog that never knew them. A length the catalog left
out is measured from the file once it is complete.

---

## Stream cache and progressive playback

Playing a remote track starts a **fill**: a background download into a sparse
cache file, with a `.ranges` sidecar recording which byte ranges have arrived.
Each track's bytes are fetched once.

- A read past the downloaded region (a seek, or a format that keeps its index at
  the end) restarts the download at that offset; the gap is filled afterwards.
- A server that answers a ranged request with the whole file can't seek, so the
  fill reads it once from the start instead, and a seek waits for the download
  to get there.
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
| `subsonic_add_source` | Add a Subsonic server by address, username and password |
| `jellyfin_sign_in` / `jellyfin_add_source` | Sign in to a Jellyfin or Emby server, then add it with the chosen libraries |
| `webdav_add_source` | Add a WebDAV folder by address, with an optional username and password |
| `dlna_discover` / `dlna_add_source` | Find media servers on the network, then add one found or typed |
| `kodi_add_source` | Add a Kodi device by address, username and password |
| `audius_add_source` | Add an Audius account by handle |
| `set_source_password` | Sign a Subsonic, Jellyfin, Emby, WebDAV or Kodi source in again with its password |
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

The suite is offline. Every backend's responses are inline fixtures (JSON,
WebDAV multistatus, SOAP and DIDL-Lite) served by a loopback fake server; the stream cache is tested against a throttled fake backend,
including seek-ahead, resume, gate closure and decoding while the file is still
downloading.

One live check talks to a real server and decodes a full track through the
playback decoder:

```bash
WAVE_PLEX_URL=http://192.168.1.10:32400 WAVE_PLEX_TOKEN=... WAVE_PLEX_SECTION=3 \
  cargo test --lib -- --ignored live_plex --nocapture
```

End-to-end checks in `remote/live.rs` add a real source to a fresh library,
sync it, fetch its artwork, then play tracks through the stream cache: a read
three quarters in before anything has downloaded, then the whole file, which
must decode and get its length recorded.

```bash
WAVE_WEBDAV_URL=http://127.0.0.1:8090/ WAVE_WEBDAV_USER=u WAVE_WEBDAV_PASS=p \
  cargo test --lib -- --ignored live_webdav --nocapture
WAVE_DLNA_ADDRESS=127.0.0.1:7879 cargo test --lib -- --ignored live_dlna --nocapture
WAVE_KODI_URL=192.168.1.30:8080 WAVE_KODI_USER=kodi WAVE_KODI_PASS=... \
  cargo test --lib -- --ignored live_kodi --nocapture
cargo test --lib -- --ignored live_audius --nocapture
cargo test --lib -- --ignored live_search_results --nocapture
```

`rclone serve webdav <folder>` and `rclone serve dlna <folder>` make quick
local servers for the first two.
