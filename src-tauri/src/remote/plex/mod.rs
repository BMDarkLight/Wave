// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Plex client: sign-in, server discovery, catalog paging and ranged audio.

pub mod api;
pub mod auth;
pub mod connection;
pub mod direct;
#[cfg(test)]
pub(crate) mod testkit;

use crate::app::settings::AppSettings;
use crate::remote::backend::RemoteError;
use reqwest::blocking::{Client, RequestBuilder};
use std::sync::OnceLock;
use std::time::Duration;

/// Identifies Wave to Plex. `client_id` is stable per install so the server
/// and plex.tv see one device across launches.
#[derive(Debug, Clone)]
pub struct PlexHeaders {
    pub client_id: String,
    pub version: String,
    pub platform: String,
    pub device_name: String,
}

impl PlexHeaders {
    pub fn new(client_id: String) -> Self {
        Self {
            client_id,
            version: env!("CARGO_PKG_VERSION").to_string(),
            platform: std::env::consts::OS.to_string(),
            device_name: "Wave".to_string(),
        }
    }

    pub fn apply(&self, req: RequestBuilder) -> RequestBuilder {
        self.apply_identity(req)
            .header("Accept", "application/json")
    }

    /// The device headers without `Accept`, for audio and image requests.
    pub fn apply_identity(&self, req: RequestBuilder) -> RequestBuilder {
        req.header("X-Plex-Client-Identifier", &self.client_id)
            .header("X-Plex-Product", "Wave")
            .header("X-Plex-Version", &self.version)
            .header("X-Plex-Platform", &self.platform)
            .header("X-Plex-Device-Name", &self.device_name)
    }
}

/// Returns the stored client identifier, generating one on first use. The
/// caller saves the settings.
pub fn client_identifier(settings: &mut AppSettings) -> String {
    settings
        .plex_client_identifier
        .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
        .clone()
}

/// Adds the server token, unless there is none: a server that lets the local
/// network in without signing in can turn away an empty token.
pub(crate) fn with_server_token(req: RequestBuilder, token: &str) -> RequestBuilder {
    if token.is_empty() {
        req
    } else {
        req.header("X-Plex-Token", token)
    }
}

/// Maps a failed response status. Never includes the URL, which can carry a
/// token.
pub(crate) fn check_status(
    resp: reqwest::blocking::Response,
) -> Result<reqwest::blocking::Response, RemoteError> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else if status == reqwest::StatusCode::UNAUTHORIZED {
        Err(RemoteError::Unauthorized)
    } else {
        Err(RemoteError::Http(status.as_u16()))
    }
}

/// Client for short JSON requests.
pub(crate) fn api_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

/// Client for audio bytes: a long stream must not hit a total timeout.
pub(crate) fn stream_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::settings::AppSettings;

    fn headers() -> PlexHeaders {
        PlexHeaders {
            client_id: "client-1".into(),
            version: "9.9.9".into(),
            platform: "macos".into(),
            device_name: "Test Mac".into(),
        }
    }

    #[test]
    fn apply_sets_plex_headers() {
        let req = headers()
            .apply(reqwest::blocking::Client::new().get("http://127.0.0.1/"))
            .build()
            .unwrap();
        let h = req.headers();
        assert_eq!(h["X-Plex-Client-Identifier"], "client-1");
        assert_eq!(h["X-Plex-Product"], "Wave");
        assert_eq!(h["X-Plex-Version"], "9.9.9");
        assert_eq!(h["X-Plex-Platform"], "macos");
        assert_eq!(h["X-Plex-Device-Name"], "Test Mac");
        assert_eq!(h["Accept"], "application/json");
    }

    #[test]
    fn client_identifier_is_generated_once_and_kept() {
        let mut settings = AppSettings::default();
        assert!(settings.plex_client_identifier.is_none());
        let first = client_identifier(&mut settings);
        assert_eq!(first.len(), 36);
        assert_eq!(settings.plex_client_identifier.as_deref(), Some(&*first));
        assert_eq!(client_identifier(&mut settings), first);
    }

    /// Streams one real track from a Plex server and decodes all of it
    /// through the playback decoder, so a passing run means the bytes really
    /// play, not just that a response looked right.
    #[test]
    #[ignore = "needs WAVE_PLEX_URL, WAVE_PLEX_TOKEN and WAVE_PLEX_SECTION"]
    fn live_plex_fetch_and_decode() {
        use crate::audio::symphonia_source::SymphoniaSource;
        use crate::library::Library;
        use crate::remote::backend::RemoteBackend;
        use crate::remote::gate::NetworkGate;
        use crate::remote::plex::auth::PlexSection;
        use crate::remote::plex::connection::{PlexBackend, PlexSourceConfig};
        use crate::remote::stream::{fill_for_with, reader::ProgressiveReader};
        use crate::remote::uri::RemoteUri;
        use crate::remote::RemoteRuntime;
        use std::sync::Arc;
        use std::time::Duration;

        let (Ok(url), Ok(token), Ok(section)) = (
            std::env::var("WAVE_PLEX_URL"),
            std::env::var("WAVE_PLEX_TOKEN"),
            std::env::var("WAVE_PLEX_SECTION"),
        ) else {
            eprintln!("Skipped: set WAVE_PLEX_URL, WAVE_PLEX_TOKEN and WAVE_PLEX_SECTION");
            return;
        };

        let gate: &'static NetworkGate = Box::leak(Box::new(NetworkGate::new_for_test()));
        gate.set_policy(true, true);
        let config = PlexSourceConfig {
            server_id: "live".into(),
            server_name: "Live".into(),
            server_token: token,
            account_token: String::new(),
            sections: vec![PlexSection {
                key: section,
                title: "Music".into(),
            }],
            connections: Vec::new(),
            preferred_uri: Some(url),
            direct: true,
        };
        let backend = Arc::new(PlexBackend::with_gate(config, headers(), gate));
        let page = backend
            .catalog_page(0)
            .expect("the first catalog page loads");
        let first = page
            .tracks
            .first()
            .expect("the section has at least one track")
            .clone();
        eprintln!(
            "Streaming \"{}\" by {} ({} bytes, {})",
            first.title, first.artist, first.size, first.extension
        );

        let dir = std::env::temp_dir().join(format!("wave-live-plex-{}", uuid::Uuid::new_v4()));
        let library = Library::new_with_path(&dir.join("library.db")).unwrap();
        let source = library
            .insert_remote_source("plex", "live", "Live", "{}")
            .unwrap();
        let mut walk = library.begin_remote_walk(&source).unwrap();
        library
            .upsert_remote_page(&mut walk, "plex", std::slice::from_ref(&first))
            .unwrap();
        library.finish_remote_walk(walk).unwrap();
        let rt: &'static RemoteRuntime = Box::leak(Box::new(RemoteRuntime::new_for_test(
            library,
            Box::new(|_| {}),
        )));
        rt.insert_backend_for_test(&source, backend);

        let uri = RemoteUri::new(&source, &first.remote_id);
        let fill = fill_for_with(rt, gate, &uri).expect("the stream starts");
        let reader = ProgressiveReader::new(fill, Duration::from_secs(60)).unwrap();
        let decoder = SymphoniaSource::from_media_source(
            Box::new(reader),
            Some(&first.extension),
            &uri.to_path(),
        )
        .expect("the stream probes");
        let samples = decoder.count();
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("Decoded {samples} samples");
        assert!(samples > 0);
    }
}
