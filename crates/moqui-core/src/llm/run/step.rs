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

//! The step job (plan 4.4): one model call of a run and the tool calls it asks for, in a fresh execution context of
//! the agent pool, logged in as the run's user (`internal_login_user`) with the loop-guard origin set.
//!
//! A step takes the run's lease, folds in the input that came meanwhile (steering), finishes what the last step left:
//! a tool call still waiting runs, a paused one resumes with its input, and one left running by a node that stopped is
//! `UnknownOutcome` (a READ runs again; a write, destructive or external call never does, and the model is told to check
//! its outcome). Then the budget is checked (an interactive run pauses with a "continue?" question, a background one
//! fails `budget_exhausted`), the request is built from the conversation's frozen prefix, its history and the newest
//! message's tail, compacted first when it is over the model's high watermark ([`crate::llm::window`]), and the model is
//! called with no transaction open. A JSON tool mode reply is read as its protocol object, and a native reply's calls
//! written as text are taken as calls ([`crate::llm::json_mode`]). The reply is stored (the step, the assistant
//! message, its tool calls), and the calls run in model order: consecutive READ calls in parallel when the model allows
//! it, the rest one at a time. A call that pauses moves the run to InputRequired or AuthRequired and ends the step; a
//! reply with no calls completes the run; otherwise the next step job is queued.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::context::{Approvals, Exhausted, RunRef, ScreenHandle, Spill, ToolCallRef, UserRef};
use super::{
    engine, held, in_tx, lease, load_run, now_millis, pool, quietly, stamp, transition, AgentDef, Budget, Change,
    EventSink, Hold, LocalRun, RunRow, RunState, TurnContext, INPUT, MESSAGE, PART, QUESTION, RUN, TOOL_CALL,
};
use crate::context::{AgentOrigin, ExecutionContext, ExecutionContextFactory};
use crate::entity::{fence, EntityFacade, EntityValue};
use crate::llm::prompt::{self, Catalog, PrefixSpec, PromptTexts, PromptVars};
use crate::llm::tokens::Window;
use crate::llm::window;
use crate::llm::provider::{
    self, CallOptions, Finish, Message, ModelRequest, ProviderEvent, Recorder, ReplayBag, Role, Signal,
};
use crate::llm::registry::{Target, Want};
use crate::llm::tools::{self, caps, AgentTool, Classification, Offered, ToolInput, ToolOutcome, ToolPolicy};
use crate::llm::usage::CallSource;
use crate::llm::{int_of, text_of};

/// A tool result over this many characters is cut when the model gives no `result-cap-chars`.
const DEFAULT_RESULT_CAP: usize = 8000;
/// What a tool call's `resultSummary` keeps.
const SUMMARY_CAP: usize = 4000;
/// What the calls besides a step's own (the intent hint, the summary, the schema repair) spent, until the run's totals
/// and budget take it ([`Turn::settle_side`]).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SideSpend {
    usage: crate::llm::usage::Usage,
    cost: rust_decimal::Decimal,
}

impl SideSpend {
    fn add(&mut self, reply: &provider::Reply) {
        self.usage.add(&reply.usage);
        self.cost += reply.cost.unwrap_or_default();
    }
}

/// The system text of the schema repair call.
const SCHEMA_REPAIR: &str = "You fix the arguments of a tool call so that they fit the tool's JSON schema. Keep the values \
the call meant, convert them to the schema's types, and leave out what has no place in it. Answer with only the arguments \
as one JSON object.";
/// The tool calls of a run on a model that sets no `max-tool-calls` (plan 7.9): small and standard prompt variants.
pub(crate) const SMALL_TOOL_CALLS: u32 = 12;
pub(crate) const STANDARD_TOOL_CALLS: u32 = 30;
/// Independent reads run at most this many at a time, fewer when the connection pool has less room
/// ([`super::Engine::parallel_reads`]).
pub(crate) const PARALLEL_READS: usize = 4;

/// One part of a message to store.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Part {
    pub kind: &'static str,
    pub text: Option<String>,
    pub data: Option<Json>,
    pub tool_call_id: Option<String>,
    pub metadata: Option<Json>,
}

impl Part {
    pub(crate) fn text(text: &str) -> Self {
        Self {
            kind: "AgpkText",
            text: Some(text.to_string()),
            ..Self::default()
        }
    }

    /// A file the user attached: its data `{attachmentId, ref, fileName, mediaType, size, location}`.
    pub(crate) fn file(data: &Json) -> Self {
        Self {
            kind: "AgpkFile",
            data: Some(data.clone()),
            ..Self::default()
        }
    }
}

/// A user message's parts: its text, then a `file` part for each attachment.
pub(crate) fn message_parts(text: &str, attachments: &[Json]) -> Vec<Part> {
    let mut parts = vec![Part::text(text)];
    parts.extend(attachments.iter().map(Part::file));
    parts
}

/// Append a message to the conversation, numbered from its `lastMessageSeq` read for update, in a transaction of its
/// own or the one in place. Out: its id.
#[allow(clippy::too_many_arguments)]
pub(crate) fn append_message(
    facade: &EntityFacade,
    conversation_id: &str,
    run_id: Option<&str>,
    role: &str,
    user_id: Option<&str>,
    client_message_id: Option<&str>,
    parts: &[Part],
    metadata: Option<&Json>,
    replay: Option<&ReplayBag>,
) -> Result<String, Error> {
    append_message_as(
        facade,
        conversation_id,
        run_id,
        role,
        user_id,
        client_message_id,
        parts,
        metadata,
        replay,
        None,
    )
}

/// [`append_message`] of `message`, a value made earlier (its id given out before the reply came), else a new one.
#[allow(clippy::too_many_arguments)]
fn append_message_as(
    facade: &EntityFacade,
    conversation_id: &str,
    run_id: Option<&str>,
    role: &str,
    user_id: Option<&str>,
    client_message_id: Option<&str>,
    parts: &[Part],
    metadata: Option<&Json>,
    replay: Option<&ReplayBag>,
    message: Option<EntityValue>,
) -> Result<String, Error> {
    in_tx(facade, || {
        append_in_tx(
            facade,
            conversation_id,
            run_id,
            role,
            user_id,
            client_message_id,
            parts,
            metadata,
            replay,
            message,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn append_in_tx(
    facade: &EntityFacade,
    conversation_id: &str,
    run_id: Option<&str>,
    role: &str,
    user_id: Option<&str>,
    client_message_id: Option<&str>,
    parts: &[Part],
    metadata: Option<&Json>,
    replay: Option<&ReplayBag>,
    message: Option<EntityValue>,
) -> Result<String, Error> {
    let Some(mut conversation) = facade
        .find(super::CONVERSATION)?
        .eq("conversationId", Value::string(conversation_id))
        .for_update(true)
        .disable_authz()
        .one()?
    else {
        return Err(Error::new(format!("there is no agent conversation {conversation_id}")));
    };
    let seq = int_of(&conversation, "lastMessageSeq").unwrap_or(0) + 1;
    let now = now_millis();
    let mut row = match message {
        Some(row) => row,
        None => {
            let mut row = facade.make_value(MESSAGE)?;
            row.set_sequenced_id_primary(facade)?;
            row
        }
    };
    row.set("conversationId", Value::string(conversation_id));
    row.set("messageSeq", Value::I64(seq));
    row.set("role", Value::string(role));
    row.set("createdDate", stamp(now));
    if let Some(run_id) = run_id {
        row.set("runId", Value::string(run_id));
    }
    if let Some(user_id) = user_id {
        row.set("userId", Value::string(user_id));
    }
    if let Some(client_message_id) = client_message_id {
        row.set("clientMessageId", Value::string(client_message_id));
    }
    if let Some(metadata) = metadata {
        row.set("metadataJson", Value::string(metadata.to_string()));
    }
    if let Some(bag) = replay {
        row.set("replayJson", Value::string(bag.to_json()));
        row.set("replayEndpointId", Value::string(&bag.endpoint_id));
        row.set("replayModelId", Value::string(&bag.model_id));
    }
    row.create(facade)?;
    let message_id = text_of(&row, "messageId").unwrap_or_default();
    for (index, part) in parts.iter().enumerate() {
        let mut part_row = facade.make_value(PART)?;
        part_row.set("messageId", Value::string(&message_id));
        part_row.set("partSeq", Value::I64(i64::try_from(index).unwrap_or(0) + 1));
        part_row.set("partKindEnumId", Value::string(part.kind));
        if let Some(text) = &part.text {
            part_row.set("textValue", Value::string(text));
        }
        if let Some(data) = &part.data {
            part_row.set("dataJson", Value::string(data.to_string()));
        }
        if let Some(tool_call_id) = &part.tool_call_id {
            part_row.set("toolCallId", Value::string(tool_call_id));
        }
        if let Some(metadata) = &part.metadata {
            part_row.set("metadataJson", Value::string(metadata.to_string()));
        }
        if part.kind == "AgpkFile" {
            let data = part.data.as_ref();
            for (field, key) in [("fileName", "fileName"), ("mediaType", "mediaType"), ("fileLocation", "location")] {
                if let Some(value) = data.and_then(|data| data.get(key)).and_then(Json::as_str) {
                    part_row.set(field, Value::string(value));
                }
            }
        }
        part_row.create(facade)?;
    }
    conversation.set("lastMessageSeq", Value::I64(seq));
    conversation.set("lastMessageDate", stamp(now));
    conversation.update(facade)?;
    Ok(message_id)
}

/// The model of a step: the agent's feature binding (or its tier); when the client chose a tier the agent leaves open
/// and the bound model is of another, the first usable model of the chosen tier, at the agent's reasoning level or the
/// binding's.
fn resolve_target(
    llm: &crate::llm::LlmFacade,
    facade: &EntityFacade,
    agent: &AgentDef,
    run: &RunRow,
) -> Result<Target, Error> {
    if !llm.enabled() {
        return Err(Error::new("the llm-facade is disabled"));
    }
    let registry = llm.registry(facade);
    let bound = registry.resolve(&Want {
        feature: Some(agent.feature_id.as_deref().unwrap_or("assist")),
        tier: agent.tier,
        reasoning: agent.reasoning,
        local_only: agent.local_only,
        ..Want::default()
    });
    let Some(tier) = run.tier.filter(|_| agent.tier.is_none()) else {
        return bound;
    };
    if bound.as_ref().is_ok_and(|target| target.model.tier == tier) {
        return bound;
    }
    match registry.tier_model(tier, agent.local_only) {
        Some(model_id) => registry.resolve(&Want {
            model: Some(model_id),
            reasoning: agent.reasoning.or_else(|| bound.as_ref().ok().map(|target| target.reasoning)),
            local_only: agent.local_only,
            ..Want::default()
        }),
        None => bound,
    }
}

/// Input for a run not taken yet.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OpenInput {
    pub input_id: String,
    /// The `AgentInputKind` `enumCode`.
    pub kind: String,
    pub text: String,
    pub data: Option<Json>,
    /// The client's own id of a message, which the folded message keeps.
    pub client_message_id: Option<String>,
}

impl OpenInput {
    fn tool_input(&self) -> ToolInput {
        ToolInput {
            kind: self.kind.clone(),
            text: self.text.clone(),
            data: self.data.clone(),
        }
    }

    fn tail(&self) -> Option<String> {
        self.data
            .as_ref()
            .and_then(|data| data.get("tail"))
            .and_then(Json::as_str)
            .map(str::to_string)
    }
}

/// The run's input not taken yet, oldest first.
pub(crate) fn open_inputs(facade: &EntityFacade, run_id: &str) -> Result<Vec<OpenInput>, Error> {
    let rows = facade
        .find(INPUT)?
        .eq("runId", Value::string(run_id))
        .condition(Condition::is_null("consumedDate"))
        .order_by("receivedDate", false)
        .order_by("inputId", false)
        .disable_authz()
        .list()?;
    Ok(rows
        .values()
        .iter()
        .map(|row| OpenInput {
            input_id: text_of(row, "inputId").unwrap_or_default(),
            kind: match text_of(row, "inputKindEnumId").as_deref() {
                Some("AgikAnswer") => "answer",
                Some("AgikDecision") => "decision",
                Some("AgikRemote") => "remote",
                _ => "message",
            }
            .to_string(),
            text: text_of(row, "messageText").unwrap_or_default(),
            data: text_of(row, "dataJson").and_then(|text| serde_json::from_str(&text).ok()),
            client_message_id: text_of(row, "clientMessageId"),
        })
        .collect())
}

/// An open message input read again for update: its text as it is now, or none when it was taken meanwhile (a
/// queued message the user canceled is taken without being folded in).
fn locked_input(facade: &EntityFacade, input: &OpenInput) -> Result<Option<OpenInput>, Error> {
    let row = facade
        .find(INPUT)?
        .eq("inputId", Value::string(&input.input_id))
        .for_update(true)
        .disable_authz()
        .one()?;
    Ok(row.filter(|row| row.get("consumedDate").is_none_or(moqui_lib::object::is_empty)).map(|row| OpenInput {
        text: text_of(&row, "messageText").unwrap_or_default(),
        ..input.clone()
    }))
}

pub(crate) fn consume_input(facade: &EntityFacade, input_id: &str) -> Result<(), Error> {
    in_tx(facade, || {
        facade
            .find(INPUT)?
            .eq("inputId", Value::string(input_id))
            .disable_authz()
            .update_all([("consumedDate".to_string(), stamp(now_millis()))])
            .map(|_| ())
    })
}

pub(crate) fn answer_question(facade: &EntityFacade, question_id: &str, answer: &Json) -> Result<(), Error> {
    in_tx(facade, || {
        facade
            .find(QUESTION)?
            .eq("questionId", Value::string(question_id))
            .eq("statusId", Value::string("AgqOpen"))
            .for_update(true)
            .disable_authz()
            .update_all([
                ("statusId".to_string(), Value::string("AgqAnswered")),
                ("answerJson".to_string(), Value::string(answer.to_string())),
                ("answeredDate".to_string(), stamp(now_millis())),
            ])
            .map(|_| ())
    })
}

/// A tool call of the run's last assistant message, as stored.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredCall {
    pub tool_call_id: String,
    pub provider_id: String,
    pub name: String,
    pub arguments: Json,
    pub status: String,
    pub classification: Option<Classification>,
}

impl StoredCall {
    fn is_open(&self) -> bool {
        matches!(
            self.status.as_str(),
            "AgtcPending" | "AgtcRunning" | "AgtcAwaiting" | "AgtcUnknownOutcome"
        )
    }

    fn reference(&self) -> ToolCallRef {
        ToolCallRef {
            tool_call_id: self.tool_call_id.clone(),
            provider_call_id: self.provider_id.clone(),
            name: self.name.clone(),
            classification: self.classification.unwrap_or_default(),
        }
    }
}

/// The tool calls of the run's newest assistant message, in model order.
fn last_calls(facade: &EntityFacade, run_id: &str) -> Result<Vec<StoredCall>, Error> {
    let Some(message) = facade
        .find(MESSAGE)?
        .eq("runId", Value::string(run_id))
        .eq("role", Value::string("assistant"))
        .order_by("messageSeq", true)
        .limit(1)
        .disable_authz()
        .list()?
        .into_values()
        .into_iter()
        .next()
    else {
        return Ok(Vec::new());
    };
    let message_id = text_of(&message, "messageId").unwrap_or_default();
    let parts = facade
        .find(PART)?
        .eq("messageId", Value::string(&message_id))
        .eq("partKindEnumId", Value::string("AgpkToolCall"))
        .order_by("partSeq", false)
        .disable_authz()
        .list()?;
    let ids: Vec<String> = parts
        .values()
        .iter()
        .filter_map(|part| text_of(part, "toolCallId"))
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = facade
        .find(TOOL_CALL)?
        .condition(Condition::is_in("toolCallId", ids.iter().map(Value::string).collect()))
        .disable_authz()
        .list()?;
    let by_id: HashMap<String, &EntityValue> = rows
        .values()
        .iter()
        .filter_map(|row| text_of(row, "toolCallId").map(|id| (id, row)))
        .collect();
    Ok(ids
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(|row| StoredCall {
            tool_call_id: text_of(row, "toolCallId").unwrap_or_default(),
            provider_id: text_of(row, "providerCallId").unwrap_or_default(),
            name: text_of(row, "toolName").unwrap_or_default(),
            arguments: text_of(row, "argumentsJson")
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_else(|| json!({})),
            status: text_of(row, "statusId").unwrap_or_default(),
            classification: text_of(row, "classificationEnumId").and_then(|class| Classification::parse(&class)),
        })
        .collect())
}

/// Close the tool calls a run that ended left open: each gets a result saying it did not run, so the conversation's
/// history has a result for every call.
pub(crate) fn close_open_calls(facade: &EntityFacade, run_id: &str, sink: &mut EventSink) -> Result<(), Error> {
    let Some(run) = load_run(facade, run_id)? else {
        return Ok(());
    };
    for call in last_calls(facade, run_id)?.into_iter().filter(StoredCall::is_open) {
        let text = "Not run: the run ended before this call.";
        // in the transition's own transaction, which gave the lease up
        store_result(facade, &run, &call, text, true, "AgtcFailed", sink, Hold::LeaseOrNone)?;
    }
    Ok(())
}

/// Store a tool call's result: its row's status and summary, and the tool message the model reads; only while `hold`
/// allows this node to write the run.
#[allow(clippy::too_many_arguments)]
fn store_result(
    facade: &EntityFacade,
    run: &RunRow,
    call: &StoredCall,
    text: &str,
    is_error: bool,
    status_id: &str,
    sink: &mut EventSink,
    hold: Hold,
) -> Result<(), Error> {
    store_result_with(facade, run, call, text, is_error, status_id, sink, hold, None)
}

/// [`store_result`] with the artifact of a spilled result, written in the same transaction under the lease check; an
/// artifact whose result is not stored is removed.
#[allow(clippy::too_many_arguments)]
fn store_result_with(
    facade: &EntityFacade,
    run: &RunRow,
    call: &StoredCall,
    text: &str,
    is_error: bool,
    status_id: &str,
    sink: &mut EventSink,
    hold: Hold,
    artifact: Option<&tools::spill::Artifact>,
) -> Result<(), Error> {
    let node = sink.node().to_string();
    let stored = in_tx(facade, || {
        held(facade, &run.run_id, &node, hold)?;
        if let Some(artifact) = artifact {
            artifact.write(facade)?;
        }
        facade
            .find(TOOL_CALL)?
            .eq("toolCallId", Value::string(&call.tool_call_id))
            .for_update(true)
            .disable_authz()
            .update_all([
                ("statusId".to_string(), Value::string(status_id)),
                ("endedDate".to_string(), stamp(now_millis())),
                (
                    "resultSummary".to_string(),
                    Value::string(text.chars().take(SUMMARY_CAP).collect::<String>()),
                ),
            ])?;
        append_message(
            facade,
            &run.conversation_id,
            Some(&run.run_id),
            "tool",
            None,
            None,
            &[Part {
                kind: "AgpkToolResult",
                text: Some(text.to_string()),
                tool_call_id: Some(call.tool_call_id.clone()),
                metadata: Some(json!({"providerCallId": call.provider_id, "name": call.name, "isError": is_error})),
                ..Part::default()
            }],
            None,
            None,
        )?;
        Ok(())
    });
    if let Err(error) = stored {
        if let Some(artifact) = artifact {
            artifact.remove(facade);
        }
        return Err(error);
    }
    let status = match status_id {
        "AgtcDone" => "done",
        "AgtcUnknownOutcome" => "unknown-outcome",
        "AgtcDenied" => "denied",
        _ => "failed",
    };
    sink.emit(
        "tool.result",
        json!({"toolCallId": call.tool_call_id, "name": call.name, "status": status, "isError": is_error,
            "text": text.chars().take(2000).collect::<String>()}),
    );
    Ok(())
}

/// Set a tool call's status, while this node holds the run's lease.
fn set_call_status(
    facade: &EntityFacade,
    run_id: &str,
    node: &str,
    tool_call_id: &str,
    status_id: &str,
) -> Result<(), Error> {
    in_tx(facade, || {
        held(facade, run_id, node, Hold::Lease)?;
        let mut fields = vec![("statusId".to_string(), Value::string(status_id))];
        if status_id == "AgtcRunning" {
            fields.push(("startedDate".to_string(), stamp(now_millis())));
        }
        facade
            .find(TOOL_CALL)?
            .eq("toolCallId", Value::string(tool_call_id))
            .for_update(true)
            .disable_authz()
            .update_all(fields)
            .map(|_| ())
    })
}

/// The conversation's history as the model reads it, and the tail of its newest user message: messages in order,
/// compacted ones left out and what compaction did in place ([`crate::llm::window`]); an older user message keeps the
/// tail it was sent with, as text, so the bytes sent before are sent again (append-only).
#[cfg(test)]
pub(crate) fn load_history(
    facade: &EntityFacade,
    conversation_id: &str,
) -> Result<(Vec<Message>, Option<String>), Error> {
    Ok(window::load(facade, conversation_id)?.messages())
}

enum Next {
    /// Queue the next step.
    Continue,
    /// The run paused, ended, or is not this node's to work.
    Stop,
}

/// The pool job of one step. It never fails: an error ends the run Failed, logged.
pub(crate) fn run_step(factory: &Arc<ExecutionContextFactory>, run_id: &str) {
    let engine = engine(factory);
    let local = engine.local(run_id);
    if local.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| step(factory, run_id, &local)));
    local.busy.store(false, Ordering::SeqCst);
    let lost = local.is_lost() || matches!(&outcome, Ok(Err(error)) if super::is_lease_lost(error));
    if lost {
        // another node took the run over: this one writes nothing more and leaves its state to that node
        tracing::info!("agent run {run_id} went to another node, so this node stopped working it");
        engine.forget_if(run_id, &local);
        return;
    }
    let failure = match outcome {
        Ok(Ok(Next::Continue)) => {
            if let Err(error) = pool::submit(factory, run_id) {
                tracing::error!(
                    "the next step of agent run {run_id} could not be queued, so the recovery sweep will: {error}"
                );
                if let Some(facade) = factory.entity() {
                    if let Err(error) = quietly(|| lease::release(&facade, run_id, engine.node())) {
                        tracing::error!("the lease of agent run {run_id} could not be given up: {error}");
                    }
                }
                engine.forget_if(run_id, &local);
            }
            return;
        }
        Ok(Ok(Next::Stop)) => return,
        Ok(Err(error)) => error.to_string(),
        Err(_) => "the step panicked".to_string(),
    };
    tracing::error!("agent run {run_id} failed: {failure}");
    if let Some(facade) = factory.entity() {
        let user_id = quietly(|| load_run(&facade, run_id))
            .ok()
            .flatten()
            .map(|run| run.user_id)
            .unwrap_or_default();
        let mut sink = EventSink::new(factory, &facade, run_id, &user_id);
        sink.emit("error", json!({"code": "step_failed", "message": failure}));
        let ended = quietly(|| {
            transition(
                &facade,
                &mut sink,
                run_id,
                RunState::Failed,
                &Change::failed("step_failed", &failure),
            )
        });
        if let Err(error) = ended {
            tracing::error!("agent run {run_id} could not be marked failed: {error}");
        }
    }
    engine.forget_if(run_id, &local);
}

/// The working state of one step.
struct Turn {
    factory: Arc<ExecutionContextFactory>,
    facade: EntityFacade,
    run: RunRow,
    run_ref: RunRef,
    user: UserRef,
    agent: AgentDef,
    offered: Vec<Offered>,
    /// The agent's tool policy, for what it says of a tool's writes.
    policy: ToolPolicy,
    budget: Budget,
    sink: EventSink,
    local: Arc<LocalRun>,
    result_cap: usize,
    /// The model's `list-limit`; 0 is the list tools' default.
    list_limit: usize,
    /// Reads run at most this many at a time (1 is one at a time).
    parallel: usize,
    per_tool: HashMap<String, u32>,
    /// The conversation's screen session, when the run has the `screen` capability.
    screen: Option<ScreenHandle>,
    /// The step's model, for the calls besides the turn's own (the schema repair).
    target: Option<Target>,
    /// The tool calls a run on this model may make: its `max-tool-calls`, else 12 for the small variant and 30.
    model_tool_calls: u32,
    /// What the side calls spent and the run's totals do not have yet.
    side: std::sync::Mutex<SideSpend>,
}

fn step(factory: &Arc<ExecutionContextFactory>, run_id: &str, local: &Arc<LocalRun>) -> Result<Next, Error> {
    let engine = engine(factory);
    let facade = super::facade_of(factory)?;
    let Some(run) = quietly(|| load_run(&facade, run_id))? else {
        engine.forget_if(run_id, local);
        return Ok(Next::Stop);
    };
    if run.state.is_terminal() || run.state.is_paused() {
        engine.forget_if(run_id, local);
        return Ok(Next::Stop);
    }
    if !quietly(|| lease::claim(&facade, run_id, engine.node(), lease::lease_millis(factory)))? {
        tracing::info!("agent run {run_id} is held by another node, so this node leaves it");
        engine.forget_if(run_id, local);
        return Ok(Next::Stop);
    }
    let mut sink = EventSink::new(factory, &facade, run_id, &run.user_id);
    if run.cancel_requested || local.cancel.is_canceled() {
        quietly(|| {
            transition(
                &facade,
                &mut sink,
                run_id,
                RunState::Canceled,
                &Change::reason("canceled"),
            )
        })?;
        engine.forget_if(run_id, local);
        return Ok(Next::Stop);
    }
    // an MCP call's run is driven by the MCP server, never by the pool: one taken over after its node stopped ends
    if run.surface == crate::llm::prompt::Surface::Mcp {
        return fail(
            &facade,
            &mut sink,
            run_id,
            "mcp_interrupted",
            "the MCP call was interrupted; the client may call the tool again",
        );
    }
    let user = quietly(|| user_ref(&facade, run.acting_user()))?;
    login(&user)?;
    ExecutionContext::with_current(|ec| {
        ec.set_agent_origin(Some(AgentOrigin {
            agent_id: run.agent_id.clone(),
            run_id: Some(run_id.to_string()),
        }));
    });
    let Some(agent) = quietly(|| AgentDef::load(&facade, &run.agent_id))? else {
        return fail(
            &facade,
            &mut sink,
            run_id,
            "no_agent",
            &format!("there is no agent {}", run.agent_id),
        );
    };
    if run.state == RunState::Submitted {
        quietly(|| transition(&facade, &mut sink, run_id, RunState::Working, &Change::default()))?;
    }
    let llm = crate::llm::facade(factory);
    let target = match resolve_target(&llm, &facade, &agent, &run) {
        Ok(target) => target,
        Err(error) => return fail(&facade, &mut sink, run_id, "no_model", &error.to_string()),
    };
    let variant = target.model.behavior.prompt_variant;
    let policy = quietly(|| ToolPolicy::load(&facade, &agent.agent_id))?;
    let offered = tools::offered(
        &run.caps,
        variant,
        &policy,
        &agent.role_groups(),
        &[],
    );
    let per_tool = quietly(|| tool_counts(&facade, run_id))?;
    let budget = Budget {
        caps: run.limits.clone(),
        steps: run.step_count,
        tool_calls: run.tool_call_count,
        tokens: run.input_tokens + run.output_tokens,
        cost: run.cost,
        active_ms: run.active_seconds * 1000,
        ..Budget::default()
    };
    let screen = (run.caps.has(caps::SCREEN) && crate::llm::screen::installed())
        .then(|| ScreenHandle::new(&run.conversation_id).sized(target.model.behavior.screen_size));
    let model_tool_calls = target.model.behavior.max_tool_calls.unwrap_or(match variant {
        crate::llm::conf::PromptVariant::Small => SMALL_TOOL_CALLS,
        crate::llm::conf::PromptVariant::Standard => STANDARD_TOOL_CALLS,
    });
    let mut turn = Turn {
        screen,
        factory: Arc::clone(factory),
        facade: facade.clone(),
        run_ref: RunRef {
            run_id: run_id.to_string(),
            conversation_id: run.conversation_id.clone(),
            step_seq: i64::from(run.step_count) + 1,
        },
        run,
        user,
        agent,
        offered,
        policy,
        budget,
        sink,
        local: Arc::clone(local),
        result_cap: target
            .model
            .behavior
            .result_cap_chars
            .and_then(|cap| usize::try_from(cap).ok())
            .unwrap_or(DEFAULT_RESULT_CAP),
        list_limit: target
            .model
            .behavior
            .list_limit
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or(0),
        parallel: if target.model.behavior.parallel_tool_calls == Some(false) {
            1
        } else {
            engine.parallel_reads()
        },
        per_tool,
        target: Some(target.clone()),
        model_tool_calls,
        side: std::sync::Mutex::new(SideSpend::default()),
    };
    let started = Instant::now();
    let next = turn.work(&target, started);
    if local.is_lost() {
        turn.sink.discard();
        return next;
    }
    let elapsed = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let node = engine.node().to_string();
    if let Err(error) = quietly(|| add_active_time(&facade, run_id, &node, elapsed)) {
        tracing::error!("the working time of agent run {run_id} could not be stored: {error}");
    }
    turn.sink.flush_logged();
    next
}

/// The record references the conversation's tool results and user messages hold (plan 7.7).
fn seen_references(facade: &EntityFacade, conversation_id: &str) -> Result<tools::format::Seen, Error> {
    let messages = facade
        .find(MESSAGE)?
        .eq("conversationId", Value::string(conversation_id))
        .condition(Condition::is_in(
            "role",
            vec![Value::string("tool"), Value::string("user")],
        ))
        .disable_authz()
        .list()?;
    let ids: Vec<Value> = messages
        .values()
        .iter()
        .filter_map(|row| text_of(row, "messageId"))
        .map(Value::string)
        .collect();
    let mut seen = tools::format::Seen::default();
    if ids.is_empty() {
        return Ok(seen);
    }
    let parts = facade
        .find(PART)?
        .condition(Condition::is_in("messageId", ids))
        .disable_authz()
        .list()?;
    for part in parts.values() {
        if let Some(text) = text_of(part, "textValue") {
            seen.add(&text);
        }
    }
    Ok(seen)
}

fn fail(facade: &EntityFacade, sink: &mut EventSink, run_id: &str, code: &str, text: &str) -> Result<Next, Error> {
    sink.emit("error", json!({"code": code, "message": text}));
    quietly(|| transition(facade, sink, run_id, RunState::Failed, &Change::failed(code, text)))?;
    Ok(Next::Stop)
}

fn user_ref(facade: &EntityFacade, user_id: &str) -> Result<UserRef, Error> {
    let row = facade
        .find("moqui.security.UserAccount")?
        .eq("userId", Value::string(user_id))
        .disable_authz()
        .one()?
        .ok_or_else(|| Error::new(format!("there is no user {user_id}")))?;
    Ok(UserRef {
        user_id: user_id.to_string(),
        username: text_of(&row, "username").unwrap_or_else(|| user_id.to_string()),
    })
}

/// Log in as the run's user in the context entered on this thread.
fn login(user: &UserRef) -> Result<(), Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    crate::user::internal_login_user(&user.username).map_err(|error| Error::new(error.to_string()))
}

fn tool_counts(facade: &EntityFacade, run_id: &str) -> Result<HashMap<String, u32>, Error> {
    let mut counts = HashMap::new();
    for row in facade
        .find(TOOL_CALL)?
        .eq("runId", Value::string(run_id))
        .condition(Condition::is_in(
            "statusId",
            ["AgtcRunning", "AgtcDone", "AgtcFailed", "AgtcUnknownOutcome"]
                .into_iter()
                .map(Value::string)
                .collect(),
        ))
        .disable_authz()
        .list()?
        .values()
    {
        *counts.entry(text_of(row, "toolName").unwrap_or_default()).or_insert(0) += 1;
    }
    Ok(counts)
}

fn add_active_time(facade: &EntityFacade, run_id: &str, node: &str, millis: i64) -> Result<(), Error> {
    in_tx(facade, || {
        // a step that paused or ended the run gave its lease up already
        let mut row = held(facade, run_id, node, Hold::LeaseOrNone)?;
        let carried = int_of(&row, "activeSeconds").unwrap_or(0) * 1000;
        row.set("activeSeconds", Value::I64((carried + millis + 500) / 1000));
        row.update(&facade.clone())
    })
}

/// How the tool calls of a step came out.
enum ToolsEnd {
    Continue,
    Paused,
    Canceled,
}

impl Turn {
    fn work(&mut self, target: &Target, started: Instant) -> Result<Next, Error> {
        let facade = self.facade.clone();
        let run_id = self.run.run_id.clone();
        // the input that came meanwhile: an answer resumes a paused call, a message is folded in
        let mut inputs = quietly(|| open_inputs(&facade, &run_id))?;
        let open: Vec<StoredCall> = quietly(|| last_calls(&facade, &run_id))?
            .into_iter()
            .filter(StoredCall::is_open)
            .collect();
        if !open.is_empty() {
            match self.run_calls(open, &mut inputs)? {
                ToolsEnd::Continue => {}
                ToolsEnd::Paused => return Ok(Next::Stop),
                ToolsEnd::Canceled => return self.end_canceled(),
            }
        }
        let node = self.sink.node().to_string();
        for input in inputs {
            let folded = quietly(|| {
                in_tx(&facade, || {
                    held(&facade, &run_id, &node, Hold::Lease)?;
                    // read again under its lock: the user may have edited it (its text now) or canceled it (taken)
                    let Some(input) = locked_input(&facade, &input)? else {
                        return Ok(false);
                    };
                    let tail = input.tail();
                    let metadata = tail.map(|tail| json!({"tail": tail}));
                    let attachments: Vec<Json> = input
                        .data
                        .as_ref()
                        .and_then(|data| data.get("attachments"))
                        .and_then(Json::as_array)
                        .cloned()
                        .unwrap_or_default();
                    append_message(
                        &facade,
                        &self.run.conversation_id,
                        Some(&run_id),
                        "user",
                        Some(&self.user.user_id),
                        input.client_message_id.as_deref(),
                        &message_parts(&input.text, &attachments),
                        metadata.as_ref(),
                        None,
                    )?;
                    if input.text.contains('@') {
                        let index = crate::llm::skills::SkillIndex::for_audience(
                            &self.factory,
                            &facade,
                            &crate::llm::skills::Audience {
                                user_id: Some(&self.user.user_id),
                                agent_id: Some(&self.agent.agent_id),
                                surface: Some(self.run.surface),
                            },
                        );
                        crate::llm::skills::mention::append(
                            &facade,
                            &index,
                            &self.run.conversation_id,
                            &run_id,
                            &self.user.user_id,
                            self.run.surface,
                            &input.text,
                        )?;
                    }
                    consume_input(&facade, &input.input_id)?;
                    Ok(true)
                })
            })?;
            if folded {
                self.sink
                    .emit("input.folded", json!({"inputId": input.input_id, "kind": input.kind}));
            }
        }
        if let Some(exhausted) = self.exhausted()? {
            return self.budget_stop(exhausted);
        }
        if self.local.cancel.is_canceled() {
            return self.end_canceled();
        }
        self.call_model(target, started)
    }

    fn exhausted(&self) -> Result<Option<Exhausted>, Error> {
        if let Some(exhausted) = self.budget.exhausted() {
            return Ok(Some(exhausted));
        }
        let limits = crate::llm::facade(&self.factory).conf().limits.clone();
        let quota = quietly(|| {
            crate::llm::usage::check_quota(&self.facade, &limits, &self.run.user_id, "agent", now_millis())
        })?;
        Ok(quota.exceeded.is_some().then_some(Exhausted::Quota))
    }

    /// A cap is reached: an interactive run asks whether to go on, a background one (or a used-up quota) fails.
    fn budget_stop(&mut self, exhausted: Exhausted) -> Result<Next, Error> {
        let facade = self.facade.clone();
        let run_id = self.run.run_id.clone();
        if exhausted == Exhausted::Quota || !self.run.caps.interactive() {
            let text = format!("the run reached its {} limit", exhausted.as_str());
            return fail(&facade, &mut self.sink, &run_id, "budget_exhausted", &text);
        }
        let text = format!("This run reached its limit of {}. Continue?", exhausted.as_str());
        let node = self.sink.node().to_string();
        let question_id = quietly(|| {
            in_tx(&facade, || {
                held(&facade, &run_id, &node, Hold::Lease)?;
                let mut row = facade.make_value(QUESTION)?;
                row.set_sequenced_id_primary(&facade)?;
                row.set("runId", Value::string(&run_id));
                row.set("userId", Value::string(&self.run.user_id));
                row.set("questionText", Value::string(&text));
                row.set("choicesJson", Value::string(json!(["continue", "stop"]).to_string()));
                row.set("statusId", Value::string("AgqOpen"));
                row.set("createdDate", stamp(now_millis()));
                row.create(&facade)?;
                Ok(text_of(&row, "questionId").unwrap_or_default())
            })
        })?;
        self.sink.emit(
            "question.asked",
            json!({"questionId": question_id, "text": text, "choices": ["continue", "stop"], "kind": "budget",
                "exhausted": exhausted.as_str()}),
        );
        let pause = tools::Pause {
            kind: tools::PauseKind::Budget,
            reference: question_id,
            expires: None,
            event: None,
        };
        quietly(|| {
            transition(
                &facade,
                &mut self.sink,
                &run_id,
                RunState::InputRequired,
                &Change {
                    reason: Some(format!("budget: {}", exhausted.as_str())),
                    pause: Some(pause),
                    ..Change::default()
                },
            )
        })?;
        super::engine(&self.factory).forget_if(&run_id, &self.local);
        Ok(Next::Stop)
    }

    fn end_canceled(&mut self) -> Result<Next, Error> {
        if self.local.is_lost() {
            return Ok(Next::Stop);
        }
        let facade = self.facade.clone();
        let run_id = self.run.run_id.clone();
        quietly(|| {
            transition(
                &facade,
                &mut self.sink,
                &run_id,
                RunState::Canceled,
                &Change::reason("canceled"),
            )
        })?;
        super::engine(&self.factory).forget_if(&run_id, &self.local);
        Ok(Next::Stop)
    }

    /// The skill catalog of a new conversation: the user's index for the agent and surface, by the agent's pins, the
    /// tools offered, and the first message with the page it was sent from.
    fn catalog(&self, tools: &[provider::ToolSpec], history: &[Message], tail: Option<&str>) -> Catalog {
        let index = crate::llm::skills::SkillIndex::for_audience(
            &self.factory,
            &self.facade,
            &crate::llm::skills::Audience {
                user_id: Some(&self.run.user_id),
                agent_id: Some(&self.agent.agent_id),
                surface: Some(self.run.surface),
            },
        );
        let pins = crate::llm::skills::pins(&self.facade, &self.agent.agent_id);
        let names: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
        let first = history
            .iter()
            .find(|message| message.role == Role::User)
            .map(|message| message.text.as_str())
            .unwrap_or_default();
        let page = tail.and_then(crate::llm::skills::tail_page);
        crate::llm::skills::catalog(
            &index,
            &crate::llm::skills::CatalogInput {
                pins: &pins,
                first_text: first,
                page: page.as_deref(),
                tools: &names,
            },
        )
    }

    /// The request of a call: the conversation's frozen prefix (made now for a new conversation), its history with what
    /// compaction did, and the tail of the newest message; and the history and prefix it was made from.
    fn build_request(&self, target: &Target) -> Result<(ModelRequest, window::History, prompt::FrozenPrefix), Error> {
        let facade = &self.facade;
        let tools: Vec<provider::ToolSpec> = self
            .offered
            .iter()
            .map(|offered| offered.tool.spec().to_provider())
            .collect();
        let conversation_id = self.run.conversation_id.clone();
        let loaded = quietly(|| window::load(facade, &conversation_id))?;
        let (history, tail) = loaded.messages();
        let frozen = quietly(|| {
            prompt::freeze(facade, &conversation_id, &tools, || {
                let mut catalog = self.catalog(&tools, &history, tail.as_deref());
                if let Some(size) = target.model.behavior.catalog_size.and_then(|size| usize::try_from(size).ok()) {
                    if catalog.entries.len() > size {
                        catalog.more += catalog.entries.len() - size;
                        catalog.entries.truncate(size);
                    }
                }
                let user_facts =
                    crate::llm::facts::collect(&self.factory, facade, &self.run.user_id, &self.agent.agent_id);
                let facts = &user_facts.facts;
                let memory =
                    prompt::user_memory(facade, &self.run.user_id, &self.agent.agent_id).unwrap_or_else(|error| {
                        tracing::error!("the agent memory of {} could not be read: {error}", self.run.user_id);
                        Vec::new()
                    });
                let fact = |label: &str| {
                    facts
                        .iter()
                        .find(|(name, _)| name == label)
                        .map(|(_, value)| value.clone())
                };
                let vars = PromptVars {
                    user_name: fact("name")
                        .or_else(|| fact("username"))
                        .unwrap_or_else(|| "the user".to_string()),
                    language_name: prompt::language_name(fact("locale").as_deref()),
                    room_name: crate::llm::chat::room_name(facade, &conversation_id).unwrap_or_default(),
                    ..PromptVars::default()
                };
                let instructions = self
                    .agent
                    .row
                    .as_ref()
                    .map(|row| prompt::agent_instructions(&self.factory, row))
                    .unwrap_or_default();
                prompt::build_prefix(
                    &PromptTexts::of(&self.factory),
                    &PrefixSpec {
                        agent_id: &self.agent.agent_id,
                        surface: self.run.surface,
                        variant: target.model.behavior.prompt_variant,
                        vars: &vars,
                        agent_instructions: &instructions,
                        org_instructions: &user_facts.org_instructions,
                        catalog: &catalog,
                        user_facts: facts,
                        memory: &memory,
                        tools: &tools,
                        tool_mode: target.model.behavior.tool_mode,
                    },
                )
            })
        })?;
        let request = request_from(&frozen, &loaded, target, &conversation_id);
        Ok((request, loaded, frozen))
    }

    /// The intent pre-call (`intent-prefilter="hint"`, plan 7.9): before the first call for a new user message, a tiny
    /// call names its likely intent, which goes into the message's stored tail as `likely: data`, so every later call
    /// sends the same bytes. A call that fails or answers no intent adds nothing. Out: whether the tail changed.
    fn intent_hint(&self, target: &Target, history: &mut window::History) -> bool {
        let Some(entry) = history.entries.last() else {
            return false;
        };
        if target.model.behavior.no_side_calls
            || entry.message.role != Role::User
            || entry.tail.as_deref().is_some_and(|tail| tail.contains("\nlikely: "))
        {
            return false;
        }
        let prompt = PromptTexts::of(&self.factory).intent;
        if prompt.is_empty() {
            return false;
        }
        let message: String = entry.message.text.chars().take(600).collect();
        let request = ModelRequest {
            system: vec![prompt],
            history: vec![Message::user(format!("route({})", json!(prompt::elide_markers(&message))))],
            reasoning: Some(crate::llm::conf::ReasoningLevel::None),
            max_output: Some(16),
            ..ModelRequest::default()
        };
        let llm = crate::llm::facade(&self.factory);
        let mut options = self.side_options(&llm, Duration::from_secs(30));
        let reply = provider::call(target, &request, &mut options);
        moqui_lib::sync::lock(&self.side).add(&reply);
        let Some(intent) = (reply.finish == Finish::Stop)
            .then(|| crate::llm::json_mode::parse_intent(&reply.text))
            .flatten()
        else {
            tracing::info!(
                "the intent pre-call of agent run {} answered no intent ({})",
                self.run.run_id,
                reply.finish.as_str()
            );
            return false;
        };
        let tail = crate::llm::json_mode::tail_with_intent(
            entry.tail.as_deref().unwrap_or("<context>\n</context>"),
            intent,
        );
        let message_id = entry.message_id.clone();
        let mut metadata = entry.metadata.clone();
        metadata.insert("tail".to_string(), json!(tail));
        let node = self.sink.node().to_string();
        let stored = quietly(|| {
            in_tx(&self.facade, || {
                held(&self.facade, &self.run.run_id, &node, Hold::Lease)?;
                self.facade
                    .find(MESSAGE)?
                    .eq("messageId", Value::string(&message_id))
                    .disable_authz()
                    .update_all([("metadataJson".to_string(), Value::string(Json::Object(metadata.clone()).to_string()))])
                    .map(|_| ())
            })
        });
        if let Err(error) = stored {
            tracing::error!("the intent hint of agent run {} could not be stored: {error}", self.run.run_id);
            return false;
        }
        if let Some(entry) = history.entries.last_mut() {
            entry.tail = Some(tail);
            entry.metadata = metadata;
        }
        true
    }

    /// The options of a call besides the turn's own (the intent hint, the schema repair): logged as the run's, no
    /// streaming, no retries past the provider's.
    fn side_options<'a>(&'a self, llm: &'a crate::llm::LlmFacade, limit: Duration) -> CallOptions<'a> {
        let mut options = CallOptions::new(CallSource::Agent);
        options.user_id = Some(self.run.user_id.clone());
        options.run_id = Some(self.run.run_id.clone());
        options.step_seq = Some(self.run_ref.step_seq);
        options.cancel = self.local.cancel.clone();
        options.deadline = Some(Instant::now() + limit);
        options.length_retry = false;
        options.recorder = Some(Recorder {
            llm,
            facade: &self.facade,
        });
        options
    }

    fn call_model(&mut self, target: &Target, started: Instant) -> Result<Next, Error> {
        let facade = self.facade.clone();
        let run_id = self.run.run_id.clone();
        let (mut request, mut history, frozen) = self.build_request(target)?;
        if target.model.behavior.intent_hint && self.intent_hint(target, &mut history) {
            request = request_from(&frozen, &history, target, &self.run.conversation_id);
        }
        let json_tools = frozen.json_tools;
        let window = Window::for_model(
            &target.model,
            None,
            target.reasoning != crate::llm::conf::ReasoningLevel::None,
        );
        let node = self.sink.node().to_string();
        let compactor = Compactor {
            factory: &self.factory,
            facade: &self.facade,
            run_id: &self.run.run_id,
            conversation_id: &self.run.conversation_id,
            user_id: &self.run.user_id,
            step_seq: self.run_ref.step_seq,
            node: &node,
            cancel: &self.local.cancel,
            side: &self.side,
        };
        // the window (plan 7.8): compacted in stages only over the high watermark, so the calls between compactions
        // share the prefix cache
        let estimate = estimate(&request, &history, target);
        let mut compactions = Vec::new();
        if window::needs_compaction(estimate, &window) {
            let outcome = compactor.run(target, &request, &mut history, &window, false)?;
            request = request_from(&frozen, &history, target, &self.run.conversation_id);
            let full = outcome.full;
            compactions.push(outcome);
            if full {
                for outcome in &compactions {
                    self.sink.emit("compaction", compaction_event(self.run_ref.step_seq, outcome));
                }
                self.settle_side()?;
                return fail(
                    &facade,
                    &mut self.sink,
                    &run_id,
                    "context_full",
                    "This conversation no longer fits the model's context window. Start a new conversation, or use a \
                     model with a larger window.",
                );
            }
        }
        let mut message = quietly(|| -> Result<EntityValue, Error> {
            let mut row = facade.make_value(MESSAGE)?;
            row.set_sequenced_id_primary(&facade)?;
            Ok(row)
        })?;
        let message_id = text_of(&message, "messageId").unwrap_or_default();
        let step_seq = self.run_ref.step_seq;
        self.sink.emit(
            "message.start",
            json!({"messageId": message_id, "role": "assistant", "stepSeq": step_seq, "model": target.model.id}),
        );
        quietly(|| self.sink.flush_logged());
        let llm = crate::llm::facade(&self.factory);
        let deadline = self.budget.caps.max_run_seconds.map(|max| {
            let left = i64::try_from(max.saturating_mul(1000)).unwrap_or(i64::MAX) - self.budget.active_ms;
            started + Duration::from_millis(u64::try_from(left.max(30_000)).unwrap_or(30_000))
        });
        // a provider's overflow compacts at once, whatever the estimate said
        let mut overflowed = None;
        let mut compact = |_: &ModelRequest| -> Option<ModelRequest> {
            let outcome = match compactor.run(target, &request, &mut history, &window, true) {
                Ok(outcome) => outcome,
                Err(error) => {
                    tracing::error!("agent run {} could not be compacted after an overflow: {error}", compactor.run_id);
                    return None;
                }
            };
            let smaller = (outcome.changed() && !outcome.full)
                .then(|| request_from(&frozen, &history, target, compactor.conversation_id));
            overflowed = Some(outcome);
            smaller
        };
        let sink = &mut self.sink;
        // the program of a write_ui call as it streams, for the canvas's canvas.delta
        let mut canvases = crate::llm::ui::StreamingPrograms::default();
        let mut on_signal = |signal: Signal<'_>| match signal {
            // a JSON-mode reply is protocol text: its answer is sent once it is read
            Signal::Event(ProviderEvent::Text { .. }) if json_tools => {}
            Signal::Event(ProviderEvent::Text { delta, .. }) => sink.text_delta(&message_id, 0, delta),
            Signal::Event(ProviderEvent::Reasoning { delta }) => {
                sink.live("reasoning.delta", json!({"messageId": message_id, "text": delta}));
            }
            Signal::Event(ProviderEvent::ToolStart { index, call_id, name }) => {
                canvases.start(*index, name);
                sink.live(
                    "tool.args",
                    json!({"messageId": message_id, "index": index, "callId": call_id, "name": name, "text": ""}),
                );
            }
            Signal::Event(ProviderEvent::ToolArgs { index, delta }) => {
                sink.live(
                    "tool.args",
                    json!({"messageId": message_id, "index": index, "text": delta}),
                );
                if let Some(mut payload) = canvases.args(*index, delta) {
                    payload["messageId"] = json!(message_id);
                    sink.live("canvas.delta", payload);
                }
            }
            Signal::Event(_) => {}
            Signal::Restart { attempt, reason } => sink.restart(&message_id, attempt, reason),
        };
        let mut options = CallOptions::new(CallSource::Agent);
        options.user_id = Some(self.run.user_id.clone());
        options.run_id = Some(run_id.clone());
        options.step_seq = Some(step_seq);
        options.cancel = self.local.cancel.clone();
        options.deadline = deadline;
        options.nudge = Some(provider::EMPTY_NUDGE.to_string());
        options.compact = Some(&mut compact);
        options.recorder = Some(Recorder {
            llm: &llm,
            facade: &facade,
        });
        options.sink = Some(&mut on_signal);
        debug_assert!(
            !crate::transaction::thread_in_transaction(),
            "no transaction may be open across a model call"
        );
        let first_request = request.clone();
        let mut reply = provider::call(target, &first_request, &mut options);
        drop(options);
        if let Some(outcome) = overflowed.take() {
            compactions.push(outcome);
            request = request_from(&frozen, &history, target, &self.run.conversation_id);
        }
        if self.local.is_lost() {
            // another node has the run: its reply is not this node's to store
            return Ok(Next::Stop);
        }
        let sent_chars = crate::llm::tokens::RequestChars::sent(&request, target).total();
        if reply.usage.input > 0 {
            crate::llm::tokens::observe(
                &target.model,
                sent_chars,
                reply.usage.input,
                crate::llm::tokens::cjk_share(&request.system_text()),
            );
        }
        // JSON tool mode: the reply is one protocol object (one retry when it is not); native mode: calls written as
        // text are taken as calls
        let mut raw = None;
        let answered = matches!(reply.finish, Finish::Stop | Finish::Length | Finish::ToolCalls);
        if json_tools && answered && reply.tool_calls.is_empty() {
            raw = self.read_json_reply(target, &request, &mut reply);
            if !reply.text.is_empty() {
                self.sink.text_delta(&message_id, 0, &reply.text);
            }
        } else if answered && reply.tool_calls.is_empty() && !reply.text.is_empty() {
            let names: Vec<&str> = self.offered.iter().map(|offered| offered.tool.spec().name.as_str()).collect();
            if let Some((calls, rest)) = crate::llm::json_mode::leaked_calls(&reply.text, &names) {
                tracing::info!(
                    "agent run {run_id} step {step_seq}: {} tool call(s) written as text were taken as calls",
                    calls.len()
                );
                self.sink.restart(&message_id, reply.attempts + 1, "tool calls written as text");
                reply.tool_calls = calls
                    .into_iter()
                    .enumerate()
                    .map(|(index, (name, arguments))| provider::ToolCall {
                        id: format!("text_{run_id}_{step_seq}_{index}"),
                        name,
                        arguments,
                    })
                    .collect();
                // what the model wrote, which a replay plays again
                raw = Some(std::mem::replace(&mut reply.text, rest));
                reply.finish = Finish::ToolCalls;
                if !reply.text.is_empty() {
                    self.sink.text_delta(&message_id, 0, &reply.text);
                }
            }
        }
        if !reply.text.is_empty() {
            self.sink.part_done(&message_id, 0);
        }
        for outcome in &compactions {
            self.sink.emit("compaction", compaction_event(step_seq, outcome));
        }
        self.settle_side()?;
        if !reply.reasoning.is_empty() {
            self.sink.emit(
                "reasoning.summary",
                json!({"messageId": message_id, "text": reply.reasoning.chars().take(500).collect::<String>()}),
            );
        }
        // the provider's count of this request anchors the next estimate (only a call made once counts it alone)
        let mut metadata = serde_json::Map::new();
        if let Some(raw) = &raw {
            metadata.insert("raw".to_string(), json!(raw));
        }
        if reply.attempts == 1 && reply.usage.input > 0 {
            message.set("tokenCount", Value::I64(i64::try_from(reply.usage.input).unwrap_or(i64::MAX)));
            metadata.insert("promptChars".to_string(), json!(sent_chars));
        }
        let metadata = (!metadata.is_empty()).then_some(Json::Object(metadata));
        let calls = quietly(|| self.store_reply(target, &reply, &mut message, metadata.as_ref()))?;
        self.budget.steps += 1;
        self.budget.tokens += reply.usage.input + reply.usage.output;
        self.budget.cost += reply.cost.unwrap_or_default();
        // the share of the prompt read from the provider's cache: a chat template that breaks the cache shows here
        let cache_ratio = cache_ratio(reply.usage.cached_input, reply.usage.input);
        self.sink.emit(
            "usage",
            json!({"stepSeq": step_seq, "input": reply.usage.input, "cachedInput": reply.usage.cached_input,
                "cacheRatio": cache_ratio, "output": reply.usage.output, "reasoning": reply.usage.reasoning,
                "cost": reply.cost.map(|cost| cost.to_string())}),
        );
        if compactions.is_empty() && step_seq > 1 && reply.usage.input > 2048 && cache_ratio.is_some_and(|ratio| ratio < 0.5)
            && reply.usage.cached_input > 0
        {
            tracing::warn!(
                "agent run {run_id} step {step_seq}: only {:.0}% of the prompt came from model {}'s cache with no \
                 compaction; its chat template may change earlier messages",
                cache_ratio.unwrap_or(0.0) * 100.0,
                target.model.id
            );
        }
        quietly(|| self.sink.flush_logged());
        let canceled = self.local.cancel.is_canceled() || reply.finish == Finish::Canceled;
        if canceled {
            return self.end_canceled();
        }
        match &reply.finish {
            Finish::Stop | Finish::ToolCalls | Finish::Length => {
                if reply.cut_tool_calls > 0 {
                    tracing::warn!(
                        "agent run {run_id} step {step_seq}: {} tool calls were cut at the output limit and dropped",
                        reply.cut_tool_calls
                    );
                }
                let open: Vec<StoredCall> = calls.into_iter().filter(StoredCall::is_open).collect();
                let any_calls = !reply.tool_calls.is_empty() || !reply.bad_tool_calls.is_empty();
                if any_calls {
                    return match self.run_calls(open, &mut Vec::new())? {
                        ToolsEnd::Continue => Ok(Next::Continue),
                        ToolsEnd::Paused => Ok(Next::Stop),
                        ToolsEnd::Canceled => self.end_canceled(),
                    };
                }
                let reason =
                    (reply.finish == Finish::Length).then(|| "the answer was cut at the output limit".to_string());
                // links to /apps/ paths no tool returned are invented references (plan 7.7): the clients show them
                // as plain text, and the evaluation counts them
                let mut result = json!({"text": reply.text, "messageId": message_id});
                let seen = quietly(|| seen_references(&facade, &self.run.conversation_id)).unwrap_or_else(|error| {
                    tracing::error!("the references of agent run {run_id} could not be read: {error}");
                    tools::format::Seen::default()
                });
                let checked = tools::format::check_answer(&reply.text, &seen);
                if !checked.invented.is_empty() {
                    self.sink.emit(
                        "answer.references",
                        json!({"messageId": message_id, "invented": checked.invented, "text": checked.text}),
                    );
                    result["invented"] = json!(checked.invented);
                }
                quietly(|| {
                    transition(
                        &facade,
                        &mut self.sink,
                        &run_id,
                        RunState::Completed,
                        &Change {
                            reason,
                            result: Some(result),
                            unless_input: true,
                            ..Change::default()
                        },
                    )
                })?;
                // a message that came during this step is folded in by the next one, which answers it
                if quietly(|| load_run(&facade, &run_id))?.is_some_and(|run| run.state == RunState::Working) {
                    return Ok(Next::Continue);
                }
                super::engine(&self.factory).forget_if(&run_id, &self.local);
                Ok(Next::Stop)
            }
            other => {
                let (code, text) = match other {
                    Finish::Error(error) => (
                        error.class.as_str().to_string(),
                        crate::llm::usage::redact(&error.to_string(), &[]),
                    ),
                    Finish::Empty => ("empty".to_string(), "the model answered nothing".to_string()),
                    Finish::ContextOverflow => (
                        "context_overflow".to_string(),
                        "the conversation is over the model's context window".to_string(),
                    ),
                    Finish::ContentFilter => (
                        "content_filter".to_string(),
                        "the provider's content filter stopped the answer".to_string(),
                    ),
                    Finish::Refusal => ("refusal".to_string(), "the model declined".to_string()),
                    _ => (other.as_str().to_string(), other.as_str().replace('_', " ")),
                };
                let ended = fail(&facade, &mut self.sink, &run_id, &code, &text);
                super::engine(&self.factory).forget_if(&run_id, &self.local);
                ended
            }
        }
    }

    /// Count what the side calls spent in the run's budget and its totals (`AgentRun` tokens and cost), under the lease.
    fn settle_side(&mut self) -> Result<(), Error> {
        let spent = std::mem::take(&mut *moqui_lib::sync::lock(&self.side));
        if spent.usage.input + spent.usage.output == 0 && spent.cost.is_zero() {
            return Ok(());
        }
        self.budget.tokens += spent.usage.input + spent.usage.output;
        self.budget.cost += spent.cost;
        let facade = self.facade.clone();
        let run_id = self.run.run_id.clone();
        let node = self.sink.node().to_string();
        quietly(|| {
            in_tx(&facade, || {
                let mut row = held(&facade, &run_id, &node, Hold::LeaseOrNone)?;
                let add = |row: &EntityValue, field: &str, value: u64| {
                    Value::I64(int_of(row, field).unwrap_or(0) + i64::try_from(value).unwrap_or(i64::MAX))
                };
                row.set("inputTokens", add(&row, "inputTokens", spent.usage.input));
                row.set("cachedInputTokens", add(&row, "cachedInputTokens", spent.usage.cached_input));
                row.set("outputTokens", add(&row, "outputTokens", spent.usage.output));
                let cost = crate::llm::decimal_of(&row, "cost").unwrap_or_default() + spent.cost;
                row.set("cost", Value::Decimal(cost));
                row.update(&facade)
            })
        })
    }

    /// A JSON-mode reply (plan 7.9) read as the protocol's object: a call, or the answer. One that is not a valid object
    /// is asked again once with the protocol's reminder; plain text is the answer. Out: the model's own text, kept as
    /// the message's `raw` when it is not what is stored, so the next request sends the bytes the model wrote.
    fn read_json_reply(&mut self, target: &Target, request: &ModelRequest, reply: &mut provider::Reply) -> Option<String> {
        use crate::llm::json_mode::{self, Parsed};
        let mut parsed = json_mode::parse_reply(&reply.text);
        if let Parsed::Invalid(why) = &parsed {
            tracing::info!(
                "agent run {} step {}: the JSON-mode reply is not a protocol object ({why}); asking once more",
                self.run.run_id,
                self.run_ref.step_seq
            );
            let mut again = request.clone();
            again.history.push(Message::assistant(reply.text.clone()));
            again.history.push(Message::user(json_mode::RETRY_NOTE));
            let llm = crate::llm::facade(&self.factory);
            let mut options = self.side_options(&llm, Duration::from_secs(300));
            let second = provider::call(target, &again, &mut options);
            drop(options);
            let mut usage = reply.usage;
            usage.add(&second.usage);
            let cost = match (reply.cost, second.cost) {
                (Some(first), Some(more)) => Some(first + more),
                (first, more) => first.or(more),
            };
            let mut call_ids = std::mem::take(&mut reply.call_ids);
            call_ids.extend(second.call_ids.iter().cloned());
            let attempts = reply.attempts + second.attempts;
            if matches!(second.finish, Finish::Stop | Finish::Length | Finish::ToolCalls) {
                *reply = second;
            }
            reply.usage = usage;
            reply.cost = cost;
            reply.call_ids = call_ids;
            reply.attempts = attempts;
            parsed = match json_mode::parse_reply(&reply.text) {
                // still no object: the text is the answer
                Parsed::Invalid(_) => Parsed::Reply(reply.text.trim().to_string()),
                other => other,
            };
        }
        let written = reply.text.clone();
        match parsed {
            Parsed::Call { name, args } => {
                reply.tool_calls = vec![provider::ToolCall {
                    // unique in the conversation: the window and the canvases find a call by its id
                    id: format!("json_{}_{}_0", self.run.run_id, self.run_ref.step_seq),
                    name,
                    arguments: args,
                }];
                reply.text = String::new();
                reply.finish = Finish::ToolCalls;
                Some(written)
            }
            Parsed::Reply(text) => {
                let raw = (text != written).then_some(written);
                reply.text = text;
                raw
            }
            Parsed::Invalid(_) => None,
        }
    }

    /// Store the step, the assistant message and its tool calls, and the run's totals, in one transaction. A call whose
    /// arguments did not parse is stored failed, with a result telling the model what was wrong. Out: the calls, in
    /// model order (the complete ones, then the bad ones).
    fn store_reply(
        &mut self,
        target: &Target,
        reply: &provider::Reply,
        message: &mut EntityValue,
        metadata: Option<&Json>,
    ) -> Result<Vec<StoredCall>, Error> {
        let facade = self.facade.clone();
        let run = self.run.clone();
        let step_seq = self.run_ref.step_seq;
        let now = now_millis();
        let class_of = |name: &str| {
            self.offered
                .iter()
                .find(|offered| offered.tool.spec().name == name)
                .map(|offered| offered.tool.spec().classification)
        };
        let mut wanted: Vec<(String, String, Json, Option<String>)> = reply
            .tool_calls
            .iter()
            .map(|call| (call.id.clone(), call.name.clone(), call.arguments.clone(), None))
            .collect();
        wanted.extend(reply.bad_tool_calls.iter().map(|call| {
            (
                call.id.clone(),
                call.name.clone(),
                json!({"raw": call.raw_arguments}),
                Some(call.error.clone()),
            )
        }));
        let node = self.sink.node().to_string();
        let (calls, bad) = in_tx(&facade, || {
            held(&facade, &run.run_id, &node, Hold::Lease)?;
            let mut step = facade.make_value("moqui.agent.AgentRunStep")?;
            step.set("runId", Value::string(&run.run_id));
            step.set("stepSeq", Value::I64(step_seq));
            if let Some(call_id) = reply.call_ids.last() {
                step.set("callId", Value::string(call_id));
            }
            step.set("modelId", Value::string(&target.model.id));
            step.set("finishReason", Value::string(reply.finish.as_str()));
            for (field, value) in [
                ("inputTokens", reply.usage.input),
                ("cachedInputTokens", reply.usage.cached_input),
                ("cacheWriteTokens", reply.usage.cache_write),
                ("outputTokens", reply.usage.output),
                ("reasoningTokens", reply.usage.reasoning),
            ] {
                step.set(field, Value::I64(i64::try_from(value).unwrap_or(i64::MAX)));
            }
            if let Some(cost) = reply.cost {
                step.set("cost", Value::Decimal(cost));
            }
            step.set("toolCallCount", Value::I64(i64::try_from(wanted.len()).unwrap_or(0)));
            step.set("startedDate", stamp(now));
            step.set("endedDate", stamp(now));
            step.create(&facade)?;
            let mut parts = Vec::new();
            if !reply.text.is_empty() {
                parts.push(Part::text(&reply.text));
            }
            if !reply.reasoning.is_empty() {
                parts.push(Part {
                    kind: "AgpkReasoning",
                    text: Some(reply.reasoning.chars().take(2000).collect()),
                    ..Part::default()
                });
            }
            let mut calls = Vec::new();
            let mut bad = Vec::new();
            for (provider_id, name, arguments, error) in &wanted {
                let mut row = facade.make_value(TOOL_CALL)?;
                row.set_sequenced_id_primary(&facade)?;
                let tool_call_id = text_of(&row, "toolCallId").unwrap_or_default();
                let class = class_of(name);
                let status = if error.is_some() { "AgtcFailed" } else { "AgtcPending" };
                row.set("providerCallId", Value::string(provider_id));
                row.set("runId", Value::string(&run.run_id));
                row.set("stepSeq", Value::I64(step_seq));
                row.set("toolName", Value::string(name));
                if let Some(class) = class {
                    row.set("classificationEnumId", Value::string(class.enum_id()));
                }
                row.set("argumentsJson", Value::string(arguments.to_string()));
                row.set(
                    "argumentsHash",
                    Value::string(moqui_lib::json::canonical_sha256(arguments)),
                );
                // a bad call is closed with its result below
                row.set("statusId", Value::string("AgtcPending"));
                row.set("surfaceEnumId", Value::string(run.surface.enum_id()));
                row.set("userId", Value::string(&run.user_id));
                row.create(&facade)?;
                parts.push(Part {
                    kind: "AgpkToolCall",
                    data: Some(json!({"id": provider_id, "name": name, "arguments": arguments})),
                    tool_call_id: Some(tool_call_id.clone()),
                    ..Part::default()
                });
                let stored = StoredCall {
                    tool_call_id,
                    provider_id: provider_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    status: status.to_string(),
                    classification: class,
                };
                if let Some(error) = error {
                    bad.push((stored.clone(), error.clone()));
                }
                calls.push(stored);
            }
            append_message_as(
                &facade,
                &run.conversation_id,
                Some(&run.run_id),
                "assistant",
                None,
                None,
                &parts,
                metadata,
                reply.replay.as_ref(),
                Some(message.clone()),
            )?;
            let Some(mut row) = facade
                .find(RUN)?
                .eq("runId", Value::string(&run.run_id))
                .for_update(true)
                .disable_authz()
                .one()?
            else {
                return Err(Error::new(format!("there is no agent run {}", run.run_id)));
            };
            let add = |row: &EntityValue, field: &str, value: u64| {
                Value::I64(int_of(row, field).unwrap_or(0) + i64::try_from(value).unwrap_or(i64::MAX))
            };
            row.set("stepCount", Value::I64(step_seq));
            row.set("inputTokens", add(&row, "inputTokens", reply.usage.input));
            row.set(
                "cachedInputTokens",
                add(&row, "cachedInputTokens", reply.usage.cached_input),
            );
            row.set("outputTokens", add(&row, "outputTokens", reply.usage.output));
            let cost = crate::llm::decimal_of(&row, "cost").unwrap_or_default() + reply.cost.unwrap_or_default();
            row.set("cost", Value::Decimal(cost));
            row.update(&facade)?;
            Ok((calls, bad))
        })?;
        let message_id = text_of(message, "messageId");
        for call in &calls {
            self.sink.emit(
                "tool.call",
                json!({"toolCallId": call.tool_call_id, "messageId": message_id, "name": call.name,
                    "arguments": call.arguments, "classification": call.classification.map(Classification::as_str)}),
            );
        }
        for (call, error) in &bad {
            let text = format!(
                "The arguments of this {} call are not a JSON object ({error}). Call it again with valid JSON arguments.",
                call.name
            );
            store_result(
                &facade,
                &run,
                call,
                &text,
                true,
                "AgtcFailed",
                &mut self.sink,
                Hold::Lease,
            )?;
        }
        Ok(calls)
    }

    /// Run the open calls in model order; consecutive READ calls run in parallel when the model allows it. A call left
    /// running by a node that stopped is `UnknownOutcome`: a READ runs again, anything else is reported to the model and
    /// not run. A paused call resumes with the first input for it.
    fn run_calls(&mut self, calls: Vec<StoredCall>, inputs: &mut Vec<OpenInput>) -> Result<ToolsEnd, Error> {
        let facade = self.facade.clone();
        let mut index = 0;
        while index < calls.len() {
            if self.local.cancel.is_canceled() {
                return Ok(ToolsEnd::Canceled);
            }
            let call = &calls[index];
            let offered = self
                .offered
                .iter()
                .find(|offered| offered.tool.spec().name == call.name)
                .cloned();
            match call.status.as_str() {
                "AgtcRunning" | "AgtcUnknownOutcome" => {
                    let read = offered
                        .as_ref()
                        .is_some_and(|offered| offered.tool.spec().classification.is_read());
                    quietly(|| {
                        set_call_status(
                            &facade,
                            &self.run.run_id,
                            self.sink.node(),
                            &call.tool_call_id,
                            "AgtcUnknownOutcome",
                        )
                    })?;
                    if !read {
                        let text = format!(
                            "The outcome of this {} call is unknown: the server working on it stopped before it finished. \
                             Check whether it took effect before you call it again.",
                            call.name
                        );
                        quietly(|| {
                            store_result(
                                &facade,
                                &self.run,
                                call,
                                &text,
                                true,
                                "AgtcUnknownOutcome",
                                &mut self.sink,
                                Hold::Lease,
                            )
                        })?;
                        index += 1;
                        continue;
                    }
                    // a read runs again
                }
                "AgtcAwaiting" => {
                    let at = inputs
                        .iter()
                        .position(|input| input.kind != "message")
                        .or_else(|| (!inputs.is_empty()).then_some(0));
                    let input = at.map(|at| inputs.remove(at));
                    let outcome = match (&offered, &input) {
                        (Some(offered), Some(input)) => {
                            let node = self.sink.node().to_string();
                            quietly(|| {
                                in_tx(&facade, || {
                                    held(&facade, &self.run.run_id, &node, Hold::Lease)?;
                                    consume_input(&facade, &input.input_id)
                                })
                            })?;
                            let args = tools::repair::repair(&call.arguments, &offered.tool.spec().parameters).args;
                            self.invoke(call, offered, args, Some(&input.tool_input()))
                        }
                        (None, _) => ToolOutcome::Failed(format!("There is no tool named {} now.", call.name)),
                        (_, None) => ToolOutcome::Failed("No answer came for this call.".to_string()),
                    };
                    if let Some(pause) = self.finish_call(call, outcome)? {
                        return self.pause(pause);
                    }
                    index += 1;
                    continue;
                }
                _ => {}
            }
            // a run of consecutive reads that may go together
            let mut batch = vec![index];
            if self.parallel > 1 {
                while batch.len() < self.parallel {
                    let next = index + batch.len();
                    let Some(candidate) = calls.get(next) else {
                        break;
                    };
                    let is_read = |call: &StoredCall| {
                        self.offered
                            .iter()
                            .find(|offered| offered.tool.spec().name == call.name)
                            .is_some_and(|offered| {
                                offered.tool.spec().classification.is_read() && !offered.tool.spec().may_pause
                            })
                    };
                    if candidate.status != "AgtcPending" || !is_read(call) || !is_read(candidate) {
                        break;
                    }
                    batch.push(next);
                }
            }
            let mut prepared = Vec::new();
            // each call of the batch counts against the caps as it is prepared, before any of them starts
            let mut planned: HashMap<String, u32> = HashMap::new();
            for at in &batch {
                let call = &calls[*at];
                match self.prepare(call, &planned) {
                    Ok((offered, args)) => {
                        *planned.entry(call.name.clone()).or_insert(0) += 1;
                        prepared.push((call.clone(), offered, args));
                    }
                    Err(text) => {
                        let text = self.repeated(call, &text).unwrap_or(text);
                        quietly(|| {
                            store_result(
                                &facade,
                                &self.run,
                                call,
                                &text,
                                true,
                                "AgtcFailed",
                                &mut self.sink,
                                Hold::Lease,
                            )
                        })?;
                    }
                }
            }
            index += batch.len();
            // a schema repair call of the batch's preparing
            self.settle_side()?;
            let outcomes = if prepared.len() > 1 {
                self.invoke_parallel(&prepared)?
            } else {
                let mut outcomes = Vec::new();
                for (call, offered, args) in &prepared {
                    self.start_call(call)?;
                    outcomes.push(self.invoke(call, offered, args.clone(), None));
                }
                outcomes
            };
            let mut paused = None;
            for ((call, _, _), outcome) in prepared.iter().zip(outcomes) {
                if let Some(pause) = self.finish_call(call, outcome)? {
                    paused.get_or_insert(pause);
                }
            }
            if let Some(pause) = paused {
                return self.pause(pause);
            }
            quietly(|| self.sink.flush_logged());
        }
        Ok(ToolsEnd::Continue)
    }

    /// The checks before a call: the tool is offered, the run and the tool have calls left, the arguments fit.
    /// `planned` are the calls of the batch prepared before this one, by tool, not counted yet.
    fn prepare(&self, call: &StoredCall, planned: &HashMap<String, u32>) -> Result<(Offered, Json), String> {
        let Some(offered) = self
            .offered
            .iter()
            .find(|offered| offered.tool.spec().name == call.name)
        else {
            let names: Vec<&str> = self
                .offered
                .iter()
                .map(|offered| offered.tool.spec().name.as_str())
                .collect();
            let lazy = tools::lazy_names(&self.facade);
            return Err(tools::suggest::unknown_tool(&call.name, &names, &lazy).to_text());
        };
        let planned_total: u32 = planned.values().sum();
        let calls_so_far = self.budget.tool_calls + planned_total;
        if self.budget.caps.max_tool_calls.is_some_and(|max| calls_so_far >= max) {
            return Err("This run has used all of its tool calls.".to_string());
        }
        // the model's own budget a turn (plan 7.9: 12 for a small model, 30 for a large one)
        if calls_so_far >= self.model_tool_calls {
            return Err(tools::format::ToolError::new(
                "budget",
                format!(
                    "This turn has used its {} tool calls. Answer with what the results so far show, and say what is left.",
                    self.model_tool_calls
                ),
            )
            .to_text());
        }
        if let Some(limit) = offered.rule.calls_per_run {
            let so_far =
                self.per_tool.get(&call.name).copied().unwrap_or(0) + planned.get(&call.name).copied().unwrap_or(0);
            if so_far >= limit {
                return Err(format!(
                    "{} may be called {limit} times in a run, and it was.",
                    call.name
                ));
            }
        }
        // the arguments fitted to the schema first (plan 7.9); what is left over is reported with the result
        let schema = &offered.tool.spec().parameters;
        let repaired = tools::repair::repair(&call.arguments, schema);
        let Err(problem) = tools::schema::check_arguments(&repaired.args, schema) else {
            return Ok((offered.clone(), repaired.args));
        };
        // then, for a model with JSON-schema output, one schema-constrained repair call
        if let Some(args) = self.repair_with_model(call, schema, &repaired.args, &problem) {
            return Ok((offered.clone(), args));
        }
        Err(tools::format::ToolError::new(
            "arguments",
            format!(
                "The arguments of {} do not fit its schema: {problem}. Call it again with arguments that fit.",
                call.name
            ),
        )
        .ignored(repaired.ignored.clone())
        .to_text())
    }

    /// The schema-constrained repair of arguments the deterministic repair could not fit (plan 7.9, after Twenty): one
    /// call to the step's model with the tool's schema as the response schema, only when the model's `json-schema`
    /// verdict is yes. A reply that is prose, or does not fit either, is refused loudly in the log and gives `None`.
    fn repair_with_model(&self, call: &StoredCall, schema: &Json, args: &Json, problem: &str) -> Option<Json> {
        let target = self.target.as_ref()?;
        if schema.is_null() || target.model.behavior.no_side_calls {
            return None;
        }
        let verdicts = crate::llm::registry::capabilities(&self.facade, &target.model.id).ok()?;
        if verdicts.get("json-schema").map(|verdict| verdict.verdict) != Some(crate::llm::registry::Verdict::Yes) {
            return None;
        }
        let request = ModelRequest {
            system: vec![SCHEMA_REPAIR.to_string()],
            history: vec![Message::user(format!(
                "Tool: {}\nSchema: {schema}\nArguments: {args}\nProblem: {problem}",
                call.name
            ))],
            reasoning: Some(crate::llm::conf::ReasoningLevel::None),
            max_output: Some(1024),
            response_schema: Some(provider::ResponseSchema {
                name: "arguments".to_string(),
                schema: schema.clone(),
                strict: false,
            }),
            ..ModelRequest::default()
        };
        let llm = crate::llm::facade(&self.factory);
        let mut options = self.side_options(&llm, Duration::from_secs(60));
        let reply = provider::call(target, &request, &mut options);
        moqui_lib::sync::lock(&self.side).add(&reply);
        if reply.finish != Finish::Stop {
            tracing::warn!(
                "the schema repair of {} in agent run {} ended {}",
                call.name,
                self.run.run_id,
                reply.finish.as_str()
            );
            return None;
        }
        let fixed = match provider::reply_json(&reply.text) {
            Ok(fixed) => fixed,
            Err(why) => {
                // structure was asked for: prose is a failure, never an argument
                tracing::error!(
                    "the schema repair of {} in agent run {} answered prose, not the arguments: {why}",
                    call.name,
                    self.run.run_id
                );
                return None;
            }
        };
        let fixed = tools::repair::repair(&fixed, schema).args;
        match tools::schema::check_arguments(&fixed, schema) {
            Ok(()) => {
                tracing::info!("the arguments of {} in agent run {} were fitted by a repair call", call.name, self.run.run_id);
                Some(fixed)
            }
            Err(still) => {
                tracing::error!(
                    "the schema repair of {} in agent run {} still does not fit: {still}",
                    call.name,
                    self.run.run_id
                );
                None
            }
        }
    }

    /// A failure that this run's same call (the tool and the hash of its arguments) already had: the `repeated` error
    /// (plan 7.9), so a small model stops trying it.
    fn repeated(&self, call: &StoredCall, text: &str) -> Option<String> {
        let hash = moqui_lib::json::canonical_sha256(&call.arguments);
        let summary: String = text.chars().take(SUMMARY_CAP).collect();
        let earlier = quietly(|| {
            self.facade
                .find(TOOL_CALL)?
                .eq("runId", Value::string(&self.run.run_id))
                .eq("toolName", Value::string(&call.name))
                .eq("argumentsHash", Value::string(&hash))
                .eq("statusId", Value::string("AgtcFailed"))
                .disable_authz()
                .list()
        })
        .ok()?;
        let same = earlier.values().iter().any(|row| {
            text_of(row, "toolCallId").as_deref() != Some(call.tool_call_id.as_str())
                && text_of(row, "resultSummary").as_deref() == Some(summary.trim())
        });
        same.then(|| {
            let first = text.lines().next().unwrap_or(text);
            tools::format::ToolError::new(
                "repeated",
                format!(
                    "This {} call failed the same way before. Do not call it again with these arguments: tell the user \
                     what failed, or try another way.",
                    call.name
                ),
            )
            .hint(first.chars().take(300).collect::<String>())
            .to_text()
        })
    }

    /// Mark a call running and count it.
    fn start_call(&mut self, call: &StoredCall) -> Result<(), Error> {
        let facade = self.facade.clone();
        let run_id = self.run.run_id.clone();
        quietly(|| {
            in_tx(&facade, || {
                set_call_status(
                    &facade,
                    &self.run.run_id,
                    self.sink.node(),
                    &call.tool_call_id,
                    "AgtcRunning",
                )?;
                let Some(mut row) = facade
                    .find(RUN)?
                    .eq("runId", Value::string(&run_id))
                    .for_update(true)
                    .disable_authz()
                    .one()?
                else {
                    return Ok(());
                };
                row.set(
                    "toolCallCount",
                    Value::I64(int_of(&row, "toolCallCount").unwrap_or(0) + 1),
                );
                row.update(&facade)
            })
        })?;
        self.budget.tool_calls += 1;
        *self.per_tool.entry(call.name.clone()).or_insert(0) += 1;
        Ok(())
    }

    /// How a call of `offered` is approved: the policy's rule for a write of it (a READ tool is judged as a WRITE for the
    /// writes it holds for approval) and its approval mode, whether someone can be asked, and the conf's time to live.
    fn approvals(&self, offered: &Offered, can_ask: bool) -> Approvals<'static> {
        let spec = offered.tool.spec();
        let write_rule = if spec.classification.is_read() {
            self.policy.rule_as(&spec.name, Classification::Write)
        } else {
            offered.rule
        };
        Approvals::new(write_rule.mode, can_ask)
            .with_write_rule(write_rule)
            .with_ttl_minutes(crate::llm::facade(&self.factory).conf().limits.approval_ttl_minutes)
    }

    /// Call (or resume) one tool on this thread, a READ under the read fence.
    fn invoke(&mut self, call: &StoredCall, offered: &Offered, args: Json, input: Option<&ToolInput>) -> ToolOutcome {
        let reference = call.reference();
        let can_ask = self.run.caps.has(caps::APPROVAL_INLINE) || self.run.caps.has(caps::APPROVAL_LINK);
        let approvals = self.approvals(offered, can_ask);
        let ctx = TurnContext {
            run: Some(&self.run_ref),
            user: &self.user,
            agent: &self.agent,
            surface: self.run.surface,
            caps: &self.run.caps,
            budget: &mut self.budget,
            cancel: &self.local.cancel,
            events: &mut self.sink,
            screen: self.screen.as_mut(),
            approvals,
            spill: Spill {
                result_cap: self.result_cap,
                list_limit: self.list_limit,
                run_id: Some(&self.run_ref.run_id),
                tool_call_id: &call.tool_call_id,
            },
            call: &reference,
        };
        invoke(offered.tool.as_ref(), ctx, args, input)
    }

    /// Independent reads, each on a thread of its own in a context logged in as the user; their events and spending
    /// join this step's in model order.
    fn invoke_parallel(&mut self, prepared: &[(StoredCall, Offered, Json)]) -> Result<Vec<ToolOutcome>, Error> {
        for (call, _, _) in prepared {
            self.start_call(call)?;
        }
        let this = &*self;
        let can_ask = this.run.caps.has(caps::APPROVAL_INLINE) || this.run.caps.has(caps::APPROVAL_LINK);
        let results: Vec<(ToolOutcome, EventSink, Budget)> = std::thread::scope(|scope| {
            let handles: Vec<_> = prepared
                .iter()
                .map(|(call, offered, args)| {
                    let mut sink = this.sink.detached();
                    let mut budget = this.budget.clone();
                    let args = args.clone();
                    scope.spawn(move || {
                        let context = this.factory.new_context();
                        let _entered = context.enter();
                        let outcome = match login(&this.user) {
                            Ok(()) => {
                                context.set_agent_origin(Some(AgentOrigin {
                                    agent_id: this.agent.agent_id.clone(),
                                    run_id: Some(this.run_ref.run_id.clone()),
                                }));
                                let reference = call.reference();
                                let ctx = TurnContext {
                                    run: Some(&this.run_ref),
                                    user: &this.user,
                                    agent: &this.agent,
                                    surface: this.run.surface,
                                    caps: &this.run.caps,
                                    budget: &mut budget,
                                    cancel: &this.local.cancel,
                                    events: &mut sink,
                                    screen: None,
                                    approvals: this.approvals(offered, can_ask),
                                    spill: Spill {
                                        result_cap: this.result_cap,
                                        list_limit: this.list_limit,
                                        run_id: Some(&this.run_ref.run_id),
                                        tool_call_id: &call.tool_call_id,
                                    },
                                    call: &reference,
                                };
                                invoke(offered.tool.as_ref(), ctx, args, None)
                            }
                            Err(error) => ToolOutcome::Failed(format!("the tool could not run as the user: {error}")),
                        };
                        crate::entity::fence::clear();
                        (outcome, sink, budget)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| {
                        (
                            ToolOutcome::Failed("the tool failed unexpectedly".to_string()),
                            this.sink.detached(),
                            this.budget.clone(),
                        )
                    })
                })
                .collect()
        });
        let base_tokens = self.budget.tool_tokens;
        let base_cost = self.budget.tool_cost;
        let mut outcomes = Vec::with_capacity(results.len());
        for (outcome, sink, budget) in results {
            self.sink.absorb(sink);
            self.budget.tool_tokens += budget.tool_tokens.saturating_sub(base_tokens);
            self.budget.tool_cost += budget.tool_cost - base_cost;
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }

    /// Store what a call came to; a pause is given back for the caller to make.
    fn finish_call(&mut self, call: &StoredCall, outcome: ToolOutcome) -> Result<Option<tools::Pause>, Error> {
        let facade = self.facade.clone();
        let run = self.run.clone();
        let spill = Spill {
            result_cap: self.result_cap,
            list_limit: self.list_limit,
            run_id: Some(&run.run_id),
            tool_call_id: &call.tool_call_id,
        };
        // the arguments the repair left out, said after the result
        let ignored = self
            .offered
            .iter()
            .find(|offered| offered.tool.spec().name == call.name)
            .map(|offered| tools::repair::repair(&call.arguments, &offered.tool.spec().parameters).ignored)
            .unwrap_or_default();
        let note = if ignored.is_empty() {
            String::new()
        } else {
            format!("\nignored: {} (not arguments of {})", ignored.join(", "), call.name)
        };
        match outcome {
            ToolOutcome::Done(result) => {
                // outside the read fence: an over-cap result is stored as the call's artifact, with the result
                let (text, artifact) = spill.finish(&result.text);
                let text = text + &note;
                quietly(|| {
                    store_result_with(
                        &facade,
                        &run,
                        call,
                        &text,
                        false,
                        "AgtcDone",
                        &mut self.sink,
                        Hold::Lease,
                        artifact.as_ref(),
                    )
                })?;
                Ok(None)
            }
            ToolOutcome::Failed(text) => {
                // an error that teaches is JSON, stored as it is
                let text = if tools::format::is_error_json(&text) {
                    spill.fit(&text)
                } else {
                    spill.fit(&format!("Error: {text}"))
                } + &note;
                let text = self.repeated(call, &text).unwrap_or(text);
                quietly(|| {
                    store_result(
                        &facade,
                        &run,
                        call,
                        &text,
                        true,
                        "AgtcFailed",
                        &mut self.sink,
                        Hold::Lease,
                    )
                })?;
                Ok(None)
            }
            ToolOutcome::Pause(pause) => {
                quietly(|| {
                    set_call_status(
                        &facade,
                        &self.run.run_id,
                        self.sink.node(),
                        &call.tool_call_id,
                        "AgtcAwaiting",
                    )
                })?;
                Ok(Some(pause))
            }
        }
    }

    /// Pause the run for a tool: it gives up its lease and its worker; the calls after the paused one wait.
    fn pause(&mut self, pause: tools::Pause) -> Result<ToolsEnd, Error> {
        let facade = self.facade.clone();
        let run_id = self.run.run_id.clone();
        if let Some((event_type, payload)) = &pause.event {
            self.sink.emit(event_type, payload.clone());
        }
        let to = crate::llm::approval::pause_state(pause.kind, self.run.surface);
        quietly(|| {
            transition(
                &facade,
                &mut self.sink,
                &run_id,
                to,
                &Change {
                    pause: Some(pause),
                    ..Change::default()
                },
            )
        })?;
        super::engine(&self.factory).forget_if(&run_id, &self.local);
        Ok(ToolsEnd::Paused)
    }
}

/// The request of a call from the conversation's frozen prefix and its history as compaction left it.
fn request_from(
    frozen: &prompt::FrozenPrefix,
    history: &window::History,
    target: &Target,
    conversation_id: &str,
) -> ModelRequest {
    let (messages, tail) = history.messages();
    let mut request = frozen.request(messages, tail, Some(conversation_id));
    request.reasoning = Some(target.reasoning);
    request
}

/// The tokens of `request`: the provider's count of the newest reply's request plus the characters added since, by the
/// model's calibration (plan 7.8).
fn estimate(request: &ModelRequest, history: &window::History, target: &Target) -> u64 {
    let chars = crate::llm::tokens::RequestChars::sent(request, target).total();
    crate::llm::tokens::Estimator::for_model(&target.model).since(history.anchor, chars)
}

/// The share of `input` read from the cache, when there was input.
fn cache_ratio(cached: u64, input: u64) -> Option<f64> {
    (input > 0).then(|| ((cached.min(input) as f64 / input as f64) * 1000.0).round() / 1000.0)
}

/// The `compaction` event of a compaction.
fn compaction_event(step_seq: i64, outcome: &window::Outcome) -> Json {
    json!({"stepSeq": step_seq, "stages": outcome.stages, "beforeTokens": outcome.before, "afterTokens": outcome.after,
        "digested": outcome.digested, "summarized": outcome.summarized, "summaryFallback": outcome.summary_fallback,
        "full": outcome.full})
}

/// What a compaction needs of the step, borrowed apart from its event sink (which streams the call meanwhile).
struct Compactor<'a> {
    factory: &'a Arc<ExecutionContextFactory>,
    facade: &'a EntityFacade,
    run_id: &'a str,
    conversation_id: &'a str,
    user_id: &'a str,
    step_seq: i64,
    node: &'a str,
    cancel: &'a provider::CancelToken,
    side: &'a std::sync::Mutex<SideSpend>,
}

impl Compactor<'_> {
    /// Compact `history` for `request` within `window` ([`window::compact`]), the summary on the agent's model, and
    /// store what changed while this node holds the run's lease.
    fn run(
        &self,
        target: &Target,
        request: &ModelRequest,
        history: &mut window::History,
        window: &Window,
        force: bool,
    ) -> Result<window::Outcome, Error> {
        let chars = crate::llm::tokens::RequestChars::sent(request, target);
        // the conversation's own ratio when the provider counted a request of it, else the model's calibration
        let chars_per_token = match history.anchor {
            Some((tokens, at)) if tokens > 0 && at > 0 => at as f64 / tokens as f64,
            _ => crate::llm::tokens::Estimator::for_model(&target.model).k,
        };
        let budget = window::Budget {
            fixed_chars: chars.system + chars.tools,
            chars_per_token,
            window: *window,
        };
        let prompt = PromptTexts::of(self.factory).summary;
        let mut summarize = |input: &window::SummaryInput| self.summarize(target, &prompt, input);
        let outcome = window::compact(history, &budget, force, &mut summarize);
        if outcome.changed() {
            quietly(|| {
                in_tx(self.facade, || {
                    held(self.facade, self.run_id, self.node, Hold::Lease)?;
                    window::persist(self.facade, self.conversation_id, history)
                })
            })?;
            for entry in &mut history.entries {
                entry.dirty = false;
            }
            history.anchor = None;
            tracing::info!(
                "agent run {} compacted its conversation {} ({}): about {} to {} tokens",
                self.run_id,
                self.conversation_id,
                outcome.stages.join(", "),
                outcome.before,
                outcome.after
            );
        }
        Ok(outcome)
    }

    /// The summary of older turns on the agent's model (answered question 9); `None` when the call does not answer, so
    /// the deterministic one is used.
    fn summarize(&self, target: &Target, prompt: &str, input: &window::SummaryInput) -> Option<String> {
        if prompt.is_empty() || target.model.behavior.no_side_calls {
            return None;
        }
        let llm = crate::llm::facade(self.factory);
        let request = window::summary_request(prompt, input);
        let mut options = CallOptions::new(CallSource::Agent);
        options.user_id = Some(self.user_id.to_string());
        options.run_id = Some(self.run_id.to_string());
        options.step_seq = Some(self.step_seq);
        options.cancel = self.cancel.clone();
        options.deadline = Some(Instant::now() + Duration::from_secs(120));
        options.length_retry = false;
        options.recorder = Some(Recorder {
            llm: &llm,
            facade: self.facade,
        });
        let reply = provider::call(target, &request, &mut options);
        moqui_lib::sync::lock(self.side).add(&reply);
        match reply.finish {
            Finish::Stop | Finish::Length if !reply.text.trim().is_empty() => Some(reply.text),
            other => {
                tracing::warn!(
                    "the compaction summary of agent run {} did not come from the model ({}), so the deterministic one is used",
                    self.run_id,
                    other.as_str()
                );
                None
            }
        }
    }
}

/// Call (or resume) a tool: a READ under the read fence, so a write it tries fails and the model is told so; a panic
/// is a failure.
pub(crate) fn invoke(
    tool: &dyn AgentTool,
    mut ctx: TurnContext<'_>,
    args: Json,
    input: Option<&ToolInput>,
) -> ToolOutcome {
    let name = tool.spec().name.clone();
    let fenced = tool.spec().classification.is_read();
    let guard = fenced.then(|| fence::enter(format!("tool {name}")));
    let outcome = catch_unwind(AssertUnwindSafe(|| match input {
        Some(input) => tool.resume(&mut ctx, args, input),
        None => tool.call(&mut ctx, args),
    }))
    .unwrap_or_else(|_| {
        tracing::error!("agent tool {name} panicked");
        ToolOutcome::Failed("the tool failed unexpectedly".to_string())
    });
    if let Some(guard) = guard {
        let violations = guard.violations();
        if !violations.is_empty() {
            let tried: Vec<String> = violations.iter().map(fence::Violation::describe).collect();
            tracing::error!("agent tool {name} is a read tool but tried {}", tried.join(", "));
            return ToolOutcome::Failed(format!(
                "{name} is a read tool, but it tried {}, which a read tool may not do; nothing was changed.",
                tried.join(", ")
            ));
        }
    }
    outcome
}
