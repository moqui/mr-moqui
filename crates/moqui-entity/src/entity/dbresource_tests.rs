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

use crate::entity::EntityFacade;

fn facade() -> EntityFacade {
    let facade = crate::testing::open();
    facade.assume_db_tables();
    facade
}

#[test]
fn first_write_is_version_01_with_its_history_row() {
    let facade = facade();
    facade.write_db_resource("dbresource://wiki/a.md", b"one").unwrap();
    let current = facade
        .db_resource_current_version("dbresource://wiki/a.md")
        .unwrap()
        .expect("current");
    assert_eq!(current.version_name, "01");
    assert_eq!(current.previous_version_name, None);
    assert!(!current.version_date.is_null());
    let root = facade
        .db_resource_root_version("dbresource://wiki/a.md")
        .unwrap()
        .expect("root");
    assert_eq!(root.version_name, "01");
    assert_eq!(
        facade.read_db_resource_version("dbresource://wiki/a.md", "01").unwrap(),
        Some(b"one".to_vec())
    );
}

#[test]
fn a_later_write_keeps_the_past_version_and_adds_the_next() {
    let facade = facade();
    let location = "dbresource://wiki/b.md";
    facade.write_db_resource(location, b"one").unwrap();
    facade.write_db_resource(location, b"two").unwrap();
    facade.write_db_resource(location, b"three").unwrap();
    let current = facade.db_resource_current_version(location).unwrap().expect("current");
    assert_eq!(current.version_name, "03");
    assert_eq!(current.previous_version_name.as_deref(), Some("02"));
    assert_eq!(
        facade
            .db_resource_root_version(location)
            .unwrap()
            .expect("root")
            .version_name,
        "01"
    );
    assert_eq!(
        facade.read_db_resource_version(location, "01").unwrap(),
        Some(b"one".to_vec())
    );
    assert_eq!(
        facade.read_db_resource_version(location, "02").unwrap(),
        Some(b"two".to_vec())
    );
    assert_eq!(
        facade.read_db_resource_version(location, "03").unwrap(),
        Some(b"three".to_vec())
    );
    assert_eq!(facade.read_db_resource_version(location, "09").unwrap(), None);
    assert_eq!(facade.read_db_resource(location).unwrap().bytes, b"three".to_vec());
    let names: Vec<String> = facade
        .db_resource_version_history(location)
        .unwrap()
        .into_iter()
        .map(|version| version.version_name)
        .collect();
    assert_eq!(names, ["03", "02", "01"]);
    assert_eq!(
        facade
            .db_resource_version(location, "02")
            .unwrap()
            .expect("02")
            .previous_version_name
            .as_deref(),
        Some("01")
    );
}

#[test]
fn a_write_across_a_file_or_onto_a_directory_fails() {
    let facade = facade();
    facade.write_db_resource("dbresource://tree/file.txt", b"one").unwrap();
    let err = facade
        .write_db_resource("dbresource://tree/file.txt/below.txt", b"x")
        .unwrap_err();
    assert!(err.to_string().contains("found file instead"), "{err}");
    let err = facade.write_db_resource("dbresource://tree", b"x").unwrap_err();
    assert!(err.to_string().contains("is a directory"), "{err}");
    let tree = facade.read_db_resource("dbresource://tree").unwrap();
    assert!(tree.exists && !tree.is_file);
    assert_eq!(tree.children.len(), 1);
    let file = facade.read_db_resource("dbresource://tree/file.txt").unwrap();
    assert!(file.is_file && file.children.is_empty());
    assert_eq!(file.bytes, b"one".to_vec());
}

#[test]
fn the_first_write_sets_the_mime_type_and_children_are_in_filename_order() {
    let facade = facade();
    for name in ["c.md", "a.json", "b.bin"] {
        facade
            .write_db_resource(&format!("dbresource://order/{name}"), b"x")
            .unwrap();
    }
    let names: Vec<String> = facade
        .read_db_resource("dbresource://order")
        .unwrap()
        .children
        .into_iter()
        .map(|child| child.name)
        .collect();
    assert_eq!(names, ["a.json", "b.bin", "c.md"]);
    let mime = |name: &str| {
        let (id, row) = facade.file_row(&format!("dbresource://order/{name}")).unwrap().unwrap();
        assert!(!id.is_empty());
        row.get("mimeType")
            .and_then(moqui_lib::object::Value::as_str)
            .map(str::to_string)
    };
    assert_eq!(mime("a.json").as_deref(), Some("application/json"));
    assert_eq!(mime("c.md").as_deref(), Some("text/plain"));
    assert_eq!(mime("b.bin"), None);
}

#[test]
fn next_versions_are_the_versions_made_from_one() {
    let facade = facade();
    let location = "dbresource://wiki/n.md";
    facade.write_db_resource(location, b"one").unwrap();
    facade.write_db_resource(location, b"two").unwrap();
    let next: Vec<String> = facade
        .db_resource_next_versions(location, "01")
        .unwrap()
        .into_iter()
        .map(|version| version.version_name)
        .collect();
    assert_eq!(next, ["02"]);
    assert!(facade.db_resource_next_versions(location, "02").unwrap().is_empty());
    assert!(facade
        .db_resource_next_versions("dbresource://wiki/none.md", "01")
        .unwrap()
        .is_empty());
}

#[test]
fn delete_removes_the_history_and_a_missing_file_has_no_versions() {
    let facade = facade();
    let location = "dbresource://wiki/c.md";
    facade.write_db_resource(location, b"one").unwrap();
    facade.write_db_resource(location, b"two").unwrap();
    assert!(facade.delete_db_resource(location).unwrap());
    assert!(facade.db_resource_version_history(location).unwrap().is_empty());
    assert_eq!(facade.db_resource_current_version(location).unwrap(), None);
    let rows = facade
        .find("moqui.resource.DbResourceFileHistory")
        .unwrap()
        .list()
        .unwrap();
    assert!(rows.is_empty(), "{} history rows left", rows.len());
}

/// A page of numbered lines with line `edit` changed, big enough that a one-line change is stored as a delta.
fn page(edit: usize) -> String {
    (1..=40)
        .map(|line| {
            if line == edit {
                format!("line {line} edited in version {edit}\n")
            } else {
                format!("line {line}\n")
            }
        })
        .collect()
}

fn history_rows(facade: &EntityFacade) -> Vec<(String, String, Option<Vec<u8>>)> {
    facade
        .find("moqui.resource.DbResourceFileHistory")
        .unwrap()
        .order_by("versionName", false)
        .list()
        .unwrap()
        .iter()
        .map(|row| {
            (
                super::text_of(row.get("versionName")).unwrap_or_default(),
                super::text_of(row.get("isDiff")).unwrap_or_default(),
                super::blob_bytes(row.get("fileData"), "test").unwrap(),
            )
        })
        .collect()
}

#[test]
fn past_text_versions_are_reverse_deltas_and_every_version_reads_back() {
    let facade = facade();
    let location = "dbresource://wiki/long.md";
    for version in 1..=25 {
        facade.write_db_resource(location, page(version).as_bytes()).unwrap();
    }
    for version in 1..=25 {
        let name = format!("{version:02}");
        let bytes = facade.read_db_resource_version(location, &name).unwrap();
        assert_eq!(bytes, Some(page(version).into_bytes()), "version {name}");
    }
    let rows = history_rows(&facade);
    assert_eq!(rows.len(), 25);
    for (name, is_diff, data) in &rows {
        let number: u32 = name.parse().unwrap();
        match number {
            25 => assert!(data.is_none(), "the current version's row holds no bytes"),
            20 => assert_eq!((is_diff.as_str(), data.as_deref()), ("N", Some(page(20).as_bytes()))),
            _ => {
                assert_eq!(is_diff, "Y", "version {name}");
                let delta = String::from_utf8(data.clone().unwrap()).unwrap();
                assert!(
                    delta.starts_with(&format!("--- {:02}\n+++ {name}\n", number + 1)),
                    "{delta}"
                );
            }
        }
    }
}

#[test]
fn binary_bytes_and_large_rewrites_are_stored_whole() {
    let facade = facade();
    let image = "dbresource://files/a.png";
    let big = page(0).repeat(3);
    facade.write_db_resource(image, big.as_bytes()).unwrap();
    facade
        .write_db_resource(image, (big.clone() + "x\n").as_bytes())
        .unwrap();
    let invalid = "dbresource://files/b.txt";
    let mut latin1 = page(0).into_bytes();
    latin1.push(0xE9);
    facade.write_db_resource(invalid, &latin1).unwrap();
    facade.write_db_resource(invalid, page(3).as_bytes()).unwrap();
    let rewrite = "dbresource://files/c.txt";
    facade.write_db_resource(rewrite, page(0).as_bytes()).unwrap();
    facade.write_db_resource(rewrite, b"something else entirely\n").unwrap();
    let first: Vec<_> = history_rows(&facade)
        .into_iter()
        .filter(|(name, ..)| name == "01")
        .collect();
    assert_eq!(first.len(), 3);
    for (_, is_diff, data) in first {
        assert_eq!(is_diff, "N");
        assert!(data.is_some());
    }
    assert_eq!(
        facade.read_db_resource_version(image, "01").unwrap(),
        Some(big.into_bytes())
    );
    assert_eq!(facade.read_db_resource_version(invalid, "01").unwrap(), Some(latin1));
    assert_eq!(
        facade.read_db_resource_version(rewrite, "01").unwrap(),
        Some(page(0).into_bytes())
    );
}

#[test]
fn a_broken_delta_reads_as_no_version_and_the_others_still_read() {
    let facade = facade();
    let location = "dbresource://wiki/broken.md";
    for version in 1..=4 {
        facade.write_db_resource(location, page(version).as_bytes()).unwrap();
    }
    let (resource_id, _) = facade.file_row(location).unwrap().unwrap();
    let mut row = crate::entity::EntityValue::make(&facade, "moqui.resource.DbResourceFileHistory").unwrap();
    row.set("resourceId", moqui_lib::object::Value::string(&resource_id));
    row.set("versionName", moqui_lib::object::Value::string("02"));
    row.set(
        "fileData",
        moqui_lib::object::Value::Bytes(b"@@ -1 +1 @@\n-nope\n+still nope\n".to_vec()),
    );
    row.update(&facade).unwrap();
    assert_eq!(
        facade.read_db_resource_version(location, "03").unwrap(),
        Some(page(3).into_bytes())
    );
    assert_eq!(facade.read_db_resource_version(location, "02").unwrap(), None);
    // 01 is rebuilt through 02's delta, so it is lost with it
    assert_eq!(facade.read_db_resource_version(location, "01").unwrap(), None);
    assert_eq!(
        facade.read_db_resource_version(location, "04").unwrap(),
        Some(page(4).into_bytes())
    );
}

#[test]
fn large_content_is_a_content_blob_that_past_versions_keep() {
    let facade = facade();
    let location = "dbresource://media/clip.bin";
    let first: Vec<u8> = (0..200_000u32).map(|n| (n * 13 % 251) as u8).collect();
    let second: Vec<u8> = first.iter().rev().copied().collect();
    facade.write_db_resource(location, &first).unwrap();
    let (_, row) = facade.file_row(location).unwrap().unwrap();
    let blob = super::text_of(row.get("blobId")).expect("a blob");
    assert_eq!(blob, crate::resource::blob::blob_id(&first));
    assert!(row.get("fileData").is_none_or(moqui_lib::object::Value::is_null));
    assert_eq!(facade.stat_db_resource(location).unwrap().unwrap().size, 200_000);
    assert_eq!(facade.read_db_resource_bytes(location).unwrap().unwrap(), first);

    // a new version: the past one keeps its blob, no copy
    let mut input = &second[..];
    facade
        .write_db_resource_from(location, &mut input, Some(second.len() as u64))
        .unwrap();
    assert_eq!(facade.read_db_resource_bytes(location).unwrap().unwrap(), second);
    assert_eq!(
        facade.read_db_resource_version(location, "01").unwrap(),
        Some(first.clone())
    );
    let past: Vec<_> = history_rows(&facade)
        .into_iter()
        .filter(|(name, ..)| name == "01")
        .collect();
    assert_eq!(past.len(), 1);
    assert!(past[0].2.is_none(), "the past row holds no bytes of its own");
    let (stat, source) = facade.open_db_resource(location).unwrap().unwrap();
    assert_eq!(stat.size, second.len() as u64);
    assert_eq!(source.read_at(199_990, 100).unwrap(), second[199_990..]);
    // the same content elsewhere is the same blob
    facade.write_db_resource("dbresource://media/copy.bin", &first).unwrap();
    let blobs = facade.find("moqui.resource.ContentBlob").unwrap().list().unwrap();
    assert_eq!(blobs.len(), 2);

    // a row written before content blobs moves into one
    facade
        .write_db_resource("dbresource://media/old.bin", b"small")
        .unwrap();
    let (_, mut old) = facade.file_row("dbresource://media/old.bin").unwrap().unwrap();
    old.set("fileData", moqui_lib::object::Value::Bytes(second.clone()));
    old.set("fileSize", moqui_lib::object::Value::Null);
    old.update(&facade).unwrap();
    let (moved, _) = facade
        .migrate_db_resource_blobs(std::time::Instant::now() + std::time::Duration::from_secs(60))
        .unwrap();
    assert_eq!(moved, 1);
    let (_, old) = facade.file_row("dbresource://media/old.bin").unwrap().unwrap();
    assert_eq!(
        super::text_of(old.get("blobId")),
        Some(crate::resource::blob::blob_id(&second))
    );
    assert_eq!(
        facade
            .read_db_resource_bytes("dbresource://media/old.bin")
            .unwrap()
            .unwrap(),
        second
    );
    assert_eq!(
        facade
            .stat_db_resource("dbresource://media/old.bin")
            .unwrap()
            .unwrap()
            .size,
        200_000
    );
}
