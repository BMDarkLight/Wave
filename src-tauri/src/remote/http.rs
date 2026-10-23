// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! The HTTP clients every remote backend shares.

use reqwest::blocking::Client;
use std::sync::OnceLock;
use std::time::Duration;

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

/// Whether a password sent as Basic auth (readable by anyone on the path) is
/// acceptable for `server_url`: always over HTTPS, and over plain HTTP only to
/// this machine or an address that can only be on the local network. A NAS on
/// the LAN rarely has a certificate, while plain HTTP across the internet would
/// hand the password to every hop.
pub(crate) fn basic_auth_travels_safely(server_url: &str) -> bool {
    let Ok(url) = url::Url::parse(server_url) else {
        return false;
    };
    if url.scheme() == "https" {
        return true;
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Some(url::Host::Ipv6(ip)) => {
            let first = ip.segments()[0];
            // Unique local fc00::/7 and link-local fe80::/10.
            ip.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost"
                || !name.contains('.')
                || [".local", ".lan", ".home", ".home.arpa", ".internal"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
        }
        None => false,
    }
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
    use super::basic_auth_travels_safely as safe;

    #[test]
    fn basic_auth_only_leaves_the_local_network_over_https() {
        assert!(safe("https://cloud.example.com/remote.php/dav"));
        assert!(safe("http://192.168.1.20:5005"));
        assert!(safe("http://10.0.0.4/dav"));
        assert!(safe("http://127.0.0.1:8080"));
        assert!(safe("http://[fd12:3456::1]:8080"));
        assert!(safe("http://nas:5005"));
        assert!(safe("http://diskstation.local:5005"));
        assert!(!safe("http://cloud.example.com/dav"));
        assert!(!safe("http://203.0.113.9/dav"));
        assert!(!safe("not a url"));
    }
}
