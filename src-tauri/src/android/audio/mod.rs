// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Android ExoPlayer audio backend (JNI).
//!
//! Plays `content://` and `file://` URIs natively via Media3 ExoPlayer.
//! Queue / shuffle / repeat / media notifications stay in Rust + the
//! existing media-session plugin. This module is decode + output only.

#![cfg(target_os = "android")]

mod jni_bridge;

pub use jni_bridge::*;
