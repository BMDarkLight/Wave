//! JNI bridge to `app.bmdarklight.wave.audio.WaveExoPlayer`.

#![cfg(target_os = "android")]

use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::android::jni as android_jni;

const PLAYER_CLASS: &str = "app.bmdarklight.wave.audio.WaveExoPlayer";

static EXO_READY: AtomicBool = AtomicBool::new(false);
static EXO_PLAYER: Mutex<Option<ExoHandle>> = Mutex::new(None);

struct ExoHandle {
    /// Process JVM — must not Drop (that would call DestroyJavaVM).
    vm: ManuallyDrop<JavaVM>,
    instance: GlobalRef,
}

// GlobalRef is already Send+Sync via Arc; ManuallyDrop<JavaVM> is a raw pointer.
unsafe impl Send for ExoHandle {}
unsafe impl Sync for ExoHandle {}

impl ExoHandle {
    fn with_env<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut JNIEnv) -> Result<R, String>,
    {
        let mut env = self
            .vm
            .attach_current_thread()
            .map_err(|e| format!("AttachCurrentThread: {e}"))?;
        f(&mut env)
    }

    fn call_void(&self, method: &str, sig: &str, args: &[JValue]) -> Result<(), String> {
        self.with_env(|env| {
            env.call_method(self.instance.as_obj(), method, sig, args)
                .map_err(|e| format!("{method}: {e}"))?;
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err(format!("{method} threw"));
            }
            Ok(())
        })
    }

    fn call_long(&self, method: &str) -> Result<i64, String> {
        self.with_env(|env| {
            let result = env
                .call_method(self.instance.as_obj(), method, "()J", &[])
                .map_err(|e| format!("{method}: {e}"))?;
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err(format!("{method} threw"));
            }
            result.j().map_err(|e| format!("{method} result: {e}"))
        })
    }

    fn call_int(&self, method: &str, sig: &str, args: &[JValue]) -> Result<i32, String> {
        self.with_env(|env| {
            let result = env
                .call_method(self.instance.as_obj(), method, sig, args)
                .map_err(|e| format!("{method}: {e}"))?;
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err(format!("{method} threw"));
            }
            result.i().map_err(|e| format!("{method} result: {e}"))
        })
    }

    fn call_bool(&self, method: &str) -> Result<bool, String> {
        self.with_env(|env| {
            let result = env
                .call_method(self.instance.as_obj(), method, "()Z", &[])
                .map_err(|e| format!("{method}: {e}"))?;
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err(format!("{method} threw"));
            }
            result.z().map_err(|e| format!("{method} result: {e}"))
        })
    }
}

/// Ensure the Java ExoPlayer singleton exists. Safe to call repeatedly.
pub fn ensure_initialized() -> Result<(), String> {
    if EXO_READY.load(Ordering::Acquire) && EXO_PLAYER.lock().ok().is_some_and(|g| g.is_some()) {
        return Ok(());
    }

    android_jni::ensure_jni_thread_attached();

    let ctx = match std::panic::catch_unwind(ndk_context::android_context) {
        Ok(ctx) => ctx,
        Err(_) => return Err("ndk_context not available".into()),
    };

    let vm = unsafe { JavaVM::from_raw(ctx.vm() as *mut _) }
        .map_err(|e| format!("JavaVM::from_raw: {e}"))?;

    // Scope the AttachGuard so it drops before we wrap `vm` in ManuallyDrop.
    let global = {
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| format!("attach_current_thread: {e}"))?;

        let activity = unsafe { JObject::from_raw(ctx.context() as *mut _) };
        if activity.is_null() {
            return Err("Android activity is null".into());
        }

        let class = load_class(&mut env, &activity, PLAYER_CLASS)?;

        let instance = env
            .call_static_method(
                &class,
                "getOrCreate",
                "(Landroid/content/Context;)Lapp/bmdarklight/wave/audio/WaveExoPlayer;",
                &[JValue::Object(&activity)],
            )
            .map_err(|e| format!("getOrCreate: {e}"))?
            .l()
            .map_err(|e| format!("getOrCreate value: {e}"))?;

        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            return Err("WaveExoPlayer.getOrCreate threw".into());
        }
        if instance.is_null() {
            return Err("WaveExoPlayer.getOrCreate returned null".into());
        }

        env.new_global_ref(&instance)
            .map_err(|e| format!("new_global_ref: {e}"))?
    };

    let handle = ExoHandle {
        vm: ManuallyDrop::new(vm),
        instance: global,
    };
    *EXO_PLAYER
        .lock()
        .map_err(|_| "ExoPlayer mutex poisoned".to_string())? = Some(handle);
    EXO_READY.store(true, Ordering::Release);
    tracing::info!("WaveExoPlayer initialized");
    Ok(())
}

fn load_class<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject<'local>,
    binary_name: &str,
) -> Result<jni::objects::JClass<'local>, String> {
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
    Ok(jni::objects::JClass::from(class_obj))
}

fn with_player<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce(&ExoHandle) -> Result<R, String>,
{
    ensure_initialized()?;
    let guard = EXO_PLAYER
        .lock()
        .map_err(|_| "ExoPlayer mutex poisoned".to_string())?;
    let player = guard.as_ref().ok_or("ExoPlayer not initialized")?;
    f(player)
}

/// Play a URI (`content://`, `file://`, or absolute path).
pub fn exo_play_uri(uri: &str) -> Result<(), String> {
    with_player(|p| {
        p.with_env(|env| {
            let j_uri = env.new_string(uri).map_err(|e| format!("uri string: {e}"))?;
            env.call_method(
                p.instance.as_obj(),
                "playUri",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&j_uri)],
            )
            .map_err(|e| format!("playUri: {e}"))?;
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err("playUri threw".into());
            }
            Ok(())
        })
    })
}

pub fn exo_prepare_uri_at(uri: &str, position_ms: i64) -> Result<(), String> {
    with_player(|p| {
        p.with_env(|env| {
            let j_uri = env.new_string(uri).map_err(|e| format!("uri string: {e}"))?;
            env.call_method(
                p.instance.as_obj(),
                "prepareUriAt",
                "(Ljava/lang/String;J)V",
                &[JValue::Object(&j_uri), JValue::Long(position_ms)],
            )
            .map_err(|e| format!("prepareUriAt: {e}"))?;
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err("prepareUriAt threw".into());
            }
            Ok(())
        })
    })
}

pub fn exo_play() -> Result<(), String> {
    with_player(|p| p.call_void("play", "()V", &[]))
}

pub fn exo_pause() -> Result<(), String> {
    with_player(|p| p.call_void("pause", "()V", &[]))
}

pub fn exo_stop() -> Result<(), String> {
    with_player(|p| p.call_void("stop", "()V", &[]))
}

pub fn exo_seek(position_ms: i64) -> Result<(), String> {
    with_player(|p| p.call_void("seekTo", "(J)V", &[JValue::Long(position_ms)]))
}

pub fn exo_set_volume(volume: f32) -> Result<(), String> {
    with_player(|p| p.call_void("setVolume", "(F)V", &[JValue::Float(volume)]))
}

pub fn exo_set_eq_enabled(enabled: bool) -> Result<(), String> {
    with_player(|p| p.call_void("setEqEnabled", "(Z)V", &[JValue::Bool(enabled as u8)]))
}

pub fn exo_set_eq_bands(bands: &[f32; 10]) -> Result<(), String> {
    with_player(|p| {
        p.with_env(|env| {
            let array = env
                .new_float_array(bands.len() as i32)
                .map_err(|e| format!("new_float_array: {e}"))?;
            env.set_float_array_region(&array, 0, bands)
                .map_err(|e| format!("set_float_array_region: {e}"))?;
            env.call_method(
                p.instance.as_obj(),
                "setEqBands",
                "([F)V",
                &[JValue::Object(&array)],
            )
            .map_err(|e| format!("setEqBands: {e}"))?;
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err("setEqBands threw".into());
            }
            Ok(())
        })
    })
}

pub fn exo_set_crossfade_duration(seconds: f32) -> Result<(), String> {
    with_player(|p| p.call_void("setCrossfadeDuration", "(F)V", &[JValue::Float(seconds)]))
}

pub fn exo_set_gapless_enabled(enabled: bool) -> Result<(), String> {
    with_player(|p| p.call_void("setGaplessEnabled", "(Z)V", &[JValue::Bool(enabled as u8)]))
}

pub fn exo_play_media_items(uris: &[String], start_index: usize) -> Result<(), String> {
    with_player(|p| {
        p.with_env(|env| {
            let string_class = env
                .find_class("java/lang/String")
                .map_err(|e| format!("String class: {e}"))?;
            let array = env
                .new_object_array(uris.len() as i32, string_class, JObject::null())
                .map_err(|e| format!("new_object_array: {e}"))?;
            for (i, uri) in uris.iter().enumerate() {
                let j_uri = env.new_string(uri).map_err(|e| format!("uri string: {e}"))?;
                env.set_object_array_element(&array, i as i32, j_uri)
                    .map_err(|e| format!("set_object_array_element: {e}"))?;
            }
            env.call_method(
                p.instance.as_obj(),
                "playMediaItems",
                "([Ljava/lang/String;I)V",
                &[JValue::Object(&array), JValue::Int(start_index as i32)],
            )
            .map_err(|e| format!("playMediaItems: {e}"))?;
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err("playMediaItems threw".into());
            }
            Ok(())
        })
    })
}

pub fn exo_set_upcoming_uri(uri: Option<&str>) -> Result<(), String> {
    with_player(|p| {
        p.with_env(|env| {
            match uri {
                Some(uri) => {
                    let j_uri = env.new_string(uri).map_err(|e| format!("uri string: {e}"))?;
                    env.call_method(
                        p.instance.as_obj(),
                        "setUpcomingUri",
                        "(Ljava/lang/String;)V",
                        &[JValue::Object(&j_uri)],
                    )
                    .map_err(|e| format!("setUpcomingUri: {e}"))?;
                }
                None => {
                    env.call_method(
                        p.instance.as_obj(),
                        "setUpcomingUri",
                        "(Ljava/lang/String;)V",
                        &[JValue::Object(&JObject::null())],
                    )
                    .map_err(|e| format!("setUpcomingUri: {e}"))?;
                }
            }
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err("setUpcomingUri threw".into());
            }
            Ok(())
        })
    })
}

pub fn exo_consume_media_index_change() -> Result<Option<i32>, String> {
    with_player(|p| {
        let index = p.call_int("consumeMediaIndexChange", "()I", &[])?;
        if index < 0 {
            Ok(None)
        } else {
            Ok(Some(index))
        }
    })
}

pub fn exo_has_next_media_item() -> Result<bool, String> {
    with_player(|p| p.call_bool("hasNextMediaItem"))
}

pub fn exo_seek_to_next_media_item() -> Result<bool, String> {
    with_player(|p| p.call_bool("seekToNextMediaItem"))
}

pub fn exo_get_current_media_index() -> Result<i32, String> {
    with_player(|p| p.call_int("getCurrentMediaIndex", "()I", &[]))
}

pub fn exo_consume_crossfade_handoff() -> Result<Option<String>, String> {
    with_player(|p| {
        p.with_env(|env| {
            let result = env
                .call_method(
                    p.instance.as_obj(),
                    "consumeCrossfadeHandoff",
                    "()Ljava/lang/String;",
                    &[],
                )
                .map_err(|e| format!("consumeCrossfadeHandoff: {e}"))?;
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err("consumeCrossfadeHandoff threw".into());
            }
            let obj = result.l().map_err(|e| format!("consumeCrossfadeHandoff value: {e}"))?;
            if obj.is_null() {
                return Ok(None);
            }
            let jstr = JString::from(obj);
            let uri: String = env
                .get_string(&jstr)
                .map_err(|e| format!("consumeCrossfadeHandoff string: {e}"))?
                .into();
            Ok(Some(uri))
        })
    })
}

pub fn exo_get_position() -> Result<i64, String> {
    with_player(|p| p.call_long("getCurrentPosition"))
}

pub fn exo_get_duration() -> Result<i64, String> {
    with_player(|p| p.call_long("getDuration"))
}

pub fn exo_is_playing() -> Result<bool, String> {
    with_player(|p| p.call_bool("isPlaying"))
}

/// True when ExoPlayer reached end-of-media for the current item.
pub fn exo_playback_ended() -> Result<bool, String> {
    with_player(|p| p.call_bool("isEnded"))
}

pub fn is_exo_ready() -> bool {
    EXO_READY.load(Ordering::Acquire)
        && EXO_PLAYER
            .lock()
            .ok()
            .is_some_and(|g| g.is_some())
}
