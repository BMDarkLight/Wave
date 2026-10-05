// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! `wave now --json --watch`: playback changes as a stream of JSON lines.
//!
//! One line per event, flushed as it happens, so another program can follow
//! playback without polling. The event shapes are documented in docs/cli.md.

use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::cli::{json, ui};
use crate::playback_daemon::{daemon_request_if_running, DaemonRequest, PlaybackStatus};

/// How far the position may stray from where steady playback would have
/// taken it before the difference counts as a seek. Polling jitter stays
/// well under this.
const SEEK_TOLERANCE_SECONDS: f64 = 1.5;

/// Failed requests in a row before a daemon that still looks alive is given
/// up on. A single slow reply is not a reason to end the stream.
const MAX_FAILED_POLLS: u32 = 5;

/// Whether `next` differs from `last`, the status most recently reported,
/// in anything but the position steady playback would have moved on to.
/// `elapsed` is the time between the two readings.
pub fn status_changed(last: &PlaybackStatus, next: &PlaybackStatus, elapsed: Duration) -> bool {
    let same = |s: &PlaybackStatus| {
        (
            s.state.clone(),
            s.path.clone(),
            s.duration_seconds.map(f64::to_bits),
            s.volume.to_bits(),
            s.device.clone(),
            s.repeat.clone(),
            s.shuffle,
            s.queue_position,
            s.queue_length,
            s.title.clone(),
            s.artist.clone(),
        )
    };
    if same(last) != same(next) {
        return true;
    }
    let expected = if last.state == "playing" {
        last.position_seconds + elapsed.as_secs_f64()
    } else {
        last.position_seconds
    };
    (next.position_seconds - expected).abs() > SEEK_TOLERANCE_SECONDS
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn event(kind: &str, fields: Value) -> Value {
    let mut line = json!({
        "event": kind,
        "at": now_ms(),
        "schema_version": json::SCHEMA_VERSION,
    });
    if let (Some(line), Value::Object(fields)) = (line.as_object_mut(), fields) {
        line.extend(fields);
    }
    line
}

/// Write one event line. A reader that has gone away ends the stream
/// quietly: nobody is left to report an error to.
fn emit(line: Value) {
    let mut stdout = std::io::stdout().lock();
    if writeln!(stdout, "{line}")
        .and_then(|_| stdout.flush())
        .is_err()
    {
        std::process::exit(crate::cli::EXIT_BROKEN_PIPE);
    }
}

/// One reading of the daemon: its status and the queue. `None` once the
/// daemon is gone.
fn poll() -> Result<Option<(PlaybackStatus, Vec<String>)>, String> {
    match daemon_request_if_running(DaemonRequest::QueueList)? {
        None => Ok(None),
        Some(response) => match response.status {
            Some(status) => Ok(Some((status, response.queue.unwrap_or_default()))),
            None => Err(response
                .error
                .unwrap_or_else(|| "The daemon returned no status.".to_string())),
        },
    }
}

pub fn run(interval: f64) -> ! {
    if !ui::current().json {
        ui::fail(
            "--watch prints JSON lines, so it needs --json.",
            Some("wave now --json --watch"),
            ui::EXIT_USAGE,
        );
    }
    let interval = Duration::from_secs_f64(crate::cli::now::clamp_interval(interval));

    let (mut last, mut tracks) = match poll() {
        Ok(Some(reading)) => reading,
        Ok(None) => ui::no_daemon(),
        Err(e) => ui::fail_with(e),
    };
    emit(event("status", json!({ "status": last })));
    emit(event(
        "queue",
        json!({ "queue_position": last.queue_position, "tracks": tracks }),
    ));

    let mut last_at = std::time::Instant::now();
    let mut failures = 0;
    loop {
        std::thread::sleep(interval);
        let (status, queue) = match poll() {
            Ok(Some(reading)) => {
                failures = 0;
                reading
            }
            Ok(None) => break,
            Err(_) => {
                failures += 1;
                if failures >= MAX_FAILED_POLLS {
                    break;
                }
                continue;
            }
        };
        if queue != tracks {
            emit(event(
                "queue",
                json!({ "queue_position": status.queue_position, "tracks": queue }),
            ));
            tracks = queue;
        }
        if status_changed(&last, &status, last_at.elapsed()) {
            emit(event("status", json!({ "status": status })));
            last = status;
            last_at = std::time::Instant::now();
        }
    }
    emit(event("daemon_stopped", json!({})));
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> PlaybackStatus {
        PlaybackStatus {
            state: "playing".into(),
            file: Some("a.flac".into()),
            path: Some("/music/a.flac".into()),
            position_seconds: 10.0,
            duration_seconds: Some(200.0),
            volume: 0.5,
            repeat: "off".into(),
            queue_position: Some(0),
            queue_length: 3,
            ..Default::default()
        }
    }

    #[test]
    fn steady_playback_is_not_a_change() {
        let mut next = playing();
        next.position_seconds = 10.5;
        assert!(!status_changed(
            &playing(),
            &next,
            Duration::from_millis(500)
        ));
        // Long after the last event, the position is still where steady
        // playback would put it.
        next.position_seconds = 70.0;
        assert!(!status_changed(&playing(), &next, Duration::from_secs(60)));
    }

    #[test]
    fn a_seek_is_a_change() {
        let mut next = playing();
        next.position_seconds = 90.0;
        assert!(status_changed(
            &playing(),
            &next,
            Duration::from_millis(500)
        ));
        next.position_seconds = 0.0;
        assert!(status_changed(
            &playing(),
            &next,
            Duration::from_millis(500)
        ));
    }

    #[test]
    fn a_paused_track_should_not_move() {
        let mut last = playing();
        last.state = "paused".into();
        let mut next = last.clone();
        assert!(!status_changed(&last, &next, Duration::from_secs(30)));
        next.position_seconds = 40.0;
        assert!(status_changed(&last, &next, Duration::from_secs(30)));
    }

    #[test]
    fn state_track_and_settings_are_changes() {
        let base = playing();
        let tick = Duration::from_millis(500);
        let changed = |edit: &dyn Fn(&mut PlaybackStatus)| {
            let mut next = base.clone();
            next.position_seconds += 0.5;
            edit(&mut next);
            status_changed(&base, &next, tick)
        };
        assert!(changed(&|s| s.state = "paused".into()));
        assert!(changed(&|s| s.path = Some("/music/b.flac".into())));
        assert!(changed(&|s| s.volume = 0.6));
        assert!(changed(&|s| s.repeat = "all".into()));
        assert!(changed(&|s| s.shuffle = true));
        assert!(changed(&|s| s.queue_position = Some(1)));
        assert!(changed(&|s| s.queue_length = 4));
        assert!(!changed(&|_| {}));
    }
}
