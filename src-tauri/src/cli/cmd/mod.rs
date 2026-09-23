// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! One module per subcommand group.
//!
//! These resolve arguments and talk to the library or the daemon. Formatting
//! belongs in `cli::render`.

pub mod devices;
pub mod dsp;
pub mod favorite;
pub mod metadata;
pub mod playback;
pub mod playlists;
pub mod queue;
pub mod stats;
pub mod tracks;
