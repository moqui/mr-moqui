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

//! Concurrent create-or-update of one primary key that is not there yet (Parity-Diffs, atomic create-or-update): each
//! call succeeds and there is one row, rather than the transactions that lost the race failing on the key, as the
//! parallel preview and thumbnail requests of a file did on its `ResourceRecent` row. Three paths are raced, on
//! embedded Postgres and (ignored, as it needs a server) on MySQL: the one-statement upsert a value takes where nothing
//! depends on whether it creates or updates, the UPDATE-first path (a value that sets `lastUpdatedStamp` is not one
//! upsert), and Java's read-first one (`create-or-update=true`).

#![cfg(any(all(unix, feature = "embedded-postgres"), feature = "mysql"))]

use std::sync::{Arc, Barrier};
use std::time::Duration;

use moqui_core::entity::EntityFacade;
use moqui_core::transaction::{PoolConfig, TransactionAttribute};
use moqui_lib::object::Value;

const ENTITY: &str = "moqui.basic.EnumerationType";
/// Racing rounds per mode; each starts with the row gone.
const ROUNDS: usize = 2;
/// The callers of a round.
const CALLERS: usize = 6;

#[cfg(all(unix, feature = "embedded-postgres"))]
#[test]
fn racing_create_or_update_of_a_missing_key_all_succeed() {
    use moqui_core::driver::DbDriver;
    use moqui_host::db::embedded::EmbeddedCluster;
    use moqui_host::db::entity_facade::open_entity_facade;

    if !crate::common::own_process() {
        return;
    }
    let _serial = crate::common::serial();
    let _properties = crate::common::PropertyReset::of(&["create-or-update"]);
    let dir = crate::common::temp_dir("moqui-cou-race");
    let _guard = crate::common::DirGuard(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let facade = open_entity_facade(&framework(), Arc::clone(&driver), pool()).unwrap();
    facade.check_tables(&[ENTITY]).unwrap();
    race_rounds(&facade, "");
    drop(facade);
    drop(cluster);
}

/// The same race on MySQL, which has no UPDATE probe (both modes look with a `SELECT` first) and inserts with
/// `ON DUPLICATE KEY UPDATE pk = pk`. First, on the server itself: that insert changes 0 rows when the key is there (the
/// driver does not set `CLIENT_FOUND_ROWS`), which is how a create-or-update knows to update instead.
#[cfg(feature = "mysql")]
#[test]
#[ignore = "needs a MySQL server: MOQUI_TEST_MYSQL=user:pass@host:port/database"]
fn racing_create_or_update_of_a_missing_key_all_succeed_on_mysql() {
    use moqui_core::driver::DbDriver;
    use moqui_host::db::entity_facade::open_entity_facade_dialect;

    if !crate::common::own_process() {
        return;
    }
    let _serial = crate::common::serial();
    let _properties = crate::common::PropertyReset::of(&["create-or-update"]);
    let spec = std::env::var("MOQUI_TEST_MYSQL").expect("MOQUI_TEST_MYSQL=user:pass@host:port/database");
    let (userinfo, rest) = spec.trim().rsplit_once('@').expect("user:pass@host:port/database");
    let (user, password) = userinfo.split_once(':').expect("user:pass");
    let (hostport, database) = rest.split_once('/').expect("host:port/database");
    let (host, port) = hostport.rsplit_once(':').expect("host:port");
    let driver = moqui_host::db::mysql::MysqlDriver::new(
        host.to_string(),
        port.parse().expect("a port number"),
        user.to_string(),
        password.to_string(),
        database.to_string(),
        false,
    );
    driver.connect_mysql().expect("MySQL rejected the connection");
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let facade = open_entity_facade_dialect(&framework(), Arc::clone(&driver), pool(), "mysql").unwrap();
    facade.check_tables(&[ENTITY]).unwrap();
    let clean = || {
        let mut conn = driver.connect().unwrap();
        conn.execute(
            "DELETE FROM `ENUMERATION_TYPE` WHERE `ENUM_TYPE_ID` LIKE 'RACE\\_%'",
            &[],
        )
        .unwrap();
    };
    clean();

    let mut conn = driver.connect().unwrap();
    let insert = "INSERT INTO `ENUMERATION_TYPE` (`ENUM_TYPE_ID`, `DESCRIPTION`) VALUES (?, ?) \
                  ON DUPLICATE KEY UPDATE `ENUM_TYPE_ID` = `ENUM_TYPE_ID`";
    let params = |description: &str| [Value::string("RACE_ROWS"), Value::string(description)];
    assert_eq!(
        conn.execute(insert, &params("first")).unwrap(),
        1,
        "a new key inserts one row"
    );
    assert_eq!(
        conn.execute(insert, &params("second")).unwrap(),
        0,
        "an existing key changes no row"
    );
    let rows = conn
        .query(
            "SELECT `DESCRIPTION` FROM `ENUMERATION_TYPE` WHERE `ENUM_TYPE_ID` = 'RACE_ROWS'",
            &[],
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    drop(conn);

    // `ON DUPLICATE KEY` matches any unique key, not just the primary one, so an entity with a unique index is not one
    // upsert on MySQL: a new key that conflicts on another unique index inserts nothing, the row is not there by its
    // key, and the write fails as that conflict, leaving the other row as it was.
    let resource = "moqui.resource.DbResource";
    facade.check_tables(&[resource]).unwrap();
    let clean_resources = || {
        let mut conn = driver.connect().unwrap();
        conn.execute("DELETE FROM `DB_RESOURCE` WHERE `RESOURCE_ID` LIKE 'RACE\\_%'", &[])
            .unwrap();
    };
    clean_resources();
    let named = |id: &str| {
        let mut row = facade.make_value(resource).unwrap();
        row.set("resourceId", Value::string(id));
        row.set("parentResourceId", Value::string("RACE_UK_PARENT"));
        row.set("filename", Value::string("same.txt"));
        row
    };
    named("RACE_UK_A").create(&facade).unwrap();
    let err = named("RACE_UK_B").create_or_update(&facade).unwrap_err().to_string();
    assert!(
        err.contains("another row has the same value of a unique index"),
        "{err}"
    );
    let named_rows = facade
        .find(resource)
        .unwrap()
        .eq("parentResourceId", Value::string("RACE_UK_PARENT"))
        .list()
        .unwrap();
    assert_eq!(named_rows.len(), 1);
    assert_eq!(
        named_rows.get(0).unwrap().get("resourceId").and_then(Value::as_str),
        Some("RACE_UK_A")
    );
    clean_resources();

    race_rounds(&facade, "mysql_");
    clean();
    drop(facade);
}

fn framework() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework")
}

fn pool() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: CALLERS as u32 + 2,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}

/// [`ROUNDS`] races on a missing key in each mode, each followed by a race on the row it left. `prefix` starts the keys.
fn race_rounds(facade: &EntityFacade, prefix: &str) {
    for mode in ["upsert", "update-first", "read-first"] {
        if mode == "read-first" {
            moqui_lib::env::set_property("create-or-update", "true".to_string());
        }
        let stamped = mode == "update-first";
        for round in 0..ROUNDS {
            let id = format!("RACE_{prefix}{mode}_{round}");
            race(facade, &id, stamped, |index| format!("caller {index}"))
                .unwrap_or_else(|err| panic!("{mode} round {round}: {err}"));
            let rows = facade
                .find(ENTITY)
                .unwrap()
                .eq("enumTypeId", Value::string(&id))
                .list()
                .unwrap();
            assert_eq!(rows.len(), 1, "{mode} round {round}: one row");
            let description = rows.get(0).unwrap().get("description").and_then(Value::as_str).unwrap();
            assert!(
                description.starts_with("caller "),
                "{mode} round {round}: {description}"
            );
            // A second race on the row that is there now updates it.
            race(facade, &id, stamped, |index| format!("again {index}"))
                .unwrap_or_else(|err| panic!("{mode} round {round}, existing row: {err}"));
            let rows = facade
                .find(ENTITY)
                .unwrap()
                .eq("enumTypeId", Value::string(&id))
                .list()
                .unwrap();
            assert_eq!(rows.len(), 1, "{mode} round {round}: still one row");
            let description = rows.get(0).unwrap().get("description").and_then(Value::as_str).unwrap();
            assert!(description.starts_with("again "), "{mode} round {round}: {description}");
        }
    }
}

/// [`CALLERS`] threads create-or-update `id` at once, each in its own transaction that stays open a moment after the
/// write, so every write is sent before the first commits; `stamped` values set `lastUpdatedStamp`. The first error any
/// of them got, with its caller.
fn race(facade: &EntityFacade, id: &str, stamped: bool, description: impl Fn(usize) -> String) -> Result<(), String> {
    let barrier = Arc::new(Barrier::new(CALLERS));
    let threads: Vec<_> = (0..CALLERS)
        .map(|index| {
            let facade = facade.clone();
            let barrier = Arc::clone(&barrier);
            let id = id.to_string();
            let description = description(index);
            std::thread::spawn(move || {
                let mut row = facade.make_value(ENTITY).unwrap();
                row.set("enumTypeId", Value::string(&id));
                row.set("description", Value::string(description));
                if stamped {
                    row.set("lastUpdatedStamp", Value::string("2026-10-30 12:00:00"));
                }
                let tx = facade.transactions_for(ENTITY).unwrap();
                barrier.wait();
                tx.run(TransactionAttribute::ForceNew, None, || {
                    row.create_or_update(&facade)?;
                    std::thread::sleep(Duration::from_millis(100));
                    Ok(())
                })
                .map_err(|err| format!("caller {index}: {err}"))
            })
        })
        .collect();
    let mut first = None;
    for thread in threads {
        if let Err(err) = thread.join().unwrap() {
            first.get_or_insert(err);
        }
    }
    first.map_or(Ok(()), Err)
}
