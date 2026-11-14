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

//! A worktree for an agent's session (plans/design-dev-aide.md 21.8, question 19): `git worktree add` of a working
//! tree, on a branch of its own, under the runtime's `worktrees` directory, kept as a temporary root ("worktree of X")
//! bound to an Assist conversation. The file and git tools of that conversation are rooted in it in place of X
//! (`agent::session_context`), so two sessions edit two worktrees of one repository and share no uncommitted change. A
//! worktree is not loaded (question 19, as proposed): its files are checked and committed, and merged or proposed from
//! there, but not run live; a session that needs the running server works in the main tree. Removing one refuses while
//! it has uncommitted changes, unless they are to be discarded; its branch stays.

use std::path::{Path, PathBuf};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::json;

use crate::git::Mode;
use crate::roots::{Root, ROOT_ENTITY};
use crate::tier::Tier;
use crate::vcs::{audit_git, git_write_root};
use crate::workspace::{flag, json_value, text, Context};

/// The directory under the runtime that holds the worktrees (git ignores it; the main tree's file tree does not reach
/// it, `confine::WORKTREES`).
pub const DIR: &str = crate::confine::WORKTREES;

/// What the session panel says of every worktree (question 19).
pub const NOT_LOADED: &str = "A worktree is not loaded: its files are checked and committed here, and merged or \
    proposed from its branch, but the running server keeps the main tree's. A session that needs to run what it changes \
    works in the main tree.";

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_worktrees), get_worktrees)?;
    registry.service_function(moqui_core::service_location!(create_worktree), create_worktree)?;
    registry.service_function(moqui_core::service_location!(remove_worktree), remove_worktree)
}

/// A name part from a conversation's id or a label: letters, digits and `-`, at most 24.
#[must_use]
pub fn tag_of(text: &str) -> String {
    let tag: String = text
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let tag = tag.trim_matches('-').to_string();
    tag.chars()
        .take(24)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

/// The directory of a worktree of root `main_id` with `tag`, under `worktrees`.
#[must_use]
pub fn worktree_dir(worktrees: &Path, main_id: &str, tag: &str) -> PathBuf {
    worktrees.join(format!("{}-{}", tag_of(main_id), tag_of(tag)))
}

/// The branch a worktree is made on when none is named: `agent/<tag>`.
#[must_use]
pub fn default_branch(tag: &str) -> String {
    format!("agent/{}", tag_of(tag))
}

/// Why a worktree may not be removed now: it has uncommitted changes (`git status --porcelain`) and they are not to be
/// discarded.
#[must_use]
pub fn removal_refusal(label: &str, porcelain: &str, discard: bool) -> Option<String> {
    let changed = porcelain.lines().filter(|line| !line.trim().is_empty()).count();
    (changed > 0 && !discard).then(|| {
        format!("{label} has uncommitted changes in {changed} files: commit them first, or remove it discarding them")
    })
}

/// The worktree roots of an agent's conversation, each as (the main root's id, the worktree root's id): the tools of
/// that session use the worktree in place of the main tree.
#[must_use]
pub fn redirects(roots: &[Root], conversation_id: &str) -> Vec<(String, String)> {
    if conversation_id.is_empty() {
        return Vec::new();
    }
    roots
        .iter()
        .filter(|root| root.conversation_id.as_deref() == Some(conversation_id))
        .filter_map(|root| root.worktree_of.clone().map(|main| (main, root.id.clone())))
        .collect()
}

fn root_json(context: &Context, root: &Root) -> serde_json::Value {
    let porcelain = context
        .git()
        .read(&root.path, &["status", "--porcelain"])
        .unwrap_or_default();
    let changes = porcelain.lines().filter(|line| !line.trim().is_empty()).count();
    let branch = context
        .git()
        .read(&root.path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|out| out.trim().to_string())
        .unwrap_or_default();
    json!({
        "rootId": root.id, "label": root.label, "location": root.path.display().to_string(),
        "mainRootId": root.worktree_of, "conversationId": root.conversation_id, "branch": branch,
        "changes": changes,
    })
}

/// `get#Worktrees`: the worktree roots (of `rootId`, or of a conversation, or all), each with its branch and its
/// uncommitted changes, and what the panel says of them (not loaded).
fn get_worktrees(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let main = text(parameters, "rootId");
    let conversation = text(parameters, "conversationId");
    let list: Vec<serde_json::Value> = context
        .roots
        .iter()
        .filter(|root| root.worktree_of.is_some())
        .filter(|root| main.is_empty() || root.worktree_of.as_deref() == Some(main.as_str()))
        .filter(|root| conversation.is_empty() || root.conversation_id.as_deref() == Some(conversation.as_str()))
        .map(|root| root_json(&context, root))
        .collect();
    let mut out = Parameters::new();
    out.insert("worktrees".into(), json_value(&list));
    out.insert("note".into(), Value::string(NOT_LOADED));
    Ok(out)
}

/// `create#Worktree`: a worktree of `rootId` for an agent's session (`conversationId`), on `branch` (a new
/// `agent/<tag>` by default, or an existing branch not checked out elsewhere), kept as a temporary root.
fn create_worktree(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let main = git_write_root(&context, parameters, "makes worktrees")?;
    if main.worktree_of.is_some() {
        return Err(LibError::new(format!(
            "{} is a worktree: make another of its main tree",
            main.label
        )));
    }
    let conversation = text(parameters, "conversationId");
    let tag = if conversation.is_empty() {
        moqui_core::user::random_string(8)?
    } else {
        conversation.clone()
    };
    if tag_of(&tag).is_empty() {
        return Err(LibError::new("a worktree's conversation id has letters or digits"));
    }
    let branch = match text(parameters, "branch").trim() {
        "" => default_branch(&tag),
        named => named.to_string(),
    };
    crate::vcs::valid_branch(&branch)?;
    let worktrees = context.factory.runtime_dir().join(DIR);
    std::fs::create_dir_all(&worktrees).map_err(|err| LibError::new(format!("{}: {err}", worktrees.display())))?;
    let dir = worktree_dir(&worktrees, &main.id, &tag);
    if dir.exists() {
        return Err(LibError::new(format!("{} is there already", dir.display())));
    }
    let dir_text = dir.display().to_string();
    let exists = context
        .git()
        .read(
            &main.path,
            &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")],
        )
        .is_ok_and(|out| !out.trim().is_empty());
    let args: Vec<&str> = if exists {
        vec!["worktree", "add", &dir_text, &branch]
    } else {
        vec!["worktree", "add", "-b", &branch, &dir_text, "HEAD"]
    };
    context
        .git()
        .run(&main.path, Mode::Write, &args, &[], None)?
        .checked("worktree add")?;
    let location = std::fs::canonicalize(&dir).unwrap_or(dir);
    let label = Some(text(parameters, "label"))
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| format!("worktree of {} ({branch})", main.label));
    let mut row = context.facade.make_value(ROOT_ENTITY)?;
    row.set_sequenced_id_primary(&context.facade)?;
    row.set("location", Value::string(location.display().to_string()));
    row.set("label", Value::string(&label));
    row.set("worktreeOf", Value::string(main.path.display().to_string()));
    row.set("worktreeBranch", Value::string(&branch));
    row.set(
        "worktreeUserId",
        Value::string(moqui_core::user::current_user_id().ok().flatten().unwrap_or_default()),
    );
    if !conversation.is_empty() {
        row.set("conversationId", Value::string(&conversation));
    }
    row.create(&context.facade)?;
    audit_git(
        &context,
        main,
        "worktree-add",
        json!({ "location": location.display().to_string(), "branch": branch, "conversationId": conversation }),
    );
    let roots = crate::roots::roots(&context.factory, &context.settings, Some(&context.facade));
    let made = roots.iter().find(|root| root.path == location);
    let mut out = Parameters::new();
    out.insert(
        "rootId".into(),
        made.map_or(Value::Null, |root| Value::string(&root.id)),
    );
    out.insert("location".into(), Value::string(location.display().to_string()));
    out.insert("branch".into(), Value::string(branch));
    out.insert("note".into(), Value::string(NOT_LOADED));
    Ok(out)
}

/// `remove#Worktree`: a worktree root removed (`git worktree remove`) with its row, refused while it has uncommitted
/// changes unless `discardChanges`; its branch stays, with its commits.
fn remove_worktree(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = context.root(&text(parameters, "rootId"))?;
    let Some(main_id) = root.worktree_of.as_deref() else {
        return Err(LibError::new(format!("{} is not a worktree of a session", root.label)));
    };
    let main = context.root(main_id)?;
    let discard = flag(parameters, "discardChanges");
    let porcelain = context.git().read(&root.path, &["status", "--porcelain"])?;
    if let Some(why) = removal_refusal(&root.label, &porcelain, discard) {
        return Err(LibError::new(why));
    }
    let branch = context
        .git()
        .read(&root.path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|out| out.trim().to_string())
        .unwrap_or_default();
    let location = root.path.display().to_string();
    let mut args = vec!["worktree", "remove"];
    if discard {
        args.push("--force");
    }
    args.push(&location);
    context
        .git()
        .run(&main.path, Mode::Write, &args, &[], None)?
        .checked("worktree remove")?;
    if let Some(row) = crate::roots::root_row(&context.facade, &root.path) {
        row.delete(&context.facade)?;
    }
    audit_git(
        &context,
        main,
        "worktree-remove",
        json!({ "location": location, "branch": branch, "discarded": discard && !porcelain.trim().is_empty() }),
    );
    let mut out = Parameters::new();
    out.insert("branch".into(), Value::string(&branch));
    out.insert(
        "message".into(),
        Value::string(format!(
            "Removed {}; the branch {branch} stays in {}",
            root.label, main.label
        )),
    );
    Ok(out)
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
