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

//! The comms of several pairings on one edge node (plans/Edge-Node2.md §4.1, E16): XMPP clients of two local nodes'
//! domains on one pair of ports, each to the pairing whose domain it names (the TLS server name on the direct TLS
//! port, the stream's `to` on the STARTTLS port), and a port of one pairing (an `EdgePort` row) to that pairing only.
//!
//! Both ends are real servers in the test's own process, as in `edge_xmpp.rs`: a local node with `xmpp_domain` and
//! its own XMPP server, linked as `local-x`, and a second pairing `local-y`, linked by a dialer that serves no tunnels
//! (any tunnel to it is refused), so a connection that reaches the local node's XMPP server went down `local-x`'s link.

#![cfg(all(unix, feature = "edge", feature = "xmpp", feature = "embedded-postgres"))]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::edge::{EdgeSetup, LinkState};
use moqui_host::edge_link::{local_web, xmpp_probe, Backoff, Dialer, LocalConfig};
use moqui_lib::object::{to_plain_string, Value};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::watch;

use crate::edge_web::{authority, call, down, issue, pair_node, wait_status, wait_until};

/// A STARTTLS stream's header naming `domain`.
fn header(domain: &str) -> String {
    format!(
        "<?xml version='1.0'?><stream:stream xmlns='jabber:client' \
         xmlns:stream='http://etherx.jabber.org/streams' to='{domain}' version='1.0'>"
    )
}

/// Whether a connection to `address` that sends `first` gets nothing back and is closed within a few seconds.
async fn refused(address: SocketAddr, first: &[u8]) -> bool {
    let Ok(mut tcp) = TcpStream::connect(address).await else {
        return true;
    };
    let _ = tcp.write_all(first).await;
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut rest)).await;
    closed.is_ok() && rest.is_empty()
}

/// Whether a direct TLS connection to `address` naming `server_name` is closed before any server answer: a refusal,
/// not a certificate the client did not take.
async fn refused_tls(
    address: SocketAddr,
    server_name: &str,
    root: &rustls::pki_types::CertificateDer<'static>,
) -> bool {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.clone()).unwrap();
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(address).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from(server_name.to_string()).unwrap();
    match tokio::time::timeout(Duration::from_secs(5), connector.connect(name, tcp)).await {
        Ok(Err(err)) => err.kind() != std::io::ErrorKind::InvalidData,
        _ => false,
    }
}

/// The port rows of the edge node, as `get#EdgePorts` gives them: (port, listening).
fn port_rows(edge: &crate::common::Gate) -> Vec<(String, bool)> {
    let out = call(edge, "get#EdgePorts", &[]).unwrap();
    let Some(Value::List(rows)) = out.get("portList") else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| match row {
            Value::Map(row) => Some((
                row.get("port").map(to_plain_string).unwrap_or_default(),
                row.get("listening").map(to_plain_string).unwrap_or_default() == "true",
            )),
            _ => None,
        })
        .collect()
}

/// A free port no service of the edge node's takes (`moqui_core::edge::ports::reserved`).
fn row_port() -> u16 {
    loop {
        let port = crate::common::free_port();
        if !(51310..=51320).contains(&port) && !(42690..=42699).contains(&port) && port > 10_025 {
            return port;
        }
    }
}

#[test]
fn two_pairings_xmpp_domains_share_one_pair_of_ports_and_a_port_row_goes_to_its_pairing() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let authority = authority();
    let (local_cert, local_key) = issue(&authority, &["localhost", "moqui.local"]);
    let (edge_cert, edge_key) = issue(&authority, &["localhost"]);
    let local_port = crate::common::free_port().to_string();
    let local = crate::common::start_with_conf_files(
        "edge-domains-local",
        None,
        "",
        &[
            ("webapp_https_listen_port", &local_port),
            ("xmpp_domain", "moqui.local"),
            ("xmpp_tls_port", "0"),
            ("xmpp_tls_host", "127.0.0.1"),
        ],
        &[("cert/default.crt", &local_cert), ("cert/default.key", &local_key)],
    );
    let (edge_port, tunnel_port, tunnel_tls_port) = (
        crate::common::free_port(),
        crate::common::free_port(),
        crate::common::free_port(),
    );
    let edge = crate::common::start_second_with(
        "edge-domains-edge",
        "",
        &[
            ("node_role", "front_edge"),
            ("webapp_https_listen_port", &edge_port.to_string()),
            ("edge_xmpp_port", &tunnel_port.to_string()),
            ("edge_xmpp_tls_port", &tunnel_tls_port.to_string()),
            ("edge_xmpp_host", "127.0.0.1"),
            // narrow media ranges, so a free port is not one of them (a port row may not take this node's own)
            ("turn_relay_ports", "51310-51320"),
            ("sip_rtp_ports", "42690-42699"),
        ],
        &[("cert/default.crt", &edge_cert), ("cert/default.key", &edge_key)],
    );
    let tunnel = SocketAddr::from(([127, 0, 0, 1], tunnel_port));
    let tunnel_tls = SocketAddr::from(([127, 0, 0, 1], tunnel_tls_port));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let factory = Arc::clone(local.server.factory());
    let (_shutdown, shutdown) = watch::channel(false);
    // local-x serves the local node; local-y serves no tunnels, so one sent to it is refused
    let mut dialers = Vec::new();
    for (node, domain, serves) in [("local-x", "moqui.local", true), ("local-y", "other.example", false)] {
        let paired = pair_node(&rt, &edge, &url, node, &authority.root);
        let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, node);
        config.backoff = Backoff {
            first: Duration::from_millis(100),
            max: Duration::from_millis(500),
        };
        config.connect_timeout = Duration::from_secs(3);
        if serves {
            config.web = local_web(&factory);
        }
        let dialer = Dialer::start(config, rt.handle(), shutdown.clone(), None).unwrap();
        wait_status(&dialer, Duration::from_secs(10), "link up", |status| {
            status.state == LinkState::Linked
        });
        wait_until(Duration::from_secs(5), "the down connection", || down(&edge, node));
        let setup = EdgeSetup {
            xmpp_domain: domain.to_string(),
            ..EdgeSetup::default()
        };
        let configured = rt.block_on(dialer.configure(&setup)).unwrap();
        assert!(configured.offer.xmpp.available, "{:?}", configured.offer.xmpp);
        assert!(!configured.offer.xmpp.needs_node, "{node} named its domain");
        dialers.push(dialer);
    }

    // the domain of local-x, on each port: by the stream's to (STARTTLS) and by the server name (direct TLS)
    for (port, direct) in [(tunnel_port, false), (tunnel_tls_port, true)] {
        let reached = rt
            .block_on(xmpp_probe(Arc::clone(&factory), url.clone(), port, direct))
            .unwrap_or_else(|why| panic!("local-x's domain (direct {direct}): {why}"));
        assert!(reached.contains("moqui.local"), "{reached}");
    }
    // local-y's domain goes down local-y's link only (which takes no tunnel), and a domain of no one is refused
    assert!(
        rt.block_on(refused(tunnel, header("other.example").as_bytes())),
        "STARTTLS to local-y"
    );
    assert!(
        rt.block_on(refused_tls(tunnel_tls, "other.example", &authority.root)),
        "direct TLS to local-y"
    );
    assert!(
        rt.block_on(refused(tunnel, header("nobody.example").as_bytes())),
        "a domain of no one"
    );
    assert!(rt.block_on(refused_tls(tunnel_tls, "nobody.example", &authority.root)));

    // a port of one pairing each: local-x's direct TLS row reaches it, local-y's refuses local-x's domain
    let (row_x, row_y) = (row_port(), row_port());
    call(
        &edge,
        "add#EdgePort",
        &[
            ("port", &row_x.to_string()),
            ("pairedNodeId", "local-x"),
            ("tunnelKind", "xmpps"),
        ],
    )
    .unwrap();
    call(
        &edge,
        "add#EdgePort",
        &[
            ("port", &row_y.to_string()),
            ("pairedNodeId", "local-y"),
            ("tunnelKind", "xmpp"),
        ],
    )
    .unwrap();
    let taken = call(
        &edge,
        "add#EdgePort",
        &[
            ("port", &row_x.to_string()),
            ("pairedNodeId", "local-y"),
            ("tunnelKind", "xmpps"),
        ],
    );
    assert!(taken.is_err(), "a port another pairing has: {taken:?}");
    wait_until(Duration::from_secs(5), "the rows' ports listening", || {
        port_rows(&edge).iter().filter(|(_, listening)| *listening).count() == 2
    });
    let reached = rt
        .block_on(xmpp_probe(Arc::clone(&factory), url.clone(), row_x, true))
        .unwrap_or_else(|why| panic!("local-x's own port: {why}"));
    assert!(reached.contains("moqui.local"), "{reached}");
    let row_y_address = SocketAddr::from(([127, 0, 0, 1], row_y));
    assert!(
        rt.block_on(refused(row_y_address, header("moqui.local").as_bytes())),
        "local-y's port goes to local-y, whatever domain it names"
    );
    // removed, the port closes
    call(&edge, "remove#EdgePort", &[("port", &row_x.to_string())]).unwrap();
    wait_until(Duration::from_secs(5), "the removed port closed", || {
        std::net::TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], row_x))).is_err()
    });
    // local-y away: what names no pairing's domain is still refused, never sent to local-x, the one still linked
    dialers[1].stop();
    wait_until(Duration::from_secs(5), "local-y's link down", || {
        !down(&edge, "local-y")
    });
    assert!(
        rt.block_on(refused(tunnel, header("nobody.example").as_bytes())),
        "a domain of no one, one away"
    );
    assert!(
        rt.block_on(refused(tunnel, b"<stream:stream xmlns='jabber:client' version='1.0'>")),
        "a stream naming no domain, one away"
    );
    assert!(rt.block_on(refused_tls(tunnel_tls, "nobody.example", &authority.root)));
    assert!(
        rt.block_on(refused(tunnel, header("other.example").as_bytes())),
        "local-y's domain while it is away"
    );
    // local-y revoked: its row stays, and its port closes
    call(&edge, "revoke#EdgeClient", &[("pairedNodeId", "local-y")]).unwrap();
    wait_until(Duration::from_secs(5), "a revoked pairing's port closed", || {
        std::net::TcpStream::connect(row_y_address).is_err()
    });
    assert!(port_rows(&edge)
        .iter()
        .any(|(port, listening)| *port == row_y.to_string() && !listening));
    for dialer in &dialers {
        dialer.stop();
    }
    drop(edge);
    assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
}

/// The comms parts of E16's acceptance (plans/Edge-Node2.md §5): two local nodes paired with one edge node, each with
/// its own XMPP domain on the one pair of ports and its own TURN secret; revoking one ends its TURN access and its XMPP
/// domain there while the other goes on. Run once when written: `cargo test -p moqui-host --test it --
/// --ignored edge_domains_comms::two_local_nodes_comms_stay_apart`.
#[cfg(feature = "rtc")]
#[test]
#[ignore = "acceptance: E16 comms, two local nodes on one edge node (plans/Edge-Node2.md §5)"]
fn two_local_nodes_comms_stay_apart() {
    if !crate::common::own_process() {
        return;
    }
    const EDGE_SECRET: &str = "edge-domains-turn-secret";
    let authority = authority();
    let (local_cert, local_key) = issue(&authority, &["localhost", "moqui.local"]);
    let (edge_cert, edge_key) = issue(&authority, &["localhost"]);
    let local_port = crate::common::free_port().to_string();
    let local = crate::common::start_with_conf_files(
        "edge-domains-acc-local",
        None,
        "",
        &[
            ("webapp_https_listen_port", &local_port),
            ("xmpp_domain", "moqui.local"),
            ("xmpp_tls_port", "0"),
            ("xmpp_tls_host", "127.0.0.1"),
        ],
        &[("cert/default.crt", &local_cert), ("cert/default.key", &local_key)],
    );
    let (edge_port, tunnel_port, tunnel_tls_port) = (
        crate::common::free_port(),
        crate::common::free_port(),
        crate::common::free_port(),
    );
    let (media_port, turn_port) = (crate::common::free_port(), crate::common::free_port());
    let media = format!("127.0.0.1:{media_port}");
    let edge = crate::common::start_second_with(
        "edge-domains-acc-edge",
        "",
        &[
            ("node_role", "front_edge"),
            ("webapp_https_listen_port", &edge_port.to_string()),
            ("edge_xmpp_port", &tunnel_port.to_string()),
            ("edge_xmpp_tls_port", &tunnel_tls_port.to_string()),
            ("edge_xmpp_host", "127.0.0.1"),
            ("rtc_media_port", &media_port.to_string()),
            ("rtc_media_host", "127.0.0.1"),
            ("rtc_public_address", &media),
            ("turn_port", &turn_port.to_string()),
            ("turn_host", "127.0.0.1"),
            ("turn_public_host", "127.0.0.1"),
            ("turn_relay_ports", "51410-51500"),
            ("turn_auth_secret", EDGE_SECRET),
        ],
        &[("cert/default.crt", &edge_cert), ("cert/default.key", &edge_key)],
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let factory = Arc::clone(local.server.factory());
    let (_shutdown, shutdown) = watch::channel(false);
    let mut dialers = Vec::new();
    for (node, domain, serves) in [("local-x", "moqui.local", true), ("local-y", "other.example", false)] {
        let paired = pair_node(&rt, &edge, &url, node, &authority.root);
        let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, node);
        config.backoff = Backoff {
            first: Duration::from_millis(100),
            max: Duration::from_millis(500),
        };
        config.connect_timeout = Duration::from_secs(3);
        if serves {
            config.web = local_web(&factory);
        }
        let dialer = Dialer::start(config, rt.handle(), shutdown.clone(), None).unwrap();
        wait_status(&dialer, Duration::from_secs(10), "link up", |status| {
            status.state == LinkState::Linked
        });
        let setup = EdgeSetup {
            xmpp_domain: domain.to_string(),
            ..EdgeSetup::default()
        };
        rt.block_on(dialer.configure(&setup)).unwrap();
        dialers.push(dialer);
    }
    let edge_id = dialers[0].status().peer.instance_id;
    let turn_url = format!("turn:127.0.0.1:{turn_port}?transport=udp");

    // TURN: local-x signs with the secret its link handed it, naming its pairing; local-y's is its own
    let ice = crate::xmpp::as_user(&factory, "john.doe", || {
        crate::xmpp::call(
            "org.moqui.call.CallServices.make#IceServers",
            &[
                ("callLegId", Value::string("100010")),
                ("mediaNodeId", Value::string(&edge_id)),
                ("turnUrls", Value::List(vec![Value::string(&turn_url)])),
            ],
        )
    });
    let (_, x_user, x_credential) = crate::rtc::ice_servers(&ice);
    assert!(x_user.ends_with(":100010@local-x"), "{x_user}");
    let expiry = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 600
    };
    // a pairing's secret: of its id, and of its key's fingerprint and when it paired (the edge node's rows)
    let secret_of = |node: &str| {
        let edge_factory = edge.server.factory();
        let context = edge_factory.new_context();
        let _entered = context.enter();
        let salt = moqui_core::edge::media::pairing_turn_salt(&edge_factory.entity().unwrap(), node)
            .unwrap()
            .unwrap_or_else(|| panic!("{node} is not active"));
        moqui_rtc::turn::pairing_secret(EDGE_SECRET.as_bytes(), node, &salt)
    };
    let y_secret = secret_of("local-y");
    let (y_user, y_credential) = moqui_rtc::turn::credential(y_secret.as_bytes(), "200020@local-y", expiry());
    let probe = |user: &str, credential: &str| moqui_host::edge_link::turn_probe(&turn_url, user, credential, &media);
    // the TURN server reads the active pairings every second: a pairing just made is taken within it
    wait_until(Duration::from_secs(8), "local-x's TURN credential taken", || {
        probe(&x_user, &x_credential).is_ok()
    });
    probe(&y_user, &y_credential).unwrap_or_else(|why| panic!("local-y's credential: {why}"));
    let x_secret = secret_of("local-x");
    let (crossed, crossed_credential) = moqui_rtc::turn::credential(x_secret.as_bytes(), "200021@local-y", expiry());
    assert!(
        probe(&crossed, &crossed_credential).is_err(),
        "local-x's secret for local-y's leg"
    );

    // XMPP: each domain to its own pairing on the one pair of ports
    let reached = rt
        .block_on(xmpp_probe(Arc::clone(&factory), url.clone(), tunnel_tls_port, true))
        .unwrap_or_else(|why| panic!("local-x's domain: {why}"));
    assert!(reached.contains("moqui.local"), "{reached}");
    let tunnel = SocketAddr::from(([127, 0, 0, 1], tunnel_port));
    assert!(
        rt.block_on(refused(tunnel, header("other.example").as_bytes())),
        "local-y's domain"
    );

    // revoking local-x ends its TURN access, with no new turn_auth_secret, and its domain there; local-y goes on
    call(&edge, "revoke#EdgeClient", &[("pairedNodeId", "local-x")]).unwrap();
    wait_until(Duration::from_secs(8), "local-x's TURN credential refused", || {
        probe(&x_user, &x_credential).is_err()
    });
    let (y_user, y_credential) = moqui_rtc::turn::credential(y_secret.as_bytes(), "200022@local-y", expiry());
    probe(&y_user, &y_credential).unwrap_or_else(|why| panic!("local-y after local-x was revoked: {why}"));
    let x_tls = SocketAddr::from(([127, 0, 0, 1], tunnel_tls_port));
    assert!(
        rt.block_on(refused_tls(x_tls, "moqui.local", &authority.root)),
        "a revoked pairing's domain"
    );
    // paired again under the same id, with a new key: the credentials of the pairing before stay refused
    for dialer in &dialers {
        dialer.stop();
    }
    pair_node(&rt, &edge, &url, "local-x", &authority.root);
    let (old, old_credential) = moqui_rtc::turn::credential(x_secret.as_bytes(), "100011@local-x", expiry());
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        probe(&x_user, &x_credential).is_err(),
        "a credential seen before, after pairing again"
    );
    assert!(
        probe(&old, &old_credential).is_err(),
        "the secret before, after pairing again"
    );
    let (new, new_credential) =
        moqui_rtc::turn::credential(secret_of("local-x").as_bytes(), "100012@local-x", expiry());
    probe(&new, &new_credential).unwrap_or_else(|why| panic!("the new pairing's credential: {why}"));
    for dialer in &dialers {
        dialer.stop();
    }
    drop(edge);
}
