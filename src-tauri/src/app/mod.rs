// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

#[cfg(not(target_os = "android"))]
pub mod desktop_update;
#[cfg(not(target_os = "android"))]
pub mod launch;
pub mod library_recovery;
#[cfg(not(target_os = "android"))]
pub mod open_files;
pub mod paths;
pub mod settings;
pub mod single_instance;
pub mod theme;
pub mod updater;
