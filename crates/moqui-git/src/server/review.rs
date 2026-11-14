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

//! What a push may build on, and a push for review (docs/interfaces/Git-Hosting.md, Reachability and Push to
//! review; plans/Dogfood.md F5, N-REACH): the checks of the objects a push names and does not bring, the checks of
//! a push to `refs/for/<branch>`, and the merge request it opens or updates. The push itself is `server::mod`'s.

use std::collections::HashSet;

use gix::objs::Kind;
use gix::ObjectId;

use super::{may_push, path, public_base_url, receive, review_push_enabled, review_starts_hidden, Session};
use crate::revwalk;
use crate::store::GitStore;
use moqui_core::context::{Error, ExecutionContext};
use moqui_lib::object::Value;

impl Session<'_> {
    /// One of `ids`, objects a push names without sending them (a ref's new value, the parent of a pushed commit, the
    /// target of a pushed tag), that is not the repository's own to build on, when there is one: an id the store
    /// does not have, a commit no readable ref reaches, or a tag object no readable ref names
    /// ([`Self::ref_named`]). That the store has it is not enough, as the store is shared with the fork network and
    /// holds hidden merge requests; and one that is not there is answered as one that is another's. A tree or a blob
    /// passes here: it is checked as a file of a commit the pusher reads ([`Self::foreign_object`],
    /// [`Self::own_value`]).
    pub(super) fn own_history(&self, ids: &[ObjectId]) -> Result<Option<ObjectId>, Error> {
        let store = self.repo.store.as_ref();
        let reached = self.provider.reachable_of(self.repo, ids)?;
        let unreached: Vec<ObjectId> = ids.iter().filter(|id| !reached.contains(*id)).copied().collect();
        if unreached.is_empty() {
            return Ok(None);
        }
        let mut tags = Vec::new();
        for chunk in unreached.chunks(500) {
            let kinds = store.meta_many(chunk)?;
            for id in chunk {
                match kinds.get(id).map(|meta| meta.kind) {
                    None | Some(Kind::Commit) => return Ok(Some(*id)),
                    Some(Kind::Tag) => tags.push(*id),
                    Some(Kind::Tree | Kind::Blob) => {}
                }
            }
        }
        if tags.is_empty() {
            return Ok(None);
        }
        let named = self.ref_named()?;
        Ok(tags.into_iter().find(|id| !named.contains(id)))
    }

    /// The objects the refs a reader reads name: each ref's value, and for a tag every tag object down to what it
    /// tags. A fetch gives them to any reader, so a push may name them.
    fn ref_named(&self) -> Result<HashSet<ObjectId>, Error> {
        let store = self.repo.store.as_ref();
        let refs = self.provider.readable_refs(self.repo)?.readable;
        let mut named: HashSet<ObjectId> = refs.iter().map(|(_, target)| *target).collect();
        let tags: Vec<ObjectId> = refs
            .iter()
            .filter(|(name, _)| name.starts_with("refs/tags/"))
            .map(|(_, target)| *target)
            .collect();
        for chunk in tags.chunks(500) {
            // an annotated tag's object is read; a tag of a commit is not
            let kinds = store.meta_many(chunk)?;
            for id in chunk {
                if kinds.get(id).is_none_or(|meta| meta.kind != Kind::Tag) {
                    continue;
                }
                let mut current = *id;
                for _ in 0..16 {
                    let Some((Kind::Tag, data)) = store.find(&current)? else {
                        break;
                    };
                    current = gix::objs::TagRef::from_bytes(&data, crate::ops::HASH)
                        .map_err(|err| Error::new(format!("git tag {current}: {err}")))?
                        .target();
                    named.insert(current);
                }
            }
        }
        Ok(named)
    }

    /// Whether `new`, the new value of a ref that the push did not bring, is the pusher's to point a ref at: a
    /// commit the readable refs reach, an object a readable ref names (an annotated tag given a second name), or,
    /// for a user who may push, a file or directory of a commit the readable refs point to. An id the store does not
    /// have is answered as one that is another's, before the ref's name or the user's rights are looked at, so the
    /// answer does not tell the two apart.
    pub(super) fn own_value(&self, new: &ObjectId) -> Result<bool, Error> {
        if self.own_history(&[*new])?.is_some() {
            return Ok(false);
        }
        if self.provider.reachable(self.repo, new)? || self.ref_named()?.contains(new) {
            return Ok(true);
        }
        // a tree or a blob: a user with no push right makes no ref of one
        if !may_push(&self.repo.definition.id) {
            return Ok(false);
        }
        Ok(self.foreign_object(&[*new], &[], &[])?.is_none())
    }

    /// Whether a push to `refs/for/<target>` may be made: the server takes them, the branch is there, and the push
    /// brings a commit the branch does not have. View is enough; the caps are checked apart ([`Self::review_cap`]).
    pub(super) fn check_review(
        &self,
        view: &dyn GitStore,
        command: &receive::Command,
        target: &str,
    ) -> Result<(), String> {
        if !review_push_enabled() {
            return Err("pushes for review (refs/for/) are off on this server".into());
        }
        let Some(new) = command.new else {
            return Err("a ref for review is not deleted: close the merge request".into());
        };
        let failed = |err: Error| err.message().to_string();
        let Some(target_head) = view.read_ref(&crate::ops::branch_ref(target)).map_err(failed)? else {
            return Err(format!("GitNoSuchRef: no branch {target} to review for"));
        };
        match view.find(&new).map_err(failed)?.map(|(kind, _)| kind) {
            None => return Err(format!("missing necessary objects: {new}")),
            Some(Kind::Commit) => {}
            Some(kind) => return Err(format!("a review is of a commit, and {new} is a {kind}")),
        }
        if revwalk::is_ancestor(view, &new, &target_head).map_err(failed)? {
            return Err(format!("no new commits: {target} already has {new}"));
        }
        Ok(())
    }

    /// After a push to `refs/for/<target>` stored its objects: open the user's merge request for the branch and
    /// topic, or give the open one its new head (`propose#MergeRequest`), from the push options `topic=`, `title=`,
    /// `description=` and `draft`. What to tell the client; a refusal is the ref's.
    pub(super) fn propose(&self, command: &receive::Command, options: &[String]) -> Result<Vec<String>, String> {
        let id = &self.repo.definition.id;
        let store = self.repo.store.as_ref();
        let (Some(target), Some(new)) = (command.name.strip_prefix(crate::reach::REVIEW_REFS), command.new) else {
            return Err("not a ref for review".into());
        };
        let option = |name: &str| {
            options
                .iter()
                .find_map(|option| option.strip_prefix(name)?.strip_prefix('='))
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };
        let mut parameters = std::collections::BTreeMap::new();
        parameters.insert("repositoryId".to_string(), Value::string(id));
        parameters.insert("targetRef".to_string(), Value::string(target));
        parameters.insert("headCommitId".to_string(), Value::string(new.to_string()));
        for (parameter, name) in [("topic", "topic"), ("title", "title"), ("description", "description")] {
            if let Some(value) = option(name) {
                parameters.insert(parameter.to_string(), Value::string(value));
            }
        }
        if options.iter().any(|option| option.trim() == "draft") {
            parameters.insert("draft".to_string(), Value::Bool(true));
        }
        if review_starts_hidden() {
            parameters.insert("hidden".to_string(), Value::Bool(true));
        }
        // the title and text a new merge request has when the push names none: its first commit's
        let target_head = store
            .read_ref(&crate::ops::branch_ref(target))
            .ok()
            .flatten()
            .into_iter()
            .collect::<Vec<_>>();
        let first = revwalk::new_commits(store, &[new], &target_head)
            .ok()
            .and_then(|commits| commits.last().map(|commit| commit.id))
            .unwrap_or(new);
        if let Ok(details) = crate::ops::commit_details(store, &first) {
            parameters.insert("defaultTitle".to_string(), Value::string(details.summary()));
            let body = details
                .message
                .split_once('\n')
                .map(|(_, body)| body.trim().to_string())
                .unwrap_or_default();
            if !body.is_empty() {
                parameters.insert("defaultDescription".to_string(), Value::string(body));
            }
        }
        let call = moqui_core::service::call::CallOptions {
            disable_authz: true,
            ..Default::default()
        };
        let out = moqui_core::service::call::sync_checked(
            "org.moqui.git.GitReviewServices.propose#MergeRequest",
            &parameters,
            &call,
        )
        .map_err(|err| {
            ExecutionContext::with_current(|ec| ec.message().clear_errors());
            err.message().to_string()
        })?;
        let text = |name: &str| {
            out.get(name)
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        };
        let (num, request_id) = (text("mergeRequestNum"), text("mergeRequestId"));
        let verb = if text("created") == "true" { "opened" } else { "updated" };
        let mut lines = vec![format!("merge request !{num} {verb} for {target} ({request_id})")];
        let template = crate::setting("merge-request-url");
        if !template.is_empty() {
            let url = template
                .replace("{repositoryPath}", &path::path_of(id))
                .replace("{repositoryId}", id)
                .replace("{mergeRequestNum}", &num)
                .replace("{mergeRequestId}", &request_id);
            lines.push(if url.starts_with('/') {
                format!("  {}{url}", public_base_url())
            } else {
                format!("  {url}")
            });
        }
        Ok(lines)
    }

    /// The commits whose files a push may name without bringing them (as a tree's entry, a tag's target, the base
    /// of a delta): for a user who may push, what the repository's readable refs point to; for one who may not, the
    /// branches its pushes for review are for and the heads of its own open pushes for review. And `built_on`, the
    /// commits the push builds on, which were checked to be the repository's own.
    fn nameable_roots(
        &self,
        pusher: bool,
        review_targets: &[String],
        built_on: &[ObjectId],
    ) -> Result<Vec<ObjectId>, Error> {
        let store = self.repo.store.as_ref();
        let mut tips: Vec<ObjectId> = Vec::new();
        if pusher {
            tips.extend(
                self.provider
                    .readable_refs(self.repo)?
                    .readable
                    .into_iter()
                    .map(|(_, tip)| tip),
            );
        } else {
            for target in review_targets {
                tips.extend(store.read_ref(&crate::ops::branch_ref(target))?);
            }
            tips.extend(self.provider.own_review_heads(self.repo)?);
        }
        let mut roots: Vec<ObjectId> = built_on.to_vec();
        for tip in tips {
            roots.extend(revwalk::peel_to_commit(store, &tip)?);
        }
        Ok(roots)
    }

    /// One of the objects a push names and did not bring (`named`: what its objects refer to outside the pack, and
    /// the bases its thin pack was completed with) that the pusher may not name, when there is one: an object that
    /// is not a commit the readable refs reach, is not a file or directory of a commit of
    /// [`Self::nameable_roots`], and is not an object a readable ref names. A store shared with a fork network, or
    /// holding a hidden merge request, has objects the pusher may not read; named in a push they would be read back
    /// through it.
    ///
    /// The walk is the server's work for whoever pushes, so for a user with no push right it is small: at most
    /// `git-server` `review-max-built-on` (8) commits built on, and `review-max-trees` (20,000) trees read.
    ///
    /// # Errors
    ///
    /// `GitReviewPushTooWide` for a push by a user with no push right that builds on more commits than that.
    pub(super) fn foreign_object(
        &self,
        named: &[ObjectId],
        review_targets: &[String],
        built_on: &[ObjectId],
    ) -> Result<Option<ObjectId>, Error> {
        let store = self.repo.store.as_ref();
        let commits = self.provider.reachable_of(self.repo, named)?;
        let wanted: HashSet<ObjectId> = named.iter().filter(|id| !commits.contains(*id)).copied().collect();
        if wanted.is_empty() {
            return Ok(None);
        }
        let pusher = may_push(&self.repo.definition.id);
        let built_on: Vec<ObjectId> = built_on.iter().filter(|id| commits.contains(*id)).copied().collect();
        let budget = if pusher {
            crate::reach::WITHIN_MAX_TREES
        } else {
            let most: usize = crate::number_setting("review-max-built-on", 8);
            if built_on.len() > most {
                return Err(Error::new(format!(
                    "GitReviewPushTooWide: a push for review builds on at most {most} commits of the repository, \
                     and this one builds on {}",
                    built_on.len()
                )));
            }
            crate::number_setting("review-max-trees", 20_000)
        };
        let roots = self.nameable_roots(pusher, review_targets, &built_on)?;
        let found = crate::reach::objects_within(store, &roots, &wanted, budget)?;
        let mut left: Vec<ObjectId> = wanted.into_iter().filter(|id| !found.contains(id)).collect();
        if !left.is_empty() {
            let named = self.ref_named()?;
            left.retain(|id| !named.contains(id));
        }
        Ok(left.into_iter().next())
    }
}

#[cfg(test)]
#[path = "review_tests.rs"]
pub(crate) mod tests;
