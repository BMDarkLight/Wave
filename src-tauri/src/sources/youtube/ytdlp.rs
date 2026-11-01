// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Runs the installed yt-dlp: searches YouTube Music, and turns a video id
//! into a direct audio URL the player can stream.
//!
//! Every call is a blocking child process with a deadline, so like the rest
//! of [`crate::sources`] it must stay off the command response path.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::tools::Tool;
use super::{expires_from_url, is_video_id, Candidate, Stream};

const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Extraction solves YouTube's player challenge in Deno, which is the slow
/// part of a first play.
const STREAM_TIMEOUT: Duration = Duration::from_secs(30);
/// AAC in MP4 first: Wave seeks in YouTube's MP4 files to the sample, and
/// can't seek in its WebM ones. Opus in WebM is the fallback.
const AUDIO_FORMAT: &str = "140/bestaudio[ext=m4a]/251/bestaudio[acodec!=none][vcodec=none]";

pub struct YtDlp {
    program: PathBuf,
    deno: PathBuf,
    cache: PathBuf,
}

impl YtDlp {
    /// The programs installed in `dir`, or `None` until both are there.
    pub fn in_dir(dir: &Path) -> Option<Self> {
        let program = Tool::YtDlp.path(dir);
        let deno = Tool::Deno.path(dir);
        (program.is_file() && deno.is_file()).then(|| Self {
            program,
            deno,
            cache: dir.join("cache"),
        })
    }

    /// Videos on YouTube for `query`, best match first. YouTube's own search
    /// rather than YouTube Music's: only it reports each result's length and
    /// channel, which is what tells the album cut from the radio edit.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Candidate>, String> {
        let search = format!("ytsearch{}:{query}", limit.clamp(1, 20));
        let out = self.run(&["--flat-playlist", "-J", "--", &search], SEARCH_TIMEOUT)?;
        parse_search(&out)
    }

    /// The audio stream for one video, in format `itag` when given and
    /// otherwise the best audio-only format.
    pub fn stream_format(&self, video_id: &str, itag: Option<&str>) -> Result<Stream, String> {
        if !is_video_id(video_id) {
            return Err(format!("Not a YouTube video id: {video_id}"));
        }
        let format = match itag {
            Some(itag) if !itag.is_empty() && itag.bytes().all(|b| b.is_ascii_digit()) => itag,
            Some(itag) => return Err(format!("Not a YouTube format: {itag}")),
            None => AUDIO_FORMAT,
        };
        let url = format!("https://music.youtube.com/watch?v={video_id}");
        let out = self.run(
            &["-J", "--no-playlist", "-f", format, "--", &url],
            STREAM_TIMEOUT,
        )?;
        parse_stream(&out)
    }

    fn run(&self, args: &[&str], timeout: Duration) -> Result<String, String> {
        let deno = format!("deno:{}", self.deno.display());
        let cache = self.cache.display().to_string();
        let mut command = Command::new(&self.program);
        command
            // The user's own yt-dlp config must not change what Wave gets.
            .args(["--ignore-config", "--no-warnings", "--no-progress"])
            .args(["--js-runtimes", &deno, "--cache-dir", &cache])
            .args(args)
            .env("PYTHONUTF8", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let output = run_with_deadline(command, timeout)?;
        if output.success {
            Ok(output.stdout)
        } else {
            Err(error_line(&output.stderr))
        }
    }
}

struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Run `command`, killing it once `timeout` passes. Both pipes are drained on
/// their own threads so a chatty child can't block on a full pipe.
fn run_with_deadline(mut command: Command, timeout: Duration) -> Result<Output, String> {
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start yt-dlp: {e}"))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let mut bytes = Vec::new();
                let _ = pipe.read_to_end(&mut bytes);
                text = String::from_utf8_lossy(&bytes).into_owned();
            }
            text
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("yt-dlp took longer than {}s", timeout.as_secs()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(format!("Lost track of yt-dlp: {e}")),
        }
    };
    Ok(Output {
        success: status.success(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// yt-dlp's own explanation: its last `ERROR:` line, or failing that its last
/// line of any kind.
fn error_line(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .rev()
        .find_map(|l| l.strip_prefix("ERROR:"))
        .map(str::trim)
        .or(lines.last().copied())
        .unwrap_or("yt-dlp failed without saying why")
        .to_string()
}

#[derive(Deserialize)]
struct SearchJson {
    #[serde(default)]
    entries: Vec<EntryJson>,
}

#[derive(Deserialize)]
struct EntryJson {
    id: Option<String>,
    title: Option<String>,
    channel: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
}

fn parse_search(json: &str) -> Result<Vec<Candidate>, String> {
    let parsed: SearchJson =
        serde_json::from_str(json).map_err(|e| format!("Unexpected yt-dlp output: {e}"))?;
    Ok(parsed
        .entries
        .into_iter()
        .filter_map(|entry| {
            let id = entry.id.filter(|id| is_video_id(id))?;
            Some(Candidate {
                id,
                title: entry.title.unwrap_or_default(),
                channel: entry.channel.or(entry.uploader),
                duration_seconds: entry.duration,
            })
        })
        .collect())
}

#[derive(Deserialize)]
struct StreamJson {
    url: Option<String>,
    ext: Option<String>,
    acodec: Option<String>,
    duration: Option<f64>,
    filesize: Option<u64>,
    format_id: Option<String>,
    #[serde(default)]
    http_headers: std::collections::BTreeMap<String, String>,
}

fn parse_stream(json: &str) -> Result<Stream, String> {
    let parsed: StreamJson =
        serde_json::from_str(json).map_err(|e| format!("Unexpected yt-dlp output: {e}"))?;
    let url = parsed
        .url
        .filter(|u| u.starts_with("https://"))
        .ok_or("yt-dlp found no audio stream")?;
    let expires_at = expires_from_url(&url);
    Ok(Stream {
        itag: parsed.format_id,
        url,
        http_headers: parsed.http_headers.into_iter().collect(),
        ext: parsed.ext.unwrap_or_else(|| "webm".into()),
        codec: parsed.acodec.filter(|c| c != "none"),
        duration_seconds: parsed.duration,
        filesize: parsed.filesize,
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_eleven_url_safe_characters_make_a_video_id() {
        assert!(is_video_id("dQw4w9WgXcQ"));
        assert!(is_video_id("a-b_c-d_e-f"));
        assert!(!is_video_id("-x"));
        assert!(!is_video_id("dQw4w9WgXcQ1"));
        assert!(!is_video_id("dQw4w9WgX Q"));
        assert!(!is_video_id("--exec=rm -"));
    }

    #[test]
    fn reads_search_results_and_drops_entries_without_an_id() {
        let json = r#"{"entries": [
            {"id": "aaaaaaaaaaa", "title": "Song", "channel": "Artist - Topic", "duration": 215.0},
            {"id": "bbbbbbbbbbb", "title": "Song (Live)", "uploader": "Someone"},
            {"title": "no id"},
            {"id": "UCnotavideo_channel", "title": "a channel"}
        ]}"#;
        let found = parse_search(json).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].channel.as_deref(), Some("Artist - Topic"));
        assert_eq!(found[0].duration_seconds, Some(215.0));
        assert_eq!(
            found[1].channel.as_deref(),
            Some("Someone"),
            "uploader stands in"
        );
        assert_eq!(parse_search("{}").unwrap(), vec![]);
    }

    #[test]
    fn reads_the_stream_and_when_it_expires() {
        let json = r#"{
            "url": "https://rr1---sn-x.googlevideo.com/videoplayback?expire=1793600000&itag=251",
            "ext": "webm", "acodec": "opus", "duration": 215.4, "filesize": 3500000, "format_id": "251",
            "http_headers": {"User-Agent": "Mozilla/5.0", "Accept": "*/*"}
        }"#;
        let stream = parse_stream(json).unwrap();
        assert_eq!(stream.ext, "webm");
        assert_eq!(stream.itag.as_deref(), Some("251"));
        assert_eq!(stream.codec.as_deref(), Some("opus"));
        assert_eq!(stream.expires_at, Some(1_793_600_000));
        assert_eq!(stream.filesize, Some(3_500_000));
        assert!(stream
            .http_headers
            .contains(&("User-Agent".into(), "Mozilla/5.0".into())));
    }

    #[test]
    fn a_stream_without_an_https_url_is_an_error() {
        assert!(parse_stream(r#"{"ext": "webm"}"#).is_err());
        assert!(parse_stream(r#"{"url": "file:///etc/passwd"}"#).is_err());
    }

    #[test]
    fn reports_yt_dlps_own_error() {
        let stderr =
            "[youtube] abc: Downloading webpage\nERROR: [youtube] abc: Video unavailable\n";
        assert_eq!(error_line(stderr), "[youtube] abc: Video unavailable");
        assert_eq!(error_line("Traceback\nKeyError: 'x'\n"), "KeyError: 'x'");
        assert_eq!(error_line(""), "yt-dlp failed without saying why");
    }

    #[cfg(unix)]
    #[test]
    fn a_child_past_its_deadline_is_killed() {
        let mut command = Command::new("sleep");
        command
            .arg("5")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = Instant::now();
        let err = run_with_deadline(command, Duration::from_millis(200))
            .err()
            .unwrap();
        assert!(err.contains("longer than"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
