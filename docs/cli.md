# Wave CLI for scripts and other programs

`wave` is built to be driven by other software as well as by people. This page
is the contract for that use: what goes to which stream, the shape of every
JSON document, what each exit code means, and how input can be fed in. Anything
described here is covered by the integration tests in
[`src-tauri/tests/cli.rs`](../src-tauri/tests/cli.rs).

For the human-facing tour of the commands, see the
[Command line](../README.md#command-line) section of the README and
`wave --help`.

## Contents

- [The JSON contract](#the-json-contract)
- [Errors](#errors)
- [Schema version](#schema-version)
- [Playback status](#playback-status)
- [Watching playback](#watching-playback)

## The JSON contract

Pass `--json` anywhere on the command line to get machine-readable output.

- **On success** the exit status is 0, stdout holds exactly one JSON document,
  and stderr is empty.
- **On failure** the exit status is not 0, stdout is empty, and stderr holds
  exactly one JSON error document (see [Errors](#errors)).
- Nothing else is ever printed: no colour codes, progress bars, prompts, or
  hints.
- `--json` never waits for input it was not given. A command that would ask a
  person to confirm, such as `tracks reset`, fails with a `usage` error instead
  and names the flag that confirms it (`--yes`).

Commands come in two kinds, and their success output differs.

**Reads** (`tracks list`, `playlists info`, `stats …`, `queue list`, `playback
status`, …) print the data itself: a JSON array for a list, an object for a
single item.

```bash
$ wave --json playlists list
[
  {
    "id": "5fa7d96a-8b10-4df5-bf83-72cd751757a8",
    "profile_id": "default",
    "name": "Library",
    "track_count": 4,
    "created_at": 1791116050,
    "updated_at": 1791116050,
    "sync_folder": null
  }
]
```

**Changes** (`playlists create`, `queue add`, `metadata set`, …) print an
envelope with `"ok": true`, a human-readable `message`, and any fields the
command reports about what it did.

```bash
$ wave --json playlists create "Road Trip"
{"message":"Created playlist \"Road Trip\"","ok":true,"playlist":{"created_at":1791203083,"id":"46c9b376-363a-444a-92d3-89d1936663a5","name":"Road Trip","profile_id":"default","sync_folder":null,"track_count":0,"updated_at":1791203083}}
```

Field order is not significant, and `message` is for display only: its wording
may change in any release, so never parse it.

## Errors

A failed command writes one error document to stderr:

```json
{"ok": false, "error": "Playlist not found: Nope", "code": 3, "kind": "not_found"}
```

- `code` is the process exit status.
- `kind` is a stable name for the same thing; branch on either.
- `error` is a human-readable message. Like `message` above, its wording is not
  part of the contract.

| Exit status | `kind` | Meaning |
|---|---|---|
| 0 | | Success. |
| 1 | `general` | The command failed for a reason not listed below, such as an unreadable file or a full disk. |
| 2 | `usage` | The command line was wrong: an unknown command or flag, a missing argument, or a value out of range (`dsp bass 30`, `playback seek abc`, `queue remove 99`). Retrying the same command will fail the same way. |
| 3 | `not_found` | The track, playlist, file, or cover the command names does not exist, or a name matches more than one playlist or track. |
| 4 | `no_daemon` | The command needs the playback daemon and it is not running. Start playback first (`wave play …`). |
| 5 | `conflict` | The request clashes with what is already there: a playlist name that is taken, a track already in the playlist, a protected playlist (Library, Favorites), or the desktop app being open while the command needs the playback daemon. |

Errors in the command line itself are reported the same way under `--json`,
with exit status 2:

```json
{"ok": false, "error": "invalid value 'sideways' for '[MODE]' [possible values: off, one, all]", "code": 2, "kind": "usage"}
```

`--help` and `--version` are not errors: they print their text and exit 0,
with or without `--json`.

## Schema version

The shapes on this page are versioned as a whole. `wave --json` with no
command reports the version along with a summary of the library:

```bash
$ wave --json
{
  "schema_version": 1,
  "version": "0.5.0",
  "tracks": 1284,
  "playlists": 9,
  "listened": "4h 6m 1s",
  "playback": null
}
```

`schema_version` goes up whenever a field is removed or renamed, or its meaning
changes. Adding a field does not raise it, so read the fields you need and
ignore the rest. A program can check the version once at startup and refuse to
run against one it does not know.

## Playback status

Playback runs in a background daemon that the first playback command starts.
`wave --json playback status` describes what it is doing:

```json
{
  "state": "playing",
  "file": "Once in a Lifetime.flac",
  "path": "/Users/me/Music/Talking Heads/Remain in Light/Once in a Lifetime.flac",
  "position_seconds": 72.4,
  "duration_seconds": 260.0,
  "volume": 0.62,
  "device": "MacBook Pro Speakers",
  "repeat": "off",
  "shuffle": false,
  "queue_position": 0,
  "queue_length": 12,
  "title": "Once in a Lifetime",
  "artist": "Talking Heads"
}
```

| Field | Type | Meaning |
|---|---|---|
| `state` | string | `playing`, `paused` or `stopped`. |
| `file` | string or null | File name of the loaded track; `null` when nothing is loaded. |
| `path` | string or null | Full path of the loaded track; `null` when nothing is loaded. |
| `position_seconds` | number | Position in the track. |
| `duration_seconds` | number or null | Length of the track; `null` until it is known. |
| `volume` | number | 0.0 to 1.0. |
| `device` | string | Name of the audio output. |
| `repeat` | string | `off`, `one` or `all`, the same words `queue repeat` takes. |
| `shuffle` | boolean | |
| `queue_position` | integer or null | Index of the current track in `queue list`, counting from 0; `null` when no queue entry is current. |
| `queue_length` | integer | Number of tracks in the queue. |
| `title`, `artist` | string or null | From the library; `null` for a file the library does not know. |

Every command that changes playback (`play`, `playback pause`, `queue add`,
`devices volume`, …) returns the same object under `status`, showing where
playback ended up after the change, so a second request is never needed:

```json
{"message": "Paused.", "ok": true, "status": {"state": "paused", "...": "..."}}
```

`wave --json queue list` gives the queue in stored order, with the same 0-based
position. With shuffle on, tracks play in a different order than listed.

```json
{"queue_position": 0, "tracks": ["/music/a.flac", "/music/b.flac"]}
```

Pass a position from that list straight to `queue remove`. Removing the track
that is playing moves playback on to the next one, keeping it paused if it was
paused, or stops when it was the last.

When no daemon is running, playback commands fail with exit status 4
(`no_daemon`), except `play` and `playback start`, which start one. Several of
those can run at once: they share one daemon. While the Wave desktop app is
open it owns playback, and commands that need the daemon fail with exit status
5 (`conflict`).

## Watching playback

`wave now --json --watch` follows playback as a stream of
[JSON Lines](https://jsonlines.org): one compact JSON object per line, written
and flushed the moment something happens. It runs until the playback daemon
stops, so a program can follow playback without polling.

```bash
$ wave now --json --watch
{"at":1791203512843,"event":"status","schema_version":1,"status":{"artist":"Talking Heads","file":"a.flac",...,"state":"playing",...}}
{"at":1791203512843,"event":"queue","queue_position":0,"schema_version":1,"tracks":["/music/a.flac","/music/b.flac"]}
{"at":1791203519102,"event":"status","schema_version":1,"status":{...,"state":"paused",...}}
{"at":1791203530410,"event":"daemon_stopped","schema_version":1}
```

Every line has these fields:

| Field | Meaning |
|---|---|
| `event` | `status`, `queue` or `daemon_stopped`. |
| `at` | When the change was seen, in milliseconds since the Unix epoch. |
| `schema_version` | See [Schema version](#schema-version). |

The events:

- **`status`** carries the full [playback status](#playback-status) under
  `status`. One is sent first, then again whenever anything in it changes: the
  state, the track, the volume, the device, repeat or shuffle, the queue
  position or length, or the position jumping because of a seek. Steady
  playback is not an event; to show a moving position, add the time since `at`
  to `position_seconds` while `state` is `playing`.
- **`queue`** carries `queue_position` and `tracks`, the same as
  `queue list`. One is sent first, then again whenever the queue's contents
  change.
- **`daemon_stopped`** is the last line. The command then exits with status 0.

Unknown event names may be added later; skip them. `--interval` (0.1 to 10
seconds, default 0.5) sets how often the daemon is checked, which bounds how
late an event can arrive. If the daemon is not running when the watch starts,
the command fails with exit status 4 (`no_daemon`), like other playback
commands. Closing the pipe ends the watch quietly.
