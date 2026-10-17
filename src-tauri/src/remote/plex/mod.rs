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

#[allow(dead_code)]
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
#[allow(dead_code)]
pub fn client_identifier(settings: &mut AppSettings) -> String {
    settings
        .plex_client_identifier
        .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
        .clone()
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
}
