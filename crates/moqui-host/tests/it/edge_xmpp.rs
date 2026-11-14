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

//! The XMPP tunnel (plans/archive/Edge-Node.md §5.4, E9): outside XMPP clients sign in through the edge node's tunnel ports
//! and chat with each other on the local node, which ends their TLS. Both ends are real servers in the test's own
//! process, as in `edge_web.rs`: the local node with `xmpp_domain` and a direct TLS port of its own, then the edge node
//! (`node_role=front_edge`), which inherits those properties and still starts no XMPP server, with `edge_xmpp_port`
//! (STARTTLS) and `edge_xmpp_tls_port` (direct TLS).
//!
//! Ann is tokio-xmpp over the direct TLS tunnel (SCRAM-SHA-256-PLUS with the local node's tls-exporter, so the TLS is
//! the local node's); Ben is a raw stream over the STARTTLS tunnel, SASL2 with SCRAM-PLUS and Bind 2. Ann writes to
//! Ben, he answers, and the chat is on the local node. The wizard's Test step's probe (`xmpp_probe`, §5.7, E10)
//! reaches the local node's server through both ports. With the link down both ports are refused, and so is the probe.

#![cfg(all(
    unix,
    feature = "edge",
    feature = "ssh",
    feature = "xmpp",
    feature = "embedded-postgres"
))]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use futures_util::StreamExt as _;
use moqui_core::edge::LinkState;
use moqui_host::edge_link::{local_web, xmpp_probe, Backoff, Dialer, LocalConfig};
use moqui_lib::object::Value;
use moqui_xmpp::framing::{Frame, StreamReader};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_xmpp::minidom::Element;
use tokio_xmpp::xmlstream::Timeouts;
use tokio_xmpp::{Client, Event};

use crate::edge_web::{authority, down, issue, pair_node, wait_status, wait_until};
use crate::xmpp::{as_user, call, get, next_where, stanza, tls_over, DirectTls, Raw, WAIT};

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="EDGE_XMPP_ANN" username="edge.ann" userFullName="Ann Outside"/>
    <moqui.security.UserAccount userId="EDGE_XMPP_BEN" username="edge.ben" userFullName="Ben Outside"/>
</entity-facade-xml>"#;

const STREAM: &str = "<?xml version='1.0'?><stream:stream xmlns='jabber:client' \
    xmlns:stream='http://etherx.jabber.org/streams' to='moqui.local' version='1.0'>";

/// The STARTTLS port: the stream opened in the clear, STARTTLS, then TLS with the local node, read up to the features
/// of the stream inside it.
async fn starttls(address: SocketAddr) -> Raw {
    let mut tcp = TcpStream::connect(address).await.unwrap();
    tcp.write_all(STREAM.as_bytes()).await.unwrap();
    let mut reader = StreamReader::new(1 << 20);
    let until = tokio::time::Instant::now() + WAIT;
    let mut seen: Vec<Element> = Vec::new();
    let mut asked = false;
    loop {
        if !asked {
            if let Some(features) = seen.iter().find(|el| el.name() == "features") {
                assert!(
                    features.has_child("starttls", "urn:ietf:params:xml:ns:xmpp-tls"),
                    "{}",
                    String::from(features)
                );
                tcp.write_all(b"<starttls xmlns='urn:ietf:params:xml:ns:xmpp-tls'/>")
                    .await
                    .unwrap();
                asked = true;
            }
        } else if seen.iter().any(|el| el.name() == "proceed") {
            break;
        }
        let mut buf = [0_u8; 4096];
        let n = tokio::time::timeout_at(until, tcp.read(&mut buf))
            .await
            .expect("the STARTTLS exchange in time")
            .unwrap();
        assert!(n > 0, "the server closed the stream before TLS; got {seen:?}");
        for frame in reader.feed(&buf[..n]).unwrap() {
            if let Frame::Element(element) = frame {
                seen.push(element);
            }
        }
    }
    let (stream, exporter) = tls_over(tcp, false).await;
    Raw::over(stream, exporter).await
}

/// Whether a connection to `address` gets nothing back and is closed within a few seconds, after `first` is sent.
async fn refused(address: SocketAddr, first: &[u8]) -> bool {
    let Ok(mut tcp) = TcpStream::connect(address).await else {
        return true;
    };
    let _ = tcp.write_all(first).await;
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut rest)).await;
    closed.is_ok() && rest.is_empty()
}

#[test]
fn outside_xmpp_clients_chat_through_the_edge_nodes_tunnel_ports() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let authority = authority();
    let (local_cert, local_key) = issue(&authority, &["localhost", "moqui.local"]);
    let (edge_cert, edge_key) = issue(&authority, &["localhost"]);
    let local_port = crate::common::free_port().to_string();
    let local = crate::common::start_with_conf_files(
        "edge-xmpp-local",
        Some(DATA),
        "",
        &[
            ("webapp_https_listen_port", &local_port),
            ("xmpp_domain", "moqui.local"),
            ("xmpp_tls_port", "0"),
            ("xmpp_tls_host", "127.0.0.1"),
        ],
        &[("cert/default.crt", &local_cert), ("cert/default.key", &local_key)],
    );
    assert!(
        local.server.xmpp_addresses().1.is_some(),
        "the local node's own direct TLS port"
    );
    let edge_port = crate::common::free_port();
    let tunnel_port = crate::common::free_port();
    let tunnel_tls_port = crate::common::free_port();
    let (edge_port_text, tunnel_text, tunnel_tls_text) = (
        edge_port.to_string(),
        tunnel_port.to_string(),
        tunnel_tls_port.to_string(),
    );
    // the edge node shares the process's xmpp_domain and xmpp_tls_port, and starts no XMPP server for its role
    let edge = crate::common::start_second_with(
        "edge-xmpp-edge",
        "",
        &[
            ("node_role", "front_edge"),
            ("webapp_https_listen_port", &edge_port_text),
            ("edge_xmpp_port", &tunnel_text),
            ("edge_xmpp_tls_port", &tunnel_tls_text),
            ("edge_xmpp_host", "127.0.0.1"),
        ],
        &[("cert/default.crt", &edge_cert), ("cert/default.key", &edge_key)],
    );
    assert_eq!(
        edge.server.xmpp_addresses(),
        (None, None),
        "an edge node started an XMPP server"
    );
    let tunnel = SocketAddr::from(([127, 0, 0, 1], tunnel_port));
    let tunnel_tls = SocketAddr::from(([127, 0, 0, 1], tunnel_tls_port));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let paired = pair_node(&rt, &edge, &url, "local-x", &authority.root);

    // before the link: nothing answers on the tunnel ports
    assert!(
        rt.block_on(refused(tunnel, STREAM.as_bytes())),
        "the STARTTLS tunnel answered with no link"
    );

    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, "local-x");
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(3);
    config.web = local_web(local.server.factory());
    let (_shutdown, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_status(&dialer, Duration::from_secs(10), "link up", |status| {
        status.state == LinkState::Linked
    });
    wait_until(Duration::from_secs(5), "the down connection", || down(&edge, "local-x"));

    let factory = Arc::clone(local.server.factory());
    // the wizard's Test step: a client stream through each tunnel port comes back to this node's server
    for (port, direct) in [(tunnel_port, false), (tunnel_tls_port, true)] {
        let reached = rt
            .block_on(xmpp_probe(Arc::clone(&factory), url.clone(), port, direct))
            .unwrap_or_else(|why| panic!("the probe (direct {direct}): {why}"));
        assert!(reached.contains("moqui.local"), "{reached}");
    }
    let password = |username: &str| {
        as_user(&factory, username, || {
            get(
                &call(
                    "org.moqui.impl.UserServices.create#UserAppPassword",
                    &[("name", Value::string("phone"))],
                ),
                "password",
            )
        })
    };
    let (ann_password, ben_password) = (password("edge.ann"), password("edge.ben"));

    rt.block_on(async {
        // ---- Ben: the STARTTLS tunnel, SASL2 with SCRAM-SHA-256-PLUS (the local node's exporter) and Bind 2 ----
        let mut ben = starttls(tunnel).await;
        let (mut scram, first) =
            moqui_xmpp::scram::ClientExchange::new("edge.ben", &ben_password, Some(ben.exporter.clone()));
        ben.write(&format!(
            "<authenticate xmlns='urn:xmpp:sasl:2' mechanism='SCRAM-SHA-256-PLUS'><initial-response>{}</initial-response>\
             <user-agent id='5b0c-ben'/><bind xmlns='urn:xmpp:bind:0'><tag>test</tag></bind></authenticate>",
            STANDARD.encode(first)
        ))
        .await;
        let challenge = ben.until(|el| el.name() == "challenge" || el.name() == "failure").await;
        assert_eq!(challenge.name(), "challenge", "{}", String::from(&challenge));
        let last = scram.respond(&STANDARD.decode(challenge.text()).unwrap()).unwrap();
        ben.write(&format!("<response xmlns='urn:xmpp:sasl:2'>{}</response>", STANDARD.encode(last)))
            .await;
        let success = ben.until(|el| el.name() == "success" || el.name() == "failure").await;
        assert_eq!(success.name(), "success", "{}", String::from(&success));
        ben.write("<presence/>").await;

        // ---- Ann: tokio-xmpp on the direct TLS tunnel, SCRAM-SHA-256-PLUS ----
        let jid: tokio_xmpp::jid::Jid = "edge.ann@moqui.local".parse().unwrap();
        let mut ann = Client::new_with_connector(jid, ann_password, DirectTls(tunnel_tls), Timeouts::tight());
        let online = tokio::time::timeout(WAIT, ann.next())
            .await
            .expect("signed in in time")
            .expect("an event");
        let Event::Online { bound_jid, .. } = online else {
            panic!("not online: {online:?}");
        };
        assert!(bound_jid.to_string().starts_with("edge.ann@moqui.local/"), "{bound_jid}");
        ann.send_stanza(stanza("<presence xmlns='jabber:client'/>")).await.unwrap();

        // Ann writes to Ben, and he answers
        ann.send_stanza(stanza(
            "<message xmlns='jabber:client' type='chat' to='edge.ben@moqui.local' id='edge-1'><body>Hello through the edge</body></message>",
        ))
        .await
        .unwrap();
        let got = ben
            .until(|el| {
                el.name() == "message"
                    && el
                        .get_child("body", "jabber:client")
                        .is_some_and(|body| body.text() == "Hello through the edge")
            })
            .await;
        assert_eq!(got.attr("from"), Some("edge.ann@moqui.local"), "{}", String::from(&got));
        ben.write(
            "<message type='chat' to='edge.ann@moqui.local' id='edge-2'><body>Hello back</body></message>",
        )
        .await;
        let answer = next_where(&mut ann, |el| {
            el.name() == "message"
                && el
                    .get_child("body", "jabber:client")
                    .is_some_and(|body| body.text() == "Hello back")
        })
        .await;
        assert!(
            answer.attr("from").is_some_and(|from| from.starts_with("edge.ben@moqui.local")),
            "{}",
            String::from(&answer)
        );
    });

    // the chat is the local node's: the direct room between them has both messages
    let timeline = as_user(&factory, "edge.ann", || {
        let room = get(
            &call(
                "org.moqui.chat.XmppServices.get#DirectRoom",
                &[("userId", Value::string("EDGE_XMPP_BEN"))],
            ),
            "chatRoomId",
        );
        let page = call(
            "org.moqui.chat.ChatServices.get#Timeline",
            &[("chatRoomId", Value::string(&room))],
        );
        format!("{:?}", page.get("eventList"))
    });
    assert!(
        timeline.contains("Hello through the edge") && timeline.contains("Hello back"),
        "{timeline}"
    );

    // with the link down, both ports are refused
    dialer.stop();
    wait_until(Duration::from_secs(5), "the link down", || !down(&edge, "local-x"));
    assert!(
        rt.block_on(refused(tunnel, STREAM.as_bytes())),
        "the STARTTLS tunnel answered with the link down"
    );
    assert!(
        rt.block_on(refused(
            tunnel_tls,
            &[0x16, 0x03, 0x01, 0x00, 0x05, 0x01, 0x00, 0x00, 0x01, 0x00]
        )),
        "the direct TLS tunnel answered with the link down"
    );
    let probe = rt.block_on(xmpp_probe(Arc::clone(&factory), url.clone(), tunnel_port, false));
    assert!(probe.is_err(), "the probe with the link down: {probe:?}");
    drop(edge);
    assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
}
