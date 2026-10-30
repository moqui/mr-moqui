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

use super::super::ops::{self, Edit};
use super::super::store::MemoryStore;
use super::*;

fn author(name: &str) -> Author {
    Author {
        name: name.to_string(),
        email: format!("{name}@example.com"),
        user_id: Some(name.to_uppercase()),
        seconds: None,
    }
}

fn path(text: &str) -> Vec<String> {
    text.split('/').map(str::to_string).collect()
}

/// A commit of `files` on `parent`'s tree, made on branch `branch`.
fn commit_on(store: &MemoryStore, branch: &str, files: &[(&str, &str)], message: &str) -> ObjectId {
    let edits: Vec<Edit> = files
        .iter()
        .map(|(at, text)| {
            Edit::Put(
                path(at),
                EntryKind::Blob,
                store.write(Kind::Blob, text.as_bytes()).unwrap(),
            )
        })
        .collect();
    ops::commit(store, branch, &author("dev"), |_| {
        Ok(Some((edits.clone(), message.to_string())))
    })
    .unwrap()
    .unwrap()
}

fn branch_from(store: &MemoryStore, name: &str, at: ObjectId) {
    assert!(store.compare_and_set_ref(&ops::branch_ref(name), None, at).unwrap());
}

fn file(store: &dyn GitStore, commit: &ObjectId, at: &str) -> Option<String> {
    ops::read_at(store, commit, &path(at))
        .unwrap()
        .map(|bytes| String::from_utf8(bytes).unwrap())
}

const BASE: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";

/// main with a base file; `feature` from it changes the first line, main the last.
fn diverged(store: &MemoryStore) -> (ObjectId, ObjectId, ObjectId) {
    let base = commit_on(store, "main", &[("a.txt", BASE)], "base");
    branch_from(store, "feature", base);
    commit_on(
        store,
        "feature",
        &[("a.txt", &BASE.replace("one", "ONE"))],
        "feature: first line",
    );
    let feature = commit_on(store, "feature", &[("b.txt", "new file\n")], "feature: b");
    let main = commit_on(
        store,
        "main",
        &[("a.txt", &BASE.replace("seven", "SEVEN"))],
        "main: last line",
    );
    (base, feature, main)
}

fn request<'a>(method: Method, source: ObjectId, target: ObjectId, committer: &'a Author) -> MergeRequest<'a> {
    MergeRequest {
        method,
        source,
        target,
        source_name: "feature",
        target_name: "main",
        message: None,
        committer,
        squash_author: None,
    }
}

#[test]
fn the_merge_base_is_the_newest_common_ancestor() {
    let store = MemoryStore::default();
    let (base, feature, main) = diverged(&store);
    assert_eq!(merge_base(&store, &feature, &main).unwrap(), Some(base));
    assert_eq!(merge_base(&store, &base, &main).unwrap(), Some(base));
    let checked = check(&store, &feature, &main).unwrap();
    assert!(
        !checked.merged && !checked.fast_forward && checked.conflicts.is_empty(),
        "{checked:?}"
    );
    assert!(check(&store, &base, &main).unwrap().merged);
    assert!(check(&store, &main, &base).unwrap().fast_forward);
}

#[test]
fn a_merge_commit_has_both_parents_and_both_changes() {
    let store = MemoryStore::default();
    let (_, feature, main) = diverged(&store);
    let me = author("merger");
    let overlay = Overlay::new(&store);
    let merged = merge(&overlay, &request(Method::Merge, feature, main, &me))
        .unwrap()
        .unwrap();
    // nothing is in the store until the overlay is flushed
    assert!(store.find(&merged.head).unwrap().is_none());
    assert!(overlay.flush().unwrap() >= 3);
    let commit = ops::commit_details(&store, &merged.head).unwrap();
    assert_eq!(commit.parents, vec![main, feature]);
    assert!(
        commit.message.starts_with("Merge branch 'feature' into 'main'"),
        "{}",
        commit.message
    );
    assert_eq!(ops::user_of(&commit.message).as_deref(), Some("MERGER"));
    let text = file(&store, &merged.head, "a.txt").unwrap();
    assert!(text.starts_with("ONE\n") && text.ends_with("SEVEN\n"), "{text}");
    assert_eq!(file(&store, &merged.head, "b.txt").as_deref(), Some("new file\n"));
}

#[test]
fn squash_and_rebase_results() {
    let store = MemoryStore::default();
    let (_, feature, main) = diverged(&store);
    let me = author("merger");
    let mr_author = author("writer");

    let overlay = Overlay::new(&store);
    let mut squash = request(Method::Squash, feature, main, &me);
    squash.squash_author = Some(&mr_author);
    squash.message = Some("Feature work (!1)");
    let squashed = merge(&overlay, &squash).unwrap().unwrap();
    overlay.flush().unwrap();
    let commit = ops::commit_details(&store, &squashed.head).unwrap();
    assert_eq!(commit.parents, vec![main], "a squash has the target as its only parent");
    assert_eq!(commit.author.name, "writer");
    assert_eq!(commit.committer.name, "merger");
    assert!(commit.message.starts_with("Feature work (!1)"));
    assert_eq!(squashed.made, vec![squashed.head]);
    let text = file(&store, &squashed.head, "a.txt").unwrap();
    assert!(text.starts_with("ONE\n") && text.ends_with("SEVEN\n"), "{text}");

    let overlay = Overlay::new(&store);
    let rebased = merge(&overlay, &request(Method::Rebase, feature, main, &me))
        .unwrap()
        .unwrap();
    overlay.flush().unwrap();
    assert_eq!(rebased.made.len(), 2, "each commit of the source replayed");
    let first = ops::commit_details(&store, &rebased.made[0]).unwrap();
    assert_eq!(first.parents, vec![main]);
    assert_eq!(first.message.trim(), "feature: first line\n\nMoqui-User-Id: DEV".trim());
    assert_eq!(first.author.name, "dev");
    assert_eq!(first.committer.name, "merger");
    let last = ops::commit_details(&store, &rebased.head).unwrap();
    assert_eq!(last.parents, vec![rebased.made[0]]);
    assert_eq!(file(&store, &rebased.head, "b.txt").as_deref(), Some("new file\n"));
    assert!(file(&store, &rebased.head, "a.txt").unwrap().ends_with("SEVEN\n"));

    // a fast-forward is refused on diverged branches and made when the target is behind
    let overlay = Overlay::new(&store);
    assert_eq!(
        merge(&overlay, &request(Method::FastForward, feature, main, &me)).unwrap(),
        Err(Refused::NotFastForward)
    );
    assert_eq!(
        merge(&overlay, &request(Method::FastForward, rebased.head, main, &me)).unwrap(),
        Ok(Merged {
            head: rebased.head,
            made: Vec::new()
        })
    );
    assert_eq!(
        merge(&overlay, &request(Method::Merge, main, rebased.head, &me)).unwrap(),
        Err(Refused::AlreadyMerged)
    );
}

#[test]
fn a_conflict_refuses_the_merge_and_names_the_paths() {
    let store = MemoryStore::default();
    let base = commit_on(&store, "main", &[("a.txt", BASE), ("c.txt", "same\n")], "base");
    branch_from(&store, "feature", base);
    let feature = commit_on(
        &store,
        "feature",
        &[("a.txt", &BASE.replace("four", "FOUR from feature"))],
        "f",
    );
    let main = commit_on(
        &store,
        "main",
        &[("a.txt", &BASE.replace("four", "four from main"))],
        "m",
    );
    let checked = check(&store, &feature, &main).unwrap();
    assert_eq!(checked.conflicts, vec!["a.txt".to_string()]);
    let me = author("merger");
    for method in [Method::Merge, Method::Squash, Method::Rebase] {
        let overlay = Overlay::new(&store);
        match merge(&overlay, &request(method, feature, main, &me)).unwrap() {
            Err(Refused::Conflicts { paths, .. }) => assert_eq!(paths, vec!["a.txt".to_string()]),
            other => panic!("{method:?}: {other:?}"),
        }
    }
    assert_eq!(store.read_ref("refs/heads/main").unwrap(), Some(main));
}
