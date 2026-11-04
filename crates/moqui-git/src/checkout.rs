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

//! A commit's files written to a plain directory, and the paths two commits differ by (plans/Component-Deploy.md,
//! Phase 3): a component deployed from a repository loads from those files, as components load from files, and an
//! update of it reloads the paths that changed. The repository is a `db` or `remote` GitRepository, or a bare
//! repository directory a branch was fetched into.

use std::path::{Path, PathBuf};

use gix::objs::Kind;
use gix::ObjectId;
use moqui_core::context::Error;

use super::store::GitStore;
use super::{fs, ops, revwalk, GitProvider};

/// What [`GitProvider::write_tree`] wrote.
#[derive(Clone, Debug, Default)]
pub struct WrittenTree {
    /// The commit written, as a full id.
    pub commit: String,
    pub files: usize,
    /// What was not written, by path: a symbolic link or a submodule.
    pub skipped: Vec<String>,
    /// Git LFS pointer files, written as the pointers they are.
    pub lfs_pointers: Vec<String>,
}

/// HTTPS credentials for [`GitProvider::fetch_branch`].
#[derive(Clone, Debug, Default)]
pub struct FetchCredentials {
    pub username: Option<String>,
    pub token: Option<String>,
}

impl GitProvider {
    /// Fetch `branch` of `url` into a new bare repository at `path`, for a deploy to read before it keeps it (a `db`
    /// repository imports it, a `remote` one is moved into place). Answers the branch's head. Refused in sim mode
    /// and by the remote call fence, as every fetch is.
    ///
    /// # Errors
    ///
    /// When the fetch fails or the branch has no commits.
    pub fn fetch_branch(
        &self,
        path: &Path,
        url: &str,
        branch: &str,
        credentials: &FetchCredentials,
    ) -> Result<String, Error> {
        let redacted = moqui_core::outbound::redacted(url);
        if self.entity()?.sim_active() {
            return Err(Error::new(format!(
                "fetch of {redacted} denied because sim mode is active"
            )));
        }
        moqui_core::entity::fence::check_effect(
            moqui_core::entity::fence::Effect::RemoteCall,
            &format!("fetch of {redacted}"),
        )?;
        let credentials = fs::Credentials {
            username: credentials.username.clone(),
            token: credentials.token.clone(),
        };
        let outcome = fs::fetch(path, url, branch, &credentials, false)?;
        outcome
            .head
            .map(|head| head.to_string())
            .ok_or_else(|| Error::new(format!("{redacted} has no branch {branch}")))
    }

    /// Write the files of `revision` (a commit id or a prefix of one; the head of the repository's branch when
    /// `None`) of the repository `repository_id` into `target`, which must not exist.
    ///
    /// # Errors
    ///
    /// When the repository or the revision is not found, `target` exists, or a file cannot be written.
    pub fn write_tree(&self, repository_id: &str, revision: Option<&str>, target: &Path) -> Result<WrittenTree, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        write_commit(store, &repo.definition.branch, revision, target)
    }

    /// [`Self::write_tree`] from the bare repository at `path` (a fetched branch, [`Self::fetch_branch`]).
    ///
    /// # Errors
    ///
    /// As [`Self::write_tree`].
    pub fn write_tree_at(
        path: &Path,
        branch: &str,
        revision: Option<&str>,
        target: &Path,
    ) -> Result<WrittenTree, Error> {
        let store = fs::FsStore::open(path)?;
        write_commit(&store, branch, revision, target)
    }

    /// The file at `file` (a `/` separated path) in the head of `branch` of the bare repository at `path`: a deploy
    /// reads the `component.xml` of what it fetched.
    ///
    /// # Errors
    ///
    /// When the repository or the branch is not found.
    pub fn read_head_file(path: &Path, branch: &str, file: &str) -> Result<Option<Vec<u8>>, Error> {
        let store = fs::FsStore::open(path)?;
        let head = ops::head(&store, branch)?.ok_or_else(|| Error::new(format!("no branch {branch}")))?;
        let segments: Vec<String> = file.split('/').map(str::to_string).collect();
        ops::read_at(&store, &head.id, &segments)
    }

    /// The head of the repository's branch, as a full id; `None` when it has no commits.
    ///
    /// # Errors
    ///
    /// When the repository is not found.
    pub fn branch_head(&self, repository_id: &str) -> Result<Option<String>, Error> {
        let repo = self.repo(repository_id)?;
        let head = repo.store.read_ref(&ops::branch_ref(&repo.definition.branch))?;
        Ok(head.map(|id| id.to_string()))
    }

    /// The paths of the files that differ from the commit `from` to the commit `to` of the repository, by path.
    ///
    /// # Errors
    ///
    /// When the repository or a commit is not found.
    pub fn changed_paths(&self, repository_id: &str, from: &str, to: &str) -> Result<Vec<String>, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let tree_of = |revision: &str| -> Result<ObjectId, Error> {
            let commit = commit_of(store, &repo.definition.branch, Some(revision))?;
            Ok(ops::read_commit(store, &commit)?.tree)
        };
        let (old, new) = (tree_of(from)?, tree_of(to)?);
        let changes = super::diff::tree_changes(store, Some(&old), Some(&new))?;
        Ok(changes.into_iter().map(|change| change.path).collect())
    }
}

/// The commit `revision` names, else the head of `branch`.
fn commit_of(store: &dyn GitStore, branch: &str, revision: Option<&str>) -> Result<ObjectId, Error> {
    let found = match revision.map(str::trim).filter(|revision| !revision.is_empty()) {
        Some(revision) => match ops::resolve_revision(store, branch, revision)? {
            Some(id) => revwalk::peel_to_commit(store, &id)?,
            None => None,
        },
        None => store.read_ref(&ops::branch_ref(branch))?,
    };
    found.ok_or_else(|| Error::new(format!("no commit {} on branch {branch}", revision.unwrap_or("head"))))
}

fn write_commit(
    store: &dyn GitStore,
    branch: &str,
    revision: Option<&str>,
    target: &Path,
) -> Result<WrittenTree, Error> {
    if target.exists() {
        return Err(Error::new(format!("{} exists already", target.display())));
    }
    let commit = commit_of(store, branch, revision)?;
    let tree = ops::read_commit(store, &commit)?.tree;
    let mut written = WrittenTree {
        commit: commit.to_string(),
        ..WrittenTree::default()
    };
    std::fs::create_dir_all(target).map_err(|err| io(target, &err))?;
    let mut pending: Vec<(ObjectId, PathBuf, String)> = vec![(tree, target.to_path_buf(), String::new())];
    while let Some((tree, dir, prefix)) = pending.pop() {
        for entry in ops::read_tree(store, &tree)?.entries {
            let name = entry.filename.to_string();
            if name == ".git" || name == "." || name == ".." || name.contains('/') {
                written.skipped.push(format!("{prefix}{name}"));
                continue;
            }
            let path = dir.join(&name);
            let relative = format!("{prefix}{name}");
            if entry.mode.is_tree() {
                std::fs::create_dir_all(&path).map_err(|err| io(&path, &err))?;
                pending.push((entry.oid, path, format!("{relative}/")));
            } else if entry.mode.is_link() || entry.mode.is_commit() {
                written.skipped.push(relative);
            } else {
                let bytes = ops::object(store, &entry.oid, Kind::Blob)?;
                if bytes.starts_with(b"version https://git-lfs.github.com/spec/") {
                    written.lfs_pointers.push(relative.clone());
                }
                std::fs::write(&path, &bytes).map_err(|err| io(&path, &err))?;
                #[cfg(unix)]
                if entry.mode.is_executable() {
                    use std::os::unix::fs::PermissionsExt;
                    let permissions = std::fs::Permissions::from_mode(0o755);
                    std::fs::set_permissions(&path, permissions).map_err(|err| io(&path, &err))?;
                }
                written.files += 1;
            }
        }
    }
    Ok(written)
}

fn io(path: &Path, err: &std::io::Error) -> Error {
    Error::new(format!("{}: {err}", path.display()))
}

#[cfg(test)]
#[path = "checkout_tests.rs"]
mod tests;
