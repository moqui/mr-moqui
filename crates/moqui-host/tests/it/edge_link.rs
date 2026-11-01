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

//! The edge link (plans/Edge-Node.md §5.1, E1). The edge end is a real server (`node_role=edge`, the HTTPS listener on
//! a certificate from a test authority) in the test's own process; the local end is the link's dialer driven with an
//! explicit `LocalConfig`, which needs no factory and opens no port, so the two share the process without sharing
//! properties. Pairing works once per code; a local node with no listening port links up and down, with Hello both
//! ways; a stale generation is fenced; unpairing on either side cuts the link and a new dial is refused; a killed
//! connection, a new dial from another address and an edge node restart each recover on a short backoff.

#![cfg(all(unix, feature = "edge", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use moqui_core::edge::{LinkState, LinkStatus};
use moqui_host::edge_link::dial::try_down;
use moqui_host::edge_link::{
    connect_up, pair, Backoff, Dialer, LinkError, LocalConfig, PairConfig, ALPN, ALPN_PAIR, ROLE_PAIR, ROLE_UP,
};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use tokio::sync::watch;

use crate::common::Gate;

const SERVICES: &str = "org.moqui.impl.EdgeServices";

/// A test certificate authority and the edge node's `localhost` certificate and key (PEM) it signed.
struct Certs {
    root: CertificateDer<'static>,
    cert_pem: String,
    key_pem: String,
}

fn pem(label: &str, der: &[u8]) -> String {
    let text = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = text
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

fn certs() -> Certs {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "Edge Test CA");
    ca_params.distinguished_name = name;
    let root = ca_params.self_signed(&ca_key).unwrap().der().clone();
    let issuer = rcgen::Issuer::new(ca_params, ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let cert = params.signed_by(&key, &issuer).unwrap();
    Certs {
        root,
        cert_pem: pem("CERTIFICATE", cert.der()),
        key_pem: pem("PRIVATE KEY", &key.serialize_der()),
    }
}

/// The edge node, with HTTPS on `port`, which a restart binds again.
fn start_edge(label: &str, certs: &Certs, port: u16) -> Gate {
    let port = port.to_string();
    crate::common::start_with_files(
        label,
        None,
        &[("node_role", "edge"), ("webapp_https_listen_port", &port)],
        &[
            ("cert/default.crt", &certs.cert_pem),
            ("cert/default.key", &certs.key_pem),
        ],
    )
}

/// An EdgeServices service on the edge node, as an administrator.
fn call(gate: &Gate, verb_noun: &str, parameters: &[(&str, &str)]) -> Result<BTreeMap<String, Value>, String> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect();
    moqui_core::service::sync(&format!("{SERVICES}.{verb_noun}"), &parameters).map_err(|err| err.to_string())
}

fn code(gate: &Gate) -> String {
    to_plain_string(&call(gate, "create#PairingCode", &[]).unwrap()["code"])
}

/// The paired node's row in get#EdgeClients.
fn client(gate: &Gate, node: &str) -> BTreeMap<String, String> {
    let out = call(gate, "get#EdgeClients", &[]).unwrap();
    let Some(Value::List(list)) = out.get("clientList") else {
        panic!("clientList: {out:?}")
    };
    list.iter()
        .find_map(|row| {
            let Value::Map(row) = row else { return None };
            (to_plain_string(&row["pairedNodeId"]) == node).then(|| {
                row.iter()
                    .map(|(name, value)| (name.clone(), to_plain_string(value)))
                    .collect()
            })
        })
        .unwrap_or_else(|| panic!("{node} is not paired: {out:?}"))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// Short timings, so a lost link comes back in well under a second.
fn quick(config: &mut LocalConfig) {
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.keepalive = Duration::from_secs(2);
    config.refused_backoff = Backoff {
        first: Duration::from_millis(200),
        max: Duration::from_millis(500),
    };
    config.ping_every = Duration::from_millis(500);
    config.connect_timeout = Duration::from_secs(3);
}

/// Accepts any server certificate: for the raw connections that check what the edge node refuses.
#[derive(Debug)]
struct AnyCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Presents no client certificate, and notes whether the server asked for one.
#[derive(Debug)]
struct Asked(Arc<AtomicBool>);

impl rustls::client::ResolvesClientCert for Asked {
    fn resolve(
        &self,
        _hints: &[&[u8]],
        _schemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.0.store(true, Ordering::SeqCst);
        None
    }

    fn has_certs(&self) -> bool {
        true
    }
}

/// A TLS connection to the edge node with no client certificate, offering `alpn`: the stream, and whether the server
/// asked for a client certificate.
fn raw_tls(
    rt: &tokio::runtime::Runtime,
    port: u16,
    alpn: &[u8],
) -> (tokio_rustls::client::TlsStream<tokio::net::TcpStream>, bool) {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let asked = Arc::new(AtomicBool::new(false));
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCert(provider)))
        .with_client_cert_resolver(Arc::new(Asked(Arc::clone(&asked))));
    config.alpn_protocols = vec![alpn.to_vec()];
    let stream = rt.block_on(async {
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .unwrap()
    });
    let alpn_seen = stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
    assert_eq!(alpn_seen.as_deref(), Some(alpn));
    (stream, asked.load(Ordering::SeqCst))
}

/// Send a role byte on a raw link connection and read the edge node's answer: its code and message.
fn role_answer(
    rt: &tokio::runtime::Runtime,
    stream: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    role: u8,
) -> (u8, String) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    rt.block_on(async {
        stream.write_all(&[role]).await.unwrap();
        stream.flush().await.unwrap();
        let mut head = [0_u8; 3];
        stream.read_exact(&mut head).await.unwrap();
        let mut text = vec![0_u8; usize::from(u16::from_be_bytes([head[1], head[2]]))];
        stream.read_exact(&mut text).await.unwrap();
        (head[0], String::from_utf8_lossy(&text).into_owned())
    })
}

fn pair_config(url: &str, code: &str, node: &str, certs: &Certs) -> PairConfig {
    let mut config = PairConfig::new(url, code, node);
    config.trust_root = Some(certs.root.clone());
    config.connect_timeout = Duration::from_secs(5);
    config
}

/// Wait up to `limit` for the dialer's status to pass `check`.
fn wait_status(dialer: &Dialer, limit: Duration, what: &str, check: impl Fn(&LinkStatus) -> bool) -> LinkStatus {
    let started = Instant::now();
    loop {
        let status = dialer.status();
        if check(&status) {
            return status;
        }
        assert!(started.elapsed() < limit, "{what}: still {status:?}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn pairing_links_both_ways_and_unpairing_cuts_the_link() {
    if !crate::common::own_process() {
        return;
    }
    let certs = certs();
    let port = crate::common::free_port();
    let gate = start_edge("edge-pair", &certs, port);
    let url = format!("https://localhost:{port}");
    let rt = runtime();

    // a code pairs once; again, expired, or with another fingerprint confirmed, it does not
    let code_a = code(&gate);
    let mut config = pair_config(&url, &code_a, "local-a", &certs);
    let checked = rt
        .block_on(moqui_host::edge_link::check(
            &url,
            Some(&certs.root),
            Duration::from_secs(5),
        ))
        .unwrap();
    assert!(checked.compatible, "{checked:?}");
    let status = call(&gate, "get#LinkStatus", &[]).unwrap();
    assert_eq!(
        to_plain_string(&status["linkFingerprint"]),
        checked.fingerprint,
        "{status:?}"
    );
    config.expect_pin = Some("00".repeat(32));
    let wrong = rt.block_on(pair(&config)).unwrap_err();
    assert!(wrong.message().contains("not the one confirmed"), "{wrong}");
    config.expect_pin = Some(checked.fingerprint.clone());
    let paired = rt.block_on(pair(&config)).unwrap();
    assert_eq!(paired.pin, checked.fingerprint);
    assert!(paired.edge.compatible);
    let again = rt.block_on(pair(&config)).unwrap_err();
    assert!(
        matches!(&again, LinkError::Refused(message) if message.contains("unknown, used or expired")),
        "{again:?}"
    );
    let expired = call(&gate, "create#PairingCode", &[]).unwrap();
    {
        let factory = gate.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let mut row = facade.make_value("moqui.edge.EdgePairingCode").unwrap();
        row.set("pairingCodeId", expired["pairingCodeId"].clone());
        row.set(
            "expireDate",
            moqui_core::user::stamp_value(moqui_core::user::now_millis() - 60_000),
        );
        row.update(&facade).unwrap();
    }
    let late = pair_config(&url, &to_plain_string(&expired["code"]), "local-a", &certs);
    assert!(rt.block_on(pair(&late)).unwrap_err().message().contains("expired"));
    assert_eq!(client(&gate, "local-a")["statusId"], "EdgeClientActive");

    // the local node has no listening port: it dials both connections; Hello goes both ways, and each carries a call
    let mut local = LocalConfig::new(&url, &paired.key, &paired.pin, "local-a");
    quick(&mut local);
    let (_shutdown, shutdown) = watch::channel(false);
    let dialer = Dialer::start(local.clone(), rt.handle(), shutdown.clone(), None).unwrap();
    let up = wait_status(&dialer, Duration::from_secs(10), "link up", |status| {
        status.state == LinkState::Linked && status.rtt_ms.is_some()
    });
    assert!(up.generation >= 1);
    assert_eq!(up.peer.fingerprint, paired.pin);
    assert!(!up.peer.binary_version.is_empty(), "the edge node's Hello: {up:?}");
    wait_until(Duration::from_secs(5), "the down connection", || {
        client(&gate, "local-a")["down"] == "true"
    });
    let row = client(&gate, "local-a");
    assert_eq!(row["linked"], "true");
    assert_eq!(row["generation"], up.generation.to_string());
    assert!(!row["binaryVersion"].is_empty(), "the local node's Hello: {row:?}");
    let ping = call(&gate, "ping#EdgeClient", &[("pairedNodeId", "local-a")]).unwrap();
    assert!(matches!(ping["rttMs"], Value::F64(rtt) if rtt > 0.0), "{ping:?}");

    // a browser's http/1.1 on the same port is served and never asked for a client certificate; the link's protocols
    // ask, and without one an up connection and pairing are refused
    let (mut browser, asked) = raw_tls(&rt, port, b"http/1.1");
    assert!(!asked, "an http/1.1 client was asked for a client certificate");
    let answer = rt.block_on(async {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        browser
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut text = String::new();
        let _ = browser.read_to_string(&mut text).await;
        text
    });
    assert!(answer.starts_with("HTTP/1.1 "), "{answer}");
    let (mut up_raw, asked) = raw_tls(&rt, port, ALPN);
    assert!(asked, "the link did not ask for a client certificate");
    let (answer_code, message) = role_answer(&rt, &mut up_raw, ROLE_UP);
    assert_eq!(answer_code, 1, "{message}");
    let (mut pair_raw, asked) = raw_tls(&rt, port, ALPN_PAIR);
    assert!(asked);
    let (answer_code, message) = role_answer(&rt, &mut pair_raw, ROLE_PAIR);
    assert_eq!(answer_code, 1, "{message}");
    assert!(message.contains("client certificate"), "{message}");

    // a new code does not take over a node that is active with another key
    let takeover = rt
        .block_on(pair(&pair_config(&url, &code(&gate), "local-a", &certs)))
        .unwrap_err();
    assert!(takeover.message().contains("already paired"), "{takeover:?}");
    assert_eq!(client(&gate, "local-a")["keyFingerprint"], paired.key_fingerprint);

    // a new up connection fences the one before: it can no longer answer
    dialer.stop();
    wait_until(Duration::from_secs(5), "the dialer's link closed", || {
        client(&gate, "local-a")["linked"] == "false"
    });
    let first = rt.block_on(connect_up(&local)).unwrap();
    rt.block_on(first.ping()).unwrap();
    let second = rt.block_on(connect_up(&local)).unwrap();
    assert_eq!(second.generation, first.generation + 1);
    let fenced = rt.block_on(first.ping());
    assert!(fenced.is_err(), "the stale generation still answers");
    rt.block_on(second.ping()).unwrap();
    // a down connection must name the live generation
    let stale = rt.block_on(try_down(&local, first.generation)).unwrap_err();
    assert!(stale.message().contains("no up connection of generation"), "{stale:?}");
    drop((first, second));

    // unpairing from the local node revokes its key on the edge node, cuts the link, and a new dial is refused
    let dialer = Dialer::start(local.clone(), rt.handle(), shutdown, None).unwrap();
    wait_status(&dialer, Duration::from_secs(10), "link up again", |status| {
        status.state == LinkState::Linked
    });
    rt.block_on(dialer.unpair()).unwrap();
    wait_until(Duration::from_secs(5), "the edge node's link closed", || {
        let row = client(&gate, "local-a");
        row["statusId"] == "EdgeClientRevoked" && row["linked"] == "false"
    });
    let refused = rt.block_on(connect_up(&local)).err().expect("a revoked key links");
    assert!(
        matches!(&refused, LinkError::Refused(message) if message.contains("revoked")),
        "{refused:?}"
    );
}

/// A TCP relay in front of the edge node: what NAT does to the local node's connections, with a switch that cuts them
/// all. Each connection through it comes from a new port.
struct Relay {
    address: SocketAddr,
    cut: watch::Sender<u64>,
    /// Makes the connections open now go silent: nothing more is forwarded, and nothing is closed.
    stall: watch::Sender<u64>,
}

fn relay(rt: &tokio::runtime::Runtime, target: SocketAddr) -> Relay {
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let address = listener.local_addr().unwrap();
    let (cut, cuts) = watch::channel(0_u64);
    let (stall, stalls) = watch::channel(0_u64);
    rt.spawn(async move {
        loop {
            let Ok((mut inbound, _)) = listener.accept().await else {
                return;
            };
            let mut cuts = cuts.clone();
            cuts.mark_unchanged();
            let mut stalls = stalls.clone();
            stalls.mark_unchanged();
            tokio::spawn(async move {
                let Ok(mut outbound) = tokio::net::TcpStream::connect(target).await else {
                    return;
                };
                let stalled = tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound) => false,
                    _ = cuts.changed() => false,
                    _ = stalls.changed() => true,
                };
                if stalled {
                    // a dropped NAT mapping: both ends stay open, and nothing arrives
                    let _ = cuts.changed().await;
                }
                drop((inbound, outbound));
            });
        }
    });
    Relay { address, cut, stall }
}

#[test]
fn a_lost_link_recovers_and_revoking_on_the_edge_node_refuses_it() {
    if !crate::common::own_process() {
        return;
    }
    let certs = certs();
    let port = crate::common::free_port();
    let mut gate = start_edge("edge-recover", &certs, port);
    let rt = runtime();
    let direct = format!("https://localhost:{port}");
    let paired = rt
        .block_on(pair(&pair_config(&direct, &code(&gate), "local-b", &certs)))
        .unwrap();
    let relay = relay(&rt, SocketAddr::from(([127, 0, 0, 1], port)));
    let mut local = LocalConfig::new(
        &format!("https://localhost:{}", relay.address.port()),
        &paired.key,
        &paired.pin,
        "local-b",
    );
    quick(&mut local);
    // a silent connection is noticed within about a second
    local.keepalive = Duration::from_secs(1);
    local.connect_timeout = Duration::from_millis(1500);
    let (_shutdown, shutdown) = watch::channel(false);
    let dialer = Dialer::start(local.clone(), rt.handle(), shutdown, None).unwrap();
    let first = wait_status(&dialer, Duration::from_secs(10), "link up", |status| {
        status.state == LinkState::Linked
    });
    wait_until(Duration::from_secs(5), "the down connection", || {
        client(&gate, "local-b")["down"] == "true"
    });
    let before = client(&gate, "local-b")["remoteAddress"].clone();

    // the connections are cut, as a NAT mapping that is dropped: the link comes back, from another address
    relay.cut.send_modify(|cuts| *cuts += 1);
    let second = wait_status(&dialer, Duration::from_secs(10), "link up after the cut", |status| {
        status.state == LinkState::Linked && status.generation > first.generation
    });
    assert!(second.reconnects >= 1, "{second:?}");
    wait_until(Duration::from_secs(5), "the down connection again", || {
        client(&gate, "local-b")["down"] == "true"
    });
    let after = client(&gate, "local-b")["remoteAddress"].clone();
    assert_ne!(before, after, "the new dial comes from another address");

    // the edge node restarts: the link comes back
    gate = gate.restart();
    let third = wait_status(
        &dialer,
        Duration::from_secs(15),
        "link up after the edge node restarts",
        |status| status.state == LinkState::Linked && status.generation > second.generation,
    );
    assert!(third.reconnects > second.reconnects, "{third:?}");

    // the connections go silent, as a NAT mapping dropped without a reset: the Ping times out and the link comes back
    relay.stall.send_modify(|stalls| *stalls += 1);
    let fourth = wait_status(
        &dialer,
        Duration::from_secs(10),
        "link up after the silence",
        |status| status.state == LinkState::Linked && status.generation > third.generation,
    );
    assert!(fourth.reconnects > third.reconnects, "{fourth:?}");

    // revoked on the edge node: the link is cut and the next dial is refused
    let revoked = call(&gate, "revoke#EdgeClient", &[("pairedNodeId", "local-b")]).unwrap();
    assert_eq!(revoked["revoked"], Value::Bool(true));
    let refused = wait_status(&dialer, Duration::from_secs(10), "refused after the revoke", |status| {
        status.state == LinkState::Refused
    });
    assert!(refused.last_error.contains("revoked"), "{refused:?}");
    assert_eq!(client(&gate, "local-b")["linked"], "false");

    // a key read as active just before the revoke still finds no live link: as if the row were read a moment early
    {
        let factory = gate.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let mut row = facade.make_value("moqui.edge.EdgeClient").unwrap();
        row.set("pairedNodeId", Value::string("local-b"));
        row.set("statusId", Value::string("EdgeClientActive"));
        row.update(&facade).unwrap();
    }
    let late = rt.block_on(connect_up(&local)).err().expect("a revoked node links");
    assert!(matches!(late, LinkError::Refused(_)), "{late:?}");
    assert_eq!(client(&gate, "local-b")["linked"], "false");
    assert_eq!(dialer.status().state, LinkState::Refused);
}
