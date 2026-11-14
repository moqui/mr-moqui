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

//! A commit of files made through a service, `commit#GitFiles` (docs/interfaces/Git-Hosting.md, Commits through
//! Moqui; plans/Dogfood.md N-COMMIT): to a branch, to a new branch made from one, or as a proposal with no branch
//! (the commit a merge request's head, as a push to `refs/for/<branch>` leaves it). A branch moves through
//! [`GitProvider::write_refs`], so the branch rules and the gitECA events are a push's.

use gix::objs::tree::EntryKind;
use gix::ObjectId;

use super::lfs_files::FileContent;
use super::ops::{self, Author, Edit};
use super::{events, merge, store, GitProvider, RepositoryKind};
use moqui_core::context::Error;

/// What `commit#GitFiles` asks for.
#[derive(Debug, Default)]
pub(crate) struct FileCommit<'a> {
    /// The branch committed to, or that the new branch or the proposal starts from; the default branch when empty.
    pub branch: Option<&'a str>,
    /// Make this branch at `branch`'s head and commit there.
    pub new_branch: Option<&'a str>,
    /// Keep the commit for a merge request into `branch`, moving no branch.
    pub proposal: bool,
    pub message: &'a str,
    /// Paths and their new content.
    pub puts: &'a [(Vec<String>, Vec<u8>)],
    pub removes: &'a [Vec<String>],
    /// Paths moved, from and to.
    pub renames: &'a [(Vec<String>, Vec<String>)],
    /// The blob each path had when the edit began (`None`: no file there): another now is a conflict.
    pub expected: &'a [(Vec<String>, Option<ObjectId>)],
    /// The author the application gives, by name and address; the committer is then the server.
    pub author: Option<(&'a str, &'a str)>,
    /// The user has no push right (a proposal by a reader): no file kept in Git LFS is stored, and the bytes count
    /// as pushed for review.
    pub viewer: bool,
}

/// A path whose blob is not the one an edit began from.
#[derive(Debug)]
pub(crate) struct BlobConflict {
    pub path: String,
    pub expected: Option<ObjectId>,
    pub current: Option<ObjectId>,
}

/// What became of a [`FileCommit`].
#[derive(Debug)]
pub(crate) enum FileCommitOutcome {
    /// The tree is the head's: nothing to commit.
    Unchanged,
    /// The branch `ref_name` is at `commit`.
    Committed {
        commit: ObjectId,
        ref_name: String,
    },
    /// `commit` is in the store for a merge request into the branch `target`; no ref moved.
    Proposed {
        commit: ObjectId,
        target: String,
    },
    Conflict(Vec<BlobConflict>),
    /// Refused, with why: by the branch rules, a gitECA rule, a ref that is there or is not.
    Refused(String),
}

fn bad_path(path: &[String]) -> bool {
    path.is_empty()
        || path.iter().any(|segment| {
            segment.is_empty() || segment == "." || segment == ".." || segment.eq_ignore_ascii_case(".git")
        })
}

impl GitProvider {
    /// The committer of a commit whose author the application gave: the server (`git-server` `committer-name` and
    /// `committer-email`, else `Moqui` at the host), with the current user in the `Moqui-User-Id` trailer.
    fn server_committer(&self) -> Author {
        let name = Some(super::setting("committer-name"))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Moqui".to_string());
        let email = Some(super::setting("committer-email"))
            .filter(|email| !email.is_empty())
            .unwrap_or_else(|| "moqui@localhost".to_string());
        Author {
            name,
            email,
            user_id: None,
            seconds: None,
        }
    }

    /// Make the commit of `request` in repository `repository_id` (see the module). The caller checked the user's
    /// right: update for a branch, create for a new one, view for a proposal.
    pub(crate) fn commit_change(
        &self,
        repository_id: &str,
        request: &FileCommit<'_>,
    ) -> Result<FileCommitOutcome, Error> {
        let repo = self.repo(repository_id)?;
        if repo.definition.kind == RepositoryKind::Remote {
            return Ok(FileCommitOutcome::Refused(format!(
                "repository {repository_id} is a read-only mirror"
            )));
        }
        if repo.definition.archived {
            return Ok(FileCommitOutcome::Refused(format!(
                "repository {repository_id} is archived"
            )));
        }
        let store = repo.store.as_ref();
        let branch = request
            .branch
            .map(|branch| branch.strip_prefix("refs/heads/").unwrap_or(branch))
            .filter(|branch| !branch.is_empty())
            .unwrap_or(&repo.definition.branch);
        let base_ref = ops::branch_ref(branch);
        let head = store.read_ref(&base_ref)?;
        // only the first commit of an empty repository has no branch to start from
        if head.is_none()
            && (request.branch.is_some()
                || request.new_branch.is_some()
                || request.proposal
                || !request.expected.is_empty())
        {
            return Ok(FileCommitOutcome::Refused(format!(
                "no branch {branch} in repository {repository_id}"
            )));
        }
        let head_tree = match &head {
            Some(head) => Some(ops::read_commit(store, head)?.tree),
            None => None,
        };
        let entry_at = |path: &[String]| match &head_tree {
            Some(tree) => ops::resolve(store, tree, path),
            None => Ok(None),
        };
        let mut conflicts = Vec::new();
        for (path, expected) in request.expected {
            let current = entry_at(path)?.filter(|(mode, _)| !mode.is_tree()).map(|(_, id)| id);
            if current != *expected {
                conflicts.push(BlobConflict {
                    path: path.join("/"),
                    expected: *expected,
                    current,
                });
            }
        }
        if !conflicts.is_empty() {
            return Ok(FileCommitOutcome::Conflict(conflicts));
        }
        let mut edits = Vec::new();
        for (from, to) in request.renames {
            if bad_path(from) || bad_path(to) {
                return Err(Error::new(format!("bad path {} or {}", from.join("/"), to.join("/"))));
            }
            let Some((mode, id)) = entry_at(from)? else {
                return Ok(FileCommitOutcome::Refused(format!(
                    "no file {} in {branch} to rename",
                    from.join("/")
                )));
            };
            if entry_at(to)?.is_some() {
                return Ok(FileCommitOutcome::Refused(format!(
                    "{} is there already in {branch}",
                    to.join("/")
                )));
            }
            edits.push(Edit::Remove(from.clone()));
            edits.push(Edit::Put(to.clone(), mode.kind(), id));
        }
        for path in request.removes {
            if bad_path(path) {
                return Err(Error::new(format!("bad path {}", path.join("/"))));
            }
            edits.push(Edit::Remove(path.clone()));
        }
        for (path, bytes) in request.puts {
            if bad_path(path) {
                return Err(Error::new(format!("bad path {}", path.join("/"))));
            }
            if request.viewer && self.lfs_tracked(&repo, path)? {
                return Ok(FileCommitOutcome::Refused(format!(
                    "{} is kept in Git LFS: push it with git, to a branch you may push to",
                    path.join("/")
                )));
            }
            let kind = match entry_at(path)? {
                Some((mode, _)) if mode.is_tree() => {
                    return Ok(FileCommitOutcome::Refused(format!(
                        "{} is a directory in {branch}",
                        path.join("/")
                    )));
                }
                Some((mode, _)) if mode.is_executable() => EntryKind::BlobExecutable,
                _ => EntryKind::Blob,
            };
            let blob = self.store_file(&repo, path, FileContent::Bytes(bytes))?;
            edits.push(Edit::Put(path.clone(), kind, blob));
        }
        if edits.is_empty() {
            return Ok(FileCommitOutcome::Unchanged);
        }
        let tree = ops::apply_edits(store, head_tree.as_ref(), &edits)?;
        if head_tree == Some(tree) {
            return Ok(FileCommitOutcome::Unchanged);
        }
        // the author the application gives (an account's name and confirmed address), committed by the server; else
        // the current user is both
        let user = self.author();
        let (author, committer) = match request.author {
            Some((name, email)) => (
                Author {
                    name: name.to_string(),
                    email: email.to_string(),
                    user_id: user.user_id.clone(),
                    seconds: None,
                },
                self.server_committer(),
            ),
            None => (user.clone(), user),
        };
        let parents: Vec<ObjectId> = head.into_iter().collect();
        let commit = merge::write_commit(
            store,
            tree,
            &parents,
            merge::author_signature(&author),
            merge::author_signature(&committer),
            &merge::with_user_trailer(request.message, &author),
        )?;
        if request.proposal {
            // no ref moves: the merge request's head is set by propose#MergeRequest. A write, for the push log and
            // the rules that follow one, named as a push for review names it
            let prior = store.list_refs("refs/")?;
            let origin = events::UpdateOrigin {
                source: "write",
                transport: "internal",
                user_id: user_id(),
                pack_bytes: request.puts.iter().map(|(_, bytes)| bytes.len() as u64).sum(),
                review_push: request.viewer,
                ..Default::default()
            };
            let change = store::RefChange {
                name: format!("{}{branch}", super::reach::REVIEW_REFS),
                old: None,
                new: Some(commit),
            };
            self.record_update(repository_id, &origin, store, &prior, &[change]);
            return Ok(FileCommitOutcome::Proposed {
                commit,
                target: branch.to_string(),
            });
        }
        let (ref_name, expected) = match request.new_branch {
            Some(new_branch) => {
                let name = ops::branch_ref(new_branch.strip_prefix("refs/heads/").unwrap_or(new_branch));
                if store.read_ref(&name)?.is_some() {
                    return Ok(FileCommitOutcome::Refused(format!("{new_branch} already exists")));
                }
                (name, None)
            }
            None => (base_ref, parents.first().copied()),
        };
        let written = self.write_refs(
            repository_id,
            &[super::hosting::RefWrite {
                name: ref_name.clone(),
                expected,
                new: Some(commit),
            }],
            false,
            None,
        )?;
        Ok(match written.into_iter().next() {
            Some(Ok(())) => FileCommitOutcome::Committed { commit, ref_name },
            Some(Err(reason)) => FileCommitOutcome::Refused(reason),
            None => FileCommitOutcome::Refused("no ref written".into()),
        })
    }
}

fn user_id() -> Option<String> {
    moqui_core::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
}
