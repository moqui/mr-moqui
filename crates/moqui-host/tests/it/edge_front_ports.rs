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

//! A front node before an edge cluster (plans/Edge-Node2.md §4.7, E19 part B): its TCP ports and the edge members'
//! link names. Every node is a real server in the test's own process; `node_role` and the other properties are process
//! properties each server reads as it starts, so the order of the starts decides each one's settings.
//!
//! - **SMTP through the front.** An `EdgePort` row of kind `smtp` on the front sends each connection to a member of the
//!   edge pool, a PROXY header naming the client first; the member's SMTP listener takes the client from it (its
//!   allow-list sees the client, not the front), while a direct client's PROXY header is never read. A node with the
//!   edge set refuses an `smtp` row.
//! - **Link names.** Each edge member names its own `edge_link_url` in its front link's Hello; the front keeps it as a
//!   link name of that member and passes its connections through untouched to it alone, picked by the server name
//!   before the ALPN. A local node pairs with and links to an edge member through the front; another member's name goes
//!   to that member only, and is refused while it is down. Only a link name is picked before the ALPN: a link to an
//!   ordinary passthrough host is the front's own, and no pairing's host takes the front's own link address.
//! - **Acceptance** (§5, E19): the full stack, a front node before an edge cluster before a local cluster. The site is
//!   a plain screen site on the local cluster (its app on a terminate host) rather than an mr-pass site: loading mr-pass
//!   and Mantle in a host test is too slow, and what the scenario checks is the path through the tiers.

#![cfg(all(
    unix,
    feature = "edge",
    feature = "ssh",
    feature = "embedded-postgres",
    feature = "inbound-smtp"
))]

use std::net::SocketAddr;
use std::time::Duration;

use base64::Engine as _;
use moqui_core::edge::LinkState;
use moqui_host::edge_link::{new_key, Dialer, LocalConfig};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::common::Gate;
use crate::edge_cluster::{files, links, EDGE_CONF};
use crate::edge_link::{quick, runtime, wait_status};
use crate::edge_web::{authority, call, issue, pair_node, wait_until};

const CHECK: Duration = Duration::from_secs(10);
/// The edge cluster's pairing on the front.
const POOL: &str = "edge-pool";
/// The second edge member's link name; the first's is `localhost`, which the test's dialer reaches.
const EDGE2: &str = "edge2.example.test";

/// A front node: HTTPS on `port`, a link key of its own, certificate `cert`.
fn start_front(label: &str, port: u16, cert: &(String, String)) -> Gate {
    let port = port.to_string();
    let key = new_key().unwrap();
    crate::common::start_with_files(
        label,
        None,
        &[
            ("node_role", "front"),
            ("webapp_https_listen_port", &port),
            ("edge_link_key", &key),
        ],
        &[("cert/default.crt", &cert.0), ("cert/default.key", &cert.1)],
    )
}

/// The front's host names: each with its pairing, mode and member.
fn front_hosts(front: &Gate) -> Vec<(String, String, String, String)> {
    let out = call(front, "get#EdgeHosts", &[]).unwrap();
    let Some(Value::List(list)) = out.get("hostList") else {
        panic!("hostList: {out:?}")
    };
    list.iter()
        .filter_map(|row| {
            let Value::Map(row) = row else { return None };
            let text = |field: &str| row.get(field).map(to_plain_string).unwrap_or_default();
            Some((
                text("hostName"),
                text("pairedNodeId"),
                text("modeEnumId"),
                text("memberId"),
            ))
        })
        .collect()
}

/// An SMTP connection to `port` on loopback from `source`: the greeting, and the connection for more.
fn smtp_from(
    rt: &tokio::runtime::Runtime,
    source: [u8; 4],
    port: u16,
) -> Option<(String, tokio::io::BufReader<tokio::net::TcpStream>)> {
    rt.block_on(async {
        let socket = tokio::net::TcpSocket::new_v4().ok()?;
        socket.bind(SocketAddr::from((source, 0))).ok()?;
        let stream = socket.connect(SocketAddr::from(([127, 0, 0, 1], port))).await.ok()?;
        let mut stream = tokio::io::BufReader::new(stream);
        let greeting = reply(&mut stream).await;
        Some((greeting, stream))
    })
}

/// One reply: its last line (a multi-line reply's lines but the last have `-` after the code); empty at the end.
async fn reply(stream: &mut tokio::io::BufReader<tokio::net::TcpStream>) -> String {
    loop {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read_line(&mut line)).await;
        if !matches!(read, Ok(Ok(count)) if count > 0) || line.as_bytes().get(3) != Some(&b'-') {
            return line.trim_end().to_string();
        }
    }
}

fn command(
    rt: &tokio::runtime::Runtime,
    stream: &mut tokio::io::BufReader<tokio::net::TcpStream>,
    line: &str,
) -> String {
    rt.block_on(async {
        stream
            .get_mut()
            .write_all(format!("{line}\r\n").as_bytes())
            .await
            .unwrap();
        reply(stream).await
    })
}

#[test]
fn a_front_passes_smtp_to_an_edge_member_with_the_clients_address() {
    if !crate::common::own_process() {
        return;
    }
    let authority = authority();
    let front_port = crate::common::free_port();
    let front = start_front("front-smtp-front", front_port, &issue(&authority, &["localhost"]));
    let rt = runtime();
    let front_url = format!("https://localhost:{front_port}");
    let paired = pair_node(&rt, &front, &front_url, POOL, &authority.root);

    // the edge member, the edge cluster's mail edge: its SMTP listener takes mail from 127.0.0.2 only
    let edge_cert = issue(&authority, &["localhost"]);
    let edge_port = crate::common::free_port().to_string();
    let edge = crate::common::start_second_with(
        "front-smtp-edge",
        EDGE_CONF,
        &[
            ("node_role", "front_edge"),
            ("webapp_https_listen_port", &edge_port),
            ("edge_link_key", &new_key().unwrap()),
            ("smtp_listen_port", "0"),
            ("smtp_allowed_addresses", "127.0.0.2"),
            ("edge_link_url", &format!("https://edge1.example.test:{front_port}")),
            (moqui_core::edge::FRONT_URL, &front_url),
            (moqui_core::edge::FRONT_KEY, &paired.key),
            (moqui_core::edge::FRONT_PIN, &paired.pin),
        ],
        &files(&[("cert/default.crt", &edge_cert.0), ("cert/default.key", &edge_cert.1)]),
    );
    let edge_smtp = edge
        .server
        .smtp_address()
        .expect("the edge member's SMTP listener")
        .port();
    // its hook dials the front from its properties and names its own link address there
    let edge_id = moqui_core::cluster::instance_id(edge.server.factory()).unwrap();
    wait_until(CHECK, "the edge member's link name on the front", || {
        front_hosts(&front).contains(&(
            "edge1.example.test".to_string(),
            POOL.to_string(),
            "EdgeHostLinkName".to_string(),
            edge_id.clone(),
        ))
    });

    // an smtp port is a front node's: the edge member refuses one
    let refused = call(
        &edge,
        "add#EdgePort",
        &[("port", "47125"), ("pairedNodeId", POOL), ("tunnelKind", "smtp")],
    )
    .unwrap_err();
    assert!(refused.contains("front node's"), "{refused}");

    // the front's mail port goes to the pool
    let smtp_port = crate::common::free_port();
    call(
        &front,
        "add#EdgePort",
        &[
            ("port", &smtp_port.to_string()),
            ("pairedNodeId", POOL),
            ("tunnelKind", "smtp"),
        ],
    )
    .unwrap();
    // the member's listener sees the client the front names: 127.0.0.2 is taken, 127.0.0.1 refused as not allowed
    let through = std::cell::RefCell::new(None);
    wait_until(CHECK, "SMTP through the front", || {
        *through.borrow_mut() =
            smtp_from(&rt, [127, 0, 0, 2], smtp_port).filter(|(greeting, _)| greeting.starts_with("220 "));
        through.borrow().is_some()
    });
    let (greeting, mut stream) = through.into_inner().unwrap();
    assert!(greeting.ends_with("ESMTP mr-moqui"), "{greeting}");
    assert!(command(&rt, &mut stream, "EHLO mx.sender.example").starts_with("250"));
    assert!(command(&rt, &mut stream, "QUIT").starts_with("221"));
    let (refused, _) = smtp_from(&rt, [127, 0, 0, 1], smtp_port).unwrap();
    assert!(refused.starts_with("554 5.7.1"), "{refused}");

    // a direct client's PROXY header is not read: a command not known, and its own address stays the client
    let (greeting, mut direct) = smtp_from(&rt, [127, 0, 0, 2], edge_smtp).unwrap();
    assert!(greeting.starts_with("220 "), "{greeting}");
    let answer = command(&rt, &mut direct, "PROXY TCP4 127.0.0.1 127.0.0.1 40000 25");
    assert!(answer.starts_with("500 "), "{answer}");
    let (refused, _) = smtp_from(&rt, [127, 0, 0, 1], edge_smtp).unwrap();
    assert!(refused.starts_with("554 5.7.1"), "{refused}");
}

/// A certificate's DER from its PEM.
fn der(pem: &str) -> Vec<u8> {
    let body: String = pem.lines().filter(|line| !line.starts_with("-----")).collect();
    base64::engine::general_purpose::STANDARD.decode(body).unwrap()
}

/// The certificate the server at `port` presents for `name`, over a TLS handshake offering `alpn`: none when the
/// connection closed or the certificate is not trusted for the name. With the pairing ALPN an edge member answers with
/// its own web certificate, so the member is told apart.
fn presented_with(
    rt: &tokio::runtime::Runtime,
    root: &CertificateDer<'static>,
    port: u16,
    name: &str,
    alpn: &[u8],
) -> Option<Vec<u8>> {
    let mut config = (*crate::edge_cluster::client_config(root)).clone();
    config.alpn_protocols = vec![alpn.to_vec()];
    let tls = std::sync::Arc::new(config);
    rt.block_on(async {
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        let server_name = rustls::pki_types::ServerName::try_from(name.to_string()).ok()?;
        let connect = tokio_rustls::TlsConnector::from(tls).connect(server_name, tcp);
        let stream = tokio::time::timeout(Duration::from_secs(5), connect).await.ok()?.ok()?;
        let cert = stream.get_ref().1.peer_certificates()?.first()?;
        Some(cert.as_ref().to_vec())
    })
}

/// [`presented_with`] the link's pairing ALPN, as a local node pairing through the front.
fn presented(rt: &tokio::runtime::Runtime, root: &CertificateDer<'static>, port: u16, name: &str) -> Option<Vec<u8>> {
    presented_with(rt, root, port, name, b"moqui-edge-pair/1")
}

/// An edge member's link to its front, as its hook dials it from `front_node_*` (`LocalConfig::front_from_properties`):
/// a test dialer serving that member's app and relaying its tunnels, naming `link_url` in its Hello. One process keeps
/// only the last started server's hook, so a test with two members that dial dials for them.
fn front_link(
    rt: &tokio::runtime::Runtime,
    url: &str,
    paired: &moqui_host::edge_link::Paired,
    server: &moqui_host::Server,
    runtime_dir: &std::path::Path,
    link_url: &str,
) -> (Dialer, watch::Sender<bool>) {
    let id = moqui_core::cluster::instance_id(server.factory()).unwrap();
    let mut config = LocalConfig::new(url, &paired.key, &paired.pin, &id);
    quick(&mut config);
    config.front = true;
    config.link_url = link_url.to_string();
    config.web = moqui_host::edge_link::local_web(server.factory());
    config.member_key = Some(moqui_host::edge_link::MemberKey::load_or_create(runtime_dir).unwrap());
    let (stop, shutdown) = watch::channel(false);
    (Dialer::start(config, rt.handle(), shutdown, None).unwrap(), stop)
}

#[test]
fn an_edge_members_link_name_passes_through_the_front_to_it_alone() {
    if !crate::common::own_process() {
        return;
    }
    let authority = authority();
    let front_port = crate::common::free_port();
    let front = start_front("front-names-front", front_port, &issue(&authority, &["localhost"]));
    let rt = runtime();
    let paired = pair_node(
        &rt,
        &front,
        &format!("https://localhost:{front_port}"),
        POOL,
        &authority.root,
    );

    // two edge members on one database, one pairing on the front, which each dials at its address (no server name);
    // each names its link address at the front: the first `localhost`, the second EDGE2. Both certificates have both
    // names, so a connection that reached the wrong member would show it
    let front_ip_url = format!("https://127.0.0.1:{front_port}");
    let link_urls = [
        format!("https://localhost:{front_port}"),
        format!("https://{EDGE2}:{front_port}"),
    ];
    let link_key = new_key().unwrap();
    let certs = [
        issue(&authority, &["localhost", EDGE2]),
        issue(&authority, &["localhost", EDGE2]),
    ];
    let (first_port, cluster_port) = (
        crate::common::free_port().to_string(),
        crate::common::free_port().to_string(),
    );
    let first = crate::common::start_second_with(
        "front-names-edge1",
        EDGE_CONF,
        &[
            ("node_role", "front_edge"),
            ("webapp_https_listen_port", &first_port),
            ("cluster_https_port", &cluster_port),
            ("edge_link_key", &link_key),
            ("edge_link_url", &link_urls[0]),
        ],
        &files(&[("cert/default.crt", &certs[0].0), ("cert/default.key", &certs[0].1)]),
    );
    let second_port = crate::common::free_port().to_string();
    let second = crate::common::start_member_with_files(
        "front-names-edge2",
        &first.cluster_dir,
        EDGE_CONF,
        &[
            ("webapp_https_listen_port", &second_port),
            ("edge_link_url", &link_urls[1]),
        ],
        &files(&[("cert/default.crt", &certs[1].0), ("cert/default.key", &certs[1].1)]),
    );
    let ids = [
        moqui_core::cluster::instance_id(first.server.factory()).unwrap(),
        moqui_core::cluster::instance_id(second.0.factory()).unwrap(),
    ];
    let first_link = front_link(
        &rt,
        &front_ip_url,
        &paired,
        &first.server,
        &first.runtime,
        &link_urls[0],
    );
    wait_status(&first_link.0, CHECK, "the first member linked to the front", |status| {
        status.state == LinkState::Linked
    });
    let second_link = front_link(&rt, &front_ip_url, &paired, &second.0, &second.1 .0, &link_urls[1]);
    wait_status(
        &second_link.0,
        CHECK,
        "the second member linked to the front",
        |status| status.state == LinkState::Linked,
    );
    wait_until(CHECK, "each member's link name on the front", || {
        let hosts = front_hosts(&front);
        ["localhost", EDGE2].iter().zip(&ids).all(|(name, id)| {
            hosts.contains(&(
                (*name).to_string(),
                POOL.to_string(),
                "EdgeHostLinkName".to_string(),
                id.clone(),
            ))
        })
    });

    // each name reaches its own member, from one client address (which balancing would send to one member)
    let (first_der, second_der) = (der(&certs[0].0), der(&certs[1].0));
    wait_until(CHECK, "each link name to its own member", || {
        presented(&rt, &authority.root, front_port, "localhost").as_ref() == Some(&first_der)
            && presented(&rt, &authority.root, front_port, EDGE2).as_ref() == Some(&second_der)
    });
    for _ in 0..4 {
        assert_eq!(
            presented(&rt, &authority.root, front_port, EDGE2).as_ref(),
            Some(&second_der)
        );
        assert_eq!(
            presented(&rt, &authority.root, front_port, "localhost").as_ref(),
            Some(&first_der)
        );
    }
    // a link name takes links, their pairing and ACME only: a browser's connection to it is refused at the front
    assert_eq!(
        presented_with(&rt, &authority.root, front_port, EDGE2, b"http/1.1"),
        None
    );

    // a local node pairs with the edge cluster through the front, at the first member's link name, and links to that
    // member there: the pairing (ALPN moqui-edge-pair/1) and the link (moqui-edge/1, its client certificate) pass
    // through the front untouched, by their server name
    let local = pair_node(&rt, &first, &link_urls[0], "local-names", &authority.root);
    let mut config = LocalConfig::new(&link_urls[0], &local.key, &local.pin, "local-names");
    quick(&mut config);
    let (_stop, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_status(
        &dialer,
        CHECK,
        "linked to the first member through the front",
        |status| {
            links(status)
                .get(&ids[0])
                .is_some_and(|(state, _)| *state == LinkState::Linked)
        },
    );

    // the second member's front link stops: its name is refused, never sent to the first member, and stays its own
    let (second_dialer, stop_second) = second_link;
    stop_second.send(true).unwrap();
    drop(second_dialer);
    wait_until(CHECK, "the second member's name refused", || {
        presented(&rt, &authority.root, front_port, EDGE2).is_none()
    });
    for _ in 0..3 {
        assert_eq!(presented(&rt, &authority.root, front_port, EDGE2), None);
    }
    assert!(
        front_hosts(&front)
            .iter()
            .any(|(name, _, _, member)| name == EDGE2 && *member == ids[1]),
        "the name stays the second member's"
    );
    assert_eq!(
        presented(&rt, &authority.root, front_port, "localhost").as_ref(),
        Some(&first_der)
    );
    drop((dialer, first_link));
    drop(second);
    drop(first);
    drop(front);
}

#[test]
fn a_front_keeps_its_own_link_name_and_takes_links_at_other_passthrough_hosts() {
    if !crate::common::own_process() {
        return;
    }
    let authority = authority();
    let front_port = crate::common::free_port();
    // the front's own link address, where its pairings may dial it: no pairing's host name may take it
    let key = new_key().unwrap();
    let front_cert = issue(&authority, &["localhost"]);
    let front = crate::common::start_with_files(
        "front-own-front",
        None,
        &[
            ("node_role", "front"),
            ("webapp_https_listen_port", &front_port.to_string()),
            ("edge_link_key", &key),
            ("edge_link_url", &format!("https://link.front.test:{front_port}")),
        ],
        &[("cert/default.crt", &front_cert.0), ("cert/default.key", &front_cert.1)],
    );
    let rt = runtime();
    let front_url = format!("https://localhost:{front_port}");
    let paired = pair_node(&rt, &front, &front_url, "local-own", &authority.root);
    // a node with no edge set names a link address: the front does not keep it
    let mut config = LocalConfig::new(&front_url, &paired.key, &paired.pin, "local-own");
    quick(&mut config);
    config.front = true;
    config.link_url = format!("https://plain.example.test:{front_port}");
    let (stop, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config.clone(), rt.handle(), shutdown, None).unwrap();
    wait_status(&dialer, CHECK, "the front pairing linked", |status| {
        status.state == LinkState::Linked
    });
    let passthrough = |name: &str| moqui_core::edge::EdgeHostInfo {
        host_name: name.to_string(),
        paired_node_id: String::new(),
        mode_enum_id: "EdgeHostPassthrough".to_string(),
        sticky_cookie: None,
    };
    let configured = rt
        .block_on(dialer.configure(&moqui_core::edge::setup::EdgeSetup {
            hosts: vec![passthrough("link.front.test")],
            set_hosts: true,
            ..moqui_core::edge::setup::EdgeSetup::default()
        }))
        .unwrap();
    let results: std::collections::BTreeMap<String, String> = configured
        .hosts
        .iter()
        .map(|result| (result.host_name.clone(), result.error.clone()))
        .collect();
    assert!(
        results
            .get("link.front.test")
            .is_some_and(|error| error.contains("answers for this name itself")),
        "{results:?}"
    );
    // an ordinary passthrough host of the pairing, named as the front is dialed (the System app's add)
    call(
        &front,
        "add#EdgeHost",
        &[
            ("hostName", "localhost"),
            ("pairedNodeId", "local-own"),
            ("modeEnumId", "EdgeHostPassthrough"),
        ],
    )
    .unwrap();
    let hosts = front_hosts(&front);
    assert!(
        hosts
            .iter()
            .all(|(name, _, mode, _)| name != "plain.example.test" && mode != "EdgeHostLinkName"),
        "{hosts:?}"
    );

    // a link dialed at an ordinary passthrough host is the front's own: it links again there, though that host's node
    // (this one) is away while it dials
    stop.send(true).unwrap();
    drop(dialer);
    let (_stop, shutdown) = watch::channel(false);
    let again = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_status(&again, CHECK, "linked at a passthrough host's name", |status| {
        status.state == LinkState::Linked
    });
    drop(again);
    drop(front);
}

// ===== acceptance (plans/Edge-Node2.md §5, E19) =====

/// An EdgeServices service on `server`, as an administrator (`edge_web::call` for a member that is not a gate).
fn call_on(
    server: &moqui_host::Server,
    verb_noun: &str,
    parameters: &[(&str, &str)],
) -> Result<std::collections::BTreeMap<String, Value>, String> {
    let factory = server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let parameters: std::collections::BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect();
    moqui_core::service::sync(&format!("org.moqui.edge.EdgeServices.{verb_noun}"), &parameters)
        .map_err(|err| err.to_string())
}

/// Each member's requests so far for pairing `node` on the node at `server`, by `get#EdgeBalancing`.
fn requests(server: &moqui_host::Server, node: &str) -> std::collections::BTreeMap<String, u64> {
    let out = call_on(server, "get#EdgeBalancing", &[("pairedNodeId", node)]).unwrap();
    let Some(Value::List(list)) = out.get("memberList") else {
        panic!("memberList: {out:?}")
    };
    list.iter()
        .filter_map(|row| {
            let Value::Map(row) = row else { return None };
            Some((
                to_plain_string(&row["instanceId"]),
                to_plain_string(&row["requests"]).parse().unwrap_or(0),
            ))
        })
        .collect()
}

/// The members that took requests between two counts.
fn took(
    before: &std::collections::BTreeMap<String, u64>,
    after: &std::collections::BTreeMap<String, u64>,
) -> Vec<String> {
    after
        .iter()
        .filter(|(id, count)| **count > before.get(*id).copied().unwrap_or(0))
        .map(|(id, _)| id.clone())
        .collect()
}

/// A member of the local cluster linked to the edge cluster: a dialer serving that server's app.
fn local_member(
    rt: &tokio::runtime::Runtime,
    url: &str,
    paired: &moqui_host::edge_link::Paired,
    server: &moqui_host::Server,
    runtime_dir: &std::path::Path,
) -> (Dialer, watch::Sender<bool>) {
    let id = moqui_core::cluster::instance_id(server.factory()).unwrap();
    let mut config = LocalConfig::new(url, &paired.key, &paired.pin, &id);
    quick(&mut config);
    config.web = moqui_host::edge_link::local_web(server.factory());
    config.member_key = Some(moqui_host::edge_link::MemberKey::load_or_create(runtime_dir).unwrap());
    let (stop, shutdown) = watch::channel(false);
    (Dialer::start(config, rt.handle(), shutdown, None).unwrap(), stop)
}

#[test]
#[ignore = "acceptance: a site through a front node over two edge members and a local cluster, any one node below stopped (Edge-Node2 §5, E19)"]
fn acceptance_the_full_stack_keeps_a_site_up_with_a_node_below_the_front_stopped() {
    if !crate::common::own_process() {
        return;
    }
    use crate::edge_front::{get, request, sign_in, ERP};
    const NODE: &str = "full-local";
    const LONG: Duration = Duration::from_secs(30);
    let authority = authority();
    let rt = runtime();
    // the services reach the last started node's hook, so each pairing is made while its node is the last started, and
    // the front starts last: its balancing is what the test reads
    let front_port = crate::common::free_port();
    let front_url = format!("https://localhost:{front_port}");

    // the local cluster: two members on one database, each dialing the edge cluster
    let local_cluster_port = crate::common::free_port().to_string();
    let local1 = crate::common::start_with_properties(
        "full-local-1",
        None,
        &[("node_role", ""), ("cluster_https_port", &local_cluster_port)],
    );
    let local2 = crate::common::start_member("full-local-2", &local1.cluster_dir, &[]);

    // the edge cluster: a database of its own, held by a plain node, and two members that can each be stopped. The
    // first is reached at its link name through the front, the second at its own address
    let edge_cluster_port = crate::common::free_port().to_string();
    let edge_db = crate::common::start_second_with(
        "full-edge-db",
        EDGE_CONF,
        &[
            ("node_role", ""),
            ("cluster_https_port", &edge_cluster_port),
            ("webapp_https_listen_port", "off"),
        ],
        &files(&[]),
    );
    let link_key = new_key().unwrap();
    let edge_cert = issue(&authority, &["localhost"]);
    let edge_files = files(&[("cert/default.crt", &edge_cert.0), ("cert/default.key", &edge_cert.1)]);
    let edge_ports = [crate::common::free_port(), crate::common::free_port()];
    let edge_urls = [front_url.clone(), format!("https://127.0.0.1:{}", edge_ports[1])];
    let mut edge1 = Some(crate::common::start_member_with_files(
        "full-edge-1",
        &edge_db.cluster_dir,
        EDGE_CONF,
        &[
            ("node_role", "front_edge"),
            ("webapp_https_listen_port", &edge_ports[0].to_string()),
            ("edge_link_key", &link_key),
            ("edge_link_url", &edge_urls[0]),
        ],
        &edge_files,
    ));
    // the local cluster pairs with the edge cluster (at the member's own address: pairing through the front is the
    // regression test's), the same key and pin wherever it dials
    let code = to_plain_string(&call_on(&edge1.as_ref().unwrap().0, "create#PairingCode", &[]).unwrap()["code"]);
    let mut pair_config =
        moqui_host::edge_link::PairConfig::new(&format!("https://localhost:{}", edge_ports[0]), &code, NODE);
    pair_config.trust_root = Some(authority.root.clone());
    let to_edge = rt.block_on(moqui_host::edge_link::pair(&pair_config)).unwrap();
    let mut edge2 = Some(crate::common::start_member_with_files(
        "full-edge-2",
        &edge_db.cluster_dir,
        EDGE_CONF,
        &[
            ("webapp_https_listen_port", &edge_ports[1].to_string()),
            ("edge_link_url", &edge_urls[1]),
        ],
        &edge_files,
    ));
    let edge_ids = [
        moqui_core::cluster::instance_id(edge1.as_ref().unwrap().0.factory()).unwrap(),
        moqui_core::cluster::instance_id(edge2.as_ref().unwrap().0.factory()).unwrap(),
    ];
    // the site: the local cluster's app on a terminate host of the edge cluster (the last started member's hook tells
    // the other to load its names again)
    call_on(
        &edge2.as_ref().unwrap().0,
        "add#EdgeHost",
        &[
            ("hostName", ERP),
            ("pairedNodeId", NODE),
            ("modeEnumId", "EdgeHostTerminate"),
        ],
    )
    .unwrap();

    // the front, last
    let front_cert = issue(&authority, &["localhost", ERP]);
    let front = crate::common::start_second_with(
        "full-front",
        "",
        &[
            ("node_role", "front"),
            ("webapp_https_listen_port", &front_port.to_string()),
            ("edge_link_key", &new_key().unwrap()),
        ],
        &[("cert/default.crt", &front_cert.0), ("cert/default.key", &front_cert.1)],
    );
    let to_front = pair_node(&rt, &front, &front_url, POOL, &authority.root);
    // each edge member's front link, as its hook dials it (one process keeps one hook: see front_link)
    let front_ip_url = format!("https://127.0.0.1:{front_port}");
    let edge_runtimes = [
        edge1.as_ref().unwrap().1 .0.clone(),
        edge2.as_ref().unwrap().1 .0.clone(),
    ];
    let mut edge_links = [
        Some(front_link(
            &rt,
            &front_ip_url,
            &to_front,
            &edge1.as_ref().unwrap().0,
            &edge_runtimes[0],
            &edge_urls[0],
        )),
        Some(front_link(
            &rt,
            &front_ip_url,
            &to_front,
            &edge2.as_ref().unwrap().0,
            &edge_runtimes[1],
            &edge_urls[1],
        )),
    ];
    for (link, _) in edge_links.iter().flatten() {
        wait_status(link, LONG, "an edge member linked to the front", |status| {
            status.state == LinkState::Linked
        });
    }
    // the edge members give the front the site's name (their hook's Configure, front::names_for_front)
    let configured = rt
        .block_on(
            edge_links[0]
                .as_ref()
                .unwrap()
                .0
                .configure(&moqui_core::edge::setup::EdgeSetup {
                    hosts: vec![moqui_core::edge::EdgeHostInfo {
                        host_name: ERP.to_string(),
                        paired_node_id: String::new(),
                        mode_enum_id: "EdgeHostTerminate".to_string(),
                        sticky_cookie: None,
                    }],
                    set_hosts: true,
                    ..moqui_core::edge::setup::EdgeSetup::default()
                }),
        )
        .unwrap();
    assert!(
        configured.hosts.iter().all(|result| result.error.is_empty()),
        "{configured:?}"
    );

    // the local members link to both edge members: the first through the front at its link name, the second directly
    moqui_lib::env::set_property(moqui_core::edge::NODE_URL, front_url.clone());
    let mut dialers = vec![
        Some(local_member(&rt, &front_url, &to_edge, &local1.server, &local1.runtime)),
        Some(local_member(&rt, &front_url, &to_edge, &local2.0, &local2.1 .0)),
    ];
    for (dialer, _) in dialers.iter().flatten() {
        wait_status(dialer, LONG, "a local member linked to both edge members", |status| {
            let links = links(status);
            edge_ids
                .iter()
                .all(|id| links.get(id).is_some_and(|(state, _)| *state == LinkState::Linked))
        });
    }

    let tls = crate::edge_cluster::client_config(&authority.root);
    wait_until(LONG, "the site through the front", || {
        get(&rt, &tls, front_port, ERP, "/Login", &[]).0 == 200
    });

    // visitors are balanced over both edge members
    wait_until(CHECK, "both edge members take the site's requests", || {
        let before = requests(&front.server, POOL);
        for _ in 0..6 {
            get(&rt, &tls, front_port, ERP, "/health", &[]);
        }
        took(&before, &requests(&front.server, POOL)).len() == 2
    });

    // a signed-in browser stays on one edge member
    let cookie = sign_in(&rt, &tls, front_port);
    let signed_in = |cookie: &str| {
        let (status, head) = request(
            &rt,
            &tls,
            front_port,
            ERP,
            ("GET", "/rest/s1/moqui/basic/enumTypes"),
            &[("Cookie", cookie)],
            "",
        );
        assert!(matches!(status, 0 | 200 | 401 | 403 | 502 | 503), "{head}");
        status == 200
    };
    let before = requests(&front.server, POOL);
    for _ in 0..4 {
        assert!(signed_in(&cookie), "signed in through the front");
    }
    let on_edge = took(&before, &requests(&front.server, POOL));
    assert_eq!(on_edge.len(), 1, "one edge member: {on_edge:?}");

    // that edge member stops (its front link, then the server): the other takes the site, still signed in
    let index = edge_ids.iter().position(|id| *id == on_edge[0]).unwrap();
    if let Some((dialer, stop)) = edge_links[index].take() {
        stop.send(true).unwrap();
        drop(dialer);
    }
    if index == 0 {
        drop(edge1.take());
    } else {
        drop(edge2.take());
    }
    crate::edge_cluster::in_context(&edge_db.server, |facade| {
        facade
            .find("moqui.server.ClusterMember")
            .unwrap()
            .eq("instanceId", Value::string(&on_edge[0]))
            .delete_all()
            .unwrap();
    });
    wait_until(LONG, "still signed in through the other edge member", || {
        signed_in(&cookie)
    });
    // (the stopped server let go of this process's one edge link hook, the front's, so its balancing is not read
    // after this: the site staying up is what is checked)
    for _ in 0..4 {
        assert!(signed_in(&cookie), "signed in through the other edge member");
    }

    // a local member stops too: the other takes the site, still signed in
    if let Some((dialer, stop)) = dialers[0].take() {
        stop.send(true).unwrap();
        drop(dialer);
    }
    wait_until(LONG, "still signed in on the other local member", || signed_in(&cookie));
    for _ in 0..4 {
        assert!(
            signed_in(&cookie),
            "signed in with one edge member and one local member"
        );
    }
    drop(dialers);
    drop(edge_links);
    drop((edge1, edge2, edge_db, front, local2, local1));
}
