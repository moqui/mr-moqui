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

//! Entity facade timings and allocations, called directly. No screen, no QuickJS, no service layer.
//!
//! Out of the default `cargo test` link, and ignored so naming the binary does not start a postmaster.
//! Run it optimized. A dev-profile binary is a different program.
//!
//! ```text
//! CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test entity_speed -- --ignored --nocapture
//! ```
//!
//! `ENTITY_SPEED_CALLS` is the loop size (default 1000). `ENTITY_SPEED_BATCHES` is how many timed
//! batches to keep (default 5). `ENTITY_SPEED_THREADS` is the thread counts of the threads section (default
//! `1,2,4,8`), and `ENTITY_SPEED_THREADS_ONLY` runs that section alone. The printed per-call figures use the upper
//! median batch; see `speed/mod.rs` for the allocation columns. The rows a create keeps in the test's own list count as kept.
//! `SPEED_PG_HOST` runs it on a new database of an external Postgres instead of the embedded one (`speed::SpeedDb`).
//!
//! There is no `ExecutionContext`, so artifact authorization and hit counting do not run. `fsync` and
//! synchronous commit stay on. The pool is one ordinary connection and one reserve connection (sequence
//! refill). A connection returned within the last second is checked out again without `SELECT 1`.
//!
//! Two modes:
//!
//! - **one transaction** matches `SpeedTest.xml` (`begin-transaction="true"`): the batch holds one
//!   connection and commits once, after the timed rows. That commit is its own row, for the whole batch.
//! - **autocommit** is the durable rate: each call checks out a connection, runs its statement, and commits.

#![cfg(all(unix, feature = "embedded-postgres"))]

mod speed;

use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::driver::DbDriver;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::transaction::PoolConfig;
use moqui_host::db::entity_facade::open_entity_facade;
use moqui_lib::cache::{CacheConfig, MCache};
use moqui_lib::object::Value;

use speed::{env_usize, DirGuard, Rows};

const ENUM: &str = "moqui.basic.Enumeration";
const ENUM_TYPE: &str = "moqui.basic.EnumerationType";
const TEST: &str = "moqui.test.TestEntity";
const WARMUP: usize = 32;
/// Rows of `TestEntity` the list and iterator rows read, each call.
const LIST_ROWS: usize = 100;

#[derive(Clone, Copy)]
enum Mode {
    /// One frame for the batch. Commit is timed on its own, after the operations.
    Transaction,
    /// No frame. Each statement autocommits, including the pool checkout test.
    Autocommit,
}

#[test]
#[ignore = "entity timings: cargo test --release -p moqui-host --test entity_speed -- --ignored --nocapture"]
fn entity_facade_speed() {
    speed::warn_if_debug("entity_speed");
    let calls = env_usize("ENTITY_SPEED_CALLS", 1000);
    let batches = env_usize("ENTITY_SPEED_BATCHES", 5);

    let dir = speed::temp_dir("entity-speed");
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = speed::SpeedDb::start(&dir, 20);
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(&framework, Arc::clone(&driver), pool()).expect("open entity facade");
    facade
        .check_tables(&[ENUM_TYPE, ENUM, TEST, "moqui.entity.SequenceValueItem"])
        .expect("check tables");
    seed(&facade);
    let now = Value::Timestamp(jiff::Timestamp::now());
    let cache: MCache<String, String> = MCache::new("entity-speed", CacheConfig::default());

    println!();
    println!(
        "entity facade, {calls} calls x {batches} batches, warmup {WARMUP}. \
         Pool is 1 ordinary + 1 reserve. No ExecutionContext. Lists read {LIST_ROWS} rows, {} calls.",
        list_calls(calls)
    );
    if std::env::var_os("ENTITY_SPEED_THREADS_ONLY").is_none() {
        run_mode(&facade, &cache, &now, Mode::Transaction, calls, batches);
        run_mode(&facade, &cache, &now, Mode::Autocommit, calls, batches);
    }
    drop(facade);

    let threads = thread_counts();
    let most = threads.iter().copied().max().unwrap_or(1);
    let facade = open_entity_facade(&framework, Arc::clone(&driver), thread_pool(most)).expect("open entity facade");
    run_threads(&facade, &now, &threads, calls, batches);

    drop(facade);
    drop(cluster);
}

fn run_mode(
    facade: &EntityFacade,
    cache: &MCache<String, String>,
    now: &Value,
    mode: Mode,
    calls: usize,
    batches: usize,
) {
    let label = match mode {
        Mode::Transaction => "one transaction (commit is the last row, once for the whole batch)",
        Mode::Autocommit => "autocommit (each call checks out and commits; a just-returned connection skips SELECT 1)",
    };
    println!("\n== {label}");
    let mut rows = Rows::default();
    one_batch(facade, cache, now, mode, WARMUP, 0, &mut rows);
    rows.record = true;
    for batch in 1..=batches {
        eprintln!("  batch {batch}/{batches}");
        one_batch(facade, cache, now, mode, calls, batch, &mut rows);
    }
    rows.print();
}

/// The facade shared by several threads at once, each with a connection of its own: the cache path (no statement),
/// a PK find in a transaction, and writes in a transaction. A row's us/call is the run's time over every thread's
/// calls, so a row that scales keeps it falling as threads are added; calls/s is the total rate.
fn run_threads(facade: &EntityFacade, now: &Value, threads: &[usize], calls: usize, batches: usize) {
    println!(
        "\n== threads ({} threads at once, {calls} calls each; us/call is the run's time over all their calls)",
        threads.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    );
    let mut rows = Rows::default();
    for batch in 0..=batches {
        rows.record = batch > 0;
        if batch > 0 {
            eprintln!("  batch {batch}/{batches}");
        }
        let calls = if batch == 0 { WARMUP } else { calls };
        for &count in threads {
            rows.run_threads(&format!("{count} threads: one PK Enumeration, entity cache"), count, calls, |_| {
                for _ in 0..calls {
                    black_box(find_one(facade, "_NA_", true));
                }
            });
            rows.run_threads(
                &format!("{count} threads: one PK Enumeration, database, one tx"),
                count,
                calls,
                |_| {
                    facade.begin(ENUM, None).expect("begin");
                    for _ in 0..calls {
                        black_box(find_one(facade, "_NA_", false));
                    }
                    facade.commit(ENUM).expect("commit");
                },
            );
            rows.run_threads(
                &format!("{count} threads: create, update, delete TestEntity, one tx"),
                count,
                calls,
                |thread| {
                    facade.begin(TEST, None).expect("begin");
                    let mut made: Vec<EntityValue> = (0..calls / 3)
                        .map(|i| {
                            let mut value = facade.make_value(TEST).expect("make TestEntity");
                            value.set("testId", Value::string(format!("t{count}-{thread}-{batch}-{i}")));
                            fill(&mut value, i, now);
                            value.create(facade).expect("create");
                            value
                        })
                        .collect();
                    for value in &mut made {
                        value.set("testMedium", Value::string("Updated"));
                        value.update(facade).expect("update");
                    }
                    for value in &made {
                        value.delete(facade).expect("delete");
                    }
                    facade.commit(TEST).expect("commit");
                },
            );
        }
    }
    rows.print();
}

/// `ENTITY_SPEED_THREADS`, a comma-separated list (default `1,2,4,8`).
fn thread_counts() -> Vec<usize> {
    std::env::var("ENTITY_SPEED_THREADS")
        .ok()
        .map(|text| text.split(',').filter_map(|part| part.trim().parse().ok()).filter(|n| *n > 0).collect())
        .filter(|counts: &Vec<usize>| !counts.is_empty())
        .unwrap_or_else(|| vec![1, 2, 4, 8])
}

/// A connection for each of `threads`, and the reserve.
fn thread_pool(threads: usize) -> PoolConfig {
    PoolConfig {
        min: 1,
        max: u32::try_from(threads).unwrap_or(u32::MAX).saturating_add(1),
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}

fn list_calls(calls: usize) -> usize {
    (calls / 10).max(1)
}

fn one_batch(
    facade: &EntityFacade,
    cache: &MCache<String, String>,
    now: &Value,
    mode: Mode,
    calls: usize,
    batch: usize,
    rows: &mut Rows,
) {
    if matches!(mode, Mode::Transaction) {
        facade.begin(TEST, None).expect("begin");
        // Enlist and BEGIN before the timer, so the first timed call is not the checkout.
        let _ = find_one(facade, "_NA_", false);
    }

    rows.run("build Enumeration find", calls, || {
        for i in 0..calls {
            let find = facade
                .find(ENUM)
                .expect("find")
                .eq("enumId", Value::string(format!("_NA_{i}")));
            black_box(find);
        }
    });

    let keys: Vec<String> = (0..calls).map(|i| format!("speed-{batch}-{i}")).collect();
    rows.run("MCache put", calls, || {
        for key in &keys {
            cache.put(key.clone(), "_NA_".to_string()).expect("cache put");
        }
    });
    rows.run("MCache get", calls, || {
        for key in &keys {
            black_box(cache.get(key).expect("cache get"));
        }
    });

    rows.run("one PK Enumeration, entity cache", calls, || {
        for _ in 0..calls {
            black_box(find_one(facade, "_NA_", true));
        }
    });
    rows.run("list Enumeration by type, entity cache", calls, || {
        for _ in 0..calls {
            black_box(find_list(facade, true));
        }
    });
    rows.run("one PK Enumeration, database", calls, || {
        for _ in 0..calls {
            black_box(find_one(facade, "_NA_", false));
        }
    });
    rows.run("list Enumeration by type, database", calls, || {
        for _ in 0..calls {
            black_box(find_list(facade, false));
        }
    });
    rows.run("count Enumeration by type, database", calls, || {
        for _ in 0..calls {
            let count = facade
                .find(ENUM)
                .expect("count find")
                .eq("enumTypeId", Value::string("EntitySyncArtifactAppl"))
                .use_cache(false)
                .count()
                .expect("count");
            assert_eq!(count, 2);
        }
    });
    let lists = list_calls(calls);
    rows.run(&format!("list {LIST_ROWS} TestEntity rows, database"), lists, || {
        for _ in 0..lists {
            let list = list_test_rows(facade).list().expect("list rows");
            assert_eq!(list.len(), LIST_ROWS);
            black_box(list);
        }
    });
    // A cursor needs a transaction, as in Java.
    if matches!(mode, Mode::Transaction) {
        rows.run(&format!("iterate {LIST_ROWS} TestEntity rows, database"), lists, || {
            for _ in 0..lists {
                let mut iterator = list_test_rows(facade).iterator().expect("iterator");
                let mut seen = 0;
                while let Some(value) = iterator.next_value().expect("next value") {
                    black_box(value);
                    seen += 1;
                }
                iterator.close().expect("close iterator");
                assert_eq!(seen, LIST_ROWS);
            }
        });
    }

    let mut sequenced = rows.run("create TestEntity, sequenced id", calls, || {
        (0..calls).map(|i| create_sequenced(facade, i, now)).collect::<Vec<_>>()
    });
    let mut preset = rows.run("create TestEntity, preset id", calls, || {
        (0..calls).map(|i| create_preset(facade, batch, i, now)).collect::<Vec<_>>()
    });
    let written = sequenced.len() + preset.len();
    rows.run("update TestEntity", written, || {
        for value in sequenced.iter_mut().chain(preset.iter_mut()) {
            value.set("testMedium", Value::string("Updated"));
            value.update(facade).expect("update");
        }
    });
    rows.run("update TestEntity, nothing changed", written, || {
        for value in sequenced.iter().chain(preset.iter()) {
            value.update(facade).expect("update unchanged");
        }
    });
    rows.run("create_or_update TestEntity, existing", calls, || {
        for value in &mut sequenced {
            value.set("testLong", Value::string("Stored"));
            value.create_or_update(facade).expect("create or update");
        }
    });
    rows.run("delete TestEntity", written, || {
        for value in sequenced.iter().chain(preset.iter()) {
            value.delete(facade).expect("delete");
        }
    });

    if matches!(mode, Mode::Transaction) {
        rows.run("commit whole batch", 1, || {
            facade.commit(TEST).expect("commit");
        });
    }
}

fn find_one(facade: &EntityFacade, id: &str, cache: bool) -> EntityValue {
    facade
        .find(ENUM)
        .expect("enumeration find")
        .eq("enumId", Value::string(id))
        .use_cache(cache)
        .one()
        .expect("enumeration one")
        .unwrap_or_else(|| panic!("no Enumeration {id}"))
}

fn find_list(facade: &EntityFacade, cache: bool) -> usize {
    let list = facade
        .find(ENUM)
        .expect("enumeration list")
        .eq("enumTypeId", Value::string("EntitySyncArtifactAppl"))
        .use_cache(cache)
        .list()
        .expect("enumeration list");
    assert_eq!(list.len(), 2, "EntitySyncArtifactAppl rows");
    list.len()
}

fn list_test_rows(facade: &EntityFacade) -> moqui_core::entity::EntityFind {
    facade
        .find(TEST)
        .expect("test list")
        .eq("testIndicator", Value::string("L"))
        .use_cache(false)
}

fn create_sequenced(facade: &EntityFacade, index: usize, now: &Value) -> EntityValue {
    let mut value = facade.make_value(TEST).expect("make TestEntity");
    fill(&mut value, index, now);
    value.set_sequenced_id_primary(facade).expect("sequenced id");
    value.create(facade).expect("create sequenced");
    value
}

fn create_preset(facade: &EntityFacade, batch: usize, index: usize, now: &Value) -> EntityValue {
    let mut value = facade.make_value(TEST).expect("make TestEntity");
    value.set("testId", Value::string(format!("s{batch}-{index}")));
    fill(&mut value, index, now);
    value.create(facade).expect("create preset");
    value
}

fn fill(value: &mut EntityValue, index: usize, now: &Value) {
    value.set("testMedium", Value::string(format!("Medium {index}")));
    value.set("testLong", Value::string(format!("Long {index}")));
    value.set("testIndicator", Value::string("Y"));
    value.set("testDateTime", now.clone());
    value.set(
        "testNumberInteger",
        Value::I64(i64::try_from(index).unwrap_or(i64::MAX)),
    );
    value.set("testNumberDecimal", Value::string(format!("{index}.5")));
}

fn seed(facade: &EntityFacade) {
    facade.begin(ENUM, None).expect("begin seed");
    enum_type(facade, "_NA_", "Not Applicable");
    enum_type(
        facade,
        "EntitySyncArtifactAppl",
        "Entity Sync Artifact Application Type",
    );
    enumeration(facade, "_NA_", "_NA_", "Not Applicable");
    enumeration(facade, "EsaaInclude", "EntitySyncArtifactAppl", "Include");
    enumeration(facade, "EsaaExclude", "EntitySyncArtifactAppl", "Exclude");
    let now = Value::Timestamp(jiff::Timestamp::now());
    for index in 0..LIST_ROWS {
        let mut row = facade.make_value(TEST).expect("make list row");
        row.set("testId", Value::string(format!("list-{index}")));
        fill(&mut row, index, &now);
        row.set("testIndicator", Value::string("L"));
        row.create(facade).expect("create list row");
    }
    facade.commit(ENUM).expect("commit seed");
    let _ = find_one(facade, "_NA_", false);
    assert_eq!(find_list(facade, false), 2);
}

fn enum_type(facade: &EntityFacade, id: &str, description: &str) {
    let mut row = facade.make_value(ENUM_TYPE).expect("make EnumerationType");
    row.set("enumTypeId", Value::string(id));
    row.set("description", Value::string(description));
    row.create(facade).expect("create EnumerationType");
}

fn enumeration(facade: &EntityFacade, id: &str, type_id: &str, description: &str) {
    let mut row = facade.make_value(ENUM).expect("make Enumeration");
    row.set("enumId", Value::string(id));
    row.set("enumTypeId", Value::string(type_id));
    row.set("description", Value::string(description));
    row.create(facade).expect("create Enumeration");
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
