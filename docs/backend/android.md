# Android Playback

Wave on Android uses a **hybrid** audio stack: Rust still owns the queue and transport policy, while **Media3 ExoPlayer** handles decode and output. Desktop continues to use Rodio + Symphonia + CPAL.

## Mental model

| Concern | Owner | Notes |
|---------|--------|-------|
| Playback queue, shuffle, repeat | Rust `AudioPlayer` (`src-tauri/src/audio/player.rs`) | Same command surface as desktop |
| Decode / output | ExoPlayer via JNI | `content://` and `file://` URIs play natively |
| Lock-screen / notification controls | `tauri-plugin-media-session` + `MediaNativeBridge` | Driven over JNI in both directions, no Activity needed |
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
├── media_bridge.rs     # Media session <-> Rust over JNI (Android-only)
├── lifecycle.rs        # Keep playing when the UI is destroyed; reopen the window
└── audio/
    ├── mod.rs
    └── jni_bridge.rs   # JNI to WaveExoPlayer
```

Java shipped into the APK (copied by CI into `gen/android`):

```text
src-tauri/android-src/java/app/bmdarklight/wave/
├── FolderPickerCallback.java
├── SafMediaScanner.java
├── MediaNativeBridge.java    # JNI bridge to the media session, both directions
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

`WaveExoPlayer` is deliberately a plain ExoPlayer holder, not a `MediaSessionService`, so it does not fight the plugin for the session or notification.

All session state lives in `MediaSessionState`, a process-wide Kotlin object that runs on the main thread and outlives the Activity. The Tauri plugin class is only a thin adapter for the guest JS API.

Rust to session (now playing, play/pause, position, shuffle/repeat):

1. `MediaBridgeState` (`integrations/media_controls.rs`)
2. `android::media_bridge::session_update` / `session_timeline` / `session_clear`
3. JNI → `MediaNativeBridge` → `MediaSessionBridge` → `MediaSessionState`

This path does not use Tauri plugin commands on purpose: those dispatch through the Activity and panic once it has been destroyed. `tick_media_session` re-reads the engine every tick so the session stays right with the WebView frozen or gone; position reports within a second of what the session already extrapolates are dropped.

Session to Rust (notification, lock screen, Bluetooth/AVRCP, headset buttons, audio focus):

1. `MediaSessionCompat.Callback` / `MediaActionReceiver` / `WaveExoPlayer` listener → `MediaNativeBridge.dispatch(action)`
2. JNI → `android::media_bridge` worker thread
3. `commands::handle_native_media_action`

ExoPlayer handles audio focus and headphone unplugs itself. When it pauses on its own it dispatches `pause`; during a transient focus loss (a call) it dispatches `suspend` / `unsuspend`, which only freeze the Rust transport clock because ExoPlayer resumes by itself.

Notification behaviour:

- Updates are coalesced (a track change is several calls) and held up to 500 ms for the new cover, so the shade never flashes the old art under a new title.
- The small icon is `drawable/ic_notification` from `android-src/res`. Without it the notification falls back to the template's `ic_launcher_foreground`, which is the stock Android robot.
- Paused: the service stays in the foreground for 10 minutes (as Media3 does), then leaves it and the notification becomes dismissable. Dismissing it pauses, clears the controls and keeps the queue.

## Closing the app while playing

Swiping Wave out of recents destroys the Activity, and Tauri treats the last window closing as an exit. `android::lifecycle` prevents that exit while something is playing, so the foreground service keeps the process and the music going. When the user comes back, Android starts a new Activity and `RunEvent::Resumed` builds a new main window for it. If nothing is playing the app exits as before, after saving the playback state.

Because the UI can disappear under a running engine, `ndk_context` holds the Application context rather than the Activity. Code that needs a real Activity (the folder picker) asks tao for the live one with `android::jni::live_activity`.

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

1. Copies `android-src/java` into `gen/android/app/src/main/java` and `android-src/res` into the app resources
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
