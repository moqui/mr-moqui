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

use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_db::dialect::Dialects;
use moqui_db::model::EntityModel;
use moqui_db::sql::insert_or_update;
use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::feed::{probe_query, Touch, TOUCHES};
use super::{
    conf_data_locations, eca_rules, error_chain, feed_service_names, load_entity_data, match_rules,
    register_feed_insert, LoadOptions,
};
use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};
use crate::entity::{EntityFacade, EntityGroup};
use crate::transaction::{PoolConfig, TransactionManager};

struct NoCancel;

impl DbCancel for NoCancel {
    fn cancel(&self) -> Result<(), Error> {
        Ok(())
    }
}

struct EmptyConn;

impl DbConnection for EmptyConn {
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

struct EmptyDriver;

impl DbDriver for EmptyDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(EmptyConn))
    }
}

struct Script {
    inserts: Mutex<Vec<Vec<Value>>>,
    missing_feed: bool,
    /// Entity inserts fail while set, as a duplicate key would.
    fail_inserts: AtomicBool,
    /// Queries of the feed and document definition tables.
    definition_reads: AtomicUsize,
    /// Savepoints set, one for each feed probe.
    savepoints: AtomicUsize,
    /// Rolling back to a savepoint fails while set.
    broken_rollback: AtomicBool,
    /// `DataFeedPending` inserts fail while set.
    fail_pending: AtomicBool,
}

fn new_script(missing_feed: bool, fail_inserts: bool) -> Arc<Script> {
    Arc::new(Script {
        inserts: Mutex::new(Vec::new()),
        missing_feed,
        fail_inserts: AtomicBool::new(fail_inserts),
        definition_reads: AtomicUsize::new(0),
        savepoints: AtomicUsize::new(0),
        broken_rollback: AtomicBool::new(false),
        fail_pending: AtomicBool::new(false),
    })
}

struct ScriptConn {
    script: Arc<Script>,
}

impl DbConnection for ScriptConn {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, Error> {
        if sql.contains("data_feed_pending") {
            if self.script.fail_pending.load(Ordering::SeqCst) {
                return Err(Error::new("postgres execute: deadlock detected"));
            }
            self.script.inserts.lock().expect("inserts").push(params.to_vec());
        } else if sql.starts_with("INSERT") && self.script.fail_inserts.load(Ordering::SeqCst) {
            return Err(Error::new("duplicate key value violates unique constraint"));
        }
        Ok(1)
    }
    fn query(&mut self, sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        if (sql.contains("data_feed") || sql.contains("data_document")) && !sql.contains("data_feed_pending") {
            self.script.definition_reads.fetch_add(1, Ordering::SeqCst);
        }
        if self.script.missing_feed && sql.contains("data_feed") {
            let source = std::io::Error::other("relation \"DataFeed\" does not exist");
            return Err(Error::new("postgres query").with_source(source));
        }
        if sql.contains("xmin") {
            return Ok(vec![db_row(&[("fooId", "F1")])]);
        }
        // The document has one field: a write that sets it is fed, as Java's `dataFeedCheckAndRegister`.
        if sql.contains("data_document_field") {
            return Ok(vec![db_row(&[("dataDocumentId", "DOC1"), ("fieldPath", "fooId")])]);
        }
        if sql.contains("data_document_condition") {
            return Ok(Vec::new());
        }
        if sql.contains("data_feed_document") {
            return Ok(vec![db_row(&[("dataFeedId", "FEED1"), ("dataDocumentId", "DOC1")])]);
        }
        if sql.contains("data_document") {
            return Ok(vec![db_row(&[
                ("dataDocumentId", "DOC1"),
                ("primaryEntityName", "moqui.test.Foo"),
            ])]);
        }
        if sql.contains("data_feed") {
            return Ok(vec![db_row(&[
                ("dataFeedId", "FEED1"),
                ("feedReceiveServiceName", "index#DataDocuments"),
                ("feedDeleteServiceName", "delete#DataDocument"),
            ])]);
        }
        Ok(Vec::new())
    }
    fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query(sql, params)
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
        self.script.savepoints.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        if self.script.broken_rollback.load(Ordering::SeqCst) {
            return Err(Error::new("current transaction is aborted"));
        }
        Ok(())
    }
    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

struct ScriptDriver {
    script: Arc<Script>,
}

impl DbDriver for ScriptDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(ScriptConn {
            script: Arc::clone(&self.script),
        }))
    }
}

fn db_row(pairs: &[(&str, &str)]) -> DbRow {
    DbRow {
        columns: pairs
            .iter()
            .map(|(name, value)| ((*name).into(), Value::string(*value)))
            .collect(),
    }
}

fn open_with(driver: Arc<dyn DbDriver>) -> EntityFacade {
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

fn open_empty() -> EntityFacade {
    open_with(Arc::new(EmptyDriver))
}

fn framework_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework")
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> (PathBuf, DirGuard) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("moqui-load-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    let guard = DirGuard(path.clone());
    (path, guard)
}

/// A zip of `entries`, each written with `method`.
fn zip_of(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        writer
            .start_file(*name, SimpleFileOptions::default().compression_method(method))
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn eca_rules_are_stored_and_disable_matches_nothing() {
    let facade = open_empty();
    let rules = eca_rules(&facade, "moqui.screen.form.DbForm");
    let form = rules.iter().find(|rule| rule.id == "DbFormCache").expect("DbFormCache");
    assert!(form.actions.contains("ec.cache"), "{}", form.actions);
    assert!(form.on_create && form.on_update && form.on_delete);
    let member = eca_rules(&facade, "moqui.security.UserGroupMember");
    let privileged = member
        .iter()
        .find(|rule| rule.id == "UserGroupMemberPrivileged")
        .expect("privileged");
    assert!(privileged.run_before);
    assert!(privileged.actions.contains("assertPrivilegedGroupMembership"));
    facade.begin("moqui.screen.form.DbForm", None).unwrap();
    let ids = match_rules(&facade, "moqui.screen.form.DbForm", "create");
    assert!(ids.iter().any(|id| id == "DbFormCache"));
    facade.set_disable_entity_eca(true);
    assert!(match_rules(&facade, "moqui.screen.form.DbForm", "update").is_empty());
    facade.rollback("moqui.screen.form.DbForm").unwrap();
}

#[test]
fn conf_locations_name_the_seed_files() {
    let locations = conf_data_locations(&open_empty()).unwrap();
    assert!(locations.iter().any(|location| location.contains("MoquiSetupData.xml")));
}

#[test]
fn loader_reads_seed_data_inside_entity_definitions() {
    let facade = open_empty();
    let (dir, _guard) = temp_dir();
    let xml = dir.join("entities.xml");
    std::fs::write(
        &xml,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<entities xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <entity entity-name="One" package="moqui.test">
<field name="testId" type="id" is-pk="true"/>
<field name="testMedium" type="text-medium"/>
<relationship type="one" related="moqui.test.Foo"/>
<seed-data>
  <moqui.test.TestEntity testId="S1" testMedium="one"/>
</seed-data>
  </entity>
  <entity entity-name="Two" package="moqui.test">
<field name="testId" type="id" is-pk="true"/>
<seed-data>
  <moqui.test.TestEntity testId="S2" testMedium="two"/>
</seed-data>
  </entity>
</entities>"#,
    )
    .unwrap();
    let loaded = load_entity_data(&facade, xml.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(loaded.records, 2);
    let demo = LoadOptions {
        data_types: vec!["demo".to_string()],
        ..LoadOptions::default()
    };
    let skipped = load_entity_data(&facade, xml.to_str().unwrap(), &demo).unwrap();
    assert_eq!(skipped.records, 0);

    let bare = dir.join("bare.xml");
    std::fs::write(
        &bare,
        r#"<entities><entity entity-name="Only" package="moqui.test"><field name="testId" type="id" is-pk="true"/></entity></entities>"#,
    )
    .unwrap();
    let none = load_entity_data(&facade, bare.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(none.records, 0);

    // As Java's SAX handler, any other root is not an error: only the data elements inside it load.
    let other = dir.join("other.xml");
    std::fs::write(
        &other,
        r#"<services><service verb="noop"/><entity-facade-xml type="demo"><moqui.test.TestEntity testId="O1"/></entity-facade-xml></services>"#,
    )
    .unwrap();
    let seed_only = LoadOptions {
        data_types: vec!["seed".to_string()],
        ..LoadOptions::default()
    };
    assert_eq!(
        load_entity_data(&facade, other.to_str().unwrap(), &seed_only)
            .unwrap()
            .records,
        0
    );
    assert_eq!(
        load_entity_data(&facade, other.to_str().unwrap(), &demo)
            .unwrap()
            .records,
        1
    );
}

#[test]
fn nested_many_relationship_inherits_parent_primary_key() {
    let facade = open_empty();
    let xml = r#"<moqui.service.job.ServiceJob jobName="clean_test" serviceName="example" cronExpression="0 0 2 * * ?">
  <parameters parameterName="daysToKeep" parameterValue="1"/>
  <parameters jobName="other" parameterName="kept" parameterValue="2"/>
</moqui.service.job.ServiceJob>"#;
    let node = MNode::parse_text("job", xml).unwrap();
    let record = super::loader::parse_record(&facade, "moqui.service.job.ServiceJob", &node).unwrap();
    assert_eq!(record.children.len(), 2);
    assert_eq!(record.children[0].entity, "moqui.service.job.ServiceJobParameter");
    assert_eq!(
        record.children[0].fields.get("jobName").and_then(Value::as_str),
        Some("clean_test")
    );
    assert_eq!(
        record.children[0].fields.get("parameterName").and_then(Value::as_str),
        Some("daysToKeep")
    );
    assert_eq!(
        record.children[1].fields.get("jobName").and_then(Value::as_str),
        Some("other")
    );
}

#[test]
fn loader_streams_attributes_comments_and_json_arrays() {
    let element = r#"<moqui.test.TestEntity testId="A1" testMedium="a>b"/>"#;
    let end = super::xml::xml_element_end(element, true).unwrap().unwrap();
    assert_eq!(end, element.len());
    let node = MNode::parse_text("element", element).unwrap();
    assert_eq!(node.attribute("testMedium").as_deref(), Some("a>b"));

    let quoted = r#"{"testMedium":"say \"hi\""}"#;
    let value_end = super::json::json_value_end(quoted, true).unwrap().unwrap();
    assert_eq!(value_end, quoted.len());
    let super::json::Json::Object(pairs) = super::json::parse_json(quoted).unwrap() else {
        panic!("quoted json is not an object");
    };
    let medium = pairs.iter().find(|(key, _)| key == "testMedium").expect("testMedium");
    assert!(matches!(&medium.1, super::json::Json::String(text) if text == "say \"hi\""));

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- see <http://creativecommons.org/publicdomain/zero/1.0/> -->
<!DOCTYPE entity-facade-xml [
  <!ELEMENT entity-facade-xml ANY>
]>
<entity-facade-xml type="seed">
  <moqui.test.TestEntity testId="A1" testMedium="a>b"/>
  <moqui.test.TestEntity testId="A2" testMedium="plain"/>
</entity-facade-xml>
"#;
    let facade = open_empty();
    let xml_reader = std::io::BufReader::with_capacity(1, std::io::Cursor::new(xml.as_bytes()));
    let xml_loaded = super::xml::load_xml_reader(&facade, "chunk.xml", xml_reader, &LoadOptions::default()).unwrap();
    assert_eq!(xml_loaded, 2);

    let json = r#"[{"_dataType":"seed"},{"_entity":"moqui.test.TestEntity","testId":"B1","testMedium":"one"},{"_entity":"moqui.test.TestEntity","testId":"B2","testMedium":"say \"hi\""}]"#;
    let json_reader = std::io::BufReader::with_capacity(1, std::io::Cursor::new(json.as_bytes()));
    let json_loaded =
        super::json::load_json_reader(&facade, "chunk.json", json_reader, &LoadOptions::default()).unwrap();
    assert_eq!(json_loaded, 2);

    let (dir, _guard) = temp_dir();
    let json_path = dir.join("rows.json");
    std::fs::write(&json_path, json).unwrap();
    let from_file = load_entity_data(&facade, json_path.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(from_file.records, 2);
}

#[test]
fn loader_scans_one_large_element_in_linear_time() {
    let mut medium = String::new();
    while medium.len() < 8 * 1024 * 1024 {
        medium.push_str("0123456789abcdef");
    }
    let xml = format!(
        "<entity-facade-xml type=\"seed\"><moqui.test.TestEntity testId=\"BIG\" testMedium=\"{medium}\"/></entity-facade-xml>"
    );
    let facade = open_empty();
    let started = std::time::Instant::now();
    let reader = std::io::BufReader::with_capacity(4096, std::io::Cursor::new(xml.as_bytes()));
    let loaded = super::xml::load_xml_reader(&facade, "big.xml", reader, &LoadOptions::default()).unwrap();
    assert_eq!(loaded, 1);
    assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
}

#[test]
fn zip_entries_stream_past_the_old_inflate_limit() {
    // One element larger than the 16 MiB the hand-written inflate held, deflated.
    let mut medium = String::with_capacity(17 * 1024 * 1024);
    while medium.len() < 17 * 1024 * 1024 {
        medium.push_str("0123456789abcdef");
    }
    let xml = format!(
        "<entity-facade-xml type=\"seed\"><moqui.test.TestEntity testId=\"BIG\" testMedium=\"{medium}\"/>\
         <moqui.test.TestEntity testId=\"SMALL\" testMedium=\"small\"/></entity-facade-xml>"
    );
    drop(medium);
    let csv = b"moqui.test.TestEntity,seed\ntestId,testMedium\nC1,csv\n";
    let json = br#"[{"_entity":"moqui.test.TestEntity","testId":"J1","testMedium":"json"}]"#;
    let inner = zip_of(&[("inner.csv", csv)], CompressionMethod::Deflated);
    let archive = zip_of(
        &[
            ("data/big.xml", xml.as_bytes()),
            ("data/rows.json", json),
            ("data/inner.zip", &inner),
            ("data/notes.txt", b"not data"),
        ],
        CompressionMethod::Deflated,
    );
    assert!(archive.len() < xml.len() / 100, "the entry compressed");
    let (dir, _guard) = temp_dir();
    let path = dir.join("big.zip");
    std::fs::write(&path, archive).unwrap();
    let facade = open_empty();
    let loaded = load_entity_data(&facade, path.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(loaded.records, 4);
}

#[test]
fn zip_nesting_and_entry_names_are_bounded() {
    let csv = b"moqui.test.TestEntity,seed\ntestId,testMedium\nN1,nested\n";
    let mut archive = zip_of(&[("rows.csv", csv)], CompressionMethod::Stored);
    for _ in 0..6 {
        archive = zip_of(&[("inner.zip", &archive)], CompressionMethod::Stored);
    }
    let (dir, _guard) = temp_dir();
    let deep = dir.join("deep.zip");
    std::fs::write(&deep, archive).unwrap();
    let facade = open_empty();
    let err = load_entity_data(&facade, deep.to_str().unwrap(), &LoadOptions::default()).unwrap_err();
    assert!(err.to_string().contains("too deep"), "{err}");

    let escape = dir.join("escape.zip");
    std::fs::write(&escape, zip_of(&[("../rows.csv", csv)], CompressionMethod::Stored)).unwrap();
    let err = load_entity_data(&facade, escape.to_str().unwrap(), &LoadOptions::default()).unwrap_err();
    assert!(err.to_string().contains("not a safe path"), "{err}");

    let mut corrupt = zip_of(&[("rows.csv", csv)], CompressionMethod::Stored);
    // The stored data starts after the 30-byte local header and the name; flip a byte of it so the CRC fails.
    corrupt[30 + "rows.csv".len() + 2] ^= 0x20;
    let bad = dir.join("bad.zip");
    std::fs::write(&bad, corrupt).unwrap();
    assert!(load_entity_data(&facade, bad.to_str().unwrap(), &LoadOptions::default()).is_err());
}

#[test]
fn json_decodes_surrogate_pairs_and_bounds_nesting() {
    let super::json::Json::Object(pairs) = super::json::parse_json(r#"{"note":"smile \ud83d\ude00"}"#).unwrap() else {
        panic!("json is not an object");
    };
    assert!(matches!(&pairs[0].1, super::json::Json::String(text) if text == "smile \u{1f600}"));
    assert!(
        super::json::parse_json(r#""\ud83d""#).is_err(),
        "a lone surrogate is not a character"
    );

    // Deep nesting is an error, not a stack overflow.
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let err = super::json::parse_json(&deep).err().expect("deep nesting is refused");
    assert!(err.to_string().contains("recursion limit"), "{err}");
    let facade = open_empty();
    let file = format!(r#"[{{"_entity":"moqui.test.TestEntity","testId":"D1","testMedium":{deep}}}]"#);
    let reader = std::io::BufReader::new(std::io::Cursor::new(file.into_bytes()));
    assert!(super::json::load_json_reader(&facade, "deep.json", reader, &LoadOptions::default()).is_err());

    // Members keep their document order.
    let super::json::Json::Object(pairs) = super::json::parse_json(r#"{"b":1,"a":2,"c":3}"#).unwrap() else {
        panic!("json is not an object");
    };
    let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["b", "a", "c"]);
}

#[test]
fn json_numbers_convert_by_field_type_without_losing_digits() {
    let facade = crate::testing::open();
    let json = r#"[{"_entity":"moqui.test.TestEntity","testId":"N1","testNumberDecimal":12345678901234567890.123456789,
        "testNumberInteger":9007199254740993,"testMedium":1.50}]"#;
    let reader = std::io::BufReader::new(std::io::Cursor::new(json.as_bytes()));
    let loaded = super::json::load_json_reader(&facade, "numbers.json", reader, &LoadOptions::default()).unwrap();
    assert_eq!(loaded, 1);
    let row = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("N1"))
        .one()
        .unwrap()
        .expect("row");
    let decimal = row.get("testNumberDecimal").cloned().unwrap_or(Value::Null);
    assert_eq!(
        decimal,
        Value::Decimal("12345678901234567890.123456789".parse().unwrap()),
        "{decimal:?}"
    );
    assert_eq!(row.get("testNumberInteger"), Some(&Value::I64(9_007_199_254_740_993)));
    assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("1.50"));
}

#[test]
fn the_upsert_is_one_statement_on_mysql_and_postgres() {
    let framework = framework_dir();
    let conf = MNode::parse_file(framework.join("conf/MoquiDefaultConf.xml")).unwrap();
    let dialects = Dialects::load(&conf).unwrap();
    let model = EntityModel::load_directory(&conf, &framework.join("entity")).unwrap();
    let entity = model.entity("moqui.test.Foo").unwrap();
    let mysql = dialects.get("mysql").unwrap();
    let sql = insert_or_update(entity, mysql, &["fooId", "fooText"]).unwrap();
    assert!(sql.contains("ON DUPLICATE KEY UPDATE"), "{sql}");
    let postgres = dialects.get("postgres").unwrap();
    let sql = insert_or_update(entity, postgres, &["fooId", "fooText"]).unwrap();
    assert!(sql.contains("ON CONFLICT"), "{sql}");
}

#[test]
fn feed_insert_uses_the_enlisted_connection_and_skips_a_missing_catalog() {
    let script = new_script(false, false);
    let facade = open_with(Arc::new(ScriptDriver {
        script: Arc::clone(&script),
    }));
    facade.begin("moqui.test.Foo", None).unwrap();
    match_rules(&facade, "moqui.test.Foo", "create");
    let tx = facade.transactions_for("moqui.test.Foo").unwrap();
    register_feed_insert(&facade, &tx).unwrap();
    enlist(&tx);
    facade.commit("moqui.test.Foo").unwrap();
    let inserts = script.inserts.lock().expect("inserts");
    assert_eq!(inserts.len(), 1, "{inserts:?}");
    assert_eq!(inserts[0][1], Value::string("DOC1"));
    assert_eq!(inserts[0][2], Value::string("moqui.test.Foo"));
    assert_eq!(inserts[0][3], Value::string("F1"));
    assert_eq!(inserts[0][4], Value::string("index"));
    drop(inserts);
    let held = feed_service_names(&facade);
    assert!(held
        .iter()
        .any(|service| service.receive_service == "index#DataDocuments"));
    assert!(held
        .iter()
        .any(|service| service.delete_service == "delete#DataDocument"));

    facade.set_disable_data_feed(true);
    facade.begin("moqui.test.Foo", None).unwrap();
    match_rules(&facade, "moqui.test.Foo", "create");
    let tx = facade.transactions_for("moqui.test.Foo").unwrap();
    register_feed_insert(&facade, &tx).unwrap();
    enlist(&tx);
    facade.commit("moqui.test.Foo").unwrap();
    assert_eq!(script.inserts.lock().expect("inserts").len(), 1);
    facade.set_disable_data_feed(false);

    facade.set_sim_active(true);
    facade.begin("moqui.test.Foo", None).unwrap();
    match_rules(&facade, "moqui.test.Foo", "delete");
    let tx = facade.transactions_for("moqui.test.Foo").unwrap();
    register_feed_insert(&facade, &tx).unwrap();
    enlist(&tx);
    facade.commit("moqui.test.Foo").unwrap();
    assert_eq!(script.inserts.lock().expect("inserts").len(), 1);
    facade.set_sim_active(false);

    let missing = new_script(true, false);
    let facade = open_with(Arc::new(ScriptDriver {
        script: Arc::clone(&missing),
    }));
    facade.begin("moqui.test.Foo", None).unwrap();
    match_rules(&facade, "moqui.test.Foo", "create");
    let tx = facade.transactions_for("moqui.test.Foo").unwrap();
    register_feed_insert(&facade, &tx).unwrap();
    enlist(&tx);
    facade.commit("moqui.test.Foo").unwrap();
    assert!(missing.inserts.lock().expect("inserts").is_empty());
}

/// As in Java's `EntityDataFeed`, a feed that fails is logged and the business transaction commits; only a savepoint
/// that cannot be rolled back, which leaves the transaction unusable, stops the commit.
#[test]
fn a_failed_feed_insert_does_not_roll_back_the_business_transaction() {
    let script = new_script(false, false);
    script.fail_pending.store(true, Ordering::SeqCst);
    let facade = open_with(Arc::new(ScriptDriver {
        script: Arc::clone(&script),
    }));
    facade.begin("moqui.test.Foo", None).unwrap();
    match_rules(&facade, "moqui.test.Foo", "create");
    let tx = facade.transactions_for("moqui.test.Foo").unwrap();
    register_feed_insert(&facade, &tx).unwrap();
    enlist(&tx);
    facade.commit("moqui.test.Foo").unwrap();
    assert!(script.inserts.lock().expect("inserts").is_empty());

    script.broken_rollback.store(true, Ordering::SeqCst);
    facade.begin("moqui.test.Foo", None).unwrap();
    match_rules(&facade, "moqui.test.Foo", "create");
    let tx = facade.transactions_for("moqui.test.Foo").unwrap();
    register_feed_insert(&facade, &tx).unwrap();
    enlist(&tx);
    let err = facade.commit("moqui.test.Foo").unwrap_err();
    assert!(error_chain(&err).contains("unusable"), "{}", error_chain(&err));
}

#[test]
fn a_missing_table_probe_fails_when_the_savepoint_rollback_fails() {
    let script = new_script(true, false);
    let mut conn = ScriptConn {
        script: Arc::clone(&script),
    };
    let rows = probe_query(&mut conn, "SELECT * FROM data_feed", &[]).unwrap();
    assert!(rows.is_empty());

    script.broken_rollback.store(true, Ordering::SeqCst);
    let err = probe_query(&mut conn, "SELECT * FROM data_feed", &[]).unwrap_err();
    assert!(err.to_string().contains("aborted"), "{err}");
}

fn enlist(tx: &TransactionManager) {
    tx.with_connection(|conn| conn.execute("touch", &[])).unwrap();
}

#[test]
fn autocommit_match_does_not_insert_on_a_later_commit() {
    let script = new_script(false, false);
    let facade = open_with(Arc::new(ScriptDriver {
        script: Arc::clone(&script),
    }));
    assert!(match_rules(&facade, "moqui.test.Foo", "create").is_empty());
    facade.begin("moqui.test.Foo", None).unwrap();
    let tx = facade.transactions_for("moqui.test.Foo").unwrap();
    register_feed_insert(&facade, &tx).unwrap();
    enlist(&tx);
    facade.commit("moqui.test.Foo").unwrap();
    assert!(script.inserts.lock().expect("inserts").is_empty());
}

fn create_foo(facade: &EntityFacade, id: &str) -> Result<(), Error> {
    let mut value = facade.make_value("moqui.test.Foo")?;
    value.set("fooId", Value::string(id));
    value.create(facade)
}

fn frame_touches() -> usize {
    TOUCHES.with(|touches| touches.borrow().values().map(Vec::len).sum())
}

/// Whether each touch of the open frames keeps its written values.
fn touch_fields_kept() -> Vec<bool> {
    TOUCHES.with(|touches| touches.borrow().values().flatten().map(Touch::keeps_fields).collect())
}

#[test]
fn a_failed_first_write_leaves_no_touch_and_feed_names_do_not_grow() {
    let script = new_script(false, true);
    let facade = open_with(Arc::new(ScriptDriver {
        script: Arc::clone(&script),
    }));
    facade.begin("moqui.test.Foo", None).unwrap();
    assert!(create_foo(&facade, "F9").is_err());
    assert_eq!(frame_touches(), 0, "the failed write noted nothing");
    facade.rollback("moqui.test.Foo").unwrap();
    assert_eq!(frame_touches(), 0);
    assert!(script.inserts.lock().expect("inserts").is_empty());

    script.fail_inserts.store(false, Ordering::SeqCst);
    facade.begin("moqui.test.Foo", None).unwrap();
    create_foo(&facade, "F1").unwrap();
    assert_eq!(frame_touches(), 1);
    facade.commit("moqui.test.Foo").unwrap();
    assert_eq!(frame_touches(), 0, "the commit took this frame's touches");
    let ids: Vec<Value> = script
        .inserts
        .lock()
        .expect("inserts")
        .iter()
        .map(|params| params[3].clone())
        .collect();
    assert_eq!(ids, vec![Value::string("F1")], "pending rows only for the written row");

    for id in ["F2", "F3"] {
        facade.begin("moqui.test.Foo", None).unwrap();
        create_foo(&facade, id).unwrap();
        facade.commit("moqui.test.Foo").unwrap();
    }
    assert_eq!(script.inserts.lock().expect("inserts").len(), 3);
    assert_eq!(
        feed_service_names(&facade).len(),
        1,
        "one entry per feed, whatever the commits"
    );
    assert_eq!(frame_touches(), 0);
}

#[test]
fn feed_definitions_are_read_once_until_one_is_written() {
    let script = new_script(false, false);
    let facade = open_with(Arc::new(ScriptDriver {
        script: Arc::clone(&script),
    }));
    let reads = || script.definition_reads.load(Ordering::SeqCst);
    facade.begin("moqui.test.Foo", None).unwrap();
    create_foo(&facade, "F1").unwrap();
    facade.commit("moqui.test.Foo").unwrap();
    let first = reads();
    assert!(first > 0, "the first commit read the definitions");
    assert_eq!(script.inserts.lock().expect("inserts").len(), 1);

    facade.begin("moqui.test.Foo", None).unwrap();
    create_foo(&facade, "F2").unwrap();
    facade.commit("moqui.test.Foo").unwrap();
    assert_eq!(reads(), first, "the second commit used the kept definitions");
    assert_eq!(script.inserts.lock().expect("inserts").len(), 2);

    // A write to an entity no document reads sets no savepoint and reads nothing, and its touch keeps no values
    // once the definitions are known; a document entity's keeps them.
    let savepoints = script.savepoints.load(Ordering::SeqCst);
    facade.begin("moqui.test.TestEntity", None).unwrap();
    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string("T1"));
    row.create(&facade).unwrap();
    assert_eq!(touch_fields_kept(), vec![false]);
    facade.commit("moqui.test.TestEntity").unwrap();
    assert_eq!(script.savepoints.load(Ordering::SeqCst), savepoints);
    assert_eq!(reads(), first);
    assert_eq!(script.inserts.lock().expect("inserts").len(), 2);
    facade.begin("moqui.test.Foo", None).unwrap();
    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string("T2"));
    row.create(&facade).unwrap();
    create_foo(&facade, "F2b").unwrap();
    assert_eq!(touch_fields_kept(), vec![false, true]);
    facade.commit("moqui.test.Foo").unwrap();
    assert_eq!(reads(), first);
    assert_eq!(script.inserts.lock().expect("inserts").len(), 3, "F2b was fed");

    // A definition write, here outside a transaction, makes the next commit read them again.
    let mut feed = facade.make_value("moqui.entity.feed.DataFeed").unwrap();
    feed.set("dataFeedId", Value::string("FEED2"));
    feed.create(&facade).unwrap();
    facade.begin("moqui.test.Foo", None).unwrap();
    create_foo(&facade, "F3").unwrap();
    facade.commit("moqui.test.Foo").unwrap();
    assert!(reads() > first, "the definitions were read again");
    let again = reads();

    // A definition written inside the frame is read from that frame and not kept for others.
    facade.begin("moqui.test.Foo", None).unwrap();
    let mut document = facade.make_value("moqui.entity.document.DataDocument").unwrap();
    document.set("dataDocumentId", Value::string("DOC2"));
    document.create(&facade).unwrap();
    create_foo(&facade, "F4").unwrap();
    facade.commit("moqui.test.Foo").unwrap();
    assert!(reads() > again);
    let after_frame = reads();
    facade.begin("moqui.test.Foo", None).unwrap();
    create_foo(&facade, "F5").unwrap();
    facade.commit("moqui.test.Foo").unwrap();
    assert!(
        reads() > after_frame,
        "the commit of the definition write counts as a write"
    );
    let settled = reads();
    facade.begin("moqui.test.Foo", None).unwrap();
    create_foo(&facade, "F6").unwrap();
    facade.commit("moqui.test.Foo").unwrap();
    assert_eq!(reads(), settled);
}

/// As Java's `loadEecaRulesAll`: a rule's entity is kept by its full name, a rule for an entity that is not defined is
/// skipped, and an entity's rules with no id come before those with one.
#[test]
fn eca_rule_entities_are_full_names_and_rules_with_no_id_come_first() {
    let facade = open_empty();
    super::add_eca_rules(
        &facade,
        r#"<eecas>
            <eeca id="ShortName" entity="TestEntity" on-create="true"><actions><log message="a"/></actions></eeca>
            <eeca entity="moqui.test.TestEntity" on-create="true"><actions><log message="b"/></actions></eeca>
            <eeca id="Undefined" entity="no.such.Entity" on-create="true"><actions><log message="c"/></actions></eeca>
        </eecas>"#,
    )
    .unwrap();
    let ids: Vec<String> = eca_rules(&facade, "moqui.test.TestEntity")
        .into_iter()
        .map(|rule| rule.id)
        .collect();
    assert_eq!(ids, ["", "ShortName"]);
    assert!(eca_rules(&facade, "no.such.Entity").is_empty());
    facade.begin("moqui.test.TestEntity", None).unwrap();
    let matched = match_rules(&facade, "moqui.test.TestEntity", "create");
    facade.rollback("moqui.test.TestEntity").unwrap();
    assert_eq!(matched, ["", "ShortName"]);
}
