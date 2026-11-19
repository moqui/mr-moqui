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

use std::sync::Arc;
use std::time::Duration;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use super::*;
use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};
use crate::entity::EntityGroup;
use crate::transaction::{PoolConfig, TransactionManager};

struct NoCancel;

impl DbCancel for NoCancel {
    fn cancel(&self) -> Result<(), LibError> {
        Ok(())
    }
}

struct FakeConn;

impl DbConnection for FakeConn {
    fn execute(&mut self, _sql: &str, _params: &[Value]) -> Result<u64, LibError> {
        Ok(0)
    }

    fn query(&mut self, _sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, LibError> {
        Ok(Vec::new())
    }

    fn query_once(&mut self, _sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, LibError> {
        Ok(Vec::new())
    }

    fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), LibError> {
        Ok(())
    }

    fn next_row(&mut self) -> Result<Option<DbRow>, LibError> {
        Ok(None)
    }

    fn close_stream(&mut self) -> Result<(), LibError> {
        Ok(())
    }

    fn stream_open(&self) -> bool {
        false
    }

    fn begin(&mut self) -> Result<(), LibError> {
        Ok(())
    }

    fn commit(&mut self) -> Result<(), LibError> {
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), LibError> {
        Ok(())
    }

    fn savepoint(&mut self, _name: &str) -> Result<(), LibError> {
        Ok(())
    }

    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), LibError> {
        Ok(())
    }

    fn release_savepoint(&mut self, _name: &str) -> Result<(), LibError> {
        Ok(())
    }

    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

struct FakeDriver;

impl DbDriver for FakeDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, LibError> {
        Ok(Box::new(FakeConn))
    }
}

fn facade() -> ResourceFacade {
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
    let entity = EntityFacade::open(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/components/base/framework"),
        "transactional",
        vec![EntityGroup::new("transactional", "postgres", transactions)],
    )
    .unwrap();
    ResourceFacade::new(
        PathBuf::from("/tmp/moqui-resource-test"),
        HashMap::new(),
        Arc::new(RwLock::new(Some(entity))),
    )
}

fn empty_facade() -> ResourceFacade {
    ResourceFacade::new(
        PathBuf::from("/tmp/moqui-resource-test"),
        HashMap::new(),
        Arc::new(RwLock::new(None)),
    )
}

#[test]
fn relative_file_children_are_read_and_written_under_the_relative_root() {
    let root = format!("target/file-resource-{}", std::process::id());
    let resources = empty_facade();
    let location = format!("file:{root}/nested/value.txt");
    resources.write_bytes(&location, b"local").unwrap();
    let parent = resources.get(&format!("file:{root}/nested")).unwrap();
    let children = parent.children().unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].location(), location);
    assert_eq!(resources.get(&location).unwrap().read_text().unwrap(), "local");
    assert_eq!(
        resources.get(&location).unwrap().file_path().unwrap(),
        std::path::Path::new(&format!("{root}/nested/value.txt"))
    );
    assert!(resources.get("file://remote/tmp/a").is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dbresource_without_facade_is_not_available() {
    let err = empty_facade().get("dbresource://notes/hello.txt").unwrap_err();
    assert!(err.to_string().contains("not available"), "{err}");
}

#[test]
fn missing_dbresource_names_the_location() {
    let resources = facade();
    let root = resources.get("dbresource://").unwrap();
    assert!(root.exists());
    assert!(root.is_directory());
    let location = "dbresource://notes/hello.txt";
    let err = resources.get(location).unwrap_err();
    assert!(err.to_string().contains("resource not found"), "{err}");
    assert!(err.to_string().contains(location), "{err}");
}

#[test]
fn db_scheme_stays_unsupported() {
    let err = empty_facade().get("db://notes/hello.txt").unwrap_err();
    assert!(err.to_string().contains("scheme db"), "{err}");
    let err = facade().get("db://notes/hello.txt").unwrap_err();
    assert!(err.to_string().contains("scheme db"), "{err}");
}

#[test]
fn a_location_splits_into_its_scheme_path_and_version() {
    assert_eq!(scheme_of("component://a/b.xml"), "component");
    assert_eq!(scheme_of("dbresource://x"), "dbresource");
    // one letter is a drive, a slash before the colon is a path
    assert_eq!(scheme_of("c:/tmp/a.txt"), "file");
    assert_eq!(scheme_of("runtime/a:b"), "file");
    assert_eq!(scheme_of("plain.txt"), "file");
    assert_eq!(scheme_of("é:x"), "é");

    assert_eq!(
        split_version("dbresource://a.txt#v2"),
        Some(("dbresource://a.txt", "v2"))
    );
    assert_eq!(split_version("a#b#c"), Some(("a", "b#c")));
    assert_eq!(split_version("a.txt#"), None);
    assert_eq!(split_version("#v2"), None);
    assert_eq!(split_version("a.txt"), None);

    assert_eq!(strip_hash("a.txt#v2"), "a.txt");
    assert_eq!(strip_hash("a.txt#"), "a.txt");
    assert_eq!(strip_hash("#v2"), "#v2");
    assert_eq!(strip_hash("é#ü"), "é");
}

/// A write, delete or move at a location with a `#` would change the file before the `#` (a reference cuts the
/// location there), so it is refused, and the file there is left as it was; on `dbresource://` the `#` is part of the
/// name and nothing is cut (SEC-PFS-82, SEC-BAP-82).
#[test]
fn a_write_at_a_location_with_a_hash_is_refused() {
    let root = format!("target/file-resource-hash-{}", std::process::id());
    let resources = empty_facade();
    let file = format!("file:{root}/screen/App.xml");
    resources.write_bytes(&file, b"<screen/>").unwrap();
    let cut = format!("{file}#");
    let refused = |result: Result<(), Error>| {
        let err = result.unwrap_err();
        assert!(err.to_string().contains("'#'"), "{err}");
    };
    refused(resources.write_bytes(&cut, b"replaced"));
    refused(resources.write_bytes(&format!("{file}#/release-notes.md"), b"replaced"));
    refused(resources.write_from(&cut, &mut std::io::Cursor::new(b"replaced".to_vec()), Some(8)));
    refused(resources.write_bytes_noted(&cut, b"replaced", Some("a note")));
    refused(resources.write_many(&[(cut.clone(), b"replaced".to_vec())]));
    refused(resources.make_directory(&cut));
    refused(resources.delete(&cut).map(|_| ()));
    refused(resources.rename(&cut, &format!("file:{root}/moved.xml")));
    refused(resources.rename(&file, &format!("file:{root}/moved.xml#")));
    refused(resources.restore_version(&cut, "1", None).map(|_| ()));
    refused(resources.write_bytes("runtime://conf/MoquiDevConf.xml#", b"replaced"));
    refused(resources.write_bytes("component://mr-boss/screen/Commons.xml#", b"replaced"));
    refused(resources.write_bytes("git://repo/ci.xml#", b"replaced"));
    refused(
        resources
            .apply_change_set(
                &format!("file:{root}/screen#"),
                &[ResourceChange::Write {
                    path: "a.txt".into(),
                    bytes: b"x".to_vec(),
                }],
                None,
            )
            .map(|_| ()),
    );
    let outcome = resources
        .apply_change_set(
            &format!("file:{root}/screen"),
            &[ResourceChange::Write {
                path: "App.xml#".into(),
                bytes: b"replaced".to_vec(),
            }],
            None,
        )
        .unwrap();
    assert!(outcome.applied.is_empty(), "{outcome:?}");
    assert_eq!(outcome.failures.len(), 1, "{outcome:?}");
    assert_eq!(resources.get(&file).unwrap().read_text().unwrap(), "<screen/>");
    // the database tree keeps a '#' in the name, so it is not refused there
    assert!(super::refuse_cut_write("dbresource://mantle/content/order/1/Invoice #1.pdf").is_ok());
    assert!(super::refuse_cut_write(&file).is_ok());
    std::fs::remove_dir_all(root).unwrap();
}
