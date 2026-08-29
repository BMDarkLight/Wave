# Command reference

All commands are registered in `src-tauri/src/lib.rs` and implemented in `src-tauri/src/commands.rs`.

Unless noted, argument names below use **JavaScript camelCase** (Tauri maps Rust `snake_case` parameters automatically). Response field names use **snake_case** as serialized by Serde.

---

## Playback

### `play_track`

Start playing an audio file by absolute path. Does **not** update the playback queue.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | `string` | yes | Absolute file path |

**Returns:** `void`

**Example**

```typescript
await invoke("play_track", { path: "D:\\Music\\album\\01.flac" });
```

**Errors (examples):** file open/decode failures, unsupported format.

---

### `pause_track`

Pause the currently playing track. Preserves playback position.

**Arguments:** none

**Returns:** `void`

**Errors:** `"No track is currently playing"` when nothing is loaded.

---

### `resume_track`

Resume after `pause_track`.

**Arguments:** none

**Returns:** `void`

**Errors:** `"No track is currently playing"` when nothing is loaded.

---

### `stop_track`

Stop playback and clear the loaded track.

**Arguments:** none

**Returns:** `void`

---

### `get_playback_state`

Returns a snapshot of the audio engine state. Safe to poll frequently.

**Arguments:** none

**Returns:** [`PlaybackState`](./types.md#playbackstate)

**Example**

```typescript
const state = await invoke<PlaybackState>("get_playback_state");
// state.is_playing, state.position_seconds, ...
```

---

### `seek_track`

Seek to a position within the current track. Uses Rodio’s native seek (no full re-decode).

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `seconds` | `number` | yes | Target position (≥ 0) |

**Returns:** `void`

**Errors:** `"No track loaded"`, seek failures for the current format.

---

### `set_volume`

Set output volume.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `volume` | `number` | yes | Linear volume from `0.0` (mute) to `1.0` (full) |

**Returns:** `void`

**Errors:** `"Volume must be between 0.0 and 1.0"` if out of range.

---

## Library & playlists

The default profile is `"default"` and the default playlist is `"All Local Files"`. Most library commands operate on that playlist unless noted.

### `add_track_to_playlist`

Read metadata from disk, upsert into SQLite, and append to the default playlist.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | `string` | yes | Absolute path to an audio file |

**Returns:** [`Track`](./types.md#track) (with extracted metadata)

**Errors:** file missing, unsupported extension, `"Track is already in the playlist"`.

---

### `remove_track_from_playlist`

Remove a track from the default playlist by file path. Does not delete the file from disk.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | `string` | yes | Absolute file path |

**Returns:** `void`

**Errors:** `"Track is not in the playlist"`.

---

### `get_playlist`

Load all tracks from the default persisted playlist, ordered by position.

**Arguments:** none

**Returns:** `Track[]`

---

### `clear_playlist`

Remove every track entry from the default playlist. Does not delete files or track records from the library database.

**Arguments:** none

**Returns:** `void`

---

### `play_track_from_playlist`

Play the track at a **zero-based index** in the default playlist. Also:

1. Loads the full playlist into the in-memory playback queue
2. Sets the queue’s current index
3. Pushes metadata to OS media controls

**Prefer this over `play_track`** when the user selects a song from the library UI.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `index` | `number` | yes | Zero-based index in the default playlist |

**Returns:** `void`

**Errors:** `"Track not found at index {n}"`, audio playback errors.

---

### `index_music_library`

Recursively scan a directory for supported audio files, extract metadata, and import into a playlist.

**Arguments**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `directory` | `string` | yes | — | Folder to scan |
| `profileId` | `string` | no | `"default"` | Profile id |
| `playlistName` | `string` | no | `"All Local Files"` | Target playlist name |

**Returns:** `Track[]` — only tracks **newly added** to the playlist during this scan (skipped/duplicate files are omitted)

**Errors:** `"Library path is not a directory"`. Individual file failures are logged server-side and skipped.

---

### `list_playlists`

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `profileId` | `string` | no | Filter to a single profile; omit for all profiles |

**Returns:** [`PlaylistInfo[]`](./types.md#playlistinfo)

---

### `get_library_database_path`

**Arguments:** none

**Returns:** `string` — absolute path to `wave-library.sqlite` in the app data directory

---

### `get_supported_audio_extensions`

**Arguments:** none

**Returns:** `string[]` — lowercase extensions without dots

```
aac, aiff, alac, caf, flac, m4a, m4b, m4p, mka, mkv, mp1, mp2,
mp3, mp4, oga, ogg, opus, wav, wave, weba
```

---

## Search & sources

Search escalates through three tiers: the current scope (filtered client-side),
the local library (`search_library`), and remote providers (`search_sources`).
Only the third reaches the network, and only on explicit user action. See
[Song sourcing](./sources.md).

### `search_library`

Tier 2. Realtime search across title, artist, album, filename, and lyrics, with
matched-field metadata and lyric snippets. Reads the `library_tracks` view, so
streamed-but-not-downloaded tracks never appear.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `query` | `string` | yes | Search text |
| `limit` | `number \| null` | no | Defaults to 80, capped at 200 |

**Returns:** `SearchHit[]`

---

### `search_sources`

Tier 3. Queries every configured remote provider concurrently and returns one
section per provider.

Returns an empty array immediately when `outside_sourcing_enabled` is off — no
provider is contacted. A provider that fails yields a section with `error` set
and `tracks` empty rather than failing the whole call.

Each result is annotated with `already_in_library`, the local path of a track
the user already owns, so the UI can mark it and play the local copy instead of
re-fetching.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `query` | `string` | yes | Search text |
| `limit` | `number \| null` | no | Per provider. Defaults to 20, capped at 50 |

**Returns:** `ProviderResults[]`

**Example**

```typescript
const sections = await invoke("search_sources", { query: "blackened", limit: 20 });
for (const section of sections) {
  if (section.error) console.warn(`${section.display_name}: ${section.error}`);
}
```

---

### `stream_source_track`

Fetch a remote result into the local cache and return it as a playable `Track`.
Play the returned `path` exactly as you would a local file.

Behavior depends on the result:

- **Full length** — cached to disk and registered as a library row flagged
  `source_state: "cached"`. Hidden from browse and search until downloaded.
- **Preview** (`is_full_length: false`) — cached to a session-scoped directory
  and returned with `source_state: "preview"`. **No database row is created**,
  and it is excluded from listening statistics and recently-played.

Safe to call repeatedly: an already-cached track is reused rather than
re-fetched.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `track` | `SourceTrack` | yes | A result from `search_sources` |

**Returns:** `Track`

**Errors (examples):** `"This recording isn't available for download"` (401/403),
`"The source is rate limiting — try again in a moment"` (429), resolve failures.

---

### `download_source_track`

Keep a remote track: copy it into the platform download destination and promote
its row into the library, making it browsable and searchable immediately with no
rescan.

Streams the track first if it is not already cached. Copies rather than moves,
so a track playing from the cache is never disturbed.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `track` | `SourceTrack` | yes | Must have `downloadable: true` |

**Returns:** `Track` — the promoted library row

**Errors:** rejects a result whose provider does not permit saving. May emit
[`source-download-fallback`](./events.md) when the intended folder is unusable.

---

### `get_source_settings`

**Arguments:** none

**Returns:** `SourceSettings`

---

### `set_source_settings`

Persist source settings. Blank credentials are stored as unset, which makes the
affected provider report itself as needing setup rather than failing.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `settings` | `SourceSettings` | yes | Full settings object |

**Returns:** `void`

---

## Favorites

**Favorites** is a special seeded playlist — just like "All Local Files" — that is created automatically on startup, shows up in `list_playlists`, and **cannot be deleted or renamed**. Use these commands to manage it; it also works with the generic by-id playlist commands (`get_playlist_tracks_by_id`, `play_track_from_specific_playlist`, etc.) using the playlist id from `list_playlists`.

### `add_track_to_favorites`

Read metadata from disk, upsert into SQLite, and append to the Favorites playlist. The track is added to the library if not already present.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | `string` | yes | Absolute path to an audio file |

**Returns:** [`Track`](./types.md#track)

**Errors:** file missing, unsupported extension, `"Track is already in the playlist"`.

---

### `remove_track_from_favorites`

Remove a track from the Favorites playlist by file path. Does not delete the file or the track record from the library.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | `string` | yes | Absolute file path |

**Returns:** `void`

**Errors:** `"Track is not in the playlist"`.

---

### `get_favorites`

List every track in the Favorites playlist, ordered by position.

**Arguments:** none

**Returns:** `Track[]`

---

### `is_track_in_favorites`

Whether a track (by file path) is in the Favorites playlist. Use this to render the heart toggle state in the UI.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | `string` | yes | Absolute file path |

**Returns:** `boolean`

---

### `toggle_favorite`

Toggle the favorite state of a track. Returns the **new** state.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | `string` | yes | Absolute file path |

**Returns:** `boolean` — `true` if now favorited, `false` if unfavorited

**Example**

```typescript
const nowFavorited = await invoke<boolean>("toggle_favorite", { path: track.path });
// update heart icon accordingly
```

**Errors:** when favoriting: file missing, unsupported extension. When unfavoriting: `"Track is not in the playlist"`.

---

### `clear_favorites`

Remove every track from the Favorites playlist. Does not delete files or track records from the library database.

**Arguments:** none

**Returns:** `void`

---

## Albums & artists

These commands let the frontend build Spotify-style browse views and “go to
album” / “go to artist” flows straight from file metadata — no playlists
required.

Albums are grouped by `(album, album_artist)` (falling back to the track
`artist` when the `album_artist` tag is missing), so same-named albums by
different artists stay separate. Artists are grouped by the track `artist`
tag.

### `list_albums`

List every distinct album in the library with aggregate info. Use this to
render an album grid.

**Arguments:** none

**Returns:** [`AlbumSummary[]`](./types.md#albumsummary)

```typescript
const albums = await invoke<AlbumSummary[]>("list_albums");
// albums[0] => { name: "Abbey Road", album_artist: "The Beatles",
//                track_count: 3, year: 1969, cover_art_data_url: "data:..." }
```

---

### `list_artists`

List every distinct artist with track and album counts. Use this to render an
artist list / discography index.

**Arguments:** none

**Returns:** [`ArtistSummary[]`](./types.md#artistsummary)

```typescript
const artists = await invoke<ArtistSummary[]>("list_artists");
// artists[0] => { name: "The Beatles", track_count: 12, album_count: 3 }
```

---

### `get_album_tracks`

Return every track belonging to an album — the backend of the “right-click a
song → go to album” flow.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `album` | `string` | yes | Album name (matches the track `album` tag) |
| `albumArtist` | `string` \| null | no | Resolved album artist. Pass `Track.album_artist ?? Track.artist` (or the `AlbumSummary.album_artist` value) to keep same-named albums apart. Omit to match the album name only |

**Returns:** `Track[]` — ordered by disc number then track number.

**Errors:** `"Album name cannot be empty"`.

```typescript
// From a clicked track:
const tracks = await invoke<Track[]>("get_album_tracks", {
  album: track.album,
  albumArtist: track.album_artist ?? track.artist,
});
```

When `albumArtist` is omitted, every track whose `album` tag matches is
returned, even across different artists.

---

### `get_artist_tracks`

Return every track by an artist (a discography) — the backend of the
“right-click a song → go to artist” flow. Matches the track `artist` tag.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `artist` | `string` | yes | Artist name (matches the track `artist` tag) |

**Returns:** `Track[]` — ordered by album, then disc number, then track number.

**Errors:** `"Artist name cannot be empty"`.

```typescript
const tracks = await invoke<Track[]>("get_artist_tracks", { artist: "The Beatles" });
```

---

### `create_album_playlist`

Create a new persisted playlist from every track matching an album name. Useful
when you want the album as a real playlist (e.g. to export, reorder, or keep).

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `album` | `string` | yes | Album name |
| `name` | `string` \| null | no | Playlist name; defaults to the album name. Auto-suffixed on collision |

**Returns:** [`PlaylistInfo`](./types.md#playlistinfo)

**Errors:** `"Album name cannot be empty"`, `"No tracks found for album \"{album}\""`.

---

### `create_artist_playlist`

Create a new persisted playlist from every track matching an artist name (a
discography playlist).

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `artist` | `string` | yes | Artist name |
| `name` | `string` \| null | no | Playlist name; defaults to the artist name. Auto-suffixed on collision |

**Returns:** [`PlaylistInfo`](./types.md#playlistinfo)

**Errors:** `"Artist name cannot be empty"`, `"No tracks found for artist \"{artist}\""`.

> `get_album_tracks` / `get_artist_tracks` are the read-only queries you’ll
> usually want for browse views. The `create_*_playlist` commands persist the
> same result into a playlist you can save/export.

---

## Queue & playback modes

The playback queue is **in-memory** and separate from the SQLite playlist. It is populated automatically when you call `play_track_from_playlist`.

### `get_queue`

**Arguments:** none

**Returns:** [`QueueState`](./types.md#queuestate)

---

### `play_next`

Advance to the next track in the queue.

**Behavior:**

- **`repeat: "one"`** — replays the current track
- **`repeat: "all"`** — wraps to the first track
- **`repeat: "off"`** — returns `null` at end of queue
- Respects shuffle order when enabled

**Arguments:** none

**Returns:** `string | null` — path of the newly playing track, or `null` if queue exhausted

---

### `play_previous`

Go to the previous track, with a common UX rule:

- If **more than 3 seconds** into the current track → seek to start (returns current path)
- Otherwise → previous queue entry (respects shuffle/repeat)

**Arguments:** none

**Returns:** `string | null` — path of the track now playing, or `null`

---

### `set_shuffle`

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `enabled` | `boolean` | yes | Turn shuffle on or off |

**Returns:** `void`

When enabled, rebuilds a shuffle order with the current track kept first.

---

### `set_repeat`

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `mode` | `string` | yes | `"off"`, `"one"`, or `"all"` |

**Returns:** `void`

**Errors:** `"Invalid repeat mode: {mode}"` for unknown values.

---

### `get_playback_mode`

**Arguments:** none

**Returns:** [`PlaybackMode`](./types.md#playbackmode)

---

## Equalizer

### `get_eq_settings`

Read the current equalizer configuration.

**Arguments:** none

**Returns:** [`EqSettings`](./types.md#eqsettings)

```typescript
const eq = await invoke<EqSettings>("get_eq_settings");
// eq.bands    → [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
// eq.enabled  → false
```

---

### `set_eq_bands`

Set gain for each of the 10 ISO frequency bands (31 Hz – 16 kHz). The EQ
chain must be enabled via `set_eq_enabled` for changes to be audible.

Gains are in **decibels (dB)** — positive values boost, negative values cut.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `bands` | `number[]` | yes | 10 floating-point gains in dB, one per band |

**Returns:** `void`

**Errors:** `"Expected exactly 10 EQ band values"`.

```typescript
await invoke("set_eq_bands", {
  bands: [2, 2, 0, -1, -1, 0, 1, 2, 2, 1],  // bass + treble smile
});
```

**ISO band – index mapping**

| Index | Frequency |
|-------|-----------|
| 0 | 31 Hz |
| 1 | 62 Hz |
| 2 | 125 Hz |
| 3 | 250 Hz |
| 4 | 500 Hz |
| 5 | 1 kHz |
| 6 | 2 kHz |
| 7 | 4 kHz |
| 8 | 8 kHz |
| 9 | 16 kHz |

---

### `set_eq_enabled`

Enable or disable the entire equalizer chain. When disabled, audio passes
through unprocessed (zero additional CPU cost).

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `enabled` | `boolean` | yes | `true` to apply EQ, `false` to bypass |

**Returns:** `void`

```typescript
await invoke("set_eq_enabled", { enabled: true });
```

---

### `export_eq_settings`

Export the current equalizer configuration to a JSON file on disk. The file
includes the band gains, enabled state, and frequency labels for reference.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | `string` | yes | Output file path (e.g. `"/path/to/my-eq.json"`) |
| `name` | `string` \| `null` | no | Optional user-facing name stored in the file |

**Returns:** `void`

**Errors:** file write failures, JSON serialization errors.

```typescript
await invoke("export_eq_settings", {
  path: "bass-boost.json",
  name: "My Bass Boost",
});
```

**Exported JSON format:**

```json
{
  "name": "My Bass Boost",
  "enabled": true,
  "bands": [4.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
  "frequencies": [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0]
}
```

---

### `import_eq_settings`

Load an EQ preset from a JSON file and apply it immediately. The file must
match the format produced by `export_eq_settings`.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | `string` | yes | Path to a `.json` EQ preset file |

**Returns:** `void`

**Errors:** file read errors, JSON parse errors, invalid band data.

```typescript
await invoke("import_eq_settings", {
  path: "bass-boost.json",
});
```

---

## OS media controls

### `update_media_metadata`

Push now-playing metadata to the OS media surface (Windows SMTC, macOS Control Center, Linux MPRIS).

Call when the displayed track changes or when playback starts, so the system UI shows title/artist/album/artwork.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `metadata` | [`MediaMetadata`](./types.md#mediametadata) | yes | Track info for the OS |

**Returns:** `void`

**Example**

```typescript
await invoke("update_media_metadata", {
  metadata: {
    title: "Song Title",
    artist: "Artist Name",
    album: "Album Name",
    duration_seconds: 240,
    cover_url: "data:image/jpeg;base64,...",
  },
});
```

If OS media controls failed to initialize at startup, this command succeeds silently (no-op).

See also [Events](./events.md) for inbound OS button presses.

---

## Window / app settings

### `get_close_action`

Return what the main window **close button** (title-bar ✕) does.

**Arguments:** none

**Returns:** [`CloseAction`](./types.md#closeaction) — `"quit"` or `"hide_window"`

**Example**

```typescript
const action = await invoke<CloseAction>("get_close_action");
// "quit" | "hide_window"
```

---

### `set_close_action`

Set the close-button behavior explicitly.

**Arguments**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `action` | [`CloseAction`](./types.md#closeaction) | yes | `"quit"` or `"hide_window"` |

**Returns:** [`CloseAction`](./types.md#closeaction) — the value that was saved

**Example**

```typescript
await invoke("set_close_action", { action: "hide_window" });
```

When set to `"hide_window"`, clicking close hides the window to the system tray; playback continues and the app can be reopened from the tray icon or taskbar. When set to `"quit"`, close exits Wave completely.

---

### `toggle_close_action`

Switch between `"quit"` and `"hide_window"` and persist the new preference.

**Arguments:** none

**Returns:** [`CloseAction`](./types.md#closeaction) — the new value after toggling

**Example**

```typescript
const action = await invoke<CloseAction>("toggle_close_action");
```

Use this from a settings toggle in the UI; call `get_close_action` on startup to initialize the control.
