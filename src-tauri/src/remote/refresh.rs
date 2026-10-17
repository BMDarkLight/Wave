// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Throttles manual remote catalog refreshes.

use parking_lot::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Inner {
    started: Option<Instant>,
    running: bool,
}

pub struct RefreshCooldown {
    inner: Mutex<Inner>,
}

impl RefreshCooldown {
    pub const COOLDOWN: Duration = Duration::from_secs(30);

    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Self::new()
    }

    pub fn global() -> &'static RefreshCooldown {
        static COOLDOWN: OnceLock<RefreshCooldown> = OnceLock::new();
        COOLDOWN.get_or_init(RefreshCooldown::new)
    }

    /// Err carries the whole seconds left, rounded up and never below 1.
    pub fn try_begin(&self, now: Instant) -> Result<(), u32> {
        let mut inner = self.inner.lock();
        if let Some(started) = inner.started {
            let elapsed = now.saturating_duration_since(started);
            if inner.running || elapsed < Self::COOLDOWN {
                let left = Self::COOLDOWN.saturating_sub(elapsed);
                let secs = left.as_secs() + u64::from(left.subsec_nanos() > 0);
                return Err((secs as u32).max(1));
            }
        }
        inner.started = Some(now);
        inner.running = true;
        Ok(())
    }

    pub fn finish(&self) {
        self.inner.lock().running = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn second_refresh_within_30s_is_refused_with_remaining_seconds() {
        let cd = RefreshCooldown::new_for_test();
        let t0 = Instant::now();
        assert_eq!(cd.try_begin(t0), Ok(()));
        cd.finish();
        assert_eq!(cd.try_begin(t0 + Duration::from_secs(10)), Err(20));
    }

    #[test]
    fn refresh_running_blocks_even_after_30s() {
        let cd = RefreshCooldown::new_for_test();
        let t0 = Instant::now();
        assert_eq!(cd.try_begin(t0), Ok(()));
        assert_eq!(cd.try_begin(t0 + Duration::from_secs(40)), Err(1));
    }

    #[test]
    fn allowed_after_cooldown() {
        let cd = RefreshCooldown::new_for_test();
        let t0 = Instant::now();
        assert_eq!(cd.try_begin(t0), Ok(()));
        cd.finish();
        assert_eq!(cd.try_begin(t0 + Duration::from_secs(30)), Ok(()));
    }
}
