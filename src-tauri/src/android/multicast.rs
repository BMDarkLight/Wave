// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The Wi-Fi multicast lock library sharing holds, through the Java
//! `WifiMulticast` class. Without it Android drops the searches other devices
//! send, and the shared library could only be added by its address.

use jni::objects::{JObject, JValue};
use jni::JavaVM;
use std::mem::ManuallyDrop;

const CLASS: &str = "app.bmdarklight.wave.WifiMulticast";

pub fn acquire() {
    if let Err(e) = call(true) {
        tracing::warn!("Couldn't hold the Wi-Fi multicast lock: {e}");
    }
}

pub fn release() {
    if let Err(e) = call(false) {
        tracing::debug!("Couldn't release the Wi-Fi multicast lock: {e}");
    }
}

fn call(acquire: bool) -> Result<(), String> {
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
        .with_local_frame(8, |env| -> jni::errors::Result<Result<(), String>> {
            let class = match crate::android::media_bridge::load_app_class(env, &context, CLASS) {
                Ok(class) => class,
                Err(e) => return Ok(Err(e)),
            };
            let outcome = if acquire {
                env.call_static_method(
                    &class,
                    "acquire",
                    "(Landroid/content/Context;)Z",
                    &[JValue::Object(&context)],
                )
                .and_then(|value| value.z())
                .map_err(|e| e.to_string())
                .and_then(|held| {
                    if held {
                        Ok(())
                    } else {
                        Err("not held".to_string())
                    }
                })
            } else {
                env.call_static_method(&class, "release", "()V", &[])
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            };
            Ok(outcome)
        })
        .map_err(|e| e.to_string())
        .and_then(|result| result);
    if result.is_err() && env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result
}
