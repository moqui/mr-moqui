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

//! Cascading media nodes (plans/Comms-Framework.md W7): one call on two servers of one cluster, on one test database,
//! each with its media port and `rtc_cascade_enabled`. The call's media is on the first (the origin); a leg whose
//! socket is on the second has its media there (a leg node), which relays to the origin. Media goes both ways, a
//! subscriber at the leg node changes its layer of a publisher at the origin, and losing the leg node ends only the leg
//! node's legs.
//!
//! The second server runs in a child process of this test binary (the test `rtc::cascade::leg_node_process`), not in
//! this one: the media driver's command channel, the notification socket's bindings and the notify bus's origin id are
//! process globals, so two media nodes cannot share a process. A child process also makes the leg node's loss a real
//! one: it is killed.

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use moqui_lib::object::{to_plain_string, Value};
use serde_json::json;

use super::{call, open_socket, run_until, text, Peer, VIDEO_LAYER_BYTES};

/// The cluster directory the leg node's child process adopts; set only for that child.
const LEG_NODE_ENV: &str = "MOQUI_RTC_LEG_NODE_CLUSTER";
const LEG_NODE_TEST: &str = "rtc::cascade::leg_node_process";
/// How long the leg node's process may take to print its ready line.
const LEG_NODE_START: Duration = Duration::from_secs(30);
const PROPERTIES: [(&str, &str); 2] = [("rtc_media_port", "0"), ("rtc_cascade_enabled", "true")];
/// The generated secrets a second member of the cluster takes from the first.
const SHARED_SECRETS: [&str; 5] = [
    "session_cookie_key",
    "cluster_tls_cert",
    "cluster_tls_key",
    "entity_ds_crypt_pass",
    "turn_auth_secret",
];

/// The second server, in its child process.
struct LegNode {
    child: Child,
    http: SocketAddr,
    node: String,
    media: SocketAddr,
    runtime: PathBuf,
}

impl LegNode {
    fn start(cluster_dir: &Path) -> LegNode {
        let mut command = Command::new(std::env::current_exe().unwrap());
        // the cluster's members share their secrets: the session key and the cluster's certificate (each member checks
        // the others' fingerprints), and the key of the encrypted fields of the one database
        for name in SHARED_SECRETS {
            let value = moqui_lib::env::get_prop_or_env(name);
            if !value.is_empty() {
                command.env(name, value);
            }
        }
        let mut child = command
            .args([LEG_NODE_TEST, "--exact", "--nocapture", "--test-threads", "1"])
            .env("MOQUI_TEST_OWN_PROCESS", LEG_NODE_TEST)
            .env(LEG_NODE_ENV, cluster_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the leg node's process");
        // its ready line, read on a thread so a child that hangs as it starts fails the test instead of holding it; the
        // rest of its output is read and dropped there too, so its pipe does not fill
        let stdout = child.stdout.take().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
            let ready = lines
                .by_ref()
                .find_map(|line| line.find("LEG_NODE ").map(|at| line[at..].to_string()));
            let _ = ready_tx.send(ready);
            lines.for_each(drop);
        });
        let ready = match ready_rx.recv_timeout(LEG_NODE_START) {
            Ok(Some(ready)) => ready,
            outcome => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the leg node did not start: {outcome:?}");
            }
        };
        let field = |name: &str| {
            ready
                .split_whitespace()
                .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
                .unwrap_or_default()
                .to_string()
        };
        LegNode {
            child,
            http: field("http").parse().unwrap(),
            node: field("node"),
            media: field("media").parse().unwrap(),
            runtime: PathBuf::from(field("runtime")),
        }
    }

    /// The leg node is lost: its process killed, nothing stopped in order.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LegNode {
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
        self.kill();
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}

/// The first server, the call's media node: on a cluster port it advertises to the leg node.
fn start_origin(label: &str) -> crate::common::Gate {
    let port = crate::common::free_port().to_string();
    let mut properties = PROPERTIES.to_vec();
    properties.push(("cluster_https_port", &port));
    crate::common::start_with_properties(label, None, &properties)
}

/// The leg node's process: a server on the gate's database, until its stdin closes.
#[test]
fn leg_node_process() {
    let Some(cluster_dir) = std::env::var_os(LEG_NODE_ENV) else {
        return;
    };
    if !crate::common::own_process() {
        return;
    }
    let (server, runtime) = crate::common::start_member("rtc-leg-node", Path::new(&cluster_dir), &PROPERTIES);
    let node = moqui_core::cluster::instance_id(server.factory()).unwrap();
    println!(
        "LEG_NODE http={} node={node} media={} runtime={}",
        server.address(),
        server.rtc_address().unwrap(),
        runtime.0.display()
    );
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    drop(server);
    drop(runtime);
}

/// Waits until both nodes registered their media.
fn wait_for_nodes(nodes: &[&str]) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let registered = nodes
            .iter()
            .all(|node| !field("moqui.call.CallMediaNode", "instanceId", node, "instanceId").is_empty());
        if registered {
            return;
        }
        assert!(Instant::now() < until, "both media nodes register");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn create_user(username: &str) -> String {
    let created = call(
        "org.moqui.impl.UserServices.create#UserAccount",
        &[
            ("username", Value::string(username)),
            ("newPassword", Value::string(PASSWORD)),
            ("newPasswordVerify", Value::string(PASSWORD)),
            ("userFullName", Value::string(username)),
        ],
    );
    text(&created, "userId")
}

const PASSWORD: &str = "Cascade-77-pass!";

/// john.doe starts a call to these users, its media put on `origin`, and each answers: the call and its legs (john's
/// first).
fn start_call(origin: &str, callees: &[&str]) -> (String, Vec<String>) {
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let ids: Vec<Value> = callees.iter().map(|id| Value::string(*id)).collect();
    let started = call(
        "org.moqui.call.CallServices.start#Call",
        &[("calleeUserIdList", Value::List(ids))],
    );
    let call_id = text(&started, "callSessionId");
    // the call's media on the first server, whichever registered last
    set_field(
        "moqui.call.CallSession",
        "callSessionId",
        &call_id,
        "mediaNodeId",
        Value::string(origin),
    );
    let mut legs = vec![text(&started, "callLegId")];
    for username in callees.iter().map(|id| username_of(id)) {
        moqui_core::user::internal_login_user(&username).unwrap();
        let answered = call(
            "org.moqui.call.CallServices.answer#Call",
            &[("callSessionId", Value::string(&call_id))],
        );
        legs.push(text(&answered, "callLegId"));
    }
    (call_id, legs)
}

fn username_of(user_id: &str) -> String {
    field("moqui.security.UserAccount", "userId", user_id, "username")
}

fn facade() -> moqui_core::entity::EntityFacade {
    moqui_core::context::ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .expect("an entity facade")
}

fn set_field(entity: &str, key: &str, id: &str, field: &str, value: Value) {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let facade = facade();
    let mut found = facade
        .find(entity)
        .unwrap()
        .eq(key, Value::string(id))
        .one()
        .unwrap()
        .expect("the row");
    found.set(field, value);
    found.update(&facade).unwrap();
}

fn field(entity: &str, key: &str, id: &str, field: &str) -> String {
    facade()
        .find(entity)
        .unwrap()
        .disable_authz()
        .eq(key, Value::string(id))
        .one()
        .unwrap()
        .and_then(|r| r.get(field).map(to_plain_string))
        .unwrap_or_default()
}

#[test]
fn a_call_on_two_nodes_carries_media_both_ways_and_changes_a_layer_at_the_leg_node() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start_origin("rtc-cascade");
    let leg_node = LegNode::start(&gate.cluster_dir);
    let http = gate.server.address();
    let factory = std::sync::Arc::clone(gate.server.factory());
    let origin = moqui_core::cluster::instance_id(&factory).unwrap();
    assert_ne!(origin, leg_node.node);

    let (call_id, legs) = {
        let context = factory.new_context();
        let _entered = context.enter();
        wait_for_nodes(&[&origin, &leg_node.node]);
        let callee = create_user("cascade.callee");
        start_call(&origin, &[&callee])
    };
    let (caller_leg, callee_leg) = (legs[0].clone(), legs[1].clone());

    // john's socket is on the origin, the callee's on the leg node
    let john_cookie = crate::common::sign_in(http, "john.doe", "moqui");
    let callee_cookie = crate::common::sign_in(leg_node.http, "cascade.callee", PASSWORD);
    factory.flush_sessions().unwrap();
    let mut john = Peer::new(open_socket(http, &john_cookie), &call_id, &caller_leg);
    let mut callee = Peer::new(open_socket(leg_node.http, &callee_cookie), &call_id, &callee_leg);
    john.join_with_video();
    callee.join();
    // the layer the leg node sends depends on its estimate of the callee's link (mid or high on a busy loopback)
    let layered = |len: Option<usize>| len.is_some_and(|n| n == VIDEO_LAYER_BYTES[1] || n == VIDEO_LAYER_BYTES[2]);
    let flowed = run_until(&mut [&mut john, &mut callee], Duration::from_secs(10), |p| {
        p[0].received >= 20 && p[1].received >= 20 && layered(p[1].last_video_len)
    });
    assert!(
        flowed,
        "media both ways: john {} callee {} (video {:?}); frames {:?} {:?}",
        john.received, callee.received, callee.last_video_len, john.frames, callee.frames
    );
    // the callee's media is on the leg node: its answer names the leg node's media port, and its leg the leg node
    let joined = callee.frame("joined").cloned().unwrap_or_default();
    assert!(
        joined["sdp"]
            .as_str()
            .unwrap_or_default()
            .contains(&format!(" {} typ host", leg_node.media.port())),
        "{joined}"
    );
    {
        let context = factory.new_context();
        let _entered = context.enter();
        assert_eq!(
            field("moqui.call.CallLeg", "callLegId", &callee_leg, "mediaNodeId"),
            leg_node.node
        );
        assert_eq!(
            field("moqui.call.CallLeg", "callLegId", &caller_leg, "mediaNodeId"),
            origin
        );
    }
    // john's track is named to the callee by the leg node, john's connection by the origin across the link
    assert!(callee
        .frames
        .iter()
        .any(|f| f["rtc"] == json!("track") && f["callLegId"] == json!(caller_leg) && f["kind"] == json!("video")));
    assert!(callee.frames.iter().any(|f| f["rtc"] == json!("participant")
        && f["callLegId"] == json!(caller_leg)
        && f["connected"] == json!(true)));

    // a layer change at the leg node: the callee asks for john's low layer
    callee.send(json!({"rtc": "subscribe", "id": 5, "publisher": caller_leg, "layer": "low"}));
    let low = Some(VIDEO_LAYER_BYTES[0]);
    let changed = run_until(&mut [&mut john, &mut callee], Duration::from_secs(5), |p| {
        p[1].last_video_len == low
    });
    assert!(
        changed,
        "the low layer at the leg node: {:?}; frames {:?}",
        callee.last_video_len, callee.frames
    );
    assert!(callee
        .frames
        .iter()
        .any(|f| f["rtc"] == json!("layer") && f["publisher"] == json!(caller_leg) && f["layer"] == json!("low")));
    drop(callee);
    drop(john);
    drop(leg_node);
}

#[test]
fn losing_the_leg_node_ends_only_its_legs() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start_origin("rtc-cascade-loss");
    let mut leg_node = LegNode::start(&gate.cluster_dir);
    let http = gate.server.address();
    let factory = std::sync::Arc::clone(gate.server.factory());
    let origin = moqui_core::cluster::instance_id(&factory).unwrap();

    let (call_id, legs) = {
        let context = factory.new_context();
        let _entered = context.enter();
        wait_for_nodes(&[&origin, &leg_node.node]);
        let near = create_user("cascade.near");
        let far = create_user("cascade.far");
        start_call(&origin, &[&near, &far])
    };
    let john_cookie = crate::common::sign_in(http, "john.doe", "moqui");
    let near_cookie = crate::common::sign_in(http, "cascade.near", PASSWORD);
    let far_cookie = crate::common::sign_in(leg_node.http, "cascade.far", PASSWORD);
    factory.flush_sessions().unwrap();
    let mut john = Peer::new(open_socket(http, &john_cookie), &call_id, &legs[0]);
    let mut near = Peer::new(open_socket(http, &near_cookie), &call_id, &legs[1]);
    let mut far = Peer::new(open_socket(leg_node.http, &far_cookie), &call_id, &legs[2]);
    john.join();
    near.join();
    far.join();
    let flowed = run_until(&mut [&mut john, &mut near, &mut far], Duration::from_secs(10), |p| {
        p.iter().all(|p| p.received >= 40)
    });
    assert!(
        flowed,
        "everyone hears the others: {} {} {}; {:?}",
        john.received, near.received, far.received, far.frames
    );

    // the leg node is lost; its heartbeat stops (aged here, as a minute without one would)
    leg_node.kill();
    let ended = {
        let context = factory.new_context();
        let _entered = context.enter();
        let stale = moqui_core::user::stamp_value(moqui_core::user::now_millis() - 120_000);
        set_field(
            "moqui.call.CallMediaNode",
            "instanceId",
            &leg_node.node,
            "heartbeatDate",
            stale,
        );
        call("org.moqui.call.CallServices.expire#Calls", &[]);
        (
            field("moqui.call.CallLeg", "callLegId", &legs[2], "statusId"),
            field("moqui.call.CallLeg", "callLegId", &legs[2], "endReasonEnumId"),
            field("moqui.call.CallSession", "callSessionId", &call_id, "statusId"),
            field("moqui.call.CallLeg", "callLegId", &legs[0], "statusId"),
            field("moqui.call.CallLeg", "callLegId", &legs[1], "statusId"),
        )
    };
    assert_eq!(
        ended,
        (
            "ClsLeft".to_string(),
            "CerMediaNodeLost".to_string(),
            "CssActive".to_string(),
            "ClsAnswered".to_string(),
            "ClsAnswered".to_string()
        )
    );
    // the origin's legs still hear each other
    let (heard_john, heard_near) = (john.received, near.received);
    let going = run_until(&mut [&mut john, &mut near], Duration::from_secs(3), |p| {
        p[0].received > heard_john + 20 && p[1].received > heard_near + 20
    });
    assert!(going, "the origin's legs go on: {} {}", john.received, near.received);
    drop(leg_node);
}
