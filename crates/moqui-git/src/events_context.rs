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

//! The contexts a gitECA rule of each event had, or would have had, from a change of the push log (a `GitUpdate`
//! and its refs), built by the same parts as the events themselves: for trying a rule's condition against a recorded
//! push (the Dev AIDE's ECA view, plans/design-dev-aide.md 10). The post events (ref-update, commit, tag,
//! post-receive) are of the refs the change made; the pre events (pre-receive, pre-ref-update) of every ref it asked
//! for. What a recorded change does not keep is as for a push: no merge, and the refs before it are its `priorTips`.
//!
//! A replay is read by who may push to the repository, but the store may hold commits that user does not read (a
//! hidden merge request's, another fork's in a fork network): every commit it lists, and every tip it walks from, is
//! one the user reads by id (`readable_by_id`, as `get#GitCommit` is), and a ref the change did not make (a refused
//! push's) lists none. What is left out marks the list short (`newCommitsTruncated`, and the answer's `truncated`), so
//! the answer says no more about the store than the reads by id do. The walks are bounded, and so are the commits
//! listed in all ([`COMMIT_BUDGET`]).

use std::collections::BTreeMap;

use gix::ObjectId;
use moqui_lib::object::Value;

use super::{
    commits_by_ref, insert_new_commits, max_commits, parse_id, post_base, pre_base, receive_commits, ref_fields,
    tag_event_value, Gained, PreOrigin, RefUpdate, GIT_UPDATE, GIT_UPDATE_REF,
};
use crate::revwalk::{self, peel_to_commit};
use crate::values::commit_value;
use crate::{ops, rows_guard, text, GitProvider, Repo};
use moqui_core::context::Error;

/// The most commits a replay lists in all its contexts, as commits or as ids.
pub(crate) const COMMIT_BUDGET: usize = 200;

/// One context of an event: what a rule's condition and actions read, and which ref or commit it is of.
#[derive(Clone, Debug)]
pub(crate) struct EventContext {
    pub label: String,
    pub fields: BTreeMap<String, Value>,
}

/// A replay's contexts, and whether it left some out (past the limit or the commit budget, or not the user's to read).
#[derive(Debug, Default)]
pub(crate) struct EventContexts {
    pub contexts: Vec<EventContext>,
    pub truncated: bool,
}

/// The events a context is built for.
pub(crate) const EVENTS: [&str; 6] = [
    "pre-receive",
    "pre-ref-update",
    "ref-update",
    "commit",
    "tag",
    "post-receive",
];

/// The reads of one replay: the user's reads by id, and the commits it may still list.
struct Replay<'a> {
    provider: &'a GitProvider,
    repo: &'a Repo,
    budget: usize,
    /// Something was left out.
    truncated: bool,
}

impl Replay<'_> {
    /// Whether the user reads the commit `id` by its id; one that cannot be told is not.
    fn readable(&self, id: &ObjectId) -> bool {
        self.provider.readable_by_id(self.repo, id).unwrap_or_else(|err| {
            tracing::warn!(repository = %self.repo.definition.id, "commit {id} of a replay not read: {err}");
            false
        })
    }

    /// The commits `new` reaches and `hidden` does not, oldest first, at most `room` and what the budget has left, only
    /// those the user reads, from a tip the user reads; and whether more are there (or some were left out).
    fn walk(&mut self, new: ObjectId, hidden: &[ObjectId], room: usize) -> (Vec<ObjectId>, bool) {
        let store = self.repo.store.as_ref();
        let tip = match peel_to_commit(store, &new) {
            Ok(Some(tip)) => tip,
            // a tag of a tree or a blob brings no commit; an object the store no longer has brings unknown ones
            Ok(None) => return (Vec::new(), !matches!(store.find(&new), Ok(Some(_)))),
            Err(err) => {
                tracing::warn!(repository = %self.repo.definition.id, "{new} of a replay: {err}");
                return (Vec::new(), true);
            }
        };
        if !self.readable(&tip) {
            self.truncated = true;
            return (Vec::new(), true);
        }
        // the budget spent: no walk, and the list is short
        if self.budget == 0 {
            self.truncated = true;
            return (Vec::new(), true);
        }
        let room = room.min(self.budget);
        let (commits, more) = match revwalk::new_commits_at_most(store, &[tip], hidden, room) {
            Ok(walked) => walked,
            Err(err) => {
                tracing::warn!(repository = %self.repo.definition.id, "the commits of {tip} in a replay: {err}");
                return (Vec::new(), true);
            }
        };
        let walked = commits.len();
        let mut ids: Vec<ObjectId> = commits
            .into_iter()
            .map(|commit| commit.id)
            .filter(|id| self.readable(id))
            .collect();
        ids.reverse();
        let dropped = ids.len() < walked;
        self.budget -= ids.len();
        if dropped || (more && self.budget == 0) {
            self.truncated = true;
        }
        (ids, more || dropped)
    }

    /// The commits a ref of a pre event would gain: those of [`Replay::walk`] from its new commit, hidden by its old
    /// one or, for a new ref, by what the refs reached before (`prior_tips`), at most `eca-max-commits` as the events
    /// list.
    fn gained(&mut self, reference: &RefUpdate, prior_tips: &[ObjectId]) -> Gained {
        let Some(new) = reference.new else {
            return Gained {
                ids: Vec::new(),
                truncated: false,
            };
        };
        let store = self.repo.store.as_ref();
        let hidden: Vec<ObjectId> = match reference.old {
            Some(old) => vec![peel_to_commit(store, &old).ok().flatten().unwrap_or(old)],
            None => prior_tips.to_vec(),
        };
        let (ids, truncated) = self.walk(new, &hidden, max_commits());
        Gained { ids, truncated }
    }
}

/// A context's fields: `base` with `extra` over it.
fn with(base: &BTreeMap<String, Value>, extra: Vec<(&str, Value)>) -> BTreeMap<String, Value> {
    let mut fields = base.clone();
    for (name, value) in extra {
        fields.insert(name.to_string(), value);
    }
    fields
}

impl GitProvider {
    /// The contexts of `event`'s rules from the update `update_id` of `repository_id`: one for pre-receive and
    /// post-receive, one a ref for pre-ref-update and ref-update, one a tag created, one a new commit; at most `limit`
    /// contexts and [`COMMIT_BUDGET`] commits.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown event, an update that is not the repository's, or one that cannot be read.
    pub(crate) fn event_contexts(
        &self,
        repository_id: &str,
        update_id: &str,
        event: &str,
        limit: usize,
    ) -> Result<EventContexts, Error> {
        if !EVENTS.contains(&event) {
            return Err(Error::new(format!("no git event {event}: {}", EVENTS.join(", "))));
        }
        let entity = self.entity()?;
        let (update, rows) = {
            let _rows = rows_guard();
            let update = entity
                .find(GIT_UPDATE)?
                .eq("updateId", Value::string(update_id))
                .one()?
                .filter(|row| text(row.get("repositoryId")).as_deref() == Some(repository_id))
                .ok_or_else(|| Error::new(format!("repository {repository_id} has no update {update_id}")))?;
            let rows = entity
                .find(GIT_UPDATE_REF)?
                .eq("updateId", Value::string(update_id))
                .order_by("refName", false)
                .list()?;
            (update, rows)
        };
        let source = text(update.get("source")).unwrap_or_default();
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let branch_ref = ops::branch_ref(&repo.definition.branch);
        let reference_of = |row: &moqui_core::entity::EntityValue| RefUpdate {
            name: text(row.get("refName")).unwrap_or_default(),
            old: parse_id(row.get("oldObjectId")),
            new: parse_id(row.get("newObjectId")),
            change_type: text(row.get("changeType")).unwrap_or_default(),
        };
        let made_row = |row: &moqui_core::entity::EntityValue| text(row.get("result")).as_deref() == Some("ok");
        // every ref asked for, and whether the change made it
        let asked: Vec<(RefUpdate, bool)> = rows.iter().map(|row| (reference_of(row), made_row(row))).collect();
        let made: Vec<RefUpdate> = asked
            .iter()
            .filter(|(_, made)| *made)
            .map(|(reference, _)| reference.clone())
            .collect();
        let mut replay = Replay {
            provider: self,
            repo: &repo,
            budget: COMMIT_BUDGET,
            truncated: false,
        };
        let mut out = Vec::new();
        match event {
            "pre-receive" | "pre-ref-update" => {
                let options: Vec<String> = text(update.get("pushOptions"))
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_string)
                    .collect();
                let transport = text(update.get("transport")).unwrap_or_default();
                let user_id = text(update.get("userId"));
                let pack_bytes = update
                    .get("packBytes")
                    .map(moqui_lib::object::to_plain_string)
                    .and_then(|bytes| bytes.parse::<u64>().ok())
                    .unwrap_or(0);
                let chosen: Vec<&RefUpdate> = asked.iter().map(|(reference, _)| reference).collect();
                let base = pre_base(
                    &PreOrigin {
                        repository_id,
                        source: &source,
                        transport: &transport,
                        user_id: user_id.as_deref(),
                        push_options: &options,
                        pack_bytes,
                        merge: None,
                    },
                    &chosen,
                    &branch_ref,
                );
                let prior_tips: Vec<ObjectId> = text(update.get("priorTips"))
                    .unwrap_or_default()
                    .split_whitespace()
                    .filter_map(|hex| ObjectId::from_hex(hex.as_bytes()).ok())
                    .filter_map(|id| peel_to_commit(store, &id).ok().flatten())
                    .collect();
                // the refs a context is made of: every ref for pre-receive, those inside the limit for pre-ref-update
                let shown = if event == "pre-receive" {
                    asked.len()
                } else {
                    asked.len().min(limit)
                };
                if shown < asked.len() {
                    replay.truncated = true;
                }
                let gains: Vec<Gained> = asked[..shown]
                    .iter()
                    .map(|(reference, made)| {
                        if *made {
                            replay.gained(reference, &prior_tips)
                        } else {
                            // a ref the change did not make: its commits are not the repository's, and are not listed
                            Gained {
                                ids: Vec::new(),
                                truncated: reference.new.is_some(),
                            }
                        }
                    })
                    .collect();
                if event == "pre-receive" {
                    let mut fields = base.clone();
                    let (ids, truncated) = receive_commits(&gains, max_commits());
                    insert_new_commits(&mut fields, store, &ids, truncated);
                    out.push(EventContext {
                        label: format!("the push of {} refs", asked.len()),
                        fields,
                    });
                } else {
                    for ((reference, _), gain) in asked.iter().zip(&gains) {
                        let mut fields = with(&base, ref_fields(reference, &branch_ref));
                        insert_new_commits(&mut fields, store, &gain.ids, gain.truncated);
                        out.push(EventContext {
                            label: reference.name.clone(),
                            fields,
                        });
                    }
                }
            }
            _ => {
                let base = post_base(&update, repository_id, &source, &made, &branch_ref);
                match event {
                    "post-receive" => out.push(EventContext {
                        label: format!("the change of {} refs", made.len()),
                        fields: base,
                    }),
                    "ref-update" => {
                        for reference in &made {
                            if out.len() >= limit {
                                replay.truncated = true;
                                break;
                            }
                            let ids = match reference.new {
                                Some(new) => {
                                    replay
                                        .walk(new, &reference.old.into_iter().collect::<Vec<_>>(), max_commits())
                                        .0
                                }
                                None => Vec::new(),
                            };
                            let mut extra = ref_fields(reference, &branch_ref);
                            extra.push((
                                "newCommitIds",
                                Value::List(ids.iter().map(|id| Value::string(id.to_string())).collect()),
                            ));
                            out.push(EventContext {
                                label: reference.name.clone(),
                                fields: with(&base, extra),
                            });
                        }
                    }
                    "tag" => {
                        let created = made
                            .iter()
                            .filter(|reference| reference.name.starts_with("refs/tags/") && reference.old.is_none());
                        for reference in created {
                            let Some(new) = reference.new else { continue };
                            if out.len() >= limit {
                                replay.truncated = true;
                                break;
                            }
                            // a tag of a commit the user does not read is not shown, nor one that cannot be read
                            if let Ok(Some(commit)) = peel_to_commit(store, &new) {
                                if !replay.readable(&commit) {
                                    replay.truncated = true;
                                    continue;
                                }
                            }
                            let tag = match tag_event_value(store, &reference.name, &new) {
                                Ok(tag) => tag,
                                Err(err) => {
                                    tracing::warn!(
                                        update = update_id,
                                        "tag {} of a recorded change: {err}",
                                        reference.name
                                    );
                                    replay.truncated = true;
                                    continue;
                                }
                            };
                            out.push(EventContext {
                                label: reference.name.clone(),
                                fields: with(
                                    &base,
                                    vec![
                                        ("tag", tag),
                                        ("refName", Value::string(&reference.name)),
                                        ("changeType", Value::string(&reference.change_type)),
                                    ],
                                ),
                            });
                        }
                    }
                    _ => {
                        let room = max_commits().min(limit);
                        let (walked, more) =
                            commits_by_ref(&update, &made, &branch_ref, room, &mut |new, hidden, room| {
                                Ok(replay.walk(new, hidden, room))
                            });
                        let mut truncated = more;
                        for (reference, commits) in walked {
                            let commits = match commits {
                                Ok(commits) => commits,
                                Err(err) => {
                                    tracing::warn!(update = update_id, "the commits of a recorded change: {err}");
                                    truncated = true;
                                    continue;
                                }
                            };
                            for commit in commits {
                                // a commit no longer in the store (collected since) is passed over
                                let details = match ops::commit_details(store, &commit) {
                                    Ok(details) => details,
                                    Err(err) => {
                                        tracing::warn!(
                                            update = update_id,
                                            "commit {commit} of a recorded change: {err}"
                                        );
                                        truncated = true;
                                        continue;
                                    }
                                };
                                out.push(EventContext {
                                    label: format!("{} on {}", commit.to_hex_with_len(10), reference.name),
                                    fields: with(
                                        &base,
                                        vec![
                                            ("commit", commit_value(&details)),
                                            ("refName", Value::string(&reference.name)),
                                            ("isDefaultBranch", Value::Bool(reference.name == branch_ref)),
                                        ],
                                    ),
                                });
                            }
                        }
                        replay.truncated |= truncated;
                    }
                }
            }
        }
        Ok(EventContexts {
            contexts: out,
            truncated: replay.truncated,
        })
    }
}
