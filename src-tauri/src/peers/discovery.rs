// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Finding other Waves. Runs alongside the share server, so it starts and
//! stops with Outside Sourcing.
//!
//! Multicast is the weak spot on most networks: a VPN takes the default
//! route, Wi-Fi drops packets, phones sleep. So searches go out on every
//! interface and are repeated, announcements other Waves make are listened
//! for too, and every device already known is asked directly now and then,
//! which finds it even where multicast does not get through at all.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::api::PeerService;
use super::client;

/// The SSDP type every Wave announces and answers to.
pub const WAVE_DEVICE: &str = "urn:bmdarklight-app:device:Wave:1";

/// When searches go out after start, then how often after that.
const FIRST_SEARCHES: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_secs(2),
    Duration::from_secs(6),
];
const SEARCH_EVERY: Duration = Duration::from_secs(120);
/// How often known devices are asked directly.
const CHECK_EVERY: Duration = Duration::from_secs(30);
/// A device not heard from for this long counts as gone.
const GONE_AFTER: Duration = Duration::from_secs(95);
/// A location is not probed again sooner than this.
const PROBE_GAP: Duration = Duration::from_secs(20);

const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);

/// The LOCATION of a Wave in a search reply or an `ssdp:alive`, if the
/// message is about one.
pub fn wave_location(message: &str) -> Option<String> {
    let mut lines = message.split("\r\n");
    let first = lines.next()?.trim();
    let reply = first.starts_with("HTTP/1.1 200");
    let notify = first.eq_ignore_ascii_case("NOTIFY * HTTP/1.1");
    if !reply && !notify {
        return None;
    }
    let mut kind = None;
    let mut alive = reply;
    let mut location = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_uppercase().as_str() {
            "ST" | "NT" => kind = Some(value.to_string()),
            "NTS" => alive = value.eq_ignore_ascii_case("ssdp:alive"),
            "LOCATION" => location = Some(value.to_string()),
            _ => {}
        }
    }
    (kind.as_deref() == Some(WAVE_DEVICE) && alive)
        .then_some(location)
        .flatten()
        .filter(|l| l.starts_with("http://"))
}

/// `http://host:port` from a location URL.
pub fn base_of(location: &str) -> Option<String> {
    let rest = location.strip_prefix("http://")?;
    let host = rest.split('/').next()?;
    (!host.is_empty()).then(|| format!("http://{host}"))
}

fn search_message() -> String {
    format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: {GROUP}:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: {WAVE_DEVICE}\r\n\r\n"
    )
}

fn search(socket: &socket2::Socket) {
    let target: SocketAddr = (GROUP, 1900).into();
    let message = search_message();
    for interface in crate::remote::dlna::multicast_interfaces() {
        if !interface.is_unspecified() && socket.set_multicast_if_v4(&interface).is_err() {
            continue;
        }
        // Twice, since UDP may drop one.
        for _ in 0..2 {
            let _ = socket.send_to(message.as_bytes(), &target.into());
        }
    }
}

fn search_socket() -> std::io::Result<socket2::Socket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)).into())?;
    let _ = socket.set_multicast_ttl_v4(2);
    Ok(socket)
}

/// Searches, listens and checks until `stop` is set. `heard` brings the
/// locations of Waves the SSDP listener heard announce themselves.
pub fn run(service: Arc<PeerService>, stop: Arc<AtomicBool>, heard: Receiver<String>) {
    let socket = match search_socket() {
        Ok(socket) => Some(socket),
        Err(e) => {
            tracing::warn!("Can't search for other Waves: {e}");
            None
        }
    };
    let replies: Option<UdpSocket> = socket
        .as_ref()
        .and_then(|s| s.try_clone().ok())
        .map(Into::into);
    if let Some(replies) = &replies {
        let _ = replies.set_read_timeout(Some(Duration::from_millis(250)));
    }
    let started = Instant::now();
    let mut searched = 0;
    let mut last_search = started;
    let mut last_check = started;
    let mut probed: HashMap<String, Instant> = HashMap::new();
    let mut buf = [0u8; 2048];

    let probe = |base: &str, probed: &mut HashMap<String, Instant>| {
        if probed.get(base).is_some_and(|at| at.elapsed() < PROBE_GAP) {
            return;
        }
        probed.insert(base.to_string(), Instant::now());
        if let Ok(hello) = client::hello(base) {
            service.sighted(&hello.id, &hello.name, base);
        }
    };

    while !stop.load(Ordering::Relaxed) {
        if let Some(socket) = &socket {
            let due = match FIRST_SEARCHES.get(searched) {
                Some(after) => started.elapsed() >= *after,
                None => last_search.elapsed() >= SEARCH_EVERY,
            };
            if due {
                search(socket);
                searched += 1;
                last_search = Instant::now();
            }
        }
        match &replies {
            Some(replies) => {
                if let Ok((n, _)) = replies.recv_from(&mut buf) {
                    if let Some(base) = wave_location(&String::from_utf8_lossy(&buf[..n]))
                        .as_deref()
                        .and_then(base_of)
                    {
                        probe(&base, &mut probed);
                    }
                }
            }
            None => std::thread::sleep(Duration::from_millis(250)),
        }
        while let Ok(location) = heard.try_recv() {
            if let Some(base) = base_of(&location) {
                probe(&base, &mut probed);
            }
        }
        if last_check.elapsed() >= CHECK_EVERY {
            last_check = Instant::now();
            // Every device known so far, and paired ones at their last
            // address, asked directly.
            let mut bases: Vec<String> = service
                .registry
                .lock()
                .found
                .values()
                .map(|f| f.address.clone())
                .collect();
            bases.extend(service.host().peers().into_iter().map(|p| p.address));
            bases.sort();
            bases.dedup();
            for base in bases {
                probed.remove(&base);
                probe(&base, &mut probed);
            }
            service.expire(GONE_AFTER);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_replies_and_announcements_are_picked_out() {
        let reply = format!(
            "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nEXT:\r\nLOCATION: http://10.0.0.7:50800/wave/hello\r\nST: {WAVE_DEVICE}\r\nUSN: uuid:x::{WAVE_DEVICE}\r\n\r\n"
        );
        assert_eq!(
            wave_location(&reply).as_deref(),
            Some("http://10.0.0.7:50800/wave/hello")
        );
        let alive = format!(
            "NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nNT: {WAVE_DEVICE}\r\nNTS: ssdp:alive\r\nLOCATION: http://10.0.0.8:41000/wave/hello\r\n\r\n"
        );
        assert_eq!(
            wave_location(&alive).as_deref(),
            Some("http://10.0.0.8:41000/wave/hello")
        );
    }

    #[test]
    fn other_devices_and_goodbyes_are_ignored() {
        let tv = "HTTP/1.1 200 OK\r\nLOCATION: http://10.0.0.9/desc.xml\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";
        assert!(wave_location(tv).is_none());
        let bye = format!("NOTIFY * HTTP/1.1\r\nNT: {WAVE_DEVICE}\r\nNTS: ssdp:byebye\r\nLOCATION: http://10.0.0.8:1/wave/hello\r\n\r\n");
        assert!(wave_location(&bye).is_none());
        let search =
            format!("M-SEARCH * HTTP/1.1\r\nST: {WAVE_DEVICE}\r\nMAN: \"ssdp:discover\"\r\n\r\n");
        assert!(wave_location(&search).is_none());
    }

    #[test]
    fn the_base_address_is_kept() {
        assert_eq!(
            base_of("http://10.0.0.7:50800/wave/hello").as_deref(),
            Some("http://10.0.0.7:50800")
        );
        assert_eq!(base_of("https://x/y"), None);
        assert_eq!(base_of("http:///y"), None);
    }
}
