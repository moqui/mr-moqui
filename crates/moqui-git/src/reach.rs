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

//! Which commits of a repository are read by id (docs/interfaces/Git-Hosting.md, Reachability; plans/Dogfood.md F4,
//! N-REACH). A fork network keeps one object store, and the head of a merge request pushed for review is kept in its
//! target's store, so that an object is in a repository's store does not say the repository's readers may read it: a
//! commit is read by its id only when a branch, a tag or the head of a merge request that is not hidden reaches it.
//!
//! The answer is kept per repository as the set of commits its refs reach, brought up to date when the refs change:
//! a new tip is walked back to the commits already known, so a push costs its new commits. A tip that went away
//! (a deleted branch, a forced one) and is not the parent of a commit of the set makes the set again. The walk stops
//! at `git-server` `reach-max-commits` (1,000,000) commits of a repository; past it a commit is not found by id.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gix::objs::Kind;
use gix::ObjectId;

use super::store::GitStore;
use super::{rows_guard, GitProvider, Repo};
use moqui_core::context::Error;
use moqui_lib::object::Value;

/// The ref namespace of merge request heads: `refs/merge-requests/<mergeRequestNum>/head`.
pub(crate) const MERGE_REQUEST_REFS: &str = "refs/merge-requests/";

/// The refs a push for review names: `refs/for/<branch>`.
pub(crate) const REVIEW_REFS: &str = "refs/for/";

/// A hit is answered from the set without reading the refs again for this long.
const FRESH: Duration = Duration::from_secs(2);

/// At most this many repositories' sets are kept; one more lets go of the one used longest ago.
const MAX_REPOSITORIES: usize = 128;

const DEFAULT_MAX_COMMITS: usize = 1_000_000;

const BATCH: usize = 500;

/// The ref of merge request `num`'s head.
pub(crate) fn merge_request_ref(num: i64) -> String {
    format!("{MERGE_REQUEST_REFS}{num}/head")
}

/// The parents of a commit, from its header lines.
pub(crate) fn parents_of(data: &[u8]) -> Vec<ObjectId> {
    let mut parents = Vec::new();
    for line in data.split(|byte| *byte == b'\n') {
        if let Some(hex) = line.strip_prefix(b"parent ") {
            parents.extend(ObjectId::from_hex(hex).ok());
        } else if !line.starts_with(b"tree ") {
            // the parents follow the tree, before the author
            break;
        }
    }
    parents
}

/// The commits the refs of one repository reach.
#[derive(Debug, Default)]
pub(crate) struct Reached {
    /// The commits the refs named when the set was last brought up to date.
    tips: HashSet<ObjectId>,
    /// Each commit reached, and whether a commit of the set has it as a parent.
    commits: HashMap<ObjectId, bool>,
    /// What the ref targets that are not commits (annotated tags) peel to.
    peeled: HashMap<ObjectId, Option<ObjectId>>,
    /// The walk stopped at the limit: a commit that is not in the set may still be reached.
    full: bool,
    read: Option<Instant>,
}

impl Reached {
    fn contains(&self, id: &ObjectId) -> bool {
        self.commits.contains_key(id)
    }

    /// Add `tips` and what they reach to the set, walking back from each to the commits already in it.
    fn walk(&mut self, store: &dyn GitStore, tips: &HashSet<ObjectId>, limit: usize) -> Result<(), Error> {
        let mut pending: Vec<ObjectId> = Vec::new();
        for tip in tips {
            if !self.commits.contains_key(tip) {
                self.commits.insert(*tip, false);
                pending.push(*tip);
            }
        }
        while !pending.is_empty() {
            if self.commits.len() >= limit {
                self.full = true;
                break;
            }
            let at = pending.len().saturating_sub(BATCH);
            let batch = pending.split_off(at);
            let commits = &mut self.commits;
            store.find_many(&batch, &mut |_, kind, data| {
                if kind != Kind::Commit {
                    return Ok(());
                }
                for parent in parents_of(data) {
                    match commits.get_mut(&parent) {
                        Some(interior) => *interior = true,
                        None => {
                            commits.insert(parent, true);
                            pending.push(parent);
                        }
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Bring the set to what `tips` reach. The new tips are walked first, back to the commits already known. A tip
    /// that went away is then still reached when a commit of the set has it as a parent (the branch moved forward,
    /// which the walk of its new tip just found, or was merged before it was deleted): every commit of the set is
    /// still an ancestor of a tip, and nothing more is read. Otherwise (a deleted branch that was not merged, a
    /// forced one) the set is made again from the tips.
    fn update(&mut self, store: &dyn GitStore, tips: &HashSet<ObjectId>, limit: usize) -> Result<(), Error> {
        if self.read.is_some() && self.tips == *tips {
            self.read = Some(Instant::now());
            return Ok(());
        }
        if self.full {
            self.commits.clear();
            self.full = false;
        }
        self.walk(store, tips, limit)?;
        let gone = self
            .tips
            .iter()
            .any(|tip| !tips.contains(tip) && !self.commits.get(tip).copied().unwrap_or(false));
        if gone {
            self.commits.clear();
            self.full = false;
            self.walk(store, tips, limit)?;
        }
        self.tips.clone_from(tips);
        self.read = Some(Instant::now());
        Ok(())
    }

    /// The commits of the set whose ids start with the hex digits `prefix`, at most `limit` of them.
    fn with_prefix(&self, prefix: &str, limit: usize) -> Vec<ObjectId> {
        let prefix = prefix.to_ascii_lowercase();
        let whole = prefix.len() / 2;
        let Ok(bytes) = (0..whole)
            .map(|at| u8::from_str_radix(&prefix[at * 2..at * 2 + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
        else {
            return Vec::new();
        };
        let odd = prefix
            .get(whole * 2..)
            .and_then(|digit| u8::from_str_radix(digit, 16).ok());
        self.commits
            .keys()
            .filter(|id| {
                let id = id.as_bytes();
                id.starts_with(&bytes) && odd.is_none_or(|nibble| id.get(whole).is_some_and(|byte| byte >> 4 == nibble))
            })
            .take(limit)
            .copied()
            .collect()
    }

    /// The commit a ref target names, remembered for a target that is not a commit of the set.
    fn commit_of(&mut self, store: &dyn GitStore, target: ObjectId) -> Result<Option<ObjectId>, Error> {
        if self.commits.contains_key(&target) {
            return Ok(Some(target));
        }
        if let Some(peeled) = self.peeled.get(&target) {
            return Ok(*peeled);
        }
        let peeled = super::revwalk::peel_to_commit(store, &target)?;
        if peeled != Some(target) {
            if self.peeled.len() > 100_000 {
                self.peeled.clear();
            }
            self.peeled.insert(target, peeled);
        }
        Ok(peeled)
    }
}

/// The refs of a repository by who reads them, each with what it points to.
#[derive(Debug, Default)]
pub(crate) struct Refs {
    /// Branches, tags and the heads of merge requests that are not hidden.
    pub readable: Vec<(String, ObjectId)>,
    /// The heads of hidden merge requests.
    pub hidden: Vec<(String, ObjectId)>,
}

/// The sets of the repositories read by id on this node: at most [`MAX_REPOSITORIES`], the one used longest ago
/// going first.
#[derive(Debug, Default)]
pub(crate) struct ReachSets {
    sets: HashMap<String, (Arc<Mutex<Reached>>, u64)>,
    used: u64,
}

impl ReachSets {
    fn of(&mut self, repository_id: &str, most: usize) -> Arc<Mutex<Reached>> {
        self.used += 1;
        let used = self.used;
        if let Some((set, last)) = self.sets.get_mut(repository_id) {
            *last = used;
            return Arc::clone(set);
        }
        while self.sets.len() >= most.max(1) {
            let Some(oldest) = self
                .sets
                .iter()
                .min_by_key(|(_, (_, last))| *last)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.sets.remove(&oldest);
        }
        let set = Arc::new(Mutex::new(Reached::default()));
        self.sets.insert(repository_id.to_string(), (Arc::clone(&set), used));
        set
    }
}

/// The sets of the repositories read by id on this node.
pub(crate) type ReachCache = Mutex<ReachSets>;

thread_local! {
    /// Set while a service asked for with authorization disabled reads the heads of hidden merge requests too.
    static WITH_HIDDEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While it lives, reads by id on this thread reach the heads of hidden merge requests too.
pub(crate) struct WithHidden(bool);

impl Drop for WithHidden {
    fn drop(&mut self) {
        WITH_HIDDEN.with(|flag| flag.set(self.0));
    }
}

/// Reads by id on this thread reach the heads of hidden merge requests when `on`, until the guard is dropped.
pub(crate) fn with_hidden(on: bool) -> WithHidden {
    WithHidden(WITH_HIDDEN.with(|flag| flag.replace(on || flag.get())))
}

pub(crate) fn hidden_included() -> bool {
    WITH_HIDDEN.with(std::cell::Cell::get)
}

impl GitProvider {
    /// The refs of the heads of the repository's hidden merge requests (`GitMergeRequest.hiddenDate`).
    pub(crate) fn hidden_heads(&self, repository_id: &str) -> Result<HashSet<String>, Error> {
        let _rows = rows_guard();
        let entity = self.entity()?;
        let rows = entity
            .find("moqui.resource.GitMergeRequest")?
            .eq("repositoryId", Value::string(repository_id))
            .condition(moqui_core::entity::Condition::is_not_null("hiddenDate"))
            .select_field("mergeRequestNum")
            .list()?;
        Ok(rows
            .iter()
            .filter_map(|row| super::long(row.get("mergeRequestNum")))
            .map(merge_request_ref)
            .collect())
    }

    /// The refs a reader of the repository reads: its branches, its tags and the heads of its merge requests that
    /// are not hidden; and those hidden heads.
    pub(crate) fn readable_refs(&self, repo: &Repo) -> Result<Refs, Error> {
        let refs = repo.store.list_refs("refs/")?;
        let hidden = if refs.iter().any(|(name, _)| name.starts_with(MERGE_REQUEST_REFS)) {
            self.hidden_heads(&repo.definition.id)?
        } else {
            HashSet::new()
        };
        let mut found = Refs::default();
        for (name, id) in refs {
            if hidden.contains(&name) {
                found.hidden.push((name, id));
            } else if name.starts_with("refs/heads/")
                || name.starts_with("refs/tags/")
                || name.starts_with(MERGE_REQUEST_REFS)
            {
                found.readable.push((name, id));
            }
        }
        Ok(found)
    }

    fn reached(&self, repository_id: &str) -> Arc<Mutex<Reached>> {
        moqui_lib::sync::lock(&self.inner.reach).of(repository_id, MAX_REPOSITORIES)
    }

    /// Forget what the refs of a repository reach, when it is deleted or its storage replaced.
    pub(crate) fn forget_reached(&self, repository_id: &str) {
        moqui_lib::sync::lock(&self.inner.reach).sets.remove(repository_id);
    }

    /// Which of `ids` are commits a ref the repository's readers read reaches (see the module): what a read by id,
    /// a fetch by id and the parents a push names are checked against.
    pub(crate) fn reachable_of(&self, repo: &Repo, ids: &[ObjectId]) -> Result<HashSet<ObjectId>, Error> {
        let store = repo.store.as_ref();
        let entry = self.reached(&repo.definition.id);
        let mut reached = moqui_lib::sync::lock(&entry);
        let fresh = reached.read.is_some_and(|read| read.elapsed() < FRESH);
        let mut held = Vec::new();
        if !fresh || !ids.iter().all(|id| reached.contains(id)) || hidden_included() {
            let Refs { readable, hidden } = self.readable_refs(repo)?;
            let mut tips = HashSet::new();
            for (_, target) in readable {
                tips.extend(reached.commit_of(store, target)?);
            }
            let limit = super::number_setting("reach-max-commits", DEFAULT_MAX_COMMITS).max(1);
            reached.update(store, &tips, limit)?;
            if reached.full {
                tracing::warn!(
                    repository = %repo.definition.id,
                    "the refs reach more than git-server reach-max-commits ({limit}) commits; older ones are not read by id"
                );
            }
            held = hidden;
        }
        let mut found: HashSet<ObjectId> = ids.iter().filter(|id| reached.contains(id)).copied().collect();
        drop(reached);
        if hidden_included() && found.len() < ids.len() {
            // a hidden head and what only it reaches: asked for rarely, so walked each time
            for (_, head) in held {
                let Some(head) = super::revwalk::peel_to_commit(store, &head)? else {
                    continue;
                };
                for id in ids {
                    if !found.contains(id) && super::revwalk::is_ancestor(store, id, &head).unwrap_or(false) {
                        found.insert(*id);
                    }
                }
            }
        }
        Ok(found)
    }

    /// Whether `id` is a commit a ref the repository's readers read reaches.
    pub(crate) fn reachable(&self, repo: &Repo, id: &ObjectId) -> Result<bool, Error> {
        Ok(self.reachable_of(repo, &[*id])?.contains(id))
    }

    /// The commits the repository's readable refs reach whose ids start with the hex digits `prefix`, at most
    /// `limit`: a prefix is looked up among these only, so it says nothing of what else the store holds.
    pub(crate) fn reachable_prefix(&self, repo: &Repo, prefix: &str, limit: usize) -> Result<Vec<ObjectId>, Error> {
        // bring the set up to date
        self.reachable_of(repo, &[ObjectId::null(gix::hash::Kind::Sha1)])?;
        let entry = self.reached(&repo.definition.id);
        let reached = moqui_lib::sync::lock(&entry);
        Ok(reached.with_prefix(prefix, limit))
    }

    /// Whether the commits the refs of a repository have left (a deleted or forced branch's) are still read by id by
    /// who works in it: the compatibility rule of [`Self::readable_by_id`]. Only where every commit of the store was
    /// once a reader's to read: a repository that is in no fork network, none of whose merge requests is or ever was
    /// hidden (`GitRepository.reviewHiddenDate`, and a merge request hidden now), on a server that does not start
    /// merge requests hidden (`review-start-hidden`). The row is read now, not as it was when the repository was
    /// opened: another node may have made the first fork, or hidden the first merge request, since.
    pub(crate) fn leftovers_readable(&self, repository_id: &str) -> Result<bool, Error> {
        if super::server::review_starts_hidden() {
            return Ok(false);
        }
        let _rows = rows_guard();
        let entity = self.entity()?;
        let Some(row) = entity
            .find(super::GIT_REPOSITORY)?
            .eq("repositoryId", Value::string(repository_id))
            .one()?
        else {
            return Ok(false);
        };
        if super::text(row.get("forkNetworkId")).is_some() || super::text(row.get("reviewHiddenDate")).is_some() {
            return Ok(false);
        }
        let hidden = entity
            .find("moqui.resource.GitMergeRequest")?
            .eq("repositoryId", Value::string(repository_id))
            .condition(moqui_core::entity::Condition::is_not_null("hiddenDate"))
            .select_field("mergeRequestId")
            .limit(1)
            .list()?;
        let none = hidden.iter().next().is_none();
        Ok(none)
    }

    /// Record that a merge request of the repository was hidden (`GitRepository.reviewHiddenDate`, kept from the
    /// first one on): its store has held commits its readers were not to read, so from then on only its refs decide
    /// what is read by id ([`Self::leftovers_readable`]), also after the merge request is shown or its ref released.
    pub(crate) fn note_review_hidden(&self, repository_id: &str) -> Result<(), Error> {
        let _rows = rows_guard();
        self.entity()?
            .find(super::GIT_REPOSITORY)?
            .eq("repositoryId", Value::string(repository_id))
            .condition(moqui_core::entity::Condition::is_null("reviewHiddenDate"))
            .update_all([("reviewHiddenDate".to_string(), moqui_core::user::now_value())])?;
        Ok(())
    }

    /// Whether the current user reads the commit `id` of the repository by its id: one its readable refs reach; and,
    /// where the compatibility rule holds ([`Self::leftovers_readable`]), any commit of its store for a user with
    /// the update right (the commits of a deleted or forced branch, which the pages of who works in the repository
    /// still show). In a fork network the store is other repositories' too, and a store that has held a hidden merge
    /// request has commits nobody was to read (its replaced heads, its head once its ref is released), so there
    /// only the refs decide.
    pub(crate) fn readable_by_id(&self, repo: &Repo, id: &ObjectId) -> Result<bool, Error> {
        if self.reachable(repo, id)? {
            return Ok(true);
        }
        if !super::server::permitted(&repo.definition.id, "update") || !self.leftovers_readable(&repo.definition.id)? {
            return Ok(false);
        }
        Ok(matches!(repo.store.find(id)?, Some((Kind::Commit, _))))
    }
}

/// The trees (and tree of each commit) that one walk of [`objects_within`] reads at most, for a user who may push.
pub(crate) const WITHIN_MAX_TREES: usize = 1_000_000;

/// Which of `wanted` are the commits `roots`, their trees, or trees and blobs within those trees (the files of
/// those commits, not of their history): what a reader of those commits reads anyway. The walk stops when all are
/// found, and after `budget` trees.
pub(crate) fn objects_within(
    store: &dyn GitStore,
    roots: &[ObjectId],
    wanted: &HashSet<ObjectId>,
    budget: usize,
) -> Result<HashSet<ObjectId>, Error> {
    let mut found: HashSet<ObjectId> = HashSet::new();
    let mut seen: HashSet<ObjectId> = HashSet::new();
    let mut trees: Vec<ObjectId> = Vec::new();
    for chunk in roots.chunks(BATCH) {
        store.find_many(chunk, &mut |id, kind, data| {
            if kind != Kind::Commit {
                return Ok(());
            }
            if wanted.contains(&id) {
                found.insert(id);
            }
            let tree = data
                .strip_prefix(b"tree ")
                .and_then(|rest| rest.get(..40))
                .and_then(|hex| ObjectId::from_hex(hex).ok());
            if let Some(tree) = tree {
                if seen.insert(tree) {
                    trees.push(tree);
                }
            }
            Ok(())
        })?;
    }
    let mut read = 0;
    while !trees.is_empty() && found.len() < wanted.len() && read < budget {
        let at = trees.len().saturating_sub(BATCH);
        let batch = trees.split_off(at);
        read += batch.len();
        for id in &batch {
            if wanted.contains(id) {
                found.insert(*id);
            }
        }
        store.find_many(&batch, &mut |id, kind, data| {
            if kind != Kind::Tree {
                return Ok(());
            }
            let tree = gix::objs::TreeRef::from_bytes(data, super::ops::HASH)
                .map_err(|err| Error::new(format!("git tree {id}: {err}")))?;
            for entry in tree.entries {
                if entry.mode.is_commit() {
                    continue;
                }
                let object = entry.oid.to_owned();
                if wanted.contains(&object) {
                    found.insert(object);
                }
                if entry.mode.is_tree() && seen.insert(object) {
                    trees.push(object);
                }
            }
            Ok(())
        })?;
    }
    Ok(found)
}

/// Whether the commit `tip` is `target` or has it as an ancestor, reading at most `budget` commits (what is read is
/// taken from it): `None` when the budget ran out first. `not` holds commits known not to reach `target`, and takes
/// those this walk found, so asking of many tips reads each commit of the history once.
pub(crate) fn reaches(
    store: &dyn GitStore,
    target: &ObjectId,
    tip: &ObjectId,
    not: &mut HashSet<ObjectId>,
    budget: &mut usize,
) -> Result<Option<bool>, Error> {
    if tip == target {
        return Ok(Some(true));
    }
    if not.contains(tip) {
        return Ok(Some(false));
    }
    let mut visited: HashSet<ObjectId> = HashSet::from([*tip]);
    let mut pending = vec![*tip];
    while !pending.is_empty() {
        let at = pending.len().saturating_sub(BATCH);
        let batch = pending.split_off(at);
        if *budget < batch.len() {
            return Ok(None);
        }
        *budget -= batch.len();
        let mut found = false;
        store.find_many(&batch, &mut |_, kind, data| {
            if kind != Kind::Commit {
                return Ok(());
            }
            for parent in parents_of(data) {
                if parent == *target {
                    found = true;
                } else if !not.contains(&parent) && visited.insert(parent) {
                    pending.push(parent);
                }
            }
            Ok(())
        })?;
        if found {
            return Ok(Some(true));
        }
    }
    not.extend(visited);
    Ok(Some(false))
}

#[cfg(test)]
#[path = "reach_tests.rs"]
mod tests;
