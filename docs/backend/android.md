# Android Playback

Wave on Android uses a **hybrid** audio stack: Rust still owns the queue and transport policy, while **Media3 ExoPlayer** handles decode and output. Desktop continues to use Rodio + Symphonia + CPAL.

## Mental model

| Concern | Owner | Notes |
|---------|--------|-------|
| Playback queue, shuffle, repeat | Rust `AudioPlayer` (`src-tauri/src/audio/player.rs`) | Same command surface as desktop |
| Decode / output | ExoPlayer via JNI | `content://` and `file://` URIs play natively |
| Lock-screen / notification controls | `tauri-plugin-media-session` + `MediaNativeBridge` | Actions drain into Rust on the GUI tick |
| Library / metadata | SQLite + MediaMetadataRetriever (URI) / Symphonia (files) | Folder scans store SAF URIs — **no audio copies** |

```text
UI / invoke commands
        │
        ▼
  AudioPlayer (Rust)  ── queue, next/prev, repeat, auto-advance
        │
        │  play / pause / seek / volume / ended?
        ▼
  android::audio (JNI) ── WaveExoPlayer.getOrCreate(Context)
        │
        ▼
  Media3 ExoPlayer     ── system audio focus, noisy-audio handling
```

Auto-advance, notification buttons, and `play_next` / `play_previous` all go through the Rust queue. ExoPlayer is **not** given a playlist of its own; it plays one URI at a time.

## Zero-copy library model

Media source folders (SAF trees) and direct file picks index **`content://` URIs** as `tracks.path` with `is_saf_uri = true`. Playback and metadata use the URI in place:

| Step | Behavior |
|------|----------|
| Scan | `scan_saf_folder` lists document URIs under the tree |
| Index | `import_scanned_audio` / `sync_playlist_folder` store URIs (via `resolve_library_source`) — no copy into `imports/` |
| Metadata | `MediaMetadataProbe` (JNI `MediaMetadataRetriever`) for `content://`; Symphonia for real files |
| Cover art | Embedded picture from the probe → resize → `data:` URL / disk cache (`cover_art.rs`). Audio itself is never duplicated |
| Play | `resolve_playback_source` keeps `content://` for ExoPlayer |

`imports/` is **legacy**. After a successful URI folder scan or sync, the UI calls `clear_audio_imports` to remove old materialized copies. You can also invoke that command manually.

Materialize remains only as a last-resort fallback if a URI cannot be opened for metadata.

## Custom playlists

- **Add** opens a searchable library picker (`search_library_tracks`) so members link existing indexed tracks.
- **Pick a file…** still available; on Android the picked URI is indexed zero-copy (same as folders).
- Library playlist **+** stays “scan media folder” (source folders), not the search picker.

## Why ExoPlayer on Android

- Rodio/CPAL/oboe struggled with SAF `content://` URIs and background reliability.
- ExoPlayer streams `content://` directly — **playback does not require copying** the file into app-private storage.
- Audio focus and “becoming noisy” (headphones unplugged) are handled by Media3.

## Source layout

Rust (crate module `android`):

```text
src-tauri/src/android/
├── mod.rs
├── jni.rs              # Seed ndk_context from tao; JVM attach helpers
├── import.rs           # resolve_library_source / resolve_playback_source
├── metadata.rs         # JNI → MediaMetadataProbe for content://
├── folder_picker.rs    # SAF folder picker (JNI → FolderPickerCallback)
├── saf_scan.rs         # Recursive audio listing under a SAF tree URI
├── media_bridge.rs     # Notification actions → Rust (Android-only)
└── audio/
    ├── mod.rs
    └── jni_bridge.rs   # JNI to WaveExoPlayer
```

Java shipped into the APK (copied by CI into `gen/android`):

```text
src-tauri/android-src/java/app/bmdarklight/wave/
├── FolderPickerCallback.java
├── SafMediaScanner.java
├── MediaNativeBridge.java
├── MediaMetadataProbe.java   # MediaMetadataRetriever for URI metadata + art
└── audio/
    └── WaveExoPlayer.java      # Media3 ExoPlayer singleton
```

ProGuard keep rules: `src-tauri/android-src/proguard-wave.pro`.

## Playback path

1. Frontend calls the same commands as desktop (`play_track`, `play_tracks`, `pause_track`, …).
2. `resolve_playback_source` returns `content://` URIs unchanged on Android; other paths may still be materialized.
3. `AudioPlayer::play` on Android calls `android::audio::exo_play_uri` instead of opening a Rodio sink.
4. Position / duration / playing / ended are read from ExoPlayer; queue state stays in Rust.
5. A background tick (same as desktop) calls `tick_auto_advance` and drains `media_bridge` actions so transport works when the WebView is frozen.

## Media session vs ExoPlayer

Wave keeps **one** MediaSession owner for the notification UI: the vendored `tauri-plugin-media-session` plugin (MediaSessionCompat + foreground service).

`WaveExoPlayer` is deliberately a plain ExoPlayer holder — not a `MediaSessionService` — so it does not fight the plugin for the session or notification.

Notification button presses:

1. Plugin → `MediaNativeBridge.dispatch(action)`
2. JNI → `android::media_bridge`
3. Tick loop → `commands::handle_native_media_action`

## Cover art on Android

- Embedded art comes from `MediaMetadataProbe` for URIs (or Symphonia for files).
- Large embeds are **resized** (target ~200 KiB) and stored as `data:` URLs in SQLite; a disk cache under app data is written when possible.
- Online Cover Art Archive enrichment runs only when no embedded art is available.
- The UI shows a small image icon when `cover_art_source === "cover-art-archive"`.

## SAF: folders and scanning

| Command | Role |
|---------|------|
| `pick_media_folder` | Opens the system SAF tree picker; returns a persistable `content://…/tree/…` URI |
| `scan_saf_folder` | Lists audio document URIs under that tree (JNI `SafMediaScanner`) |
| `import_scanned_audio` / `sync_playlist_folder` | Index URIs in batches (zero-copy); progress UI still applies |
| `search_library_tracks` | Search indexed tracks for playlist membership |
| `clear_audio_imports` | Delete legacy files under app `imports/` |

`tauri-plugin-fs` `readDir` cannot walk SAF trees; always use `scan_saf_folder` on Android.

Full-device MediaStore browsing (all phone music without picking a folder) is out of scope — media sources remain user-picked SAF trees.

## CI / packaging notes

`.github/workflows/android.yml` after `tauri android init`:

1. Copies `android-src/java` into `gen/android/app/src/main/java`
2. Injects Media3 dependencies (`media3-exoplayer`, `media3-common`) into the app Gradle file
3. Patches permissions + portrait lock into `AndroidManifest.xml`
4. Hooks `proguard-wave.pro` for JNI entry points including `WaveExoPlayer` and `MediaMetadataProbe`

Local `gen/android` is generated and not the source of truth for Java/Kotlin.

## What is *not* on Android ExoPlayer

- Desktop-style Rodio biquad EQ chain — Android uses the system `Equalizer`
  AudioEffect attached to ExoPlayer's audio session instead (same 10-band UI
  gains, mapped onto the device's native band layout).
- ExoPlayer multi-item queue / shuffle — Rust owns that.
- Replacing the media-session plugin with Media3 `MediaSessionService` — intentionally deferred.

## Related docs

- [Backend overview](./README.md) — command surface shared by all platforms
- [Commands](./commands.md) — invoke reference (`pick_media_folder`, `scan_saf_folder`, playback, …)
- [DSP](./dsp.md) — desktop Rodio equalizer pipeline
