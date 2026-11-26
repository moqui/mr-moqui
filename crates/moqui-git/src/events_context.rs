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
//! for, with the commits the repository still has (a refused push's new objects were not kept, so it lists none and
//! says the list is short). What a recorded change does not keep is as for a push: no merge, and the refs before it
//! are its `priorTips`.

use std::collections::BTreeMap;

use gix::ObjectId;
use moqui_lib::object::Value;

use super::{
    commits_by_ref, gained, insert_new_commits, new_commit_ids, parse_id, post_base, pre_base, receive_commits,
    ref_fields, tag_event_value, PreOrigin, RefUpdate, GIT_UPDATE, GIT_UPDATE_REF,
};
use crate::ops;
use crate::revwalk::peel_to_commit;
use crate::values::commit_value;
use crate::{rows_guard, text, GitProvider};
use moqui_core::context::Error;

/// One context of an event: what a rule's condition and actions read, and which ref or commit it is of.
#[derive(Clone, Debug)]
pub(crate) struct EventContext {
    pub label: String,
    pub fields: BTreeMap<String, Value>,
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

impl GitProvider {
    /// The contexts of `event`'s rules from the update `update_id` of `repository_id`: one for pre-receive and
    /// post-receive, one a ref for pre-ref-update and ref-update, one a tag created, one a new commit; at most `limit`.
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
    ) -> Result<Vec<EventContext>, Error> {
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
        let asked: Vec<RefUpdate> = rows.iter().map(reference_of).collect();
        let made: Vec<RefUpdate> = rows
            .iter()
            .filter(|row| text(row.get("result")).as_deref() == Some("ok"))
            .map(reference_of)
            .collect();
        let with = |base: &BTreeMap<String, Value>, extra: Vec<(&str, Value)>| {
            let mut fields = base.clone();
            for (name, value) in extra {
                fields.insert(name.to_string(), value);
            }
            fields
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
                let chosen: Vec<&RefUpdate> = asked.iter().collect();
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
                let gains: Vec<_> = asked
                    .iter()
                    .map(|reference| {
                        let mut gain = gained(store, reference, &prior_tips);
                        // a refused push's new objects were not kept: its commits are not known, which the list says
                        if reference
                            .new
                            .is_some_and(|new| !matches!(store.find(&new), Ok(Some(_))))
                        {
                            gain.truncated = true;
                        }
                        gain
                    })
                    .collect();
                if event == "pre-receive" {
                    let mut fields = base.clone();
                    let (ids, truncated) = receive_commits(&gains, limit);
                    insert_new_commits(&mut fields, store, &ids, truncated);
                    out.push(EventContext {
                        label: format!("the push of {} refs", asked.len()),
                        fields,
                    });
                } else {
                    for (reference, gain) in asked.iter().zip(&gains).take(limit) {
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
                        for reference in made.iter().take(limit) {
                            let mut extra = ref_fields(reference, &branch_ref);
                            extra.push(("newCommitIds", new_commit_ids(store, reference)));
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
                        for reference in created.take(limit) {
                            let Some(new) = reference.new else { continue };
                            let tag = tag_event_value(store, &reference.name, &new)?;
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
                        let (walked, _) = commits_by_ref(store, &update, &made, &branch_ref, limit);
                        for (reference, commits) in walked {
                            let commits = match commits {
                                Ok(commits) => commits,
                                Err(err) => {
                                    tracing::warn!(update = update_id, "the commits of a recorded change: {err}");
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
                    }
                }
            }
        }
        Ok(out)
    }
}
