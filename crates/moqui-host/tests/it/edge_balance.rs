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

//! Balancing a pool's traffic (plans/Edge-Node2.md §4.4, E17 part B). The edge node is a real server
//! (`node_role=front_edge`) with a certificate for `shop.localhost`, a terminate host of one pairing.
//!
//! - **Regression** (`a_pool_shares_a_host_by_turn_and_a_session_stays_on_one_member`): the pool is two dialers of the
//!   pairing, each with its own instance id and a [`FixedMember`] for its state and cluster, both serving one real local
//!   server's app (`local_web`), so the edge node's counts per member (`get#EdgeBalancing`) tell which member took each
//!   request. Requests with no cookie take turns; a session cookie stays on one member, moves while it is drained (a
//!   WebSocket opened on it before goes on), and comes back; a host's own sticky cookie; a member that goes moves its
//!   cookies; none left, the offline page.
//! - **Acceptance** (§5, E17): two real members of one cluster, the second in a child process with the link's `.env`
//!   properties, so its own hook dials: they share a host's requests evenly, and a browser signed in stays on one member
//!   and moves when it stops, still signed in; a drained member finishes a WebSocket it has and gets no new requests.

#![cfg(all(unix, feature = "edge", feature = "ssh", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::edge::LinkState;
use moqui_host::edge_link::{local_web, Dialer, FixedMember, LocalConfig, PoolMember};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::common::Gate;
use crate::edge_link::{quick, runtime, wait_status};
use crate::edge_web::{authority, call, issue, pair_node, wait_until};

const SHOP: &str = "shop.localhost";

/// The edge node, started after the local server (`node_role` is a process property the local server must not read),
/// with HTTPS on a port of its own and a certificate for `localhost` and [`SHOP`].
fn start_edge(label: &str, root_cert: &(String, String)) -> (Gate, u16) {
    let port = crate::common::free_port();
    let port_text = port.to_string();
    let edge = crate::common::start_second_with(
        label,
        "",
        &[("node_role", "front_edge"), ("webapp_https_listen_port", &port_text)],
        &[("cert/default.crt", &root_cert.0), ("cert/default.key", &root_cert.1)],
    );
    (edge, port)
}

/// A dialer for member `id` of the pairing, serving `local`'s app, with a fixed state and cluster.
fn member(
    rt: &tokio::runtime::Runtime,
    url: &str,
    paired: &moqui_host::edge_link::Paired,
    id: &str,
    cluster: &[(&str, bool)],
    local: &Gate,
) -> (Dialer, Arc<FixedMember>, watch::Sender<bool>) {
    let fixed = FixedMember::new(id);
    fixed.set_members(cluster);
    let mut config = LocalConfig::new(url, &paired.key, &paired.pin, id);
    quick(&mut config);
    config.web = local_web(local.server.factory());
    config.member = Some(Arc::clone(&fixed) as Arc<dyn PoolMember>);
    let (stop, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_status(&dialer, Duration::from_secs(10), "member linked", |status| {
        status.state == LinkState::Linked
    });
    (dialer, fixed, stop)
}

/// `get#EdgeBalancing` of `node`: the defaults, and each member's row by instance id, as text.
fn balancing(edge: &Gate, node: &str) -> (BTreeMap<String, String>, BTreeMap<String, BTreeMap<String, String>>) {
    let out = call(edge, "get#EdgeBalancing", &[("pairedNodeId", node)]).unwrap();
    let Some(Value::List(list)) = out.get("memberList") else {
        panic!("memberList: {out:?}")
    };
    let members = list
        .iter()
        .filter_map(|row| {
            let Value::Map(row) = row else { return None };
            let row: BTreeMap<String, String> = row
                .iter()
                .map(|(name, value)| (name.clone(), to_plain_string(value)))
                .collect();
            Some((row["instanceId"].clone(), row))
        })
        .collect();
    let defaults = out
        .iter()
        .filter(|(name, _)| name.as_str() != "memberList")
        .map(|(name, value)| (name.clone(), to_plain_string(value)))
        .collect();
    (defaults, members)
}

/// Each member's requests so far.
fn requests(edge: &Gate, node: &str) -> BTreeMap<String, u64> {
    balancing(edge, node)
        .1
        .into_iter()
        .map(|(id, row)| (id, row["requests"].parse().unwrap_or(0)))
        .collect()
}

/// What each member took between `before` and `after`.
fn taken(before: &BTreeMap<String, u64>, after: &BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    after
        .iter()
        .map(|(id, count)| (id.clone(), count - before.get(id).copied().unwrap_or(0)))
        .collect()
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

async fn browser(tls: &Arc<rustls::ClientConfig>, port: u16) -> Tls {
    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from(SHOP).unwrap();
    tokio_rustls::TlsConnector::from(Arc::clone(tls))
        .connect(name, tcp)
        .await
        .unwrap()
}

/// One request to [`SHOP`] through the edge node on a new connection: the status, the head and the body.
fn https(
    rt: &tokio::runtime::Runtime,
    tls: &Arc<rustls::ClientConfig>,
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {SHOP}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() || method == "POST" {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);
    rt.block_on(async {
        let mut stream = browser(tls, port).await;
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

/// `count` GETs of `/health` with `cookie` (none when empty), each answered 200.
fn gets(rt: &tokio::runtime::Runtime, tls: &Arc<rustls::ClientConfig>, port: u16, cookie: &str, count: usize) {
    let headers: Vec<(&str, &str)> = if cookie.is_empty() {
        Vec::new()
    } else {
        vec![("Cookie", cookie)]
    };
    for _ in 0..count {
        let (status, head, body) = https(rt, tls, port, "GET", "/health", &headers, "");
        assert_eq!(status, 200, "{head}\n{body}");
    }
}

/// A WebSocket to `/notws` through the edge node with `cookie` (a signed-in session's), open: its stream.
async fn open_websocket(tls: &Arc<rustls::ClientConfig>, port: u16, cookie: &str) -> Tls {
    let mut stream = browser(tls, port).await;
    let upgrade = format!(
        "GET /notws HTTP/1.1\r\nHost: {SHOP}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nCookie: {cookie}\r\n\r\n"
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
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    stream
}

/// A masked ping with "edge" on an open WebSocket, and its pong (RFC 6455 §5.2).
async fn ping(stream: &mut Tls) {
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
            return;
        }
    }
}

/// The one member that took requests in `taken`, all `count` of them.
fn only_one(taken: &BTreeMap<String, u64>, count: u64) -> String {
    let busy: Vec<(&String, &u64)> = taken.iter().filter(|(_, n)| **n > 0).collect();
    match busy.as_slice() {
        [(member, n)] if **n == count => (*member).clone(),
        _ => panic!("not one member with {count}: {taken:?}"),
    }
}

#[test]
fn a_pool_shares_a_host_by_turn_and_a_session_stays_on_one_member() {
    if !crate::common::own_process() {
        return;
    }
    let authority = authority();
    let edge_cert = issue(&authority, &["localhost", SHOP]);
    let local = crate::common::start_with_properties("edge-balance-local", None, &[]);
    let (edge, port) = start_edge("edge-balance-edge", &edge_cert);
    let rt = runtime();
    let url = format!("https://localhost:{port}");
    let paired = pair_node(&rt, &edge, &url, "member-a", &authority.root);
    call(
        &edge,
        "add#EdgeHost",
        &[
            ("hostName", SHOP),
            ("pairedNodeId", "member-a"),
            ("modeEnumId", "EdgeHostTerminate"),
        ],
    )
    .unwrap();
    let cluster = [("member-a", true), ("member-b", true)];
    let (_dialer_a, fixed_a, _stop_a) = member(&rt, &url, &paired, "member-a", &cluster, &local);
    let (_dialer_b, _fixed_b, stop_b) = member(&rt, &url, &paired, "member-b", &cluster, &local);
    wait_until(Duration::from_secs(5), "both members' down connections", || {
        let (_, members) = balancing(&edge, "member-a");
        ["member-a", "member-b"]
            .iter()
            .all(|id| members.get(*id).is_some_and(|row| row["takesWork"] == "true"))
    });
    let (defaults, _) = balancing(&edge, "member-a");
    assert_eq!(defaults["stickyCookie"], "moqui.session", "{defaults:?}");
    assert_eq!(defaults["setAsideFailures"], "3");
    assert_eq!(defaults["setAsideSeconds"], "30");
    let tls = client_config(&authority.root);

    // no cookie: the members take turns
    let before = requests(&edge, "member-a");
    gets(&rt, &tls, port, "", 8);
    let shared = taken(&before, &requests(&edge, "member-a"));
    assert_eq!((shared["member-a"], shared["member-b"]), (4, 4), "{shared:?}");

    // a session cookie stays on one member
    let signed_in = sign_in(&rt, &tls, port);
    let session = signed_in.as_str();
    let before = requests(&edge, "member-a");
    gets(&rt, &tls, port, session, 6);
    let sticky = only_one(&taken(&before, &requests(&edge, "member-a")), 6);
    let other = if sticky == "member-a" { "member-b" } else { "member-a" };
    // a WebSocket opened with that cookie is on that member too
    let before = requests(&edge, "member-a");
    let mut socket = rt.block_on(open_websocket(&tls, port, session));
    assert_eq!(only_one(&taken(&before, &requests(&edge, "member-a")), 1), sticky);

    // drained, it gets no new requests (its cookie moves) and finishes what it has: the WebSocket goes on
    let drained = call(
        &edge,
        "drain#EdgeMember",
        &[("pairedNodeId", "member-a"), ("instanceId", &sticky), ("drain", "true")],
    )
    .unwrap();
    assert_eq!(to_plain_string(&drained["state"]), "draining");
    wait_until(
        Duration::from_secs(5),
        "the drained member's state on the edge node",
        || balancing(&edge, "member-a").1[&sticky]["state"] == "draining",
    );
    let before = requests(&edge, "member-a");
    gets(&rt, &tls, port, session, 4);
    gets(&rt, &tls, port, "", 2);
    assert_eq!(only_one(&taken(&before, &requests(&edge, "member-a")), 6), other);
    rt.block_on(ping(&mut socket));
    // back, the cookie comes back to it
    call(
        &edge,
        "drain#EdgeMember",
        &[
            ("pairedNodeId", "member-a"),
            ("instanceId", &sticky),
            ("drain", "false"),
        ],
    )
    .unwrap();
    wait_until(Duration::from_secs(5), "the member live again", || {
        balancing(&edge, "member-a").1[&sticky]["takesWork"] == "true"
    });
    let before = requests(&edge, "member-a");
    gets(&rt, &tls, port, session, 3);
    assert_eq!(only_one(&taken(&before, &requests(&edge, "member-a")), 3), sticky);
    drop(socket);

    // a host's own sticky cookie: the session cookie no longer decides, the host's does
    let added = call(
        &edge,
        "add#EdgeHost",
        &[
            ("hostName", SHOP),
            ("pairedNodeId", "member-a"),
            ("modeEnumId", "EdgeHostTerminate"),
            ("stickyCookie", "SHOPSID"),
        ],
    )
    .unwrap();
    assert_eq!(to_plain_string(&added["stickyCookie"]), "SHOPSID");
    let hosts = call(&edge, "get#EdgeHosts", &[("pairedNodeId", "member-a")]).unwrap();
    assert!(format!("{hosts:?}").contains("SHOPSID"), "{hosts:?}");
    wait_until(Duration::from_secs(5), "the host table loaded again", || {
        let before = requests(&edge, "member-a");
        gets(&rt, &tls, port, session, 2);
        taken(&before, &requests(&edge, "member-a")).values().all(|n| *n == 1)
    });
    let before = requests(&edge, "member-a");
    gets(&rt, &tls, port, &format!("SHOPSID=cart-9; {session}"), 4);
    only_one(&taken(&before, &requests(&edge, "member-a")), 4);
    // a mode change that names no cookie keeps it (a Configure's host names)
    call(
        &edge,
        "add#EdgeHost",
        &[
            ("hostName", SHOP),
            ("pairedNodeId", "member-a"),
            ("modeEnumId", "EdgeHostTerminate"),
        ],
    )
    .unwrap();
    assert!(format!("{:?}", call(&edge, "get#EdgeHosts", &[]).unwrap()).contains("SHOPSID"));

    // a member that goes: its requests go to the other; none left, the offline page
    stop_b.send(true).unwrap();
    wait_until(Duration::from_secs(5), "member-b gone", || {
        balancing(&edge, "member-a").1["member-b"]["linked"] == "false"
    });
    let before = requests(&edge, "member-a");
    gets(&rt, &tls, port, "", 3);
    assert_eq!(only_one(&taken(&before, &requests(&edge, "member-a")), 3), "member-a");
    drop(fixed_a);
    call(
        &edge,
        "drain#EdgeMember",
        &[
            ("pairedNodeId", "member-a"),
            ("instanceId", "member-a"),
            ("drain", "true"),
        ],
    )
    .unwrap();
    wait_until(Duration::from_secs(5), "no member takes work", || {
        balancing(&edge, "member-a").1["member-a"]["takesWork"] == "false"
    });
    let (status, _, body) = https(&rt, &tls, port, "GET", "/health", &[], "");
    assert_eq!(status, 503);
    assert!(body.contains("Offline"), "{body}");
}

// ===== acceptance (plans/Edge-Node2.md §5, E17): two real members of one cluster =====

/// The cluster directory and the link's properties a second member's child process takes; set only for that child.
const MEMBER_CLUSTER: &str = "MOQUI_EDGE_BALANCE_CLUSTER";
const MEMBER_LINK: &str = "MOQUI_EDGE_BALANCE_LINK";
const MEMBER_TEST: &str = "edge_balance::balance_member_process";
/// The generated secrets a second member of the cluster takes from the first: its session cookies are the first's.
const SHARED_SECRETS: [&str; 4] = [
    "session_cookie_key",
    "cluster_tls_cert",
    "cluster_tls_key",
    "entity_ds_crypt_pass",
];

/// A second member of the local cluster in a child process of this test binary, on the first's database, whose own
/// hook links to the edge node with the pairing's key as the `.env` would give it. Dropping it stops it.
struct ChildMember {
    child: std::process::Child,
    node: String,
}

impl ChildMember {
    fn start(cluster_dir: &std::path::Path, link: &[(&str, &str)]) -> ChildMember {
        use std::io::BufRead as _;
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        for name in SHARED_SECRETS {
            let value = moqui_lib::env::get_prop_or_env(name);
            if !value.is_empty() {
                command.env(name, value);
            }
        }
        let link: Vec<String> = link.iter().map(|(name, value)| format!("{name}={value}")).collect();
        let mut child = command
            .args([MEMBER_TEST, "--exact", "--nocapture", "--test-threads", "1"])
            .env("MOQUI_TEST_OWN_PROCESS", MEMBER_TEST)
            .env(MEMBER_CLUSTER, cluster_dir)
            .env(MEMBER_LINK, link.join("\n"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the member's process");
        let stdout = child.stdout.take().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut lines = std::io::BufReader::new(stdout).lines().map_while(Result::ok);
            let ready = lines.by_ref().find_map(|line| {
                line.find("BALANCE_MEMBER ")
                    .map(|at| line[at + 15..].trim().to_string())
            });
            let _ = ready_tx.send(ready);
            lines.for_each(drop);
        });
        match ready_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Some(node)) => ChildMember { child, node },
            outcome => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the second member did not start: {outcome:?}");
            }
        }
    }
}

impl Drop for ChildMember {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < until && matches!(self.child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The second member's process: a server on the first's database that links to the edge node, until its stdin closes.
#[test]
fn balance_member_process() {
    let (Some(cluster_dir), Some(link)) = (std::env::var_os(MEMBER_CLUSTER), std::env::var(MEMBER_LINK).ok()) else {
        return;
    };
    if !crate::common::own_process() {
        return;
    }
    let link: Vec<(&str, &str)> = link.lines().filter_map(|line| line.split_once('=')).collect();
    let (server, runtime) =
        crate::common::start_member("edge-balance-member", std::path::Path::new(&cluster_dir), &link);
    let node = moqui_core::cluster::instance_id(server.factory()).unwrap();
    println!("BALANCE_MEMBER {node}");
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    drop(server);
    drop(runtime);
}

/// A local cluster of two real members linked to an edge node with [`SHOP`] as the pairing's terminate host: the first
/// member is the test's own server with a dialer, the second a [`ChildMember`] that dials by itself.
struct Pool {
    rt: tokio::runtime::Runtime,
    tls: Arc<rustls::ClientConfig>,
    port: u16,
    /// The pairing, under the first member's instance id.
    node: String,
    second: Option<ChildMember>,
    second_id: String,
    stop_first: watch::Sender<bool>,
    _dialer: Dialer,
    edge: Gate,
    /// The first member's server: dropped last.
    _local: Gate,
}

impl Pool {
    fn start(label: &str) -> Pool {
        let authority = authority();
        let edge_cert = issue(&authority, &["localhost", SHOP]);
        let cluster_port = crate::common::free_port().to_string();
        let local = crate::common::start_with_properties(
            &format!("{label}-local"),
            None,
            &[("cluster_https_port", &cluster_port)],
        );
        let (edge, port) = start_edge(&format!("{label}-edge"), &edge_cert);
        let rt = runtime();
        let url = format!("https://localhost:{port}");
        let node = moqui_core::cluster::instance_id(local.server.factory()).unwrap();
        let paired = pair_node(&rt, &edge, &url, &node, &authority.root);
        call(
            &edge,
            "add#EdgeHost",
            &[
                ("hostName", SHOP),
                ("pairedNodeId", &node),
                ("modeEnumId", "EdgeHostTerminate"),
            ],
        )
        .unwrap();
        // what the first member's .env would say, as the second's does: a node behind an edge node writes a new session
        // before its response (both servers of this process read it; the edge node's hook has started already)
        moqui_lib::env::set_property(moqui_core::edge::NODE_URL, url.clone());
        let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, &node);
        quick(&mut config);
        config.web = local_web(local.server.factory());
        config.member_key = Some(moqui_host::edge_link::MemberKey::load_or_create(&local.runtime).unwrap());
        let (stop_first, shutdown) = watch::channel(false);
        let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
        wait_status(&dialer, Duration::from_secs(10), "the first member linked", |status| {
            status.state == LinkState::Linked
        });
        let second = ChildMember::start(
            &local.cluster_dir,
            &[
                (moqui_core::edge::NODE_URL, &url),
                (moqui_core::edge::NODE_KEY, &paired.key),
                (moqui_core::edge::NODE_PIN, &paired.pin),
                (moqui_core::edge::NODE_PAIRING, &node),
            ],
        );
        let second_id = second.node.clone();
        wait_until(Duration::from_secs(20), "both members take work", || {
            let (_, members) = balancing(&edge, &node);
            [node.as_str(), second_id.as_str()]
                .iter()
                .all(|id| members.get(*id).is_some_and(|row| row["takesWork"] == "true"))
        });
        Pool {
            rt,
            tls: client_config(&authority.root),
            port,
            node,
            second: Some(second),
            second_id,
            stop_first,
            _dialer: dialer,
            edge,
            _local: local,
        }
    }

    fn requests(&self) -> BTreeMap<String, u64> {
        requests(&self.edge, &self.node)
    }

    fn get(&self, path: &str, cookie: &str) -> (u16, String, String) {
        let headers: Vec<(&str, &str)> = if cookie.is_empty() {
            Vec::new()
        } else {
            vec![("Cookie", cookie)]
        };
        https(&self.rt, &self.tls, self.port, "GET", path, &headers, "")
    }

    fn sign_in(&self) -> String {
        sign_in(&self.rt, &self.tls, self.port)
    }

    /// Whether `cookie` is signed in: a REST read that needs a user is answered.
    fn signed_in(&self, cookie: &str) -> bool {
        let (status, head, body) = self.get("/rest/s1/moqui/basic/enumTypes", cookie);
        assert!(matches!(status, 200 | 401 | 403), "{head}\n{body}");
        status == 200
    }
}

/// Sign john.doe in through the edge node with the login form, as a browser does: the cookies.
fn sign_in(rt: &tokio::runtime::Runtime, tls: &Arc<rustls::ClientConfig>, port: u16) -> String {
    let (_, head, _) = https(rt, tls, port, "GET", "/Login", &[], "");
    let cookie = set_cookies("", &head);
    let (status, head, body) = https(
        rt,
        tls,
        port,
        "POST",
        "/Login/login",
        &[
            ("Cookie", &cookie),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        "username=john.doe&password=moqui",
    );
    assert_eq!(status, 302, "{head}\n{body}");
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

#[test]
#[ignore = "acceptance: two members share a host's requests evenly; a browser session stays on one member and moves when it stops, still signed in (Edge-Node2 §5, E17)"]
fn two_members_share_requests_and_a_session_moves_when_its_member_stops() {
    if !crate::common::own_process() {
        return;
    }
    let mut pool = Pool::start("edge-balance-share");
    // evenly, by turn
    let before = pool.requests();
    for _ in 0..10 {
        let (status, head, _) = pool.get("/health", "");
        assert_eq!(status, 200, "{head}");
    }
    let shared = taken(&before, &pool.requests());
    assert_eq!((shared[&pool.node], shared[&pool.second_id]), (5, 5), "{shared:?}");
    // a browser signed in stays on one member
    assert!(!pool.signed_in(""), "the check needs a user");
    let cookie = pool.sign_in();
    let before = pool.requests();
    for _ in 0..4 {
        assert!(pool.signed_in(&cookie), "signed in through the edge node");
    }
    let on = only_one(&taken(&before, &pool.requests()), 4);
    // that member stops: the session moves to the other member, still signed in (sessions are rows in the pool's
    // database)
    if on == pool.second_id {
        drop(pool.second.take());
    } else {
        pool.stop_first.send(true).unwrap();
    }
    wait_until(Duration::from_secs(10), "the stopped member out of the pool", || {
        balancing(&pool.edge, &pool.node).1[&on]["takesWork"] == "false"
    });
    let before = pool.requests();
    for _ in 0..3 {
        assert!(pool.signed_in(&cookie), "still signed in on the other member");
    }
    let now_on = only_one(&taken(&before, &pool.requests()), 3);
    assert_ne!(now_on, on);
}

#[test]
#[ignore = "acceptance: a drained member finishes its requests and gets no new ones (Edge-Node2 §5, E17)"]
fn a_drained_member_finishes_its_requests_and_gets_no_new_ones() {
    if !crate::common::own_process() {
        return;
    }
    let pool = Pool::start("edge-balance-drain");
    let cookie = pool.sign_in();
    let before = pool.requests();
    let mut socket = pool.rt.block_on(open_websocket(&pool.tls, pool.port, &cookie));
    let on = only_one(&taken(&before, &pool.requests()), 1);
    // drained from the edge node's System app: the member says so on its link
    let drained = call(
        &pool.edge,
        "drain#EdgeMember",
        &[("pairedNodeId", &pool.node), ("instanceId", &on), ("drain", "true")],
    )
    .unwrap();
    assert_eq!(to_plain_string(&drained["state"]), "draining");
    wait_until(Duration::from_secs(5), "the member draining", || {
        balancing(&pool.edge, &pool.node).1[&on]["state"] == "draining"
    });
    // no new requests, its session's included; what it has goes on
    let before = pool.requests();
    for _ in 0..4 {
        assert!(pool.signed_in(&cookie));
        let (status, _, _) = pool.get("/health", "");
        assert_eq!(status, 200);
    }
    let taken_now = taken(&before, &pool.requests());
    assert_eq!(taken_now.get(&on).copied().unwrap_or(0), 0, "{taken_now:?}");
    pool.rt.block_on(ping(&mut socket));
    let (_, members) = balancing(&pool.edge, &pool.node);
    assert_eq!(members[&on]["takesWork"], "false");
    // resumed, it takes work again
    call(
        &pool.edge,
        "drain#EdgeMember",
        &[("pairedNodeId", &pool.node), ("instanceId", &on), ("drain", "false")],
    )
    .unwrap();
    wait_until(Duration::from_secs(5), "the member live again", || {
        balancing(&pool.edge, &pool.node).1[&on]["takesWork"] == "true"
    });
    pool.rt.block_on(ping(&mut socket));
}
