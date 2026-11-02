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

//! Web access through an edge node (plans/Edge-Node.md §5.3, E2). Both ends are real servers in the test's own
//! process: the local node starts first, with its HTTPS listener on a certificate for `pass.localhost`, an SSH server
//! and web protection that denies 203.0.113.0/24; then the edge node (`node_role=edge`, set only after the local node
//! has read its properties) with a certificate for `term.localhost` and an SSH tunnel port. The local node's link is
//! its dialer driven with an explicit `LocalConfig` that serves the local server's own app (`local_web`), so the
//! edge node's hook is the one the services reach.
//!
//! Through the edge node, in terminate mode (`term.localhost`) and passthrough mode (`pass.localhost`): an app page, a
//! `/notws` WebSocket, a DAV PROPFIND and git's smart HTTP; in terminate mode the local node's web protection and its
//! visit see the client the edge node believed. With the link down: the offline page, and passthrough refused. A PROXY
//! header on the local node's own sockets is refused. The SSH tunnel port reaches the local node's SSH server.

#![cfg(all(unix, feature = "edge", feature = "ssh", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use moqui_core::edge::{LinkState, LinkStatus};
use moqui_host::edge_link::{local_web, pair, Backoff, Dialer, LocalConfig, PairConfig};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::common::Gate;

const SERVICES: &str = "org.moqui.impl.EdgeServices";
const TERM: &str = "term.localhost";
const PASS: &str = "pass.localhost";

/// The local node's web protection: on, with one range denied, so a client the edge node names is seen to be refused.
const LOCAL_CONF: &str = r#"<webapp-list><webapp name="webroot"><web-protection enabled="true">
    <client-rule name="edge-test" action="deny" cidr="203.0.113.0/24"/>
</web-protection></webapp></webapp-list>"#;
/// The edge node believes X-Forwarded-For from loopback, so the test can play clients at other addresses.
const EDGE_CONF: &str = r#"<webapp-list><webapp name="webroot" client-ip-header="X-Forwarded-For" trusted-proxies="127.0.0.1"/></webapp-list>"#;

/// A test certificate authority, and certificates (PEM) it signed.
pub(crate) struct Authority {
    pub(crate) root: CertificateDer<'static>,
    issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
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

pub(crate) fn authority() -> Authority {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "Edge Web Test CA");
    params.distinguished_name = name;
    let root = params.self_signed(&key).unwrap().der().clone();
    Authority {
        root,
        issuer: rcgen::Issuer::new(params, key),
    }
}

/// A certificate and key (PEM) for `names`.
pub(crate) fn issue(authority: &Authority, names: &[&str]) -> (String, String) {
    let key = rcgen::KeyPair::generate().unwrap();
    let names: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    let cert = rcgen::CertificateParams::new(names)
        .unwrap()
        .signed_by(&key, &authority.issuer)
        .unwrap();
    (pem("CERTIFICATE", cert.der()), pem("PRIVATE KEY", &key.serialize_der()))
}

/// An EdgeServices service on the edge node, as an administrator.
pub(crate) fn call(gate: &Gate, verb_noun: &str, parameters: &[(&str, &str)]) -> Result<BTreeMap<String, Value>, String> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect();
    let out =
        moqui_core::service::sync(&format!("{SERVICES}.{verb_noun}"), &parameters).map_err(|err| err.to_string())?;
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(format!("{errors:?}"))
    }
}

/// Pair `node` with the edge node: its key and the edge node's pin.
pub(crate) fn pair_node(
    rt: &tokio::runtime::Runtime,
    edge: &Gate,
    url: &str,
    node: &str,
    root: &CertificateDer<'static>,
) -> moqui_host::edge_link::Paired {
    let code = to_plain_string(&call(edge, "create#PairingCode", &[]).unwrap()["code"]);
    let mut config = PairConfig::new(url, &code, node);
    config.trust_root = Some(root.clone());
    config.connect_timeout = Duration::from_secs(5);
    rt.block_on(pair(&config)).unwrap()
}

/// Whether the edge node has `node`'s down connection.
pub(crate) fn down(edge: &Gate, node: &str) -> bool {
    let out = call(edge, "get#EdgeClients", &[]).unwrap();
    let Some(Value::List(list)) = out.get("clientList") else {
        panic!("clientList: {out:?}")
    };
    list.iter().any(|row| {
        let Value::Map(row) = row else { return false };
        to_plain_string(&row["pairedNodeId"]) == node && row["down"] == Value::Bool(true)
    })
}

pub(crate) fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub(crate) fn wait_status(dialer: &Dialer, limit: Duration, what: &str, check: impl Fn(&LinkStatus) -> bool) {
    let started = Instant::now();
    loop {
        let status = dialer.status();
        if check(&status) {
            return;
        }
        assert!(started.elapsed() < limit, "{what}: still {status:?}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn client_config(root: &CertificateDer<'static>) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.clone()).unwrap();
    let mut config =
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

type Tls = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;

/// A browser's TLS connection to `port` for `host`, checked against the test authority.
async fn browser(tls: &Arc<rustls::ClientConfig>, port: u16, host: &str) -> std::io::Result<Tls> {
    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).unwrap();
    tokio_rustls::TlsConnector::from(Arc::clone(tls))
        .connect(name, tcp)
        .await
}

/// One request on a new connection: the status, the head, and the body.
fn https(
    rt: &tokio::runtime::Runtime,
    tls: &Arc<rustls::ClientConfig>,
    port: u16,
    host: &str,
    request: &str,
) -> (u16, String, String) {
    rt.block_on(async {
        let mut stream = browser(tls, port, host).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut answer)).await;
        let answer = String::from_utf8_lossy(&answer).into_owned();
        let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        (status, head.to_string(), body.to_string())
    })
}

fn request(method: &str, path: &str, host: &str, headers: &[(&str, &str)]) -> String {
    let mut text = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("\r\n");
    text
}

/// Open `/notws` with a login key over `port` for `host`, then trade a ping for a pong: the head of the 101 answer.
fn websocket(
    rt: &tokio::runtime::Runtime,
    tls: &Arc<rustls::ClientConfig>,
    port: u16,
    host: &str,
    key: &str,
) -> String {
    rt.block_on(async {
        let mut stream = browser(tls, port, host).await.unwrap();
        let upgrade = format!(
            "GET /notws HTTP/1.1\r\nHost: {host}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
             Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nlogin_key: {key}\r\n\r\n"
        );
        stream.write_all(upgrade.as_bytes()).await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let byte = tokio::time::timeout(Duration::from_secs(10), stream.read_u8())
                .await
                .expect("the upgrade's answer")
                .expect("the upgrade's answer");
            head.push(byte);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        if !head.starts_with("HTTP/1.1 101") {
            return head;
        }
        // a masked ping with "edge" (RFC 6455 §5.2)
        let mask = [1_u8, 2, 3, 4];
        let mut frame = vec![0x89, 0x80 | 4];
        frame.extend_from_slice(&mask);
        frame.extend(b"edge".iter().zip(mask.iter().cycle()).map(|(byte, mask)| byte ^ mask));
        stream.write_all(&frame).await.unwrap();
        loop {
            let mut start = [0_u8; 2];
            tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut start))
                .await
                .expect("a frame back")
                .unwrap();
            let mut payload = vec![0_u8; usize::from(start[1] & 0x7F)];
            stream.read_exact(&mut payload).await.unwrap();
            if start[0] & 0x0F == 0xA {
                assert_eq!(payload, b"edge");
                break;
            }
        }
        head
    })
}

fn has_header(head: &str, name: &str, value: &str) -> bool {
    head.lines().any(|line| {
        line.split_once(':')
            .is_some_and(|(key, text)| key.trim().eq_ignore_ascii_case(name) && text.trim().eq_ignore_ascii_case(value))
    })
}

#[test]
fn the_apps_reach_the_local_node_through_the_edge_node_in_both_modes() {
    if !crate::common::own_process() {
        return;
    }
    let authority = authority();
    let (local_cert, local_key) = issue(&authority, &["localhost", PASS]);
    let (edge_cert, edge_key) = issue(&authority, &["localhost", TERM]);
    let local_port = crate::common::free_port();
    let edge_port = crate::common::free_port();
    let tunnel_port = crate::common::free_port();
    let host_key = base64::engine::general_purpose::STANDARD.encode([5_u8; 32]);
    let local_port_text = local_port.to_string();
    let local = crate::common::start_with_conf_files(
        "edge-web-local",
        None,
        LOCAL_CONF,
        &[
            ("webapp_https_listen_port", &local_port_text),
            ("ssh_port", "0"),
            ("ssh_host_key", &host_key),
        ],
        &[("cert/default.crt", &local_cert), ("cert/default.key", &local_key)],
    );
    let edge_port_text = edge_port.to_string();
    let tunnel_port_text = tunnel_port.to_string();
    let edge = crate::common::start_second_with(
        "edge-web-edge",
        EDGE_CONF,
        &[
            ("node_role", "edge"),
            ("webapp_https_listen_port", &edge_port_text),
            ("edge_ssh_tunnel_port", &tunnel_port_text),
            ("edge_ssh_tunnel_host", "127.0.0.1"),
        ],
        &[("cert/default.crt", &edge_cert), ("cert/default.key", &edge_key)],
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let paired = pair_node(&rt, &edge, &url, "local-w", &authority.root);

    // the hosts: the edge node takes only names of paired nodes, each for one node
    let added = call(
        &edge,
        "add#EdgeHost",
        &[
            ("hostName", "Term.Localhost"),
            ("pairedNodeId", "local-w"),
            ("modeEnumId", "EdgeHostTerminate"),
        ],
    )
    .unwrap();
    assert_eq!(to_plain_string(&added["hostName"]), TERM);
    call(
        &edge,
        "add#EdgeHost",
        &[("hostName", PASS), ("pairedNodeId", "local-w")],
    )
    .unwrap();
    let stranger = call(
        &edge,
        "add#EdgeHost",
        &[("hostName", "x.localhost"), ("pairedNodeId", "nobody")],
    );
    assert!(stranger.unwrap_err().contains("no local node nobody"));
    pair_node(&rt, &edge, &url, "local-other", &authority.root);
    let taken = call(
        &edge,
        "add#EdgeHost",
        &[("hostName", TERM), ("pairedNodeId", "local-other")],
    );
    assert!(taken.unwrap_err().contains("is a host of local node local-w"));
    let hosts = call(&edge, "get#EdgeHosts", &[("pairedNodeId", "local-w")]).unwrap();
    let Some(Value::List(hosts)) = hosts.get("hostList") else {
        panic!("{hosts:?}")
    };
    assert_eq!(hosts.len(), 2);
    // and System › Edge Clients lists them
    let admin = crate::common::sign_in(edge.server.address(), "john.doe", "moqui");
    let page = crate::common::http_request("GET", "/System/EdgeClients", &[("Cookie", &admin)], "");
    let (status, _, page) = crate::common::exchange(edge.server.address(), &page);
    assert_eq!(status, 200, "{page}");
    assert!(
        page.contains("Host names") && page.contains(TERM) && page.contains(PASS),
        "{page}"
    );

    // the local node links, and its down connection serves its own app
    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, "local-w");
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(3);
    config.web = local_web(local.server.factory());
    assert!(config.web.is_some(), "the local server's app");
    let (_shutdown, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_status(&dialer, Duration::from_secs(10), "link up", |status| {
        status.state == LinkState::Linked
    });
    wait_until(Duration::from_secs(5), "the down connection", || down(&edge, "local-w"));

    // what the local node's sockets need: john.doe's login key, and an app password for DAV
    let (login_key, app_password) = {
        let factory = local.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let login_key = moqui_core::user::get_login_key(None).unwrap();
        let _authz = context.disable_authz();
        let parameters = BTreeMap::from([("name".to_string(), Value::string("Edge"))]);
        let out = moqui_core::service::sync("org.moqui.impl.UserServices.create#UserAppPassword", &parameters).unwrap();
        (login_key, to_plain_string(&out["password"]))
    };
    let dav_auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("john.doe:{app_password}"))
    );
    let tls = client_config(&authority.root);

    for host in [TERM, PASS] {
        // an app page
        let (status, _, body) = https(&rt, &tls, edge_port, host, &request("GET", "/Login", host, &[]));
        assert_eq!(status, 200, "{host}: {body}");
        assert!(body.contains("<html"), "{host}: {body}");
        // the notification socket
        let head = websocket(&rt, &tls, edge_port, host, &login_key);
        assert!(head.starts_with("HTTP/1.1 101"), "{host}: {head}");
        assert!(
            has_header(&head, "sec-websocket-accept", "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "{host}: {head}"
        );
        // DAV
        let propfind = request(
            "PROPFIND",
            "/dav/files/john.doe/",
            host,
            &[("Depth", "1"), ("Authorization", &dav_auth)],
        );
        let (status, _, body) = https(&rt, &tls, edge_port, host, &propfind);
        assert_eq!(status, 207, "{host}: {body}");
        assert!(body.contains("/dav/files/john.doe/"), "{host}: {body}");
        // git's smart HTTP asks for credentials
        let refs = request("GET", "/git/hosted.git/info/refs?service=git-upload-pack", host, &[]);
        let (status, head, _) = https(&rt, &tls, edge_port, host, &refs);
        assert_eq!(status, 401, "{host}: {head}");
        assert!(
            head.to_ascii_lowercase().contains("www-authenticate: basic"),
            "{host}: {head}"
        );
    }

    // terminate: the local node sees the client the edge node believed, in web protection and in its visit
    let denied = request("GET", "/Login", TERM, &[("X-Forwarded-For", "203.0.113.9")]);
    assert_eq!(
        https(&rt, &tls, edge_port, TERM, &denied).0,
        403,
        "denied by the local node"
    );
    let allowed = request("GET", "/Login", TERM, &[("X-Forwarded-For", "198.51.100.7")]);
    let (status, head, _) = https(&rt, &tls, edge_port, TERM, &allowed);
    assert_eq!(status, 200, "{head}");
    // the visit starts when the session cookie comes back
    let cookies: Vec<&str> = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("set-cookie"))
        .filter_map(|(_, value)| value.trim().split(';').next())
        .collect();
    assert!(!cookies.is_empty(), "{head}");
    let cookie = cookies.join("; ");
    let again = request(
        "GET",
        "/Login",
        TERM,
        &[("X-Forwarded-For", "198.51.100.7"), ("Cookie", &cookie)],
    );
    assert_eq!(https(&rt, &tls, edge_port, TERM, &again).0, 200);
    {
        let factory = local.server.factory();
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let visits = factory
            .entity()
            .unwrap()
            .find("moqui.server.Visit")
            .unwrap()
            .eq("clientIpAddress", Value::string("198.51.100.7"))
            .count()
            .unwrap();
        assert!(visits >= 1, "no visit from the client the edge node named");
    }
    // a client cannot name itself to the local node: the edge node's own header is replaced
    let forged = request(
        "GET",
        "/Login",
        TERM,
        &[
            ("X-Forwarded-For", "198.51.100.8"),
            ("X-Moqui-Edge-Client", "203.0.113.1:1"),
        ],
    );
    let (status, head, _) = https(&rt, &tls, edge_port, TERM, &forged);
    // the local node saw the client the edge node believed, not the forged one, which it would deny
    assert_eq!(status, 200, "{head}");
    let session: Vec<&str> = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("set-cookie"))
        .filter_map(|(_, value)| value.trim().split(';').next())
        .collect();
    let again = request(
        "GET",
        "/Login",
        TERM,
        &[
            ("X-Forwarded-For", "198.51.100.8"),
            ("X-Moqui-Edge-Client", "203.0.113.1:1"),
            ("Cookie", &session.join("; ")),
        ],
    );
    assert_eq!(https(&rt, &tls, edge_port, TERM, &again).0, 200);
    assert!(visits_from(&local, "198.51.100.8") >= 1, "the local node's visit is not from the believed client");
    assert_eq!(visits_from(&local, "203.0.113.1"), 0, "the local node took the forged client header");
    // and the other way: a forged allowed address does not lift the local node's denial of the believed client
    let hiding = request(
        "GET",
        "/Login",
        TERM,
        &[
            ("X-Forwarded-For", "203.0.113.9"),
            ("X-Moqui-Edge-Client", "198.51.100.1:1"),
        ],
    );
    assert_eq!(https(&rt, &tls, edge_port, TERM, &hiding).0, 403);
    // a passthrough host's name under the edge node's own TLS is misdirected
    let fronted = request("GET", "/Login", PASS, &[]);
    assert_eq!(https(&rt, &tls, edge_port, "localhost", &fronted).0, 421);

    // the SSH tunnel port reaches the local node's SSH server
    let banner = rt.block_on(async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", tunnel_port))
            .await
            .unwrap();
        let mut banner = [0_u8; 8];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut banner))
            .await
            .expect("the SSH server's banner")
            .unwrap();
        banner
    });
    assert_eq!(&banner, b"SSH-2.0-");

    // a PROXY header on the local node's own sockets is never taken
    let proxy = moqui_host_proxy_v2();
    let refused = rt.block_on(async {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", local_port)).await.unwrap();
        tcp.write_all(&proxy).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from(PASS).unwrap();
        tokio_rustls::TlsConnector::from(Arc::clone(&tls))
            .connect(name, tcp)
            .await
    });
    assert!(refused.is_err(), "the HTTPS listener took a PROXY header");
    let (status, _, _) = rt.block_on(async {
        let mut tcp = tokio::net::TcpStream::connect(local.server.address()).await.unwrap();
        let mut bytes = proxy.clone();
        bytes.extend_from_slice(request("GET", "/Login", "localhost", &[]).as_bytes());
        tcp.write_all(&bytes).await.unwrap();
        let mut answer = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut answer)).await;
        let answer = String::from_utf8_lossy(&answer).into_owned();
        let status: u16 = answer
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        (status, (), ())
    });
    assert_ne!(status, 200, "the HTTP listener took a PROXY header");

    // with the link down: the offline page in terminate mode, the connection refused in passthrough, and no SSH
    dialer.stop();
    wait_until(Duration::from_secs(5), "the link down", || !down(&edge, "local-w"));
    let (status, head, body) = https(&rt, &tls, edge_port, TERM, &request("GET", "/Login", TERM, &[]));
    assert_eq!(status, 503, "{head}");
    assert!(has_header(&head, "retry-after", "30"), "{head}");
    assert!(body.contains("Offline"), "{body}");
    let passthrough = rt.block_on(browser(&tls, edge_port, PASS));
    assert!(
        passthrough.is_err(),
        "a passthrough host was answered with its node's link down"
    );
    let ssh = rt.block_on(async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", tunnel_port))
            .await
            .unwrap();
        let mut rest = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await;
        rest
    });
    assert!(ssh.is_empty(), "the SSH tunnel answered with the link down");

    // a removed host is the edge node's own again
    let removed = call(&edge, "remove#EdgeHost", &[("hostName", TERM)]).unwrap();
    assert_eq!(removed["removed"], Value::Bool(true));
    let (status, _, _) = https(&rt, &tls, edge_port, TERM, &request("GET", "/Login", TERM, &[]));
    assert_eq!(status, 200, "the edge node's own login page");
    drop(edge);
}

/// The local node's visits from `client`.
fn visits_from(local: &Gate, client: &str) -> i64 {
    let factory = local.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    factory
        .entity()
        .unwrap()
        .find("moqui.server.Visit")
        .unwrap()
        .eq("clientIpAddress", Value::string(client))
        .count()
        .unwrap()
}

/// A PROXY v2 header naming 203.0.113.50 as the client.
fn moqui_host_proxy_v2() -> Vec<u8> {
    let mut header = vec![
        0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A, 0x21, 0x11, 0, 12,
    ];
    header.extend_from_slice(&[203, 0, 113, 50, 127, 0, 0, 1, 0x9C, 0x40, 0x01, 0xBB]);
    header
}
