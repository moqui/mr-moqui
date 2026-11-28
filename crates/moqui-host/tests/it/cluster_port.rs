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

//! The cluster port on `cluster-member@listen` (plans/Runtime-Config.md §3.5, RC6): under `auto` a member alone opens
//! none, a member that starts into a cluster opens it before it reports ready, and one alone notices a second member
//! within `lone-check-seconds` and opens it then; the joiner's posts to a member whose port is still closed fail and
//! back off, and its first that arrive come after a clear of every entity cache. A busy cluster port, at the start or
//! when it opens later, is a Failed row and the server goes on (§6). The other tests' servers run it `on`
//! (`common::TEST_CLUSTER_LISTEN`).

#![cfg(all(unix, feature = "axum", feature = "embedded-postgres"))]

use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::driver::DbConnection;
use moqui_core::listeners::{Cause, ReconcileRequest};
use moqui_core::notify::{TopicMessage, ENTITY_CACHE_INVALIDATE};
use moqui_core::startup::{Section, State};
use moqui_host::db::postgres::{PostgresConn, PostgresDriver};
use moqui_lib::object::Value;

/// The cluster port's row.
fn row(factory: &ExecutionContextFactory) -> moqui_core::startup::Entry {
    moqui_core::startup::report(factory)
        .entries()
        .into_iter()
        .find(|row| row.section == Section::Listener && row.name == "Cluster")
        .expect("the cluster port's row")
}

fn wait_until(limit: Duration, what: &str, done: impl Fn() -> bool) {
    let until = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < until, "{what} within {limit:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A pass of `factory`'s listeners now, as the registry's retry of a failed listener runs one every 30 s.
fn reconcile(factory: &ExecutionContextFactory) {
    moqui_core::listeners::control(factory)
        .expect("the server's listener control")
        .reconcile(ReconcileRequest::new(Cause::Signal("test")), Duration::from_secs(10))
        .expect("the pass");
}

/// An `Enumeration` row written on `factory`: peers cache the entity, so its invalidation is posted to them.
fn write(factory: &Arc<ExecutionContextFactory>, id: &str) {
    let facade = factory.entity().unwrap();
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut value = facade.make_value("moqui.basic.Enumeration").unwrap();
    value.set("enumId", Value::string(id));
    value.set("description", Value::string("written by the joiner"));
    value.create(&facade).unwrap();
}

/// A port held by the test on every address, so a server cannot bind it; a connection to it is accepted and closed at
/// once, so a member's post there fails its TLS handshake at once, and each one is counted.
struct Held {
    accepted: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Held {
    fn on(port: u16) -> Self {
        let listener = TcpListener::bind(("0.0.0.0", port)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let (accepted, stop) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let thread = {
            let (accepted, stop) = (Arc::clone(&accepted), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            drop(stream);
                            accepted.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            })
        };
        Self {
            accepted,
            stop,
            thread: Some(thread),
        }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

/// Let the port go: the thread ends and drops the listener.
impl Drop for Held {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Each entity cache invalidation published in this process, in order, by the members' writes and their peers'
/// applies alike: whether it clears every cache, and the `enumId`s it names.
#[derive(Default)]
struct Seen(Mutex<Vec<(bool, Vec<String>)>>);

impl Seen {
    fn note(&self, message: &TopicMessage) {
        let body = &message.body;
        let clear_all = body.get("clearAll").and_then(serde_json::Value::as_bool) == Some(true);
        let keys = body
            .get("primaryKeys")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_else(|| body.get("primaryKey").into_iter().cloned().collect());
        let ids = keys
            .iter()
            .filter_map(|key| key.get("enumId").and_then(serde_json::Value::as_str))
            .map(ToString::to_string)
            .collect();
        self.0.lock().unwrap().push((clear_all, ids));
    }

    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    /// Whether a clear of every cache was published from `from` on.
    fn cleared_since(&self, from: usize) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .skip(from)
            .any(|(clear_all, _)| *clear_all)
    }

    /// From `from` on: where the first message naming an id that starts with `prefix` a second time is (a peer's
    /// apply of a write already published where it was made), and whether a clear of every cache came before it.
    fn cleared_before_applied(&self, from: usize, prefix: &str) -> (Option<usize>, bool) {
        let seen = self.0.lock().unwrap();
        let mut named: Vec<&str> = Vec::new();
        let mut cleared = false;
        for (at, (clear_all, ids)) in seen.iter().enumerate() {
            let ours: Vec<&str> = ids
                .iter()
                .map(String::as_str)
                .filter(|id| id.starts_with(prefix))
                .collect();
            if at >= from && ours.iter().any(|id| named.contains(id)) {
                return (Some(at), cleared);
            }
            named.extend(ours);
            cleared |= at >= from && *clear_all;
        }
        (None, cleared)
    }
}

/// A lone member under `auto` has no cluster port; a second member starts into the cluster with its port open, and
/// the first notices it within its `lone-check-seconds` (1 here) and opens its own. The first's port is held at that
/// moment, so its pass fails as a busy port does (a Failed row, and the server goes on), and the joiner's posts to it
/// fail and back off; once the port is let go and opens, the joiner sends its clear of every entity cache when its
/// backoff ends, with no other write to carry it, and its next write arrives after it. With the joiner gone and its row
/// deleted, as the heartbeats delete a dead member's, the first member's port closes again.
#[test]
fn a_lone_member_opens_its_cluster_port_when_a_second_member_starts() {
    if !crate::common::own_process() {
        return;
    }
    let port = crate::common::free_port();
    let port_text = port.to_string();
    let first = crate::common::start_with_properties(
        "cluster-auto",
        None,
        &[
            ("moqui_conf__cluster_member__listen", "auto"),
            ("moqui_conf__cluster_member__lone_check_seconds", "1"),
            ("moqui_conf__cluster_member__https_port", &port_text),
        ],
    );
    let factory = Arc::clone(first.server.factory());
    assert_eq!(
        first.server.cluster_address(),
        None,
        "a member alone opens no cluster port"
    );
    assert_eq!(row(&factory).state, State::off("no other member"));

    let seen = Arc::new(Seen::default());
    let _subscription = {
        let seen = Arc::clone(&seen);
        moqui_core::notify::subscribe(
            ENTITY_CACHE_INVALIDATE,
            Arc::new(move |message: &TopicMessage| seen.note(message)),
        )
        .unwrap()
    };
    let held = Held::on(port);
    let (second, second_runtime) = crate::common::start_member(
        "cluster-auto-2",
        &first.cluster_dir,
        &[("moqui_conf__cluster_member__listen", "auto")],
    );
    let joiner = Arc::clone(second.factory());
    // a short backoff, so the test does not wait out five seconds
    moqui_core::cluster::set_peer_backoff(&joiner, Duration::from_millis(300));
    // it started into a cluster: its port was open before it reported ready
    let joiner_port = second.cluster_address().expect("the joiner's cluster port");
    assert_ne!(joiner_port.port(), port);
    assert!(matches!(row(&joiner).state, State::On(_)), "{:?}", row(&joiner).state);
    // its first post finds the first member's port closed (held here): it fails every try and backs off
    write(&joiner, "JOINER-1");
    wait_until(Duration::from_secs(5), "the joiner's three tries", || {
        held.accepted() >= 3
    });
    // the first member notices the second within its lone check and asks for a pass, which finds the port busy
    wait_until(Duration::from_secs(5), "the first member's pass", || {
        matches!(row(&factory).state, State::Failed(_))
    });
    let failed = row(&factory);
    assert!(failed.state.detail().contains(&port_text), "{:?}", failed.state);
    let cause = failed.cause.unwrap_or_default();
    assert!(cause.contains("signal cluster member"), "{cause}");
    assert_eq!(first.server.cluster_address(), None);
    // the first sends the joiner its clear of every cache at once: the joiner's port is open
    wait_until(Duration::from_secs(5), "the first member's clear at the joiner", || {
        !second.cluster_posts().is_empty()
    });

    // the port let go, the next pass opens it (the registry asks for one every 30 s while a listener is failed)
    let from = seen.len();
    drop(held);
    reconcile(&factory);
    assert_eq!(first.server.cluster_address().map(|address| address.port()), Some(port));
    assert!(matches!(row(&factory).state, State::On(_)), "{:?}", row(&factory).state);
    // the joiner owes it a clear: it sends it at its next short check once its backoff is over, with no write to carry it
    wait_until(Duration::from_secs(5), "the joiner's clear at the first member", || {
        seen.cleared_since(from)
    });
    // and its next write arrives after it
    let mut written = 1;
    let until = Instant::now() + Duration::from_secs(8);
    while seen.cleared_before_applied(from, "JOINER-").0.is_none() {
        assert!(
            Instant::now() < until,
            "nothing the joiner wrote reached the first member"
        );
        written += 1;
        write(&joiner, &format!("JOINER-{written}"));
        let applied = Instant::now() + Duration::from_secs(1);
        while seen.cleared_before_applied(from, "JOINER-").0.is_none() && Instant::now() < applied {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let (applied, cleared) = seen.cleared_before_applied(from, "JOINER-");
    assert!(
        cleared,
        "the joiner's write at {applied:?} came before its clear of every cache: {:?}",
        seen.0.lock().unwrap()
    );

    // the joiner gone and its row deleted, as the heartbeats delete a dead member's: no other member is left, and the
    // first member's port closes at its next read of the member table
    let joiner_id = moqui_core::cluster::instance_id(&joiner).unwrap();
    drop(joiner);
    drop(second);
    drop(second_runtime);
    raw_connection(&first)
        .execute(
            "DELETE FROM cluster_member WHERE instance_id = ?",
            &[Value::string(&joiner_id)],
        )
        .unwrap();
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::cluster::heartbeat(&factory);
    }
    wait_until(Duration::from_secs(5), "the first member's port closed", || {
        first.server.cluster_address().is_none()
    });
    assert_eq!(row(&factory).state, State::off("no other member"));
}

/// With `listen="on"` a cluster port held by another at the start is a Failed row naming it, and the server starts
/// without it (plans/Runtime-Config.md §6); once the port is let go, a pass opens it. `listen` set to `off` while the
/// server runs closes it, and `on` opens it again.
#[test]
fn a_busy_cluster_port_at_the_start_is_a_failed_row_and_the_server_starts() {
    if !crate::common::own_process() {
        return;
    }
    let port = crate::common::free_port();
    let port_text = port.to_string();
    let held = TcpListener::bind(("0.0.0.0", port)).unwrap();
    let gate = crate::common::start_with_properties(
        "cluster-busy",
        None,
        &[
            ("moqui_conf__cluster_member__listen", "on"),
            ("moqui_conf__cluster_member__https_port", &port_text),
        ],
    );
    let factory = Arc::clone(gate.server.factory());
    let failed = row(&factory);
    assert!(matches!(failed.state, State::Failed(_)), "{:?}", failed.state);
    assert!(failed.state.detail().contains(&port_text), "{:?}", failed.state);
    assert_eq!(gate.server.cluster_address(), None);
    // the server serves without it
    let http = gate.server.address();
    assert!(std::net::TcpStream::connect(http).is_ok(), "HTTP on {http}");
    drop(held);
    reconcile(&factory);
    let open = Some(SocketAddr::from(([0, 0, 0, 0], port)));
    assert_eq!(gate.server.cluster_address(), open);
    // switched off and on again while the server runs (its `listeners` hook asks for each pass)
    factory
        .set_conf_value("cluster-member@listen", Some("off"), false)
        .unwrap();
    wait_until(Duration::from_secs(5), "the port closed", || {
        gate.server.cluster_address().is_none()
    });
    assert_eq!(row(&factory).state, State::off("cluster-member@listen is off"));
    factory
        .set_conf_value("cluster-member@listen", Some("on"), false)
        .unwrap();
    wait_until(Duration::from_secs(10), "the port open again", || {
        gate.server.cluster_address() == open
    });
}

/// A connection of the test's own to the gate's embedded database, as the shared database is reached by any member.
fn raw_connection(gate: &crate::common::Gate) -> PostgresConn {
    let data = std::fs::canonicalize(gate.cluster_dir.join("data")).unwrap();
    let password = std::fs::read_to_string(gate.cluster_dir.join("pgpass")).unwrap();
    PostgresDriver::unix_socket("postgres", "moqui", data)
        .password(password.trim())
        .connect_postgres()
        .unwrap()
}
