// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Decides whether remote traffic may leave the device right now.

use super::backend::RemoteError;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateState {
    Allowed,
    Disabled,
    Cellular,
}

pub struct NetworkGate {
    enabled: AtomicBool,
    cellular_allowed: AtomicBool,
    cellular_probe: RwLock<fn() -> bool>,
}

fn not_cellular() -> bool {
    false
}

impl NetworkGate {
    fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            cellular_allowed: AtomicBool::new(false),
            cellular_probe: RwLock::new(not_cellular),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Self::new()
    }

    pub fn global() -> &'static NetworkGate {
        static GATE: OnceLock<NetworkGate> = OnceLock::new();
        GATE.get_or_init(NetworkGate::new)
    }

    /// The gate for streamed search results, which follow the search switch
    /// rather than Outside Sourcing.
    pub fn search() -> &'static NetworkGate {
        static GATE: OnceLock<NetworkGate> = OnceLock::new();
        GATE.get_or_init(NetworkGate::new)
    }

    /// The gate that governs a source's traffic.
    pub fn for_source(source_id: &str) -> &'static NetworkGate {
        if source_id == super::search::SEARCH_SOURCE {
            Self::search()
        } else {
            Self::global()
        }
    }

    /// [`Self::for_source`] for a track path; local paths get the global gate.
    pub fn for_path(path: &str) -> &'static NetworkGate {
        match super::uri::RemoteUri::parse(path) {
            Some(uri) => Self::for_source(&uri.source_id),
            None => Self::global(),
        }
    }

    pub fn set_policy(&self, enabled: bool, cellular_allowed: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.cellular_allowed
            .store(cellular_allowed, Ordering::Relaxed);
    }

    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn set_cellular_probe(&self, probe: fn() -> bool) {
        *self.cellular_probe.write() = probe;
    }

    pub fn state(&self) -> GateState {
        if !self.enabled.load(Ordering::Relaxed) {
            return GateState::Disabled;
        }
        if !self.cellular_allowed.load(Ordering::Relaxed) && (*self.cellular_probe.read())() {
            return GateState::Cellular;
        }
        GateState::Allowed
    }

    pub fn check(&self) -> Result<(), RemoteError> {
        match self.state() {
            GateState::Allowed => Ok(()),
            blocked => Err(RemoteError::Blocked(blocked)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_by_default() {
        assert_eq!(NetworkGate::new_for_test().state(), GateState::Disabled);
    }

    #[test]
    fn cellular_blocks_unless_allowed() {
        let gate = NetworkGate::new_for_test();
        gate.set_cellular_probe(|| true);
        gate.set_policy(true, false);
        assert_eq!(gate.state(), GateState::Cellular);
        gate.set_policy(true, true);
        assert_eq!(gate.state(), GateState::Allowed);
        gate.set_policy(true, false);
        gate.set_cellular_probe(|| false);
        assert_eq!(gate.state(), GateState::Allowed);
    }
}
