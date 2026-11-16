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

//! What a viewer's read costs (SEC-GIT-04): `get#GitBlob` reads no file over `blob-max-bytes`, a commit's diff diffs
//! at most `commit-max-diff-files` (300) files, and `get#GitLog` with a path compares at most `log-max-commits` commits.

use moqui_lib::object::Value;

use crate::server::hosting_tests::{entry, get, grant_entities, list, ok, s, text_of};
use crate::server::tests::{define, fixture_with, READER, WRITER};

const GIT: &str = "org.moqui.git.GitServices";

fn git(name: &str) -> String {
    format!("{GIT}.{name}")
}

#[test]
fn a_viewers_reads_are_held_to_their_limits() {
    let fixture = fixture_with(
        "read-limits",
        moqui_core::context::framework_dir(),
        r#"<git-server blob-max-bytes="64" log-max-commits="3" commit-max-diff-files="2"/>"#,
    );
    grant_entities(&fixture);
    define(&fixture, "site", "db");
    let mut files: Vec<(String, Value)> = (0..3).map(|n| (format!("f/{n}.txt"), s(&format!("{n}\n")))).collect();
    files.push(("big.txt".into(), s(&"x".repeat(100))));
    let first = ok(
        &fixture,
        WRITER,
        &git("commit#GitFiles"),
        &[
            ("repositoryId", s("site")),
            ("message", s("Many files")),
            ("fileMap", Value::Map(files.into_iter().collect())),
        ],
    );
    let first = get(&first, "commitId").to_string();
    for n in 0..4 {
        ok(
            &fixture,
            WRITER,
            &git("commit#GitFiles"),
            &[
                ("repositoryId", s("site")),
                ("message", s(&format!("Later {n}"))),
                (
                    "fileMap",
                    Value::Map([("later.txt".to_string(), s(&format!("{n}\n")))].into_iter().collect()),
                ),
            ],
        );
    }

    // a file over blob-max-bytes: its size, and that it is too large, but not its content
    let big = ok(
        &fixture,
        READER,
        &git("get#GitBlob"),
        &[("repositoryId", s("site")), ("path", s("big.txt"))],
    );
    assert_eq!(big.get("tooLarge"), Some(&Value::Bool(true)), "{big:?}");
    assert!(!big.contains_key("text") && !big.contains_key("bytes"), "{big:?}");
    assert_eq!(big.get("size").map(text_of).as_deref(), Some("100"), "{big:?}");
    let small = ok(
        &fixture,
        READER,
        &git("get#GitBlob"),
        &[("repositoryId", s("site")), ("path", s("f/2.txt"))],
    );
    assert_eq!(get(&small, "text"), "2\n");

    // the 4 files of the commit are listed, and the first 2 (commit-max-diff-files) diffed
    let diff = ok(
        &fixture,
        READER,
        &git("get#GitCommitDiff"),
        &[("repositoryId", s("site")), ("commitId", s(&first))],
    );
    let files = list(&diff, "fileDiffList");
    assert_eq!(files.len(), 4);
    let diffed = files
        .iter()
        .filter(|file| !matches!(entry(file, "unifiedDiff"), Value::Null))
        .count();
    assert_eq!(diffed, 2);

    // big.txt changed in the first commit only: four later commits are past the log's budget of three
    let log = ok(
        &fixture,
        READER,
        &git("get#GitLog"),
        &[("repositoryId", s("site")), ("path", s("big.txt"))],
    );
    assert!(list(&log, "commitList").is_empty(), "{log:?}");
    assert_eq!(log.get("hasMore"), Some(&Value::Bool(false)));
    let log = ok(
        &fixture,
        READER,
        &git("get#GitLog"),
        &[("repositoryId", s("site")), ("path", s("later.txt"))],
    );
    assert_eq!(list(&log, "commitList").len(), 3);
    assert_eq!(log.get("hasMore"), Some(&Value::Bool(true)));
}
