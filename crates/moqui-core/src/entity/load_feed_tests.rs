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

use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};
use crate::entity::load_feed::{
    data_file_services, eca_rules, error_chain, load_entity_data, load_entity_text, load_entity_xml_text,
    location_in_components, LoadOptions,
};
use crate::entity::{EntityFacade, EntityGroup};
use crate::transaction::{PoolConfig, TransactionManager};
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

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
    let facade = EntityFacade::open(
        &framework_dir(),
        "transactional",
        vec![EntityGroup::new("transactional", "postgres", transactions)],
    )
    .unwrap();
    // As `moqui_entity::testing::open`: an audited field (a UserGroupPermission's dates) would take an EntityAuditLog id
    // from the sequence, whose upsert answers no row on a driver that answers nothing.
    facade.set_disable_audit_log(true);
    facade
}

fn open_empty() -> EntityFacade {
    open_with(Arc::new(EmptyDriver))
}

fn framework_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/components/base/framework")
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

/// A stored entry the way Java `ZipOutputStream` writes it to a stream: flag bit 3, zero local sizes, and a data
/// descriptor after the data. Only the central directory carries the sizes.
fn descriptor_zip(name: &str, data: &[u8]) -> Vec<u8> {
    let mut writer = ZipWriter::new_stream(Vec::new());
    writer
        .start_file(
            name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(data).unwrap();
    let out = writer.finish().unwrap().into_inner();
    assert_eq!(out[6] & 0x08, 0x08, "the entry has a data descriptor");
    out
}

fn test_medium(facade: &EntityFacade, id: &str) -> Option<String> {
    facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string(id))
        .one()
        .unwrap()
        .and_then(|row| row.get("testMedium").and_then(Value::as_str).map(str::to_string))
}

fn test_field(facade: &EntityFacade, entity: &str, pk: (&str, &str), field: &str) -> Option<Value> {
    facade
        .find(entity)
        .unwrap()
        .eq(pk.0, Value::string(pk.1))
        .one()
        .unwrap()
        .and_then(|row| row.get(field).cloned())
        .filter(|value| !value.is_null())
}

#[test]
fn loader_filters_types_holds_services_and_reads_zip() {
    let facade = open_empty();
    // A data-file service element runs in the entered context.
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let (dir, _guard) = temp_dir();
    let xml = dir.join("rows.xml");
    std::fs::write(
        &xml,
        r#"<entity-facade-xml type="seed">
            <moqui.test.TestEntity testId="T1" testMedium="xml"/>
            <create-moqui.test.TestEntity testId="SVC" testMedium="held"/>
        </entity-facade-xml>"#,
    )
    .unwrap();
    let loaded = load_entity_data(&facade, xml.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(loaded.records, 1);
    let held = data_file_services(&facade);
    assert!(held
        .iter()
        .any(|service| service.name == "create#moqui.test.TestEntity"));
    assert_eq!(held[0].parameters.get("testId").map(String::as_str), Some("SVC"));

    let demo = LoadOptions {
        data_types: vec!["demo".to_string()],
        ..LoadOptions::default()
    };
    let skipped = load_entity_data(&facade, xml.to_str().unwrap(), &demo).unwrap();
    assert_eq!(skipped.records, 0);

    let csv_path = dir.join("rows.csv");
    std::fs::write(&csv_path, "moqui.test.TestEntity,seed\ntestId,testMedium\nC1,csv\n").unwrap();
    let csv_loaded = load_entity_data(&facade, csv_path.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(csv_loaded.records, 1);

    let json_path = dir.join("rows.json");
    std::fs::write(
        &json_path,
        r#"{"_entity":"moqui.test.TestEntity","_dataType":"seed","testId":"J1","testMedium":"json"}"#,
    )
    .unwrap();
    let json_loaded = load_entity_data(&facade, json_path.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(json_loaded.records, 1);

    let nested = dir.join("nested.json");
    std::fs::write(
        &nested,
        r#"{"_entity":"moqui.security.UserGroup","userGroupId":"G1","description":"g","permissions":{"userPermissionId":"P1","fromDate":"0"}}"#,
    )
    .unwrap();
    // The empty driver cannot see the parent row, so the child foreign key uses a dummy.
    let nested_options = LoadOptions {
        dummy_fks: true,
        ..LoadOptions::default()
    };
    let nested_loaded = load_entity_data(&facade, nested.to_str().unwrap(), &nested_options).unwrap();
    assert_eq!(nested_loaded.records, 2);
    assert!(!facade.flags().dummy_fks);

    let zip_path = dir.join("rows.zip");
    let csv = b"moqui.test.TestEntity,seed\ntestId,testMedium\nZ1,zip\n";
    std::fs::write(&zip_path, zip_of(&[("rows.csv", csv)], CompressionMethod::Stored)).unwrap();
    let zip_loaded = load_entity_data(&facade, zip_path.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(zip_loaded.records, 1);

    let described = dir.join("described.zip");
    std::fs::write(&described, descriptor_zip("rows.csv", csv)).unwrap();
    let described_loaded = load_entity_data(&facade, described.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(described_loaded.records, 1);

    let deflated = dir.join("deflated.zip");
    std::fs::write(&deflated, zip_of(&[("rows.csv", csv)], CompressionMethod::Deflated)).unwrap();
    let deflated_loaded = load_entity_data(&facade, deflated.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(deflated_loaded.records, 1);

    let tools = LoadOptions {
        components: vec!["tools".to_string()],
        ..LoadOptions::default()
    };
    // As Java's `componentNameList`, the components narrow only the locations a load gathers: a location given is
    // read (here it is not there), not skipped.
    assert!(!location_in_components(
        "component://other/data/x.xml",
        &tools.components
    ));
    assert!(location_in_components(
        "component://tools/data/x.xml",
        &tools.components
    ));
    assert!(location_in_components(xml.to_str().unwrap(), &tools.components));
    let given = load_entity_data(&facade, "component://other/data/x.xml", &tools).unwrap_err();
    assert!(given.to_string().contains("component other was not found"), "{given}");

    facade.set_dummy_fks(false);
    facade.set_disable_fk_create(false);
    let flags = LoadOptions {
        dummy_fks: true,
        disable_fk_create: true,
        only_create: true,
        ..LoadOptions::default()
    };
    let again = load_entity_data(&facade, xml.to_str().unwrap(), &flags).unwrap();
    assert_eq!(again.records, 1);
    assert!(!facade.flags().dummy_fks);
    assert!(!facade.flags().disable_fk_create);
}

/// As Java's `loadAllData`, a file of a directory that fails is logged and counted, and the files after it still load.
#[test]
fn a_failed_file_in_a_directory_is_skipped_and_the_next_one_loads() {
    let facade = open_empty();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let (dir, _guard) = temp_dir();
    std::fs::write(
        dir.join("a-broken.xml"),
        r#"<entity-facade-xml type="seed"><moqui.test.TestEntity testId="T0"</entity-facade-xml>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("b-good.xml"),
        r#"<entity-facade-xml type="seed"><moqui.test.TestEntity testId="T1" testMedium="xml"/></entity-facade-xml>"#,
    )
    .unwrap();
    let loaded = load_entity_data(&facade, dir.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(loaded.records, 1);
    assert_eq!(loaded.failed_files, 1);
}

/// As Java's `loadSingleFile`, one file is one transaction: a record that fails takes the file's earlier records
/// with it, and the next file of the directory still loads.
#[test]
fn a_file_loads_in_one_transaction_and_a_failure_rolls_back_the_whole_file() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let (dir, _guard) = temp_dir();
    std::fs::write(
        dir.join("a-partial.xml"),
        r#"<entity-facade-xml type="seed">
            <moqui.test.TestEntity testId="T1" testMedium="first"/>
            <no.such.Thing id="X"/>
        </entity-facade-xml>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("b-good.xml"),
        r#"<entity-facade-xml type="seed"><moqui.test.TestEntity testId="T2" testMedium="second"/></entity-facade-xml>"#,
    )
    .unwrap();
    let loaded = load_entity_data(&facade, dir.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!((loaded.records, loaded.failed_files), (1, 1));
    assert_eq!(test_medium(&facade, "T1"), None);
    assert_eq!(test_medium(&facade, "T2").as_deref(), Some("second"));
}

/// Java's `defaultValues` (XML, JSON and CSV top-level records), CSV dialect options, and `onlyCreate`, which calls
/// no data-file service.
#[test]
fn default_values_csv_options_and_only_create_follow_java() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let defaults = LoadOptions {
        default_values: vec![("testMedium".to_string(), "default".to_string())],
        ..LoadOptions::default()
    };
    load_entity_xml_text(
        &facade,
        "xmlText",
        r#"<entity-facade-xml>
            <moqui.test.TestEntity testId="X1"/>
            <moqui.test.TestEntity testId="X2" testMedium="own"/>
        </entity-facade-xml>"#,
        &defaults,
    )
    .unwrap();
    assert_eq!(test_medium(&facade, "X1").as_deref(), Some("default"));
    assert_eq!(test_medium(&facade, "X2").as_deref(), Some("own"));

    let (dir, _guard) = temp_dir();
    let json = dir.join("rows.json");
    std::fs::write(&json, r#"[{"_entity": "moqui.test.TestEntity", "testId": "J1"}]"#).unwrap();
    load_entity_data(&facade, json.to_str().unwrap(), &defaults).unwrap();
    assert_eq!(test_medium(&facade, "J1").as_deref(), Some("default"));

    let csv = dir.join("rows.csv");
    std::fs::write(&csv, "moqui.test.TestEntity\ntestId;testMedium\n'C;1';semi\n").unwrap();
    let dialect = LoadOptions {
        csv_delimiter: Some(';'),
        csv_quote_char: Some('\''),
        ..LoadOptions::default()
    };
    load_entity_data(&facade, csv.to_str().unwrap(), &dialect).unwrap();
    assert_eq!(test_medium(&facade, "C;1").as_deref(), Some("semi"));

    let only_create = LoadOptions {
        only_create: true,
        ..LoadOptions::default()
    };
    load_entity_xml_text(
        &facade,
        "xmlText",
        r#"<entity-facade-xml>
            <moqui.test.TestEntity testId="X2" testMedium="changed"/>
            <create-moqui.test.TestEntity testId="SVC" testMedium="service"/>
        </entity-facade-xml>"#,
        &only_create,
    )
    .unwrap();
    assert_eq!(test_medium(&facade, "X2").as_deref(), Some("own"));
    assert_eq!(test_medium(&facade, "SVC"), None);
    assert!(data_file_services(&facade).is_empty());
}

/// Java's CSV reader: its own primary-key check and then `createOrUpdate`, so no entity-auto `fromDate` or secondary
/// sequence; a Base64 BLOB cell; an empty cell that clears the field; and a short row, a column that is not a field,
/// bad Base64 or text the field type does not take failing the file.
#[test]
fn csv_rows_check_their_own_key_and_create_or_update() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let (dir, _guard) = temp_dir();
    let load = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        load_entity_data(&facade, path.to_str().unwrap(), &LoadOptions::default())
    };

    load(
        "blob.csv",
        "moqui.resource.DbResourceFile\nresourceId,fileData\nR1,aGVsbG8\n",
    )
    .unwrap();
    assert_eq!(
        test_field(
            &facade,
            "moqui.resource.DbResourceFile",
            ("resourceId", "R1"),
            "fileData"
        ),
        Some(Value::Bytes(b"hello".to_vec()))
    );
    let bad_blob = load(
        "bad-blob.csv",
        "moqui.resource.DbResourceFile\nresourceId,fileData\nR2,@@@\n",
    )
    .unwrap_err();
    assert!(error_chain(&bad_blob).contains("not Base64"), "{bad_blob}");

    load(
        "first.csv",
        "moqui.test.TestEntity\ntestId,testMedium,testNumberInteger\nC1,kept,7\n",
    )
    .unwrap();
    load(
        "clear.csv",
        "moqui.test.TestEntity\ntestId,testMedium,testNumberInteger\nC1,,8\n",
    )
    .unwrap();
    assert_eq!(test_medium(&facade, "C1"), None);
    // A one-field key that is missing is sequenced.
    let sequenced = load(
        "sequenced.csv",
        "moqui.test.TestEntity\ntestId,testMedium\n,sequenced\n",
    )
    .unwrap();
    assert_eq!(sequenced.records, 1);

    // A multi-field key with `fromDate` missing is an error; the entity-auto store would have filled it.
    let member = load(
        "member.csv",
        "moqui.security.UserGroupMember\nuserGroupId,userId\nADMIN,U1\n",
    )
    .unwrap_err();
    assert!(error_chain(&member).contains("incomplete primary key"), "{member}");

    // A name given twice reads the later column; an empty header cell is no column.
    load(
        "twice.csv",
        "moqui.test.TestEntity\ntestId,testMedium,testMedium,\nC5,first,second,\n",
    )
    .unwrap();
    assert_eq!(test_medium(&facade, "C5").as_deref(), Some("second"));

    let short = load("short.csv", "moqui.test.TestEntity\ntestId,testMedium\nC2\n").unwrap_err();
    assert!(error_chain(&short).contains("has no column 2"), "{short}");
    let unknown = load("unknown.csv", "moqui.test.TestEntity\ntestId,noSuchField\nC3,x\n").unwrap_err();
    assert!(
        error_chain(&unknown).contains("noSuchField is not a field"),
        "{unknown}"
    );
    let number = load(
        "number.csv",
        "moqui.test.TestEntity\ntestId,testNumberInteger\nC4,abc\n",
    )
    .unwrap_err();
    assert!(error_chain(&number).contains("[abc] is not valid"), "{number}");
    assert_eq!(test_medium(&facade, "C4"), None);
}

/// The data load's CSV and JSON text is loaded from memory, never written to a file another local user could read
/// (plans/Security-Review-Low.md SEC-OUT-89); an error names the text.
#[test]
fn csv_and_json_text_load_from_memory() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let plain = LoadOptions::default();
    let csv = "moqui.test.TestEntity\ntestId,testMedium\nT1,from csv\n";
    assert_eq!(
        load_entity_text(&facade, "csvText", csv, false, &plain)
            .unwrap()
            .records,
        1
    );
    assert_eq!(test_medium(&facade, "T1").as_deref(), Some("from csv"));
    let json = r#"[{"_entity": "moqui.test.TestEntity", "testId": "T2", "testMedium": "from json"}]"#;
    assert_eq!(
        load_entity_text(&facade, "jsonText", json, true, &plain)
            .unwrap()
            .records,
        1
    );
    assert_eq!(test_medium(&facade, "T2").as_deref(), Some("from json"));
    let bad = load_entity_text(
        &facade,
        "csvText",
        "moqui.test.TestEntity\ntestId,noSuchField\nT3,x\n",
        false,
        &plain,
    )
    .unwrap_err();
    assert!(error_chain(&bad).contains("noSuchField is not a field"), "{bad}");
}

/// An empty XML attribute, a JSON null and the text `null` are null, as Java puts them, so a store clears the
/// field and a `defaultValues` entry does not fill it. Text the field type does not take fails the record.
#[test]
fn empty_values_clear_fields_and_bad_text_fails() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let xml = |text: &str, options: &LoadOptions| {
        load_entity_xml_text(
            &facade,
            "xmlText",
            &format!("<entity-facade-xml>{text}</entity-facade-xml>"),
            options,
        )
    };
    let plain = LoadOptions::default();
    xml(
        r#"<moqui.test.TestEntity testId="N1" testMedium="kept" value="v"/>"#,
        &plain,
    )
    .unwrap();
    xml(
        r#"<moqui.test.TestEntity testId="N1" testMedium="" value="null"/>"#,
        &plain,
    )
    .unwrap();
    assert_eq!(test_medium(&facade, "N1"), None);
    assert_eq!(
        test_field(&facade, "moqui.test.TestEntity", ("testId", "N1"), "value"),
        None
    );

    let defaults = LoadOptions {
        default_values: vec![("testMedium".to_string(), "default".to_string())],
        ..LoadOptions::default()
    };
    xml(r#"<moqui.test.TestEntity testId="N2" testMedium=""/>"#, &defaults).unwrap();
    assert_eq!(test_medium(&facade, "N2"), None);

    let (dir, _guard) = temp_dir();
    let json = dir.join("rows.json");
    std::fs::write(
        &json,
        r#"[{"_entity": "moqui.test.TestEntity", "testId": "N2", "testMedium": "set"}]"#,
    )
    .unwrap();
    load_entity_data(&facade, json.to_str().unwrap(), &plain).unwrap();
    assert_eq!(test_medium(&facade, "N2").as_deref(), Some("set"));
    std::fs::write(
        &json,
        r#"[{"_entity": "moqui.test.TestEntity", "testId": "N2", "testMedium": null}]"#,
    )
    .unwrap();
    load_entity_data(&facade, json.to_str().unwrap(), &plain).unwrap();
    assert_eq!(test_medium(&facade, "N2"), None);

    // A field element's text is kept as written, as Java's SAX handler reads it.
    xml(
        "<moqui.test.TestEntity testId=\"N4\"><testMedium>  spaced\n</testMedium></moqui.test.TestEntity>",
        &plain,
    )
    .unwrap();
    assert_eq!(test_medium(&facade, "N4").as_deref(), Some("  spaced\n"));

    let bad = xml(r#"<moqui.test.TestEntity testId="N3" testDate="not a date"/>"#, &plain).unwrap_err();
    assert!(error_chain(&bad).contains("[not a date] is not valid"), "{bad}");
    assert_eq!(test_medium(&facade, "N3"), None);
}

/// A CSV or JSON service name is any name Java's `isServiceDefined` takes, not only a `verb#noun` one.
#[test]
fn csv_and_json_call_a_service_named_without_a_hash() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let (dir, _guard) = temp_dir();
    let service_dir = dir.join("service/test");
    std::fs::create_dir_all(&service_dir).unwrap();
    let services = service_dir.join("LoadServices.xml");
    std::fs::write(
        &services,
        r#"<services>
            <service verb="copy" noun="Row">
                <in-parameters><parameter name="rowId"/></in-parameters>
                <actions>
                    <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                    <script>made.set("testId", rowId); made.set("testMedium", "copied");</script>
                    <entity-create value-field="made"/>
                </actions>
            </service>
        </services>"#,
    )
    .unwrap();
    let failures = crate::service::lint(std::slice::from_ref(&services)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let csv = dir.join("rows.csv");
    std::fs::write(&csv, "test.LoadServices.copyRow\nrowId\nS1\n").unwrap();
    load_entity_data(&facade, csv.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(test_medium(&facade, "S1").as_deref(), Some("copied"));
    let json = dir.join("rows.json");
    std::fs::write(&json, r#"{"_entity": "test.LoadServices.copyRow", "rowId": "S2"}"#).unwrap();
    load_entity_data(&facade, json.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(test_medium(&facade, "S2").as_deref(), Some("copied"));
}

/// Java's `-- Checked data` message follows each file a check reads, unless `messageNoActionFiles(false)` and the
/// file gave no messages.
#[test]
fn a_check_names_each_file() {
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let (dir, _guard) = temp_dir();
    let file = dir.join("rows.xml");
    std::fs::write(
        &file,
        r#"<entity-facade-xml><moqui.test.TestEntity testId="K1" testMedium="same"/></entity-facade-xml>"#,
    )
    .unwrap();
    let location = file.to_str().unwrap();
    load_entity_data(&facade, location, &LoadOptions::default()).unwrap();
    let checked = crate::entity::load_feed::check_entity_data(&facade, location, &LoadOptions::default()).unwrap();
    assert_eq!(checked.messages, [format!("-- Checked data (1 records) in {location}")]);
    let quiet = LoadOptions {
        hide_no_action_files: true,
        ..LoadOptions::default()
    };
    assert!(crate::entity::load_feed::check_entity_data(&facade, location, &quiet)
        .unwrap()
        .messages
        .is_empty());
    std::fs::write(
        &file,
        r#"<entity-facade-xml><moqui.test.TestEntity testId="K1" testMedium="other"/></entity-facade-xml>"#,
    )
    .unwrap();
    let differs = crate::entity::load_feed::check_entity_data(&facade, location, &quiet).unwrap();
    assert_eq!(differs.messages.len(), 2, "{:?}", differs.messages);
    assert!(differs.messages[1].starts_with("-- Checked data (1 records)"));
}
const ADMIN_MEMBER: &str = r#"<entity-facade-xml><moqui.security.UserGroupMember userId="SEC-NONE" userGroupId="ADMIN" fromDate="2001-02-03 04:05:06.0"/></entity-facade-xml>"#;

fn open_eca() -> (EntityFacade, crate::context::ExecutionContext) {
    let facade = crate::testing::open_with(
        &framework_dir(),
        crate::testing::Options {
            entity_eca: true,
            ..Default::default()
        },
    );
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    (facade, context)
}

fn admin_member_count(facade: &EntityFacade) -> usize {
    facade
        .find("moqui.security.UserGroupMember")
        .unwrap()
        .eq("userId", Value::string("SEC-NONE"))
        .eq("userGroupId", Value::string("ADMIN"))
        .list()
        .unwrap()
        .len()
}

#[test]
fn an_admin_group_member_is_not_stored_while_entity_eca_runs() {
    let (facade, context) = open_eca();
    let _entered = context.enter();
    let err = load_entity_xml_text(&facade, "xmlText", ADMIN_MEMBER, &LoadOptions::default()).unwrap_err();
    let text = error_chain(&err);
    assert!(
        text.contains("not authorized to change membership of group ADMIN"),
        "{text}"
    );
    assert_eq!(admin_member_count(&facade), 0);
}

// A check (Tools › Data › Load's Check Data) answers the stored value of each field the data gives that differs, so it
// takes no entity a generic read refuses and compares no password or encrypted field (plans/Security-Review-Medium.md
// SEC-TLS-84).
#[test]
fn a_check_answers_no_password_or_encrypted_value_and_refuses_the_restricted_entities() {
    use crate::entity::load_feed::check_info_with;
    let facade = crate::testing::open();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    load_entity_xml_text(
        &facade,
        "xmlText",
        r#"<entity-facade-xml>
            <moqui.security.UserAccount userId="SEC_CHECK" username="sec.check" userFullName="Stored Name"
                currentPassword="stored-hash" passwordSalt="stored-salt" passwordHashType="SHA-256"/>
            <moqui.llm.LlmEndpoint endpointId="SEC_CHECK" description="Stored" apiKey="stored-api-key"/>
            <moqui.security.UserGroup userGroupId="SEC_CHECK" description="Check group"/>
            <moqui.security.UserGroupMember userGroupId="SEC_CHECK" userId="SEC_CHECK" fromDate="2001-02-03 04:05:06.0"/>
        </entity-facade-xml>"#,
        &LoadOptions::default(),
    )
    .unwrap();
    let restricted = LoadOptions {
        restrict_sensitive_entities: true,
        ..LoadOptions::default()
    };
    let check = |text: &str| {
        check_info_with(|| load_entity_xml_text(&facade, "xmlText", text, &restricted)).map(|(_, result)| result)
    };
    let checked = check(
        r#"<entity-facade-xml>
            <moqui.security.UserAccount userId="SEC_CHECK" userFullName="Other Name" currentPassword="guess"
                passwordSalt="guess" passwordHashType="NONE"/>
            <moqui.llm.LlmEndpoint endpointId="SEC_CHECK" description="Other" apiKey="guess"/>
        </entity-facade-xml>"#,
    )
    .unwrap();
    let text = |row: &moqui_lib::object::ValueMap, name: &str| {
        row.get(name)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    };
    let fields: Vec<String> = checked.diff_info.iter().map(|row| text(row, "field")).collect();
    assert_eq!(fields, ["userFullName", "description"], "{:?}", checked.diff_info);
    let stored: Vec<String> = checked.diff_info.iter().map(|row| text(row, "dbValue")).collect();
    assert_eq!(stored, ["Stored Name", "Stored"]);
    // a restricted entity is refused, as a load refuses it, and its stored values are not read
    let err = check(
        r#"<entity-facade-xml><moqui.security.UserGroupMember userGroupId="SEC_CHECK" userId="SEC_CHECK"
            fromDate="2001-02-03 04:05:06.0" thruDate="2001-02-04 00:00:00.0"/></entity-facade-xml>"#,
    )
    .unwrap_err();
    assert!(
        error_chain(&err).contains("is not allowed in this data load"),
        "{}",
        error_chain(&err)
    );
    // a check without the option (the testkit's checkData) reads any entity and compares every field
    let (_, open) = check_info_with(|| {
        load_entity_xml_text(
            &facade,
            "xmlText",
            r#"<entity-facade-xml>
                <moqui.security.UserGroupMember userGroupId="SEC_CHECK" userId="SEC_CHECK" fromDate="2001-02-03 04:05:06.0"/>
                <moqui.security.UserAccount userId="SEC_CHECK" passwordSalt="guess"/>
            </entity-facade-xml>"#,
            &LoadOptions::default(),
        )
    })
    .unwrap();
    let fields: Vec<String> = open.diff_info.iter().map(|row| text(row, "field")).collect();
    assert_eq!(fields, ["passwordSalt"], "{:?}", open.diff_info);
}

// A load's own switch is this thread's and ends with its scope, nested or not; it leaves the security checks
// (plans/Security-Review-Medium.md SEC-DAT-83).
#[test]
fn a_loads_switch_is_this_threads_and_leaves_the_security_checks() {
    use crate::entity::WriteSwitches;
    let (facade, context) = open_eca();
    let _entered = context.enter();
    // the facade's own flags, which every thread sees
    let shared = facade.shared_flags();
    assert!(!shared.disable_entity_eca);
    let off = WriteSwitches {
        disable_entity_eca: true,
        disable_audit_log: true,
        ..WriteSwitches::default()
    };
    {
        let _outer = off.enter();
        assert!(facade.flags().disable_entity_eca && facade.flags().disable_audit_log);
        assert_eq!(facade.shared_flags(), shared);
        let elsewhere = {
            let facade = facade.clone();
            std::thread::spawn(move || facade.flags()).join().unwrap()
        };
        assert_eq!(elsewhere, shared);
        {
            let _inner = WriteSwitches {
                disable_data_feed: true,
                ..WriteSwitches::default()
            }
            .enter();
            assert!(facade.flags().disable_data_feed && facade.flags().disable_entity_eca);
        }
        assert_eq!(WriteSwitches::current(), off);
        // the privileged-group check still runs: an ADMIN member is refused to a caller who is not in ADMIN
        let err = load_entity_xml_text(&facade, "xmlText", ADMIN_MEMBER, &LoadOptions::default()).unwrap_err();
        assert!(
            error_chain(&err).contains("not authorized to change membership of group ADMIN"),
            "{}",
            error_chain(&err)
        );
        assert_eq!(admin_member_count(&facade), 0);
    }
    assert_eq!(WriteSwitches::current(), WriteSwitches::default());
    // a panic in the load puts them back too
    let unwound = std::panic::catch_unwind(|| {
        let _scope = off.enter();
        panic!("a load that fails");
    });
    assert!(unwound.is_err());
    assert_eq!(WriteSwitches::current(), WriteSwitches::default());
}

/// Rows created together run their entity's create ECA rules as a create does, the entity named by its short name too:
/// each row then goes through its own create, and the security check refuses the admin group's member.
#[test]
fn rows_created_together_by_a_short_name_run_the_create_eca_rules() {
    let (facade, context) = open_eca();
    let _entered = context.enter();
    let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
    member.set("userId", Value::string("SEC-NONE"));
    member.set("userGroupId", Value::string("ADMIN"));
    member.set("fromDate", Value::string("2001-02-03 04:05:06.0"));
    let err = facade.create_values("UserGroupMember", vec![member]).unwrap_err();
    let text = error_chain(&err);
    assert!(
        text.contains("not authorized to change membership of group ADMIN"),
        "{text}"
    );
    assert_eq!(admin_member_count(&facade), 0);
}

#[test]
fn disable_entity_eca_stores_an_admin_group_member() {
    let (facade, context) = open_eca();
    let _entered = context.enter();
    assert!(eca_rules(&facade, "moqui.security.UserGroupMember")
        .iter()
        .any(|rule| rule.actions.contains("assertPrivilegedGroupMembership")));
    facade.set_disable_entity_eca(true);
    let loaded = load_entity_xml_text(&facade, "xmlText", ADMIN_MEMBER, &LoadOptions::default()).unwrap();
    assert_eq!(loaded.records, 1);
    assert_eq!(admin_member_count(&facade), 1);
}
