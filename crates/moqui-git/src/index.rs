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

//! The path index of a branch: each commit on its first-parent line with the files it changed (against its first
//! parent), and for each path (a file, or a directory with anything under it) the last commit that changed it. It is
//! built once per branch head and extended in place when the head moves forward, so a file list's modified dates and
//! users, a file's history (which follows an exact move: the same blob removed at one path and added at another in one
//! commit) and the log of commits under a directory read memory rather than walking the branch from its head on each
//! call. Indexes are kept per repository and branch, bounded in number and in bytes ([`super::GitProvider`]).

use std::collections::HashMap;
use std::sync::Arc;

use gix::objs::tree::EntryMode;
use gix::ObjectId;

use super::ops::{self, CommitInfo};
use super::store::GitStore;
use moqui_core::context::{Error, ResourceVersion};

/// How many commits an index holds; older history of a longer branch is not indexed (its files' last change is then
/// the oldest indexed commit, and their history stops there).
pub(crate) const MAX_INDEX_COMMITS: usize = 200_000;

/// A file that a commit changed: its blob before (`None`: added) and after (`None`: removed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileChange {
    pub path: String,
    pub old: Option<ObjectId>,
    pub new: Option<ObjectId>,
}

/// A commit of the first-parent line and the files it changed.
#[derive(Debug)]
pub(crate) struct IndexedCommit {
    pub info: CommitInfo,
    pub changes: Vec<FileChange>,
}

impl IndexedCommit {
    /// As a version: the commit id, date, `Moqui-User-Id` and message (the note).
    pub(crate) fn version(&self, previous: Option<&IndexedCommit>) -> ResourceVersion {
        ResourceVersion {
            version_name: self.info.id.to_string(),
            previous_version_name: previous.map(|older| older.info.id.to_string()),
            version_date: ops::version_date(self.info.seconds),
            user_id: ops::user_of(&self.info.message),
            note: ops::note_of(&self.info.message),
        }
    }
}

/// One version of a file in its history: the commit and the file's blob there.
#[derive(Clone, Debug)]
pub(crate) struct FileVersion {
    pub commit: usize,
    pub blob: ObjectId,
}

/// The index of one branch at `head`.
#[derive(Debug)]
pub(crate) struct BranchIndex {
    pub head: ObjectId,
    /// Oldest first, so extending the index keeps the positions `last` points to.
    commits: Vec<Arc<IndexedCommit>>,
    /// Path (a file, or a directory: "" is the root) to the position of the last commit that changed it.
    last: HashMap<String, usize>,
    /// The branch has more commits than were indexed.
    pub truncated: bool,
    /// A rough size, for the bound on the cache.
    pub bytes: usize,
}

/// The files that differ between two trees (`None`: no tree), under `prefix`.
pub(crate) fn diff_trees(
    store: &dyn GitStore,
    old: Option<&ObjectId>,
    new: Option<&ObjectId>,
    prefix: &str,
    out: &mut Vec<FileChange>,
) -> Result<(), Error> {
    if old == new {
        return Ok(());
    }
    let entries = |id: Option<&ObjectId>| -> Result<Vec<(String, EntryMode, ObjectId)>, Error> {
        match id {
            Some(id) => Ok(ops::read_tree(store, id)?
                .entries
                .into_iter()
                .map(|entry| {
                    (
                        String::from_utf8_lossy(&entry.filename).into_owned(),
                        entry.mode,
                        entry.oid,
                    )
                })
                .collect()),
            None => Ok(Vec::new()),
        }
    };
    let (old_entries, new_entries) = (entries(old)?, entries(new)?);
    let old_map: HashMap<&str, (EntryMode, ObjectId)> = old_entries
        .iter()
        .map(|(name, mode, id)| (name.as_str(), (*mode, *id)))
        .collect();
    let new_map: HashMap<&str, (EntryMode, ObjectId)> = new_entries
        .iter()
        .map(|(name, mode, id)| (name.as_str(), (*mode, *id)))
        .collect();
    let mut names: Vec<&str> = old_map.keys().chain(new_map.keys()).copied().collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        // a submodule entry names a commit of another repository; it has no file here
        let old = old_map.get(name).copied().filter(|(mode, _)| !mode.is_commit());
        let new = new_map.get(name).copied().filter(|(mode, _)| !mode.is_commit());
        if old.map(|(_, id)| id) == new.map(|(_, id)| id) {
            continue;
        }
        let tree = |entry: Option<(EntryMode, ObjectId)>| entry.filter(|(mode, _)| mode.is_tree()).map(|(_, id)| id);
        let blob = |entry: Option<(EntryMode, ObjectId)>| entry.filter(|(mode, _)| !mode.is_tree()).map(|(_, id)| id);
        let (old_tree, new_tree) = (tree(old), tree(new));
        if old_tree.is_some() || new_tree.is_some() {
            diff_trees(store, old_tree.as_ref(), new_tree.as_ref(), &path, out)?;
        }
        let (old_blob, new_blob) = (blob(old), blob(new));
        if old_blob != new_blob {
            out.push(FileChange {
                path,
                old: old_blob,
                new: new_blob,
            });
        }
    }
    Ok(())
}

/// Where a [`walk`] ended.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WalkEnd {
    /// At `stop`.
    Stop,
    /// At `max` commits, before `stop` or the root.
    Max,
    /// At the root commit.
    Root,
}

/// Commits from `from` along the first-parent line, newest first, until `stop` (not included) or the root, or `max`;
/// and which of them it ended at.
fn walk(
    store: &dyn GitStore,
    from: &ObjectId,
    stop: Option<&ObjectId>,
    max: usize,
) -> Result<(Vec<IndexedCommit>, WalkEnd), Error> {
    let mut found = Vec::new();
    let mut current = ops::read_commit(store, from)?;
    loop {
        if Some(&current.id) == stop {
            return Ok((found, WalkEnd::Stop));
        }
        if found.len() >= max {
            return Ok((found, WalkEnd::Max));
        }
        let parent = match current.parents.first() {
            Some(id) => Some(ops::read_commit(store, id)?),
            None => None,
        };
        let mut changes = Vec::new();
        diff_trees(
            store,
            parent.as_ref().map(|parent| &parent.tree),
            Some(&current.tree),
            "",
            &mut changes,
        )?;
        found.push(IndexedCommit { info: current, changes });
        match parent {
            Some(parent) => current = parent,
            None => return Ok((found, WalkEnd::Root)),
        }
    }
}

fn commit_bytes(commit: &IndexedCommit) -> usize {
    160 + commit.info.message.len()
        + commit
            .changes
            .iter()
            .map(|change| 64 + change.path.len())
            .sum::<usize>()
}

impl BranchIndex {
    /// The index of the branch whose head is `head`.
    pub(crate) fn build(store: &dyn GitStore, head: &ObjectId) -> Result<Self, Error> {
        let (mut commits, end) = walk(store, head, None, MAX_INDEX_COMMITS)?;
        commits.reverse();
        let mut index = Self {
            head: *head,
            commits: Vec::with_capacity(commits.len()),
            last: HashMap::new(),
            truncated: end == WalkEnd::Max,
            bytes: 0,
        };
        for commit in commits {
            index.push(commit);
        }
        Ok(index)
    }

    fn push(&mut self, commit: IndexedCommit) {
        let position = self.commits.len();
        self.bytes += commit_bytes(&commit);
        for change in &commit.changes {
            let mut path = change.path.as_str();
            loop {
                if let Some(existing) = self.last.get_mut(path) {
                    *existing = position;
                } else {
                    self.bytes += 48 + path.len();
                    self.last.insert(path.to_string(), position);
                }
                if path.is_empty() {
                    break;
                }
                path = path.rsplit_once('/').map_or("", |(parent, _)| parent);
            }
        }
        self.head = commit.info.id;
        self.commits.push(Arc::new(commit));
    }

    /// Bring the index to `head`: the new commits when the old head is on its first-parent line, else a new index (the
    /// branch was replaced, as a fetch of a rewritten remote does).
    pub(crate) fn advance(&mut self, store: &dyn GitStore, head: &ObjectId) -> Result<(), Error> {
        if self.head == *head {
            return Ok(());
        }
        let room = MAX_INDEX_COMMITS.saturating_sub(self.commits.len()).max(1);
        let (mut commits, end) = walk(store, head, Some(&self.head), room)?;
        if end != WalkEnd::Stop {
            *self = Self::build(store, head)?;
            return Ok(());
        }
        commits.reverse();
        for commit in commits {
            self.push(commit);
        }
        Ok(())
    }

    fn at(&self, position: usize) -> &IndexedCommit {
        &self.commits[position]
    }

    /// The last commit that changed `path` (a file, or anything under a directory; "" is the whole branch). A path
    /// older than the indexed commits of a truncated index answers the oldest one.
    pub(crate) fn last_change(&self, path: &str) -> Option<&IndexedCommit> {
        match self.last.get(path) {
            Some(position) => Some(self.at(*position)),
            None if self.truncated => self.commits.first().map(Arc::as_ref),
            None => None,
        }
    }

    /// The versions of the file at `path`, newest first: each commit whose copy of the file differs from its parent's
    /// (a commit that removes it is not one). The history follows a move back to the path the file had before: a
    /// commit that adds the file with the blob another path of the same commit removed.
    pub(crate) fn history(&self, path: &str) -> Vec<FileVersion> {
        let mut tracking = path.to_string();
        let mut found = Vec::new();
        for position in (0..self.commits.len()).rev() {
            let commit = self.at(position);
            let Some(change) = commit.changes.iter().find(|change| change.path == tracking) else {
                continue;
            };
            let Some(blob) = change.new else {
                continue;
            };
            found.push(FileVersion { commit: position, blob });
            if change.old.is_none() {
                if let Some(source) = commit
                    .changes
                    .iter()
                    .find(|other| other.new.is_none() && other.old == Some(blob) && other.path != tracking)
                {
                    tracking = source.path.clone();
                }
            }
        }
        found
    }

    /// [`Self::history`] as versions, each one's previous version the next older.
    pub(crate) fn versions(&self, path: &str) -> Vec<(ResourceVersion, FileVersion)> {
        let history = self.history(path);
        history
            .iter()
            .enumerate()
            .map(|(index, version)| {
                let previous = history.get(index + 1).map(|older| self.at(older.commit));
                (self.at(version.commit).version(previous), version.clone())
            })
            .collect()
    }

    /// The commits that changed anything at or under `path`, newest first, at most `limit`, each with the paths it
    /// changed there (relative to `path`; empty for the file itself).
    pub(crate) fn log(&self, path: &str, limit: usize) -> Vec<(ResourceVersion, Vec<String>)> {
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        let mut found = Vec::new();
        for position in (0..self.commits.len()).rev() {
            if found.len() >= limit {
                break;
            }
            let commit = self.at(position);
            let paths: Vec<String> = commit
                .changes
                .iter()
                .filter_map(|change| {
                    if path.is_empty() {
                        Some(change.path.clone())
                    } else if change.path == path {
                        Some(String::new())
                    } else {
                        change.path.strip_prefix(&prefix).map(str::to_string)
                    }
                })
                .collect();
            if paths.is_empty() {
                continue;
            }
            let previous = position.checked_sub(1).map(|older| self.at(older));
            found.push((commit.version(previous), paths));
        }
        found
    }
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
