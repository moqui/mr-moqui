/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The TURN settings, and the bounds of its TCP connections on a real port.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use super::{settings, start, StreamLimits};

fn of(pairs: &[(&str, &str)]) -> Result<Option<super::TurnSettings>, String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let property = move |name: &str| map.get(name).cloned().unwrap_or_default();
    settings(
        &property,
        "127.0.0.1:10000".parse().unwrap(),
        "203.0.113.5:10000".parse().unwrap(),
        &[],
    )
}

#[test]
fn turn_is_off_without_a_listener_and_needs_a_secret() {
    assert!(of(&[]).unwrap().is_none());
    assert!(of(&[("turn_port", "3478")]).is_err());
    let s = of(&[("turn_port", "3478"), ("turn_auth_secret", "s3cret")])
        .unwrap()
        .unwrap();
    assert_eq!(s.public_host, "203.0.113.5");
    assert_eq!(s.relay_ports, (49152, 65535));
    assert_eq!(
        s.config.allowed_peers,
        vec!["203.0.113.5:10000".parse().unwrap(), "127.0.0.1:10000".parse().unwrap()]
    );
    // another media node by address alone is on the media port, or as given
    let s = of(&[
        ("turn_port", "3478"),
        ("turn_auth_secret", "s3cret"),
        ("turn_allowed_peers", "198.51.100.7, 198.51.100.8:11000"),
    ])
    .unwrap()
    .unwrap();
    assert!(s.config.allowed_peers.contains(&"198.51.100.7:10000".parse().unwrap()));
    assert!(s.config.allowed_peers.contains(&"198.51.100.8:11000".parse().unwrap()));
}

#[test]
fn a_bad_range_or_address_is_an_error() {
    let base = [("turn_port", "3478"), ("turn_auth_secret", "s3cret")];
    assert!(of(&[base[0], base[1], ("turn_relay_ports", "60000-50000")]).is_err());
    assert!(of(&[base[0], base[1], ("turn_tls443_host", "not an address")]).is_err());
    assert!(of(&[base[0], base[1], ("turn_allowed_peers", "nope")]).is_err());
}

/// Whether the server closed `stream` within `wait`, rather than keeping it open.
fn closed_within(stream: &mut TcpStream, wait: Duration) -> bool {
    stream.set_read_timeout(Some(wait)).unwrap();
    let mut buf = [0_u8; 512];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return true,
            // an answer to a message it sent; read on
            Ok(_) => {}
            Err(err) => {
                return !matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                )
            }
        }
    }
}

/// A STUN Binding request, a whole TURN message.
fn binding_request() -> Vec<u8> {
    let mut message = vec![0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xa4, 0x42];
    message.extend_from_slice(&[7; 12]);
    message
}

/// One connection past the per-source share is closed at once; one that sends no message is closed by the handshake
/// deadline; one that sent a message and then nothing by the idle deadline (HL-031).
#[test]
fn tcp_connections_are_capped_per_source_and_closed_by_their_deadlines() {
    let mut s = of(&[
        ("turn_port", "0"),
        ("turn_host", "127.0.0.1"),
        ("turn_auth_secret", "s3cret"),
    ])
    .unwrap()
    .unwrap();
    s.limits = StreamLimits {
        max_connections: 8,
        per_source: 2,
        handshake: Duration::from_millis(500),
        idle: Duration::from_millis(900),
        max_queued: 4,
    };
    let runtime = std::env::temp_dir();
    let node = start(&runtime, &s).unwrap();
    let address = node.bound.tcp.unwrap();
    let mut silent = TcpStream::connect(address).unwrap();
    let mut spoke = TcpStream::connect(address).unwrap();
    spoke.write_all(&binding_request()).unwrap();
    let mut past = TcpStream::connect(address).unwrap();
    let started = Instant::now();
    assert!(closed_within(&mut past, Duration::from_millis(300)), "past the share");
    assert!(
        closed_within(&mut silent, Duration::from_secs(2)),
        "the handshake deadline"
    );
    assert!(
        !closed_within(&mut spoke, Duration::from_millis(100)),
        "a message was its handshake"
    );
    assert!(closed_within(&mut spoke, Duration::from_secs(2)), "the idle deadline");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(800) && elapsed < Duration::from_secs(2),
        "{elapsed:?}"
    );
    drop(node);
}
