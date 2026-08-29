# Song Sourcing — Three-Tier Search, Streaming, and Download

Date: 2026-08-29
Status: Approved

## Problem

Wave's search escalates through two tiers today: the current scope (playlist,
album, artist page) and the full local library. There is no third rung. A song
you do not own is simply not findable, and there is no path from "I searched for
it" to "it is in my library".

This adds a third tier — remote sources — plus the ability to stream a result
immediately or download it into the library.

## Constraints discovered in the codebase

- Tiers 1 and 2 already exist. `useLibrarySearch` fetches `search_library`;
  `App.tsx` filters those hits to the active scope via `hitMatchesSearchScope`
  and offers a "search the full library" button when the scope comes up empty.
  This work extends an existing ladder rather than inventing one.
- Playback is path-keyed end to end: `AudioPlayer::play(&str)` ->
  `SymphoniaSource::new(path)` -> `File::open`. Queue entries are paths and
  `settings.last_queue: Vec<String>` persists them. The engine has no concept
  of a URL.
- `enrichment.rs` is the precedent for outbound network calls: blocking
  `reqwest` on a dedicated background thread, rate limited, every failure
  degrading to "no data" rather than an error.
- Android's folder picker already requests and persists WRITE permission, but
  explicitly falls back to READ-only when the system rejects R+W, and no
  `createDocument` / `openOutputStream` JNI path exists yet.
- No tag-writing crate is present. `symphonia` reads tags; it does not write
  them.

## Decisions

1. **Providers: Deezer + free-audio catalogs.** Deezer is the discovery and
   metadata layer — free, unauthenticated search with strong artwork, but its
   public API exposes only 30-second preview audio. Full-length audio behind
   Deezer is encrypted and requires account-token decryption; that is DRM
   circumvention and is out of scope permanently. Jamendo and Internet Archive
   supply full-length, legally downloadable audio. FMA's API was deprecated
   after the 2018 Tribe of Noise acquisition and is treated as a spike, not a
   commitment.

2. **Manual escalation.** The source tier fires on an explicit button, matching
   the existing "search the full library" affordance. No network traffic until
   asked; no per-keystroke rate-limit pressure.

3. **Streamed tracks are real library rows**, flagged as remote. Queue, now
   playing, favorites, lyrics, EQ, and normalization then work with zero
   special-casing, and download becomes "copy the file, flip the flag".

4. **Cache-then-play.** Remote audio is fetched to a cache file and handed to
   the existing engine as a path. `SymphoniaSource`, seeking, gapless, DSP, and
   the Android ExoPlayer path are untouched. Progressive playback remains a
   later drop-in against the same cache layout. True HTTP `MediaSource` was
   rejected: seeks become re-buffers, duration reporting is murky before the
   whole file is seen, and Android gains nothing because ExoPlayer consumes
   URLs natively.

5. **Download destinations differ per platform.** Desktop writes to a dedicated
   Wave downloads folder; Android writes into the primary existing media
   folder. Both are auto-indexed. Android falls back to app-private storage
   when the SAF tree grant is read-only.

## Data model

Additive columns on `tracks`, via the existing `ALTER TABLE ... ADD COLUMN`
helper:

| column | meaning |
| --- | --- |
| `source_provider` | NULL for local files, else `deezer` / `jamendo` / `archive` |
| `source_id` | provider's track id |
| `source_url` | audio URL the bytes came from |
| `source_state` | NULL = local, `cached` = streamed only, `downloaded` = kept |
| `source_fetched_at` | cache LRU ordering |

`UNIQUE(source_provider, source_id)` makes re-streaming reuse a row.

Visibility is one rule: a row belongs to the library when
`source_state IS NULL OR source_state = 'downloaded'`. Because `library.rs`
scatters `FROM tracks` across browse, search, stats, and home suggestions, the
rule is enforced by a view rather than by a repeated predicate:

```sql
CREATE VIEW library_tracks AS
  SELECT * FROM tracks WHERE source_state IS NULL OR source_state = 'downloaded';
```

Browse, search, and stats read `library_tracks`. Only playback, the cache
manager, and the download promoter touch `tracks` directly.

## Provider abstraction

```rust
pub struct SourceTrack {
    pub provider: &'static str,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_seconds: Option<f64>,
    pub artwork_url: Option<String>,
    pub audio_url: Option<String>,
    pub is_full_length: bool,
    pub downloadable: bool,
    pub attribution: Option<String>,
}

pub trait SourceProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn search(&self, c: &Client, q: &str, limit: usize) -> Result<Vec<SourceTrack>, SourceError>;
    fn resolve_audio(&self, c: &Client, t: &SourceTrack) -> Result<String, SourceError>;
}
```

`is_full_length` and `downloadable` let the UI tell the truth without
hardcoding provider names: Deezer hits render a "30s preview" badge and no
download button; Jamendo and Archive hits render their license and a download
button. Providers are queried concurrently, each on its own thread with its own
rate limit; one provider failing never blocks another.

## Module layout

```
src-tauri/src/sources/
  mod.rs        trait, registry, SourceTrack, SourceError, concurrent fan-out
  deezer.rs     search -> metadata + 30s preview URL
  jamendo.rs    search -> full-length CC audio
  archive.rs    advancedsearch + file listing
  cache.rs      fetch, path layout, LRU eviction, size cap
  download.rs   fetch to destination, promote cached row, register folder
```

## Flows

**Search.** New hook `useSourceSearch.ts`, separate from `useLibrarySearch` so
each stays single-purpose; they share only the query string. Command
`search_sources(query, limit)` returns
`SourceHit { track, already_in_library: Option<String> }`. A hit matching the
library on normalized title + artist renders as "in your library" and plays the
local copy on click — marked, not hidden, so results never go mysteriously
missing.

**Stream.** `stream_source_track(provider, id)`: resolve URL, check cache by
`(provider, id)`, fetch to `<cache>/sources/<provider>/<id>.<ext>` via temp file
plus atomic rename, upsert a `source_state='cached'` row pointing at it, route
artwork through the existing `album_art` table, return the `Track`. Progress
rides the `app.emit` pattern already used in `commands.rs`.

**Download.** `download_source_track(provider, id)`: same fetch, destination per
platform. Two rules inside it:

- *Copy, do not move.* Promoting a cached row while that track is playing would
  pull the file out from under an open handle — survivable on Unix, a hard
  failure on Windows. Download copies, updates the row, and leaves the cache
  file for the next eviction pass.
- *No tag writing in v1.* Doing it properly means adding `lofty`, against the
  project's "no unnecessary dependencies" goal. Provider metadata lands in the
  DB row, which drives every screen. Additive later, confined to `download.rs`.

**Eviction.** Size cap in settings (512 MB default), LRU by
`source_fetched_at`, with a hard invariant that the currently-playing track and
anything in the live queue are never evicted. Only `source_state='cached'` rows
are ever touched.

## Error handling

Every failure degrades to "less", never to "broken":

| failure | behavior |
| --- | --- |
| one provider errors or times out | that section reports unavailable; others render |
| audio resolve or fetch fails | toast; no row inserted; no partial file on disk |
| fully offline | button works; all providers report unavailable |
| Android SAF write denied | fall back to app-private folder, toast the location |
| interrupted download | temp file discarded; cached row untouched |

## Testing

Fixture-driven, no network in the suite. Recorded JSON per provider covers
parsing plus malformed and empty responses. Cache tests cover eviction
ordering, cap enforcement, and the never-evict-playing invariant. Library tests
cover the `library_tracks` view, promotion, and the UNIQUE dedupe. Frontend
tests cover `useSourceSearch` debounce and request cancellation, mirroring the
existing `reqId` pattern. The SAF write path requires manual device
verification.

## Out of scope for v1

Playlist import from providers, remote artist/album browsing, remote "related
tracks", and bulk/background downloading. Each is additive once the trait and
cache exist.
