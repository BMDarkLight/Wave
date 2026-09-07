// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Android folder picker using Storage Access Framework (SAF) via JNI.

use tauri::AppHandle;

/// Result from the folder picker.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FolderPickerResult {
    pub uri: String,
    pub display_name: Option<String>,
}

/// Opens the Android Storage Access Framework folder picker.
/// Returns a content:// URI with persistable URI permission.
#[cfg(target_os = "android")]
pub fn pick_folder(_app: &AppHandle) -> Result<FolderPickerResult, String> {
    use jni::objects::{JObject, JString, JThrowable, JValue, JValueOwned};
    use jni::JavaVM;
    use ndk_context::android_context;

    fn jstring_to_owned(env: &mut jni::JNIEnv<'_>, obj: JObject<'_>) -> Option<String> {
        if obj.is_null() {
            return None;
        }
        let js: JString = obj.into();
        env.get_string(&js)
            .ok()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
    }

    fn throwable_message(
        env: &mut jni::JNIEnv<'_>,
        ex: JThrowable<'_>,
        depth: u8,
    ) -> Option<String> {
        if depth > 6 {
            return Some("Folder picker: nested Java exception".into());
        }
        // Unwrap ExecutionException / InvocationTargetException / NoSuchMethodError causes.
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
        .map_err(|e| format!("Folder picker: failed to get JavaVM: {e}"))?;
    let activity = ctx.context();
    if activity.is_null() {
        return Err("Folder picker: Android Activity context is null".into());
    }

    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("Folder picker: failed to attach JNI thread: {e}"))?;

    let activity_obj = unsafe { JObject::from_raw(activity as *mut _) };

    // Load via the Activity ClassLoader — system FindClass often misses app classes.
    let picker_class = {
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
            "Folder picker: getClassLoader failed",
        )?;
        let loader = loader_v
            .l()
            .map_err(|e| format!("Folder picker: bad ClassLoader: {e}"))?;
        if loader.is_null() {
            return Err("Folder picker: Activity ClassLoader is null".into());
        }

        let class_name = env
            .new_string("app.bmdarklight.wave.FolderPickerCallback")
            .map_err(|e| format!("Folder picker: class name string failed: {e}"))?;
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
            "Folder picker: loadClass(FolderPickerCallback) failed",
        )?;
        let class_obj = class_v
            .l()
            .map_err(|e| format!("Folder picker: bad Class object: {e}"))?;
        if class_obj.is_null() {
            return Err(
                "Folder picker: FolderPickerCallback class missing from APK — rebuild with Android CI (Configure Android project step)".into(),
            );
        }
        jni::objects::JClass::from(class_obj)
    };

    // JNI-friendly API: String[] pickForJni(Activity) — avoids CompletableFuture
    // method descriptors that R8/desugar often break for GetStaticMethodID.
    let result_value = call_checked(
        &mut env,
        |env| {
            env.call_static_method(
                &picker_class,
                "pickForJni",
                "(Landroid/app/Activity;)[Ljava/lang/String;",
                &[JValue::Object(&activity_obj)],
            )
        },
        "Folder picker: FolderPickerCallback.pickForJni() failed",
    )?;

    let result_obj = result_value
        .l()
        .map_err(|e| format!("Folder picker: bad String[] result: {e}"))?;

    // null => user cancelled
    if result_obj.is_null() {
        return Err("Folder picker cancelled".into());
    }

    let array = jni::objects::JObjectArray::from(result_obj);
    let len = env
        .get_array_length(&array)
        .map_err(|e| format!("Folder picker: bad result array length: {e}"))?;
    if len < 1 {
        return Err("Folder picker returned an empty result".into());
    }

    let uri_obj = env
        .get_object_array_element(&array, 0)
        .map_err(|e| format!("Folder picker: reading uri failed: {e}"))?;
    if uri_obj.is_null() {
        return Err("Folder picker returned null URI".into());
    }
    let uri_string = jstring_to_owned(&mut env, uri_obj)
        .ok_or_else(|| "Folder picker returned empty URI".to_string())?;

    let display_name = if len >= 2 {
        let name_obj = env
            .get_object_array_element(&array, 1)
            .map_err(|e| format!("Folder picker: reading displayName failed: {e}"))?;
        jstring_to_owned(&mut env, name_obj)
    } else {
        None
    };

    Ok(FolderPickerResult {
        uri: uri_string,
        display_name,
    })
}

/// Fallback for non-Android targets.
#[cfg(not(target_os = "android"))]
pub fn pick_folder(_app: &AppHandle) -> Result<FolderPickerResult, String> {
    Err("Folder picker is only available on Android".to_string())
}
