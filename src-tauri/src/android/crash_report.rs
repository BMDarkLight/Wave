//! Read the Android CrashReporter file so the UI can show the last failure
//! without adb.

#![cfg(target_os = "android")]

use jni::objects::{JObject, JString, JValue};
use jni::JavaVM;

use crate::android::jni as android_jni;

fn with_crash_reporter<F, R>(f: F) -> Result<R, String>
where
    F: for<'local> FnOnce(&mut jni::JNIEnv<'local>, JObject<'local>) -> Result<R, String>,
{
    android_jni::ensure_jni_thread_attached();
    let ctx = match std::panic::catch_unwind(ndk_context::android_context) {
        Ok(ctx) => ctx,
        Err(_) => return Err("ndk_context not available".into()),
    };
    let vm = unsafe { JavaVM::from_raw(ctx.vm() as *mut _) }
        .map_err(|e| format!("JavaVM::from_raw: {e}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("attach_current_thread: {e}"))?;
    let activity = unsafe { JObject::from_raw(ctx.context() as *mut _) };
    if activity.is_null() {
        return Err("Android activity is null".into());
    }
    f(&mut env, activity)
}

fn load_reporter_class<'local>(
    env: &mut jni::JNIEnv<'local>,
    activity: &JObject<'local>,
) -> Result<jni::objects::JClass<'local>, String> {
    let loader = env
        .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| format!("getClassLoader: {e}"))?
        .l()
        .map_err(|e| format!("getClassLoader value: {e}"))?;
    let name = env
        .new_string("app.bmdarklight.wave.CrashReporter")
        .map_err(|e| format!("new_string: {e}"))?;
    let class_obj = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[(&name).into()],
        )
        .map_err(|e| format!("loadClass: {e}"))?
        .l()
        .map_err(|e| format!("loadClass value: {e}"))?;
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Err("CrashReporter loadClass threw".into());
    }
    Ok(jni::objects::JClass::from(class_obj))
}

pub fn take_last() -> Option<String> {
    with_crash_reporter(|env, activity| {
        let cls = load_reporter_class(env, &activity)?;
        let result = env
            .call_static_method(
                cls,
                "takeLast",
                "(Landroid/content/Context;)Ljava/lang/String;",
                &[JValue::Object(&activity)],
            )
            .map_err(|e| format!("takeLast: {e}"))?;
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            return Ok(None);
        }
        let obj = result.l().map_err(|e| format!("takeLast value: {e}"))?;
        if obj.is_null() {
            return Ok(None);
        }
        let jstr: JString = obj.into();
        let text: String = env
            .get_string(&jstr)
            .map_err(|e| format!("get_string: {e}"))?
            .into();
        if text.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    })
    .ok()
    .flatten()
}

pub fn clear_last() {
    let _ = with_crash_reporter(|env, activity| {
        let cls = load_reporter_class(env, &activity)?;
        env.call_static_method(
            cls,
            "clearLast",
            "(Landroid/content/Context;)V",
            &[JValue::Object(&activity)],
        )
        .map_err(|e| format!("clearLast: {e}"))?;
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        Ok(())
    });
}
