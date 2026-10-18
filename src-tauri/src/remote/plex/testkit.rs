// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! A canned HTTP server for Plex tests, on 127.0.0.1.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

pub(crate) type Recorded = Arc<std::sync::Mutex<Vec<String>>>;

/// Serves each canned response to one connection in turn, then keeps the
/// last one. Records the raw request head of every connection.
pub(crate) fn fake_server(responses: Vec<String>) -> (u16, Recorded) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Recorded = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let mut stream: TcpStream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut head = Vec::new();
            let mut buf = [0u8; 512];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => head.extend_from_slice(&buf[..k]),
                }
            }
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&head).into_owned());
            let reply = &responses[n.min(responses.len() - 1)];
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (port, seen)
}

pub(crate) fn http(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
