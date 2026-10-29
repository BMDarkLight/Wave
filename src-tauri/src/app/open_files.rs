// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Audio files opened from outside Wave: a double click or "Open With" in the
//! file manager, or files dropped on the window. They play at once in the mini
//! player and stay out of the library.
//!
//! Windows and Linux start `wave <files>`. When Wave is already running, that
//! second process hands the files over a loopback socket and exits, the same
//! way `wave playback` talks to the playback daemon. macOS sends an Apple
//! event to the running app instead, which arrives as `RunEvent::Opened`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::app_paths::gui_handoff_path;
use crate::metadata::is_supported_audio_file;

const HANDOFF_TIMEOUT: Duration = Duration::from_secs(5);
/// A list of paths, so a few hundred files fit with room to spare.
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct HandoffFile {
    pid: u32,
    port: u16,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct HandoffRequest {
    token: String,
    open: Vec<String>,
}

/// The audio files named on the command line, when every argument is one.
/// Anything else (a flag, a subcommand, a missing file) means the CLI.
pub fn audio_files_in_args(args: &[String]) -> Option<Vec<String>> {
    let files: Vec<String> = args
        .iter()
        .skip(1)
        .map(|arg| audio_file(arg))
        .collect::<Option<_>>()?;
    (!files.is_empty()).then_some(files)
}

/// `arg` as an absolute path to an audio file, accepting `file://` URLs too.
fn audio_file(arg: &str) -> Option<String> {
    let path = match arg.strip_prefix("file://") {
        Some(_) => url::Url::parse(arg).ok()?.to_file_path().ok()?,
        None => PathBuf::from(arg),
    };
    if !path.is_file() || !is_supported_audio_file(&path) {
        return None;
    }
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    Some(path.to_string_lossy().into_owned())
}

/// The audio files among `paths`, in order. Used for drops and Apple events,
/// which can mix in folders and other files.
pub fn audio_files_only(paths: impl IntoIterator<Item = PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .filter_map(|path| audio_file(&path.to_string_lossy()))
        .collect()
}

/// Give `files` to the Wave that is already running, or with none, ask it to
/// show itself. One that is still starting gets a few seconds to start
/// listening.
pub fn hand_off(files: &[String]) -> Result<(), String> {
    let started = std::time::Instant::now();
    let target = loop {
        if let Some(target) = read_handoff_file() {
            break target;
        }
        if started.elapsed() >= HANDOFF_TIMEOUT {
            return Err(crate::single_instance::already_running_message());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let addr = format!("127.0.0.1:{}", target.port);
    let mut stream =
        TcpStream::connect_timeout(&addr.parse().map_err(|e| format!("{e}"))?, HANDOFF_TIMEOUT)
            .map_err(|e| format!("Could not reach the running Wave: {e}"))?;
    stream
        .set_read_timeout(Some(HANDOFF_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let request = HandoffRequest {
        token: target.token,
        open: files.to_vec(),
    };
    let line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    writeln!(stream, "{line}").map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    if reply.trim() == "ok" {
        Ok(())
    } else {
        Err(format!(
            "The running Wave did not take the files: {}",
            reply.trim()
        ))
    }
}

fn read_handoff_file() -> Option<HandoffFile> {
    let contents = std::fs::read_to_string(gui_handoff_path()).ok()?;
    let file: HandoffFile = serde_json::from_str(&contents).ok()?;
    crate::single_instance::is_process_alive(file.pid).then_some(file)
}

/// Start taking files from later launches. `open` runs on a background
/// thread for each batch, and gets no files for a launch that only wants
/// Wave brought up.
pub fn listen(open: impl Fn(Vec<String>) + Send + Sync + 'static) -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = uuid::Uuid::new_v4().to_string();
    write_handoff_file(port, &token)?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if let Some(files) = read_request(stream, &token) {
                open(files);
            }
        }
    });
    Ok(())
}

/// Drop the published port, so a launch after Wave quits starts afresh.
pub fn stop_listening() {
    let path = gui_handoff_path();
    if read_handoff_file().is_some_and(|file| file.pid == std::process::id()) {
        let _ = std::fs::remove_file(path);
    }
}

fn read_request(mut stream: TcpStream, token: &str) -> Option<Vec<String>> {
    stream.set_read_timeout(Some(HANDOFF_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(HANDOFF_TIMEOUT)).ok()?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_REQUEST_BYTES))
        .read_line(&mut line)
        .ok()?;
    let request: HandoffRequest = serde_json::from_str(line.trim()).ok()?;
    if request.token != token {
        let _ = writeln!(stream, "unauthorized");
        return None;
    }
    let _ = writeln!(stream, "ok");
    // No files: a plain launch, which only wants the running Wave shown.
    if request.open.is_empty() {
        return Some(Vec::new());
    }
    // Checked again here: the token keeps other users out, not bad paths.
    let files = audio_files_only(request.open.into_iter().map(PathBuf::from));
    (!files.is_empty()).then_some(files)
}

fn write_handoff_file(port: u16, token: &str) -> Result<(), String> {
    let path = gui_handoff_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string(&HandoffFile {
        pid: std::process::id(),
        port,
        token: token.to_string(),
    })
    .map_err(|e| e.to_string())?;
    // Holds the token, so owner-only from the start.
    let _ = std::fs::remove_file(&path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|e| e.to_string())?;
    file.write_all(json.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wave-open-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn audio_files_on_the_command_line_open_in_the_app() {
        let dir = temp_dir();
        let song = dir.join("song.mp3");
        let other = dir.join("b side.flac");
        std::fs::write(&song, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        let args = vec![
            "wave".to_string(),
            song.to_string_lossy().into_owned(),
            url::Url::from_file_path(&other).unwrap().to_string(),
        ];
        let files = audio_files_in_args(&args).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("song.mp3"));
        assert!(files[1].ends_with("b side.flac"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn anything_else_is_left_to_the_cli() {
        let dir = temp_dir();
        let song = dir.join("song.mp3");
        let notes = dir.join("notes.txt");
        std::fs::write(&song, b"x").unwrap();
        std::fs::write(&notes, b"x").unwrap();
        let song = song.to_string_lossy().into_owned();
        let arg = |a: &str| vec!["wave".to_string(), a.to_string()];
        assert!(audio_files_in_args(&["wave".to_string()]).is_none());
        assert!(audio_files_in_args(&arg("tracks")).is_none());
        assert!(audio_files_in_args(&arg("--help")).is_none());
        assert!(audio_files_in_args(&arg(&notes.to_string_lossy())).is_none());
        assert!(audio_files_in_args(&arg("/no/such/file.mp3")).is_none());
        assert!(audio_files_in_args(&["wave".into(), song, "tracks".into()]).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
