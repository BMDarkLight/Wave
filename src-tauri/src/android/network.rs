// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Whether the phone is on mobile data, through the Java `NetworkState` class.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The network gate asks before every chunk a fill reads, so one answer
/// serves for a while instead of a JNI round trip each time.
const FRESH_FOR: Duration = Duration::from_secs(2);

type Answer = Mutex<Option<(Instant, bool)>>;

/// True on cellular or another metered network that is not Wi-Fi or
/// Ethernet, and whenever it can't tell.
#[cfg(target_os = "android")]
pub fn is_cellular() -> bool {
    static LAST: Answer = Mutex::new(None);
    cached(&LAST, Instant::now(), probe::is_cellular_or_metered)
}

/// The last answer while it is fresh, else a new one. A probe that fails
/// counts as cellular, so mobile data is never used on a guess.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn cached(last: &Answer, now: Instant, probe: impl FnOnce() -> Result<bool, String>) -> bool {
    if let Some((at, cellular)) = *last.lock().unwrap_or_else(PoisonError::into_inner) {
        if now.saturating_duration_since(at) < FRESH_FOR {
            return cellular;
        }
    }
    let cellular = probe().unwrap_or_else(|e| {
        tracing::debug!("NetworkState call failed, assuming mobile data: {e}");
        true
    });
    *last.lock().unwrap_or_else(PoisonError::into_inner) = Some((now, cellular));
    cellular
}

#[cfg(target_os = "android")]
mod probe {
    use jni::objects::{JObject, JValue};
    use jni::JavaVM;
    use std::mem::ManuallyDrop;

    const CLASS: &str = "app.bmdarklight.wave.NetworkState";

    /// Asks through the Application context rather than the Activity, since
    /// playback and its downloads go on with no window open.
    pub(super) fn is_cellular_or_metered() -> Result<bool, String> {
        if !crate::android::jni::android_audio_ready() {
            return Err("ndk_context not seeded yet".into());
        }
        let ctx = std::panic::catch_unwind(ndk_context::android_context)
            .map_err(|_| "ndk_context not available".to_string())?;
        // The process VM, which must never be destroyed from here.
        let vm = ManuallyDrop::new(
            unsafe { JavaVM::from_raw(ctx.vm() as *mut _) }.map_err(|e| e.to_string())?,
        );
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let context = unsafe { JObject::from_raw(ctx.context() as jni::sys::jobject) };
        let result = env
            .with_local_frame(8, |env| -> jni::errors::Result<Result<bool, String>> {
                let class = match crate::android::media_bridge::load_app_class(env, &context, CLASS)
                {
                    Ok(class) => class,
                    Err(e) => return Ok(Err(e)),
                };
                Ok(env
                    .call_static_method(
                        &class,
                        "isCellularOrMetered",
                        "(Landroid/content/Context;)Z",
                        &[JValue::Object(&context)],
                    )
                    .and_then(|value| value.z())
                    .map_err(|e| e.to_string()))
            })
            .map_err(|e| e.to_string())
            .and_then(|result| result);
        if result.is_err() && env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_failed_probe_counts_as_cellular() {
        let last = Answer::default();
        assert!(cached(&last, Instant::now(), || Err("no context".into())));
    }

    #[test]
    fn an_answer_is_reused_while_fresh_then_asked_again() {
        let last = Answer::default();
        let asked = Cell::new(0);
        let wifi = || {
            asked.set(asked.get() + 1);
            Ok(false)
        };
        let start = Instant::now();
        assert!(!cached(&last, start, wifi));
        assert!(!cached(&last, start + Duration::from_millis(1900), wifi));
        assert_eq!(asked.get(), 1);

        assert!(cached(&last, start + FRESH_FOR, || Ok(true)));
        let failure_at = start + FRESH_FOR * 2;
        assert!(cached(&last, failure_at, || Err("threw".into())));
        // A failure is held as long as any other answer.
        assert!(cached(&last, failure_at + Duration::from_secs(1), wifi));
        assert_eq!(asked.get(), 1);
        assert!(!cached(&last, failure_at + FRESH_FOR, wifi));
        assert_eq!(asked.get(), 2);
    }
}
