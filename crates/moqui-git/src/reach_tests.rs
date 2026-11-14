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

/// A store that counts the objects read from it.
#[derive(Debug, Default)]
struct Counted {
    store: MemoryStore,
    reads: std::sync::atomic::AtomicUsize,
}

impl Counted {
    fn reads(&self) -> usize {
        self.reads.swap(0, std::sync::atomic::Ordering::Relaxed)
    }
}

impl GitStore for Counted {
    fn find(&self, id: &ObjectId) -> Result<Option<(Kind, Vec<u8>)>, Error> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.store.find(id)
    }

    fn find_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ObjectId>, Error> {
        self.store.find_prefix(prefix, limit)
    }

    fn write(&self, kind: Kind, data: &[u8]) -> Result<ObjectId, Error> {
        self.store.write(kind, data)
    }

    fn read_ref(&self, name: &str) -> Result<Option<ObjectId>, Error> {
        self.store.read_ref(name)
    }

    fn list_refs(&self, prefix: &str) -> Result<Vec<(String, ObjectId)>, Error> {
        self.store.list_refs(prefix)
    }

    fn update_refs(
        &self,
        edits: &[crate::store::RefEdit],
        atomic: bool,
    ) -> Result<Vec<crate::store::RefOutcome>, Error> {
        self.store.update_refs(edits, atomic)
    }
}

#[test]
fn a_branch_that_moves_forward_costs_its_new_commits() {
    let counted = Counted::default();
    let mut tip = commit(&counted.store, "0", &[]);
    for at in 1..50 {
        tip = commit(&counted.store, &at.to_string(), &[tip]);
    }
    let mut reached = Reached::default();
    reached.update(&counted, &tips(&[tip]), 1000).unwrap();
    assert_eq!(counted.reads(), 50, "the history, once");
    // a fast-forward by two commits reads those two, and the history is not read again
    let next = commit(&counted.store, "50", &[tip]);
    let after = commit(&counted.store, "51", &[next]);
    reached.update(&counted, &tips(&[after]), 1000).unwrap();
    assert_eq!(counted.reads(), 2);
    assert!(reached.contains(&tip) && reached.contains(&after));
    // a second branch from an old commit reads its own commit
    let side = commit(&counted.store, "side", &[tip]);
    reached.update(&counted, &tips(&[after, side]), 1000).unwrap();
    assert_eq!(counted.reads(), 1);
    // deleted unmerged: the set is made again
    reached.update(&counted, &tips(&[after]), 1000).unwrap();
    assert_eq!(counted.reads(), 52);
    assert!(!reached.contains(&side));
    // a prefix is looked up among the reached commits only
    let hex = side.to_string();
    assert!(reached.with_prefix(&hex[..9], 2).is_empty());
    assert_eq!(reached.with_prefix(&after.to_string()[..9], 2), vec![after]);
    assert_eq!(reached.with_prefix(&after.to_string()[..8], 2), vec![after]);
}

#[test]
fn the_sets_of_many_repositories_let_go_of_the_one_used_longest_ago() {
    let mut sets = ReachSets::default();
    let first = sets.of("a", 2);
    sets.of("b", 2);
    // a is used again, so c takes b's place
    assert!(Arc::ptr_eq(&first, &sets.of("a", 2)));
    sets.of("c", 2);
    assert!(Arc::ptr_eq(&first, &sets.of("a", 2)));
    assert_eq!(sets.sets.len(), 2);
    assert!(!sets.sets.contains_key("b"));
}

#[test]
fn the_files_of_commits_are_found_and_nothing_else() {
    let store = MemoryStore::default();
    let blob = |text: &str| store.write(Kind::Blob, text.as_bytes()).unwrap();
    let tree = |entries: &[(&str, ObjectId, bool)]| {
        let mut data = Vec::new();
        for (name, id, directory) in entries {
            data.extend_from_slice(if *directory { b"40000 " } else { b"100644 " });
            data.extend_from_slice(name.as_bytes());
            data.push(0);
            data.extend_from_slice(id.as_bytes());
        }
        store.write(Kind::Tree, &data).unwrap()
    };
    let commit_of = |tree: ObjectId| {
        let data = format!("tree {tree}\nauthor A <a@example.com> 1 +0000\ncommitter A <a@example.com> 1 +0000\n\nx\n");
        store.write(Kind::Commit, data.as_bytes()).unwrap()
    };
    let (a, b, secret) = (blob("a"), blob("b"), blob("secret"));
    let sub = tree(&[("b.txt", b, false)]);
    let public = commit_of(tree(&[("a.txt", a, false), ("sub", sub, true)]));
    let private = commit_of(tree(&[("secret.txt", secret, false)]));
    let wanted: HashSet<ObjectId> = [a, b, sub, secret, private].into_iter().collect();
    let found = objects_within(&store, &[public], &wanted, 100).unwrap();
    assert_eq!(found, [a, b, sub].into_iter().collect());
    // within its budget of trees
    assert!(!objects_within(&store, &[public], &wanted, 1).unwrap().contains(&b));
}
