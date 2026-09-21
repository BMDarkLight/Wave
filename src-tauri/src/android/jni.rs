// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Android JNI / `ndk-context` bootstrap for cpal/oboe.
//!
//! On Android, cpal/oboe require:
//! 1. The calling thread to be attached to the JVM.
//! 2. `ndk_context` to hold both a `JavaVM` and a real Android `Context`
//!    jobject (Activity or Application); oboe uses it for `AudioManager`.
//!
//! Tao 0.35+ no longer calls `ndk_context::initialize_android_context` (it
//! keeps its own multi-window context map). We copy the main activity's VM +
//! jobject from tao into `ndk_context` before opening an audio stream.

#[cfg(target_os = "android")]
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
#[cfg(target_os = "android")]
use std::sync::Mutex;

/// Cached JavaVM for per-thread attachment.
#[cfg(target_os = "android")]
static VM_PTR: AtomicPtr<jni::sys::JavaVM> = AtomicPtr::new(std::ptr::null_mut());

/// Whether `ndk_context` has been successfully seeded from tao.
#[cfg(target_os = "android")]
static NDK_READY: AtomicBool = AtomicBool::new(false);

/// Serialises init attempts so we never double-call `initialize_android_context`.
#[cfg(target_os = "android")]
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// Returns true if `ndk_context` already has a value (avoids the assert on
/// double-init).
#[cfg(target_os = "android")]
fn ndk_context_is_set() -> bool {
    std::panic::catch_unwind(|| {
        let _ = ndk_context::android_context();
    })
    .is_ok()
}

/// Seed `ndk_context` from tao's main activity, if available.
///
/// Safe to call repeatedly: succeeds once and becomes a no-op. Returns
/// `false` when tao has not published an activity yet (caller should retry).
#[cfg(target_os = "android")]
fn try_seed_ndk_context() -> bool {
    if NDK_READY.load(Ordering::Acquire) {
        return true;
    }

    let _guard = INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if NDK_READY.load(Ordering::Acquire) {
        return true;
    }

    if ndk_context_is_set() {
        // Something else already initialized it; cache the VM for attachment.
        let ctx = ndk_context::android_context();
        VM_PTR.store(ctx.vm() as *mut jni::sys::JavaVM, Ordering::Release);
        NDK_READY.store(true, Ordering::Release);
        return true;
    }

    let Some(ctx) = tauri::tao::platform::android::prelude::main_android_context() else {
        tracing::warn!("tao main_android_context not ready, cannot seed ndk_context yet");
        return false;
    };

    if ctx.java_vm.is_null() || ctx.context_jobject.is_null() {
        tracing::warn!(
            "tao android context has null pointers (vm={:p}, ctx={:p})",
            ctx.java_vm,
            ctx.context_jobject
        );
        return false;
    }

    // tao already holds a GlobalRef on the activity; the raw jobject stays
    // valid for the lifetime of the main activity.
    unsafe {
        ndk_context::initialize_android_context(ctx.java_vm, ctx.context_jobject);
    }

    VM_PTR.store(ctx.java_vm as *mut jni::sys::JavaVM, Ordering::Release);
    NDK_READY.store(true, Ordering::Release);
    tracing::info!("ndk_context seeded from tao main activity");
    true
}

/// Attach the current thread to the JVM and ensure `ndk_context` is populated
/// so oboe/cpal can open an audio stream.
///
/// This is a no-op on non-Android targets.
#[cfg(target_os = "android")]
pub(crate) fn ensure_jni_thread_attached() {
    if !try_seed_ndk_context() {
        return;
    }

    let vm_ptr = VM_PTR.load(Ordering::Acquire);
    if vm_ptr.is_null() {
        return;
    }

    unsafe {
        // from_raw wraps the pointer without taking ownership.
        let vm = match jni::JavaVM::from_raw(vm_ptr) {
            Ok(vm) => vm,
            Err(e) => {
                tracing::warn!("JavaVM::from_raw failed: {e}");
                return;
            }
        };

        match vm.attach_current_thread_permanently() {
            Ok(_env) => {
                tracing::debug!("JNI thread attached");
            }
            Err(e) => {
                tracing::warn!("JNI AttachCurrentThread failed: {e}");
            }
        }

        // Prevent Drop from calling JNI_DestroyJavaVM on the shared VM.
        std::mem::forget(vm);
    }
}

/// Whether Android audio is ready (ndk_context seeded). Always true elsewhere.
#[cfg(target_os = "android")]
pub(crate) fn android_audio_ready() -> bool {
    NDK_READY.load(Ordering::Acquire) || try_seed_ndk_context()
}

#[cfg(not(target_os = "android"))]
pub(crate) fn ensure_jni_thread_attached() {}

#[cfg(not(target_os = "android"))]
pub(crate) fn android_audio_ready() -> bool {
    true
}
