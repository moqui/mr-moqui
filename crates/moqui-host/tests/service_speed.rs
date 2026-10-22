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

//! Service facade timings and allocations: the service calls of `SpeedTest.xml` (the entity-auto CrUD of
//! `moqui.test.TestEntity` and `BasicServices.echo#Data`), called from Rust in an entered `ExecutionContext`, with the
//! same entity operations called directly in that context for comparison. No screen and no script.
//!
//! Out of the default `cargo test` link, and ignored so naming the binary does not start a postmaster.
//!
//! ```text
//! CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test service_speed -- --ignored --nocapture
//! ```
//!
//! `SERVICE_SPEED_CALLS` (default 1000) and `SERVICE_SPEED_BATCHES` (default 5) size it; see `speed/mod.rs` for the
//! columns. Each call builds its parameter map, as a caller does, so a few allocations per call are the test's.
//! `SPEED_PG_HOST` runs it on a new database of an external Postgres instead of the embedded one (`speed::SpeedDb`).
//!
//! Authorization is disabled, as `ec.artifactExecution.disableAuthz()`; artifact hits are counted. Two modes:
//!
//! - **one transaction** matches `SpeedTest.xml` (`begin-transaction="true"`): every call joins the batch's
//!   transaction, which commits once after the timed rows.
//! - **own transaction**: each service call begins and commits its own (`use-or-begin` with none in place), and each
//!   direct entity call autocommits.

#![cfg(all(unix, feature = "embedded-postgres"))]

mod speed;

use std::collections::BTreeMap;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::driver::DbDriver;
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::PoolConfig;
use moqui_host::db::entity_facade::{attach, open_entity_facade};
use moqui_lib::object::Value;

use speed::{env_usize, DirGuard, Rows};

const TEST: &str = "moqui.test.TestEntity";
const ENUM: &str = "moqui.basic.Enumeration";
const ECHO: &str = "org.moqui.impl.BasicServices.echo#Data";
const WARMUP: usize = 32;

#[derive(Clone, Copy)]
enum Mode {
    Transaction,
    OwnTransaction,
}

#[test]
#[ignore = "service timings: cargo test --release -p moqui-host --test service_speed -- --ignored --nocapture"]
fn service_facade_speed() {
    speed::warn_if_debug("service_speed");
    // Warnings and errors on the console, so a failing call says why.
    moqui_host::logging::install_with_console(Some(tracing::Level::WARN));
    let calls = env_usize("SERVICE_SPEED_CALLS", 1000);
    let batches = env_usize("SERVICE_SPEED_BATCHES", 5);

    let runtime = speed::temp_dir("service-speed-rt");
    let cluster_dir = speed::temp_dir("service-speed-pg");
    let _guards = (DirGuard(runtime.clone()), DirGuard(cluster_dir.clone()));
    std::fs::create_dir_all(&runtime).expect("runtime dir");
    let (cluster, driver) = speed::SpeedDb::start(&cluster_dir, 20);
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let factory = ExecutionContextFactory::boot_with_framework(&runtime, &framework).expect("boot");
    let facade = open_entity_facade(&framework, Arc::clone(&driver), pool()).expect("open entity facade");
    attach(&factory, facade.clone());
    // Every framework table: a service call reads the security entities (the tarpit, authorization) as well.
    facade.check_all_tables().expect("check tables");
    seed(&facade);

    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        println!();
        println!(
            "service facade, {calls} calls x {batches} batches, warmup {WARMUP}. \
             Pool is 1 ordinary + 1 reserve. ExecutionContext entered, authorization disabled."
        );
        run_mode(&facade, Mode::Transaction, calls, batches);
        run_mode(&facade, Mode::OwnTransaction, calls, batches);
    }

    drop(factory);
    drop(facade);
    drop(driver);
    drop(cluster);
}

fn run_mode(facade: &EntityFacade, mode: Mode, calls: usize, batches: usize) {
    let label = match mode {
        Mode::Transaction => "one transaction (commit is the last row, once for the whole batch)",
        Mode::OwnTransaction => "own transaction (each service call commits its own; each entity call autocommits)",
    };
    println!("\n== {label}");
    let mut rows = Rows::default();
    one_batch(facade, mode, WARMUP, &mut rows);
    rows.record = true;
    for batch in 1..=batches {
        eprintln!("  batch {batch}/{batches}");
        one_batch(facade, mode, calls, &mut rows);
    }
    rows.print();
}

fn one_batch(facade: &EntityFacade, mode: Mode, calls: usize, rows: &mut Rows) {
    let now = Value::Timestamp(jiff::Timestamp::now());
    if matches!(mode, Mode::Transaction) {
        facade.begin(TEST, None).expect("begin");
        let _ = facade.find(ENUM).expect("find").eq("enumId", Value::string("_NA_")).one();
    }

    rows.run("service echo#Data", calls, || {
        for _ in 0..calls {
            let out = call(
                ECHO,
                [
                    ("textIn1", Value::string("foo")),
                    ("numberIn", Value::string("12.34")),
                    ("timestampIn", now.clone()),
                ],
            );
            debug_assert_eq!(out.get("textOut2").and_then(Value::as_str), Some("ping"));
            black_box(out);
        }
    });
    let ids = rows.run("service create#TestEntity, sequenced id", calls, || {
        (0..calls)
            .map(|i| {
                let out = call(
                    "create#moqui.test.TestEntity",
                    [
                        ("testMedium", Value::string(format!("Medium {i}"))),
                        ("testLong", Value::string(format!("Long {i}"))),
                        ("testIndicator", Value::string("Y")),
                        ("testDateTime", now.clone()),
                        ("testNumberInteger", Value::I64(i64::try_from(i).unwrap_or(i64::MAX))),
                        ("testNumberDecimal", Value::string(format!("{i}.5"))),
                    ],
                );
                out.get("testId").cloned().unwrap_or_else(|| panic!("create returned no testId: {:?}", errors()))
            })
            .collect::<Vec<_>>()
    });
    rows.run("service update#TestEntity", ids.len(), || {
        for id in &ids {
            call(
                "update#moqui.test.TestEntity",
                [("testId", id.clone()), ("testMedium", Value::string("Updated Text"))],
            );
        }
    });
    rows.run("service delete#TestEntity", ids.len(), || {
        for id in &ids {
            call("delete#moqui.test.TestEntity", [("testId", id.clone())]);
        }
    });

    // The same in the context, without the service layer.
    rows.run("entity one PK Enumeration, cache, in context", calls, || {
        for _ in 0..calls {
            black_box(find_enum(facade, true));
        }
    });
    rows.run("entity one PK Enumeration, database, in context", calls, || {
        for _ in 0..calls {
            black_box(find_enum(facade, false));
        }
    });
    let mut values = rows.run("entity create TestEntity, in context", calls, || {
        (0..calls)
            .map(|i| {
                let mut value = facade.make_value(TEST).expect("make");
                value.set("testMedium", Value::string(format!("Medium {i}")));
                value.set("testLong", Value::string(format!("Long {i}")));
                value.set("testIndicator", Value::string("Y"));
                value.set("testDateTime", now.clone());
                value.set("testNumberInteger", Value::I64(i64::try_from(i).unwrap_or(i64::MAX)));
                value.set("testNumberDecimal", Value::string(format!("{i}.5")));
                value.set_sequenced_id_primary(facade).expect("sequenced id");
                value.create(facade).expect("create");
                value
            })
            .collect::<Vec<_>>()
    });
    rows.run("entity update TestEntity, in context", values.len(), || {
        for value in &mut values {
            value.set("testMedium", Value::string("Updated Text"));
            value.update(facade).expect("update");
        }
    });
    rows.run("entity delete TestEntity, in context", values.len(), || {
        for value in &values {
            value.delete(facade).expect("delete");
        }
    });

    if matches!(mode, Mode::Transaction) {
        rows.run("commit whole batch", 1, || facade.commit(TEST).expect("commit"));
    }
    let errors = errors();
    assert!(errors.is_empty(), "service errors: {errors:?}");
}

/// The message facade's errors, cleared.
fn errors() -> Vec<String> {
    ExecutionContext::with_current(|ctx| {
        let errors = ctx.message().errors().to_vec();
        ctx.message().clear_errors();
        errors
    })
    .unwrap_or_default()
}

/// `ec.service.sync().name(name).parameters(...).call()`. A failure is in the message facade, checked per batch.
fn call<const N: usize>(name: &str, parameters: [(&str, Value); N]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
    moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn find_enum(facade: &EntityFacade, cache: bool) -> bool {
    facade
        .find(ENUM)
        .expect("find")
        .eq("enumId", Value::string("_NA_"))
        .use_cache(cache)
        .one()
        .expect("one")
        .is_some()
}

fn seed(facade: &EntityFacade) {
    facade.begin(ENUM, None).expect("begin seed");
    let mut kind = facade.make_value("moqui.basic.EnumerationType").expect("type");
    kind.set("enumTypeId", Value::string("_NA_"));
    kind.set("description", Value::string("Not Applicable"));
    kind.create(facade).expect("create type");
    let mut row = facade.make_value(ENUM).expect("enum");
    row.set("enumId", Value::string("_NA_"));
    row.set("enumTypeId", Value::string("_NA_"));
    row.set("description", Value::string("Not Applicable"));
    row.create(facade).expect("create enum");
    facade.commit(ENUM).expect("commit seed");
}

fn pool() -> PoolConfig {
    PoolConfig {
        min: 1,
        max: 2,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}
