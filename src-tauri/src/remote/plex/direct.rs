// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Connecting to a Plex server by its address, without plex.tv: for networks
//! where plex.tv can't be reached, or servers that allow the local network in
//! without signing in.

use super::auth::{list_music_sections, PlexConnection, PlexSection};
use super::connection::PlexSourceConfig;
use super::{api_client, check_status, with_server_token, PlexHeaders};
use crate::remote::backend::RemoteError;
use serde::Deserialize;

/// Plex's port when an address doesn't name one.
const DEFAULT_PORT: u16 = 32400;

/// A server found at an address, before it is added.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectProbe {
    pub server_id: String,
    pub name: String,
    pub sections: Vec<PlexSection>,
}

#[derive(Deserialize)]
struct Container<T> {
    #[serde(rename = "MediaContainer")]
    media_container: T,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ServerInfo {
    machine_identifier: String,
    friendly_name: String,
}

/// The address as typed, made usable: `192.168.1.10` becomes
/// `http://192.168.1.10:32400`. An address with a scheme is kept as given,
/// since it may sit behind a reverse proxy on another port.
pub fn normalize_server_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Enter the server's address".to_string());
    }
    if let Some((scheme, rest)) = trimmed.split_once("://") {
        if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
            return Err("Use an http:// or https:// address".to_string());
        }
        if rest.is_empty() {
            return Err("Enter the server's address".to_string());
        }
        return Ok(trimmed.to_string());
    }
    if host_and_port(trimmed).1.is_some() {
        Ok(format!("http://{trimmed}"))
    } else {
        Ok(format!("http://{trimmed}:{DEFAULT_PORT}"))
    }
}

/// `host[:port]` split at its port, IPv6 brackets respected.
fn host_and_port(authority: &str) -> (&str, Option<u16>) {
    let colon = if authority.starts_with('[') {
        authority.find("]:").map(|i| i + 1)
    } else {
        authority.rfind(':')
    };
    match colon {
        Some(i) => match authority[i + 1..].parse() {
            Ok(port) => (&authority[..i], Some(port)),
            Err(_) => (authority, None),
        },
        None => (authority, None),
    }
}

/// The `host[:port]` part of a URL, for naming a server that won't say its
/// own name.
fn authority(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
}

fn get(url: &str, token: &str, headers: &PlexHeaders) -> Result<String, RemoteError> {
    let resp = with_server_token(headers.apply(api_client().get(url)), token)
        .send()
        .map_err(|_| RemoteError::Unreachable("Can't reach the Plex server".to_string()))?;
    check_status(resp)?
        .text()
        .map_err(|_| RemoteError::Unreachable("Can't reach the Plex server".to_string()))
}

fn server_info(body: &str) -> Result<ServerInfo, RemoteError> {
    serde_json::from_str::<Container<ServerInfo>>(body)
        .map(|c| c.media_container)
        .map_err(|e| RemoteError::Parse(e.to_string()))
}

/// Checks the server at `url` and lists its music libraries. `token` may be
/// empty for a server that lets this network in without signing in.
pub fn probe_direct(
    url: &str,
    token: &str,
    headers: &PlexHeaders,
) -> Result<DirectProbe, RemoteError> {
    // `/identity` answers without a token on every server.
    let identity = server_info(&get(&format!("{url}/identity"), token, headers)?)?;
    if identity.machine_identifier.is_empty() {
        return Err(RemoteError::Parse("No server id".to_string()));
    }
    // The name needs the same access as the libraries; a server that hides it
    // is named by its address.
    let name = get(&format!("{url}/"), token, headers)
        .ok()
        .and_then(|body| server_info(&body).ok())
        .map(|info| info.friendly_name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| authority(url).to_string());
    let sections = list_music_sections(url, token, headers)?;
    Ok(DirectProbe {
        server_id: identity.machine_identifier,
        name,
        sections,
    })
}

/// The stored config for a directly connected server.
pub fn direct_config(
    url: &str,
    token: &str,
    probe: &DirectProbe,
    library_keys: &[String],
) -> PlexSourceConfig {
    let (address, port) = host_and_port(authority(url));
    let https = url.to_ascii_lowercase().starts_with("https://");
    PlexSourceConfig {
        server_id: probe.server_id.clone(),
        server_name: probe.name.clone(),
        server_token: token.to_string(),
        account_token: String::new(),
        sections: probe
            .sections
            .iter()
            .filter(|section| library_keys.contains(&section.key))
            .cloned()
            .collect(),
        // Not marked local, so no plain-HTTP twin is guessed for it: the
        // address is exactly what the user gave.
        connections: vec![PlexConnection {
            uri: url.to_string(),
            local: false,
            relay: false,
            address: address.trim_matches(['[', ']']).to_string(),
            port: port.unwrap_or(if https { 443 } else { 80 }),
            protocol: if https { "https" } else { "http" }.to_string(),
        }],
        preferred_uri: Some(url.to_string()),
        direct: true,
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{fake_server, http};
    use super::*;

    fn headers() -> PlexHeaders {
        PlexHeaders {
            client_id: "client-1".into(),
            version: "1.0.0".into(),
            platform: "macos".into(),
            device_name: "Mac".into(),
        }
    }

    const IDENTITY: &str = r#"{"MediaContainer":{"size":0,"claimed":true,"machineIdentifier":"abc123","version":"1.40.1"}}"#;
    const ROOT: &str =
        r#"{"MediaContainer":{"friendlyName":"Living Room","machineIdentifier":"abc123"}}"#;
    const SECTIONS: &str = r#"{"MediaContainer":{"Directory":[
        {"key":"3","title":"Music","type":"artist"},
        {"key":"4","title":"Movies","type":"movie"},
        {"key":"7","title":"Live Sets","type":"artist"}]}}"#;

    #[test]
    fn a_bare_address_gets_http_and_the_plex_port() {
        assert_eq!(
            normalize_server_url("192.168.1.10").unwrap(),
            "http://192.168.1.10:32400"
        );
        assert_eq!(
            normalize_server_url(" nas.local:32401 ").unwrap(),
            "http://nas.local:32401"
        );
    }

    #[test]
    fn an_address_with_a_scheme_is_kept_without_the_trailing_slash() {
        assert_eq!(
            normalize_server_url("http://nas:32400/").unwrap(),
            "http://nas:32400"
        );
        assert_eq!(
            normalize_server_url("https://plex.example.com/").unwrap(),
            "https://plex.example.com"
        );
    }

    #[test]
    fn empty_or_unsupported_addresses_are_refused() {
        assert!(normalize_server_url("   ").is_err());
        assert!(normalize_server_url("ftp://nas").is_err());
    }

    #[test]
    fn probe_reads_the_id_the_name_and_only_music_libraries() {
        let (port, seen) = fake_server(vec![
            http("200 OK", IDENTITY),
            http("200 OK", ROOT),
            http("200 OK", SECTIONS),
        ]);
        let url = format!("http://127.0.0.1:{port}");
        let probe = probe_direct(&url, "tok-1", &headers()).unwrap();
        assert_eq!(probe.server_id, "abc123");
        assert_eq!(probe.name, "Living Room");
        assert_eq!(
            probe
                .sections
                .iter()
                .map(|s| s.key.as_str())
                .collect::<Vec<_>>(),
            vec!["3", "7"]
        );
        let requests = seen.lock().unwrap().join("\n").to_ascii_lowercase();
        assert!(requests.contains("x-plex-token: tok-1"), "{requests}");
    }

    #[test]
    fn probe_without_a_token_sends_none() {
        let (port, seen) = fake_server(vec![
            http("200 OK", IDENTITY),
            http("200 OK", ROOT),
            http("200 OK", SECTIONS),
        ]);
        probe_direct(&format!("http://127.0.0.1:{port}"), "", &headers()).unwrap();
        let requests = seen.lock().unwrap().join("\n").to_ascii_lowercase();
        assert!(!requests.contains("x-plex-token"), "{requests}");
    }

    #[test]
    fn a_server_that_hides_its_name_is_named_by_its_address() {
        let (port, _) = fake_server(vec![
            http("200 OK", IDENTITY),
            http("401 Unauthorized", ""),
            http("200 OK", SECTIONS),
        ]);
        let probe = probe_direct(&format!("http://127.0.0.1:{port}"), "", &headers()).unwrap();
        assert_eq!(probe.name, format!("127.0.0.1:{port}"));
    }

    #[test]
    fn a_server_that_wants_a_token_says_so() {
        let (port, _) = fake_server(vec![
            http("200 OK", IDENTITY),
            http("401 Unauthorized", ""),
            http("401 Unauthorized", ""),
        ]);
        let err = probe_direct(&format!("http://127.0.0.1:{port}"), "", &headers()).unwrap_err();
        assert_eq!(err, RemoteError::Unauthorized);
    }

    #[test]
    fn nothing_listening_is_unreachable() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let err = probe_direct(&format!("http://127.0.0.1:{port}"), "", &headers()).unwrap_err();
        assert!(matches!(err, RemoteError::Unreachable(_)), "{err:?}");
    }

    #[test]
    fn the_config_keeps_the_address_the_token_and_the_chosen_libraries() {
        let probe = DirectProbe {
            server_id: "abc123".into(),
            name: "Living Room".into(),
            sections: vec![
                PlexSection {
                    key: "3".into(),
                    title: "Music".into(),
                },
                PlexSection {
                    key: "7".into(),
                    title: "Live Sets".into(),
                },
            ],
        };
        let config = direct_config("http://nas:32400", "tok", &probe, &["7".to_string()]);
        assert!(config.direct);
        assert_eq!(config.server_id, "abc123");
        assert_eq!(config.server_name, "Living Room");
        assert_eq!(config.server_token, "tok");
        assert_eq!(config.account_token, "");
        assert_eq!(config.preferred_uri.as_deref(), Some("http://nas:32400"));
        assert_eq!(config.sections.len(), 1);
        assert_eq!(config.sections[0].key, "7");
        assert_eq!(config.connections.len(), 1);
        assert_eq!(config.connections[0].uri, "http://nas:32400");
        assert!(!config.connections[0].local && !config.connections[0].relay);
    }
}
