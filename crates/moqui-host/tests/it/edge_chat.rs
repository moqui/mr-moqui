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

//! Chat bridges and SMS through the edge node (plans/archive/Edge-Node.md §5.4, E6). Both ends are real servers in the test's
//! own process, as in `edge_mail.rs`, with the edge conf of MoquiDefaultConf.xml (the kinds chatWebhook and chatMedia,
//! the replicated `chatBridges`). An SMS bridge on Telnyx is on the local node; a test Ed25519 key signs its webhooks
//! as Telnyx does, and a fake Telnyx takes its sends.
//!
//! - In: a signed webhook to the edge node while the link is down is answered 200 and queued; the same event again is
//!   dropped; a bad signature is refused at the edge node. When the link is back the text is taken once on the local
//!   node (`receive#Sms`, then `ingest#Event`).
//! - Out: an MMS's file is put on the edge node before the send goes, its address is the edge node's and is served
//!   there, and refused once it ran out.

#![cfg(all(unix, feature = "edge", feature = "embedded-postgres", feature = "channels"))]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use base64::Engine as _;
use moqui_core::edge::LinkState;
use moqui_core::entity::EntityFacade;
use moqui_core::service::chat_bridge::{
    BridgeInfo, ChatBridgeAdapter, DeliveryAttachment, DeliveryKind, DeliveryOutcome, DeliveryRequest,
};
use moqui_host::edge_link::{local_web, pair, Backoff, Dialer, LocalConfig, PairConfig};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use serde_json::json;
use tokio::sync::watch;

use crate::common::Gate;

const NODE: &str = "local-chat";
const OURS: &str = "+15550001000";
const THEIRS: &str = "+15557778888";

/// Both nodes: quick retries. The kinds and the set are MoquiDefaultConf.xml's.
const CONF: &str = r#"<edge queue-retry-seconds="0.1"/>"#;

fn pem(label: &str, der: &[u8]) -> String {
    let text = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = text
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

/// A test authority's root, and a `localhost` certificate and key (PEM) it signed.
fn certs() -> (CertificateDer<'static>, String, String) {
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

fn facade(gate: &Gate) -> EntityFacade {
    gate.server.factory().entity().unwrap()
}

/// A service on `gate`'s server as john.doe, with authorization off; it must add no errors.
fn call(gate: &Gate, name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    assert!(errors.is_empty(), "{name}: {errors:?}");
    out
}

/// Create (or update) a row on `gate`.
fn create(gate: &Gate, entity: &str, fields: &[(&str, Value)]) {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = facade(gate);
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in fields {
        row.set(*name, value.clone());
    }
    row.create_or_update(&facade).unwrap();
}

/// The rows of `entity` on `gate` with these field values.
fn rows(gate: &Gate, entity: &str, conditions: &[(&str, &str)]) -> Vec<BTreeMap<String, String>> {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut find = facade(gate).find(entity).unwrap().use_cache(false);
    for (name, value) in conditions {
        find = find.eq(name, Value::string(*value));
    }
    find.list()
        .unwrap()
        .values()
        .iter()
        .map(|row| {
            row.fields()
                .iter()
                .map(|(name, value)| (name.to_string(), to_plain_string(value)))
                .collect()
        })
        .collect()
}

fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Whether the edge node has the local node's down connection.
fn down(edge: &Gate) -> bool {
    let out = call(edge, "org.moqui.edge.EdgeServices.get#EdgeClients", &[]);
    let Some(Value::List(list)) = out.get("clientList") else {
        panic!("clientList: {out:?}")
    };
    list.iter().any(|row| {
        let Value::Map(row) = row else { return false };
        to_plain_string(&row["pairedNodeId"]) == NODE && row["down"] == Value::Bool(true)
    })
}

/// A fake Telnyx: each POST's body is kept and answered with a new message id.
fn fake_telnyx() -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let sends = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&sends);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            let head_end = loop {
                let Ok(read) = stream.read(&mut chunk) else { break None };
                if read == 0 {
                    break None;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(at + 4);
                }
            };
            let Some(head_end) = head_end else { continue };
            let head = String::from_utf8_lossy(&buffer[..head_end]).to_ascii_lowercase();
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            while buffer.len() < head_end + length {
                match stream.read(&mut chunk) {
                    Ok(read) if read > 0 => buffer.extend_from_slice(&chunk[..read]),
                    _ => break,
                }
            }
            let body: serde_json::Value = serde_json::from_slice(&buffer[head_end..]).unwrap_or_default();
            let count = {
                let mut sends = keep.lock().unwrap();
                sends.push(body);
                sends.len()
            };
            let answer = json!({"data": {"id": format!("msg-{count}"), "to": [{"status": "queued"}]}}).to_string();
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                )
                .as_bytes(),
            );
        }
    });
    (base, sends)
}

/// A Telnyx webhook of `body`, signed now with `key`, posted to the edge node: its status and body.
fn post_webhook(
    client: &reqwest::blocking::Client,
    url: &str,
    bridge_id: &str,
    key: &Ed25519KeyPair,
    body: &serde_json::Value,
) -> (u16, String) {
    let body = body.to_string();
    let stamp = (jiff::Timestamp::now().as_second()).to_string();
    let mut message = stamp.as_bytes().to_vec();
    message.push(b'|');
    message.extend_from_slice(body.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(key.sign(&message).as_ref());
    let response = client
        .post(format!("{url}/channels/{bridge_id}/webhook"))
        .header("content-type", "application/json")
        .header("telnyx-signature-ed25519", signature)
        .header("telnyx-timestamp", stamp)
        .body(body)
        .send()
        .unwrap();
    let status = response.status().as_u16();
    (status, response.text().unwrap_or_default())
}

fn get(client: &reqwest::blocking::Client, url: &str) -> (u16, Vec<u8>) {
    let response = client.get(url).send().unwrap();
    let status = response.status().as_u16();
    (status, response.bytes().map(|bytes| bytes.to_vec()).unwrap_or_default())
}

#[test]
fn texts_cross_the_edge_node_and_wait_while_the_link_is_down() {
    if !crate::common::own_process() {
        return;
    }
    let (root, cert, key) = certs();
    let (telnyx, sends) = fake_telnyx();
    let signer = Ed25519KeyPair::from_seed_unchecked(&[7u8; 32]).unwrap();
    let public = base64::engine::general_purpose::STANDARD.encode(signer.public_key().as_ref());
    let properties = [
        ("channels_webhooks_enabled", "true"),
        ("channels_media_key", "an-edge-chat-test-key"),
    ];
    let local = crate::common::start_with_conf_files("edge-chat-local", None, CONF, &properties, &[]);
    // the bridge and a number routed to john.doe, before the first link sends the set whole
    let config = json!({"apiKey": "KEY-1", "publicKey": public, "apiBaseUrl": telnyx, "allowInternalAddresses": true});
    call(
        &local,
        "org.moqui.chat.ChatBridgeServices.store#Bridge",
        &[
            ("name", Value::string("Telnyx")),
            ("typeEnumId", Value::string("CbtSms")),
            ("adapterName", Value::string("sms")),
            ("configJson", Value::string(config.to_string())),
        ],
    );
    let bridge = rows(&local, "moqui.chat.ChatBridge", &[("name", "Telnyx")]).remove(0);
    assert_eq!(
        bridge["webhookPublicKey"], public,
        "the webhook's key, kept apart from the configuration"
    );
    let bridge_id = bridge["chatBridgeId"].clone();
    let bridge_id = bridge_id.as_str();
    create(
        &local,
        "moqui.call.PhoneNumber",
        &[
            ("phoneNumber", Value::string(OURS)),
            ("messagingBridgeId", Value::string(bridge_id)),
            ("routeTypeEnumId", Value::string("PnrUser")),
            ("routeId", Value::string("EX_JOHN_DOE")),
        ],
    );
    create(
        &local,
        "moqui.call.PhoneNumberUser",
        &[
            ("phoneNumber", Value::string(OURS)),
            ("userId", Value::string("EX_JOHN_DOE")),
            ("fromDate", Value::I64(jiff::Timestamp::now().as_millisecond() - 60_000)),
        ],
    );
    create(
        &local,
        "moqui.call.VoicePolicy",
        &[
            ("voicePolicyId", Value::string("DEFAULT")),
            ("homeCountryCode", Value::string("1")),
        ],
    );

    let edge_port = crate::common::free_port();
    let edge_port_text = edge_port.to_string();
    let edge = crate::common::start_second_with(
        "edge-chat-edge",
        CONF,
        &[
            ("node_role", "edge"),
            ("webapp_https_listen_port", &edge_port_text),
            ("channels_webhooks_enabled", "true"),
        ],
        &[("cert/default.crt", &cert), ("cert/default.key", &key)],
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let code = to_plain_string(&call(&edge, "org.moqui.edge.EdgeServices.create#PairingCode", &[])["code"]);
    let mut pairing = PairConfig::new(&url, &code, NODE);
    pairing.trust_root = Some(root.clone());
    pairing.connect_timeout = Duration::from_secs(5);
    let paired = rt.block_on(pair(&pairing)).unwrap();
    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, NODE);
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(3);
    config.web = local_web(local.server.factory());
    let (_shutdown, shutdown) = watch::channel(false);
    let start = || {
        let dialer = Dialer::start(config.clone(), rt.handle(), shutdown.clone(), None).unwrap();
        wait_until(Duration::from_secs(10), "the link up", || {
            dialer.status().state == LinkState::Linked
        });
        wait_until(Duration::from_secs(5), "the down connection", || down(&edge));
        dialer
    };
    let client = reqwest::blocking::Client::builder()
        .tls_certs_only([reqwest::Certificate::from_der(root.as_ref()).unwrap()])
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();

    let dialer = start();
    wait_until(Duration::from_secs(10), "the bridges replicated", || {
        let facade = facade(&edge);
        moqui_core::edge::replica::cursors(&facade, Some(NODE))
            .unwrap()
            .iter()
            .any(|cursor| cursor.set_name == "chatBridges" && cursor.snapshot_date.is_some())
    });
    let replicated = rows(&edge, "moqui.edge.EdgeReplicaValue", &[("setName", "chatBridges")]);
    assert_eq!(replicated.len(), 1, "{replicated:?}");
    assert!(
        !replicated[0]["fieldsJson"].contains("KEY-1"),
        "the provider's key stays on the local node"
    );

    // a disabled bridge: its network gets 503 from the edge node, so it tries again
    let disable = |flag: &str| {
        call(
            &local,
            "org.moqui.chat.ChatBridgeServices.store#Bridge",
            &[
                ("chatBridgeId", Value::string(bridge_id)),
                ("disabled", Value::string(flag)),
            ],
        );
        let want = flag == "Y";
        wait_until(Duration::from_secs(10), "the bridge's status replicated", || {
            replica_field(&edge, bridge_id, "statusId") == Some("CbsDisabled".to_string()) || !want
        });
        if !want {
            wait_until(Duration::from_secs(10), "the bridge enabled on the edge node", || {
                replica_field(&edge, bridge_id, "statusId") != Some("CbsDisabled".to_string())
            });
        }
    };
    let probe = json!({"data": {"event_type": "message.finalized", "id": "ev-probe",
        "payload": {"id": "none", "to": [{"phone_number": THEIRS, "status": "delivered"}]}}});
    disable("Y");
    let (status, _) = post_webhook(&client, &url, bridge_id, &signer, &probe);
    assert_eq!(status, 503, "a disabled bridge, to its network");
    disable("N");

    // a bridge with no webhook key in its copy is refused (and logged); one saved before E6 gets its key at start
    let no_key = json!({"apiKey": "KEY-2", "apiBaseUrl": telnyx});
    let old = json!({"apiKey": "KEY-3", "apiBaseUrl": telnyx, "publicKey": public, "webhookToleranceSeconds": 120});
    for (id, config) in [("SMSNOKEY", &no_key), ("SMSOLD", &old)] {
        create(
            &local,
            "moqui.chat.ChatBridge",
            &[
                ("chatBridgeId", Value::string(id)),
                ("typeEnumId", Value::string("CbtSms")),
                ("adapterName", Value::string("sms")),
                ("name", Value::string(id)),
                ("statusId", Value::string("CbsNotConnected")),
                ("configJson", Value::string(config.to_string())),
            ],
        );
    }
    let filled = in_context(&local, |facade| {
        moqui_core::service::chat_bridge::fill_webhook_keys(facade)
    });
    assert_eq!(filled, 1, "only the bridge with a key in its configuration");
    let old_row = rows(&local, "moqui.chat.ChatBridge", &[("chatBridgeId", "SMSOLD")]).remove(0);
    assert_eq!(old_row["webhookPublicKey"], public);
    assert_eq!(old_row["webhookToleranceSeconds"], "120");
    wait_until(Duration::from_secs(10), "the new bridges replicated", || {
        replica_field(&edge, "SMSNOKEY", "adapterName").is_some()
            && replica_field(&edge, "SMSOLD", "webhookPublicKey").is_some_and(|key| key == public)
    });
    let (status, _) = post_webhook(&client, &url, "SMSNOKEY", &signer, &probe);
    assert_eq!(status, 401, "no key in the copy");
    let (status, body) = post_webhook(&client, &url, "SMSOLD", &signer, &probe);
    assert_eq!((status, body.contains("queued")), (200, true), "{body}");

    // out: an MMS's file goes to the edge node before the send, and the edge node serves it until it runs out
    moqui_lib::env::set_property("channels_public_url", url.clone());
    moqui_lib::env::set_property(moqui_core::edge::NODE_URL, url.clone());
    let picture: Vec<u8> = (0..1500u32).map(|n| (n % 251) as u8).collect();
    for id in ["A1", "A2", "A3"] {
        attach(&local, id, &picture);
    }
    let mms = |key: &str, attachment: &str| send_mms(&local, &telnyx, bridge_id, key, attachment);
    // the send waits (not counted) until its file is on the edge node
    let outcome = until_sent(|| mms("edge-mms-1", "A1"));
    assert!(matches!(outcome, DeliveryOutcome::Accepted { .. }), "{outcome:?}");
    let sent = sends.lock().unwrap()[0].clone();
    let media = sent["media_urls"][0].as_str().unwrap().to_string();
    assert!(
        media.starts_with(&format!("{url}/channels/{bridge_id}/media/")),
        "the edge node's address: {media}"
    );
    let (status, bytes) = get(&client, &media);
    assert_eq!(status, 200, "served by the edge node");
    assert_eq!(bytes, picture);
    // only for its bridge, and not once it ran out
    let other = media.replace(&format!("/channels/{bridge_id}/"), "/channels/NOT_IT/");
    let (status, body) = get(&client, &other);
    assert_eq!(status, 404, "{other}: {}", String::from_utf8_lossy(&body));
    let kept = rows(&edge, "moqui.edge.EdgeMedia", &[("chatBridgeId", bridge_id)]);
    assert_eq!(kept.len(), 1, "{kept:?}");
    let expire = || {
        in_context(&edge, |facade| {
            facade
                .find("moqui.edge.EdgeMedia")
                .unwrap()
                .update_all([(
                    "expireDate".to_string(),
                    moqui_core::user::stamp_value(jiff::Timestamp::now().as_millisecond() - 1000),
                )])
                .unwrap()
        })
    };
    expire();
    let (status, _) = get(&client, &media);
    assert_eq!(status, 404, "refused once it ran out");
    assert!(rows(&edge, "moqui.edge.EdgeMedia", &[]).is_empty(), "and removed");

    // a file over the edge node's limit is refused there, and the send waits, not failed
    moqui_lib::env::set_property("channels_edge_media_max_bytes", "1000");
    let outcome = mms("edge-mms-2", "A2");
    assert!(matches!(outcome, DeliveryOutcome::Wait { .. }), "{outcome:?}");
    wait_until(Duration::from_secs(10), "the file refused by the edge node", || {
        rows(
            &local,
            "moqui.edge.EdgeQueueItem",
            &[("conversationKey", "chatMedia:A2")],
        )
        .first()
        .and_then(|row| row.get("lastErrorText").cloned())
        .is_some_and(|error| error.contains("channels_edge_media_max_bytes"))
    });
    assert!(rows(&edge, "moqui.edge.EdgeMedia", &[]).is_empty());
    moqui_lib::env::set_property("channels_edge_media_max_bytes", "5242880");

    // in: the link down, a signed text is taken by the edge node and queued, once; an MMS waits for the link
    dialer.stop();
    drop(dialer);
    wait_until(Duration::from_secs(5), "the link down", || !down(&edge));
    let outcome = mms("edge-mms-3", "A3");
    assert!(
        matches!(outcome, DeliveryOutcome::Wait { .. }),
        "held, not failed: {outcome:?}"
    );
    let text = json!({"data": {"event_type": "message.received", "id": "ev-edge-1",
        "occurred_at": "2026-11-01T12:00:00.000+00:00", "record_type": "event",
        "payload": {"id": "in-edge-1", "direction": "inbound", "type": "SMS",
            "from": {"phone_number": THEIRS}, "to": [{"phone_number": OURS}],
            "text": "Sent while the office was away", "received_at": "2026-11-01T12:00:00.000+00:00"}}});
    let (status, body) = post_webhook(&client, &url, bridge_id, &signer, &text);
    assert_eq!((status, body.contains("queued")), (200, true), "{body}");
    let (status, body) = post_webhook(&client, &url, bridge_id, &signer, &text);
    assert_eq!(
        (status, body.contains("duplicate")),
        (200, true),
        "the repeat is dropped: {body}"
    );
    let forger = Ed25519KeyPair::from_seed_unchecked(&[9u8; 32]).unwrap();
    let (status, _) = post_webhook(&client, &url, bridge_id, &forger, &text);
    assert_eq!(status, 401, "a bad signature is refused at the edge node");
    let text_key = format!("chatWebhook:{bridge_id}:ev-edge-1");
    let text_item = |status: &str| {
        rows(
            &edge,
            "moqui.edge.EdgeQueueItem",
            &[("idempotencyKey", text_key.as_str()), ("statusEnumId", status)],
        )
        .len()
    };
    assert_eq!(text_item("EqsPending"), 1, "queued once");
    let taken = || {
        rows(
            &local,
            "moqui.chat.ChatEventExternal",
            &[("chatBridgeId", bridge_id), ("externalEventId", "in-edge-1")],
        )
        .len()
    };
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(taken(), 0, "nothing arrives while the link is down");

    // the link back: the text taken once on the local node, and the held MMS goes
    let dialer = start();
    wait_until(Duration::from_secs(10), "the text on the local node", || taken() == 1);
    wait_until(Duration::from_secs(5), "the item delivered", || {
        text_item("EqsDelivered") == 1
    });
    let batches = rows(
        &local,
        "moqui.chat.ChatBridgeInbound",
        &[("chatBridgeId", bridge_id), ("transactionId", "ev-edge-1")],
    );
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(taken(), 1);
    let outcome = until_sent(|| mms("edge-mms-3", "A3"));
    assert!(matches!(outcome, DeliveryOutcome::Accepted { .. }), "{outcome:?}");

    // the edge node's retention pass removes a file that ran out, asked for or not
    // A3's, and A2's once the limit was raised and its item tried again
    let kept = rows(&edge, "moqui.edge.EdgeMedia", &[]).len();
    assert!(kept >= 1, "{kept}");
    expire();
    let factory = edge.server.factory();
    let removed = in_context(&edge, |facade| {
        moqui_core::service::chat_bridge::sweep_edge_media(factory, facade, jiff::Timestamp::now().as_millisecond())
    });
    assert_eq!(removed, kept);
    assert!(rows(&edge, "moqui.edge.EdgeMedia", &[]).is_empty());
    dialer.stop();
}

/// Run `work` on `gate`'s facade in a context of its own, with authorization off.
fn in_context<T>(gate: &Gate, work: impl FnOnce(&EntityFacade) -> T) -> T {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    work(&facade(gate))
}

/// A field of bridge `bridge_id`'s replicated copy on the edge node.
fn replica_field(edge: &Gate, bridge_id: &str, name: &str) -> Option<String> {
    in_context(edge, |facade| {
        let key = BTreeMap::from([("chatBridgeId".to_string(), bridge_id.to_string())]);
        moqui_core::edge::replica::get_value(facade, Some(NODE), "chatBridges", "moqui.chat.ChatBridge", &key)
            .unwrap()
            .and_then(|value| value.fields[name].as_str().map(str::to_string))
    })
}

/// A file `id` of `bytes` on the local node, as a message's attachment.
fn attach(local: &Gate, id: &str, bytes: &[u8]) {
    let location = format!("dbresource://chat/edge-test/{id}/photo.png");
    in_context(local, |facade| facade.write_db_resource(&location, bytes).unwrap());
    create(
        local,
        "moqui.chat.ChatAttachment",
        &[
            ("chatAttachmentId", Value::string(id)),
            ("location", Value::string(&location)),
            ("fileName", Value::string("photo.png")),
            ("contentType", Value::string("image/png")),
            ("fileSize", Value::I64(i64::try_from(bytes.len()).unwrap())),
        ],
    );
}

/// One attempt of an MMS with file `attachment`, through the SMS adapter on the local node, as its delivery worker
/// makes it (`key` its delivery's idempotency key).
fn send_mms(local: &Gate, telnyx: &str, bridge_id: &str, key: &str, attachment: &str) -> DeliveryOutcome {
    in_context(local, |_| {
        let info = BridgeInfo {
            chat_bridge_id: bridge_id.to_string(),
            type_enum_id: "CbtSms".to_string(),
            adapter_name: "sms".to_string(),
            name: "Telnyx".to_string(),
            config: json!({"apiKey": "KEY-1", "apiBaseUrl": telnyx, "allowInternalAddresses": true}),
        };
        let request = DeliveryRequest {
            chat_bridge_delivery_id: String::new(),
            idempotency_key: key.to_string(),
            chat_room_id: String::new(),
            event_seq: 0,
            external_room_id: format!("{OURS}/{THEIRS}"),
            kind: DeliveryKind::Message,
            text: Some("A picture".to_string()),
            html: None,
            sender_name: None,
            sender_user_id: None,
            attachments: vec![DeliveryAttachment {
                chat_attachment_id: attachment.to_string(),
                file_name: Some("photo.png".to_string()),
                content_type: Some("image/png".to_string()),
                file_size: Some(1500),
            }],
            target_remote_id: None,
            reaction_key: None,
            attempt: 1,
        };
        moqui_channels::telnyx::TelnyxSms::new().deliver(&info, &request)
    })
}

/// The worker's attempts of one send, until it is not waiting for its files (10 s at most).
fn until_sent(attempt: impl Fn() -> DeliveryOutcome) -> DeliveryOutcome {
    let started = Instant::now();
    loop {
        let outcome = attempt();
        if !matches!(outcome, DeliveryOutcome::Wait { .. }) || started.elapsed() > Duration::from_secs(10) {
            return outcome;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
