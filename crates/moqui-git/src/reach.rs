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

/// At most this many repositories' sets are kept; one more forgets them all.
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

    /// Bring the set to what `tips` reach. A tip that went away is still reached when a commit of the set has it as
    /// a parent (the branch moved forward, or was merged before it was deleted): every commit of the set is then
    /// still an ancestor of a tip. Otherwise the set is made again.
    fn update(&mut self, store: &dyn GitStore, tips: &HashSet<ObjectId>, limit: usize) -> Result<(), Error> {
        if self.read.is_some() && self.tips == *tips {
            self.read = Some(Instant::now());
            return Ok(());
        }
        let gone = self
            .tips
            .iter()
            .any(|tip| !tips.contains(tip) && !self.commits.get(tip).copied().unwrap_or(false));
        if gone || self.full {
            self.commits.clear();
            self.full = false;
        }
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
        self.tips.clone_from(tips);
        self.read = Some(Instant::now());
        Ok(())
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

/// The sets of the repositories read by id on this node.
pub(crate) type ReachCache = Mutex<HashMap<String, Arc<Mutex<Reached>>>>;

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
        let mut cache = moqui_lib::sync::lock(&self.inner.reach);
        if cache.len() >= MAX_REPOSITORIES && !cache.contains_key(repository_id) {
            cache.clear();
        }
        Arc::clone(cache.entry(repository_id.to_string()).or_default())
    }

    /// Forget what the refs of a repository reach, when it is deleted or its storage replaced.
    pub(crate) fn forget_reached(&self, repository_id: &str) {
        moqui_lib::sync::lock(&self.inner.reach).remove(repository_id);
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
