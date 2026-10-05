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

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

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
        let path = self.music(name);
        write_silent_wav(Path::new(&path));
        path
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
        // A daemon started by a test must not outlive it.
        let _ = self.run(&["playback", "shutdown"]);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Half a second of silent 16-bit mono PCM.
fn write_silent_wav(path: &Path) {
    let samples = vec![0u8; 8000];
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
