// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The app update's notifications, through the Java `UpdateNotifier` class:
//! download progress, then "ready to install", which installs when tapped.
//!
//! Calls go through the Application context, so they work while Wave plays
//! with no window open. A failed call is logged and otherwise ignored:
//! Settings shows the same state.

use jni::objects::{JClass, JObject, JValue};
use jni::JNIEnv;
use std::mem::ManuallyDrop;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CLASS: &str = "app.bmdarklight.wave.UpdateNotifier";
/// Android drops notification updates an app posts faster than a few a
/// second, so progress is posted at most this often.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// Runs `call` with Wave's Java class `class_name` and the Application context,
/// logging a failure as `what`.
pub(crate) fn with_class(
    class_name: &str,
    what: &str,
    call: impl FnOnce(&mut JNIEnv, &JClass, &JObject) -> jni::errors::Result<()>,
) {
    let result = (|| -> Result<(), String> {
        if !crate::android::jni::android_audio_ready() {
            return Err("ndk_context not seeded yet".into());
        }
        let ctx = std::panic::catch_unwind(ndk_context::android_context)
            .map_err(|_| "ndk_context not available".to_string())?;
        // The process VM, which must never be destroyed from here.
        let vm = ManuallyDrop::new(
            unsafe { jni::JavaVM::from_raw(ctx.vm() as *mut _) }.map_err(|e| e.to_string())?,
        );
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let context = unsafe { JObject::from_raw(ctx.context() as jni::sys::jobject) };
        let outcome = env
            .with_local_frame(8, |env| -> jni::errors::Result<Result<(), String>> {
                let class =
                    match crate::android::media_bridge::load_app_class(env, &context, class_name) {
                        Ok(class) => class,
                        Err(e) => return Ok(Err(e)),
                    };
                Ok(call(env, &class, &context).map_err(|e| e.to_string()))
            })
            .map_err(|e| e.to_string())
            .and_then(|result| result);
        if outcome.is_err() && env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        outcome
    })();
    if let Err(e) = result {
        tracing::debug!("Java call {class_name} ({what}) failed: {e}");
    }
}

/// When progress was last posted, and at what percent.
static LAST_PROGRESS: Mutex<Option<(Instant, u64)>> = Mutex::new(None);

/// Shows the download of `version` at `received` of `total` bytes.
pub fn progress(version: &str, received: u64, total: u64) {
    let percent = (received * 100).checked_div(total).unwrap_or(0);
    {
        let mut last = LAST_PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
        let finished = total > 0 && received >= total;
        if let Some((at, shown)) = *last {
            if !finished && (at.elapsed() < PROGRESS_EVERY || shown == percent) && received > 0 {
                return;
            }
        }
        *last = Some((Instant::now(), percent));
    }
    with_class(CLASS, "progress", |env, class, context| {
        let version = env.new_string(version)?;
        env.call_static_method(
            class,
            "progress",
            "(Landroid/content/Context;Ljava/lang/String;JJ)V",
            &[
                JValue::Object(context),
                JValue::Object(&version),
                JValue::Long(i64::try_from(received).unwrap_or(i64::MAX)),
                JValue::Long(i64::try_from(total).unwrap_or(i64::MAX)),
            ],
        )?;
        Ok(())
    });
}

/// Replaces the progress with "ready to install"; tapping it installs `apk`.
pub fn ready(version: &str, apk: &Path) {
    *LAST_PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let apk = apk.to_string_lossy();
    with_class(CLASS, "ready", |env, class, context| {
        let version = env.new_string(version)?;
        let apk = env.new_string(apk.as_ref())?;
        env.call_static_method(
            class,
            "ready",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
            &[
                JValue::Object(context),
                JValue::Object(&version),
                JValue::Object(&apk),
            ],
        )?;
        Ok(())
    });
}

/// Replaces the progress with why the download failed.
pub fn failed(version: &str, reason: &str) {
    *LAST_PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    with_class(CLASS, "failed", |env, class, context| {
        let version = env.new_string(version)?;
        let reason = env.new_string(reason)?;
        env.call_static_method(
            class,
            "failed",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
            &[
                JValue::Object(context),
                JValue::Object(&version),
                JValue::Object(&reason),
            ],
        )?;
        Ok(())
    });
}

/// Whether Wave may post notifications. False when it can't tell, so the
/// in-app announcement stays as the fallback.
pub fn enabled() -> bool {
    let mut answer = false;
    with_class(CLASS, "enabled", |env, class, context| {
        answer = env
            .call_static_method(
                class,
                "enabled",
                "(Landroid/content/Context;)Z",
                &[JValue::Object(context)],
            )?
            .z()?;
        Ok(())
    });
    answer
}

/// Takes down any update notification, once there is nothing to install.
pub fn cancel() {
    with_class(CLASS, "cancel", |env, class, context| {
        env.call_static_method(
            class,
            "cancel",
            "(Landroid/content/Context;)V",
            &[JValue::Object(context)],
        )?;
        Ok(())
    });
}
