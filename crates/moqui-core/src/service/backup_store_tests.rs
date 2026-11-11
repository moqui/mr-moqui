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

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use moqui_lib::age::{decrypt, Identity};

use super::*;

/// Objects in memory, and the calls made.
#[derive(Default)]
struct Memory {
    objects: RefCell<BTreeMap<String, Vec<u8>>>,
    calls: RefCell<Vec<String>>,
}

impl ObjectStore for Memory {
    fn list(&self, prefix: &str) -> Result<Vec<S3Object>, LibError> {
        Ok(self
            .objects
            .borrow()
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .map(|(key, data)| S3Object {
                key: key.clone(),
                size: data.len() as u64,
            })
            .collect())
    }

    fn put_with(&self, key: &str, fill: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>) -> Result<u64, LibError> {
        let mut data = Vec::new();
        fill(&mut data).map_err(|err| LibError::new(err.to_string()))?;
        self.calls.borrow_mut().push(format!("put {key}"));
        let len = data.len() as u64;
        self.objects.borrow_mut().insert(key.to_string(), data);
        Ok(len)
    }

    fn rename(&self, from: &str, to: &str, _size: u64) -> Result<(), LibError> {
        self.calls.borrow_mut().push(format!("rename {from} {to}"));
        let data = self.objects.borrow_mut().remove(from).unwrap();
        self.objects.borrow_mut().insert(to.to_string(), data);
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), LibError> {
        self.calls.borrow_mut().push(format!("delete {key}"));
        self.objects.borrow_mut().remove(key);
        Ok(())
    }
}

/// A dump, a full and an incremental in a new directory.
fn archives(what: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("backup-store-{what}-{}", std::process::id()));
    std::fs::create_dir_all(base.join(FULL).join("base")).unwrap();
    std::fs::write(base.join(FULL).join("base/1"), b"block data").unwrap();
    std::fs::write(base.join(FULL).join("backup_manifest"), b"{}").unwrap();
    std::fs::create_dir_all(base.join(INCR)).unwrap();
    std::fs::write(base.join(INCR).join("INCREMENTAL.1"), b"changed").unwrap();
    std::fs::write(base.join(DUMP), b"pg_dump custom").unwrap();
    base
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn outcome(keep: &[(&str, PathBuf)], renames: &[(&str, &str)]) -> BackupOutcome {
    BackupOutcome {
        skipped: false,
        backup_dir: PathBuf::new(),
        archive_path: PathBuf::new(),
        backup_kind: "full".into(),
        bytes_written: 0,
        promoted: Vec::new(),
        removed: Vec::new(),
        keep: keep
            .iter()
            .map(|(name, path)| KeptArchive {
                name: (*name).to_string(),
                path: path.clone(),
            })
            .collect(),
        renames: renames
            .iter()
            .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
            .collect(),
        errors: Vec::new(),
    }
}

fn untar(data: &[u8], into: &Path) {
    tar::Archive::new(flate2::read::GzDecoder::new(data))
        .unpack(into)
        .unwrap();
}

const DUMP: &str = "moqui-20261101T020000Z-d.dump";
const FULL: &str = "moqui-20261102T020000Z-d";
const FULL_WEEKLY: &str = "moqui-20261102T020000Z-w";
const INCR: &str = "moqui-20261103T020000Z-d";

#[test]
fn archives_go_up_once_encrypted_and_follow_the_ladder_on_the_store() {
    let base = archives("sealed");
    let _guard = DirGuard(base.clone());
    let identity = Identity::generate().unwrap();
    let recipient = identity.recipient();
    let store = Memory::default();
    store
        .objects
        .borrow_mut()
        .insert("bk/notes.txt".into(), b"left alone".to_vec());
    store
        .objects
        .borrow_mut()
        .insert("bk/moqui-20251101T020000Z-y.dump.age".into(), b"dropped".to_vec());

    let first = outcome(&[(DUMP, base.join(DUMP)), (FULL, base.join(FULL))], &[]);
    assert_eq!(
        mirror(&store, "s3://b/bk", &key_prefix("/bk/"), Some(&recipient), &first).unwrap(),
        "Y"
    );
    let keys: Vec<String> = store.objects.borrow().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            format!("bk/{DUMP}.age"),
            format!("bk/{FULL}.tar.gz.age"),
            "bk/notes.txt".to_string()
        ]
    );
    let dump = decrypt(
        &identity,
        &mut store.objects.borrow()[&format!("bk/{DUMP}.age")].as_slice(),
    )
    .unwrap();
    assert_eq!(dump, b"pg_dump custom");
    let sealed = store.objects.borrow()[&format!("bk/{FULL}.tar.gz.age")].clone();
    assert!(!sealed.windows(10).any(|w| w == b"block data"));
    let out = base.join("restored");
    untar(&decrypt(&identity, &mut sealed.as_slice()).unwrap(), &out);
    assert!(!out.join(FULL).exists());
    let mut text = String::new();
    std::fs::File::open(out.join("base/1"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "block data");

    // the next run promotes the full and adds an incremental: one copy, one upload, the dump dropped
    std::fs::rename(base.join(FULL), base.join(FULL_WEEKLY)).unwrap();
    store.calls.borrow_mut().clear();
    let next = outcome(
        &[(FULL_WEEKLY, base.join(FULL_WEEKLY)), (INCR, base.join(INCR))],
        &[(FULL, FULL_WEEKLY)],
    );
    assert_eq!(
        mirror(&store, "s3://b/bk", &key_prefix("bk"), Some(&recipient), &next).unwrap(),
        "Y"
    );
    assert_eq!(
        *store.calls.borrow(),
        [
            format!("rename bk/{FULL}.tar.gz.age bk/{FULL_WEEKLY}.tar.gz.age"),
            format!("put bk/{INCR}.tar.gz.age"),
            format!("delete bk/{DUMP}.age"),
        ]
    );
    assert!(store.objects.borrow().contains_key("bk/notes.txt"));
    assert_eq!(store.objects.borrow().len(), 3);
}

#[test]
fn without_encryption_archives_go_up_plain_and_sealed_objects_are_left_alone() {
    let base = archives("plain");
    let _guard = DirGuard(base.clone());
    let store = Memory::default();
    store
        .objects
        .borrow_mut()
        .insert(format!("bk/{DUMP}.age"), b"sealed earlier".to_vec());
    let first = outcome(&[(DUMP, base.join(DUMP)), (FULL, base.join(FULL))], &[]);
    assert_eq!(mirror(&store, "s3://b/bk", "bk/", None, &first).unwrap(), "Y");
    let objects = store.objects.borrow();
    assert_eq!(objects[&format!("bk/{DUMP}")], b"pg_dump custom");
    assert!(objects.contains_key(&format!("bk/{DUMP}.age")));
    let out = base.join("restored");
    untar(&objects[&format!("bk/{FULL}.tar.gz")], &out);
    assert_eq!(std::fs::read(out.join("base/1")).unwrap(), b"block data");
    assert_eq!(objects.len(), 3);
}

#[test]
fn over_sftp_encrypted_each_archive_is_one_sealed_file_and_a_promotion_a_rename() {
    let base = archives("sftp");
    let _guard = DirGuard(base.clone());
    let identity = Identity::generate().unwrap();
    let recipient = identity.recipient().to_string();
    let context = crate::context::ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let connector = crate::sftp::tests::memory(context.factory());
    let mut client = SftpClient::new("backup.example.com", "joe", None).unwrap();
    client.password("pw").connect().unwrap();

    let first = outcome(&[(DUMP, base.join(DUMP)), (FULL, base.join(FULL))], &[]);
    assert_eq!(
        mirror_sftp_encrypted(&mut client, "/srv/bk", &recipient, &first).unwrap(),
        "Y"
    );
    std::fs::rename(base.join(FULL), base.join(FULL_WEEKLY)).unwrap();
    let next = outcome(
        &[(FULL_WEEKLY, base.join(FULL_WEEKLY)), (INCR, base.join(INCR))],
        &[(FULL, FULL_WEEKLY)],
    );
    assert_eq!(
        mirror_sftp_encrypted(&mut client, "/srv/bk/", &recipient, &next).unwrap(),
        "Y"
    );
    let disk = connector.disk.lock().unwrap();
    let names: Vec<&String> = disk.files.keys().collect();
    assert_eq!(
        names,
        [
            &format!("/srv/bk/{FULL_WEEKLY}.tar.gz.age"),
            &format!("/srv/bk/{INCR}.tar.gz.age")
        ]
    );
    let plain = decrypt(
        &identity,
        &mut disk.files[&format!("/srv/bk/{INCR}.tar.gz.age")].as_slice(),
    )
    .unwrap();
    let out = base.join("restored");
    untar(&plain, &out);
    assert_eq!(std::fs::read(out.join("INCREMENTAL.1")).unwrap(), b"changed");
    assert!(mirror_sftp_encrypted(&mut client, "/srv/bk", "ssh-ed25519 AAAA", &next).is_err());
}

#[test]
fn keys_map_back_to_archive_names() {
    assert_eq!(archive_of("p/", &format!("p/{DUMP}.age"), true), Some((DUMP, ".age")));
    assert_eq!(
        archive_of("p/", &format!("p/{FULL}.tar.gz.age"), true),
        Some((FULL, ".tar.gz.age"))
    );
    assert_eq!(archive_of("p/", &format!("p/{FULL}"), true), None);
    assert_eq!(archive_of("p/", &format!("p/x/{FULL}.age"), true), None);
    assert_eq!(archive_of("", &format!("{DUMP}.age"), true), Some((DUMP, ".age")));
    assert_eq!(archive_of("p/", &format!("p/{DUMP}"), false), Some((DUMP, "")));
    assert_eq!(
        archive_of("p/", &format!("p/{FULL}.tar.gz"), false),
        Some((FULL, ".tar.gz"))
    );
    assert_eq!(archive_of("p/", &format!("p/{DUMP}.age"), false), None);
    assert_eq!(key_prefix("/a/b/"), "a/b/");
    assert_eq!(key_prefix(""), "");
}
