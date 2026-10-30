// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Playing on another device instead of this one's own output.
//!
//! While the player has a [`CastOutput`], the queue, repeat and shuffle stay
//! here and work as always; each track the player would open is handed to
//! the output instead, and what the player reports about position, pause
//! and the end of a track comes from the other device.

/// The other device's playback, as last heard.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CastStatus {
    pub position: f64,
    pub duration: Option<f64>,
    pub playing: bool,
    pub paused: bool,
    /// The track played to its end there.
    pub ended: bool,
    /// Loading, or waiting for the network.
    pub buffering: bool,
}

pub trait CastOutput: Send {
    /// The device's id and the name to show for it.
    fn device(&self) -> (String, String);
    /// Starts `path` there at `position`, playing or paused. Fails when the
    /// track can't be played on that device at all.
    fn load(&mut self, path: &str, position: f64, playing: bool) -> Result<(), String>;
    fn pause(&mut self);
    fn resume(&mut self);
    fn seek(&mut self, seconds: f64);
    fn set_volume(&mut self, volume: f32);
    /// Stops playing there; the output is done after this.
    fn stop(&mut self);
    fn status(&self) -> CastStatus;
    /// Why the device can no longer be played to: it went away, or someone
    /// there started something else.
    fn lost(&self) -> Option<String>;
}
