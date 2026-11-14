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

//! The runner's own pieces, with no server: cache keys, path patterns and archives. A whole job is run against the
//! runner REST in `server/ci_tests.rs`.

use std::path::PathBuf;

use super::{archive, expand_key, matches};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-ci-runner-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &std::path::Path, name: &str, text: &str) {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn a_cache_key_takes_the_hash_of_a_file_of_the_checkout() {
    let dir = scratch("key");
    write(&dir, "Cargo.lock", "one");
    let first = expand_key("cargo-${hash:Cargo.lock}-x", &dir);
    assert!(
        first.starts_with("cargo-") && first.ends_with("-x") && first.len() == "cargo--x".len() + 16,
        "{first}"
    );
    assert_eq!(first, expand_key("cargo-${hash:Cargo.lock}-x", &dir));
    write(&dir, "Cargo.lock", "two");
    assert_ne!(first, expand_key("cargo-${hash:Cargo.lock}-x", &dir));
    // a file that is not there, or outside the checkout, hashes to nothing
    assert_eq!(expand_key("k-${hash:missing}", &dir), "k-");
    assert_eq!(expand_key("k-${hash:../../etc/passwd}", &dir), "k-");
    assert_eq!(expand_key("plain", &dir), "plain");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn patterns_name_files_inside_the_workspace_only() {
    let dir = scratch("glob");
    write(&dir, "test-results/20260101/TEST-a.xml", "a");
    write(&dir, "test-results/20260101/TEST-b.xml", "b");
    write(&dir, "test-results/20260101/notes.txt", "n");
    write(&dir, "dist/app.zip", "z");
    let names = |pattern: &str| -> Vec<String> {
        matches(&dir, pattern)
            .iter()
            .map(|path| path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/"))
            .collect()
    };
    assert_eq!(
        names("test-results/*/*.xml"),
        ["test-results/20260101/TEST-a.xml", "test-results/20260101/TEST-b.xml"]
    );
    assert_eq!(names("dist/*.zip"), ["dist/app.zip"]);
    assert_eq!(names("dist"), ["dist"]);
    assert_eq!(names("dist/app.zip"), ["dist/app.zip"]);
    assert!(names("missing/*.xml").is_empty());
    assert!(names("../*").is_empty());
    assert!(names("/etc/*").is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_archive_round_trips_by_workspace_paths() {
    let from = scratch("pack-from");
    let to = scratch("pack-to");
    write(&from, "target/debug/app", "binary");
    write(&from, "target/cache/x.bin", "x");
    write(&from, "notes.txt", "notes");
    let archive_file = from
        .parent()
        .unwrap()
        .join(format!("moqui-ci-runner-{}.tar.gz", std::process::id()));
    archive::pack(&from, &[from.join("target"), from.join("notes.txt")], &archive_file).unwrap();
    assert!(archive::unpack(&archive_file, &to).unwrap() >= 3);
    assert_eq!(std::fs::read_to_string(to.join("target/debug/app")).unwrap(), "binary");
    assert_eq!(std::fs::read_to_string(to.join("target/cache/x.bin")).unwrap(), "x");
    assert_eq!(std::fs::read_to_string(to.join("notes.txt")).unwrap(), "notes");
    // a file outside the workspace is not packed
    assert!(archive::pack(&from, &[to.join("notes.txt")], &archive_file).is_err());
    for path in [&from, &to] {
        std::fs::remove_dir_all(path).unwrap();
    }
    std::fs::remove_file(&archive_file).unwrap();
}
