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

use gix::objs::WriteTo;

use super::*;
use crate::store::MemoryStore;

/// A commit with `parents`, over the empty tree; `name` makes each one distinct.
fn commit(store: &MemoryStore, name: &str, parents: &[ObjectId]) -> ObjectId {
    let tree = store.write(Kind::Tree, b"").unwrap();
    let signature = gix::actor::Signature {
        name: "Test".into(),
        email: "test@example.com".into(),
        time: gix::date::Time::new(1_700_000_000, 0),
    };
    let commit = gix::objs::Commit {
        tree,
        parents: parents.iter().copied().collect(),
        author: signature.clone(),
        committer: signature,
        encoding: None,
        message: format!("{name}\n").into(),
        extra_headers: Vec::new(),
    };
    let mut buf = Vec::new();
    commit.write_to(&mut buf).unwrap();
    store.write(Kind::Commit, &buf).unwrap()
}

fn tips(ids: &[ObjectId]) -> HashSet<ObjectId> {
    ids.iter().copied().collect()
}

#[test]
fn the_set_follows_the_refs_as_they_move_and_go() {
    let store = MemoryStore::default();
    let a = commit(&store, "a", &[]);
    let b = commit(&store, "b", &[a]);
    let c = commit(&store, "c", &[b]);
    // a side branch, and a commit of another repository that shares the store
    let side = commit(&store, "side", &[a]);
    let foreign = commit(&store, "foreign", &[b]);

    let mut reached = Reached::default();
    reached.update(&store, &tips(&[b]), 1000).unwrap();
    assert!(reached.contains(&a) && reached.contains(&b));
    assert!(!reached.contains(&c) && !reached.contains(&foreign) && !reached.contains(&side));

    // the branch moves forward: its old tip is the parent of a commit of the set, so nothing is walked again
    reached.update(&store, &tips(&[c, side]), 1000).unwrap();
    assert!(reached.contains(&c) && reached.contains(&side) && reached.contains(&b));
    assert!(!reached.contains(&foreign));

    // the side branch is deleted unmerged: it is reached no more, the rest still is
    reached.update(&store, &tips(&[c]), 1000).unwrap();
    assert!(!reached.contains(&side));
    assert!(reached.contains(&a) && reached.contains(&b) && reached.contains(&c));

    // a branch deleted after it was merged stays reached through the merge
    let merge = commit(&store, "merge", &[c, side]);
    reached.update(&store, &tips(&[merge, side]), 1000).unwrap();
    reached.update(&store, &tips(&[merge]), 1000).unwrap();
    assert!(reached.contains(&side) && reached.contains(&merge));

    // a forced branch: the commits only its old tip reached go
    reached.update(&store, &tips(&[b]), 1000).unwrap();
    assert!(!reached.contains(&c) && !reached.contains(&merge) && !reached.contains(&side));
    assert!(reached.contains(&a));
}

#[test]
fn the_walk_stops_at_its_limit_and_says_so() {
    let store = MemoryStore::default();
    let mut tip = commit(&store, "0", &[]);
    let first = tip;
    for at in 1..20 {
        tip = commit(&store, &at.to_string(), &[tip]);
    }
    let mut reached = Reached::default();
    reached.update(&store, &tips(&[tip]), 5).unwrap();
    assert!(reached.full);
    assert!(reached.contains(&tip) && !reached.contains(&first));
    // the same refs are not walked again
    reached.update(&store, &tips(&[tip]), 5).unwrap();
    assert!(reached.full && !reached.contains(&first));
    reached.update(&store, &tips(&[tip, first]), 1000).unwrap();
    assert!(!reached.full && reached.contains(&first));
}

#[test]
fn tips_are_asked_which_of_them_contain_a_commit_within_a_budget() {
    let store = MemoryStore::default();
    let a = commit(&store, "a", &[]);
    let b = commit(&store, "b", &[a]);
    let c = commit(&store, "c", &[b]);
    let side = commit(&store, "side", &[a]);
    let mut not = HashSet::new();
    let mut budget = 100;
    assert_eq!(reaches(&store, &b, &c, &mut not, &mut budget).unwrap(), Some(true));
    assert_eq!(reaches(&store, &b, &b, &mut not, &mut budget).unwrap(), Some(true));
    assert_eq!(reaches(&store, &b, &side, &mut not, &mut budget).unwrap(), Some(false));
    assert!(not.contains(&a) && not.contains(&side));
    // what is known not to reach it is not read again
    let before = budget;
    assert_eq!(reaches(&store, &b, &a, &mut not, &mut budget).unwrap(), Some(false));
    assert_eq!(budget, before);
    let mut none = 0;
    assert_eq!(reaches(&store, &a, &c, &mut HashSet::new(), &mut none).unwrap(), None);
}

#[test]
fn merge_request_refs_have_their_number() {
    assert_eq!(merge_request_ref(12), "refs/merge-requests/12/head");
}
