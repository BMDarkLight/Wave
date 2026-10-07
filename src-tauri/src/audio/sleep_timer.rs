// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Sleep timer: pause playback after a set time or when the current track ends.
//!
//! The timer itself only does arithmetic. The player and the two playback
//! loops (GUI and daemon) ask it what to do on each tick and act on the answer.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Volume ramps down over this long before the timer pauses playback.
pub const SLEEP_FADE: Duration = Duration::from_secs(10);

/// Longest countdown accepted, so a typo cannot arm a timer for weeks.
pub const MAX_SLEEP: Duration = Duration::from_secs(24 * 60 * 60);

/// What a caller asks the timer to do.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SleepRequest {
    Off,
    Countdown { seconds: u64 },
    EndOfTrack,
}

/// What the timer is doing, as shown to the UI and in `playback status` JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SleepTimerStatus {
    /// `off`, `countdown` or `end_of_track`.
    pub mode: String,
    /// Seconds left on a countdown; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_seconds: Option<f64>,
}

impl Default for SleepTimerStatus {
    fn default() -> Self {
        Self {
            mode: "off".to_string(),
            remaining_seconds: None,
        }
    }
}

/// What the player should do on this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SleepStep {
    /// Timer is off; leave the volume alone.
    Idle,
    /// Play at this fraction of the user's volume (1.0 outside the fade).
    Level(f32),
    /// The countdown ran out: pause now.
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Mode {
    #[default]
    Off,
    Countdown {
        deadline: Instant,
    },
    EndOfTrack,
}

#[derive(Debug, Clone, Default)]
pub struct SleepTimer {
    mode: Mode,
}

impl SleepTimer {
    pub fn set(&mut self, request: SleepRequest, now: Instant) -> Result<(), String> {
        self.mode = match request {
            SleepRequest::Off => Mode::Off,
            SleepRequest::EndOfTrack => Mode::EndOfTrack,
            SleepRequest::Countdown { seconds } => {
                let length = Duration::from_secs(seconds);
                if length.is_zero() || length > MAX_SLEEP {
                    return Err("Sleep timer must be between 1 second and 24 hours".to_string());
                }
                Mode::Countdown {
                    deadline: now + length,
                }
            }
        };
        Ok(())
    }

    pub fn cancel(&mut self) {
        self.mode = Mode::Off;
    }

    pub fn is_end_of_track(&self) -> bool {
        self.mode == Mode::EndOfTrack
    }

    pub fn status(&self, now: Instant) -> SleepTimerStatus {
        match self.mode {
            Mode::Off => SleepTimerStatus {
                mode: "off".to_string(),
                remaining_seconds: None,
            },
            Mode::EndOfTrack => SleepTimerStatus {
                mode: "end_of_track".to_string(),
                remaining_seconds: None,
            },
            Mode::Countdown { deadline } => SleepTimerStatus {
                mode: "countdown".to_string(),
                remaining_seconds: Some(deadline.saturating_duration_since(now).as_secs_f64()),
            },
        }
    }

    /// `track_left` is how much of the current track remains, when known. It
    /// drives the fade in end-of-track mode.
    pub fn step(&self, now: Instant, track_left: Option<Duration>) -> SleepStep {
        match self.mode {
            Mode::Off => SleepStep::Idle,
            Mode::EndOfTrack => SleepStep::Level(track_left.map_or(1.0, fade_level)),
            Mode::Countdown { deadline } => {
                let left = deadline.saturating_duration_since(now);
                if left.is_zero() {
                    SleepStep::Expired
                } else {
                    SleepStep::Level(fade_level(left))
                }
            }
        }
    }
}

/// The choices the tray menus offer, as (menu id suffix, label).
#[cfg(not(target_os = "android"))]
pub const SLEEP_MENU: [(&str, &str); 7] = [
    ("15", "15 minutes"),
    ("30", "30 minutes"),
    ("45", "45 minutes"),
    ("60", "1 hour"),
    ("90", "90 minutes"),
    ("end", "End of track"),
    ("off", "Off"),
];

/// Turn a [`SLEEP_MENU`] id suffix back into a request.
#[cfg(not(target_os = "android"))]
pub fn request_for_menu_choice(choice: &str) -> Option<SleepRequest> {
    match choice {
        "end" => Some(SleepRequest::EndOfTrack),
        "off" => Some(SleepRequest::Off),
        minutes => minutes
            .parse::<u64>()
            .ok()
            .map(|minutes| SleepRequest::Countdown {
                seconds: minutes * 60,
            }),
    }
}

fn fade_level(left: Duration) -> f32 {
    (left.as_secs_f32() / SLEEP_FADE.as_secs_f32()).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn countdown(seconds: u64) -> SleepRequest {
        SleepRequest::Countdown { seconds }
    }

    #[test]
    fn off_by_default() {
        let timer = SleepTimer::default();
        let now = Instant::now();
        assert_eq!(timer.step(now, None), SleepStep::Idle);
        assert_eq!(timer.status(now).mode, "off");
    }

    #[test]
    fn countdown_plays_at_full_level_before_the_fade() {
        let now = Instant::now();
        let mut timer = SleepTimer::default();
        timer.set(countdown(60), now).unwrap();
        assert_eq!(
            timer.step(now + Duration::from_secs(30), None),
            SleepStep::Level(1.0)
        );
    }

    #[test]
    fn countdown_fades_over_the_last_ten_seconds() {
        let now = Instant::now();
        let mut timer = SleepTimer::default();
        timer.set(countdown(60), now).unwrap();
        let SleepStep::Level(level) = timer.step(now + Duration::from_secs(55), None) else {
            panic!("expected a fade level");
        };
        assert!((level - 0.5).abs() < 1e-3);
    }

    #[test]
    fn countdown_expires_at_the_deadline() {
        let now = Instant::now();
        let mut timer = SleepTimer::default();
        timer.set(countdown(60), now).unwrap();
        assert_eq!(
            timer.step(now + Duration::from_secs(60), None),
            SleepStep::Expired
        );
        assert_eq!(
            timer.step(now + Duration::from_secs(90), None),
            SleepStep::Expired
        );
    }

    #[test]
    fn countdown_reports_remaining_seconds() {
        let now = Instant::now();
        let mut timer = SleepTimer::default();
        timer.set(countdown(600), now).unwrap();
        let status = timer.status(now + Duration::from_secs(100));
        assert_eq!(status.mode, "countdown");
        assert_eq!(status.remaining_seconds, Some(500.0));
    }

    #[test]
    fn countdown_rejects_zero_and_over_a_day() {
        let now = Instant::now();
        let mut timer = SleepTimer::default();
        assert!(timer.set(countdown(0), now).is_err());
        assert!(timer.set(countdown(MAX_SLEEP.as_secs() + 1), now).is_err());
        assert_eq!(timer.status(now).mode, "off");
    }

    #[test]
    fn end_of_track_fades_with_the_track_and_never_expires() {
        let now = Instant::now();
        let mut timer = SleepTimer::default();
        timer.set(SleepRequest::EndOfTrack, now).unwrap();
        assert!(timer.is_end_of_track());
        assert_eq!(
            timer.step(now, Some(Duration::from_secs(120))),
            SleepStep::Level(1.0)
        );
        assert_eq!(
            timer.step(now, Some(Duration::from_secs(2))),
            SleepStep::Level(0.2)
        );
        assert_eq!(timer.step(now, Some(Duration::ZERO)), SleepStep::Level(0.0));
        assert_eq!(timer.step(now, None), SleepStep::Level(1.0));
    }

    #[test]
    fn cancel_turns_it_off() {
        let now = Instant::now();
        let mut timer = SleepTimer::default();
        timer.set(SleepRequest::EndOfTrack, now).unwrap();
        timer.cancel();
        assert!(!timer.is_end_of_track());
        assert_eq!(timer.step(now, None), SleepStep::Idle);
    }

    #[test]
    fn every_menu_choice_maps_to_a_request() {
        for (choice, _) in SLEEP_MENU {
            assert!(request_for_menu_choice(choice).is_some(), "{choice}");
        }
        assert_eq!(request_for_menu_choice("30"), Some(countdown(1800)));
        assert_eq!(
            request_for_menu_choice("end"),
            Some(SleepRequest::EndOfTrack)
        );
        assert_eq!(request_for_menu_choice("nope"), None);
    }

    #[test]
    fn request_round_trips_through_json() {
        let json = serde_json::to_string(&countdown(90)).unwrap();
        assert_eq!(json, r#"{"mode":"countdown","seconds":90}"#);
        let back: SleepRequest = serde_json::from_str(r#"{"mode":"end_of_track"}"#).unwrap();
        assert_eq!(back, SleepRequest::EndOfTrack);
    }
}
