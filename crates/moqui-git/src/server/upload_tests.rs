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

use std::cell::Cell;
use std::sync::atomic::Ordering;

use super::*;
use crate::server::tests::{commit_on, CountedStore};

/// The tip of a chain of `length` commits on `branch`.
fn chain(store: &CountedStore, branch: &str, length: usize) -> ObjectId {
    let mut tip = commit_on(&store.inner, branch, &format!("{branch} 0\n"));
    for at in 1..length {
        tip = commit_on(&store.inner, branch, &format!("{branch} {at}\n"));
    }
    tip
}

/// SEC-GIT-83: common haves interleaved with made-up ones and with a stored commit the readers do not read. Each common
/// have walks the want's history once at most, a made-up have walks nothing, and the unread commit is asked of the
/// readers once however often it comes.
#[test]
fn interleaved_haves_walk_once_for_each_common_one_and_ask_once_of_an_unread_one() {
    let store = CountedStore::default();
    let old = chain(&store, "old", 3);
    let hidden = commit_on(&store.inner, "hidden", "a merge request's head\n");
    let tip = chain(&store, "main", 40);
    let asked = Cell::new(0);
    let readable = |ids: &[ObjectId]| -> Result<HashSet<ObjectId>, Error> {
        asked.set(asked.get() + 1);
        Ok(ids.iter().filter(|id| **id != hidden).copied().collect())
    };
    let old_commits: Vec<ObjectId> = {
        let mut ids = vec![old];
        let mut at = old;
        while let Some(parent) = ops::read_commit(&store, &at).unwrap().parents.first().copied() {
            ids.push(parent);
            at = parent;
        }
        ids
    };
    let mut negotiation = Negotiation::new(&store, &readable, &[tip, tip, tip]);
    assert_eq!(negotiation.wants, [tip], "each want once");
    let walks_before = store.reads.load(Ordering::Relaxed);
    for common in &old_commits {
        assert!(negotiation.have(*common).unwrap());
        assert!(!negotiation.ok_to_give_up().unwrap());
        let after_common = store.reads.load(Ordering::Relaxed);
        for made_up in 0..50u32 {
            let id = ObjectId::from_hex(format!("{made_up:040x}").as_bytes()).unwrap();
            assert!(!negotiation.have(id).unwrap());
            assert!(!negotiation.have(hidden).unwrap());
            assert!(!negotiation.ok_to_give_up().unwrap());
        }
        // the made-up haves and the unread commit read only themselves: no walk
        assert!(
            store.reads.load(Ordering::Relaxed) - after_common <= 2 * 50 + 1,
            "{} objects read for haves that add nothing",
            store.reads.load(Ordering::Relaxed) - after_common
        );
    }
    let walked = store.reads.load(Ordering::Relaxed) - walks_before;
    // three common haves, each one walk of the main chain (40 commits) and a few reads of its own, plus the made-up
    // ones; a walk for each have would be thousands
    assert!(walked <= 3 * (40 + 5) + 3 * (2 * 50 + 1), "{walked} objects read");
    // the hidden commit was asked of the readers once, and each common have once
    assert_eq!(asked.get(), 1 + old_commits.len());
}

/// SEC-GIT-83: past its bound of commits walked a negotiation gives up, answering ready without reading more, so the
/// pack goes out for the commits acknowledged.
#[test]
fn a_negotiation_past_its_bound_of_commits_walked_answers_ready() {
    let store = CountedStore::default();
    let old = commit_on(&store.inner, "old", "an unmerged branch\n");
    let tip = chain(&store, "main", 60);
    let readable = |ids: &[ObjectId]| -> Result<HashSet<ObjectId>, Error> { Ok(ids.iter().copied().collect()) };
    let mut negotiation = Negotiation::new(&store, &readable, &[tip]);
    negotiation.most_walked = 10;
    assert!(negotiation.have(old).unwrap());
    let before = store.reads.load(Ordering::Relaxed);
    assert!(
        negotiation.ok_to_give_up().unwrap(),
        "the want does not reach the old commit, but the walk is cut off"
    );
    assert!(store.reads.load(Ordering::Relaxed) - before <= 10);
    let after = store.reads.load(Ordering::Relaxed);
    assert!(negotiation.ok_to_give_up().unwrap());
    assert_eq!(
        store.reads.load(Ordering::Relaxed),
        after,
        "no more reads past the bound"
    );
    // the bound of the server
    assert_eq!(
        Negotiation::new(&store, &readable, &[tip]).most_walked,
        MOST_COMMITS_WALKED
    );
}
