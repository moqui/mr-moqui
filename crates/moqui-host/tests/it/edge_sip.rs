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

//! SIP on the edge node and the voicemail fallback (plans/archive/Edge-Node.md §5.4 "Voice (SIP)", decision 4, E8). The edge
//! node runs in a child process of this test binary (the test `edge_sip::edge_node_process`) with its SIP engine and
//! call media, on a database of its own, as `edge_call.rs`'s does: the SIP engine, the media driver and the notify bus
//! are process globals. The local node, in the test's process, has no SIP engine or media of its own; it pairs with the
//! edge node and its link is a dialer the test drives.
//!
//! The fake trunk of `sip.rs` is set up on the local node; the edge node gets it by replication and registers it. A
//! call from the trunk to the edge node is routed by the local node (`receive#PhoneCall`, delegated): its user is busy,
//! so it goes to voicemail (`begin#Voicemail`, delegated), recorded on the edge node and queued to the local node, which
//! stores it in the box the call went to. With the link down, a second call is answered from the number's replicated
//! fallback; its message waits on the edge node's queue and is stored, in the number's box, once the link is back.

#![cfg(all(unix, feature = "edge", feature = "sip", feature = "embedded-postgres"))]

use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use moqui_core::edge::LinkState;
use moqui_host::edge_link::{local_web, pair, Backoff, Dialer, LocalConfig, PairConfig};
use moqui_lib::object::{to_plain_string, Value};
use moqui_sip::media::PhoneMedia;
use moqui_sip::sdp;
use rsipstack::sip::prelude::HeadersExt;
use serde_json::json;
use tokio::sync::watch;

use crate::edge_call::{certs, field, wait_until, EdgeNode};
use crate::rtc::{call, text};
use crate::sip::{register_text, tone, values, Trunk};

/// What the edge node's child process is given; set only for that child.
const EDGE_ENV: &str = "MOQUI_EDGE_SIP_NODE";
const EDGE_TEST: &str = "edge_sip::edge_node_process";
const NUMBER: &str = "+15551230000";
const CALLER: &str = "+15557770000";
const OFFLINE_CALLER: &str = "+15557771111";
/// The edge node's registrar's realm, which the local node's phones' verifiers must be for.
const EDGE_REALM: &str = "sip.edge.example.test";
const DESK: &str = "edge.desk";
const DESK_PASSWORD: &str = "Edge-desk-77!";

/// The edge node's process: an edge node with call media and a SIP engine on its own database, until its stdin closes.
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
        "edge-sip-edge",
        None,
        "",
        &[
            ("node_role", "front_edge"),
            ("webapp_https_listen_port", &port),
            ("rtc_media_port", "0"),
            ("sip_port", "0"),
            ("sip_rtp_ports", "42600-42699"),
            ("sip_registrar_enabled", "true"),
            ("sip_domain", EDGE_REALM),
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
        "EDGE_NODE node={node} code={code} media={} sip={} runtime={}",
        gate.server.rtc_address().unwrap(),
        gate.server.sip_address().expect("the edge node's SIP listener"),
        gate.runtime.display()
    );
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    drop(gate);
}

/// A call from `from` to the number, at the edge node's SIP address: answered for voicemail, the caller talks over the
/// greeting and the beep and after it, presses `#`, and the engine hangs up after its goodbye.
fn leave_message(trunk: &Trunk, sip_at: SocketAddr, call_id: &str, from: &str) {
    // the last call's goodbye, still on the trunk's RTP socket, would latch this call's media to its port
    let mut buf = vec![0u8; 2048];
    while trunk.rtp.recv_from(&mut buf).is_ok() {}
    let phone_local = trunk.local_media();
    let offer = sdp::offer(&phone_local);
    let invite = format!(
        "INVITE sip:{NUMBER}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-{call_id}\r\nMax-Forwards: 70\r\n\
         From: <sip:{from}@127.0.0.1>;tag={call_id}\r\nTo: <sip:{NUMBER}@{sip_at}>\r\nCall-ID: {call_id}\r\n\
         CSeq: 1 INVITE\r\nContact: <sip:trunk@{}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{offer}",
        trunk.addr(),
        trunk.addr(),
        offer.len()
    );
    trunk.send(&invite, sip_at);
    let ok = trunk.response(200);
    let settled = sdp::accept(&phone_local, &sdp::parse(&String::from_utf8_lossy(&ok.body)).unwrap()).unwrap();
    let to = ok.to_header().unwrap().value().to_string();
    let ack = format!(
        "ACK sip:{NUMBER}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-{call_id}-ack\r\nMax-Forwards: 70\r\n\
         From: <sip:{from}@127.0.0.1>;tag={call_id}\r\nTo: {to}\r\nCall-ID: {call_id}\r\nCSeq: 1 ACK\r\n\
         Content-Length: 0\r\n\r\n",
        trunk.addr()
    );
    trunk.send(&ack, sip_at);
    let mut phone = PhoneMedia::new(&settled).unwrap();
    // the greeting and the beep take 1.4 s: # after 2.7 s leaves about 1.3 s of message
    let (mut heard, mut tick, mut pressed) = (0, 0, false);
    let mut next = Instant::now();
    let hash_at = Instant::now() + Duration::from_millis(2700);
    let until = Instant::now() + Duration::from_millis(3000);
    while Instant::now() < until {
        if Instant::now() >= next {
            next += Duration::from_millis(20);
            if !pressed && Instant::now() >= hash_at {
                phone.send_digits("#");
                pressed = true;
            }
            if let Some(packet) = phone.send(&tone(tick)) {
                trunk.rtp.send_to(&packet, phone.remote()).unwrap();
            }
            tick += 1;
        }
        while let Ok((len, from)) = trunk.rtp.recv_from(&mut buf) {
            phone.receive(from, &buf[..len]);
            heard += 1;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        heard >= 50,
        "the caller hears the greeting and the beep from the edge node: {heard} packets"
    );
    let (bye, from) = trunk.request("BYE");
    trunk.reply(&bye, from, 200, "", "");
}

/// John's voicemails on the local node, newest first.
fn voicemails(
    factory: &std::sync::Arc<moqui_core::context::ExecutionContextFactory>,
) -> Vec<std::collections::BTreeMap<String, Value>> {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let found = call("org.moqui.call.VoicemailServices.find#Voicemails", &[]);
    match found.get("voicemailList") {
        Some(Value::List(list)) => list
            .iter()
            .filter_map(|v| match v {
                Value::Map(map) => Some(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Waits until the local node's replicated set `set` has no write waiting and was acknowledged after `since`.
fn replicated(factory: &moqui_core::context::ExecutionContextFactory, set: &str, since: i64) {
    let facade = factory.entity().unwrap();
    wait_until(Duration::from_secs(5), &format!("the set {set} replicated"), || {
        moqui_core::edge::replica::replica(&facade).is_some_and(|replica| {
            replica
                .status()
                .iter()
                .any(|s| s.name == set && s.waiting == 0 && s.last_ack.is_some_and(|ack| ack >= since))
        })
    });
}

#[test]
fn a_trunk_call_to_the_edge_node_is_routed_by_the_local_node_and_a_message_left_while_it_is_away_arrives_later() {
    if !crate::common::own_process() {
        return;
    }
    let started = Instant::now();
    let (root, cert, key) = certs();
    let edge_port = crate::common::free_port();
    // the edge node boots in its process while the local node boots in this one
    let (child, ready) = EdgeNode::spawn_test(
        EDGE_TEST,
        EDGE_ENV,
        &json!({"port": edge_port, "cert": cert, "key": key}),
    );
    let local = crate::common::start_with_properties("edge-sip-local", None, &[]);
    let edge = EdgeNode::ready(child, &ready);
    let sip_at: SocketAddr = edge.field("sip").parse().expect("the edge node's SIP address");
    let factory = std::sync::Arc::clone(local.server.factory());
    assert!(local.server.sip_address().is_none(), "the local node has no SIP engine");
    let local_id = moqui_core::cluster::instance_id(&factory).unwrap();

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
    let dialer = Dialer::start(config.clone(), rt.handle(), shutdown.clone(), None).unwrap();
    wait_until(Duration::from_secs(10), "the link up", || {
        dialer.status().state == LinkState::Linked
    });
    assert_eq!(
        dialer.status().peer.sip_address,
        sip_at.to_string(),
        "the SIP address from its Hello"
    );

    // the local node keeps the edge node's SipNode row from the link, beside its media row: the only voice node
    let trunk = Trunk::new();
    {
        let context = factory.new_context();
        let _entered = context.enter();
        wait_until(Duration::from_secs(5), "the edge node's SIP and media rows", || {
            field("moqui.call.SipNode", "instanceId", &edge.node, "edgeLinked") == "Y"
                && field("moqui.call.CallMediaNode", "instanceId", &edge.node, "edgeLinked") == "Y"
        });
        assert_eq!(
            field("moqui.call.SipNode", "instanceId", &edge.node, "sipAddress"),
            sip_at.to_string()
        );
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let since = moqui_core::user::now_millis();
        let trunk_id = text(
            &call(
                "org.moqui.call.VoiceServices.store#VoiceTrunk",
                &[
                    ("name", Value::string("Fake trunk")),
                    ("host", Value::string("127.0.0.1")),
                    ("port", Value::I32(i32::from(trunk.addr().port()))),
                    ("transportEnumId", Value::string("VtrUdp")),
                    ("mediaEncryptionEnumId", Value::string("VmePlain")),
                    ("providerProfileEnumId", Value::string("VppPlainLan")),
                    ("username", Value::string("acct")),
                    ("password", Value::string("pw-of-acct")),
                    ("registerFlag", Value::string("Y")),
                ],
            ),
            "voiceTrunkId",
        );
        let john = moqui_core::user::current_user_id().unwrap().expect("john is signed in");
        call(
            "org.moqui.call.VoiceServices.store#PhoneNumber",
            &[
                ("phoneNumber", Value::string(NUMBER)),
                ("voiceTrunkId", Value::string(&trunk_id)),
                ("routeTypeEnumId", Value::string("PnrUser")),
                ("routeId", Value::string(&john)),
            ],
        );
        // john is in another call: the local node sends his calls to voicemail at once
        call(
            "store#moqui.security.UserStatus",
            &[("userId", Value::string(&john)), ("inCall", Value::string("Y"))],
        );
        // the fallback for while this node is away (the edge link writes them every minute; a paired node's services
        // as their sources change, which this test's dialer, not the link hook, does not make)
        let refreshed = call("org.moqui.call.VoiceServices.refresh#PhoneFallbacks", &[]);
        assert_eq!(text(&refreshed, "writtenCount"), "1", "{refreshed:?}");
        assert_eq!(
            field("moqui.call.PhoneNumberFallback", "phoneNumber", NUMBER, "voicemailFlag"),
            "Y"
        );
        replicated(&factory, "voiceFallbacks", since);
        replicated(&factory, "voiceTrunks", since);
    }

    // the edge node registers the trunk from its copy
    let (register, from) = trunk.request("REGISTER");
    let contact = values(&register.headers, "Contact")[0].clone();
    assert!(
        contact.contains(&sip_at.port().to_string()),
        "from the edge node: {contact}"
    );
    trunk.reply(&register, from, 200, &format!("Contact: {contact};expires=300\r\n"), "");

    // a call through the trunk to the edge node, routed by the local node: busy, so to voicemail
    leave_message(&trunk, sip_at, "edge-sip-1", CALLER);
    wait_until(
        Duration::from_secs(5),
        "the routed message stored on the local node",
        || voicemails(&factory).len() == 1,
    );
    let routed = voicemails(&factory).remove(0);
    assert_eq!(routed.get("fromNumber").map(to_plain_string).as_deref(), Some(CALLER));
    let call_id = routed.get("callSessionId").map(to_plain_string).unwrap_or_default();
    assert!(!call_id.is_empty(), "the call the local node routed: {routed:?}");
    {
        let context = factory.new_context();
        let _entered = context.enter();
        assert_eq!(
            field("moqui.call.CallSession", "callSessionId", &call_id, "mediaNodeId"),
            edge.node,
            "the call's media is on the edge node"
        );
        assert!(!field("moqui.call.CallSession", "callSessionId", &call_id, "voicemailBoxId").is_empty());
    }

    // a desk phone outside the office: made on the local node for the edge node's realm, registered with the edge
    // node's registrar (its answer checked on the local node), and rung by a colleague's call through the edge node
    let desk_phone = Trunk::new();
    let (desk_id, device) = {
        let context = factory.new_context();
        let _entered = context.enter();
        let created = call(
            "org.moqui.impl.UserServices.create#UserAccount",
            &[
                ("username", Value::string(DESK)),
                ("newPassword", Value::string(DESK_PASSWORD)),
                ("newPasswordVerify", Value::string(DESK_PASSWORD)),
                ("userFullName", Value::string("Edge Desk")),
            ],
        );
        moqui_core::user::internal_login_user(DESK).unwrap();
        let device = call(
            "org.moqui.call.SipDeviceServices.create#SipDevice",
            &[("name", Value::string("Home desk"))],
        );
        (text(&created, "userId"), device)
    };
    assert_eq!(text(&device, "domain"), EDGE_REALM, "the edge node's realm: {device:?}");
    assert_eq!(
        device.get("registrarEnabled"),
        Some(&Value::Bool(true)),
        "the edge node's registrar"
    );
    let (user, password) = (text(&device, "username"), text(&device, "password"));
    let at = desk_phone.addr();
    desk_phone.send(&register_text(at, sip_at, &user, 1, None), sip_at);
    let challenge = desk_phone.response(401);
    let challenges: Vec<moqui_sip::digest::Challenge> = values(&challenge.headers, "WWW-Authenticate")
        .iter()
        .filter_map(|v| moqui_sip::digest::Challenge::parse(v))
        .collect();
    let credentials = moqui_sip::digest::Credentials {
        username: user.clone(),
        password,
        algorithms: vec![moqui_sip::digest::Algorithm::Sha256],
        ..Default::default()
    };
    let chosen = moqui_sip::digest::choose(&challenges, &credentials).expect("a SHA-256 challenge");
    assert_eq!(chosen.realm, EDGE_REALM);
    let auth = moqui_sip::digest::answer(&credentials, chosen, "REGISTER", &format!("sip:{sip_at}"), "c1").unwrap();
    desk_phone.send(&register_text(at, sip_at, &user, 2, Some(&auth)), sip_at);
    desk_phone.response(200);
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user(DESK).unwrap();
        wait_until(
            Duration::from_secs(5),
            "the desk phone's binding kept on the local node",
            || {
                let listed = call("org.moqui.call.SipDeviceServices.find#SipDevices", &[]);
                matches!(listed.get("deviceList"), Some(Value::List(list)) if list.iter().any(|d| matches!(d,
                Value::Map(m) if matches!(m.get("registrationList"), Some(Value::List(r)) if !r.is_empty()))))
            },
        );
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let started = call(
            "org.moqui.call.CallServices.start#Call",
            &[("calleeUserIdList", Value::List(vec![Value::string(&desk_id)]))],
        );
        assert!(!text(&started, "callSessionId").is_empty(), "{started:?}");
    }
    let (invite, from) = desk_phone.request("INVITE");
    assert!(
        invite.uri.to_string().contains(&user),
        "the edge node rings the desk phone: {}",
        invite.uri
    );
    assert_eq!(from, sip_at, "over the flow it registered on");
    desk_phone.reply(&invite, from, 603, "", "");

    // the link down: the local node is away; the edge node answers from the number's fallback
    dialer.stop();
    drop(dialer);
    std::thread::sleep(Duration::from_millis(300));
    leave_message(&trunk, sip_at, "edge-sip-2", OFFLINE_CALLER);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        voicemails(&factory).len(),
        1,
        "the message waits on the edge node while the link is down"
    );

    // the link back: the queued message is stored in the number's box, with its owner's notices
    let dialer = Dialer::start(config, rt.handle(), shutdown, None).unwrap();
    wait_until(
        Duration::from_secs(10),
        "the message left while the local node was away",
        || voicemails(&factory).len() == 2,
    );
    let later = voicemails(&factory)
        .into_iter()
        .find(|v| v.get("fromNumber").map(to_plain_string).as_deref() == Some(OFFLINE_CALLER))
        .expect("the offline caller's message");
    assert!(
        later
            .get("callSessionId")
            .is_none_or(|id| to_plain_string(id).is_empty()),
        "no call on the local node: {later:?}"
    );
    let duration: u64 = later
        .get("durationMs")
        .map(to_plain_string)
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);
    assert!((1000..=2600).contains(&duration), "about 1.3 s recorded: {duration} ms");
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let file = call(
            "org.moqui.call.VoicemailServices.get#VoicemailAudio",
            &[("voicemailId", later["voicemailId"].clone())],
        );
        let _authz = context.disable_authz();
        let bytes = factory
            .resource()
            .location_bytes(&text(&file, "location"))
            .unwrap()
            .expect("the audio is stored");
        assert!(bytes.starts_with(b"OggS"), "Ogg Opus");
    }
    dialer.stop();
    drop(edge);
    let took = started.elapsed();
    eprintln!("edge SIP test took {took:?}");
}
