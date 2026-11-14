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

/// `sync#MergeRequestHead`: keep `refs/merge-requests/<num>/head` of a merge request's repository at its source's
/// head. Out: `headCommitId` (empty while a source branch is gone), `headRef`. With authorization disabled only.
fn sync_merge_request_head(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !authz_disabled() {
        add_error("sync#MergeRequestHead is called by the merge request services only");
        return none();
    }
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
    let Some((provider, id)) = provider_and_id(&scoped, true) else {
        return none();
    };
    let Some(num) = field(&row, "mergeRequestNum").and_then(|num| num.parse::<i64>().ok()) else {
        add_error(format!("merge request {mr_id} has no number"));
        return none();
    };
    let head_ref = merge_request_ref(num);
    let pushed = field(&row, "pushedForReview").as_deref() == Some("Y");
    let source_repository = field(&row, "sourceRepositoryId").unwrap_or_else(|| id.clone());
    let outcome = (|| -> Result<Option<ObjectId>, moqui_core::context::Error> {
        let head = if pushed {
            match text(parameters, "headCommitId") {
                Some(given) => {
                    let head = ObjectId::from_hex(given.trim().as_bytes())
                        .map_err(|_| moqui_core::context::Error::new(format!("{given} is not a commit id")))?;
                    // a commit of the store: the caller says it is the pusher's own
                    match provider.repo(&id)?.store.find(&head)? {
                        Some((gix::objs::Kind::Commit, _)) => Some(head),
                        _ => {
                            return Err(moqui_core::context::Error::new(format!(
                                "{given} is not a commit of repository {id}"
                            )))
                        }
                    }
                }
                None => provider.ref_of(&id, &head_ref)?.map(|(_, head)| head),
            }
        } else {
            let source_ref = format!("refs/heads/{}", field(&row, "sourceRef").unwrap_or_default());
            match provider.ref_of(&source_repository, &source_ref) {
                Ok(found) => found.map(|(_, head)| head),
                // a fork that was deleted: the head stays where it was
                Err(err) if err.message().contains("is not defined") => None,
                Err(err) => return Err(err),
            }
        };
        let Some(head) = head else {
            return Ok(None);
        };
        provider.share_objects(&source_repository, &id, head)?;
        provider.set_merge_request_head(&id, num, head)?;
        Ok(Some(head))
    })();
    match outcome {
        Ok(head) => Ok(out([
            (
                "headCommitId",
                head.map_or(Value::Null, |head| Value::string(head.to_string())),
            ),
            ("headRef", Value::string(head_ref)),
        ])),
        Err(err) => {
            add_refusal(err.message());
            none()
        }
    }
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
    // what its refs reach changed: read again on this node at once (another node within its two seconds)
    provider.forget_reached(&id);
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
    let Some((_, id)) = viewable(parameters) else {
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
