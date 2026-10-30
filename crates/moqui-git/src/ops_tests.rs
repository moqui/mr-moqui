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

use super::super::store::MemoryStore;
use super::*;

fn path(text: &str) -> Vec<String> {
    text.split('/').map(str::to_string).collect()
}

fn author() -> Author {
    Author {
        name: "Test User".to_string(),
        email: "test@example.com".to_string(),
        user_id: Some("EX_JOHN_DOE".to_string()),
        seconds: None,
    }
}

fn put(store: &dyn GitStore, at: &str, text: &str) -> Option<ObjectId> {
    let blob = store.write(Kind::Blob, text.as_bytes()).unwrap();
    let at = path(at);
    commit(store, "main", &author(), |_| {
        Ok(Some((
            vec![Edit::Put(at.clone(), EntryKind::Blob, blob)],
            "put".to_string(),
        )))
    })
    .unwrap()
}

#[test]
fn trees_commits_and_history_over_a_store() {
    let store = MemoryStore::default();
    let first = put(&store, "notes/a.md", "one\n").expect("first commit");
    put(&store, "notes/b.md", "other\n").expect("second commit");
    let third = put(&store, "notes/a.md", "two\n").expect("third commit");
    assert_eq!(
        put(&store, "notes/a.md", "two\n"),
        None,
        "an unchanged tree makes no commit"
    );
    let head = head(&store, "main").unwrap().unwrap();
    assert_eq!(head.id, third);
    assert_eq!(head.parents.len(), 1);
    assert_eq!(user_of(&head.message).as_deref(), Some("EX_JOHN_DOE"));
    let names: Vec<String> = list(&store, &head.tree)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["notes"]);
    let notes = resolve(&store, &head.tree, &path("notes")).unwrap().unwrap();
    assert!(notes.0.is_tree());
    assert_eq!(list(&store, &notes.1).unwrap().len(), 2);
    let versions: Vec<moqui_core::context::ResourceVersion> = super::super::index::BranchIndex::build(&store, &head.id)
        .unwrap()
        .versions("notes/a.md")
        .into_iter()
        .map(|(version, _)| version)
        .collect();
    let names: Vec<&str> = versions.iter().map(|version| version.version_name.as_str()).collect();
    assert_eq!(names, [third.to_string(), first.to_string()]);
    assert_eq!(
        versions[0].previous_version_name.as_deref(),
        Some(first.to_string().as_str())
    );
    assert_eq!(versions[0].user_id.as_deref(), Some("EX_JOHN_DOE"));
    assert_eq!(versions[0].note.as_deref(), Some("put"));
    assert_eq!(
        read_at(&store, &first, &path("notes/a.md")).unwrap(),
        Some(b"one\n".to_vec())
    );
    assert_eq!(read_at(&store, &first, &path("notes/b.md")).unwrap(), None);
}

#[test]
fn removing_the_last_file_removes_its_directories() {
    let store = MemoryStore::default();
    put(&store, "a/b/c.txt", "x").unwrap();
    put(&store, "top.txt", "y").unwrap();
    commit(&store, "main", &author(), |_| {
        Ok(Some((vec![Edit::Remove(path("a/b/c.txt"))], "rm".to_string())))
    })
    .unwrap()
    .unwrap();
    let head = head(&store, "main").unwrap().unwrap();
    let names: Vec<String> = list(&store, &head.tree)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["top.txt"]);
    let missing = commit(&store, "main", &author(), |_| {
        Ok(Some((vec![Edit::Remove(path("no/such.txt"))], "rm".to_string())))
    })
    .unwrap();
    assert_eq!(missing, None);
    let blob = store.write(Kind::Blob, b"z").unwrap();
    let err = commit(&store, "main", &author(), |_| {
        Ok(Some((
            vec![Edit::Put(path("top.txt/below.txt"), EntryKind::Blob, blob)],
            "x".to_string(),
        )))
    })
    .unwrap_err();
    assert!(err.to_string().contains("found file instead"), "{err}");
}

#[test]
fn a_moved_branch_is_retried_on_the_new_head() {
    let store = MemoryStore::default();
    put(&store, "a.txt", "1").unwrap();
    let blob = store.write(Kind::Blob, b"2").unwrap();
    let mut calls = 0;
    let made = commit(&store, "main", &author(), |_| {
        calls += 1;
        if calls == 1 {
            // another writer commits between this one's read of the head and its ref update
            put(&store, "b.txt", "other writer").unwrap();
        }
        Ok(Some((
            vec![Edit::Put(path("c.txt"), EntryKind::Blob, blob)],
            "c".to_string(),
        )))
    })
    .unwrap()
    .unwrap();
    assert_eq!(calls, 2);
    let head = head(&store, "main").unwrap().unwrap();
    assert_eq!(head.id, made);
    let names: Vec<String> = list(&store, &head.tree)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
}

#[test]
fn trailers_are_the_last_paragraph_when_every_line_is_one() {
    let found = trailers("Fix login\n\nLonger text.\n\nTask: PROJ-100023\nSigned-off-by: A <a@b.c>\n  continued\n");
    assert_eq!(
        found,
        [
            ("Task".to_string(), "PROJ-100023".to_string()),
            ("Signed-off-by".to_string(), "A <a@b.c> continued".to_string())
        ]
    );
    assert!(trailers("Task: only a subject\n").is_empty());
    assert!(trailers("Subject\n\nTask: X\nnot a trailer\n").is_empty());
    assert!(trailers("Subject\n\nSee: http://x\n").len() == 1);
}

#[test]
fn the_user_trailer_joins_a_trailer_block_the_message_ends_in() {
    let store = MemoryStore::default();
    let blob = store.write(Kind::Blob, b"x").unwrap();
    let id = commit(&store, "main", &author(), |_| {
        Ok(Some((
            vec![Edit::Put(path("a.txt"), EntryKind::Blob, blob)],
            "Subject\n\nTask: PROJ-1".to_string(),
        )))
    })
    .unwrap()
    .unwrap();
    let message = read_commit(&store, &id).unwrap().message;
    assert_eq!(message, "Subject\n\nTask: PROJ-1\nMoqui-User-Id: EX_JOHN_DOE\n");
    assert_eq!(trailers(&message).len(), 2);
}

#[test]
fn git_entry_order_puts_a_directory_after_a_dotted_file() {
    let store = MemoryStore::default();
    put(&store, "a.b", "file").unwrap();
    put(&store, "a/x", "in dir").unwrap();
    put(&store, "a-c", "file").unwrap();
    let head = head(&store, "main").unwrap().unwrap();
    let names: Vec<String> = list(&store, &head.tree)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    // git compares a directory name as if it ended in '/', so "a" sorts after "a-c" and "a.b"
    assert_eq!(names, ["a-c", "a.b", "a"]);
}
