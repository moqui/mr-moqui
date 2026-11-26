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

//! Hosted roots (plans/design-dev-aide.md 4.10, P10): a repository of this server's git hosting at a branch, edited in
//! the Workspace with no working tree. Its tree, files, Go to file and search are read through
//! `org.moqui.git.GitServices` (`get#GitTree`, `get#GitBlob`) as the user, so `AT_GIT_REPOSITORY` decides what they
//! read, at the root's branch; each Save is a commit by the user through `commit#GitFiles`, which checks the update
//! right and the branch rules as a push does (a protected branch refuses it) and refuses the commit when the file's
//! blob is no longer the one the editor opened, which the Workspace answers with Compare as it does a file changed on
//! disk. A hosted root is not loaded or run, as a worktree is not (question 19): its files are checked, never
//! reloaded into the server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::EntityValue;
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value, ValueMap};
use serde::Serialize;
use serde_json::json;

use crate::audit::{self, Write};
use crate::roots::{self, Root, RootKind};
use crate::tier::Tier;
use crate::workspace::{json_value, text, unfenced, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(
        moqui_core::service_location!(get_hosted_repositories),
        get_hosted_repositories,
    )?;
    registry.service_function(moqui_core::service_location!(get_hosted_root), get_hosted_root)?;
    registry.service_function(moqui_core::service_location!(store_hosted_root), store_hosted_root)?;
    registry.service_function(moqui_core::service_location!(delete_hosted_root), delete_hosted_root)
}

/// `DevAideRoot.rootKind` of a hosted root.
pub const KIND: &str = "hosted";

const GIT: &str = "org.moqui.git.GitServices.";

/// The most files Go to file lists and the most directories it reads, and the most files a search reads.
const MAX_FILES: usize = 20_000;
const MAX_DIRECTORIES: usize = 2_000;
const MAX_SEARCHED: usize = 5_000;

/// What a hosted root is: the repository and the branch its saves commit to.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostedBranch {
    pub repository_id: String,
    pub branch: String,
    /// Who added it: they (or the full tier) remove it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hosted_by: Option<String>,
}

/// The location a hosted root's row keeps: a name for the root, never a directory.
fn location_of(repository_id: &str, branch: &str) -> String {
    format!("git://{repository_id}@{branch}")
}

/// A hosted root's id: the repository and branch, in the characters a root id has.
fn id_of(repository_id: &str, branch: &str) -> String {
    format!("{repository_id}-{branch}")
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

/// The root of a `DevAideRoot` row of kind hosted, or None (logged) when it names no repository.
pub(crate) fn root_of(row: &EntityValue) -> Option<Root> {
    let field = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
    let (repository_id, branch) = (field("gitRepositoryId"), field("gitBranch"));
    if repository_id.is_empty() || branch.is_empty() {
        tracing::warn!(
            "Dev AIDE hosted root {} names no repository and branch; left out",
            field("rootId")
        );
        return None;
    }
    let label = Some(field("label"))
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| format!("{repository_id} @ {branch}"));
    Some(Root {
        id: id_of(&repository_id, &branch),
        label,
        path: PathBuf::from(location_of(&repository_id, &branch)),
        kind: RootKind::Hosted,
        components: Vec::new(),
        // the hosting's rights decide its writes, not the site components (4.10)
        upstream: false,
        read_only: field("readOnly") == "Y",
        publish_branch: None,
        publish_branch_required: false,
        hosted: None,
        worktree_of: None,
        conversation_id: None,
        worktree_user_id: None,
        hosted_branch: Some(HostedBranch {
            repository_id,
            branch,
            hosted_by: Some(field("hostedByUserId")).filter(|user| !user.is_empty()),
        }),
    })
}

/// A path of a hosted root, as the repository names it: relative, with no `.`, `..` or `.git` segment.
fn clean_path(relative: &str) -> Result<String, LibError> {
    let segments: Vec<&str> = relative
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.iter().any(|segment| matches!(*segment, "." | ".." | ".git")) {
        return Err(LibError::new(format!("{relative} is not a path in the repository")));
    }
    Ok(segments.join("/"))
}

/// Call `org.moqui.git.GitServices.<name>` as the user: what they may not do there is refused there. The call's
/// errors are its answer's, not left in the message facade, so one read that fails (a file of a search) fails alone.
fn git(name: &str, parameters: &[(&str, Value)]) -> Result<BTreeMap<String, Value>, LibError> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    ExecutionContext::with_current(|ec| ec.message().push_errors());
    let called = moqui_core::service::sync(&format!("{GIT}{name}"), &parameters);
    let errors = ExecutionContext::with_current(|ec| {
        let mut message = ec.message();
        let errors = message.errors().to_vec();
        message.clear_errors();
        message.pop_errors();
        errors
    })
    .unwrap_or_default();
    let out = called.map_err(|err| LibError::new(err.to_string()))?;
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(LibError::new(errors.join("; ")))
    }
}

fn out_text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

fn out_list<'a>(out: &'a BTreeMap<String, Value>, key: &str) -> &'a [Value] {
    match out.get(key) {
        Some(Value::List(items)) => items,
        _ => &[],
    }
}

fn item_text(item: &Value, key: &str) -> String {
    match item {
        Value::Map(map) => map.get(key).map(to_plain_string).unwrap_or_default(),
        _ => String::new(),
    }
}

/// A directory of the repository at `revision`: its commit and entries (name, type, blob id, size).
struct Listing {
    commit: String,
    entries: Vec<Value>,
}

fn listing(hosted: &HostedBranch, revision: &str, dir: &str) -> Result<Listing, LibError> {
    let out = git(
        "get#GitTree",
        &[
            ("repositoryId", Value::string(&hosted.repository_id)),
            ("revision", Value::string(revision)),
            ("path", Value::string(dir)),
        ],
    )?;
    Ok(Listing {
        commit: out_text(&out, "commitId"),
        entries: out_list(&out, "entryList").to_vec(),
    })
}

/// The entry of the file `path` at `revision`, with the commit it was read at.
fn entry_at(hosted: &HostedBranch, revision: &str, path: &str) -> Result<Option<(String, Value)>, LibError> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let found = listing(hosted, revision, dir)?;
    Ok(found
        .entries
        .into_iter()
        .find(|entry| item_text(entry, "name") == name && item_text(entry, "type") != "dir")
        .map(|entry| (found.commit, entry)))
}

/// A file's content as `get#GitBlob` answers it at `revision`.
struct Blob {
    text: Option<String>,
    binary: bool,
    too_large: bool,
    size: i64,
}

fn blob(hosted: &HostedBranch, revision: &str, path: &str) -> Result<Blob, LibError> {
    let out = git(
        "get#GitBlob",
        &[
            ("repositoryId", Value::string(&hosted.repository_id)),
            ("revision", Value::string(revision)),
            ("path", Value::string(path)),
        ],
    )?;
    let flag = |key: &str| matches!(out.get(key), Some(Value::Bool(true)));
    Ok(Blob {
        text: match out.get("text") {
            Some(Value::String(body)) => Some(body.to_string()),
            _ => None,
        },
        binary: flag("binary"),
        too_large: flag("tooLarge"),
        size: match out.get("size") {
            Some(Value::I64(size)) => *size,
            _ => 0,
        },
    })
}

/// The file at the branch's head: its blob id and text, or None when there is no such file.
fn current(hosted: &HostedBranch, path: &str) -> Result<Option<(String, Blob)>, LibError> {
    match entry_at(hosted, &hosted.branch, path)? {
        Some((commit, entry)) => Ok(Some((item_text(&entry, "objectId"), blob(hosted, &commit, path)?))),
        None => Ok(None),
    }
}

/// Why the user may not commit to the branch, from its rules, or None.
fn protected_reason(hosted: &HostedBranch) -> Option<String> {
    let rules = git(
        "get#GitBranchRules",
        &[
            ("repositoryId", Value::string(&hosted.repository_id)),
            ("refName", Value::string(&hosted.branch)),
        ],
    )
    .ok()?;
    let direct = !matches!(rules.get("mayPushDirectly"), Some(Value::Bool(false)));
    (!direct).then(|| {
        format!(
            "{} of {} is protected: a Save is a direct commit, which its branch rules refuse as they refuse a push",
            hosted.branch, hosted.repository_id
        )
    })
}

/// `list#Directory` of a hosted root: the directory at the branch, folders first.
pub(crate) fn list_directory(hosted: &HostedBranch, dir: &str) -> Result<Parameters, LibError> {
    let dir = clean_path(dir)?;
    let found = listing(hosted, &hosted.branch, &dir)?;
    let entries: Vec<serde_json::Value> = found
        .entries
        .iter()
        .filter(|entry| item_text(entry, "type") != "submodule")
        .map(|entry| {
            json!({ "name": item_text(entry, "name"), "dir": item_text(entry, "type") == "dir", "status": "",
                "ignored": false, "modified": 0 })
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("entries".into(), json_value(&entries));
    Ok(out)
}

/// `get#File` of a hosted root: the file at the branch, its hash the blob id a Save commits over.
pub(crate) fn get_file(
    context: &Context,
    root: &Root,
    hosted: &HostedBranch,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let path = clean_path(&text(parameters, "path"))?;
    let mut out = Parameters::new();
    out.insert("path".into(), Value::string(&path));
    let mut read_only = context
        .may_write(root, &path)
        .err()
        .map(|err| err.to_string())
        .or_else(|| protected_reason(hosted));
    let Some((blob_id, content)) = current(hosted, &path)? else {
        if text(parameters, "missingOk") == "true" {
            out.insert("missing".into(), Value::Bool(true));
            out.insert("binary".into(), Value::Bool(false));
            out.insert("readOnly".into(), Value::Bool(read_only.is_some()));
            out.insert("readOnlyReason".into(), read_only.map_or(Value::Null, Value::string));
            return Ok(out);
        }
        return Err(LibError::new(format!(
            "{path} is not a file of {} at {}",
            hosted.repository_id, hosted.branch
        )));
    };
    out.insert("hash".into(), Value::string(&blob_id));
    out.insert("size".into(), Value::I64(content.size));
    match content.text {
        Some(body) if !content.too_large && (body.len() as u64) <= context.settings.max_edit_bytes => {
            let crlf = body.contains("\r\n");
            let (body, bom) = match body.strip_prefix('\u{feff}') {
                Some(rest) => (rest.to_string(), true),
                None => (body, false),
            };
            out.insert("lineEnding".into(), Value::string(if crlf { "CRLF" } else { "LF" }));
            out.insert("bom".into(), Value::Bool(bom));
            out.insert("text".into(), Value::string(body));
            out.insert("binary".into(), Value::Bool(false));
        }
        _ if content.binary => {
            read_only = Some("a binary file".into());
            out.insert("binary".into(), Value::Bool(true));
        }
        _ => {
            read_only = Some(format!(
                "{path} is over {} bytes, so it opens read only",
                context.settings.max_edit_bytes
            ));
            out.insert("binary".into(), Value::Bool(false));
        }
    }
    out.insert("status".into(), Value::string(""));
    out.insert("readOnly".into(), Value::Bool(read_only.is_some()));
    out.insert("readOnlyReason".into(), read_only.map_or(Value::Null, Value::string));
    out.insert("component".into(), Value::Null);
    Ok(out)
}

/// The commit message of a Save: the one the user gave, else "Edit <path>", with the root's current task as a trailer,
/// as a commit of the Changes tab has.
fn message_of(root: &Root, path: &str, parameters: &Parameters) -> String {
    let given = text(parameters, "message");
    let message = if given.trim().is_empty() {
        format!("Edit {path}")
    } else {
        given
    };
    let task = Some(text(parameters, "task"))
        .filter(|task| !task.is_empty())
        .or_else(|| {
            moqui_core::user::get_preference(&format!("{}.{}", crate::vcs::CURRENT_TASK, root.id))
                .ok()
                .flatten()
        });
    crate::git::with_trailer(&message, task.as_deref())
}

/// `save#File` of a hosted root: a commit of the file to the branch by the user. Refused by the hosting when the user
/// may not update the repository or the branch's rules refuse a direct commit; answered as a conflict, with the text at
/// the branch, when the file's blob there is no longer `baseHash` (the branch moved past it since it was opened).
pub(crate) fn save_file(
    context: &Context,
    root: &Root,
    hosted: &HostedBranch,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let path = clean_path(&text(parameters, "path"))?;
    if path.is_empty() {
        return Err(LibError::new("no file to save"));
    }
    context.may_write(root, &path)?;
    let content = text(parameters, "text");
    if content.len() as u64 > context.settings.max_edit_bytes {
        return Err(LibError::new(format!(
            "the text is over {} bytes",
            context.settings.max_edit_bytes
        )));
    }
    unfenced(
        moqui_core::entity::fence::Effect::Resource,
        &format!("git://{}/{path}", hosted.repository_id),
    )?;
    let base_hash = text(parameters, "baseHash");
    // the line endings and byte order mark the file has at the branch, kept as a save on disk keeps them
    let (crlf, bom) = match current(hosted, &path)? {
        Some((_, Blob { text: Some(body), .. })) => (body.contains("\r\n"), body.starts_with('\u{feff}')),
        _ => (false, false),
    };
    let normalized = content.replace("\r\n", "\n");
    let mut body = if crlf {
        normalized.replace('\n', "\r\n")
    } else {
        normalized
    };
    if bom {
        body.insert(0, '\u{feff}');
    }
    let one = |value: Value| -> Value { Value::Map(std::iter::once((path.clone(), value)).collect::<ValueMap>()) };
    // a repository with no commit yet has no branch to name: its first commit makes the default one, the root's
    let empty = listing(hosted, &hosted.branch, "")?.commit.is_empty();
    let committed = git(
        "commit#GitFiles",
        &[
            ("repositoryId", Value::string(&hosted.repository_id)),
            (
                "branch",
                if empty { Value::Null } else { Value::string(&hosted.branch) },
            ),
            ("message", Value::string(message_of(root, &path, parameters))),
            ("fileMap", one(Value::string(&body))),
            ("expectedBlobIds", one(Value::string(&base_hash))),
        ],
    )
    .map_err(|err| {
        let text = err.to_string();
        if text.contains("GitProtectedBranch") {
            LibError::new(format!(
                "{} of {} is protected: a Save is a direct commit, which its branch rules refuse as they refuse a push ({text})",
                hosted.branch, hosted.repository_id
            ))
        } else {
            err
        }
    })?;
    let mut out = Parameters::new();
    if matches!(committed.get("conflict"), Some(Value::Bool(true))) {
        let now = current(hosted, &path)?;
        out.insert("conflict".into(), Value::Bool(true));
        out.insert(
            "diskHash".into(),
            Value::string(now.as_ref().map(|(id, _)| id.clone()).unwrap_or_default()),
        );
        out.insert(
            "diskText".into(),
            now.and_then(|(_, content)| content.text)
                .map_or(Value::Null, |body| Value::string(body.trim_start_matches('\u{feff}'))),
        );
        return Ok(out);
    }
    let commit = out_text(&committed, "commitId");
    let hash = if commit.is_empty() {
        base_hash.clone()
    } else {
        entry_at(hosted, &commit, &path)?
            .map(|(_, entry)| item_text(&entry, "objectId"))
            .unwrap_or_default()
    };
    audit::record(
        &context.facade,
        &Write {
            action: if base_hash.is_empty() { "DAA_CREATE" } else { "DAA_SAVE" },
            root: &root.id,
            path: &path,
            hash_before: Some(&base_hash).filter(|hash| !hash.is_empty()).map(String::as_str),
            hash_after: Some(&hash),
            detail: Some(json!({ "repositoryId": hosted.repository_id, "branch": hosted.branch, "commitId": commit })),
        },
    );
    let problems = if path.ends_with(".xml") {
        moqui_devkit::check::check_text(&context.factory, Path::new(&path), &content)
    } else {
        Vec::new()
    };
    out.insert("conflict".into(), Value::Bool(false));
    out.insert("hash".into(), Value::string(&hash));
    out.insert("commitId".into(), Value::string(&commit));
    out.insert("status".into(), Value::string(""));
    out.insert("restart".into(), Value::Bool(false));
    out.insert("rebuild".into(), Value::Bool(false));
    out.insert("problems".into(), json_value(&problems));
    Ok(out)
}

/// `check#File` of a hosted root: the problems of the text, as a save finds them.
pub(crate) fn check_file(context: &Context, parameters: &Parameters) -> Result<Parameters, LibError> {
    let path = clean_path(&text(parameters, "path"))?;
    let problems = if path.ends_with(".xml") {
        moqui_devkit::check::check_text(&context.factory, Path::new(&path), &text(parameters, "text"))
    } else {
        Vec::new()
    };
    let mut out = Parameters::new();
    out.insert("problems".into(), json_value(&problems));
    Ok(out)
}

/// The files of the branch, each with its blob's size, at one commit: a walk of its directories, at most
/// [`MAX_DIRECTORIES`] of them and `limit` files; a directory that cannot be read is logged and passed over.
fn files(hosted: &HostedBranch, limit: usize) -> (String, Vec<(String, i64)>) {
    let mut commit = String::new();
    let mut out = Vec::new();
    let mut pending = vec![String::new()];
    let mut read = 0_usize;
    while let Some(dir) = pending.pop() {
        if read >= MAX_DIRECTORIES || out.len() >= limit {
            break;
        }
        read += 1;
        let revision = if commit.is_empty() {
            hosted.branch.clone()
        } else {
            commit.clone()
        };
        let found = match listing(hosted, &revision, &dir) {
            Ok(found) => found,
            Err(err) => {
                tracing::warn!("Hosted root {} {dir}: {err}", hosted.repository_id);
                continue;
            }
        };
        if commit.is_empty() {
            commit = found.commit;
        }
        for entry in &found.entries {
            let name = item_text(entry, "name");
            let path = if dir.is_empty() { name } else { format!("{dir}/{name}") };
            match item_text(entry, "type").as_str() {
                "dir" => pending.push(path),
                "file" | "executable" => {
                    let size = match entry {
                        Value::Map(map) => match map.get("size") {
                            Some(Value::I64(size)) => *size,
                            _ => 0,
                        },
                        _ => 0,
                    };
                    out.push((path, size));
                }
                _ => {}
            }
        }
    }
    out.sort();
    out.truncate(limit);
    (commit, out)
}

/// `find#Files` of a hosted root: every file of the branch, for Go to file.
pub(crate) fn find_files(hosted: &HostedBranch) -> Parameters {
    let (_, found) = files(hosted, MAX_FILES);
    let paths: Vec<String> = found.into_iter().map(|(path, _)| path).collect();
    let mut out = Parameters::new();
    out.insert("files".into(), json_value(&paths));
    out
}

/// `search#Files` of a hosted root: the lines matching `matcher` in the text files of the branch, at most `limit`,
/// each file read at the one commit the walk read; a file that cannot be read is passed over.
pub(crate) fn search_files(
    context: &Context,
    hosted: &HostedBranch,
    matcher: &regex::Regex,
    filter: &str,
    limit: usize,
) -> Vec<serde_json::Value> {
    let (commit, found) = files(hosted, MAX_SEARCHED);
    let mut results = Vec::new();
    for (path, size) in found {
        if !filter.is_empty() && !path.contains(filter.trim_matches('*')) {
            continue;
        }
        if u64::try_from(size).unwrap_or(0) > context.settings.max_edit_bytes {
            continue;
        }
        let Ok(Blob { text: Some(body), .. }) = blob(hosted, &commit, &path) else {
            continue;
        };
        for (index, line) in body.lines().enumerate() {
            if matcher.is_match(line) {
                results.push(json!({ "path": path, "line": index + 1, "text": line }));
                if results.len() >= limit {
                    return results;
                }
            }
        }
    }
    results
}

/// A repository the user may view: its id and default branch, and what they may do in it.
fn access(repository_id: &str) -> Result<(bool, bool), LibError> {
    let out = git(
        "get#GitRepositoryAccess",
        &[("repositoryId", Value::string(repository_id))],
    )?;
    let flag = |key: &str| matches!(out.get(key), Some(Value::Bool(true)));
    Ok((flag("canUpdate"), flag("archived")))
}

/// The branches of a repository, the default first.
fn branches(repository_id: &str) -> Result<(String, Vec<String>), LibError> {
    let out = git(
        "get#GitRefs",
        &[
            ("repositoryId", Value::string(repository_id)),
            ("refType", Value::string("branch")),
        ],
    )?;
    let default = out_text(&out, "defaultBranch");
    let mut names: Vec<String> = out_list(&out, "refList")
        .iter()
        .map(|item| item_text(item, "shortName"))
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_by_key(|name| (name != &default, name.clone()));
    Ok((default, names))
}

/// `get#HostedRepositories`: the repositories of this server's hosting the user may update, not archived, each with
/// its branches, for Repositories › Add hosted root. A repository that cannot be read is logged and left out.
fn get_hosted_repositories(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let _context = Context::of(ec, Tier::Extend)?;
    let found = git(
        "find#GitRepositories",
        &[("pageSize", Value::I64(200)), ("pageIndex", Value::I64(0))],
    )?;
    let mut repositories = Vec::new();
    for item in out_list(&found, "repositoryList") {
        let id = item_text(item, "repositoryId");
        let listed = access(&id).and_then(|(update, archived)| {
            if !update || archived {
                return Ok(None);
            }
            let (default, names) = branches(&id)?;
            Ok(Some(
                json!({ "repositoryId": id, "description": item_text(item, "description"),
                "repositoryKind": item_text(item, "repositoryKind"),
                "defaultBranch": if default.is_empty() { item_text(item, "branch") } else { default },
                "branches": names }),
            ))
        });
        match listed {
            Ok(Some(row)) => repositories.push(row),
            Ok(None) => {}
            Err(err) => tracing::warn!("Hosted repositories: {id} is left out: {err}"),
        }
    }
    let mut out = Parameters::new();
    out.insert("repositories".into(), json_value(&repositories));
    Ok(out)
}

/// `get#HostedRoot`: what the Workspace says of a hosted root: its repository and branch, whether the user may commit
/// to it, and why not.
fn get_hosted_root(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let hosted = root
        .hosted_branch
        .as_ref()
        .ok_or_else(|| LibError::new(format!("{} is not a hosted root", root.label)))?;
    let (update, archived) = access(&hosted.repository_id)?;
    let refusal = if archived {
        Some(format!("{} is archived", hosted.repository_id))
    } else if !update {
        Some(format!("You may not update {}", hosted.repository_id))
    } else {
        protected_reason(hosted)
    };
    let mut out = Parameters::new();
    out.insert("repositoryId".into(), Value::string(&hosted.repository_id));
    out.insert("branch".into(), Value::string(&hosted.branch));
    out.insert("canCommit".into(), Value::Bool(refusal.is_none()));
    out.insert("refusal".into(), refusal.map_or(Value::Null, Value::string));
    Ok(out)
}

/// `store#HostedRoot`: a hosted root of a repository the user may update at one of its branches (the default branch
/// when none is named), or its label. A root, like the others, every Dev AIDE user sees; what each reads and commits
/// there is the hosting's to decide.
fn store_hosted_root(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let repository_id = text(parameters, "repositoryId").trim().to_string();
    if repository_id.is_empty() {
        return Err(LibError::new("Add hosted root needs a repository"));
    }
    let (update, archived) = access(&repository_id)?;
    if !update || archived {
        return Err(LibError::new(format!(
            "You may not update {repository_id}{}",
            if archived { ": it is archived" } else { "" }
        )));
    }
    let (default, names) = branches(&repository_id)?;
    let branch = Some(text(parameters, "branch").trim().to_string())
        .filter(|branch| !branch.is_empty())
        .unwrap_or(default);
    // an empty repository has no branch yet: its default one is made by the first commit
    if branch.is_empty() || (!names.is_empty() && !names.contains(&branch)) {
        return Err(LibError::new(format!("{repository_id} has no branch {branch}")));
    }
    let _authz = ec.disable_authz();
    let location = location_of(&repository_id, &branch);
    let mut row = match roots::root_row(&context.facade, Path::new(&location)) {
        Some(row) => row,
        None => {
            let mut row = context.facade.make_value(roots::ROOT_ENTITY)?;
            row.set_sequenced_id_primary(&context.facade)?;
            row.set("location", Value::string(&location));
            row.set("rootKind", Value::string(KIND));
            row.set("gitRepositoryId", Value::string(&repository_id));
            row.set("gitBranch", Value::string(&branch));
            let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
            row.set("hostedByUserId", Value::string(user));
            row
        }
    };
    if parameters.contains_key("label") {
        row.set("label", Value::string(text(parameters, "label").trim()));
    }
    row.create_or_update(&context.facade)?;
    let mut out = Parameters::new();
    out.insert("rootId".into(), Value::string(id_of(&repository_id, &branch)));
    Ok(out)
}

/// `delete#HostedRoot`: a hosted root taken out of the Workspace, by who added it or the full tier; the repository
/// stays as it is.
fn delete_hosted_root(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let hosted = root
        .hosted_branch
        .as_ref()
        .ok_or_else(|| LibError::new(format!("{} is not a hosted root", root.label)))?;
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    if context.tier != Tier::Full && hosted.hosted_by.as_deref() != Some(user.as_str()) {
        return Err(LibError::new(format!(
            "{} was added by another user: they or the full tier remove it",
            root.label
        )));
    }
    let _authz = ec.disable_authz();
    if let Some(row) = roots::root_row(&context.facade, &root.path) {
        row.delete(&context.facade)?;
    }
    Ok(Parameters::new())
}

#[cfg(test)]
#[path = "hosted_tests.rs"]
mod tests;
