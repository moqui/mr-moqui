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

//! Calls with the edge node as media node (plans/archive/Edge-Node.md §5.4, E7). The local node runs in the test's process with
//! no media socket of its own; the edge node runs in a child process of this test binary (the test
//! `edge_call::edge_node_process`), with its own database, its media socket and its embedded TURN: the media driver's
//! command channel, the notification socket's bindings and the notify bus are process globals, so the media node and
//! the node whose sockets signal to it cannot share a process (as `rtc/cascade.rs`'s leg node). The local node pairs
//! with it and its link is a dialer the test drives, serving the local server (`local_web`).
//!
//! An outside browser stand-in, relayed by the edge node's TURN with the credential the local node signed with the
//! edge node's secret (each node has its own `turn_auth_secret`), and an office one on UDP, in one call: both signal on
//! the local node's `/notws`, the frames go to the edge node over the link and back, the media is on the edge node.
//! The local node keeps the edge node's `CallMediaNode` row from the link, and the call's media events are delegated
//! to it. The call is recorded on the edge node, and the segments reach the local node through the queue.

#![cfg(all(unix, feature = "edge", feature = "rtc", feature = "embedded-postgres"))]

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;
use moqui_core::edge::LinkState;
use moqui_host::edge_link::{local_web, pair, Backoff, Dialer, LocalConfig, PairConfig};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use serde_json::json;
use tokio::sync::watch;

use crate::rtc::{call, ice_servers, list_of, open_socket, relayed_peer, run_until, text, Peer};

/// What the edge node's child process is given; set only for that child.
const EDGE_ENV: &str = "MOQUI_EDGE_CALL_NODE";
const EDGE_TEST: &str = "edge_call::edge_node_process";
/// How long the edge node's process may take to print its ready line.
const EDGE_START: Duration = Duration::from_secs(40);
const CALLEE: &str = "edge.callee";
const PASSWORD: &str = "Edge-callee-77!";
/// Each node's own TURN secret: the local node must sign the edge node's TURN credentials with the edge node's.
const LOCAL_SECRET: &str = "local-node-turn-secret";
const EDGE_SECRET: &str = "edge-node-turn-secret";

pub(crate) fn pem(label: &str, der: &[u8]) -> String {
    let text = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = text
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

/// A test authority's root, and a `localhost` certificate and key (PEM) it signed.
pub(crate) fn certs() -> (CertificateDer<'static>, String, String) {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let root = ca.self_signed(&ca_key).unwrap().der().clone();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    (
        root,
        pem("CERTIFICATE", cert.der()),
        pem("PRIVATE KEY", &key.serialize_der()),
    )
}

/// The edge node, in its child process (shared with `edge_sip.rs`).
pub(crate) struct EdgeNode {
    child: Child,
    pub(crate) node: String,
    pub(crate) code: String,
    pub(crate) media: SocketAddr,
    pub(crate) runtime: PathBuf,
    /// Its ready line, whose other fields [`EdgeNode::field`] reads.
    line: String,
}

impl EdgeNode {
    /// Start the edge node's process; [`EdgeNode::ready`] waits for it.
    fn spawn(https_port: u16, cert: &str, key: &str) -> (Child, std::sync::mpsc::Receiver<Option<String>>) {
        let setup = json!({"port": https_port, "cert": cert, "key": key});
        EdgeNode::spawn_test(EDGE_TEST, EDGE_ENV, &setup)
    }

    /// Start the test `test` of this binary as an edge node's process, given `setup` in the variable `env`.
    pub(crate) fn spawn_test(
        test: &str,
        env: &str,
        setup: &serde_json::Value,
    ) -> (Child, std::sync::mpsc::Receiver<Option<String>>) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([test, "--exact", "--nocapture", "--test-threads", "1"])
            .env("MOQUI_TEST_OWN_PROCESS", test)
            .env(env, setup.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the edge node's process");
        // its ready line, read on a thread so a child that hangs fails the test instead of holding it; the rest of its
        // output is read and dropped there too, so its pipe does not fill
        let stdout = child.stdout.take().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
            let ready = lines
                .by_ref()
                .find_map(|line| line.find("EDGE_NODE ").map(|at| line[at..].to_string()));
            let _ = ready_tx.send(ready);
            lines.for_each(drop);
        });
        (child, ready_rx)
    }

    pub(crate) fn ready(mut child: Child, ready: &std::sync::mpsc::Receiver<Option<String>>) -> EdgeNode {
        let line = match ready.recv_timeout(EDGE_START) {
            Ok(Some(line)) => line,
            outcome => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the edge node did not start: {outcome:?}");
            }
        };
        let field = |name: &str| field_of(&line, name);
        EdgeNode {
            child,
            node: field("node"),
            code: field("code"),
            media: field("media").parse().unwrap(),
            runtime: PathBuf::from(field("runtime")),
            line: line.clone(),
        }
    }

    /// A field of its ready line (`name=value`).
    pub(crate) fn field(&self, name: &str) -> String {
        field_of(&self.line, name)
    }
}

fn field_of(line: &str, name: &str) -> String {
    line.split_whitespace()
        .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
        .unwrap_or_default()
        .to_string()
}

impl Drop for EdgeNode {
    fn drop(&mut self) {
        // closing its stdin stops it in order
        drop(self.child.stdin.take());
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if !matches!(self.child.try_wait(), Ok(None)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}

/// The edge node's process: an edge node with call media and TURN on its own database, until its stdin closes.
#[test]
fn edge_node_process() {
    let Some(setup) = std::env::var_os(EDGE_ENV) else {
        return;
    };
    if !crate::common::own_process() {
        return;
    }
    let setup: serde_json::Value = serde_json::from_str(&setup.to_string_lossy()).unwrap();
    let port = setup["port"].as_u64().unwrap().to_string();
    let (cert, key) = (setup["cert"].as_str().unwrap(), setup["key"].as_str().unwrap());
    let gate = crate::common::start_with_conf_files(
        "edge-call-edge",
        None,
        "",
        &[
            ("node_role", "edge"),
            ("webapp_https_listen_port", &port),
            ("rtc_media_port", "0"),
            ("turn_port", "0"),
            ("turn_relay_ports", "51200-51300"),
            ("turn_auth_secret", EDGE_SECRET),
            ("call_recording_enabled", "true"),
        ],
        &[("cert/default.crt", cert), ("cert/default.key", key)],
    );
    let factory = std::sync::Arc::clone(gate.server.factory());
    let node = moqui_core::cluster::instance_id(&factory).unwrap();
    let code = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        text(&call("org.moqui.edge.EdgeServices.create#PairingCode", &[]), "code")
    };
    println!(
        "EDGE_NODE node={node} code={code} media={} runtime={}",
        gate.server.rtc_address().unwrap(),
        gate.runtime.display()
    );
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    drop(gate);
}

/// A row of `entity` on the local node by its key, read with authorization off.
pub(crate) fn field(entity: &str, key: &str, id: &str, name: &str) -> String {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    moqui_core::context::ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .expect("an entity facade")
        .find(entity)
        .unwrap()
        .disable_authz()
        .eq(key, Value::string(id))
        .use_cache(false)
        .one()
        .unwrap()
        .and_then(|row| row.get(name).map(to_plain_string))
        .unwrap_or_default()
}

pub(crate) fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn an_outside_and_an_office_browser_call_through_the_edge_node_and_the_recording_reaches_the_local_node() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let (root, cert, key) = certs();
    let edge_port = crate::common::free_port();
    // the edge node boots in its process while the local node boots in this one
    let (child, ready) = EdgeNode::spawn(edge_port, &cert, &key);
    let local = crate::common::start_with_properties(
        "edge-call-local",
        None,
        &[("turn_auth_secret", LOCAL_SECRET), ("call_recording_enabled", "true")],
    );
    let edge = EdgeNode::ready(child, &ready);
    let http = local.server.address();
    let factory = std::sync::Arc::clone(local.server.factory());
    assert!(
        local.server.rtc_address().is_none(),
        "the local node has no media socket"
    );
    let local_id = moqui_core::cluster::instance_id(&factory).unwrap();
    assert_ne!(local_id, edge.node);

    // pair, then link: the local node's dialer, serving the local server
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let mut pairing = PairConfig::new(&url, &edge.code, &local_id);
    pairing.trust_root = Some(root);
    pairing.connect_timeout = Duration::from_secs(5);
    let paired = rt.block_on(pair(&pairing)).unwrap();
    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, &local_id);
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(3);
    config.web = local_web(&factory);
    let (_shutdown, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_until(Duration::from_secs(10), "the link up", || {
        dialer.status().state == LinkState::Linked
    });
    let status = dialer.status();
    assert_eq!(
        status.peer.instance_id, edge.node,
        "the edge node's instance id from its Hello"
    );
    assert_eq!(status.peer.media_address, edge.media.to_string());
    assert!(
        status.peer.turn_urls.iter().any(|u| u.ends_with("transport=udp")),
        "{:?}",
        status.peer.turn_urls
    );

    // the local node keeps the edge node's media row from the link: the only media node, so the call's
    let (call_id, caller_leg, callee_leg, ice) = {
        let context = factory.new_context();
        let _entered = context.enter();
        wait_until(Duration::from_secs(5), "the edge node's media row", || {
            field("moqui.call.CallMediaNode", "instanceId", &edge.node, "edgeLinked") == "Y"
        });
        assert_eq!(
            field("moqui.call.CallMediaNode", "instanceId", &edge.node, "mediaAddress"),
            edge.media.to_string()
        );
        assert_eq!(
            field("moqui.call.CallMediaNode", "instanceId", &local_id, "instanceId"),
            "",
            "the local node registers no media of its own"
        );
        let created = call(
            "org.moqui.impl.UserServices.create#UserAccount",
            &[
                ("username", Value::string(CALLEE)),
                ("newPassword", Value::string(PASSWORD)),
                ("newPasswordVerify", Value::string(PASSWORD)),
                ("userFullName", Value::string("Edge Callee")),
            ],
        );
        let callee = text(&created, "userId");
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let started = call(
            "org.moqui.call.CallServices.start#Call",
            &[("calleeUserIdList", Value::List(vec![Value::string(&callee)]))],
        );
        let call_id = text(&started, "callSessionId");
        let ice = ice_servers(&call(
            "org.moqui.call.CallServices.get#IceServers",
            &[("callSessionId", Value::string(&call_id))],
        ));
        moqui_core::user::internal_login_user(CALLEE).unwrap();
        let answered = call(
            "org.moqui.call.CallServices.answer#Call",
            &[("callSessionId", Value::string(&call_id))],
        );
        assert_eq!(
            field("moqui.call.CallSession", "callSessionId", &call_id, "mediaNodeId"),
            edge.node,
            "the call's media is on the edge node"
        );
        (call_id, text(&started, "callLegId"), text(&answered, "callLegId"), ice)
    };
    // john's TURN credential is the edge node's: its URLs, signed with its secret (the allocation below is the check)
    let (urls, username, credential) = ice;
    assert!(
        username.contains(&format!(":{caller_leg}@")),
        "the leg and its pairing (E16): {username}"
    );
    let turn_udp: SocketAddr = urls
        .iter()
        .find(|u| u.ends_with("transport=udp"))
        .and_then(|u| u.strip_prefix("turn:"))
        .and_then(|u| u.split('?').next())
        .and_then(|a| a.parse().ok())
        .unwrap_or_else(|| panic!("the edge node's TURN UDP address in {urls:?}"));

    // john is outside, relayed by the edge node's TURN; the callee is in the office, on UDP; both signal on the local
    // node's /notws
    let john_cookie = crate::common::sign_in(http, "john.doe", "moqui");
    let callee_cookie = crate::common::sign_in(http, CALLEE, PASSWORD);
    factory.flush_sessions().unwrap();
    let mut john = relayed_peer(
        open_socket(http, &john_cookie),
        &call_id,
        &caller_leg,
        turn_udp,
        &username,
        &credential,
    );
    let mut callee = Peer::new(open_socket(http, &callee_cookie), &call_id, &callee_leg);
    john.join();
    callee.join();
    let flowed = run_until(&mut [&mut john, &mut callee], Duration::from_secs(10), |p| {
        p.iter().all(|p| p.received >= 20)
    });
    assert!(
        flowed,
        "audio both ways through the edge node: john {} callee {}; frames {:?} {:?}",
        john.received, callee.received, john.frames, callee.frames
    );
    // the answer came from the edge node's media socket, over the link
    let joined = callee.frame("joined").cloned().unwrap_or_default();
    assert!(
        joined["sdp"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!(" {} typ host", edge.media.port())),
        "{joined}"
    );
    assert!(callee
        .frames
        .iter()
        .any(|f| f["rtc"] == json!("participant") && f["callLegId"] == json!(caller_leg)));
    // the media events were delegated to the local node: each leg's media connected is noted on it (joinedDate)
    {
        let context = factory.new_context();
        let _entered = context.enter();
        wait_until(Duration::from_secs(5), "the legs' media connected, noted here", || {
            [&caller_leg, &callee_leg]
                .iter()
                .all(|leg| !field("moqui.call.CallLeg", "callLegId", leg, "joinedDate").is_empty())
        });
    }

    // john records: the recorder runs on the edge node, and its segments reach the local node through the queue
    let as_john = |name: &str, parameters: &[(&str, Value)]| {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        call(name, parameters)
    };
    let recording_id = text(
        &as_john(
            "org.moqui.call.CallRecordingServices.start#Recording",
            &[("callSessionId", Value::string(&call_id))],
        ),
        "callRecordingId",
    );
    assert!(!recording_id.is_empty());
    let noticed = |p: &Peer, state: &str| {
        p.frames.iter().any(|f| {
            f["rtc"] == json!("recording") && f["state"] == json!(state) && f["callRecordingId"] == json!(recording_id)
        })
    };
    assert!(
        run_until(&mut [&mut john, &mut callee], Duration::from_secs(3), |p| {
            noticed(p[0], "recording") && noticed(p[1], "recording")
        }),
        "the notice from the edge node: john {:?} callee {:?}",
        john.frames,
        callee.frames
    );
    run_until(&mut [&mut john, &mut callee], Duration::from_millis(1_000), |_| false);
    as_john(
        "org.moqui.call.CallRecordingServices.stop#Recording",
        &[("callSessionId", Value::string(&call_id))],
    );
    let mut segments = Vec::new();
    let until = Instant::now() + Duration::from_secs(8);
    while Instant::now() < until {
        run_until(&mut [&mut john, &mut callee], Duration::from_millis(100), |_| false);
        let found = as_john(
            "org.moqui.call.CallRecordingServices.find#Recordings",
            &[("callSessionId", Value::string(&call_id))],
        );
        segments = list_of(found.get("recordingList"))
            .first()
            .map(|r| {
                list_of(r.get("periods"))
                    .iter()
                    .flat_map(|p| list_of(p.get("segments")))
                    .collect()
            })
            .unwrap_or_default();
        if segments.len() >= 3 {
            break;
        }
    }
    let kind =
        |s: &std::collections::BTreeMap<String, Value>| s.get("trackKind").map(to_plain_string).unwrap_or_default();
    assert_eq!(
        segments.iter().filter(|s| kind(s) == "audio").count(),
        2,
        "a track per leg, stored on the local node: {segments:?}"
    );
    assert_eq!(
        segments.iter().filter(|s| kind(s) == "mix").count(),
        1,
        "the mixed audio"
    );
    for segment in &segments {
        let location = text(
            &as_john(
                "org.moqui.call.CallRecordingServices.get#RecordingFile",
                &[("callRecordingSegmentId", segment["callRecordingSegmentId"].clone())],
            ),
            "location",
        );
        let bytes = {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            factory
                .resource()
                .location_bytes(&location)
                .unwrap()
                .unwrap_or_default()
        };
        assert_eq!(&bytes[..4.min(bytes.len())], b"OggS", "{location}");
        assert!(bytes.len() > 200, "{location}: {} bytes", bytes.len());
    }
    drop(john);
    drop(callee);
    dialer.stop();
    drop(edge);
    eprintln!("edge call test took {:?}", started.elapsed());
}
