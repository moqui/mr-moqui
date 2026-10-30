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

//! Walks of the commit graph over any [`GitStore`], newest committer date first as git's are: the commits reachable
//! from some tips and not from hidden ones (the new commits of a push, what a fetch sends), whether one commit is an
//! ancestor of another (a fast-forward), and the log of a branch. Hidden commits mark their ancestors hidden as the
//! walk reaches them; commits dated out of order are caught by reading a few more commits once only hidden ones are
//! queued (git's slop), and a commit first walked as visible is marked hidden when a hidden path reaches it later.
//!
//! Grafted commits (a shallow client's `shallow` commits) are walked as if they had no parents.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use gix::objs::Kind;
use gix::ObjectId;

use super::ops::{self, CommitInfo};
use super::store::GitStore;
use moqui_core::context::Error;

/// How many commits one walk may read, so a damaged repository cannot loop.
const MAX_WALK: usize = 10_000_000;

/// How many more commits a limited walk reads once only hidden commits are queued.
const SLOP: usize = 5;

/// After this many commits read one at a time, a walk reads the rest of the history in one scan, where the store
/// can ([`GitStore::scan_commits`]), of at most [`PRELOAD_MAX`] commits.
const PRELOAD_AFTER: usize = 1000;
const PRELOAD_MAX: usize = 2_000_000;

/// A queued commit: newest first, then in the order queued.
#[derive(PartialEq, Eq)]
struct Queued {
    seconds: i64,
    seq: u64,
    id: ObjectId,
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        self.seconds.cmp(&other.seconds).then_with(|| other.seq.cmp(&self.seq))
    }
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct Node {
    commit: CommitInfo,
    hidden: bool,
    queued: bool,
}

/// One walk. Push tips (visible) and hidden tips, then read commits with [`Walk::next`] (a log) or all at once with
/// [`Walk::limited`] (exact when commits are dated out of order).
pub(crate) struct Walk<'a> {
    store: &'a dyn GitStore,
    grafts: Option<&'a HashSet<ObjectId>>,
    nodes: HashMap<ObjectId, Node>,
    queue: BinaryHeap<Queued>,
    seq: u64,
    /// Queued commits that are not hidden: when none are left, what is still queued can only hide.
    visible_queued: usize,
    /// Commits read by a scan, and how many were read one at a time before it.
    preloaded: Option<HashMap<ObjectId, CommitInfo>>,
    single_reads: usize,
}

impl<'a> Walk<'a> {
    pub(crate) fn new(store: &'a dyn GitStore) -> Self {
        Self {
            store,
            grafts: None,
            nodes: HashMap::new(),
            queue: BinaryHeap::new(),
            seq: 0,
            visible_queued: 0,
            preloaded: None,
            single_reads: 0,
        }
    }

    /// Read the commits of the store in one scan, for a walk of much of the history (a full clone). False when the
    /// store reads commits one at a time only.
    pub(crate) fn preload(&mut self) -> Result<bool, Error> {
        if self.preloaded.is_some() {
            return Ok(true);
        }
        let mut commits = HashMap::new();
        let scanned = self.store.scan_commits(&mut |id, data| {
            commits.insert(id, ops::parse_commit(&id, data)?);
            Ok(commits.len() < PRELOAD_MAX)
        })?;
        if scanned {
            self.preloaded = Some(commits);
        }
        Ok(scanned)
    }

    /// A commit, from the scan when there was one.
    fn commit(&mut self, id: &ObjectId, hidden: bool) -> Result<Option<CommitInfo>, Error> {
        if let Some(commit) = self.preloaded.as_ref().and_then(|commits| commits.get(id)) {
            return Ok(Some(commit.clone()));
        }
        self.single_reads += 1;
        if self.single_reads == PRELOAD_AFTER && self.preload()? {
            if let Some(commit) = self.preloaded.as_ref().and_then(|commits| commits.get(id)) {
                return Ok(Some(commit.clone()));
            }
        }
        match self.store.find(id)? {
            Some((Kind::Commit, data)) => Ok(Some(ops::parse_commit(id, &data)?)),
            // a hidden parent the repository does not have: a shallow repository's boundary
            None if hidden => Ok(None),
            None => Err(Error::new(format!("git commit {id} is missing"))),
            Some((kind, _)) => Err(Error::new(format!("git object {id} is a {kind}, not a commit"))),
        }
    }

    /// Walk the commits in `grafts` as if they had no parents.
    pub(crate) fn with_grafts(mut self, grafts: &'a HashSet<ObjectId>) -> Self {
        self.grafts = Some(grafts);
        self
    }

    /// Start from `id`, which may be an annotated tag (peeled to its commit). A hidden tip the store does not have
    /// (a client's `have` of a commit this repository never had) is passed over; a visible one is an error. False
    /// when `id` is not a commit and was passed over.
    pub(crate) fn push(&mut self, id: ObjectId, hidden: bool) -> Result<bool, Error> {
        let Some(commit) = peel_to_commit(self.store, &id)? else {
            if hidden {
                return Ok(false);
            }
            return Err(Error::new(format!("git object {id} is not a commit")));
        };
        self.add(commit, hidden)?;
        Ok(true)
    }

    /// Pass over a hidden tip that is missing; the caller checks visible ones.
    fn add(&mut self, id: ObjectId, hidden: bool) -> Result<(), Error> {
        if let Some(node) = self.nodes.get(&id) {
            if hidden && !node.hidden {
                self.hide(id);
            }
            return Ok(());
        }
        if self.nodes.len() >= MAX_WALK {
            return Err(Error::new(format!("commit walk passed {MAX_WALK} commits")));
        }
        let Some(commit) = self.commit(&id, hidden)? else {
            return Ok(());
        };
        self.seq += 1;
        self.queue.push(Queued {
            seconds: commit.seconds,
            seq: self.seq,
            id,
        });
        if !hidden {
            self.visible_queued += 1;
        }
        self.nodes.insert(
            id,
            Node {
                commit,
                hidden,
                queued: true,
            },
        );
        Ok(())
    }

    fn parents(&self, commit: &CommitInfo) -> Vec<ObjectId> {
        if self.grafts.is_some_and(|grafts| grafts.contains(&commit.id)) {
            Vec::new()
        } else {
            commit.parents.clone()
        }
    }

    /// Mark `id` and the ancestors walked so far hidden.
    fn hide(&mut self, id: ObjectId) {
        let mut pending = vec![id];
        while let Some(id) = pending.pop() {
            let Some(node) = self.nodes.get_mut(&id) else {
                continue;
            };
            if node.hidden {
                continue;
            }
            node.hidden = true;
            if node.queued {
                self.visible_queued -= 1;
            }
            let commit = node.commit.clone();
            pending.extend(
                self.parents(&commit)
                    .into_iter()
                    .filter(|parent| self.nodes.contains_key(parent)),
            );
        }
    }

    /// The next commit in date order with whether it is hidden; its parents are queued.
    fn pop(&mut self) -> Result<Option<(ObjectId, bool)>, Error> {
        let Some(queued) = self.queue.pop() else {
            return Ok(None);
        };
        let node = self.nodes.get_mut(&queued.id).expect("a queued commit has its node");
        node.queued = false;
        let hidden = node.hidden;
        if !hidden {
            self.visible_queued -= 1;
        }
        let commit = node.commit.clone();
        for parent in self.parents(&commit) {
            self.add(parent, hidden)?;
        }
        Ok(Some((queued.id, hidden)))
    }

    /// The next visible commit, newest first, for a log. A commit dated older than one of its descendants that is
    /// also reachable from a hidden tip can come out before the hidden path reaches it; [`Walk::limited`] is exact.
    pub(crate) fn next(&mut self) -> Result<Option<CommitInfo>, Error> {
        while let Some((id, hidden)) = self.pop()? {
            if !hidden {
                return Ok(Some(self.nodes[&id].commit.clone()));
            }
        }
        Ok(None)
    }

    /// Every commit reachable from the visible tips and not from the hidden ones, newest first.
    pub(crate) fn limited(mut self) -> Result<Vec<CommitInfo>, Error> {
        let mut order = Vec::new();
        let mut slop = SLOP;
        while let Some((id, _)) = self.pop()? {
            order.push(id);
            if self.visible_queued == 0 {
                slop -= 1;
                if slop == 0 {
                    break;
                }
            } else {
                slop = SLOP;
            }
        }
        Ok(order
            .into_iter()
            .filter_map(|id| {
                let node = &self.nodes[&id];
                (!node.hidden).then(|| node.commit.clone())
            })
            .collect())
    }
}

/// The commit an object names: itself, or the target of an annotated tag (and of a tag of a tag). `None` when it is
/// not in the store or names a tree or a blob.
pub(crate) fn peel_to_commit(store: &dyn GitStore, id: &ObjectId) -> Result<Option<ObjectId>, Error> {
    let mut current = *id;
    for _ in 0..16 {
        match store.find(&current)? {
            Some((Kind::Commit, _)) => return Ok(Some(current)),
            Some((Kind::Tag, data)) => {
                let tag = gix::objs::TagRef::from_bytes(&data, ops::HASH)
                    .map_err(|err| Error::new(format!("git tag {current}: {err}")))?;
                current = tag.target();
            }
            _ => return Ok(None),
        }
    }
    Err(Error::new(format!("git tag {id} is nested too deeply")))
}

/// The commits reachable from `tips` and not from `hidden`, newest first.
pub(crate) fn new_commits(
    store: &dyn GitStore,
    tips: &[ObjectId],
    hidden: &[ObjectId],
) -> Result<Vec<CommitInfo>, Error> {
    let mut walk = Walk::new(store);
    for tip in hidden {
        walk.push(*tip, true)?;
    }
    for tip in tips {
        walk.push(*tip, false)?;
    }
    walk.limited()
}

/// Whether `ancestor` is `descendant` or reachable from it: a ref moving from `ancestor` to `descendant` is a
/// fast-forward.
pub(crate) fn is_ancestor(store: &dyn GitStore, ancestor: &ObjectId, descendant: &ObjectId) -> Result<bool, Error> {
    if ancestor == descendant {
        return Ok(true);
    }
    Ok(new_commits(store, &[*ancestor], &[*descendant])?.is_empty())
}

#[cfg(test)]
mod tests {
    use gix::objs::WriteTo;

    use super::super::store::MemoryStore;
    use super::*;

    /// A commit with `parents` at `seconds`, over the empty tree; `name` makes each one distinct.
    fn commit(store: &MemoryStore, name: &str, seconds: i64, parents: &[ObjectId]) -> ObjectId {
        let tree = store.write(Kind::Tree, b"").unwrap();
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
            message: format!("{name}\n").into(),
            extra_headers: Vec::new(),
        };
        let mut buf = Vec::new();
        commit.write_to(&mut buf).unwrap();
        store.write(Kind::Commit, &buf).unwrap()
    }

    fn names(store: &MemoryStore, commits: &[CommitInfo]) -> Vec<String> {
        commits
            .iter()
            .map(|commit| ops::read_commit(store, &commit.id).unwrap().message.trim().to_string())
            .collect()
    }

    #[test]
    fn new_commits_of_a_merge_leave_out_what_was_there() {
        let store = MemoryStore::default();
        let a = commit(&store, "a", 100, &[]);
        let b = commit(&store, "b", 200, &[a]);
        let c = commit(&store, "c", 300, &[b]);
        let side = commit(&store, "side", 250, &[a]);
        let merge = commit(&store, "merge", 400, &[c, side]);
        let found = new_commits(&store, &[merge], &[b]).unwrap();
        assert_eq!(names(&store, &found), ["merge", "c", "side"]);
        assert!(new_commits(&store, &[b], &[merge]).unwrap().is_empty());
        let all = new_commits(&store, &[merge], &[]).unwrap();
        assert_eq!(names(&store, &all), ["merge", "c", "side", "b", "a"]);
        assert!(is_ancestor(&store, &a, &merge).unwrap());
        assert!(is_ancestor(&store, &side, &merge).unwrap());
        assert!(!is_ancestor(&store, &merge, &side).unwrap());
        assert!(!is_ancestor(&store, &c, &side).unwrap());
    }

    #[test]
    fn a_commit_dated_before_its_parent_is_still_hidden() {
        let store = MemoryStore::default();
        // the hidden tip's history has a commit dated far in the past, below one that is newer than the visible tip
        let base = commit(&store, "base", 1000, &[]);
        let skewed = commit(&store, "skewed", 10, &[base]);
        let hidden_tip = commit(&store, "hidden", 2000, &[skewed]);
        let visible = commit(&store, "visible", 1500, &[base]);
        let found = new_commits(&store, &[visible], &[hidden_tip]).unwrap();
        assert_eq!(names(&store, &found), ["visible"]);
        // and the other way: the visible side's old-dated commit is new
        let found = new_commits(&store, &[hidden_tip], &[visible]).unwrap();
        assert_eq!(names(&store, &found), ["hidden", "skewed"]);
    }

    #[test]
    fn grafts_cut_the_walk() {
        let store = MemoryStore::default();
        let a = commit(&store, "a", 100, &[]);
        let b = commit(&store, "b", 200, &[a]);
        let c = commit(&store, "c", 300, &[b]);
        let grafts = HashSet::from([b]);
        let mut walk = Walk::new(&store).with_grafts(&grafts);
        walk.push(c, false).unwrap();
        assert_eq!(names(&store, &walk.limited().unwrap()), ["c", "b"]);
        let mut walk = Walk::new(&store);
        walk.push(c, false).unwrap();
        let mut log = Vec::new();
        while let Some(commit) = walk.next().unwrap() {
            log.push(commit);
        }
        assert_eq!(names(&store, &log), ["c", "b", "a"]);
    }

    #[test]
    fn a_missing_hidden_tip_is_passed_over_and_a_missing_visible_one_is_an_error() {
        let store = MemoryStore::default();
        let a = commit(&store, "a", 100, &[]);
        let unknown = ObjectId::from_hex(b"1234567890123456789012345678901234567890").unwrap();
        assert_eq!(names(&store, &new_commits(&store, &[a], &[unknown]).unwrap()), ["a"]);
        assert!(new_commits(&store, &[unknown], &[]).is_err());
    }
}
