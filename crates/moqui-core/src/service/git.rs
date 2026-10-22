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

//! `org.moqui.impl.GitServices`: fetch, init, export and import of the repositories behind `git://` locations
//! (`crate::resource::git`). No Java counterpart. A failure is an error in the message facade, as a Java service's is.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::resource::git::browse::{short_name, RefInfo};
use crate::resource::git::diff::{FileChange, FileDiff};
use crate::resource::git::values::{commit_value, map, opt_text, tag_value, timestamp};
use crate::resource::git::{GitProvider, GIT_ARTIFACT_TYPE};

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, fetch_repository)?;
    rust_service!(map, fetch_all_repositories)?;
    rust_service!(map, init_repository)?;
    rust_service!(map, export_repository)?;
    rust_service!(map, import_repository)?;
    rust_service!(map, get_refs)?;
    rust_service!(map, get_log)?;
    rust_service!(map, get_commit)?;
    rust_service!(map, get_commit_diff)?;
    rust_service!(map, get_blob)?;
    rust_service!(map, get_clone_urls)?;
    rust_service!(map, commit_files)?;
    rust_service!(map, process_update)?;
    rust_service!(map, process_pending_updates)?;
    rust_service!(map, setup_lfs)?;
    rust_service!(map, gc_repositories)?;
    rust_service!(map, fsck_repository)?;
    Ok(())
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

pub(super) fn add_message(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_message(text));
}

/// The provider and the required `repositoryId`, or the error recorded.
pub(super) fn provider_and_id(parameters: &BTreeMap<String, Value>, need_id: bool) -> Option<(GitProvider, String)> {
    let Some(provider) = ExecutionContext::with_current(|ec| ec.factory().resource().git().cloned()).flatten() else {
        add_error("git resources are not installed (resource-reference scheme=\"git\" in the conf)");
        return None;
    };
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

/// `fetch#GitRepository`: fetch a remote mirror's branch. Out: `headCommitId`.
fn fetch_repository(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
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
    match provider.init(&id) {
        Ok(done) => add_message(done),
        Err(err) => {
            add_error(err.message());
            return Ok(BTreeMap::new());
        }
    }
    if flag(parameters, "contentRepository") {
        if let Err(err) = provider.setup_lfs(&id, &crate::resource::git::attributes::DEFAULT_LFS_PATTERNS) {
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
    if let Err(err) = crate::user::authorize_own(GIT_ARTIFACT_TYPE, &id, "update") {
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
        crate::resource::git::attributes::DEFAULT_LFS_PATTERNS.to_vec()
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

/// `export#GitRepository`: write a db repository as a bare repository on disk. Out: `exportPath`, `objectCount`.
fn export_repository(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return Ok(BTreeMap::new());
    };
    match provider.export(&id, text(parameters, "targetPath").as_deref()) {
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
    match provider.import(&id, text(parameters, "source").as_deref()) {
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

// ---- reading a repository: refs, log, commits, diffs, files ----

/// The provider and repository id when the current user may `view` it; otherwise the error recorded.
pub(super) fn viewable(parameters: &BTreeMap<String, Value>) -> Option<(GitProvider, String)> {
    let (provider, id) = provider_and_id(parameters, true)?;
    // a drive's members see the repository the drive holds, as the git server lets them fetch it
    if crate::resource::git::server::permitted(&id, "view") {
        return Some((provider, id));
    }
    match crate::user::authorize_own(GIT_ARTIFACT_TYPE, &id, "view") {
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
/// `unifiedDiff` and `html` (the escaped `moqui-diff` table) when it is text under the size limit. Out:
/// `fileDiffList`.
fn get_commit_diff(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = viewable(parameters) else {
        return Ok(BTreeMap::new());
    };
    let revision = text(parameters, "commitId").unwrap_or_default();
    let path = text(parameters, "path");
    match provider.commit_diffs(&id, &revision, path.as_deref()) {
        Ok(Some(diffs)) => Ok(out([(
            "fileDiffList",
            Value::List(
                diffs
                    .iter()
                    .map(|(change, diff)| {
                        let mut value = change_value(change, Some(diff));
                        if let (Value::Map(entries), Some(text)) = (&mut value, &diff.diff) {
                            entries.insert("unifiedDiff".to_string(), Value::string(text.unified()));
                            entries.insert("html".to_string(), Value::string(text.render_html()));
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
/// bytes (else `bytes`); nothing when it is not a file there.
fn get_blob(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = viewable(parameters) else {
        return Ok(BTreeMap::new());
    };
    let revision = text(parameters, "revision").unwrap_or_default();
    match provider.blob_at(&id, &revision, &path_segments(parameters, "path")) {
        Ok(Some(bytes)) => {
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

/// `get#GitCloneUrls`: how to clone a repository. Out: `httpsUrl` (`<base>/git/<id>.git`), and `sshUrl` when the SSH
/// server is on (`ssh-server` `port`), with the current user's username, as the SSH server logs in Moqui users.
fn get_clone_urls(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((_, id)) = viewable(parameters) else {
        return Ok(BTreeMap::new());
    };
    let property = |name: &str| moqui_lib::env::get_prop_or_env(name).trim().to_string();
    // an `ssh-server` attribute, else its property
    let ssh_setting = |name: &str, property_name: &str| {
        let configured =
            ExecutionContext::with_current(|ec| ec.factory().conf_setting("ssh-server", name)).unwrap_or_default();
        if configured.is_empty() {
            property(property_name)
        } else {
            configured
        }
    };
    let http = format!("{}/git/{id}.git", crate::resource::git::server::public_base_url());
    let ssh_port = Some(ssh_setting("public-port", "ssh_public_port"))
        .filter(|port| !port.is_empty())
        .unwrap_or_else(|| ssh_setting("port", "ssh_port"));
    let ssh = (!ssh_port.is_empty() && ssh_port != "0").then(|| {
        let host = [
            ssh_setting("public-host", "ssh_public_host"),
            property("webapp_http_host"),
            ssh_setting("host", "ssh_host"),
        ]
        .into_iter()
        .find(|host| !host.is_empty() && host != "0.0.0.0")
        .unwrap_or_else(|| "localhost".to_string());
        let user = crate::user::current_username()
            .ok()
            .flatten()
            .unwrap_or_else(|| "username".to_string());
        if ssh_port == "22" {
            format!("{user}@{host}:{id}.git")
        } else {
            format!("ssh://{user}@{host}:{ssh_port}/{id}.git")
        }
    });
    Ok(out([
        ("httpsUrl", Value::string(http)),
        ("sshUrl", ssh.map_or(Value::Null, Value::string)),
    ]))
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

/// `commit#GitFiles`: commit files on the default branch in one commit with a message. Out: `commitId` (empty when
/// nothing changed).
fn commit_files(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = provider_and_id(parameters, true) else {
        return Ok(BTreeMap::new());
    };
    if let Err(err) = crate::user::authorize_own(GIT_ARTIFACT_TYPE, &id, "update") {
        add_error(err.message());
        return Ok(BTreeMap::new());
    }
    let Some(message) = text(parameters, "message") else {
        add_error("message is required");
        return Ok(BTreeMap::new());
    };
    let segments = |path: &str| -> Vec<String> {
        path.split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_string)
            .collect()
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
    let removes: Vec<Vec<String>> = match parameters.get("removePaths") {
        Some(Value::List(paths)) => paths.iter().map(|path| segments(&to_plain_string(path))).collect(),
        _ => Vec::new(),
    };
    match provider.commit_files(&id, &message, &puts, &removes) {
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

pub(super) fn flag(parameters: &BTreeMap<String, Value>, key: &str) -> bool {
    text(parameters, key).is_some_and(|value| matches!(value.as_str(), "true" | "Y" | "y"))
}

/// `gc#GitRepositories`: delete the unreachable objects of db repositories and repack them. Out: `results`, a Map
/// per repository.
fn gc_repositories(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((provider, id)) = provider_and_id(parameters, false) else {
        return Ok(BTreeMap::new());
    };
    let minutes = number(parameters, "timeoutMinutes", 60).max(1);
    let options = crate::resource::git::gc::GcOptions::from_conf(
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
