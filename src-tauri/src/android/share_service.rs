// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Keeping library sharing alive with no window, through the Java
//! `ShareService` and `SafFile` classes: a foreground service holds the
//! process, and SAF documents open through the Application context, which
//! outlives the Activity.

use crate::android::update_notice::with_class;
use jni::objects::JValue;
use std::fs::File;
use std::os::fd::FromRawFd;

const SERVICE: &str = "app.bmdarklight.wave.ShareService";
const SAF_FILE: &str = "app.bmdarklight.wave.SafFile";

/// Starts the foreground service, or updates the name it shows.
pub fn start(name: &str) {
    with_class(SERVICE, "start", |env, class, context| {
        let name = env.new_string(name)?;
        env.call_static_method(
            class,
            "start",
            "(Landroid/content/Context;Ljava/lang/String;)V",
            &[JValue::Object(context), JValue::Object(&name)],
        )?;
        Ok(())
    });
}

pub fn stop() {
    with_class(SERVICE, "stop", |env, class, context| {
        env.call_static_method(
            class,
            "stop",
            "(Landroid/content/Context;)V",
            &[JValue::Object(context)],
        )?;
        Ok(())
    });
}

/// A SAF document opened for reading, with or without a window.
pub fn open_document(uri: &str) -> std::io::Result<File> {
    let mut fd = -1;
    with_class(SAF_FILE, "openForReading", |env, class, context| {
        let uri = env.new_string(uri)?;
        fd = env
            .call_static_method(
                class,
                "openForReading",
                "(Landroid/content/Context;Ljava/lang/String;)I",
                &[JValue::Object(context), JValue::Object(&uri)],
            )?
            .i()?;
        Ok(())
    });
    if fd < 0 {
        return Err(std::io::Error::other(format!("Couldn't open {uri}")));
    }
    // SAFETY: the descriptor was detached from its ParcelFileDescriptor, so
    // nothing else owns or closes it.
    Ok(unsafe { File::from_raw_fd(fd) })
}
