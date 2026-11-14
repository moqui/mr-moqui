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

//! Review in the Workspace (plans/design-dev-aide.md 21.4, P12): a merge request's changes in the merge view with the
//! server's review threads on their lines, a reply, a new comment on a line, resolve, and a review (approve, comment,
//! request changes); and the open threads of the merge request whose branch is checked out, for the editor's gutter.
//! Threads are an application's (Mantle's): a repository an application keeps (Smithy) is read and written through
//! that application's REST (`changes` with its `threadList`, `comments`, `threads/{id}/resolve`, `reviews`), any
//! other through the framework's (`mergeRequests/{n}/threads`, which the server's `review-thread-service` answers).
//! Each write is EXTERNAL: made on the server as the user, from a session, never by a header login (21.9).

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::client::{self, body, query_text, Server};
use super::review::{need_hosted, Call, Hosted, Via};
use crate::tier::Tier;
use crate::vcs::{audit_git, git_root};
use crate::workspace::{json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_review_threads), get_review_threads)?;
    registry.service_function(
        moqui_core::service_location!(get_merge_request_changes),
        get_merge_request_changes,
    )?;
    registry.service_function(
        moqui_core::service_location!(get_merge_request_file),
        get_merge_request_file,
    )?;
    registry.service_function(
        moqui_core::service_location!(create_review_comment),
        create_review_comment,
    )?;
    registry.service_function(
        moqui_core::service_location!(resolve_review_thread),
        resolve_review_thread,
    )?;
    registry.service_function(moqui_core::service_location!(submit_review), submit_review)?;
    registry.service_function(moqui_core::service_location!(get_file_threads), get_file_threads)
}

/// The path of a merge request: the framework's (`{repositories}/{id}/mergeRequests/{n}`) or an application's
/// (`{restBase}/repositories/{id}/merge-requests/{n}`).
fn merge_request_path(via: &Via, repositories: &str, repository_id: &str, number: &str) -> String {
    let (id, number) = (query_text(repository_id), query_text(number));
    match via {
        Via::Framework => format!("{repositories}/{id}/mergeRequests/{number}"),
        Via::Application(application) => {
            format!("{}/repositories/{id}/merge-requests/{number}", application.rest_base)
        }
    }
}

/// The threads of a merge request (of `path`): the framework's `threads`, or the application's `changes`, whose
/// answer has its `threadList` (no diffs asked).
#[must_use]
pub(crate) fn threads_call(via: &Via, repositories: &str, repository_id: &str, number: &str, path: &str) -> Call {
    let base = merge_request_path(via, repositories, repository_id, number);
    let mut query = Vec::new();
    if !path.is_empty() {
        query.push(("path", path.to_string()));
    }
    match via {
        Via::Framework => Call {
            post: false,
            path: format!("{base}/threads"),
            query,
            body: None,
        },
        Via::Application(_) => {
            query.push(("withDiffs", "false".to_string()));
            Call {
                post: false,
                path: format!("{base}/changes"),
                query,
                body: None,
            }
        }
    }
}

/// Where a comment is: a reply to a thread, or a new one on a line of a file (the side's line number), or on the merge
/// request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Comment {
    pub body: String,
    pub thread_id: String,
    pub path: String,
    pub commit_id: String,
    pub old_line: Option<i64>,
    pub new_line: Option<i64>,
    pub line_text: String,
}

#[must_use]
pub(crate) fn comment_call(
    via: &Via,
    repositories: &str,
    repository_id: &str,
    number: &str,
    comment: &Comment,
) -> Call {
    let base = merge_request_path(via, repositories, repository_id, number);
    let line = |value: Option<i64>| value.map_or(Json::Null, |line| json!(line));
    match via {
        Via::Framework => Call {
            post: true,
            path: format!("{base}/threads"),
            query: Vec::new(),
            body: Some(body(&[
                ("body", json!(comment.body)),
                ("threadId", json!(comment.thread_id)),
                ("path", json!(comment.path)),
                ("commitId", json!(comment.commit_id)),
                ("oldLine", line(comment.old_line)),
                ("newLine", line(comment.new_line)),
                ("lineText", json!(comment.line_text)),
            ])),
        },
        Via::Application(_) => Call {
            post: true,
            path: format!("{base}/comments"),
            query: Vec::new(),
            body: Some(body(&[
                ("body", json!(comment.body)),
                ("sourceFormat", json!("markdown")),
                ("threadId", json!(comment.thread_id)),
                ("path", json!(comment.path)),
                ("commitId", json!(comment.commit_id)),
                ("oldLine", line(comment.old_line)),
                ("newLine", line(comment.new_line)),
                ("operationId", json!(client::operation_id())),
            ])),
        },
    }
}

#[must_use]
pub(crate) fn resolve_call(
    via: &Via,
    repositories: &str,
    repository_id: &str,
    number: &str,
    thread_id: &str,
    resolved: bool,
) -> Call {
    let base = merge_request_path(via, repositories, repository_id, number);
    Call {
        post: true,
        path: format!("{base}/threads/{}/resolve", query_text(thread_id)),
        query: Vec::new(),
        body: Some(json!({ "resolved": resolved })),
    }
}

/// A review: `approve`, `comment` or `request_changes`, with its text.
#[must_use]
pub(crate) fn review_call(
    via: &Via,
    repositories: &str,
    repository_id: &str,
    number: &str,
    decision: &str,
    text: &str,
) -> Call {
    let base = merge_request_path(via, repositories, repository_id, number);
    let body = match via {
        Via::Framework => body(&[("decision", json!(decision)), ("note", json!(text))]),
        Via::Application(_) => body(&[
            ("decision", json!(decision)),
            ("body", json!(text)),
            ("sourceFormat", json!("markdown")),
            ("operationId", json!(client::operation_id())),
        ]),
    };
    Call {
        post: true,
        path: format!("{base}/reviews"),
        query: Vec::new(),
        body: Some(body),
    }
}

/// A thread as the page shows it, from Mantle's shape (`communicationEventId`, `commentList`) through the framework,
/// or Smithy's (`threadId`, `postList` with an `author`): its id, file and line, whether it is resolved or outdated,
/// and its comments (author, time, text and its format: `markdown`, or `html` already cleaned by the server).
#[must_use]
pub(crate) fn thread_item(thread: &Json) -> Json {
    let pick = |value: &Json, keys: &[&str]| -> Json {
        keys.iter()
            .map(|key| &value[*key])
            .find(|value| !value.is_null())
            .cloned()
            .unwrap_or(Json::Null)
    };
    let comments: Vec<Json> = ["commentList", "postList", "comments"]
        .iter()
        .find_map(|key| thread[*key].as_array())
        .into_iter()
        .flatten()
        .map(|comment| {
            let author = &comment["author"];
            let name = match author {
                Json::Object(_) => pick(author, &["name", "displayName", "handle"]),
                Json::String(_) => author.clone(),
                _ => pick(comment, &["authorName", "authorHandle"]),
            };
            json!({
                "commentId": pick(comment, &["commentId", "communicationEventId", "postId"]),
                "author": name,
                "created": pick(comment, &["entryDate", "createdAt", "created"]),
                "body": pick(comment, &["body", "text"]),
                "bodyFormat": pick(comment, &["bodyFormat"]).as_str().map_or_else(
                    // Smithy's posts and Comments come rendered (its threadsOf), as HTML
                    || if thread["postList"].is_array() { json!("html") } else { json!("markdown") },
                    |format| json!(format),
                ),
                "state": pick(comment, &["state"]),
            })
        })
        .collect();
    let line = |key: &str| thread[key].as_i64().map_or(Json::Null, |line| json!(line));
    json!({
        "threadId": pick(thread, &["threadId", "communicationEventId"]),
        "path": pick(thread, &["path"]),
        "oldLine": line("oldLine"),
        "newLine": line("newLine"),
        "commitId": pick(thread, &["commitId"]),
        "lineText": pick(thread, &["lineText"]),
        "outdated": thread["outdated"].as_bool().unwrap_or(false),
        "resolved": thread["resolved"].as_bool().unwrap_or(false),
        "comments": comments,
    })
}

/// The threads of an answer of either shape.
fn threads_of(answer: &Json) -> Vec<Json> {
    client::result_of(answer)["threadList"]
        .as_array()
        .into_iter()
        .flatten()
        .map(thread_item)
        .collect()
}

/// A merge request's number from the page: a whole number.
fn number_of(parameters: &Parameters) -> Result<String, LibError> {
    let number = text(parameters, "mergeRequestNum");
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(LibError::new("a merge request's number is a whole number"));
    }
    Ok(number)
}

/// A write on the server is a person's, from a session (21.9).
fn session_only() -> Result<(), LibError> {
    if moqui_core::screen::gate::header_authenticated_request() {
        return Err(LibError::new(
            "Comments and reviews are made from a session, not a request logged in by a header",
        ));
    }
    Ok(())
}

fn send(hosted: &Hosted, call: &Call) -> Result<Json, LibError> {
    call.send(&hosted.server)
}

fn threads(hosted: &Hosted, number: &str, path: &str) -> Result<(Vec<Json>, bool), LibError> {
    let call = threads_call(
        &hosted.via(),
        &hosted.server.rest_path("repositories"),
        &hosted.id(),
        number,
        path,
    );
    let answer = send(hosted, &call)?;
    let available = answer["threadsAvailable"].as_bool() != Some(false);
    Ok((threads_of(&answer), available))
}

/// `get#ReviewThreads`: a merge request's threads (of `path`), and whether the server keeps threads at all.
fn get_review_threads(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let (threads, available) = threads(&hosted, &number_of(parameters)?, &text(parameters, "path"))?;
    let mut out = Parameters::new();
    out.insert("threads".into(), json_value(&threads));
    out.insert("threadsAvailable".into(), Value::Bool(available));
    Ok(out)
}

/// The diff of a merge request as the server gives it: the framework's `diff`, or the application's `changes` (which
/// refuses the diff of an unsigned merge request: `diffRefused`).
fn changes_call(hosted: &Hosted, number: &str) -> Call {
    let via = hosted.via();
    let base = merge_request_path(&via, &hosted.server.rest_path("repositories"), &hosted.id(), number);
    match via {
        Via::Framework => Call {
            post: false,
            path: format!("{base}/diff"),
            query: vec![("withDiffs", "false".to_string())],
            body: None,
        },
        Via::Application(_) => Call {
            post: false,
            path: format!("{base}/changes"),
            query: vec![("withDiffs", "false".to_string())],
            body: None,
        },
    }
}

/// `get#MergeRequestChanges`: what a merge request changes, for the merge view: its base and head, its files and its
/// threads. An application that refuses the diff (an unsigned author) answers with its reason and no files.
fn get_merge_request_changes(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let number = number_of(parameters)?;
    let answer = send(&hosted, &changes_call(&hosted, &number))?;
    let made = client::result_of(&answer);
    let files: Vec<Json> = made["fileList"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|file| {
            json!({ "path": file["path"].as_str().or_else(|| file["newPath"].as_str()).unwrap_or(""),
                "oldPath": file["oldPath"], "status": file["status"].as_str().or_else(|| file["changeType"].as_str()) })
        })
        .collect();
    let (threads, available) = match threads(&hosted, &number, "") {
        Ok(found) => found,
        Err(err) => {
            tracing::warn!("Dev AIDE: the threads of !{number} were not read: {err}");
            (Vec::new(), false)
        }
    };
    let mut out = Parameters::new();
    out.insert(
        "baseCommitId".into(),
        json_text(&made["mergeBaseId"], &made["baseCommitId"]),
    );
    out.insert("headCommitId".into(), json_text(&made["headCommitId"], &Json::Null));
    out.insert("files".into(), json_value(&files));
    out.insert("threads".into(), json_value(&threads));
    out.insert("threadsAvailable".into(), Value::Bool(available));
    out.insert(
        "diffRefused".into(),
        Value::Bool(made["diffRefused"].as_bool() == Some(true)),
    );
    out.insert(
        "reason".into(),
        made["reason"].as_str().map_or(Value::Null, Value::string),
    );
    Ok(out)
}

fn json_text(first: &Json, second: &Json) -> Value {
    first
        .as_str()
        .or_else(|| second.as_str())
        .map_or(Value::Null, Value::string)
}

/// A file at a revision on the server (`blob`), its text; empty for one that is not there (added or deleted).
fn blob_text(server: &Server, repositories: &str, repository_id: &str, revision: &str, path: &str) -> String {
    if revision.is_empty() {
        return String::new();
    }
    server
        .get(
            &format!("{repositories}/{}/blob", query_text(repository_id)),
            &[("revision", revision), ("path", path)],
        )
        .ok()
        .and_then(|answer| answer["text"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// `get#MergeRequestFile`: one file of a merge request at its base and its head, for the merge view, with the file's
/// threads.
fn get_merge_request_file(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let number = number_of(parameters)?;
    let path = text(parameters, "path");
    if path.is_empty() || path.contains("..") {
        return Err(LibError::new("a file of the merge request is required"));
    }
    let repositories = hosted.server.rest_path("repositories");
    let (base, head) = (text(parameters, "baseCommitId"), text(parameters, "headCommitId"));
    let original = blob_text(&hosted.server, &repositories, &hosted.id(), &base, &path);
    let modified = blob_text(&hosted.server, &repositories, &hosted.id(), &head, &path);
    let (threads, _) = threads(&hosted, &number, &path).unwrap_or_default();
    let mut out = Parameters::new();
    out.insert("path".into(), Value::string(&path));
    out.insert("original".into(), Value::string(original));
    out.insert("modified".into(), Value::string(modified));
    out.insert("threads".into(), json_value(&threads));
    Ok(out)
}

fn line_of(parameters: &Parameters, key: &str) -> Option<i64> {
    text(parameters, key)
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|line| *line > 0)
}

/// `create#ReviewComment`: a comment on the server as the user: a reply (`threadId`), a new thread on a line of a file
/// (`path` with `newLine` or `oldLine`), or on the merge request.
fn create_review_comment(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    session_only()?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let number = number_of(parameters)?;
    let comment = Comment {
        body: text(parameters, "body"),
        thread_id: text(parameters, "threadId"),
        path: text(parameters, "path"),
        commit_id: text(parameters, "commitId"),
        old_line: line_of(parameters, "oldLine"),
        new_line: line_of(parameters, "newLine"),
        line_text: text(parameters, "lineText"),
    };
    if comment.body.trim().is_empty() {
        return Err(LibError::new("a comment has text"));
    }
    let call = comment_call(
        &hosted.via(),
        &hosted.server.rest_path("repositories"),
        &hosted.id(),
        &number,
        &comment,
    );
    let answer = send(&hosted, &call)?;
    let made = client::result_of(&answer);
    audit_git(
        &context,
        root,
        "review-comment",
        json!({ "server": hosted.server.base, "repository": hosted.id(), "number": number, "path": comment.path,
            "reply": !comment.thread_id.is_empty() }),
    );
    let mut out = Parameters::new();
    out.insert(
        "threadId".into(),
        made["threadId"]
            .as_str()
            .or_else(|| made["communicationEventId"].as_str())
            .or_else(|| made["postId"].as_str())
            .map_or(Value::Null, Value::string),
    );
    out.insert(
        "held".into(),
        Value::Bool(made["held"].as_bool() == Some(true) || made["state"] == "pending"),
    );
    Ok(out)
}

/// `resolve#ReviewThread`: a thread resolved on the server, or reopened (`resolved` false).
fn resolve_review_thread(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    session_only()?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let number = number_of(parameters)?;
    let thread = text(parameters, "threadId");
    if thread.is_empty() {
        return Err(LibError::new("threadId is required"));
    }
    let resolved = text(parameters, "resolved") != "false";
    let call = resolve_call(
        &hosted.via(),
        &hosted.server.rest_path("repositories"),
        &hosted.id(),
        &number,
        &thread,
        resolved,
    );
    send(&hosted, &call)?;
    audit_git(
        &context,
        root,
        "review-resolve",
        json!({ "number": number, "thread": thread, "resolved": resolved }),
    );
    let mut out = Parameters::new();
    out.insert("threadId".into(), Value::string(thread));
    out.insert("resolved".into(), Value::Bool(resolved));
    Ok(out)
}

/// `submit#Review`: a review of a merge request on the server: `approve`, `comment` or `request_changes`, with text.
fn submit_review(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    session_only()?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let number = number_of(parameters)?;
    let decision = text(parameters, "decision");
    if !matches!(decision.as_str(), "approve" | "comment" | "request_changes") {
        return Err(LibError::new("a review approves, comments or requests changes"));
    }
    let call = review_call(
        &hosted.via(),
        &hosted.server.rest_path("repositories"),
        &hosted.id(),
        &number,
        &decision,
        &text(parameters, "body"),
    );
    send(&hosted, &call)?;
    audit_git(
        &context,
        root,
        "review",
        json!({ "number": number, "decision": decision }),
    );
    let mut out = Parameters::new();
    out.insert("decision".into(), Value::string(decision));
    Ok(out)
}

/// The merge request a branch is of: `mr/<n>` (as Check out names it), else an open one whose source is the branch.
fn branch_merge_request(hosted: &Hosted, branch: &str) -> Option<String> {
    if let Some(number) = branch
        .strip_prefix("mr/")
        .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
    {
        return Some(number.to_string());
    }
    let call = super::review::list_call(
        &hosted.via(),
        &hosted.server.rest_path("repositories"),
        &hosted.id(),
        "open",
        &hosted.server.username,
    );
    let answer = call.send(&hosted.server).ok()?;
    client::first_list(&answer)
        .iter()
        .map(|item| super::review::merge_request_item(item, &hosted.server))
        .find(|item| item["sourceRef"].as_str() == Some(branch))
        .and_then(|item| match &item["mergeRequestNum"] {
            Json::Number(number) => Some(number.to_string()),
            Json::String(number) => Some(number.clone()),
            _ => None,
        })
}

/// `get#FileThreads`: the open threads on a file of the merge request whose branch is checked out (`mr/<n>`, or the
/// branch an open merge request is from), for the editor's gutter: each as a mark on its line of the new side.
fn get_file_threads(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let mut out = Parameters::new();
    let Some(hosted) = super::review::hosted(&context, root) else {
        out.insert("threads".into(), Value::List(Vec::new()));
        return Ok(out);
    };
    let branch = crate::vcs::current_branch(&context, root).unwrap_or_default();
    let Some(number) = branch_merge_request(&hosted, &branch) else {
        out.insert("threads".into(), Value::List(Vec::new()));
        return Ok(out);
    };
    let (threads, _) = threads(&hosted, &number, &text(parameters, "path"))?;
    let open: Vec<Json> = threads
        .into_iter()
        .filter(|thread| thread["resolved"] != true && thread["newLine"].is_i64())
        .collect();
    out.insert("mergeRequestNum".into(), Value::string(number));
    out.insert("threads".into(), json_value(&open));
    Ok(out)
}
