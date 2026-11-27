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

//! `org.moqui.git.GitServices`: fetch, init, export and import of the repositories behind `git://` locations
//! ([`crate`]). No Java counterpart. A failure is an error in the message facade, as a Java service's is.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::rust_service;

use crate::browse::{short_name, RefInfo};
use crate::diff::{FileChange, FileDiff};
use crate::values::{commit_value, map, opt_text, tag_value, timestamp};
use crate::{GitProvider, GIT_ARTIFACT_TYPE};

#[cfg(test)]
mod clone_url_tests;
pub(crate) mod commit;
#[cfg(test)]
mod elfinder_tests;
#[cfg(test)]
mod files_tests;
#[cfg(test)]
mod gate_tests;
pub mod hosting;
#[cfg(test)]
mod images_tests;
#[cfg(test)]
mod import_source_tests;
mod repository;
pub(crate) mod mirror;
#[cfg(test)]
mod read_limits_tests;
pub(crate) mod rest;
pub mod review;
#[cfg(test)]
mod script_ports_tests;
#[cfg(test)]
mod trust_tests;
#[cfg(test)]
mod wiki_tests;

/// The Rust services of `org.moqui.git.GitServices` and of [`hosting`].
pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, fetch_repository)?;
    rust_service!(map, fetch_all_repositories)?;
    rust_service!(map, push_mirror)?;
    rust_service!(map, init_repository)?;
    rust_service!(map, export_repository)?;
    rust_service!(map, import_repository)?;
    rust_service!(map, get_refs)?;
    rust_service!(map, get_log)?;
    rust_service!(map, get_commit)?;
    rust_service!(map, get_commit_diff)?;
    rust_service!(map, get_blob)?;
    rust_service!(map, get_clone_urls)?;
    rust_service!(map, process_update)?;
    rust_service!(map, process_pending_updates)?;
    rust_service!(map, setup_lfs)?;
    rust_service!(map, gc_repositories)?;
    rust_service!(map, fsck_repository)?;
    hosting::register(map)?;
    review::register(map)?;
    commit::register(map)?;
    rest::register(map)?;
    mirror::register(map)?;
    repository::register(map)
}

pub(super) fn text(parameters: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .filter(|text| !text.trim().is_empty())
}

pub(super) fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

/// The code of a refusal an application must tell from the others: an error's text starts with it and `: `
/// (docs/interfaces/Git-Hosting.md, Error codes), as an error in the message facade carries no code of its own.
/// `None` for a reason that has none, or that starts with a code already.
pub(crate) fn code_of(reason: &str) -> Option<&'static str> {
    let coded = reason
        .split_once(": ")
        .is_some_and(|(code, _)| code.starts_with("Git") && code.chars().all(|c| c.is_ascii_alphanumeric()));
    if coded {
        None
    } else if reason.starts_with("protected branch:") {
        Some("GitProtectedBranch")
    } else if reason.ends_with("already exists") {
        Some("GitRefExists")
    } else if reason.contains("default branch cannot be deleted") {
        Some("GitDefaultBranch")
    } else if reason.contains("is archived") {
        Some("GitArchived")
    } else if reason.starts_with("no branch ") {
        Some("GitNoSuchRef")
    } else if reason.starts_with("stale info") {
        Some("GitStaleRef")
    } else {
        None
    }
}

/// [`add_error`] of a refusal, with its code before it when it has one ([`code_of`]).
pub(super) fn add_refusal(reason: impl Into<String>) {
    let reason = reason.into();
    match code_of(&reason) {
        Some(code) => add_error(format!("{code}: {reason}")),
        None => add_error(reason),
    }
}

/// While it lives, the service's reads by commit id reach the heads of hidden merge requests too: for a service
/// called with `includeHidden` and authorization disabled (the application shows a hidden merge request to its
/// author and its moderators, and decides who they are).
pub(super) fn hidden_guard(parameters: &BTreeMap<String, Value>) -> crate::reach::WithHidden {
    let asked = flag(parameters, "includeHidden")
        && ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
    crate::reach::with_hidden(asked)
}

pub(super) fn add_message(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_message(text));
}

/// Whether the service runs with authorization disabled: the framework's own call (a rule, a job, an async call) or
/// an application's that decided who may.
pub(super) fn internal_call() -> bool {
    ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false)
}

/// The provider and the required `repositoryId`, or the error recorded. With `AT_GIT_REPOSITORY` authorization off in
/// the conf every repository check would pass, so, as the git server, a call with authorization on is refused then.
pub(super) fn provider_and_id(parameters: &BTreeMap<String, Value>, need_id: bool) -> Option<(GitProvider, String)> {
    let Some(provider) = ExecutionContext::with_current(|ec| crate::provider(ec.factory())).flatten() else {
        add_error("git resources are not installed (resource-reference scheme=\"git\" in the conf)");
        return None;
    };
    let checked = ExecutionContext::with_current(|ec| ec.factory().artifact_flag(GIT_ARTIFACT_TYPE, "authz-enabled"))
        .unwrap_or(false);
    if !checked && !internal_call() {
        add_error("the git services are off: AT_GIT_REPOSITORY authorization is not enabled in the conf");
        return None;
    }
    let id = text(parameters, "repositoryId").unwrap_or_default();
    if need_id && id.is_empty() {
        add_error("repositoryId is required");
        return None;
    }
    Some((provider, id))
}

pub(super) fn out(entries: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

pub(super) fn count(value: usize) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// `fetch#GitRepository`: fetch a remote mirror's branch. Out: `headCommitId`. Needs the delete right on the
/// repository (it reaches out of the server with the repository's credential), or authorization disabled.
fn fetch_repository(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = hosting::allowed(parameters, "delete") else {
        return Ok(BTreeMap::new());
    };
    match provider.fetch(&id) {
        Ok(head) => Ok(out([("headCommitId", head.map_or(Value::Null, Value::string))])),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `push#GitMirror`: push a repository to its push mirror. Out: `pushResult`, empty when it has none. Needs the
/// delete right on the repository, as its `GitRepositoryMirror` rows do, or authorization disabled (the rule's async
/// call).
fn push_mirror(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = hosting::allowed(parameters, "delete") else {
        return Ok(BTreeMap::new());
    };
    match provider.push_mirror(&id) {
        Ok(done) => Ok(out([("pushResult", done.map_or(Value::Null, Value::string))])),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `fetch#AllGitRepositories`: fetch every remote mirror whose interval has passed. A failure is recorded on that
/// repository's row and logged; the job goes on with the rest. Out: `fetchedIds`, `failedIds`.
fn fetch_all_repositories(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, _)) = provider_and_id(parameters, false) else {
        return Ok(BTreeMap::new());
    };
    match provider.fetch_due() {
        Ok((fetched, failed)) => Ok(out([
            (
                "fetchedIds",
                Value::List(fetched.into_iter().map(Value::string).collect()),
            ),
            (
                "failedIds",
                Value::List(failed.into_iter().map(Value::string).collect()),
            ),
        ])),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `init#GitRepository`: create a local bare repository, or fetch a remote one for the first time; with
/// `contentRepository`, keep images, video, audio, PDFs and archives in Git LFS.
fn init_repository(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return Ok(BTreeMap::new());
    };
    // the commit of .gitattributes asks what setup#GitLfs asks, before anything is made
    let content = flag(parameters, "contentRepository");
    if content {
        if let Err(err) = moqui_core::user::authorize_own(GIT_ARTIFACT_TYPE, &id, "update") {
            add_error(err.message());
            return Ok(BTreeMap::new());
        }
    }
    match provider.init(&id) {
        Ok(done) => add_message(done),
        Err(err) => {
            add_error(err.message());
            return Ok(BTreeMap::new());
        }
    }
    if content {
        if let Err(err) = provider.setup_lfs(&id, &crate::attributes::DEFAULT_LFS_PATTERNS) {
            add_error(err.message());
        }
    }
    Ok(BTreeMap::new())
}

/// `setup#GitLfs`: keep files matching `patterns` (images, video, audio, PDFs and archives when empty) in Git LFS,
/// by a commit of the repository's `.gitattributes`. Out: `commitId` (empty when it had them all).
fn setup_lfs(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return Ok(BTreeMap::new());
    };
    if let Err(err) = moqui_core::user::authorize_own(GIT_ARTIFACT_TYPE, &id, "update") {
        add_error(err.message());
        return Ok(BTreeMap::new());
    }
    let given: Vec<String> = match parameters.get("patterns") {
        Some(Value::List(patterns)) => patterns
            .iter()
            .map(to_plain_string)
            .filter(|text| !text.trim().is_empty())
            .collect(),
        Some(value) if !is_empty(value) => to_plain_string(value)
            .split([',', ' ', '\n'])
            .filter(|pattern| !pattern.trim().is_empty())
            .map(|pattern| pattern.trim().to_string())
            .collect(),
        _ => Vec::new(),
    };
    let patterns: Vec<&str> = if given.is_empty() {
        crate::attributes::DEFAULT_LFS_PATTERNS.to_vec()
    } else {
        given.iter().map(String::as_str).collect()
    };
    match provider.setup_lfs(&id, &patterns) {
        Ok(made) => Ok(out([(
            "commitId",
            made.map_or(Value::Null, |id| Value::string(id.to_string())),
        )])),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// Where a caller with authorization on exports to: a `targetPath` of safe relative segments under it, so an export
/// writes no other directory of the server and no repository it holds.
pub(crate) const EXPORT_DIR: &str = "git-export";

/// `export#GitRepository`: write a db repository as a bare repository on disk. Out: `exportPath`, `objectCount`.
/// Needs the delete right on the repository, or authorization disabled. With authorization on, `targetPath` is a
/// relative path under `runtime/git-export`; with it disabled, any path, as before, unless `underExportDir` holds it
/// there too (`FileServices.export#Drive`, which checked the drive's own view right and calls with authorization
/// disabled).
fn export_repository(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = hosting::allowed(parameters, "delete") else {
        return Ok(BTreeMap::new());
    };
    let held = !internal_call() || flag(parameters, "underExportDir");
    let target = match text(parameters, "targetPath").map(|path| path.trim().to_string()) {
        Some(path) if !path.is_empty() && held => {
            // segments as a repository path's, one ending in .git allowed (a bare repository's usual name)
            let checked = path
                .split('/')
                .map(|segment| {
                    segment
                        .strip_suffix(".git")
                        .filter(|name| !name.is_empty())
                        .unwrap_or(segment)
                })
                .collect::<Vec<_>>()
                .join("/");
            if let Err(why) = crate::server::path::check_path(&checked) {
                add_error(format!(
                    "targetPath is a relative path under runtime/{EXPORT_DIR} ({why}); another place takes \
                     authorization disabled"
                ));
                return Ok(BTreeMap::new());
            }
            Some(format!("{EXPORT_DIR}/{path}"))
        }
        other => other,
    };
    match provider.export(&id, target.as_deref()) {
        Ok((path, written)) => {
            let path = path.to_string_lossy().into_owned();
            add_message(format!("Exported git repository {id} to {path} ({written} objects)"));
            Ok(out([
                ("exportPath", Value::string(path)),
                ("objectCount", count(written)),
            ]))
        }
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `import#GitRepository`: copy a branch into a db repository. Out: `headCommitId`, `objectCount`.
fn import_repository(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return Ok(BTreeMap::new());
    };
    let source = text(parameters, "source");
    if let Err(why) = import_source_allowed(&id, source.as_deref()) {
        add_error(why);
        return Ok(BTreeMap::new());
    }
    match provider.import(&id, source.as_deref()) {
        Ok((head, copied)) => {
            add_message(format!(
                "Imported {copied} objects into git repository {id}, head {head}"
            ));
            Ok(out([
                ("headCommitId", Value::string(head)),
                ("objectCount", count(copied)),
            ]))
        }
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// The sealed permission a caller needs to import from a directory on the server or a URL that is not http(s): reading
/// the host's files is host access, as a terminal on the host is.
const IMPORT_HOST_PERMISSION: &str = "TERMINAL_HOST";

/// Whether the caller may import repository `id` from `source` (plans/archive/Security-Review.md SEC-GIT-01). No source is the
/// repository's own `remoteUrl`, which whoever configured it chose. A URL is fetched only over https (http with
/// `webhook-private-network`), with no credential in it, from a host whose addresses are all public unless private
/// networks are allowed, whoever calls. With authorization on, another repository (`git://<id>`) needs the view right
/// on it, and a directory or another scheme the sealed `TERMINAL_HOST`.
fn import_source_allowed(id: &str, source: Option<&str>) -> Result<(), String> {
    let Some(source) = source.map(str::trim).filter(|source| !source.is_empty()) else {
        return Ok(());
    };
    let authz_off = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
    if let Some(other) = source.strip_prefix("git://") {
        let other = other.trim_matches('/');
        if authz_off || crate::server::permitted(other, "view") {
            return Ok(());
        }
        return Err(format!("Repository {other} not found"));
    }
    if source.starts_with("https://") || source.starts_with("http://") {
        let private = crate::mirror::private_network();
        crate::server::smart_http::check_url(source, private)
            .map_err(|why| format!("cannot import {id} from {}: {why}", moqui_core::outbound::redacted(source)))?;
        if !private {
            let rest = source.split_once("://").map_or("", |(_, rest)| rest);
            let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
            let default_port = if source.starts_with("https://") { 443 } else { 80 };
            let (host, port) = moqui_core::sftp::split_host_port(authority, default_port).map_err(|err| err.message().to_string())?;
            moqui_lib::net::public_addresses(&host, port)
                .map_err(|err| format!("cannot import {id} from {}: {err}", moqui_core::outbound::redacted(source)))?;
        }
        return Ok(());
    }
    if authz_off || moqui_core::user::has_permission(IMPORT_HOST_PERMISSION).unwrap_or(false) {
        return Ok(());
    }
    Err(format!(
        "cannot import {id} from {}: only an https URL or another repository, unless you have {IMPORT_HOST_PERMISSION}",
        moqui_core::outbound::redacted(source)
    ))
}

// ---- reading a repository: refs, log, commits, diffs, files ----

/// The provider and repository id when the current user may `view` it; otherwise the error recorded.
pub(super) fn viewable(parameters: &BTreeMap<String, Value>) -> Option<(GitProvider, String)> {
    let (provider, id) = provider_and_id(parameters, true)?;
    // a drive's members see the repository the drive holds, as the git server lets them fetch it
    if crate::server::permitted(&id, "view") {
        return Some((provider, id));
    }
    match moqui_core::user::authorize_own(GIT_ARTIFACT_TYPE, &id, "view") {
        Ok(()) => Some((provider, id)),
        Err(err) => {
            add_error(err.message());
            None
        }
    }
}

fn ref_value(info: &RefInfo, branch: &str) -> Value {
    let commit = info.commit.as_ref();
    map([
        ("refName", Value::string(&info.name)),
        ("shortName", Value::string(short_name(&info.name))),
        ("refType", Value::string(info.kind.name())),
        ("objectId", Value::string(info.target.to_string())),
        ("isDefault", Value::Bool(info.name == format!("refs/heads/{branch}"))),
        ("annotated", Value::Bool(info.tag.is_some())),
        (
            "commitId",
            commit.map_or(Value::Null, |commit| Value::string(commit.id.to_string())),
        ),
        (
            "summary",
            commit.map_or(Value::Null, |commit| Value::string(commit.summary())),
        ),
        (
            "commitDate",
            commit.map_or(Value::Null, |commit| timestamp(commit.committer.seconds)),
        ),
        (
            "authorName",
            commit.map_or(Value::Null, |commit| opt_text(&commit.author.name)),
        ),
        ("tag", info.tag.as_ref().map_or(Value::Null, tag_value)),
    ])
}

pub(super) fn change_value(change: &FileChange, diff: Option<&FileDiff>) -> Value {
    let mut entries = vec![
        ("path", Value::string(&change.path)),
        ("changeType", Value::string(change.kind.name())),
        (
            "oldObjectId",
            change.old.map_or(Value::Null, |(_, id)| Value::string(id.to_string())),
        ),
        (
            "newObjectId",
            change.new.map_or(Value::Null, |(_, id)| Value::string(id.to_string())),
        ),
    ];
    if let Some(diff) = diff {
        let stats = diff.diff.as_ref().map(|diff| diff.stats);
        entries.extend([
            ("binary", Value::Bool(diff.binary)),
            ("tooLarge", Value::Bool(diff.too_large)),
            ("insertions", stats.map_or(Value::Null, |stats| count(stats.insertions))),
            ("deletions", stats.map_or(Value::Null, |stats| count(stats.deletions))),
        ]);
    }
    map(entries)
}

/// A per-repository cap of the conf's `git-server` (`max-webhooks`, `max-mirrors`): `default` when it is not a number,
/// and no cap when it is 0.
pub(super) fn max_per_repository(name: &str, default: i64) -> i64 {
    match crate::setting(name).parse::<i64>() {
        Ok(0) => i64::MAX,
        Ok(most) if most > 0 => most,
        _ => default,
    }
}

pub(super) fn number(parameters: &BTreeMap<String, Value>, key: &str, default: usize) -> usize {
    text(parameters, key)
        .and_then(|text| text.parse().ok())
        .unwrap_or(default)
}

/// A path parameter as segments; empty for none.
pub(super) fn path_segments(parameters: &BTreeMap<String, Value>, key: &str) -> Vec<String> {
    text(parameters, key)
        .map(|path| {
            path.split('/')
                .filter(|segment| !segment.is_empty() && *segment != ".")
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `get#GitRefs`: the branches and tags. Out: `defaultBranch`, `headCommitId`, `refList`.
fn get_refs(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = viewable(parameters) else {
        return Ok(BTreeMap::new());
    };
    let prefix = match text(parameters, "refType").as_deref() {
        Some("branch") => "refs/heads/",
        Some("tag") => "refs/tags/",
        _ => "refs/",
    };
    match provider.refs(&id, prefix) {
        Ok(refs) => Ok(out([
            ("defaultBranch", Value::string(&refs.branch)),
            (
                "headCommitId",
                refs.head.map_or(Value::Null, |head| Value::string(head.to_string())),
            ),
            (
                "refList",
                Value::List(refs.refs.iter().map(|info| ref_value(info, &refs.branch)).collect()),
            ),
        ])),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `get#GitLog`: commits newest first from a revision, optionally only those that changed a path. Out: `commitList`,
/// `hasMore` (pass the last commit's id as `afterCommitId` for the next page).
fn get_log(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = viewable(parameters) else {
        return Ok(BTreeMap::new());
    };
    let _hidden = hidden_guard(parameters);
    let revision = text(parameters, "revision").unwrap_or_default();
    let after = text(parameters, "afterCommitId");
    let limit = number(parameters, "limit", 50);
    match provider.log(
        &id,
        &revision,
        &path_segments(parameters, "path"),
        after.as_deref(),
        limit,
    ) {
        Ok(page) => Ok(out([
            (
                "commitList",
                Value::List(page.commits.iter().map(commit_value).collect()),
            ),
            ("hasMore", Value::Bool(page.more)),
        ])),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `get#GitCommit`: a commit and the files it changed against its first parent, with line counts. Out: `commit`,
/// `fileList`, `insertions`, `deletions`; no `commit` when the revision names no commit.
fn get_commit(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = viewable(parameters) else {
        return Ok(BTreeMap::new());
    };
    let _hidden = hidden_guard(parameters);
    let revision = text(parameters, "commitId").unwrap_or_default();
    let with_stats = text(parameters, "stats").is_none_or(|stats| stats != "false" && stats != "N");
    let outcome = provider.commit_changes(&id, &revision).and_then(|found| {
        let Some((commit, changes)) = found else {
            return Ok(None);
        };
        let diffs = if with_stats {
            provider
                .commit_diffs(&id, &commit.id.to_string(), None)?
                .unwrap_or_default()
        } else {
            changes.into_iter().map(|change| (change, empty_diff())).collect()
        };
        Ok(Some((commit, diffs)))
    });
    match outcome {
        Ok(Some((commit, diffs))) => {
            let (mut insertions, mut deletions) = (0, 0);
            for (_, diff) in &diffs {
                if let Some(diff) = &diff.diff {
                    insertions += diff.stats.insertions;
                    deletions += diff.stats.deletions;
                }
            }
            Ok(out([
                ("commit", commit_value(&commit)),
                (
                    "fileList",
                    Value::List(
                        diffs
                            .iter()
                            .map(|(change, diff)| change_value(change, with_stats.then_some(diff)))
                            .collect(),
                    ),
                ),
                ("insertions", count(insertions)),
                ("deletions", count(deletions)),
            ]))
        }
        Ok(None) => Ok(BTreeMap::new()),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

fn empty_diff() -> FileDiff {
    FileDiff {
        binary: false,
        too_large: false,
        diff: None,
    }
}

/// `get#GitCommitDiff`: the line diff of each file a commit changed (or of the one at `path`): the file's change with
/// `unifiedDiff` and `html` (the escaped `moqui-diff` table) when it is text under the size limit; with `diffLineLimit`
/// its `hunks` instead, of only the lines of that window from `diffLineOffset`, as get#GitCompare's
/// ([`hosting::diff_window`]). Out: `fileDiffList`.
fn get_commit_diff(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = viewable(parameters) else {
        return Ok(BTreeMap::new());
    };
    let _hidden = hidden_guard(parameters);
    let revision = text(parameters, "commitId").unwrap_or_default();
    let path = text(parameters, "path");
    let most = crate::browse::commit_max_diff_files();
    let line_offset = number(parameters, "diffLineOffset", 0);
    let line_limit = number(parameters, "diffLineLimit", 0);
    match provider.commit_diffs(&id, &revision, path.as_deref()) {
        Ok(Some(diffs)) => Ok(out([(
            "fileDiffList",
            Value::List(
                diffs
                    .iter()
                    .enumerate()
                    .map(|(position, (change, diff))| {
                        let mut value = change_value(change, Some(diff));
                        // past the first files a file is counted, not shown (its hunks have no lines)
                        let full = path.is_some() || position < most;
                        if let (Value::Map(entries), Some(text)) = (&mut value, &diff.diff) {
                            if full && line_limit > 0 {
                                hosting::diff_window(entries, text, line_offset, line_limit);
                            } else if full {
                                entries.insert("unifiedDiff".to_string(), Value::string(text.unified()));
                                entries.insert("html".to_string(), Value::string(text.render_html()));
                            } else {
                                entries.insert("diffOmitted".to_string(), Value::Bool(true));
                            }
                        }
                        value
                    })
                    .collect(),
            ),
        )])),
        Ok(None) => Ok(BTreeMap::new()),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `get#GitBlob`: the file at a path in a revision. Out: `size`, `binary`, and `text` for a UTF-8 file without NUL
/// bytes (else `bytes`); nothing when it is not a file there. A file over `git-server` `blob-max-bytes` (16 MiB, an
/// LFS file's content counted) is not read: `size` and `tooLarge` only (a `git://` location streams it).
fn get_blob(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = viewable(parameters) else {
        return Ok(BTreeMap::new());
    };
    let _hidden = hidden_guard(parameters);
    let revision = text(parameters, "revision").unwrap_or_default();
    let most = Some(crate::number_setting("blob-max-bytes", 16u64 * 1024 * 1024))
        .filter(|most| *most > 0)
        .unwrap_or(16 * 1024 * 1024);
    match provider.file_at(&id, &revision, &path_segments(parameters, "path"), most) {
        Ok(Some(crate::browse::FileAt::TooLarge(size))) => Ok(out([
            ("size", Value::I64(i64::try_from(size).unwrap_or(i64::MAX))),
            ("tooLarge", Value::Bool(true)),
        ])),
        Ok(Some(crate::browse::FileAt::Content(bytes))) => {
            let size = count(bytes.len());
            let binary = bytes.iter().take(8000).any(|byte| *byte == 0);
            let content = match (binary, String::from_utf8(bytes)) {
                (false, Ok(text)) => ("text", Value::string(text)),
                (_, Ok(text)) => ("bytes", Value::Bytes(text.into_bytes())),
                (_, Err(err)) => ("bytes", Value::Bytes(err.into_bytes())),
            };
            let binary = content.0 == "bytes";
            Ok(out([("size", size), ("binary", Value::Bool(binary)), content]))
        }
        Ok(None) => Ok(BTreeMap::new()),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `get#GitCloneUrls`: how to clone a repository. Out: `httpsUrl` (`<base>/git/<path>.git`, the path the repository's
/// `repositoryPath`, else its id), `sshUrl` when the SSH server is on (`ssh-server` `port`, and `enabled` not false),
/// with the current user's username, as the SSH server logs in Moqui users, and `repositoryPath`. The SSH host and
/// port are `public-host` and `public-port` when either is set; else, on a node paired with an edge node whose offer
/// names an SSH tunnel to it, that tunnel's host and port (`moqui_core::edge::setup::ssh_address`); else
/// `webapp-list/webapp[webroot]@http-host` and the server's own port.
fn get_clone_urls(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((_, id)) = viewable(parameters) else {
        return Ok(BTreeMap::new());
    };
    let id = crate::server::path::path_of(&id);
    let http = format!("{}/git/{id}.git", crate::server::public_base_url());
    let ssh = ssh_host_port().map(|(host, port)| {
        let user = moqui_core::user::current_username()
            .ok()
            .flatten()
            .unwrap_or_else(|| "username".to_string());
        if port == "22" {
            format!("{user}@{host}:{id}.git")
        } else {
            format!("ssh://{user}@{host}:{port}/{id}.git")
        }
    });
    Ok(out([
        ("httpsUrl", Value::string(http)),
        ("sshUrl", ssh.map_or(Value::Null, Value::string)),
        ("repositoryPath", Value::string(id)),
    ]))
}

/// The host and port SSH clone URLs name, from the `ssh-server` settings and an edge node's tunnel; none while the SSH
/// server is off ([`get_clone_urls`], the hosting offer).
pub(crate) fn ssh_host_port() -> Option<(String, String)> {
    let ssh_setting = |name: &str| moqui_core::context::conf_env::setting(&format!("ssh-server@{name}"));
    let public_host = ssh_setting("public-host");
    let public_port = ssh_setting("public-port");
    // behind an edge node, with neither public setting: the edge node's SSH tunnel, as its offer named it
    let edge = (public_host.is_empty() && public_port.is_empty())
        .then(|| ExecutionContext::with_current(|ec| moqui_core::edge::setup::ssh_address(ec.factory())).flatten())
        .flatten();
    SshAddress {
        enabled: ExecutionContext::with_current(|ec| ec.factory().conf_flag("ssh-server", "enabled", true))
            .unwrap_or(true),
        public_host,
        public_port,
        edge,
        http_host: moqui_core::context::conf_env::webroot_setting("http-host"),
        host: ssh_setting("host"),
        port: ssh_setting("port"),
    }
    .host_and_port()
}

/// What an SSH clone URL is made of ([`get_clone_urls`]): the `ssh-server` settings and the edge node's SSH tunnel.
struct SshAddress {
    enabled: bool,
    public_host: String,
    public_port: String,
    /// The edge node's tunnel, host and port, when this node is paired and neither public setting is set.
    edge: Option<(String, u16)>,
    http_host: String,
    host: String,
    port: String,
}

impl SshAddress {
    /// The host and port an SSH clone URL names; none while the SSH server is off (or listens on port 0 with nothing
    /// in front of it). The edge node's tunnel only while this node's own server listens behind it.
    fn host_and_port(self) -> Option<(String, String)> {
        let behind = self.edge.is_none() || !self.port.is_empty();
        let port = match &self.edge {
            Some((_, port)) => port.to_string(),
            None if !self.public_port.is_empty() => self.public_port,
            None => self.port,
        };
        if !self.enabled || !behind || port.is_empty() || port == "0" {
            return None;
        }
        let host = [
            self.edge.map(|(host, _)| host).unwrap_or_default(),
            self.public_host,
            self.http_host,
            self.host,
        ]
        .into_iter()
        .find(|host| !host.is_empty() && host != "0.0.0.0")
        .unwrap_or_else(|| "localhost".to_string());
        Some((host, port))
    }
}

// ---- gitECA events ----

/// `process#GitUpdate`: run the post events of a pending update. Out: `ecaStatus`.
fn process_update(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, _)) = provider_and_id(parameters, false) else {
        return Ok(BTreeMap::new());
    };
    let Some(update_id) = text(parameters, "updateId") else {
        add_error("updateId is required");
        return Ok(BTreeMap::new());
    };
    match provider.process_update(&update_id) {
        Ok(status) => Ok(out([("ecaStatus", Value::string(status))])),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `process#PendingGitUpdates`: run every pending update. Out: `processedCount`, `failedCount`.
fn process_pending_updates(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, _)) = provider_and_id(parameters, false) else {
        return Ok(BTreeMap::new());
    };
    match provider.process_pending() {
        Ok((processed, failed)) => Ok(out([
            ("processedCount", count(processed)),
            ("failedCount", count(failed)),
        ])),
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

pub(super) fn flag(parameters: &BTreeMap<String, Value>, key: &str) -> bool {
    text(parameters, key).is_some_and(|value| matches!(value.as_str(), "true" | "Y" | "y"))
}

/// `gc#GitRepositories`: delete the unreachable objects of db and local repositories and repack them. Out: `results`,
/// a Map per repository.
fn gc_repositories(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = provider_and_id(parameters, false) else {
        return Ok(BTreeMap::new());
    };
    let minutes = number(parameters, "timeoutMinutes", 60).max(1);
    let options = crate::gc::GcOptions::from_conf(
        flag(parameters, "dryRun"),
        std::time::Duration::from_secs(u64::try_from(minutes).unwrap_or(60) * 60),
    );
    let repository = (!id.is_empty()).then_some(id.as_str());
    match provider.gc(repository, &options) {
        Ok(outcomes) => {
            let mut results = Vec::with_capacity(outcomes.len());
            for (repository_id, outcome) in outcomes {
                let mut entries = vec![("repositoryId", Value::string(&repository_id))];
                match outcome {
                    Ok(report) => {
                        let number = |value: u64| Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
                        entries.extend([
                            ("deletedCount", number(report.deleted)),
                            ("deletedBytes", number(report.deleted_bytes)),
                            ("repackedCount", count(report.repacked)),
                            ("savedBytes", Value::I64(report.saved_bytes)),
                            ("objectCount", count(report.objects)),
                            ("storedBytes", number(report.stored_bytes)),
                            ("summary", Value::string(report.summary())),
                        ]);
                    }
                    Err(err) => {
                        // recorded on the repository and logged; the others still ran
                        add_error(format!("git gc of {repository_id}: {}", err.message()));
                        entries.push(("error", Value::string(err.message())));
                    }
                }
                results.push(map(entries));
            }
            Ok(out([("results", Value::List(results))]))
        }
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}

/// `fsck#GitRepository`: decode every object of a db repository and check it against its id. Out: `checkedCount`,
/// `errors`.
fn fsck_repository(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return Ok(BTreeMap::new());
    };
    let minutes = number(parameters, "timeoutMinutes", 60).max(1);
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(u64::try_from(minutes).unwrap_or(60) * 60);
    match provider.fsck(&id, deadline) {
        Ok(report) => {
            if report.errors.is_empty() {
                add_message(format!(
                    "Checked {} objects of git repository {id}: no errors",
                    report.checked
                ));
            } else {
                add_error(format!(
                    "git repository {id} has {} damaged objects or refs: {}",
                    report.errors.len(),
                    report.errors.iter().take(5).cloned().collect::<Vec<_>>().join("; ")
                ));
            }
            if let Some(stopped) = &report.stopped {
                add_message(stopped.clone());
            }
            Ok(out([
                ("checkedCount", count(report.checked)),
                (
                    "errors",
                    Value::List(report.errors.into_iter().map(Value::string).collect()),
                ),
            ]))
        }
        Err(err) => {
            add_error(err.message());
            Ok(BTreeMap::new())
        }
    }
}
