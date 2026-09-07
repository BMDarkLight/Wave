// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Probe audio metadata from `content://` URIs via MediaMetadataRetriever.

use tauri::AppHandle;

/// Metadata returned from the Android MediaMetadataRetriever probe.
#[derive(Debug, Clone, Default)]
pub struct UriProbeResult {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub track_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub mime: Option<String>,
    pub display_name: Option<String>,
    pub file_size: i64,
    pub cover_jpeg: Option<Vec<u8>>,
}

/// Probe a `content://` URI. Desktop stub returns an error.
#[cfg(not(target_os = "android"))]
pub fn probe_content_uri(_app: &AppHandle, uri: &str) -> Result<UriProbeResult, String> {
    Err(format!(
        "content:// metadata probe is only available on Android (got {uri})"
    ))
}

#[cfg(target_os = "android")]
pub fn probe_content_uri(_app: &AppHandle, uri: &str) -> Result<UriProbeResult, String> {
    use base64::{engine::general_purpose, Engine as _};
    use jni::objects::{JObject, JString, JValue};
    use jni::JavaVM;
    use ndk_context::android_context;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ProbeJson {
        title: Option<String>,
        artist: Option<String>,
        album: Option<String>,
        album_artist: Option<String>,
        genre: Option<String>,
        year: Option<i32>,
        track_number: Option<i32>,
        duration_ms: Option<i64>,
        mime: Option<String>,
        display_name: Option<String>,
        file_size: Option<i64>,
        cover_base64: Option<String>,
        #[allow(dead_code)]
        cover_mime: Option<String>,
    }

    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return Err("MediaMetadataProbe: URI is empty".into());
    }
    if !trimmed.starts_with("content://") {
        return Err(format!(
            "MediaMetadataProbe: expected content:// URI, got: {trimmed}"
        ));
    }

    crate::android::jni::ensure_jni_thread_attached();

    let ctx = android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm() as *mut _) }
        .map_err(|e| format!("MediaMetadataProbe: JavaVM: {e}"))?;
    let activity = ctx.context();
    if activity.is_null() {
        return Err("MediaMetadataProbe: Activity is null".into());
    }

    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("MediaMetadataProbe: attach: {e}"))?;
    let activity_obj = unsafe { JObject::from_raw(activity as *mut _) };

    let loader = env
        .call_method(&activity_obj, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| format!("getClassLoader: {e}"))?
        .l()
        .map_err(|e| format!("getClassLoader value: {e}"))?;
    if loader.is_null() {
        return Err("MediaMetadataProbe: ClassLoader is null".into());
    }

    let class_name = env
        .new_string("app.bmdarklight.wave.MediaMetadataProbe")
        .map_err(|e| format!("class name: {e}"))?;
    let class_obj = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[(&class_name).into()],
        )
        .map_err(|e| format!("loadClass: {e}"))?
        .l()
        .map_err(|e| format!("loadClass value: {e}"))?;
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Err("MediaMetadataProbe class missing from APK".into());
    }
    if class_obj.is_null() {
        return Err("MediaMetadataProbe class is null".into());
    }
    let class = jni::objects::JClass::from(class_obj);

    let uri_j = env
        .new_string(trimmed)
        .map_err(|e| format!("uri string: {e}"))?;
    let result = env
        .call_static_method(
            &class,
            "probe",
            "(Landroid/app/Activity;Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&activity_obj), JValue::Object(&uri_j)],
        )
        .map_err(|e| format!("probe: {e}"))?;

    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Err("MediaMetadataProbe.probe threw".into());
    }

    let json_obj = result
        .l()
        .map_err(|e| format!("probe result: {e}"))?;
    if json_obj.is_null() {
        return Err("MediaMetadataProbe.probe returned null".into());
    }
    let json_j: JString = json_obj.into();
    let json = env
        .get_string(&json_j)
        .map_err(|e| format!("probe json: {e}"))?
        .to_string_lossy()
        .into_owned();

    let parsed: ProbeJson =
        serde_json::from_str(&json).map_err(|e| format!("probe JSON parse: {e}"))?;

    let cover_jpeg = parsed
        .cover_base64
        .as_deref()
        .and_then(|b64| general_purpose::STANDARD.decode(b64).ok())
        .filter(|bytes| !bytes.is_empty());

    Ok(UriProbeResult {
        title: parsed.title,
        artist: parsed.artist,
        album: parsed.album,
        album_artist: parsed.album_artist,
        genre: parsed.genre,
        year: parsed.year,
        track_number: parsed.track_number,
        duration_ms: parsed.duration_ms,
        mime: parsed.mime,
        display_name: parsed.display_name,
        file_size: parsed.file_size.unwrap_or(0),
        cover_jpeg,
    })
}
