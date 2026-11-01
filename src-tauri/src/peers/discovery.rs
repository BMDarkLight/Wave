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
//! Waves find each other with Radii's LAN beacon: each broadcasts a hello
//! with the `wave` role and the port it serves on, and answers the hellos
//! it hears. Broadcast gets through where multicast often doesn't, on a
//! phone's hotspot say, and the answer means hearing one way is enough. A
//! hello only says where to look: every Wave is asked who it is over HTTP
//! before it is listed. Every device already known is also asked directly
//! now and then, which finds it even where broadcasts don't get through.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use radii_beacon::{Beacon, BeaconConfig, DEFAULT_PORT};
use radii_proto::ListenAddr;

use super::api::PeerService;
use super::client;

/// The role every Wave announces in its hello.
const ROLE: &str = "wave";

/// When announcements go out after start, or after a search asked for,
/// each after the one before; then how often after that.
const FIRST_SEARCHES: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_secs(2),
    Duration::from_secs(4),
];
const SEARCH_EVERY: Duration = Duration::from_secs(60);
/// How often known devices are asked directly.
const CHECK_EVERY: Duration = Duration::from_secs(30);
/// A device not heard from for this long counts as gone.
const GONE_AFTER: Duration = Duration::from_secs(95);
/// An address is not probed again sooner than this.
const PROBE_GAP: Duration = Duration::from_secs(20);
/// How long one wait for a hello lasts before the loop looks around again.
const LISTEN: Duration = Duration::from_millis(250);

/// Set when someone asks to look again now rather than at the next round.
static SEARCH_NOW: AtomicBool = AtomicBool::new(false);

/// Hosts to send a hello to directly, as soon as discovery gets to it.
fn knocks() -> &'static Mutex<Vec<Ipv4Addr>> {
    static KNOCKS: Mutex<Vec<Ipv4Addr>> = Mutex::new(Vec::new());
    &KNOCKS
}

/// Announces this Wave and asks every known device again, as soon as the
/// running discovery gets to it.
pub fn search_now() {
    SEARCH_NOW.store(true, Ordering::Relaxed);
}

/// Sends this Wave's hello straight to `host`, so a Wave there finds this
/// one even when broadcasts don't reach it.
pub fn knock(host: Ipv4Addr) {
    let mut pending = knocks().lock();
    if !pending.contains(&host) {
        pending.push(host);
    }
}

/// The base URL of a Wave from an address typed in by hand: an IPv4
/// address on a local network, with or without a port, `http://` and a
/// path. Wave's usual port is assumed when none is given.
pub fn base_for_typed(typed: &str) -> Result<(String, Ipv4Addr), String> {
    let typed = typed.trim();
    let rest = typed.strip_prefix("http://").unwrap_or(typed);
    let host = rest.split('/').next().unwrap_or_default();
    let at = match host.parse::<SocketAddrV4>() {
        Ok(at) => at,
        Err(_) => SocketAddrV4::new(
            host.parse::<Ipv4Addr>()
                .map_err(|_| format!("\"{typed}\" isn't an address like 192.168.1.20"))?,
            crate::share::PREFERRED_PORT,
        ),
    };
    if !crate::share::http::is_local(IpAddr::V4(*at.ip())) || at.ip().is_unspecified() {
        return Err(format!("{} isn't on a local network", at.ip()));
    }
    Ok((format!("http://{at}"), *at.ip()))
}

/// Where broadcasts go besides 255.255.255.255, which only leaves by the
/// default route: each local network's own broadcast address.
fn broadcast_addresses() -> Vec<Ipv4Addr> {
    let mut found: Vec<Ipv4Addr> = Vec::new();
    #[cfg(unix)]
    if let Ok(interfaces) = nix::ifaddrs::getifaddrs() {
        use nix::net::if_::InterfaceFlags;
        for interface in interfaces {
            if !interface.flags.contains(InterfaceFlags::IFF_UP)
                || interface.flags.contains(InterfaceFlags::IFF_LOOPBACK)
            {
                continue;
            }
            let own = interface
                .address
                .as_ref()
                .and_then(|a| a.as_sockaddr_in())
                .map(|a| a.ip());
            let broadcast = interface
                .broadcast
                .as_ref()
                .and_then(|a| a.as_sockaddr_in())
                .map(|a| a.ip());
            if let (Some(own), Some(broadcast)) = (own, broadcast) {
                if crate::share::http::is_local(IpAddr::V4(own)) && !broadcast.is_unspecified() {
                    found.push(broadcast);
                }
            }
        }
    }
    // Without the netmasks, each address's /24, the usual home network.
    #[cfg(not(unix))]
    for ip in crate::remote::dlna::multicast_interfaces() {
        if !ip.is_unspecified() && !ip.is_loopback() && crate::share::http::is_local(IpAddr::V4(ip))
        {
            let [a, b, c, _] = ip.octets();
            found.push(Ipv4Addr::new(a, b, c, 255));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Announces, listens and checks until `stop` is set. `port` is where this
/// Wave's API answers. A beacon that can't start is reported in `error`;
/// known devices are still checked without it.
pub fn run(
    service: Arc<PeerService>,
    port: u16,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
) {
    let (own_id, _) = service.host().identity();
    let mut beacon = match Beacon::bind(BeaconConfig {
        node_id: own_id,
        roles: vec![ROLE.into()],
        listen_addrs: vec![ListenAddr {
            addr: SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).to_string(),
            role: ROLE.into(),
        }],
        port: DEFAULT_PORT,
    }) {
        Ok(beacon) => Some(beacon),
        Err(e) => {
            tracing::warn!("Wave can't announce itself on the network: {e}");
            *error.lock() = Some(format!(
                "Other Waves can't find this one on their own ({e})"
            ));
            None
        }
    };
    let started = Instant::now();
    let mut searched = 0;
    let mut last_search = started;
    let mut last_check = started;
    let mut probed: HashMap<String, Instant> = HashMap::new();

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
        let asked = SEARCH_NOW.swap(false, Ordering::Relaxed);
        if asked {
            // A few announcements close together, as at start, and every
            // known device asked again straight away.
            searched = 0;
            probed.clear();
        }
        match &mut beacon {
            Some(beacon) => {
                let due = asked
                    || match FIRST_SEARCHES.get(searched) {
                        Some(after) => searched == 0 || last_search.elapsed() >= *after,
                        None => last_search.elapsed() >= SEARCH_EVERY,
                    };
                if due {
                    if let Err(e) = beacon.announce(&broadcast_addresses()) {
                        tracing::debug!("Couldn't announce this Wave: {e}");
                    }
                    searched += 1;
                    last_search = Instant::now();
                }
                for host in std::mem::take(&mut *knocks().lock()) {
                    let _ = beacon.announce_at(SocketAddrV4::new(host, DEFAULT_PORT).into());
                }
                match beacon.recv(LISTEN) {
                    Ok(Some(seen)) if seen.has_role(ROLE) => {
                        if let Some(at) = seen.listener(ROLE) {
                            if crate::share::http::is_local(at.ip()) {
                                probe(&format!("http://{at}"), &mut probed);
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::debug!("Beacon read failed: {e}");
                        std::thread::sleep(LISTEN);
                    }
                }
            }
            None => std::thread::sleep(LISTEN),
        }
        if asked || last_check.elapsed() >= CHECK_EVERY {
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
    fn typed_addresses_are_read_with_or_without_a_port() {
        let port = crate::share::PREFERRED_PORT;
        assert_eq!(
            base_for_typed(" 192.168.1.20 ").unwrap(),
            (
                format!("http://192.168.1.20:{port}"),
                Ipv4Addr::new(192, 168, 1, 20)
            )
        );
        assert_eq!(
            base_for_typed("http://10.0.0.7:41000/wave/hello")
                .unwrap()
                .0,
            "http://10.0.0.7:41000"
        );
        assert!(base_for_typed("phone.local").is_err());
        assert!(base_for_typed("8.8.8.8").is_err());
        assert!(base_for_typed("0.0.0.0").is_err());
        assert!(base_for_typed("").is_err());
    }

    #[test]
    fn a_host_is_knocked_on_once() {
        let host = Ipv4Addr::new(192, 0, 2, 44);
        knock(host);
        knock(host);
        let pending = std::mem::take(&mut *knocks().lock());
        assert_eq!(pending.iter().filter(|h| **h == host).count(), 1);
    }
}
