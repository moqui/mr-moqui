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

//! `org.moqui.git.GitServices.commit#GitFiles`: a commit of files made by a service, for a page that edits a file
//! (plans/Dogfood.md N-COMMIT, D14). Guide: `docs/interfaces/Git-Hosting.md`, Commits through Moqui.

use std::collections::BTreeMap;

use gix::ObjectId;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};

use super::{add_error, add_refusal, flag, out, provider_and_id, text};
use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::rust_service;

use crate::file_commit::{FileCommit, FileCommitOutcome};
use crate::values::map;
use crate::GIT_ARTIFACT_TYPE;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, commit_files)
}

fn segments(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

fn id_value(id: Option<ObjectId>) -> Value {
    id.map_or(Value::Null, |id| Value::string(id.to_string()))
}

/// `commit#GitFiles`: one commit of files with a message, as the current user: to the default branch or `branch`
/// (needs update), to `newBranch` made from it (needs create), or with `asProposal` as the head of a merge request
/// into it with no branch (view is enough, as for a push to `refs/for/<branch>`). `fileMap` is path to new text or
/// bytes, `removePaths` the paths deleted, `renameMap` old path to new, `expectedBlobIds` path to the blob the edit
/// began from (empty: no file there). Out: `commitId` (empty when nothing changed), `refName`; for a proposal
/// `mergeRequestId`, `mergeRequestNum`, `mergeRequestCreated`; and when a path's blob is another than expected no
/// error but `conflict` true, `errorCode` `GitFileChanged` and `conflictList` (each `path`, `expectedBlobId`,
/// `currentBlobId`), so a page shows both texts.
fn commit_files(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let none = || Ok(BTreeMap::new());
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return none();
    };
    let proposal = flag(parameters, "asProposal");
    let new_branch = text(parameters, "newBranch");
    if proposal && new_branch.is_some() {
        add_error("asProposal and newBranch are two ways to save: give one");
        return none();
    }
    let authorized = |action: &str| moqui_core::user::authorize_own(GIT_ARTIFACT_TYPE, &id, action);
    let may_push = ["create", "update", "delete"]
        .iter()
        .any(|action| crate::server::permitted(&id, action));
    let right = if proposal {
        // a user who reads the repository offers a change to it, as by a push to refs/for/<branch>
        if moqui_core::user::current_user_id()
            .ok()
            .flatten()
            .is_none_or(|user| user.is_empty())
        {
            add_error("sign in to propose a change");
            return none();
        }
        if !crate::server::review_push_enabled() && !may_push {
            add_error("changes for review are off on this server");
            return none();
        }
        authorized("view")
    } else if new_branch.is_some() {
        authorized("create")
    } else {
        authorized("update")
    };
    if let Err(err) = right {
        add_error(err.message());
        return none();
    }
    let Some(message) = text(parameters, "message") else {
        add_error("message is required");
        return none();
    };
    let mut puts = Vec::new();
    if let Some(Value::Map(files)) = parameters.get("fileMap") {
        for (path, content) in files {
            let bytes = match content {
                Value::Bytes(bytes) => bytes.clone(),
                other => to_plain_string(other).into_bytes(),
            };
            puts.push((segments(path), bytes));
        }
    }
    if let Some(Value::Map(files)) = parameters.get("fileLocationMap") {
        // a location is read as the server: only the application names one (a staged, scanned upload)
        if !files.is_empty() && !ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false) {
            add_error("fileLocationMap is given only with authorization disabled");
            return none();
        }
        for (path, location) in files {
            let location = to_plain_string(location);
            let read = ExecutionContext::with_current(|ec| {
                ec.factory()
                    .resource()
                    .get(&location)
                    .and_then(|reference| reference.read_bytes())
            });
            match read {
                Some(Ok(bytes)) => puts.push((segments(path), bytes)),
                Some(Err(err)) => {
                    add_error(format!("{location} was not read: {}", err.message()));
                    return none();
                }
                None => return none(),
            }
        }
    }
    let removes: Vec<Vec<String>> = match parameters.get("removePaths") {
        Some(Value::List(paths)) => paths.iter().map(|path| segments(&to_plain_string(path))).collect(),
        _ => Vec::new(),
    };
    let renames: Vec<(Vec<String>, Vec<String>)> = match parameters.get("renameMap") {
        Some(Value::Map(paths)) => paths
            .iter()
            .map(|(from, to)| (segments(from), segments(&to_plain_string(to))))
            .collect(),
        _ => Vec::new(),
    };
    let mut expected = Vec::new();
    if let Some(Value::Map(blobs)) = parameters.get("expectedBlobIds") {
        for (path, blob) in blobs {
            let blob = to_plain_string(blob);
            let blob = match blob.trim() {
                "" => None,
                hex => match ObjectId::from_hex(hex.as_bytes()) {
                    Ok(id) => Some(id),
                    Err(_) => {
                        add_error(format!("{hex} is not a blob id (expectedBlobIds of {path})"));
                        return none();
                    }
                },
            };
            expected.push((segments(path), blob));
        }
    }
    let author = match (text(parameters, "authorName"), text(parameters, "authorEmail")) {
        (Some(name), Some(email)) => Some((name, email)),
        (None, None) => None,
        _ => {
            add_error("authorName and authorEmail are given together");
            return none();
        }
    };
    let internal = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
    // who a commit is by is the caller's own account, unless the application says (an account's display name and
    // confirmed address): a user does not write commits in another's name through a page
    if author.is_some() && !internal {
        add_error("authorName and authorEmail are given only with authorization disabled");
        return none();
    }
    if let Some((name, email)) = &author {
        // a signature's fields end at these: git's own rule
        if [name, email].iter().any(|part| part.contains(['<', '>', '\n'])) {
            add_error("authorName and authorEmail have no <, > or line break");
            return none();
        }
    }
    // a user with no push right stores objects only within what a push for review may bring
    let viewer = proposal && !may_push;
    if viewer {
        // refused too often this hour: nothing more is built for it
        if let Some(reason) = provider.review_refusals_over() {
            add_error(reason);
            return none();
        }
        let bytes: u64 = puts.iter().map(|(_, bytes)| bytes.len() as u64).sum();
        if let Some(reason) = provider.review_cap(bytes) {
            provider.note_review_refusal();
            add_error(reason);
            return none();
        }
    }
    let branch = text(parameters, "branch");
    let request = FileCommit {
        branch: branch.as_deref(),
        new_branch: new_branch.as_deref(),
        proposal,
        message: &message,
        puts: &puts,
        removes: &removes,
        renames: &renames,
        expected: &expected,
        author: author.as_ref().map(|(name, email)| (name.as_str(), email.as_str())),
        viewer,
        // an application's own write past the rules of a branch (release notes on a protected default branch)
        bypass_rules: internal && flag(parameters, "bypassBranchRules"),
    };
    let outcome = match provider.commit_change(&id, &request) {
        Ok(outcome) => outcome,
        Err(err) => {
            if viewer {
                provider.note_review_refusal();
            }
            add_refusal(err.message());
            return none();
        }
    };
    if viewer && matches!(outcome, FileCommitOutcome::Refused(_)) {
        provider.note_review_refusal();
    }
    match outcome {
        FileCommitOutcome::Unchanged => Ok(out([("commitId", Value::Null), ("conflict", Value::Bool(false))])),
        FileCommitOutcome::Committed { commit, ref_name } => Ok(out([
            ("commitId", Value::string(commit.to_string())),
            ("refName", Value::string(ref_name)),
            ("conflict", Value::Bool(false)),
        ])),
        FileCommitOutcome::Refused(reason) => {
            add_refusal(reason);
            none()
        }
        FileCommitOutcome::Conflict(conflicts) => Ok(out([
            ("commitId", Value::Null),
            ("conflict", Value::Bool(true)),
            ("errorCode", Value::string("GitFileChanged")),
            (
                "conflictList",
                Value::List(
                    conflicts
                        .iter()
                        .map(|conflict| {
                            map([
                                ("path", Value::string(&conflict.path)),
                                ("expectedBlobId", id_value(conflict.expected)),
                                ("currentBlobId", id_value(conflict.current)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])),
        FileCommitOutcome::Proposed { commit, target } => {
            let (summary, body) = match message.trim().split_once('\n') {
                Some((summary, body)) => (summary.trim().to_string(), body.trim().to_string()),
                None => (message.trim().to_string(), String::new()),
            };
            let mut proposed = BTreeMap::new();
            proposed.insert("repositoryId".to_string(), Value::string(&id));
            proposed.insert("targetRef".to_string(), Value::string(&target));
            proposed.insert("headCommitId".to_string(), Value::string(commit.to_string()));
            proposed.insert("defaultTitle".to_string(), Value::string(summary));
            if !body.is_empty() {
                proposed.insert("defaultDescription".to_string(), Value::string(body));
            }
            for (parameter, name) in [
                ("mergeRequestId", "mergeRequestId"),
                ("topic", "topic"),
                ("title", "title"),
                ("description", "description"),
            ] {
                if let Some(value) = text(parameters, name) {
                    proposed.insert(parameter.to_string(), Value::string(value));
                }
            }
            if flag(parameters, "draft") {
                proposed.insert("draft".to_string(), Value::Bool(true));
            }
            if crate::server::review_starts_hidden() {
                proposed.insert("hidden".to_string(), Value::Bool(true));
            }
            let call = moqui_core::service::call::CallOptions {
                disable_authz: true,
                ..Default::default()
            };
            let made = moqui_core::service::call::sync_checked(
                "org.moqui.git.GitReviewServices.propose#MergeRequest",
                &proposed,
                &call,
            );
            match made {
                // its errors are in the message facade already
                Err(_) => none(),
                Ok(made) => Ok(out([
                    ("commitId", Value::string(commit.to_string())),
                    (
                        "refName",
                        made.get("mergeRequestNum")
                            .map(to_plain_string)
                            .and_then(|num| num.parse::<i64>().ok())
                            .map_or(Value::Null, |num| Value::string(crate::reach::merge_request_ref(num))),
                    ),
                    ("conflict", Value::Bool(false)),
                    (
                        "mergeRequestId",
                        made.get("mergeRequestId").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "mergeRequestNum",
                        made.get("mergeRequestNum").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "mergeRequestCreated",
                        made.get("created").cloned().unwrap_or(Value::Bool(false)),
                    ),
                ])),
            }
        }
    }
}

#[cfg(test)]
#[path = "commit_tests.rs"]
pub(crate) mod tests;
