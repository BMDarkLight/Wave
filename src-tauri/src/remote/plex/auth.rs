// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! plex.tv sign-in by PIN, server discovery and music section listing.

use super::{api_client, check_status, with_server_token, PlexHeaders};
use crate::remote::backend::RemoteError;
use crate::remote::uri::percent_encode;
use serde::{Deserialize, Serialize};

const PINS_URL: &str = "https://plex.tv/api/v2/pins";
const RESOURCES_URL: &str =
    "https://clients.plex.tv/api/v2/resources?includeHttps=1&includeRelay=1";

#[derive(Debug, Clone, PartialEq)]
pub struct PlexPin {
    pub id: i64,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PlexConnection {
    pub uri: String,
    pub local: bool,
    pub relay: bool,
    pub address: String,
    pub port: u16,
    pub protocol: String,
}

/// Holds the server-scoped token, so it deliberately has no `Debug`.
#[derive(Clone, Serialize, Deserialize)]
pub struct PlexServer {
    pub client_identifier: String,
    pub name: String,
    pub owned: bool,
    pub access_token: String,
    pub connections: Vec<PlexConnection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlexSection {
    pub key: String,
    pub title: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawPin {
    id: i64,
    code: Option<String>,
    auth_token: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawResource {
    name: Option<String>,
    provides: Option<String>,
    client_identifier: Option<String>,
    owned: bool,
    access_token: Option<String>,
    connections: Vec<PlexConnection>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SectionsEnvelope {
    #[serde(rename = "MediaContainer")]
    container: SectionsContainer,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SectionsContainer {
    #[serde(rename = "Directory")]
    directory: Vec<RawSection>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawSection {
    key: Option<String>,
    title: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

fn parse_err(e: serde_json::Error) -> RemoteError {
    RemoteError::Parse(e.to_string())
}

fn plex_tv_unreachable() -> RemoteError {
    RemoteError::Unreachable("Can't reach plex.tv".to_string())
}

pub(crate) fn parse_pin(json: &str) -> Result<PlexPin, RemoteError> {
    let raw: RawPin = serde_json::from_str(json).map_err(parse_err)?;
    let code = raw
        .code
        .filter(|c| !c.is_empty())
        .ok_or_else(|| RemoteError::Parse("pin without a code".to_string()))?;
    Ok(PlexPin { id: raw.id, code })
}

pub(crate) fn parse_pin_token(json: &str) -> Result<Option<String>, RemoteError> {
    let raw: RawPin = serde_json::from_str(json).map_err(parse_err)?;
    Ok(raw.auth_token.filter(|t| !t.is_empty()))
}

pub(crate) fn parse_resources(json: &str) -> Result<Vec<PlexServer>, RemoteError> {
    let raw: Vec<RawResource> = serde_json::from_str(json).map_err(parse_err)?;
    Ok(raw
        .into_iter()
        .filter(|r| {
            r.provides
                .as_deref()
                .is_some_and(|p| p.split(',').any(|v| v.trim() == "server"))
        })
        .map(|r| PlexServer {
            client_identifier: r.client_identifier.unwrap_or_default(),
            name: r.name.unwrap_or_default(),
            owned: r.owned,
            access_token: r.access_token.unwrap_or_default(),
            connections: r.connections,
        })
        .collect())
}

pub(crate) fn parse_sections(json: &str) -> Result<Vec<PlexSection>, RemoteError> {
    let raw: SectionsEnvelope = serde_json::from_str(json).map_err(parse_err)?;
    Ok(raw
        .container
        .directory
        .into_iter()
        .filter(|d| d.kind.as_deref() == Some("artist"))
        .filter_map(|d| {
            Some(PlexSection {
                key: d.key?,
                title: d.title.unwrap_or_default(),
            })
        })
        .collect())
}

/// Starts a PIN sign-in. The user approves it in the browser at `auth_url`.
pub fn create_pin(h: &PlexHeaders) -> Result<PlexPin, RemoteError> {
    let resp = h
        .apply(api_client().post(format!("{PINS_URL}?strong=true")))
        .send()
        .map_err(|_| plex_tv_unreachable())?;
    let body = check_status(resp)?
        .text()
        .map_err(|_| plex_tv_unreachable())?;
    parse_pin(&body)
}

pub fn auth_url(h: &PlexHeaders, pin: &PlexPin) -> String {
    format!(
        "https://app.plex.tv/auth#?clientID={}&code={}&context%5Bdevice%5D%5Bproduct%5D=Wave",
        percent_encode(&h.client_id),
        percent_encode(&pin.code)
    )
}

/// `Ok(None)` while the user has not approved yet, `Err(Unauthorized)` once
/// the PIN has expired.
pub fn poll_pin(h: &PlexHeaders, id: i64) -> Result<Option<String>, RemoteError> {
    let resp = h
        .apply(api_client().get(format!("{PINS_URL}/{id}")))
        .send()
        .map_err(|_| plex_tv_unreachable())?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(RemoteError::Unauthorized);
    }
    let body = check_status(resp)?
        .text()
        .map_err(|_| plex_tv_unreachable())?;
    parse_pin_token(&body)
}

pub fn list_servers(h: &PlexHeaders, account_token: &str) -> Result<Vec<PlexServer>, RemoteError> {
    let resp = h
        .apply(api_client().get(RESOURCES_URL))
        .header("X-Plex-Token", account_token)
        .send()
        .map_err(|_| plex_tv_unreachable())?;
    let body = check_status(resp)?
        .text()
        .map_err(|_| plex_tv_unreachable())?;
    parse_resources(&body)
}

pub fn list_music_sections(
    conn_uri: &str,
    server_token: &str,
    h: &PlexHeaders,
) -> Result<Vec<PlexSection>, RemoteError> {
    let resp = with_server_token(
        h.apply(api_client().get(format!("{conn_uri}/library/sections"))),
        server_token,
    )
    .send()
    .map_err(|_| RemoteError::Unreachable("Can't reach the Plex server".to_string()))?;
    let body = check_status(resp)?
        .text()
        .map_err(|_| RemoteError::Unreachable("Can't reach the Plex server".to_string()))?;
    parse_sections(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers() -> PlexHeaders {
        PlexHeaders {
            client_id: "client-1".into(),
            version: "1.0.0".into(),
            platform: "macos".into(),
            device_name: "Mac".into(),
        }
    }

    #[test]
    fn parses_resources_keeps_servers_only() {
        let json = r#"[
          {
            "name": "Home Server",
            "provides": "server",
            "clientIdentifier": "srv-1",
            "owned": true,
            "accessToken": "server-token",
            "connections": [
              { "protocol": "https", "address": "192.168.1.5", "port": 32400,
                "uri": "https://192-168-1-5.abc.plex.direct:32400", "local": true, "relay": false },
              { "protocol": "https", "address": "203.0.113.9", "port": 32400,
                "uri": "https://203-0-113-9.abc.plex.direct:32400", "local": false, "relay": false },
              { "protocol": "https", "address": "10.0.0.1", "port": 8443,
                "uri": "https://10-0-0-1.abc.plex.direct:8443", "local": false, "relay": true }
            ]
          },
          {
            "name": "Living Room TV",
            "provides": "client,player",
            "clientIdentifier": "tv-1",
            "owned": true,
            "accessToken": null,
            "connections": []
          }
        ]"#;
        let servers = parse_resources(json).unwrap();
        assert_eq!(servers.len(), 1);
        let s = &servers[0];
        assert_eq!(s.client_identifier, "srv-1");
        assert_eq!(s.name, "Home Server");
        assert!(s.owned);
        assert_eq!(s.access_token, "server-token");
        assert_eq!(s.connections.len(), 3);
        assert!(s.connections[0].local);
        assert!(s.connections[2].relay);
        assert_eq!(s.connections[0].port, 32400);
    }

    #[test]
    fn parses_music_sections_only() {
        let json = r#"{"MediaContainer":{"Directory":[
          {"key":"1","title":"Movies","type":"movie"},
          {"key":"3","title":"Music","type":"artist"}
        ]}}"#;
        let sections = parse_sections(json).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].key, "3");
        assert_eq!(sections[0].title, "Music");
    }

    #[test]
    fn auth_url_carries_client_and_code() {
        let pin = PlexPin {
            id: 7,
            code: "ABCD".into(),
        };
        assert_eq!(
            auth_url(&headers(), &pin),
            "https://app.plex.tv/auth#?clientID=client-1&code=ABCD&context%5Bdevice%5D%5Bproduct%5D=Wave"
        );
    }

    #[test]
    fn poll_pin_reads_auth_token_or_none() {
        assert_eq!(
            parse_pin_token(r#"{"id":7,"authToken":null}"#).unwrap(),
            None
        );
        assert_eq!(parse_pin_token(r#"{"id":7,"authToken":""}"#).unwrap(), None);
        assert_eq!(
            parse_pin_token(r#"{"id":7,"authToken":"abc"}"#).unwrap(),
            Some("abc".to_string())
        );
    }

    #[test]
    fn parses_created_pin() {
        let pin = parse_pin(r#"{"id":42,"code":"WXYZ","authToken":null}"#).unwrap();
        assert_eq!(pin.id, 42);
        assert_eq!(pin.code, "WXYZ");
    }
}
