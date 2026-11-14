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

use gix::objs::tree::EntryKind;

use super::super::ops::{apply_edits, commit, read_commit, Author, Edit};
use super::super::store::MemoryStore;
use super::*;

const LIMITS: Limits = Limits {
    max_bytes: 1024 * 1024,
    max_lines: 1000,
    max_commits: 100,
    max_diffs: 100,
    max_time: std::time::Duration::from_secs(30),
};

/// A commit of `tree` with these parents, at `seconds`.
fn commit_of(store: &MemoryStore, tree: ObjectId, parents: &[ObjectId], seconds: i64) -> ObjectId {
    let signature = gix::actor::Signature {
        name: "Test".into(),
        email: "test@example.com".into(),
        time: gix::date::Time::new(seconds, 0),
    };
    let commit = gix::objs::Commit {
        tree,
        parents: parents.iter().copied().collect(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: "merge".into(),
        extra_headers: Vec::new(),
    };
    let mut buf = Vec::new();
    gix::objs::WriteTo::write_to(&commit, &mut buf).unwrap();
    store.write(Kind::Commit, &buf).unwrap()
}

/// The tree of `base` with `data` at `at`.
fn tree_with(store: &MemoryStore, base: &ObjectId, at: &str, data: &[u8]) -> ObjectId {
    let blob = store.write(Kind::Blob, data).unwrap();
    apply_edits(
        store,
        Some(&read_commit(store, base).unwrap().tree),
        &[Edit::Put(path(at), EntryKind::Blob, blob)],
    )
    .unwrap()
}

fn path(text: &str) -> Vec<String> {
    text.split('/').map(str::to_string).collect()
}

/// A commit on `branch` that writes `data` at `at`, at `seconds`.
fn put(store: &MemoryStore, branch: &str, at: &str, data: &[u8], seconds: i64) -> ObjectId {
    let author = Author {
        name: "Test".into(),
        email: "test@example.com".into(),
        user_id: None,
        seconds: Some(seconds),
    };
    let blob = store.write(Kind::Blob, data).unwrap();
    commit(store, branch, &author, |_| {
        Ok(Some((
            vec![Edit::Put(path(at), EntryKind::Blob, blob)],
            format!("write {at}"),
        )))
    })
    .unwrap()
    .expect("a commit")
}

fn hunks(store: &MemoryStore, commit: &ObjectId, at: &str) -> Vec<(usize, usize, ObjectId)> {
    blame(store, commit, &path(at), LIMITS)
        .unwrap()
        .expect("a file")
        .hunks
        .into_iter()
        .map(|hunk| (hunk.start, hunk.lines, hunk.commit))
        .collect()
}

#[test]
fn each_line_is_the_commit_that_last_changed_it() {
    let store = MemoryStore::default();
    let first = put(&store, "main", "src/a.txt", b"one\ntwo\nthree\n", 100);
    let second = put(&store, "main", "src/a.txt", b"one\nTWO\nthree\n", 200);
    // a commit that does not touch the file takes none of its lines
    put(&store, "main", "other.txt", b"other\n", 300);
    let fourth = put(&store, "main", "src/a.txt", b"zero\none\nTWO\nthree\nfour\n", 400);
    assert_eq!(
        hunks(&store, &fourth, "src/a.txt"),
        [
            (1, 1, fourth),
            (2, 1, first),
            (3, 1, second),
            (4, 1, first),
            (5, 1, fourth)
        ]
    );
    // at an older commit the later ones are not there
    assert_eq!(
        hunks(&store, &second, "src/a.txt"),
        [(1, 1, first), (2, 1, second), (3, 1, first)]
    );
    let blamed = blame(&store, &fourth, &path("src/a.txt"), LIMITS).unwrap().unwrap();
    assert_eq!(blamed.text, "zero\none\nTWO\nthree\nfour\n");
    // not a file there
    assert!(blame(&store, &fourth, &path("src"), LIMITS).unwrap().is_none());
    assert!(blame(&store, &fourth, &path("nosuch.txt"), LIMITS).unwrap().is_none());
}

#[test]
fn a_merge_passes_each_side_its_lines() {
    let store = MemoryStore::default();
    let base = put(&store, "main", "a.txt", b"one\ntwo\nthree\n", 100);
    store.compare_and_set_ref("refs/heads/side", None, base).unwrap();
    let ours = put(&store, "main", "a.txt", b"ONE\ntwo\nthree\n", 200);
    let theirs = put(&store, "side", "a.txt", b"one\ntwo\nTHREE\n", 300);
    // the merge of the two, with a line of its own
    let tree = tree_with(&store, &ours, "a.txt", b"ONE\ntwo\nTHREE\nmerged\n");
    let merge = commit_of(&store, tree, &[ours, theirs], 400);
    assert_eq!(
        hunks(&store, &merge, "a.txt"),
        [(1, 1, ours), (2, 1, base), (3, 1, theirs), (4, 1, merge)]
    );
}

#[test]
fn a_blame_past_its_limits_is_refused() {
    let store = MemoryStore::default();
    put(&store, "main", "a.txt", b"one\n", 100);
    put(&store, "main", "b.txt", b"b\n", 200);
    let head = put(&store, "main", "c.txt", b"c\n", 300);
    // a.txt was written two commits back: three commits are read
    let short = Limits {
        max_commits: 2,
        ..LIMITS
    };
    let refused = blame(&store, &head, &path("a.txt"), short).unwrap_err();
    assert!(
        refused.message().contains("longer than 2 commits"),
        "{}",
        refused.message()
    );
    assert!(blame(&store, &head, &path("c.txt"), short).is_ok(), "one commit read");
    let small = Limits { max_bytes: 3, ..LIMITS };
    let refused = blame(&store, &head, &path("a.txt"), small).unwrap_err();
    assert!(refused.message().contains("too large"), "{}", refused.message());
    let binary = put(&store, "main", "bin.dat", b"\0\x01\x02", 400);
    let refused = blame(&store, &binary, &path("bin.dat"), LIMITS).unwrap_err();
    assert!(refused.message().contains("binary"), "{}", refused.message());
}

#[test]
fn an_octopus_merge_gives_each_parent_its_lines_and_counts_each_comparison() {
    let store = MemoryStore::default();
    let base = put(&store, "main", "a.txt", b"one\ntwo\nthree\nfour\n", 100);
    let mut sides = Vec::new();
    for (at, (branch, text)) in [
        ("one", "ONE\ntwo\nthree\nfour\n"),
        ("two", "one\nTWO\nthree\nfour\n"),
        ("three", "one\ntwo\nTHREE\nfour\n"),
    ]
    .into_iter()
    .enumerate()
    {
        store
            .compare_and_set_ref(&format!("refs/heads/{branch}"), None, base)
            .unwrap();
        sides.push(put(&store, branch, "a.txt", text.as_bytes(), 200 + at as i64));
    }
    let tree = tree_with(&store, &sides[0], "a.txt", b"ONE\nTWO\nTHREE\nfour\n");
    let merge = commit_of(&store, tree, &sides, 400);
    assert_eq!(
        hunks(&store, &merge, "a.txt"),
        [(1, 1, sides[0]), (2, 1, sides[1]), (3, 1, sides[2]), (4, 1, base)]
    );
    // the merge is compared with its three parents, and each of them with the base: six comparisons
    let exact = Limits { max_diffs: 6, ..LIMITS };
    assert!(blame(&store, &merge, &path("a.txt"), exact).is_ok());
    let fewer = Limits { max_diffs: 5, ..LIMITS };
    let refused = blame(&store, &merge, &path("a.txt"), fewer).unwrap_err();
    assert!(
        refused.message().contains("more than 5 versions"),
        "{}",
        refused.message()
    );
}

#[test]
fn a_blame_is_refused_past_its_lines_and_its_time() {
    let store = MemoryStore::default();
    put(&store, "main", "a.txt", b"one\ntwo\nthree\n", 100);
    let head = put(&store, "main", "a.txt", b"one\ntwo\nthree\nfour", 200);
    let short = Limits { max_lines: 3, ..LIMITS };
    let refused = blame(&store, &head, &path("a.txt"), short).unwrap_err();
    assert!(refused.message().contains("more than 3 lines"), "{}", refused.message());
    assert!(blame(&store, &head, &path("a.txt"), Limits { max_lines: 4, ..LIMITS }).is_ok());
    assert_eq!(
        (line_count(""), line_count("a"), line_count("a\n"), line_count("a\nb")),
        (0, 1, 1, 2)
    );
    let no_time = Limits {
        max_time: std::time::Duration::ZERO,
        ..LIMITS
    };
    let refused = blame(&store, &head, &path("a.txt"), no_time).unwrap_err();
    assert!(
        refused.message().contains("longer than 0 seconds"),
        "{}",
        refused.message()
    );
}
