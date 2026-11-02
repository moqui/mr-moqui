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

//! The wizard's TURN check against the embedded TURN server on loopback, as an edge node runs it (E7): the credential
//! of its own secret allocates and gets a permission for the media address's IP; one of another secret is refused.

use std::net::SocketAddr;

use super::*;

const SECRET: &str = "edge-turn-secret";

fn turn_node(media: SocketAddr) -> (super::super::turn::TurnNode, String) {
    let property = |name: &str| match name {
        "turn_port" => "0".to_string(),
        "turn_host" => "127.0.0.1".to_string(),
        "turn_auth_secret" => SECRET.to_string(),
        _ => String::new(),
    };
    let settings = super::super::turn::settings(&property, media, media, &[])
        .unwrap()
        .expect("turn_port is set");
    let dir = std::env::temp_dir();
    let node = super::super::turn::start(&dir, &settings).unwrap();
    let udp = node.bound.udp.expect("the UDP listener");
    (node, format!("turn:127.0.0.1:{}?transport=udp", udp.port()))
}

fn expiry() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600
}

#[test]
fn a_credential_of_the_edge_nodes_secret_allocates_and_relays_to_its_media_address() {
    let media: SocketAddr = "127.0.0.1:10000".parse().unwrap();
    let (_node, url) = turn_node(media);
    let (username, password) = moqui_rtc::turn::credential(SECRET.as_bytes(), "edgeSetupProbe.a", expiry());
    let done = probe(&url, &username, &password, &media.to_string()).unwrap();
    assert!(
        done.contains("permission for its media address's IP 127.0.0.1"),
        "{done}"
    );
    // another secret's credential is refused, as a local node's own would be
    let (username, password) = moqui_rtc::turn::credential(b"another-secret", "edgeSetupProbe.b", expiry());
    let refused = probe(&url, &username, &password, &media.to_string()).unwrap_err();
    assert!(refused.contains("refused the allocation"), "{refused}");
}

#[test]
fn only_a_turn_url_over_udp_is_probed() {
    assert_eq!(
        udp_server("turn:edge.example.net:3478?transport=udp").unwrap(),
        "edge.example.net:3478"
    );
    assert_eq!(udp_server("turn:edge.example.net").unwrap(), "edge.example.net:3478");
    assert_eq!(udp_server("turn:[2001:db8::5]:3479").unwrap(), "[2001:db8::5]:3479");
    assert!(udp_server("turn:edge.example.net:3478?transport=tcp").is_err());
    assert!(udp_server("turns:edge.example.net:5349?transport=tcp").is_err());
    assert!(udp_server("turn::3478").is_err());
    assert!(udp_server("turn:edge.example.net:port").is_err());
}
