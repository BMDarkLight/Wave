// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Installing a downloaded update and opening release pages, through the
//! Java `AppUpdater` class.

#![cfg(target_os = "android")]

use std::path::Path;

use jni::objects::JValue;

use crate::android::media_bridge::with_activity_class;

const CLASS: &str = "app.bmdarklight.wave.AppUpdater";

/// Open Android's installer on the APK at `path`.
pub fn install(path: &Path) -> Result<(), String> {
    let path = path.to_string_lossy();
    with_activity_class(CLASS, |env, class, activity| {
        let path = env.new_string(path.as_ref())?;
        env.call_static_method(
            class,
            "install",
            "(Landroid/app/Activity;Ljava/lang/String;)V",
            &[JValue::Object(activity), JValue::Object(&path)],
        )?;
        Ok(())
    })
    .map_err(|e| format!("Could not open the installer: {e}"))
}

/// Open `url` in the browser.
pub fn open_url(url: &str) -> Result<(), String> {
    with_activity_class(CLASS, |env, class, activity| {
        let url = env.new_string(url)?;
        env.call_static_method(
            class,
            "openUrl",
            "(Landroid/app/Activity;Ljava/lang/String;)V",
            &[JValue::Object(activity), JValue::Object(&url)],
        )?;
        Ok(())
    })
    .map_err(|e| format!("Could not open the browser: {e}"))
}
