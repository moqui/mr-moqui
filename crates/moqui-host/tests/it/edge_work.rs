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

//! How work crosses the edge link (plans/Edge-Node.md §5.2, E3). Both ends are real servers in the test's own process,
//! as in `edge_web.rs`: the local node starts first, then the edge node (`node_role=edge`, set only after the local node
//! read its edge conf); the local node's link is its dialer driven with an explicit `LocalConfig` serving the local
//! server (`local_web`), stopped and started again to cut the link. Both have the same edge conf: the diagnostic
//! `echo#Delegate` delegated, a test queue kind whose handler is a script service of a test component written into each
//! runtime (it writes an `edgetest.EdgeTestEcho` row per item applied; a payload of `refuse` is refused, `slow` takes
//! 1.5 s and `pace` 20 ms), and a replicated set of `moqui.security.UserGroup` descriptions.
//!
//! - Delegate: a listed service runs on the local node as `_edge_` with the caller as data; one listed but not granted
//!   to `_EDGE_` is refused by authorization; one the local node's conf does not list is refused there even when the
//!   edge node's link asks directly; with the link down a call fails at once.
//! - Queue: items of two keys arrive once and in order; a refused head holds its key until it fails for good; an item
//!   whose answer is lost to a cut while its handler runs is delivered again on the next link and applied once (the
//!   redelivery waits on the first apply's receipt); with the link cut mid-stream, the rest and what is queued
//!   meanwhile arrive once, in order, when it is back; the local node's outbox reaches the edge node.
//! - Replicate: a committed change arrives, a rolled-back one does not; an update and a delete go too; a change made
//!   while the link is down arrives after; with the edge node's copies and cursors wiped, the next link sends the set
//!   whole; a local node that lost its notes (as by a restart) sends the set whole under a new epoch.

#![cfg(all(unix, feature = "edge", feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use base64::Engine as _;
use moqui_core::edge::LinkState;
use moqui_core::entity::EntityFacade;
use moqui_host::edge_link::{local_web, pair, Backoff, Dialer, LocalConfig, PairConfig};
use moqui_lib::object::{to_plain_string, Value};
use rustls::pki_types::CertificateDer;
use tokio::sync::watch;

use crate::common::Gate;

const SERVICES: &str = "org.moqui.impl.EdgeServices";
const NODE: &str = "local-q";
const GROUP: &str = "moqui.security.UserGroup";
const ECHO: &str = "edgetest.EdgeTestEcho";
const HANDLER: &str = "edgetest.EdgeTestServices.apply#EchoItem";

/// The edge conf of both nodes: quick retries, three attempts.
const EDGE_CONF: &str = r#"<edge queue-retry-seconds="0.1" queue-max-attempts="3">
    <delegate service="org.moqui.impl.EdgeServices.echo#Delegate"/>
    <delegate service="org.moqui.impl.EdgeServices.get#EdgeHosts"/>
    <queue-kind name="echo" service="edgetest.EdgeTestServices.apply#EchoItem"/>
    <replicate set="groups"><entity name="moqui.security.UserGroup" fields="description"/></replicate>
</edge>"#;

const COMPONENT: &str = r#"<component name="edgetest"/>"#;

const ENTITIES: &str = r#"<entities>
    <entity entity-name="EdgeTestEcho" package="edgetest" use="transactional" cache="never">
        <field name="echoId" type="id" is-pk="true"/>
        <field name="queueItemId" type="id"/>
        <field name="conversationKey" type="text-medium"/>
        <field name="payload" type="text-long"/>
    </entity>
</entities>"#;

const HANDLER_SERVICES: &str = r#"<services>
    <service verb="apply" noun="EchoItem">
        <in-parameters>
            <parameter name="queueItemId"/>
            <parameter name="kind"/>
            <parameter name="conversationKey" required="true"/>
            <parameter name="idempotencyKey"/>
            <parameter name="payload" allow-html="any"/>
            <parameter name="contentLocation"/>
        </in-parameters>
        <actions>
            <if condition="payload == 'refuse'"><return error="true" message="the test handler refuses a payload of refuse"/></if>
            <script>
                const wait = String(payload || '').startsWith('slow') ? 1500 : String(payload || '').startsWith('pace') ? 20 : 0
                const until = Date.now() + wait
                while (Date.now() &lt; until) {}
            </script>
            <entity-make-value value-field="echo" entity-name="edgetest.EdgeTestEcho"/>
            <entity-sequenced-id-primary value-field="echo"/>
            <set field="echo.queueItemId" from="queueItemId"/>
            <set field="echo.conversationKey" from="conversationKey"/>
            <set field="echo.payload" from="payload"/>
            <entity-create value-field="echo"/>
        </actions>
    </service>
</services>"#;

fn component_files<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut files = vec![
        ("component/edgetest/component.xml", COMPONENT),
        ("component/edgetest/entity/EdgeTestEntities.xml", ENTITIES),
        (
            "component/edgetest/service/edgetest/EdgeTestServices.xml",
            HANDLER_SERVICES,
        ),
    ];
    files.extend_from_slice(extra);
    files
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

/// `body` in a context of `gate`'s server, with authorization off.
fn in_context<T>(gate: &Gate, body: impl FnOnce(&EntityFacade) -> T) -> T {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    body(&facade(gate))
}

/// Grant the test handler to `_EDGE_`, as a comms phase's seed data does for its own handlers.
fn grant_handler(gate: &Gate) {
    in_context(gate, |facade| {
        let mut member = facade.make_value("moqui.security.ArtifactGroupMember").unwrap();
        member.set("artifactGroupId", Value::string("EDGE_DELEGATE"));
        member.set("artifactName", Value::string(HANDLER));
        member.set("nameIsPattern", Value::string("N"));
        member.set("artifactTypeEnumId", Value::string("AT_SERVICE"));
        member.set("inheritAuthz", Value::string("Y"));
        member.create(facade).unwrap();
    });
}

/// A service on `gate`'s server as john.doe, with authorization off: its result and the errors it added.
fn call(gate: &Gate, name: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, Vec<String>) {
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let _authz = context.disable_authz();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| {
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().add_error(err.message().to_string()));
        BTreeMap::new()
    });
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    (out, errors)
}

fn edge_service(gate: &Gate, verb_noun: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let (out, errors) = call(gate, &format!("{SERVICES}.{verb_noun}"), parameters);
    assert!(errors.is_empty(), "{verb_noun}: {errors:?}");
    out
}

/// Queue a test item on `gate` for the other end.
fn enqueue(gate: &Gate, key: &str, payload: &str) {
    edge_service(
        gate,
        "enqueue#Item",
        &[
            ("kind", Value::string("echo")),
            ("conversationKey", Value::string(key)),
            ("payload", Value::string(payload)),
        ],
    );
}

/// The payloads the test handler applied on `gate` for `key`, in order.
fn echoed(gate: &Gate, key: &str) -> Vec<String> {
    in_context(gate, |facade| {
        facade
            .find(ECHO)
            .unwrap()
            .eq("conversationKey", Value::string(key))
            .order_by("echoId", false)
            .use_cache(false)
            .list()
            .unwrap()
            .values()
            .iter()
            .map(|row| to_plain_string(row.get("payload").unwrap_or(&Value::Null)))
            .collect()
    })
}

/// Rows of `entity` on `gate`, of those with `field` equal to `value` when given.
fn count(gate: &Gate, entity: &str, filter: Option<(&str, &str)>) -> i64 {
    in_context(gate, |facade| {
        let mut find = facade.find(entity).unwrap().use_cache(false);
        if let Some((field, value)) = filter {
            find = find.eq(field, Value::string(value));
        }
        find.count().unwrap()
    })
}

/// The edge node's item for the local node with `payload`: its status and lease holder.
fn item(edge: &Gate, payload: &str) -> (String, String) {
    in_context(edge, |facade| {
        let row = facade
            .find("moqui.edge.EdgeQueueItem")
            .unwrap()
            .eq("payload", Value::string(payload))
            .use_cache(false)
            .one()
            .unwrap()
            .unwrap_or_else(|| panic!("no item {payload}"));
        let text = |field: &str| row.get(field).map(to_plain_string).unwrap_or_default();
        (text("statusEnumId"), text("leaseOwner"))
    })
}

fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Write (or delete, with `None`) a group on the local node in its own transaction, committed unless `roll_back`.
fn write_group(local: &Gate, id: &str, description: Option<&str>, roll_back: bool) {
    in_context(local, |facade| {
        let tx = facade.transactions_for(GROUP).unwrap();
        let _ = tx.run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
            let mut row = facade.make_value(GROUP)?;
            row.set("userGroupId", Value::string(id));
            match description {
                Some(description) => {
                    row.set("description", Value::string(description));
                    row.create_or_update(facade)?;
                }
                None => row.delete(facade)?,
            }
            if roll_back {
                return Err(moqui_lib::error::Error::new("roll back"));
            }
            Ok(())
        });
    });
}

/// The edge node's copy of the local node's group `id`: its description, or `None`.
fn replica(edge: &Gate, id: &str) -> Option<String> {
    let pk = BTreeMap::from([("userGroupId".to_string(), id.to_string())]);
    in_context(edge, |facade| {
        moqui_core::edge::replica::get_value(facade, Some(NODE), "groups", GROUP, &pk)
            .unwrap()
            .map(|value| value.fields["description"].as_str().unwrap_or_default().to_string())
    })
}

/// The edge node's cursor of the groups set: its epoch.
fn cursor_epoch(edge: &Gate) -> String {
    in_context(edge, |facade| {
        moqui_core::edge::replica::cursors(facade, Some(NODE))
            .unwrap()
            .first()
            .map(|cursor| cursor.epoch.clone())
            .unwrap_or_default()
    })
}

fn wipe_replicas(edge: &Gate) {
    in_context(edge, |facade| {
        for entity in ["moqui.edge.EdgeReplicaValue", "moqui.edge.EdgeReplicaCursor"] {
            facade
                .find(entity)
                .unwrap()
                .eq("pairedNodeId", Value::string(NODE))
                .delete_all()
                .unwrap();
        }
    });
}

/// Whether the edge node has the local node's down connection.
fn down(edge: &Gate) -> bool {
    let out = edge_service(edge, "get#EdgeClients", &[]);
    let Some(Value::List(list)) = out.get("clientList") else {
        panic!("clientList: {out:?}")
    };
    list.iter().any(|row| {
        let Value::Map(row) = row else { return false };
        to_plain_string(&row["pairedNodeId"]) == NODE && row["down"] == Value::Bool(true)
    })
}

#[test]
fn work_crosses_the_link_once_and_in_order_and_survives_a_cut() {
    if !crate::common::own_process() {
        return;
    }
    let (root, cert, key) = certs();
    let local = crate::common::start_with_conf_files("edge-work-local", None, EDGE_CONF, &[], &component_files(&[]));
    let edge_port = crate::common::free_port();
    let edge_port_text = edge_port.to_string();
    let edge = crate::common::start_second_with(
        "edge-work-edge",
        EDGE_CONF,
        &[("node_role", "edge"), ("webapp_https_listen_port", &edge_port_text)],
        &component_files(&[("cert/default.crt", &cert), ("cert/default.key", &key)]),
    );
    grant_handler(&local);
    grant_handler(&edge);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let code = to_plain_string(&edge_service(&edge, "create#PairingCode", &[])["code"]);
    let mut pairing = PairConfig::new(&url, &code, NODE);
    pairing.trust_root = Some(root);
    pairing.connect_timeout = Duration::from_secs(5);
    let paired = rt.block_on(pair(&pairing)).unwrap();

    let mut config = LocalConfig::new(&url, &paired.key, &paired.pin, NODE);
    config.backoff = Backoff {
        first: Duration::from_millis(100),
        max: Duration::from_millis(500),
    };
    config.connect_timeout = Duration::from_secs(3);
    config.web = local_web(local.server.factory());
    assert!(config.web.is_some(), "the local server's app");
    let (_shutdown, shutdown) = watch::channel(false);
    let start = || {
        let dialer = Dialer::start(config.clone(), rt.handle(), shutdown.clone(), None).unwrap();
        let started = Instant::now();
        while dialer.status().state != LinkState::Linked {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "link up: {:?}",
                dialer.status()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        wait_until(Duration::from_secs(5), "the down connection", || down(&edge));
        dialer
    };
    let cut = |dialer: Dialer| {
        dialer.stop();
        drop(dialer);
        wait_until(Duration::from_secs(5), "the link down", || !down(&edge));
    };
    let dialer = start();

    // the first link sends the replicated set whole
    wait_until(Duration::from_secs(10), "the groups whole", || {
        replica(&edge, "ADMIN").is_some()
    });

    // delegate: a listed service runs on the local node as _edge_, and the caller travels as data
    let (out, errors) = call(
        &edge,
        &format!("{SERVICES}.echo#Delegate"),
        &[
            ("message", Value::string("hi")),
            ("edgeCaller", Value::string("forged")),
        ],
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(to_plain_string(&out["message"]), "hi");
    assert_eq!(to_plain_string(&out["username"]), "_edge_");
    let Some(Value::Map(caller)) = out.get("edgeCaller") else {
        panic!("the caller: {out:?}")
    };
    assert_eq!(to_plain_string(&caller["username"]), "john.doe");
    // listed on both nodes but not granted to _EDGE_: authorization refuses it on the local node
    let (_, errors) = call(&edge, &format!("{SERVICES}.get#EdgeHosts"), &[]);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("is not authorized") && error.contains("get#EdgeHosts")),
        "{errors:?}"
    );
    // the edge node's link asking for a service the local node's conf does not list: refused there
    let unlisted = moqui_core::edge::DelegateCall {
        service: format!("{SERVICES}.unpair#EdgeNode"),
        parameters: serde_json::json!({}),
        caller: serde_json::json!({"username": "john.doe"}),
        operation_id: "edge-work-unlisted".to_string(),
        timeout: None,
    };
    let answer = moqui_core::edge::link().unwrap().delegate(&unlisted).unwrap();
    assert_eq!(
        answer.errors,
        [format!(
            "{SERVICES}.unpair#EdgeNode is not a delegated service on this node"
        )]
    );

    // queue: two conversation keys, each in order
    for n in 1..=3 {
        enqueue(&edge, "A", &format!("A{n}"));
        enqueue(&edge, "B", &format!("B{n}"));
    }
    wait_until(Duration::from_secs(10), "the first items", || {
        echoed(&local, "A").len() == 3 && echoed(&local, "B").len() == 3
    });
    assert_eq!(echoed(&local, "A"), ["A1", "A2", "A3"]);
    assert_eq!(echoed(&local, "B"), ["B1", "B2", "B3"]);
    // an idempotency key queued again is the same item
    let keyed = [
        ("kind", Value::string("echo")),
        ("conversationKey", Value::string("D")),
        ("idempotencyKey", Value::string("d-1")),
        ("payload", Value::string("D1")),
    ];
    let first = edge_service(&edge, "enqueue#Item", &keyed);
    let again = edge_service(&edge, "enqueue#Item", &keyed);
    assert_eq!(first["queueItemId"], again["queueItemId"]);
    assert_eq!(again["created"], Value::Bool(false));
    // a kind the conf does not list is refused at enqueue
    let (_, errors) = call(
        &edge,
        &format!("{SERVICES}.enqueue#Item"),
        &[
            ("kind", Value::string("unlisted")),
            ("conversationKey", Value::string("D")),
            ("payload", Value::string("x")),
        ],
    );
    assert!(
        errors.iter().any(|error| error.contains("not a queue kind")),
        "{errors:?}"
    );

    // a refused head holds its key until it fails for good, then the next goes
    enqueue(&edge, "C", "refuse");
    enqueue(&edge, "C", "C2");
    wait_until(Duration::from_secs(10), "C2 after the refused head failed", || {
        echoed(&local, "C") == ["C2"]
    });
    assert_eq!(item(&edge, "refuse").0, "EqsFailed");

    // the answer is lost: the link is cut while the handler runs; the next link delivers it again, and the
    // redelivery waits on the first apply's receipt, then is only acknowledged
    enqueue(&edge, "S", "slow-1");
    wait_until(Duration::from_secs(10), "slow-1 sent", || {
        !item(&edge, "slow-1").1.is_empty()
    });
    std::thread::sleep(Duration::from_millis(300));
    cut(dialer);
    let dialer = start();
    wait_until(Duration::from_secs(10), "slow-1 delivered", || {
        item(&edge, "slow-1").0 == "EqsDelivered"
    });
    assert_eq!(echoed(&local, "S"), ["slow-1"], "applied once");

    // the link is cut mid-stream: what is left, and what is queued meanwhile, arrives once and in order after
    let expected = |key: &str, last: usize| -> Vec<String> { (1..=last).map(|n| format!("pace-{key}-{n}")).collect() };
    for n in 1..=25 {
        enqueue(&edge, "M", &format!("pace-M-{n}"));
        enqueue(&edge, "N", &format!("pace-N-{n}"));
    }
    wait_until(Duration::from_secs(10), "some of the stream", || {
        echoed(&local, "M").len() + echoed(&local, "N").len() >= 8
    });
    cut(dialer);
    std::thread::sleep(Duration::from_millis(300));
    let before = echoed(&local, "M").len() + echoed(&local, "N").len();
    assert!(before < 50, "the cut was mid-stream ({before} of 50 arrived)");
    enqueue(&edge, "M", "pace-M-26");
    enqueue(&edge, "N", "pace-N-26");
    write_group(&local, "EDGE_T_DOWN", Some("Made while down"), false);
    let started = Instant::now();
    let (_, errors) = call(
        &edge,
        &format!("{SERVICES}.echo#Delegate"),
        &[("message", Value::string("hi"))],
    );
    assert!(started.elapsed() < Duration::from_secs(2), "it failed at once");
    assert!(
        errors.iter().any(|error| error.contains(moqui_core::edge::OFFLINE)),
        "{errors:?}"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        echoed(&local, "M").len() + echoed(&local, "N").len(),
        before,
        "nothing arrives while the link is down"
    );
    let dialer = start();
    wait_until(Duration::from_secs(10), "the rest of the stream", || {
        echoed(&local, "M").len() == 26 && echoed(&local, "N").len() == 26
    });
    assert_eq!(echoed(&local, "M"), expected("M", 26));
    assert_eq!(echoed(&local, "N"), expected("N", 26));
    let local_id = moqui_core::edge::queue::local_node_id(local.server.factory());
    let receipts = count(&local, "moqui.edge.EdgeReceipt", Some(("pairedNodeId", &local_id)));
    assert_eq!(
        receipts,
        count(&local, ECHO, None),
        "one receipt per item applied, each applied once"
    );
    assert_eq!(receipts, 3 + 3 + 1 + 1 + 1 + 26 + 26);
    // the change made while the link was down arrives after it
    wait_until(Duration::from_secs(10), "the change made while down", || {
        replica(&edge, "EDGE_T_DOWN").as_deref() == Some("Made while down")
    });

    // the local node's outbox reaches the edge node, with the edge node's receipt
    enqueue(&local, "up-1", "U1");
    wait_until(Duration::from_secs(10), "the outbox item", || {
        echoed(&edge, "up-1") == ["U1"]
    });
    assert_eq!(
        count(&edge, "moqui.edge.EdgeReceipt", Some(("conversationKey", "up-1"))),
        1
    );

    // replicate: a committed change arrives, a rolled-back one never, and an update and a delete go too
    write_group(&local, "EDGE_T_ROLLED", Some("Never"), true);
    write_group(&local, "EDGE_T_ONE", Some("One"), false);
    wait_until(Duration::from_secs(10), "the committed change", || {
        replica(&edge, "EDGE_T_ONE").as_deref() == Some("One")
    });
    assert_eq!(replica(&edge, "EDGE_T_ROLLED"), None);
    write_group(&local, "EDGE_T_ONE", Some("One again"), false);
    wait_until(Duration::from_secs(10), "the update", || {
        replica(&edge, "EDGE_T_ONE").as_deref() == Some("One again")
    });
    write_group(&local, "EDGE_T_ONE", None, false);
    wait_until(Duration::from_secs(10), "the delete", || {
        replica(&edge, "EDGE_T_ONE").is_none()
    });

    // the edge node loses its copies and cursors: the next link sends the set whole again
    cut(dialer);
    wipe_replicas(&edge);
    assert_eq!(replica(&edge, "ADMIN"), None);
    let dialer = start();
    wait_until(Duration::from_secs(10), "the groups whole again", || {
        replica(&edge, "ADMIN").is_some() && replica(&edge, "EDGE_T_DOWN").is_some()
    });

    // the local node loses its notes, as a restart does: a change it never noted arrives in a full send under a new
    // epoch
    cut(dialer);
    let epoch = cursor_epoch(&edge);
    moqui_core::edge::replica::disable(&facade(&local));
    write_group(&local, "EDGE_T_UNNOTED", Some("Not noted"), false);
    let dialer = start();
    wait_until(
        Duration::from_secs(10),
        "the full send after the notes were lost",
        || replica(&edge, "EDGE_T_UNNOTED").as_deref() == Some("Not noted"),
    );
    assert_ne!(cursor_epoch(&edge), epoch, "a new epoch");
    dialer.stop();
}
