// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The peer API's host for a server with no window and no `AppHandle`.
//!
//! Identity and paired devices live in the server's own settings file, so a
//! hosted server can be paired and scoped without opening the desktop app.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::library::Library;
use crate::peers::api::Host;
use crate::peers::PeerRecord;

use super::{save_config, ServerConfig};

pub struct HeadlessHost {
    config: Mutex<ServerConfig>,
    library: Arc<parking_lot::Mutex<Library>>,
}

impl HeadlessHost {
    pub fn new(config: ServerConfig, library: Arc<parking_lot::Mutex<Library>>) -> Self {
        Self {
            config: Mutex::new(config),
            library,
        }
    }
}

impl Host for HeadlessHost {
    fn identity(&self) -> (String, String) {
        match self.config.lock() {
            Ok(config) => (config.device_id.clone(), config.name.clone()),
            Err(_) => (String::new(), crate::share::default_name()),
        }
    }

    fn peers(&self) -> Vec<PeerRecord> {
        self.config
            .lock()
            .map(|config| config.peers.clone())
            .unwrap_or_default()
    }

    fn update_peers(&self, change: &mut dyn FnMut(&mut Vec<PeerRecord>)) {
        let Ok(mut config) = self.config.lock() else {
            return;
        };
        change(&mut config.peers);
        if let Err(e) = save_config(&config) {
            tracing::warn!("Couldn't save paired devices to the server settings: {e}");
        }
    }

    fn fingerprints(&self) -> HashMap<String, String> {
        self.library
            .lock()
            .local_fingerprints()
            .map(|by_print| {
                by_print
                    .into_iter()
                    .map(|(print, id)| (id, print))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn emit(&self, event: &str, payload: serde_json::Value) {
        tracing::debug!("Server event {event}: {payload}");
    }

    fn paired(&self, _peer: &PeerRecord) {
        // A hosted server reads no remote sources, so there is nothing to add.
    }

    fn forgotten(&self, _peer_id: &str) {}

    fn reachable(&self, _peer_id: &str, _online: bool) {}
}
