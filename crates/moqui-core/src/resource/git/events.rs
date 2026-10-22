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

//! The events of a repository's refs, for gitECA rules (`service::git_eca`). Each change is a `GitUpdate` row with a
//! `GitUpdateRef` per ref: a push's before its refs change (`recording`, then `pending`), a fetch's, an import's or a
//! write's after (`pending`). A pending update's post events run on a job of the git pool soon after
//! ([`GitProvider::process_update`]), or from the `process_PendingGitUpdates_frequent` job when that did not happen:
//! the rows are what make the events survive a restart. The commits new to the repository are worked out then, from
//! the refs before the change (`priorTips`): a collection of unreachable objects (`gc#GitRepositories`) keeps the
//! tips of an update not yet done, so the walk gives the same answer later.
//! The pre events of a push run in the push itself, against its quarantined objects ([`pre_receive`]).

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use gix::objs::Kind;
use gix::ObjectId;
use moqui_lib::object::Value;

use super::browse::{short_name, RefKind};
use super::ops;
use super::revwalk::{self, peel_to_commit};
use super::store::{GitStore, RefChange};
use super::values::{commit_value, map, opt_text, timestamp};
use super::{rows_guard, text, GitProvider};
use crate::context::{Error, ExecutionContext};
use crate::entity::{Condition, EntityValue};
use crate::service::git_eca::{self, GitEvent, GitEventKey, RuleOutcome};

pub(crate) const GIT_UPDATE: &str = "moqui.resource.GitUpdate";
pub(crate) const GIT_UPDATE_REF: &str = "moqui.resource.GitUpdateRef";

/// The most commits one change raises commit events for (`git-server` `eca-max-commits`); a first push of a long history
/// stops there, with a warning.
const DEFAULT_MAX_COMMITS: usize = 10_000;

/// A recording that has not moved on for this long was cut short (a crash between the record and the ref update).
const STALE_RECORDING_MILLIS: i64 = 2 * 60 * 1000;
/// A run that has not finished for this long is run again.
const STALE_RUNNING_MILLIS: i64 = 30 * 60 * 1000;

/// The conf's `git-server` `eca-max-commits`, else [`DEFAULT_MAX_COMMITS`].
fn max_commits() -> usize {
    Some(super::number_setting("eca-max-commits", DEFAULT_MAX_COMMITS))
        .filter(|max| *max > 0)
        .unwrap_or(DEFAULT_MAX_COMMITS)
}

/// One ref of a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RefUpdate {
    pub name: String,
    pub old: Option<ObjectId>,
    pub new: Option<ObjectId>,
    /// create, update (a fast-forward), force or delete.
    pub change_type: String,
}

/// How a ref moved: `update` only when the old commit is an ancestor of the new one.
pub(crate) fn change_type(store: &dyn GitStore, old: Option<ObjectId>, new: Option<ObjectId>) -> &'static str {
    match (old, new) {
        (None, _) => "create",
        (_, None) => "delete",
        (Some(old), Some(new)) => {
            let old = peel_to_commit(store, &old).ok().flatten();
            let new = peel_to_commit(store, &new).ok().flatten();
            match (old, new) {
                (Some(old), Some(new)) if revwalk::is_ancestor(store, &old, &new).unwrap_or(false) => "update",
                _ => "force",
            }
        }
    }
}

/// Who and how, for a recorded change.
#[derive(Clone, Debug, Default)]
pub(crate) struct UpdateOrigin {
    pub source: &'static str,
    pub transport: &'static str,
    pub user_id: Option<String>,
    pub remote_address: Option<String>,
    pub push_options: Vec<String>,
}

fn id_text(id: Option<ObjectId>) -> Value {
    id.map_or(Value::Null, |id| Value::string(id.to_string()))
}

fn parse_id(value: Option<&Value>) -> Option<ObjectId> {
    text(value).and_then(|hex| ObjectId::from_hex(hex.as_bytes()).ok())
}

fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

impl GitProvider {
    /// Write a `GitUpdate` and its refs, each ref with `result` (`pending` while a push makes it): in a transaction of
    /// its own, or with `join` in the caller's (a write through `git://`, which the caller may still roll back). Its
    /// id; a failure is logged and gives `None`, as the change itself goes on.
    fn write_update(
        &self,
        repository_id: &str,
        origin: &UpdateOrigin,
        prior: &[(String, ObjectId)],
        refs: &[(RefUpdate, String)],
        status: &str,
        join: bool,
    ) -> Option<String> {
        let _rows = rows_guard();
        let written = (|| -> Result<String, Error> {
            let entity = self.entity()?;
            let transactions = entity.transactions_for(GIT_UPDATE)?;
            let attribute = if join {
                crate::transaction::TransactionAttribute::UseOrBegin
            } else {
                crate::transaction::TransactionAttribute::ForceNew
            };
            transactions
                .run(attribute, None, || {
                    let mut update = EntityValue::make(&entity, GIT_UPDATE)?;
                    update.set_sequenced_id_primary(&entity)?;
                    let update_id = text(update.get("updateId")).unwrap_or_default();
                    update.set("repositoryId", Value::string(repository_id));
                    update.set("source", Value::string(origin.source));
                    update.set("transport", Value::string(origin.transport));
                    update.set("userId", origin.user_id.clone().map_or(Value::Null, Value::string));
                    update.set(
                        "remoteAddress",
                        origin.remote_address.clone().map_or(Value::Null, Value::string),
                    );
                    update.set("updateDate", crate::user::now_value());
                    update.set(
                        "priorTips",
                        Value::string(
                            prior
                                .iter()
                                .map(|(_, id)| id.to_string())
                                .collect::<HashSet<_>>()
                                .into_iter()
                                .collect::<Vec<_>>()
                                .join(" "),
                        ),
                    );
                    if !origin.push_options.is_empty() {
                        update.set("pushOptions", Value::string(origin.push_options.join("\n")));
                    }
                    update.set("ecaStatus", Value::string(status));
                    update.create(&entity)?;
                    for (reference, result) in refs {
                        let mut row = EntityValue::make(&entity, GIT_UPDATE_REF)?;
                        row.set("updateId", Value::string(&update_id));
                        row.set("refName", Value::string(&reference.name));
                        row.set("oldObjectId", id_text(reference.old));
                        row.set("newObjectId", id_text(reference.new));
                        row.set("changeType", Value::string(&reference.change_type));
                        row.set("result", Value::string(result));
                        row.create(&entity)?;
                    }
                    Ok(update_id)
                })
                .map_err(Error::from)
        })();
        match written {
            Ok(id) => Some(id),
            Err(err) => {
                tracing::error!(repository = repository_id, "cannot record a git update: {err}");
                None
            }
        }
    }

    /// Record a push's refs before they change.
    pub(crate) fn begin_update(
        &self,
        repository_id: &str,
        origin: &UpdateOrigin,
        prior: &[(String, ObjectId)],
        refs: &[(RefUpdate, String)],
    ) -> Option<String> {
        self.write_update(repository_id, origin, prior, refs, "recording", false)
    }

    /// Record a change refused before any ref moved (each ref with why), for the push log: nothing to run after it.
    pub(crate) fn record_refused(
        &self,
        repository_id: &str,
        origin: &UpdateOrigin,
        prior: &[(String, ObjectId)],
        refs: &[(RefUpdate, String)],
    ) {
        self.write_update(repository_id, origin, prior, refs, "none", false);
    }

    /// Set each ref's result once a push made (or refused) it, and make the update pending (`none` when nothing
    /// changed), then schedule its post events.
    pub(crate) fn finish_update(&self, repository_id: &str, update_id: &str, results: &[(String, String)]) {
        let _rows = rows_guard();
        let changed = results.iter().any(|(_, result)| result == "ok");
        let outcome = (|| -> Result<(), Error> {
            let entity = self.entity()?;
            for (name, result) in results {
                entity
                    .find(GIT_UPDATE_REF)?
                    .eq("updateId", Value::string(update_id))
                    .eq("refName", Value::string(name))
                    .update_all([("result".to_string(), Value::string(result))])?;
            }
            entity
                .find(GIT_UPDATE)?
                .eq("updateId", Value::string(update_id))
                .update_all([(
                    "ecaStatus".to_string(),
                    Value::string(if changed { "pending" } else { "none" }),
                )])?;
            Ok(())
        })();
        if let Err(err) = outcome {
            tracing::error!(update = update_id, "cannot record the outcome of a git update: {err}");
            return;
        }
        if changed {
            self.schedule(repository_id);
        }
    }

    /// Record a change already made (a fetch, an import, a write) and schedule its post events. Nothing when no ref
    /// changed, or for a write that no rule listens to.
    pub(crate) fn record_update(
        &self,
        repository_id: &str,
        origin: &UpdateOrigin,
        store: &dyn GitStore,
        prior: &[(String, ObjectId)],
        changes: &[RefChange],
    ) {
        if changes.is_empty() {
            return;
        }
        if origin.source == "write" && !git_eca::listening_after("write") {
            return;
        }
        let refs: Vec<(RefUpdate, String)> = changes
            .iter()
            .map(|change| {
                (
                    RefUpdate {
                        name: change.name.clone(),
                        old: change.old,
                        new: change.new,
                        change_type: change_type(store, change.old, change.new).to_string(),
                    },
                    "ok".to_string(),
                )
            })
            .collect();
        let join = origin.source == "write";
        if self
            .write_update(repository_id, origin, prior, &refs, "pending", join)
            .is_none()
        {
            return;
        }
        // a write's commit is in the caller's transaction: its events run once that commits
        let deferred = join
            && (|| -> Result<bool, Error> {
                let transactions = self.entity()?.transactions_for(GIT_UPDATE)?;
                if !transactions.in_transaction() {
                    return Ok(false);
                }
                let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
                    .ok_or_else(|| Error::new("no execution context"))?;
                let provider = self.clone();
                let id = repository_id.to_string();
                transactions.on_commit(move || schedule_on(&factory, provider, id))?;
                Ok(true)
            })()
            .unwrap_or(false);
        if !deferred {
            self.schedule(repository_id);
        }
    }

    /// Run a repository's pending updates on a job of the git pool. When the pool cannot take it, they stay pending
    /// for the job that runs pending updates.
    pub(crate) fn schedule(&self, repository_id: &str) {
        if let Some(factory) = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) {
            schedule_on(&factory, self.clone(), repository_id.to_string());
        }
    }

    /// Run the pending updates of a repository one at a time, oldest first, so its events run in the order its refs
    /// changed; a run in another worker of this process waits its turn. (processed, failed) counts.
    pub fn process_repository(&self, repository_id: &str) -> Result<(usize, usize), Error> {
        let lock = Arc::clone(
            moqui_lib::sync::lock(&self.inner.event_locks)
                .entry(repository_id.to_string())
                .or_default(),
        );
        let _turn = moqui_lib::sync::lock(&lock);
        let entity = self.entity()?;
        let (mut processed, mut failed) = (0, 0);
        loop {
            let next = {
                let _rows = rows_guard();
                entity
                    .find(GIT_UPDATE)?
                    .eq("repositoryId", Value::string(repository_id))
                    .eq("ecaStatus", Value::string("pending"))
                    .order_by("updateDate", false)
                    .order_by("updateId", false)
                    .select_field("updateId")
                    .limit(1)
                    .list()?
                    .iter()
                    .find_map(|row| text(row.get("updateId")))
            };
            let Some(update_id) = next else {
                return Ok((processed, failed));
            };
            match self.process_update(&update_id) {
                Ok(status) if status == "failed" => failed += 1,
                Ok(_) => processed += 1,
                Err(err) => {
                    tracing::error!(update = %update_id, "git update events failed: {err}");
                    failed += 1;
                    // the update keeps its status; do not loop on it
                    let _rows = rows_guard();
                    entity
                        .find(GIT_UPDATE)?
                        .eq("updateId", Value::string(&update_id))
                        .eq("ecaStatus", Value::string("pending"))
                        .update_all([
                            ("ecaStatus".to_string(), Value::string("failed")),
                            ("ecaError".to_string(), Value::string(err.message())),
                        ])?;
                }
            }
        }
    }

    /// Run the post events of a pending update, as its user: ref-update, commit, tag and post-receive. The update is
    /// claimed first, so it runs once. Its status after: done, failed, or the status it had when it was not pending.
    pub fn process_update(&self, update_id: &str) -> Result<String, Error> {
        let entity = self.entity()?;
        let claimed = {
            let _rows = rows_guard();
            entity
                .find(GIT_UPDATE)?
                .eq("updateId", Value::string(update_id))
                .eq("ecaStatus", Value::string("pending"))
                .update_all([("ecaStatus".to_string(), Value::string("running"))])?
        };
        let (update, refs) = {
            let _rows = rows_guard();
            let update = entity
                .find(GIT_UPDATE)?
                .eq("updateId", Value::string(update_id))
                .one()?
                .ok_or_else(|| Error::new(format!("no GitUpdate {update_id}")))?;
            let refs = entity
                .find(GIT_UPDATE_REF)?
                .eq("updateId", Value::string(update_id))
                .order_by("refName", false)
                .list()?;
            (update, refs)
        };
        if claimed == 0 {
            return Ok(text(update.get("ecaStatus")).unwrap_or_default());
        }
        // the events run as the user who made the change; a caller's own user (a screen's Run again, a test) is put back
        // after, so the rest of its request is not that user's
        let user_id = text(update.get("userId"));
        let mut caller: Option<Option<String>> = None;
        if let Some(user_id) = &user_id {
            if crate::user::current_user_id().ok().flatten().as_deref() != Some(user_id.as_str()) {
                if let Some(username) = username_of(&entity, user_id) {
                    caller = Some(crate::user::current_username().ok().flatten());
                    if let Err(err) = crate::user::internal_login_user(&username) {
                        tracing::warn!(update = update_id, "git update events run without their user: {err}");
                    }
                }
            }
        }
        let errors = match self.run_post_events(&update, &refs) {
            Ok(errors) => errors,
            Err(err) => vec![err.message().to_string()],
        };
        match caller {
            Some(Some(username)) => {
                if let Err(err) = crate::user::internal_login_user(&username) {
                    tracing::error!(
                        update = update_id,
                        "the caller's user {username} was not put back after the git update events: {err}"
                    );
                    crate::user::session::clear_login();
                }
            }
            Some(None) => crate::user::session::clear_login(),
            None => {}
        }
        let status = if errors.is_empty() { "done" } else { "failed" };
        let _rows = rows_guard();
        let mut fields = vec![
            ("ecaStatus".to_string(), Value::string(status)),
            ("processedDate".to_string(), crate::user::now_value()),
        ];
        fields.push((
            "ecaError".to_string(),
            if errors.is_empty() {
                Value::Null
            } else {
                Value::string(errors.join("\n"))
            },
        ));
        entity
            .find(GIT_UPDATE)?
            .eq("updateId", Value::string(update_id))
            .update_all(fields)?;
        Ok(status.to_string())
    }

    /// Process every pending update, after putting back updates whose recording or run was cut short. (processed,
    /// failed) counts; an update that fails is recorded and logged, and the others still run.
    pub fn process_pending(&self) -> Result<(usize, usize), Error> {
        let entity = self.entity()?;
        let stale = {
            let _rows = rows_guard();
            let now = now_millis();
            let before = |millis: i64| crate::user::stamp_value(now - millis);
            let recording = entity
                .find(GIT_UPDATE)?
                .eq("ecaStatus", Value::string("recording"))
                .condition(Condition::less("updateDate", before(STALE_RECORDING_MILLIS)))
                .list()?;
            entity
                .find(GIT_UPDATE)?
                .eq("ecaStatus", Value::string("running"))
                .condition(Condition::less("updateDate", before(STALE_RUNNING_MILLIS)))
                .update_all([("ecaStatus".to_string(), Value::string("pending"))])?;
            recording
        };
        for update in stale.iter() {
            if let Err(err) = self.reconcile(update) {
                tracing::error!("cannot reconcile a cut-short git update: {err}");
            }
        }
        let repositories: Vec<String> = {
            let _rows = rows_guard();
            let mut ids: Vec<String> = entity
                .find(GIT_UPDATE)?
                .eq("ecaStatus", Value::string("pending"))
                .select_field("repositoryId")
                .distinct(true)
                .list()?
                .iter()
                .filter_map(|row| text(row.get("repositoryId")))
                .collect();
            ids.sort();
            ids.dedup();
            ids
        };
        let (mut processed, mut failed) = (0, 0);
        for repository_id in repositories {
            match self.process_repository(&repository_id) {
                Ok((done, bad)) => {
                    processed += done;
                    failed += bad;
                }
                Err(err) => {
                    tracing::error!(repository = %repository_id, "git update events failed: {err}");
                    failed += 1;
                }
            }
        }
        Ok((processed, failed))
    }

    /// A push recorded and then cut short: each ref that now points where the push meant it to was made.
    fn reconcile(&self, update: &EntityValue) -> Result<(), Error> {
        let _rows = rows_guard();
        let entity = self.entity()?;
        let update_id = text(update.get("updateId")).unwrap_or_default();
        let repository_id = text(update.get("repositoryId")).unwrap_or_default();
        let repo = self.repo(&repository_id)?;
        let mut results = Vec::new();
        for row in entity
            .find(GIT_UPDATE_REF)?
            .eq("updateId", Value::string(&update_id))
            .list()?
            .iter()
        {
            let name = text(row.get("refName")).unwrap_or_default();
            let result = text(row.get("result")).unwrap_or_default();
            if result != "pending" {
                results.push((name, result));
                continue;
            }
            let made = repo.store.read_ref(&name)? == parse_id(row.get("newObjectId"));
            results.push((
                name,
                if made {
                    "ok".into()
                } else {
                    "not made: the push was cut short".into()
                },
            ));
        }
        drop(_rows);
        self.finish_update(&repository_id, &update_id, &results);
        Ok(())
    }

    /// The post events of one update, in order: each new commit (oldest first), each tag created, each ref that
    /// changed, then post-receive. The errors of the rules that failed.
    fn run_post_events(&self, update: &EntityValue, refs: &crate::entity::EntityList) -> Result<Vec<String>, Error> {
        let repository_id = text(update.get("repositoryId")).unwrap_or_default();
        let source = text(update.get("source")).unwrap_or_default();
        let source = source.as_str();
        if !git_eca::listening_after(source) {
            return Ok(Vec::new());
        }
        let repo = self.repo(&repository_id)?;
        let store = repo.store.as_ref();
        let branch_ref = ops::branch_ref(&repo.definition.branch);
        let made: Vec<RefUpdate> = refs
            .iter()
            .filter(|row| text(row.get("result")).as_deref() == Some("ok"))
            .map(|row| RefUpdate {
                name: text(row.get("refName")).unwrap_or_default(),
                old: parse_id(row.get("oldObjectId")),
                new: parse_id(row.get("newObjectId")),
                change_type: text(row.get("changeType")).unwrap_or_default(),
            })
            .collect();
        let entity = self.entity()?;
        let transactions = entity.transactions_for(GIT_UPDATE)?;
        let mut base = BTreeMap::new();
        base.insert("repositoryId".to_string(), Value::string(&repository_id));
        base.insert("source".to_string(), Value::string(source));
        base.insert(
            "transport".to_string(),
            text(update.get("transport")).map_or(Value::Null, Value::string),
        );
        base.insert(
            "userId".to_string(),
            text(update.get("userId")).map_or(Value::Null, Value::string),
        );
        base.insert(
            "updateId".to_string(),
            text(update.get("updateId")).map_or(Value::Null, Value::string),
        );
        base.insert(
            "pushOptions".to_string(),
            Value::List(
                text(update.get("pushOptions"))
                    .unwrap_or_default()
                    .lines()
                    .map(Value::string)
                    .collect(),
            ),
        );
        base.insert(
            "refUpdates".to_string(),
            Value::List(made.iter().map(|reference| ref_value(reference, &branch_ref)).collect()),
        );
        let mut errors = Vec::new();
        let run = |key: GitEventKey<'_>, extra: Vec<(&str, Value)>, errors: &mut Vec<String>| {
            let mut fields = base.clone();
            for (name, value) in extra {
                fields.insert(name.to_string(), value);
            }
            let key = GitEventKey {
                has_errors: !errors.is_empty(),
                ..key
            };
            let RuleOutcome {
                errors: failed,
                messages,
            } = git_eca::run_git_rules(&key, &fields, Some(&transactions));
            for message in messages {
                tracing::info!(repository = %repository_id, event = key.event.name(), "{message}");
            }
            errors.extend(failed);
        };
        // commits new to the repository, each once, with the ref it came on: the default branch first, then the other
        // branches, then tags
        if git_eca::listening(GitEvent::Commit, source) {
            let mut hidden: Vec<ObjectId> = text(update.get("priorTips"))
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|hex| ObjectId::from_hex(hex.as_bytes()).ok())
                .collect();
            let mut order: Vec<&RefUpdate> = made.iter().filter(|reference| reference.new.is_some()).collect();
            order.sort_by_key(|reference| {
                (
                    reference.name != branch_ref,
                    !reference.name.starts_with("refs/heads/"),
                    reference.name.clone(),
                )
            });
            let limit = max_commits();
            let mut count = 0;
            for reference in order {
                let Some(new) = reference.new else { continue };
                if count >= limit {
                    break;
                }
                let commits = match revwalk::new_commits(store, &[new], &hidden) {
                    Ok(commits) => commits,
                    Err(err) => {
                        errors.push(format!("{}: {err}", reference.name));
                        continue;
                    }
                };
                hidden.push(new);
                let mut commits: Vec<_> = commits.into_iter().rev().collect();
                if count + commits.len() > limit {
                    tracing::warn!(
                        repository = %repository_id,
                        "a change brings more than {limit} new commits (git-server eca-max-commits); commit events stop there"
                    );
                    commits.truncate(limit - count);
                }
                count += commits.len();
                for commit in commits {
                    let details = ops::commit_details(store, &commit.id)?;
                    run(
                        event_key(
                            GitEvent::Commit,
                            source,
                            &repository_id,
                            Some(&reference.name),
                            &branch_ref,
                        ),
                        vec![
                            ("commit", commit_value(&details)),
                            ("refName", Value::string(&reference.name)),
                            ("isDefaultBranch", Value::Bool(reference.name == branch_ref)),
                        ],
                        &mut errors,
                    );
                }
            }
        }
        if git_eca::listening(GitEvent::Tag, source) {
            for reference in made
                .iter()
                .filter(|reference| reference.name.starts_with("refs/tags/") && reference.old.is_none())
            {
                let Some(new) = reference.new else { continue };
                run(
                    event_key(
                        GitEvent::Tag,
                        source,
                        &repository_id,
                        Some(&reference.name),
                        &branch_ref,
                    ),
                    vec![
                        ("tag", tag_event_value(store, &reference.name, &new)?),
                        ("refName", Value::string(&reference.name)),
                        ("changeType", Value::string(&reference.change_type)),
                    ],
                    &mut errors,
                );
            }
        }
        if git_eca::listening(GitEvent::RefUpdate, source) {
            for reference in &made {
                let mut fields = ref_fields(reference, &branch_ref);
                fields.push(("newCommitIds", new_commit_ids(store, reference)));
                run(
                    event_key(
                        GitEvent::RefUpdate,
                        source,
                        &repository_id,
                        Some(&reference.name),
                        &branch_ref,
                    ),
                    fields,
                    &mut errors,
                );
            }
        }
        run(
            event_key(GitEvent::PostReceive, source, &repository_id, None, &branch_ref),
            Vec::new(),
            &mut errors,
        );
        Ok(errors)
    }
}

fn schedule_on(factory: &Arc<crate::context::ExecutionContextFactory>, provider: GitProvider, repository_id: String) {
    let id = repository_id.clone();
    let submitted = super::server::pool(factory).try_submit(Arc::clone(factory), move || {
        if let Err(err) = provider.process_repository(&id) {
            tracing::error!(repository = %id, "git update events failed: {err}");
        }
        Ok(())
    });
    if let Err(err) = submitted {
        tracing::warn!(repository = %repository_id, "git update events left for the pending job: {err}");
    }
}

/// What a rule of `event` is matched against.
fn event_key<'a>(
    event: GitEvent,
    source: &'a str,
    repository_id: &'a str,
    ref_name: Option<&'a str>,
    branch_ref: &str,
) -> GitEventKey<'a> {
    GitEventKey {
        event,
        source,
        repository_id,
        ref_name,
        is_default_branch: ref_name == Some(branch_ref),
        has_errors: false,
    }
}

fn username_of(entity: &crate::entity::EntityFacade, user_id: &str) -> Option<String> {
    let _rows = rows_guard();
    entity
        .find("moqui.security.UserAccount")
        .ok()?
        .eq("userId", Value::string(user_id))
        .one()
        .ok()
        .flatten()
        .and_then(|row| text(row.get("username")))
}

/// A ref change as a map: `refName`, `refType`, `shortName`, `oldId`, `newId`, `changeType`, `isDefaultBranch`.
fn ref_value(reference: &RefUpdate, branch_ref: &str) -> Value {
    map(ref_fields(reference, branch_ref))
}

fn ref_fields(reference: &RefUpdate, branch_ref: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("refName", Value::string(&reference.name)),
        ("refType", Value::string(RefKind::of(&reference.name).name())),
        ("shortName", Value::string(short_name(&reference.name))),
        ("oldId", id_text(reference.old)),
        ("newId", id_text(reference.new)),
        ("changeType", Value::string(&reference.change_type)),
        ("isDefaultBranch", Value::Bool(reference.name == branch_ref)),
    ]
}

/// The commits a ref gained: reachable from its new commit and not from its old one.
fn new_commit_ids(store: &dyn GitStore, reference: &RefUpdate) -> Value {
    let (Some(new), old) = (reference.new, reference.old) else {
        return Value::List(Vec::new());
    };
    let Some(new) = peel_to_commit(store, &new).ok().flatten() else {
        return Value::List(Vec::new());
    };
    let hidden: Vec<ObjectId> = old.into_iter().collect();
    match revwalk::new_commits(store, &[new], &hidden) {
        Ok(commits) => Value::List(
            commits
                .into_iter()
                .rev()
                .take(max_commits())
                .map(|commit| Value::string(commit.id.to_string()))
                .collect(),
        ),
        Err(err) => {
            tracing::warn!("new commits of {}: {err}", reference.name);
            Value::List(Vec::new())
        }
    }
}

/// A tag as a map: `tagName`, `targetId`, `commitId` (what it peels to), `annotated`, and for an annotated tag
/// `message`, `taggerName`, `taggerEmail`, `taggerDate`.
fn tag_event_value(store: &dyn GitStore, name: &str, id: &ObjectId) -> Result<Value, Error> {
    let mut entries = vec![
        ("tagName", Value::string(short_name(name))),
        ("targetId", Value::string(id.to_string())),
        (
            "commitId",
            peel_to_commit(store, id)?.map_or(Value::Null, |commit| Value::string(commit.to_string())),
        ),
    ];
    match store.find(id)? {
        Some((Kind::Tag, _)) => {
            let tag = ops::tag_details(store, id)?;
            entries.push(("annotated", Value::Bool(true)));
            entries.push(("message", Value::string(tag.message.trim_end())));
            if let Some(tagger) = &tag.tagger {
                entries.push(("taggerName", opt_text(&tagger.name)));
                entries.push(("taggerEmail", opt_text(&tagger.email)));
                entries.push(("taggerDate", timestamp(tagger.seconds)));
            }
        }
        _ => entries.push(("annotated", Value::Bool(false))),
    }
    Ok(map(entries))
}

/// What the pre events of a push decided.
#[derive(Debug, Default)]
pub(crate) struct PreReceive {
    /// Why the whole push is refused, when a pre-receive rule refused it.
    pub refused: Option<String>,
    /// Why each ref is refused, by its index in the push, when a pre-ref-update rule refused it.
    pub refused_refs: BTreeMap<usize, String>,
    /// The other messages of the rules, for the client.
    pub messages: Vec<String>,
}

/// Run the pre events of a push against `view` (the repository with the pushed objects over it): pre-receive once
/// with every ref, then pre-ref-update for each ref the push may still make (`candidates`, by index). A ref write
/// through Moqui (`create#GitRef`, `merge#GitRefs`) runs them the same way, with its origin's source (`write`).
pub(crate) fn pre_receive(
    view: &dyn GitStore,
    repository_id: &str,
    branch: &str,
    origin: &UpdateOrigin,
    refs: &[RefUpdate],
    candidates: &[usize],
) -> PreReceive {
    let mut decided = PreReceive::default();
    let source = origin.source;
    let wants_receive = git_eca::listening(GitEvent::PreReceive, source);
    let wants_ref = git_eca::listening(GitEvent::PreRefUpdate, source);
    if !wants_receive && !wants_ref {
        return decided;
    }
    let branch_ref = ops::branch_ref(branch);
    let mut base = BTreeMap::new();
    base.insert("repositoryId".to_string(), Value::string(repository_id));
    base.insert("source".to_string(), Value::string(source));
    base.insert("transport".to_string(), Value::string(origin.transport));
    base.insert(
        "userId".to_string(),
        origin.user_id.clone().map_or(Value::Null, Value::string),
    );
    base.insert(
        "pushOptions".to_string(),
        Value::List(origin.push_options.iter().map(Value::string).collect()),
    );
    base.insert(
        "refUpdates".to_string(),
        Value::List(
            candidates
                .iter()
                .map(|index| ref_value(&refs[*index], &branch_ref))
                .collect(),
        ),
    );
    if wants_receive {
        let mut fields = base.clone();
        let tips: Vec<ObjectId> = candidates
            .iter()
            .filter_map(|index| refs[*index].new)
            .filter_map(|id| peel_to_commit(view, &id).ok().flatten())
            .collect();
        let hidden: Vec<ObjectId> = candidates.iter().filter_map(|index| refs[*index].old).collect();
        let new_commits: Vec<Value> = revwalk::new_commits(view, &tips, &hidden)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .take(max_commits())
            .filter_map(|commit| ops::commit_details(view, &commit.id).ok())
            .map(|details| commit_value(&details))
            .collect();
        fields.insert("newCommits".to_string(), Value::List(new_commits));
        let outcome = git_eca::run_git_rules(
            &event_key(GitEvent::PreReceive, source, repository_id, None, &branch_ref),
            &fields,
            None,
        );
        decided.messages.extend(outcome.messages);
        if !outcome.errors.is_empty() {
            decided.refused = Some(outcome.errors.join("; "));
            return decided;
        }
    }
    if wants_ref {
        for index in candidates {
            let reference = &refs[*index];
            let mut fields = base.clone();
            for (name, value) in ref_fields(reference, &branch_ref) {
                fields.insert(name.to_string(), value);
            }
            fields.insert("newCommitIds".to_string(), new_commit_ids(view, reference));
            let outcome = git_eca::run_git_rules(
                &event_key(
                    GitEvent::PreRefUpdate,
                    source,
                    repository_id,
                    Some(&reference.name),
                    &branch_ref,
                ),
                &fields,
                None,
            );
            decided.messages.extend(outcome.messages);
            if !outcome.errors.is_empty() {
                decided.refused_refs.insert(*index, outcome.errors.join("; "));
            }
        }
    }
    decided
}
