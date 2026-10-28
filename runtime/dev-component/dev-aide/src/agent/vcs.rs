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

//! `vcs` (plans/design-dev-aide.md 13.2): a root's git status, a file's diff and the log (reads); a commit of named
//! files with its message (a write the user approves, the card naming the files); and a push (EXTERNAL, the card naming
//! the remote and the commits). For the extend tier the push is Publish of the site component. Each runs the dev-aide's
//! own VcsServices as the user, so the git layer's rules (argv, no prompt, the author, the audit) hold.

use std::collections::BTreeMap;

use moqui_core::llm::approval::{self, ApprovalRequest, ChangeRow, Requested, Resumed, TargetKind};
use moqui_core::llm::run::context::TurnContext;
use moqui_core::llm::tools::format::ToolError;
use moqui_core::llm::tools::schema::{object, Property};
use moqui_core::llm::tools::{caps, AgentTool, Classification, ToolInput, ToolOutcome, ToolSpec};
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::{arg, context, list_arg};
use crate::tier::Tier;

const VCS: &str = "org.moqui.devaide.VcsServices.";

pub struct VcsTool(ToolSpec);

impl Default for VcsTool {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "vcs",
            "Git in the Workspace's roots: status, diff (a file's), log; commit named files with a summary (the user \
             approves); push the current branch (the user approves; for a site extender this is Publish).",
            object(&[
                Property::required("action", "string", "status, diff, log, commit or push"),
                Property::required("root", "string", "The root's id (a repository: my-site, mr-mantle ...)"),
                Property::optional("path", "string", "diff, log: a path in the root"),
                Property::optional("files", "array", "commit: the paths to commit"),
                Property::optional("summary", "string", "commit: the message's first line"),
                Property::optional("body", "string", "commit: the message's body"),
            ]),
            Classification::Read,
        )
        .needs(&[caps::DEV]);
        spec.may_pause = true;
        spec.lazy = true;
        Self(spec)
    }
}

impl AgentTool for VcsTool {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        match run(ctx, &args) {
            Ok(outcome) => outcome,
            Err(error) => error.outcome(),
        }
    }

    fn resume(&self, ctx: &mut TurnContext<'_>, _args: Json, _input: &ToolInput) -> ToolOutcome {
        let view = match approval::for_call(ctx) {
            Ok(Some(view)) => view,
            Ok(None) => return ToolOutcome::Failed("the approval is not there".into()),
            Err(error) => return ToolOutcome::Failed(error.to_string()),
        };
        match approval::resumed(&view) {
            Ok(Resumed::Approved) => approved(ctx, &view.approval_id, &view.args),
            Ok(Resumed::Done(text) | Resumed::HandedOff(text)) => ToolOutcome::text(text),
            Ok(Resumed::Waiting(pause)) => ToolOutcome::Pause(pause),
            Err(error) => ToolOutcome::Failed(error.to_string()),
        }
    }
}

/// Call a VcsServices service as the user, answering its out-parameters or its errors.
fn service(name: &str, parameters: &[(&str, String)]) -> Result<Json, ToolError> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::string(value)))
        .collect();
    let out = moqui_core::service::sync(&format!("{VCS}{name}"), &parameters)
        .map_err(|err| ToolError::new("git", err.to_string()))?;
    let errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    if !errors.is_empty() {
        return Err(ToolError::new("git", errors.join("; ")));
    }
    Ok(Json::Object(
        out.iter()
            .map(|(key, value)| (key.clone(), moqui_lib::json::to_json(value)))
            .collect(),
    ))
}

fn run(ctx: &mut TurnContext<'_>, args: &Json) -> Result<ToolOutcome, ToolError> {
    let context = context(Tier::View)?;
    let root = arg(args, "root")
        .ok_or_else(|| ToolError::new("no_root", "Name the root."))?
        .to_string();
    context
        .root(&root)
        .map_err(|err| ToolError::new("no_root", err.to_string()))?;
    let path = arg(args, "path").unwrap_or("").to_string();
    match arg(args, "action").unwrap_or("status") {
        "status" => Ok(super::json(&service("get#Status", &[("rootId", root)])?)),
        "diff" => Ok(super::json(&service("get#Diff", &[("rootId", root), ("path", path)])?)),
        "log" => Ok(super::json(&service("get#Log", &[("rootId", root), ("path", path)])?)),
        "commit" => {
            let files = list_arg(args, "files");
            let summary = arg(args, "summary").unwrap_or("").to_string();
            if files.is_empty() || summary.is_empty() {
                return Err(ToolError::new(
                    "bad_arguments",
                    "A commit names its files and a summary.",
                ));
            }
            let request = ApprovalRequest {
                target_kind: TargetKind::Tool,
                target_name: format!("git commit in {root}"),
                classification: Classification::Write,
                args: json!({"action": "commit", "root": root, "files": files, "summary": summary,
                    "body": arg(args, "body").unwrap_or("")}),
                summary: format!("Commit {} files in {root}: {summary}", files.len()),
                changes: files
                    .iter()
                    .map(|file| ChangeRow {
                        field: file.clone(),
                        label: file.clone(),
                        current: String::new(),
                        proposed: "commit".into(),
                    })
                    .collect(),
                ..ApprovalRequest::default()
            };
            Ok(ask(ctx, &request))
        }
        "push" => {
            let status = service("get#Status", &[("rootId", root.clone())])?;
            let branch = status["branch"].as_str().unwrap_or("").to_string();
            let ahead = status["ahead"].as_i64().unwrap_or(0);
            let publish = context.tier == Tier::Extend;
            let what = if publish {
                format!("Publish the site's changes in {root} (commit them and push)")
            } else {
                format!(
                    "Push {ahead} commits of {branch} in {root} to {}",
                    status["upstream"].as_str().unwrap_or("its remote")
                )
            };
            let request = ApprovalRequest {
                target_kind: TargetKind::Tool,
                target_name: format!("git push in {root}"),
                classification: Classification::External,
                args: json!({"action": if publish { "publish" } else { "push" }, "root": root}),
                summary: what,
                ..ApprovalRequest::default()
            };
            Ok(ask(ctx, &request))
        }
        other => Err(ToolError::new(
            "bad_action",
            format!("{other}: the action is status, diff, log, commit or push."),
        )),
    }
}

fn ask(ctx: &mut TurnContext<'_>, request: &ApprovalRequest) -> ToolOutcome {
    match approval::request(ctx, request) {
        Requested::Pause(pause) => ToolOutcome::Pause(pause),
        Requested::Approved(grant) => approved(ctx, &grant.approval_id, &request.args),
        Requested::Refused(why) => ToolOutcome::Failed(why),
    }
}

fn approved(ctx: &mut TurnContext<'_>, approval_id: &str, args: &Json) -> ToolOutcome {
    if let Err(why) = approval::begin(ctx, approval_id, args) {
        return ToolOutcome::text(why);
    }
    let _lifted = moqui_core::entity::fence::lift();
    let root = args["root"].as_str().unwrap_or("").to_string();
    let outcome = match args["action"].as_str().unwrap_or("") {
        "commit" => {
            let files: Vec<String> = args["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect();
            service("stage#Files", &[("rootId", root.clone()), ("paths", files.join(","))]).and_then(|_| {
                service(
                    "commit#Changes",
                    &[
                        ("rootId", root.clone()),
                        ("summary", args["summary"].as_str().unwrap_or("").to_string()),
                        ("body", args["body"].as_str().unwrap_or("").to_string()),
                    ],
                )
            })
        }
        "publish" => service("publish#Site", &[("rootId", root.clone())]),
        "push" => service("push#Remote", &[("rootId", root.clone())]),
        other => Err(ToolError::new("bad_action", format!("{other} is not run"))),
    };
    match outcome {
        Ok(result) => {
            approval::finish(ctx, approval_id, true, &result);
            super::json(&result)
        }
        Err(error) => {
            approval::finish(ctx, approval_id, false, &json!({"error": error.instruction}));
            error.outcome()
        }
    }
}

moqui_core::submit_tool!(VcsTool);
