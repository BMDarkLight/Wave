// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The phone's light or dark setting and the system bar icons, through the
//! Java `SystemTheme` class.

#![cfg(target_os = "android")]

use jni::objects::{JClass, JObject, JValue};
use jni::{JNIEnv, JavaVM};

const CLASS: &str = "app.bmdarklight.wave.SystemTheme";

/// Whether the phone is in dark mode. `None` while the app has no Activity.
pub fn is_dark() -> Option<bool> {
    with_class(|env, class, activity| {
        env.call_static_method(
            class,
            "isDark",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(activity)],
        )?
        .z()
    })
}

/// Dark status and navigation bar icons for a light page, light ones for a
/// dark page.
pub fn set_light_bars(light: bool) {
    with_class(|env, class, activity| {
        env.call_static_method(
            class,
            "setLightBars",
            "(Landroid/app/Activity;Z)V",
            &[JValue::Object(activity), JValue::Bool(light.into())],
        )?;
        Ok(())
    });
}

fn with_class<T>(
    f: impl for<'local> FnOnce(
        &mut JNIEnv<'local>,
        &JClass<'local>,
        &JObject<'local>,
    ) -> jni::errors::Result<T>,
) -> Option<T> {
    let (vm_ptr, activity) = crate::android::jni::live_activity()?;
    let vm = unsafe { JavaVM::from_raw(vm_ptr as *mut _) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { JObject::from_raw(activity as jni::sys::jobject) };
    // Command threads can stay attached, so free the call's local refs here.
    let result = env.with_local_frame(8, |env| -> Result<T, CallError> {
        let class = crate::android::media_bridge::load_app_class(env, &activity, CLASS)
            .map_err(CallError::Load)?;
        Ok(f(env, &class, &activity)?)
    });
    match result {
        Ok(value) => Some(value),
        Err(e) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            tracing::warn!("SystemTheme call failed: {e}");
            None
        }
    }
}

enum CallError {
    Load(String),
    Jni(jni::errors::Error),
}

impl From<jni::errors::Error> for CallError {
    fn from(e: jni::errors::Error) -> Self {
        CallError::Jni(e)
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Load(e) => f.write_str(e),
            CallError::Jni(e) => e.fmt(f),
        }
    }
}
