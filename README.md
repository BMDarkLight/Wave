# Wave

**Wave** is a lightweight, cross-platform, and portable music player built with modern technologies.  
It focuses on **performance**, **simplicity**, and **offline-first usage**, while remaining easily extensible for advanced audio features.

---

## Features

- Local music library (folder-based)
- Three-tier search: current view, then your library, then the internet
- Stream or download full-length tracks from Internet Archive and Jamendo
- High-performance audio playback
- Cross-platform (Windows, macOS, Linux, Android)
- Portable & lightweight (no heavy runtime)
- Designed for future EQ & DSP extensions
- Offline-first (no server required)

---

## Tech Stack

### Frontend
- **React**
- **TypeScript**
- **Vite**

### App Shell
- **Tauri** (lightweight native shell)

### Backend / Audio Engine
- **Rust**
- **Rodio** + **Symphonia** + **CPAL** – desktop playback
- **Media3 ExoPlayer** (JNI) – Android playback (`content://` / SAF-friendly)

### Storage
- **SQLite** – music library, playlists, settings

---

## Project Structure

```text
Wave/
├── src/                         # React + TypeScript frontend
│   ├── components/             # UI building blocks
│   ├── pages/                  # Route-level screens
│   ├── hooks/                  # Frontend behavior hooks
│   ├── lib/                    # Shared frontend utilities
│   └── utils/player.ts         # Typed wrapper around Tauri backend commands
├── src-tauri/                  # Rust/Tauri backend
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── android-src/            # Java sources copied into gen/android by CI
│   └── src/
│       ├── app/                # App paths, settings, and single-instance runtime logic
│       ├── android/            # Android JNI: ExoPlayer, SAF, media bridge
│       ├── audio/              # Playback engine (Rodio on desktop; ExoPlayer hooks on Android)
│       ├── integrations/       # Tray and OS media-control integration
│       ├── os_media/           # Windows-specific media integration
│       ├── cli.rs              # Headless/CLI entry surface
│       ├── commands.rs         # Tauri invoke command handlers
│       ├── dto.rs              # Shared DTOs between backend and frontend
│       ├── error.rs            # Backend error definitions
│       ├── library.rs          # SQLite-backed library and playlist logic
│       ├── metadata.rs         # Track metadata extraction and enrichment
│       ├── path_validation.rs  # Safe path validation helpers
│       ├── sources/            # Remote song sourcing: providers, cache, downloads
│       ├── playback_daemon.rs  # Background playback daemon and IPC
│       ├── lib.rs              # Tauri backend composition root
│       └── main.rs             # Native process entry point
├── docs/
│   └── backend/                # Backend API and architecture documentation
└── README.md
```

### Backend Layout Notes

- The backend lives in `src-tauri/`; `tauri.conf.json` is inside that directory, not at the repository root.
- `src-tauri/src/lib.rs` is the GUI/backend composition root where state and Tauri commands are registered.
- `src-tauri/src/main.rs` selects between GUI mode, CLI mode, and the playback daemon at startup.
- `src/utils/player.ts` is the frontend-facing wrapper around the backend command surface.
- Detailed backend API docs live in `docs/backend/README.md`.
- Android ExoPlayer + SAF details: [`docs/backend/android.md`](docs/backend/android.md).
- Remote sourcing (providers, streaming cache, downloads): [`docs/backend/sources.md`](docs/backend/sources.md).

---

## Getting Started

### Prerequisites

Make sure you have the following installed:

- **Node.js** (LTS)
- **Rust**
- **Cargo**
- **Git**

Verify installation:

```bash
node -v
rustc --version
cargo --version
```

---

### Optional: remote sources

Wave works fully offline. Searching the internet for music is opt-in — toggle
**Search outside sources** in Settings.

Deezer and Internet Archive need no setup. Jamendo needs a free client ID from
[developer.jamendo.com](https://developer.jamendo.com), entered in Settings.

Note that Deezer's API only serves 30-second previews; previews play but are
never saved to your library. Full-length audio comes from Internet Archive and
Jamendo.

---

### Install Dependencies

```bash
npm install
```

---

### Run in Development Mode

```bash
npm run tauri dev
```

This will start both the frontend and the native backend.

---

## Build for Production

```bash
npm run tauri build
```

The final portable binaries will be generated for your platform.

---

## Design Goals

- **Fast startup**
- **Low memory usage**
- **Clean architecture**
- **No unnecessary dependencies**
- **Long-term maintainability**

---

## License

This project is licensed under the **MIT License**.

---

## Author

Built with ❤️ by **Behdad**
