//! Android platform integration: JNI bootstrap, SAF import/scan, ExoPlayer.

pub mod folder_picker;
pub mod import;
pub mod jni;
pub mod metadata;
pub mod saf_scan;

#[cfg(target_os = "android")]
pub mod audio;

#[cfg(target_os = "android")]
pub mod media_bridge;

#[cfg(target_os = "android")]
pub mod crash_report;
