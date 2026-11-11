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

use std::sync::mpsc;

use super::*;

#[test]
fn a_page_opens_only_the_servers_sockets() {
    assert_eq!(
        socket_url("https://erp.example.com", "/notws").unwrap(),
        "wss://erp.example.com:443/notws"
    );
    assert_eq!(
        socket_url("http://localhost:8080/moqui", "/callws?x=1").unwrap(),
        "ws://localhost:8080/moqui/callws?x=1"
    );
    assert_eq!(
        socket_url("https://dev.example.com", "/termws?session=abc").unwrap(),
        "wss://dev.example.com:443/termws?session=abc"
    );
    for path in ["/apps/x", "notws", "/notws/../apps", "/notws#x", "//evil/notws"] {
        assert!(socket_url("https://erp.example.com", path).is_err(), "{path}");
    }
}

#[test]
fn only_a_close_for_authentication_is_final() {
    assert!(revoked(1008, "authentication is no longer valid"));
    assert!(!revoked(1008, "too many notifications waiting"));
    assert!(!revoked(1000, "authentication"));
}

/// A server on loopback that checks the key, takes a frame from the page, sends one and closes as `/notws` does when
/// the key is revoked: the page hears it open, the frame, then that it was revoked, and nothing after.
#[test]
#[allow(clippy::result_large_err)] // tungstenite's handshake callback returns its own error type
fn frames_pass_both_ways_and_a_revoked_key_ends_the_socket() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let mut key = String::new();
        let mut socket = tungstenite::accept_hdr(tcp, |request: &tungstenite::handshake::server::Request, response| {
            key = request
                .headers()
                .get("login_key")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            Ok(response)
        })
        .unwrap();
        let first = loop {
            match socket.read().unwrap() {
                tungstenite::Message::Text(text) => break text.as_str().to_string(),
                _ => continue,
            }
        };
        socket
            .send(tungstenite::Message::Text("{\"topic\":\"shellCounts\"}".into()))
            .unwrap();
        socket
            .close(Some(tungstenite::protocol::CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::from(1008),
                reason: "authentication is no longer valid".into(),
            }))
            .unwrap();
        let _ = socket.flush();
        // let the close reach the client before the socket drops
        while socket.read().is_ok() {}
        (key, first)
    });

    let (sent, heard) = mpsc::channel();
    let socket = open(
        &format!("http://127.0.0.1:{port}"),
        "the-key",
        "/notws",
        Box::new(move |event| {
            let _ = sent.send(event);
        }),
    )
    .unwrap();
    socket.send("subscribe:ALL".to_string());
    let wait = Duration::from_secs(5);
    assert_eq!(heard.recv_timeout(wait).unwrap(), Event::Open);
    assert_eq!(
        heard.recv_timeout(wait).unwrap(),
        Event::Text("{\"topic\":\"shellCounts\"}".to_string())
    );
    assert_eq!(heard.recv_timeout(wait).unwrap(), Event::Revoked);
    let (key, first) = server.join().unwrap();
    assert_eq!(key, "the-key");
    assert_eq!(first, "subscribe:ALL");
    assert!(heard.recv_timeout(Duration::from_millis(300)).is_err());
}

/// The terminal's socket (plans/Terminal-Fancy.md §12): binary frames both ways, every frame the page sent kept, and a
/// close heard with its reason and not connected again, since the page attaches again itself.
#[test]
fn the_terminals_socket_carries_bytes_and_ends_with_its_reason() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(tcp).unwrap();
        let mut typed = Vec::new();
        while typed.len() < 300 {
            if let tungstenite::Message::Binary(bytes) = socket.read().unwrap() {
                typed.extend_from_slice(&bytes);
            }
        }
        socket
            .send(tungstenite::Message::Binary(vec![0x1b, b'[', b'H'].into()))
            .unwrap();
        socket
            .close(Some(tungstenite::protocol::CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::Normal,
                reason: "opened elsewhere".into(),
            }))
            .unwrap();
        let _ = socket.flush();
        while socket.read().is_ok() {}
        // nothing connects again
        listener.set_nonblocking(true).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        (typed, listener.accept().is_err())
    });
    let (sent, heard) = mpsc::channel();
    let socket = open(
        &format!("http://127.0.0.1:{port}"),
        "the-key",
        "/termws?session=abc",
        Box::new(move |event| {
            let _ = sent.send(event);
        }),
    )
    .unwrap();
    // more than the queue's limit of a text socket, before it opens: none is dropped
    for _ in 0..300 {
        socket.send_binary(vec![b'x']);
    }
    let wait = Duration::from_secs(5);
    assert_eq!(heard.recv_timeout(wait).unwrap(), Event::Open);
    assert_eq!(heard.recv_timeout(wait).unwrap(), Event::Binary(vec![0x1b, b'[', b'H']));
    assert_eq!(
        heard.recv_timeout(wait).unwrap(),
        Event::Ended("opened elsewhere".to_string())
    );
    let (typed, no_second) = server.join().unwrap();
    assert_eq!(typed, vec![b'x'; 300]);
    assert!(no_second, "the host does not connect the terminal again");
    assert!(heard.recv_timeout(Duration::from_millis(200)).is_err());
}
