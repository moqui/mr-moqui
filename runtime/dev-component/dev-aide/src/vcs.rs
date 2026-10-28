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

//! `org.moqui.devaide.VcsServices`: the git of a root (plans/design-dev-aide.md 4.5 and 4.6): status, diffs and the log;
//! stage, unstage and discard by file; commit with the current task's trailer, the user as author and the server's git
//! identity as committer; fetch, pull (fast-forward only) and push; switch and create a branch; and Publish (D18): the
//! site component's changes staged, committed and pushed, to the publish branch when one is set (question 15).
//! Network commands answer git's own message on failure; in shared mode they are off until an admin enables them.

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::audit::{self, Write};
use crate::git::{self, Mode};
use crate::roots::{Root, RootKind};
use crate::tier::Tier;
use crate::workspace::{flag, json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_status), get_status)?;
    registry.service_function(moqui_core::service_location!(get_diff), get_diff)?;
    registry.service_function(moqui_core::service_location!(get_log), get_log)?;
    registry.service_function(moqui_core::service_location!(get_commit), get_commit)?;
    registry.service_function(moqui_core::service_location!(stage_files), stage_files)?;
    registry.service_function(moqui_core::service_location!(unstage_files), unstage_files)?;
    registry.service_function(moqui_core::service_location!(discard_files), discard_files)?;
    registry.service_function(moqui_core::service_location!(commit_changes), commit_changes)?;
    registry.service_function(moqui_core::service_location!(fetch_remote), fetch_remote)?;
    registry.service_function(moqui_core::service_location!(pull_remote), pull_remote)?;
    registry.service_function(moqui_core::service_location!(push_remote), push_remote)?;
    registry.service_function(moqui_core::service_location!(switch_branch), switch_branch)?;
    registry.service_function(moqui_core::service_location!(create_branch), create_branch)?;
    registry.service_function(moqui_core::service_location!(init_repository), init_repository)?;
    registry.service_function(moqui_core::service_location!(publish_site), publish_site)
}

/// The preference with a root's current task (`DEV_AIDE_CURRENT_TASK.<rootId>`), whose trailer a commit gets.
pub const CURRENT_TASK: &str = "DEV_AIDE_CURRENT_TASK";
/// The publish branch an admin set for a user (question 15), a name or a pattern with `{user}`.
pub const PUBLISH_BRANCH: &str = "DEV_AIDE_PUBLISH_BRANCH";

fn git_root<'a>(context: &'a Context, parameters: &Parameters) -> Result<&'a Root, LibError> {
    let root = context.root(&text(parameters, "rootId"))?;
    if root.kind != RootKind::Git {
        return Err(LibError::new(format!("{} is not a git repository", root.label)));
    }
    Ok(root)
}

/// The paths of a `paths` parameter (a list, or comma-separated text), each confined to the root.
fn paths(context: &Context, root: &Root, parameters: &Parameters) -> Result<Vec<String>, LibError> {
    let listed: Vec<String> = match parameters.get("paths") {
        Some(Value::List(items)) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        Some(other) => moqui_lib::object::to_plain_string(other)
            .split(',')
            .map(|path| path.trim().to_string())
            .collect(),
        None => Vec::new(),
    };
    let mut out = Vec::new();
    for path in listed.into_iter().filter(|path| !path.is_empty()) {
        out.push(context.resolve(root, &path)?.relative);
    }
    if out.is_empty() {
        return Err(LibError::new("no paths"));
    }
    Ok(out)
}

/// The author of a commit: the logged-in user's name and email (D19: a shared server names who made each commit).
fn author_env(context: &Context) -> Vec<(&'static str, String)> {
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let row = context
        .facade
        .find("moqui.security.UserAccount")
        .and_then(|find| find.eq("userId", Value::string(&user)).one())
        .ok()
        .flatten();
    let field = |name: &str| {
        row.as_ref()
            .and_then(|row| row.get(name))
            .map(moqui_lib::object::to_plain_string)
            .filter(|value| !value.is_empty())
    };
    let mut env = Vec::new();
    let name = field("userFullName").or_else(|| field("username"));
    if let Some(name) = name {
        env.push(("GIT_AUTHOR_NAME", name));
    }
    if let Some(email) = field("emailAddress") {
        env.push(("GIT_AUTHOR_EMAIL", email));
    }
    env
}

fn network_allowed(context: &Context) -> Result<(), LibError> {
    if context.settings.shared && !context.settings.network_git {
        return Err(LibError::new(
            "Network git commands are off on this shared dev server until an admin enables them in Dev AIDE Settings",
        ));
    }
    Ok(())
}

fn audit_git(context: &Context, root: &Root, command: &str, detail: serde_json::Value) {
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_GIT",
            root: &root.id,
            path: "",
            detail: Some(serde_json::json!({ "command": command, "detail": detail })),
            ..Write::default()
        },
    );
}

/// `get#Status`: the branch, upstream, ahead and behind, the changed files grouped as the Changes tab shows them, the
/// branches and remotes, and the author and committer identities.
fn get_status(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let git = context.git();
    let status = git::parse_status(&git.read(
        &root.path,
        &["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"],
    )?);
    let branches = git::parse_branches(
        &git.read(&root.path, &["for-each-ref", git::BRANCH_FORMAT, "refs/heads"])
            .unwrap_or_default(),
    );
    let remotes = git::parse_remotes(&git.read(&root.path, &["remote", "-v"]).unwrap_or_default());
    let config = |key: &str| {
        git.read(&root.path, &["config", "--get", key])
            .map(|value| value.trim().to_string())
            .unwrap_or_default()
    };
    let entries: Vec<serde_json::Value> = status
        .entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "path": entry.path, "original": entry.original, "letter": entry.letter().to_string(),
                "index": entry.index.to_string(), "worktree": entry.worktree.to_string(),
                "staged": entry.staged(), "unstaged": entry.unstaged(), "conflicted": entry.conflicted,
                "untracked": entry.index == '?',
                "site": root.is_site_path(&entry.path),
            })
        })
        .collect();
    let author: std::collections::BTreeMap<&str, String> = author_env(&context).into_iter().collect();
    let task = moqui_core::user::get_preference(&format!("{CURRENT_TASK}.{}", root.id))
        .ok()
        .flatten();
    let mut out = Parameters::new();
    out.insert("branch".into(), status.branch.map_or(Value::Null, Value::string));
    out.insert("upstream".into(), status.upstream.map_or(Value::Null, Value::string));
    out.insert("ahead".into(), Value::I64(i64::from(status.ahead)));
    out.insert("behind".into(), Value::I64(i64::from(status.behind)));
    out.insert("entries".into(), json_value(&entries));
    out.insert("branches".into(), json_value(&branches));
    out.insert(
        "remotes".into(),
        json_value(
            &remotes
                .iter()
                .map(|(name, url)| serde_json::json!({"name": name, "url": url}))
                .collect::<Vec<_>>(),
        ),
    );
    out.insert(
        "authorName".into(),
        Value::string(
            author
                .get("GIT_AUTHOR_NAME")
                .cloned()
                .unwrap_or_else(|| config("user.name")),
        ),
    );
    out.insert(
        "authorEmail".into(),
        Value::string(
            author
                .get("GIT_AUTHOR_EMAIL")
                .cloned()
                .unwrap_or_else(|| config("user.email")),
        ),
    );
    out.insert("committerName".into(), Value::string(config("user.name")));
    out.insert("committerEmail".into(), Value::string(config("user.email")));
    out.insert("currentTask".into(), task.map_or(Value::Null, Value::string));
    Ok(out)
}

/// The text of `spec` (`HEAD:path`, `:path` for the index), empty when it is not there.
fn show(context: &Context, root: &Root, spec: &str) -> String {
    context.git().read(&root.path, &["show", spec]).unwrap_or_default()
}

/// `get#Diff`: the two sides of a file's change: the work tree against the index, the index against HEAD with
/// `staged`, or a commit against its parent with `commitId`.
fn get_diff(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    let path = &resolved.relative;
    let commit = text(parameters, "commitId");
    let (original, modified) = if !commit.is_empty() {
        if !commit.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(LibError::new("a commit id is hexadecimal"));
        }
        (
            show(&context, root, &format!("{commit}^:{path}")),
            show(&context, root, &format!("{commit}:{path}")),
        )
    } else if flag(parameters, "staged") {
        (
            show(&context, root, &format!("HEAD:{path}")),
            show(&context, root, &format!(":{path}")),
        )
    } else {
        let index = context
            .git()
            .run(&root.path, Mode::Read, &["show", &format!(":{path}")], &[], None)?;
        let original = if index.success() {
            index.stdout
        } else {
            show(&context, root, &format!("HEAD:{path}"))
        };
        (original, std::fs::read_to_string(&resolved.path).unwrap_or_default())
    };
    let mut out = Parameters::new();
    out.insert("original".into(), Value::string(original));
    out.insert("modified".into(), Value::string(modified));
    Ok(out)
}

/// `get#Log`: a page of the branch's commits, or a path's.
fn get_log(ec: &ExecutionContext, _service: &ServiceInfo<'_>, parameters: &Parameters) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let size: usize = text(parameters, "pageSize").parse().unwrap_or(50).clamp(1, 500);
    let index: usize = text(parameters, "pageIndex").parse().unwrap_or(0);
    let skip = (index * size).to_string();
    let count = size.to_string();
    let mut args = vec!["log", git::LOG_FORMAT, "--skip", &skip, "-n", &count];
    let path = text(parameters, "path");
    let resolved;
    if !path.is_empty() {
        resolved = context.resolve(root, &path)?.relative;
        args.extend(["--", resolved.as_str()]);
    }
    // a repository with no commit yet has no log
    let output = context.git().run(&root.path, Mode::Read, &args, &[], None)?;
    let commits = if output.success() {
        git::parse_log(&output.stdout)
    } else {
        Vec::new()
    };
    let mut out = Parameters::new();
    out.insert("commits".into(), json_value(&commits));
    Ok(out)
}

/// `get#Commit`: a commit's message and the files it changed.
fn get_commit(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let id = text(parameters, "commitId");
    if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(LibError::new("a commit id is hexadecimal"));
    }
    let git = context.git();
    let commit = git::parse_log(&git.read(&root.path, &["show", "-s", git::LOG_FORMAT, &id])?);
    let body = git
        .read(&root.path, &["show", "-s", "--format=%B", &id])
        .unwrap_or_default();
    let files: Vec<serde_json::Value> = git
        .read(&root.path, &["show", "--format=", "--name-status", "-z", &id])
        .unwrap_or_default()
        .split('\0')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .chunks(2)
        .filter(|pair| pair.len() == 2)
        .map(|pair| serde_json::json!({ "status": &pair[0][..1], "path": pair[1] }))
        .collect();
    let mut out = Parameters::new();
    out.insert("commit".into(), commit.first().map_or(Value::Null, json_value));
    out.insert("message".into(), Value::string(body.trim_end()));
    out.insert("files".into(), json_value(&files));
    Ok(out)
}

/// Run a write command over paths and answer git's output.
fn write_paths(
    context: &Context,
    root: &Root,
    args: &[&str],
    paths: &[String],
    what: &str,
) -> Result<Parameters, LibError> {
    let mut argv: Vec<&str> = args.to_vec();
    argv.push("--");
    argv.extend(paths.iter().map(String::as_str));
    let output = context
        .git()
        .run(&root.path, Mode::Write, &argv, &[], None)?
        .checked(what)?;
    audit_git(context, root, what, serde_json::json!(paths));
    let mut out = Parameters::new();
    out.insert(
        "output".into(),
        Value::string(format!("{}{}", output.stdout, output.stderr)),
    );
    Ok(out)
}

/// `stage#Files`.
fn stage_files(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let paths = paths(&context, root, parameters)?;
    write_paths(&context, root, &["add", "-A"], &paths, "add")
}

/// `unstage#Files`.
fn unstage_files(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let paths = paths(&context, root, parameters)?;
    let has_head = context
        .git()
        .run(
            &root.path,
            Mode::Read,
            &["rev-parse", "--verify", "-q", "HEAD"],
            &[],
            None,
        )?
        .success();
    if has_head {
        write_paths(&context, root, &["restore", "--staged"], &paths, "restore --staged")
    } else {
        write_paths(&context, root, &["rm", "--cached", "-r", "-q"], &paths, "rm --cached")
    }
}

/// `discard#Files`: the work tree's changes of tracked files thrown away, and untracked files deleted; each must be
/// writable by the user.
fn discard_files(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let paths = paths(&context, root, parameters)?;
    for path in &paths {
        context.may_write(root, path)?;
    }
    let status = git::parse_status(&context.git().read(
        &root.path,
        &["status", "--porcelain=v2", "-z", "--untracked-files=all", "--", "."],
    )?);
    let (untracked, tracked): (Vec<String>, Vec<String>) = paths.into_iter().partition(|path| {
        status
            .entries
            .iter()
            .any(|entry| entry.path == *path && entry.index == '?')
    });
    for path in &untracked {
        let resolved = context.resolve(root, path)?;
        std::fs::remove_file(&resolved.path).map_err(|err| LibError::new(format!("{path}: {err}")))?;
    }
    let mut out = if tracked.is_empty() {
        Parameters::new()
    } else {
        write_paths(&context, root, &["restore", "--worktree"], &tracked, "restore")?
    };
    let changed: Vec<std::path::PathBuf> = untracked
        .iter()
        .chain(tracked.iter())
        .map(|path| root.path.join(path))
        .collect();
    let report = moqui_core::devtools::reload_paths(&context.factory, &changed);
    out.insert("reload".into(), json_value(&report));
    Ok(out)
}

/// The message of a commit: a summary and a body, with the root's current task as a trailer.
fn message_of(root: &Root, parameters: &Parameters) -> Result<String, LibError> {
    let summary = text(parameters, "summary");
    if summary.trim().is_empty() {
        return Err(LibError::new("a commit needs a summary"));
    }
    let body = text(parameters, "body");
    let message = if body.trim().is_empty() {
        summary
    } else {
        format!("{}\n\n{}", summary.trim_end(), body.trim_end())
    };
    let task = Some(text(parameters, "task"))
        .filter(|task| !task.is_empty())
        .or_else(|| {
            moqui_core::user::get_preference(&format!("{CURRENT_TASK}.{}", root.id))
                .ok()
                .flatten()
        });
    Ok(git::with_trailer(&message, task.as_deref()))
}

/// Commit what is staged, with the user as author; the hooks run unless `skipHooks`. Answers the new commit and git's
/// output (the hooks' too).
fn commit(
    context: &Context,
    root: &Root,
    message: &str,
    parameters: &Parameters,
) -> Result<(String, String), LibError> {
    let mut args = vec!["commit", "-F", "-"];
    if flag(parameters, "amend") {
        args.push("--amend");
    }
    if flag(parameters, "signOff") {
        args.push("--signoff");
    }
    if flag(parameters, "skipHooks") {
        args.push("--no-verify");
    }
    let env = author_env(context);
    let output = context.git().run(&root.path, Mode::Write, &args, &env, Some(message))?;
    let shown = format!("{}{}", output.stdout, output.stderr);
    if !output.success() {
        return Err(LibError::new(format!("git commit: {}", shown.trim())));
    }
    let id = context
        .git()
        .read(&root.path, &["rev-parse", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string();
    audit_git(context, root, "commit", serde_json::json!({ "commit": id }));
    Ok((id, shown))
}

/// `commit#Changes`.
fn commit_changes(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    if context.tier == Tier::Extend && root.upstream {
        return Err(LibError::new(format!(
            "{} is upstream; the extend tier commits only site components",
            root.label
        )));
    }
    let message = message_of(root, parameters)?;
    let (id, output) = commit(&context, root, &message, parameters)?;
    let mut out = Parameters::new();
    out.insert("commitId".into(), Value::string(id));
    out.insert("output".into(), Value::string(output));
    Ok(out)
}

/// The git arguments of a network command, from the context and the root.
type Argv = dyn Fn(&Context, &Root) -> Result<Vec<String>, LibError>;

fn network(ec: &ExecutionContext, parameters: &Parameters, build: &Argv, what: &str) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    network_allowed(&context)?;
    if context.tier == Tier::Extend && root.upstream && what != "fetch" {
        return Err(LibError::new(format!(
            "{} is upstream; the extend tier publishes only site components",
            root.label
        )));
    }
    let args = build(&context, root)?;
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = context.git().run(&root.path, Mode::Network, &argv, &[], None)?;
    let shown = format!("{}{}", output.stdout, output.stderr);
    audit_git(&context, root, what, serde_json::json!({ "ok": output.success() }));
    if !output.success() {
        return Err(LibError::new(format!(
            "git {what}: {} (the server's git uses its own credential helper and SSH agent; nothing prompts, so a \
             remote that asks for a password fails: set up a credential helper or an SSH key, or run it in a terminal)",
            shown.trim()
        )));
    }
    let mut out = Parameters::new();
    out.insert("output".into(), Value::string(shown));
    // what a pull or a switch changes on disk, the file watcher reloads
    Ok(out)
}

/// `fetch#Remote`.
fn fetch_remote(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    network(
        ec,
        parameters,
        &|_, _| Ok(vec!["fetch".into(), "--prune".into()]),
        "fetch",
    )
}

/// `pull#Remote`: fast-forward only.
///
/// The files the pull changed are reloaded at once (the watcher then passes over them), which keeps the artifact index
/// current, and the answer names the site's extensions whose targets the pull removed (`extensionsAffected`, the
/// upgrade checklist of plans/design-dev-aide.md 4.7).
fn pull_remote(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let head = |context: &Context, root: &Root| {
        context
            .git()
            .read(&root.path, &["rev-parse", "-q", "--verify", "HEAD"])
            .map(|out| out.trim().to_string())
            .unwrap_or_default()
    };
    let before = {
        let context = Context::of(ec, Tier::Extend)?;
        let root = git_root(&context, parameters)?;
        head(&context, root)
    };
    let mut out = network(
        ec,
        parameters,
        &|_, _| Ok(vec!["pull".into(), "--ff-only".into()]),
        "pull",
    )?;
    let context = Context::of(ec, Tier::Extend)?;
    let root = git_root(&context, parameters)?;
    let after = head(&context, root);
    if before.is_empty() || before == after {
        return Ok(out);
    }
    let changed: Vec<std::path::PathBuf> = context
        .git()
        .read(&root.path, &["diff", "--name-only", "-z", &before, &after])
        .unwrap_or_default()
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|path| root.path.join(path))
        .collect();
    let report = moqui_core::devtools::reload_paths(&context.factory, &changed);
    out.insert("reload".into(), json_value(&report));
    match crate::artifacts::customizations(&context) {
        Ok((extensions, broken)) => {
            let affected: Vec<&serde_json::Value> = extensions
                .iter()
                .filter(|extension| extension["state"] == "broken")
                .collect();
            out.insert(
                "extensionsAffected".into(),
                Value::I64(i64::try_from(broken).unwrap_or(0)),
            );
            out.insert("brokenExtensions".into(), json_value(&affected));
        }
        Err(err) => tracing::error!("Dev AIDE: the extensions were not checked after a pull: {err}"),
    }
    Ok(out)
}

/// The current branch, which a push needs.
fn current_branch(context: &Context, root: &Root) -> Result<String, LibError> {
    let branch = context
        .git()
        .read(&root.path, &["symbolic-ref", "--short", "-q", "HEAD"])
        .unwrap_or_default();
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(LibError::new("HEAD is detached; switch to a branch first"));
    }
    Ok(branch.to_string())
}

/// The remote a push goes to: the branch's, else origin, else the only one.
fn push_remote_name(context: &Context, root: &Root, branch: &str) -> Result<String, LibError> {
    let configured = context
        .git()
        .read(&root.path, &["config", "--get", &format!("branch.{branch}.remote")])
        .unwrap_or_default();
    if !configured.trim().is_empty() {
        return Ok(configured.trim().to_string());
    }
    let remotes = git::parse_remotes(&context.git().read(&root.path, &["remote", "-v"]).unwrap_or_default());
    remotes
        .iter()
        .find(|(name, _)| name == "origin")
        .or_else(|| remotes.first())
        .map(|(name, _)| name.clone())
        .ok_or_else(|| LibError::new("the repository has no remote to push to (Settings: add one)"))
}

/// `push#Remote`: the current branch to its upstream, set on the first push.
fn push_remote(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    network(
        ec,
        parameters,
        &|context, root| {
            if root.publish_branch_required {
                return Err(LibError::new(format!(
                    "{} publishes to its publish branch: use Publish",
                    root.label
                )));
            }
            let branch = current_branch(context, root)?;
            let remote = push_remote_name(context, root, &branch)?;
            Ok(vec!["push".into(), "-u".into(), remote, branch])
        },
        "push",
    )
}

/// `switch#Branch`: refused with git's message when local changes conflict.
fn switch_branch(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let name = text(parameters, "branch");
    valid_branch(&name)?;
    let output = context
        .git()
        .run(&root.path, Mode::Write, &["switch", &name], &[], None)?
        .checked("switch")?;
    audit_git(&context, root, "switch", serde_json::json!({ "branch": name }));
    let mut out = Parameters::new();
    out.insert(
        "output".into(),
        Value::string(format!("{}{}", output.stdout, output.stderr)),
    );
    // the file watcher reloads what changed; a changed entity or conf file still takes a restart
    out.insert("restart".into(), Value::Bool(true));
    Ok(out)
}

/// `create#Branch`: a branch from the current commit, switched to unless `switch` is false.
fn create_branch(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let name = text(parameters, "branch");
    valid_branch(&name)?;
    let switch = text(parameters, "switch") != "false";
    let args: Vec<&str> = if switch {
        vec!["switch", "-c", &name]
    } else {
        vec!["branch", &name]
    };
    let output = context
        .git()
        .run(&root.path, Mode::Write, &args, &[], None)?
        .checked("branch")?;
    audit_git(&context, root, "branch", serde_json::json!({ "branch": name }));
    let mut out = Parameters::new();
    out.insert(
        "output".into(),
        Value::string(format!("{}{}", output.stdout, output.stderr)),
    );
    Ok(out)
}

fn valid_branch(name: &str) -> Result<(), LibError> {
    let valid = !name.is_empty()
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.ends_with('/')
        && !name.ends_with(".lock")
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.'));
    if valid {
        Ok(())
    } else {
        Err(LibError::new(format!(
            "{name} is not a branch name this takes (letters, digits, - _ / .)"
        )))
    }
}

/// `init#Repository`: `git init` in a plain root (Settings, with a confirm).
fn init_repository(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let root = context.root(&text(parameters, "rootId"))?;
    if root.kind == RootKind::Git {
        return Err(LibError::new(format!("{} is a git repository already", root.label)));
    }
    context
        .git()
        .run(&root.path, Mode::Write, &["init", "-q"], &[], None)?
        .checked("init")?;
    audit_git(&context, root, "init", serde_json::Value::Null);
    Ok(Parameters::new())
}

/// The publish branch of a root for the current user: the root's in Settings, else the one an admin set for the user,
/// `{user}` the username. `None` pushes to the current branch.
fn publish_branch(root: &Root) -> Option<String> {
    let pattern = root
        .publish_branch
        .clone()
        .or_else(|| moqui_core::user::get_preference(PUBLISH_BRANCH).ok().flatten())
        .filter(|pattern| !pattern.trim().is_empty())?;
    let user = moqui_core::user::current_username()
        .ok()
        .flatten()
        .unwrap_or_else(|| "user".into());
    Some(pattern.replace("{user}", &user))
}

/// `publish#Site`: the site component's changes in this root staged, committed with `summary` (or "Customize: <files>")
/// and the task trailer, and pushed: to the publish branch when one is set, else to the current branch's upstream.
fn publish_site(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let site_dirs: Vec<String> = root
        .components
        .iter()
        .filter(|component| component.site)
        .map(|component| {
            if component.path.is_empty() {
                ".".to_string()
            } else {
                component.path.clone()
            }
        })
        .collect();
    if site_dirs.is_empty() {
        return Err(LibError::new(format!("{} holds no site component", root.label)));
    }
    let git = context.git();
    let mut status_args = vec!["status", "--porcelain=v2", "-z", "--untracked-files=all", "--"];
    status_args.extend(site_dirs.iter().map(String::as_str));
    let changed: Vec<String> = git::parse_status(&git.read(&root.path, &status_args)?)
        .entries
        .into_iter()
        .filter(|entry| entry.index != '!')
        .map(|entry| entry.path)
        .collect();
    let mut out = Parameters::new();
    if changed.is_empty() {
        out.insert("published".into(), Value::Bool(false));
        out.insert(
            "message".into(),
            Value::string("Nothing to publish: the site components have no changes"),
        );
        return Ok(out);
    }
    let mut add = vec!["add".to_string(), "-A".to_string(), "--".to_string()];
    add.extend(site_dirs.iter().cloned());
    let argv: Vec<&str> = add.iter().map(String::as_str).collect();
    git.run(&root.path, Mode::Write, &argv, &[], None)?.checked("add")?;
    let mut parameters = parameters.clone();
    if text(&parameters, "summary").trim().is_empty() {
        let names: Vec<&str> = changed
            .iter()
            .map(|path| path.rsplit('/').next().unwrap_or(path))
            .take(5)
            .collect();
        let more = if changed.len() > 5 {
            format!(" and {} more", changed.len() - 5)
        } else {
            String::new()
        };
        parameters.insert(
            "summary".into(),
            Value::string(format!("Customize: {}{more}", names.join(", "))),
        );
    }
    let message = message_of(root, &parameters)?;
    let (id, commit_output) = commit(&context, root, &message, &parameters)?;
    network_allowed(&context)?;
    let branch = current_branch(&context, root)?;
    let remote = push_remote_name(&context, root, &branch)?;
    let target = publish_branch(root);
    let refspec = target
        .as_ref()
        .map_or_else(|| branch.clone(), |target| format!("HEAD:refs/heads/{target}"));
    let push_args: Vec<&str> = if target.is_some() {
        vec!["push", &remote, &refspec]
    } else {
        vec!["push", "-u", &remote, &refspec]
    };
    let output = git.run(&root.path, Mode::Network, &push_args, &[], None)?;
    audit_git(
        &context,
        root,
        "publish",
        serde_json::json!({ "commit": id, "branch": target.clone().unwrap_or_else(|| branch.clone()), "ok": output.success() }),
    );
    out.insert("commitId".into(), Value::string(&id));
    out.insert(
        "output".into(),
        Value::string(format!("{commit_output}{}{}", output.stdout, output.stderr)),
    );
    if !output.success() {
        out.insert("published".into(), Value::Bool(false));
        out.insert(
            "message".into(),
            Value::string(format!(
                "Committed {}; the push failed: {}",
                &id[..id.len().min(9)],
                output.stderr.trim()
            )),
        );
        return Ok(out);
    }
    out.insert("published".into(), Value::Bool(true));
    out.insert(
        "pushedBranch".into(),
        Value::string(target.clone().unwrap_or_else(|| branch.clone())),
    );
    out.insert(
        "message".into(),
        Value::string(match &target {
            Some(target) => format!("Published to {remote} {target}; merge {target} into {branch} to share it"),
            None => format!("Published to {remote} {branch}"),
        }),
    );
    out.insert("files".into(), json_value(&changed));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names() {
        for good in ["feature/due-date", "PROJ-12-due", "publish/john.doe"] {
            assert!(valid_branch(good).is_ok(), "{good}");
        }
        for bad in ["", "-x", "a..b", "a/", "x.lock", "a b", "a;rm"] {
            assert!(valid_branch(bad).is_err(), "{bad}");
        }
    }
}
