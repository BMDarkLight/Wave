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

Auto-advance, notification buttons, and `play_next` / `play_previous` all go through the Rust queue. With gapless playback on, Rust hands ExoPlayer the upcoming tracks as a playlist so transitions are seamless, and reads each item change back; the order is still decided in Rust.

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
├── saf_io.rs           # Copy bytes in and out of SAF documents (tag edits, imports)
├── media_bridge.rs     # Media session <-> Rust over JNI (Android-only)
├── lifecycle.rs        # Keep playing when the UI is destroyed; reopen the window
├── crash_report.rs     # Read the last crash report (shown in the app, no adb)
└── audio/
    ├── mod.rs
    └── jni_bridge.rs   # JNI to WaveExoPlayer
```

Java and resources shipped into the APK (applied to `gen/android` by `scripts/configure-android.sh`):

```text
src-tauri/android-src/
├── java/app/bmdarklight/wave/
│   ├── CrashInitProvider.java    # Runs at process start: crash handler, session context, UiLifecycle
│   ├── CrashReporter.java        # Saves crashes and handled errors for the in-app report
│   ├── FolderPickerCallback.java # SAF folder picker
│   ├── KeyboardInsets.java       # Keeps inputs above the soft keyboard
│   ├── MediaMetadataProbe.java   # MediaMetadataRetriever for URI metadata + art
│   ├── MediaNativeBridge.java    # JNI bridge to the media session, both directions
│   ├── SafMediaScanner.java      # Recursive audio listing under a SAF tree
│   ├── UiLifecycle.java          # Rebuilds the UI for a new Activity after background playback
│   └── audio/
│       ├── WaveExoPlayer.java    # Media3 ExoPlayer singleton, audio focus, EQ, crossfade
│       └── PeakAnalyzer.java     # Loudness and waveform analysis
├── res/drawable/ic_notification.xml  # Status bar / notification icon (the Wave mark)
└── proguard-wave.pro             # Keep rules for everything native code calls over JNI
```

The media session itself is the vendored plugin in `src-tauri/vendor/tauri-plugin-media-session/android/` (Kotlin): `MediaSessionState` (session, notification, publishing), `MediaSessionCleanupService` (the foreground service), `MediaActionReceiver` (notification buttons), `MediaSessionBridge` (entry points for the app's Java) and `MediaSessionPlugin` (the Tauri-facing adapter).

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
- Held for a call (transient focus loss): the session shows paused but the service stays in the foreground. ExoPlayer resumes on its own when the call ends, with no user action, and Android refuses to start a foreground service from the background without one. `WaveExoPlayer` flags the hold to `MediaSessionState` before telling the engine.
- Paused: the service leaves the foreground right away and the notification stays, now dismissable (Media3's `stopForeground(false)`). Resuming from the notification or a media button gets a foreground-start exemption from the system; if Android still refuses, playback runs and the notification is posted without the foreground guarantee.
- Dismissing the paused notification, or a stop from a car, headset or the lock screen, pauses, clears the controls and keeps the queue. With the UI closed, the process then exits.

## Closing the app while playing

Swiping Wave out of recents destroys the Activity, and Tauri treats the last window closing as an exit. `android::lifecycle` prevents that exit while something is playing. The foreground service then keeps the process alive; Android does not kill a process with a running foreground service when its task is removed. If nothing is playing the app exits as before, after saving the playback state, and the notification is removed synchronously on the way out.

While the process runs without a window ("headless"):

- **Nothing may dispatch through the Activity.** On Android, Tauri plugin commands and `app.path()` go through the Activity and panic once it is gone; that panic aborts the process and stops the music. Use `app_paths::app_data_dir` / `app_cache_dir` (resolved once at startup), drive the media session over JNI, and guard plugin calls such as `saf_io::open`.
- **Reopening** starts a new Activity in the same process. `UiLifecycle` (Java) reports it with a `ui-started` action and `lifecycle::ensure_main_window` builds a new main window for it; the playback tick retries as a fallback. Tauri's own resume event is per window, so with no window it never fires. `UiLifecycle` also unbinds Tauri's `PluginManager` from the destroyed Activity so file dialogs and permission prompts work in the new one.
- **The rebuilt webview reuses the id `main`.** wry tears an Activity's webview down later, on the UI thread, by that id. Android can destroy the old Activity only after starting the new one, and a teardown that ran after the rebuild would remove the new webview's asset and IPC handlers and leave a blank page. `ensure_main_window` makes a round trip through wry's UI-thread queue first, so any teardown still waiting there runs before the build.
- **Ending**: dismissing the paused notification, or a media-session stop, exits the process.
- The playback tick and the media-action worker catch panics, since with no UI they are what keeps auto-advance and the controls working.

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

## Building

`src-tauri/gen/android` is generated by `tauri android init` and is not the source of truth for any Java, Kotlin or resource. A stock init has no audio engine; `scripts/configure-android.sh` turns it into Wave's app:

1. Copies `android-src/java` into the app's sources, and the launcher icons plus `android-src/res` into its resources
2. Hooks `proguard-wave.pro` into the release build's ProGuard rules
3. Adds the permissions (`FOREGROUND_SERVICE_MEDIA_PLAYBACK`, `POST_NOTIFICATIONS`, `READ_MEDIA_AUDIO`, `WAKE_LOCK`, ...), the portrait lock and `CrashInitProvider` to `AndroidManifest.xml`
4. Adds the Media3 dependencies (`media3-exoplayer`, `media3-common`) to the app Gradle file

It is safe to run more than once. CI (`.github/workflows/android.yml`) runs it after `tauri android init`, and so should a local build; run it again after editing anything under `android-src/`, since the copies in `gen/android` are what gets built. The README's [Getting started](../../README.md#getting-started) has the full local steps, including release signing with `scripts/setup-android-signing.sh`.

The Rust crate builds for Android with the usual targets (`aarch64-linux-android`, `armv7-linux-androideabi`). To check it compiles without a full APK build:

```bash
cargo check --manifest-path src-tauri/Cargo.toml --target aarch64-linux-android --lib
```

That needs the NDK's clang as the C/C++ compiler and linker for that target (`CC_aarch64_linux_android`, `CXX_aarch64_linux_android`, `AR_aarch64_linux_android`, `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER`); `oboe-sys` fails to build without the `CXX_` one. The Tauri CLI sets these itself for `tauri android build`.

## Checking background playback on a device

None of this can be exercised in the browser or by unit tests. After changing anything in the playback, media session or lifecycle code, check on a phone:

1. Play, lock the screen, wait for a track change: the next track starts and the notification follows.
2. Play, swipe Wave out of recents: music continues and the notification stays.
3. From there, open Wave again: the UI comes back on what is playing.
4. Pause from the notification and swipe the notification away: Wave exits (nothing left in recents or the shade), and the queue is there on the next launch.
5. Take a call mid-song and hang up: the music resumes by itself.
6. Unplug headphones or disconnect Bluetooth: playback pauses, and the notification shows paused.
7. Bluetooth headset or car: play/pause/next work and the head unit shows title and artist.

Crashes and handled errors are saved by `CrashReporter` and shown in the app on the next launch, so a report can be copied without adb.

## What is *not* on Android ExoPlayer

- Desktop-style Rodio biquad EQ chain — Android uses the system `Equalizer`
  AudioEffect attached to ExoPlayer's audio session instead (same 10-band UI
  gains, mapped onto the device's native band layout).
- Shuffle and queue order: Rust owns those; ExoPlayer only ever sees the upcoming items in order.
- Replacing the media-session plugin with Media3 `MediaSessionService`: intentionally deferred.
- A `MediaBrowserService`: without one, Android's "resume playback" card and a Bluetooth play press cannot restart Wave after Android has killed the process, and Android Auto cannot browse the library.

## Related docs

- [Backend overview](./README.md) — command surface shared by all platforms
- [Commands](./commands.md) — invoke reference (`pick_media_folder`, `scan_saf_folder`, playback, …)
- [DSP](./dsp.md) — desktop Rodio equalizer pipeline
