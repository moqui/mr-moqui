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

//! The network bounds of a session (plans/High-Level-Review.md HL-019) against a real MySQL or MariaDB server, behind a
//! relay that stops forwarding part way through a statement, as a host that stops answering without a reset does.
//! Acceptance tests: they need `MOQUI_TEST_MYSQL=user:pass@host:port/database` and take about 15 seconds.
//!
//! ```sh
//! MOQUI_TEST_MYSQL=root:moqui@127.0.0.1:33306/moqtest \
//!     cargo test -p moqui-drivers --features mysql --lib -- --ignored mysql::driver::network_tests
//! ```
//!
//! Not checked here: TCP keepalive ending a wait on a host that stopped answering. The relay is a live TCP peer that
//! answers keepalive probes, so that needs packets dropped below TCP (a firewall rule), with at least 90 s of quiet.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::transaction::{PoolConfig, TransactionAttribute, TransactionManager};

use super::*;

struct Target {
    username: String,
    password: String,
    host: String,
    port: u16,
    database: String,
}

fn target() -> Target {
    let spec =
        std::env::var("MOQUI_TEST_MYSQL").expect("MOQUI_TEST_MYSQL=user:pass@host:port/database names the server");
    let (userinfo, rest) = spec.trim().rsplit_once('@').expect("user:pass@host:port/database");
    let (username, password) = userinfo.split_once(':').expect("user:pass");
    let (hostport, database) = rest.split_once('/').expect("host:port/database");
    let (host, port) = hostport.rsplit_once(':').expect("host:port");
    Target {
        username: username.to_string(),
        password: password.to_string(),
        host: host.to_string(),
        port: port.parse().expect("a port number"),
        database: database.to_string(),
    }
}

/// A TCP relay to the server. [`Relay::stall_open`] stops every open connection forwarding in both directions; with
/// [`Relay::stall_new`] a new connection is accepted and never answered.
struct Relay {
    port: u16,
    stall_new: Arc<AtomicBool>,
    open: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
}

impl Relay {
    fn start(host: &str, port: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let relay = Self {
            port: listener.local_addr().unwrap().port(),
            stall_new: Arc::new(AtomicBool::new(false)),
            open: Arc::new(Mutex::new(Vec::new())),
        };
        let (stall_new, open) = (Arc::clone(&relay.stall_new), Arc::clone(&relay.open));
        let upstream = format!("{host}:{port}");
        std::thread::spawn(move || {
            let mut silent = Vec::new();
            for client in listener.incoming() {
                let Ok(client) = client else { return };
                if stall_new.load(Ordering::Acquire) {
                    silent.push(client);
                    continue;
                }
                let Ok(server) = TcpStream::connect(&upstream) else {
                    continue;
                };
                let stalled = Arc::new(AtomicBool::new(false));
                open.lock().unwrap().push(Arc::clone(&stalled));
                pump(
                    client.try_clone().unwrap(),
                    server.try_clone().unwrap(),
                    Arc::clone(&stalled),
                );
                pump(server, client, stalled);
            }
        });
        relay
    }

    fn stall_open(&self) {
        for stalled in self.open.lock().unwrap().iter() {
            stalled.store(true, Ordering::Release);
        }
    }

    fn stall_new(&self, on: bool) {
        self.stall_new.store(on, Ordering::Release);
    }
}

/// Copy `from` to `to` until either ends; once `stalled`, nothing more is read or written and neither end is closed.
fn pump(mut from: TcpStream, mut to: TcpStream, stalled: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        from.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let mut buffer = [0u8; 16 * 1024];
        loop {
            if stalled.load(Ordering::Acquire) {
                // keep both sockets open, so the client sees no reset: only silence
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            match from.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if stalled.load(Ordering::Acquire) {
                        continue;
                    }
                    if to.write_all(&buffer[..count]).is_err() {
                        break;
                    }
                }
                Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => break,
            }
        }
        let _ = to.shutdown(Shutdown::Both);
    });
}

fn manager(driver: MysqlDriver, max: u32) -> TransactionManager {
    TransactionManager::new(
        Arc::new(driver),
        PoolConfig {
            min: 0,
            max,
            reserve: 0,
            acquire_timeout: Duration::from_secs(10),
            ..Default::default()
        },
    )
    .unwrap()
}

fn select_one(manager: &TransactionManager) -> Result<(), Error> {
    manager.run(TransactionAttribute::ForceNew, None, || {
        manager.with_connection(|conn| conn.query("SELECT 1", &[]).map(|_| ()))
    })
}

/// A statement whose answer never comes, in a transaction with a deadline: the cancel's `KILL QUERY` reaches the
/// server on a connection of its own, the worker is free once the datasource's `read-timeout` passes, and the pool
/// opens a new connection after, as the broken one is not given out again.
#[test]
#[ignore = "acceptance: needs MOQUI_TEST_MYSQL; see the module docs"]
fn a_stalled_statement_ends_within_the_read_timeout_and_the_pool_recovers() {
    let target = target();
    let relay = Relay::start(&target.host, target.port);
    let read_timeout = Duration::from_secs(2);
    let driver = MysqlDriver::new(
        "127.0.0.1",
        relay.port,
        &target.username,
        &target.password,
        &target.database,
        false,
    )
    .read_timeout(read_timeout);
    let manager = manager(driver, 1);
    select_one(&manager).expect("a statement through the relay");

    let started = Instant::now();
    let outcome = manager.run(TransactionAttribute::ForceNew, Some(Duration::from_secs(1)), || {
        manager.with_connection(|conn| {
            relay.stall_open();
            conn.execute("DO SLEEP(30)", &[])
        })
    });
    let elapsed = started.elapsed();
    assert!(outcome.is_err(), "{outcome:?}");
    assert!(
        elapsed >= read_timeout - Duration::from_millis(200) && elapsed < read_timeout + Duration::from_secs(2),
        "the worker waited {elapsed:?}, not the read timeout of {read_timeout:?}: {outcome:?}"
    );
    // the one connection of the pool is the broken one: a statement after it works on a new connection
    let started = Instant::now();
    select_one(&manager).expect("the pool opens a new connection after a broken one");
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
}

/// The host stops answering everything, the cancel's new connection too: the worker is still free within the read
/// limit, as the cancel runs off the deadline timer and each of its steps is bounded, and the pool recovers once the
/// host answers again.
#[test]
#[ignore = "acceptance: needs MOQUI_TEST_MYSQL; see the module docs"]
fn a_cancel_to_a_stalled_host_does_not_hold_the_worker() {
    let target = target();
    let relay = Relay::start(&target.host, target.port);
    let read_timeout = Duration::from_secs(3);
    let driver = MysqlDriver::new(
        "127.0.0.1",
        relay.port,
        &target.username,
        &target.password,
        &target.database,
        false,
    )
    .read_timeout(read_timeout);
    let manager = manager(driver, 2);
    select_one(&manager).expect("a statement through the relay");

    let started = Instant::now();
    let outcome = manager.run(TransactionAttribute::ForceNew, Some(Duration::from_secs(1)), || {
        manager.with_connection(|conn| {
            relay.stall_new(true);
            relay.stall_open();
            conn.execute("DO SLEEP(30)", &[])
        })
    });
    let elapsed = started.elapsed();
    assert!(outcome.is_err(), "{outcome:?}");
    assert!(
        elapsed < read_timeout + Duration::from_secs(2),
        "the worker waited {elapsed:?}: {outcome:?}"
    );

    // the cancel on its own: each step bounded, to a host that accepts and never answers
    let cancel = MysqlCancel {
        state: Arc::new(KillState {
            endpoint: Endpoint {
                host: "127.0.0.1".to_string(),
                port: relay.port,
                username: target.username.clone(),
                password: target.password.clone(),
                database: target.database.clone(),
                tls: false,
                read_timeout: Some(read_timeout),
                isolation: None,
            },
            connection_id: AtomicU32::new(1),
            busy: AtomicBool::new(true),
        }),
    };
    let started = Instant::now();
    assert!(cancel.cancel().is_err());
    let elapsed = started.elapsed();
    assert!(
        elapsed >= CANCEL_TIMEOUT - Duration::from_millis(200) && elapsed < CANCEL_TIMEOUT + Duration::from_secs(2),
        "the cancel took {elapsed:?}, not its {CANCEL_TIMEOUT:?}"
    );

    relay.stall_new(false);
    select_one(&manager).expect("the pool opens a new connection once the host answers");
}

/// The deadline's `KILL QUERY` ends that statement only: one on another connection runs to its end, and the next
/// statement on the killed statement's connection is not ended by a late cancel.
#[test]
#[ignore = "acceptance: needs MOQUI_TEST_MYSQL; see the module docs"]
fn the_deadline_kills_its_own_statement_only() {
    let target = target();
    let driver = || {
        MysqlDriver::new(
            target.host.as_str(),
            target.port,
            target.username.as_str(),
            target.password.as_str(),
            target.database.as_str(),
            false,
        )
    };
    let manager = Arc::new(manager(driver(), 1));
    let other = std::thread::spawn({
        let mut conn = driver().connect_mysql().unwrap();
        move || {
            let started = Instant::now();
            let rows = conn.query("SELECT SLEEP(3) AS slept", &[]).unwrap();
            (started.elapsed(), rows[0].get("slept").cloned())
        }
    });

    let started = Instant::now();
    let outcome = manager.run(TransactionAttribute::ForceNew, Some(Duration::from_secs(1)), || {
        manager.with_connection(|conn| conn.execute("DO SLEEP(6)", &[]))
    });
    assert!(outcome.is_err(), "{outcome:?}");
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());

    // SLEEP gives 0 when it ran to its end and 1 when it was killed
    let (elapsed, slept) = other.join().unwrap();
    assert!(
        elapsed >= Duration::from_secs(3),
        "the other connection's statement was ended at {elapsed:?}"
    );
    assert!(matches!(slept, Some(Value::I64(0) | Value::I32(0))), "{slept:?}");

    // the same pooled connection, with no deadline, runs its statement to the end
    let started = Instant::now();
    let slept = manager
        .run(TransactionAttribute::ForceNew, None, || {
            manager.with_connection(|conn| conn.query("SELECT SLEEP(2) AS slept", &[]))
        })
        .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(2), "{:?}", started.elapsed());
    assert!(
        matches!(slept[0].get("slept"), Some(Value::I64(0) | Value::I32(0))),
        "{:?}",
        slept[0].get("slept")
    );
}
