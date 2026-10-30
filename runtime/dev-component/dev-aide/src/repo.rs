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

//! `org.moqui.devaide.VcsServices`, git depth (plans/design-dev-aide.md 4.5 to 4.7, P8): hunks staged, unstaged and
//! discarded one at a time; branches renamed, deleted, merged and compared; tags and stashes; revert and cherry-pick;
//! a commit's files browsed; blame; the operation in progress and its conflicts, resolved or aborted; remotes; and a
//! clone into a component directory. Each write takes the user's tier, is audited, and answers a conflict as a state
//! (`conflict: true` with git's output), not as an error, so the page opens the conflicts.

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::audit::{self, Write};
use crate::git::{self, Mode};
use crate::patch;
use crate::roots::Root;
use crate::tier::Tier;
use crate::vcs::{audit_git, git_root, network, network_allowed, valid_branch};
use crate::workspace::{flag, json_value, sha256, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_hunks), get_hunks)?;
    registry.service_function(moqui_core::service_location!(stage_hunks), stage_hunks)?;
    registry.service_function(moqui_core::service_location!(unstage_hunks), unstage_hunks)?;
    registry.service_function(moqui_core::service_location!(discard_hunks), discard_hunks)?;
    registry.service_function(moqui_core::service_location!(get_branches), get_branches)?;
    registry.service_function(moqui_core::service_location!(rename_branch), rename_branch)?;
    registry.service_function(moqui_core::service_location!(delete_branch), delete_branch)?;
    registry.service_function(moqui_core::service_location!(merge_branch), merge_branch)?;
    registry.service_function(moqui_core::service_location!(compare_refs), compare_refs)?;
    registry.service_function(moqui_core::service_location!(get_tags), get_tags)?;
    registry.service_function(moqui_core::service_location!(create_tag), create_tag)?;
    registry.service_function(moqui_core::service_location!(delete_tag), delete_tag)?;
    registry.service_function(moqui_core::service_location!(push_tag), push_tag)?;
    registry.service_function(moqui_core::service_location!(get_stashes), get_stashes)?;
    registry.service_function(moqui_core::service_location!(create_stash), create_stash)?;
    registry.service_function(moqui_core::service_location!(apply_stash), apply_stash)?;
    registry.service_function(moqui_core::service_location!(pop_stash), pop_stash)?;
    registry.service_function(moqui_core::service_location!(drop_stash), drop_stash)?;
    registry.service_function(moqui_core::service_location!(revert_commit), revert_commit)?;
    registry.service_function(moqui_core::service_location!(cherry_pick_commit), cherry_pick_commit)?;
    registry.service_function(moqui_core::service_location!(get_tree), get_tree)?;
    registry.service_function(moqui_core::service_location!(get_file_at), get_file_at)?;
    registry.service_function(moqui_core::service_location!(get_blame), get_blame)?;
    registry.service_function(moqui_core::service_location!(get_conflict_sides), get_conflict_sides)?;
    registry.service_function(moqui_core::service_location!(resolve_conflict), resolve_conflict)?;
    registry.service_function(moqui_core::service_location!(abort_operation), abort_operation)?;
    registry.service_function(moqui_core::service_location!(add_remote), add_remote)?;
    registry.service_function(moqui_core::service_location!(set_remote_url), set_remote_url)?;
    registry.service_function(moqui_core::service_location!(remove_remote), remove_remote)?;
    registry.service_function(moqui_core::service_location!(prune_remote), prune_remote)?;
    registry.service_function(moqui_core::service_location!(clone_repository), clone_repository)?;
    registry.service_function(
        moqui_core::service_location!(cancel_git_operation),
        cancel_git_operation,
    )
}

/// git's output of a command, both streams.
fn shown(output: &git::Output) -> String {
    format!("{}{}", output.stdout, output.stderr)
}

/// An answer with git's output.
fn answer(output: &git::Output) -> Parameters {
    let mut out = Parameters::new();
    out.insert("output".into(), Value::string(shown(output)));
    out
}

/// A write that may stop at a conflict (a merge, a revert, a cherry-pick, a stash applied): success, or a conflict
/// answered as a state; any other failure is git's message.
fn may_conflict(context: &Context, root: &Root, output: git::Output, what: &str) -> Result<Parameters, LibError> {
    let mut out = answer(&output);
    if output.success() {
        out.insert("conflict".into(), Value::Bool(false));
        return Ok(out);
    }
    let unmerged = git::parse_status(
        &context
            .git()
            .read(&root.path, &["status", "--porcelain=v2", "-z"])
            .unwrap_or_default(),
    )
    .entries
    .iter()
    .any(|entry| entry.conflicted);
    if unmerged {
        out.insert("conflict".into(), Value::Bool(true));
        Ok(out)
    } else {
        Err(output
            .checked(what)
            .err()
            .unwrap_or_else(|| LibError::new(format!("git {what} failed"))))
    }
}

/// A commit id or a ref name a read takes: hexadecimal, or a branch or tag name, with `~n` or `^` after it.
pub(crate) fn valid_rev(rev: &str) -> Result<(), LibError> {
    let base = rev.trim_end_matches(|ch: char| ch == '^' || ch == '~' || ch.is_ascii_digit());
    let base = if base.is_empty() { rev } else { base };
    if base == "HEAD" || valid_branch(base).is_ok() {
        Ok(())
    } else {
        Err(LibError::new(format!("{rev} is not a commit or ref this takes")))
    }
}

fn number(parameters: &Parameters, key: &str) -> Result<u32, LibError> {
    text(parameters, key)
        .parse()
        .map_err(|_| LibError::new(format!("{key} is a number")))
}

// ---- hunks ----

/// A file's diff: the work tree against the index, or the index against HEAD with `staged`.
fn file_diff(context: &Context, root: &Root, path: &str, staged: bool) -> Result<String, LibError> {
    let mut args = vec!["diff", "--no-ext-diff", "--no-color", "-U3"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", path]);
    context.git().read(&root.path, &args)
}

/// `get#Hunks`: a file's hunks with their ranges, and the diff's hash a hunk write is checked against.
fn get_hunks(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let path = context.resolve(root, &text(parameters, "path"))?.relative;
    let diff = file_diff(&context, root, &path, flag(parameters, "staged"))?;
    let mut out = Parameters::new();
    out.insert("hunks".into(), json_value(&patch::parse(&diff).hunks));
    out.insert("diffHash".into(), Value::string(sha256(diff.as_bytes())));
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HunkWrite {
    Stage,
    Unstage,
    Discard,
}

/// The selected hunks of a file (`hunks`, indexes into `get#Hunks`) applied as `how` says, refused when the diff
/// changed since the page read it (`diffHash`).
fn apply_hunks(ec: &ExecutionContext, parameters: &Parameters, how: HunkWrite) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = crate::vcs::git_write_root(&context, parameters, "stages and discards")?;
    let path = context.resolve(root, &text(parameters, "path"))?.relative;
    if how == HunkWrite::Discard {
        context.may_write(root, &path)?;
    }
    let diff = file_diff(&context, root, &path, how == HunkWrite::Unstage)?;
    let expected = text(parameters, "diffHash");
    if !expected.is_empty() && expected != sha256(diff.as_bytes()) {
        return Err(LibError::new(format!(
            "{path} changed since its diff was read: look again"
        )));
    }
    let selected: Vec<usize> = match parameters.get("hunks") {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| moqui_lib::object::to_plain_string(item).parse().ok())
            .collect(),
        _ => text(parameters, "hunks")
            .split(',')
            .filter_map(|item| item.trim().parse().ok())
            .collect(),
    };
    let parsed = patch::parse(&diff);
    let reverse = how != HunkWrite::Stage;
    let patch_text =
        patch::patch_for(&parsed, &selected, reverse).ok_or_else(|| LibError::new("no hunk of the file was chosen"))?;
    let mut args = vec!["apply", "--whitespace=nowarn", "--recount"];
    if how != HunkWrite::Discard {
        args.push("--cached");
    }
    if reverse {
        args.push("-R");
    }
    args.push("-");
    let output = context
        .git()
        .run(&root.path, Mode::Write, &args, &[], Some(&patch_text))?
        .checked("apply")?;
    let what = match how {
        HunkWrite::Stage => "stage hunks",
        HunkWrite::Unstage => "unstage hunks",
        HunkWrite::Discard => "discard hunks",
    };
    audit_git(&context, root, what, json!({ "path": path, "hunks": selected }));
    Ok(answer(&output))
}

fn stage_hunks(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    apply_hunks(ec, parameters, HunkWrite::Stage)
}

fn unstage_hunks(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    apply_hunks(ec, parameters, HunkWrite::Unstage)
}

fn discard_hunks(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    apply_hunks(ec, parameters, HunkWrite::Discard)
}

// ---- branches ----

const REF_FORMAT: &str = "--format=%(refname)%1f%(refname:short)%1f%(upstream:short)%1f%(upstream:track)%1f%(HEAD)%1f%(committerdate:iso-strict)%1f%(contents:subject)";

/// `[ahead 2, behind 1]` as its two counts, and whether the upstream is gone.
fn track(text: &str) -> (u32, u32, bool) {
    let count = |word: &str| {
        text.split(word)
            .nth(1)
            .and_then(|rest| rest.trim_start().split(|ch: char| !ch.is_ascii_digit()).next())
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(0)
    };
    (count("ahead"), count("behind"), text.contains("gone"))
}

/// `get#Branches`: the local branches with their upstream, ahead and behind and last commit, and the remote ones.
fn get_branches(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let listed = context
        .git()
        .read(&root.path, &["for-each-ref", REF_FORMAT, "refs/heads", "refs/remotes"])?;
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for line in listed.lines() {
        let fields: Vec<&str> = line.split('\u{1f}').collect();
        if fields.len() < 7 {
            continue;
        }
        let (ahead, behind, gone) = track(fields[3]);
        let row = json!({
            "name": fields[1], "upstream": fields[2], "ahead": ahead, "behind": behind, "gone": gone,
            "current": fields[4] == "*", "date": fields[5], "subject": fields[6],
        });
        if fields[0].starts_with("refs/heads/") {
            local.push(row);
        } else if !fields[0].ends_with("/HEAD") {
            remote.push(row);
        }
    }
    let mut out = Parameters::new();
    out.insert("local".into(), json_value(&local));
    out.insert("remote".into(), json_value(&remote));
    Ok(out)
}

/// A local branch write with git's output, audited.
fn branch_write(
    ec: &ExecutionContext,
    parameters: &Parameters,
    args: &[&str],
    what: &str,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let output = context
        .git()
        .run(&root.path, Mode::Write, args, &[], None)?
        .checked(what)?;
    audit_git(&context, root, what, json!(args));
    Ok(answer(&output))
}

/// `rename#Branch`.
fn rename_branch(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let (from, to) = (text(parameters, "branch"), text(parameters, "newName"));
    valid_branch(&from)?;
    valid_branch(&to)?;
    branch_write(ec, parameters, &["branch", "-m", &from, &to], "branch -m")
}

/// `delete#Branch`: merged branches only, unless `force` (the page's second confirm).
fn delete_branch(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let branch = text(parameters, "branch");
    valid_branch(&branch)?;
    let mode = if flag(parameters, "force") { "-D" } else { "-d" };
    branch_write(ec, parameters, &["branch", mode, &branch], "branch -d")
}

/// `merge#Branch` into the current one: fast-forward only, unless `mergeCommit`; a conflict is answered as one.
fn merge_branch(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let branch = text(parameters, "branch");
    valid_rev(&branch)?;
    let args: Vec<&str> = if flag(parameters, "mergeCommit") {
        vec!["merge", "--no-ff", "--no-edit", &branch]
    } else {
        vec!["merge", "--ff-only", &branch]
    };
    let env = crate::vcs::author_env(&context);
    let output = context.git().run(&root.path, Mode::Write, &args, &env, None)?;
    audit_git(
        &context,
        root,
        "merge",
        json!({ "branch": branch, "ok": output.success() }),
    );
    may_conflict(&context, root, output, "merge")
}

/// `compare#Refs`: the commits on `head` that `base` has not (at most 200), and the files that differ since their
/// common ancestor, as Gitea's compare shows them.
fn compare_refs(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let (base, head) = (text(parameters, "base"), text(parameters, "head"));
    valid_rev(&base)?;
    valid_rev(&head)?;
    let git = context.git();
    let range = format!("{base}..{head}");
    let commits = git::parse_log(&git.read(&root.path, &["log", git::LOG_FORMAT, "-n", "200", &range])?);
    let three = format!("{base}...{head}");
    let files = name_status(&git.read(&root.path, &["diff", "--no-ext-diff", "--name-status", "-z", &three])?);
    let mut out = Parameters::new();
    out.insert("commits".into(), json_value(&commits));
    out.insert("files".into(), json_value(&files));
    Ok(out)
}

/// `--name-status -z` as each file's status letter and path (a rename's new path).
fn name_status(out: &str) -> Vec<Json> {
    let mut parts = out.split('\0').filter(|part| !part.is_empty());
    let mut files = Vec::new();
    while let Some(status) = parts.next() {
        let letter = status.get(..1).unwrap_or(status);
        let mut path = parts.next().unwrap_or("");
        if matches!(letter, "R" | "C") {
            path = parts.next().unwrap_or(path);
        }
        files.push(json!({ "status": letter, "path": path }));
    }
    files
}

// ---- tags ----

/// `get#Tags`: newest first, each with its commit, date and annotation's subject.
fn get_tags(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let listed = context.git().read(
        &root.path,
        &[
            "for-each-ref",
            "--sort=-creatordate",
            "--format=%(refname:short)%1f%(objecttype)%1f%(*objectname)%(objectname)%1f%(creatordate:iso-strict)%1f%(contents:subject)",
            "refs/tags",
        ],
    )?;
    let tags: Vec<Json> = listed
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\u{1f}').collect();
            (fields.len() >= 5).then(|| {
                // an annotated tag's own object then its commit; a lightweight one's commit only
                let commit = if fields[1] == "tag" { fields[2].get(..40).unwrap_or(fields[2]) } else { fields[2] };
                json!({ "name": fields[0], "annotated": fields[1] == "tag", "commit": commit, "date": fields[3], "subject": fields[4] })
            })
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("tags".into(), json_value(&tags));
    Ok(out)
}

/// `create#Tag`: annotated when there is a `message`, at `commitId` or HEAD.
fn create_tag(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let tag = text(parameters, "tag");
    valid_branch(&tag)?;
    let message = text(parameters, "message");
    let commit = text(parameters, "commitId");
    if !commit.is_empty() {
        valid_rev(&commit)?;
    }
    let mut args = vec!["tag"];
    if !message.trim().is_empty() {
        args.extend(["-a", "-F", "-"]);
    }
    args.push(&tag);
    if !commit.is_empty() {
        args.push(&commit);
    }
    // the tagger is the server's identity, as a commit's committer is (4.2)
    let stdin = Some(message.as_str()).filter(|message| !message.trim().is_empty());
    let output = context
        .git()
        .run(&root.path, Mode::Write, &args, &[], stdin)?
        .checked("tag")?;
    audit_git(&context, root, "tag", json!({ "tag": tag, "commit": commit }));
    Ok(answer(&output))
}

/// `delete#Tag`: the local tag.
fn delete_tag(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let tag = text(parameters, "tag");
    valid_branch(&tag)?;
    branch_write(ec, parameters, &["tag", "-d", &tag], "tag -d")
}

/// `push#Tag` to the branch's remote (or `remote`).
fn push_tag(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let tag = text(parameters, "tag");
    valid_branch(&tag)?;
    let remote = text(parameters, "remote");
    if !remote.is_empty() {
        valid_branch(&remote)?;
    }
    if Context::of(ec, Tier::Full).is_err() {
        return Err(LibError::new("Pushing a tag needs the full tier of Dev AIDE"));
    }
    network(
        ec,
        parameters,
        &move |context, root| {
            let remote = if remote.is_empty() {
                let branch = crate::vcs::current_branch(context, root)?;
                crate::vcs::push_remote_name(context, root, &branch)?
            } else {
                remote.clone()
            };
            Ok(vec!["push".into(), remote, format!("refs/tags/{tag}")])
        },
        "push tag",
    )
}

// ---- stashes ----

/// `get#Stashes`: newest first, with each one's message and date.
fn get_stashes(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let listed = context
        .git()
        .read(&root.path, &["stash", "list", "--format=%gd%x1f%gs%x1f%cI"])
        .unwrap_or_default();
    let stashes: Vec<Json> = listed
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let fields: Vec<&str> = line.split('\u{1f}').collect();
            (fields.len() >= 3)
                .then(|| json!({ "index": index, "ref": fields[0], "message": fields[1], "date": fields[2] }))
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("stashes".into(), json_value(&stashes));
    Ok(out)
}

/// `create#Stash`: the work tree's changes, with untracked files when `untracked`, under `message`.
fn create_stash(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let message = text(parameters, "message");
    let mut args = vec!["stash", "push"];
    if flag(parameters, "untracked") {
        args.push("-u");
    }
    if !message.trim().is_empty() {
        args.extend(["-m", message.trim()]);
    }
    branch_write(ec, parameters, &args, "stash")
}

/// A stash by its `index` (`stash@{n}`).
fn stash_ref(parameters: &Parameters) -> Result<String, LibError> {
    Ok(format!("stash@{{{}}}", number(parameters, "index")?))
}

/// `apply#Stash` and `pop#Stash`: a conflict is answered as one, and the stash is kept (git keeps it on a conflict).
fn restore_stash(ec: &ExecutionContext, parameters: &Parameters, pop: bool) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let stash = stash_ref(parameters)?;
    let verb = if pop { "pop" } else { "apply" };
    let output = context
        .git()
        .run(&root.path, Mode::Write, &["stash", verb, &stash], &[], None)?;
    audit_git(
        &context,
        root,
        &format!("stash {verb}"),
        json!({ "stash": stash, "ok": output.success() }),
    );
    may_conflict(&context, root, output, "stash")
}

fn apply_stash(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    restore_stash(ec, parameters, false)
}

fn pop_stash(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    restore_stash(ec, parameters, true)
}

fn drop_stash(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let stash = stash_ref(parameters)?;
    branch_write(ec, parameters, &["stash", "drop", &stash], "stash drop")
}

// ---- commits ----

/// A commit applied again (`cherry-pick`) or undone (`revert`) on the current branch, the user its author; a conflict
/// is answered as one.
fn replay(ec: &ExecutionContext, parameters: &Parameters, verb: &str) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let id = text(parameters, "commitId");
    if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(LibError::new("a commit id is hexadecimal"));
    }
    let args: Vec<&str> = if verb == "revert" {
        vec!["revert", "--no-edit", &id]
    } else {
        vec!["cherry-pick", &id]
    };
    let env = crate::vcs::author_env(&context);
    let output = context.git().run(&root.path, Mode::Write, &args, &env, None)?;
    audit_git(&context, root, verb, json!({ "commit": id, "ok": output.success() }));
    may_conflict(&context, root, output, verb)
}

fn revert_commit(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    replay(ec, parameters, "revert")
}

fn cherry_pick_commit(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    replay(ec, parameters, "cherry-pick")
}

/// `get#Tree`: a directory's entries at a commit (Browse files at this commit), directories first.
fn get_tree(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let rev = text(parameters, "rev");
    valid_rev(&rev)?;
    let dir = text(parameters, "path");
    let dir = if dir.is_empty() {
        String::new()
    } else {
        context.resolve(root, &dir)?.relative
    };
    let mut args = vec!["ls-tree", "-z", &rev];
    let spec = format!("{dir}/");
    if !dir.is_empty() {
        args.extend(["--", spec.as_str()]);
    }
    let listed = context.git().read(&root.path, &args)?;
    let mut entries: Vec<Json> = listed
        .split('\0')
        .filter_map(|record| {
            let (meta, path) = record.split_once('\t')?;
            let kind = meta.split(' ').nth(1)?;
            Some(json!({ "path": path, "name": path.rsplit('/').next().unwrap_or(path), "dir": kind == "tree" }))
        })
        .collect();
    entries.sort_by_key(|entry| {
        (
            !entry["dir"].as_bool().unwrap_or(false),
            entry["name"].as_str().unwrap_or("").to_string(),
        )
    });
    let mut out = Parameters::new();
    out.insert("entries".into(), json_value(&entries));
    Ok(out)
}

/// `get#FileAt`: a file's text at a commit, read only; a binary or too long file says so.
fn get_file_at(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let rev = text(parameters, "rev");
    valid_rev(&rev)?;
    let path = context.resolve(root, &text(parameters, "path"))?.relative;
    let body = context.git().read(&root.path, &["show", &format!("{rev}:{path}")])?;
    let mut out = Parameters::new();
    if body.len() as u64 > context.settings.max_edit_bytes || body.contains('\0') {
        out.insert("text".into(), Value::Null);
        out.insert("binary".into(), Value::Bool(true));
    } else {
        out.insert("text".into(), Value::string(body));
    }
    Ok(out)
}

/// `get#Blame`: who last changed each run of a file's lines, at `rev` or in the work tree.
fn get_blame(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let path = context.resolve(root, &text(parameters, "path"))?.relative;
    let rev = text(parameters, "rev");
    let mut args = vec!["blame", "--porcelain"];
    if !rev.is_empty() {
        valid_rev(&rev)?;
        args.push(&rev);
    }
    args.extend(["--", path.as_str()]);
    let output = context.git().run(&root.path, Mode::Read, &args, &[], None)?;
    let mut out = Parameters::new();
    // an untracked file has no blame
    let ranges = if output.success() {
        git::parse_blame(&output.stdout)
    } else {
        Vec::new()
    };
    out.insert("ranges".into(), json_value(&ranges));
    Ok(out)
}

// ---- conflicts ----

/// The operation in progress that stopped at a conflict: `merge`, `cherry-pick`, `revert` or `rebase` by git's own
/// marker files, `stash` when files are unmerged with none of them (a stash applied), else `None`.
pub(crate) fn operation(context: &Context, root: &Root, conflicted: bool) -> Option<&'static str> {
    let git_dir = context
        .git()
        .read(&root.path, &["rev-parse", "--absolute-git-dir"])
        .ok()?;
    let git_dir = std::path::PathBuf::from(git_dir.trim());
    for (marker, name) in [
        ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
    ] {
        if git_dir.join(marker).exists() {
            return Some(name);
        }
    }
    conflicted.then_some("stash")
}

/// `get#ConflictSides`: a conflicted file's common ancestor, ours and theirs (the index's stages 1 to 3) and the work
/// tree's text with its markers and hash, which `resolve#Conflict` is checked against.
fn get_conflict_sides(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    let path = &resolved.relative;
    let stage = |number: u8| crate::vcs::show(&context, root, &format!(":{number}:{path}"));
    let bytes = std::fs::read(&resolved.path).unwrap_or_default();
    let mut out = Parameters::new();
    out.insert("base".into(), Value::string(stage(1)));
    out.insert("ours".into(), Value::string(stage(2)));
    out.insert("theirs".into(), Value::string(stage(3)));
    out.insert("result".into(), Value::string(String::from_utf8_lossy(&bytes)));
    out.insert("hash".into(), Value::string(sha256(&bytes)));
    // a file with sides is conflicted
    out.insert(
        "operation".into(),
        operation(&context, root, true).map_or(Value::Null, Value::string),
    );
    Ok(out)
}

/// `resolve#Conflict`: the resolved text saved through the save pipeline (checked against `baseHash`, reloaded and
/// checked) and the file staged, which marks it resolved.
fn resolve_conflict(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    let result = text(parameters, "text");
    if result
        .lines()
        .any(|line| line.starts_with("<<<<<<< ") || line.starts_with(">>>>>>> "))
    {
        return Err(LibError::new(format!(
            "{} still has conflict markers",
            resolved.relative
        )));
    }
    let mut out = crate::workspace::save_text(&context, root, &resolved, &result, &text(parameters, "baseHash"), None)?;
    if out
        .get("conflict")
        .is_some_and(|value| matches!(value, Value::Bool(true)))
    {
        return Ok(out);
    }
    let staged = context
        .git()
        .run(&root.path, Mode::Write, &["add", "--", &resolved.relative], &[], None)?
        .checked("add")?;
    audit_git(&context, root, "resolve", json!({ "path": resolved.relative }));
    out.insert("output".into(), Value::string(shown(&staged)));
    Ok(out)
}

/// `abort#Operation`: the merge, cherry-pick or revert in progress undone (`--abort`). A stash applied has none: git
/// kept the stash, so the files are resolved or discarded.
fn abort_operation(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = crate::vcs::git_write_root(&context, parameters, "aborts operations")?;
    let verb = match operation(&context, root, true) {
        Some(verb @ ("merge" | "cherry-pick" | "revert")) => verb,
        Some("rebase") => {
            return Err(LibError::new(
                "A rebase is in progress: Dev AIDE does not run rebases; finish or abort it in a terminal (git rebase --abort)",
            ))
        }
        Some(_) => {
            return Err(LibError::new(
                "A stash applied has no abort: git kept the stash, so resolve the files or discard their changes",
            ))
        }
        None => return Err(LibError::new("No merge, cherry-pick or revert is in progress")),
    };
    let output = context
        .git()
        .run(&root.path, Mode::Write, &[verb, "--abort"], &[], None)?
        .checked(verb)?;
    audit_git(&context, root, &format!("{verb} --abort"), json!({}));
    Ok(answer(&output))
}

// ---- remotes ----

/// A remote's URL as this takes it: not an option, no control characters.
fn valid_url(url: &str) -> Result<(), LibError> {
    if url.is_empty() || url.starts_with('-') || url.chars().any(char::is_control) {
        Err(LibError::new(format!("{url} is not a remote URL this takes")))
    } else {
        Ok(())
    }
}

fn add_remote(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let (name, url) = (text(parameters, "name"), text(parameters, "url"));
    valid_branch(&name)?;
    valid_url(&url)?;
    branch_write(ec, parameters, &["remote", "add", &name, &url], "remote add")
}

fn set_remote_url(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let (name, url) = (text(parameters, "name"), text(parameters, "url"));
    valid_branch(&name)?;
    valid_url(&url)?;
    branch_write(ec, parameters, &["remote", "set-url", &name, &url], "remote set-url")
}

fn remove_remote(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let name = text(parameters, "name");
    valid_branch(&name)?;
    branch_write(ec, parameters, &["remote", "remove", &name], "remote remove")
}

/// `prune#Remote`: the remote-tracking branches whose branch the remote no longer has.
fn prune_remote(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let name = text(parameters, "name");
    valid_branch(&name)?;
    network(
        ec,
        parameters,
        &move |_, _| Ok(vec!["remote".into(), "prune".into(), name.clone()]),
        "fetch",
    )
}

// ---- clone and cancel ----

/// `clone#Repository`: a URL cloned into `runtime/component/<name>`, or `runtime/dev-component/<name>` with `dev`; it
/// loads after a restart. The full tier's, with network git allowed.
fn clone_repository(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    network_allowed(&context)?;
    let url = text(parameters, "url");
    valid_url(&url)?;
    let name = text(parameters, "name");
    let name = if name.is_empty() {
        url.trim_end_matches('/')
            .rsplit(['/', ':'])
            .next()
            .unwrap_or("")
            .trim_end_matches(".git")
            .to_string()
    } else {
        name
    };
    if name.is_empty()
        || name.starts_with('.')
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(LibError::new(format!("{name} is not a component directory name")));
    }
    let parent = context.factory.runtime_dir().join(if flag(parameters, "dev") {
        "dev-component"
    } else {
        "component"
    });
    std::fs::create_dir_all(&parent).map_err(|err| LibError::new(format!("{}: {err}", parent.display())))?;
    if parent.join(&name).exists() {
        return Err(LibError::new(format!(
            "{} is there already",
            parent.join(&name).display()
        )));
    }
    let operation = text(parameters, "operationId");
    let progress = (!operation.is_empty()).then(|| crate::vcs::progress(&context, &operation));
    let mut args = vec!["clone"];
    if progress.is_some() {
        args.push("--progress");
    }
    args.extend(["--", url.as_str(), name.as_str()]);
    let output = context
        .git()
        .run_with(&parent, Mode::Network, &args, &[], None, progress)?;
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_GIT",
            root: "",
            path: &name,
            detail: Some(json!({ "command": "clone", "url": url, "ok": output.success() })),
            ..Write::default()
        },
    );
    let output = output.checked("clone")?;
    let mut out = answer(&output);
    out.insert(
        "location".into(),
        Value::string(parent.join(&name).display().to_string()),
    );
    out.insert("restart".into(), Value::Bool(true));
    Ok(out)
}

/// `cancel#GitOperation`: the network command running as `operationId` stopped.
fn cancel_git_operation(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let _context = Context::of(ec, Tier::Extend)?;
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let key = crate::vcs::operation_key(&user, &text(parameters, "operationId"));
    let mut out = Parameters::new();
    out.insert("cancelled".into(), Value::Bool(git::cancel(&key)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_counts() {
        assert_eq!(track("[ahead 2, behind 13]"), (2, 13, false));
        assert_eq!(track("[behind 1]"), (0, 1, false));
        assert_eq!(track("[gone]"), (0, 0, true));
        assert_eq!(track(""), (0, 0, false));
    }

    #[test]
    fn renames_name_their_new_path() {
        let files = name_status("M\0a.txt\0R100\0old.txt\0new.txt\0D\0gone.txt\0");
        let paths: Vec<&str> = files.iter().map(|file| file["path"].as_str().unwrap()).collect();
        assert_eq!(paths, vec!["a.txt", "new.txt", "gone.txt"]);
        assert_eq!(files[1]["status"], "R");
    }

    #[test]
    fn revisions_this_takes() {
        for good in ["HEAD", "HEAD~2", "main", "origin/main", "v1.2", "a1b2c3d", "HEAD^"] {
            assert!(valid_rev(good).is_ok(), "{good}");
        }
        for bad in ["", "-x", "a..b", "main;rm", "--all"] {
            assert!(valid_rev(bad).is_err(), "{bad}");
        }
    }
}
