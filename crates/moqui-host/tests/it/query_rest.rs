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

//! Dynamic view, entity data writer, and `EntityFacade.rest` against embedded Postgres.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::driver::DbDriver;
use moqui_core::transaction::PoolConfig;
use moqui_lib::object::Value;

use moqui_host::db::embedded::EmbeddedCluster;
use moqui_host::db::entity_facade::open_entity_facade;

#[test]
fn rest_dynamic_view_and_writer() {
    let _lock = crate::common::shared();
    let dir = temp_cluster_dir();
    let _data_guard = DirGuard(dir.clone());
    let mut export_name = dir.file_name().unwrap().to_os_string();
    export_name.push("-export");
    let export = dir.with_file_name(export_name);
    let _export_guard = DirGuard(export.clone());
    std::fs::create_dir_all(&export).unwrap();
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/components/base/framework");
    let facade = open_entity_facade(
        &framework,
        driver,
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            acquire_timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .unwrap();
    facade
        .check_tables(&[
            "moqui.test.TestEntity",
            "moqui.test.Foo",
            "moqui.test.Bar",
            // Writes run the entity-auto services; a service frame reads the tarpit and authz checks.
            "moqui.security.ArtifactTarpit",
            "moqui.security.ArtifactAuthz",
            "moqui.security.ArtifactGroupMember",
        ])
        .unwrap();
    let factory_dir = dir.join("runtime");
    std::fs::create_dir_all(&factory_dir).unwrap();
    let factory = moqui_core::context::ExecutionContextFactory::boot_with_framework(&factory_dir, &framework).unwrap();
    moqui_host::db::entity_facade::attach(&factory, facade.clone());
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();

    let created = facade
        .rest(
            "create",
            "moqui.test.TestEntity",
            &params(vec![
                ("path", Value::List(vec![Value::string("R1")])),
                ("testMedium", Value::string("created")),
            ]),
        )
        .unwrap();
    assert_eq!(map_str(&created, "testId"), Some("R1"));
    let found = facade
        .rest(
            "find",
            "moqui.test.TestEntity",
            &params(vec![("path", Value::List(vec![Value::string("R1")]))]),
        )
        .unwrap();
    assert_eq!(map_str(&found, "testMedium"), Some("created"));
    assert_eq!(map_str(&found, "_entity"), Some("moqui.test.TestEntity"));

    facade
        .rest(
            "update",
            "moqui.test.TestEntity",
            &params(vec![
                ("testId", Value::string("R1")),
                ("testMedium", Value::string("updated")),
            ]),
        )
        .unwrap();
    let updated = facade
        .rest(
            "find",
            "moqui.test.TestEntity",
            &params(vec![("testId", Value::string("R1"))]),
        )
        .unwrap();
    assert_eq!(map_str(&updated, "testMedium"), Some("updated"));

    facade
        .rest(
            "store",
            "moqui.test.TestEntity",
            &params(vec![
                ("testId", Value::string("R1")),
                ("testMedium", Value::string("stored")),
            ]),
        )
        .unwrap();
    let stored = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("R1"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(stored.get("testMedium").and_then(Value::as_str), Some("stored"));
    let via_rest = facade
        .rest(
            "find",
            "moqui.test.TestEntity",
            &params(vec![("testId", Value::string("R1"))]),
        )
        .unwrap();
    assert_eq!(map_str(&via_rest, "testMedium"), Some("stored"));
    let err = facade
        .rest(
            "find",
            "moqui.test.TestEntity",
            &params(vec![
                ("testId", Value::string("R1")),
                ("master", Value::string("default")),
            ]),
        )
        .unwrap_err();
    assert!(err.to_string().contains("No master definition found"), "{err}");

    facade
        .rest(
            "delete",
            "moqui.test.TestEntity",
            &params(vec![("testId", Value::string("R1"))]),
        )
        .unwrap();
    assert!(facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("R1"))
        .one()
        .unwrap()
        .is_none());
    let err = facade
        .rest(
            "get",
            "moqui.test.TestEntity",
            &params(vec![("testId", Value::string("R1"))]),
        )
        .unwrap_err();
    assert!(err.to_string().contains("No value found"), "{err}");

    for (foo_id, text) in [("F1", "alpha"), ("F2", "beta")] {
        let mut foo = facade.make_value("moqui.test.Foo").unwrap();
        foo.set("fooId", Value::string(foo_id));
        foo.set("fooText", Value::string(text));
        foo.create(&facade).unwrap();
    }
    let mut bar = facade.make_value("moqui.test.Bar").unwrap();
    bar.set("barId", Value::string("B1"));
    bar.set("fooId", Value::string("F1"));
    bar.set("barRank", Value::I64(3));
    bar.create(&facade).unwrap();
    let rows = facade
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
                    "alias",
                    Value::List(vec![
                        value_map(vec![
                            ("entity-alias", Value::string("FOO")),
                            ("name", Value::string("fooText")),
                            ("field", Value::string("fooText")),
                        ]),
                        value_map(vec![
                            ("entity-alias", Value::string("BAR")),
                            ("name", Value::string("barRank")),
                            ("field", Value::string("barRank")),
                        ]),
                    ]),
                ),
            ]),
        )
        .unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].get("fooText").and_then(Value::as_str), Some("alpha"));
    assert_eq!(rows[0].get("barRank"), Some(&Value::I64(3)));

    facade
        .write_data(
            "moqui.test.TestEntity",
            &[params(vec![
                ("testId", Value::string("W1")),
                ("testMedium", Value::string("written")),
                ("directory", Value::string(export.display().to_string())),
            ])],
        )
        .unwrap();
    let xml = std::fs::read_to_string(export.join("moqui.test.TestEntity.xml")).unwrap();
    assert!(xml.contains("<entity-facade-xml>"), "{xml}");
    assert!(xml.contains("testId=\"W1\""), "{xml}");
    let csv = std::fs::read_to_string(export.join("moqui.test.TestEntity.csv")).unwrap();
    assert!(csv.starts_with("moqui.test.TestEntity,export,testId\n"), "{csv}");
    let json = std::fs::read_to_string(export.join("moqui.test.TestEntity.json")).unwrap();
    assert!(json.contains("\"_entity\": \"moqui.test.TestEntity\""), "{json}");
    assert!(json.contains("\"testId\": \"W1\""), "{json}");

    drop(facade);
    drop(cluster);
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

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_cluster_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("moqui-pg-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}
