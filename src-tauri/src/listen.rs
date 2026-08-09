//! In-memory listen session tracker.
//!
//! Flushes listen seconds / play-skip counts / track-to-track edges into the
//! library DB whenever the current track changes or the app is closing.

use std::time::Instant;

/// Why a listen session is being closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenEndReason {
    /// Track reached the end (or crossfade handoff counted as completion).
    Completed,
    /// User skipped to another track before the listen threshold.
    Skipped,
    /// Pause-away / stop / new selection / app close — credit seconds only.
    Partial,
}

#[derive(Debug, Clone)]
struct ActiveListen {
    /// Path currently reported by the audio engine (tick matching).
    path: String,
    /// Best path for DB lookup (library path when known).
    record_path: String,
    /// Highest playback position observed this session (seconds).
    max_position: f64,
    /// Last observed position — used to ignore backward seeks when accumulating.
    last_position: f64,
    /// Accumulated forward playback progress this session.
    accumulated: f64,
    duration: Option<f64>,
    /// Record path that was playing immediately before this one.
    from_record_path: Option<String>,
    _started_at: Instant,
}

#[derive(Debug, Default)]
pub struct ListenTracker {
    active: Option<ActiveListen>,
}

/// Result ready to persist via `Library::record_listen`.
#[derive(Debug, Clone)]
pub struct ListenFlush {
    pub path: String,
    pub seconds: f64,
    pub completed: bool,
    pub skipped: bool,
    pub from_path: Option<String>,
}

impl ListenTracker {
    pub fn new() -> Self {
        Self { active: None }
    }

    /// Begin (or replace) a listen session for `path`.
    ///
    /// New sessions always start at position 0 — callers must not seed with the
    /// previous track's scrubber position (a common Android race).
    pub fn start(
        &mut self,
        path: String,
        record_path: String,
        duration: Option<f64>,
        from_record_path: Option<String>,
    ) {
        self.active = Some(ActiveListen {
            path,
            record_path,
            max_position: 0.0,
            last_position: 0.0,
            accumulated: 0.0,
            duration,
            from_record_path,
            _started_at: Instant::now(),
        });
    }

    fn paths_match(active: &ActiveListen, player_path: &str) -> bool {
        active.path == player_path || active.record_path == player_path
    }

    /// Observe the current playback position while the track is playing.
    pub fn observe_position(&mut self, position: f64) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        let pos = position.max(0.0);
        if pos > active.last_position {
            let delta = pos - active.last_position;
            // Ignore large jumps (seeks); credit continuous playback only.
            if delta < 5.0 {
                active.accumulated += delta;
            }
        }
        active.last_position = pos;
        if pos > active.max_position {
            active.max_position = pos;
        }
    }

    /// Update known duration once metadata is available.
    pub fn set_duration(&mut self, duration: Option<f64>) {
        if let Some(active) = self.active.as_mut() {
            active.duration = duration.or(active.duration);
        }
    }

    /// Align the engine path used for tick matching (e.g. after materialization).
    pub fn set_player_path(&mut self, player_path: String) {
        if let Some(active) = self.active.as_mut() {
            active.path = player_path;
        }
    }

    pub fn current_path(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.path.as_str())
    }

    pub fn current_record_path(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.record_path.as_str())
    }

    pub fn matches_player_path(&self, player_path: &str) -> bool {
        self.active
            .as_ref()
            .is_some_and(|a| Self::paths_match(a, player_path))
    }

    /// Seconds credited so far in the active session (for race guards).
    pub fn peek_seconds(&self) -> Option<f64> {
        self.active.as_ref().map(|a| {
            if a.accumulated >= 1.0 {
                a.accumulated
            } else {
                a.max_position
            }
        })
    }

    /// End the active session and return a flush payload (if any seconds).
    pub fn end(&mut self, reason: ListenEndReason) -> Option<ListenFlush> {
        let active = self.active.take()?;
        let duration = active.duration.filter(|d| *d > 0.0);
        // Prefer accumulated playback; fall back to max position only when we
        // never got tick deltas (e.g. very short session between ticks).
        let mut seconds = if active.accumulated >= 1.0 {
            active.accumulated
        } else {
            active.max_position
        };
        if let Some(d) = duration {
            seconds = seconds.min(d);
        }
        if seconds < 1.0 && !matches!(reason, ListenEndReason::Completed) {
            // Still emit a tiny flush for Completed natural ends near 0 edge cases.
            return None;
        }
        if seconds < 1.0 {
            seconds = 1.0;
        }

        let listened_enough = match duration {
            Some(d) if d > 0.0 => seconds >= 30.0 || seconds >= d * 0.5,
            _ => seconds >= 30.0,
        };

        let (completed, skipped) = match reason {
            ListenEndReason::Completed => (true, false),
            ListenEndReason::Skipped => {
                if listened_enough {
                    (true, false)
                } else {
                    (false, true)
                }
            }
            ListenEndReason::Partial => (listened_enough, false),
        };

        Some(ListenFlush {
            path: active.record_path,
            seconds,
            completed,
            skipped,
            from_path: active.from_record_path,
        })
    }

    /// Switch to a new track, returning the flush for the previous one.
    ///
    /// `new_path` is used only for tick matching; `record_path` is what gets
    /// written to the DB. Position is always reset — never inherit the previous
    /// track's scrubber value.
    pub fn switch_track(
        &mut self,
        new_path: String,
        record_path: String,
        duration: Option<f64>,
        reason: ListenEndReason,
    ) -> Option<ListenFlush> {
        let from_record = self.active.as_ref().map(|a| a.record_path.clone());
        let same = self
            .active
            .as_ref()
            .is_some_and(|a| a.record_path == record_path);
        let flush = if same {
            self.end(ListenEndReason::Partial)
        } else {
            self.end(reason)
        };
        let from_for_edge = if same { None } else { from_record };
        self.start(new_path, record_path, duration, from_for_edge);
        flush
    }
}
