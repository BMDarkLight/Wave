<div align="center">

# Wave

**A lightweight, cross-platform, offline-first music player.**

Local library, real audio DSP, synced lyrics, and an opt-in tier of free online
catalogs — in a single portable app built on Rust + Tauri + React.

[![Build](https://github.com/BMDarkLight/Wave/actions/workflows/build.yml/badge.svg)](https://github.com/BMDarkLight/Wave/actions/workflows/build.yml)
[![Android Build](https://github.com/BMDarkLight/Wave/actions/workflows/android.yml/badge.svg)](https://github.com/BMDarkLight/Wave/actions/workflows/android.yml)
[![Rust](https://github.com/BMDarkLight/Wave/actions/workflows/rust.yml/badge.svg)](https://github.com/BMDarkLight/Wave/actions/workflows/rust.yml)
[![Frontend](https://github.com/BMDarkLight/Wave/actions/workflows/frontend.yml/badge.svg)](https://github.com/BMDarkLight/Wave/actions/workflows/frontend.yml)
[![License: AGPL v3](https://img.shields.io/badge/License-AGPL%20v3-blue.svg)](LICENSE)

<img src="docs/screenshots/desktop-home.png" alt="Wave on desktop — Home" width="100%">

</div>

---

## Table of contents

- [Installation](#installation)
- [Highlights](#highlights)
- [Features](#features)
  - [Home & discovery](#home--discovery)
  - [Library, albums & artists](#library-albums--artists)
  - [Metadata editor](#metadata-editor)
  - [Three-tier search](#three-tier-search)
  - [Queue & playback](#queue--playback)
  - [Lyrics](#lyrics)
  - [Audio engine & DSP](#audio-engine--dsp)
  - [Playlists, favorites & folder sync](#playlists-favorites--folder-sync)
  - [Listening stats](#listening-stats)
  - [Desktop & OS integration](#desktop--os-integration)
  - [Command line](#command-line)
- [Android](#android)
- [Screens](#screens)
- [Tech stack](#tech-stack)
- [Project structure](#project-structure)
- [Getting started](#getting-started)
- [Design goals](#design-goals)
- [License](#license)

---

## Installation

Wave installs as a normal desktop app and puts a `wave` command on your `PATH`.

| Platform    | Install                                                         |
| ----------- | --------------------------------------------------------------- |
| Debian/Ubuntu | `sudo apt install ./Wave_0.5.0_amd64.deb`                      |
| Fedora/RHEL | `sudo dnf install ./Wave-0.5.0-1.x86_64.rpm`                     |
| Arch        | `yay -S wave-music-player-bin`                                   |
| Flatpak     | `flatpak install app.bmdarklight.wave`                           |
| AppImage    | download, `chmod +x`, run                                        |
| macOS       | `brew install --cask wave`, or the `.dmg`                        |
| Windows     | `winget install BMDarkLight.Wave`, `choco install wave`, or the installer |

Grab the packages from the [latest release](https://github.com/BMDarkLight/Wave/releases).

See **[docs/INSTALL.md](docs/INSTALL.md)** for per-platform details, how the
`wave` command gets onto `PATH` on each system, and where the packaging sources
live.

---

## Highlights

| | |
|---|---|
| 🎧 **Offline-first** | Your library is local files plus a SQLite index. No account, no server, no telemetry. |
| ⚡ **Native audio** | Rodio + Symphonia + CPAL on desktop, Media3 ExoPlayer on Android. No web audio stack. |
| 🔍 **Three-tier search** | Filters the current view, then the whole library (lyrics included), then — only if you ask — the internet. |
| 🎚️ **Real DSP** | 10-band biquad equalizer, crossfade, gapless playback, and loudness normalization. |
| 📝 **Synced lyrics** | Auto-fetched from LRCLIB, highlighted line by line, importable and exportable. |
| 🌐 **Free catalogs** | Stream or download full-length tracks from Internet Archive and Jamendo. Off by default. |
| 📱 **One codebase** | Windows, macOS, Linux, and Android — desktop and touch layouts from the same UI. |
| 🪶 **Portable** | A single native binary; no Electron, no heavy runtime. |

---

## Features

### Home & discovery

The home screen is built from your own listening history — a featured track based
on what you last played, a "Mix for you" row of neighbors from songs you finish,
album shelves, and suggested artists similar to the ones you actually listen to.

<img src="docs/screenshots/desktop-home.png" alt="Home screen with featured track, mix and album shelves" width="100%">

### Library, albums & artists

Point Wave at a folder and it scans, extracts tags, embeds cover art, and indexes
everything into SQLite. The library view is virtualized, sortable, and resizable,
with inline favorite toggles and a right-click menu for **Play Next**, **Add to
Queue**, **Add to Playlist**, **Edit Metadata**, **Go to Album**, and **Go to
Artist**.

Supported formats: `aac`, `aiff`, `alac`, `caf`, `flac`, `m4a`, `m4b`, `m4p`,
`mka`, `mkv`, `mp1`, `mp2`, `mp3`, `mp4`, `oga`, `ogg`, `opus`, `wav`, `wave`,
`weba`.

<img src="docs/screenshots/desktop-library.png" alt="Library track list" width="100%">

Album and artist pages are generated from your tags — full-resolution cover,
tracklist, and a per-artist discography you can jump into from any track.

<img src="docs/screenshots/desktop-album.png" alt="Album page" width="100%">

### Metadata editor

Right-click any track, or use the **…** button on its row, and pick **Edit
Metadata** to fix title, artist, album, album artist, genre, year, track and
disc number, or to replace the cover art from an image file. Changes are
written into the audio file itself, not just into Wave's database, so they
survive a re-scan and other players see them too.

The track menu sits on every list Wave shows: the library, album and artist
pages, search results, and the played-tracks views. On narrow windows and on
touch the **…** button stays visible, since there is no right mouse button to
fall back on.

Albums get the same editor for every track at once, from **Edit metadata** on
the album page. Fields the tracks disagree on start blank and are left alone
unless you fill them in, so setting a year across an album never flattens
eleven different titles into one. Cover art picked there is re-encoded to a
bounded JPEG before it goes into each file.

An empty field clears the tag. Title, artist and album are the exceptions:
Wave falls back to the filename for those, so they always keep a value.

The editor works on Android too. Tracks there come from the folder picker as
documents rather than plain files, so Wave copies one into its cache, writes
the tags, and copies it back. If you granted the folder read-only access when
you added it, the editor says so and offers to add it again, which is enough to
upgrade the permission.

30-second previews aren't files Wave can rewrite, so the option doesn't appear
for them. WAV and AIFF are written correctly but Wave's own decoder stops
reading a RIFF file at the audio data, so a WAV re-imported from scratch comes
back with its old tags.

### Three-tier search

Search widens only as far as it has to, and never touches the network on its own:

1. **Current view** — instant filter over whatever list you are looking at.
2. **Your library** — realtime SQLite search across title, artist, album,
   filename, **and lyrics**, with badges showing which field matched and a
   snippet of the matching lyric line.
3. **The internet** — an explicit button, never automatic. Off entirely unless
   you enable **Search outside sources** in Settings.

<img src="docs/screenshots/desktop-search.png" alt="Library search with matched-field badges and lyric snippets" width="100%">

Tier 3 queries Internet Archive, Jamendo, and Deezer concurrently and groups the
results by provider with their licence line. Full-length audio can be streamed or
saved to your library; Deezer only exposes 30-second previews, so those play but
are never written to disk. A provider that fails degrades to an "unavailable"
section instead of breaking the search.

<img src="docs/screenshots/desktop-sources.png" alt="Remote source results grouped by provider" width="100%">

### Queue & playback

An in-memory queue independent of your playlists: reorder by drag, remove
individual entries, queue a track next, shuffle, and repeat off / one / all.
Previous rewinds the current track if you are more than three seconds in.

<img src="docs/screenshots/desktop-queue.png" alt="Queue panel" width="100%">

### Lyrics

Lyrics are fetched automatically from [LRCLIB](https://lrclib.net) when a song
starts (toggleable), stored with the track, and highlighted line by line in sync
with playback. You can also import your own `.lrc` files, export everything as a
backup, and restore a lyrics backup into matching library tracks later.

<img src="docs/screenshots/desktop-lyrics.png" alt="Synced lyrics panel" width="100%">

### Audio engine & DSP

- **10-band graphic equalizer** (31 Hz – 16 kHz) built from real biquad peaking
  filters, with presets and ±12 dB per band, applied live without a gap.
- **Crossfade** up to 8 seconds between tracks.
- **Gapless playback** for continuous albums.
- **Volume normalization** that pulls quiet and loud tracks toward the median
  loudness of your queue without clipping.
- **Audio output device switching** at runtime.

<img src="docs/screenshots/desktop-equalizer.png" alt="Equalizer, crossfade and gapless controls" width="100%">

### Playlists, favorites & folder sync

Create, rename, delete, import, and export playlists (`m3u` and `json`).
**Favorites** and **All Local Files** are permanent seeded playlists. Any playlist
can be *linked to a folder* — Wave then re-scans that folder on demand and keeps
the playlist in step with what is actually on disk.

<img src="docs/screenshots/desktop-settings.png" alt="Settings — playlists, lyrics and music sources" width="100%">

### Listening stats

Total listening time, play counts, and ranked top songs, artists, albums, and
genres — computed locally from your own playback history.

<img src="docs/screenshots/desktop-stats.png" alt="Listening statistics" width="100%">

### Desktop & OS integration

- **System tray / menu bar** icon with transport controls and a playlist submenu.
- **OS media controls** — the platform's own now-playing widget, media keys, and
  lock-screen artwork stay in sync.
- **Close-to-tray** — choose whether the close button quits Wave or hides it.
- **Single instance** — a second launch hands off to the running app instead of
  starting a competing audio engine.

### Command line

The same binary runs headless. `wave --cli` opens with the Wave mark, what is
in the library, what is playing, and the commands worth knowing first; `wave
--help` has the full reference. Subcommands cover `tracks`, `playlists`,
`playback`, `queue`, `devices`, `favorite`, `metadata`, `dsp`, `stats` and
`now`, backed by a background playback daemon with its own tray icon and a
localhost control socket.

```bash
wave tracks import ~/Music          # scan files or folders into the library
wave tracks list                    # fits your terminal, short IDs
wave tracks info 3f2a91c4           # any unambiguous ID prefix works
wave playlists export <id> m3u out.m3u
wave play Favorites                 # a playlist by name or id, or a track
wave now                            # live now-playing dashboard
wave metadata set <id> --genre "Post-Punk" --year 1979
wave stats artists --limit 10       # top artists by listen time
wave tracks list --json             # machine-readable output
wave completions zsh > _wave        # shell completions
```

Output adapts to the terminal: colour is dropped when stdout is not a TTY or
when `NO_COLOR` is set (`--color` overrides both), the block characters fall
back to ASCII when the console cannot render them or when `WAVE_ASCII` is set,
and tables stack rather than wrap on narrow terminals. `--json` is the stable
surface for scripts. Exit codes are 0 for success, 1 for a general failure, 2
for a usage error, 3 when something is not found, and 4 when the playback
daemon is not running.

---

## Android

Android is a first-class target, not a resized desktop build. The Rust core still
owns the library and the queue, but decoding and output go through **Media3
ExoPlayer** over JNI so `content://` URIs from the Storage Access Framework play
directly — no copying your music into app storage.

- Touch layout with a drawer, a bottom mini-player, and a full **Now Playing**
  sheet you can drag to dismiss.
- **SAF folder picker** and scanning — grant a folder once, keep your files where
  they are.
- **Media session** bridge: notification controls, lock screen, Bluetooth, and
  Android Auto-style transport events.
- Android back button is trapped so it closes panels and sheets before leaving.
- Built for `arm64-v8a` and `armeabi-v7a`, `minSdkVersion 24`.

<div align="center">

<img src="docs/screenshots/android-home.png" alt="Android — Home" width="30%">
<img src="docs/screenshots/android-library.png" alt="Android — Library" width="30%">
<img src="docs/screenshots/android-now-playing.png" alt="Android — Now Playing" width="30%">

<img src="docs/screenshots/android-lyrics.png" alt="Android — Synced lyrics" width="30%">
<img src="docs/screenshots/android-queue.png" alt="Android — Up next" width="30%">
<img src="docs/screenshots/android-nav.png" alt="Android — Navigation drawer" width="30%">

<img src="docs/screenshots/android-search.png" alt="Android — Library search" width="30%">
<img src="docs/screenshots/android-sources.png" alt="Android — Remote sources" width="30%">
<img src="docs/screenshots/android-eq.png" alt="Android — Equalizer and playback settings" width="30%">

</div>

Details: [`docs/backend/android.md`](docs/backend/android.md).

---

## Tech stack

### Frontend
- **React** + **TypeScript** + **Vite**

### App shell
- **Tauri 2** (lightweight native shell)

### Backend / audio engine
- **Rust**
- **Rodio** + **Symphonia** + **CPAL** — desktop playback
- **Lofty** — writing edited tags back to files
- **Media3 ExoPlayer** (JNI) — Android playback (`content://` / SAF-friendly)

### Storage
- **SQLite** — music library, playlists, listening history, settings

### External services (all optional)
- **LRCLIB** — lyrics
- **MusicBrainz** + **Cover Art Archive** — metadata and cover art enrichment
- **Internet Archive**, **Jamendo**, **Deezer** — remote search tier

---

## Project structure

```text
Wave/
├── src/                         # React + TypeScript frontend
│   ├── components/             # UI building blocks (incl. dialogs/)
│   ├── hooks/                  # Frontend behavior hooks
│   ├── utils/                  # Shared helpers
│   │   └── player.ts           # Typed wrapper around Tauri backend commands
│   └── App.tsx                 # Application shell and view routing
├── src-tauri/                  # Rust/Tauri backend
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── android-src/            # Java sources copied into gen/android by CI
│   └── src/
│       ├── app/                # App paths, settings, single-instance runtime logic
│       ├── android/            # Android JNI: ExoPlayer, SAF, media bridge
│       ├── audio/              # Playback engine, DSP, loudness normalization
│       ├── integrations/       # Tray and OS media-control integration
│       ├── os_media/           # Windows-specific media integration
│       ├── sources/            # Remote song sourcing: providers, cache, downloads
│       ├── cli/                # Headless CLI: parsing, rendering, one module per command group
│       ├── commands.rs         # Tauri invoke command handlers
│       ├── cover_art.rs        # Cover art extraction and caching
│       ├── dto.rs              # Shared DTOs between backend and frontend
│       ├── enrichment.rs       # MusicBrainz / Cover Art Archive / LRCLIB lookups
│       ├── error.rs            # Backend error definitions
│       ├── library.rs          # SQLite-backed library and playlist logic
│       ├── listen.rs           # Play history and listening statistics
│       ├── metadata.rs         # Track metadata extraction
│       ├── path_validation.rs  # Safe path validation helpers
│       ├── playback_daemon.rs  # Background playback daemon and IPC
│       ├── tag_edit.rs         # Writing edited tags back to audio files
│       ├── lib.rs              # Tauri backend composition root
│       └── main.rs             # Native process entry point
├── docs/
│   ├── backend/                # Backend API and architecture documentation
│   └── screenshots/            # Images used by this README
└── README.md
```

### Backend layout notes

- The backend lives in `src-tauri/`; `tauri.conf.json` is inside that directory, not at the repository root.
- `src-tauri/src/lib.rs` is the GUI/backend composition root where state and Tauri commands are registered.
- `src-tauri/src/main.rs` selects between GUI mode, CLI mode, and the playback daemon at startup.
- `src/utils/player.ts` is the frontend-facing wrapper around the backend command surface.
- Detailed backend API docs live in [`docs/backend/README.md`](docs/backend/README.md).
- Android ExoPlayer + SAF details: [`docs/backend/android.md`](docs/backend/android.md).
- Remote sourcing (providers, streaming cache, downloads): [`docs/backend/sources.md`](docs/backend/sources.md).
- Equalizer and filter math: [`docs/backend/dsp.md`](docs/backend/dsp.md).

---

## Getting started

### Prerequisites

- **Node.js** (LTS)
- **Rust** and **Cargo**
- **Git**

Verify:

```bash
node -v && rustc --version && cargo --version
```

### Install dependencies

```bash
npm install
```

### Run in development

```bash
npm run tauri dev
```

This starts the frontend and the native backend together. Opening the plain Vite
URL in a browser will not work — the UI needs the Tauri backend behind it.

### Build for production

```bash
npm run tauri build
```

This produces the installers for your platform — `.deb`, `.rpm` and AppImage on
Linux, an NSIS installer on Windows, `.app` and `.dmg` on macOS — under
`src-tauri/target/release/bundle/`.

Packaging inputs (desktop entry, installer hooks, distro manifests) live in
[`packaging/`](packaging); see [docs/INSTALL.md](docs/INSTALL.md).

### Android

```bash
npm run android:init     # once, to generate the Gradle project
npm run android:dev      # run on a connected device or emulator
npm run android:build    # produce an APK / AAB
```

Requires the Android SDK + NDK and the `aarch64-linux-android` /
`armv7-linux-androideabi` Rust targets.

### Optional: remote sources

Wave works fully offline. Searching the internet for music is opt-in — toggle
**Search outside sources** in Settings.

Deezer and Internet Archive need no setup. Jamendo needs a free client ID from
[developer.jamendo.com](https://developer.jamendo.com), entered in Settings.

Deezer's API only serves 30-second previews; previews play but are never saved to
your library. Full-length audio comes from Internet Archive and Jamendo.

---

## Design goals

- **Fast startup**
- **Low memory usage**
- **Clean architecture**
- **No unnecessary dependencies**
- **Long-term maintainability**

---

## License

Licensed under the **GNU Affero General Public License v3.0 (AGPL-3.0)**, with additional terms permitted under AGPLv3 Section 7. See [LICENSE](LICENSE).

This means: you're free to use, study, modify, and redistribute this code, including running it as a network service — but any modified version (or service built on one) must also be released as source under AGPL-3.0, must keep author attribution intact, and must be clearly marked as a different, unaffiliated project (see the Trademark Notice below and the additional terms in [LICENSE](LICENSE)).

### Trademark Notice

"Wave," the Wave name, and its logo/branding are **not** licensed under AGPL-3.0 and are not covered by the code license above. They are trademarks/branding of the original author. The AGPL-3.0 license grants rights to the *source code* — it does not grant permission to use the "Wave" name, logo, or branding for a fork, modified version, or derivative service.

If you fork or modify this project, please:
- Use a different name and logo that isn't confusingly similar to "Wave"
- Clearly state that your version is unaffiliated with and not endorsed by the original project

## Author

Built with ❤️ by **Behdad**
