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

//! The A2A operations over the run model, shared by both bindings.
//!
//! - **Mapping**: `contextId` is the conversation, `Task.id` the run, `status.state` the run's state (its
//!   `statusCode`), `status.message` what a paused run waits for (an approval's link, a question with its data part)
//!   or why it failed, `history` the run's user and agent messages (`historyLength`), `artifacts` the `response`
//!   artifact with the answer. A page of tasks is loaded in a fixed number of queries ([`tasks`]).
//! - **Send**: a new message starts a run of the `a2a` surface in a new conversation, or in its `contextId`'s (a
//!   message while that conversation's run is active goes to the run); one with a `taskId` answers the task's question
//!   or steers it while it works. The `messageId` is the client message id, unique with the user, so the same
//!   message again is the task it made (idempotent, with the task as it now stands). A task waiting for its owner's
//!   approval takes no message: the owner decides in Moqui and the task goes on by itself.
//! - **Streams** ([`follow`]) project the run's stored events in sequence: `run.state` is a `statusUpdate`, a stored
//!   `text.chunk` an `artifactUpdate` of the `response` artifact (`append` after the first chunk of a message; the
//!   answer, whole, replaces it with `lastChunk` before the final status; a `step.restart` empties it, since the call
//!   that streamed the text starts again), and the stream ends on a terminal or an
//!   interrupted state. It replays what is stored after its place, then follows the node's hub (with the remote tail
//!   for a run another node works), as the drawer's stream does.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::types::{self, A2aError, Cursor, InMessage, PartLimits, SendConfig};
use super::{Caller, Failure};
use crate::context::ExecutionContextFactory;
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::approval::{self, ApprovalView};
use crate::llm::prompt::Surface;
use crate::llm::run::{self, events, RunRow, RunState};
use crate::llm::tools::PauseKind;
use crate::llm::{int_of, millis_of, text_of};

pub(crate) const RUN: &str = "moqui.agent.AgentRun";
const RUN_EVENT: &str = "moqui.agent.AgentRunEvent";
const CONVERSATION: &str = "moqui.agent.AgentConversation";
const MESSAGE: &str = "moqui.agent.AgentMessage";
const PART: &str = "moqui.agent.AgentMessagePart";
const QUESTION: &str = "moqui.agent.AgentQuestion";
const APPROVAL: &str = "moqui.agent.AgentApproval";
const SURFACE: &str = "AgsfA2a";
/// A stream's keep-alive comment comes this often.
const PING_EVERY: Duration = Duration::from_secs(15);
/// How often a stream reads the run's state, so it ends even when it missed the last event.
const CHECK_EVERY: Duration = Duration::from_secs(5);
/// Stored events read per page of a replay.
const PAGE: i64 = 500;
/// How long CancelTask waits for a working run to stop.
const CANCEL_WAIT: Duration = Duration::from_secs(5);

/// What an operation works with: the factory, its entity facade and the authenticated caller.
pub struct Ctx<'a> {
    pub factory: &'a Arc<ExecutionContextFactory>,
    pub facade: EntityFacade,
    pub caller: &'a Caller,
}

impl<'a> Ctx<'a> {
    /// # Errors
    ///
    /// Returns an internal error when the entity facade is not installed.
    pub fn new(factory: &'a Arc<ExecutionContextFactory>, caller: &'a Caller) -> Result<Self, A2aError> {
        let facade = factory.entity().ok_or_else(A2aError::internal)?;
        Ok(Self {
            factory,
            facade,
            caller,
        })
    }

    fn limits(&self) -> PartLimits {
        let llm = crate::llm::facade(self.factory);
        let conf = &llm.conf().a2a;
        PartLimits {
            max_parts: usize::try_from(conf.max_parts).unwrap_or(32).max(1),
            max_part_bytes: usize::try_from(conf.max_part_bytes).unwrap_or(65536).max(1),
        }
    }
}

/// A run as a task: its row and the fields the wire needs beyond [`RunRow`].
#[derive(Clone, Debug)]
pub struct TaskRow {
    pub run: RunRow,
    pub last_event_seq: i64,
    pub submitted: i64,
    pub ended: Option<i64>,
    pub result_text: Option<String>,
    pub error_code: Option<String>,
}

impl TaskRow {
    pub fn of(row: &EntityValue) -> Self {
        let result_text = text_of(row, "resultJson")
            .and_then(|text| serde_json::from_str::<Json>(&text).ok())
            .and_then(|result| result.get("text").and_then(Json::as_str).map(str::to_string))
            .filter(|text| !text.trim().is_empty());
        Self {
            run: RunRow::of(row),
            last_event_seq: int_of(row, "lastEventSeq").unwrap_or(0),
            submitted: row.get("submittedDate").and_then(millis_of).unwrap_or(0),
            ended: row.get("endedDate").and_then(millis_of),
            result_text,
            error_code: text_of(row, "errorCode"),
        }
    }

    pub fn id(&self) -> &str {
        &self.run.run_id
    }
}

/// The run `run_id`, as a task.
///
/// # Errors
///
/// Returns an error when the row cannot be read.
pub fn load_task(facade: &EntityFacade, run_id: &str) -> Result<Option<TaskRow>, Error> {
    run::quietly(|| {
        Ok(facade
            .find(RUN)?
            .eq("runId", Value::string(run_id))
            .disable_authz()
            .one()?
            .map(|row| TaskRow::of(&row)))
    })
}

/// The caller's task `task_id`: a run of theirs on the `a2a` surface; anyone else's, or one of another surface, is
/// not found.
pub(crate) fn owned(ctx: &Ctx<'_>, task_id: &str) -> Result<TaskRow, Failure> {
    let task = load_task(&ctx.facade, task_id).map_err(|error| Failure::of(&error, "the task lookup"))?;
    task.filter(|task| task.run.user_id == ctx.caller.user_id && task.run.surface == Surface::A2a)
        .ok_or_else(|| Failure::A2a(A2aError::task_not_found(task_id)))
}

/// Whether the run waits for no one here: ended, or paused with no step of it still running on this node.
fn settled(factory: &ExecutionContextFactory, task: &TaskRow) -> bool {
    (task.run.state.is_terminal() || task.run.state.is_paused())
        && run::engine(factory)
            .local_if(task.id())
            .is_none_or(|local| !local.busy.load(Ordering::SeqCst))
}

// ---------- the wire objects of tasks ----------

/// What a page of paused tasks waits for, loaded at once.
#[derive(Default)]
struct Pauses {
    approvals: HashMap<String, ApprovalView>,
    questions: HashMap<String, EntityValue>,
    /// The link of a remote's Auth pause, by its question.
    links: HashMap<String, String>,
}

impl Pauses {
    fn load(facade: &EntityFacade, rows: &[&TaskRow]) -> Result<Self, Error> {
        let mut approval_ids = Vec::new();
        let mut question_ids = Vec::new();
        for task in rows.iter().filter(|task| task.run.state.is_paused()) {
            match (task.run.pause_kind, &task.run.pause_ref) {
                (Some(PauseKind::Approval), Some(reference)) => approval_ids.push(Value::string(reference)),
                (Some(PauseKind::Question | PauseKind::Budget | PauseKind::Auth), Some(reference)) => {
                    question_ids.push(Value::string(reference));
                }
                _ => {}
            }
        }
        let mut pauses = Pauses::default();
        if !approval_ids.is_empty() {
            for row in facade
                .find(APPROVAL)?
                .condition(Condition::is_in("approvalId", approval_ids))
                .disable_authz()
                .list()?
                .values()
            {
                let view = ApprovalView::of(row);
                pauses.approvals.insert(view.approval_id.clone(), view);
            }
        }
        if !question_ids.is_empty() {
            for row in facade
                .find(QUESTION)?
                .condition(Condition::is_in("questionId", question_ids))
                .disable_authz()
                .list()?
                .values()
            {
                let question_id = text_of(row, "questionId").unwrap_or_default();
                // a remote's link: kept on the call's waiting row (13.12)
                if let (Some(run_id), Some(call_id)) = (text_of(row, "runId"), text_of(row, "toolCallId")) {
                    if let Some(link) = crate::llm::remote::waiting_of(facade, &run_id, &call_id)?
                        .and_then(|waiting| crate::llm::tools::delegate::waiting_state(&waiting).get("link").cloned())
                        .and_then(|link| link.as_str().map(str::to_string))
                    {
                        pauses.links.insert(question_id.clone(), link);
                    }
                }
                pauses.questions.insert(question_id, row.clone());
            }
        }
        Ok(pauses)
    }
}

/// The `status.message` of a task: what a paused task waits for, or why it did not finish.
fn status_message(task: &TaskRow, base_url: &str, pauses: &Pauses) -> Option<Json> {
    let (run_id, context_id) = (task.id(), task.run.conversation_id.as_str());
    let agent = |id: String, parts: Vec<Json>| types::message(&id, context_id, run_id, true, parts);
    let reference = task.run.pause_ref.clone().unwrap_or_default();
    match task.run.state {
        RunState::InputRequired | RunState::AuthRequired => match task.run.pause_kind {
            Some(PauseKind::Approval) => {
                let url = approval::link(base_url, context_id);
                let view = pauses.approvals.get(&reference)?;
                let text = format!(
                    "Waiting for the owner's approval in Moqui: {}. The task goes on by itself once they decide at {url}",
                    view.summary()
                );
                Some(agent(
                    format!("{run_id}-approval-{reference}"),
                    vec![
                        types::text_part(&text),
                        types::data_part(approval::link_data(view, &url)),
                    ],
                ))
            }
            Some(PauseKind::Handoff) => {
                let url = approval::link(base_url, context_id);
                Some(agent(
                    format!("{run_id}-handoff-{reference}"),
                    vec![
                        types::text_part(&format!(
                            "The owner is making this change in a Moqui screen; the task goes on by itself once they save it at {url}"
                        )),
                        types::data_part(json!({"type": "moqui.handoff", "handoffId": reference, "url": url})),
                    ],
                ))
            }
            Some(PauseKind::Question | PauseKind::Budget) => {
                let row = pauses.questions.get(&reference)?;
                let text = text_of(row, "questionText").unwrap_or_default();
                let parse = |field: &str| {
                    text_of(row, field)
                        .and_then(|text| serde_json::from_str::<Json>(&text).ok())
                        .unwrap_or(Json::Null)
                };
                let options: Vec<Json> = parse("choicesJson")
                    .as_array()
                    .map(|options| {
                        options
                            .iter()
                            .map(|option| match option {
                                Json::String(label) => json!({"label": label}),
                                other => other.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut data = json!({"type": "moqui.question", "questionId": reference, "text": text, "options": options,
                    "kind": if task.run.pause_kind == Some(PauseKind::Budget) { "budget" } else { "question" }});
                let fields = parse("schemaJson");
                if fields.is_array() {
                    data["fields"] = fields;
                }
                Some(agent(
                    format!("{run_id}-question-{reference}"),
                    vec![types::text_part(&text), types::data_part(data)],
                ))
            }
            // a remote this task called waits for the caller at its link (13.12): Done or Cancel, as an answer
            Some(PauseKind::Auth) => {
                let row = pauses.questions.get(&reference)?;
                let text = text_of(row, "questionText").unwrap_or_default();
                let mut data = json!({"type": "moqui.remoteLink", "questionId": reference, "text": text,
                    "options": [{"label": "Done"}, {"label": "Cancel"}]});
                if let Some(link) = pauses.links.get(&reference) {
                    data["url"] = json!(link);
                }
                Some(agent(
                    format!("{run_id}-link-{reference}"),
                    vec![types::text_part(&text), types::data_part(data)],
                ))
            }
            _ => None,
        },
        RunState::Failed => Some(agent(
            format!("{run_id}-failed"),
            vec![types::text_part(&format!(
                "The agent could not finish this task ({}).",
                task.error_code.as_deref().unwrap_or("failed")
            ))],
        )),
        RunState::Rejected => Some(agent(
            format!("{run_id}-rejected"),
            vec![types::text_part(&format!(
                "The task was not started ({}).",
                task.error_code.as_deref().unwrap_or("rejected")
            ))],
        )),
        _ => None,
    }
}

/// The parts of a stored message as the wire has them: the client's own parts of a user message when they were kept,
/// else its text and data parts.
fn wire_parts(message: &EntityValue, parts: &[&EntityValue]) -> Vec<Json> {
    if let Some(kept) = text_of(message, "metadataJson")
        .and_then(|text| serde_json::from_str::<Json>(&text).ok())
        .and_then(|metadata| metadata.pointer("/a2a/parts").and_then(Json::as_array).cloned())
        .filter(|parts| !parts.is_empty())
    {
        return kept;
    }
    parts
        .iter()
        .filter_map(|part| match text_of(part, "partKindEnumId").as_deref() {
            Some("AgpkText") => text_of(part, "textValue").map(|text| types::text_part(&text)),
            Some("AgpkData") => text_of(part, "dataJson")
                .and_then(|text| serde_json::from_str::<Json>(&text).ok())
                .map(types::data_part),
            _ => None,
        })
        .collect()
}

/// The wire `Task`s of `rows`, in their order: the status times, what paused tasks wait for, the history and its parts
/// each in one query for the whole page. `history` is `historyLength` (`None` the whole history); `artifacts` adds
/// the `response` artifact of a task with an answer.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn tasks(
    facade: &EntityFacade,
    base_url: &str,
    rows: &[TaskRow],
    history: Option<usize>,
    artifacts: bool,
) -> Result<Vec<Json>, Error> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    run::quietly(|| {
        let ids: Vec<Value> = rows.iter().map(|task| Value::string(task.id())).collect();
        // the time of each task's latest state
        let mut status_at: HashMap<String, i64> = HashMap::new();
        for row in facade
            .find(RUN_EVENT)?
            .condition(Condition::is_in("runId", ids.clone()))
            .eq("eventType", Value::string("run.state"))
            .select_field("runId")
            .select_field("eventSeq")
            .select_field("eventDate")
            .order_by("eventSeq", false)
            .disable_authz()
            .list()?
            .values()
        {
            if let (Some(run_id), Some(at)) = (text_of(row, "runId"), row.get("eventDate").and_then(millis_of)) {
                status_at.insert(run_id, at);
            }
        }
        let pauses = Pauses::load(facade, &rows.iter().collect::<Vec<_>>())?;
        let mut messages: HashMap<String, Vec<Json>> = HashMap::new();
        if history != Some(0) {
            let message_rows = facade
                .find(MESSAGE)?
                .condition(Condition::is_in("runId", ids))
                .condition(Condition::is_in(
                    "role",
                    vec![Value::string("user"), Value::string("assistant")],
                ))
                .order_by("messageSeq", false)
                .disable_authz()
                .list()?;
            let message_ids: Vec<Value> = message_rows
                .values()
                .iter()
                .filter_map(|row| text_of(row, "messageId").map(Value::string))
                .collect();
            let part_rows = if message_ids.is_empty() {
                Vec::new()
            } else {
                facade
                    .find(PART)?
                    .condition(Condition::is_in("messageId", message_ids))
                    .order_by("partSeq", false)
                    .disable_authz()
                    .list()?
                    .values()
                    .to_vec()
            };
            let mut parts_of: HashMap<String, Vec<&EntityValue>> = HashMap::new();
            for part in &part_rows {
                parts_of
                    .entry(text_of(part, "messageId").unwrap_or_default())
                    .or_default()
                    .push(part);
            }
            for row in message_rows.values() {
                let message_id = text_of(row, "messageId").unwrap_or_default();
                let parts = wire_parts(row, parts_of.get(&message_id).map(Vec::as_slice).unwrap_or_default());
                if parts.is_empty() {
                    continue;
                }
                let run_id = text_of(row, "runId").unwrap_or_default();
                let agent = text_of(row, "role").as_deref() == Some("assistant");
                let wire_id = if agent {
                    message_id
                } else {
                    text_of(row, "clientMessageId").unwrap_or(message_id)
                };
                let context_id = text_of(row, "conversationId").unwrap_or_default();
                messages.entry(run_id.clone()).or_default().push(types::message(
                    &wire_id,
                    &context_id,
                    &run_id,
                    agent,
                    parts,
                ));
            }
        }
        Ok(rows
            .iter()
            .map(|task| {
                let millis = status_at
                    .get(task.id())
                    .copied()
                    .or(task.ended)
                    .or(Some(task.submitted));
                let mut wire = json!({"id": task.id(), "contextId": task.run.conversation_id,
                    "status": types::status(task.run.state.wire(), status_message(task, base_url, &pauses), millis)});
                if artifacts {
                    if let Some(text) = &task.result_text {
                        wire["artifacts"] = json!([types::response_artifact(text)]);
                    }
                }
                if history != Some(0) {
                    let mut list = messages.remove(task.id()).unwrap_or_default();
                    let keep = history.unwrap_or(types::MAX_HISTORY);
                    if list.len() > keep {
                        list.drain(..list.len() - keep);
                    }
                    wire["history"] = Json::Array(list);
                }
                wire["metadata"] = json!({"agentId": task.run.agent_id});
                wire
            })
            .collect())
    })
}

/// One task's wire object.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn task_json(facade: &EntityFacade, base_url: &str, task: &TaskRow, history: Option<usize>) -> Result<Json, Error> {
    Ok(tasks(facade, base_url, std::slice::from_ref(task), history, true)?
        .pop()
        .unwrap_or(Json::Null))
}

// ---------- SendMessage ----------

/// What a send did: the task as it stands, and the configuration of the send.
pub struct Sent {
    pub task: TaskRow,
    pub config: SendConfig,
}

/// The agent a new task runs: the request's or the message's `metadata.agentId`, else the agent whose card lists
/// `metadata.skillId`, else the conf's `default-agent`; it must be one the caller may use over A2A.
fn agent_of(ctx: &Ctx<'_>, params: &Json, message: &InMessage) -> Result<String, Failure> {
    let meta = |key: &str| {
        params
            .pointer(&format!("/metadata/{key}"))
            .or_else(|| message.metadata.as_ref().and_then(|metadata| metadata.get(key)))
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let exposed = super::card::exposed_agents(&ctx.facade).map_err(|error| Failure::of(&error, "the agent lookup"))?;
    let wanted = meta("agentId").or_else(|| {
        let skill = meta("skillId")?;
        exposed
            .iter()
            .find(|(agent, row)| {
                super::card::agent_skills(agent, row)
                    .iter()
                    .any(|listed| listed["id"].as_str() == Some(skill.as_str()))
            })
            .map(|(agent, _)| agent.agent_id.clone())
    });
    let agent_id = wanted.unwrap_or_else(|| crate::llm::facade(ctx.factory).conf().a2a.default_agent.clone());
    if exposed.iter().any(|(agent, _)| agent.agent_id == agent_id) {
        Ok(agent_id)
    } else {
        Err(A2aError::invalid_params(format!("there is no agent {agent_id} on this server's A2A card")).into())
    }
}

/// `SendMessage` and `SendStreamingMessage` up to the task: checked, started or handed to its task, idempotent on
/// the message id.
///
/// # Errors
///
/// Returns the A2A error of a request that does not fit, or the failure of a run that could not start.
pub(crate) fn send(ctx: &Ctx<'_>, params: &Json) -> Result<Sent, Failure> {
    let message = types::parse_message(params.get("message"), ctx.limits())?;
    let config = types::parse_config(params.get("configuration"))?;
    if params
        .get("metadata")
        .is_some_and(|metadata| !metadata.is_object() && !metadata.is_null())
    {
        return Err(A2aError::invalid_params("metadata must be an object").into());
    }
    for location in &message.locations {
        // a location of this server's own content the caller may read; it is never fetched here. Missing and
        // unreadable are the same answer.
        if !readable(ctx, location) {
            return Err(A2aError::invalid_params(format!("Part.url {location} is not a location you can read")).into());
        }
    }
    for reference in &message.reference_task_ids {
        owned(ctx, reference)?;
    }
    if let Some(push) = &config.push {
        super::push::check_url(ctx.factory, &push.url)?;
    }
    let user_id = ctx.caller.user_id.as_str();
    let replay = run::quietly(|| run::existing_message(&ctx.facade, user_id, Some(&message.message_id)))
        .map_err(|error| Failure::of(&error, "the message lookup"))?;
    if let Some(existing) = replay {
        let task = owned(ctx, &existing.run_id)?;
        return Ok(Sent { task, config });
    }
    let task = match &message.task_id {
        Some(task_id) => continue_task(ctx, task_id, &message)?,
        None => start_task(ctx, params, &message)?,
    };
    if let Some(push) = &config.push {
        super::push::create(ctx, &task, push)?;
    }
    Ok(Sent { task, config })
}

/// Whether the caller may read `location` (a `dbresource://` one): it exists, and it is in a drive the caller may
/// view (`FileServices.get#DriveAccess`, the drive whose `rootLocation` holds it most closely), or under a spill of
/// one of the caller's own runs (`dbresource://agent/<runId>/`). Anything else, or a check that fails, is not.
fn readable(ctx: &Ctx<'_>, location: &str) -> bool {
    // whether it exists is looked up past authorization; whether the caller may read it is the drive's rule below
    if run::quietly(|| ctx.factory.resource().stat(location))
        .ok()
        .flatten()
        .is_none()
    {
        return false;
    }
    if let Some(rest) = location.strip_prefix("dbresource://agent/") {
        let run_id = rest.split('/').next().unwrap_or_default();
        return !run_id.is_empty()
            && run::quietly(|| run::run_owner(&ctx.facade, run_id))
                .ok()
                .flatten()
                .is_some_and(|owner| owner == ctx.caller.user_id);
    }
    let drives = run::quietly(|| {
        ctx.facade
            .find("moqui.resource.ResourceDrive")?
            .condition(Condition::like("rootLocation", Value::string("dbresource://%")))
            .select_field("driveId")
            .select_field("rootLocation")
            .disable_authz()
            .list()
    });
    let Ok(drives) = drives else {
        return false;
    };
    let within = |root: &str| {
        let root = root.trim_end_matches('/');
        location == root || location.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
    };
    let Some(drive_id) = drives
        .values()
        .iter()
        .filter_map(|row| Some((text_of(row, "driveId")?, text_of(row, "rootLocation")?)))
        .filter(|(_, root)| within(root))
        .max_by_key(|(_, root)| root.len())
        .map(|(drive_id, _)| drive_id)
    else {
        return false;
    };
    let parameters: std::collections::BTreeMap<String, Value> = [("driveId".to_string(), Value::string(&drive_id))]
        .into_iter()
        .collect();
    // the service reads the drive's members for the caller; its own rows are moqui.* entities the caller may not read
    let options = crate::service::call::CallOptions {
        disable_authz: true,
        ..Default::default()
    };
    crate::service::call::sync_checked("org.moqui.impl.FileServices.get#DriveAccess", &parameters, &options)
        .ok()
        .and_then(|out| {
            out.get("level")
                .and_then(|level| moqui_lib::object::to_plain_string(level).parse::<i64>().ok())
        })
        .is_some_and(|level| level >= 1)
}

fn model_text(message: &InMessage) -> String {
    if message.reference_task_ids.is_empty() {
        message.text.clone()
    } else {
        format!(
            "{}\n[It refers to the tasks {}]",
            message.text,
            message.reference_task_ids.join(", ")
        )
    }
}

fn start_task(ctx: &Ctx<'_>, params: &Json, message: &InMessage) -> Result<TaskRow, Failure> {
    let user_id = ctx.caller.user_id.clone();
    let mut agent_id = None;
    if let Some(context_id) = &message.context_id {
        let conversation = run::quietly(|| {
            ctx.facade
                .find(CONVERSATION)?
                .eq("conversationId", Value::string(context_id))
                .disable_authz()
                .one()
        })
        .map_err(|error| Failure::of(&error, "the context lookup"))?
        .filter(|row| {
            text_of(row, "userId").as_deref() == Some(user_id.as_str())
                && text_of(row, "surfaceEnumId").as_deref() == Some(SURFACE)
        })
        .ok_or_else(|| A2aError::invalid_params(format!("there is no context {context_id}")))?;
        agent_id = text_of(&conversation, "agentId");
    }
    let agent_id = match agent_id {
        Some(agent_id) => agent_id,
        None => agent_of(ctx, params, message)?,
    };
    let request = run::StartRequest {
        user_id: user_id.clone(),
        agent_id,
        conversation_id: message.context_id.clone(),
        surface: Surface::A2a,
        text: model_text(message),
        client_request_id: Some(message.message_id.clone()),
        client_message_id: Some(message.message_id.clone()),
        ..run::StartRequest::default()
    };
    let started = match run::start_run(ctx.factory, &request) {
        Ok(started) => started,
        Err(error) => {
            // the same message sent at once twice: the other one made the task
            if let Ok(Some(existing)) =
                run::quietly(|| run::existing_message(&ctx.facade, &user_id, Some(&message.message_id)))
            {
                return owned(ctx, &existing.run_id);
            }
            return Err(Failure::of(&error, "the start of a task"));
        }
    };
    if !started.queued && !started.existing {
        keep_parts(ctx, message);
    }
    owned(ctx, &started.run_id)
}

/// Keep the client's own parts (and its metadata) on the stored user message, so the history gives them back. One
/// that cannot be kept is logged; the task goes on.
fn keep_parts(ctx: &Ctx<'_>, message: &InMessage) {
    let only_text = message
        .parts
        .iter()
        .all(|part| part.get("text").is_some() && part.as_object().is_some_and(|part| part.len() == 1));
    if only_text && message.metadata.is_none() {
        return;
    }
    let kept = run::quietly(|| {
        run::in_tx(&ctx.facade, || {
            let Some(mut row) = ctx
                .facade
                .find(MESSAGE)?
                .eq("userId", Value::string(&ctx.caller.user_id))
                .eq("clientMessageId", Value::string(&message.message_id))
                .for_update(true)
                .disable_authz()
                .one()?
            else {
                return Ok(());
            };
            let mut metadata = text_of(&row, "metadataJson")
                .and_then(|text| serde_json::from_str::<Json>(&text).ok())
                .filter(Json::is_object)
                .unwrap_or_else(|| json!({}));
            metadata["a2a"] = json!({"parts": message.parts, "metadata": message.metadata});
            row.set("metadataJson", Value::string(metadata.to_string()));
            row.update(&ctx.facade)
        })
    });
    if let Err(error) = kept {
        tracing::error!(
            "the parts of A2A message {} could not be kept: {error}",
            message.message_id
        );
    }
}

/// A message to a task: the answer to its question, or input it folds in while it works. A task waiting for its
/// owner's approval, and an ended one, take none.
fn continue_task(ctx: &Ctx<'_>, task_id: &str, message: &InMessage) -> Result<TaskRow, Failure> {
    let task = owned(ctx, task_id)?;
    if message
        .context_id
        .as_deref()
        .is_some_and(|context| context != task.run.conversation_id)
    {
        return Err(A2aError::invalid_params("message.contextId is not the context of message.taskId").into());
    }
    if task.run.state.is_terminal() {
        return Err(A2aError::unsupported(format!(
            "task {task_id} is {}; an ended task takes no more messages",
            task.run.state.wire()
        ))
        .into());
    }
    let paused = task.run.state.is_paused();
    let kind = match task.run.pause_kind.filter(|_| paused) {
        None => "AgikMessage",
        // a remote's link (Auth, 13.12) is answered Done or Cancel as a question is
        Some(PauseKind::Question | PauseKind::Budget | PauseKind::Auth) => "AgikAnswer",
        Some(PauseKind::RemoteTask) => "AgikRemote",
        Some(PauseKind::Approval | PauseKind::Handoff) => {
            return Err(A2aError::unsupported(
                "the task waits for its owner's approval in Moqui and takes no message meanwhile; it goes on by itself once they decide",
            )
            .into())
        }
    };
    if kind == "AgikAnswer" && task.run.pause_kind == Some(PauseKind::Question) {
        if let Some(question_id) = &task.run.pause_ref {
            crate::llm::tools::ask::check_answer(&ctx.facade, question_id, message.data.as_ref())
                .map_err(A2aError::invalid_params)?;
        }
    }
    let text = model_text(message);
    let added = run::quietly(|| {
        run::in_tx(&ctx.facade, || {
            run::add_input_with(
                &ctx.facade,
                task_id,
                &ctx.caller.user_id,
                kind,
                &text,
                message.data.as_ref(),
                None,
                Some(&message.message_id),
            )
        })
    });
    if let Err(error) = added {
        if let Ok(Some(existing)) =
            run::quietly(|| run::existing_message(&ctx.facade, &ctx.caller.user_id, Some(&message.message_id)))
        {
            return owned(ctx, &existing.run_id);
        }
        return Err(Failure::of(&error, "a message to a task"));
    }
    if paused {
        run::quietly(|| run::resume_paused(ctx.factory, &ctx.facade, task_id))
            .map_err(|error| Failure::of(&error, "the resume of a task"))?;
    }
    owned(ctx, task_id)
}

/// Whether the response to a send can go now: the caller asked for it at once, or the task waits for no one here.
pub(crate) fn answers_now(ctx: &Ctx<'_>, sent: &Sent) -> bool {
    sent.config.return_immediately || settled(ctx.factory, &sent.task)
}

// ---------- GetTask, ListTasks, CancelTask ----------

/// `GetTask`.
///
/// # Errors
///
/// TaskNotFound for a task that is not the caller's.
pub(crate) fn get(ctx: &Ctx<'_>, params: &Json) -> Result<Json, Failure> {
    let task_id = required_id(params, "id")?;
    let history = types::history_length(params.get("historyLength"))?;
    let task = owned(ctx, &task_id)?;
    task_json(&ctx.facade, &ctx.caller.base_url, &task, history).map_err(|error| Failure::of(&error, "GetTask"))
}

pub(crate) fn required_id(params: &Json, field: &str) -> Result<String, A2aError> {
    params
        .get(field)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| A2aError::invalid_params(format!("{field} is required")))
}

/// `ListTasks`: the caller's tasks, newest first, `pageSize` at a time with a keyset cursor over (submitted time, task
/// id), filtered by `contextId`, `status` and `statusTimestampAfter` (any change at or after it).
///
/// # Errors
///
/// InvalidParams for a filter or token that does not fit.
pub(crate) fn list(ctx: &Ctx<'_>, params: &Json) -> Result<Json, Failure> {
    let size = types::page_size(params.get("pageSize"))?;
    let history = types::history_length(params.get("historyLength"))?;
    let cursor = Cursor::decode(params.get("pageToken").and_then(Json::as_str))?;
    let state = types::state_filter(params.get("status"))?;
    let after = types::timestamp_millis(params.get("statusTimestampAfter"), "statusTimestampAfter")?;
    let context_id = params
        .get("contextId")
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let artifacts = types::flag(params.get("includeArtifacts"));
    let failed = |error: Error| Failure::of(&error, "ListTasks");
    let find = || -> Result<crate::entity::EntityFind, Error> {
        let mut find = ctx
            .facade
            .find(RUN)?
            .eq("userId", Value::string(&ctx.caller.user_id))
            .eq("surfaceEnumId", Value::string(SURFACE))
            .disable_authz();
        if let Some(context_id) = context_id {
            find = find.eq("conversationId", Value::string(context_id));
        }
        if let Some(state) = state {
            find = find.eq("statusId", Value::string(state.status_id()));
        }
        if let Some(after) = after {
            find = find.condition(Condition::greater_equal("lastUpdatedStamp", run::stamp(after)));
        }
        Ok(find)
    };
    let (rows, total) = run::quietly(|| -> Result<(Vec<TaskRow>, i64), Error> {
        let total = find()?.count()?;
        let mut page = find()?;
        if let Some(cursor) = &cursor {
            page = page.condition(Condition::or(vec![
                Condition::less("submittedDate", run::stamp(cursor.submitted)),
                Condition::and(vec![
                    Condition::equals("submittedDate", run::stamp(cursor.submitted)),
                    Condition::less("runId", Value::string(&cursor.run_id)),
                ]),
            ]));
        }
        let rows = page
            .order_by("submittedDate", true)
            .order_by("runId", true)
            .limit(i64::try_from(size + 1).unwrap_or(101))
            .list()?;
        Ok((rows.values().iter().map(TaskRow::of).collect(), total))
    })
    .map_err(failed)?;
    let more = rows.len() > size;
    let rows: Vec<TaskRow> = rows.into_iter().take(size).collect();
    let next = if more {
        rows.last()
            .map(|last| {
                Cursor {
                    submitted: last.submitted,
                    run_id: last.id().to_string(),
                }
                .encode()
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let wire = tasks(&ctx.facade, &ctx.caller.base_url, &rows, history, artifacts).map_err(failed)?;
    Ok(json!({"tasks": wire, "nextPageToken": next, "pageSize": size, "totalSize": total}))
}

/// `CancelTask` up to its wait: an ended task is TaskNotCancelable; another is asked to stop ([`wait_canceled`] gives
/// it back once it has). When the run ends some other way first, that end stands.
///
/// # Errors
///
/// TaskNotFound, TaskNotCancelable.
pub(crate) fn cancel(ctx: &Ctx<'_>, params: &Json) -> Result<TaskRow, Failure> {
    let task_id = required_id(params, "id")?;
    let task = owned(ctx, &task_id)?;
    if task.run.state.is_terminal() {
        return Err(A2aError::not_cancelable(task.run.state.wire()).into());
    }
    match run::cancel_run(ctx.factory, &task_id, Some(&ctx.caller.user_id)) {
        Ok(_) => owned(ctx, &task_id),
        Err(error) => {
            // it ended meanwhile: the end stands
            let now = owned(ctx, &task_id)?;
            if now.run.state.is_terminal() {
                return Err(A2aError::not_cancelable(now.run.state.wire()).into());
            }
            Err(Failure::of(&error, "CancelTask"))
        }
    }
}

/// The wire task of a canceled `task` once it has stopped, waiting up to [`CANCEL_WAIT`] (on a thread of its own, in a
/// context with authorization off).
///
/// # Errors
///
/// Returns an error when the task cannot be read.
pub fn wait_canceled(factory: &Arc<ExecutionContextFactory>, task: &TaskRow, base_url: &str) -> Result<Json, Error> {
    let facade = run::facade_of(factory)?;
    let deadline = Instant::now() + CANCEL_WAIT;
    let mut now = task.clone();
    while !now.run.state.is_terminal() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        match load_task(&facade, task.id())? {
            Some(found) => now = found,
            None => break,
        }
    }
    task_json(&facade, base_url, &now, None)
}

/// `SubscribeToTask` up to its stream: the caller's task, not ended.
///
/// # Errors
///
/// TaskNotFound; UnsupportedOperation for an ended task.
pub(crate) fn subscribe(ctx: &Ctx<'_>, params: &Json) -> Result<TaskRow, Failure> {
    let task_id = required_id(params, "id")?;
    let task = owned(ctx, &task_id)?;
    if task.run.state.is_terminal() {
        return Err(A2aError::unsupported(format!(
            "task {task_id} is {}; an ended task cannot be subscribed to",
            task.run.state.wire()
        ))
        .into());
    }
    Ok(task)
}

// ---------- streams ----------

/// A stream of a task's updates from its stored events after `after`.
#[derive(Clone, Debug)]
pub struct Follow {
    pub run_id: String,
    pub context_id: String,
    pub base_url: String,
    pub after: i64,
    /// End when the task pauses. False for a subscriber of a task waiting for its owner's approval, whose stream goes
    /// on through the decision to the next pause or the end.
    pub close_on_pause: bool,
    /// The text chunks of the answer as `artifactUpdate`s (a stream's); a push sends states and the whole answer.
    pub text: bool,
}

impl Follow {
    /// A stream of `task` from where its snapshot stands.
    pub fn of(task: &TaskRow, base_url: &str, close_on_pause: bool, text: bool) -> Self {
        Self {
            run_id: task.id().to_string(),
            context_id: task.run.conversation_id.clone(),
            base_url: base_url.to_string(),
            after: task.last_event_seq,
            close_on_pause,
            text,
        }
    }
}

/// The projection of a run's stored events into `StreamResponse`s.
pub struct Projector {
    follow: Follow,
    resumed: bool,
}

impl Projector {
    pub fn new(follow: Follow) -> Self {
        Self { follow, resumed: false }
    }

    /// The `StreamResponse`s of one stored event, and whether the stream ends with it.
    ///
    /// # Errors
    ///
    /// Returns an error when the task cannot be read for its status.
    pub fn project(
        &mut self,
        facade: &EntityFacade,
        event_type: &str,
        payload: &Json,
    ) -> Result<(Vec<Json>, bool), Error> {
        let (task_id, context_id) = (self.follow.run_id.clone(), self.follow.context_id.clone());
        match event_type {
            "text.chunk" if self.follow.text => {
                let text = payload["text"].as_str().unwrap_or_default();
                if text.is_empty() {
                    return Ok((Vec::new(), false));
                }
                let append = payload["offset"].as_u64().unwrap_or(0) > 0;
                Ok((
                    vec![types::artifact_update(
                        &task_id,
                        &context_id,
                        types::response_artifact(text),
                        append,
                        false,
                    )],
                    false,
                ))
            }
            // the call started again: what it streamed is void, so the artifact is replaced with nothing yet
            "step.restart" if self.follow.text => Ok((
                vec![types::artifact_update(
                    &task_id,
                    &context_id,
                    types::response_artifact(""),
                    false,
                    false,
                )],
                false,
            )),
            "run.state" => {
                let Some(state) = payload["state"].as_str().and_then(RunState::from_wire) else {
                    return Ok((Vec::new(), false));
                };
                if state == RunState::Working {
                    self.resumed = true;
                }
                let mut out = Vec::new();
                let mut message = None;
                let mut millis = Some(run::now_millis());
                if state.is_terminal() || state.is_paused() {
                    if let Some(task) = load_task(facade, &task_id)? {
                        if state == RunState::Completed {
                            if let Some(text) = &task.result_text {
                                out.push(types::artifact_update(
                                    &task_id,
                                    &context_id,
                                    types::response_artifact(text),
                                    false,
                                    true,
                                ));
                            }
                        }
                        // the status as it was at this event: a later state's message is not this one's
                        if task.run.state == state {
                            let pauses = run::quietly(|| Pauses::load(facade, &[&task]))?;
                            message = status_message(&task, &self.follow.base_url, &pauses);
                            millis = task.ended.or(millis);
                        }
                    }
                }
                out.push(types::status_update(
                    &task_id,
                    &context_id,
                    types::status(state.wire(), message, millis),
                ));
                let ends = state.is_terminal() || (state.is_paused() && (self.follow.close_on_pause || self.resumed));
                Ok((out, ends))
            }
            "run.done" => Ok((Vec::new(), true)),
            _ => Ok((Vec::new(), false)),
        }
    }
}

/// Follow a task: project each stored event after `follow.after` to `emit` (`None` is a keep-alive), until the stream
/// ends (a terminal or interrupted state, as [`Follow::close_on_pause`] says), `emit` says the client is gone,
/// `stop` says the server stops, or `deadline` passes. Runs in an execution context of the caller's thread with
/// authorization off. Out: the last stored sequence projected.
///
/// # Errors
///
/// Returns an error when the events cannot be read.
pub fn follow(
    factory: &Arc<ExecutionContextFactory>,
    follow: &Follow,
    deadline: Option<Instant>,
    stop: &dyn Fn() -> bool,
    emit: &mut dyn FnMut(Option<Json>) -> bool,
) -> Result<i64, Error> {
    let facade = factory
        .entity()
        .ok_or_else(|| Error::new("the entity facade is not installed"))?;
    let run_id = follow.run_id.as_str();
    let mut projector = Projector::new(follow.clone());
    let mut last = follow.after;
    // the stored events after `last`, page by page: Some(true) when the stream is over, None when the client left
    let replay = |last: &mut i64,
                  projector: &mut Projector,
                  emit: &mut dyn FnMut(Option<Json>) -> bool|
     -> Result<Option<bool>, Error> {
        loop {
            let page = events::stored_after(&facade, run_id, *last, PAGE)?;
            for event in &page {
                *last = event.seq;
                let (out, ends) = projector.project(&facade, &event.event_type, &event.payload)?;
                for response in out {
                    if !emit(Some(response)) {
                        return Ok(None);
                    }
                }
                if ends {
                    return Ok(Some(true));
                }
            }
            if i64::try_from(page.len()).unwrap_or(0) < PAGE {
                return Ok(Some(false));
            }
        }
    };
    if replay(&mut last, &mut projector, emit)? != Some(false) {
        return Ok(last);
    }
    let engine = run::engine(factory);
    let hub = engine.hub();
    let _watch = hub.watch(factory, run_id, last);
    let mut follower = run::hub::Follower::new(&hub, run_id, last);
    let mut wrote = Instant::now();
    let mut checked = Instant::now();
    loop {
        if stop() || deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(last);
        }
        for event in follower.next(&hub, &facade, Duration::from_secs(1))? {
            let Some(seq) = event.seq else {
                continue;
            };
            last = seq;
            let (out, ends) = projector.project(&facade, &event.event_type, &event.payload)?;
            for response in out {
                if !emit(Some(response)) {
                    return Ok(last);
                }
                wrote = Instant::now();
            }
            if ends {
                return Ok(last);
            }
        }
        if wrote.elapsed() >= PING_EVERY {
            if !emit(None) {
                return Ok(last);
            }
            wrote = Instant::now();
        }
        if checked.elapsed() >= CHECK_EVERY {
            checked = Instant::now();
            let done = load_task(&facade, run_id)?.is_none_or(|task| {
                task.run.state.is_terminal() || (task.run.state.is_paused() && follow.close_on_pause)
            });
            if done && !engine.is_local(run_id) {
                // the last events may have come after the hub was read: project what is stored, then end
                replay(&mut last, &mut projector, emit)?;
                return Ok(last);
            }
        }
    }
}

/// Wait up to `wait` for a task to end or pause (a blocking send), without a request worker. Out: the task then.
///
/// # Errors
///
/// Returns an error when the task cannot be read.
pub fn wait_settled(
    factory: &Arc<ExecutionContextFactory>,
    task: &TaskRow,
    base_url: &str,
    wait: Duration,
    stop: &dyn Fn() -> bool,
) -> Result<Option<TaskRow>, Error> {
    let facade = factory
        .entity()
        .ok_or_else(|| Error::new("the entity facade is not installed"))?;
    if !settled(factory, task) {
        follow(
            factory,
            &Follow::of(task, base_url, true, false),
            Some(Instant::now() + wait),
            stop,
            &mut |_| true,
        )?;
        // the step that paused or ended the run may still be finishing its last writes here
        let until = Instant::now() + Duration::from_secs(2);
        while let Some(now) = load_task(&facade, task.id())? {
            if settled(factory, &now) || Instant::now() >= until {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    load_task(&facade, task.id())
}
