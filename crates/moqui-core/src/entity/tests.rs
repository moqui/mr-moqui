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

use super::{EntityFacade, EntityGroup};
use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};
use crate::transaction::{PoolConfig, TransactionManager};
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

fn test_entity_row(facade: &EntityFacade, test_id: &str) -> Option<super::EntityValue> {
    facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string(test_id))
        .one()
        .unwrap()
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
    let fk = super::FkScope::enter(true, false);
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

/// Java reads timestamp text without an offset in the session time zone wherever text becomes a timestamp: a field set
/// from text (`setString`; here a value set from text and created), a data file, a condition value and the data check
/// that compares them, so each reads the same text as the same moment. A date stays a civil date, and text with an
/// offset is the moment it names.
#[test]
fn timestamp_text_is_read_in_the_session_time_zone_when_set_loaded_found_and_checked() {
    use super::load_feed::{check_entity_xml_text, load_entity_xml_text, LoadOptions};
    use super::Condition;

    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let stamp = |text: &str| Value::Timestamp(text.parse::<jiff::Timestamp>().unwrap());
    let options = LoadOptions::default();
    for (index, (zone, midnight, noon)) in [
        ("America/Chicago", "2016-11-01T05:00:00Z", "2016-11-10T18:00:00Z"),
        ("UTC", "2016-11-01T00:00:00Z", "2016-11-10T12:00:00Z"),
    ]
    .into_iter()
    .enumerate()
    {
        crate::user::session::set_time_zone(Some(zone));
        let (set_id, load_id, offset_id) = (
            format!("TZ_SET_{index}"),
            format!("TZ_LOAD_{index}"),
            format!("TZ_Z_{index}"),
        );
        let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
        row.set("testId", Value::string(&set_id));
        row.set("testDateTime", Value::string("2016-11-01"));
        row.set("testDate", Value::string("2016-11-01"));
        row.create(&facade).unwrap();
        let read = test_entity_row(&facade, &set_id).unwrap();
        assert_eq!(read.get("testDateTime"), Some(&stamp(midnight)), "{zone}");
        assert_eq!(
            read.get("testDate"),
            Some(&Value::Date(jiff::civil::date(2016, 11, 1))),
            "{zone}"
        );

        let document = format!(
            r#"<entity-facade-xml>
                <moqui.test.TestEntity testId="{load_id}" testDateTime="2016-11-10 12:00:00.0"/>
                <moqui.test.TestEntity testId="{offset_id}" testDateTime="2016-11-10T12:00:00Z"/>
            </entity-facade-xml>"#
        );
        load_entity_xml_text(&facade, "zone", &document, &options).unwrap();
        let loaded = test_entity_row(&facade, &load_id).unwrap();
        assert_eq!(loaded.get("testDateTime"), Some(&stamp(noon)), "{zone}");
        let exact = test_entity_row(&facade, &offset_id).unwrap();
        assert_eq!(
            exact.get("testDateTime"),
            Some(&stamp("2016-11-10T12:00:00Z")),
            "{zone}"
        );

        let found = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .condition(Condition::equals("testDateTime", Value::string("2016-11-10 12:00")))
            .list()
            .unwrap();
        let ids: Vec<_> = found
            .values()
            .iter()
            .filter_map(|row| row.get("testId").and_then(Value::as_str))
            .collect();
        assert!(ids.contains(&load_id.as_str()), "{zone}: {ids:?}");

        let check = format!(
            r#"<entity-facade-xml>
                <moqui.test.TestEntity testId="{set_id}" testDateTime="2016-11-01 00:00:00.0" testDate="2016-11-01"/>
                <moqui.test.TestEntity testId="{load_id}" testDateTime="2016-11-10 12:00"/>
                <moqui.test.TestEntity testId="{offset_id}" testDateTime="{}"/>
            </entity-facade-xml>"#,
            "2016-11-10T12:00:00Z"
                .parse::<jiff::Timestamp>()
                .unwrap()
                .as_millisecond()
        );
        let checked = check_entity_xml_text(&facade, "zone-check", &check, &options).unwrap();
        assert!(checked.messages.is_empty(), "{zone}: {:?}", checked.messages);
        let off = format!(
            r#"<entity-facade-xml><moqui.test.TestEntity testId="{load_id}" testDateTime="2016-11-10 13:00"/></entity-facade-xml>"#
        );
        let mismatched = check_entity_xml_text(&facade, "zone-check", &off, &options).unwrap();
        assert_eq!(mismatched.messages.len(), 1, "{zone}: {:?}", mismatched.messages);
    }
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
    // What a read found is remembered with the facade until a write to either entity: a translation added or changed
    // after is seen by the next read.
    let mut added = facade.make_value("moqui.basic.LocalizedEntityField").unwrap();
    for (name, value) in [
        ("entityName", entity),
        ("fieldName", "description"),
        ("pkValue", "E3"),
        ("locale", "fr"),
        ("localized", "Simple"),
    ] {
        added.set(name, Value::string(value));
    }
    added.create(&facade).unwrap();
    assert_eq!(description("E3"), Some(Value::string("Simple")));
    message.set("localized", Value::string("Salut"));
    message.update(&facade).unwrap();
    assert_eq!(description("E2"), Some(Value::string("Salut")));
    added.delete(&facade).unwrap();
    message.set("localized", Value::string("Au revoir"));
    message.update(&facade).unwrap();
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

/// An update ECA rule with `get-original-value` sees the row as the update leaves it: a field the value did not change
/// is the stored one, not the one the value read before another update changed it, so it does not look changed.
#[test]
fn an_update_eca_rule_sees_the_stored_value_of_a_field_the_value_did_not_change() {
    crate::script::clear_context_stack();
    let facade = crate::testing::open_with(
        &framework_dir(),
        crate::testing::Options {
            entity_eca: true,
            ..Default::default()
        },
    );
    super::load_feed::add_eca_rules(
        &facade,
        r#"<eecas>
            <eeca id="TestSeesStored" entity="moqui.test.TestEntity" on-update="true" get-original-value="true">
                <actions><script>
                    const seen = ec.entity.makeValue("moqui.test.Foo");
                    seen.fooId = "SEEN-" + entityValue.testMedium;
                    seen.fooText = entityValue.value + "|" + originalValue.value;
                    seen.create();
                </script></actions>
            </eeca>
        </eecas>"#,
    )
    .unwrap();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    create_test_entity(&facade, "EECA", "m0", "v0");
    let (mut stale, mut other) = (test_entity_read(&facade, "EECA"), test_entity_read(&facade, "EECA"));
    other.set("value", Value::string("v1"));
    other.update(&facade).unwrap();
    stale.set("testMedium", Value::string("m1"));
    stale.update(&facade).unwrap();
    let seen = |id: &str| {
        facade
            .find("moqui.test.Foo")
            .unwrap()
            .eq("fooId", Value::string(id))
            .one()
            .unwrap()
            .and_then(|row| row.get("fooText").cloned())
    };
    assert_eq!(seen("SEEN-m0"), Some(Value::string("v1|v0")));
    assert_eq!(seen("SEEN-m1"), Some(Value::string("v1|v1")));
    assert_eq!(
        test_entity_read(&facade, "EECA").get("value"),
        Some(&Value::string("v1"))
    );
}

#[test]
fn a_sequence_primary_prefix_is_expanded_with_the_value_fields() {
    let context = crate::context::ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let mut values = crate::entity::FieldMap::new();
    values.insert("kind".into(), Value::string("INV"));
    assert_eq!(
        super::expand_prefix(Some("${kind}-"), &values).unwrap().as_deref(),
        Some("INV-")
    );
    assert_eq!(super::expand_prefix(Some("P"), &values).unwrap().as_deref(), Some("P"));
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

/// A scheduler poll or a data load outside any request or job writes no trace per statement; a statement inside a
/// trace is a client span of it.
#[cfg(feature = "telemetry")]
#[test]
fn a_statement_is_a_span_only_inside_a_trace() {
    use crate::entity::document::Document;
    use crate::telemetry::{Exporter, SpanKind};
    use moqui_lib::error::Error;
    use moqui_lib::object::Value;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Memory(Mutex<Vec<Document>>);
    impl Exporter for Memory {
        fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
            let count = documents.len();
            if index == "otel_spans" {
                self.0.lock().unwrap().extend(documents);
            }
            Ok(count)
        }
    }
    let context = crate::context::ExecutionContextFactory::test_context_with_conf(
        "<moqui-conf><telemetry sample-ratio=\"1\"/></moqui-conf>",
    );
    let facade = crate::testing::open();
    context.factory().set_entity(facade.clone());
    let telemetry = crate::telemetry::for_factory(context.factory());
    let memory = Arc::new(Memory::default());
    telemetry.set_exporter(memory.clone());
    let root_id;
    {
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let find = || {
            facade
                .find("moqui.basic.Enumeration")
                .unwrap()
                .use_cache(false)
                .list()
                .unwrap()
        };
        find();
        let root = telemetry.start_span("job", SpanKind::Server, None);
        root_id = root.context().unwrap().span_id;
        {
            let _trace = crate::telemetry::attach(root.context());
            find();
        }
        root.finish();
    }
    telemetry.flush();
    let spans = memory.0.lock().unwrap();
    let statements: Vec<_> = spans
        .iter()
        .filter(|span| span.get("name").and_then(Value::as_str) == Some("sql"))
        .collect();
    assert_eq!(statements.len(), 1, "{spans:?}");
    assert_eq!(
        statements[0].get("parent_span_id").and_then(Value::as_str),
        Some(root_id.as_str())
    );
    drop(spans);
    telemetry.shutdown();
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

/// The script runtime's shared entity values are records in a response.
#[test]
fn response_json_serializes_shared_entity_values_and_entity_lists_as_records() {
    use crate::entity::{EntityList, EntityValue};
    use moqui_lib::object::HostObject;
    use std::sync::{Arc, Mutex};

    let record = EntityValue::from_read_fields(
        Arc::from("test.TimePeriod"),
        BTreeMap::from([
            (Arc::from("timePeriodId"), Value::string("FY2026")),
            (Arc::from("periodName"), Value::string("Fiscal <&> year")),
            (Arc::from("fromDate"), Value::Date(jiff::civil::date(2026, 1, 1))),
            (Arc::from("parentPeriodId"), Value::Null),
        ]),
    );
    let cell = Arc::new(Mutex::new(record.clone()));
    let shared = Value::Object(HostObject(cell.clone()));
    let expected = serde_json::json!({"timePeriodId":"FY2026", "periodName":"Fiscal <&> year",
        "fromDate":1767225600000_i64, "parentPeriodId":null});
    let decode =
        |value: &Value| serde_json::from_str::<serde_json::Value>(&super::query_rest::encode_json(value)).unwrap();
    assert_eq!(decode(&shared), expected);
    assert_eq!(
        decode(&Value::List(vec![shared.clone()])),
        serde_json::json!([expected])
    );
    let list = Value::Object(HostObject::new(EntityList::new(vec![record])));
    assert_eq!(decode(&list), serde_json::json!([expected]));
    cell.lock()
        .unwrap()
        .set("periodName", Value::string("Updated fiscal year"));
    let nested = Value::Map(indexmap::IndexMap::from([(
        "options".to_string(),
        Value::List(vec![shared]),
    )]));
    let mut changed = expected;
    changed["periodName"] = serde_json::json!("Updated fiscal year");
    assert_eq!(decode(&nested), serde_json::json!({"options":[changed]}));
}
