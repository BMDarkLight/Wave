//! List audio files under an Android SAF tree URI via JNI.
//!
//! `tauri-plugin-fs::read_dir` cannot walk `content://…/tree/…` URIs (it tries
//! to convert them to filesystem paths and fails with "URL is not a valid path").

use tauri::AppHandle;

/// Recursively list audio document URIs under a SAF tree URI.
#[cfg(target_os = "android")]
pub fn list_audio_files(_app: &AppHandle, tree_uri: &str) -> Result<Vec<String>, String> {
    use jni::objects::{JObject, JObjectArray, JString, JThrowable, JValue, JValueOwned};
    use jni::JavaVM;
    use ndk_context::android_context;

    let trimmed = tree_uri.trim();
    if trimmed.is_empty() {
        return Err("SAF scan: tree URI is empty".into());
    }
    if !trimmed.starts_with("content://") {
        return Err(format!(
            "SAF scan: expected content:// URI, got: {trimmed}"
        ));
    }

    // Frees the local ref for `obj` before returning. The calling loop runs
    // on a permanently-attached JNI thread (see `jni::ensure_jni_thread_attached`),
    // so there's no per-call frame to reclaim these automatically — scanning a
    // folder with hundreds of files would otherwise fill the local reference
    // table and crash with a JNI "local reference table overflow".
    fn jstring_to_owned(env: &mut jni::JNIEnv<'_>, obj: JObject<'_>) -> Option<String> {
        if obj.is_null() {
            return None;
        }
        let js: JString = obj.into();
        let result = env
            .get_string(&js)
            .ok()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty());
        let _ = env.delete_local_ref(js);
        result
    }

    fn throwable_message(
        env: &mut jni::JNIEnv<'_>,
        ex: JThrowable<'_>,
        depth: u8,
    ) -> Option<String> {
        if depth > 6 {
            return Some("SAF scan: nested Java exception".into());
        }
        match env.call_method(&ex, "getCause", "()Ljava/lang/Throwable;", &[]) {
            Ok(cause_v) => {
                if let Ok(cause) = cause_v.l() {
                    if !cause.is_null() {
                        let cause_t: JThrowable = cause.into();
                        if let Some(inner) = throwable_message(env, cause_t, depth + 1) {
                            return Some(inner);
                        }
                    }
                }
            }
            Err(_) => {
                let _ = env.exception_clear();
            }
        }
        match env.call_method(&ex, "getMessage", "()Ljava/lang/String;", &[]) {
            Ok(msg_v) => {
                if let Ok(obj) = msg_v.l() {
                    if let Some(msg) = jstring_to_owned(env, obj) {
                        return Some(msg);
                    }
                }
            }
            Err(_) => {
                let _ = env.exception_clear();
            }
        }
        match env.call_method(&ex, "toString", "()Ljava/lang/String;", &[]) {
            Ok(v) => v.l().ok().and_then(|obj| jstring_to_owned(env, obj)),
            Err(_) => {
                let _ = env.exception_clear();
                None
            }
        }
    }

    fn jni_error(env: &mut jni::JNIEnv<'_>, fallback: &str) -> String {
        match env.exception_occurred() {
            Ok(ex) if !ex.is_null() => {
                let _ = env.exception_clear();
                throwable_message(env, ex, 0).unwrap_or_else(|| fallback.to_string())
            }
            _ => {
                let _ = env.exception_clear();
                fallback.to_string()
            }
        }
    }

    fn call_checked<'local>(
        env: &mut jni::JNIEnv<'local>,
        call: impl FnOnce(&mut jni::JNIEnv<'local>) -> jni::errors::Result<JValueOwned<'local>>,
        what: &str,
    ) -> Result<JValueOwned<'local>, String> {
        if env.exception_check().unwrap_or(false) {
            return Err(jni_error(env, what));
        }
        match call(env) {
            Ok(value) => {
                if env.exception_check().unwrap_or(false) {
                    Err(jni_error(env, what))
                } else {
                    Ok(value)
                }
            }
            Err(err) => {
                if env.exception_check().unwrap_or(false) {
                    Err(jni_error(env, &format!("{what} ({err})")))
                } else {
                    Err(format!("{what}: {err}"))
                }
            }
        }
    }

    let ctx = android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm() as *mut _) }
        .map_err(|e| format!("SAF scan: failed to get JavaVM: {e}"))?;
    let activity = ctx.context();
    if activity.is_null() {
        return Err("SAF scan: Android Activity context is null".into());
    }

    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("SAF scan: failed to attach JNI thread: {e}"))?;

    let activity_obj = unsafe { JObject::from_raw(activity as *mut _) };

    let scanner_class = {
        let loader_v = call_checked(
            &mut env,
            |env| {
                env.call_method(
                    &activity_obj,
                    "getClassLoader",
                    "()Ljava/lang/ClassLoader;",
                    &[],
                )
            },
            "SAF scan: getClassLoader failed",
        )?;
        let loader = loader_v
            .l()
            .map_err(|e| format!("SAF scan: bad ClassLoader: {e}"))?;
        if loader.is_null() {
            return Err("SAF scan: Activity ClassLoader is null".into());
        }

        let class_name = env
            .new_string("app.bmdarklight.wave.SafMediaScanner")
            .map_err(|e| format!("SAF scan: class name string failed: {e}"))?;
        let class_v = call_checked(
            &mut env,
            |env| {
                env.call_method(
                    &loader,
                    "loadClass",
                    "(Ljava/lang/String;)Ljava/lang/Class;",
                    &[(&class_name).into()],
                )
            },
            "SAF scan: loadClass(SafMediaScanner) failed",
        )?;
        let class_obj = class_v
            .l()
            .map_err(|e| format!("SAF scan: bad Class object: {e}"))?;
        if class_obj.is_null() {
            return Err(
                "SAF scan: SafMediaScanner class missing from APK — rebuild Android CI".into(),
            );
        }
        jni::objects::JClass::from(class_obj)
    };

    let uri_jstring = env
        .new_string(trimmed)
        .map_err(|e| format!("SAF scan: URI string failed: {e}"))?;

    let result_value = call_checked(
        &mut env,
        |env| {
            env.call_static_method(
                &scanner_class,
                "listAudioFiles",
                "(Landroid/app/Activity;Ljava/lang/String;)[Ljava/lang/String;",
                &[
                    JValue::Object(&activity_obj),
                    JValue::Object(&uri_jstring),
                ],
            )
        },
        "SAF scan: SafMediaScanner.listAudioFiles() failed",
    )?;

    let result_obj = result_value
        .l()
        .map_err(|e| format!("SAF scan: bad String[] result: {e}"))?;
    if result_obj.is_null() {
        return Ok(Vec::new());
    }

    let array = JObjectArray::from(result_obj);
    let len = env
        .get_array_length(&array)
        .map_err(|e| format!("SAF scan: bad result array length: {e}"))?;

    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len {
        let elem = env
            .get_object_array_element(&array, i)
            .map_err(|e| format!("SAF scan: reading result[{i}] failed: {e}"))?;
        if let Some(s) = jstring_to_owned(&mut env, elem) {
            out.push(s);
        }
    }

    Ok(out)
}

#[cfg(not(target_os = "android"))]
pub fn list_audio_files(_app: &AppHandle, _tree_uri: &str) -> Result<Vec<String>, String> {
    Err("SAF folder scan is only available on Android".to_string())
}
