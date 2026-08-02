//! Android native media-action bridge.
//!
//! The media-session plugin dispatches play/pause/next/… into
//! `MediaNativeBridge.dispatch`, which calls this module over JNI.
//! Actions are applied on a dedicated worker thread immediately so transport
//! stays responsive when the WebView is frozen in the background.

#![cfg(target_os = "android")]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use jni::objects::{JClass, JObject, JString};
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
        .spawn(move || {
            loop {
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
                if let Err(error) = commands::handle_native_media_action(&app, &action) {
                    tracing::warn!("Android native media action '{action}' failed: {error}");
                }
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

    let class = match load_app_class(&mut env, &activity, "app.bmdarklight.wave.MediaNativeBridge") {
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

    NATIVES_READY.store(true, Ordering::Release);
    tracing::info!("MediaNativeBridge natives registered");
}

fn load_app_class<'local>(
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
