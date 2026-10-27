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
use std::path::{Path, PathBuf};

use moqui_lib::object::Value;

use super::Browser;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn framework() -> PathBuf {
    crate_dir().join("../../runtime/base-component/framework")
}

/// A copy of `crates/moqui-browser/runtime` in a temp dir. The factory writes `.env` there.
struct TempRuntime(PathBuf);

impl TempRuntime {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-browser-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(&crate_dir().join("runtime"), &dir);
        Self(dir)
    }
}

impl Drop for TempRuntime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn fields(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

#[test]
fn entity_crud_view_find_and_sequences_on_sqlite() {
    let runtime = TempRuntime::new("crud");
    let browser = Browser::open(&runtime.0, &framework(), ":memory:").unwrap();
    let facade = browser.entity();
    // Writes read DataFeed and the ECA tables, so a usable database has every framework table.
    browser.check_tables(&[]).unwrap();
    for name in ["moqui.test.TestEntity", "moqui.test.Foo", "moqui.test.Bar"] {
        assert!(facade.table_exists(name).unwrap(), "{name} was not created");
    }
    // A second check finds the tables and columns and changes nothing.
    browser.check_tables(&["moqui.test.TestEntity"]).unwrap();

    browser
        .store(
            "moqui.test.TestEntity",
            &fields(&[("testId", Value::string("T1")), ("testMedium", Value::string("hello"))]),
        )
        .unwrap();
    let found = browser
        .find(
            "moqui.test.TestEntity",
            &fields(&[("testId", Value::string("T1"))]),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].get("testMedium"), Some(&Value::string("hello")));

    browser
        .store(
            "moqui.test.TestEntity",
            &fields(&[
                ("testId", Value::string("T1")),
                ("testMedium", Value::string("changed")),
            ]),
        )
        .unwrap();
    browser
        .request(true, || {
            let mut row = facade.make_value("moqui.test.TestEntity")?;
            row.set("testId", Value::string("T2"));
            row.create(facade)?;
            row.delete(facade)
        })
        .unwrap();
    let all = browser
        .find("moqui.test.TestEntity", &BTreeMap::new(), &[], None)
        .unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].get("testMedium"), Some(&Value::string("changed")));

    browser
        .store(
            "moqui.test.Foo",
            &fields(&[("fooId", Value::string("F1")), ("fooText", Value::string("alpha"))]),
        )
        .unwrap();
    browser
        .store(
            "moqui.test.Bar",
            &fields(&[
                ("barId", Value::string("B1")),
                ("fooId", Value::string("F1")),
                ("barRank", Value::I64(3)),
            ]),
        )
        .unwrap();
    let joined = browser.find("moqui.test.FooBar", &BTreeMap::new(), &[], None).unwrap();
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].get("fooText"), Some(&Value::string("alpha")));
    assert_eq!(joined[0].get("barRank"), Some(&Value::I64(3)));

    // Sequenced ids refill on the reserve connection inside the open transaction.
    let ids = browser
        .request(true, || {
            facade.begin("moqui.basic.Enumeration", None)?;
            let mut ids = Vec::new();
            for index in 0..3 {
                let mut enumeration = facade.make_value("moqui.basic.Enumeration")?;
                enumeration.set_sequenced_id_primary(facade)?;
                enumeration.set("description", Value::string(format!("E{index}")));
                enumeration.create(facade)?;
                ids.push(enumeration.get("enumId").cloned().unwrap_or(Value::Null));
            }
            facade.commit("moqui.basic.Enumeration")?;
            Ok(ids)
        })
        .unwrap();
    assert_eq!(ids.len(), 3);
    assert_ne!(ids[0], ids[1]);
    let counted = browser
        .request(true, || facade.find("moqui.basic.Enumeration")?.count())
        .unwrap();
    assert_eq!(counted, 3);
    // Limit and offset with the limit style.
    let page = browser
        .request(true, || {
            Ok(facade
                .find("moqui.basic.Enumeration")?
                .order_by("description", false)
                .offset(1)
                .limit(1)
                .list()?
                .len())
        })
        .unwrap();
    assert_eq!(page, 1);
}

#[test]
fn renders_a_text_screen() {
    let runtime = TempRuntime::new("render");
    let browser = Browser::open(&runtime.0, &framework(), ":memory:").unwrap();
    // Even an anonymous screen reads ArtifactAuthz.
    browser.check_tables(&[]).unwrap();
    let root = runtime.0.join("screen/Hello.xml").display().to_string();
    let (status, body) = browser
        .render(&root, "", "text", fields(&[("name", Value::string("tester"))]))
        .unwrap();
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Hello tester from Moqui in WASM"), "{body}");
}

/// Every framework entity, then the framework seed data, the way `moqui load` starts.
#[test]
fn framework_tables_and_seed_data_load() {
    let runtime = TempRuntime::new("seed");
    let browser = Browser::open(&runtime.0, &framework(), ":memory:").unwrap();
    browser.check_tables(&[]).unwrap();
    let mut loaded = 0;
    let mut files: Vec<PathBuf> = std::fs::read_dir(framework().join("data"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "xml"))
        .collect();
    files.sort();
    let types = vec!["seed".to_string(), "seed-initial".to_string()];
    let mut failures = Vec::new();
    for file in &files {
        match browser.load_data(&file.display().to_string(), &types) {
            Ok(records) => loaded += records,
            Err(err) => failures.push(format!("{}: {err}", file.display())),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(loaded > 100, "only {loaded} records");
    let enums = browser
        .request(true, || browser.entity().find("moqui.basic.Enumeration")?.count())
        .unwrap();
    assert!(enums > 10, "{enums} enumerations");
}

/// `force_new` suspends the caller's transaction and takes a second connection, as `callFuture()`
/// and `requires-new` do. With every logical connection on one SQLite handle the inner transaction is a
/// savepoint in the outer one, so a write in both works instead of failing on the single writer.
#[test]
fn a_suspended_write_transaction_and_a_new_one_both_write() {
    let runtime = TempRuntime::new("suspend");
    let browser = Browser::open(&runtime.0, &framework(), ":memory:").unwrap();
    browser.check_tables(&[]).unwrap();
    let facade = browser.entity();
    let write = |id: &str| -> Result<(), moqui_lib::error::Error> {
        let mut row = facade.make_value("moqui.test.TestEntity")?;
        row.set("testId", Value::string(id));
        row.create(facade)
    };
    browser
        .request(true, || {
            let tx = facade.transactions_for("moqui.test.TestEntity")?;
            tx.begin(None)?;
            write("OUTER")?;
            assert!(tx.force_new(None)?);
            write("INNER")?;
            // Commit of the new transaction resumes the parked one.
            tx.commit()?;
            write("OUTER2")?;
            tx.commit()
        })
        .unwrap();
    let all = browser
        .find("moqui.test.TestEntity", &BTreeMap::new(), &[], None)
        .unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn custom_runtime_loads_component_entities_extensions_and_merged_configuration() {
    use moqui_core::driver::DbDriver;
    let runtime = TempRuntime::new("component-model");
    let component = runtime.0.join("component/custom-model");
    for dir in ["entity", "extra", "service/test/browser"] {
        std::fs::create_dir_all(component.join(dir)).unwrap();
    }
    std::fs::write(
        component.join("component.xml"),
        r#"<component name="custom-model" version="1"/>"#,
    )
    .unwrap();
    std::fs::write(
        component.join("MoquiConf.xml"),
        r#"<moqui-conf><entity-facade sql-name-style="java" crypt-pass="component-pass">
        <load-entity location="component://custom-model/extra/Extra.xml"/>
    </entity-facade></moqui-conf>"#,
    )
    .unwrap();
    std::fs::write(
        runtime.0.join("conf/MoquiDevConf.xml"),
        r#"<moqui-conf><entity-facade sql-name-style="legacy" crypt-pass="runtime-pass"/></moqui-conf>"#,
    )
    .unwrap();
    std::fs::write(component.join("entity/Model.xml"), r#"<entities>
        <entity entity-name="RuntimeRecord" package="test.browser">
            <field name="recordId" type="id" is-pk="true"/>
            <field name="secretText" type="text-medium" encrypt="true"/>
        </entity>
        <extend-entity entity-name="TestEntity" package="moqui.test"><field name="componentNote" type="text-medium"/></extend-entity>
    </entities>"#).unwrap();
    std::fs::write(
        component.join("extra/Extra.xml"),
        r#"<entities><entity entity-name="ExtraRecord" package="test.browser">
        <field name="recordId" type="id" is-pk="true"/>
    </entity></entities>"#,
    )
    .unwrap();
    std::fs::write(component.join("service/test/browser/ModelServices.xml"), r#"<services>
        <service verb="create" noun="test.browser.RuntimeRecord" type="entity-auto" authenticate="false"><in-parameters><auto-parameters/></in-parameters><out-parameters><auto-parameters include="pk"/></out-parameters></service>
    </services>"#).unwrap();
    let browser = Browser::open(&runtime.0, &framework(), ":memory:").unwrap();
    assert_eq!(
        browser.entity().entity_dirs(),
        vec![
            framework().join("entity").canonicalize().unwrap(),
            component.join("entity").canonicalize().unwrap()
        ]
    );
    assert!(browser
        .entity()
        .entity_files()
        .iter()
        .any(|file| file.ends_with("extra/Extra.xml")));
    assert_eq!(browser.entity().crypt().unwrap().0.pass, "runtime-pass");
    browser.check_tables(&[]).unwrap();
    let parameters = fields(&[
        ("recordId", Value::string("R1")),
        ("secretText", Value::string("private text")),
    ]);
    let result = browser
        .call_service(
            "test.browser.ModelServices.create#test.browser.RuntimeRecord",
            &parameters,
        )
        .unwrap();
    assert_eq!(result.get("recordId"), Some(&Value::string("R1")));
    let found = browser
        .find(
            "test.browser.RuntimeRecord",
            &fields(&[("recordId", Value::string("R1"))]),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(found[0].get("secretText"), Some(&Value::string("private text")));
    browser
        .store(
            "moqui.test.TestEntity",
            &fields(&[
                ("testId", Value::string("T1")),
                ("componentNote", Value::string("from the component")),
            ]),
        )
        .unwrap();
    let found = browser
        .find(
            "moqui.test.TestEntity",
            &fields(&[("testId", Value::string("T1"))]),
            &[],
            None,
        )
        .unwrap();
    assert_eq!(
        found[0].get("componentNote"),
        Some(&Value::string("from the component"))
    );
    browser
        .store(
            "test.browser.ExtraRecord",
            &fields(&[("recordId", Value::string("E1"))]),
        )
        .unwrap();
    let mut connection = browser.sqlite.connect().unwrap();
    let stored = connection
        .query(
            "SELECT secretText FROM RuntimeRecord WHERE recordId = ?",
            &[Value::string("R1")],
        )
        .unwrap();
    let encrypted = stored[0].columns[0].1.as_str().unwrap();
    assert_ne!(encrypted, "private text");
    assert!(!encrypted.is_empty());
}
