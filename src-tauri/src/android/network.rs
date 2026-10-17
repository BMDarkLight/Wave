// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Whether the phone is on mobile data, through the Java `NetworkState` class.

#![cfg(target_os = "android")]

use jni::objects::{JObject, JValue};
use jni::JavaVM;
use std::mem::ManuallyDrop;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

const CLASS: &str = "app.bmdarklight.wave.NetworkState";
/// The network gate asks before every chunk a fill reads, so one answer
/// serves for a while instead of a JNI round trip each time.
const FRESH_FOR: Duration = Duration::from_secs(2);

static LAST: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

/// True on cellular or another metered network that is not Wi-Fi or
/// Ethernet. False when it can't tell.
pub fn is_cellular() -> bool {
    if let Some((at, cellular)) = *LAST.lock().unwrap_or_else(PoisonError::into_inner) {
        if at.elapsed() < FRESH_FOR {
            return cellular;
        }
    }
    let cellular = probe().unwrap_or_else(|e| {
        tracing::debug!("NetworkState call failed: {e}");
        false
    });
    *LAST.lock().unwrap_or_else(PoisonError::into_inner) = Some((Instant::now(), cellular));
    cellular
}

/// Asks through the Application context rather than the Activity, since
/// playback and its downloads go on with no window open.
fn probe() -> Result<bool, String> {
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
            let class = match crate::android::media_bridge::load_app_class(env, &context, CLASS) {
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
