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

use std::path::{Path, PathBuf};

use moqui_core::driver::{DbConnection, DbDriver};
use moqui_lib::object::{basic_convert, Value};

use super::super::super::postgres::PostgresConn;
use super::*;

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_archive_unpacks_once_without_its_top_directory() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let install_dir = dir.join("install").join(SHIPPED_VERSION);
    install(&install_dir).unwrap();
    let postgres = install_dir.join("bin/postgres");
    assert!(postgres.is_file(), "{} is unpacked", postgres.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            postgres.metadata().unwrap().permissions().mode() & 0o111,
            0,
            "postgres is executable"
        );
    }
    assert!(!install_dir.join("bin/psql").exists(), "psql is not shipped");
    install(&install_dir).unwrap();
    let left: Vec<_> = std::fs::read_dir(dir.join("install"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(
        left,
        vec![std::ffi::OsString::from(SHIPPED_VERSION)],
        "no unpack directory is left"
    );
}

#[test]
fn the_shipped_version_is_of_the_shipped_major() {
    assert!(SHIPPED_VERSION.starts_with(&format!("{SHIPPED_MAJOR}.")));
}

#[test]
fn embedded_cluster_queries_stream_cache_and_drops() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    // theseus 18.6.0 NEEDED libxml2.so.2. A host that only ships .so.16 runs initdb through the shim in the
    // install's lib, and this process's environment is left alone.
    let library_path = std::env::var_os("LD_LIBRARY_PATH");
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    assert_eq!(std::env::var_os("LD_LIBRARY_PATH"), library_path);
    if crate::xmlshim::needed().is_some() {
        let shim = dir.join("install").join(SHIPPED_VERSION).join("lib/libxml2.so.2");
        assert!(shim.is_file(), "{} missing", shim.display());
        assert!(!dir.join("unpack").exists());
    }
    let data_dir = cluster.data_dir().to_path_buf();
    assert!(
        data_dir.join(".s.PGSQL.5432").exists(),
        "socket missing in {}",
        data_dir.display()
    );

    let mut conn = driver.connect_postgres().unwrap();
    assert_eq!(show(&mut conn, "fsync"), "on");
    assert_eq!(show(&mut conn, "logging_collector"), "off");
    assert_eq!(show(&mut conn, "max_connections"), "20");
    assert_eq!(show(&mut conn, "listen_addresses"), "");
    let buffers = show(&mut conn, "shared_buffers");
    assert!(buffers == "16MB" || buffers == "16384kB", "shared_buffers={buffers}");

    let selected = conn.query_once("SELECT 1", &[]).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].columns[0].1, Value::I32(1));

    let mut boxed = driver.connect().unwrap();
    let boxed_rows = boxed.query_once("SELECT 1", &[]).unwrap();
    assert_eq!(boxed_rows[0].columns[0].1, Value::I32(1));
    drop(boxed);

    conn.execute("CREATE TABLE note (body text)", &[]).unwrap();
    let inserted = conn
        .execute("INSERT INTO note (body) VALUES (?)", &[Value::string("hello?")])
        .unwrap();
    assert_eq!(inserted, 1);
    let notes = conn
        .query(
            "SELECT body FROM note WHERE body = ? AND '?' = '?'",
            &[Value::string("hello?")],
        )
        .unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].get("body"), Some(&Value::string("hello?")));

    round_trip_scalars(&mut conn);
    savepoint_keeps_the_first_row(&mut conn);
    stream_requires_a_transaction(&mut conn);
    stream_reads_every_inserted_row(&mut conn);
    a_refused_stream_fails_at_open_and_a_closed_one_reopens(&mut conn);
    deferred_rows_go_out_as_statements_of_many_rows(&mut conn);
    rollback_ends_an_aborted_transaction_with_an_open_stream(&mut conn);
    statement_timeout_is_never_lifted_by_rounding(&mut conn);
    a_pipelined_begin_starts_the_transaction_its_first_statement_runs_in(&mut conn);
    deferred_writes_run_in_order_and_commit(&mut conn);
    a_deferred_failure_is_the_next_error_and_stops_the_commit(&mut conn);
    a_first_run_prepares_and_a_failed_one_keeps_no_statement(&mut conn);

    let mut other = driver.connect_postgres().unwrap();
    conn.begin().unwrap();
    conn.execute("INSERT INTO note (body) VALUES (?)", &[Value::string("hidden")])
        .unwrap();
    let unseen = other
        .query("SELECT body FROM note WHERE body = ?", &[Value::string("hidden")])
        .unwrap();
    assert!(unseen.is_empty(), "{unseen:?}");
    conn.commit().unwrap();
    let seen = other
        .query("SELECT body FROM note WHERE body = ?", &[Value::string("hidden")])
        .unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].get("body"), Some(&Value::string("hidden")));
    drop(other);

    let mut cached = driver.connect_postgres().unwrap();
    assert_eq!(cached.statement_cache_len(), 0);
    cached.query("SELECT 1", &[]).unwrap();
    cached.query("SELECT 1", &[]).unwrap();
    assert_eq!(cached.statement_cache_len(), 1);
    cached.query_once("SELECT 1", &[]).unwrap();
    cached.query_once("SELECT 2", &[]).unwrap();
    assert_eq!(cached.statement_cache_len(), 1);
    cached.canceller().cancel().unwrap();
    drop(cached);
    drop(conn);

    let pid = postmaster_pid(&data_dir);
    drop(cluster);
    assert!(
        !data_dir.join("postmaster.pid").exists(),
        "postmaster.pid still present"
    );
    assert!(!process_live(pid), "postmaster {pid} still alive");
}

/// The figures in plans/archive/EmbeddedPostgres.md: `cargo test -p moqui-host --lib postmaster_memory -- --nocapture`.
/// PSS splits shared_buffers across the processes that map it, so its sum is the cluster's share of RAM.
#[cfg(target_os = "linux")]
#[test]
fn postmaster_memory_stays_small_for_both_roles() {
    for role in [ClusterRole::Entity, ClusterRole::Sim] {
        let dir = temp_cluster_dir();
        let _guard = DirGuard(dir.clone());
        let options = StartOptions {
            role,
            ..StartOptions::new(20)
        };
        let (cluster, driver) = EmbeddedCluster::start_with(&dir, &options).unwrap();
        let mut conn = driver.connect_postgres().unwrap();
        conn.execute(
            "CREATE TABLE filler AS SELECT g, md5(g::text) AS t FROM generate_series(1, 20000) g",
            &[],
        )
        .unwrap();
        conn.query_once("SELECT count(*) FROM filler", &[]).unwrap();
        let pid = postmaster_pid(cluster.data_dir());
        let (processes, rss_kb, pss_kb) = cluster_memory(pid);
        println!(
            "{role:?}: {processes} processes, RSS sum {} MB, PSS sum {} MB",
            rss_kb / 1024,
            pss_kb / 1024
        );
        assert!(pss_kb < 200 * 1024, "{role:?} PSS {pss_kb} kB");
        drop(conn);
        drop(cluster);
    }
}

/// Process count and the RSS and PSS sums, in kB, of a postmaster and its children.
#[cfg(target_os = "linux")]
fn cluster_memory(postmaster: u32) -> (usize, u64, u64) {
    let mut pids = vec![postmaster];
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let ppid = stat
            .rfind(')')
            .and_then(|end| stat[end + 1..].split_whitespace().nth(1))
            .and_then(|field| field.parse::<u32>().ok());
        if ppid == Some(postmaster) {
            pids.push(pid);
        }
    }
    let mut rss = 0;
    let mut pss = 0;
    for pid in &pids {
        let rollup = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap_or_default();
        for line in rollup.lines() {
            let mut parts = line.split_whitespace();
            let (Some(name), Some(kb)) = (parts.next(), parts.next().and_then(|kb| kb.parse::<u64>().ok())) else {
                continue;
            };
            match name {
                "Rss:" => rss += kb,
                "Pss:" => pss += kb,
                _ => {}
            }
        }
    }
    (pids.len(), rss, pss)
}

/// A session the server ends or cancels fails the call that finds it with the server's reason, and leaves
/// the connection closed (or, for a cancel or a timeout, usable after a rollback); the pool then opens a new
/// one. The session is ended with `pg_terminate_backend`, as an administrator or a server shutdown ends it.
#[test]
fn connections_fail_cleanly_when_the_server_ends_or_cancels_them() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (_cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let mut admin = driver.connect_postgres().unwrap();
    admin
        .execute("CREATE TABLE ended (id text PRIMARY KEY, n int)", &[])
        .unwrap();
    for n in 0..120 {
        admin
            .execute(
                "INSERT INTO ended (id, n) VALUES (?, ?)",
                &[Value::string(format!("e{n}")), Value::I32(n)],
            )
            .unwrap();
    }
    let pid_of = |conn: &mut PostgresConn| {
        let rows = conn.query_once("SELECT pg_backend_pid() AS pid", &[]).unwrap();
        match rows[0].get("pid") {
            Some(Value::I32(pid)) => *pid,
            other => panic!("pid {other:?}"),
        }
    };
    /// End the session `pid` and wait until it is gone, and past the client's quiet time.
    fn end(admin: &mut PostgresConn, pid: i32) {
        admin
            .query_once("SELECT pg_terminate_backend(?)", &[Value::I32(pid)])
            .unwrap();
        let alive = "SELECT count(*) AS c FROM pg_stat_activity WHERE pid = ?";
        while admin.query_once(alive, &[Value::I32(pid)]).unwrap()[0].get("c") != Some(&Value::I64(0)) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    // Mid-transaction: the next statement fails and the connection is closed.
    let mut conn = driver.connect_postgres().unwrap();
    let pid = pid_of(&mut conn);
    conn.begin().unwrap();
    conn.execute("UPDATE ended SET n = n + 1 WHERE id = ?", &[Value::string("e1")])
        .unwrap();
    end(&mut admin, pid);
    let err = conn
        .execute("UPDATE ended SET n = n + 1 WHERE id = ?", &[Value::string("e2")])
        .unwrap_err();
    assert!(err.message().contains("terminating connection"), "{err}");
    assert!(DbConnection::is_closed(&conn));
    let _ = conn.rollback();

    // Mid-pipeline: rows held and writes sent unanswered; the commit fails and nothing is written.
    let mut conn = driver.connect_postgres().unwrap();
    let pid = pid_of(&mut conn);
    conn.begin().unwrap();
    let upsert = "INSERT INTO ended (id, n) VALUES (?, ?) ON CONFLICT (id) DO UPDATE SET n = EXCLUDED.n";
    for n in 0..40_u32 {
        let row = vec![Value::string(format!("p{n}")), Value::I64(i64::from(n))];
        conn.execute_deferred_row(upsert, row, u64::from(n)).unwrap();
    }
    conn.execute_deferred("UPDATE ended SET n = 0 WHERE id = ?", vec![Value::string("e3")], None)
        .unwrap();
    end(&mut admin, pid);
    assert!(conn.commit().is_err());
    assert!(DbConnection::is_closed(&conn));
    let _ = conn.rollback();
    let written = admin
        .query_once("SELECT count(*) AS c FROM ended WHERE id LIKE 'p%'", &[])
        .unwrap();
    assert_eq!(written[0].get("c"), Some(&Value::I64(0)));

    // Mid-stream: the rows already fetched are read, then the next fetch fails.
    let mut conn = driver.connect_postgres().unwrap();
    let pid = pid_of(&mut conn);
    conn.begin().unwrap();
    conn.open_stream("SELECT id FROM ended ORDER BY n", &[]).unwrap();
    assert!(conn.next_row().unwrap().is_some());
    end(&mut admin, pid);
    let mut failed = None;
    for _ in 0..200 {
        match conn.next_row() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(err) => {
                failed = Some(err);
                break;
            }
        }
    }
    assert!(failed.is_some(), "the stream ended without an error");
    assert!(DbConnection::is_closed(&conn));
    let _ = conn.rollback();

    // A statement timeout fires: the statement fails, and after the rollback the connection is used again.
    let mut conn = driver.connect_postgres().unwrap();
    conn.begin().unwrap();
    conn.set_statement_timeout(Some(std::time::Duration::from_millis(50)))
        .unwrap();
    let err = conn.query_once("SELECT pg_sleep(5)", &[]).unwrap_err();
    assert!(err.message().contains("statement timeout"), "{err}");
    conn.rollback().unwrap();
    assert!(!DbConnection::is_closed(&conn));
    assert_eq!(conn.query_once("SELECT 1", &[]).unwrap().len(), 1);

    // A cancel from another thread, as the deadline thread sends it.
    let canceller = conn.canceller();
    let cancelling = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        canceller.cancel().unwrap();
    });
    let err = conn.query_once("SELECT pg_sleep(5)", &[]).unwrap_err();
    cancelling.join().unwrap();
    assert!(err.message().contains("canceling statement"), "{err}");
    assert_eq!(conn.query_once("SELECT 1", &[]).unwrap().len(), 1);

    // The pool: a connection the server ended fails one call; the next has a new connection.
    let tx = moqui_core::transaction::TransactionManager::new(
        std::sync::Arc::new(driver.clone()) as std::sync::Arc<dyn DbDriver>,
        moqui_core::transaction::PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let pid = tx
        .with_connection(|conn| conn.query_once("SELECT pg_backend_pid() AS pid", &[]))
        .unwrap()[0]
        .get("pid")
        .cloned();
    let Some(Value::I32(pid)) = pid else {
        panic!("pid {pid:?}")
    };
    end(&mut admin, pid);
    assert!(tx.with_connection(|conn| conn.query_once("SELECT 1", &[])).is_err());
    let again = tx.with_connection(|conn| conn.query_once("SELECT pg_backend_pid() AS pid", &[]));
    assert_ne!(again.unwrap()[0].get("pid"), Some(&Value::I32(pid)));
    let _ = tx.destroy_all_in_thread();
}

#[test]
fn same_version_restart_keeps_the_row() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    {
        let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
        let mut conn = driver.connect_postgres().unwrap();
        conn.execute("CREATE TABLE kept (n int)", &[]).unwrap();
        conn.execute("INSERT INTO kept (n) VALUES (?)", &[Value::I32(7)])
            .unwrap();
        drop(conn);
        drop(cluster);
    }
    let (_cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let mut conn = driver.connect_postgres().unwrap();
    let rows = conn.query("SELECT n FROM kept", &[]).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n"), Some(&Value::I32(7)));
}

static RELATIVE_CLUSTER_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Regression: `moqui load` passed the relative `runtime/db/embedded` straight into
/// postgresql_embedded, and the postmaster could not create its lock file because a
/// relative socket dir resolves against the wrong base. `start` must resolve to
/// absolute install/data/socket paths, so a relative dir boots and serves its socket.
#[test]
fn relative_cluster_dir_starts_and_serves_its_socket() {
    let name = format!(
        "moqui-pg-rel-{}-{}",
        std::process::id(),
        RELATIVE_CLUSTER_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let rel = PathBuf::from(&name);
    let abs = std::env::current_dir().unwrap().join(&rel);
    let _guard = DirGuard(abs.clone());
    // Unix socket paths need fewer than 108 bytes. Skip the live boot when this
    // checkout sits too deep for that; the absolute-resolution assert below still
    // documents the invariant.
    if abs.join("data").join(".s.PGSQL.5432").as_os_str().len() >= 108 {
        println!(
            "skipping live relative boot: socket path too long under {}",
            abs.display()
        );
        return;
    }
    let (cluster, driver) = EmbeddedCluster::start(&rel, 20).unwrap();
    let data_dir = cluster.data_dir().to_path_buf();
    assert!(
        data_dir.is_absolute(),
        "data dir still relative: {}",
        data_dir.display()
    );
    assert!(
        data_dir.join(".s.PGSQL.5432").exists(),
        "socket missing in {}",
        data_dir.display()
    );
    let mut conn = driver.connect_postgres().unwrap();
    let rows = conn.query_once("SELECT 1", &[]).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].columns[0].1, Value::I32(1));
}

#[test]
fn adopt_live_postmaster_keeps_the_same_pid() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let data_dir = cluster.data_dir().to_path_buf();
    drop(driver);
    let pid = postmaster_pid(&data_dir);
    let _kill = KillGuard(pid);
    std::mem::forget(cluster);
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    assert_eq!(postmaster_pid(&data_dir), pid);
    let mut conn = driver.connect_postgres().unwrap();
    let rows = conn.query_once("SELECT 1", &[]).unwrap();
    assert_eq!(rows[0].columns[0].1, Value::I32(1));
    drop(conn);
    drop(cluster);
    assert_eq!(postmaster_pid(&data_dir), pid, "adopted postmaster was stopped");
    assert!(process_live(pid), "adopted postmaster {pid} must keep running");
}

#[test]
fn live_major_mismatch_refuses_with_the_pid() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let data_dir = cluster.data_dir().to_path_buf();
    drop(driver);
    let pid = postmaster_pid(&data_dir);
    let _kill = KillGuard(pid);
    std::mem::forget(cluster);
    std::fs::write(data_dir.join("PG_VERSION"), "17\n").unwrap();
    let err = start_error(&dir);
    assert!(err.message().contains(&pid.to_string()), "{err}");
    assert!(err.message().contains("17"), "{err}");
    assert!(err.message().contains(SHIPPED_VERSION), "{err}");
    assert!(process_live(pid), "refusing to adopt killed postmaster {pid}");
}

#[test]
fn a_lock_copied_from_another_live_cluster_is_not_adopted() {
    let live = temp_cluster_dir();
    let copy = temp_cluster_dir();
    let _guards = (DirGuard(live.clone()), DirGuard(copy.clone()));
    let (cluster, driver) = EmbeddedCluster::start(&live, 20).unwrap();
    drop(driver);
    let (stopped, stopped_driver) = EmbeddedCluster::start(&copy, 20).unwrap();
    drop(stopped_driver);
    let copy_data = stopped.data_dir().to_path_buf();
    drop(stopped);
    let pid = postmaster_pid(cluster.data_dir());
    std::fs::copy(
        cluster.data_dir().join("postmaster.pid"),
        copy_data.join("postmaster.pid"),
    )
    .unwrap();
    let err = start_error(&copy);
    assert!(err.message().contains(&pid.to_string()), "{err}");
    assert!(err.message().contains("copied lock"), "{err}");
    assert!(process_live(pid), "the live postmaster was touched");
    drop(cluster);
}

#[test]
fn clusters_share_one_install_and_an_old_one_moves_there() {
    let root = temp_cluster_dir();
    let _guard = DirGuard(root.clone());
    let runtime = root.join("runtime");
    let first = runtime.join("db/first");
    // An install from an earlier build, inside the first cluster directory.
    let (cluster, driver) = EmbeddedCluster::start(&first, 20).unwrap();
    drop(driver);
    drop(cluster);
    assert!(first.join("install").join(SHIPPED_VERSION).is_dir());
    let shared = shared_install_dir(&runtime, &first);
    assert_eq!(
        shared,
        std::fs::canonicalize(&runtime).unwrap().join("install/postgres")
    );
    assert!(
        shared.join(SHIPPED_VERSION).join("bin").is_dir(),
        "the old install moved"
    );
    assert!(!first.join("install").exists());
    // The install a later build unpacked in runtime/db moves to runtime/install the same way.
    let other_runtime = root.join("other/runtime");
    std::fs::create_dir_all(other_runtime.join("db")).unwrap();
    std::fs::rename(&shared, other_runtime.join("db/install")).unwrap();
    let moved = shared_install_dir(&other_runtime, &other_runtime.join("db/embedded"));
    assert_eq!(
        moved,
        std::fs::canonicalize(&other_runtime).unwrap().join("install/postgres")
    );
    assert!(
        moved.join(SHIPPED_VERSION).join("bin").is_dir(),
        "the runtime/db install moved"
    );
    assert!(!other_runtime.join("db/install").exists());
    std::fs::rename(&moved, &shared).unwrap();
    let options = StartOptions {
        install_dir: Some(shared.clone()),
        ..StartOptions::new(20)
    };
    let (one, one_driver) = EmbeddedCluster::start_with(&first, &options).unwrap();
    let (two, two_driver) = EmbeddedCluster::start_with(runtime.join("db/second"), &options).unwrap();
    assert!(!runtime.join("db/second/install").exists());
    for driver in [&one_driver, &two_driver] {
        let rows = driver.connect_postgres().unwrap().query_once("SELECT 1", &[]).unwrap();
        assert_eq!(rows.len(), 1);
    }
    drop((one_driver, two_driver));
    drop(one);
    drop(two);
}

#[test]
fn dead_pid_starts_a_new_postmaster() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let data_dir = cluster.data_dir().to_path_buf();
    drop(driver);
    let pid = postmaster_pid(&data_dir);
    let _kill = KillGuard(pid);
    std::mem::forget(cluster);
    kill_pid(pid);
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let mut conn = driver.connect_postgres().unwrap();
    let rows = conn.query_once("SELECT 1", &[]).unwrap();
    assert_eq!(rows[0].columns[0].1, Value::I32(1));
    drop(conn);
    drop(cluster);
}

#[test]
fn stopped_major_mismatch_names_the_missing_binaries() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, _driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let data_dir = cluster.data_dir().to_path_buf();
    drop(cluster);
    std::fs::write(data_dir.join("PG_VERSION"), "17\n").unwrap();
    let err = start_error(&dir);
    assert!(err.message().contains("17"), "{err}");
    assert!(err.message().contains(SHIPPED_VERSION), "{err}");
    assert!(err.message().contains("binaries"), "{err}");
}

#[test]
fn stopped_major_mismatch_with_a_sibling_install_does_not_upgrade() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, _driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let data_dir = cluster.data_dir().to_path_buf();
    drop(cluster);
    std::fs::write(data_dir.join("PG_VERSION"), "17\n").unwrap();
    let upgrade = dir.join("postgresql-17").join("bin").join("pg_upgrade");
    std::fs::create_dir_all(upgrade.parent().unwrap()).unwrap();
    std::fs::write(&upgrade, b"").unwrap();
    let err = start_error(&dir);
    assert!(err.message().contains("17"), "{err}");
    assert!(err.message().contains(SHIPPED_VERSION), "{err}");
    assert!(err.message().contains("pg_upgrade"), "{err}");
    assert_eq!(std::fs::read(&upgrade).unwrap(), b"");
}

#[test]
fn save_db_round_trips_the_search_schema() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let mut conn = driver.connect_postgres().unwrap();
    conn.execute("CREATE SCHEMA search", &[]).unwrap();
    conn.execute("CREATE TABLE search.kept (body text)", &[]).unwrap();
    conn.execute("INSERT INTO search.kept (body) VALUES (?)", &[Value::string("kept")])
        .unwrap();
    let dump = dir.join("moqui.dump");
    cluster.save_db(&dump).unwrap();
    let header = std::fs::read(&dump).unwrap();
    assert!(header.starts_with(b"PGDMP"), "dump is not custom format");
    let list = std::process::Command::new(dir.join("install").join(SHIPPED_VERSION).join("bin/pg_restore"))
        .arg("--list")
        .arg(&dump)
        .output()
        .unwrap();
    assert!(list.status.success(), "{}", String::from_utf8_lossy(&list.stderr));
    let listed = String::from_utf8_lossy(&list.stdout);
    assert!(listed.contains("search"), "{listed}");
    drop(conn);
    conn_drop_schema(&driver);
    cluster.load_save(&dump).unwrap();
    let mut conn = driver.connect_postgres().unwrap();
    let rows = conn.query("SELECT body FROM search.kept", &[]).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("body"), Some(&Value::string("kept")));
}

fn start_error(dir: &Path) -> moqui_lib::error::Error {
    match EmbeddedCluster::start(dir, 20) {
        Err(err) => err,
        Ok((cluster, _driver)) => {
            panic!("start succeeded for {}", cluster.data_dir().display());
        }
    }
}

fn conn_drop_schema(driver: &PostgresDriver) {
    let mut conn = driver.connect_postgres().unwrap();
    conn.execute("DROP SCHEMA search CASCADE", &[]).unwrap();
}

struct KillGuard(u32);

impl Drop for KillGuard {
    fn drop(&mut self) {
        if process_live(self.0) {
            let _ = std::process::Command::new("kill")
                .args(["-9", &self.0.to_string()])
                .status();
        }
    }
}

fn kill_pid(pid: u32) {
    let status = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .expect("kill");
    assert!(status.success(), "kill {pid}");
    for _ in 0..50 {
        if !process_live(pid) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("postmaster {pid} still present after kill");
}

fn round_trip_scalars(conn: &mut PostgresConn) {
    conn.execute(
        "CREATE TABLE sample (
                    txt text,
                    flag boolean,
                    i2 int2,
                    i4 int4,
                    i8 int8,
                    f4 float4,
                    f8 float8,
                    n numeric,
                    ts timestamptz,
                    d date,
                    t time,
                    b bytea,
                    z int4
                )",
        &[],
    )
    .unwrap();
    let timestamp = kind("2024-07-15 01:14:00", "Timestamp");
    let date = kind("2024-07-15", "Date");
    let time = kind("01:14:00", "Time");
    let decimal = kind("3950.123456", "BigDecimal");
    let bytes = Value::Bytes(vec![1, 2, 3, 255]);
    conn.execute(
        "INSERT INTO sample (txt, flag, i2, i4, i8, f4, f8, n, ts, d, t, b, z) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            Value::string("alpha"),
            Value::Bool(true),
            Value::I32(4),
            Value::I32(7),
            Value::I64(9),
            Value::F64(1.5),
            Value::F64(2.25),
            decimal.clone(),
            timestamp.clone(),
            date.clone(),
            time.clone(),
            bytes.clone(),
            Value::Null,
        ],
    )
    .unwrap();
    let rows = conn
        .query(
            "SELECT txt, flag, i2, i4, i8, f4, f8, n, ts, d, t, b, z FROM sample",
            &[],
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.get("txt"), Some(&Value::string("alpha")));
    assert_eq!(row.get("flag"), Some(&Value::Bool(true)));
    assert_eq!(row.get("i2"), Some(&Value::I32(4)));
    assert_eq!(row.get("i4"), Some(&Value::I32(7)));
    assert_eq!(row.get("i8"), Some(&Value::I64(9)));
    assert_eq!(row.get("f4"), Some(&Value::F64(1.5)));
    assert_eq!(row.get("f8"), Some(&Value::F64(2.25)));
    assert_eq!(row.get("n"), Some(&decimal));
    assert_eq!(row.get("ts"), Some(&timestamp));
    assert_eq!(row.get("d"), Some(&date));
    assert_eq!(row.get("t"), Some(&time));
    assert_eq!(row.get("b"), Some(&bytes));
    assert_eq!(row.get("z"), Some(&Value::Null));
}

fn savepoint_keeps_the_first_row(conn: &mut PostgresConn) {
    conn.execute("CREATE TABLE sp (n int)", &[]).unwrap();
    conn.begin().unwrap();
    conn.execute("INSERT INTO sp (n) VALUES (?)", &[Value::I32(1)]).unwrap();
    conn.savepoint("my sp").unwrap();
    conn.execute("INSERT INTO sp (n) VALUES (?)", &[Value::I32(2)]).unwrap();
    conn.rollback_to_savepoint("my sp").unwrap();
    conn.release_savepoint("my sp").unwrap();
    conn.commit().unwrap();
    let rows = conn.query("SELECT n FROM sp", &[]).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n"), Some(&Value::I32(1)));
}

fn stream_requires_a_transaction(conn: &mut PostgresConn) {
    let err = conn.open_stream("SELECT 1", &[]).unwrap_err();
    assert!(err.message().contains("transaction is required"), "{err}");
    assert!(!conn.stream_open());
}

fn stream_reads_every_inserted_row(conn: &mut PostgresConn) {
    conn.execute("CREATE TABLE streamed (n int)", &[]).unwrap();
    conn.begin().unwrap();
    for n in 1..=51 {
        conn.execute("INSERT INTO streamed (n) VALUES (?)", &[Value::I32(n)])
            .unwrap();
    }
    conn.open_stream("SELECT n FROM streamed ORDER BY n", &[]).unwrap();
    assert!(conn.stream_open());
    for n in 1..=51 {
        let row = conn.next_row().unwrap().unwrap_or_else(|| panic!("missing row {n}"));
        assert_eq!(row.get("n"), Some(&Value::I32(n)));
    }
    assert!(conn.next_row().unwrap().is_none());
    assert!(conn.stream_open());
    conn.close_stream().unwrap();
    assert!(!conn.stream_open());
    conn.commit().unwrap();
}

/// The `DECLARE` goes out with the first `FETCH`, so a query the server refuses still fails at the open, with its
/// own error. The `CLOSE` goes out with the next request, so the cursor can be declared again right after it.
fn a_refused_stream_fails_at_open_and_a_closed_one_reopens(conn: &mut PostgresConn) {
    conn.begin().unwrap();
    let err = conn
        .open_stream("SELECT no_such_column FROM streamed", &[])
        .unwrap_err();
    assert!(err.message().contains("no_such_column"), "{err}");
    assert!(!conn.stream_open());
    conn.rollback().unwrap();

    conn.begin().unwrap();
    conn.open_stream("SELECT n FROM streamed WHERE n > ? ORDER BY n", &[Value::I32(49)])
        .unwrap();
    assert_eq!(conn.next_row().unwrap().unwrap().get("n"), Some(&Value::I32(50)));
    conn.close_stream().unwrap();
    conn.open_stream("SELECT n FROM streamed ORDER BY n", &[]).unwrap();
    let mut count = 0;
    while conn.next_row().unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 51);
    conn.close_stream().unwrap();
    conn.commit().unwrap();
}

/// Rows of one upsert go out several to a statement. A key the held rows already have goes after them (one upsert
/// cannot change a row twice), another statement or a query sends them first, a rollback drops them, and a
/// row the server refuses fails the commit.
fn deferred_rows_go_out_as_statements_of_many_rows(conn: &mut PostgresConn) {
    conn.execute("CREATE TABLE batched (id text PRIMARY KEY, n int NOT NULL)", &[])
        .unwrap();
    conn.execute("CREATE TABLE beside (id text PRIMARY KEY)", &[]).unwrap();
    let upsert = "INSERT INTO batched (id, n) VALUES (?, ?) ON CONFLICT (id) DO UPDATE SET n = EXCLUDED.n";
    let key = |id: &str| {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut hasher);
        hasher.finish()
    };
    let row = |conn: &mut PostgresConn, id: &str, n: i32| {
        conn.execute_deferred_row(upsert, vec![Value::string(id), Value::I32(n)], key(id))
            .unwrap();
    };
    conn.begin().unwrap();
    for n in 0..125 {
        row(conn, &format!("r{n}"), n);
        if n == 60 {
            // The same key again, and a row of another statement between.
            row(conn, "r7", 700);
            conn.execute_deferred_row("INSERT INTO beside (id) VALUES (?)", vec![Value::string("b")], 1)
                .unwrap();
        }
    }
    // A query sends what is held first, and sees it.
    let counted = conn.query("SELECT count(*) AS c FROM batched", &[]).unwrap();
    assert_eq!(counted[0].get("c"), Some(&Value::I64(125)));
    row(conn, "r124", 1240);
    conn.commit().unwrap();
    let rows = conn.query(
        "SELECT id, n FROM batched WHERE id IN (?, ?, ?)",
        &[Value::string("r7"), Value::string("r100"), Value::string("r124")],
    );
    let rows = rows.unwrap();
    let n_of = |id: &str| {
        rows.iter()
            .find(|row| row.get("id") == Some(&Value::string(id)))
            .and_then(|row| row.get("n").cloned())
    };
    assert_eq!(n_of("r7"), Some(Value::I32(700)));
    assert_eq!(n_of("r100"), Some(Value::I32(100)));
    assert_eq!(n_of("r124"), Some(Value::I32(1240)));
    assert_eq!(conn.query("SELECT id FROM beside", &[]).unwrap().len(), 1);

    conn.begin().unwrap();
    row(conn, "dropped", 1);
    conn.rollback().unwrap();
    let dropped = conn.query("SELECT id FROM batched WHERE id = ?", &[Value::string("dropped")]);
    assert!(dropped.unwrap().is_empty());

    conn.begin().unwrap();
    row(conn, "fine", 1);
    conn.execute_deferred_row(upsert, vec![Value::string("refused"), Value::Null], key("refused"))
        .unwrap();
    let err = conn.commit().unwrap_err();
    assert!(err.message().contains("null value"), "{err}");
    conn.rollback().unwrap();
    let fine = conn.query("SELECT id FROM batched WHERE id = ?", &[Value::string("fine")]);
    assert!(fine.unwrap().is_empty());
}

/// ROLLBACK closes the cursor itself, so a stream left open in an aborted transaction does not stop it.
fn rollback_ends_an_aborted_transaction_with_an_open_stream(conn: &mut PostgresConn) {
    conn.begin().unwrap();
    conn.open_stream("SELECT n FROM streamed ORDER BY n", &[]).unwrap();
    assert!(conn.next_row().unwrap().is_some());
    assert!(conn.query_once("SELECT 1 / 0", &[]).is_err());
    conn.rollback().unwrap();
    assert!(!conn.stream_open());
    assert_eq!(conn.query_once("SELECT 1", &[]).unwrap().len(), 1);
}

/// A deadline that has passed is an error, and a sub-millisecond remainder still sets a limit.
fn statement_timeout_is_never_lifted_by_rounding(conn: &mut PostgresConn) {
    conn.begin().unwrap();
    assert!(conn.set_statement_timeout(Some(std::time::Duration::ZERO)).is_err());
    conn.rollback().unwrap();
    conn.begin().unwrap();
    conn.set_statement_timeout(Some(std::time::Duration::from_micros(300)))
        .unwrap();
    assert_eq!(show(conn, "statement_timeout"), "1ms");
    conn.rollback().unwrap();
}

/// `BEGIN` is sent without waiting, and the statement after it runs in the transaction it starts.
fn a_pipelined_begin_starts_the_transaction_its_first_statement_runs_in(conn: &mut PostgresConn) {
    conn.execute("CREATE TABLE piped (n int PRIMARY KEY)", &[]).unwrap();
    conn.begin().unwrap();
    conn.execute("INSERT INTO piped (n) VALUES (?)", &[Value::I32(1)])
        .unwrap();
    conn.rollback().unwrap();
    assert!(conn.query("SELECT n FROM piped", &[]).unwrap().is_empty());
    // `SET LOCAL` outside a transaction does nothing.
    conn.begin().unwrap();
    conn.query_once("SET LOCAL application_name = 'piped'", &[]).unwrap();
    assert_eq!(show(conn, "application_name"), "piped");
    conn.commit().unwrap();
    assert_ne!(show(conn, "application_name"), "piped");
}

/// Deferred writes run in the order sent, a read on the connection sees them, and more than the pipeline
/// holds commit.
fn deferred_writes_run_in_order_and_commit(conn: &mut PostgresConn) {
    conn.begin().unwrap();
    for n in 1..=600 {
        conn.execute_deferred("INSERT INTO piped (n) VALUES (?)", vec![Value::I32(n)], None)
            .unwrap();
    }
    conn.execute_deferred(
        "UPDATE piped SET n = ? WHERE n = ?",
        vec![Value::I32(1000), Value::I32(600)],
        Some("no row 600".to_string()),
    )
    .unwrap();
    let rows = conn.query("SELECT count(*) AS c, max(n) AS m FROM piped", &[]).unwrap();
    assert_eq!(rows[0].get("c"), Some(&Value::I64(600)));
    assert_eq!(rows[0].get("m"), Some(&Value::I32(1000)));
    conn.execute_deferred("DELETE FROM piped WHERE n > ?", vec![Value::I32(500)], None)
        .unwrap();
    conn.commit().unwrap();
    let rows = conn.query("SELECT count(*) AS c FROM piped", &[]).unwrap();
    assert_eq!(rows[0].get("c"), Some(&Value::I64(500)));
    // Out of a transaction it runs at once.
    conn.execute_deferred("DELETE FROM piped WHERE n > ?", vec![Value::I32(10)], None)
        .unwrap();
    let rows = conn.query("SELECT count(*) AS c FROM piped", &[]).unwrap();
    assert_eq!(rows[0].get("c"), Some(&Value::I64(10)));
}

/// A deferred statement that fails is the error of the next call that waits, not the "transaction is aborted"
/// of that call's own statement. Left to the commit, it fails the commit (which would otherwise roll back
/// without an error), and so does a deferred update that changed no row.
fn a_deferred_failure_is_the_next_error_and_stops_the_commit(conn: &mut PostgresConn) {
    conn.begin().unwrap();
    conn.execute_deferred("INSERT INTO piped (n) VALUES (?)", vec![Value::I32(1)], None)
        .unwrap();
    let err = conn.query("SELECT n FROM piped", &[]).unwrap_err();
    assert!(err.message().contains("duplicate key"), "{err}");
    conn.rollback().unwrap();

    conn.begin().unwrap();
    conn.execute_deferred("INSERT INTO piped (n) VALUES (?)", vec![Value::I32(11)], None)
        .unwrap();
    conn.execute_deferred("INSERT INTO piped (n) VALUES (?)", vec![Value::I32(2)], None)
        .unwrap();
    conn.execute_deferred("INSERT INTO piped (n) VALUES (?)", vec![Value::I32(12)], None)
        .unwrap();
    let err = conn.commit().unwrap_err();
    assert!(err.message().contains("duplicate key"), "{err}");
    conn.rollback().unwrap();

    conn.begin().unwrap();
    conn.execute_deferred(
        "UPDATE piped SET n = n WHERE n = ?",
        vec![Value::I32(99)],
        Some("no row 99".to_string()),
    )
    .unwrap();
    let err = conn.finish_deferred().unwrap_err();
    assert_eq!(err.message(), "no row 99");
    conn.rollback().unwrap();
    let rows = conn.query("SELECT count(*) AS c FROM piped", &[]).unwrap();
    assert_eq!(rows[0].get("c"), Some(&Value::I64(10)));
}

/// A statement's first run prepares it in the same round trip, and later runs use it. A first run that fails
/// after the statement parsed keeps none, and the next run prepares it again.
fn a_first_run_prepares_and_a_failed_one_keeps_no_statement(conn: &mut PostgresConn) {
    let before = conn.statement_cache_len();
    let select = "SELECT n FROM piped WHERE n <= ? ORDER BY n";
    for _ in 0..2 {
        let rows = conn.query(select, &[Value::I32(2)]).unwrap();
        let numbers: Vec<_> = rows.iter().map(|row| row.get("n").cloned()).collect();
        assert_eq!(numbers, [Some(Value::I32(1)), Some(Value::I32(2))]);
        assert_eq!(conn.statement_cache_len(), before + 1);
    }
    let insert = "INSERT INTO piped (n) SELECT ?";
    let err = conn.execute(insert, &[Value::I32(1)]).unwrap_err();
    assert!(err.message().contains("duplicate key"), "{err}");
    assert_eq!(conn.statement_cache_len(), before + 1);
    assert_eq!(conn.execute(insert, &[Value::I32(77)]).unwrap(), 1);
    assert_eq!(conn.execute(insert, &[Value::I32(78)]).unwrap(), 1);
    assert_eq!(conn.statement_cache_len(), before + 2);
}

fn show(conn: &mut PostgresConn, name: &str) -> String {
    let rows = conn.query_once(&format!("SHOW {name}"), &[]).unwrap();
    match &rows[0].columns[0].1 {
        Value::String(text) => text.clone(),
        other => panic!("show {name}: {other:?}"),
    }
}

fn kind(text: &str, java_type: &str) -> Value {
    let converted = basic_convert(&Value::string(text), java_type);
    assert!(
        !matches!(converted.value, Value::String(_)),
        "{text} did not convert to {java_type}: {:?}",
        converted.value
    );
    converted.value
}

fn temp_cluster_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // the clock is only microseconds on macOS, so parallel tests need the counter to get different directories
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("moqui-pg-{}-{nanos}-{seq}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}

fn postmaster_pid(data_dir: &Path) -> u32 {
    let text = std::fs::read_to_string(data_dir.join("postmaster.pid")).unwrap();
    text.lines().next().unwrap().trim().parse().unwrap()
}
