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
use jni::JNIEnv;

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
    crate::android::media_bridge::with_activity_class(CLASS, f)
        .map_err(|e| tracing::warn!("SystemTheme call failed: {e}"))
        .ok()
}
