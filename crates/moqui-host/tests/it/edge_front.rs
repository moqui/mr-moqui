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

//! A front node, and the nodes that dial it (plans/Edge-Node2.md §4.7, E19 part A). Every node is a real server in the
//! test's own process. `node_role` is a process property each server reads as it starts and keeps, so the order of the
//! starts decides each one's role; the edge link hook the services reach is the last one started.
//!
//! - **Regression.** A front node starts none of the edge set's services though they are configured, offers web access
//!   and ports only, refuses a Configure's mail, chat and PASS, a queued item and an edge pairing's link; a node with a
//!   role pairs with a front only. An edge node (`front_edge`) dials its front from `front_node_*`, gives it its local
//!   node's names in their modes, and a visitor reaches the local node through the front in terminate and in
//!   passthrough mode (the passthrough tunnel relayed by the edge node). A local node with two pairings, the edge node
//!   (`edge`) and the front, sends its web names to the front and its mail domain to the edge node, is reached through
//!   the front, its queue goes both ways over the edge pairing only, and unpairing the front leaves the edge pairing. A
//!   front refuses a replication batch, a queued item's content, a delegated call and a PASS call.
//! - **Acceptance** (§5, E19): a front node balances a local cluster with web protection and no edge node.

#![cfg(all(unix, feature = "edge", feature = "ssh", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::edge::setup::EdgeSetup;
use moqui_core::edge::{EdgeHostInfo, LinkState, HOST_PASSTHROUGH, HOST_TERMINATE};
use moqui_host::edge_link::dial::{try_deliver, try_replicate_and_content};
use moqui_host::edge_link::{connect_up, local_web, new_key, pair, Dialer, LocalConfig, MemberKey, PairConfig};
use moqui_lib::object::{to_plain_string, Value};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::common::Gate;
use crate::edge_cluster::{client_config, echoed, enqueue, files, grant_handler, in_context, EDGE_CONF};
use crate::edge_link::{quick, runtime, wait_status};
use crate::edge_web::{authority, call, issue, pair_node, wait_until};

const NODE: &str = "local-front";
/// A local node's pairing on its front, beside its edge pairing [`NODE`].
const WEB_NODE: &str = "local-front-web";
/// A local node's terminate host: the front ends its TLS.
pub(crate) const ERP: &str = "erp.example.com";
/// A local node's passthrough host: its own certificate, through the front and the edge node untouched.
const DESK: &str = "desk.example.com";
const CHECK: Duration = Duration::from_secs(10);

/// A GET of `path` on `host` through the node at `port`, with `headers`: the status (0 when it could not connect) and
/// the head.
pub(crate) fn get(
    rt: &tokio::runtime::Runtime,
    tls: &Arc<rustls::ClientConfig>,
    port: u16,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> (u16, String) {
    request(rt, tls, port, host, ("GET", path), headers, "")
}

/// A request on `host` through the node at `port` on a new connection: the status (0 when it could not connect) and
/// the head.
pub(crate) fn request(
    rt: &tokio::runtime::Runtime,
    tls: &Arc<rustls::ClientConfig>,
    port: u16,
    host: &str,
    (method, path): (&str, &str),
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String) {
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if method == "POST" {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);
    rt.block_on(async {
        let Ok(tcp) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else {
            return (0, String::new());
        };
        let name = rustls::pki_types::ServerName::try_from(host.to_string()).unwrap();
        let Ok(mut stream) = tokio_rustls::TlsConnector::from(Arc::clone(tls))
            .connect(name, tcp)
            .await
        else {
            return (0, String::new());
        };
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut answer)).await;
        let answer = String::from_utf8_lossy(&answer).into_owned();
        let head = answer
            .split_once("\r\n\r\n")
            .map_or(answer.as_str(), |(head, _)| head)
            .to_string();
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        (status, head)
    })
}

/// The host names on a node, read from its rows: each with its pairing and mode.
fn hosts_on(server: &moqui_host::Server) -> BTreeMap<String, (String, String)> {
    in_context(server, |facade| {
        facade
            .find("moqui.edge.EdgeHost")
            .unwrap()
            .use_cache(false)
            .list()
            .unwrap()
            .values()
            .iter()
            .map(|row| {
                let text = |field: &str| to_plain_string(row.get(field).unwrap_or(&Value::Null));
                (text("hostName"), (text("pairedNodeId"), text("modeEnumId")))
            })
            .collect()
    })
}

/// A field of pairing `node`'s row on a node.
fn client_field(server: &moqui_host::Server, node: &str, field: &str) -> String {
    in_context(server, |facade| {
        facade
            .find("moqui.edge.EdgeClient")
            .unwrap()
            .eq("pairedNodeId", Value::string(node))
            .use_cache(false)
            .one()
            .unwrap()
            .and_then(|row| row.get(field).map(to_plain_string))
            .unwrap_or_default()
    })
}

fn host(name: &str, mode: &str) -> EdgeHostInfo {
    EdgeHostInfo {
        host_name: name.to_string(),
        paired_node_id: String::new(),
        mode_enum_id: mode.to_string(),
        sticky_cookie: None,
    }
}

/// A front node: HTTPS on `port`, a link key of its own, a certificate for `localhost` and `names`; `properties` more.
fn front_properties(port: &str, key: &str) -> Vec<(&'static str, String)> {
    vec![
        ("node_role", "front".to_string()),
        ("webapp_https_listen_port", port.to_string()),
        ("edge_link_key", key.to_string()),
    ]
}

fn borrowed<'a>(properties: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    properties.iter().map(|(name, value)| (*name, value.as_str())).collect()
}

#[test]
fn a_front_node_has_the_front_set_only() {
    if !crate::common::own_process() {
        return;
    }
    let authority = authority();
    let cert = issue(&authority, &["localhost", ERP]);
    let port = crate::common::free_port();
    let port_text = port.to_string();
    let mut properties = front_properties(&port_text, &new_key().unwrap());
    // the edge set's services, set up as if this were an edge node: none starts on a front
    for name in ["rtc_media_port", "sip_port", "smtp_listen_port"] {
        properties.push((name, crate::common::free_port().to_string()));
    }
    let front = crate::common::start_with_files(
        "edge-front-only",
        None,
        &borrowed(&properties),
        &[("cert/default.crt", &cert.0), ("cert/default.key", &cert.1)],
    );
    let report = moqui_core::startup::report(front.server.factory()).entries();
    for name in ["Call media", "TURN", "SIP", "SMTP"] {
        let entry = report
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("{name}: {report:?}"));
        assert_eq!(entry.state.detail(), moqui_core::edge::FRONT_ONLY, "{name}: {entry:?}");
    }

    let rt = runtime();
    let url = format!("https://localhost:{port}");
    let paired = pair_node(&rt, &front, &url, NODE, &authority.root);
    assert_eq!(paired.edge.role, "front", "its Hello names its role");

    // an edge pairing's link to it is refused: pair again, and it is kept as the front pairing
    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, NODE);
    quick(&mut config);
    let (_stop, shutdown) = watch::channel(false);
    let wrong = Dialer::start(config.clone(), rt.handle(), shutdown.clone(), None).unwrap();
    wait_status(&wrong, CHECK, "an edge pairing's link refused", |status| {
        status.state == LinkState::Refused && status.last_error.contains("is a front node")
    });
    drop(wrong);

    // a queued item up is refused: a front node has no queue
    config.front = true;
    let up = rt.block_on(connect_up(&config)).unwrap();
    let refused = rt.block_on(try_deliver(&up)).unwrap_err();
    assert!(refused.contains("front node"), "{refused}");
    // nor a replication batch or a queued item's content
    let (replicated, content) = rt.block_on(try_replicate_and_content(&up));
    assert!(replicated.unwrap_err().contains("front node"));
    assert!(content.unwrap_err().contains("front node"));
    drop(up);
    // it delegates no call and sends no PASS call down a link (its hook is the one installed)
    let call = moqui_core::edge::DelegateCall {
        service: "moqui.test.Anything.run#Service".to_string(),
        node: Some(NODE.to_string()),
        ..Default::default()
    };
    let delegated = moqui_core::edge::link().unwrap().delegate(&call).unwrap_err();
    assert!(delegated.message().contains("front node"), "{delegated:?}");
    let pass = moqui_core::pass_authority::sender().unwrap().call(
        "edge:",
        moqui_core::pass_authority::Call {
            operation: "echo".to_string(),
            site_id: "shop".to_string(),
            input: serde_json::json!({}),
            ..Default::default()
        },
    );
    let pass = match pass {
        Ok(reply) => moqui_core::pass_authority::reply_json(&reply).to_string(),
        Err(err) => err.message().to_string(),
    };
    assert!(pass.contains("front node"), "{pass}");

    // the front pairing links; the offer is web access and ports only
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_status(&dialer, CHECK, "the front pairing linked", |status| {
        status.state == LinkState::Linked
    });
    let offer = rt.block_on(dialer.offer()).unwrap();
    assert!(!offer.host_modes.is_empty(), "{offer:?}");
    assert!(
        !offer.mail.available && !offer.chat.webhooks && !offer.pass.available,
        "{offer:?}"
    );
    assert!(!offer.calls.available && !offer.sip.available, "{offer:?}");
    for area in ["mail", "chat", "pass", "calls", "sip"] {
        assert!(
            offer.not_available.iter().any(|named| named == area),
            "{area}: {offer:?}"
        );
    }
    assert!(!offer.not_available.iter().any(|named| named == "web"), "{offer:?}");

    // a Configure's host names are taken, its mail, chat and PASS refused
    let setup = EdgeSetup {
        hosts: vec![host(ERP, HOST_TERMINATE)],
        set_hosts: true,
        mail_domains: vec!["example.com".to_string()],
        chat_bridge_ids: vec!["bridge1".to_string()],
        pass_sites: vec!["shop".to_string()],
        ..EdgeSetup::default()
    };
    let configured = rt.block_on(dialer.configure(&setup)).unwrap();
    let results: BTreeMap<String, String> = configured
        .hosts
        .iter()
        .map(|result| (result.host_name.clone(), result.error.clone()))
        .collect();
    assert_eq!(results.get(ERP).map(String::as_str), Some(""), "{results:?}");
    for item in ["example.com", "bridge1", "shop"] {
        assert!(
            results.get(item).is_some_and(|error| error.contains("front node")),
            "{item}: {results:?}"
        );
    }
    assert_eq!(
        hosts_on(&front.server)[ERP],
        (NODE.to_string(), HOST_TERMINATE.to_string())
    );
}

#[test]
fn an_edge_node_dials_its_front_and_forwards_its_local_nodes_names() {
    if !crate::common::own_process() {
        return;
    }
    let authority = authority();
    let local_cert = issue(&authority, &["localhost", DESK]);
    let local_port = crate::common::free_port().to_string();
    let local = crate::common::start_with_conf_files(
        "edge-front-dial-local",
        None,
        EDGE_CONF,
        &[("webapp_https_listen_port", &local_port)],
        &files(&[("cert/default.crt", &local_cert.0), ("cert/default.key", &local_cert.1)]),
    );
    let front_cert = issue(&authority, &["localhost", ERP]);
    let front_port = crate::common::free_port();
    let front_port_text = front_port.to_string();
    let front = crate::common::start_second_with(
        "edge-front-dial-front",
        "",
        &borrowed(&front_properties(&front_port_text, &new_key().unwrap())),
        &[("cert/default.crt", &front_cert.0), ("cert/default.key", &front_cert.1)],
    );
    let rt = runtime();
    let front_url = format!("https://localhost:{front_port}");
    let edge_pairing = pair_node(&rt, &front, &front_url, "edge-pool", &authority.root);

    // the edge node, with its front pairing in its properties as its .env would have it
    let edge_cert = issue(&authority, &["localhost"]);
    let edge_port = crate::common::free_port();
    let edge_port_text = edge_port.to_string();
    let edge = crate::common::start_second_with(
        "edge-front-dial-edge",
        EDGE_CONF,
        &[
            ("node_role", "front_edge"),
            ("webapp_https_listen_port", &edge_port_text),
            ("edge_link_key", &new_key().unwrap()),
            (moqui_core::edge::FRONT_URL, &front_url),
            (moqui_core::edge::FRONT_KEY, &edge_pairing.key),
            (moqui_core::edge::FRONT_PIN, &edge_pairing.pin),
        ],
        &files(&[("cert/default.crt", &edge_cert.0), ("cert/default.key", &edge_cert.1)]),
    );
    let edge_url = format!("https://localhost:{edge_port}");
    let edge_id = moqui_core::cluster::instance_id(edge.server.factory()).unwrap();

    // a node with a role pairs with a front only, refused before its code is spent
    let code = to_plain_string(&call(&edge, "create#PairingCode", &[]).unwrap()["code"]);
    let mut only_front = PairConfig::new(&edge_url, &code, "another-edge");
    only_front.trust_root = Some(authority.root.clone());
    only_front.only_front = true;
    let refused = rt.block_on(pair(&only_front)).unwrap_err();
    assert!(refused.message().contains("only with a front node"), "{refused}");

    // the local node links to the edge node, which forwards its names
    let local_pairing = pair_node(&rt, &edge, &edge_url, NODE, &authority.root);
    let mut config = LocalConfig::new(&edge_url, &local_pairing.key, &local_pairing.pin, NODE);
    quick(&mut config);
    config.web = local_web(local.server.factory());
    let (_stop, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_status(&dialer, CHECK, "the local node linked", |status| {
        status.state == LinkState::Linked
    });
    for (name, mode) in [(ERP, "EdgeHostTerminate"), (DESK, "EdgeHostPassthrough")] {
        call(
            &edge,
            "add#EdgeHost",
            &[("hostName", name), ("pairedNodeId", NODE), ("modeEnumId", mode)],
        )
        .unwrap();
    }

    // the edge node dialed its front and gave it the names in their modes, under its pairing there
    wait_until(CHECK, "the front has the local node's names", || {
        let hosts = hosts_on(&front.server);
        hosts.get(ERP) == Some(&("edge-pool".to_string(), HOST_TERMINATE.to_string()))
            && hosts.get(DESK) == Some(&("edge-pool".to_string(), HOST_PASSTHROUGH.to_string()))
    });
    let status = call(&edge, "get#LinkStatus", &[]).unwrap();
    assert_eq!(to_plain_string(&status["frontState"]), "linked", "{status:?}");
    assert_eq!(to_plain_string(&status["frontUrl"]), front_url);
    assert_eq!(to_plain_string(&status["edgeRole"]), "front");
    assert_eq!(
        client_field(&front.server, "edge-pool", "lastMemberId"),
        edge_id,
        "pooled there as itself"
    );

    // a visitor reaches the local node through the front: terminate at the front, then down the edge node's link to
    // its local node; passthrough relayed by the edge node to the local node's own TLS
    let tls = client_config(&authority.root);
    wait_until(CHECK, "a terminate host through the front", || {
        get(&rt, &tls, front_port, ERP, "/Login", &[]).0 == 200
    });
    let (status, head) = get(&rt, &tls, front_port, DESK, "/Login", &[]);
    assert_eq!(status, 200, "{head}");
}

#[test]
fn a_local_node_with_two_pairings_routes_web_by_the_front_and_work_by_the_edge() {
    if !crate::common::own_process() {
        return;
    }
    let authority = authority();
    let front_cert = issue(&authority, &["localhost", ERP]);
    let front_port = crate::common::free_port();
    let front_port_text = front_port.to_string();
    let front = crate::common::start_with_files(
        "edge-front-two-front",
        None,
        &borrowed(&front_properties(&front_port_text, &new_key().unwrap())),
        &[("cert/default.crt", &front_cert.0), ("cert/default.key", &front_cert.1)],
    );
    let rt = runtime();
    let front_url = format!("https://localhost:{front_port}");
    // each pairing made while its node's hook is the one the services reach: the last started
    // its pairing on the front by another id: in one process a revoke's cluster notice on one node reaches the other
    // node's links of the same pairing id, which two separate clusters never share
    let to_front = pair_node(&rt, &front, &front_url, WEB_NODE, &authority.root);
    let edge_cert = issue(&authority, &["localhost"]);
    let edge_port = crate::common::free_port();
    let edge_port_text = edge_port.to_string();
    let edge = crate::common::start_second_with(
        "edge-front-two-edge",
        EDGE_CONF,
        &[
            ("node_role", "edge"),
            ("webapp_https_listen_port", &edge_port_text),
            ("edge_link_key", &new_key().unwrap()),
        ],
        &files(&[("cert/default.crt", &edge_cert.0), ("cert/default.key", &edge_cert.1)]),
    );
    grant_handler(&edge.server);
    let edge_url = format!("https://localhost:{edge_port}");
    let to_edge = pair_node(&rt, &edge, &edge_url, NODE, &authority.root);

    // the local node, last, so its hook is the one the edge link services reach, with both pairings as its .env has
    // them
    let local = crate::common::start_second_with(
        "edge-front-two-local",
        EDGE_CONF,
        &[
            ("node_role", ""),
            // the edge node's port is still a property of this process
            ("webapp_https_listen_port", "off"),
            (moqui_core::edge::NODE_URL, &edge_url),
            (moqui_core::edge::NODE_KEY, &to_edge.key),
            (moqui_core::edge::NODE_PIN, &to_edge.pin),
            (moqui_core::edge::NODE_PAIRING, NODE),
            (moqui_core::edge::FRONT_URL, &front_url),
            (moqui_core::edge::FRONT_KEY, &to_front.key),
            (moqui_core::edge::FRONT_PIN, &to_front.pin),
        ],
        &files(&[]),
    );
    grant_handler(&local.server);
    let link = moqui_core::edge::link().unwrap();
    wait_until(CHECK, "both pairings linked", || {
        link.status().state == LinkState::Linked
            && link
                .front_status()
                .is_some_and(|front| front.state == LinkState::Linked)
    });
    assert_eq!(link.status().peer.role, "edge");
    assert!(link
        .front_status()
        .unwrap()
        .members
        .iter()
        .all(|member| !member.carries_work));

    // the offer: the edge pairing's, with the front's web access
    let offer = link.offer().unwrap();
    assert!(!offer.host_modes.is_empty(), "{offer:?}");
    assert!(!offer.not_available.iter().any(|area| area == "web"), "{offer:?}");

    // a Configure goes to each its part: the web name to the front, the mail domain to the edge node
    let setup = EdgeSetup {
        hosts: vec![host(ERP, HOST_TERMINATE)],
        set_hosts: true,
        mail_domains: vec!["example.com".to_string()],
        ..EdgeSetup::default()
    };
    let configured = link.configure(&setup).unwrap();
    assert!(
        configured.hosts.iter().all(|result| result.error.is_empty()),
        "{configured:?}"
    );
    assert_eq!(
        hosts_on(&front.server)[ERP],
        (WEB_NODE.to_string(), HOST_TERMINATE.to_string())
    );
    assert!(hosts_on(&edge.server).is_empty());
    assert!(client_field(&edge.server, NODE, "setupJson").contains("example.com"));
    assert!(!client_field(&front.server, WEB_NODE, "setupJson").contains("example.com"));

    // a visitor reaches it through the front
    let tls = client_config(&authority.root);
    wait_until(CHECK, "its web name through the front", || {
        get(&rt, &tls, front_port, ERP, "/Login", &[]).0 == 200
    });

    // its queue goes both ways over the edge pairing
    enqueue(&edge.server, "down", "to the local node");
    wait_until(CHECK, "an item down from the edge node", || {
        echoed(&local.server, "down") == ["to the local node"]
    });
    enqueue(&local.server, "up", "to the edge node");
    wait_until(CHECK, "an item up to the edge node", || {
        echoed(&edge.server, "up") == ["to the edge node"]
    });

    // unpairing the front revokes it there and forgets it here; the edge pairing stays
    link.unpair_front().unwrap();
    assert!(link.front_status().is_none());
    assert!(moqui_lib::env::get_prop_or_env(moqui_core::edge::FRONT_URL)
        .trim()
        .is_empty());
    assert!(!client_field(&front.server, WEB_NODE, "revokedDate").is_empty());
    let status = link.status();
    assert_eq!(status.state, LinkState::Linked, "{status:?}");
    assert!(link.unpair_front().is_err(), "no front pairing left");
}

// ===== acceptance (plans/Edge-Node2.md §5, E19) =====

/// The front's web protection: it believes X-Forwarded-For from loopback, so the test plays clients, and denies one
/// range.
const FRONT_CONF: &str = r#"<webapp-list><webapp name="webroot" client-ip-header="X-Forwarded-For" trusted-proxies="127.0.0.1">
    <web-protection enabled="true"><client-rule name="front-test" action="deny" cidr="203.0.113.0/24"/></web-protection>
</webapp></webapp-list>"#;

/// Each member's requests so far on the front, by `get#EdgeBalancing`.
fn requests(front: &Gate, node: &str) -> BTreeMap<String, u64> {
    let out = call(front, "get#EdgeBalancing", &[("pairedNodeId", node)]).unwrap();
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

fn taken(before: &BTreeMap<String, u64>, after: &BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    after
        .iter()
        .map(|(id, count)| (id.clone(), count - before.get(id).copied().unwrap_or(0)))
        .collect()
}

/// Sign john.doe in on [`ERP`] through the front with the login form, as a browser does: the cookies.
pub(crate) fn sign_in(rt: &tokio::runtime::Runtime, tls: &Arc<rustls::ClientConfig>, port: u16) -> String {
    let (_, head) = get(rt, tls, port, ERP, "/Login", &[]);
    let cookie = set_cookies("", &head);
    let (status, head) = request(
        rt,
        tls,
        port,
        ERP,
        ("POST", "/Login/login"),
        &[
            ("Cookie", &cookie),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        "username=john.doe&password=moqui",
    );
    assert_eq!(status, 302, "{head}");
    set_cookies(&cookie, &head)
}

/// `cookie` with the cookies a response head sets replacing those of the same name.
fn set_cookies(cookie: &str, head: &str) -> String {
    let headers: Vec<(String, String)> = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    crate::common::merge_cookies(cookie, &headers)
}

/// A member of the local cluster linked to the front over the front pairing: a dialer serving that server's app.
fn front_member(
    rt: &tokio::runtime::Runtime,
    url: &str,
    paired: &moqui_host::edge_link::Paired,
    server: &moqui_host::Server,
    runtime_dir: &std::path::Path,
) -> (Dialer, watch::Sender<bool>) {
    let id = moqui_core::cluster::instance_id(server.factory()).unwrap();
    let mut config = LocalConfig::new(url, &paired.key, &paired.pin, &id);
    quick(&mut config);
    config.front = true;
    config.web = local_web(server.factory());
    config.member_key = Some(MemberKey::load_or_create(runtime_dir).unwrap());
    let (stop, shutdown) = watch::channel(false);
    (Dialer::start(config, rt.handle(), shutdown, None).unwrap(), stop)
}

#[test]
#[ignore = "acceptance: a front node balances a local cluster with web protection and no edge node (Edge-Node2 §5, E19)"]
fn acceptance_a_front_node_balances_a_local_cluster_with_web_protection() {
    if !crate::common::own_process() {
        return;
    }
    let authority = authority();
    let cluster_port = crate::common::free_port().to_string();
    let first =
        crate::common::start_with_properties("edge-front-cluster-1", None, &[("cluster_https_port", &cluster_port)]);
    let second = crate::common::start_member("edge-front-cluster-2", &first.cluster_dir, &[]);
    // the front last: its hook is the one the services reach
    let front_cert = issue(&authority, &["localhost", ERP]);
    let port = crate::common::free_port();
    let port_text = port.to_string();
    let front = crate::common::start_second_with(
        "edge-front-cluster-front",
        FRONT_CONF,
        &borrowed(&front_properties(&port_text, &new_key().unwrap())),
        &[("cert/default.crt", &front_cert.0), ("cert/default.key", &front_cert.1)],
    );
    let rt = runtime();
    let url = format!("https://localhost:{port}");
    let node = moqui_core::cluster::instance_id(first.server.factory()).unwrap();
    let second_id = moqui_core::cluster::instance_id(second.0.factory()).unwrap();
    let paired = pair_node(&rt, &front, &url, &node, &authority.root);
    // what the members' .env would say: pooled behind a front, a new session is written before its response
    moqui_lib::env::set_property(moqui_core::edge::FRONT_URL, url.clone());
    // the first member links on the key; the second is admitted as a member the first one lists
    let (first_dialer, stop_first) = front_member(&rt, &url, &paired, &first.server, &first.runtime);
    wait_status(&first_dialer, CHECK, "the first member linked", |status| {
        status.state == LinkState::Linked
    });
    let (second_dialer, stop_second) = front_member(&rt, &url, &paired, &second.0, &second.1 .0);
    wait_status(&second_dialer, CHECK, "the second member linked", |status| {
        status.state == LinkState::Linked
    });
    let configured = rt
        .block_on(first_dialer.configure(&EdgeSetup {
            hosts: vec![host(ERP, HOST_TERMINATE)],
            set_hosts: true,
            ..EdgeSetup::default()
        }))
        .unwrap();
    assert!(
        configured.hosts.iter().all(|result| result.error.is_empty()),
        "{configured:?}"
    );
    let tls = client_config(&authority.root);
    wait_until(CHECK, "both members take requests", || {
        let before = requests(&front, &node);
        for _ in 0..4 {
            get(&rt, &tls, port, ERP, "/health", &[]);
        }
        let shared = taken(&before, &requests(&front, &node));
        shared.get(&node).copied().unwrap_or(0) > 0 && shared.get(&second_id).copied().unwrap_or(0) > 0
    });

    // evenly, by turn
    let before = requests(&front, &node);
    for _ in 0..10 {
        let (status, head) = get(&rt, &tls, port, ERP, "/health", &[]);
        assert_eq!(status, 200, "{head}");
    }
    let shared = taken(&before, &requests(&front, &node));
    assert_eq!((shared[&node], shared[&second_id]), (5, 5), "{shared:?}");

    // the front's web protection refuses a client before any member sees it
    let before = requests(&front, &node);
    let (status, head) = get(&rt, &tls, port, ERP, "/health", &[("X-Forwarded-For", "203.0.113.9")]);
    assert_eq!(status, 403, "{head}");
    assert!(taken(&before, &requests(&front, &node))
        .values()
        .all(|count| *count == 0));

    // a browser signed in stays on one member, and moves when it stops, still signed in
    let cookie = sign_in(&rt, &tls, port);
    let signed_in = |cookie: &str| {
        let (status, head) = get(
            &rt,
            &tls,
            port,
            ERP,
            "/rest/s1/moqui/basic/enumTypes",
            &[("Cookie", cookie)],
        );
        assert!(matches!(status, 200 | 401 | 403), "{head}");
        status == 200
    };
    let before = requests(&front, &node);
    for _ in 0..4 {
        assert!(signed_in(&cookie), "signed in through the front");
    }
    let took = taken(&before, &requests(&front, &node));
    let on: Vec<&String> = took.iter().filter(|(_, count)| **count > 0).map(|(id, _)| id).collect();
    assert_eq!(on.len(), 1, "one member: {took:?}");
    let on = on[0].clone();
    if on == node {
        stop_first.send(true).unwrap();
    } else {
        stop_second.send(true).unwrap();
    }
    let before = requests(&front, &node);
    wait_until(CHECK, "still signed in on the other member", || signed_in(&cookie));
    let took = taken(&before, &requests(&front, &node));
    assert_eq!(took.get(&on).copied().unwrap_or(0), 0, "{took:?}");
    drop((first_dialer, second_dialer));
    drop(front);
    drop(second);
}
