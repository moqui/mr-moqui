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
use super::{EntityFacade, EntityGroup, FieldMap};
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
    /// An update matches the row it names, so an update by primary key succeeds as against a database that has it.
    fn execute(&mut self, sql: &str, _params: &[Value]) -> Result<u64, Error> {
        Ok(u64::from(
            sql.trim_start()
                .get(..6)
                .is_some_and(|verb| verb.eq_ignore_ascii_case("update")),
        ))
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
            ("testId".into(), Value::string("T1")),
            ("testMedium".into(), Value::string("kept")),
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
                        ("instanceHostId".into(), Value::string("H1")),
                        ("password".into(), Value::string(password)),
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
        .upsert_record(
            "moqui.server.instance.InstanceHost",
            crate::entity::field_map(fields.clone()),
        )
        .unwrap();
    fields.insert("instanceHostId".to_string(), Value::string("H2"));
    facade.begin("moqui.server.instance.InstanceHost", None).unwrap();
    facade
        .upsert_record(
            "moqui.server.instance.InstanceHost",
            crate::entity::field_map(fields.clone()),
        )
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

/// `searchFormMap` of `barRank` = 100 on `moqui.test.FooBar` puts `BAR_RANK = ?` in the query, and
/// `barRank_from` = 100 puts `BAR_RANK >= ?`. Postgres quotes and lowercases the column; the
/// unquoted upper text is what the form names.
#[test]
fn a_foobar_search_form_puts_bar_rank_equal_and_from_in_the_query() {
    let tape = Arc::new(Mutex::new(Tape {
        sql: Vec::new(),
        password: None,
    }));
    let facade = open_recording(tape, "postgres");
    for (field, needle) in [("barRank", "BAR_RANK = ?"), ("barRank_from", "BAR_RANK >= ?")] {
        let mut input = BTreeMap::new();
        input.insert(field.to_string(), Value::string("100"));
        let result = facade
            .find("moqui.test.FooBar")
            .unwrap()
            .search_form_map(&input, &BTreeMap::new(), "", "", true)
            .unwrap();
        let texts = result.find.query_texts();
        result.find.list().unwrap();
        let sql = texts.lock().unwrap_or_else(|err| err.into_inner()).join("\n");
        let upper = sql.replace(['"', '`'], "").to_ascii_uppercase();
        assert!(upper.contains(needle), "{upper}");
    }
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
            columns: vec![("n".into(), Value::I64(number))],
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
    let element =
        super::query_rest::xml_element(&facade, "moqui.test.TestEntity", &crate::entity::field_map(fields), 0).unwrap();
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

/// A store of one row of `TestEntity`: `UPDATE` reports the row when present, `INSERT` adds it, and an insert that
/// does nothing on a conflict counts no row when it is there. Records the SQL.
#[derive(Default)]
struct StoreTape {
    sql: Vec<String>,
    params: Vec<Vec<Value>>,
    exists: bool,
    /// Another transaction creates the row just before the next insert.
    created_by_another: bool,
    /// The next insert that does nothing on a conflict meets another row's unique index value, so it counts no row
    /// and this row stays missing (MySQL's `ON DUPLICATE KEY UPDATE` takes any unique index).
    unique_conflict: bool,
    /// The next update leaves the row as it was, which MySQL counts as no row.
    unchanged: bool,
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
            if std::mem::take(&mut tape.unchanged) {
                return Ok(0);
            }
            return Ok(u64::from(tape.exists));
        }
        if sql.starts_with("INSERT") {
            if std::mem::take(&mut tape.created_by_another) {
                tape.exists = true;
            }
            let if_missing = sql.ends_with(" DO NOTHING") || sql.contains(" ON DUPLICATE KEY UPDATE ");
            if if_missing && std::mem::take(&mut tape.unique_conflict) {
                return Ok(0);
            }
            if if_missing && tape.exists && !sql.contains(" DO UPDATE ") && !sql.contains(" = VALUES(") {
                return Ok(0);
            }
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

/// [`open_store`] with a `create` ECA rule on `TestEntity`, so a create-or-update is not one upsert (a rule depends on
/// whether it creates) and takes the UPDATE-first or the find-then-write path. The rule does not run here: no context
/// is entered.
fn open_store_no_upsert(exists: bool, dialect: &str) -> (EntityFacade, Arc<Mutex<StoreTape>>) {
    let (facade, tape) = open_store(exists, dialect);
    super::load_feed::add_eca_rules(
        &facade,
        r#"<eecas><eeca entity="moqui.test.TestEntity" on-create="true"><actions><log message="x"/></actions></eeca></eecas>"#,
    )
    .unwrap();
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

/// Where nothing depends on whether it creates or updates, a create-or-update of a value not read from the database is
/// one statement that does either, for a new row and for one that is there (Parity-Diffs, atomic create-or-update).
#[test]
fn create_or_update_where_an_upsert_fits_is_one_statement() {
    for dialect in ["postgres", "mysql"] {
        let (facade, tape) = open_store(false, dialect);
        facade.create_value(&test_value(&facade)).unwrap();
        let direct_insert = tape.lock().unwrap().sql[0].clone();
        for exists in [false, true] {
            {
                let mut tape = tape.lock().unwrap();
                tape.exists = exists;
                tape.sql.clear();
                tape.params.clear();
            }
            facade.create_or_update_value(&test_value(&facade)).unwrap();
            let tape = tape.lock().unwrap();
            assert_eq!(tape.sql.len(), 1, "{dialect}, exists={exists}: {:?}", tape.sql);
            let sql = &tape.sql[0];
            assert!(sql.starts_with(&direct_insert), "{dialect}: {sql}");
            if dialect == "mysql" {
                assert!(
                    sql.contains(" ON DUPLICATE KEY UPDATE `TEST_MEDIUM` = VALUES(`TEST_MEDIUM`)"),
                    "{sql}"
                );
            } else {
                assert!(
                    sql.contains(" ON CONFLICT (\"test_id\") DO UPDATE SET \"test_medium\" = EXCLUDED.\"test_medium\""),
                    "{sql}"
                );
            }
            assert!(tape.exists);
        }
    }
}

/// A value that sets only the key: one insert that does nothing when the row is there, since an update would write
/// nothing.
#[test]
fn create_or_update_of_only_the_key_inserts_it_if_missing() {
    for (dialect, ending) in [
        ("postgres", " ON CONFLICT (\"test_id\") DO NOTHING"),
        ("mysql", " ON DUPLICATE KEY UPDATE `TEST_ID` = `TEST_ID`"),
    ] {
        for exists in [false, true] {
            let (facade, tape) = open_store(exists, dialect);
            let mut value = facade.make_value("moqui.test.TestEntity").unwrap();
            value.set("testId", Value::string("T1"));
            facade.create_or_update_value(&value).unwrap();
            let tape = tape.lock().unwrap();
            assert_eq!(tape.sql.len(), 1, "{dialect}, exists={exists}: {:?}", tape.sql);
            assert!(tape.sql[0].ends_with(ending), "{}", tape.sql[0]);
        }
    }
}

/// MySQL counts the rows an update changes, not the ones it matches, so an update that leaves the row as it was (an
/// amount that rounds to the stored one) counts none: the row is read by its key, and the update is no error. When the
/// row is not there, and on Postgres, a count of none is an update of a missing row.
#[test]
fn an_update_mysql_counts_as_no_row_is_no_error_when_the_row_is_there() {
    let update = |dialect: &str, exists: bool| {
        let (facade, tape) = open_store(exists, dialect);
        tape.lock().unwrap().unchanged = true;
        let mut value = test_value(&facade);
        value.set("testMedium", Value::string("rounds to the stored one"));
        let result = value.update(&facade).map_err(|err| err.to_string());
        let kinds: Vec<String> = writes(&tape).into_iter().map(|(kind, _)| kind).collect();
        (result, kinds)
    };
    let (result, kinds) = update("mysql", true);
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(kinds, ["UPDATE", "SELECT"], "{kinds:?}");
    let missing = "Tried to update a value that does not exist";
    let (result, _) = update("mysql", false);
    assert!(result.is_err_and(|err| err.contains(missing)));
    let (result, kinds) = update("postgres", true);
    assert!(result.is_err_and(|err| err.contains(missing)));
    assert_eq!(kinds, ["UPDATE"], "{kinds:?}");
}

/// A value read from the database writes only what it changed, and one that sets `lastUpdatedStamp` keeps an update's
/// stamp rule: neither is one upsert.
#[test]
fn create_or_update_of_a_read_value_or_a_given_stamp_is_not_one_upsert() {
    let (facade, tape) = open_store(true, "postgres");
    let mut read = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("T1"))
        .one()
        .unwrap()
        .expect("the row");
    read.set("testMedium", Value::string("read then changed"));
    tape.lock().unwrap().sql.clear();
    tape.lock().unwrap().params.clear();
    facade.create_or_update_value(&read).unwrap();
    let kinds: Vec<String> = writes(&tape).into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(kinds, ["UPDATE"], "{kinds:?}");

    tape.lock().unwrap().sql.clear();
    tape.lock().unwrap().params.clear();
    let mut stamped = test_value(&facade);
    stamped.set("lastUpdatedStamp", Value::string("2026-10-30 12:00:00"));
    facade.create_or_update_value(&stamped).unwrap();
    let kinds: Vec<String> = writes(&tape).into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(kinds, ["UPDATE"], "{kinds:?}");
}

/// On MySQL `ON DUPLICATE KEY UPDATE` takes a conflict on any unique index, so an entity with one is not stored as one
/// upsert there (a new key with another row's unique value would update that row); on Postgres the conflict target is
/// the primary key.
#[test]
fn an_upsert_does_not_fit_an_entity_with_a_unique_index_on_mysql() {
    let fields: FieldMap = [
        ("lockId", Value::string("L1")),
        ("repositoryId", Value::string("R1")),
        ("lockPath", Value::string("a.bin")),
    ]
    .into_iter()
    .map(|(name, value)| (Arc::from(name), value))
    .collect();
    for (dialect, fits) in [("postgres", true), ("mysql", false)] {
        let (facade, _tape) = open_store(false, dialect);
        assert_eq!(
            facade.upsert_fits("moqui.resource.GitLfsLock", &fields).unwrap(),
            fits,
            "{dialect}"
        );
    }
}

#[test]
fn create_or_update_on_an_existing_row_is_one_update_with_the_same_sql_as_update() {
    let (facade, tape) = open_store_no_upsert(true, "postgres");
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
fn create_or_update_on_a_new_row_updates_nothing_then_inserts_as_before() {
    let (facade, tape) = open_store_no_upsert(false, "postgres");
    facade.create_value(&test_value(&facade)).unwrap();
    let direct_insert = tape.lock().unwrap().sql.clone();
    let direct_params = tape.lock().unwrap().params[0].clone();
    assert_eq!(direct_insert.len(), 1);
    {
        let mut tape = tape.lock().unwrap();
        tape.exists = false;
        tape.sql.clear();
        tape.params.clear();
    }
    facade.create_or_update_value(&test_value(&facade)).unwrap();
    let kinds: Vec<String> = writes(&tape).into_iter().map(|(kind, _)| kind).collect();
    // The UPDATE that matched no row stands for the read: the transaction has no such row.
    assert_eq!(kinds, ["UPDATE", "INSERT"], "{kinds:?}");
    let tape = tape.lock().unwrap();
    // The create's insert, made one that does nothing on the key (Parity-Diffs, atomic create-or-update).
    assert_eq!(
        tape.sql[1],
        format!("{} ON CONFLICT (\"test_id\") DO NOTHING", direct_insert[0])
    );
    // The key and the field; the stamp is the clock's.
    assert_eq!(tape.params[1][..2], direct_params[..2]);
    assert!(tape.exists);
}

/// Another transaction creates the row between the UPDATE that found none and the insert: the insert does nothing and
/// the row is updated, with the statement an update of a row that is there writes, so the call does not fail on the key.
/// On MySQL the row is read (locking) before the update, as the insert's count does not tell the key from another
/// unique index.
#[test]
fn create_or_update_that_loses_the_race_for_a_new_row_updates_it() {
    for (dialect, expected) in [
        ("postgres", vec!["UPDATE", "INSERT", "UPDATE"]),
        ("mysql", vec!["SELECT", "INSERT", "SELECT", "UPDATE"]),
    ] {
        let (facade, tape) = open_store_no_upsert(true, dialect);
        facade.update_value(&test_value(&facade)).unwrap();
        let direct_update = tape.lock().unwrap().sql[0].clone();
        {
            let mut tape = tape.lock().unwrap();
            tape.exists = false;
            tape.created_by_another = true;
            tape.sql.clear();
            tape.params.clear();
        }
        facade.create_or_update_value(&test_value(&facade)).unwrap();
        let kinds: Vec<String> = writes(&tape).into_iter().map(|(kind, _)| kind).collect();
        assert_eq!(kinds, expected, "{dialect}: {kinds:?}");
        assert_eq!(tape.lock().unwrap().sql.last(), Some(&direct_update), "{dialect}");
    }
}

/// On MySQL an insert that does nothing because another row has the same value of a unique index (no row has the key)
/// fails as that conflict, not as an update of a missing row.
#[test]
fn create_or_update_that_meets_another_unique_value_on_mysql_names_the_conflict() {
    let (facade, tape) = open_store_no_upsert(false, "mysql");
    tape.lock().unwrap().unique_conflict = true;
    let err = facade.create_or_update_value(&test_value(&facade)).unwrap_err();
    assert!(
        err.to_string()
            .contains("another row has the same value of a unique index"),
        "{err}"
    );
    let kinds: Vec<String> = writes(&tape).into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(kinds, ["SELECT", "INSERT", "SELECT"], "{kinds:?}");
}

#[test]
fn create_or_update_flag_selects_java_read_first_and_defaults_to_one_upsert() {
    const CHILD: &str = "MOQUI_TEST_CREATE_OR_UPDATE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // Run each setting in its own process; process properties must not affect parallel tests.
        for setting in [None, Some("false"), Some("true")] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "entity::tests::create_or_update_flag_selects_java_read_first_and_defaults_to_one_upsert",
            ]);
            command.env(CHILD, "true").env_remove("create-or-update");
            if let Some(setting) = setting {
                command.env("create-or-update", setting);
            }
            assert!(command.status().unwrap().success(), "setting {setting:?}");
        }
        return;
    }
    let java = moqui_lib::env::get_prop_or_env("create-or-update") == "true";
    for (upsert_fits, exists) in [(true, false), (true, true), (false, false), (false, true)] {
        let (facade, tape) = if upsert_fits {
            open_store(exists, "postgres")
        } else {
            open_store_no_upsert(exists, "postgres")
        };
        facade.create_or_update_value(&test_value(&facade)).unwrap();
        let kinds: Vec<String> = writes(&tape).into_iter().map(|(kind, _)| kind).collect();
        let expected = match (java, upsert_fits, exists) {
            (true, _, false) => vec!["SELECT", "INSERT"],
            (true, _, true) => vec!["SELECT", "UPDATE"],
            (false, true, _) => vec!["INSERT"],
            (false, false, false) => vec!["UPDATE", "INSERT"],
            (false, false, true) => vec!["UPDATE"],
        };
        assert_eq!(
            kinds, expected,
            "java={java}, upsert_fits={upsert_fits}, exists={exists}"
        );
    }
}

#[test]
fn create_or_update_keeps_find_then_write_for_dialects_that_do_not_report_matched_rows() {
    let (facade, tape) = open_store_no_upsert(true, "mysql");
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
    let unauthorized = |err: &Error| crate::error::ErrorKind::of(err) == crate::error::ErrorKind::Unauthorized;

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
    <entity entity-name="ParityAudited" package="moqui.test">
        <field name="auditedId" type="id" is-pk="true"/>
        <field name="status" type="id" enable-audit-log="true"/>
        <field name="note" type="text-medium" enable-audit-log="true"/>
        <field name="other" type="text-medium"/>
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

/// P1-15: on an entity whose update does not read the row, a stamp the caller set on a found value is kept, and a value
/// whose stamp was not touched gets the update's.
#[test]
fn a_stamp_set_on_a_found_value_is_kept_without_a_row_read() {
    let facade = crate::testing::open();
    let entity = "moqui.test.TestEntity";
    let mut row = facade.make_value(entity).unwrap();
    row.set("testId", Value::string("STAMPED"));
    row.create(&facade).unwrap();
    let read = || {
        facade
            .find(entity)
            .unwrap()
            .eq("testId", Value::string("STAMPED"))
            .use_cache(false)
            .one()
            .unwrap()
            .unwrap()
    };
    let chosen = moqui_lib::object::basic_convert(&Value::I64(1_000_000_000_000), "java.sql.Timestamp").value;
    let mut found = read();
    found.set("testMedium", Value::string("chosen stamp"));
    found.set("lastUpdatedStamp", chosen.clone());
    found.update(&facade).unwrap();
    assert_eq!(read().get("lastUpdatedStamp"), Some(&chosen));

    let mut found = read();
    found.set("testMedium", Value::string("own stamp"));
    found.update(&facade).unwrap();
    let after = read();
    assert_eq!(after.get("testMedium"), Some(&Value::string("own stamp")));
    assert_ne!(after.get("lastUpdatedStamp"), Some(&chosen));
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

fn test_entity_read(facade: &EntityFacade, test_id: &str) -> super::EntityValue {
    facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string(test_id))
        .use_cache(false)
        .one()
        .unwrap()
        .unwrap()
}

fn create_test_entity(facade: &EntityFacade, test_id: &str, medium: &str, short: &str) {
    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string(test_id));
    row.set("testMedium", Value::string(medium));
    row.set("value", Value::string(short));
    row.set("testNumberInteger", Value::I64(1));
    row.create(facade).unwrap();
}

/// The statements on `tape` other than reads.
fn tape_writes(tape: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    tape.lock()
        .unwrap()
        .iter()
        .filter(|sql| !sql.starts_with("SELECT"))
        .cloned()
        .collect()
}

/// The `SET` part of an update statement.
fn set_part(sql: &str) -> &str {
    sql.split(" WHERE ").next().unwrap_or(sql)
}

/// A lost update: two values read the same row and each changes a different field. As an update writes only the
/// fields its value changed, both changes are kept.
#[test]
fn two_values_read_together_each_keep_the_field_they_changed() {
    let facade = crate::testing::open();
    create_test_entity(&facade, "LOST", "medium", "short");
    let (mut first, mut second) = (test_entity_read(&facade, "LOST"), test_entity_read(&facade, "LOST"));
    first.set("testMedium", Value::string("first"));
    second.set("value", Value::string("second"));
    assert!(first.is_modified() && first.is_field_modified("testMedium") && !first.is_field_modified("value"));
    assert_eq!(first.original_db_value("testMedium"), Some(Value::string("medium")));
    first.update(&facade).unwrap();
    second.update(&facade).unwrap();
    let stored = test_entity_read(&facade, "LOST");
    assert_eq!(stored.get("testMedium"), Some(&Value::string("first")));
    assert_eq!(stored.get("value"), Some(&Value::string("second")));
    // A clone keeps what the value read; a copy for another row (a changed key) writes every field it holds.
    let mut copy = second.clone();
    copy.set("value", Value::string("copied"));
    assert!(copy.is_field_modified("value") && !copy.is_field_modified("testMedium"));
    copy.set("testId", Value::string("COPY"));
    copy.create(&facade).unwrap();
    assert_eq!(
        test_entity_read(&facade, "COPY").get("testMedium"),
        Some(&Value::string("medium"))
    );
}

/// A value read and not changed, set to what it held (a number as text too), or changed and set back, updates
/// nothing: no statement and no audit row; with nothing set it does not even read the row its audit log would compare
/// with. After an update the value holds what was written, so a field set back to what it was read with is written.
#[test]
fn an_update_of_a_found_value_with_no_change_writes_nothing() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    facade.set_disable_audit_log(false);
    let entity = "moqui.service.message.SystemMessage";
    let mut row = facade.make_value(entity).unwrap();
    row.set("systemMessageId", Value::string("NC"));
    row.set("statusId", Value::string("SmsgReceived"));
    row.set("messageText", Value::string("text"));
    row.set("failCount", Value::I64(1));
    row.create(&facade).unwrap();
    let audits = || {
        facade
            .find("moqui.entity.EntityAuditLog")
            .unwrap()
            .eq("changedEntityName", Value::string(entity))
            .use_cache(false)
            .list()
            .unwrap()
    };
    let audited = audits().len();
    let read = || {
        facade
            .find(entity)
            .unwrap()
            .eq("systemMessageId", Value::string("NC"))
            .use_cache(false)
            .one()
            .unwrap()
            .unwrap()
    };
    let mut found = read();
    tape.lock().unwrap().clear();
    found.update(&facade).unwrap();
    assert!(tape.lock().unwrap().is_empty(), "{:?}", tape.lock().unwrap());
    found.set("messageText", Value::string("text"));
    found.set("failCount", Value::string("1"));
    found.set("statusId", Value::string("SmsgConsumed"));
    found.set("statusId", Value::string("SmsgReceived"));
    assert!(!found.is_modified());
    found.update(&facade).unwrap();
    found.create_or_update(&facade).unwrap();
    assert!(tape_writes(&tape).is_empty(), "{:?}", tape_writes(&tape));
    assert_eq!(audits().len(), audited);

    found.set("messageText", Value::string("changed"));
    found.update(&facade).unwrap();
    let written = tape_writes(&tape);
    assert_eq!(written.len(), 1, "{written:?}");
    let set = set_part(&written[0]);
    assert!(
        set.contains("message_text") && set.contains("last_updated_stamp"),
        "{set}"
    );
    assert!(!set.contains("status_id") && !set.contains("fail_count"), "{set}");
    found.set("messageText", Value::string("text"));
    found.update(&facade).unwrap();
    assert_eq!(read().get("messageText"), Some(&Value::string("text")));
    assert_eq!(audits().len(), audited, "messageText is not audited");
}

/// The audit log of an update has a row for each audited field the value changed, with the stored value it replaced
/// (read before the write, as Java's `dbValueMap`), and none for a field it did not change: a value read before
/// another update neither writes back nor audits the field that update changed.
#[test]
fn an_update_audits_the_fields_it_changed_with_the_stored_old_values() {
    let fixture = ParityFixture::new();
    let facade = fixture.open(None);
    facade.set_disable_audit_log(false);
    let entity = "moqui.test.ParityAudited";
    let mut row = facade.make_value(entity).unwrap();
    row.set("auditedId", Value::string("A1"));
    row.set("status", Value::string("A"));
    row.set("note", Value::string("n1"));
    row.create(&facade).unwrap();
    let read = || {
        facade
            .find(entity)
            .unwrap()
            .eq("auditedId", Value::string("A1"))
            .use_cache(false)
            .one()
            .unwrap()
            .unwrap()
    };
    let updates = |field: &str| -> Vec<(Option<String>, Option<String>)> {
        let text = |row: &super::EntityValue, name: &str| row.get(name).and_then(Value::as_str).map(str::to_string);
        facade
            .find("moqui.entity.EntityAuditLog")
            .unwrap()
            .eq("changedEntityName", Value::string(entity))
            .eq("changedFieldName", Value::string(field))
            .use_cache(false)
            .list()
            .unwrap()
            .iter()
            .filter(|row| row.get("oldValueText").is_some_and(|old| !old.is_null()))
            .map(|row| (text(row, "oldValueText"), text(row, "newValueText")))
            .collect()
    };
    let (mut first, mut stale) = (read(), read());
    first.set("note", Value::string("n2"));
    first.update(&facade).unwrap();
    stale.set("status", Value::string("B"));
    stale.update(&facade).unwrap();
    let stored = read();
    assert_eq!(stored.get("note"), Some(&Value::string("n2")));
    assert_eq!(stored.get("status"), Some(&Value::string("B")));
    assert_eq!(updates("note"), [(Some("n1".to_string()), Some("n2".to_string()))]);
    assert_eq!(updates("status"), [(Some("A".to_string()), Some("B".to_string()))]);

    // The old value is the stored one, not the one the value was read with.
    let mut later = read();
    let mut other = facade.make_value(entity).unwrap();
    other.set("auditedId", Value::string("A1"));
    other.set("status", Value::string("C"));
    other.update(&facade).unwrap();
    later.set("status", Value::string("D"));
    later.update(&facade).unwrap();
    let status = updates("status");
    assert!(
        status.contains(&(Some("C".to_string()), Some("D".to_string()))),
        "{status:?}"
    );
    assert!(
        !status.contains(&(Some("B".to_string()), Some("D".to_string()))),
        "{status:?}"
    );
    assert_eq!(updates("note").len(), 1);
}

/// A value made with `makeValue` and its key updates the fields it holds, and leaves the others as stored; after the
/// write it updates only the fields set since.
#[test]
fn a_made_value_updates_the_fields_it_holds_then_those_set_since() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    create_test_entity(&facade, "MADE", "medium", "short");
    tape.lock().unwrap().clear();
    let mut made = facade.make_value("moqui.test.TestEntity").unwrap();
    made.set("testId", Value::string("MADE"));
    made.set("testMedium", Value::string("made"));
    assert!(made.is_field_modified("testMedium") && !made.is_field_modified("value"));
    made.update(&facade).unwrap();
    made.set("value", Value::string("later"));
    made.update(&facade).unwrap();
    let written = tape_writes(&tape);
    assert_eq!(written.len(), 2, "{written:?}");
    assert!(set_part(&written[0]).contains("test_medium") && !set_part(&written[0]).contains("\"value\""));
    assert!(set_part(&written[1]).contains("\"value\"") && !set_part(&written[1]).contains("test_medium"));
    let stored = test_entity_read(&facade, "MADE");
    assert_eq!(stored.get("testMedium"), Some(&Value::string("made")));
    assert_eq!(stored.get("value"), Some(&Value::string("later")));
}

/// `createOrUpdate` of a found value updates the fields it changed; of a value whose row is gone it creates the row
/// with every field the value holds. Data load still writes the fields a record names and leaves the others.
#[test]
fn create_or_update_and_data_load_write_what_they_name() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    create_test_entity(&facade, "COU", "medium", "short");
    let mut found = test_entity_read(&facade, "COU");
    let mut other = test_entity_read(&facade, "COU");
    other.set("value", Value::string("other"));
    other.update(&facade).unwrap();
    tape.lock().unwrap().clear();
    found.set("testMedium", Value::string("found"));
    found.create_or_update(&facade).unwrap();
    let written = tape_writes(&tape);
    assert_eq!(written.len(), 1, "{written:?}");
    assert!(!set_part(&written[0]).contains("\"value\""), "{}", written[0]);
    let stored = test_entity_read(&facade, "COU");
    assert_eq!(stored.get("testMedium"), Some(&Value::string("found")));
    assert_eq!(stored.get("value"), Some(&Value::string("other")));

    stored.delete(&facade).unwrap();
    let mut gone = stored.clone();
    gone.set("testMedium", Value::string("again"));
    gone.create_or_update(&facade).unwrap();
    let again = test_entity_read(&facade, "COU");
    assert_eq!(again.get("testMedium"), Some(&Value::string("again")));
    assert_eq!(again.get("value"), Some(&Value::string("other")));

    super::load_feed::load_entity_xml_text(
        &facade,
        "named-fields",
        r#"<entity-facade-xml><moqui.test.TestEntity testId="COU" testMedium="loaded"/></entity-facade-xml>"#,
        &super::load_feed::LoadOptions::default(),
    )
    .unwrap();
    let loaded = test_entity_read(&facade, "COU");
    assert_eq!(loaded.get("testMedium"), Some(&Value::string("loaded")));
    assert_eq!(loaded.get("value"), Some(&Value::string("other")));
}

/// `updateAll` sets the fields on each row found and updates a row only when they change it, writing only them.
#[test]
fn update_all_writes_only_the_fields_it_sets_on_the_rows_they_change() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    create_test_entity(&facade, "UA1", "same", "a");
    create_test_entity(&facade, "UA2", "other", "b");
    tape.lock().unwrap().clear();
    let updated = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .update_all([("testMedium".to_string(), Value::string("same"))])
        .unwrap();
    assert_eq!(updated, 1);
    let written = tape_writes(&tape);
    assert_eq!(written.len(), 1, "{written:?}");
    assert!(!set_part(&written[0]).contains("\"value\""), "{}", written[0]);
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
        .find(|node| node.attribute_is("group-name", "transactional"))
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
/// A view built at runtime has its aliases as fields in the entity JSON schema and in a CSV of its rows (P5-19).
#[test]
fn a_db_view_entity_has_its_aliases_in_the_schema_and_the_csv() {
    let facade = crate::testing::open();
    seed_enumerations(&facade);
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
    let schema = super::schema::document(&facade, &["EnumWithType".to_string()], false).unwrap();
    let Value::Map(root) = &schema else {
        panic!("not a map: {schema:?}");
    };
    let Some(Value::Map(properties)) = root.get("properties") else {
        panic!("no properties in {schema:?}");
    };
    let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["description", "enumId", "typeDescription"]);

    let mut writer = super::data_writer::EntityDataWriter::new(&facade);
    writer
        .entity_name("EnumWithType")
        .file_type(super::data_writer::DataFileType::Csv);
    let (csv, _) = writer.text().unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "test.dbview.EnumWithType,export,enumId");
    let header: Vec<&str> = lines[1].split(',').collect();
    let row = lines
        .iter()
        .find(|line| {
            line.split(',')
                .nth(header.iter().position(|name| *name == "enumId").unwrap())
                == Some("E1")
        })
        .unwrap_or_else(|| panic!("no E1 in {csv}"));
    let cells: Vec<&str> = row.split(',').collect();
    let at = |name: &str| cells[header.iter().position(|column| *column == name).unwrap()];
    assert_eq!(
        (at("description"), at("typeDescription")),
        ("Enum E1", "Type one"),
        "{csv}"
    );
}

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

/// F5 (plans/design-dev-aide.md): a relink reads the entity files again and swaps the model in. A call that took the
/// old model keeps it; finds on other threads during the relinks see one model or the other, whole; the changed
/// entity's table is checked with its DDL answered; a file that does not parse keeps the model there was.
#[test]
fn a_relink_swaps_the_model_under_finds_in_flight() {
    let dir = std::env::temp_dir().join(format!("moqui-relink-model-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("entity")).unwrap();
    let file = dir.join("entity/RelinkEntities.xml");
    let entities = |extra: &str| {
        format!(
            "<entities><entity entity-name=\"RelinkThing\" package=\"relink\"><field name=\"thingId\" type=\"id\" is-pk=\"true\"/>{extra}</entity></entities>"
        )
    };
    std::fs::write(&file, entities("")).unwrap();
    let tape = Arc::new(Mutex::new(Tape {
        sql: Vec::new(),
        password: None,
    }));
    let driver: Arc<dyn DbDriver> = Arc::new(RecDriver {
        tape: Arc::clone(&tape),
    });
    let transactions = TransactionManager::new(
        driver,
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            acquire_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .unwrap();
    let facade = EntityFacade::open_components(
        &framework_dir(),
        &[dir.join("entity")],
        "transactional",
        vec![EntityGroup::new("transactional", "postgres", transactions)],
    )
    .unwrap();
    facade.set_disable_fk_create(true);
    let fields = |facade: &EntityFacade| {
        facade
            .with_entity("relink.RelinkThing", |entity, _, _| Ok(entity.fields.len()))
            .unwrap()
    };
    // the field and lastUpdatedStamp
    let base = fields(&facade);
    let held = facade.inner.model.model();

    std::fs::write(&file, entities("<field name=\"dueDate\" type=\"date\"/>")).unwrap();
    let report = facade.relink(std::slice::from_ref(&file)).unwrap();
    assert_eq!(report.changed, vec!["relink.RelinkThing".to_string()]);
    assert!(
        report
            .ddl
            .iter()
            .any(|sql| sql.contains("relink_thing") && sql.contains("due_date")),
        "{:?}",
        report.ddl
    );
    assert_eq!(fields(&facade), base + 1);
    assert_eq!(
        held.entity("relink.RelinkThing").unwrap().fields.len(),
        base,
        "the call that took the old model keeps it"
    );
    drop(held);

    // finds on other threads while the model is relinked back and forth
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let facade = facade.clone();
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut seen = 0usize;
                let base = base;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let count = facade
                        .with_entity("relink.RelinkThing", |entity, _, model| {
                            // the definition and the model it came with are one model
                            assert_eq!(
                                model.entity("relink.RelinkThing").unwrap().fields.len(),
                                entity.fields.len()
                            );
                            Ok(entity.fields.len())
                        })
                        .unwrap();
                    assert!(count == base + 1 || count == base + 2, "{count}");
                    seen += 1;
                }
                seen
            })
        })
        .collect();
    for round in 0..10 {
        let extra = if round % 2 == 0 {
            "<field name=\"dueDate\" type=\"date\"/><field name=\"note\" type=\"text-medium\"/>"
        } else {
            "<field name=\"dueDate\" type=\"date\"/>"
        };
        std::fs::write(&file, entities(extra)).unwrap();
        facade.relink(std::slice::from_ref(&file)).unwrap();
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for reader in readers {
        assert!(reader.join().unwrap() > 0);
    }
    assert_eq!(fields(&facade), base + 1);

    std::fs::write(&file, "<entities><entity entity-name=").unwrap();
    assert!(facade.relink(std::slice::from_ref(&file)).is_err());
    assert_eq!(
        fields(&facade),
        base + 1,
        "a file that does not parse keeps the model there was"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
