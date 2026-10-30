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

//! The Postgres NOTIFY cluster channel (`cluster_transport=notify`). Members in one process share an origin id and would
//! skip each other's messages, so the peer is played over raw connections to the same database: its NOTIFYs arrive and
//! are applied, and this member's fan-out is read by a LISTEN of the test's own.

#![cfg(all(unix, feature = "axum", feature = "embedded-postgres"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::driver::DbConnection;
use moqui_host::db::postgres::{PostgresConn, PostgresDriver};
use moqui_lib::object::{to_plain_string, Value};

const CHANNEL: &str = "moqui_cluster";

#[test]
fn a_notify_member_applies_peer_notifies_and_sends_its_own() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_properties("notify", None, &[("cluster_transport", "notify")]);
    let factory = Arc::clone(gate.server.factory());
    let facade = factory.entity().unwrap();

    let members = facade
        .find("moqui.server.ClusterMember")
        .unwrap()
        .disable_authz()
        .list()
        .unwrap();
    let row = members.values().first().expect("the member row");
    assert_eq!(
        row.get("clusterBaseUrl").map(to_plain_string).as_deref(),
        Some("pg-notify:")
    );

    let mut peer = raw_connection(&gate);
    let invalidate =
        r#"{"origin_id": "peer-1", "message_id": "m-1", "body": {"entityName": "moqui.basic.Enumeration"}}"#;

    // A peer's NOTIFY is applied as a POST would be.
    let before = gate.server.cluster_posts().len();
    notify(&mut peer, &format!("entity-cache-invalidate\npeer-1\n{invalidate}"));
    wait_for_posts(&gate, before + 1);

    // This member's own origin is skipped.
    let own = moqui_core::notify::origin_id();
    notify(&mut peer, &format!("entity-cache-invalidate\n{own}\n{invalidate}"));
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(gate.server.cluster_posts().len(), before + 1);

    // A message past the payload limit is a ClusterMessage row, and the NOTIFY carries its id.
    let padding = "x".repeat(2 * 1024 * 1024);
    let big = format!(
        r#"{{"origin_id": "peer-2", "message_id": "m-2", "body": {{"entityName": "moqui.basic.Enumeration", "primaryKey": {{"pad": "{padding}"}}}}}}"#
    );
    let id = moqui_core::cluster::store_message(&facade, "entity-cache-invalidate", &big).unwrap();
    notify(&mut peer, &format!("entity-cache-invalidate\npeer-2\n@{id}"));
    wait_for_posts(&gate, before + 2);

    // With a live peer that listens on NOTIFY, a write here goes out on the channel.
    peer.execute(
        "INSERT INTO cluster_member (instance_id, cluster_base_url, heartbeat) VALUES ('peer-x', 'pg-notify:', now())",
        &[],
    )
    .unwrap();
    let mut listener = raw_connection(&gate);
    listener.listen(CHANNEL).unwrap();
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let mut value = facade.make_value("moqui.basic.Enumeration").unwrap();
        value.set("enumId", Value::string("NOTIFY_TEST"));
        value.set("description", Value::string("sent on the channel"));
        value.create(&facade).unwrap();
    }
    let sent = received(&mut listener, own);
    assert!(sent.starts_with("entity-cache-invalidate\n"), "{sent}");
    assert!(sent.contains("moqui.basic.Enumeration"), "{sent}");

    drop(listener);
    drop(peer);
    drop(gate);
}

#[test]
fn a_notify_that_arrives_while_the_request_queue_is_full_is_applied_once_there_is_room() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_properties("notify-busy", None, &[("cluster_transport", "notify")]);
    let factory = Arc::clone(gate.server.factory());

    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let record = Arc::clone(&seen);
    let _subscription = moqui_core::notify::subscribe(
        moqui_core::notify::ENTITY_CACHE_INVALIDATE,
        Arc::new(move |message: &moqui_core::notify::TopicMessage| {
            record.lock().unwrap().push(message.message_id.clone());
        }),
    )
    .unwrap();
    let applied = || seen.lock().unwrap().iter().any(|id| id == "m-busy");

    // One worker held by a job that waits, and a queue of one that is full.
    let pool = factory.request_pool();
    pool.start(1, 1).unwrap();
    let (release, held) = std::sync::mpsc::channel::<()>();
    let blocker = pool
        .try_submit(Arc::clone(&factory), move || {
            held.recv().ok();
            Ok(())
        })
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while pool.queue_depth() > 0 {
        assert!(Instant::now() < until, "the worker did not take the blocking job");
        std::thread::sleep(Duration::from_millis(10));
    }
    let filler = pool.try_submit(Arc::clone(&factory), || Ok(())).unwrap();
    assert!(
        pool.try_submit(Arc::clone(&factory), || Ok(())).is_err(),
        "the queue is full"
    );

    let mut peer = raw_connection(&gate);
    let invalidate =
        r#"{"origin_id": "peer-busy", "message_id": "m-busy", "body": {"entityName": "moqui.basic.Enumeration"}}"#;
    notify(&mut peer, &format!("entity-cache-invalidate\npeer-busy\n{invalidate}"));
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!applied(), "applied while every worker was busy");

    release.send(()).unwrap();
    blocker.recv().unwrap().unwrap();
    filler.recv().unwrap().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !applied() {
        assert!(
            Instant::now() < until,
            "the invalidation was dropped while the queue was full"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    drop(peer);
    drop(gate);
}

fn raw_connection(gate: &crate::common::Gate) -> PostgresConn {
    let data = std::fs::canonicalize(gate.cluster_dir.join("data")).unwrap();
    let password = std::fs::read_to_string(gate.cluster_dir.join("pgpass")).unwrap();
    PostgresDriver::unix_socket("postgres", "moqui", data)
        .password(password.trim())
        .connect_postgres()
        .unwrap()
}

fn notify(conn: &mut PostgresConn, payload: &str) {
    conn.query_once(
        "SELECT pg_notify(?, ?)",
        &[Value::string(CHANNEL), Value::string(payload)],
    )
    .unwrap();
}

fn wait_for_posts(gate: &crate::common::Gate, count: usize) {
    let until = Instant::now() + Duration::from_secs(10);
    while gate.server.cluster_posts().len() < count {
        assert!(Instant::now() < until, "posts {:?}", gate.server.cluster_posts());
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The first payload on `conn` that `origin` sent.
fn received(conn: &mut PostgresConn, origin: &str) -> String {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        for payload in conn.notifications(Duration::from_millis(500)).unwrap() {
            if payload.split('\n').nth(1) == Some(origin) {
                return payload;
            }
        }
    }
    panic!("no NOTIFY from {origin}");
}
