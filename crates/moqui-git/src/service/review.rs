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

//! Services for forks, the heads of merge requests and what an application asks about a commit (plans/Dogfood.md
//! DG4: F4, F5, N-CONTAINS, N-PUSHER): `fork#GitRepository`, `get#GitCommitRefs` and `get#GitCommitPushers` of
//! `org.moqui.git.GitServices`, and `sync#MergeRequestHead` and `set#MergeRequestHidden` of
//! `org.moqui.git.GitReviewServices`. Guide: `docs/interfaces/Git-Hosting.md`.

use std::collections::BTreeMap;

use gix::ObjectId;
use moqui_lib::object::{to_plain_string, Value};

use super::hosting::{allowed, list_param, none, Out, Parameters};
use super::{add_error, add_refusal, count, number, out, provider_and_id, text, viewable};
use moqui_core::context::ExecutionContext;
use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::rust_service;

use crate::reach::merge_request_ref;
use crate::revwalk;
use crate::rows_guard;
use crate::values::map;

const GIT_MERGE_REQUEST: &str = "moqui.resource.GitMergeRequest";
const GIT_COMMIT_PUSH: &str = "moqui.resource.GitCommitPush";

/// The commits one `get#GitCommitRefs` call reads at most (`git-server` `contains-max-commits`).
const CONTAINS_MAX_COMMITS: usize = 200_000;

/// The most commits one `get#GitCommitPushers` call answers.
const MAX_PUSHER_IDS: usize = 500;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, fork_repository)?;
    rust_service!(map, sync_merge_request_head)?;
    rust_service!(map, detach_merge_request_source)?;
    rust_service!(map, set_merge_request_hidden)?;
    rust_service!(map, get_commit_refs)?;
    rust_service!(map, get_commit_pushers)?;
    Ok(())
}

fn authz_disabled() -> bool {
    ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false)
}

fn field(row: &EntityValue, name: &str) -> Option<String> {
    row.get(name)
        .filter(|value| !moqui_lib::object::is_empty(value))
        .map(to_plain_string)
}

/// `fork#GitRepository`: a new repository `repositoryId` that is a fork of `sourceRepositoryId`, which the user may
/// view: of its kind, sharing its objects, with its branches and tags as they are now. Out: `forkNetworkId`,
/// `refCount`. Who may fork, into which id and path, and how many, are the application's to decide before it calls.
fn fork_repository(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let mut source = parameters.clone();
    match text(parameters, "sourceRepositoryId") {
        Some(id) => source.insert("repositoryId".into(), Value::string(id)),
        None => {
            add_error("sourceRepositoryId is required");
            return none();
        }
    };
    let Some((provider, source_id)) = viewable(&source) else {
        return none();
    };
    let Some(id) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    // where a fork's directory is made is the server's to say: a path is taken from the application only
    let local_path = text(parameters, "localPath");
    if local_path.is_some() && !authz_disabled() {
        add_error("localPath is given only with authorization disabled");
        return none();
    }
    // a row that cannot be written fails the service, so its transaction takes back the rows made before it
    let forked = provider.fork(
        &source_id,
        id.trim(),
        text(parameters, "description").as_deref(),
        local_path.as_deref(),
    );
    let refs = match forked {
        Ok(refs) => refs,
        Err(err) => {
            add_refusal(err.message());
            return none();
        }
    };
    let network = provider
        .definition(id.trim())
        .ok()
        .and_then(|definition| definition.network);
    Ok(out([
        ("repositoryId", Value::string(id.trim())),
        ("forkNetworkId", network.map_or(Value::Null, Value::string)),
        ("refCount", count(refs)),
    ]))
}

/// The merge request `mr_id`, or the error recorded.
fn merge_request(facade: &EntityFacade, mr_id: &str) -> Result<Option<EntityValue>, moqui_lib::error::Error> {
    let row = {
        let _rows = rows_guard();
        facade
            .find(GIT_MERGE_REQUEST)?
            .eq("mergeRequestId", Value::string(mr_id))
            .one()?
    };
    if row.is_none() {
        add_error(format!("no merge request {mr_id}"));
    }
    Ok(row)
}

fn set_detached(facade: &EntityFacade, mr_id: &str) -> Result<(), moqui_lib::error::Error> {
    let _rows = rows_guard();
    facade
        .find(GIT_MERGE_REQUEST)?
        .eq("mergeRequestId", Value::string(mr_id))
        .update_all([("sourceDetachedDate".to_string(), moqui_core::user::now_value())])?;
    Ok(())
}

/// `sync#MergeRequestHead`: keep `refs/merge-requests/<num>/head` of a merge request's repository at its source's
/// head. Out: `headCommitId` (empty while a source branch is gone), `headRef`, `startHidden`, `sourceDetached`. With
/// authorization disabled only.
///
/// An open one follows its source: the branch of its repository; the branch of a fork, while its author may still
/// update the fork and the source is not detached (else the head stays, and the merge request is marked detached);
/// or `headCommitId` for one pushed for review. One that is merged, or was closed within the retention time, only
/// gets its ref back when it has none (a merge request from before the refs were kept), from its recorded head.
fn sync_merge_request_head(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !authz_disabled() {
        add_error("sync#MergeRequestHead is called by the merge request services only");
        return none();
    }
    let Some(mr_id) = text(parameters, "mergeRequestId") else {
        add_error("mergeRequestId is required");
        return none();
    };
    let Some(row) = merge_request(facade, &mr_id)? else {
        return none();
    };
    let mut scoped = parameters.clone();
    scoped.insert(
        "repositoryId".into(),
        row.get("repositoryId").cloned().unwrap_or(Value::Null),
    );
    let Some((provider, id)) = provider_and_id(&scoped, true) else {
        return none();
    };
    let Some(num) = field(&row, "mergeRequestNum").and_then(|num| num.parse::<i64>().ok()) else {
        add_error(format!("merge request {mr_id} has no number"));
        return none();
    };
    let head_ref = merge_request_ref(num);
    let pushed = field(&row, "pushedForReview").as_deref() == Some("Y");
    let fork = field(&row, "sourceRepositoryId").filter(|source| *source != id);
    let status = field(&row, "statusId").unwrap_or_default();
    let open = matches!(status.as_str(), "GmrDraft" | "GmrOpen");
    let mut detached = field(&row, "sourceDetachedDate").is_some();
    let outcome = (|| -> Result<Option<ObjectId>, moqui_core::context::Error> {
        let repo = provider.repo(&id)?;
        let store = repo.store.as_ref();
        let current = store.read_ref(&head_ref)?;
        let commit_of = |hex: &str| -> Result<Option<ObjectId>, moqui_core::context::Error> {
            let Ok(head) = ObjectId::from_hex(hex.trim().as_bytes()) else {
                return Ok(None);
            };
            Ok(matches!(store.find(&head)?, Some((gix::objs::Kind::Commit, _))).then_some(head))
        };
        if !open {
            // only what a merge request from before the refs were kept lacks: its recorded head, while that is a
            // commit of the store and the ref was not released (GitProvider::release_closed_heads)
            let days: i64 = crate::number_setting("review-ref-retention-days", 30);
            let kept = status == "GmrMerged"
                || days <= 0
                || row.get("closedDate").is_none_or(|closed| {
                    let cutoff = moqui_lib::object::basic_convert(
                        &Value::I64(jiff::Timestamp::now().as_millisecond() - days * 24 * 60 * 60 * 1000),
                        "Timestamp",
                    )
                    .value;
                    moqui_lib::object::compare_obj(closed, &cutoff).is_ok_and(|order| order.is_gt())
                });
            if current.is_none() && kept {
                if let Some(head) = field(&row, "headCommitId")
                    .map(|hex| commit_of(&hex))
                    .transpose()?
                    .flatten()
                {
                    provider.set_merge_request_head(&id, num, head)?;
                    return Ok(Some(head));
                }
            }
            return Ok(current);
        }
        let head = if pushed {
            match text(parameters, "headCommitId") {
                // a commit of the store: the caller says it is the pusher's own
                Some(given) => Some(commit_of(&given)?.ok_or_else(|| {
                    moqui_core::context::Error::new(format!("{given} is not a commit of repository {id}"))
                })?),
                None => current,
            }
        } else if let Some(source) = &fork {
            // a fork's branch is followed while the author may still write there: a branch someone only read
            // once must not keep bringing its owner's later commits here
            let author = field(&row, "authorUserId").unwrap_or_default();
            if !detached {
                let may_still =
                    moqui_core::user::as_user(&author, || super::hosting::may(source, "update")).unwrap_or(false);
                if !may_still {
                    set_detached(facade, &mr_id).map_err(|err| moqui_core::context::Error::new(err.to_string()))?;
                    detached = true;
                }
            }
            if detached {
                current
            } else {
                let source_ref = format!("refs/heads/{}", field(&row, "sourceRef").unwrap_or_default());
                match provider.ref_of(source, &source_ref) {
                    Ok(found) => found.map(|(_, head)| head),
                    // a fork that was deleted: the head stays where it was
                    Err(err) if err.message().contains("is not defined") => current,
                    Err(err) => return Err(err),
                }
            }
        } else {
            let source_ref = format!("refs/heads/{}", field(&row, "sourceRef").unwrap_or_default());
            provider.ref_of(&id, &source_ref)?.map(|(_, head)| head)
        };
        let Some(head) = head else {
            return Ok(None);
        };
        if current != Some(head) {
            if let Some(source) = &fork {
                provider.share_objects(source, &id, head)?;
                // the files it adds that are kept in LFS become the target's too, within its quota
                let target = field(&row, "targetRef").unwrap_or_default();
                let base = store.read_ref(&crate::ops::branch_ref(&target))?;
                provider.share_lfs(source, &id, base, head)?;
            }
            provider.set_merge_request_head(&id, num, head)?;
        }
        Ok(Some(head))
    })();
    match outcome {
        Ok(head) => Ok(out([
            (
                "headCommitId",
                head.map_or(Value::Null, |head| Value::string(head.to_string())),
            ),
            ("headRef", Value::string(head_ref)),
            (
                "startHidden",
                Value::Bool(crate::server::review_starts_hidden() && (pushed || fork.is_some())),
            ),
            ("sourceDetached", Value::Bool(detached)),
        ])),
        Err(err) => {
            add_refusal(err.message());
            none()
        }
    }
}

/// `detach#MergeRequestSource`: a merge request from a fork's branch stops following the branch and keeps the head
/// it has (`sourceDetachedDate`). By a user with the delete right on the source repository (its owner), the
/// merge request's author, or with authorization disabled. Out: `sourceDetachedDate`.
fn detach_merge_request_source(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(mr_id) = text(parameters, "mergeRequestId") else {
        add_error("mergeRequestId is required");
        return none();
    };
    let Some(row) = merge_request(facade, &mr_id)? else {
        return none();
    };
    let Some(source) = field(&row, "sourceRepositoryId") else {
        add_error(format!("merge request {mr_id} is not from another repository"));
        return none();
    };
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let author = field(&row, "authorUserId").is_some_and(|author| !user.is_empty() && author == user);
    if !author && !authz_disabled() && !super::hosting::may(&source, "delete") {
        add_error(format!("no merge request {mr_id}"));
        return none();
    }
    if field(&row, "sourceDetachedDate").is_none() {
        set_detached(facade, &mr_id)?;
    }
    let row = merge_request(facade, &mr_id)?;
    Ok(out([(
        "sourceDetachedDate",
        row.and_then(|row| row.get("sourceDetachedDate").cloned())
            .unwrap_or(Value::Null),
    )]))
}

/// `set#MergeRequestHidden`: hide a merge request's head from fetches and reads by id, or show it again. Out:
/// `hiddenDate`. With authorization disabled, else by a user with the delete right.
fn set_merge_request_hidden(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(mr_id) = text(parameters, "mergeRequestId") else {
        add_error("mergeRequestId is required");
        return none();
    };
    let row = {
        let _rows = rows_guard();
        facade
            .find(GIT_MERGE_REQUEST)?
            .eq("mergeRequestId", Value::string(&mr_id))
            .one()?
    };
    let Some(row) = row else {
        add_error(format!("no merge request {mr_id}"));
        return none();
    };
    let mut scoped = parameters.clone();
    scoped.insert(
        "repositoryId".into(),
        row.get("repositoryId").cloned().unwrap_or(Value::Null),
    );
    let Some((provider, id)) = allowed(&scoped, "delete") else {
        return none();
    };
    let hidden = text(parameters, "hidden").is_none_or(|value| matches!(value.as_str(), "true" | "Y" | "y"));
    let date = if hidden {
        row.get("hiddenDate")
            .filter(|value| !moqui_lib::object::is_empty(value))
            .cloned()
            .unwrap_or_else(moqui_core::user::now_value)
    } else {
        Value::Null
    };
    {
        let _rows = rows_guard();
        facade
            .find(GIT_MERGE_REQUEST)?
            .eq("mergeRequestId", Value::string(&mr_id))
            .update_all([("hiddenDate".to_string(), date.clone())])?;
    }
    let was_hidden = field(&row, "hiddenDate").is_some();
    if hidden || was_hidden {
        // from the first one on, only the refs of the repository decide what is read by id
        provider.note_review_hidden(&id)?;
    }
    // what its refs reach changed: read again on this node at once (another node within its two seconds)
    provider.forget_reached(&id);
    if !hidden && was_hidden && matches!(field(&row, "statusId").as_deref(), Some("GmrDraft" | "GmrOpen")) {
        // its webhooks were told nothing while it was hidden: it is opened for them now
        let shown = merge_request(facade, &mr_id)?;
        let queued = shown.map(|shown| super::hosting::queue_merge_request_event(facade, &shown, "opened"));
        if let Some(Err(err)) = queued {
            tracing::error!(merge_request = %mr_id, "merge request webhooks not queued: {err}");
        }
    }
    Ok(out([("hiddenDate", date)]))
}

/// `get#GitCommitRefs`: the branches and tags of a repository that contain a commit (it is their head or an ancestor
/// of it), for a commit's page. At most `limit` (50, at most 500) refs are checked, the default branch first, then
/// branches and tags by name, reading at most `git-server` `contains-max-commits` (200,000) commits; `truncated`
/// when there were more refs or the commits ran out. Out: `commitId`, `refList` (each `refName`,
/// `refType`, `shortName`, `isDefaultBranch`), `truncated`; nothing when the revision names no commit a reader
/// reads.
fn get_commit_refs(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, id)) = viewable(parameters) else {
        return none();
    };
    let revision = text(parameters, "commitId").unwrap_or_default();
    let limit = number(parameters, "limit", 50).clamp(1, 500);
    let outcome = (|| -> Result<Option<(ObjectId, Vec<String>, bool)>, moqui_core::context::Error> {
        let Some(commit) = provider.resolve_commit(&id, &revision)? else {
            return Ok(None);
        };
        let repo = provider.repo(&id)?;
        let store = repo.store.as_ref();
        let default_ref = crate::ops::branch_ref(&repo.definition.branch);
        let mut refs = store.list_refs("refs/heads/")?;
        refs.sort_by_key(|(name, _)| (*name != default_ref, name.clone()));
        refs.extend(store.list_refs("refs/tags/")?);
        let mut truncated = refs.len() > limit;
        refs.truncate(limit);
        // each tip is walked back until the commit is found; what one walk found not to reach it the next does not
        // read again, so the tips together read the history once, within the budget
        let mut budget: usize = crate::number_setting("contains-max-commits", CONTAINS_MAX_COMMITS).max(1);
        let mut not = std::collections::HashSet::new();
        let mut containing = Vec::new();
        for (name, target) in refs {
            let Some(tip) = revwalk::peel_to_commit(store, &target)? else {
                continue;
            };
            match crate::reach::reaches(store, &commit, &tip, &mut not, &mut budget)? {
                Some(true) => containing.push(name),
                Some(false) => {}
                None => {
                    truncated = true;
                    break;
                }
            }
        }
        Ok(Some((commit, containing, truncated)))
    })();
    match outcome {
        Ok(Some((commit, names, truncated))) => {
            let branch_ref = provider
                .definition(&id)
                .map(|definition| crate::ops::branch_ref(&definition.branch))
                .unwrap_or_default();
            let list = names
                .iter()
                .map(|name| {
                    map([
                        ("refName", Value::string(name)),
                        ("refType", Value::string(crate::browse::RefKind::of(name).name())),
                        ("shortName", Value::string(crate::browse::short_name(name))),
                        ("isDefaultBranch", Value::Bool(*name == branch_ref)),
                    ])
                })
                .collect();
            Ok(out([
                ("commitId", Value::string(commit.to_string())),
                ("refList", Value::List(list)),
                ("truncated", Value::Bool(truncated)),
            ]))
        }
        Ok(None) => none(),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `get#GitCommitPushers`: who brought each of `commitIdList` (at most 500) to the repository: the user of the
/// first push or write whose refs reached it (`GitCommitPush`, by its key). Out: `pusherList` (each `commitId`,
/// `userId`, `username`, `updateId`, `pushDate`), with no entry for a commit that came by a mirror fetch or an
/// import, or before pushers were kept.
fn get_commit_pushers(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((provider, id)) = viewable(parameters) else {
        return none();
    };
    let mut ids = list_param(parameters, "commitIdList");
    if let Some(one) = text(parameters, "commitId") {
        ids.push(one);
    }
    ids.sort();
    ids.dedup();
    if ids.len() > MAX_PUSHER_IDS {
        add_error(format!("at most {MAX_PUSHER_IDS} commits in one call"));
        return none();
    }
    // of the commits the caller reads by id: who pushed a commit says that it is there
    let repo = provider.repo(&id)?;
    let mut readable = Vec::new();
    for hex in ids {
        if let Ok(commit) = ObjectId::from_hex(hex.as_bytes()) {
            if provider.readable_by_id(&repo, &commit)? {
                readable.push(hex);
            }
        }
    }
    let ids = readable;
    if ids.is_empty() {
        return Ok(out([("pusherList", Value::List(Vec::new()))]));
    }
    let _rows = rows_guard();
    let rows = facade
        .find(GIT_COMMIT_PUSH)?
        .eq("repositoryId", Value::string(&id))
        .condition(Condition::is_in("commitId", ids.iter().map(Value::string).collect()))
        .list()?;
    let mut usernames: BTreeMap<String, Value> = BTreeMap::new();
    let mut list = Vec::new();
    for row in rows.iter() {
        let user_id = field(row, "userId").unwrap_or_default();
        let username = match usernames.get(&user_id) {
            Some(known) => known.clone(),
            None => {
                let found = facade
                    .find("moqui.security.UserAccount")?
                    .eq("userId", Value::string(&user_id))
                    .one()?
                    .and_then(|account| field(&account, "username"))
                    .map_or(Value::Null, Value::string);
                usernames.insert(user_id.clone(), found.clone());
                found
            }
        };
        list.push(map([
            ("commitId", row.get("commitId").cloned().unwrap_or(Value::Null)),
            ("userId", Value::string(&user_id)),
            ("username", username),
            ("updateId", row.get("updateId").cloned().unwrap_or(Value::Null)),
            ("pushDate", row.get("pushDate").cloned().unwrap_or(Value::Null)),
        ]));
    }
    Ok(out([("pusherList", Value::List(list))]))
}
