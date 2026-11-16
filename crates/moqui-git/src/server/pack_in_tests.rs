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

//! What a pushed pack costs beyond its bytes (SEC-GIT-05): a delta chain deeper than the limit is refused, each
//! object's kind is read once, and a push for review counts what its objects inflate to. Skipped without git.

use std::collections::HashSet;
use std::path::Path;

use super::super::hosting_tests::{framework_rules, grant_entities, Work};
use super::super::tests::{define, fixture_with, git_available, git_in, http_server, READER, WRITER};
use super::{receive, PackLimits};
use crate::store::MemoryStore;

/// A repository of one file in twelve versions, each a delta of the next (git keeps the newest whole), and a
/// mebibyte of zeros, repacked: its pack file's bytes.
fn chained_pack(dir: &Path) -> Vec<u8> {
    let home = dir.join("home");
    let work = dir.join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let git = |args: &[&str]| {
        let (ok, out) = git_in(&work, &home, args);
        assert!(ok, "git {args:?}: {out}");
    };
    git(&["init", "-q"]);
    // each version changes one more line, so it is nearer the one before than the newest
    let mut lines: Vec<String> = (0..240)
        .map(|n| format!("line {n} of a file that changes a little at a time\n"))
        .collect();
    std::fs::write(work.join("zeros.bin"), vec![0u8; 1024 * 1024]).unwrap();
    for version in 0..12 {
        for line in &mut lines[version * 20..version * 20 + 20] {
            *line = format!("changed in version {version}: {line}");
        }
        std::fs::write(work.join("f.txt"), lines.concat()).unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", &format!("Version {version}")]);
    }
    git(&["repack", "-a", "-d", "-f", "-q", "--window=50", "--depth=50"]);
    let pack_dir = work.join(".git/objects/pack");
    let pack = std::fs::read_dir(&pack_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "pack"))
        .unwrap();
    std::fs::read(pack).unwrap()
}

#[test]
fn a_deep_delta_chain_is_refused_and_inflated_bytes_are_counted() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let dir = std::env::temp_dir().join(format!("moqui-git-pack-in-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pack = chained_pack(&dir);
    let store = MemoryStore::default();

    let shallow = PackLimits {
        max_object: None,
        max_depth: 3,
    };
    let err = receive(&store, &mut &pack[..], &dir.join("q1"), shallow, Some(1)).unwrap_err();
    assert!(err.message().contains("GitDeltaTooDeep"), "{err}");

    let deep = PackLimits {
        max_object: None,
        max_depth: 50,
    };
    let received = receive(&store, &mut &pack[..], &dir.join("q2"), deep, Some(1)).unwrap();
    received.check_closure(&HashSet::new()).unwrap();
    // the mebibyte of zeros and the twelve versions, each counted at its own size, in a pack of a few KiB
    let inflated = received.inflated_bytes();
    assert!(inflated > 1024 * 1024 + 12 * 8000, "{inflated}");
    assert!(received.pack_bytes() < 64 * 1024, "{}", received.pack_bytes());
    drop(received);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A push for review is counted at what it inflates to: a mebibyte of zeros is a pack of about a KiB, and over a
/// cap of 64 KiB.
#[test]
fn a_push_for_review_counts_its_inflated_bytes() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = fixture_with(
        "review-inflated",
        moqui_core::context::framework_dir(),
        r#"<git-server review-max-push-bytes="65536"/>"#,
    );
    grant_entities(&fixture);
    framework_rules(&fixture);
    define(&fixture, "proj", "db");
    let base = http_server(fixture.context.factory());
    let writer = Work::clone(&fixture, &base, "proj", WRITER);
    writer.commit(&[("a.txt", "one\n")], "First");
    writer.ok(&["push", "origin", "main"]);
    let reader = Work::clone(&fixture, &base, "proj", READER);
    std::fs::write(reader.dir.join("zeros.bin"), vec![0u8; 1024 * 1024]).unwrap();
    reader.commit(&[("b.txt", "two\n")], "Zeros");
    let (pushed, out) = reader.run(&["push", "origin", "HEAD:refs/for/main"]);
    assert!(!pushed && out.contains("GitReviewPushTooLarge"), "{out}");
}
