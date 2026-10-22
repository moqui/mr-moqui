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

//! The A2A delegate tools (plans/LLM-A2A-Assist.md 4.6, 4.15 "Client"; docs/A2A.md): `a2a__<remote>`, one for each
//! `AgentRemoteA2a` row that names the run's agent in `allowedAgents`, in the lazy catalog ([`super::catalog`]), so
//! the model finds them with `find_tools` and calls them through `run_tool`.
//!
//! A delegation is EXTERNAL: it sends what the model wrote to another system and starts work there, so it asks for
//! the user's approval as a write does, with the message as its arguments, and runs only once approved (or by a
//! background agent's policy). It runs with the A2A client ([`crate::llm::a2a::client`]) under the SSRF limits of
//! [`crate::llm::remote`], never in a sim and never under the read fence. The remote's answer (its artifacts, else its
//! status message) is what the model reads, wrapped as untrusted text (`<untrusted from="a2a:<remote>">`); a large one
//! is spilled as any result is. A remote task that asks for input (`TASK_STATE_INPUT_REQUIRED`) pauses the local run
//! with the remote's question to the user; one that needs a sign-in or an approval at a link
//! (`TASK_STATE_AUTH_REQUIRED`) pauses it AuthRequired with the remote's link and Done or Cancel. The answer goes to the
//! same remote task; Cancel cancels it. A second delegation to the same remote in a run goes on in its remote context.
//! The waiting state is an `AgentRemoteTask` row.
//!
//! The helpers the MCP client tools ([`super::mcp_client`]) share are here too: the approval of a remote call, what
//! a resumed call goes on with, and the question that pauses a run for its remote.

use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::catalog::{Entry, Target};
use super::format::ToolError;
use super::schema::{object, Property};
use super::{AgentTool, Classification, Pause, PauseKind, ToolInput, ToolOutcome, ToolPolicy, ToolResult, ToolSpec};
use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::a2a::client::{self, Card, Outgoing, Remote, Stopped, TaskView};
use crate::llm::approval::{self, ApprovalRequest, ApprovalView, ChangeRow, PreviewMode, Requested, TargetKind};
use crate::llm::remote::{self, Keep, Waiting};
use crate::llm::run::context::TurnContext;
use crate::llm::text_of;

/// The prefix of a delegate tool's name.
pub const PREFIX: &str = "a2a__";
/// The longest message a delegation sends.
const MESSAGE_MAX: usize = 16_000;
const QUESTION: &str = "moqui.agent.AgentQuestion";

/// The states of an `AgentRemoteTask` that wait on the user.
pub(crate) const ASKED: &str = "input-required";
pub(crate) const LINKED: &str = "auth-required";

fn facade() -> Result<EntityFacade, ToolError> {
    ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| ToolError::new("unavailable", "Remote agents are not available on this server."))
}

fn llm_conf() -> Option<crate::llm::conf::LlmConf> {
    ExecutionContext::with_current(|ec| crate::llm::facade(ec.factory()).conf().clone())
}

/// The remote rows, sorted by id.
fn remote_rows(facade: &EntityFacade) -> Vec<EntityValue> {
    crate::llm::run::quietly(|| {
        facade
            .find(remote::REMOTE_A2A)?
            .order_by("remoteId", false)
            .use_cache(true)
            .disable_authz()
            .list()
    })
    .map(|rows| rows.values().to_vec())
    .unwrap_or_else(|error| {
        tracing::error!("the remote agents could not be read, so none is offered: {error}");
        Vec::new()
    })
}

/// The tool names of the remote rows, by remote id, in order: the same names [`entries`] gives.
pub fn names(facade: &EntityFacade) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for row in remote_rows(facade) {
        let id = text_of(&row, "remoteId").unwrap_or_default();
        let name = remote::tool_name(PREFIX, &[&id], &|name| out.iter().any(|(known, _)| known == name));
        out.push((name, id));
    }
    out
}

/// The delegate tools of the catalog of a run of `agent_id`: one for each remote that is not disabled, names the agent
/// in `allowedAgents` and can be called (a row that cannot is logged and left out); each marked with whether `policy`
/// offers it as EXTERNAL.
pub fn entries(facade: &EntityFacade, agent_id: &str, policy: &ToolPolicy) -> Vec<Entry> {
    let Some(conf) = llm_conf() else {
        return Vec::new();
    };
    if !conf.a2a.client_enabled {
        return Vec::new();
    }
    let rows = remote_rows(facade);
    let names = names(facade);
    let mut out = Vec::new();
    for row in rows {
        let id = text_of(&row, "remoteId").unwrap_or_default();
        if text_of(&row, "disabled").as_deref() == Some("Y")
            || !remote::agent_allowed(text_of(&row, "allowedAgents").as_deref(), agent_id)
        {
            continue;
        }
        let remote = match Remote::of(&row, Duration::from_secs(conf.a2a.client_timeout.max(1))) {
            Ok(remote) => remote,
            Err(why) => {
                tracing::error!("the remote agent {id} is left out of the catalog: {why}");
                continue;
            }
        };
        let Some((name, _)) = names.iter().find(|(_, known)| *known == id) else {
            continue;
        };
        let tool = DelegateTool::new(name, remote);
        let refusal = super::catalog::policy_refusal(policy, name, Classification::External);
        out.push(Entry {
            name: name.clone(),
            title: tool.0.title.clone(),
            description: tool.0.description.clone(),
            classification: Classification::External,
            allowed: refusal.is_none(),
            refusal,
            target: Target::AddOn(Arc::new(tool)),
        });
    }
    out
}

/// A delegate tool: its spec and its remote.
pub struct DelegateTool(ToolSpec, Remote);

impl DelegateTool {
    fn new(name: &str, remote: Remote) -> Self {
        let card_text = remote
            .card
            .as_ref()
            .and_then(|card| card.get("description"))
            .and_then(Json::as_str)
            .map(|text| crate::llm::prompt::one_line(text, 300))
            .filter(|text| !text.is_empty());
        let description = match (&remote.description, card_text) {
            (Some(admin), _) => crate::llm::prompt::one_line(admin, 400),
            (None, Some(card)) => format!(
                "Delegate a task to the remote agent {}. Its card says: {}",
                remote.name,
                crate::llm::prompt::untrusted(&format!("a2a:{}", remote.remote_id), &card)
            ),
            (None, None) => format!("Delegate a task to the remote agent {}.", remote.name),
        };
        let mut spec = ToolSpec::new(
            name,
            &description,
            object(&[Property::required(
                "message",
                "string",
                "What to ask it, with everything it needs: it sees nothing else of this conversation",
            )]),
            Classification::External,
        );
        spec.title = remote.name.clone();
        spec.lazy = true;
        spec.may_pause = true;
        Self(spec, remote)
    }

    fn target_name(&self) -> String {
        format!("a2a:{}", self.1.remote_id)
    }
}

impl AgentTool for DelegateTool {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        let message = args
            .get("message")
            .and_then(Json::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if message.is_empty() {
            return ToolError::new(
                "arguments",
                format!("Call {} with message, what to ask the remote agent.", self.0.name),
            )
            .outcome();
        }
        if message.chars().count() > MESSAGE_MAX {
            return ToolError::new(
                "arguments",
                format!("The message is longer than {MESSAGE_MAX} characters; send what the remote agent needs."),
            )
            .outcome();
        }
        let request = RemoteRequest {
            target_name: self.target_name(),
            classification: Classification::External,
            args: json!({"message": message}),
            summary: format!(
                "Send to the remote agent {}: {}",
                self.1.name,
                crate::llm::prompt::one_line(&message, 160)
            ),
            snapshot: json!({"remote": self.1.remote_id, "origin": self.1.address.origin}),
        };
        match request_approval(ctx, &request) {
            Ok(view) => self.execute(ctx, &view),
            Err(outcome) => outcome,
        }
    }

    fn resume(&self, ctx: &mut TurnContext<'_>, _args: Json, input: &ToolInput) -> ToolOutcome {
        match resumed(ctx) {
            Resume::Waiting(waiting) => self.go_on(ctx, &waiting, input),
            Resume::Approved(view) => self.execute(ctx, &view),
            Resume::Outcome(outcome) => outcome,
        }
    }
}

impl DelegateTool {
    /// Run an approved delegation: still allowed and to the same origin, the approval begun with the arguments hashed,
    /// then sent and followed.
    fn execute(&self, ctx: &mut TurnContext<'_>, view: &ApprovalView) -> ToolOutcome {
        if let Err(outcome) = still_allowed(ctx, view, &self.1.address.origin, &self.0.name) {
            return outcome;
        }
        let grant = match approval::begin(ctx, &view.approval_id, &view.args) {
            Ok(grant) => grant,
            Err(why) => return ToolOutcome::text(why),
        };
        let message = view.args.get("message").and_then(Json::as_str).unwrap_or_default().to_string();
        let facade = match facade() {
            Ok(facade) => facade,
            Err(error) => return error.outcome(),
        };
        let card = match client::card(&facade, &self.1) {
            Ok(card) => card,
            Err(why) => {
                approval::finish(ctx, &grant.approval_id, false, &json!({"error": why}));
                return ToolOutcome::Failed(format!(
                    "The user approved the delegation to {}, but it could not be reached, so nothing was sent: {why}",
                    self.1.name
                ));
            }
        };
        let context_id = ctx
            .run
            .and_then(|run| crate::llm::run::quietly(|| remote::context_of(&facade, &run.run_id, &self.1.remote_id)).ok())
            .flatten();
        let deadline = Instant::now() + self.1.timeout;
        let cancel = ctx.cancel.clone();
        let sent = client::send(
            &self.1,
            &card,
            &Outgoing {
                text: message,
                context_id,
                task_id: None,
            },
            deadline,
            &|| cancel.is_canceled(),
        );
        self.settle(ctx, &facade, &card, &grant.approval_id, None, sent)
    }

    /// The user answered the remote's question or came back from its link: the answer goes to the same task (or the
    /// task is read again), and Cancel cancels it.
    fn go_on(&self, ctx: &mut TurnContext<'_>, waiting: &Waiting, input: &ToolInput) -> ToolOutcome {
        let answered = answer_of(ctx, input);
        let approval_id = waiting.approval_id.clone().unwrap_or_default();
        let facade = match facade() {
            Ok(facade) => facade,
            Err(error) => return error.outcome(),
        };
        if let Some(refused) = origin_moved(ctx, &facade, waiting, &self.1.address.origin, &self.0.name) {
            return refused;
        }
        let card = match client::card(&facade, &self.1) {
            Ok(card) => card,
            Err(why) => {
                approval::finish(ctx, &approval_id, false, &json!({"error": why}));
                return ToolOutcome::Failed(format!("{} could not be reached again: {why}", self.1.name));
            }
        };
        let task_id = waiting.remote_task_ref.clone().unwrap_or_default();
        if answered.cancel {
            client::cancel_task(&self.1, &card, &task_id);
            keep_state(ctx, &facade, waiting, "canceled", None);
            approval::finish(ctx, &approval_id, false, &json!({"canceled": true}));
            return ToolOutcome::text(format!(
                "The user canceled the delegation to {}; the remote task was canceled.",
                self.1.name
            ));
        }
        let deadline = Instant::now() + self.1.timeout;
        let cancel = ctx.cancel.clone();
        let canceled = || cancel.is_canceled();
        let followed = if waiting.remote_state.as_deref() == Some(ASKED) {
            client::send(
                &self.1,
                &card,
                &Outgoing {
                    text: answered.text,
                    context_id: waiting.remote_context_id.clone(),
                    task_id: Some(task_id.clone()),
                },
                deadline,
                &canceled,
            )
        } else {
            // back from the link: the task as it stands, followed while it works
            client::get_task(&self.1, &card, &task_id, Duration::from_secs(30))
                .map_err(Stopped::Failed)
                .and_then(|task| client::follow(&self.1, &card, task, deadline, &canceled))
        };
        self.settle(ctx, &facade, &card, &approval_id, Some(waiting), followed)
    }

    /// What a delegation came to: an interrupted task pauses the run for the user (kept as an `AgentRemoteTask`); an
    /// ended one is the answer the model reads, and the approval's outcome.
    fn settle(
        &self,
        ctx: &mut TurnContext<'_>,
        facade: &EntityFacade,
        card: &Card,
        approval_id: &str,
        waiting: Option<&Waiting>,
        sent: Result<TaskView, Stopped>,
    ) -> ToolOutcome {
        let name = &self.1.name;
        let task = match sent {
            Ok(task) => task,
            Err(stopped) => {
                let why = match stopped {
                    Stopped::Failed(why) => why,
                    Stopped::Canceled => "the run was canceled; the remote task was asked to cancel".to_string(),
                    Stopped::TimedOut => format!(
                        "it did not finish in {} s, so the remote task was canceled",
                        self.1.timeout.as_secs()
                    ),
                };
                if let Some(waiting) = waiting {
                    keep_state(ctx, facade, waiting, "failed", None);
                }
                approval::finish(ctx, approval_id, false, &json!({"error": why}));
                return ToolOutcome::Failed(format!("The delegation to {name} did not complete: {why}"));
            }
        };
        // a task that waits keeps the origin it was approved for, and the link it gave
        let waiting_state = task
            .interrupted()
            .then(|| waiting_json(&self.1.address.origin, task.link(&self.1.address).as_deref(), json!({})));
        let kept = Keep {
            run_id: ctx.run.map(|run| run.run_id.as_str()).unwrap_or_default(),
            tool_call_id: &ctx.call.tool_call_id,
            remote_id: Some(&self.1.remote_id),
            approval_id: Some(approval_id).filter(|id| !id.is_empty()),
            task_ref: Some(task.id.as_str()).filter(|id| !id.is_empty()),
            context_id: Some(task.context_id.as_str()).filter(|id| !id.is_empty()),
            state: if task.interrupted() {
                if task.state == "TASK_STATE_INPUT_REQUIRED" {
                    ASKED
                } else {
                    LINKED
                }
            } else if task.state.is_empty() {
                "message"
            } else {
                task.state.as_str()
            },
            request_state: waiting_state.as_deref(),
            ..Keep::default()
        };
        let mut kept_id = waiting.map(|waiting| waiting.remote_task_id.clone());
        if ctx.run.is_some() {
            let node = ctx.events.node().to_string();
            match remote::keep(facade, &node, kept_id.as_deref(), &kept) {
                Ok(id) => kept_id = Some(id),
                Err(error) => tracing::error!("the remote task of {} could not be kept: {error}", ctx.call.tool_call_id),
            }
        }
        let from = format!("a2a:{}", self.1.remote_id);
        if task.interrupted() {
            let ask = if task.state == "TASK_STATE_INPUT_REQUIRED" {
                AskUser::Question {
                    text: crate::llm::prompt::one_line(
                        &if task.status_text.is_empty() {
                            task.answer()
                        } else {
                            task.status_text.clone()
                        },
                        2000,
                    ),
                }
            } else {
                AskUser::Link {
                    url: task.link(&self.1.address),
                    message: crate::llm::prompt::one_line(&task.status_text, 600),
                }
            };
            return match ask_user(ctx, name, ask) {
                Ok(pause) => ToolOutcome::Pause(pause),
                Err(asked) => {
                    // nobody can answer here: the remote task is not left waiting
                    client::cancel_task(&self.1, card, &task.id);
                    if let Some(id) = kept_id {
                        let canceled = Waiting {
                            remote_task_id: id,
                            ..Waiting::default()
                        };
                        keep_state(ctx, facade, &canceled, "canceled", None);
                    }
                    approval::finish(ctx, approval_id, false, &json!({"error": "nobody could answer the remote"}));
                    ToolOutcome::Failed(format!(
                        "{name} needs the user ({}), and nobody can answer here, so its task was canceled. What it said: {}",
                        task.state,
                        crate::llm::prompt::untrusted(&from, &asked)
                    ))
                }
            };
        }
        let ok = task.state.is_empty() || task.state == "TASK_STATE_COMPLETED";
        approval::finish(
            ctx,
            approval_id,
            ok,
            &json!({"state": task.state, "taskId": task.id, "contextId": task.context_id}),
        );
        let answer = task.answer();
        let state = task.state.strip_prefix("TASK_STATE_").unwrap_or("answered").to_ascii_lowercase();
        let text = if ok {
            format!(
                "{name} answered:\n{}",
                crate::llm::prompt::untrusted(&from, if answer.is_empty() { "(nothing)" } else { &answer })
            )
        } else {
            format!(
                "The remote task of {name} ended {state}:\n{}",
                crate::llm::prompt::untrusted(&from, if answer.is_empty() { "(no reason given)" } else { &answer })
            )
        };
        let data = json!({"remote": self.1.remote_id, "taskId": task.id, "contextId": task.context_id,
            "state": task.state, "approvalId": approval_id});
        if ok {
            ToolOutcome::Done(ToolResult { text, data: Some(data) })
        } else {
            ToolOutcome::Failed(text)
        }
    }
}

// ---- shared with the MCP client tools ----

/// The approval a remote call asks for.
pub(crate) struct RemoteRequest {
    /// `a2a:<remote>` or `mcp:<server>#<tool>`.
    pub target_name: String,
    pub classification: Classification,
    pub args: Json,
    pub summary: String,
    /// What the approval was asked against: the remote and its origin, checked again when it runs.
    pub snapshot: Json,
}

fn plain(value: &Json) -> String {
    match value {
        Json::Null => String::new(),
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Ask for the approval of a remote call: its arguments as the rows of the card (secret-named ones left out), no sim
/// preview (a remote call has none). `Ok` is an approval a background agent's policy gave, to run now; `Err` the
/// pause, or the refusal the model reads.
pub(crate) fn request_approval(ctx: &mut TurnContext<'_>, request: &RemoteRequest) -> Result<ApprovalView, ToolOutcome> {
    let changes: Vec<ChangeRow> = request
        .args
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| !super::data::secret_name(name))
        .map(|(name, value)| ChangeRow {
            field: name.clone(),
            label: name.clone(),
            current: String::new(),
            proposed: super::format::cut(&plain(value), 2000),
        })
        .collect();
    let asked = ApprovalRequest {
        target_kind: TargetKind::Tool,
        target_name: request.target_name.clone(),
        classification: request.classification,
        args: request.args.clone(),
        summary: request.summary.clone(),
        changes,
        snapshot: request.snapshot.clone(),
        record_refs: Json::Null,
        preview: PreviewMode::None,
        handoff: None,
    };
    match approval::request(ctx, &asked) {
        Requested::Pause(pause) => Err(ToolOutcome::Pause(pause)),
        Requested::Refused(why) => Err(ToolError::new("not_allowed", why).outcome()),
        Requested::Approved(grant) => {
            let loaded = facade()
                .ok()
                .and_then(|facade| crate::llm::run::quietly(|| approval::load(&facade, &grant.approval_id)).ok())
                .flatten();
            loaded.ok_or_else(|| ToolOutcome::Failed("the approval could not be read, so nothing ran".to_string()))
        }
    }
}

/// An approved remote call that may not run now: the agent's policy no longer offers it, or the remote's address moved
/// since the user approved it. The approval fails (through Executing) and nothing goes out.
pub(crate) fn still_allowed(
    ctx: &mut TurnContext<'_>,
    view: &ApprovalView,
    origin: &str,
    name: &str,
) -> Result<(), ToolOutcome> {
    let asked = view
        .snapshot
        .get("record")
        .and_then(|record| record.get("origin"))
        .and_then(Json::as_str)
        .unwrap_or_default();
    let why = if !ctx.approvals.write_rule.offered {
        Some("this agent may no longer make that call".to_string())
    } else if asked != origin {
        Some("the remote's address was changed by an admin after the user approved it".to_string())
    } else {
        None
    };
    let Some(why) = why else {
        return Ok(());
    };
    if let Ok(grant) = approval::begin(ctx, &view.approval_id, &view.args) {
        approval::finish(ctx, &grant.approval_id, false, &json!({"error": why}));
    }
    Err(ToolError::new(
        "not_allowed",
        format!("The user approved {name}, but {why}, so nothing was sent. Tell the user."),
    )
    .outcome())
}

/// What a resumed remote call goes on with.
pub(crate) enum Resume {
    /// It waits on its remote: the user answered the question or came back from the link.
    Waiting(Waiting),
    /// Its approval was given: it runs now.
    Approved(Box<ApprovalView>),
    /// Anything else: still pending, rejected, expired, done.
    Outcome(ToolOutcome),
}

/// What the paused call of `ctx` resumes with: its remote's waiting state when it has one, else its approval.
pub(crate) fn resumed(ctx: &TurnContext<'_>) -> Resume {
    if let (Some(run), Ok(facade)) = (ctx.run, facade()) {
        match crate::llm::run::quietly(|| remote::waiting_of(&facade, &run.run_id, &ctx.call.tool_call_id)) {
            Ok(Some(waiting)) if matches!(waiting.remote_state.as_deref(), Some(ASKED | LINKED)) => {
                return Resume::Waiting(waiting)
            }
            Ok(_) => {}
            Err(error) => return Resume::Outcome(ToolOutcome::Failed(format!("the remote call could not be read: {error}"))),
        }
    }
    let view = match approval::for_call(ctx) {
        Ok(Some(view)) => view,
        Ok(None) => return Resume::Outcome(ToolOutcome::Failed("this call has no approval to go on with".to_string())),
        Err(error) => return Resume::Outcome(ToolOutcome::Failed(format!("the approval could not be read: {error}"))),
    };
    match approval::resumed(&view) {
        Ok(approval::Resumed::Approved) => Resume::Approved(Box::new(view)),
        Ok(approval::Resumed::Waiting(pause)) => Resume::Outcome(ToolOutcome::Pause(pause)),
        Ok(approval::Resumed::Done(text) | approval::Resumed::HandedOff(text)) => Resume::Outcome(ToolOutcome::text(text)),
        Err(error) => Resume::Outcome(ToolOutcome::Failed(format!("the approval could not be read: {error}"))),
    }
}

/// Update a waiting state's row (a failure is logged).
pub(crate) fn keep_state(ctx: &TurnContext<'_>, facade: &EntityFacade, waiting: &Waiting, state: &str, request_state: Option<&str>) {
    let Some(run) = ctx.run else {
        return;
    };
    let node = ctx.events.node().to_string();
    let kept = remote::keep(
        facade,
        &node,
        Some(&waiting.remote_task_id),
        &Keep {
            run_id: &run.run_id,
            tool_call_id: &ctx.call.tool_call_id,
            state,
            request_state,
            ..Keep::default()
        },
    );
    if let Err(error) = kept {
        tracing::error!("the remote state of {} could not be kept: {error}", ctx.call.tool_call_id);
    }
}

/// The `requestState` of a waiting `AgentRemoteTask`: the origin the call was approved for, the link the remote gave and
/// what else the call keeps (an MCP `requestState` and its elicitation's key).
pub(crate) fn waiting_json(origin: &str, link: Option<&str>, more: Json) -> String {
    let mut state = json!({"origin": origin, "link": link});
    if let (Some(state), Json::Object(more)) = (state.as_object_mut(), more) {
        state.extend(more);
    }
    state.to_string()
}

/// The waiting state of a row, as [`waiting_json`] wrote it.
pub(crate) fn waiting_state(waiting: &Waiting) -> Json {
    waiting
        .request_state
        .as_deref()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Json::Null)
}

/// A waiting call whose remote moved: its row's origin is not the one the call waits on (an admin changed the URL while
/// the user was away), so it goes on nowhere: the row and the approval fail, and nothing is sent.
pub(crate) fn origin_moved(
    ctx: &mut TurnContext<'_>,
    facade: &EntityFacade,
    waiting: &Waiting,
    origin: &str,
    name: &str,
) -> Option<ToolOutcome> {
    if waiting_state(waiting).get("origin").and_then(Json::as_str) == Some(origin) {
        return None;
    }
    keep_state(ctx, facade, waiting, "failed", None);
    if let Some(id) = &waiting.approval_id {
        approval::finish(ctx, id, false, &json!({"error": "the remote's address changed while the call waited"}));
    }
    Some(
        ToolError::new(
            "not_allowed",
            format!(
                "{name} waited for the user, but an admin changed the remote's address meanwhile, so nothing was sent. \
                 Tell the user."
            ),
        )
        .outcome(),
    )
}

/// What a run pauses for when its remote waits on the user.
pub(crate) enum AskUser {
    /// The remote asked a question (A2A input-required): the user's answer goes back to it.
    Question { text: String },
    /// The remote needs the user at a link (A2A auth-required, an MCP URL elicitation): Done or Cancel.
    Link { url: Option<String>, message: String },
}

/// The user's answer to [`ask_user`]'s question.
pub(crate) struct Answered {
    pub cancel: bool,
    pub text: String,
}

/// Pause the run with a question for the user about its remote (an `ask` question, stored as `ask` stores it): the
/// remote's question with a field for the answer, or its link with Done and Cancel, which makes the run AuthRequired.
/// The remote's own text is shown to the user, not read by the model.
///
/// # Errors
///
/// Nobody can answer here (a job, or a call with no run): the remote's text, for the model.
pub(crate) fn ask_user(ctx: &mut TurnContext<'_>, remote_name: &str, ask: AskUser) -> Result<Pause, String> {
    let (args, kind, link, said) = match ask {
        AskUser::Question { text } => {
            let said = if text.is_empty() { "a question".to_string() } else { text };
            (
                json!({"question": format!("{remote_name} asks: {said}"),
                    "fields": [{"name": "answer", "label": "Your answer", "type": "textarea", "required": true}]}),
                PauseKind::Question,
                None,
                said,
            )
        }
        AskUser::Link { url, message } => {
            let what = if message.is_empty() {
                "needs you to sign in or approve there.".to_string()
            } else {
                message.clone()
            };
            // the link's host on a line of its own, said to be the remote's, so the user sees where it goes
            let question = match &url {
                Some(url) => format!(
                    "{remote_name}: {what}\nThe link is the remote's own, at {}:\n{url}\nOpen it, then choose Done (or Cancel).",
                    remote::link_host(url)
                ),
                None => format!("{remote_name}: {what}\nChoose Done when you have (or Cancel)."),
            };
            (
                json!({"question": question, "options": ["Done", "Cancel"]}),
                PauseKind::Auth,
                url,
                message,
            )
        }
    };
    if ctx.run.is_none() || !ctx.caps.interactive() {
        return Err(said);
    }
    match super::ask::Ask::default().call(ctx, args) {
        ToolOutcome::Pause(mut pause) => {
            pause.kind = kind;
            if let Some((_, payload)) = pause.event.as_mut() {
                payload["remote"] = json!(remote_name);
                if let Some(url) = &link {
                    payload["link"] = json!(url);
                }
            }
            Ok(pause)
        }
        ToolOutcome::Failed(text) | ToolOutcome::Done(ToolResult { text, .. }) => Err(format!("{said} ({text})")),
    }
}

/// The user's answer to a remote's question, and the question marked answered: Cancel (by choice or as the text) is a
/// cancel; else the form's `answer`, the choice or the text typed.
pub(crate) fn answer_of(ctx: &mut TurnContext<'_>, input: &ToolInput) -> Answered {
    let data = input.data.clone().unwrap_or(Json::Null);
    let choice = data
        .get("choice")
        .and_then(Json::as_str)
        .map(str::to_string)
        .or_else(|| {
            data.get("choices")
                .and_then(Json::as_array)
                .and_then(|choices| choices.first())
                .and_then(Json::as_str)
                .map(str::to_string)
        });
    let value = data
        .get("values")
        .and_then(|values| values.get("answer"))
        .or_else(|| data.get("answer"))
        .and_then(Json::as_str)
        .map(str::to_string);
    let text = value
        .or_else(|| choice.clone())
        .unwrap_or_else(|| input.text.trim().to_string());
    let cancel = choice.as_deref().is_some_and(|choice| choice.eq_ignore_ascii_case("cancel"))
        || text.trim().eq_ignore_ascii_case("cancel");
    if let (Some(run), Ok(facade)) = (ctx.run, facade()) {
        let _lifted = crate::entity::fence::lift();
        let node = ctx.events.node().to_string();
        let answer = json!({"text": text, "cancel": cancel});
        let answered = crate::llm::run::quietly(|| {
            crate::llm::run::in_tx(&facade, || {
                crate::llm::run::held(&facade, &run.run_id, &node, crate::llm::run::Hold::Lease)?;
                facade
                    .find(QUESTION)?
                    .eq("runId", Value::string(&run.run_id))
                    .eq("toolCallId", Value::string(&ctx.call.tool_call_id))
                    .eq("statusId", Value::string("AgqOpen"))
                    .disable_authz()
                    .update_all([
                        ("statusId".to_string(), Value::string("AgqAnswered")),
                        ("answerJson".to_string(), Value::string(answer.to_string())),
                        ("answeredDate".to_string(), crate::llm::run::stamp(crate::llm::run::now_millis())),
                    ])
            })
        });
        if let Err(error) = answered {
            tracing::error!("the answer of agent run {} could not be stored: {error}", run.run_id);
        }
        ctx.events.emit(
            "question.answered",
            json!({"toolCallId": ctx.call.tool_call_id, "answer": answer}),
        );
    }
    Answered { cancel, text }
}
