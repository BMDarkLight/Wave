// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! How the media server is found: SSDP, from section 1 of the UPnP Device
//! Architecture.
//!
//! On start the server announces itself (`ssdp:alive`) once for each thing it
//! is (the root device, its UUID, a MediaServer, each service) on every
//! network interface, and again well before the announcement expires. It
//! answers an `M-SEARCH` for any of those with a unicast reply whose LOCATION
//! uses the address the searcher can reach. On stop it says `ssdp:byebye`.

use super::http::{CONNECTION_MANAGER, CONTENT_DIRECTORY, MEDIA_SERVER};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const PORT: u16 = 1900;
/// How long an announcement is valid; the spec asks for at least 1800.
const MAX_AGE: u64 = 1800;
/// Announce again well inside `MAX_AGE`, as the spec recommends.
const REANNOUNCE: Duration = Duration::from_secs(600);

/// One thing the server announces: its notification type, USN, and the
/// path its LOCATION points at.
pub type Target = (String, String, &'static str);

/// The notification types and their USNs, per Device Architecture 1.1.2.
/// Other Waves look for the Wave type, which is announced either way; the
/// media server set only while DLNA players are served.
pub fn targets(uuid: &str, dlna: bool) -> Vec<Target> {
    let device = format!("uuid:{uuid}");
    let wave = crate::peers::discovery::WAVE_DEVICE;
    let mut out = vec![(wave.to_string(), format!("{device}::{wave}"), "/wave/hello")];
    if !dlna {
        return out;
    }
    out.push((
        "upnp:rootdevice".to_string(),
        format!("{device}::upnp:rootdevice"),
        "/rootDesc.xml",
    ));
    out.push((device.clone(), device.clone(), "/rootDesc.xml"));
    for kind in [MEDIA_SERVER, CONTENT_DIRECTORY, CONNECTION_MANAGER] {
        out.push((
            kind.to_string(),
            format!("{device}::{kind}"),
            "/rootDesc.xml",
        ));
    }
    out
}

fn server_token() -> String {
    format!(
        "{}/1 UPnP/1.0 Wave/{}",
        std::env::consts::OS,
        env!("CARGO_PKG_VERSION")
    )
}

fn location(ip: Ipv4Addr, http_port: u16, path: &str) -> String {
    format!("http://{ip}:{http_port}{path}")
}

pub fn alive(nt: &str, usn: &str, location: &str) -> String {
    format!(
        "NOTIFY * HTTP/1.1\r\nHOST: {GROUP}:{PORT}\r\nCACHE-CONTROL: max-age={MAX_AGE}\r\nLOCATION: {location}\r\nNT: {nt}\r\nNTS: ssdp:alive\r\nSERVER: {}\r\nUSN: {usn}\r\n\r\n",
        server_token()
    )
}

pub fn byebye(nt: &str, usn: &str) -> String {
    format!("NOTIFY * HTTP/1.1\r\nHOST: {GROUP}:{PORT}\r\nNT: {nt}\r\nNTS: ssdp:byebye\r\nUSN: {usn}\r\n\r\n")
}

pub fn search_reply(st: &str, usn: &str, location: &str) -> String {
    let date = httpdate();
    format!(
        "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age={MAX_AGE}\r\nDATE: {date}\r\nEXT:\r\nLOCATION: {location}\r\nSERVER: {}\r\nST: {st}\r\nUSN: {usn}\r\n\r\n",
        server_token()
    )
}

/// The current time as an RFC 1123 date.
fn httpdate() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days, after Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[days.rem_euclid(7) as usize],
        MONTHS[(month - 1) as usize],
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// What an `M-SEARCH` asks for: its ST and the most seconds it will wait.
pub fn parse_search(message: &str) -> Option<(String, u64)> {
    let mut lines = message.lines();
    if !lines
        .next()?
        .trim()
        .eq_ignore_ascii_case("M-SEARCH * HTTP/1.1")
    {
        return None;
    }
    let mut st = None;
    let mut mx = 1;
    let mut man = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_uppercase().as_str() {
            "ST" => st = Some(value.to_string()),
            "MX" => mx = value.parse().unwrap_or(1),
            "MAN" => man = value.trim_matches('"') == "ssdp:discover",
            _ => {}
        }
    }
    man.then_some(())?;
    Some((st?, mx))
}

/// The replies a search for `st` should get, one per matching target.
pub fn matching<'a>(st: &str, targets: &'a [Target]) -> Vec<&'a Target> {
    if st == "ssdp:all" {
        return targets.iter().collect();
    }
    targets.iter().filter(|(nt, _, _)| nt == st).collect()
}

/// The local address a packet to `peer` would leave from.
fn local_address_for(peer: SocketAddr) -> Option<Ipv4Addr> {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    probe.connect(peer).ok()?;
    match probe.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

fn interfaces() -> Vec<Ipv4Addr> {
    crate::remote::dlna::multicast_interfaces()
        .into_iter()
        .filter(|ip| !ip.is_unspecified())
        .collect()
}

/// Sends each target's message on every interface, with that interface's
/// address in LOCATION.
fn announce(socket: &socket2::Socket, targets: &[Target], http_port: u16, leaving: bool) {
    let group: SocketAddr = (GROUP, PORT).into();
    for interface in interfaces() {
        if socket.set_multicast_if_v4(&interface).is_err() {
            continue;
        }
        for (nt, usn, path) in targets {
            let message = if leaving {
                byebye(nt, usn)
            } else {
                alive(nt, usn, &location(interface, http_port, path))
            };
            let _ = socket.send_to(message.as_bytes(), &group.into());
        }
    }
}

/// Opens the shared SSDP port: other services on this machine (another DLNA
/// server, the OS) may hold it too, so the address is shared.
fn bind() -> std::io::Result<socket2::Socket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_reuse_address(true)?;
    #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)).into())?;
    let mut joined = false;
    for interface in interfaces() {
        joined |= socket.join_multicast_v4(&GROUP, &interface).is_ok();
    }
    if !joined {
        socket.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
    }
    let _ = socket.set_multicast_ttl_v4(2);
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    Ok(socket)
}

/// Announces and answers until `stop` is set, then says goodbye. Other
/// Waves' announcements heard on the way go to `heard`.
pub fn run(
    uuid: String,
    http_port: u16,
    dlna: bool,
    stop: Arc<AtomicBool>,
    heard: Option<std::sync::mpsc::Sender<String>>,
) -> std::io::Result<()> {
    let socket = bind()?;
    let targets = targets(&uuid, dlna);
    // UDP may drop one; the spec suggests sending a few.
    for _ in 0..2 {
        announce(&socket, &targets, http_port, false);
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut last_announce = Instant::now();
    let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 2048];
    while !stop.load(Ordering::Relaxed) {
        if last_announce.elapsed() >= REANNOUNCE {
            announce(&socket, &targets, http_port, false);
            last_announce = Instant::now();
        }
        let Ok((n, from)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let Some(peer) = from.as_socket() else {
            continue;
        };
        if !super::http::is_local(peer.ip()) {
            continue;
        }
        // SAFETY: `recv_from` initialised the first `n` bytes.
        let bytes: Vec<u8> = buf[..n]
            .iter()
            .map(|b| unsafe { b.assume_init() })
            .collect();
        let message = String::from_utf8_lossy(&bytes);
        if let Some(location) = crate::peers::discovery::wave_location(&message) {
            if let Some(heard) = &heard {
                let _ = heard.send(location);
            }
            continue;
        }
        let Some((st, mx)) = parse_search(&message) else {
            continue;
        };
        let replies: Vec<String> = match local_address_for(peer) {
            Some(ip) => matching(&st, &targets)
                .into_iter()
                .map(|(nt, usn, path)| search_reply(nt, usn, &location(ip, http_port, path)))
                .collect(),
            None => continue,
        };
        if replies.is_empty() {
            continue;
        }
        // The spec asks for a random wait up to MX seconds so a crowd of
        // devices doesn't answer at once; a short one keeps discovery quick.
        let wait = Duration::from_millis(
            (u64::from(uuid::Uuid::new_v4().as_bytes()[0]) * 4).min(mx.clamp(1, 5) * 1000),
        );
        let _ = std::thread::Builder::new()
            .name("wave-share-ssdp-reply".into())
            .spawn(move || {
                std::thread::sleep(wait);
                if let Ok(reply_socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
                    for reply in replies {
                        let _ = reply_socket.send_to(reply.as_bytes(), peer);
                    }
                }
            });
    }
    announce(&socket, &targets, http_port, true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_and_usns_follow_the_device_architecture() {
        let all = targets("abc", true);
        assert_eq!(all.len(), 6);
        assert!(all.contains(&(
            "upnp:rootdevice".into(),
            "uuid:abc::upnp:rootdevice".into(),
            "/rootDesc.xml"
        )));
        assert!(all.contains(&("uuid:abc".into(), "uuid:abc".into(), "/rootDesc.xml")));
        assert!(all.contains(&(
            MEDIA_SERVER.to_string(),
            format!("uuid:abc::{MEDIA_SERVER}"),
            "/rootDesc.xml"
        )));
        let wave = crate::peers::discovery::WAVE_DEVICE;
        assert!(all.contains(&(wave.to_string(), format!("uuid:abc::{wave}"), "/wave/hello")));
        // With DLNA off only other Waves can find it.
        let quiet = targets("abc", false);
        assert_eq!(quiet.len(), 1);
        assert_eq!(quiet[0].0, wave);
    }

    #[test]
    fn searches_are_read_and_matched() {
        let search = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n";
        let (st, mx) = parse_search(search).unwrap();
        assert_eq!((st.as_str(), mx), (MEDIA_SERVER, 2));
        let all = targets("abc", true);
        assert_eq!(matching(&st, &all).len(), 1);
        assert_eq!(matching("ssdp:all", &all).len(), 6);
        assert!(matching("urn:schemas-upnp-org:device:MediaRenderer:1", &all).is_empty());
        // No MAN header, or a NOTIFY, is not a search.
        assert!(parse_search("M-SEARCH * HTTP/1.1\r\nST: ssdp:all\r\n\r\n").is_none());
        assert!(parse_search("NOTIFY * HTTP/1.1\r\nNT: x\r\n\r\n").is_none());
    }

    #[test]
    fn messages_carry_the_required_headers() {
        let reply = search_reply(
            "upnp:rootdevice",
            "uuid:abc::upnp:rootdevice",
            "http://10.0.0.2:50800/rootDesc.xml",
        );
        for header in [
            "CACHE-CONTROL: max-age=1800",
            "DATE: ",
            "EXT:",
            "LOCATION: http://10.0.0.2:50800/rootDesc.xml",
            "SERVER: ",
            "ST: upnp:rootdevice",
            "USN: uuid:abc::upnp:rootdevice",
        ] {
            assert!(reply.contains(header), "missing {header}");
        }
        // Wave's own client reads LOCATION from exactly this.
        assert!(alive("uuid:abc", "uuid:abc", "http://h/rootDesc.xml").contains("NTS: ssdp:alive"));
        assert!(byebye("uuid:abc", "uuid:abc").contains("NTS: ssdp:byebye"));
        assert!(!byebye("uuid:abc", "uuid:abc").contains("LOCATION"));
        let date = httpdate();
        assert!(date.ends_with(" GMT") && date.len() == 29, "{date}");
    }
}
