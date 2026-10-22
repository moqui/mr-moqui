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

//! Hosting a repository through Moqui, for the services of `org.moqui.impl.GitServices` the code screens use: a
//! directory with each entry's last commit, comparing two revisions, writing refs (a branch, a tag, a delete) and
//! merging, each ref write under the repository's lock with the branch rules ([`super::rules`]) and the gitECA
//! events of source `write` (pre events in the write, post events from its `GitUpdate`, as for a push). Callers check
//! `AT_GIT_REPOSITORY` authorization; nothing here does.

use std::collections::HashMap;

use gix::objs::{Kind, WriteTo};
use gix::ObjectId;

use super::browse::short_name;
use super::diff::{self, FileChange, FileDiff};
use super::events::{self, RefUpdate, UpdateOrigin};
use super::merge::{self, Mergeability, Merged, Method, Overlay};
use super::ops::{self, Author, CommitDetails};
use super::revwalk::new_commits;
use super::rules::{self, RefChangeKind};
use super::store::{RefEdit, RefOutcome};
use super::{GitProvider, Repo, RepositoryKind};
use crate::context::Error;

/// The most commits a comparison lists, and the most files it diffs.
pub(crate) const MAX_COMPARE_COMMITS: usize = 250;
pub(crate) const MAX_COMPARE_FILES: usize = 300;

/// One entry of a directory at a revision.
#[derive(Clone, Debug)]
pub(crate) struct TreeEntry {
    pub name: String,
    pub path: String,
    /// file, executable, link, dir or submodule.
    pub kind: &'static str,
    pub id: ObjectId,
    /// The file's size (its content's, for one kept in LFS); none for a directory.
    pub size: Option<u64>,
    /// The LFS object a pointer file names.
    pub lfs_oid: Option<String>,
    /// The last commit that changed it, on the first-parent line of the revision.
    pub last: Option<CommitDetails>,
}

/// A directory at a revision.
#[derive(Clone, Debug)]
pub(crate) struct TreeListing {
    pub commit: ObjectId,
    /// The branch the revision names, when it names one.
    pub branch: Option<String>,
    pub entries: Vec<TreeEntry>,
    /// The path names a file, not a directory: no entries.
    pub is_file: bool,
}

/// Two revisions compared, as GitHub's compare: the commits in head and not in base, and the changes from their merge
/// base to head.
#[derive(Clone, Debug)]
pub(crate) struct Comparison {
    pub base: ObjectId,
    pub head: ObjectId,
    pub merge_base: Option<ObjectId>,
    pub ahead: usize,
    pub behind: usize,
    /// Oldest first, at most [`MAX_COMPARE_COMMITS`].
    pub commits: Vec<CommitDetails>,
    /// Each changed file, with its diff for the first [`MAX_COMPARE_FILES`].
    pub files: Vec<(FileChange, Option<FileDiff>)>,
    pub mergeability: Option<Mergeability>,
}

/// A ref to write: point `name` at `new` (`None` deletes it) when it points at `expected` now.
#[derive(Clone, Debug)]
pub(crate) struct RefWrite {
    pub name: String,
    pub expected: Option<ObjectId>,
    pub new: Option<ObjectId>,
}

/// What a merge did.
#[derive(Clone, Debug)]
pub(crate) enum MergeOutcome {
    /// A trial merge: whether it would merge.
    Checked(Mergeability),
    Merged(Merged),
    /// Refused, and why (a conflict lists its paths).
    Refused {
        reason: String,
        conflicts: Vec<String>,
    },
}

fn kind_of(mode: gix::objs::tree::EntryMode) -> &'static str {
    use gix::objs::tree::EntryKind;
    match mode.kind() {
        EntryKind::Tree => "dir",
        EntryKind::Blob => "file",
        EntryKind::BlobExecutable => "executable",
        EntryKind::Link => "link",
        EntryKind::Commit => "submodule",
    }
}

impl GitProvider {
    /// The branch a revision names, when it names one: empty or `HEAD` for the default branch, `refs/heads/<name>`, or
    /// a short name that is a branch.
    fn branch_of(repo: &Repo, revision: &str) -> Result<Option<String>, Error> {
        let revision = revision.trim();
        if revision.is_empty() || revision == "HEAD" {
            return Ok(Some(repo.definition.branch.clone()));
        }
        if let Some(branch) = revision.strip_prefix("refs/heads/") {
            return Ok(Some(branch.to_string()));
        }
        if revision.starts_with("refs/") {
            return Ok(None);
        }
        Ok(repo
            .store
            .read_ref(&ops::branch_ref(revision))?
            .map(|_| revision.to_string()))
    }

    /// The entries of the directory at `path` in `revision`, each with its last commit: from the path index of the
    /// branch the revision names, or of the commit itself. `None` when the revision or the path names nothing.
    pub(crate) fn tree(
        &self,
        repository_id: &str,
        revision: &str,
        path: &[String],
    ) -> Result<Option<TreeListing>, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let Some(commit_id) = self.resolve_commit(repository_id, revision)? else {
            return Ok(None);
        };
        let commit = ops::read_commit(store, &commit_id)?;
        let Some((mode, tree_id)) = ops::resolve(store, &commit.tree, path)? else {
            return Ok(None);
        };
        let branch = Self::branch_of(&repo, revision)?;
        if !mode.is_tree() {
            return Ok(Some(TreeListing {
                commit: commit_id,
                branch,
                entries: Vec::new(),
                is_file: true,
            }));
        }
        let tree = ops::read_tree(store, &tree_id)?;
        let prefix = path.join("/");
        let joined = |name: &str| {
            if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            }
        };
        let names: Vec<String> = tree
            .entries
            .iter()
            .map(|entry| joined(&String::from_utf8_lossy(&entry.filename)))
            .collect();
        // the last commit of each entry, from a path index of the branch, or of this commit
        let index_name = branch.clone().unwrap_or_else(|| format!("@{commit_id}"));
        let lasts: Vec<Option<ObjectId>> = self.indexed_at(&repo, &index_name, &commit, |index| {
            names
                .iter()
                .map(|name| index.last_change(name).map(|found| found.info.id))
                .collect()
        })?;
        let mut details: HashMap<ObjectId, CommitDetails> = HashMap::new();
        let mut entries = Vec::with_capacity(tree.entries.len());
        for ((entry, path), last) in tree.entries.iter().zip(names).zip(lasts) {
            let kind = kind_of(entry.mode);
            let mut size = None;
            let mut lfs_oid = None;
            if matches!(kind, "file" | "executable" | "link") {
                size = store.object_size(&entry.oid)?;
                if size.is_some_and(|size| size <= super::server::lfs::MAX_POINTER) {
                    if let Some((_, data)) = store.find(&entry.oid)? {
                        if let Some((oid, content_size)) = super::server::lfs::pointer(&data) {
                            lfs_oid = Some(oid);
                            size = Some(content_size);
                        }
                    }
                }
            }
            let last = match last {
                Some(id) => {
                    if let std::collections::hash_map::Entry::Vacant(slot) = details.entry(id) {
                        slot.insert(ops::commit_details(store, &id)?);
                    }
                    details.get(&id).cloned()
                }
                None => None,
            };
            entries.push(TreeEntry {
                name: String::from_utf8_lossy(&entry.filename).into_owned(),
                path,
                kind,
                id: entry.oid,
                size,
                lfs_oid,
                last,
            });
        }
        // directories first, then by name, as repository browsers list them
        entries.sort_by(|a, b| (a.kind != "dir", &a.name).cmp(&(b.kind != "dir", &b.name)));
        Ok(Some(TreeListing {
            commit: commit_id,
            branch,
            entries,
            is_file: false,
        }))
    }

    /// Compare `head` against `base` (both revisions). With `diffs` each file's line diff (else only its change);
    /// with `mergeable` a trial merge of head into base. `None` when either names no commit.
    pub(crate) fn compare(
        &self,
        repository_id: &str,
        base: &str,
        head: &str,
        diffs: bool,
        mergeable: bool,
    ) -> Result<Option<Comparison>, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let (Some(base_id), Some(head_id)) = (
            self.resolve_commit(repository_id, base)?,
            self.resolve_commit(repository_id, head)?,
        ) else {
            return Ok(None);
        };
        let merge_base = merge::merge_base(store, &base_id, &head_id)?;
        let ahead_commits = new_commits(store, &[head_id], &[base_id])?;
        let behind = new_commits(store, &[base_id], &[head_id])?.len();
        let ahead = ahead_commits.len();
        let mut commits = Vec::new();
        for commit in ahead_commits.iter().rev().take(MAX_COMPARE_COMMITS) {
            commits.push(ops::commit_details(store, &commit.id)?);
        }
        let from_tree = match &merge_base {
            Some(id) => Some(ops::read_commit(store, id)?.tree),
            None => None,
        };
        let to_tree = ops::read_commit(store, &head_id)?.tree;
        let changes = diff::tree_changes(store, from_tree.as_ref(), Some(&to_tree))?;
        let mut files = Vec::with_capacity(changes.len());
        for (position, change) in changes.into_iter().enumerate() {
            let file = if position < MAX_COMPARE_FILES {
                Some(diff::file_diff(store, &change)?)
            } else {
                None
            };
            files.push((change, file));
        }
        if !diffs {
            // the stats stay; the diff text goes
            for (_, file) in &mut files {
                if let Some(file) = file {
                    if let Some(text) = &mut file.diff {
                        for hunk in &mut text.hunks {
                            hunk.lines.clear();
                        }
                    }
                }
            }
        }
        let mergeability = if mergeable {
            Some(merge::check(store, &head_id, &base_id)?)
        } else {
            None
        };
        Ok(Some(Comparison {
            base: base_id,
            head: head_id,
            merge_base,
            ahead,
            behind,
            commits,
            files,
            mergeability,
        }))
    }

    /// Write refs as one change under the repository's lock: the branch rules (a direct change, unless it is a merge
    /// through a merge request), the pre events of source `write` (an error refuses the change), the change recorded
    /// as a `GitUpdate` with its post events scheduled. The objects of `overlay` are written to the store first. Each
    /// ref's outcome, in order: refused with why. An archived or mirror repository refuses every ref.
    pub(crate) fn write_refs(
        &self,
        repository_id: &str,
        writes: &[RefWrite],
        through_merge_request: bool,
        overlay: Option<&Overlay<'_>>,
    ) -> Result<Vec<Result<(), String>>, Error> {
        let repo = self.repo(repository_id)?;
        if repo.definition.kind == RepositoryKind::Remote {
            return Ok(vec![
                Err(format!("repository {repository_id} is a read-only mirror"));
                writes.len()
            ]);
        }
        if repo.definition.archived {
            return Ok(vec![
                Err(format!("repository {repository_id} is archived"));
                writes.len()
            ]);
        }
        let store = repo.store.as_ref();
        let entity = self.entity()?;
        let lock = self.lock_of(repository_id);
        let _turn = moqui_lib::sync::lock(&lock);
        if let Some(overlay) = overlay {
            overlay.flush()?;
        }
        let refs_before = store.list_refs("refs/")?;
        let mut results: Vec<Result<(), String>> = Vec::with_capacity(writes.len());
        let mut refs = Vec::with_capacity(writes.len());
        for write in writes {
            let current = refs_before
                .iter()
                .find(|(name, _)| *name == write.name)
                .map(|(_, id)| *id);
            let change_type = events::change_type(store, write.expected, write.new);
            refs.push(RefUpdate {
                name: write.name.clone(),
                old: write.expected,
                new: write.new,
                change_type: change_type.to_string(),
            });
            if current != write.expected {
                results.push(Err("stale info: the ref moved since it was read".to_string()));
                continue;
            }
            if write.new.is_none() && write.name == ops::branch_ref(&repo.definition.branch) {
                results.push(Err("the default branch cannot be deleted".to_string()));
                continue;
            }
            let found = rules::rules_for(&entity, repository_id, &write.name)?;
            let change = RefChangeKind::parse(change_type);
            let checked = if through_merge_request && change == RefChangeKind::Update {
                Ok(())
            } else {
                rules::check_direct(&found, &write.name, change, &rules::current_user_in)
            };
            results.push(checked);
        }
        let origin = UpdateOrigin {
            source: "write",
            transport: "internal",
            user_id: crate::user::current_user_id()
                .ok()
                .flatten()
                .filter(|id| !id.is_empty()),
            ..Default::default()
        };
        let candidates: Vec<usize> = (0..results.len()).filter(|index| results[*index].is_ok()).collect();
        if !candidates.is_empty() {
            let decided = events::pre_receive(
                store,
                repository_id,
                &repo.definition.branch,
                &origin,
                &refs,
                &candidates,
            );
            for message in &decided.messages {
                crate::context::ExecutionContext::with_current(|ec| ec.message().add_message(message.clone()));
            }
            if let Some(reason) = &decided.refused {
                for index in &candidates {
                    results[*index] = Err(format!("declined: {reason}"));
                }
            }
            for (index, reason) in &decided.refused_refs {
                results[*index] = Err(format!("declined: {reason}"));
            }
        }
        // one change: every ref or none
        if results.iter().any(Result::is_err) {
            for result in &mut results {
                if result.is_ok() {
                    *result = Err("not made: another ref of the change was refused".to_string());
                }
            }
        }
        let recorded: Vec<(RefUpdate, String)> = refs
            .iter()
            .zip(&results)
            .map(|(reference, result)| {
                (
                    reference.clone(),
                    match result {
                        Ok(()) => "pending".to_string(),
                        Err(reason) => reason.clone(),
                    },
                )
            })
            .collect();
        if results.iter().all(Result::is_err) {
            // refused before anything moved: one row for the push log, nothing to run after it
            self.record_refused(repository_id, &origin, &refs_before, &recorded);
            return Ok(results);
        }
        let update_id = self.begin_update(repository_id, &origin, &refs_before, &recorded);
        if results.iter().all(Result::is_ok) {
            let edits: Vec<RefEdit> = writes
                .iter()
                .map(|write| RefEdit {
                    name: write.name.clone(),
                    expected: write.expected,
                    new: write.new,
                })
                .collect();
            match store.update_refs(&edits, true) {
                Ok(outcomes) => {
                    for (result, outcome) in results.iter_mut().zip(outcomes) {
                        match outcome {
                            RefOutcome::Updated => {}
                            RefOutcome::Stale => *result = Err("stale info: the ref moved; try again".into()),
                            RefOutcome::Aborted => *result = Err("not made: another ref moved".into()),
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(repository = repository_id, "refs not updated: {err}");
                    for result in &mut results {
                        *result = Err(format!("failed to update ref: {}", err.message()));
                    }
                }
            }
        }
        if results.iter().any(Result::is_ok) {
            let branch_ref = ops::branch_ref(&repo.definition.branch);
            match writes.iter().find(|write| write.name == branch_ref) {
                Some(write) => self.remember_head(repository_id, write.new, None),
                None => self.remember_update(repository_id),
            }
            if repo.definition.kind != RepositoryKind::Db {
                self.refresh(repository_id);
            }
        }
        if let Some(update_id) = update_id {
            let outcomes: Vec<(String, String)> = writes
                .iter()
                .zip(&results)
                .map(|(write, result)| {
                    (
                        write.name.clone(),
                        match result {
                            Ok(()) => "ok".to_string(),
                            Err(reason) => reason.clone(),
                        },
                    )
                })
                .collect();
            self.finish_update(repository_id, &update_id, &outcomes);
        }
        Ok(results)
    }

    /// An annotated tag object for `target`, by the current user: its id.
    pub(crate) fn write_tag(
        &self,
        repository_id: &str,
        name: &str,
        target: ObjectId,
        message: &str,
    ) -> Result<ObjectId, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let target_kind = store
            .find(&target)?
            .map(|(kind, _)| kind)
            .ok_or_else(|| Error::new(format!("git object {target} is missing")))?;
        let tagger = self.author();
        let mut message = message.trim_end().to_string();
        message.push('\n');
        let tag = gix::objs::Tag {
            target,
            target_kind,
            name: name.into(),
            tagger: Some(gix::actor::Signature {
                name: tagger.name.as_str().into(),
                email: tagger.email.as_str().into(),
                time: gix::date::Time::now_local_or_utc(),
            }),
            message: message.into(),
            signature: None,
        };
        let mut buf = Vec::with_capacity(256);
        tag.write_to(&mut buf)
            .map_err(|err| Error::new(format!("encoding a git tag: {err}")))?;
        store.write(Kind::Tag, &buf)
    }

    /// The commit a revision names in a repository, with the full ref name of the branch or tag it names.
    pub(crate) fn ref_of(&self, repository_id: &str, name: &str) -> Result<Option<(String, ObjectId)>, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let names = if name.starts_with("refs/") {
            vec![name.to_string()]
        } else {
            vec![ops::branch_ref(name), format!("refs/tags/{name}")]
        };
        for candidate in names {
            if let Some(id) = store.read_ref(&candidate)? {
                return Ok(Some((candidate, id)));
            }
        }
        Ok(None)
    }

    /// Merge `source` (a revision) into the branch `target`, by `method`. With `check_only` a trial merge that changes
    /// nothing. The merge's commits are by the current user (a squash's author `squash_author` when given); the target
    /// moves through [`Self::write_refs`], so the branch rules and gitECA events run as for a push.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn merge_refs(
        &self,
        repository_id: &str,
        source: &str,
        target: &str,
        method: Method,
        message: Option<&str>,
        squash_author: Option<Author>,
        check_only: bool,
        through_merge_request: bool,
    ) -> Result<MergeOutcome, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let target_ref = if target.starts_with("refs/") {
            target.to_string()
        } else {
            ops::branch_ref(target)
        };
        if !target_ref.starts_with("refs/heads/") {
            return Err(Error::new(format!("{target} is not a branch")));
        }
        let Some(target_id) = store.read_ref(&target_ref)? else {
            return Err(Error::new(format!(
                "no branch {} in repository {repository_id}",
                short_name(&target_ref)
            )));
        };
        let Some(source_id) = self.resolve_commit(repository_id, source)? else {
            return Err(Error::new(format!(
                "{source} names no commit in repository {repository_id}"
            )));
        };
        if check_only {
            return Ok(MergeOutcome::Checked(merge::check(store, &source_id, &target_id)?));
        }
        let committer = self.author();
        let overlay = Overlay::new(store);
        let source_name = short_name(source).to_string();
        let target_name = short_name(&target_ref).to_string();
        let request = merge::MergeRequest {
            method,
            source: source_id,
            target: target_id,
            source_name: &source_name,
            target_name: &target_name,
            message,
            committer: &committer,
            squash_author: squash_author.as_ref(),
        };
        let merged = match merge::merge(&overlay, &request)? {
            Ok(merged) => merged,
            Err(refused) => {
                let conflicts = match &refused {
                    merge::Refused::Conflicts { paths, .. } => paths.clone(),
                    _ => Vec::new(),
                };
                return Ok(MergeOutcome::Refused {
                    reason: refused.message(&source_name, &target_name),
                    conflicts,
                });
            }
        };
        let written = self.write_refs(
            repository_id,
            &[RefWrite {
                name: target_ref,
                expected: Some(target_id),
                new: Some(merged.head),
            }],
            through_merge_request,
            Some(&overlay),
        )?;
        match written.into_iter().next() {
            Some(Ok(())) => Ok(MergeOutcome::Merged(merged)),
            Some(Err(reason)) => Ok(MergeOutcome::Refused {
                reason,
                conflicts: Vec::new(),
            }),
            None => Err(Error::new("no ref written")),
        }
    }

    /// A write through `git://` moved a branch that has branch rules: it passes (the user had the update right where
    /// the write was reached) and is logged.
    pub(super) fn log_protected_write(&self, repo: &Repo, commit: ObjectId) {
        let Ok(entity) = self.entity() else { return };
        let name = ops::branch_ref(&repo.definition.branch);
        match rules::rules_for(&entity, &repo.definition.id, &name) {
            Ok(found) if !found.is_empty() => tracing::info!(
                repository = %repo.definition.id,
                branch = %repo.definition.branch,
                commit = %commit,
                user = crate::user::current_user_id().ok().flatten().unwrap_or_default(),
                "a write through Moqui moved a protected branch"
            ),
            Ok(_) => {}
            Err(err) => tracing::warn!(repository = %repo.definition.id, "branch rules not read: {err}"),
        }
    }

    /// Make `branch` the default branch: the row, and HEAD of a repository on disk.
    pub(crate) fn change_default_branch(&self, repository_id: &str, branch: &str) {
        self.set_default_branch(repository_id, branch);
    }

    /// Forget a repository's row and its open store, after its row changed (archived, a new default branch).
    pub(crate) fn row_changed(&self, repository_id: &str) {
        self.refresh(repository_id);
    }
}
