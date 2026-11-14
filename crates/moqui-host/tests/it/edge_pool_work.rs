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

//! Work from a pool, and web protection shared by its members (plans/Edge-Node2.md §4.4 "Work from the pool to the
//! edge node", §4.3, E17 part C). The servers are in the test's own process, as in `edge_work.rs`; a second member of the local
//! node's cluster is `common::start_member` on the same database. Two servers of one process share the notification
//! bus, so a cluster message reaches the other member on it as well as by the cluster's own transport.
//!
//! - Replication: with another member holding the sender lock, the linked local node sends nothing; once the lock is
//!   free it takes it and sends the set whole; a change written on the other member reaches it on the cluster topic and
//!   goes up; when another member takes the lock it stops, and when it takes the lock back it sends the set whole under
//!   a new epoch, with what was written meanwhile.
//! - Web protection: a ban started on one member is applied on the other (the sender a live member of its cluster),
//!   `get#WebProtection` lists it with what banned it, `lift#WebBan` is refused without ADMIN_OPERATION_MANAGE, and an
//!   administrator's lift ends it on both members.

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

use crate::common::{exchange, http_request, Gate};

const NODE: &str = "local-pool";
const GROUP: &str = "moqui.security.UserGroup";
const LOCK: &str = "moqui.edge.EdgeSenderLock";

const EDGE_CONF: &str = r#"<edge>
    <queue-kind name="pool" service="org.moqui.edge.EdgeServices.receive#EdgeEcho"/>
    <replicate set="groups"><entity name="moqui.security.UserGroup" fields="description"/></replicate>
</edge>"#;

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

fn facade_of(server: &moqui_host::Server) -> EntityFacade {
    server.factory().entity().unwrap()
}

/// `body` in a context of `server`, with authorization off.
fn in_context<T>(server: &moqui_host::Server, body: impl FnOnce(&EntityFacade) -> T) -> T {
    let factory = server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    body(&facade_of(server))
}

/// A service on `server` as `user` (none: no one signed in), with authorization off: its result and the errors it
/// added.
fn call(
    server: &moqui_host::Server,
    user: Option<&str>,
    name: &str,
    parameters: &[(&str, Value)],
) -> (BTreeMap<String, Value>, Vec<String>) {
    let factory = server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    if let Some(user) = user {
        moqui_core::user::internal_login_user(user).unwrap();
    }
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

fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn write_group(server: &moqui_host::Server, id: &str, description: &str) {
    in_context(server, |facade| {
        let tx = facade.transactions_for(GROUP).unwrap();
        tx.run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
            let mut row = facade.make_value(GROUP)?;
            row.set("userGroupId", Value::string(id));
            row.set("description", Value::string(description));
            row.create_or_update(facade)
        })
        .unwrap();
    });
}

/// The edge node's copy of the local node's group `id`: its description, or `None`.
fn replica(edge: &Gate, id: &str) -> Option<String> {
    let pk = BTreeMap::from([("userGroupId".to_string(), id.to_string())]);
    in_context(&edge.server, |facade| {
        moqui_core::edge::replica::get_value(facade, Some(NODE), "groups", GROUP, &pk)
            .unwrap()
            .map(|value| value.fields["description"].as_str().unwrap_or_default().to_string())
    })
}

/// The edge node's cursor of the groups set: its epoch, empty before the first batch.
fn cursor_epoch(edge: &Gate) -> String {
    in_context(&edge.server, |facade| {
        moqui_core::edge::replica::cursors(facade, Some(NODE))
            .unwrap()
            .first()
            .map(|cursor| cursor.epoch.clone())
            .unwrap_or_default()
    })
}

/// Give the sender lock to `instance` for a minute, as another member's claim would.
fn hand_lock_to(local: &Gate, instance: &str) {
    in_context(&local.server, |facade| {
        let until = moqui_core::user::now_millis() + 60_000;
        let mut row = facade.make_value(LOCK).unwrap();
        row.set("lockName", Value::string("replicate"));
        row.set("instanceId", Value::string(instance));
        row.set("leaseExpireDate", moqui_core::user::stamp_value(until));
        row.create_or_update(facade).unwrap();
    });
}

fn lock_holder(local: &Gate) -> Option<String> {
    in_context(&local.server, |facade| {
        moqui_core::edge::replica::sender::holder(facade, "replicate")
            .unwrap()
            .map(|(id, _)| id)
    })
}

#[test]
fn only_the_lock_holder_sends_replication_and_a_new_holder_sends_whole() {
    if !crate::common::own_process() {
        return;
    }
    let (root, cert, key) = certs();
    let local = crate::common::start_with_conf_files("edge-pool-work-local", None, EDGE_CONF, &[], &[]);
    // quick claims, so a takeover is seen within the test
    local
        .server
        .factory()
        .extension(|_| moqui_core::edge::replica::sender::SenderTiming {
            poll: Duration::from_millis(150),
            renew: Duration::from_millis(200),
            lease: Duration::from_secs(2),
        });
    let (member, _member_runtime) = crate::common::start_member("edge-pool-work-member", &local.cluster_dir, &[]);
    let edge_port = crate::common::free_port();
    let edge_port_text = edge_port.to_string();
    let edge = crate::common::start_second_with(
        "edge-pool-work-edge",
        EDGE_CONF,
        &[("node_role", "edge"), ("webapp_https_listen_port", &edge_port_text)],
        &[("cert/default.crt", &cert), ("cert/default.key", &key)],
    );
    let local_id = moqui_core::cluster::instance_id(local.server.factory()).unwrap();
    let member_id = moqui_core::cluster::instance_id(member.factory()).unwrap();
    assert_ne!(local_id, member_id);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let url = format!("https://localhost:{edge_port}");
    let (code, errors) = call(
        &edge.server,
        Some("john.doe"),
        "org.moqui.edge.EdgeServices.create#PairingCode",
        &[],
    );
    assert!(errors.is_empty(), "{errors:?}");
    let mut pairing = PairConfig::new(&url, &to_plain_string(&code["code"]), NODE);
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

    // another member holds the lock: the linked node sends nothing
    hand_lock_to(&local, &member_id);
    let (_shutdown, shutdown) = watch::channel(false);
    let dialer = Dialer::start(config.clone(), rt.handle(), shutdown.clone(), None).unwrap();
    wait_until(Duration::from_secs(10), "link up", || {
        dialer.status().state == LinkState::Linked
    });
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(
        cursor_epoch(&edge),
        "",
        "nothing sent while another member is the sender"
    );
    assert_eq!(replica(&edge, "ADMIN"), None);

    // the lock is free: this member takes it and sends the set whole
    in_context(&local.server, |facade| {
        moqui_core::edge::replica::sender::release(facade, "replicate", &member_id).unwrap();
    });
    wait_until(Duration::from_secs(5), "the set whole from the new sender", || {
        replica(&edge, "ADMIN").is_some()
    });
    assert_eq!(lock_holder(&local).as_deref(), Some(local_id.as_str()));
    let first = cursor_epoch(&edge);

    // a change on the other member reaches the sender on the cluster topic, and goes up
    // as its own link's dialer would, with the pool's sets
    let sets = moqui_core::edge::settings(local.server.factory()).sets.clone();
    moqui_core::edge::replica::enable(&facade_of(&member), &sets).unwrap();
    write_group(&member, "EDGE_P_MEMBER", "From the other member");
    wait_until(Duration::from_secs(5), "the other member's change", || {
        replica(&edge, "EDGE_P_MEMBER").as_deref() == Some("From the other member")
    });
    assert_eq!(cursor_epoch(&edge), first, "an incremental batch");

    // another member takes the lock over: this one stops sending
    hand_lock_to(&local, &member_id);
    wait_until(Duration::from_secs(5), "this member no longer the sender", || {
        !moqui_core::edge::replica::sender::is_sender(&facade_of(&local.server))
    });
    write_group(&local.server, "EDGE_P_MEANWHILE", "Written meanwhile");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        replica(&edge, "EDGE_P_MEANWHILE"),
        None,
        "not sent by a member that is not the sender"
    );

    // and back: the set goes whole under a new epoch, with what was written meanwhile
    in_context(&local.server, |facade| {
        moqui_core::edge::replica::sender::release(facade, "replicate", &member_id).unwrap();
    });
    wait_until(Duration::from_secs(5), "the full send of a new sender", || {
        replica(&edge, "EDGE_P_MEANWHILE").as_deref() == Some("Written meanwhile")
    });
    assert_ne!(cursor_epoch(&edge), first, "a new epoch");

    // the sender's renewals stall (its lock row held by another transaction): it stops sending before its lease could
    // lapse, and once a claim comes back it is the sender again and sends whole
    let stalled = cursor_epoch(&edge);
    std::thread::scope(|scope| {
        let factory = std::sync::Arc::clone(local.server.factory());
        let holding = scope.spawn(move || {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            let facade = factory.entity().unwrap();
            let tx = facade.transactions_for(LOCK).unwrap();
            tx.run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
                facade
                    .find(LOCK)?
                    .eq("lockName", Value::string("replicate"))
                    .use_cache(false)
                    .for_update(true)
                    .one()?;
                std::thread::sleep(Duration::from_millis(2500));
                Ok(())
            })
            .unwrap();
        });
        wait_until(Duration::from_secs(5), "the stalled sender stops sending", || {
            !moqui_core::edge::replica::sender::is_sender(&facade_of(&local.server))
        });
        holding.join().unwrap();
    });
    wait_until(Duration::from_secs(5), "the sender again, with a full send", || {
        cursor_epoch(&edge) != stalled
    });
    dialer.stop();
    moqui_core::edge::replica::disable(&facade_of(&member));
    drop(member);
}

/// Two members of one database send one outbox at once, on Postgres's row locks (plans/Edge-Node2.md §4.4): each item
/// is claimed by one, once, and the items of each key go in order.
#[test]
fn two_members_send_one_outbox_each_item_once_in_its_keys_order() {
    if !crate::common::own_process() {
        return;
    }
    use moqui_core::edge::queue::{self, Applied, NewItem, Outcome, Step};
    // the pairing every member's shared .env names, which keys the pool's queue rows
    let pairing = [(moqui_core::edge::NODE_PAIRING, NODE)];
    let local = crate::common::start_with_conf_files("edge-pool-outbox", None, EDGE_CONF, &pairing, &[]);
    let (member, _member_runtime) =
        crate::common::start_member("edge-pool-outbox-member", &local.cluster_dir, &pairing);
    let node = queue::local_node_id(local.server.factory());
    assert_eq!(node, queue::local_node_id(member.factory()), "one id for the pool");
    assert_eq!(node, NODE);
    in_context(&local.server, |facade| {
        for index in 0..8 {
            for key in ["a", "b", "c"] {
                let item = NewItem {
                    kind: "pool".to_string(),
                    conversation_key: key.to_string(),
                    idempotency_key: format!("{key}{index}"),
                    payload: Some("{}".to_string()),
                    content: None,
                    content_location: None,
                };
                queue::enqueue(local.server.factory(), facade, None, &item).unwrap();
            }
        }
    });
    let settings = moqui_core::edge::settings(local.server.factory());
    let delivered = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    std::thread::scope(|scope| {
        for (factory, owner) in [
            (std::sync::Arc::clone(local.server.factory()), "m-1/1"),
            (std::sync::Arc::clone(member.factory()), "m-2/1"),
        ] {
            let (node, settings, delivered) = (&node, &settings, &delivered);
            scope.spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                let _authz = context.disable_authz();
                let facade = &factory.entity().unwrap();
                {
                    let started = Instant::now();
                    while started.elapsed() < Duration::from_secs(10) {
                        let now = moqui_core::user::now_millis();
                        let heads = queue::due_heads(facade, node, queue::TO_EDGE, now).unwrap();
                        if heads.due.is_empty() && heads.next_due.is_none() {
                            return;
                        }
                        for head in heads.due {
                            let Some(item) = queue::claim(facade, &head, owner, now).unwrap() else {
                                continue;
                            };
                            delivered.lock().unwrap().push(item.idempotency_key.clone());
                            let applied = Outcome::Answered(Applied::Applied);
                            let step = queue::finish(facade, settings, &item, &applied, now).unwrap();
                            assert_eq!(step, Step::Delivered);
                        }
                    }
                }
            });
        }
    });
    let delivered = delivered.lock().unwrap().clone();
    assert_eq!(delivered.len(), 24, "each once: {delivered:?}");
    for key in ["a", "b", "c"] {
        let order: Vec<&String> = delivered.iter().filter(|id| id.starts_with(key)).collect();
        let expected: Vec<String> = (0..8).map(|index| format!("{key}{index}")).collect();
        assert_eq!(order, expected.iter().collect::<Vec<_>>(), "key {key} in order");
    }
    drop(member);
}

/// The ban rule of the first member: one denied request bans for ten minutes.
const BAN_CONF: &str = r#"<webapp-list><webapp name="webroot">
    <web-protection>
        <ban name="scan" events="denied" threshold="1" seconds="60" duration="600"/>
    </web-protection>
</webapp></webapp-list>"#;

#[test]
fn a_ban_on_one_member_holds_on_another_and_the_view_lifts_it() {
    if !crate::common::own_process() {
        return;
    }
    let properties = [
        ("webapp_client_ip_header", "X-Forwarded-For"),
        ("webapp_trusted_proxies", "127.0.0.1"),
    ];
    let first = crate::common::start_with_conf_files("edge-pool-ban", None, BAN_CONF, &properties, &[]);
    let (second, _second_runtime) =
        crate::common::start_member("edge-pool-ban-member", &first.cluster_dir, &properties);
    let get = |server: &moqui_host::Server, path: &str| {
        exchange(
            server.address(),
            &http_request("GET", path, &[("X-Forwarded-For", "203.0.113.50")], ""),
        )
        .0
    };
    // a scanner's path, denied, bans the client on the first member
    assert_eq!(get(&first.server, "/.env"), 404);
    assert_eq!(get(&first.server, "/Login"), 403);
    // and the second member, told on the cluster topic, refuses it too
    wait_until(Duration::from_secs(5), "the ban on the other member", || {
        get(&second, "/Login") == 403
    });
    let first_id = moqui_core::cluster::instance_id(first.server.factory()).unwrap();

    // the view lists it, with what banned it
    let service = "org.moqui.admin.OperationServices";
    let (out, errors) = call(&second, Some("john.doe"), &format!("{service}.get#WebProtection"), &[]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(out["started"], Value::Bool(true));
    let Some(Value::List(bans)) = out.get("banList") else {
        panic!("banList: {out:?}")
    };
    let ban = bans
        .iter()
        .find_map(|ban| match ban {
            Value::Map(ban) if to_plain_string(&ban["address"]) == "203.0.113.50" => Some(ban.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the ban: {bans:?}"));
    assert_eq!(to_plain_string(&ban["source"]), format!("member {first_id}: scan"));
    let (out, _) = call(
        &first.server,
        Some("john.doe"),
        &format!("{service}.get#WebProtection"),
        &[],
    );
    let Some(Value::List(rules)) = out.get("ruleList") else {
        panic!("ruleList: {out:?}")
    };
    assert!(rules.iter().any(|rule| matches!(rule, Value::Map(rule)
        if to_plain_string(&rule["name"]) == "scan" && to_plain_string(&rule["count"]) == "1")));

    // lifting needs ADMIN_OPERATION_MANAGE
    let lift = |user: Option<&str>| {
        call(
            &second,
            user,
            &format!("{service}.lift#WebBan"),
            &[("address", Value::string("203.0.113.50"))],
        )
    };
    let (_, errors) = lift(None);
    assert!(
        errors.iter().any(|error| error.contains("ADMIN_OPERATION_MANAGE")),
        "{errors:?}"
    );
    assert_eq!(get(&second, "/Login"), 403);
    let (out, errors) = lift(Some("john.doe"));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(out["lifted"], Value::Bool(true));
    assert_ne!(get(&second, "/Login"), 403);
    // the first member is told, and lifts its own
    wait_until(Duration::from_secs(5), "the lift on the first member", || {
        get(&first.server, "/Login") != 403
    });
    drop(second);
}
