// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Online play: full-length audio for catalog results, found on YouTube.
//!
//! Wave ships none of the code that talks to YouTube. When the user turns
//! Online play on and agrees to the install, [`tools`] fetches yt-dlp and the
//! Deno runtime it needs from their own GitHub releases, and [`ytdlp`] runs
//! them. Desktop only: Android can't run programs an app downloads.

pub mod commands;
#[cfg(not(target_os = "android"))]
pub mod tools;
// Searching and stream lookup are wired into playback in the next step.
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub mod ytdlp;
