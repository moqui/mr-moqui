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

//! Hosting services of `org.moqui.impl.GitServices` (and `GitReviewServices.merge#MergeRequest`): the repository
//! list and push log, a directory with each entry's last commit, refs, compare, merge, archive, commit statuses and
//! webhooks. No Java counterpart. Each checks the repository's rights (`AT_GIT_REPOSITORY`, or the drive's rule for a
//! drive's repository) as the git server does; the rows are `moqui.*` entities that entity filters do not reach, so
//! they are read only here. Guide: `docs/Git-Hosting.md`.

use std::collections::{BTreeMap, HashMap, HashSet};

use gix::ObjectId;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::git::{
    add_error, add_message, change_value, count, flag, number, out, path_segments, provider_and_id, text, viewable,
};
use crate::context::ExecutionContext;
use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::resource::git::hosting::{MergeOutcome, RefWrite};
use crate::resource::git::merge::{Mergeability, Method};
use crate::resource::git::rules::{self, BranchRule};
use crate::resource::git::values::{commit_value, map, opt_text, timestamp};
use crate::resource::git::{rows_guard, GitProvider, GIT_REPOSITORY};

const GIT_COMMIT_STATUS: &str = "moqui.resource.GitCommitStatus";
const GIT_WEBHOOK: &str = "moqui.resource.GitWebhook";
const GIT_MERGE_REQUEST: &str = "moqui.resource.GitMergeRequest";
const GIT_MERGE_REQUEST_REVIEWER: &str = "moqui.resource.GitMergeRequestReviewer";
const SYSTEM_MESSAGE: &str = "moqui.service.message.SystemMessage";

/// The states of a commit status, as GitHub's.
const STATES: [&str; 4] = ["pending", "success", "failure", "error"];

/// Webhook calls give up after this long.
const WEBHOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, get_repository_access)?;
    rust_service!(map, find_repositories)?;
    rust_service!(map, find_updates)?;
    rust_service!(map, get_tree)?;
    rust_service!(map, create_ref)?;
    rust_service!(map, delete_ref)?;
    rust_service!(map, get_compare)?;
    rust_service!(map, merge_refs)?;
    rust_service!(map, archive_repository)?;
    rust_service!(map, unarchive_repository)?;
    rust_service!(map, update_repository)?;
    rust_service!(map, get_branch_rules)?;
    rust_service!(map, create_commit_status)?;
    rust_service!(map, get_commit_statuses)?;
    rust_service!(map, queue_webhook_push)?;
    rust_service!(map, queue_webhook_event)?;
    rust_service!(map, send_webhook_delivery)?;
    rust_service!(map, test_webhook)?;
    rust_service!(map, merge_merge_request)?;
    rust_service!(map, get_merge_request_approvals)?;
    Ok(())
}

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, LibError>;

fn none() -> Out {
    Ok(BTreeMap::new())
}

fn field(row: &EntityValue, name: &str) -> Option<String> {
    row.get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .filter(|text| !text.trim().is_empty())
}

fn field_value(row: &EntityValue, name: &str) -> Value {
    row.get(name).cloned().unwrap_or(Value::Null)
}

fn opt(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::string)
}

fn id_value(id: Option<ObjectId>) -> Value {
    id.map_or(Value::Null, |id| Value::string(id.to_string()))
}

/// A list parameter: a List, or a comma separated text.
fn list_param(parameters: &Parameters, key: &str) -> Vec<String> {
    match parameters.get(key) {
        Some(Value::List(items)) => items
            .iter()
            .map(to_plain_string)
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect(),
        Some(value) if !is_empty(value) => to_plain_string(value)
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// A Boolean parameter that is unset (`None`), or true or false.
fn tri(parameters: &Parameters, key: &str) -> Option<bool> {
    text(parameters, key).map(|value| matches!(value.as_str(), "true" | "Y" | "y"))
}

/// Whether the current user may do `action` (view, create, update, delete) on the repository, by its authorization
/// or as a member of the drive it holds.
fn may(repository_id: &str, action: &str) -> bool {
    crate::resource::git::server::permitted(repository_id, action)
}

/// The provider and repository id when the user may do `action`; otherwise the error recorded.
fn allowed(parameters: &Parameters, action: &str) -> Option<(GitProvider, String)> {
    let (provider, id) = provider_and_id(parameters, true)?;
    if action == "view" {
        return viewable(parameters);
    }
    if may(&id, action) {
        return Some((provider, id));
    }
    if may(&id, "view") {
        add_error(format!("you may not {action} in repository {id}"));
    } else {
        add_error(format!("repository {id} not found"));
    }
    None
}

fn current_user() -> Option<String> {
    crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
}

fn username_of(entity: &EntityFacade, user_id: &str) -> Option<String> {
    let _rows = rows_guard();
    entity
        .find("moqui.security.UserAccount")
        .ok()?
        .eq("userId", Value::string(user_id))
        .one()
        .ok()
        .flatten()
        .and_then(|row| field(&row, "username"))
}

fn repository_row(entity: &EntityFacade, id: &str) -> Result<Option<EntityValue>, LibError> {
    let _rows = rows_guard();
    entity.find(GIT_REPOSITORY)?.eq("repositoryId", Value::string(id)).one()
}

/// The page of a list: `pageIndex` and `pageSize` (20; `pageNoLimit` for all).
fn page(parameters: &Parameters) -> (usize, usize) {
    if flag(parameters, "pageNoLimit") {
        return (0, usize::MAX);
    }
    (
        number(parameters, "pageIndex", 0),
        number(parameters, "pageSize", 20).max(1),
    )
}

// ---- access, the repository list and the push log ----

/// `get#GitRepositoryAccess`: what the current user may do in a repository. Out: `canView`, `canCreate`,
/// `canUpdate`, `canDelete`, `archived`.
fn get_repository_access(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((_, id)) = provider_and_id(parameters, true) else {
        return none();
    };
    let archived = repository_row(facade, &id)
        .ok()
        .flatten()
        .is_some_and(|row| field(&row, "archivedDate").is_some());
    Ok(out([
        ("canView", Value::Bool(may(&id, "view"))),
        ("canCreate", Value::Bool(may(&id, "create"))),
        ("canUpdate", Value::Bool(may(&id, "update"))),
        ("canDelete", Value::Bool(may(&id, "delete"))),
        ("archived", Value::Bool(archived)),
    ]))
}

/// `find#GitRepositories`: the repositories the user may view, by id, paged; archived ones only when asked. Out:
/// `repositoryList`, `repositoryListCount`.
fn find_repositories(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let wanted: Vec<String> = list_param(parameters, "repositoryIdList");
    let search = text(parameters, "text").map(|text| text.to_lowercase());
    let kind = text(parameters, "repositoryKind");
    let archived_only = flag(parameters, "archivedOnly");
    let include_archived = archived_only || flag(parameters, "includeArchived");
    let rows = {
        let _rows = rows_guard();
        let mut find = facade.find(GIT_REPOSITORY)?.order_by("repositoryId", false);
        if !wanted.is_empty() {
            find = find.condition(Condition::is_in(
                "repositoryId",
                wanted.iter().map(Value::string).collect(),
            ));
        }
        if let Some(kind) = &kind {
            find = find.eq("repositoryKind", Value::string(kind));
        }
        find.list()?
    };
    let mut visible: Vec<EntityValue> = Vec::new();
    for row in rows.iter() {
        let id = field(row, "repositoryId").unwrap_or_default();
        let archived = field(row, "archivedDate").is_some();
        if (archived && !include_archived) || (archived_only && !archived) {
            continue;
        }
        if let Some(search) = &search {
            let haystack = format!(
                "{} {}",
                id.to_lowercase(),
                field(row, "description").unwrap_or_default().to_lowercase()
            );
            if !haystack.contains(search.as_str()) {
                continue;
            }
        }
        if may(&id, "view") {
            visible.push(row.clone());
        }
    }
    let total = visible.len();
    let (index, size) = page(parameters);
    let shown: Vec<EntityValue> = visible
        .into_iter()
        .skip(index.saturating_mul(size))
        .take(size)
        .collect();
    // open merge requests of the page's repositories, in one query
    let mut open: HashMap<String, i64> = HashMap::new();
    if !shown.is_empty() {
        let _rows = rows_guard();
        let ids: Vec<Value> = shown
            .iter()
            .filter_map(|row| field(row, "repositoryId"))
            .map(Value::string)
            .collect();
        for row in facade
            .find(GIT_MERGE_REQUEST)?
            .condition(Condition::is_in("repositoryId", ids))
            .condition(Condition::is_in(
                "statusId",
                vec![Value::string("GmrOpen"), Value::string("GmrDraft")],
            ))
            .select_field("repositoryId")
            .select_field("mergeRequestId")
            .list()?
            .iter()
        {
            *open.entry(field(row, "repositoryId").unwrap_or_default()).or_default() += 1;
        }
    }
    let list: Vec<Value> = shown
        .iter()
        .map(|row| {
            let id = field(row, "repositoryId").unwrap_or_default();
            map([
                ("repositoryId", Value::string(&id)),
                ("description", field_value(row, "description")),
                (
                    "repositoryKind",
                    Value::string(field(row, "repositoryKind").unwrap_or_else(|| "local".into())),
                ),
                (
                    "branch",
                    Value::string(field(row, "branch").unwrap_or_else(|| "main".into())),
                ),
                ("headCommitId", field_value(row, "headCommitId")),
                ("lastUpdateDate", field_value(row, "lastUpdateDate")),
                ("archivedDate", field_value(row, "archivedDate")),
                ("defaultMergeMethodEnumId", field_value(row, "defaultMergeMethodEnumId")),
                ("remoteUrl", field_value(row, "remoteUrl")),
                ("openMergeRequestCount", Value::I64(open.get(&id).copied().unwrap_or(0))),
            ])
        })
        .collect();
    Ok(out([
        ("repositoryList", Value::List(list)),
        ("repositoryListCount", count(total)),
        ("pageIndex", count(index)),
        ("pageSize", count(if size == usize::MAX { total } else { size })),
    ]))
}

/// `find#GitUpdates`: a repository's push log, newest first, each with its refs. Out: `updateList`,
/// `updateListCount`.
fn find_updates(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((_, id)) = viewable(parameters) else {
        return none();
    };
    let (index, size) = page(parameters);
    let _rows = rows_guard();
    let mut find = facade
        .find(crate::resource::git::events::GIT_UPDATE)?
        .eq("repositoryId", Value::string(&id));
    if let Some(source) = text(parameters, "source") {
        find = find.eq("source", Value::string(source));
    }
    let total = find.clone().count()?;
    let rows = find
        .order_by("updateDate", true)
        .order_by("updateId", true)
        .offset(i64::try_from(index.saturating_mul(size)).unwrap_or(i64::MAX))
        .limit(i64::try_from(size).unwrap_or(i64::MAX))
        .list()?;
    let ids: Vec<Value> = rows
        .iter()
        .filter_map(|row| field(row, "updateId"))
        .map(Value::string)
        .collect();
    let mut refs: HashMap<String, Vec<Value>> = HashMap::new();
    if !ids.is_empty() {
        for row in facade
            .find(crate::resource::git::events::GIT_UPDATE_REF)?
            .condition(Condition::is_in("updateId", ids))
            .order_by("refName", false)
            .list()?
            .iter()
        {
            let name = field(row, "refName").unwrap_or_default();
            refs.entry(field(row, "updateId").unwrap_or_default())
                .or_default()
                .push(map([
                    ("refName", Value::string(&name)),
                    (
                        "shortName",
                        Value::string(crate::resource::git::browse::short_name(&name)),
                    ),
                    ("changeType", field_value(row, "changeType")),
                    ("oldObjectId", field_value(row, "oldObjectId")),
                    ("newObjectId", field_value(row, "newObjectId")),
                    ("result", field_value(row, "result")),
                ]));
        }
    }
    let mut users: HashMap<String, Option<String>> = HashMap::new();
    let list: Vec<Value> = rows
        .iter()
        .map(|row| {
            let update_id = field(row, "updateId").unwrap_or_default();
            let user_id = field(row, "userId");
            let username = user_id.as_ref().and_then(|user| {
                users
                    .entry(user.clone())
                    .or_insert_with(|| username_of(facade, user))
                    .clone()
            });
            map([
                ("updateId", Value::string(&update_id)),
                ("source", field_value(row, "source")),
                ("transport", field_value(row, "transport")),
                ("userId", opt(user_id)),
                ("username", opt(username)),
                ("remoteAddress", field_value(row, "remoteAddress")),
                ("updateDate", field_value(row, "updateDate")),
                ("ecaStatus", field_value(row, "ecaStatus")),
                ("ecaError", field_value(row, "ecaError")),
                ("processedDate", field_value(row, "processedDate")),
                ("refList", Value::List(refs.remove(&update_id).unwrap_or_default())),
            ])
        })
        .collect();
    Ok(out([
        ("updateList", Value::List(list)),
        ("updateListCount", Value::I64(total)),
    ]))
}

// ---- the tree, refs, compare, merge ----

fn commit_summary(commit: &crate::resource::git::ops::CommitDetails) -> Value {
    map([
        ("commitId", Value::string(commit.id.to_string())),
        ("shortId", Value::string(commit.id.to_string()[..10].to_string())),
        ("summary", Value::string(commit.summary())),
        ("authorName", opt_text(&commit.author.name)),
        ("authorEmail", opt_text(&commit.author.email)),
        ("authorDate", timestamp(commit.author.seconds)),
        ("committerDate", timestamp(commit.committer.seconds)),
        (
            "moquiUserId",
            crate::resource::git::ops::user_of(&commit.message).map_or(Value::Null, Value::string),
        ),
    ])
}

/// `get#GitTree`: the directory at `path` in `revision`, each entry with its last commit. Out: `commitId`, `branch`,
/// `isFile`, `entryList`; nothing when the revision or the path names nothing.
fn get_tree(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, id)) = viewable(parameters) else {
        return none();
    };
    let revision = text(parameters, "revision").unwrap_or_default();
    match provider.tree(&id, &revision, &path_segments(parameters, "path")) {
        Ok(Some(listing)) => Ok(out([
            ("commitId", Value::string(listing.commit.to_string())),
            ("branch", opt(listing.branch)),
            ("isFile", Value::Bool(listing.is_file)),
            (
                "entryList",
                Value::List(
                    listing
                        .entries
                        .iter()
                        .map(|entry| {
                            map([
                                ("name", Value::string(&entry.name)),
                                ("path", Value::string(&entry.path)),
                                ("type", Value::string(entry.kind)),
                                ("objectId", Value::string(entry.id.to_string())),
                                (
                                    "size",
                                    entry.size.map_or(Value::Null, |size| {
                                        Value::I64(i64::try_from(size).unwrap_or(i64::MAX))
                                    }),
                                ),
                                ("lfs", Value::Bool(entry.lfs_oid.is_some())),
                                ("lfsOid", opt(entry.lfs_oid.clone())),
                                ("lastCommit", entry.last.as_ref().map_or(Value::Null, commit_summary)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])),
        Ok(None) => none(),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// A ref name from a parameter and its type: `refs/...` as it is, else a branch (or a tag with `refType` tag).
fn full_ref(name: &str, ref_type: Option<&str>) -> String {
    if name.starts_with("refs/") {
        name.to_string()
    } else if ref_type == Some("tag") {
        format!("refs/tags/{name}")
    } else {
        format!("refs/heads/{name}")
    }
}

fn valid_ref(name: &str) -> bool {
    use gix::bstr::ByteSlice;
    (name.starts_with("refs/heads/") || name.starts_with("refs/tags/"))
        && gix::validate::reference::name(name.as_bytes().as_bstr()).is_ok()
}

/// `create#GitRef`: a branch, or a tag (annotated when a message is given), at a revision. Needs create. Out:
/// `refName`, `objectId`, `commitId`.
fn create_ref(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, id)) = allowed(parameters, "create") else {
        return none();
    };
    let Some(name) = text(parameters, "refName") else {
        add_error("refName is required");
        return none();
    };
    let ref_type = text(parameters, "refType");
    let full = full_ref(name.trim(), ref_type.as_deref());
    if !valid_ref(&full) {
        add_error(format!("{name} is not a valid branch or tag name"));
        return none();
    }
    let revision = text(parameters, "revision").unwrap_or_default();
    let outcome = (|| -> Result<Result<(ObjectId, ObjectId), String>, crate::context::Error> {
        let Some(commit) = provider.resolve_commit(&id, &revision)? else {
            return Ok(Err(format!("{revision} names no commit in repository {id}")));
        };
        if provider.ref_of(&id, &full)?.is_some() {
            return Ok(Err(format!(
                "{} already exists",
                crate::resource::git::browse::short_name(&full)
            )));
        }
        let target = match text(parameters, "message").filter(|_| full.starts_with("refs/tags/")) {
            Some(message) => {
                provider.write_tag(&id, crate::resource::git::browse::short_name(&full), commit, &message)?
            }
            None => commit,
        };
        let written = provider.write_refs(
            &id,
            &[RefWrite {
                name: full.clone(),
                expected: None,
                new: Some(target),
            }],
            false,
            None,
        )?;
        Ok(match written.into_iter().next() {
            Some(Ok(())) => Ok((target, commit)),
            Some(Err(reason)) => Err(reason),
            None => Err("no ref written".into()),
        })
    })();
    match outcome {
        Ok(Ok((target, commit))) => Ok(out([
            ("refName", Value::string(&full)),
            ("objectId", Value::string(target.to_string())),
            ("commitId", Value::string(commit.to_string())),
        ])),
        Ok(Err(reason)) => {
            add_error(reason);
            none()
        }
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `delete#GitRef`: delete a branch or tag. Needs delete; the default branch cannot be deleted.
fn delete_ref(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, id)) = allowed(parameters, "delete") else {
        return none();
    };
    let Some(name) = text(parameters, "refName") else {
        add_error("refName is required");
        return none();
    };
    let outcome = (|| -> Result<Result<(), String>, crate::context::Error> {
        let full = if name.starts_with("refs/") {
            Some(name.clone())
        } else if let Some(ref_type) = text(parameters, "refType") {
            Some(full_ref(&name, Some(&ref_type)))
        } else {
            provider.ref_of(&id, &name)?.map(|(full, _)| full)
        };
        let Some(full) = full else {
            return Ok(Err(format!("no branch or tag {name}")));
        };
        let Some((_, current)) = provider.ref_of(&id, &full)? else {
            return Ok(Err(format!("no branch or tag {name}")));
        };
        let written = provider.write_refs(
            &id,
            &[RefWrite {
                name: full,
                expected: Some(current),
                new: None,
            }],
            false,
            None,
        )?;
        Ok(written.into_iter().next().unwrap_or(Err("no ref written".into())))
    })();
    match outcome {
        Ok(Ok(())) => none(),
        Ok(Err(reason)) => {
            add_error(reason);
            none()
        }
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

fn mergeability_entries(checked: &Mergeability) -> Vec<(&'static str, Value)> {
    vec![
        (
            "mergeable",
            Value::Bool(!checked.merged && checked.conflicts.is_empty()),
        ),
        ("alreadyMerged", Value::Bool(checked.merged)),
        ("fastForward", Value::Bool(checked.fast_forward)),
        ("baseCommitId", id_value(checked.base)),
        (
            "conflictPaths",
            Value::List(checked.conflicts.iter().map(Value::string).collect()),
        ),
    ]
}

/// `get#GitCompare`: what `head` has that `base` lacks: the merge base, the commits (oldest first), ahead and
/// behind, the files changed from the merge base with their stats, and with `withDiffs` each file's diff as the
/// `moqui-diff` html and as hunks of numbered lines. With `checkMergeable` a trial merge of head into base. Out:
/// nothing when either names no commit.
fn get_compare(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, id)) = viewable(parameters) else {
        return none();
    };
    let base = text(parameters, "base").unwrap_or_default();
    let Some(head) = text(parameters, "head") else {
        add_error("head is required");
        return none();
    };
    let with_diffs = flag(parameters, "withDiffs");
    match provider.compare(&id, &base, &head, with_diffs, flag(parameters, "checkMergeable")) {
        Ok(Some(compared)) => {
            let (mut insertions, mut deletions) = (0, 0);
            let files: Vec<Value> = compared
                .files
                .iter()
                .map(|(change, file)| {
                    let mut value = match file {
                        Some(file) => change_value(change, Some(file)),
                        None => change_value(change, None),
                    };
                    if let (Value::Map(entries), Some(Some(text))) = (&mut value, file.as_ref().map(|file| &file.diff))
                    {
                        insertions += text.stats.insertions;
                        deletions += text.stats.deletions;
                        if with_diffs {
                            entries.insert("html".to_string(), Value::string(text.render_html()));
                            if let Value::Map(diff) = text.to_value() {
                                if let Some(hunks) = diff.get("hunks") {
                                    entries.insert("hunks".to_string(), hunks.clone());
                                }
                            }
                        }
                    }
                    if let Value::Map(entries) = &mut value {
                        entries.insert("diffed".to_string(), Value::Bool(file.is_some()));
                    }
                    value
                })
                .collect();
            let mut entries = vec![
                ("baseCommitId", Value::string(compared.base.to_string())),
                ("headCommitId", Value::string(compared.head.to_string())),
                ("mergeBaseId", id_value(compared.merge_base)),
                ("aheadBy", count(compared.ahead)),
                ("behindBy", count(compared.behind)),
                (
                    "commitList",
                    Value::List(compared.commits.iter().map(commit_value).collect()),
                ),
                ("commitsTruncated", Value::Bool(compared.ahead > compared.commits.len())),
                ("fileList", Value::List(files)),
                ("insertions", count(insertions)),
                ("deletions", count(deletions)),
            ];
            if let Some(checked) = &compared.mergeability {
                entries.extend(mergeability_entries(checked));
            }
            Ok(out(entries))
        }
        Ok(None) => none(),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `merge#GitRefs`: merge a source revision into a target branch by `method` (a merge commit by default,
/// fast-forward, squash or rebase), as the current user; refused on a conflict, by the branch rules (a protected
/// target changes through a merge request), or when the target already has the source. With `checkOnly` a trial
/// merge that changes nothing (view is enough). Out: `commitId` (the target's new head), `madeCommitIds`, and the
/// mergeability: `mergeable`, `alreadyMerged`, `fastForward`, `baseCommitId`, `conflictPaths`.
fn merge_refs(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let check_only = flag(parameters, "checkOnly");
    let Some((provider, id)) = allowed(parameters, if check_only { "view" } else { "update" }) else {
        return none();
    };
    let (Some(source), Some(target)) = (text(parameters, "source"), text(parameters, "target")) else {
        add_error("source and target are required");
        return none();
    };
    let Some(method) = Method::parse(&text(parameters, "method").unwrap_or_default()) else {
        add_error("method must be GmmMerge, GmmFastForward, GmmSquash or GmmRebase");
        return none();
    };
    let message = text(parameters, "message");
    match provider.merge_refs(
        &id,
        &source,
        &target,
        method,
        message.as_deref(),
        None,
        check_only,
        false,
    ) {
        Ok(outcome) => Ok(merge_out(outcome)),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

fn merge_out(outcome: MergeOutcome) -> BTreeMap<String, Value> {
    match outcome {
        MergeOutcome::Checked(checked) => out(mergeability_entries(&checked)),
        MergeOutcome::Merged(merged) => out([
            ("commitId", Value::string(merged.head.to_string())),
            (
                "madeCommitIds",
                Value::List(merged.made.iter().map(|id| Value::string(id.to_string())).collect()),
            ),
            ("mergeable", Value::Bool(true)),
            ("conflictPaths", Value::List(Vec::new())),
        ]),
        MergeOutcome::Refused { reason, conflicts } => {
            add_error(reason);
            out([
                ("mergeable", Value::Bool(false)),
                (
                    "conflictPaths",
                    Value::List(conflicts.iter().map(Value::string).collect()),
                ),
            ])
        }
    }
}

// ---- the repository: archive and settings ----

fn set_row(facade: &EntityFacade, id: &str, fields: &[(&str, Value)]) -> Result<(), LibError> {
    let _rows = rows_guard();
    let mut row = EntityValue::make(facade, GIT_REPOSITORY)?;
    row.set("repositoryId", Value::string(id));
    for (name, value) in fields {
        row.set(*name, value.clone());
    }
    row.update(facade)?;
    Ok(())
}

/// The open (and draft) merge requests of a repository closed, as it is archived. How many.
fn close_open_merge_requests(facade: &EntityFacade, id: &str) -> Result<i64, LibError> {
    let _rows = rows_guard();
    facade
        .find(GIT_MERGE_REQUEST)?
        .eq("repositoryId", Value::string(id))
        .condition(Condition::is_in(
            "statusId",
            vec![Value::string("GmrOpen"), Value::string("GmrDraft")],
        ))
        .update_all([
            ("statusId".to_string(), Value::string("GmrClosed")),
            ("closedDate".to_string(), crate::user::now_value()),
            ("lastUpdatedDate".to_string(), crate::user::now_value()),
        ])
}

/// `archive#GitRepository`: read only from now (pushes, ref writes, merges and writes refused), its open merge
/// requests closed, left out of the lists. Needs delete.
fn archive_repository(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((provider, id)) = allowed(parameters, "delete") else {
        return none();
    };
    let outcome = set_row(facade, &id, &[("archivedDate", crate::user::now_value())])
        .and_then(|()| close_open_merge_requests(facade, &id));
    provider.row_changed(&id);
    match outcome {
        Ok(closed) => {
            add_message(format!("Archived repository {id}; {closed} open merge requests closed"));
            Ok(out([("closedMergeRequestCount", Value::I64(closed))]))
        }
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `unarchive#GitRepository`: writable again. Needs delete.
fn unarchive_repository(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((provider, id)) = allowed(parameters, "delete") else {
        return none();
    };
    let outcome = set_row(facade, &id, &[("archivedDate", Value::Null)]);
    provider.row_changed(&id);
    if let Err(err) = outcome {
        add_error(err.message());
    }
    none()
}

/// `update#GitRepository`: the description, the default branch (an existing branch) and the default merge method
/// (GmmMerge or GmmSquash). Needs delete.
fn update_repository(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((provider, id)) = allowed(parameters, "delete") else {
        return none();
    };
    let mut fields: Vec<(&str, Value)> = Vec::new();
    if parameters.contains_key("description") {
        fields.push(("description", opt(text(parameters, "description"))));
    }
    if let Some(method) = text(parameters, "defaultMergeMethodEnumId") {
        if !matches!(method.as_str(), "GmmMerge" | "GmmSquash") {
            add_error("the default merge method is GmmMerge or GmmSquash");
            return none();
        }
        fields.push(("defaultMergeMethodEnumId", Value::string(method)));
    }
    let branch = text(parameters, "branch").map(|branch| branch.trim_start_matches("refs/heads/").to_string());
    if let Some(branch) = &branch {
        match provider.ref_of(&id, &format!("refs/heads/{branch}")) {
            Ok(Some(_)) => fields.push(("branch", Value::string(branch))),
            Ok(None) => {
                add_error(format!("no branch {branch} in repository {id}"));
                return none();
            }
            Err(err) => {
                add_error(err.message());
                return none();
            }
        }
    }
    if fields.is_empty() {
        return none();
    }
    let outcome = set_row(facade, &id, &fields);
    if let (Ok(()), Some(branch)) = (&outcome, &branch) {
        provider.change_default_branch(&id, branch);
    }
    provider.row_changed(&id);
    if let Err(err) = outcome {
        add_error(err.message());
    }
    none()
}

// ---- branch rules ----

fn rule_value(rule: &BranchRule) -> Value {
    map([
        ("branchRuleId", Value::string(&rule.id)),
        ("refPattern", Value::string(&rule.pattern)),
        ("allowForcePush", Value::Bool(rule.allow_force_push)),
        ("allowDelete", Value::Bool(rule.allow_delete)),
        ("requireMergeRequest", Value::Bool(rule.require_merge_request)),
        ("requiredApprovals", Value::I64(i64::from(rule.required_approvals))),
        ("dismissStaleApprovals", Value::Bool(rule.dismiss_stale_approvals)),
        (
            "requiredStatusContexts",
            Value::List(rule.required_contexts.iter().map(Value::string).collect()),
        ),
        (
            "allowedMergeMethods",
            Value::List(rule.allowed_methods.iter().map(Value::string).collect()),
        ),
        ("pushUserGroupId", opt(rule.push_group.clone())),
    ])
}

/// `get#GitBranchRules`: the branch rules of a repository that name a ref (all of them without one), and what they
/// ask together. Out: `ruleList`, `protected`, `requireMergeRequest`, `requiredApprovals`, `dismissStaleApprovals`,
/// `requiredStatusContexts`, `allowedMergeMethods` (empty: any), `mayPushDirectly`.
fn get_branch_rules(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((_, id)) = viewable(parameters) else {
        return none();
    };
    let found: Vec<BranchRule> = match text(parameters, "refName") {
        Some(name) => {
            let full = full_ref(&name, text(parameters, "refType").as_deref());
            match rules::rules_for(facade, &id, &full) {
                Ok(found) => found,
                Err(err) => {
                    add_error(err.message());
                    return none();
                }
            }
        }
        None => {
            let _rows = rows_guard();
            facade
                .find(rules::GIT_BRANCH_RULE)?
                .eq("repositoryId", Value::string(&id))
                .order_by("refPattern", false)
                .list()?
                .iter()
                .map(BranchRule::of)
                .collect()
        }
    };
    let mut contexts: Vec<String> = Vec::new();
    let mut methods: Option<Vec<String>> = None;
    for rule in &found {
        for context in &rule.required_contexts {
            if !contexts.contains(context) {
                contexts.push(context.clone());
            }
        }
        if !rule.allowed_methods.is_empty() {
            methods = Some(match methods {
                None => rule.allowed_methods.clone(),
                Some(kept) => kept
                    .into_iter()
                    .filter(|method| rule.allowed_methods.contains(method))
                    .collect(),
            });
        }
    }
    let direct = text(parameters, "refName").is_none_or(|name| {
        let full = full_ref(&name, text(parameters, "refType").as_deref());
        rules::check_direct(&found, &full, rules::RefChangeKind::Update, &rules::current_user_in).is_ok()
    });
    Ok(out([
        ("ruleList", Value::List(found.iter().map(rule_value).collect())),
        ("protected", Value::Bool(!found.is_empty())),
        (
            "requireMergeRequest",
            Value::Bool(found.iter().any(|rule| rule.require_merge_request)),
        ),
        (
            "requiredApprovals",
            Value::I64(i64::from(
                found.iter().map(|rule| rule.required_approvals).max().unwrap_or(0),
            )),
        ),
        (
            "dismissStaleApprovals",
            Value::Bool(found.iter().any(|rule| rule.dismiss_stale_approvals)),
        ),
        (
            "requiredStatusContexts",
            Value::List(contexts.iter().map(Value::string).collect()),
        ),
        (
            "allowedMergeMethods",
            Value::List(methods.unwrap_or_default().iter().map(Value::string).collect()),
        ),
        ("mayPushDirectly", Value::Bool(direct)),
    ]))
}

// ---- commit statuses ----

fn status_value(row: &EntityValue, facade: &EntityFacade) -> Value {
    let created_by = field(row, "createdByUserId");
    let creator = created_by.as_ref().and_then(|user| username_of(facade, user));
    map([
        ("commitStatusId", field_value(row, "commitStatusId")),
        ("id", field_value(row, "commitStatusId")),
        ("commitId", field_value(row, "commitId")),
        (
            "context",
            Value::string(field(row, "context").unwrap_or_else(|| "default".into())),
        ),
        ("state", field_value(row, "state")),
        ("targetUrl", field_value(row, "targetUrl")),
        ("target_url", field_value(row, "targetUrl")),
        ("description", field_value(row, "description")),
        ("createdByUserId", opt(created_by)),
        ("creator", opt(creator)),
        ("createdDate", field_value(row, "createdDate")),
        ("created_at", field_value(row, "createdDate")),
    ])
}

/// `create#GitCommitStatus`: a check of a commit by outside CI, in GitHub's shape (`state`, `target_url` or
/// `targetUrl`, `description`, `context`). Needs a push right on the repository; the commit must be in it. Out: the
/// status, with GitHub's names too (`id`, `target_url`, `created_at`).
fn create_commit_status(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return none();
    };
    if !["create", "update", "delete"].iter().any(|action| may(&id, action)) {
        if may(&id, "view") {
            add_error(format!("posting a commit status needs a push right on repository {id}"));
        } else {
            add_error(format!("repository {id} not found"));
        }
        return none();
    }
    let state = text(parameters, "state").unwrap_or_default().to_lowercase();
    if !STATES.contains(&state.as_str()) {
        add_error("state must be pending, success, failure or error");
        return none();
    }
    let Some(revision) = text(parameters, "commitId") else {
        add_error("commitId is required");
        return none();
    };
    let commit = match provider.resolve_commit(&id, &revision) {
        Ok(Some(commit)) => commit,
        Ok(None) => {
            add_error(format!("no commit {revision} in repository {id}"));
            return none();
        }
        Err(err) => {
            add_error(err.message());
            return none();
        }
    };
    let target_url = text(parameters, "targetUrl").or_else(|| text(parameters, "target_url"));
    if target_url
        .as_deref()
        .is_some_and(|url| !(url.starts_with("https://") || url.starts_with("http://")))
    {
        add_error("target_url must be an http or https URL");
        return none();
    }
    let _rows = rows_guard();
    let mut row = EntityValue::make(facade, GIT_COMMIT_STATUS)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("repositoryId", Value::string(&id));
    row.set("commitId", Value::string(commit.to_string()));
    row.set(
        "context",
        Value::string(text(parameters, "context").unwrap_or_else(|| "default".into())),
    );
    row.set("state", Value::string(&state));
    row.set("targetUrl", opt(target_url));
    row.set("description", opt(text(parameters, "description")));
    row.set("createdByUserId", opt(current_user()));
    row.set("createdDate", crate::user::now_value());
    row.create(facade)?;
    match status_value(&row, facade) {
        Value::Map(entries) => Ok(entries.into_iter().collect()),
        _ => none(),
    }
}

/// The latest status of each context of each commit of `commits` in a repository, by commit.
fn latest_statuses(
    facade: &EntityFacade,
    id: &str,
    commits: &[String],
) -> Result<BTreeMap<String, Vec<EntityValue>>, LibError> {
    let _rows = rows_guard();
    let rows = facade
        .find(GIT_COMMIT_STATUS)?
        .eq("repositoryId", Value::string(id))
        .condition(Condition::is_in(
            "commitId",
            commits.iter().map(Value::string).collect(),
        ))
        .order_by("createdDate", true)
        .order_by("commitStatusId", true)
        .list()?;
    let mut by_commit: BTreeMap<String, Vec<EntityValue>> = BTreeMap::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for row in rows.iter() {
        let commit = field(row, "commitId").unwrap_or_default();
        let context = field(row, "context").unwrap_or_else(|| "default".into());
        if seen.insert((commit.clone(), context)) {
            by_commit.entry(commit).or_default().push(row.clone());
        }
    }
    Ok(by_commit)
}

/// The combined state of statuses, as GitHub's: failure when any failed or erred, pending when any is pending,
/// success when all succeeded; empty for none.
fn combined(statuses: &[EntityValue]) -> Option<&'static str> {
    if statuses.is_empty() {
        return None;
    }
    let states: Vec<String> = statuses.iter().filter_map(|row| field(row, "state")).collect();
    Some(if states.iter().any(|state| state == "failure" || state == "error") {
        "failure"
    } else if states.iter().any(|state| state == "pending") {
        "pending"
    } else {
        "success"
    })
}

/// `get#GitCommitStatuses`: the statuses of a commit (or of each of `commitIdList`), the latest of each context, and
/// the combined state. With `allStatuses` every status, newest first. Out: `state`, `statusList`, and with
/// `commitIdList` `stateByCommit` (a Map of commit id to its combined state).
fn get_commit_statuses(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((provider, id)) = viewable(parameters) else {
        return none();
    };
    let mut commits = list_param(parameters, "commitIdList");
    if let Some(revision) = text(parameters, "commitId") {
        match provider.resolve_commit(&id, &revision) {
            Ok(Some(commit)) => commits.push(commit.to_string()),
            Ok(None) => {}
            Err(err) => {
                add_error(err.message());
                return none();
            }
        }
    }
    if commits.is_empty() {
        return Ok(out([("statusList", Value::List(Vec::new()))]));
    }
    if flag(parameters, "allStatuses") {
        let _rows = rows_guard();
        let rows = facade
            .find(GIT_COMMIT_STATUS)?
            .eq("repositoryId", Value::string(&id))
            .condition(Condition::is_in(
                "commitId",
                commits.iter().map(Value::string).collect(),
            ))
            .order_by("createdDate", true)
            .list()?;
        return Ok(out([(
            "statusList",
            Value::List(rows.iter().map(|row| status_value(row, facade)).collect()),
        )]));
    }
    let latest = latest_statuses(facade, &id, &commits)?;
    let all: Vec<EntityValue> = latest.values().flatten().cloned().collect();
    let by_commit: BTreeMap<String, Value> = latest
        .iter()
        .map(|(commit, rows)| (commit.clone(), opt(combined(rows).map(str::to_string))))
        .collect();
    Ok(out([
        ("state", opt(combined(&all).map(str::to_string))),
        (
            "statusList",
            Value::List(all.iter().map(|row| status_value(row, facade)).collect()),
        ),
        ("stateByCommit", Value::Map(by_commit.into_iter().collect())),
    ]))
}

// ---- webhooks ----

/// The active webhooks of a repository that take `event`.
fn webhooks_for(facade: &EntityFacade, id: &str, event: &str) -> Result<Vec<EntityValue>, LibError> {
    let _rows = rows_guard();
    Ok(facade
        .find(GIT_WEBHOOK)?
        .eq("repositoryId", Value::string(id))
        .eq("active", Value::string("Y"))
        .list()?
        .iter()
        .filter(|row| {
            let events = field(row, "events").unwrap_or_default();
            events.trim().is_empty() || events.split(',').any(|item| item.trim() == event)
        })
        .cloned()
        .collect())
}

fn repository_json(id: &str, branch: &str) -> serde_json::Value {
    let base = crate::resource::git::server::public_base_url();
    serde_json::json!({
        "id": id,
        "name": id,
        "full_name": id,
        "clone_url": format!("{base}/git/{id}.git"),
        "default_branch": branch,
    })
}

fn sender_json(facade: &EntityFacade, user_id: Option<&str>) -> serde_json::Value {
    match user_id {
        Some(user_id) => serde_json::json!({
            "user_id": user_id,
            "login": username_of(facade, user_id).unwrap_or_else(|| user_id.to_string()),
        }),
        None => serde_json::Value::Null,
    }
}

/// Queue a delivery of `payload` to a webhook: a SystemMessage of type GitWebhook, sent soon after (and retried by
/// `retry#GitWebhookDeliveries`). Its id.
fn queue_delivery(webhook: &EntityValue, event: &str, payload: &serde_json::Value, send_now: bool) -> Option<String> {
    let webhook_id = field(webhook, "webhookId").unwrap_or_default();
    let mut parameters = BTreeMap::new();
    parameters.insert("systemMessageTypeId".to_string(), Value::string("GitWebhook"));
    parameters.insert("messageText".to_string(), Value::string(payload.to_string()));
    parameters.insert("receiverId".to_string(), Value::string(&webhook_id));
    parameters.insert("docType".to_string(), Value::string(event));
    parameters.insert("docSubType".to_string(), opt(field(webhook, "repositoryId")));
    parameters.insert("sendNow".to_string(), Value::Bool(send_now));
    match crate::service::sync("org.moqui.impl.SystemMessageServices.queue#SystemMessage", &parameters) {
        Ok(result) => result.get("systemMessageId").map(to_plain_string),
        Err(err) => {
            tracing::error!(webhook = %webhook_id, event, "webhook delivery not queued: {err}");
            None
        }
    }
}

/// `queue#GitWebhookPush`: for the framework's post-receive rule: a push event per branch that changed and a tag
/// event per tag, to each active webhook that takes it. A webhook that cannot be queued is logged and the others
/// still are.
fn queue_webhook_push(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return none();
    };
    let push_hooks = webhooks_for(facade, &id, "push")?;
    let tag_hooks = webhooks_for(facade, &id, "tag")?;
    if push_hooks.is_empty() && tag_hooks.is_empty() {
        return none();
    }
    let refs: Vec<BTreeMap<String, Value>> = match parameters.get("refUpdates") {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(entries) => Some(entries.clone().into_iter().collect()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let user_id = text(parameters, "userId");
    let source = text(parameters, "source").unwrap_or_else(|| "push".into());
    let branch = provider
        .refs(&id, "refs/heads/")
        .map(|refs| refs.branch)
        .unwrap_or_else(|_| "main".into());
    let mut queued = 0;
    for reference in refs {
        let name = text(&reference, "refName").unwrap_or_default();
        let (event, hooks) = if name.starts_with("refs/tags/") {
            ("tag", &tag_hooks)
        } else if name.starts_with("refs/heads/") {
            ("push", &push_hooks)
        } else {
            continue;
        };
        if hooks.is_empty() {
            continue;
        }
        let old = text(&reference, "oldId");
        let new = text(&reference, "newId");
        let change = text(&reference, "changeType").unwrap_or_default();
        let mut commits = Vec::new();
        let mut total = 0;
        if let Some(new) = new.as_deref().filter(|_| event == "push") {
            let range = provider.compare(&id, old.as_deref().unwrap_or(""), new, false, false);
            if let (Some(_), Ok(Some(compared))) = (&old, &range) {
                total = compared.ahead;
                commits = compared.commits.iter().rev().take(20).rev().cloned().collect();
            } else if old.is_none() {
                if let Ok(page) = provider.log(&id, new, &[], None, 20) {
                    total = page.commits.len();
                    commits = page.commits.into_iter().rev().collect();
                }
            }
        }
        let commit_json = |commit: &crate::resource::git::ops::CommitDetails| {
            serde_json::json!({
                "id": commit.id.to_string(),
                "message": commit.message,
                "timestamp": jiff::Timestamp::from_second(commit.author.seconds).map(|t| t.to_string()).unwrap_or_default(),
                "author": {"name": commit.author.name, "email": commit.author.email},
                "committer": {"name": commit.committer.name, "email": commit.committer.email},
            })
        };
        let payload = serde_json::json!({
            "event": event,
            "ref": name,
            "before": old.clone().unwrap_or_else(|| "0".repeat(40)),
            "after": new.clone().unwrap_or_else(|| "0".repeat(40)),
            "created": change == "create",
            "deleted": change == "delete",
            "forced": change == "force",
            "source": source,
            "repository": repository_json(&id, &branch),
            "pusher": sender_json(facade, user_id.as_deref()),
            "sender": sender_json(facade, user_id.as_deref()),
            "commits": commits.iter().map(commit_json).collect::<Vec<_>>(),
            "head_commit": commits.last().map(commit_json),
            "total_commits": total,
        });
        for hook in hooks {
            if queue_delivery(hook, event, &payload, true).is_some() {
                queued += 1;
            }
        }
    }
    Ok(out([("queuedCount", count(queued))]))
}

/// `queue#GitWebhookEvent`: an event of the repository (`merge_request`, with an `action`) to each active webhook
/// that takes it, with `payload` (a Map) as its body beside the repository and the sender.
fn queue_webhook_event(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return none();
    };
    let Some(event) = text(parameters, "event") else {
        add_error("event is required");
        return none();
    };
    let hooks = webhooks_for(facade, &id, &event)?;
    if hooks.is_empty() {
        return Ok(out([("queuedCount", count(0))]));
    }
    let branch = provider
        .refs(&id, "refs/heads/")
        .map(|refs| refs.branch)
        .unwrap_or_else(|_| "main".into());
    let mut payload = match parameters.get("payload") {
        Some(value @ Value::Map(_)) => moqui_lib::json::to_json(value),
        _ => serde_json::json!({}),
    };
    if let serde_json::Value::Object(object) = &mut payload {
        object.insert("event".into(), serde_json::Value::String(event.clone()));
        if let Some(action) = text(parameters, "action") {
            object.insert("action".into(), serde_json::Value::String(action));
        }
        object.insert("repository".into(), repository_json(&id, &branch));
        object.insert("sender".into(), sender_json(facade, current_user().as_deref()));
    }
    let queued = hooks
        .iter()
        .filter(|hook| queue_delivery(hook, &event, &payload, true).is_some())
        .count();
    Ok(out([("queuedCount", count(queued))]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The `sha256=<hex>` signature of a body with a webhook's secret, as GitHub's `X-Hub-Signature-256`.
pub(crate) fn signature(secret: &str, body: &[u8]) -> String {
    use hmac::Mac;
    let mut mac = <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(b"").expect("an empty key"));
    mac.update(body);
    format!("sha256={}", hex(&mac.finalize().into_bytes()))
}

/// POST a webhook's body. The status, or why it was not sent.
fn deliver(url: &str, event: &str, delivery: &str, secret: Option<&str>, body: &str) -> Result<u16, String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!(
            "{} is not an http or https URL",
            moqui_lib::http::redacted(url)
        ));
    }
    let private = crate::resource::git::setting("webhook-private-network") == "true";
    let sim = ExecutionContext::with_current(|ec| ec.factory().entity().is_some_and(|entity| entity.sim_active()))
        .unwrap_or(false);
    let client = moqui_lib::http::RestClient::new()
        .deny_internal_addresses(!private)
        .timeout(WEBHOOK_TIMEOUT)
        .max_response_bytes(64 * 1024)
        .truncate_body(true)
        .redirects(0, false);
    let mut call = client
        .post(url)
        .header("user-agent", "Moqui-Webhook/1.0")
        .header("x-moqui-event", event)
        .header("x-moqui-delivery", delivery);
    if let Some(secret) = secret.filter(|secret| !secret.is_empty()) {
        let signed = signature(secret, body.as_bytes());
        call = call
            .header("x-hub-signature-256", signed.clone())
            .header("x-moqui-signature-256", signed);
    }
    call.text("application/json", body)
        .send(sim)
        .map(|response| response.status())
        .map_err(|err| err.to_string())
}

/// The outcome of an attempt on the webhook's row, in a transaction of its own: a failed delivery rolls its send back.
fn record_attempt(facade: &EntityFacade, webhook_id: &str, status: &str) {
    let _rows = rows_guard();
    let outcome = (|| -> Result<(), LibError> {
        facade
            .transactions_for(GIT_WEBHOOK)?
            .run(crate::transaction::TransactionAttribute::ForceNew, None, || {
                facade
                    .find(GIT_WEBHOOK)?
                    .eq("webhookId", Value::string(webhook_id))
                    .update_all([
                        ("lastDeliveryDate".to_string(), crate::user::now_value()),
                        ("lastDeliveryStatus".to_string(), Value::string(status)),
                    ])?;
                Ok(())
            })
    })();
    if let Err(err) = outcome {
        tracing::error!(webhook = webhook_id, "webhook delivery not recorded: {err}");
    }
}

/// `send#GitWebhookDelivery`: the send service of SystemMessage type GitWebhook: POST the message's JSON to its
/// webhook's URL, signed with the webhook's secret. A status that is not 2xx, or no answer, is an error, so the
/// message stays to be retried. Out: `remoteMessageId` (the HTTP status).
fn send_webhook_delivery(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(message_id) = text(parameters, "systemMessageId") else {
        add_error("systemMessageId is required");
        return none();
    };
    let _rows = rows_guard();
    let Some(message) = facade
        .find(SYSTEM_MESSAGE)?
        .eq("systemMessageId", Value::string(&message_id))
        .one()?
    else {
        add_error(format!("no system message {message_id}"));
        return none();
    };
    let webhook_id = field(&message, "receiverId").unwrap_or_default();
    let Some(webhook) = facade
        .find(GIT_WEBHOOK)?
        .eq("webhookId", Value::string(&webhook_id))
        .one()?
    else {
        add_error(format!("webhook {webhook_id} of message {message_id} is gone"));
        return none();
    };
    let event = field(&message, "docType").unwrap_or_else(|| "push".into());
    let body = field(&message, "messageText").unwrap_or_else(|| "{}".into());
    let url = field(&webhook, "url").unwrap_or_default();
    drop(_rows);
    match deliver(&url, &event, &message_id, field(&webhook, "secret").as_deref(), &body) {
        Ok(status) => {
            record_attempt(facade, &webhook_id, &status.to_string());
            if (200..300).contains(&status) {
                Ok(out([("remoteMessageId", Value::string(status.to_string()))]))
            } else {
                add_error(format!(
                    "webhook {webhook_id} at {} answered HTTP {status}",
                    moqui_lib::http::redacted(&url)
                ));
                none()
            }
        }
        Err(reason) => {
            record_attempt(facade, &webhook_id, "error");
            add_error(format!("webhook {webhook_id} not delivered: {reason}"));
            none()
        }
    }
}

/// `test#GitWebhook`: send a `ping` event to a webhook now, recorded as its other deliveries are. Needs delete on its
/// repository. Out: `systemMessageId`, `statusCode`, `delivered`, `errorText`.
fn test_webhook(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(webhook_id) = text(parameters, "webhookId") else {
        add_error("webhookId is required");
        return none();
    };
    let webhook = {
        let _rows = rows_guard();
        facade
            .find(GIT_WEBHOOK)?
            .eq("webhookId", Value::string(&webhook_id))
            .one()?
    };
    let Some(webhook) = webhook else {
        add_error(format!("no webhook {webhook_id}"));
        return none();
    };
    let repository_id = field(&webhook, "repositoryId").unwrap_or_default();
    let mut scoped = parameters.clone();
    scoped.insert("repositoryId".into(), Value::string(&repository_id));
    let Some((provider, id)) = allowed(&scoped, "delete") else {
        return none();
    };
    let branch = provider
        .refs(&id, "refs/heads/")
        .map(|refs| refs.branch)
        .unwrap_or_else(|_| "main".into());
    let payload = serde_json::json!({
        "event": "ping",
        "zen": "Moqui says hello",
        "hook_id": webhook_id,
        "repository": repository_json(&id, &branch),
        "sender": sender_json(facade, current_user().as_deref()),
    });
    let Some(message_id) = queue_delivery(&webhook, "ping", &payload, false) else {
        add_error("the test delivery could not be queued");
        return none();
    };
    let sent = crate::service::sync(
        "org.moqui.impl.SystemMessageServices.send#ProducedSystemMessage",
        &[("systemMessageId".to_string(), Value::string(&message_id))]
            .into_iter()
            .collect(),
    );
    let errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors_text().join("; ");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default();
    let status = {
        let _rows = rows_guard();
        facade
            .find(GIT_WEBHOOK)?
            .eq("webhookId", Value::string(&webhook_id))
            .one()?
            .and_then(|row| field(&row, "lastDeliveryStatus"))
    };
    let delivered = sent.is_ok() && errors.is_empty();
    let error_text = match sent {
        Err(err) => Some(err.to_string()),
        Ok(_) if !errors.is_empty() => Some(errors),
        Ok(_) => None,
    };
    if let Some(error) = &error_text {
        add_message(format!("Test delivery failed: {error}"));
    }
    Ok(out([
        ("systemMessageId", Value::string(message_id)),
        ("statusCode", opt(status)),
        ("delivered", Value::Bool(delivered)),
        ("errorText", opt(error_text)),
    ]))
}

// ---- merging a merge request ----

/// The users whose approval of merge request `mr_id` counts toward the branch rules' required approvals: approved
/// (of `head` when the rules dismiss stale approvals) by a user who may push to repository `id` (update), decided as
/// that user now. A viewer's review is a comment, not an approval, and an approval stops counting when its reviewer
/// loses the push right.
fn counted_approvals(
    facade: &EntityFacade,
    id: &str,
    mr_id: &str,
    head: Option<&str>,
) -> Result<Vec<String>, LibError> {
    let approved: Vec<String> = {
        let _rows = rows_guard();
        facade
            .find(GIT_MERGE_REQUEST_REVIEWER)?
            .eq("mergeRequestId", Value::string(mr_id))
            .eq("statusId", Value::string("GmrrApproved"))
            .order_by("userId", false)
            .list()?
            .iter()
            .filter(|reviewer| head.is_none() || field(reviewer, "reviewedCommitId").as_deref() == head)
            .filter_map(|reviewer| field(reviewer, "userId"))
            .collect()
    };
    Ok(approved
        .into_iter()
        .filter(|user| crate::user::as_user(user, || may(id, "update")).unwrap_or(false))
        .collect())
}

/// `get#MergeRequestApprovals`: for a viewer of the merge request's repository, the approvals that count toward the
/// target's branch rules ([`counted_approvals`]; of the current head when the rules dismiss stale approvals). Out:
/// `approvalCount`, `approverUserIdList`.
fn get_merge_request_approvals(parameters: &Parameters, facade: &EntityFacade) -> Out {
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
        Value::string(field(&row, "repositoryId").unwrap_or_default()),
    );
    let Some((_, id)) = allowed(&scoped, "view") else {
        return none();
    };
    let target = field(&row, "targetRef").unwrap_or_default();
    let dismiss = rules::rules_for(facade, &id, &format!("refs/heads/{target}"))?
        .iter()
        .any(|rule| rule.dismiss_stale_approvals);
    let head = field(&row, "headCommitId").unwrap_or_default();
    let users = counted_approvals(facade, &id, &mr_id, dismiss.then_some(head.as_str()))?;
    Ok(out([
        ("approvalCount", Value::I64(users.len() as i64)),
        (
            "approverUserIdList",
            Value::List(users.iter().map(Value::string).collect()),
        ),
    ]))
}

/// `GitReviewServices.merge#MergeRequest`: merge an open merge request by its method (the one given, else its own,
/// else the repository's default) once the target's branch rules are met: the approvals (of the head, when stale
/// approvals are dismissed; only those of users who may push, [`counted_approvals`]), the required checks successful on the head, a method they allow. Needs update on the
/// repository. The merge request becomes merged with the target's new head; the source branch is deleted when asked
/// (a refusal there is a message, the merge stands); webhooks get a merge_request event.
fn merge_merge_request(parameters: &Parameters, facade: &EntityFacade) -> Out {
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
    let repository_id = field(&row, "repositoryId").unwrap_or_default();
    let mut scoped = parameters.clone();
    scoped.insert("repositoryId".into(), Value::string(&repository_id));
    let Some((provider, id)) = allowed(&scoped, "update") else {
        return none();
    };
    match field(&row, "statusId").as_deref() {
        Some("GmrOpen") => {}
        Some("GmrDraft") => {
            add_error("a draft merge request does not merge: mark it ready first");
            return none();
        }
        _ => {
            add_error(format!("merge request {mr_id} is not open"));
            return none();
        }
    }
    let source = field(&row, "sourceRef").unwrap_or_default();
    let target = field(&row, "targetRef").unwrap_or_default();
    let repository = repository_row(facade, &id)?;
    let method_id = text(parameters, "mergeMethodEnumId")
        .or_else(|| field(&row, "mergeMethodEnumId"))
        .or_else(|| {
            repository
                .as_ref()
                .and_then(|row| field(row, "defaultMergeMethodEnumId"))
        })
        .unwrap_or_else(|| "GmmMerge".into());
    let Some(method) = Method::parse(&method_id) else {
        add_error(format!("{method_id} is not a merge method"));
        return none();
    };
    let head = match provider.ref_of(&id, &format!("refs/heads/{source}")) {
        Ok(Some((_, head))) => head,
        Ok(None) => {
            add_error(format!("the source branch {source} is gone"));
            return none();
        }
        Err(err) => {
            add_error(err.message());
            return none();
        }
    };
    let found = rules::rules_for(facade, &id, &format!("refs/heads/{target}"))?;
    let dismiss = found.iter().any(|rule| rule.dismiss_stale_approvals);
    let approvals = counted_approvals(facade, &id, &mr_id, dismiss.then(|| head.to_string()).as_deref())?.len();
    let successful: Vec<String> = latest_statuses(facade, &id, &[head.to_string()])?
        .values()
        .flatten()
        .filter(|status| field(status, "state").as_deref() == Some("success"))
        .filter_map(|status| field(status, "context"))
        .collect();
    if let Err(reason) = rules::check_merge(
        &found,
        method.enum_id(),
        u32::try_from(approvals).unwrap_or(u32::MAX),
        &successful,
    ) {
        add_error(format!(
            "merge request !{} cannot merge yet: {reason}",
            field(&row, "mergeRequestNum").unwrap_or_default()
        ));
        return none();
    }
    let author = field(&row, "authorUserId").map(|user| provider.author_for_user(&user));
    let message = text(parameters, "message");
    let outcome = match provider.merge_refs(
        &id,
        &format!("refs/heads/{source}"),
        &target,
        method,
        message.as_deref(),
        author,
        false,
        true,
    ) {
        Ok(outcome) => outcome,
        Err(err) => {
            add_error(err.message());
            return none();
        }
    };
    let merged = match outcome {
        MergeOutcome::Merged(merged) => merged,
        other => return Ok(merge_out(other)),
    };
    let user = current_user();
    {
        let _rows = rows_guard();
        facade
            .find(GIT_MERGE_REQUEST)?
            .eq("mergeRequestId", Value::string(&mr_id))
            .update_all([
                ("statusId".to_string(), Value::string("GmrMerged")),
                ("mergedDate".to_string(), crate::user::now_value()),
                ("lastUpdatedDate".to_string(), crate::user::now_value()),
                ("mergedByUserId".to_string(), opt(user.clone())),
                ("mergeCommitId".to_string(), Value::string(merged.head.to_string())),
                ("mergeMethodEnumId".to_string(), Value::string(method.enum_id())),
                ("headCommitId".to_string(), Value::string(head.to_string())),
                ("mergeableEnumId".to_string(), Value::string("GmaMerged")),
                ("conflictPaths".to_string(), Value::Null),
            ])?;
    }
    let delete_source = tri(parameters, "deleteSourceBranch")
        .unwrap_or_else(|| field(&row, "deleteSourceBranch").as_deref() == Some("Y"));
    if delete_source {
        let deleted = provider.write_refs(
            &id,
            &[RefWrite {
                name: format!("refs/heads/{source}"),
                expected: Some(head),
                new: None,
            }],
            false,
            None,
        );
        match deleted.map(|results| results.into_iter().next()) {
            Ok(Some(Ok(()))) => {}
            Ok(Some(Err(reason))) => add_message(format!("The source branch {source} was kept: {reason}")),
            Ok(None) => {}
            Err(err) => add_message(format!("The source branch {source} was kept: {}", err.message())),
        }
    }
    let mut event = BTreeMap::new();
    event.insert("repositoryId".to_string(), Value::string(&id));
    event.insert("event".to_string(), Value::string("merge_request"));
    event.insert("action".to_string(), Value::string("merged"));
    event.insert(
        "payload".to_string(),
        map([(
            "merge_request",
            map([
                ("id", Value::string(&mr_id)),
                ("number", field_value(&row, "mergeRequestNum")),
                ("title", field_value(&row, "title")),
                ("source_branch", Value::string(&source)),
                ("target_branch", Value::string(&target)),
                ("state", Value::string("merged")),
                ("merge_commit_sha", Value::string(merged.head.to_string())),
                ("head_sha", Value::string(head.to_string())),
            ]),
        )]),
    );
    if let Err(err) = queue_webhook_event(&event, facade) {
        tracing::error!(merge_request = %mr_id, "merge request webhooks not queued: {err}");
    }
    Ok(out([
        ("commitId", Value::string(merged.head.to_string())),
        (
            "madeCommitIds",
            Value::List(merged.made.iter().map(|id| Value::string(id.to_string())).collect()),
        ),
        ("mergeMethodEnumId", Value::string(method.enum_id())),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signature_is_githubs() {
        // the example of GitHub's "Validating webhook deliveries"
        assert_eq!(
            signature("It's a Secret to Everybody", b"Hello, World!"),
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
    }

    #[test]
    fn a_webhook_to_an_internal_address_is_refused_by_default() {
        let refused = deliver("http://127.0.0.1:9/hook", "push", "1", None, "{}").unwrap_err();
        assert!(refused.contains("internal address"), "{refused}");
        let refused = deliver("ftp://example.com/hook", "push", "1", None, "{}").unwrap_err();
        assert!(refused.contains("not an http or https URL"), "{refused}");
    }
}
