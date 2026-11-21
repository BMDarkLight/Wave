// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The `wave --json` contract, checked against the real binary.
//!
//! Other programs drive Wave through this output, so it is held to one shape:
//! on success, JSON on stdout and nothing on stderr; on failure, nothing on
//! stdout and `{"ok": false, "error", "code"}` on stderr, with `code` equal to
//! the exit status.
//!
//! Every test runs against its own empty data folder, so none of them can see
//! a real library or a running Wave. Tests that need the playback daemon need
//! an audio output too, which CI runners lack, so they are ignored by default:
//! run them with `cargo test --test cli -- --ignored`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::Value;

const EXIT_GENERAL: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_NOT_FOUND: i32 = 3;
const EXIT_NO_DAEMON: i32 = 4;
const EXIT_CONFLICT: i32 = 5;

fn kind(code: i32) -> &'static str {
    match code {
        EXIT_USAGE => "usage",
        EXIT_NOT_FOUND => "not_found",
        EXIT_NO_DAEMON => "no_daemon",
        EXIT_CONFLICT => "conflict",
        EXIT_GENERAL => "general",
        other => panic!("exit code {other} is not part of the contract"),
    }
}

/// A Wave installation of its own: data folder, music folder, and the binary.
struct Wave {
    root: PathBuf,
}

impl Wave {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "wave-cli-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("music")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        Wave { root }
    }

    fn music(&self, name: &str) -> String {
        self.root
            .join("music")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn path(&self, name: &str) -> String {
        self.root.join(name).to_string_lossy().into_owned()
    }

    /// Where this instance keeps its library and settings, per platform.
    fn data_dir(&self) -> PathBuf {
        let home = self.root.join("home");
        let base = if cfg!(target_os = "macos") {
            home.join("Library/Application Support")
        } else if cfg!(windows) {
            home.join("AppData")
        } else {
            home.join("share")
        };
        base.join("app.bmdarklight.wave")
    }

    fn command(&self, args: &[&str]) -> Command {
        let home = self.root.join("home");
        let mut command = Command::new(env!("CARGO_BIN_EXE_wave"));
        command
            .args(args)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", home.join("share"))
            .env("APPDATA", home.join("AppData"))
            .env_remove("WAVE_DB_PATH")
            .env("NO_COLOR", "1")
            // Nobody is there to answer a prompt.
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("failed to run wave")
    }

    /// Run with --json, `input` on stdin, and read stdout as JSON lines.
    /// Returns the exit status and the lines; stderr must stay empty.
    fn lines(&self, args: &[&str], input: &str) -> (i32, Vec<Value>) {
        use std::io::Write as _;
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let mut child = self
            .command(&full)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.is_empty(), "wave {args:?} wrote to stderr: {stderr}");
        let lines = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
            .collect();
        (out.status.code().unwrap(), lines)
    }

    /// Run with --json and expect success: JSON on stdout, stderr empty.
    fn ok(&self, args: &[&str]) -> Value {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let out = self.run(&full);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "wave {args:?} failed ({:?}): {stderr}",
            out.status.code()
        );
        assert!(stderr.is_empty(), "wave {args:?} wrote to stderr: {stderr}");
        serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("wave {args:?} printed invalid JSON ({e}): {stdout}"))
    }

    /// Run with --json and expect failure with `code`: stdout empty, and the
    /// error envelope on stderr. Returns the error message.
    fn fails(&self, args: &[&str], code: i32) -> String {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let out = self.run(&full);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(code),
            "wave {args:?} exit status; stderr: {stderr}"
        );
        assert!(stdout.is_empty(), "wave {args:?} wrote to stdout: {stdout}");
        let error: Value = serde_json::from_str(&stderr)
            .unwrap_or_else(|e| panic!("wave {args:?} stderr is not JSON ({e}): {stderr}"));
        assert_eq!(error["ok"], Value::Bool(false));
        assert_eq!(error["code"], Value::from(code));
        assert_eq!(error["kind"], kind(code), "wave {args:?}");
        error["error"].as_str().expect("error message").to_string()
    }

    fn write_wav(&self, name: &str) -> String {
        self.write_wav_of(name, 0.5)
    }

    fn write_wav_of(&self, name: &str, seconds: f32) -> String {
        let path = self.music(name);
        write_silent_wav(Path::new(&path), seconds);
        path
    }

    /// Play `tracks` as a playlist, so the whole queue is known when the
    /// first one starts, the way the app plays a playlist.
    fn start_playlist(&self, tracks: &[&str]) {
        self.ok(&["tracks", "import", &self.path("music")]);
        let created = self.ok(&["playlists", "create", "Queue"]);
        for track in tracks {
            self.ok(&["playlists", "add-track", "Queue", track]);
        }
        let id = created["playlist"]["id"].as_str().unwrap();
        self.ok(&["playback", "start", id]);
    }

    /// Follow playback until it stops, noting each file that becomes the
    /// current one and how long the whole run took.
    fn play_order(&self, limit: std::time::Duration) -> (Vec<String>, std::time::Duration) {
        let start = std::time::Instant::now();
        let mut order: Vec<String> = Vec::new();
        while start.elapsed() < limit {
            let status = self.ok(&["playback", "status"]);
            // Between two tracks the state can read stopped for a tick while
            // the next one is still loaded, so only an empty player counts.
            if status["state"] == "stopped" && status["file"].is_null() {
                break;
            }
            if let Some(file) = status["file"].as_str() {
                if order.last().map(String::as_str) != Some(file) {
                    order.push(file.to_string());
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        (order, start.elapsed())
    }

    fn track_names(&self, args: &[&str]) -> Vec<String> {
        let value = self.ok(args);
        let tracks = value
            .get("tracks")
            .unwrap_or(&value)
            .as_array()
            .expect("a track list");
        tracks
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }
}

impl Drop for Wave {
    fn drop(&mut self) {
        // Neither a hosted server nor a daemon started by a test may outlive it.
        let _ = self.run(&["serve", "stop"]);
        let _ = self.run(&["playback", "shutdown"]);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// `seconds` of silent 16-bit mono PCM at 8 kHz.
fn write_silent_wav(path: &Path, seconds: f32) {
    let samples = vec![0u8; (seconds * 8000.0) as usize * 2];
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&8000u32.to_le_bytes());
    wav.extend_from_slice(&16000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(&samples);
    std::fs::write(path, wav).unwrap();
}

#[test]
fn read_commands_print_json_on_an_empty_library() {
    let wave = Wave::new();
    assert_eq!(wave.ok(&["tracks", "list"]), Value::Array(vec![]));
    for args in [
        &["playlists", "list"][..],
        &["favorite", "list"],
        &["dsp", "eq-show"],
        &["dsp", "presets"],
        &["stats", "summary"],
        &["stats", "recent"],
        &["stats", "most"],
        &["stats", "artists"],
        &["stats", "albums"],
        &["stats", "genres"],
    ] {
        wave.ok(args);
    }
}

#[test]
fn an_import_counts_what_the_library_keeps() {
    let wave = Wave::new();
    wave.write_wav("one.wav");
    wave.write_wav("two.wav");
    // Same folder and name, so the same song in another container.
    wave.write_wav("two.wave");
    let music = wave.path("music");

    let first = wave.ok(&["tracks", "import", &music]);
    assert_eq!(first["imported"], 2);
    assert_eq!(first["skipped"].as_array().unwrap().len(), 1);

    let again = wave.ok(&["tracks", "import", &music]);
    assert_eq!(again["imported"], 0, "a re-import adds nothing new");

    let mut names = wave.track_names(&["tracks", "list"]);
    names.sort();
    assert_eq!(names.len(), 2, "{names:?}");

    let missing = wave.path("music/nope.wav");
    let partial = wave.ok(&["tracks", "import", &missing]);
    assert_eq!(partial["imported"], 0);
    assert!(partial["skipped"][0]
        .as_str()
        .unwrap()
        .contains("not found"));
}

#[test]
fn playlists_round_trip_through_export_and_import_in_order() {
    let wave = Wave::new();
    let a = wave.write_wav("a.wav");
    let b = wave.write_wav("b.wav");
    let c = wave.write_wav("c.wav");
    wave.ok(&["tracks", "import", &wave.path("music")]);

    wave.ok(&["playlists", "create", "Road Trip"]);
    for track in [&c, &a, &b] {
        wave.ok(&["playlists", "add-track", "Road Trip", track]);
    }
    let order = vec!["c.wav", "a.wav", "b.wav"];
    assert_eq!(wave.track_names(&["playlists", "info", "Road Trip"]), order);

    let m3u = wave.path("trip.m3u");
    let json = wave.path("trip.json");
    wave.ok(&["playlists", "export", "Road Trip", &m3u]);
    wave.ok(&["playlists", "export", "Road Trip", &json]);

    let from_m3u = wave.ok(&["playlists", "import", &m3u]);
    assert_eq!(from_m3u["tracks"], 3);
    let id = from_m3u["id"].as_str().unwrap();
    assert_eq!(wave.track_names(&["playlists", "info", id]), order);

    let from_json = wave.ok(&["playlists", "import", &json]);
    assert_eq!(from_json["tracks"], 3);

    wave.ok(&["playlists", "rename", "Road Trip", "Long Drive"]);
    wave.ok(&["playlists", "delete", "Long Drive"]);
    wave.fails(&["playlists", "delete", "Long Drive"], EXIT_NOT_FOUND);
}

#[test]
fn a_relative_m3u_entry_resolves_against_the_playlist_file() {
    let wave = Wave::new();
    wave.write_wav("song.wav");
    let list = wave.path("list.m3u");
    std::fs::write(&list, "#EXTM3U\nmusic/song.wav\nmusic/missing.wav\n").unwrap();
    assert_eq!(wave.ok(&["playlists", "import", &list])["tracks"], 1);
}

#[test]
fn tags_written_to_a_wav_survive_a_rescan() {
    let wave = Wave::new();
    let song = wave.write_wav("song.wav");
    wave.ok(&["tracks", "import", &wave.path("music")]);
    let written = wave.ok(&[
        "metadata", "set", &song, "--title", "Heroes", "--year", "1977",
    ]);
    assert_eq!(written["track"]["title"], "Heroes");

    wave.ok(&["tracks", "import", &wave.path("music")]);
    let read = wave.ok(&["metadata", "get", &song]);
    assert_eq!(read["title"], "Heroes");
    assert_eq!(read["year"], 1977);
}

#[test]
fn favorites_add_list_and_remove() {
    let wave = Wave::new();
    let song = wave.write_wav("song.wav");
    wave.ok(&["tracks", "import", &song]);
    wave.ok(&["favorite", "add", &song]);
    assert_eq!(wave.track_names(&["favorite", "list"]), vec!["song.wav"]);
    wave.ok(&["favorite", "remove", &song]);
    wave.fails(&["favorite", "remove", &song], EXIT_NOT_FOUND);
}

#[test]
fn failures_use_the_error_envelope_and_their_exit_codes() {
    let wave = Wave::new();
    wave.fails(&["tracks", "info", "no-such-track"], EXIT_NOT_FOUND);
    wave.fails(&["playlists", "rename", "Nope", "Other"], EXIT_NOT_FOUND);
    wave.fails(&["playback", "status"], EXIT_NO_DAEMON);
    wave.fails(&["queue", "list"], EXIT_NO_DAEMON);
    wave.fails(&["dsp", "bass", "30"], EXIT_USAGE);
    wave.fails(&["dsp", "eq-set", "1", "2", "3"], EXIT_USAGE);
    wave.fails(&["playback", "seek", "abc"], EXIT_USAGE);
    wave.fails(
        &["playlists", "import", &wave.path("missing.m3u")],
        EXIT_NOT_FOUND,
    );
    let text = wave.path("list.txt");
    std::fs::write(&text, "not a playlist").unwrap();
    wave.fails(&["playlists", "import", &text], EXIT_USAGE);

    wave.ok(&["playlists", "create", "Mix"]);
    wave.fails(&["playlists", "create", "Mix"], EXIT_CONFLICT);
    wave.fails(&["playlists", "delete", "Library"], EXIT_CONFLICT);

    let usage = wave.fails(&["queue", "repeat", "sideways"], EXIT_USAGE);
    assert!(usage.contains("possible values"), "{usage}");
    let missing = wave.fails(&["dsp", "eq-band"], EXIT_USAGE);
    assert!(missing.contains("<BAND>"), "{missing}");
}

#[test]
fn a_reset_is_refused_rather_than_waiting_for_an_answer() {
    let wave = Wave::new();
    wave.fails(&["tracks", "reset"], EXIT_USAGE);
    let reset = wave.ok(&["tracks", "reset", "--yes"]);
    assert_eq!(reset["ok"], true);
}

#[test]
fn the_landing_facts_report_the_schema_version() {
    let wave = Wave::new();
    let facts = wave.ok(&[]);
    assert_eq!(facts["schema_version"], 1);
    assert!(facts["version"].is_string());
}

#[test]
fn shutting_down_a_daemon_that_is_not_running_is_not_an_error() {
    let wave = Wave::new();
    wave.ok(&["playback", "shutdown"]);
}

#[test]
fn commands_run_in_parallel_all_succeed() {
    let wave = Wave::new();
    // The first open creates the library, which is where opens used to race.
    let children: Vec<_> = (0..16)
        .map(|i| {
            let name = format!("Playlist {i}");
            let args: Vec<&str> = if i % 2 == 0 {
                vec!["--json", "playlists", "create", &name]
            } else {
                vec!["--json", "tracks", "list"]
            };
            wave.command(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let playlists = wave.ok(&["playlists", "list"]);
    // Eight created, plus Library and Favorites.
    assert_eq!(playlists.as_array().unwrap().len(), 10);
}

// ── Playback daemon (needs an audio output) ──────────────────────────────────

/// Start with the volume at zero, so running these is silent.
fn silent(wave: &Wave) {
    std::fs::create_dir_all(wave.data_dir()).unwrap();
    std::fs::write(
        wave.data_dir().join("wave-settings.json"),
        r#"{"volume":0.0}"#,
    )
    .unwrap();
}

#[test]
#[ignore = "needs an audio output device"]
fn parallel_starts_share_one_daemon() {
    let wave = Wave::new();
    silent(&wave);
    let song = wave.write_wav("song.wav");
    let children: Vec<_> = (0..8)
        .map(|_| {
            wave.command(&["--json", "playback", "start", &song])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    wave.ok(&["playback", "status"]);
}

#[test]
#[ignore = "needs an audio output device"]
fn removing_the_playing_track_moves_on_to_the_next() {
    let wave = Wave::new();
    silent(&wave);
    let first = wave.write_wav("first.wav");
    let second = wave.write_wav("second.wav");
    let started = wave.ok(&["playback", "start", &first]);
    assert_eq!(started["status"]["file"], "first.wav");
    wave.ok(&["queue", "clear"]);
    // Every change reports where playback ended up.
    let added = wave.ok(&["queue", "add", &second]);
    assert_eq!(added["status"]["queue_length"], 2);
    let paused = wave.ok(&["playback", "pause"]);
    assert_eq!(paused["status"]["state"], "paused");

    let queue = wave.ok(&["queue", "list"]);
    assert_eq!(queue["queue_position"], 0, "positions count from 0");
    assert_eq!(queue["tracks"].as_array().unwrap().len(), 2);

    let removed = wave.ok(&["queue", "remove", "0"]);
    assert_eq!(removed["status"]["file"], "second.wav");
    assert_eq!(removed["status"]["state"], "paused");
    assert_eq!(removed["status"]["queue_position"], 0);

    wave.ok(&["queue", "remove", "0"]);
    let status = wave.ok(&["playback", "status"]);
    assert_eq!(status["state"], "stopped");
    assert_eq!(status["file"], Value::Null);
    assert_eq!(status["queue_position"], Value::Null);

    wave.fails(&["queue", "remove", "5"], EXIT_USAGE);
    wave.fails(&["queue", "repeat", "sideways"], EXIT_USAGE);
}

#[test]
fn watching_needs_json_and_a_running_daemon() {
    let wave = Wave::new();
    wave.fails(&["now", "--watch"], EXIT_NO_DAEMON);
    let plain = wave.run(&["now", "--watch"]);
    assert_eq!(plain.status.code(), Some(EXIT_USAGE));
}

#[test]
#[ignore = "needs an audio output device"]
fn the_watch_stream_reports_changes_until_the_daemon_stops() {
    use std::io::{BufRead, BufReader};

    let wave = Wave::new();
    silent(&wave);
    let song = wave.write_wav("song.wav");
    wave.ok(&["playback", "start", &song]);
    let mut watcher = wave
        .command(&["--json", "now", "--watch", "--interval", "0.1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(watcher.stdout.take().unwrap()).lines();
    let mut next_event = || -> Value {
        let line = lines.next().expect("stream ended early").unwrap();
        serde_json::from_str(&line).unwrap()
    };

    let first = next_event();
    assert_eq!(first["event"], "status");
    assert_eq!(first["schema_version"], 1);
    assert!(first["at"].as_u64().unwrap() > 0);
    assert_eq!(next_event()["event"], "queue");

    wave.ok(&["playback", "pause"]);
    // The short track may end on its own first; either way a status event
    // with the new state arrives.
    let paused = next_event();
    assert_eq!(paused["event"], "status");
    assert_ne!(paused["status"]["state"], "playing");

    wave.ok(&["playback", "shutdown"]);
    let mut last = next_event();
    while last["event"] == "status" {
        last = next_event();
    }
    assert_eq!(last["event"], "daemon_stopped");
    assert!(watcher.wait().unwrap().success());
}

#[test]
fn a_batch_runs_every_request_in_order() {
    let wave = Wave::new();
    let song = wave.write_wav("song.wav");
    wave.ok(&["tracks", "import", &song]);
    let input = format!(
        "# set up a playlist\n\
         playlists create \"Road Trip\"\n\
         [\"playlists\", \"add-track\", \"Road Trip\", \"{song}\"]\n\
         {{\"id\": \"again\", \"args\": [\"playlists\", \"create\", \"Road Trip\"]}}\n\
         tracks frobnicate\n\
         {{\"args\": 5}}\n\
         batch\n"
    );
    let (code, lines) = wave.lines(&["batch"], &input);
    assert_eq!(code, EXIT_GENERAL, "some requests failed");
    let summary: Vec<(i64, bool, &str)> = lines
        .iter()
        .map(|l| {
            assert_eq!(l["schema_version"], 1);
            (
                l["index"].as_i64().unwrap(),
                l["ok"].as_bool().unwrap(),
                l["kind"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (0, true, ""),
            (1, true, ""),
            (2, false, "conflict"),
            (3, false, "usage"),
            (4, false, "usage"),
            (5, false, "usage"),
        ]
    );
    assert_eq!(lines[0]["result"]["playlist"]["name"], "Road Trip");
    assert_eq!(lines[2]["id"], "again");
    assert_eq!(lines[2]["code"], EXIT_CONFLICT);
}

#[test]
fn a_batch_of_one_json_array_succeeds_as_a_whole() {
    let wave = Wave::new();
    let (code, lines) = wave.lines(
        &["batch"],
        r#"[["playlists", "create", "A"], {"id": 2, "args": ["playlists", "list"]}]"#,
    );
    assert_eq!(code, 0);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["id"], 2);
    assert_eq!(lines[1]["result"].as_array().unwrap().len(), 3);
}

#[test]
fn a_batch_can_stop_at_the_first_failure() {
    let wave = Wave::new();
    let (code, lines) = wave.lines(
        &["batch", "--stop-on-error"],
        "playlists create A\nplaylists create A\nplaylists create B\n",
    );
    assert_eq!(code, EXIT_GENERAL);
    assert_eq!(lines.len(), 2);
}

#[test]
fn a_dash_reads_tracks_from_stdin_in_any_list_format() {
    let wave = Wave::new();
    let a = wave.write_wav("a.wav");
    let b = wave.write_wav("b.wav");
    let missing = wave.path("music/missing.wav");

    let (code, lines) = wave.lines(&["tracks", "import", "-"], &format!("{a}\n{b}\n"));
    assert_eq!(code, 0);
    assert_eq!(lines[0]["imported"], 2, "one process, one summary");

    let (code, lines) = wave.lines(
        &["favorite", "add", "-"],
        &serde_json::to_string(&[&a, &missing]).unwrap(),
    );
    assert_eq!(code, EXIT_GENERAL);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["input"], a.as_str());
    assert_eq!(lines[0]["ok"], true);
    assert_eq!(lines[1]["kind"], "not_found");

    // NUL-separated, for paths that may contain newlines.
    let (code, lines) = wave.lines(&["favorite", "remove", "-"], &format!("{a}\0"));
    assert_eq!(code, 0);
    assert_eq!(lines.len(), 1);

    wave.ok(&["playlists", "create", "Mix"]);
    let (code, _) = wave.lines(
        &["playlists", "add-track", "Mix", "-"],
        &format!("{b}\n{a}\n"),
    );
    assert_eq!(code, 0);
    assert_eq!(
        wave.track_names(&["playlists", "info", "Mix"]),
        vec!["b.wav", "a.wav"]
    );
}

#[test]
#[ignore = "needs an audio output device"]
fn queueing_several_tracks_next_keeps_their_order() {
    let wave = Wave::new();
    silent(&wave);
    let now = wave.write_wav("now.wav");
    let a = wave.write_wav("a.wav");
    let b = wave.write_wav("b.wav");
    wave.ok(&["playback", "start", &now]);
    wave.ok(&["queue", "clear"]);
    wave.ok(&["playback", "pause"]);
    let (code, lines) = wave.lines(&["queue", "next", "-"], &format!("{a}\n{b}\n"));
    assert_eq!(code, 0);
    assert_eq!(lines[0]["input"], a.as_str(), "results stay in input order");
    let queue = wave.ok(&["queue", "list"]);
    let names: Vec<&str> = queue["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().rsplit('/').next().unwrap())
        .collect();
    assert_eq!(names, vec!["now.wav", "a.wav", "b.wav"]);
}

#[cfg(unix)]
#[test]
fn a_reader_closing_the_pipe_ends_output_quietly() {
    use std::io::Read as _;

    let wave = Wave::new();
    // Far more than a pipe buffer holds, so writing must hit the closed pipe.
    let mut child = wave
        .command(&["completions", "bash"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = [0u8; 16];
    child.stdout.take().unwrap().read_exact(&mut first).unwrap();
    // The read end is dropped here, as `head` would close it.
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(141), "{stderr}");
    assert!(stderr.is_empty(), "no panic message: {stderr}");
}

#[test]
#[ignore = "needs an audio output device"]
fn a_track_queued_next_mid_song_plays_right_after_it() {
    let wave = Wave::new();
    silent(&wave);
    let a = wave.write_wav_of("a.wav", 1.0);
    let b = wave.write_wav_of("b.wav", 0.5);
    let c = wave.write_wav_of("c.wav", 0.5);
    wave.start_playlist(&[&a, &b, &c]);
    // Gapless has already lined b up behind a by now.
    let x = wave.write_wav_of("x.wav", 0.5);
    wave.ok(&["queue", "next", &x]);
    let (order, _) = wave.play_order(std::time::Duration::from_secs(8));
    assert_eq!(order, ["a.wav", "x.wav", "b.wav", "c.wav"]);
}

#[test]
#[ignore = "needs an audio output device"]
fn crossfaded_tracks_each_play_once() {
    let wave = Wave::new();
    silent(&wave);
    wave.ok(&["dsp", "crossfade", "1"]);
    let a = wave.write_wav_of("a.wav", 2.0);
    let b = wave.write_wav_of("b.wav", 2.0);
    let c = wave.write_wav_of("c.wav", 2.0);
    wave.start_playlist(&[&a, &b, &c]);
    let (order, took) = wave.play_order(std::time::Duration::from_secs(12));
    assert_eq!(order, ["a.wav", "b.wav", "c.wav"]);
    // Six seconds of audio with two one-second overlaps. Playing a track
    // a second time, or waiting out a track before starting the next,
    // runs well past that.
    assert!(took.as_secs_f32() < 5.5, "took {took:?}");
}

#[test]
#[ignore = "needs an audio output device"]
fn an_unreadable_track_is_skipped_rather_than_ending_the_queue() {
    let wave = Wave::new();
    silent(&wave);
    let a = wave.write_wav_of("a.wav", 0.8);
    let b = wave.write_wav_of("b.wav", 0.8);
    let c = wave.write_wav_of("c.wav", 0.8);
    wave.ok(&["tracks", "import", &wave.path("music")]);
    // Imported fine, then replaced by something that will not decode.
    std::fs::write(&b, b"not audio").unwrap();
    wave.start_playlist(&[&a, &b, &c]);
    let (order, _) = wave.play_order(std::time::Duration::from_secs(6));
    assert_eq!(order, ["a.wav", "c.wav"]);
}

#[test]
#[ignore = "needs an audio output device"]
fn playing_the_second_song_of_a_couple_starts_with_the_first() {
    let wave = Wave::new();
    let first = wave.write_wav_of("first.wav", 0.6);
    let second = wave.write_wav_of("second.wav", 0.6);
    // The daemon resolves the path it is given, so the couple has to name
    // the resolved path (the temp dir sits behind a symlink on macOS).
    let canonical = |path: &str| std::fs::canonicalize(path).unwrap();
    let (first, second) = (canonical(&first), canonical(&second));
    let second = second.to_str().unwrap();
    std::fs::create_dir_all(wave.data_dir()).unwrap();
    let settings = serde_json::json!({
        "volume": 0.0,
        "song_coupling_enabled": true,
        "song_couples": [{ "first": first, "second": second }],
    });
    std::fs::write(
        wave.data_dir().join("wave-settings.json"),
        settings.to_string(),
    )
    .unwrap();

    let started = wave.ok(&["playback", "start", second]);
    assert_eq!(started["status"]["file"], "first.wav");
    let (order, _) = wave.play_order(std::time::Duration::from_secs(5));
    assert_eq!(order, ["first.wav", "second.wav"]);
}

// ── Hosted server: the documented `wave serve` contract ─────────────────────

fn free_port() -> u16 {
    TcpListener::bind(("0.0.0.0", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn read_json_file(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap())
        .unwrap_or_else(|e| panic!("invalid JSON in {}: {e}", path.display()))
}

fn write_png(path: &Path) -> String {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(file, 1, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[200, 50, 75, 255])
        .unwrap();
    path.to_string_lossy().into_owned()
}

#[cfg(unix)]
fn assert_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "{} has mode {mode:o}", path.display());
}

#[test]
fn serve_help_documents_every_flag_and_rule() {
    let wave = Wave::new();
    let cases: &[(&[&str], &[&str])] = &[
        (
            &["serve", "help"],
            &[
                "--service wave|subsonic|both",
                "--scope loopback|lan|internet",
                "--beacon on|off",
                "wave serve start --help",
                "wave serve config --help",
                "wave-server.json",
            ],
        ),
        (
            &["serve", "start", "--help"],
            &[
                "--mode <MODE>",
                "--service <SERVICE>",
                "--scope <SCOPE>",
                "--beacon <BEACON>",
                "--port <PORT>",
                "--name <NAME>",
                "--user <USER>",
                "--password <PASSWORD>",
                "--scope internet forces --service subsonic",
                "The Wave GUI can only pair with the /wave/ API at LAN scope",
                "plain HTTP",
            ],
        ),
        (
            &["serve", "config", "--help"],
            &[
                "--service <SERVICE>",
                "--scope <SCOPE>",
                "--beacon <BEACON>",
                "--port <PORT>",
                "--name <NAME>",
                "--user <USER>",
                "--password <PASSWORD>",
                "applies_on_restart: true",
                "Omit to keep the saved value",
                "same validation rules apply",
            ],
        ),
    ];
    for (args, needles) in cases {
        let out = wave.run(args);
        assert!(
            out.status.success(),
            "wave {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        for needle in *needles {
            assert!(
                text.contains(needle),
                "wave {args:?} help is missing {needle:?}:\n{text}"
            );
        }
    }
}

/// One raw request, because Subsonic responses and media bytes share a port.
fn http_get(port: u16, path: &str) -> (String, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP headers");
    let status = String::from_utf8_lossy(&response[..split])
        .lines()
        .next()
        .unwrap()
        .to_string();
    (status, response[split + 4..].to_vec())
}

fn rest(port: u16, endpoint: &str, user: &str, password: &str, extra: &[(&str, &str)]) -> Value {
    let mut query = format!("u={user}&p={password}&v=1.16.1&c=wave-cli-test");
    for (key, value) in extra {
        query.push_str(&format!("&{key}={value}"));
    }
    let (_, body) = http_get(port, &format!("/rest/{endpoint}.view?{query}"));
    serde_json::from_slice(&body).unwrap_or_else(|e| panic!("{endpoint}: {e}: {body:?}"))
}

fn subsonic_ok(value: &Value) -> Value {
    let response = &value["subsonic-response"];
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(response["version"], "1.16.1");
    assert_eq!(response["type"], "Wave");
    assert_eq!(response["openSubsonic"], true);
    response.clone()
}

fn subsonic_error(value: &Value, code: i64) {
    let response = &value["subsonic-response"];
    assert_eq!(response["status"], "failed", "{response}");
    assert_eq!(response["error"]["code"], code, "{response}");
    assert!(response["error"]["message"].is_string());
}

/// One request to the loopback control socket, which is not an HTTP listener.
fn control_request(port: u16, token: &str, request: &str) -> Value {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(
        stream,
        "{}",
        serde_json::json!({ "token": token, "request": request })
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    serde_json::from_str(reply.trim()).unwrap_or_else(|e| panic!("control reply: {e}: {reply:?}"))
}

#[test]
fn serve_status_start_and_stop_follow_the_json_contract() {
    let wave = Wave::new();
    let port = free_port();

    let before = wave.ok(&["serve", "status"]);
    assert_eq!(before["running"], false);
    for field in [
        "mode",
        "name",
        "user",
        "port",
        "pid",
        "service",
        "scope",
        "beacon",
        "addresses",
    ] {
        assert!(before.get(field).is_some(), "status before start: {before}");
    }

    let started = wave.ok(&[
        "serve",
        "start",
        "--port",
        &port.to_string(),
        "--name",
        "Doc Test Wave",
    ]);
    assert_eq!(started["running"], true);
    assert_eq!(started["mode"], "server");
    assert_eq!(started["name"], "Doc Test Wave");
    assert_eq!(started["user"], "wave");
    assert_eq!(started["service"], "both");
    assert_eq!(started["scope"], "lan");
    assert_eq!(started["beacon"], true);
    // The port can fall back if another process took the requested one.
    let port = started["port"].as_u64().unwrap() as u16;
    assert_eq!(started["port"], port);
    assert!(started["pid"].as_u64().unwrap() > 0);
    assert!(!started["addresses"].as_array().unwrap().is_empty());
    let password = started["password"].as_str().expect("generated password");
    assert_eq!(password.len(), 36, "a generated UUIDv4: {password}");
    assert!(started["message"]
        .as_str()
        .unwrap()
        .contains("save this password now"));

    let config_path = wave.data_dir().join("wave-server.json");
    let state_path = wave.data_dir().join("wave-server-state.json");
    let config = read_json_file(&config_path);
    assert_eq!(config["password"], password);
    assert_eq!(config["name"], "Doc Test Wave");
    assert_eq!(config["service"], "both");
    assert_eq!(config["scope"], "lan");
    assert_eq!(config["beacon"], true);
    #[cfg(unix)]
    {
        assert_owner_only(&config_path);
        assert_owner_only(&state_path);
    }
    let state = read_json_file(&state_path);
    assert_eq!(state["pid"], started["pid"]);
    assert!(state["control_port"].as_u64().unwrap() > 0);
    assert!(state["token"].as_str().unwrap().len() > 20);
    assert_eq!(state["service"], "both");
    assert_eq!(state["scope"], "lan");
    assert_eq!(state["beacon"], true);
    assert!(state.get("password").is_none());
    assert!(wave.data_dir().join("wave-server.lock").exists());

    let control_port = state["control_port"].as_u64().unwrap() as u16;
    let denied = control_request(control_port, "not-the-token", "ping");
    assert_eq!(denied["ok"], false, "a wrong token is refused: {denied}");
    let accepted = control_request(control_port, state["token"].as_str().unwrap(), "ping");
    assert_eq!(
        accepted["ok"], true,
        "the saved token is accepted: {accepted}"
    );

    let still_running = wave.ok(&[
        "serve",
        "start",
        "--name",
        "Ignored Name",
        "--user",
        "ignored",
        "--password",
        "ignored",
    ]);
    assert_eq!(still_running["running"], true);
    assert_eq!(still_running["name"], "Doc Test Wave");
    assert_eq!(still_running["user"], "wave");
    assert_eq!(still_running["port"], port);
    assert_eq!(read_json_file(&config_path)["name"], "Doc Test Wave");

    let status = wave.ok(&["serve", "status"]);
    assert_eq!(status["running"], true);
    assert_eq!(status["mode"], "server");
    assert_eq!(status["name"], "Doc Test Wave");
    assert_eq!(status["user"], "wave");
    assert_eq!(status["service"], "both");
    assert_eq!(status["scope"], "lan");
    assert_eq!(status["beacon"], true);
    assert_eq!(status["port"], port);
    assert_eq!(status["pid"], started["pid"]);
    assert!(status.get("password").is_none(), "status never prints it");

    let stopped = wave.ok(&["serve", "stop"]);
    assert_eq!(stopped["stopped"], true);
    assert!(!state_path.exists(), "stop clears runtime state");

    let after = wave.ok(&["serve", "status"]);
    assert_eq!(after["running"], false);
    assert!(after["pid"].is_null());
    assert_eq!(after["name"], "Doc Test Wave");
    assert!(after.get("password").is_none());
    let again = wave.ok(&["serve", "stop"]);
    assert_eq!(again["stopped"], false);
}

#[test]
fn serve_exposes_the_documented_subsonic_apis_and_auth_forms() {
    let wave = Wave::new();
    let music = wave.write_wav("serve song.wav");
    wave.ok(&["tracks", "import", &music]);
    wave.ok(&[
        "metadata",
        "set",
        &music,
        "--title",
        "Serve Song",
        "--artist",
        "Serve Artist",
        "--album",
        "Serve Album",
    ]);
    let cover_path = Path::new(&music).with_file_name("cover.png");
    let cover = write_png(&cover_path);
    wave.ok(&["metadata", "cover-set", &music, &cover]);
    wave.ok(&["playlists", "create", "Serve List"]);
    wave.ok(&["playlists", "add-track", "Serve List", &music]);
    let playlists = wave.ok(&["playlists", "list"]);
    let playlist_id = playlists
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Serve List")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let port = free_port();
    let started = wave.ok(&[
        "serve",
        "start",
        "--port",
        &port.to_string(),
        "--user",
        "music",
        "--password",
        "secret",
    ]);
    let port = started["port"].as_u64().unwrap() as u16;

    subsonic_ok(&rest(port, "ping", "music", "secret", &[]));
    assert_eq!(
        subsonic_ok(&rest(port, "getLicense", "music", "secret", &[]))["license"]["valid"],
        true
    );
    for unsupported in ["scrobble", "setRating"] {
        subsonic_error(&rest(port, unsupported, "music", "secret", &[]), 0);
    }
    let artists = subsonic_ok(&rest(port, "getArtists", "music", "secret", &[]));
    subsonic_ok(&rest(port, "getIndexes", "music", "secret", &[]));
    let artist_id = artists["artists"]["index"][0]["artist"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    subsonic_ok(&rest(
        port,
        "getArtist",
        "music",
        "secret",
        &[("id", &artist_id)],
    ));
    subsonic_ok(&rest(port, "getMusicDirectory", "music", "secret", &[]));
    subsonic_ok(&rest(port, "getAlbumList", "music", "secret", &[]));
    subsonic_ok(&rest(port, "getAlbumList2", "music", "secret", &[]));
    subsonic_ok(&rest(
        port,
        "search",
        "music",
        "secret",
        &[("query", "Serve")],
    ));
    subsonic_ok(&rest(
        port,
        "search2",
        "music",
        "secret",
        &[("query", "Serve")],
    ));
    let found = subsonic_ok(&rest(
        port,
        "search3",
        "music",
        "secret",
        &[("query", "Serve")],
    ));
    let song_id = found["searchResult3"]["song"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let album_id = found["searchResult3"]["song"][0]["albumId"]
        .as_str()
        .unwrap()
        .to_string();
    subsonic_ok(&rest(
        port,
        "getSong",
        "music",
        "secret",
        &[("id", &song_id)],
    ));
    subsonic_ok(&rest(
        port,
        "getAlbum",
        "music",
        "secret",
        &[("id", &album_id)],
    ));
    subsonic_ok(&rest(
        port,
        "getMusicDirectory",
        "music",
        "secret",
        &[("id", &album_id)],
    ));
    let playlists = subsonic_ok(&rest(port, "getPlaylists", "music", "secret", &[]));
    assert!(playlists["playlists"]["playlist"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "Serve List"));
    let playlist = subsonic_ok(&rest(
        port,
        "getPlaylist",
        "music",
        "secret",
        &[("id", &playlist_id)],
    ));
    assert_eq!(playlist["playlist"]["songCount"], 1);
    let (status, body) = http_get(
        port,
        &format!("/rest/getCoverArt.view?u=music&p=secret&id={song_id}"),
    );
    assert!(status.contains("200 OK"), "getCoverArt: {status}");
    assert!(body.starts_with(&[0xff, 0xd8]), "a JPEG: {body:?}");

    for endpoint in ["stream", "download"] {
        let (status, body) = http_get(
            port,
            &format!("/rest/{endpoint}.view?u=music&p=secret&id={song_id}"),
        );
        assert!(status.contains("200 OK"), "{endpoint}: {status}");
        assert!(body.starts_with(b"RIFF"), "{endpoint}: {body:?}");
    }

    subsonic_error(&rest(port, "ping", "music", "wrong", &[]), 40);
    subsonic_error(&rest(port, "ping", "wave", "secret", &[]), 40);
    subsonic_error(&rest(port, "notAnEndpoint", "music", "secret", &[]), 0);
    // Subsonic's optional password encodings: uppercase hex and salted MD5.
    subsonic_ok(&rest(
        port,
        "ping",
        "music",
        "ignored",
        &[("p", "enc:736563726574")],
    ));
    subsonic_ok(&rest(
        port,
        "ping",
        "music",
        "ignored",
        &[("s", "abc123"), ("t", "f22ae4f5eacb35c993e910606a413410")],
    ));

    let hello = http_get(port, "/wave/hello");
    assert!(hello.0.contains("200 OK"));
    let hello: Value = serde_json::from_slice(&hello.1).unwrap();
    assert!(hello["name"].as_str().unwrap().contains("Wave"));
    assert!(!hello["version"].as_str().unwrap().is_empty());
    assert!(!hello["id"].as_str().unwrap().is_empty());

    wave.ok(&["serve", "stop"]);
}

#[test]
fn serve_keeps_saved_settings_and_reports_the_bound_port() {
    let wave = Wave::new();
    let port = free_port();
    let first = wave.ok(&[
        "serve",
        "start",
        "--port",
        &port.to_string(),
        "--name",
        "Living Room",
        "--user",
        "music",
        "--password",
        "secret",
    ]);
    let port = first["port"].as_u64().unwrap() as u16;
    wave.ok(&["serve", "stop"]);

    let restarted = wave.ok(&["serve", "start"]);
    assert_eq!(restarted["name"], "Living Room");
    assert_eq!(restarted["user"], "music");
    assert_eq!(restarted["port"], port);
    assert!(restarted.get("password").is_none());
    subsonic_ok(&rest(port, "ping", "music", "secret", &[]));
    wave.ok(&["serve", "stop"]);

    let requested = free_port();
    let blocker = TcpListener::bind(("0.0.0.0", requested)).unwrap();
    let started = wave.ok(&["serve", "start", "--port", &requested.to_string()]);
    let actual = started["port"].as_u64().unwrap() as u16;
    assert_ne!(actual, requested, "a taken port falls back to a free one");
    subsonic_ok(&rest(actual, "ping", "music", "secret", &[]));
    wave.ok(&["serve", "stop"]);
    drop(blocker);
}

#[test]
fn serve_service_scope_and_beacon_control_what_is_exposed() {
    let wave = Wave::new();
    let port = free_port();
    let started = wave.ok(&[
        "serve",
        "start",
        "--port",
        &port.to_string(),
        "--service",
        "subsonic",
        "--scope",
        "internet",
        "--user",
        "music",
        "--password",
        "secret",
    ]);
    let port = started["port"].as_u64().unwrap() as u16;
    assert_eq!(started["service"], "subsonic");
    assert_eq!(started["scope"], "internet");
    assert_eq!(started["beacon"], false);
    for field in ["service", "scope", "beacon"] {
        assert_eq!(
            wave.ok(&["serve", "status"])[field],
            started[field],
            "status must report the active {field}"
        );
    }
    subsonic_ok(&rest(port, "ping", "music", "secret", &[]));
    let (status, _) = http_get(port, "/wave/hello");
    assert!(
        status.contains("404"),
        "internet never exposes /wave/: {status}"
    );
    // An impossible combination is rejected even while a server is up; it
    // must not be silently accepted by the already-running shortcut.
    let error = wave.fails(
        &["serve", "start", "--scope", "internet", "--service", "both"],
        2,
    );
    assert!(error.contains("unsupported"), "{error}");
    assert_eq!(wave.ok(&["serve", "status"])["running"], true);
    wave.ok(&["serve", "stop"]);

    let port = free_port();
    let started = wave.ok(&[
        "serve",
        "start",
        "--port",
        &port.to_string(),
        "--service",
        "wave",
        "--scope",
        "loopback",
        "--beacon",
        "off",
        "--name",
        "Loopback Wave",
    ]);
    let port = started["port"].as_u64().unwrap() as u16;
    assert_eq!(started["service"], "wave");
    assert_eq!(started["scope"], "loopback");
    assert_eq!(started["beacon"], false);
    let addresses = started["addresses"].as_array().unwrap();
    assert_eq!(
        addresses.len(),
        1,
        "loopback advertises one address: {addresses:?}"
    );
    assert!(addresses[0].as_str().unwrap().contains("127.0.0.1"));
    let (status, _) = http_get(port, "/wave/hello");
    assert!(status.contains("200 OK"), "the Wave API is on: {status}");
    let (status, _) = http_get(
        port,
        "/rest/ping.view?u=wave&p=secret&v=1.16.1&c=wave-cli-test",
    );
    assert!(status.contains("404"), "Subsonic is off: {status}");
    wave.ok(&["serve", "stop"]);

    let error = wave.fails(
        &["serve", "start", "--scope", "internet", "--service", "both"],
        2,
    );
    assert!(error.contains("unsupported"), "{error}");
    let error = wave.fails(
        &["serve", "start", "--scope", "internet", "--beacon", "on"],
        2,
    );
    assert!(error.contains("unsupported"), "{error}");
    let error = wave.fails(
        &["serve", "start", "--service", "subsonic", "--beacon", "on"],
        2,
    );
    assert!(error.contains("unsupported"), "{error}");
}

#[test]
fn serve_config_changes_saved_settings_and_reports_restart() {
    let wave = Wave::new();
    let port = free_port();
    let started = wave.ok(&[
        "serve",
        "start",
        "--port",
        &port.to_string(),
        "--name",
        "Before",
        "--password",
        "secret",
    ]);
    let port = started["port"].as_u64().unwrap() as u16;
    assert_eq!(started["service"], "both");
    assert_eq!(started["scope"], "lan");
    assert_eq!(started["beacon"], true);

    let configured = wave.ok(&[
        "serve",
        "config",
        "--name",
        "After",
        "--user",
        "music",
        "--password",
        "changed",
        "--scope",
        "internet",
        "--beacon",
        "off",
    ]);
    assert_eq!(configured["applies_on_restart"], true);
    assert_eq!(configured["name"], "After");
    assert_eq!(configured["user"], "music");
    assert_eq!(configured["service"], "subsonic");
    assert_eq!(configured["scope"], "internet");
    assert_eq!(configured["beacon"], false);
    assert!(configured.get("password").is_none());

    let active = wave.ok(&["serve", "status"]);
    assert_eq!(active["name"], "Before");
    assert_eq!(active["user"], "wave");
    assert_eq!(active["service"], "both");
    assert_eq!(active["scope"], "lan");
    assert_eq!(active["beacon"], true);
    subsonic_ok(&rest(port, "ping", "wave", "secret", &[]));

    assert_eq!(wave.ok(&["serve", "stop"])["stopped"], true);
    let saved = wave.ok(&["serve", "status"]);
    assert_eq!(saved["running"], false);
    assert!(saved["pid"].is_null());
    assert_eq!(saved["name"], "After");
    assert_eq!(saved["service"], "subsonic");
    assert_eq!(saved["scope"], "internet");
    assert_eq!(saved["beacon"], false);

    let restarted = wave.ok(&["serve", "start"]);
    let port = restarted["port"].as_u64().unwrap() as u16;
    assert_eq!(restarted["name"], "After");
    assert_eq!(restarted["user"], "music");
    assert_eq!(restarted["service"], "subsonic");
    assert_eq!(restarted["scope"], "internet");
    assert_eq!(restarted["beacon"], false);
    subsonic_ok(&rest(port, "ping", "music", "changed", &[]));
    subsonic_error(&rest(port, "ping", "music", "secret", &[]), 40);
    let (status, _) = http_get(port, "/wave/hello");
    assert!(
        status.contains("404"),
        "the new service filter is active: {status}"
    );
    wave.ok(&["serve", "stop"]);

    let configured = wave.ok(&["serve", "config", "--name", "Idle"]);
    assert_eq!(configured["applies_on_restart"], false);
}

#[test]
#[ignore = "needs an audio output device"]
fn serve_instance_mode_starts_and_stops_the_playback_daemon() {
    let wave = Wave::new();
    let music = wave.write_wav("instance.wav");
    wave.ok(&["tracks", "import", &music]);
    let daemon_state = wave.data_dir().join("playback-daemon.json");
    assert!(!daemon_state.exists(), "the test starts with no daemon");

    let port = free_port();
    let started = wave.ok(&[
        "serve",
        "start",
        "--mode",
        "instance",
        "--port",
        &port.to_string(),
        "--user",
        "music",
        "--password",
        "secret",
    ]);
    let port = started["port"].as_u64().unwrap() as u16;
    assert_eq!(started["mode"], "instance");
    assert_eq!(wave.ok(&["serve", "status"])["mode"], "instance");
    subsonic_ok(&rest(port, "ping", "music", "secret", &[]));

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !daemon_state.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(daemon_state.exists(), "instance start spawns the daemon");
    let daemon_pid = read_json_file(&daemon_state)["pid"].as_u64().unwrap();
    assert!(is_alive(daemon_pid), "the spawned daemon is running");

    assert_eq!(wave.ok(&["serve", "stop"])["stopped"], true);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while is_alive(daemon_pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!is_alive(daemon_pid), "instance stop shuts the daemon down");
}

#[test]
#[ignore = "needs an audio output device"]
fn serve_instance_mode_leaves_an_existing_daemon_running() {
    let wave = Wave::new();
    let music = wave.write_wav("existing-daemon.wav");
    wave.ok(&["tracks", "import", &music]);
    let daemon_state = wave.data_dir().join("playback-daemon.json");
    wave.ok(&["playback", "start", &music]);
    let daemon_pid = read_json_file(&daemon_state)["pid"].as_u64().unwrap();
    assert!(is_alive(daemon_pid), "the daemon exists before the server");

    let port = free_port();
    let started = wave.ok(&[
        "serve",
        "start",
        "--mode",
        "instance",
        "--port",
        &port.to_string(),
        "--user",
        "music",
        "--password",
        "secret",
    ]);
    assert_eq!(started["mode"], "instance");
    let state = read_json_file(&wave.data_dir().join("wave-server-state.json"));
    assert_eq!(state["owns_daemon"], false, "the server did not spawn it");

    assert_eq!(wave.ok(&["serve", "stop"])["stopped"], true);
    assert!(
        is_alive(daemon_pid),
        "instance stop leaves another owner's daemon running"
    );
    wave.ok(&["playback", "shutdown"]);
}

fn is_alive(pid: u64) -> bool {
    #[cfg(unix)]
    {
        let result = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        result.success()
    }
    #[cfg(not(unix))]
    {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .expect("tasklist is available on Windows");
        String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
    }
}
