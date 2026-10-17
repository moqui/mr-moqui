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

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_db::dialect::Dialects;
use moqui_db::model::EntityModel;
use moqui_db::sql::{select_view, LimitOffset};
use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::crypto::CryptConfig;
use super::{EntityFacade, EntityGroup};
use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};
use crate::transaction::{PoolConfig, TransactionManager};

fn framework_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework")
}

struct FakeDriver;

struct FakeConn;

struct NoCancel;

impl DbCancel for NoCancel {
    fn cancel(&self) -> Result<(), Error> {
        Ok(())
    }
}

impl DbConnection for FakeConn {
    fn execute(&mut self, _sql: &str, _params: &[Value]) -> Result<u64, Error> {
        Ok(0)
    }

    fn query(&mut self, _sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        Ok(Vec::new())
    }

    fn query_once(&mut self, _sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        Ok(Vec::new())
    }

    fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), Error> {
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
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

impl DbDriver for FakeDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(FakeConn))
    }
}

struct Tape {
    sql: Vec<String>,
    password: Option<String>,
}

struct IterTape {
    opened: bool,
    served: bool,
}

struct IterDriver {
    tape: Arc<Mutex<IterTape>>,
}

struct IterConn {
    tape: Arc<Mutex<IterTape>>,
}

fn test_row() -> DbRow {
    DbRow {
        columns: vec![
            ("testId".to_string(), Value::string("T1")),
            ("testMedium".to_string(), Value::string("kept")),
        ],
    }
}

impl DbConnection for IterConn {
    fn execute(&mut self, _sql: &str, _params: &[Value]) -> Result<u64, Error> {
        Ok(1)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query_once(sql, params)
    }

    fn query_once(&mut self, sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        if sql.to_ascii_lowercase().contains("test_entity") && sql.contains("SELECT") {
            return Ok(vec![test_row()]);
        }
        Ok(Vec::new())
    }

    fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), Error> {
        let mut tape = self.tape.lock().unwrap_or_else(|err| err.into_inner());
        tape.opened = true;
        tape.served = false;
        Ok(())
    }

    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        let mut tape = self.tape.lock().unwrap_or_else(|err| err.into_inner());
        if !tape.opened || tape.served {
            return Ok(None);
        }
        tape.served = true;
        Ok(Some(test_row()))
    }

    fn close_stream(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn stream_open(&self) -> bool {
        self.tape.lock().unwrap_or_else(|err| err.into_inner()).opened
    }

    fn begin(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

impl DbDriver for IterDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(IterConn {
            tape: Arc::clone(&self.tape),
        }))
    }
}

struct RecDriver {
    tape: Arc<Mutex<Tape>>,
}

struct RecConn {
    tape: Arc<Mutex<Tape>>,
}

impl DbConnection for RecConn {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, Error> {
        let mut tape = self.tape.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(stored) = params.iter().find_map(|value| {
            value
                .as_str()
                .filter(|text| text.starts_with(':') && *text != "TestEntityFieldValue")
                .map(str::to_string)
        }) {
            tape.password = Some(stored);
        }
        tape.sql.push(sql.to_string());
        Ok(1)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query_once(sql, params)
    }

    fn query_once(&mut self, sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        let tape = self.tape.lock().unwrap_or_else(|err| err.into_inner());
        if sql.to_ascii_lowercase().contains("instance_host") && sql.contains("SELECT") {
            if let Some(password) = tape.password.clone() {
                return Ok(vec![DbRow {
                    columns: vec![
                        ("instanceHostId".to_string(), Value::string("H1")),
                        ("password".to_string(), Value::string(password)),
                    ],
                }]);
            }
        }
        Ok(Vec::new())
    }

    fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), Error> {
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
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

impl DbDriver for RecDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(RecConn {
            tape: Arc::clone(&self.tape),
        }))
    }
}

fn open_recording(tape: Arc<Mutex<Tape>>, dialect: &str) -> EntityFacade {
    let driver: Arc<dyn DbDriver> = Arc::new(RecDriver { tape });
    let transactions = TransactionManager::new(
        driver,
        PoolConfig {
            min: 0,
            max: 2,
            reserve: 1,
            acquire_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .unwrap();
    EntityFacade::open(
        &framework_dir(),
        "transactional",
        vec![EntityGroup::new("transactional", dialect, transactions)],
    )
    .unwrap()
}

#[test]
fn enter_sim_starts_the_overlay_once_and_stays_on_this_thread() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::cache::{enter_sim, exit_sim, sim_entered};

    let facade = open_fake();
    let err = enter_sim(&facade).unwrap_err();
    assert!(err.to_string().contains("not configured"), "{err}");
    assert!(!facade.sim_active());

    let starts = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&starts);
    facade.set_sim_starter(Some(Box::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        TransactionManager::new(
            Arc::new(FakeDriver),
            PoolConfig {
                min: 0,
                max: 2,
                reserve: 0,
                acquire_timeout: Duration::from_secs(5),
                ..Default::default()
            },
        )
    })));
    enter_sim(&facade).unwrap();
    enter_sim(&facade).unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(facade.sim_active() && sim_entered());
    let other = facade.clone();
    let seen = std::thread::spawn(move || other.sim_active()).join().unwrap();
    assert!(!seen, "another thread is not in this thread's sim");
    exit_sim(&facade).unwrap();
    assert!(facade.sim_active(), "the outer schema is still open");
    exit_sim(&facade).unwrap();
    assert!(!facade.sim_active() && !sim_entered());
    exit_sim(&facade).unwrap();
}

fn open_fake() -> EntityFacade {
    let driver: Arc<dyn DbDriver> = Arc::new(FakeDriver);
    let transactions = TransactionManager::new(
        driver,
        PoolConfig {
            min: 0,
            max: 2,
            reserve: 1,
            acquire_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .unwrap();
    EntityFacade::open(
        &framework_dir(),
        "transactional",
        vec![EntityGroup::new("transactional", "postgres", transactions)],
    )
    .unwrap()
}

#[test]
fn locale_defaults_to_en_and_crypt_stays_unset() {
    let facade = open_fake();
    assert_eq!(facade.locale(), "en");
    assert!(facade.crypt().is_none());
    facade.set_locale("fr");
    assert_eq!(facade.locale(), "fr");
    assert!(!facade.sim_active());
    facade.set_sim_active(true);
    assert!(facade.sim_active());
    assert!(facade.sim_transactions().is_none());
    facade.set_sim_transactions(None);
    facade.with_slot(
        || String::from("seed"),
        |slot| {
            assert_eq!(slot, "seed");
            slot.push_str("-kept");
        },
    );
    facade.with_slot(|| String::from("other"), |slot| assert_eq!(slot, "seed-kept"));
    let primary = CryptConfig {
        pass: "MoquiDefaultPassword:CHANGEME".to_string(),
        salt: "20201202".to_string(),
        iterations: 10,
        algorithm: "PBEWithHmacSHA256AndAES_128".to_string(),
    };
    facade.set_crypt(primary.clone(), Vec::new());
    let (stored, alts) = facade.crypt().unwrap();
    assert_eq!(stored.pass, primary.pass);
    assert!(alts.is_empty());
}

#[test]
fn try_insert_seals_the_password_before_sql() {
    let tape = Arc::new(Mutex::new(Tape {
        sql: Vec::new(),
        password: None,
    }));
    let facade = open_recording(Arc::clone(&tape), "mysql");
    facade.set_crypt(
        CryptConfig {
            pass: "MoquiDefaultPassword:CHANGEME".to_string(),
            salt: "20201202".to_string(),
            iterations: 10,
            algorithm: "PBEWithHmacSHA256AndAES_128".to_string(),
        },
        Vec::new(),
    );
    facade.set_disable_fk_create(true);
    facade.set_disable_data_feed(true);
    let mut fields = BTreeMap::new();
    fields.insert("instanceHostId".to_string(), Value::string("H1"));
    fields.insert("password".to_string(), Value::string("TestEntityFieldValue"));
    facade
        .upsert_record("moqui.server.instance.InstanceHost", &fields)
        .unwrap();
    fields.insert("instanceHostId".to_string(), Value::string("H2"));
    facade.begin("moqui.server.instance.InstanceHost", None).unwrap();
    facade
        .upsert_record("moqui.server.instance.InstanceHost", &fields)
        .unwrap();
    facade.commit("moqui.server.instance.InstanceHost").unwrap();
    let recorded = tape.lock().unwrap_or_else(|err| err.into_inner());
    let upserts: Vec<_> = recorded
        .sql
        .iter()
        .filter(|sql| sql.contains("ON DUPLICATE KEY UPDATE"))
        .collect();
    assert_eq!(upserts.len(), 2, "{:?}", recorded.sql);
    let stored = recorded.password.clone().unwrap_or_default();
    assert!(stored.starts_with(':'), "{stored}");
    assert_ne!(stored, "TestEntityFieldValue");
    drop(recorded);
    let found = facade
        .find("moqui.server.instance.InstanceHost")
        .unwrap()
        .eq("instanceHostId", Value::string("H1"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(
        found.get("password").and_then(Value::as_str),
        Some("TestEntityFieldValue")
    );
}

#[test]
fn iterator_streams_committed_rows_and_hides_a_held_delete() {
    let tape = Arc::new(Mutex::new(IterTape {
        opened: false,
        served: false,
    }));
    let driver: Arc<dyn DbDriver> = Arc::new(IterDriver {
        tape: Arc::clone(&tape),
    });
    let transactions = TransactionManager::new(
        driver,
        PoolConfig {
            min: 0,
            max: 2,
            reserve: 1,
            acquire_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .unwrap();
    let facade = EntityFacade::open(
        &framework_dir(),
        "transactional",
        vec![EntityGroup::new("transactional", "postgres", transactions)],
    )
    .unwrap();

    facade.begin("moqui.test.TestEntity", None).unwrap();
    let mut iter = facade.find("moqui.test.TestEntity").unwrap().iterator().unwrap();
    let row = iter.next_value().unwrap().unwrap();
    assert_eq!(row.get("testId").and_then(Value::as_str), Some("T1"));
    assert!(iter.next_value().unwrap().is_none());
    drop(iter);
    assert!(tape.lock().unwrap_or_else(|err| err.into_inner()).opened);
    facade.rollback("moqui.test.TestEntity").unwrap();

    {
        let mut tape = tape.lock().unwrap_or_else(|err| err.into_inner());
        tape.opened = false;
        tape.served = false;
    }
    // The transaction cache is on only for the cache attribute.
    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    let outcome = tx.run(
        crate::transaction::TransactionAttribute::Cache,
        None,
        || -> Result<(), Error> {
            let mut doomed = facade.make_value("moqui.test.TestEntity").unwrap();
            doomed.set("testId", Value::string("T1"));
            doomed.delete(&facade).unwrap();
            let mut held = facade.find("moqui.test.TestEntity").unwrap().iterator().unwrap();
            assert!(held.next_value().unwrap().is_none());
            drop(held);
            assert!(!tape.lock().unwrap_or_else(|err| err.into_inner()).opened);
            Err(Error::new("roll back"))
        },
    );
    assert!(outcome.is_err());
}

#[test]
fn entity_rest_serves_the_verbs() {
    use std::rc::Rc;

    struct Guard(PathBuf);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn value_map(entries: Vec<(&str, Value)>) -> Value {
        let stack = moqui_lib::context_stack::ContextStack::new();
        let frame = stack.shared_map();
        {
            let mut map = frame.borrow_mut();
            for (key, value) in entries {
                map.insert(key.to_string(), value);
            }
        }
        drop(stack);
        Value::Map(match Rc::try_unwrap(frame) {
            Ok(cell) => cell.into_inner(),
            Err(frame) => frame.borrow().clone(),
        })
    }

    fn params(entries: Vec<(&str, Value)>) -> BTreeMap<String, Value> {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }

    fn map_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
        let Value::Map(map) = value else {
            return None;
        };
        map.get(key).and_then(Value::as_str)
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("moqui-writer-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    let facade = open_fake();
    // Writes run the entity-auto services, which need the entered context's service registry.
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();

    let err = facade.rest("", "moqui.test.TestEntity", &BTreeMap::new()).unwrap_err();
    assert!(err.to_string().contains("must be specified"), "{err}");
    let err = facade
        .rest("nope", "moqui.test.TestEntity", &BTreeMap::new())
        .unwrap_err();
    assert!(err.to_string().contains("not supported"), "{err}");

    match facade.rest("get", "moqui.test.TestEntity", &BTreeMap::new()).unwrap() {
        Value::List(items) => assert!(items.is_empty()),
        other => panic!("find without a primary key returned {other:?}"),
    }

    let created = facade
        .rest(
            "post",
            "moqui.test.TestEntity",
            &params(vec![
                ("testId", Value::string("C1")),
                ("testMedium", Value::string("made")),
            ]),
        )
        .unwrap();
    assert_eq!(map_str(&created, "testId"), Some("C1"));
    // a primary key default is evaluated, not copied as written: `'ALL_USERS'` loses its quotes
    let fk = super::fk::FkScope::enter(true, false);
    let with_default = facade
        .rest(
            "post",
            "moqui.screen.SubscreensItem",
            &params(vec![
                ("screenLocation", Value::string("component://x/screen/x.xml")),
                ("subscreenName", Value::string("sub")),
            ]),
        )
        .unwrap();
    drop(fk);
    assert_eq!(map_str(&with_default, "userGroupId"), Some("ALL_USERS"));
    let err = facade
        .rest(
            "find",
            "moqui.test.TestEntity",
            &params(vec![("testId", Value::string("C1"))]),
        )
        .unwrap_err();
    assert!(err.to_string().contains("No value found"), "{err}");

    // As Java's update, one with no field to set writes nothing, primary key or not; one with a field and no key fails.
    facade.rest("patch", "moqui.test.TestEntity", &BTreeMap::new()).unwrap();
    let err = facade
        .rest(
            "patch",
            "moqui.test.TestEntity",
            &params(vec![("testMedium", Value::string("keyless"))]),
        )
        .unwrap_err();
    // The error stays in the message facade, where it would stop the next service call, as in Java; a request starts
    // with none.
    assert!(err.to_string().contains("testId is required"), "{err}");
    context.message().clear_errors();
    let updated = facade
        .rest(
            "update",
            "moqui.test.TestEntity",
            &params(vec![
                ("testId", Value::string("C1")),
                ("testMedium", Value::string("updated")),
            ]),
        )
        .unwrap();
    assert_eq!(map_str(&updated, "testId"), Some("C1"));
    let stored = facade
        .rest(
            "put",
            "moqui.test.TestEntity",
            &params(vec![
                ("testId", Value::string("S1")),
                ("testMedium", Value::string("stored")),
            ]),
        )
        .unwrap();
    assert_eq!(map_str(&stored, "testId"), Some("S1"));

    let err = facade
        .rest("delete", "moqui.test.TestEntity", &BTreeMap::new())
        .unwrap_err();
    assert!(err.to_string().contains("Must specify all primary key fields"), "{err}");
    context.message().clear_errors();
    let deleted = facade
        .rest(
            "delete",
            "moqui.test.TestEntity",
            &params(vec![("testId", Value::string("C1"))]),
        )
        .unwrap();
    let Value::Map(deleted_map) = deleted else {
        panic!("delete returned {deleted:?}");
    };
    assert!(deleted_map.is_empty(), "{deleted_map:?}");

    let listed = facade
        .rest(
            "find",
            "moqui.basic.Enumeration",
            &params(vec![(
                "path",
                Value::List(vec![Value::string("E1"), Value::string("type")]),
            )]),
        )
        .unwrap();
    assert!(matches!(listed, Value::List(_)), "{listed:?}");
    let err = facade
        .rest("find", "moqui.basic.Enumeration/E1/type/ET1", &BTreeMap::new())
        .unwrap_err();
    assert!(err.to_string().contains("No value found"), "{err}");
    let err = facade
        .rest(
            "find",
            "moqui.basic.Enumeration",
            &params(vec![(
                "path",
                Value::List(vec![
                    Value::string("E1"),
                    Value::string("type"),
                    Value::string("ET1"),
                    Value::string("more"),
                ]),
            )]),
        )
        .unwrap_err();
    // As Java, each element after a relationship's keys names another relationship.
    assert!(
        err.to_string()
            .contains("No relationship found with name or alias [more]"),
        "{err}"
    );
    let err = facade
        .rest(
            "find",
            "moqui.basic.Enumeration",
            &params(vec![(
                "path",
                Value::List(vec![Value::string("E1"), Value::string("missing")]),
            )]),
        )
        .unwrap_err();
    assert!(err.to_string().contains("No relationship found"), "{err}");
    let err = facade
        .rest(
            "find",
            "moqui.basic.Enumeration",
            &params(vec![
                ("masterNameInPath", Value::Bool(true)),
                ("master", Value::string("missing")),
            ]),
        )
        .unwrap_err();
    assert!(err.to_string().contains("Master definition not found"), "{err}");

    let err = facade.dynamic_view("moqui.test.Foo", &BTreeMap::new()).unwrap_err();
    assert!(err.to_string().contains("member-entity"), "{err}");
    let named = facade.dynamic_view("moqui.test.FooBar", &BTreeMap::new()).unwrap();
    assert!(named.is_empty());
    let built = facade
        .dynamic_view(
            "dynamic.FooJoin",
            &params(vec![
                (
                    "member-entity",
                    Value::List(vec![
                        value_map(vec![
                            ("entity-alias", Value::string("FOO")),
                            ("entity-name", Value::string("moqui.test.Foo")),
                        ]),
                        value_map(vec![
                            ("entity-alias", Value::string("BAR")),
                            ("entity-name", Value::string("moqui.test.Bar")),
                            ("join-from-alias", Value::string("FOO")),
                            ("key-map", value_map(vec![("fooId", Value::string("fooId"))])),
                        ]),
                    ]),
                ),
                (
                    "alias-all",
                    value_map(vec![
                        ("entity-alias", Value::string("FOO")),
                        ("exclude", Value::string("fooId")),
                    ]),
                ),
                (
                    "alias",
                    value_map(vec![
                        ("entity-alias", Value::string("BAR")),
                        ("name", Value::string("barRank")),
                        ("field", Value::string("barRank")),
                    ]),
                ),
                ("condition", value_map(vec![("FOO.fooId", Value::string("F1"))])),
                ("order-by", Value::string("-fooText")),
                ("limit", Value::I64(5)),
            ]),
        )
        .unwrap();
    assert!(built.is_empty());
    let related = facade
        .dynamic_view(
            "dynamic.EnumType",
            &params(vec![
                (
                    "member-entity",
                    value_map(vec![
                        ("entity-alias", Value::string("ENUM")),
                        ("entity-name", Value::string("moqui.basic.Enumeration")),
                    ]),
                ),
                (
                    "member-relationship",
                    value_map(vec![
                        ("entity-alias", Value::string("ETP")),
                        ("join-from-alias", Value::string("ENUM")),
                        ("relationship", Value::string("type")),
                    ]),
                ),
                (
                    "alias",
                    Value::List(vec![
                        value_map(vec![
                            ("entity-alias", Value::string("ENUM")),
                            ("name", Value::string("enumId")),
                        ]),
                        value_map(vec![
                            ("entity-alias", Value::string("ETP")),
                            ("name", Value::string("enumTypeId")),
                        ]),
                    ]),
                ),
            ]),
        )
        .unwrap();
    assert!(related.is_empty());

    let err = facade.write_data("moqui.test.Foo", &[]).unwrap_err();
    assert!(
        err.to_string().contains("directory") || err.to_string().contains("file"),
        "{err}"
    );
    let export = temp_dir("all");
    let _export_guard = Guard(export.clone());
    let medium = "a,b & <c>";
    let mut record = BTreeMap::new();
    record.insert("testId".to_string(), Value::string("W1"));
    record.insert("testMedium".to_string(), Value::string(medium));
    record.insert("testLong".to_string(), Value::string("line1\nline2"));
    record.insert("directory".to_string(), Value::string(export.display().to_string()));
    record.insert("prefix".to_string(), Value::string("Pre"));
    facade.write_data("moqui.test.TestEntity", &[record]).unwrap();
    let xml = std::fs::read_to_string(export.join("moqui.test.TestEntity.xml")).unwrap();
    assert!(
        xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<entity-facade-xml>\n"),
        "{xml}"
    );
    assert!(xml.contains("<Premoqui.test.TestEntity "), "{xml}");
    assert!(xml.contains("testId=\"W1\""), "{xml}");
    assert!(xml.contains("testMedium=\"a,b &amp; &lt;c&gt;\""), "{xml}");
    assert!(xml.contains("<testLong><![CDATA[line1\nline2]]></testLong>"), "{xml}");
    assert!(xml.contains("</moqui.test.TestEntity>\n"), "{xml}");
    assert!(xml.contains("</entity-facade-xml>\n"), "{xml}");
    let json = std::fs::read_to_string(export.join("moqui.test.TestEntity.json")).unwrap();
    assert!(json.starts_with("[\n"), "{json}");
    assert!(json.contains("\"testId\": \"W1\""), "{json}");
    assert!(json.contains("\"testMedium\": \"a,b & <c>\""), "{json}");
    assert!(json.contains("\"_entity\": \"moqui.test.TestEntity\""), "{json}");
    assert!(json.ends_with("]\n\n"), "{json}");
    let csv = std::fs::read_to_string(export.join("moqui.test.TestEntity.csv")).unwrap();
    assert!(csv.starts_with("moqui.test.TestEntity,export,testId\n"), "{csv}");
    assert!(csv.contains("\"a,b & <c>\""), "{csv}");
    let err = facade
        .write_data(
            "moqui.test.TestEntity",
            &[params(vec![("directory", Value::string(export.display().to_string()))])],
        )
        .unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");

    let one = temp_dir("one");
    let _one_guard = Guard(one.clone());
    facade
        .write_data(
            "moqui.test.TestEntity",
            &[params(vec![
                ("testId", Value::string("J1")),
                ("directory", Value::string(one.display().to_string())),
                ("fileType", Value::string("JSON")),
            ])],
        )
        .unwrap();
    assert!(one.join("moqui.test.TestEntity.json").is_file());
    assert!(!one.join("moqui.test.TestEntity.xml").exists());
    assert!(!one.join("moqui.test.TestEntity.csv").exists());

    let csv_file = one.join("out.csv");
    facade
        .write_data(
            "moqui.test.Foo",
            &[params(vec![
                ("fooId", Value::string("F9")),
                ("fooText", Value::string("file")),
                ("file", Value::string(csv_file.display().to_string())),
            ])],
        )
        .unwrap();
    let foo_csv = std::fs::read_to_string(&csv_file).unwrap();
    assert!(foo_csv.starts_with("moqui.test.Foo,export,fooId\n"), "{foo_csv}");
    assert!(foo_csv.contains("file"), "{foo_csv}");

    let err = facade
        .write_data(
            "moqui.test.Foo",
            &[params(vec![
                ("directory", Value::string(export.display().to_string())),
                ("fileType", Value::string("yaml")),
            ])],
        )
        .unwrap_err();
    assert!(err.to_string().contains("fileType"), "{err}");
    let not_dir = one.join("notdir");
    std::fs::write(&not_dir, "x").unwrap();
    let err = facade
        .write_data(
            "moqui.test.Foo",
            &[params(vec![(
                "directory",
                Value::string(not_dir.display().to_string()),
            )])],
        )
        .unwrap_err();
    assert!(err.to_string().contains("not a directory"), "{err}");
}

#[test]
fn foobar_select_view_joins_member_entities() {
    let framework = framework_dir();
    let conf = MNode::parse_file(framework.join("conf/MoquiDefaultConf.xml")).unwrap();
    let model = EntityModel::load_directory(&conf, &framework.join("entity")).unwrap();
    let dialects = Dialects::load(&conf).unwrap();
    let dialect = dialects.get("postgres").unwrap();
    let view = model.entity("moqui.test.FooBar").unwrap();
    let sql = select_view(
        view,
        &model,
        dialect,
        &super::find::FindValues,
        None,
        &[],
        LimitOffset::default(),
    )
    .unwrap()
    .sql;
    assert!(sql.contains("JOIN"), "{sql}");
    assert!(sql.contains("\"foo\""), "{sql}");
    assert!(sql.contains("\"bar\""), "{sql}");
    assert!(!sql.contains("member-relationship"), "{sql}");
}

/// What the SQL runner asked of the connection: rows taken from the cursor, and whether it began, closed, or read
/// a query whole.
#[derive(Default)]
struct CursorTape {
    fetched: usize,
    begun: bool,
    closed: bool,
    read_whole: bool,
}

struct CursorDriver {
    tape: Arc<Mutex<CursorTape>>,
}

/// Serves 100 rows through the cursor.
struct CursorConn {
    tape: Arc<Mutex<CursorTape>>,
}

impl DbConnection for CursorConn {
    fn execute(&mut self, _sql: &str, _params: &[Value]) -> Result<u64, Error> {
        Ok(0)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query_once(sql, params)
    }

    fn query_once(&mut self, sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        // SELECT 1 is the pool's check of a new connection.
        if sql != "SELECT 1" {
            self.tape.lock().unwrap_or_else(|err| err.into_inner()).read_whole = true;
        }
        Ok(Vec::new())
    }

    fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), Error> {
        Ok(())
    }

    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        let mut tape = self.tape.lock().unwrap_or_else(|err| err.into_inner());
        if tape.fetched >= 100 {
            return Ok(None);
        }
        tape.fetched += 1;
        let number = i64::try_from(tape.fetched).unwrap_or_default();
        Ok(Some(DbRow {
            columns: vec![("n".to_string(), Value::I64(number))],
        }))
    }

    fn close_stream(&mut self) -> Result<(), Error> {
        self.tape.lock().unwrap_or_else(|err| err.into_inner()).closed = true;
        Ok(())
    }

    fn stream_open(&self) -> bool {
        false
    }

    fn begin(&mut self) -> Result<(), Error> {
        self.tape.lock().unwrap_or_else(|err| err.into_inner()).begun = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

impl DbDriver for CursorDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(CursorConn {
            tape: Arc::clone(&self.tape),
        }))
    }
}

#[test]
fn the_sql_runner_reads_a_query_through_a_cursor_to_one_row_past_the_limit() {
    let tape = Arc::new(Mutex::new(CursorTape::default()));
    let driver: Arc<dyn DbDriver> = Arc::new(CursorDriver {
        tape: Arc::clone(&tape),
    });
    let transactions = TransactionManager::new(
        driver,
        PoolConfig {
            min: 0,
            max: 2,
            reserve: 1,
            acquire_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .unwrap();
    let facade = EntityFacade::open(
        &framework_dir(),
        "transactional",
        vec![EntityGroup::new("transactional", "postgres", transactions)],
    )
    .unwrap();

    let result = super::info::run_sql(&facade, "transactional", "select n from numbers;", 5).unwrap();
    assert_eq!(result.columns, vec!["n".to_string()]);
    assert_eq!(result.records.len(), 5);
    assert_eq!(result.records[4], vec![Value::I64(5)]);
    assert!(result.limit_reached);
    {
        let mut tape = tape.lock().unwrap_or_else(|err| err.into_inner());
        assert_eq!(tape.fetched, 6, "the cursor is read to one row past the limit");
        assert!(tape.begun && tape.closed && !tape.read_whole);
        *tape = CursorTape::default();
    }

    // At or under the limit, every row and no limit reached.
    let result = super::info::run_sql(&facade, "transactional", "SELECT n FROM numbers", 0).unwrap();
    assert_eq!(result.records.len(), 100);
    assert!(!result.limit_reached);
    tape.lock().unwrap_or_else(|err| err.into_inner()).fetched = 0;
    let result = super::info::run_sql(&facade, "transactional", "select n from numbers", 100).unwrap();
    assert_eq!(result.records.len(), 100);
    assert!(!result.limit_reached);

    // A SHOW is read whole: a Postgres cursor does not take one.
    let result = super::info::run_sql(&facade, "transactional", "show search_path", 5).unwrap();
    assert!(result.records.is_empty());
    assert!(tape.lock().unwrap_or_else(|err| err.into_inner()).read_whole);
}

fn test_entity_row(facade: &EntityFacade, test_id: &str) -> Option<super::EntityValue> {
    facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string(test_id))
        .one()
        .unwrap()
}

#[test]
fn xml_cdata_text_with_a_section_end_loads_back_unchanged() {
    let facade = crate::testing::open();
    let text = "first line\n]]></testLong></moqui.test.TestEntity><moqui.test.TestEntity testId=\"INJECTED\"/>\
                <moqui.test.TestEntity testId=\"X\"><testLong><![CDATA[tail ]]>";
    let mut fields = BTreeMap::new();
    fields.insert("testId".to_string(), Value::string("CD1"));
    fields.insert("testLong".to_string(), Value::string(text));
    let element = super::query_rest::xml_element(&facade, "moqui.test.TestEntity", &fields, 0).unwrap();
    assert!(element.contains("]]]]><![CDATA[>"), "{element}");
    let document = format!("<entity-facade-xml>\n{element}</entity-facade-xml>\n");
    let loaded =
        super::load_feed::load_entity_xml_text(&facade, "cdata", &document, &super::load_feed::LoadOptions::default())
            .unwrap();
    assert_eq!(loaded.records, 1, "{document}");
    let row = test_entity_row(&facade, "CD1").expect("CD1 loaded");
    assert_eq!(row.get("testLong"), Some(&Value::string(text)));
    assert!(test_entity_row(&facade, "INJECTED").is_none());
    assert!(test_entity_row(&facade, "X").is_none());
}

#[test]
fn a_failed_record_load_inside_an_outer_transaction_leaves_it_to_the_owner() {
    let facade = crate::testing::open();
    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    assert!(tx.begin(None).unwrap());
    let mut outer = facade.make_value("moqui.test.TestEntity").unwrap();
    outer.set("testId", Value::string("OUTER"));
    outer.create(&facade).unwrap();
    // The first record stores; the second has no full primary key, an error before any statement for it runs.
    let document = r#"<entity-facade-xml>
        <moqui.test.TestEntity testId="GOOD"/>
        <moqui.basic.LocalizedEntityField entityName="moqui.test.TestEntity" localized="no key"/>
    </entity-facade-xml>"#;
    let err = super::load_feed::load_entity_xml_text(
        &facade,
        "missing-pk",
        document,
        &super::load_feed::LoadOptions::default(),
    )
    .unwrap_err();
    assert!(tx.in_transaction(), "the owner's frame is still in place: {err}");
    let commit = tx.commit().expect_err("the failed load marks the frame rollback-only");
    assert!(commit.to_string().contains("rollback-only"), "{commit}");
    assert!(!tx.in_transaction(), "no frame is left after the owner ends it");
    assert!(test_entity_row(&facade, "OUTER").is_none());
    assert!(test_entity_row(&facade, "GOOD").is_none());
}

/// A store of one row of `TestEntity`: `UPDATE` reports the row when present, `INSERT` adds it. Records the SQL.
#[derive(Default)]
struct StoreTape {
    sql: Vec<String>,
    params: Vec<Vec<Value>>,
    exists: bool,
}

struct StoreDriver {
    tape: Arc<Mutex<StoreTape>>,
}

struct StoreConn {
    inner: FakeConn,
    tape: Arc<Mutex<StoreTape>>,
}

impl DbDriver for StoreDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(StoreConn {
            inner: FakeConn,
            tape: Arc::clone(&self.tape),
        }))
    }
}

impl DbConnection for StoreConn {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, Error> {
        let mut tape = self.tape.lock().unwrap_or_else(|err| err.into_inner());
        tape.sql.push(sql.to_string());
        tape.params.push(params.to_vec());
        if sql.starts_with("UPDATE") {
            return Ok(u64::from(tape.exists));
        }
        if sql.starts_with("INSERT") {
            tape.exists = true;
        }
        Ok(1)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query_once(sql, params)
    }

    fn query_once(&mut self, sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        let mut tape = self.tape.lock().unwrap_or_else(|err| err.into_inner());
        if sql == "SELECT 1" {
            return Ok(Vec::new());
        }
        tape.sql.push(sql.to_string());
        tape.params.push(Vec::new());
        if tape.exists && sql.to_ascii_lowercase().contains("test_entity") && sql.contains("SELECT") {
            return Ok(vec![test_row()]);
        }
        Ok(Vec::new())
    }

    fn open_stream(&mut self, sql: &str, params: &[Value]) -> Result<(), Error> {
        self.inner.open_stream(sql, params)
    }
    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        self.inner.next_row()
    }
    fn close_stream(&mut self) -> Result<(), Error> {
        self.inner.close_stream()
    }
    fn stream_open(&self) -> bool {
        false
    }
    fn begin(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

fn open_store(exists: bool, dialect: &str) -> (EntityFacade, Arc<Mutex<StoreTape>>) {
    let tape = Arc::new(Mutex::new(StoreTape {
        exists,
        ..Default::default()
    }));
    let driver: Arc<dyn DbDriver> = Arc::new(StoreDriver {
        tape: Arc::clone(&tape),
    });
    let transactions = TransactionManager::new(
        driver,
        PoolConfig {
            min: 0,
            max: 2,
            reserve: 1,
            acquire_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .unwrap();
    let facade = EntityFacade::open(
        &framework_dir(),
        "transactional",
        vec![EntityGroup::new("transactional", dialect, transactions)],
    )
    .unwrap();
    facade.set_disable_fk_create(true);
    facade.set_disable_data_feed(true);
    (facade, tape)
}

fn writes(tape: &Arc<Mutex<StoreTape>>) -> Vec<(String, usize)> {
    let tape = tape.lock().unwrap_or_else(|err| err.into_inner());
    tape.sql
        .iter()
        .zip(&tape.params)
        .map(|(sql, params)| (sql.split(' ').next().unwrap_or("").to_string(), params.len()))
        .collect()
}

fn test_value(facade: &EntityFacade) -> super::EntityValue {
    let mut value = facade.make_value("moqui.test.TestEntity").unwrap();
    value.set("testId", Value::string("T1"));
    value.set("testMedium", Value::string("changed"));
    value
}

#[test]
fn create_or_update_on_an_existing_row_is_one_update_with_the_same_sql_as_update() {
    let (facade, tape) = open_store(true, "postgres");
    facade.update_value(&test_value(&facade)).unwrap();
    let direct = tape.lock().unwrap().sql.clone();
    assert_eq!(direct.len(), 1, "{direct:?}");
    tape.lock().unwrap().sql.clear();
    tape.lock().unwrap().params.clear();

    facade.create_or_update_value(&test_value(&facade)).unwrap();
    // Before: SELECT then UPDATE. Now: the UPDATE alone, the same statement.
    assert_eq!(tape.lock().unwrap().sql, direct);
}

#[test]
fn create_or_update_on_a_new_row_updates_nothing_then_finds_and_inserts_as_before() {
    let (facade, tape) = open_store(false, "postgres");
    facade.create_value(&test_value(&facade)).unwrap();
    let direct_insert = tape.lock().unwrap().sql.clone();
    assert_eq!(direct_insert.len(), 1);
    {
        let mut tape = tape.lock().unwrap();
        tape.exists = false;
        tape.sql.clear();
        tape.params.clear();
    }
    facade.create_or_update_value(&test_value(&facade)).unwrap();
    let kinds: Vec<String> = writes(&tape).into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(kinds, ["UPDATE", "SELECT", "INSERT"], "{kinds:?}");
    let tape = tape.lock().unwrap();
    assert_eq!(tape.sql[2], direct_insert[0]);
    assert!(tape.exists);
}

#[test]
fn create_or_update_keeps_find_then_write_for_dialects_that_do_not_report_matched_rows() {
    let (facade, tape) = open_store(true, "mysql");
    facade.create_or_update_value(&test_value(&facade)).unwrap();
    let kinds: Vec<String> = writes(&tape).into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(kinds, ["SELECT", "UPDATE"], "{kinds:?}");
}

#[test]
fn an_update_eca_rule_keeps_create_or_update_on_the_find_then_write_path() {
    let (facade, tape) = open_store(true, "postgres");
    super::load_feed::add_eca_rules(
        &facade,
        r#"<eecas><eeca entity="moqui.test.TestEntity" on-update="true"><actions><log message="x"/></actions></eeca></eecas>"#,
    )
    .unwrap();
    facade.create_or_update_value(&test_value(&facade)).unwrap();
    let kinds: Vec<String> = writes(&tape).into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(kinds, ["SELECT", "UPDATE"], "{kinds:?}");
}

#[test]
fn plain_map_loader_preserves_status_validation_on_updates() {
    let facade = crate::testing::open();
    facade.set_disable_fk_create(true);
    facade.set_disable_audit_log(true);
    let load = |status| {
        super::load_feed::load_entity_xml_text(
            &facade,
            "status-loader",
            &format!(
                r#"<entity-facade-xml><moqui.service.message.SystemMessage
                systemMessageId="M1" statusId="{status}"/></entity-facade-xml>"#
            ),
            &super::load_feed::LoadOptions::default(),
        )
    };
    load("First").unwrap();
    let error = load("Second").unwrap_err();
    assert!(error.to_string().contains("Status change not allowed"), "{error}");
    let mut transition = facade.make_value("moqui.basic.StatusFlowTransition").unwrap();
    transition.set("statusFlowId", Value::string("Default"));
    transition.set("statusId", Value::string("First"));
    transition.set("toStatusId", Value::string("Second"));
    transition.create(&facade).unwrap();
    load("Second").unwrap();
    let row = facade
        .find("moqui.service.message.SystemMessage")
        .unwrap()
        .list()
        .unwrap()
        .values()[0]
        .clone();
    assert_eq!(row.get("statusId"), Some(&Value::string("Second")));
}

#[test]
fn create_or_update_applies_primary_key_defaults_before_refreshing() {
    let facade = crate::testing::open();
    facade.set_disable_fk_create(true);
    let entity = "moqui.screen.SubscreensItem";
    let mut value = facade.make_value(entity).unwrap();
    value.set("screenLocation", Value::string("test.xml"));
    value.set("subscreenName", Value::string("test"));
    value.set("menuTitle", Value::string("first"));
    value.create_or_update(&facade).unwrap();
    value.set("menuTitle", Value::string("second"));
    value.create_or_update(&facade).unwrap();
    let rows = facade.find(entity).unwrap().list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows.get(0).unwrap().get("userGroupId"),
        Some(&Value::string("ALL_USERS"))
    );
    assert_eq!(rows.get(0).unwrap().get("menuTitle"), Some(&Value::string("second")));
}

#[test]
fn field_defaults_apply_on_create_only_through_create_or_update() {
    for exists in [false, true] {
        let facade = crate::testing::open();
        facade.set_disable_fk_create(true);
        let entity = "moqui.entity.document.DataDocumentLink";
        let mut value = facade.make_value(entity).unwrap();
        value.set("dataDocumentId", Value::string("D1"));
        value.set("linkSeqId", Value::string("01"));
        value.set("label", Value::string("l"));
        if exists {
            let mut original = value.clone();
            original.set("urlType", Value::string("custom"));
            original.create(&facade).unwrap();
        }
        value.create_or_update(&facade).unwrap();
        value.refresh(&facade).unwrap();
        // A new row gets 'plain'; an update must not reset an existing column to its default.
        assert_eq!(
            value.get("urlType"),
            Some(&Value::string(if exists { "custom" } else { "plain" }))
        );
    }
}

/// As Java's `EntityFacadeImpl.rest`: identity-admin and secret-config entities are not readable, `UserAccount` is not
/// writable, and a `UserAccount` read leaves out the password fields.
#[test]
fn generic_entity_rest_keeps_sensitive_entities_and_fields_out() {
    let facade = crate::testing::open();
    let mut account = facade.make_value("moqui.security.UserAccount").unwrap();
    account.set("userId", Value::string("U1"));
    account.set("username", Value::string("u1"));
    account.set("currentPassword", Value::string("hash"));
    account.set("passwordSalt", Value::string("salt"));
    account.create(&facade).unwrap();
    let unauthorized = |err: &Error| crate::context::ErrorKind::of(err) == crate::context::ErrorKind::Unauthorized;

    for entity in ["moqui.security.UserLoginKey", "moqui.basic.email.EmailServer"] {
        let err = facade.rest("get", entity, &BTreeMap::new()).unwrap_err();
        assert!(unauthorized(&err), "{entity}: {err}");
    }
    let mut params = BTreeMap::new();
    params.insert("userId".to_string(), Value::string("U2"));
    let err = facade.rest("post", "moqui.security.UserAccount", &params).unwrap_err();
    assert!(unauthorized(&err), "{err}");

    let Value::List(rows) = facade
        .rest("get", "moqui.security.UserAccount", &BTreeMap::new())
        .unwrap()
    else {
        panic!("a list");
    };
    let Value::Map(row) = &rows[0] else { panic!("a row") };
    assert_eq!(row.get("userId").and_then(Value::as_str), Some("U1"));
    assert!(
        !row.contains_key("currentPassword") && !row.contains_key("passwordSalt"),
        "{row:?}"
    );
}

/// As Java's `EntityAutoServiceRunner.deleteEntity`, which entity REST deletes go through: every primary key field
/// is needed, and `*` in one or more of them deletes each record matching the others.
#[test]
fn entity_rest_delete_needs_every_key_and_takes_a_wildcard() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let entity = "moqui.basic.StatusFlowTransition";
    for (flow, from, to) in [("F1", "A", "B"), ("F1", "A", "C"), ("F2", "A", "B")] {
        let mut row = facade.make_value(entity).unwrap();
        row.set("statusFlowId", Value::string(flow));
        row.set("statusId", Value::string(from));
        row.set("toStatusId", Value::string(to));
        row.create(&facade).unwrap();
    }
    let keys = |pairs: &[(&str, &str)]| -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), Value::string(*value)))
            .collect()
    };
    let err = facade
        .rest("delete", entity, &keys(&[("statusFlowId", "F1"), ("statusId", "*")]))
        .unwrap_err();
    assert!(err.to_string().contains("Must specify all primary key fields"), "{err}");
    context.message().clear_errors();
    assert_eq!(facade.find(entity).unwrap().count().unwrap(), 3);
    facade
        .rest(
            "delete",
            entity,
            &keys(&[("statusFlowId", "F1"), ("statusId", "*"), ("toStatusId", "*")]),
        )
        .unwrap();
    let left = facade.find(entity).unwrap().list().unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left.values()[0].get("statusFlowId").and_then(Value::as_str), Some("F2"));
}

/// As Java's `getPlainValueMap`, a REST find of a view-entity gives its alias values, and the primary key aliases
/// (here `enumId`, from the path or the parameters) find one row.
#[test]
fn entity_rest_find_of_a_view_gives_its_aliases_and_finds_one_by_its_key() {
    let facade = crate::testing::open();
    let mut kind = facade.make_value("moqui.basic.EnumerationType").unwrap();
    kind.set("enumTypeId", Value::string("T1"));
    kind.set("description", Value::string("Type One"));
    kind.create(&facade).unwrap();
    for enum_id in ["E1", "E2"] {
        let mut row = facade.make_value("moqui.basic.Enumeration").unwrap();
        row.set("enumId", Value::string(enum_id));
        row.set("enumTypeId", Value::string("T1"));
        row.create(&facade).unwrap();
    }
    let view = "moqui.basic.EnumerationAndType";
    let field = |row: &Value, name: &str| match row {
        Value::Map(map) => map.get(name).and_then(Value::as_str).map(str::to_string),
        other => panic!("not a row: {other:?}"),
    };

    let mut by_type = BTreeMap::new();
    by_type.insert("enumTypeId".to_string(), Value::string("T1"));
    let Value::List(rows) = facade.rest("get", view, &by_type).unwrap() else {
        panic!("a list");
    };
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(field(&rows[0], "typeDescription").as_deref(), Some("Type One"));
    assert_eq!(field(&rows[0], "enumTypeId").as_deref(), Some("T1"));
    assert_eq!(field(&rows[0], "_entity").as_deref(), Some(view));

    let one = facade.rest("get", &format!("{view}/E2"), &BTreeMap::new()).unwrap();
    assert_eq!(field(&one, "enumId").as_deref(), Some("E2"));
    assert_eq!(field(&one, "typeDescription").as_deref(), Some("Type One"));
    let mut missing = BTreeMap::new();
    missing.insert("enumId".to_string(), Value::string("E9"));
    let err = facade.rest("get", view, &missing).unwrap_err();
    assert!(err.to_string().contains("No value found"), "{err}");
}

/// As Java's `oneInternal`: a find one with no condition, or with a primary key field null or empty, is null.
#[test]
fn find_one_with_no_condition_or_an_empty_key_is_none() {
    let facade = crate::testing::open();
    let entity = "moqui.basic.Enumeration";
    let mut row = facade.make_value(entity).unwrap();
    row.set("enumId", Value::string("E1"));
    row.set("description", Value::string("one"));
    row.create(&facade).unwrap();
    assert!(facade.find(entity).unwrap().one().unwrap().is_none());
    assert!(facade
        .find(entity)
        .unwrap()
        .eq("enumId", Value::Null)
        .one()
        .unwrap()
        .is_none());
    assert!(facade
        .find(entity)
        .unwrap()
        .eq("enumId", Value::string(""))
        .eq("description", Value::string("one"))
        .one()
        .unwrap()
        .is_none());
    assert!(facade
        .find(entity)
        .unwrap()
        .eq("enumId", Value::string("E1"))
        .one()
        .unwrap()
        .is_some());
    assert!(facade
        .find(entity)
        .unwrap()
        .eq("description", Value::string("one"))
        .one()
        .unwrap()
        .is_some());
}

/// Java's `updateAll` and `deleteAll`: row by row through the value path, the rows that change counted, and not
/// for a create-only entity. An update that would change a create-only field fails; one that keeps it passes.
#[test]
fn update_all_and_delete_all_go_row_by_row_and_create_only_is_kept() {
    let facade = crate::testing::open();
    let entity = "moqui.basic.StatusFlowTransition";
    for to in ["B", "C"] {
        let mut row = facade.make_value(entity).unwrap();
        row.set("statusFlowId", Value::string("F1"));
        row.set("statusId", Value::string("A"));
        row.set("toStatusId", Value::string(to));
        row.create(&facade).unwrap();
    }
    let set = || vec![("transitionName".to_string(), Value::string("Go"))];
    let find = || facade.find(entity).unwrap().eq("statusFlowId", Value::string("F1"));
    assert_eq!(find().update_all(set()).unwrap(), 2);
    assert_eq!(find().update_all(set()).unwrap(), 0);
    assert_eq!(find().eq("transitionName", Value::string("Go")).count().unwrap(), 2);
    assert_eq!(find().delete_all().unwrap(), 2);
    assert_eq!(find().count().unwrap(), 0);

    let audit = "moqui.entity.EntityAuditLog";
    let mut log = facade.make_value(audit).unwrap();
    log.set("auditHistorySeqId", Value::string("A1"));
    log.set("changedFieldName", Value::string("statusId"));
    log.create(&facade).unwrap();
    log.update(&facade).unwrap();
    log.set("changedFieldName", Value::string("other"));
    let err = log.update(&facade).unwrap_err();
    assert!(
        err.to_string()
            .contains("create-only (immutable) fields [changedFieldName]"),
        "{err}"
    );
    let err = log.delete(&facade).unwrap_err();
    assert!(err.to_string().contains("create-only"), "{err}");
    let all = || facade.find(audit).unwrap().eq("auditHistorySeqId", Value::string("A1"));
    assert!(all()
        .update_all(vec![("changeReason".to_string(), Value::string("x"))])
        .is_err());
    assert!(all().delete_all().is_err());
}

/// Java's localized field get: the user's locale, then its language, then `default`, in `LocalizedEntityField`, and
/// then the same chain in `LocalizedMessage` for the stored text.
#[test]
fn localized_fields_use_the_user_locale_and_fall_back_to_localized_message() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let entity = "moqui.basic.Enumeration";
    for (id, description) in [("E1", "Hello"), ("E2", "Bye"), ("E3", "Plain")] {
        let mut row = facade.make_value(entity).unwrap();
        row.set("enumId", Value::string(id));
        row.set("description", Value::string(description));
        row.create(&facade).unwrap();
    }
    let mut field = facade.make_value("moqui.basic.LocalizedEntityField").unwrap();
    for (name, value) in [
        ("entityName", entity),
        ("fieldName", "description"),
        ("pkValue", "E1"),
        ("locale", "default"),
        ("localized", "Hello (default)"),
    ] {
        field.set(name, Value::string(value));
    }
    field.create(&facade).unwrap();
    let mut message = facade.make_value("moqui.basic.LocalizedMessage").unwrap();
    for (name, value) in [("original", "Bye"), ("locale", "fr"), ("localized", "Au revoir")] {
        message.set(name, Value::string(value));
    }
    message.create(&facade).unwrap();
    let find = |id: &str| {
        facade
            .find(entity)
            .unwrap()
            .eq("enumId", Value::string(id))
            .one()
            .unwrap()
            .unwrap()
    };
    let description = |id: &str| find(id).get_localized(&facade, "description");
    crate::user::session::set_locale(Some("fr_CA"));
    assert_eq!(description("E1"), Some(Value::string("Hello (default)")));
    assert_eq!(description("E2"), Some(Value::string("Au revoir")));
    assert_eq!(description("E3"), Some(Value::string("Plain")));
    // The stored value stays on the value, and a write of it writes what was stored, as Java's `valueMapInternal`.
    let mut read = find("E2");
    assert_eq!(read.get("description"), Some(&Value::string("Bye")));
    read.set("sequenceNum", Value::I64(7));
    read.update(&facade).unwrap();
    facade
        .find(entity)
        .unwrap()
        .eq("enumId", Value::string("E1"))
        .update_all([("sequenceNum".to_string(), Value::I64(8))])
        .unwrap();
    crate::user::session::set_locale(Some("de"));
    assert_eq!(description("E2"), Some(Value::string("Bye")));
    assert_eq!(
        find("E1").get_localized(&facade, "description"),
        Some(Value::string("Hello (default)"))
    );
    assert_eq!(find("E1").get("description"), Some(&Value::string("Hello")));
    assert_eq!(
        find("E2")
            .get("sequenceNum")
            .map(moqui_lib::object::to_plain_string)
            .as_deref(),
        Some("7")
    );
    // A value reads its localized fields in the locale of the time it is read.
    crate::user::session::set_locale(Some("fr"));
    let kept = find("E2");
    assert_eq!(
        kept.get_localized(&facade, "description"),
        Some(Value::string("Au revoir"))
    );
    crate::user::session::set_locale(Some("de"));
    assert_eq!(kept.get_localized(&facade, "description"), Some(Value::string("Bye")));
    assert_eq!(
        kept.localized_fields(&facade).get("description"),
        Some(&Value::string("Bye"))
    );
    crate::user::session::set_locale(None);
}

/// Localization is applied when a field is read, not when the row is: a translation added after the read is seen,
/// and a field read once is not looked up again in the same locale.
#[test]
fn a_localized_field_is_looked_up_when_read_and_then_remembered() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut row = facade.make_value("moqui.basic.Enumeration").unwrap();
    row.set("enumId", Value::string("L1"));
    row.set("description", Value::string("Late"));
    row.create(&facade).unwrap();
    crate::user::session::set_locale(Some("fr"));
    let read = facade
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string("L1"))
        .one()
        .unwrap()
        .unwrap();
    let mut message = facade.make_value("moqui.basic.LocalizedMessage").unwrap();
    for (name, value) in [("original", "Late"), ("locale", "fr"), ("localized", "En retard")] {
        message.set(name, Value::string(value));
    }
    message.create(&facade).unwrap();
    assert_eq!(
        read.get_localized(&facade, "description"),
        Some(Value::string("En retard"))
    );
    message.delete(&facade).unwrap();
    assert_eq!(
        read.get_localized(&facade, "description"),
        Some(Value::string("En retard"))
    );
    // A clone shares what was read; setting the field forgets it.
    let mut copy = read.clone();
    assert_eq!(
        copy.get_localized(&facade, "description"),
        Some(Value::string("En retard"))
    );
    copy.set("description", Value::string("Late"));
    assert_eq!(copy.get_localized(&facade, "description"), Some(Value::string("Late")));
    assert_eq!(
        read.get_localized(&facade, "description"),
        Some(Value::string("En retard"))
    );
    // A value that was made, not read, is plain.
    assert_eq!(row.get_localized(&facade, "description"), Some(Value::string("Late")));
    crate::user::session::set_locale(None);
}

/// Java's `EntityDataDocument` writers: the documents as JSON text, into a new file (an existing one is a message
/// error), and one file per document id in a directory.
#[test]
fn data_documents_are_written_as_json_to_text_a_file_and_a_directory() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let make = |entity: &str, pairs: &[(&str, &str)]| {
        let mut row = facade.make_value(entity).unwrap();
        for (name, value) in pairs {
            row.set(*name, Value::string(*value));
        }
        row.create(&facade).unwrap();
    };
    make(
        "moqui.entity.document.DataDocument",
        &[
            ("dataDocumentId", "EnumDoc"),
            ("primaryEntityName", "moqui.basic.Enumeration"),
            ("documentName", "Enums"),
        ],
    );
    for (seq, path) in [("1", "enumId"), ("2", "enumCode")] {
        make(
            "moqui.entity.document.DataDocumentField",
            &[("dataDocumentId", "EnumDoc"), ("fieldSeqId", seq), ("fieldPath", path)],
        );
    }
    for (id, code) in [("E1", "one"), ("E2", "two")] {
        make("moqui.basic.Enumeration", &[("enumId", id), ("enumCode", code)]);
    }
    let ids = vec!["EnumDoc".to_string()];
    let (text, count) = super::document::documents_json(&facade, &ids, None, None, None, false).unwrap();
    assert_eq!(count, 2, "{text}");
    assert!(text.contains("\"enumCode\":\"one\"") && text.contains(",\n"), "{text}");

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("moqui-doc-export-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("all.json");
    assert_eq!(
        super::document::write_documents_to_file(&facade, &file, &ids, None, None, None, true).unwrap(),
        2
    );
    let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        written.as_array().map(Vec::len),
        Some(3),
        "two documents and the closing {{}}"
    );
    assert_eq!(
        super::document::write_documents_to_file(&facade, &file, &ids, None, None, None, true).unwrap(),
        0
    );
    assert!(context
        .message()
        .errors()
        .iter()
        .any(|error| error.contains("already exists")));
    context.message().clear_errors();
    let out = dir.join("each");
    assert_eq!(
        super::document::write_documents_to_directory(&facade, &out, &ids, None, None, None, false).unwrap(),
        2
    );
    assert!(out.join("EnumDoc.json").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Java's `useClone`: a find reads from the clone datasource of the entity's group when one is open; a find for
/// update and a write stay on the group itself.
#[test]
fn a_find_that_uses_the_clone_reads_the_clone_group() {
    let facade = crate::testing::open_groups("transactional#clone1");
    let entity = "moqui.basic.Enumeration";
    let mut row = facade.make_value(entity).unwrap();
    row.set("enumId", Value::string("MAIN"));
    row.create(&facade).unwrap();
    let main = facade
        .find(entity)
        .unwrap()
        .eq("enumId", Value::string("MAIN"))
        .one()
        .unwrap();
    assert!(main.is_some());
    // The clone is its own database here, so the row is not there.
    let cloned = facade
        .find(entity)
        .unwrap()
        .eq("enumId", Value::string("MAIN"))
        .use_clone(true)
        .use_cache(false)
        .one()
        .unwrap();
    assert!(cloned.is_none());
    assert_eq!(facade.group_names()[0], "transactional");
}

const PARITY_ENTITIES: &str = r#"<entities>
    <entity entity-name="ParityItem" package="moqui.test">
        <field name="itemId" type="id" is-pk="true"/>
        <field name="code" type="id"/>
        <field name="seq" type="number-integer" default="(code == null ? null : Math.trunc(toDecimal(code).toNumber()))"/>
        <field name="kind" type="id" default="'plain'"/>
        <field name="unknown" type="id" default="NotAName"/>
        <field name="note" type="text-medium"/>
        <field name="other" type="text-medium"/>
    </entity>
    <entity entity-name="ParityLocked" package="moqui.test" optimistic-lock="true">
        <field name="lockedId" type="id" is-pk="true"/>
        <field name="note" type="text-medium"/>
    </entity>
    <view-entity entity-name="ParityItemView" package="moqui.test">
        <member-entity entity-alias="I" entity-name="moqui.test.ParityItem"/>
        <alias-all entity-alias="I"/>
        <alias name="other" pq-expression="code + '-' + note" default-display="true"/>
        <alias name="selfRead" pq-expression="pqRow.code + '?'"/>
        <alias name="broken" pq-expression="noSuchName.length"/>
        <alias name="withSuffix" pq-expression="code + pqSuffix"/>
        <alias name="sameItem" pq-expression="'not the relationship'"/>
        <relationship type="one" related="moqui.test.ParityItem" short-alias="sameItem">
            <key-map field-name="itemId"/></relationship>
    </view-entity>
</entities>"#;

/// The framework entities plus [`PARITY_ENTITIES`], in a directory removed on drop.
struct ParityFixture(PathBuf);

impl ParityFixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "moqui-parity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        for name in ["entity", "conf"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
        }
        let real = framework_dir();
        std::os::unix::fs::symlink(
            real.join("conf/MoquiDefaultConf.xml"),
            dir.join("conf/MoquiDefaultConf.xml"),
        )
        .unwrap();
        for entry in std::fs::read_dir(real.join("entity")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                std::os::unix::fs::symlink(entry.path(), dir.join("entity").join(entry.file_name())).unwrap();
            }
        }
        std::fs::write(dir.join("entity/ParityEntities.xml"), PARITY_ENTITIES).unwrap();
        Self(dir)
    }

    fn open(&self, tape: Option<Arc<Mutex<Vec<String>>>>) -> EntityFacade {
        crate::testing::open_with(
            &self.0,
            crate::testing::Options {
                tape,
                ..Default::default()
            },
        )
    }
}

impl Drop for ParityFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn parity_item(facade: &EntityFacade, id: &str) -> super::EntityValue {
    facade
        .find("moqui.test.ParityItem")
        .unwrap()
        .eq("itemId", Value::string(id))
        .use_cache(false)
        .one()
        .unwrap()
        .unwrap()
}

/// Java's `checkSetFieldDefaults`: a default is an expression run with the value's fields, an empty string is unset,
/// a name that is nothing is null, and an update fills a default too, but never over a stored value.
#[test]
fn field_defaults_are_expressions_that_also_fill_an_update() {
    let fixture = ParityFixture::new();
    let facade = fixture.open(None);
    let mut item = facade.make_value("moqui.test.ParityItem").unwrap();
    item.set("itemId", Value::string("I1"));
    item.set("code", Value::string("07"));
    item.set("kind", Value::string(""));
    item.create(&facade).unwrap();
    let created = parity_item(&facade, "I1");
    assert_eq!(created.get("seq"), Some(&Value::I64(7)));
    assert_eq!(created.get("kind"), Some(&Value::string("plain")));
    assert!(created.get("unknown").is_none_or(Value::is_null));

    let mut custom = created.clone();
    custom.set("kind", Value::string("custom"));
    custom.update(&facade).unwrap();
    // Left out, the stored value stays; set empty, the default fills it.
    let mut partial = facade.make_value("moqui.test.ParityItem").unwrap();
    partial.set("itemId", Value::string("I1"));
    partial.set("note", Value::string("n"));
    partial.update(&facade).unwrap();
    assert_eq!(parity_item(&facade, "I1").get("kind"), Some(&Value::string("custom")));
    partial.set("kind", Value::string(""));
    partial.update(&facade).unwrap();
    assert_eq!(parity_item(&facade, "I1").get("kind"), Some(&Value::string("plain")));
}

/// Java's `pq-expression` alias: `get` and `getMap` evaluate it with the row's stored fields, here
/// `get_localized`, `localized_fields` and a script's read; the stored fields and a REST find (`getPlainValueMap`)
/// have none of it. Its name keeps `alias-all` from the member field of the same name, each read evaluates it again,
/// an expression that reads the value it is on does not find the value locked, a failing one is logged and reads as
/// null, and a script reads a relationship of the same name first.
#[test]
fn a_pq_expression_alias_is_evaluated_on_read_and_left_out_of_the_stored_fields() {
    let fixture = ParityFixture::new();
    let facade = fixture.open(None);
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let _frame = crate::script::ContextFrame::push();
    crate::script::set_context_name("pqSuffix", Value::string("a")).unwrap();
    let mut item = facade.make_value("moqui.test.ParityItem").unwrap();
    for (name, value) in [("itemId", "I1"), ("code", "7"), ("note", "n"), ("other", "stored")] {
        item.set(name, Value::string(value));
    }
    item.create(&facade).unwrap();
    let view = "moqui.test.ParityItemView";
    let find = || {
        facade
            .find(view)
            .unwrap()
            .eq("itemId", Value::string("I1"))
            .one()
            .unwrap()
            .unwrap()
    };
    let mut row = find();
    assert_eq!(row.get("code"), Some(&Value::string("7")));
    for name in ["other", "selfRead", "broken", "withSuffix"] {
        assert!(!row.fields().contains_key(name), "{name} is not a stored field");
    }
    assert_eq!(row.get_localized(&facade, "other"), Some(Value::string("7-n")));
    let lines = Arc::new(Mutex::new(Vec::new()));
    let all = tracing::subscriber::with_default(crate::testing::LogSubscriber(Arc::clone(&lines)), || {
        row.localized_fields(&facade)
    });
    assert_eq!(all.get("other"), Some(&Value::string("7-n")));
    assert_eq!(all.get("code"), Some(&Value::string("7")));
    assert_eq!(all.get("broken"), Some(&Value::Null));
    let text = lines.lock().unwrap().join("\n");
    assert!(
        text.contains("alias broken on entity moqui.test.ParityItemView"),
        "{text}"
    );
    row.set("note", Value::string("m"));
    assert_eq!(row.get_localized(&facade, "other"), Some(Value::string("7-m")));
    // Not remembered: a read sees what the expression reads from outside the row now, as Java's `get`.
    assert_eq!(row.get_localized(&facade, "withSuffix"), Some(Value::string("7a")));
    crate::script::set_context_name("pqSuffix", Value::string("b")).unwrap();
    assert_eq!(row.get_localized(&facade, "withSuffix"), Some(Value::string("7b")));

    // A script reads it as a property, by `get` and in `getMap`; the expression reads the value from the context with
    // the value unlocked.
    let read = crate::script::eval_value(
        r#"(function () {
            pqRow = ec.entity.find("moqui.test.ParityItemView").condition("itemId", "I1").one();
            const value = pqRow;
            const before = value.other === "7-n" && value.get("other") === "7-n" && value.getMap().other === "7-n";
            value.note = "z";
            return before && value.other === "7-z" && value.selfRead === "7?" && value.broken === null
                && value.withSuffix === "7b" && value.sameItem.itemId === "I1" && value.get("sameItem").itemId === "I1";
        })()"#,
    )
    .unwrap();
    assert_eq!(read, Value::Bool(true));
    // As a template or XML action reads it.
    let held = crate::script::context_value("pqRow").expect("the script put the value in the context");
    assert_eq!(crate::script::value_field(&held, "other"), Some(Value::string("7-z")));
    assert_eq!(
        crate::script::value_map(&held).and_then(|map| map.get("selfRead").cloned()),
        Some(Value::string("7?"))
    );

    let found = facade
        .rest(
            "get",
            view,
            &BTreeMap::from([("itemId".to_string(), Value::string("I1"))]),
        )
        .unwrap();
    let Value::Map(found) = found else {
        panic!("a REST find by primary key returns one map: {found:?}");
    };
    assert_eq!(found.get("code"), Some(&Value::string("7")));
    for name in ["other", "selfRead", "broken", "withSuffix"] {
        assert!(!found.contains_key(name), "{name} is not in the REST output");
    }
}

/// Java's `update`: an entity whose update reads the row first writes only the fields that changed, and nothing when
/// none did; one that does not read it writes every field given, but not for a stamp alone.
#[test]
fn an_update_writes_the_changed_fields_of_a_row_it_reads_and_nothing_for_no_change() {
    let fixture = ParityFixture::new();
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = fixture.open(Some(Arc::clone(&tape)));
    let mut item = facade.make_value("moqui.test.ParityItem").unwrap();
    item.set("itemId", Value::string("I1"));
    item.set("note", Value::string("a"));
    item.set("other", Value::string("b"));
    item.create(&facade).unwrap();
    let updates = || -> Vec<String> {
        tape.lock()
            .unwrap()
            .iter()
            .filter(|sql| sql.starts_with("UPDATE"))
            .cloned()
            .collect()
    };

    let mut found = parity_item(&facade, "I1");
    found.update(&facade).unwrap();
    assert!(updates().is_empty(), "{:?}", updates());
    found.set("note", Value::string("c"));
    found.update(&facade).unwrap();
    let written = updates();
    assert_eq!(written.len(), 1, "{written:?}");
    let set = written[0].split(" WHERE ").next().unwrap();
    assert!(
        set.contains("\"note\"") && set.contains("\"last_updated_stamp\""),
        "{set}"
    );
    assert!(!set.contains("\"other\"") && !set.contains("\"kind\""), "{set}");
    assert_eq!(parity_item(&facade, "I1").get("other"), Some(&Value::string("b")));

    let mut test = facade.make_value("moqui.test.TestEntity").unwrap();
    test.set("testId", Value::string("T1"));
    test.create(&facade).unwrap();
    test.update(&facade).unwrap();
    assert_eq!(updates().len(), 1, "a stamp alone is no update: {:?}", updates());
    test.set("testMedium", Value::string("m"));
    test.update(&facade).unwrap();
    assert_eq!(updates().len(), 2);
}

/// Java stamps `lastUpdatedStamp` with the transaction start time, and an update keeps a stamp the caller changed from
/// the stored one.
#[test]
fn the_stamp_is_the_transaction_start_and_a_changed_stamp_is_kept() {
    let fixture = ParityFixture::new();
    let facade = fixture.open(None);
    let entity = "moqui.test.ParityItem";
    assert!(facade.begin(entity, None).unwrap());
    for id in ["I1", "I2"] {
        let mut item = facade.make_value(entity).unwrap();
        item.set("itemId", Value::string(id));
        item.create(&facade).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    facade.commit(entity).unwrap();
    let stamp = |id: &str| parity_item(&facade, id).get("lastUpdatedStamp").cloned();
    assert!(stamp("I1").is_some_and(|stamp| !stamp.is_null()));
    assert_eq!(stamp("I1"), stamp("I2"));

    let chosen = moqui_lib::object::basic_convert(&Value::I64(1_000_000_000_000), "java.sql.Timestamp").value;
    let mut item = parity_item(&facade, "I1");
    item.set("note", Value::string("x"));
    item.set("lastUpdatedStamp", chosen.clone());
    item.update(&facade).unwrap();
    assert_eq!(stamp("I1"), Some(chosen));
}

/// Java's `optimistic-lock`: an update of a value read before another update of the row is refused.
#[test]
fn optimistic_lock_refuses_an_update_of_a_stale_value() {
    let fixture = ParityFixture::new();
    let facade = fixture.open(None);
    let entity = "moqui.test.ParityLocked";
    let mut row = facade.make_value(entity).unwrap();
    row.set("lockedId", Value::string("L1"));
    row.create(&facade).unwrap();
    let read = || {
        facade
            .find(entity)
            .unwrap()
            .eq("lockedId", Value::string("L1"))
            .use_cache(false)
            .one()
            .unwrap()
            .unwrap()
    };
    let (mut first, mut second) = (read(), read());
    std::thread::sleep(Duration::from_millis(5));
    first.set("note", Value::string("first"));
    first.update(&facade).unwrap();
    second.set("note", Value::string("second"));
    let err = second.update(&facade).unwrap_err();
    assert!(err.to_string().contains("updated by someone else"), "{err}");
    assert_eq!(read().get("note"), Some(&Value::string("first")));
}

/// The framework conf, to change before a facade is opened on it.
fn framework_conf() -> MNode {
    MNode::parse_file(crate::testing::framework_dir().join("conf/MoquiDefaultConf.xml")).unwrap()
}

#[test]
fn the_global_sequenced_id_prefix_is_put_in_front_of_bank_ids() {
    let conf = framework_conf();
    conf.first_named("entity-facade")
        .unwrap()
        .set_attribute("sequenced-id-prefix", "G");
    let facade = crate::testing::open_with_conf(&conf);
    facade.temp_set_sequenced_id_primary("moqui.test.Foo", 1000, 10);
    let mut foo = facade.make_value("moqui.test.Foo").unwrap();
    foo.set_sequenced_id_primary(&facade).unwrap();
    assert_eq!(foo.get("fooId").and_then(Value::as_str), Some("G1000"));
    facade.temp_set_sequenced_id_primary("SomeName", 7, 10);
    assert_eq!(
        super::sequenced_id_primary(&facade, "SomeName", None, None).unwrap(),
        "G7"
    );
    // An empty prefix is none, as the framework conf has it.
    let plain = crate::testing::open();
    plain.temp_set_sequenced_id_primary("SomeName", 7, 10);
    assert_eq!(
        super::sequenced_id_primary(&plain, "SomeName", None, None).unwrap(),
        "7"
    );
}

#[test]
fn a_datasource_with_sequence_primary_use_uuid_gives_its_entities_uuids() {
    let conf = framework_conf();
    let datasource = super::datasource::datasource_nodes(&conf)
        .into_iter()
        .find(|node| node.attribute("group-name").as_deref() == Some("transactional"))
        .unwrap();
    datasource.set_attribute("sequence-primary-use-uuid", "true");
    let facade = crate::testing::open_with_conf(&conf);
    let mut foo = facade.make_value("moqui.test.Foo").unwrap();
    foo.set_sequenced_id_primary(&facade).unwrap();
    let id = foo.get("fooId").and_then(Value::as_str).unwrap().to_string();
    assert_eq!((id.len(), id.matches('-').count()), (36, 4), "{id}");
    let id = super::sequenced_id_primary(&facade, "moqui.test.Foo", None, None).unwrap();
    assert_eq!(id.len(), 36, "{id}");
    // A name that is no entity still comes from the bank.
    facade.temp_set_sequenced_id_primary("SomeName", 7, 10);
    assert_eq!(
        super::sequenced_id_primary(&facade, "SomeName", None, None).unwrap(),
        "7"
    );
}

#[test]
fn a_load_entity_file_outside_the_entity_directories_joins_the_model() {
    let dir = std::env::temp_dir().join(format!("moqui-load-entity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("Extra.xml");
    std::fs::write(
        &file,
        r#"<entities><entity entity-name="LoadedByConf" package="moqui.test.conf">
            <field name="confId" type="id" is-pk="true"/></entity></entities>"#,
    )
    .unwrap();
    let conf = framework_conf();
    let entity_facade = conf.first_named("entity-facade").unwrap();
    let extra = MNode::parse_text("load", &format!(r#"<load-entity location="{}"/>"#, file.display())).unwrap();
    entity_facade.append_node(extra);
    let missing = MNode::parse_text("load", r#"<load-entity location="component://nowhere/entity/X.xml"/>"#).unwrap();
    entity_facade.append_node(missing);
    let facade = crate::testing::open_with_conf(&conf);
    let _ = std::fs::remove_dir_all(&dir);
    // The framework's own load-entity files are its entity directory, read once.
    assert_eq!(facade.entity_files().len(), 1, "{:?}", facade.entity_files());
    assert!(facade.make_value("moqui.test.conf.LoadedByConf").is_ok());
    assert!(facade.make_value("moqui.basic.Enumeration").is_ok());
}

#[test]
fn a_sequence_primary_prefix_is_expanded_with_the_value_fields() {
    let context = crate::context::ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let mut values = BTreeMap::new();
    values.insert("kind".to_string(), Value::string("INV"));
    assert_eq!(
        super::sequence::expand_prefix(Some("${kind}-"), &values)
            .unwrap()
            .as_deref(),
        Some("INV-")
    );
    assert_eq!(
        super::sequence::expand_prefix(Some("P"), &values).unwrap().as_deref(),
        Some("P")
    );
}

fn create_row(facade: &EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in pairs {
        row.set(*name, Value::string(*value));
    }
    row.create(facade).unwrap();
}

/// Enumeration types `T1` and `T2`, with `E1` and `E2` of `T1` and `E3` of `T2`.
fn seed_enumerations(facade: &EntityFacade) {
    for (id, description) in [("T1", "Type one"), ("T2", "Type two")] {
        create_row(
            facade,
            "moqui.basic.EnumerationType",
            &[("enumTypeId", id), ("description", description)],
        );
    }
    for (id, type_id) in [("E1", "T1"), ("E2", "T1"), ("E3", "T2")] {
        create_row(
            facade,
            "moqui.basic.Enumeration",
            &[
                ("enumId", id),
                ("enumTypeId", type_id),
                ("description", &format!("Enum {id}")),
            ],
        );
    }
}

/// A `DbViewEntity` of `members` (alias, entity, join-from alias), each joined on `enumTypeId`, and `aliases` (name,
/// member alias, field).
fn create_db_view(facade: &EntityFacade, name: &str, members: &[(&str, &str, &str)], aliases: &[(&str, &str, &str)]) {
    create_row(
        facade,
        "moqui.entity.view.DbViewEntity",
        &[
            ("dbViewEntityName", name),
            ("packageName", "test.dbview"),
            ("cache", "Y"),
        ],
    );
    for (alias, entity, join_from) in members {
        let mut fields = vec![
            ("dbViewEntityName", name),
            ("entityAlias", *alias),
            ("entityName", *entity),
        ];
        if !join_from.is_empty() {
            fields.extend([("joinFromAlias", *join_from), ("joinOptional", "Y")]);
            create_row(
                facade,
                "moqui.entity.view.DbViewEntityKeyMap",
                &[
                    ("dbViewEntityName", name),
                    ("joinFromAlias", join_from),
                    ("entityAlias", alias),
                    ("fieldName", "enumTypeId"),
                ],
            );
        }
        create_row(facade, "moqui.entity.view.DbViewEntityMember", &fields);
    }
    for (alias, member, field) in aliases {
        create_row(
            facade,
            "moqui.entity.view.DbViewEntityAlias",
            &[
                ("dbViewEntityName", name),
                ("fieldAlias", alias),
                ("entityAlias", member),
                ("fieldName", field),
            ],
        );
    }
}

/// Java's `DbViewEntity`: a view-entity built from its member, key map and alias rows on its first use, under its name
/// and its full name, that finds, counts and iterates as a file view does. A name looked up before its rows are there
/// is found once they are, and a write to its rows builds it again.
#[test]
fn a_db_view_entity_is_built_from_its_rows_and_again_when_they_change() {
    let facade = crate::testing::open();
    seed_enumerations(&facade);
    assert!(facade.find("EnumWithType").is_err());
    create_db_view(
        &facade,
        "EnumWithType",
        &[
            ("MASTER", "moqui.basic.Enumeration", ""),
            ("TYPE", "moqui.basic.EnumerationType", "MASTER"),
        ],
        &[
            ("enumId", "MASTER", "enumId"),
            ("description", "MASTER", "description"),
            ("typeDescription", "TYPE", "description"),
        ],
    );
    let one = |name: &str| {
        facade
            .find(name)
            .unwrap()
            .eq("enumId", Value::string("E1"))
            .one()
            .unwrap()
            .unwrap()
    };
    let row = one("EnumWithType");
    assert_eq!(row.get("typeDescription"), Some(&Value::string("Type one")));
    assert_eq!(row.get("description"), Some(&Value::string("Enum E1")));
    assert!(row.get("enumTypeId").is_none());
    let (full_name, pk, cache) = facade
        .with_entity("EnumWithType", |entity, _, _| {
            let pk: Vec<String> = entity.pk_field_names().into_iter().map(str::to_string).collect();
            Ok((entity.full_name.clone(), pk, entity.cache.clone()))
        })
        .unwrap();
    assert_eq!(full_name, "test.dbview.EnumWithType");
    assert_eq!(pk, ["enumId"]);
    // Never cached, though the row says `Y`: the entity cache does not know a member's views built here.
    assert_eq!(cache, "never");
    assert_eq!(
        one("test.dbview.EnumWithType").get("typeDescription"),
        Some(&Value::string("Type one"))
    );
    assert!(facade.find("other.EnumWithType").is_err());
    assert_eq!(facade.find("EnumWithType").unwrap().count().unwrap(), 3);
    let mut iterator = facade
        .find("EnumWithType")
        .unwrap()
        .order_by("enumId", true)
        .iterator()
        .unwrap();
    let first = iterator.next_value().unwrap().unwrap();
    assert_eq!(first.get("enumId"), Some(&Value::string("E3")));
    iterator.close().unwrap();
    assert!(super::info::is_entity_defined(&facade, "EnumWithType"));
    let info = super::info::definition_info(&facade, "EnumWithType").unwrap();
    assert_eq!(info.get("isViewEntity"), Some(&Value::Bool(true)));

    // A new alias row: the next lookup builds the view with it.
    create_row(
        &facade,
        "moqui.entity.view.DbViewEntityAlias",
        &[
            ("dbViewEntityName", "EnumWithType"),
            ("fieldAlias", "enumTypeId"),
            ("entityAlias", "MASTER"),
        ],
    );
    assert_eq!(one("EnumWithType").get("enumTypeId"), Some(&Value::string("T1")));

    // An alias's function, as Java's `function` attribute.
    create_db_view(
        &facade,
        "EnumCount",
        &[("MASTER", "moqui.basic.Enumeration", "")],
        &[("enumTypeId", "MASTER", "enumTypeId")],
    );
    create_row(
        &facade,
        "moqui.entity.view.DbViewEntityAlias",
        &[
            ("dbViewEntityName", "EnumCount"),
            ("fieldAlias", "enumCount"),
            ("entityAlias", "MASTER"),
            ("fieldName", "enumId"),
            ("functionName", "count"),
        ],
    );
    let counted = facade
        .with_entity("EnumCount", |entity, _, _| {
            Ok(entity
                .alias("enumCount")
                .map(|alias| (alias.function.clone(), alias.aggregates())))
        })
        .unwrap();
    assert_eq!(counted, Some((Some("count".to_string()), true)));
}

/// AGENTS.md failure tolerance: a `DbViewEntity` with a member that is not an entity, or an alias of a field its
/// member does not have, is logged with its name once and is not found, until its rows change; other entities and
/// views go on. A name that is no entity is not found, with nothing logged.
#[test]
fn a_db_view_entity_that_does_not_build_is_logged_and_not_found() {
    let facade = crate::testing::open();
    seed_enumerations(&facade);
    create_db_view(
        &facade,
        "NoMemberView",
        &[("MASTER", "moqui.test.NoSuchEntity", "")],
        &[("enumId", "MASTER", "enumId")],
    );
    create_db_view(
        &facade,
        "BadAliasView",
        &[("MASTER", "moqui.basic.Enumeration", "")],
        &[("enumId", "MASTER", "noSuchField")],
    );
    create_db_view(
        &facade,
        "GoodView",
        &[("MASTER", "moqui.basic.Enumeration", "")],
        &[("enumId", "MASTER", "enumId")],
    );
    let lines = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(crate::testing::LogSubscriber(Arc::clone(&lines)), || {
        for _ in 0..2 {
            assert!(facade.find("NoMemberView").is_err());
            assert!(facade.find("BadAliasView").is_err());
            assert!(facade.find("NoSuchEntityAnywhere").is_err());
            assert!(facade.find("DataDocument.NoSuchDocument").is_err());
        }
        assert!(!super::info::is_entity_defined(&facade, "NoMemberView"));
        assert_eq!(facade.find("moqui.basic.Enumeration").unwrap().count().unwrap(), 3);
        assert_eq!(facade.find("GoodView").unwrap().count().unwrap(), 3);
    });
    let lines = lines.lock().unwrap().clone();
    let text = lines.join("\n");
    let logged = |name: &str| lines.iter().filter(|line| line.contains(name)).count();
    assert_eq!(logged("NoMemberView"), 1, "{text}");
    assert!(text.contains("moqui.test.NoSuchEntity"), "{text}");
    assert_eq!(logged("BadAliasView"), 1, "{text}");
    assert!(text.contains("noSuchField"), "{text}");
    assert!(
        !text.contains("NoSuchEntityAnywhere") && !text.contains("NoSuchDocument"),
        "{text}"
    );
    let Err(err) = facade.find("NoSuchEntityAnywhere") else {
        panic!("not an entity");
    };
    assert!(
        err.to_string().contains("no entity named NoSuchEntityAnywhere"),
        "{err}"
    );

    // Fixed rows: found.
    facade
        .find("moqui.entity.view.DbViewEntityAlias")
        .unwrap()
        .eq("dbViewEntityName", Value::string("BadAliasView"))
        .update_all([("fieldName".to_string(), Value::string("enumId"))])
        .unwrap();
    assert_eq!(facade.find("BadAliasView").unwrap().count().unwrap(), 3);
}

/// Java's `getEntityDefinition("DataDocument.<id>")`: the document's view, its aliases with `fieldNameAlias`,
/// `functionName` and `defaultDisplay`, an expression field a `pq-expression` alias that a read evaluates; a find on it
/// has the document's conditions, as Java's `makeDataDocumentFind`. As Java's `isEntityDefined`, the name is not an
/// entity name, and a write to the document's rows builds it again.
#[test]
fn a_data_document_name_is_its_view_with_its_conditions() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    seed_enumerations(&facade);
    create_row(
        &facade,
        "moqui.entity.document.DataDocument",
        &[
            ("dataDocumentId", "EnumDoc"),
            ("primaryEntityName", "moqui.basic.EnumerationType"),
        ],
    );
    for (seq, path, alias, display) in [
        ("01", "enumTypeId", "", ""),
        ("02", "description", "typeDescription", "N"),
        ("03", "enums:enumId", "", ""),
        ("04", "enums:description", "enumDescription", ""),
        ("05", "(enumTypeId + '/' + enumId)", "label", "Y"),
    ] {
        let mut fields = vec![("dataDocumentId", "EnumDoc"), ("fieldSeqId", seq), ("fieldPath", path)];
        if !alias.is_empty() {
            fields.push(("fieldNameAlias", alias));
        }
        if !display.is_empty() {
            fields.push(("defaultDisplay", display));
        }
        create_row(&facade, "moqui.entity.document.DataDocumentField", &fields);
    }
    create_row(
        &facade,
        "moqui.entity.document.DataDocumentCondition",
        &[
            ("dataDocumentId", "EnumDoc"),
            ("conditionSeqId", "01"),
            ("fieldNameAlias", "enumTypeId"),
            ("fieldValue", "T1"),
        ],
    );
    let name = "DataDocument.EnumDoc";
    let rows = facade.find(name).unwrap().order_by("enumId", false).list().unwrap();
    let ids: Vec<_> = rows.iter().map(|row| row.get("enumId").cloned()).collect();
    assert_eq!(ids, [Some(Value::string("E1")), Some(Value::string("E2"))]);
    let row = &rows.values()[0];
    assert_eq!(row.get("typeDescription"), Some(&Value::string("Type one")));
    assert_eq!(row.get("enumDescription"), Some(&Value::string("Enum E1")));
    assert!(!row.fields().contains_key("label"));
    assert_eq!(row.get_localized(&facade, "label"), Some(Value::string("T1/E1")));
    let (full_name, aliases, displays, pq) = facade
        .with_entity(name, |entity, _, _| {
            Ok((
                entity.full_name.clone(),
                entity.field_names().into_iter().map(str::to_string).collect::<Vec<_>>(),
                entity
                    .aliases
                    .iter()
                    .map(|alias| alias.default_display)
                    .collect::<Vec<_>>(),
                entity
                    .pq_aliases
                    .iter()
                    .map(|alias| (alias.name.clone(), alias.default_display))
                    .collect::<Vec<_>>(),
            ))
        })
        .unwrap();
    assert_eq!(full_name, name);
    assert_eq!(aliases, ["enumTypeId", "typeDescription", "enumId", "enumDescription"]);
    assert_eq!(displays, [None, Some(false), None, None]);
    assert_eq!(pq, [("label".to_string(), true)]);
    assert!(!super::info::is_entity_defined(&facade, name));

    create_row(
        &facade,
        "moqui.entity.document.DataDocumentField",
        &[
            ("dataDocumentId", "EnumDoc"),
            ("fieldSeqId", "06"),
            ("fieldPath", "enums:enumCode"),
        ],
    );
    assert!(facade
        .with_entity(name, |entity, _, _| Ok(entity.has_field("enumCode")))
        .unwrap());
}
