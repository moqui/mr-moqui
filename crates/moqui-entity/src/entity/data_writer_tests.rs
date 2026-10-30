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

use moqui_lib::object::Value;

use super::{DataFileType, EntityDataWriter};
use crate::entity::load_feed::{load_entity_data, LoadOptions};
use crate::entity::EntityFacade;

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
    let path = std::env::temp_dir().join(format!("moqui-writer-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    (path.clone(), DirGuard(path))
}

/// `rows` test entity rows, more than one chunk, and two `Foo` rows.
fn seeded(rows: usize) -> EntityFacade {
    let facade = crate::testing::open();
    for index in 0..rows {
        let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
        row.set("testId", Value::string(format!("T{index:04}")));
        row.set("testMedium", Value::string(format!("medium <{index}>")));
        row.create(&facade).unwrap();
    }
    for id in ["F1", "F2"] {
        let mut row = facade.make_value("moqui.test.Foo").unwrap();
        row.set("fooId", Value::string(id));
        row.create(&facade).unwrap();
    }
    facade
}

fn count(facade: &EntityFacade, entity: &str) -> i64 {
    facade.find(entity).unwrap().count().unwrap()
}

#[test]
fn zip_directory_streams_each_entity_and_loads_back() {
    let source = seeded(1203);
    let (dir, _guard) = temp_dir();
    let zip_path = dir.join("export.zip");
    let zip_name = zip_path.to_str().unwrap();
    let mut writer = EntityDataWriter::new(&source);
    writer
        .entity_name("moqui.test.TestEntity")
        .entity_name("moqui.test.Foo")
        .entity_name("moqui.basic.Enumeration");
    assert_eq!(writer.zip_directory("data", zip_name).unwrap(), 1205);

    let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
    let names: Vec<String> = archive.file_names().map(|name| name.unwrap().into_owned()).collect();
    assert_eq!(
        names,
        ["data/moqui.test.TestEntity.xml", "data/moqui.test.Foo.xml"],
        "no entry for an entity without records"
    );
    assert_eq!(
        archive.by_index(0).unwrap().compression(),
        zip::CompressionMethod::Deflated
    );

    let target = crate::testing::open();
    let loaded = load_entity_data(&target, zip_name, &LoadOptions::default()).unwrap();
    assert_eq!(loaded.records, 1205);
    assert_eq!(count(&target, "moqui.test.TestEntity"), 1203);
    assert_eq!(count(&target, "moqui.test.Foo"), 2);
    let last = target
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("T1202"))
        .one()
        .unwrap()
        .expect("last row");
    assert_eq!(last.get("testMedium").and_then(Value::as_str), Some("medium <1202>"));

    // A zip that is there already is a message error and is left as it was.
    let before = std::fs::read(&zip_path).unwrap();
    assert_eq!(writer.zip_directory("data", zip_name).unwrap(), 0);
    assert_eq!(std::fs::read(&zip_path).unwrap(), before);
}

/// A view's columns are its aliases, as Java's `getAllFieldNames`; with table column names, an alias's column is its
/// name underscored.
#[test]
fn a_csv_of_a_view_has_a_column_per_alias() {
    let source = crate::testing::open();
    let mut kind = source.make_value("moqui.basic.EnumerationType").unwrap();
    kind.set("enumTypeId", Value::string("CsvType"));
    kind.set("description", Value::string("Csv type"));
    kind.create(&source).unwrap();
    let mut row = source.make_value("moqui.basic.Enumeration").unwrap();
    row.set("enumId", Value::string("CsvOne"));
    row.set("enumTypeId", Value::string("CsvType"));
    row.set("description", Value::string("Csv one"));
    row.create(&source).unwrap();

    let mut writer = EntityDataWriter::new(&source);
    writer
        .entity_name("moqui.basic.EnumerationAndType")
        .file_type(DataFileType::Csv)
        .filter_map(BTreeMap::from([("enumTypeId".to_string(), Value::string("CsvType"))]));
    let (csv, written) = writer.text().unwrap();
    assert_eq!(written, 1, "{csv}");
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "moqui.basic.EnumerationAndType,export,enumId", "{csv}");
    let header: Vec<&str> = lines[1].split(',').collect();
    let at = |name: &str| {
        header
            .iter()
            .position(|column| *column == name)
            .unwrap_or_else(|| panic!("no {name} in {csv}"))
    };
    let cells: Vec<&str> = lines[2].split(',').collect();
    assert_eq!(cells[at("enumId")], "CsvOne", "{csv}");
    assert_eq!(cells[at("typeDescription")], "Csv type", "{csv}");

    let mut writer = EntityDataWriter::new(&source);
    writer
        .entity_name("moqui.basic.EnumerationAndType")
        .file_type(DataFileType::Csv)
        .table_column_names(true)
        .filter_map(BTreeMap::from([("enumTypeId".to_string(), Value::string("CsvType"))]));
    let (csv, _) = writer.text().unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert!(lines[0].ends_with(",export,ENUM_ID"), "{csv}");
    let header: Vec<&str> = lines[1].split(',').collect();
    assert!(
        header.contains(&"ENUM_TYPE_ID") && header.contains(&"TYPE_DESCRIPTION"),
        "{csv}"
    );
}

#[test]
fn text_files_and_a_zip_file_hold_every_record() {
    let source = seeded(600);
    let (dir, _guard) = temp_dir();

    let mut writer = EntityDataWriter::new(&source);
    writer.entity_name("moqui.test.TestEntity").file_type(DataFileType::Csv);
    let (csv, written) = writer.text().unwrap();
    assert_eq!(written, 600);
    assert_eq!(csv.lines().count(), 602, "two header lines, then a line per record");

    let json_path = dir.join("rows.json");
    let mut writer = EntityDataWriter::new(&source);
    writer
        .entity_name("moqui.test.TestEntity")
        .entity_name("moqui.test.Foo");
    assert_eq!(writer.file(json_path.to_str().unwrap()).unwrap(), 602);
    let target = crate::testing::open();
    let loaded = load_entity_data(&target, json_path.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(loaded.records, 602);

    let tables = dir.join("tables");
    let mut writer = EntityDataWriter::new(&source);
    writer
        .entity_name("moqui.test.Foo")
        .entity_name("moqui.basic.Enumeration");
    assert_eq!(writer.directory(tables.to_str().unwrap()).unwrap(), 2);
    assert!(tables.join("moqui.test.Foo.xml").exists());
    assert!(!tables.join("moqui.basic.Enumeration.xml").exists());

    let zip_path = dir.join("one.zip");
    let mut writer = EntityDataWriter::new(&source);
    writer.entity_name("moqui.test.TestEntity");
    assert_eq!(writer.zip_file("rows.xml", zip_path.to_str().unwrap()).unwrap(), 600);
    let target = crate::testing::open();
    let loaded = load_entity_data(&target, zip_path.to_str().unwrap(), &LoadOptions::default()).unwrap();
    assert_eq!(loaded.records, 600);
}

/// As Java's writer, an XML record count takes in the nested dependent records, and JSON counts each top record
/// once; a file or directory whose parent is not there is not written, as Java's `createNewFile` and `mkdir`.
#[test]
fn xml_counts_dependents_and_a_missing_parent_is_not_made() {
    let source = crate::testing::open();
    let create = |entity: &str, pairs: &[(&str, &str)]| {
        let mut row = source.make_value(entity).unwrap();
        for (name, value) in pairs {
            row.set(*name, Value::string(*value));
        }
        row.create(&source).unwrap();
    };
    create("moqui.basic.EnumerationType", &[("enumTypeId", "T1")]);
    create("moqui.basic.Enumeration", &[("enumId", "E1"), ("enumTypeId", "T1")]);
    create("moqui.basic.Enumeration", &[("enumId", "E2"), ("enumTypeId", "T1")]);
    let mut writer = EntityDataWriter::new(&source);
    writer.entity_name("moqui.basic.EnumerationType").dependent_levels(1);
    let (xml, written) = writer.text().unwrap();
    assert!(xml.contains("E2"), "{xml}");
    assert_eq!(written, 3);
    writer.file_type(DataFileType::Json);
    assert_eq!(writer.text().unwrap().1, 1);

    let (dir, _guard) = temp_dir();
    let nested = dir.join("missing/rows.xml");
    assert_eq!(writer.file(nested.to_str().unwrap()).unwrap(), 0);
    assert!(!dir.join("missing").exists());
    let deep = dir.join("missing/tables");
    assert_eq!(writer.directory(deep.to_str().unwrap()).unwrap(), 0);
    assert!(!dir.join("missing").exists());
    let zip = dir.join("made/rows.zip");
    assert_eq!(writer.zip_file("rows.json", zip.to_str().unwrap()).unwrap(), 1);
}
