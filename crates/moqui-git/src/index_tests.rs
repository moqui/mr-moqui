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
use gix::objs::Kind;

use super::super::ops::{commit, Author, Edit};
use super::super::store::MemoryStore;
use super::*;

fn path(text: &str) -> Vec<String> {
    text.split('/').map(str::to_string).collect()
}

fn author(user: &str) -> Author {
    Author {
        name: user.to_string(),
        email: format!("{user}@example.com"),
        user_id: Some(user.to_string()),
        seconds: None,
    }
}

/// One commit of `edits`: `(path, Some(text))` writes, `(path, None)` removes, by `user` with `message`.
fn change(store: &MemoryStore, user: &str, message: &str, edits: &[(&str, Option<&str>)]) -> ObjectId {
    let edits: Vec<Edit> = edits
        .iter()
        .map(|(at, text)| match text {
            Some(text) => Edit::Put(
                path(at),
                EntryKind::Blob,
                store.write(Kind::Blob, text.as_bytes()).unwrap(),
            ),
            None => Edit::Remove(path(at)),
        })
        .collect();
    commit(store, "main", &author(user), |_| {
        Ok(Some((edits.clone(), message.to_string())))
    })
    .unwrap()
    .unwrap()
}

/// A move: the entry at `from` put at `to` and `from` removed, in one commit.
fn rename(store: &MemoryStore, user: &str, from: &str, to: &str) -> ObjectId {
    let head = ops::head(store, "main").unwrap().unwrap();
    let (mode, id) = ops::resolve(store, &head.tree, &path(from)).unwrap().unwrap();
    commit(store, "main", &author(user), |_| {
        Ok(Some((
            vec![Edit::Remove(path(from)), Edit::Put(path(to), mode.kind(), id)],
            format!("Move {from} to {to}"),
        )))
    })
    .unwrap()
    .unwrap()
}

#[test]
fn last_changes_are_per_path_with_their_commit_user_and_note() {
    let store = MemoryStore::default();
    let first = change(
        &store,
        "alice",
        "Add notes",
        &[("notes/a.md", Some("a1")), ("top.txt", Some("t"))],
    );
    let second = change(&store, "bob", "Bob's b", &[("notes/b.md", Some("b1"))]);
    let head = ops::head(&store, "main").unwrap().unwrap();
    let index = BranchIndex::build(&store, &head.id).unwrap();
    let last = |at: &str| index.last_change(at).map(|commit| commit.info.id);
    assert_eq!(last("notes/a.md"), Some(first));
    assert_eq!(last("top.txt"), Some(first));
    assert_eq!(last("notes/b.md"), Some(second));
    assert_eq!(
        last("notes"),
        Some(second),
        "a folder's last change is the newest under it"
    );
    assert_eq!(last(""), Some(second));
    assert_eq!(last("missing.txt"), None);
    let version = index.last_change("notes/b.md").unwrap().version(None);
    assert_eq!(version.user_id.as_deref(), Some("bob"));
    assert_eq!(version.note.as_deref(), Some("Bob's b"));
}

#[test]
fn an_index_advances_in_place_and_rebuilds_when_the_branch_is_replaced() {
    let store = MemoryStore::default();
    change(&store, "alice", "one", &[("a.txt", Some("1"))]);
    let head = ops::head(&store, "main").unwrap().unwrap();
    let mut index = BranchIndex::build(&store, &head.id).unwrap();
    let before = index.bytes;
    let next = change(&store, "bob", "two", &[("a.txt", Some("2"))]);
    index.advance(&store, &next).unwrap();
    assert_eq!(index.head, next);
    assert_eq!(index.commits.len(), 2);
    assert!(index.bytes > before);
    assert_eq!(index.last_change("a.txt").unwrap().info.id, next);
    // a head whose line does not hold the old head (a replaced branch) is indexed anew
    let other = MemoryStore::default();
    let replaced = change(&other, "carol", "elsewhere", &[("z.txt", Some("z"))]);
    let tree = ops::read_commit(&other, &replaced).unwrap().tree;
    let blob = ops::resolve(&other, &tree, &path("z.txt")).unwrap().unwrap().1;
    store
        .write(Kind::Blob, &ops::object(&other, &blob, Kind::Blob).unwrap())
        .unwrap();
    store
        .write(Kind::Tree, &ops::object(&other, &tree, Kind::Tree).unwrap())
        .unwrap();
    store
        .write(Kind::Commit, &ops::object(&other, &replaced, Kind::Commit).unwrap())
        .unwrap();
    index.advance(&store, &replaced).unwrap();
    assert_eq!(index.commits.len(), 1);
    assert!(index.last_change("a.txt").is_none());
    assert_eq!(index.last_change("z.txt").unwrap().info.id, replaced);
}

#[test]
fn history_follows_an_exact_move_and_a_directory_move() {
    let store = MemoryStore::default();
    let v1 = change(&store, "alice", "v1", &[("docs/plan.md", Some("one\n"))]);
    let v2 = change(&store, "bob", "v2", &[("docs/plan.md", Some("two\n"))]);
    let moved = rename(&store, "alice", "docs/plan.md", "archive/plan-2026.md");
    let v3 = change(&store, "carol", "v3", &[("archive/plan-2026.md", Some("three\n"))]);
    let folder = rename(&store, "bob", "archive", "old");
    let head = ops::head(&store, "main").unwrap().unwrap();
    let index = BranchIndex::build(&store, &head.id).unwrap();
    let versions = index.versions("old/plan-2026.md");
    let names: Vec<String> = versions
        .iter()
        .map(|(version, _)| version.version_name.clone())
        .collect();
    assert_eq!(
        names,
        [folder, v3, moved, v2, v1].map(|id| id.to_string()),
        "the moves are versions, and the history goes on at the old paths"
    );
    assert_eq!(versions[4].0.previous_version_name, None);
    assert_eq!(
        versions[0].0.previous_version_name.as_deref(),
        Some(v3.to_string().as_str())
    );
    let oldest = ops::object(&store, &versions[4].1.blob, Kind::Blob).unwrap();
    assert_eq!(oldest, b"one\n");
    // a file removed and added again keeps its older versions; the removal is not one
    change(&store, "alice", "gone", &[("x.txt", Some("x1"))]);
    change(&store, "alice", "rm", &[("x.txt", None)]);
    change(&store, "alice", "back", &[("x.txt", Some("x2"))]);
    let head = ops::head(&store, "main").unwrap().unwrap();
    let index = BranchIndex::build(&store, &head.id).unwrap();
    let notes: Vec<Option<String>> = index
        .versions("x.txt")
        .into_iter()
        .map(|(version, _)| version.note)
        .collect();
    assert_eq!(notes, [Some("back".to_string()), Some("gone".to_string())]);
}

#[test]
fn the_log_under_a_folder_lists_the_commits_and_paths_there() {
    let store = MemoryStore::default();
    change(
        &store,
        "alice",
        "a",
        &[("team/a.md", Some("a")), ("other.txt", Some("o"))],
    );
    change(&store, "bob", "b", &[("other.txt", Some("o2"))]);
    change(
        &store,
        "carol",
        "c",
        &[("team/sub/c.md", Some("c")), ("team/a.md", Some("a2"))],
    );
    let head = ops::head(&store, "main").unwrap().unwrap();
    let index = BranchIndex::build(&store, &head.id).unwrap();
    let summary: Vec<(Option<String>, Vec<String>)> = index
        .log("team", 10)
        .into_iter()
        .map(|(version, paths)| (version.note, paths))
        .collect();
    assert_eq!(
        summary,
        [
            (Some("c".to_string()), vec!["a.md".to_string(), "sub/c.md".to_string()]),
            (Some("a".to_string()), vec!["a.md".to_string()]),
        ]
    );
    assert_eq!(index.log("", 1).len(), 1, "the limit applies");
    assert_eq!(
        index.log("team/a.md", 10)[0].1,
        [String::new()],
        "a file's own change has the empty path"
    );
}
