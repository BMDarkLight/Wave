// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Android native media bridge, both directions.
//!
//! Inbound: the media session, notification, Bluetooth and the player itself
//! dispatch play/pause/next/... into `MediaNativeBridge.dispatch`, which calls
//! this module over JNI. Actions are applied on a dedicated worker thread so
//! transport stays responsive when the WebView is frozen in the background.
//!
//! Outbound: [`session_update`], [`session_timeline`] and [`session_clear`]
//! push now-playing state into the session through static Java methods. They
//! deliberately avoid Tauri plugin commands, which dispatch through the
//! Activity and panic once it has been destroyed, the very case background
//! playback has to survive.

#![cfg(target_os = "android")]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM, NativeMethod};
use tauri::AppHandle;

use crate::android::jni as android_jni;
use crate::commands;

struct PendingQueue {
    actions: VecDeque<(String, Instant)>,
}

static PENDING: Mutex<PendingQueue> = Mutex::new(PendingQueue {
    actions: VecDeque::new(),
});
static PENDING_CV: Condvar = Condvar::new();
static NATIVES_READY: AtomicBool = AtomicBool::new(false);
static WORKER_STARTED: AtomicBool = AtomicBool::new(false);
/// `MediaNativeBridge`, loaded once through the app class loader. Worker
/// threads cannot find app classes with `FindClass`, so it is cached.
static BRIDGE_CLASS: OnceLock<GlobalRef> = OnceLock::new();

/// Queue a media action from the JNI callback (any thread) and wake the worker.
pub fn push_action(action: String) {
    let action = action.trim().to_string();
    if action.is_empty() {
        return;
    }
    if let Ok(mut queue) = PENDING.lock() {
        let now = Instant::now();
        // Coalesce only identical transport presses within 50ms (not toggles).
        if let Some((last, at)) = queue.actions.back() {
            if last == &action
                && now.duration_since(*at) < Duration::from_millis(50)
                && matches!(action.as_str(), "play" | "pause" | "stop")
            {
                return;
            }
        }
        queue.actions.push_back((action, now));
        PENDING_CV.notify_one();
    }
}

/// Apply any pending native media actions (best-effort drain; worker is primary).
pub fn drain_actions(app: &AppHandle) {
    if !NATIVES_READY.load(Ordering::Acquire) {
        try_install();
    }

    let actions: Vec<String> = match PENDING.lock() {
        Ok(mut queue) => queue.actions.drain(..).map(|(a, _)| a).collect(),
        Err(_) => return,
    };
    for action in actions {
        if let Err(error) = commands::handle_native_media_action(app, &action) {
            tracing::warn!("Android native media action '{action}' failed: {error}");
        }
    }
}

/// Start the dedicated media-action worker (idempotent).
pub fn start_worker(app: AppHandle) {
    if WORKER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::Builder::new()
        .name("wave-android-media".into())
        .spawn(move || loop {
            let action = {
                let mut queue = match PENDING.lock() {
                    Ok(q) => q,
                    Err(_) => break,
                };
                while queue.actions.is_empty() {
                    queue = match PENDING_CV.wait(queue) {
                        Ok(q) => q,
                        Err(_) => return,
                    };
                }
                queue.actions.pop_front().map(|(a, _)| a)
            };
            let Some(action) = action else {
                continue;
            };
            if !NATIVES_READY.load(Ordering::Acquire) {
                try_install();
            }
            // Keep the worker alive through a panicking action: it is the
            // only way notification and Bluetooth controls reach the engine.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                commands::handle_native_media_action(&app, &action)
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!("Android native media action '{action}' failed: {error}")
                }
                Err(_) => tracing::error!("Android native media action '{action}' panicked"),
            }
        })
        .ok();
}

/// Register JNI natives for [`MediaNativeBridge`] (best-effort; retried from tick).
pub fn install(app: &AppHandle) {
    try_install();
    start_worker(app.clone());
}

fn try_install() {
    if NATIVES_READY.load(Ordering::Acquire) {
        return;
    }

    android_jni::ensure_jni_thread_attached();

    let ctx = match std::panic::catch_unwind(ndk_context::android_context) {
        Ok(ctx) => ctx,
        Err(_) => return,
    };

    let vm = match unsafe { JavaVM::from_raw(ctx.vm() as *mut _) } {
        Ok(vm) => vm,
        Err(_) => return,
    };

    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };

    let activity = unsafe { JObject::from_raw(ctx.context() as *mut _) };
    if activity.is_null() {
        return;
    }

    let class = match load_app_class(
        &mut env,
        &activity,
        "app.bmdarklight.wave.MediaNativeBridge",
    ) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("MediaNativeBridge class load failed: {e}");
            return;
        }
    };

    let natives = [NativeMethod {
        name: "nativeOnMediaAction".into(),
        sig: "(Ljava/lang/String;)V".into(),
        fn_ptr: native_on_media_action as *mut std::ffi::c_void,
    }];

    if let Err(e) = env.register_native_methods(&class, &natives) {
        tracing::warn!("MediaNativeBridge RegisterNatives failed: {e}");
        return;
    }

    if BRIDGE_CLASS.get().is_none() {
        match env.new_global_ref(&class) {
            Ok(global) => {
                let _ = BRIDGE_CLASS.set(global);
            }
            Err(e) => tracing::warn!("MediaNativeBridge global ref failed: {e}"),
        }
    }
    // The session needs a context before the first update; CrashInitProvider
    // normally provides it at process start, this covers anything that skips it.
    if env
        .call_static_method(
            &class,
            "attach",
            "(Landroid/content/Context;)V",
            &[JValue::Object(&activity)],
        )
        .is_err()
    {
        clear_exception(&mut env);
    }

    NATIVES_READY.store(true, Ordering::Release);
    tracing::info!("MediaNativeBridge natives registered");
}

/// Apply a state change to the media session. `json` uses the field names of
/// the Kotlin `MediaUpdate`; missing fields keep their previous value.
pub fn session_update(json: &str) {
    call_bridge(|env, class| {
        let json = env.new_string(json)?;
        env.call_static_method(
            class,
            "updateSession",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&json)],
        )?;
        Ok(())
    });
}

/// Timeline-only update. `None` leaves a value unchanged.
pub fn session_timeline(position: Option<f64>, duration: Option<f64>, speed: Option<f64>) {
    call_bridge(|env, class| {
        env.call_static_method(
            class,
            "updateTimeline",
            "(DDD)V",
            &[
                JValue::Double(position.unwrap_or(f64::NAN)),
                JValue::Double(duration.unwrap_or(f64::NAN)),
                JValue::Double(speed.unwrap_or(f64::NAN)),
            ],
        )?;
        Ok(())
    });
}

/// Remove the notification and stop the service right now, from this thread.
/// Used just before the process exits, when a main-thread post would never run
/// and a detached notification would be left behind with dead buttons.
pub fn session_shutdown_now() {
    call_bridge(|env, class| {
        env.call_static_method(class, "shutdownSessionNow", "()V", &[])?;
        Ok(())
    });
}

/// Take down the session's notification and foreground service.
pub fn session_clear() {
    call_bridge(|env, class| {
        env.call_static_method(class, "clearSession", "()V", &[])?;
        Ok(())
    });
}

fn call_bridge<F>(f: F)
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, &JClass<'local>) -> jni::errors::Result<()>,
{
    if BRIDGE_CLASS.get().is_none() {
        try_install();
    }
    let Some(class) = BRIDGE_CLASS.get() else {
        tracing::debug!("MediaNativeBridge not loaded yet, media session update dropped");
        return;
    };
    let Ok(ctx) = std::panic::catch_unwind(ndk_context::android_context) else {
        return;
    };
    let Ok(vm) = (unsafe { JavaVM::from_raw(ctx.vm() as *mut _) }) else {
        return;
    };
    let Ok(mut env) = vm.attach_current_thread_permanently() else {
        return;
    };
    // This thread stays attached, so free the call's local refs right away.
    let result = env.with_local_frame(8, |env| -> jni::errors::Result<()> {
        let class: &JClass = class.as_obj().into();
        f(env, class)
    });
    if let Err(e) = result {
        tracing::warn!("MediaNativeBridge call failed: {e}");
        clear_exception(&mut env);
    }
}

fn clear_exception(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

pub(crate) fn load_app_class<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject<'local>,
    binary_name: &str,
) -> Result<JClass<'local>, String> {
    let loader = env
        .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| format!("getClassLoader: {e}"))?
        .l()
        .map_err(|e| format!("getClassLoader value: {e}"))?;
    if loader.is_null() {
        return Err("ClassLoader is null".into());
    }
    let name = env
        .new_string(binary_name)
        .map_err(|e| format!("new_string: {e}"))?;
    let class_obj = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[(&name).into()],
        )
        .map_err(|e| format!("loadClass({binary_name}): {e}"))?
        .l()
        .map_err(|e| format!("loadClass value: {e}"))?;
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Err(format!("loadClass threw for {binary_name}"));
    }
    if class_obj.is_null() {
        return Err(format!("loadClass returned null for {binary_name}"));
    }
    Ok(JClass::from(class_obj))
}

extern "system" fn native_on_media_action(mut env: JNIEnv, _class: JClass, action: JString) {
    let Ok(action) = env.get_string(&action) else {
        return;
    };
    push_action(action.to_string_lossy().into_owned());
}
