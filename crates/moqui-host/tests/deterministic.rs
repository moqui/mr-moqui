//! Isolated child processes keep the process-global coordinator out of normal-mode tests.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_core::context::{ExecutionContextFactory, Executor};
use moqui_core::driver::{DbCancel, DbConnection, DbDriver, DbRow};
use moqui_core::transaction::{PoolConfig, TransactionAttribute, TransactionManager};
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::json;

const BASE: i64 = 1760000000000;

#[test]
fn protocol_and_failure_cases() {
    for scenario in [
        "lifecycle",
        "operations",
        "postgres",
        "status",
        "json",
        "assignment",
        "complete",
        "timeout",
        "uncertain",
        "config",
        "normal",
        "random",
        "random-invalid",
        "random-uninstalled",
    ] {
        if scenario == "postgres" && !cfg!(all(feature = "postgres", feature = "embedded-postgres")) {
            continue;
        }
        let dir = std::env::temp_dir().join(format!("moqui-deterministic-{}-{scenario}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("deterministic.log");
        std::fs::write(&log, "{\"existing\":true}\n").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(err) => panic!("{err}"),
                };
                stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let headers = String::from_utf8(bytes).unwrap();
                assert!(headers.starts_with("POST /transaction HTTP/1.1"));
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                captured.lock().unwrap().push(request.clone());
                let sequence = request["sequence"].as_u64().unwrap_or(0);
                let response = if request["event"] == "random" {
                    let length = request["length"].as_u64().unwrap() as usize;
                    let bytes = if scenario == "random-invalid" {
                        "bad".to_string()
                    } else {
                        (0..length)
                            .map(|byte| format!("{:02x}", byte as u8))
                            .collect::<String>()
                    };
                    json!({"bytes":bytes}).to_string()
                } else if scenario == "json" {
                    "not json".to_string()
                } else if scenario == "assignment" {
                    json!({"operation_id":"x", "time_epoch_ms":"wrong"}).to_string()
                } else if request["event"] == "complete" || request["event"] == "operation" {
                    json!({"ok": scenario != "complete"}).to_string()
                } else {
                    json!({"operation_id":format!("op-{sequence:06}"), "time_epoch_ms":BASE + sequence as i64})
                        .to_string()
                };
                if scenario == "timeout" {
                    std::thread::sleep(Duration::from_millis(1200));
                }
                let status = if scenario == "status" {
                    "503 Service Unavailable"
                } else {
                    "200 OK"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                );
            }
        });
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "deterministic_child", "--nocapture"])
            .env("DETERMINISTIC_TEST_CHILD", scenario)
            .env("DETERMINISTIC_TEST_DIR", &dir)
            .env(
                "MOQUI_DETERMINISTIC",
                if scenario == "normal" { "false" } else { "true" },
            )
            .env(
                "MOQUI_DETERMINISTIC_OPERATIONS",
                if scenario == "operations" { "true" } else { "false" },
            )
            .env("MOQUI_DETERMINISTIC_OFFLINE", "true")
            .env("MOQUI_DETERMINISTIC_HOOK", format!("http://{address}/transaction"))
            .env("MOQUI_DETERMINISTIC_ENGINE", "rust")
            .env("MOQUI_DETERMINISTIC_RUN", "test-run")
            .env("MOQUI_DETERMINISTIC_LOG", &log);
        if scenario == "config" {
            child.env_remove("MOQUI_DETERMINISTIC_RUN");
        }
        if scenario == "timeout" {
            child.env("MOQUI_DETERMINISTIC_TIMEOUT_SECONDS", "1");
        }
        let output = child.output().unwrap();
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        assert!(
            output.status.success(),
            "{scenario}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let records: Vec<serde_json::Value> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let traces: Vec<_> = records.iter().filter(|record| record["event"] == "trace").collect();
        for (index, trace) in traces.iter().enumerate() {
            assert_eq!(trace["trace_sequence"], index + 1);
            assert!(!trace.to_string().contains("secret-parameter"));
        }
        if scenario == "lifecycle" {
            assert!(
                traces
                    .iter()
                    .any(|trace| trace["kind"] == "AT_SERVICE" && trace["outcome"] == "enter")
            );
            assert!(
                traces
                    .iter()
                    .any(|trace| trace["kind"] == "AT_SERVICE" && trace["outcome"] == "exit")
            );
        }
        if scenario == "operations" {
            let requests = requests.lock().unwrap();
            let operations: Vec<_> = requests
                .iter()
                .filter(|request| request["event"] == "operation")
                .collect();
            assert_eq!(operations.len(), 2);
            assert_eq!(operations[0]["name"], "moqui.example.Thing");
            assert_eq!(operations[0]["outcome"], "exit");
            assert_eq!(operations[0]["transaction_sequence"], 1);
        }
        if scenario == "postgres" {
            for sql in [
                "CREATE TABLE deterministic_trace (id text)",
                "INSERT INTO deterministic_trace VALUES (?)",
            ] {
                assert!(
                    traces.iter().any(|trace| trace["kind"] == "jdbc"
                        && trace["name"] == sql
                        && trace["outcome"] == "succeeded")
                );
            }
            assert!(
                traces
                    .iter()
                    .any(|trace| trace["kind"] == "jdbc" && trace["outcome"] == "failed")
            );
            assert!(
                traces
                    .iter()
                    .filter(|trace| trace["kind"] == "jdbc")
                    .all(|trace| trace["time_epoch_ms"].is_i64())
            );
        }
        let records: Vec<_> = records
            .into_iter()
            .filter(|record| record["event"] != "trace")
            .collect();
        assert_eq!(
            records[0],
            json!({"existing":true}),
            "append must preserve previous contents"
        );
        if scenario == "uncertain" {
            assert_eq!(
                records.len(),
                2,
                "an uncertain database commit must leave only the real begin, no fake completion"
            );
        }
        if scenario == "lifecycle" {
            let requests = requests.lock().unwrap();
            let mut logged_requests = records[1..].to_vec();
            for event in &mut logged_requests {
                if event["event"] == "begin" {
                    event.as_object_mut().unwrap().remove("operation_id");
                    event.as_object_mut().unwrap().remove("time_epoch_ms");
                }
            }
            assert_eq!(
                logged_requests, *requests,
                "local log must match actual acknowledged hook events"
            );
            let mut next = 1;
            let mut stack = Vec::new();
            let mut outcomes = Vec::new();
            let mut service_context = false;
            for record in &records[1..] {
                assert_eq!(record["version"], 1);
                assert_eq!(record["run_id"], "test-run");
                assert_eq!(record["engine"], "rust");
                if record["event"] == "begin" {
                    assert_eq!(record["sequence"], next);
                    assert_eq!(
                        record["parent_sequence"],
                        stack.last().copied().map_or(serde_json::Value::Null, |id| json!(id))
                    );
                    stack.push(next);
                    next += 1;
                    service_context |= record["artifact"]["name"] == "test#Lifecycle";
                    assert!(!record.to_string().contains("secret-parameter"));
                } else if record["event"] == "complete" {
                    assert_eq!(record["sequence"].as_u64(), stack.pop());
                    outcomes.push(record["outcome"].as_str().unwrap());
                } else {
                    assert_eq!(record["event"], "random");
                    assert!(record.get("bytes").is_none());
                }
            }
            assert!(stack.is_empty());
            assert!(service_context);
            assert!(
                outcomes.contains(&"committed")
                    && outcomes.contains(&"rolled_back")
                    && outcomes.contains(&"begin_failed")
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn deterministic_child() {
    let Ok(scenario) = std::env::var("DETERMINISTIC_TEST_CHILD") else {
        return;
    };
    if scenario == "config" {
        assert!(moqui_host::deterministic::init().is_err());
        return;
    }
    if scenario == "random-uninstalled" {
        assert!(moqui_lib::random::fill("uuid", &mut [0; 16]).is_err());
        assert!(moqui_core::user::random_code(6).is_err());
        assert!(moqui_core::user::hash_password("test-only").is_err());
        assert!(std::panic::catch_unwind(moqui_core::user::totp_secret).is_err());
        return;
    }
    moqui_host::deterministic::init().unwrap();
    if scenario == "normal" {
        assert!(!moqui_core::deterministic::enabled());
        let mut first = [0; 16];
        let mut second = [0; 16];
        moqui_lib::random::fill("normal", &mut first).unwrap();
        moqui_lib::random::fill("normal", &mut second).unwrap();
        assert_ne!(first, second);
        assert!(moqui_lib::clock::now_millis() > BASE);
        assert!(moqui_core::deterministic::begin().unwrap().is_none());
        return;
    }
    if scenario == "random-invalid" {
        assert!(moqui_lib::random::fill("uuid", &mut [0; 16]).is_err());
        assert!(moqui_lib::random::fill("uuid", &mut [0; 16]).is_err());
        assert!(moqui_core::deterministic::begin().is_err());
        return;
    }
    if scenario == "random" {
        let mut bytes = [0; 16];
        moqui_lib::random::fill("entity:moqui.server.Visitor", &mut bytes).unwrap();
        assert_eq!(bytes, std::array::from_fn::<_, 16, _>(|index| index as u8));
        assert_eq!(moqui_lib::random::below("factor-code:6", 999999).unwrap(), 66051);
        assert_eq!(moqui_core::user::random_string(12).unwrap(), "AAECAwQFBgcI");
        let expected = (0x0001_0203_0405_0607_u64 >> 11) as f64 / (1_u64 << 53) as f64;
        assert_eq!(moqui_lib::random::unit("unit").unwrap(), expected);
        return;
    }
    if ["status", "json", "assignment", "timeout"].contains(&scenario.as_str()) {
        let started = std::time::Instant::now();
        assert!(moqui_core::deterministic::begin().is_err());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "hook timeout must use bounded real time"
        );
        assert!(
            moqui_core::deterministic::begin().is_err(),
            "provider failure poisons later work"
        );
        assert!(std::panic::catch_unwind(moqui_lib::clock::now_millis).is_err());
        return;
    }
    if scenario == "complete" {
        assert!(
            moqui_core::deterministic::begin()
                .unwrap()
                .unwrap()
                .finish("committed")
                .is_err()
        );
        assert!(moqui_core::deterministic::begin().is_err());
        return;
    }
    assert!(std::panic::catch_unwind(moqui_lib::clock::now_millis).is_err());
    #[cfg(all(feature = "postgres", feature = "embedded-postgres"))]
    if scenario == "postgres" {
        let bootstrap = moqui_host::deterministic::bootstrap().unwrap().unwrap();
        let dir = std::path::PathBuf::from(std::env::var("DETERMINISTIC_TEST_DIR").unwrap());
        let (mut cluster, driver) = moqui_host::db::embedded::EmbeddedCluster::start(dir.join("pg"), 8).unwrap();
        let mut connection = driver.connect().unwrap();
        connection.begin().unwrap();
        connection
            .execute("CREATE TABLE deterministic_trace (id text)", &[])
            .unwrap();
        connection
            .execute(
                "INSERT INTO deterministic_trace VALUES (?)",
                &[Value::string("secret-parameter")],
            )
            .unwrap();
        connection.commit().unwrap();
        assert_eq!(
            connection
                .query("SELECT id FROM deterministic_trace", &[])
                .unwrap()
                .len(),
            1
        );
        assert!(connection.query_once("SELECT * FROM missing_trace_table", &[]).is_err());
        drop(connection);
        cluster.stop().unwrap();
        bootstrap.finish("committed").unwrap();
        return;
    }
    let fail = Arc::new(AtomicBool::new(false));
    let tx = TransactionManager::new(Arc::new(Driver(Arc::clone(&fail))), PoolConfig::default()).unwrap();
    if scenario == "uncertain" {
        let scope = tx.scope(None).unwrap();
        tx.with_connection(|conn| conn.execute("test", &[])).unwrap();
        assert!(scope.commit().is_err());
        assert!(moqui_core::deterministic::begin().is_err());
        assert!(std::panic::catch_unwind(moqui_lib::clock::now_millis).is_err());
        return;
    }
    let bootstrap = moqui_host::deterministic::bootstrap().unwrap().unwrap();
    if scenario == "operations" {
        moqui_core::deterministic::trace("AT_ENTITY", "list", "moqui.example.Thing", "exit").unwrap();
        moqui_core::deterministic::traced("jdbc", "executeQuery", "SELECT 1", || Ok(())).unwrap();
        bootstrap.finish("committed").unwrap();
        return;
    }
    let boot_time = moqui_lib::clock::now_millis();
    let outer = tx.scope(None).unwrap();
    let parent = moqui_lib::clock::now_millis();
    let join = tx.scope(None).unwrap();
    assert!(!join.began());
    assert_eq!(moqui_lib::clock::now_millis(), parent);
    join.commit().unwrap();
    tx.run(TransactionAttribute::ForceNew, None, || {
        assert!(moqui_lib::clock::now_millis() > parent);
        tx.with_connection(|conn| conn.execute("test", &[]))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(moqui_lib::clock::now_millis(), parent);
    tx.suspend().unwrap();
    assert!(std::panic::catch_unwind(moqui_lib::clock::now_millis).is_err());
    tx.resume().unwrap();
    assert_eq!(moqui_lib::clock::now_millis(), parent);
    outer.commit().unwrap();
    assert_eq!(moqui_lib::clock::now_millis(), boot_time);
    drop(tx.scope(None).unwrap());
    let rollback = tx.scope(None).unwrap();
    tx.set_rollback_only("test rollback only").unwrap();
    assert!(rollback.commit().is_err());
    fail.store(true, Ordering::SeqCst);
    let failed = tx.scope(None).unwrap();
    assert!(tx.with_connection(|conn| conn.execute("never", &[])).is_err());
    failed.rollback().unwrap();
    fail.store(false, Ordering::SeqCst);
    // Nontransactional connection work is an explicit independent transaction too.
    tx.with_connection(|conn| conn.execute("autocommit", &[])).unwrap();
    tx.with_reserve(|conn| conn.execute("reserve", &[])).unwrap();
    assert!(tx.with_connection(|conn| conn.query("SELECT now()", &[])).is_err());
    assert!(tx.with_connection(|conn| conn.begin()).is_err());
    bootstrap.finish("committed").unwrap();

    // Reentrant process lane serializes arbitrary application threads, not just worker counts.
    let active = Arc::new(AtomicUsize::new(0));
    let mut threads = Vec::new();
    for _ in 0..3 {
        let active = Arc::clone(&active);
        threads.push(std::thread::spawn(move || {
            let op = moqui_host::deterministic::bootstrap().unwrap().unwrap();
            assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
            std::thread::sleep(Duration::from_millis(10));
            assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
            op.finish("committed").unwrap();
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }

    let dir = std::path::PathBuf::from(std::env::var_os("DETERMINISTIC_TEST_DIR").unwrap());
    let framework = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let count = Arc::new(AtomicUsize::new(0));
    for _ in 0..2 {
        let factory = ExecutionContextFactory::boot_with_framework(&dir, &framework).unwrap();
        assert_eq!(factory.executor(), Executor::Inline);
        let facade = moqui_core::entity::EntityFacade::open_with_driver(
            &framework,
            Arc::new(Driver(Arc::clone(&fail))),
            PoolConfig::default(),
            "postgres",
            &["transactional", "logging"],
        )
        .unwrap();
        let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
        factory.set_entity(facade.clone());
        let called = Arc::clone(&count);
        factory
            .register_service("deterministic.marker", move |_, _, _| {
                called.fetch_add(1, Ordering::SeqCst);
                Ok(BTreeMap::new())
            })
            .unwrap();
        let service = dir.join("DeterministicServices.xml");
        std::fs::write(&service, r#"<services><service verb="mark" noun="Deterministic" type="rust" location="deterministic.marker" authenticate="false"/></services>"#).unwrap();
        let context = factory.new_context();
        let entered = context.enter();
        let _authz = context.disable_authz();
        context.push("AT_SERVICE", "test#Lifecycle", "AUTHZA_ALL");
        assert!(moqui_core::service::lint(&[service]).unwrap().is_empty());
        let scope = tx.scope(None).unwrap();
        let now = moqui_lib::clock::now_millis();
        assert_eq!(moqui_core::script::eval_value("Date.now()").unwrap(), Value::I64(now));
        assert!(moqui_core::script::eval_condition("new Date().getTime() === Date.now()").unwrap());
        let before = count.load(Ordering::SeqCst);
        moqui_core::service::async_call("mark#Deterministic", &BTreeMap::new()).unwrap();
        assert_eq!(factory.run_pending(), 0);
        assert_eq!(count.load(Ordering::SeqCst), before);
        assert!(moqui_core::service::call_future("mark#Deterministic", &BTreeMap::new()).is_err());
        let mut value = facade.make_value("moqui.test.TestEntity").unwrap();
        value.set("testId", Value::string("deterministic"));
        value.create(&facade).unwrap();
        scope.commit().unwrap();
        assert_eq!(factory.run_pending(), 1);
        assert_eq!(count.load(Ordering::SeqCst), before + 1);
        context.pop().unwrap();
        drop(entered);
        factory.shutdown();
    }
}

struct Driver(Arc<AtomicBool>);
struct Connection(Arc<AtomicBool>);
struct Cancel;
impl DbDriver for Driver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(Connection(Arc::clone(&self.0))))
    }
}
impl DbCancel for Cancel {
    fn cancel(&self) -> Result<(), Error> {
        Ok(())
    }
}
impl DbConnection for Connection {
    fn execute(&mut self, _sql: &str, params: &[Value]) -> Result<u64, Error> {
        let now = moqui_lib::clock::now_millis();
        assert!(now > BASE);
        for value in params {
            if let Value::Timestamp(stamp) = value {
                assert_eq!(
                    stamp.as_millisecond(),
                    now,
                    "persisted data stamps use transaction assignment"
                );
            }
        }
        Ok(1)
    }
    fn query(&mut self, _: &str, _: &[Value]) -> Result<Vec<DbRow>, Error> {
        Ok(Vec::new())
    }
    fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query(sql, params)
    }
    fn open_stream(&mut self, _: &str, _: &[Value]) -> Result<(), Error> {
        Ok(())
    }
    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        Ok(None)
    }
    fn close_stream(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn stream_open(&self) -> bool {
        false
    }
    fn begin(&mut self) -> Result<(), Error> {
        assert!(moqui_lib::clock::now_millis() > BASE);
        if self.0.load(Ordering::SeqCst) {
            Err(Error::new("injected begin failure"))
        } else {
            Ok(())
        }
    }
    fn commit(&mut self) -> Result<(), Error> {
        if std::env::var("DETERMINISTIC_TEST_CHILD").as_deref() == Ok("uncertain") {
            Err(Error::new("injected uncertain commit"))
        } else {
            Ok(())
        }
    }
    fn rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn savepoint(&mut self, _: &str) -> Result<(), Error> {
        Ok(())
    }
    fn rollback_to_savepoint(&mut self, _: &str) -> Result<(), Error> {
        Ok(())
    }
    fn release_savepoint(&mut self, _: &str) -> Result<(), Error> {
        Ok(())
    }
    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(Cancel)
    }
}
