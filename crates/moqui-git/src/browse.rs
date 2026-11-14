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

//! Reading a repository as git sees it, for the `get#Git*` services of `org.moqui.git.GitServices` and the commit
//! view: its branches and tags, a log, a commit with the files it changed and their diffs, a file at any revision.
//! The `git://` scheme reads the files of one branch; this reads any branch, tag or commit. Callers check
//! `AT_GIT_REPOSITORY` authorization; nothing here does.

use gix::objs::Kind;
use gix::ObjectId;

use super::diff::{self, FileChange, FileDiff};
use super::ops::{self, CommitDetails, TagDetails};
use super::revwalk::{self, Walk};
use super::GitProvider;
use moqui_core::context::Error;

/// The most commits one log call answers.
pub(crate) const MAX_LOG: usize = 500;

/// What kind of ref a name is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefKind {
    Branch,
    Tag,
    Other,
}

impl RefKind {
    pub(crate) fn of(name: &str) -> Self {
        if name.starts_with("refs/heads/") {
            Self::Branch
        } else if name.starts_with("refs/tags/") {
            Self::Tag
        } else {
            Self::Other
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::Tag => "tag",
            Self::Other => "other",
        }
    }
}

/// A ref's name without `refs/heads/` or `refs/tags/`.
pub(crate) fn short_name(name: &str) -> &str {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/tags/"))
        .unwrap_or(name)
}

/// One branch or tag.
#[derive(Clone, Debug)]
pub(crate) struct RefInfo {
    pub name: String,
    pub kind: RefKind,
    /// The object the ref points to: a commit, or an annotated tag.
    pub target: ObjectId,
    /// The commit it names, when it names one.
    pub commit: Option<CommitDetails>,
    /// The annotated tag it points to.
    pub tag: Option<TagDetails>,
}

/// A repository's refs and its default branch.
#[derive(Clone, Debug)]
pub(crate) struct RepositoryRefs {
    pub branch: String,
    pub head: Option<ObjectId>,
    pub refs: Vec<RefInfo>,
}

/// A page of a log.
#[derive(Clone, Debug)]
pub(crate) struct LogPage {
    pub commits: Vec<CommitDetails>,
    /// More commits follow the last one.
    pub more: bool,
}

impl GitProvider {
    /// The refs under `prefix` (`refs/` for all), each with the commit (and tag) it names.
    pub(crate) fn refs(&self, repository_id: &str, prefix: &str) -> Result<RepositoryRefs, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let mut refs = Vec::new();
        for (name, target) in store.list_refs(prefix)? {
            // the heads of merge requests are not branches or tags: get#MergeRequest gives each
            if name.starts_with(super::reach::MERGE_REQUEST_REFS) {
                continue;
            }
            let tag = match store.find(&target)? {
                Some((Kind::Tag, _)) => Some(ops::tag_details(store, &target)?),
                _ => None,
            };
            let commit = match revwalk::peel_to_commit(store, &target)? {
                Some(id) => Some(ops::commit_details(store, &id)?),
                None => None,
            };
            refs.push(RefInfo {
                kind: RefKind::of(&name),
                name,
                target,
                commit,
                tag,
            });
        }
        Ok(RepositoryRefs {
            head: store.read_ref(&ops::branch_ref(&repo.definition.branch))?,
            branch: repo.definition.branch.clone(),
            refs,
        })
    }

    /// The commit a revision names (see [`ops::resolve_revision`]), with tags peeled. A revision that is an object
    /// id, or the start of one, names a commit only when a ref the repository's readers read reaches it
    /// ([`super::reach`]): its store may hold the commits of other repositories of its fork network and the heads of
    /// hidden merge requests. The head ref of a hidden merge request names nothing either.
    pub(crate) fn resolve_commit(&self, repository_id: &str, revision: &str) -> Result<Option<ObjectId>, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let revision = revision.trim();
        let by_name = if revision.is_empty() || revision == "HEAD" {
            store.read_ref(&ops::branch_ref(&repo.definition.branch))?
        } else if revision.starts_with("refs/") {
            match store.read_ref(revision)? {
                Some(_)
                    if revision.starts_with(super::reach::MERGE_REQUEST_REFS)
                        && !super::reach::hidden_included()
                        && self.hidden_heads(repository_id)?.contains(revision) =>
                {
                    return Ok(None)
                }
                found => found,
            }
        } else {
            let mut found = store.read_ref(&ops::branch_ref(revision))?;
            if found.is_none() {
                found = store.read_ref(&format!("refs/tags/{revision}"))?;
            }
            found
        };
        if let Some(id) = by_name {
            return revwalk::peel_to_commit(store, &id);
        }
        if revision.starts_with("refs/") {
            return Ok(None);
        }
        if revision.len() == 40 {
            return match ObjectId::from_hex(revision.as_bytes()) {
                Ok(id) => match revwalk::peel_to_commit(store, &id)? {
                    Some(commit) if self.readable_by_id(&repo, &commit)? => Ok(Some(commit)),
                    _ => Ok(None),
                },
                Err(_) => Ok(None),
            };
        }
        if !super::store::is_hex_prefix(revision) {
            return Ok(None);
        }
        // the start of an id: of a commit the refs reach, so the answer (also "more than one") tells nothing of
        // what else is in the store
        let found = self.reachable_prefix(&repo, revision, 2)?;
        match found.as_slice() {
            [id] => Ok(Some(*id)),
            [] if self.leftovers_readable(repository_id)? => {
                // where the commits the refs have left are still read, by who may read those
                match ops::resolve_revision(store, &repo.definition.branch, revision) {
                    Ok(Some(id)) => match revwalk::peel_to_commit(store, &id)? {
                        Some(commit) if self.readable_by_id(&repo, &commit)? => Ok(Some(commit)),
                        _ => Ok(None),
                    },
                    _ => Ok(None),
                }
            }
            [] => Ok(None),
            _ => Err(Error::new(format!(
                "{revision} names more than one commit; give more digits"
            ))),
        }
    }

    /// The commits reachable from `revision`, newest first, that changed `path` (all commits for the empty path),
    /// after the commit `after` (a full id or a prefix), at most `limit`.
    pub(crate) fn log(
        &self,
        repository_id: &str,
        revision: &str,
        path: &[String],
        after: Option<&str>,
        limit: usize,
    ) -> Result<LogPage, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let limit = limit.clamp(1, MAX_LOG);
        let Some(start) = self.resolve_commit(repository_id, revision)? else {
            return Ok(LogPage {
                commits: Vec::new(),
                more: false,
            });
        };
        let mut walk = Walk::new(store);
        walk.push(start, false)?;
        let mut skipping = after.map(str::trim).filter(|after| !after.is_empty());
        let mut commits = Vec::new();
        while let Some(commit) = walk.next()? {
            if let Some(after) = skipping {
                if commit.id.to_string().starts_with(after) {
                    skipping = None;
                }
                continue;
            }
            if !path.is_empty() && !changed_path(store, &commit, path)? {
                continue;
            }
            if commits.len() == limit {
                return Ok(LogPage { commits, more: true });
            }
            commits.push(ops::commit_details(store, &commit.id)?);
        }
        Ok(LogPage { commits, more: false })
    }

    /// A commit and the files it changed against its first parent.
    pub(crate) fn commit_changes(
        &self,
        repository_id: &str,
        revision: &str,
    ) -> Result<Option<(CommitDetails, Vec<FileChange>)>, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let Some(id) = self.resolve_commit(repository_id, revision)? else {
            return Ok(None);
        };
        let details = ops::commit_details(store, &id)?;
        let changes = diff::commit_changes(store, &ops::read_commit(store, &id)?)?;
        Ok(Some((details, changes)))
    }

    /// The line diffs of the files a commit changed, or of the one at `path`.
    pub(crate) fn commit_diffs(
        &self,
        repository_id: &str,
        revision: &str,
        path: Option<&str>,
    ) -> Result<Option<Vec<(FileChange, FileDiff)>>, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let Some(id) = self.resolve_commit(repository_id, revision)? else {
            return Ok(None);
        };
        let mut diffs = Vec::new();
        for change in diff::commit_changes(store, &ops::read_commit(store, &id)?)? {
            if path.is_some_and(|path| path != change.path) {
                continue;
            }
            let file = diff::file_diff(store, &change)?;
            diffs.push((change, file));
        }
        Ok(Some(diffs))
    }

    /// The bytes of the file at `path` in the commit `revision` names.
    pub(crate) fn blob_at(
        &self,
        repository_id: &str,
        revision: &str,
        path: &[String],
    ) -> Result<Option<Vec<u8>>, Error> {
        let repo = self.repo(repository_id)?;
        match self.resolve_commit(repository_id, revision)? {
            Some(id) => {
                let commit = ops::read_commit(repo.store.as_ref(), &id)?;
                match ops::resolve(repo.store.as_ref(), &commit.tree, path)? {
                    Some((mode, blob)) if !mode.is_tree() => self.file_content(&repo, &blob).map(Some),
                    _ => Ok(None),
                }
            }
            None => Ok(None),
        }
    }
}

impl GitProvider {
    /// Commit files on a repository's default branch in one commit with `message`, as the current user: `puts` are
    /// paths and their new bytes, `removes` paths to delete (a directory goes with its last file). The new commit, or
    /// `None` when nothing changed.
    pub(crate) fn commit_files(
        &self,
        repository_id: &str,
        message: &str,
        puts: &[(Vec<String>, Vec<u8>)],
        removes: &[Vec<String>],
    ) -> Result<Option<ObjectId>, Error> {
        let repo = self.repo(repository_id)?;
        Self::writable(&repo, &format!("git://{repository_id}/"))?;
        let mut edits = Vec::new();
        for (path, bytes) in puts {
            if path.is_empty()
                || path
                    .iter()
                    .any(|segment| segment == "." || segment == ".." || segment == ".git")
            {
                return Err(Error::new(format!("bad path {}", path.join("/"))));
            }
            let blob = self.store_file(&repo, path, super::lfs_files::FileContent::Bytes(bytes))?;
            edits.push(super::ops::Edit::Put(
                path.clone(),
                gix::objs::tree::EntryKind::Blob,
                blob,
            ));
        }
        edits.extend(removes.iter().cloned().map(super::ops::Edit::Remove));
        if edits.is_empty() {
            return Ok(None);
        }
        let message = message.trim().to_string();
        self.commit(&repo, |_| Ok(Some((edits.clone(), message.clone()))))
    }
}

/// Whether `commit` changed what is at `path` against its first parent (a root commit changed it when it has it).
fn changed_path(store: &dyn super::store::GitStore, commit: &ops::CommitInfo, path: &[String]) -> Result<bool, Error> {
    let here = ops::resolve(store, &commit.tree, path)?.map(|(_, id)| id);
    let before = match commit.parents.first() {
        Some(parent) => ops::resolve(store, &ops::read_commit(store, parent)?.tree, path)?.map(|(_, id)| id),
        None => None,
    };
    Ok(here != before)
}
