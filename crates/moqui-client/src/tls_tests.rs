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

//! The client's TLS checks, the in-process proofs of OWASP MASVS v2.1.0 MASVS-NETWORK-1 for its own traffic
//! (docs/security/proofs/clients/README.md): the requests (`http::client_with`) and the sockets (`socket::open`) take
//! the server's certificate only from the OS trust store (rustls-platform-verifier) and only for the server's name,
//! before anything is sent. The trust store is the OS's, so no test can put a root in it: the refusal of a certificate
//! no trusted authority signed goes through the real entry point, and the control (the right certificate taken, one
//! for another name refused) through the same transport with the test's authority added to the same platform verifier
//! (`Verifier::new_with_extra_roots`, what reqwest's `tls_certs_merge` builds), as the framework's outbound client proof
//! does (`the_outbound_http_client_checks_the_servers_certificate_and_name`, crates/moqui-host/src/tls_tests.rs).

use std::io::{Read as _, Write as _};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jiff::ToSpan as _;
use moqui_lib::sync::lock;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};

use crate::socket::Event;

/// What a test server answers the request it read.
#[derive(Clone, Copy)]
enum Answer {
    /// `200 OK` with the body `checked`.
    Ok,
    /// The WebSocket upgrade, as `/notws` answers it.
    Upgrade,
}

/// A certificate and its key, as a TLS server presents them.
struct Identity {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

/// A certificate for `localhost` that signs itself, which no trusted authority signed.
fn self_signed() -> Identity {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    Identity {
        chain: vec![cert.der().clone()],
        key: PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    }
}

/// A test authority, and a server certificate for `localhost` it issued: a month long and for TLS servers, as Apple's
/// verifier wants of a certificate issued today.
fn issued() -> (CertificateDer<'static>, Identity) {
    let authority_key = rcgen::KeyPair::generate().unwrap();
    let mut authority = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    authority.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    authority.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign, rcgen::KeyUsagePurpose::CrlSign];
    authority
        .distinguished_name
        .push(rcgen::DnType::CommonName, "moqui-client proof authority");
    let root = authority.self_signed(&authority_key).unwrap().der().clone();
    let issuer = rcgen::Issuer::new(authority, authority_key);

    let today = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date();
    let ymd = |date: jiff::civil::Date| {
        rcgen::date_time_ymd(
            i32::from(date.year()),
            u8::try_from(date.month()).unwrap(),
            u8::try_from(date.day()).unwrap(),
        )
    };
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    params.not_before = ymd(today.yesterday().unwrap());
    params.not_after = ymd(today.checked_add(30.days()).unwrap());
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    params.distinguished_name.push(rcgen::DnType::CommonName, "localhost");
    let cert = params.signed_by(&key, &issuer).unwrap();
    let identity = Identity {
        chain: vec![cert.der().clone()],
        key: PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    };
    (root, identity)
}

/// A TLS server on loopback presenting `identity`. For each connection it keeps the head of the request it read after
/// the handshake (empty when the handshake failed and nothing came), then gives `answer`.
fn serve(identity: Identity, answer: Answer) -> (u16, Arc<Mutex<Vec<String>>>) {
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(identity.chain, identity.key)
            .unwrap(),
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let heads = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&heads);
    std::thread::spawn(move || {
        for tcp in listener.incoming() {
            let Ok(tcp) = tcp else { return };
            let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
            let Ok(connection) = rustls::ServerConnection::new(Arc::clone(&config)) else {
                continue;
            };
            let mut tls = rustls::StreamOwned::new(connection, tcp);
            let mut head = Vec::new();
            let mut buffer = [0_u8; 4096];
            while !head.windows(4).any(|end| end == b"\r\n\r\n") {
                match tls.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => head.extend_from_slice(&buffer[..read]),
                }
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let reply = match answer {
                _ if head.is_empty() => None,
                Answer::Ok => {
                    Some("HTTP/1.1 200 OK\r\ncontent-length: 7\r\nconnection: close\r\n\r\nchecked".to_string())
                }
                Answer::Upgrade => head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("sec-websocket-key")
                            .then(|| value.trim().to_string())
                    })
                    .map(|key| {
                        format!(
                            "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
                             sec-websocket-accept: {}\r\n\r\n",
                            tungstenite::handshake::derive_accept_key(key.as_bytes())
                        )
                    }),
            };
            lock(&kept).push(head);
            if let Some(reply) = reply {
                let _ = tls.write_all(reply.as_bytes());
                tls.conn.send_close_notify();
                let _ = tls.flush();
            }
        }
    });
    (port, heads)
}

/// The heads the server kept, once it has kept `count` (or five seconds went by).
fn heads_kept(heads: &Mutex<Vec<String>>, count: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while lock(heads).len() < count && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    lock(heads).clone()
}

/// An error and its sources, as one line.
fn chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(err) = source {
        text = format!("{text}: {err}");
        source = err.source();
    }
    text
}

/// Whether `text` names rustls's refusal of a certificate for another name (Apple's verifier gives the bare variant,
/// webpki's the one with the names).
fn wrong_name(text: &str) -> bool {
    text.contains("NotValidForName") || text.contains("not valid for name")
}

/// The security proof of OWASP MASVS v2.1.0 MASVS-NETWORK-1 (docs/security/proofs/clients/README.md), in-process, for
/// the client's requests: every request of the crate goes through `http::client_with` (reqwest on rustls with the OS
/// trust store, rustls-platform-verifier), and against a server whose certificate signs itself it fails at the TLS
/// handshake, by name or by address, so the request (and the `login_key` header a proxied one carries) never reaches
/// the server. The control is the same transport with the test's authority added to the same platform verifier
/// (`tls_certs_merge`; `client_with` cannot be given a root): it takes the certificate that authority issued for
/// `localhost`, and refuses it at 127.0.0.1, a name it was not issued for, again before sending anything.
#[test]
fn the_clients_requests_refuse_a_certificate_no_trusted_authority_signed_or_for_another_name() {
    let (port, heads) = serve(self_signed(), Answer::Ok);
    let client = crate::http::client_with(Duration::from_secs(5)).unwrap();
    for host in ["localhost", "127.0.0.1"] {
        let err = client
            .get(format!("https://{host}:{port}/rest/s1/moqui/basic"))
            .header("login_key", "the-device-key")
            .send()
            .expect_err("no trusted authority signed it");
        let err = chain(&err);
        assert!(err.contains("invalid peer certificate"), "{host}: {err}");
    }
    let heads = heads_kept(&heads, 2);
    assert!(heads.len() >= 2, "{heads:?}");
    assert!(heads.iter().all(String::is_empty), "nothing was sent: {heads:?}");

    let (root, identity) = issued();
    let (port, heads) = serve(identity, Answer::Ok);
    let trusting = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .tls_certs_merge([reqwest::Certificate::from_der(&root).unwrap()])
        .build()
        .unwrap();
    let err = trusting
        .get(format!("https://127.0.0.1:{port}/proof"))
        .send()
        .expect_err("a name it was not issued for");
    let err = chain(&err);
    assert!(wrong_name(&err), "{err}");
    assert_eq!(
        heads_kept(&heads, 1),
        vec![String::new()],
        "nothing was sent past the refused name"
    );
    let response = trusting
        .get(format!("https://localhost:{port}/proof"))
        .send()
        .expect("the name it was issued for");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.text().unwrap(), "checked");
    let heads = heads_kept(&heads, 2);
    assert!(heads[1].starts_with("GET /proof HTTP/1.1\r\n"), "{heads:?}");
}

/// The security proof of OWASP MASVS v2.1.0 MASVS-NETWORK-1 (docs/security/proofs/clients/README.md), in-process, for
/// the client's sockets: a page's `/notws` held by the host (`socket::open`, rustls with
/// `with_safe_default_protocol_versions`, `with_platform_verifier` and the server's name) to a server whose
/// certificate signs itself never opens, and the upgrade request with the device key is never sent. The control is
/// the same TLS client settings and WebSocket handshake with the test's authority added to the same platform verifier:
/// the certificate it issued for `localhost` is taken and the key goes in the upgrade, and at 127.0.0.1 it is refused
/// at the handshake, before the upgrade.
#[test]
fn the_clients_sockets_refuse_a_certificate_no_trusted_authority_signed_or_for_another_name() {
    let (port, heads) = serve(self_signed(), Answer::Upgrade);
    let (sender, events) = mpsc::channel();
    let socket = crate::socket::open(
        &format!("https://localhost:{port}"),
        "the-device-key",
        "/notws",
        Box::new(move |event| {
            let _ = sender.send(event);
        }),
    )
    .unwrap();
    let first = events
        .recv_timeout(Duration::from_secs(10))
        .expect("the socket's first event");
    drop(socket);
    assert_eq!(first, Event::Closed, "the socket did not open");
    let heads = heads_kept(&heads, 1);
    assert!(!heads.is_empty(), "the socket connected");
    assert!(heads.iter().all(String::is_empty), "no upgrade, no key: {heads:?}");
    assert!(
        events.try_iter().all(|event| event != Event::Open),
        "the socket never opened"
    );

    let (root, identity) = issued();
    let (port, heads) = serve(identity, Answer::Upgrade);
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = rustls_platform_verifier::Verifier::new_with_extra_roots([root], Arc::clone(&provider)).unwrap();
    let config = Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth(),
    );
    let handshake = |host: &str| {
        let tcp = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let name = ServerName::try_from(host.to_string()).unwrap();
        let connection = rustls::ClientConnection::new(Arc::clone(&config), name).unwrap();
        let uri: tungstenite::http::Uri = format!("wss://{host}:{port}/notws").parse().unwrap();
        let request = tungstenite::ClientRequestBuilder::new(uri).with_header("login_key", "the-device-key");
        match tungstenite::client(request, rustls::StreamOwned::new(connection, tcp)) {
            Ok(_) => Ok(()),
            Err(tungstenite::HandshakeError::Failure(err)) => Err(err),
            Err(tungstenite::HandshakeError::Interrupted(_)) => panic!("{host}: the handshake was interrupted"),
        }
    };
    let Err(tungstenite::Error::Io(err)) = handshake("127.0.0.1") else {
        panic!("a name it was not issued for is refused at the TLS handshake");
    };
    let refused = err.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>());
    assert!(
        matches!(
            refused,
            Some(rustls::Error::InvalidCertificate(
                rustls::CertificateError::NotValidForName | rustls::CertificateError::NotValidForNameContext { .. }
            ))
        ),
        "{err}"
    );
    assert_eq!(
        heads_kept(&heads, 1),
        vec![String::new()],
        "nothing was sent past the refused name"
    );
    assert!(handshake("localhost").is_ok(), "the name it was issued for");
    let heads = heads_kept(&heads, 2);
    assert!(heads[1].starts_with("GET /notws HTTP/1.1\r\n"), "{heads:?}");
    assert!(heads[1].contains("login_key: the-device-key\r\n"), "{heads:?}");
}
