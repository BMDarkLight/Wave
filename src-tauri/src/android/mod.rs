// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Android platform integration: JNI bootstrap, SAF import/scan, ExoPlayer.

pub mod folder_picker;
pub mod import;
pub mod jni;
pub mod metadata;
pub mod saf_io;
pub mod saf_scan;

#[cfg(target_os = "android")]
pub mod audio;

#[cfg(target_os = "android")]
pub mod media_bridge;

#[cfg(target_os = "android")]
pub mod crash_report;
