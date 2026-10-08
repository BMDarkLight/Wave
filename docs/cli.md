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
- [Batch requests](#batch-requests)
- [Lists from stdin](#lists-from-stdin)
- [Command reference](#command-reference)

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

If whatever reads the output closes it early, as `head` does in
`wave tracks list | head`, `wave` stops writing and exits with status 141 and
nothing on stderr, the same status a Unix tool stopped by SIGPIPE reports. This
holds for `wave batch` and `wave now --watch` too.

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
  "artist": "Talking Heads",
  "speed": 1.0,
  "sleep_timer": {"mode": "countdown", "remaining_seconds": 1740.0}
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
| `speed` | number | 0.5 to 2.0; 1.0 is normal. Resets to 1.0 when the daemon starts. |
| `sleep_timer` | object | `mode` is `off`, `countdown` or `end_of_track`. A countdown also has `remaining_seconds`. |

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
  position or length, the speed, the sleep timer being set or cleared, or the position
  jumping because of a seek. Steady playback is not an event, and neither is a
  sleep countdown ticking down; to show a moving position, add the time since `at`
  to `position_seconds` while `state` is `playing`.
- **`queue`** carries `queue_position` and `tracks`, the same as
  `queue list`. One is sent first, then again whenever the queue's contents
  change.
- **`daemon_stopped`** is the last line. The command then exits with status 0.

Unknown event names may be added later; skip them. `--interval` (0.1 to 10
seconds, default 0.5) sets how often the daemon is checked, which bounds how
late an event can arrive. If the daemon is not running when the watch starts,
the command fails with exit status 4 (`no_daemon`), like other playback
commands. Closing the pipe ends the watch quietly, with status 141.

## Batch requests

`wave batch` runs many requests in one call. It reads them from stdin, runs
them one after another in the order given, and writes one JSON line per
request as each finishes.

Each request can be written three ways, and they can be mixed:

```text
# A command line, quoted the way a shell would quote it
playlists create "Road Trip"

# A JSON array of the arguments
["playlists", "add-track", "Road Trip", "/music/a.flac"]

# A JSON object with the arguments and an id of your choosing
{"id": "fav-1", "args": ["favorite", "add", "/music/a.flac"]}
```

Blank lines and lines starting with `#` are skipped. The whole input can
instead be a single JSON array of requests, which is handy when the requests
are built by a program:

```json
[["playlists", "create", "Mix"], {"id": 2, "args": ["playlists", "list"]}]
```

The arguments are the same as on the command line, without the program name
and without `--json`, which every request gets anyway. Shell-style lines
support single quotes, double quotes (with `\"` and `\\` inside) and
backslash escapes; there are no variables, globs or pipes.

Each result line looks like this:

```json
{"index":0,"ok":true,"code":0,"result":{"message":"Created playlist \"Road Trip\"","ok":true,"playlist":{...}},"schema_version":1}
{"index":2,"id":"fav-1","ok":false,"code":5,"kind":"conflict","error":"A playlist named \"Road Trip\" already exists","schema_version":1}
```

| Field | Meaning |
|---|---|
| `index` | Position of the request in the input, counting from 0. Skipped lines are not counted. |
| `id` | The request's `id`, when it had one. |
| `ok` | Whether the request succeeded. |
| `code` | Its exit status, as in [Errors](#errors). |
| `result` | On success, the JSON the command printed. |
| `kind`, `error` | On failure, as in [Errors](#errors). |
| `schema_version` | See [Schema version](#schema-version). |

Every request runs exactly as if it had been typed on its own, with its own
exit status, and a failed request does not stop the ones after it. Pass
`--stop-on-error` to stop at the first failure instead. A line that cannot be
read (bad JSON, an unclosed quote) fails with `usage` in its place, so results
always line up with the input.

`wave batch` exits with status 0 when every request succeeded and 1 when any
failed. Unlike other commands it writes its results to stdout either way;
stderr stays empty. A batch request cannot itself be `batch`, `now --watch`,
or use `-`, since it has no stdin of its own.

## Lists from stdin

Commands that take one track accept `-` in its place, and then read any
number of tracks from stdin:

```bash
find ~/Music/Jazz -name '*.flac' | wave --json favorite add -
wave --json tracks query Bowie | jq -r '.[].id' | wave --json queue add -
```

The list can be:

- one item per line (blank lines are skipped),
- NUL-separated, for paths that contain newlines (`find -print0`), or
- a JSON array of strings: `["/music/a.flac", "/music/b.flac"]`.

The form is detected from the input. Each item can be anything the command
accepts as a track: a path, an id, or an id prefix.

The command runs once per item, and the results come back exactly as for
[`wave batch`](#batch-requests), one line each, with the item it was for under
`input`. The exit status is 0 when every item succeeded and 1 when any failed.

`-` works with `tracks info`, `tracks add`, `tracks remove`, `queue add`,
`queue next`, `playlists add-track`, `playlists remove-track`, `favorite add`,
`favorite remove`, `metadata get` and `metadata set` (the same tags are written
to every track). `queue next -` queues the tracks so that they play in the
order given.

`tracks import -` is different: it reads paths the same way but imports them
together in one run, so its output is the usual single import summary, and
`-` can sit beside other paths (`wave tracks import ~/Music -`).

## Command reference

Reads print the data directly; changes print the envelope described in
[The JSON contract](#the-json-contract), with the fields listed here beside
`ok` and `message`.

### Tracks and the library

| Command | Output |
|---|---|
| `tracks list [PLAYLIST]` | Array of [tracks](#track), all of them or one playlist's. |
| `tracks query TEXT` | Array of tracks matching title, artist or album. |
| `tracks info TRACK` | One track. |
| `tracks import PATH...` | `imported` (count of new tracks) and `skipped` (one string per file not imported, with the reason, such as a duplicate of a track already in the library). |
| `tracks add TRACK [--playlist-id P]` | `track`. |
| `tracks remove TRACK [--playlist-id P]` | `path`. |
| `tracks reset --yes` | `tracks_removed`, `playlists_deleted`. |
| `metadata get TRACK` | One track, read from the library or straight from the file. |
| `metadata set TRACK --title ...` | `track`, the updated track; or `path` alone when the file is not in the library. |
| `metadata cover-export TRACK FILE` | `output`, `mime`, `bytes`. |
| `metadata cover-set TRACK IMAGE` | `id`, the track's id. |

A `TRACK` is a full track id, an id prefix of at least four characters, or a
path to an audio file.

### Playlists and favorites

| Command | Output |
|---|---|
| `playlists list` | Array of [playlists](#playlist). |
| `playlists query TEXT` | Array of playlists whose name matches. |
| `playlists info PLAYLIST` | `playlist` and `tracks`, the playlist's tracks in order. |
| `playlists create NAME` | `playlist`. |
| `playlists rename PLAYLIST NAME` | `id`, `name`. |
| `playlists delete PLAYLIST`, `playlists clear PLAYLIST` | `id`. |
| `playlists add-track PLAYLIST TRACK` | `id` (the playlist's) and `track`. |
| `playlists remove-track PLAYLIST TRACK` | `id`. |
| `playlists import FILE` | `id`, `name`, and `tracks` (how many were added). |
| `playlists export PLAYLIST FILE` | `id`, `output`, `format` (`m3u` or `json`). |
| `favorite list` | Array of tracks. |
| `favorite add TRACK` | `track`. |
| `favorite remove TRACK` | `path`. |

A `PLAYLIST` is an exact name (any case), a unique start of a name, a full id,
or an id prefix.

### Playback

| Command | Output |
|---|---|
| `play TRACK_OR_PLAYLIST`, `playback start ...` | `status`, see [Playback status](#playback-status). Starts the daemon when needed. |
| `playback status` | The [playback status](#playback-status) itself. |
| `playback pause`, `resume`, `stop`, `next`, `previous`, `seek POS` | `status`. `seek` takes seconds, `m:ss`, or a step like `+10` or `-30`. |
| `playback sleep WHEN` | `status`. `WHEN` is minutes (`30`), a length with units (`45m`, `1h30m`, `90s`), `end` to pause when the current track ends, or `off`. |
| `playback sleep` (no value) | `sleep_timer`, as in the [playback status](#playback-status). |
| `playback speed VALUE` | `status`. `VALUE` is a rate (`1.25`), a percentage (`125%`), or a step like `+0.25`. |
| `playback speed` (no value) | `speed`. |
| `playback shutdown` | Nothing beyond the envelope; succeeds when no daemon is running. |
| `queue list` | `queue_position` and `tracks`. |
| `queue add TRACK`, `queue next TRACK`, `queue remove POSITION`, `queue clear`, `queue shuffle [on\|off]`, `queue repeat MODE` | `status`. |
| `queue repeat` (no mode) | `repeat`. |
| `devices list` | `default` and `devices`, the output names. |
| `devices switch NAME`, `devices volume LEVEL` | `status`. `LEVEL` is `50%`, `0.5`, or a step like `+10`. |
| `devices volume` (no level) | `volume`, 0.0 to 1.0. |
| `now --json` | The playback status, once. |
| `now --json --watch` | A stream of events, see [Watching playback](#watching-playback). |

### Equalizer and playback settings

| Command | Output |
|---|---|
| `dsp eq-show` | `eq_enabled`, `bands` (ten gains in dB, 31 Hz to 16 kHz), `bass`, `treble`, `crossfade_duration`, `gapless_enabled`. |
| `dsp presets` | Array of `name` and `description`. |
| `dsp eq-set ...`, `eq-band`, `eq-enable`, `eq-disable`, `eq-reset`, `preset`, `bass DB`, `treble DB`, `crossfade SECONDS`, `gapless on\|off` | `dsp`, the settings as `eq-show` prints them. |
| `dsp gapless`, `dsp crossfade`, `dsp bass`, `dsp treble` (no value) | `gapless`, `crossfade_seconds`, `bass_db` or `treble_db`. |
| `dsp export FILE` | `output`. |
| `dsp import FILE` | `dsp`, the settings now in effect. |

Gains are -12 to +12 dB and crossfade is 0 to 8 seconds; anything outside
fails with `usage`. These commands work with or without the daemon: with it
they change the sound at once, without it they change the saved settings.

### Listening stats

| Command | Output |
|---|---|
| `stats summary` | `total_listen_seconds`, `total_plays`, `tracks_played`, and `top_tracks`, `top_artists`, `top_albums`, `top_genres`. |
| `stats recent`, `stats most` | Array of tracks. |
| `stats artists`, `stats albums`, `stats genres` | Array of `name`, `listen_seconds`, `play_count`. |

Each takes `--limit N`.

### Track

| Field | Type | Meaning |
|---|---|---|
| `id` | string | Stable id; use it to refer to the track later. |
| `path` | string | Where the file is, spelled the way it was imported. |
| `name` | string | File name. |
| `title`, `artist`, `album` | string | From the tags, or guessed from the file and folder name when the file has none. |
| `album_artist`, `genre` | string or null | |
| `year`, `track_number`, `disc_number` | integer or null | |
| `duration_seconds` | number or null | |
| `format` | string | Upper-case extension, such as `FLAC`. |
| `sample_rate`, `channels`, `bit_depth` | integer or null | |
| `file_size` | integer | Bytes. |
| `modified_at`, `indexed_at` | integer | Seconds since the Unix epoch. |
| `lyrics`, `lyrics_source` | string or null | |
| `album_art_id` | string or null | Shared by every track with the same cover. |
| `cover_art_data_url`, `cover_art_mime`, `cover_art_source` | string or null | Where the library keeps the cover and what kind it is. Use `metadata cover-export` to get the image. |
| `fingerprint_sha256`, `acoustid_fingerprint`, `musicbrainz_recording_id` | string or null | |
| `is_saf_uri` | boolean | Android only: the path is a document URI. |
| `source_provider`, `source_state` | string or null | Set for tracks streamed or downloaded from an online source. |

### Playlist

| Field | Type | Meaning |
|---|---|---|
| `id` | string | Stable id. |
| `name` | string | |
| `track_count` | integer | |
| `profile_id` | string | Always `default` for now. |
| `created_at`, `updated_at` | integer | Seconds since the Unix epoch. |
| `sync_folder` | string or null | The folder the playlist mirrors, when it is linked to one. |
