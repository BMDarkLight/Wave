# Backend events

The Rust backend emits Tauri **events** when the user interacts with OS media controls (keyboard media keys, Windows taskbar transport, macOS Control Center, Linux MPRIS, etc.).

Events flow **backend → frontend**. Handle them to keep in-app UI in sync with the system.

## Setup

Use the helper in `src/utils/player.ts` or listen manually:

```typescript
import { listen } from "@tauri-apps/api/event";
import { listenToMediaControls } from "../utils/player";

// Recommended wrapper
const unlisten = await listenToMediaControls({
  onPlay: () => resumeTrack(),
  onPause: () => pauseTrack(),
  onNext: () => handleNext(),
  onPrevious: () => handlePrevious(),
  onSetPosition: (seconds) => seekTrack(seconds),
});

// On component unmount
unlisten();
```

If OS media controls failed to initialize (logged at startup as a warning), **no events are emitted** but playback commands still work.

---

## Event reference

### `media-control-play`

User pressed Play in the OS UI.

| Payload | Type |
|---------|------|
| (none) | `()` |

---

### `media-control-pause`

User pressed Pause in the OS UI.

| Payload | Type |
|---------|------|
| (none) | `()` |

---

### `media-control-toggle`

User pressed a combined Play/Pause toggle.

| Payload | Type |
|---------|------|
| (none) | `()` |

---

### `media-control-next`

User pressed Next track.

| Payload | Type |
|---------|------|
| (none) | `()` |

**Suggested handler:** call `play_next` (backend queue) or advance the library playlist index in the UI.

---

### `media-control-previous`

User pressed Previous track.

| Payload | Type |
|---------|------|
| (none) | `()` |

---

### `media-control-stop`

User pressed Stop.

| Payload | Type |
|---------|------|
| (none) | `()` |

---

### `media-control-seek-relative`

Relative seek (platform-dependent).

| Payload | Type | Values |
|---------|------|--------|
| direction | `string` | `"forward"` or `"backward"` |

Example handler:

```typescript
onSeekRelative: (direction) => {
  const delta = direction === "forward" ? 10 : -10;
  seekTrack(currentPosition + delta);
},
```

---

### `media-control-seek-by`

Seek by a fixed number of seconds (signed).

| Payload | Type | Description |
|---------|------|-------------|
| seconds | `number` | Positive = forward, negative = backward |

Example: payload `-15` means seek back 15 seconds.

---

### `media-control-set-position`

Absolute seek to a timeline position.

| Payload | Type | Description |
|---------|------|-------------|
| seconds | `number` | Absolute position in seconds |

Example handler:

```typescript
onSetPosition: (seconds) => seekTrack(seconds),
```

---

## Outbound metadata (frontend → OS)

Events above are **inbound**. To update what the OS displays, call the **`update_media_metadata`** command whenever the now-playing track changes. See [Commands → update_media_metadata](./commands.md#update_media_metadata).

Playback commands (`play_track`, `pause_track`, etc.) automatically update transport state (playing/paused/stopped) on the OS side when media controls are available.

---

## Ignored OS actions

The backend currently **does not** forward these platform events to the frontend:

- Open URI
- Raise (bring app to foreground)
- Quit
- Set volume

Handle volume in-app with `set_volume` if you add a volume slider.

---

### `source-download-fallback`

Emitted when a download could not be written to its intended destination and
landed in Wave's own downloads folder instead. On Android this means the music
folder's grant is read-only.

Surface this to the user: the file saved successfully, but somewhere they did
not choose.

| Payload | Type |
|---------|------|
| reason | `string` — human-readable explanation of where the file went |

```typescript
import { listen } from "@tauri-apps/api/event";

await listen<string>("source-download-fallback", (event) => {
  showToast(event.payload);
});
```
