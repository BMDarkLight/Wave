# Song sourcing (remote search tier)

Search in Wave escalates through three tiers:

1. **Scope** — the playlist, album, or artist page you are looking at.
2. **Library** — every indexed local file (`search_library`).
3. **Sources** — remote providers (`search_sources`).

Tiers 1 and 2 are local and run as you type. **Tier 3 never runs on its own**: it
fires only when the user presses the escalation button, and only while the
master switch in Settings is on. Nothing in this subsystem touches the network
otherwise.

---

## Design constraints

Two facts about the rest of the app shape everything here.

**Playback is path-keyed end to end.** `AudioPlayer::play(&str)` →
`SymphoniaSource::new(path)` → `File::open`. Queue entries are paths, and
`settings.last_queue` persists them. The engine has no concept of a URL, so
remote audio becomes playable by **becoming a file**: it is fetched to a cache
file and handed to the existing engine as a path. Seeking, gapless, the
equalizer, volume normalization, and the Android ExoPlayer route are all
untouched by this feature.

**Network calls follow the `enrichment.rs` doctrine.** Every call is blocking
and runs on a background thread, never on a command's response path. Every
failure degrades to "this provider found nothing" rather than failing the
search. One provider being down, slow, or unconfigured never blocks another.

---

## Providers

| Provider | Setup | Audio | Downloadable |
|----------|-------|-------|--------------|
| **Deezer** | none | 30-second previews only | no |
| **Internet Archive** | none | full length | yes |
| **Jamendo** | free client ID | full length | per-track licence |
| **Spotify** | client ID field exists | **none — see below** | no |

### Deezer

Free, unauthenticated search with the best metadata and artwork of the three.
Its API exposes audio only as a `preview` field: a 30-second MP3. Full-length
Deezer audio is encrypted and requires account-token decryption, which is out of
scope permanently. This is why `is_full_length` and `downloadable` are always
`false` for Deezer results, and why the UI shows a "30s preview" badge and no
download button.

Deezer's `duration` field reports the **full** track length (e.g. 230s for a
track whose preview is 30s). The provider deliberately overrides it, because
reporting the full length would make the seek bar lie about a clip that stops a
quarter of the way in.

### Internet Archive

Search is **item-level**, and an Archive item is often a whole concert or album
rather than a single track. Search returns one row per item, and audio is
resolved lazily in `resolve_audio` so a search stays one request per provider.
The trade-off is that a result represents "the first audio file in this item".

The search query carries two filters, both indexed fields, so neither costs an
extra request:

- `collection:(audio_music OR etree)` — without it a `mediatype:(audio)` search
  returns podcasts, lectures, and YouTube rips. A search for "blackened" ranked
  a talk-radio episode above any music.
- `NOT access-restricted-item:true` — the Archive serves 401/403 for these
  items' files, but they are indistinguishable from playable ones in search
  results, so they used to fail only at the moment of playback.

Archive items can be very large — a single 167 MB file for a 2.5-hour concert is
normal. See [Known limits](#known-limits).

### Jamendo

Full-length Creative Commons audio. Requires a free client ID from
`developer.jamendo.com`. Without one the provider reports
`SourceError::NotConfigured`, which the UI renders as a setup hint rather than a
failure — it never silently disappears from the results.

Tracks whose licence permits it use Jamendo's download URL; the rest fall back
to the streaming URL and are marked not downloadable.

### Spotify

**Spotify's API serves no audio through any endpoint.** Playback exists only
inside their SDKs, is Premium-gated, and is DRM-enforced; their Developer Terms
prohibit caching or downloading audio. No API key can make Wave stream or cache
a Spotify track.

A client-ID field exists in Settings for a future **discovery** integration —
catalogue search and playlist import, matched against the local library or a
full-length free source. **The OAuth flow is not implemented yet**, and the
settings copy states the limitation explicitly.

---

## Provider abstraction

```rust
pub trait SourceProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn rate_limit(&self) -> Duration { Duration::from_millis(0) }
    fn search(&self, c: &Client, q: &str, limit: usize) -> Result<Vec<SourceTrack>, SourceError>;
    fn resolve_audio(&self, c: &Client, t: &SourceTrack) -> Result<String, SourceError>;
}
```

`search_all` fans out to every provider concurrently, one thread each, with a
per-provider rate limit. Results come back in registry order as
`ProviderResults`, where `error: Some(..)` with an empty `tracks` is the normal
degraded case — that section renders as unavailable while the others still show
results.

`SourceTrack::is_full_length` and `downloadable` are what let the UI describe a
result truthfully without hardcoding provider names anywhere.

Adding a provider means one file implementing the trait plus a line in
`providers()`.

---

## Storage model

Streamed **full-length** tracks become real rows in `tracks`, flagged as remote.
Everything downstream — queue, now playing, favorites, lyrics, EQ,
normalization — then works with no special-casing, and downloading is just
"copy the file and flip the flag".

### Columns added to `tracks`

| Column | Meaning |
|--------|---------|
| `source_provider` | `NULL` for local files, else `deezer` / `jamendo` / `archive` |
| `source_id` | The provider's track id |
| `source_url` | Audio URL the bytes came from |
| `source_state` | `NULL` local, `cached` streamed only, `downloaded` kept |
| `source_fetched_at` | Cache LRU ordering |

`UNIQUE(source_provider, source_id)` makes re-streaming reuse a row instead of
duplicating it.

### The `library_tracks` view

A row is library content when
`source_state IS NULL OR source_state = 'downloaded'`. Because `library.rs`
scatters `FROM tracks` across browse, search, stats, and Home suggestions, that
rule is enforced by a view rather than a repeated predicate:

```sql
CREATE VIEW library_tracks AS
  SELECT * FROM tracks WHERE source_state IS NULL OR source_state = 'downloaded';
```

Browse, search, and aggregate queries read `library_tracks` via
`LIBRARY_TRACK_FROM`. Only playback, the cache manager, and the download
promoter touch `tracks` directly via `TRACK_FROM` — a cached track must stay
resolvable by id or path so it can still play.

Two related invariants:

- **Cached rows are excluded from tag-based deduplication.** `deduplicate_tracks`
  partitions by `(artist, album, title)` and keeps the earliest `indexed_at`, so
  without this a streamed track could delete a local file's row.
- **Cached rows are not in the FTS index.** Tier 2 means "my library", and
  something merely streamed is not that. The index entry is added on promotion
  to `downloaded`.

### Previews are not library content

A 30-second preview gets **no `tracks` row at all**. It is held in an in-memory
registry keyed by file path and cached in a separate, session-scoped directory
that is wiped at startup.

This keeps previews out of browse, search, counts, listening statistics, and
recently-played — a cached row would otherwise skew top artists and Home
suggestions after a single preview tap.

Previews still display correctly everywhere because the queue and player bar
already fall back to `placeholder_track` for a path with no row; that function
consults the preview registry first.

---

## Cache and eviction

| Location | Contents | Lifetime |
|----------|----------|----------|
| `<data>/source-cache/<provider>/` | Full-length streamed audio | Until evicted |
| `<data>/source-cache/previews/` | Preview clips | Wiped at startup |

Downloads are written through a temp file plus an atomic rename, so an
interrupted fetch can never leave a half-written file that Symphonia would fail
to probe.

Eviction is LRU by `source_fetched_at`, bounded by `source_cache_limit_mb`
(default 512). The currently playing track and everything in the live queue are
never evicted — deleting one would pull a file out from under an open decoder.
When every cached file is protected the cap is exceeded rather than violating
that rule. Only `source_state = 'cached'` rows are ever touched, so a download
can never be evicted.

---

## Downloads

Destination differs by platform:

- **Desktop** — `<data>/Downloads/<Artist>/<Album>/<Title>.<ext>`, registered as
  a media folder on first use so the file is browsable immediately even if no
  media folder was ever configured.
- **Android** — the primary existing media folder, so downloads sit with the
  rest of the user's music.

Two rules inside the download path:

**Copy, do not move.** Promoting a cached file while that track is playing would
pull the file out from under an open decoder — survivable on Unix, a hard
failure on Windows. The download copies, updates the row, and leaves the cache
copy for the next eviction pass.

**No tag writing.** Doing it properly would mean adding a tag-writing
dependency, against the project's "no unnecessary dependencies" goal. Provider
metadata lands in the DB row, which drives every screen.

Filenames are sanitised per component and de-duplicated with a `-1`, `-2`
suffix, so a second download of the same title never overwrites the first.

---

## Error handling

Every failure degrades to "less", never to "broken".

| Failure | Behavior |
|---------|----------|
| One provider errors or times out | That section reports unavailable; others render |
| Provider not configured | Section shows a setup hint |
| Audio resolve or fetch fails | Toast; no row inserted; no partial file on disk |
| Fully offline | Button works; every provider reports unavailable |
| Android destination not writable | Falls back to app-private storage and emits `source-download-fallback` |
| Interrupted download | Temp file discarded; cached row untouched |

HTTP failures are rendered as sentences rather than raw reqwest errors, which
used to dump a full CDN URL into a toast.

---

## Playback compatibility note

Deezer previews ship an **ID3v2.4 header declaring a body size of zero**.
Symphonia's probe walks off the end of that degenerate tag and fails the whole
file with `UnexpectedEof("out of bounds")`, even though the audio behind it is a
complete, valid MP3.

This is handled in the decoder rather than here, because any file with such a
tag fails regardless of origin: on probe failure `SymphoniaSource` re-probes
past a leading ID3v2 tag through an offset media source that rebases absolute
seeks. See `audio/symphonia_source.rs`.

---

## Known limits

- **No progressive playback.** A track downloads fully before it starts. That is
  imperceptible for a 480 KB preview and fine for a typical Jamendo track, but a
  167 MB Archive concert takes over a minute before the first note. The cache
  layout was designed so progressive playback can be added without reworking
  eviction or naming.
- **Archive results are item-level**, so a result is the first audio file in an
  item rather than a specific track.
- **Android SAF writes are not implemented.** Downloads reach the primary media
  folder only when it is a plain filesystem path. When it is a `content://` SAF
  tree — the common case on modern Android — the download falls back to
  app-private storage and the user is told where it went.
- **Spotify OAuth is not implemented**, and no Spotify integration can ever
  provide audio.

---

## Testing

The suite is offline and fixture-driven: recorded JSON per provider covers
parsing plus malformed and empty responses; cache tests cover eviction ordering,
the cap, and the never-evict-playing invariant; library tests cover the
`library_tracks` view, promotion, and dedup safety.

Live network checks are `#[ignore]`d and opt-in:

```bash
cargo test --lib -- --ignored --nocapture
```

These **fetch and decode through the real playback engine** rather than
asserting response shape. That distinction matters: two production bugs reached
the running app precisely because the earlier versions checked header bytes
without decoding, and checked a URL string without fetching it.

To diagnose a file that will not play:

```bash
WAVE_PROBE_FILE=/path/to/file.mp3 cargo test --lib probe_file -- --ignored --nocapture
```
