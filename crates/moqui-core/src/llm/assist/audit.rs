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

//! The admin services of System › LLM › Conversations, Runs and Approvals (plan 4.18, D12): the content of a
//! conversation, a run or an approval (messages, tool arguments and results, event payloads, approval arguments and
//! snapshots) is shown to its own user, or to a user with `AgentAudit`, whose every read is logged in
//! `AgentAuditLog`; `AgentAdmin` alone shows only what the lists show (who, when, which agent, the status and the
//! counts). An admin may cancel any run, which stops it and drives nothing; nobody acts in another user's
//! conversation.

use std::collections::BTreeMap;

use moqui_lib::error::Error;
use moqui_lib::object::{Value, is_empty, to_plain_string};
use serde_json::Value as Json;

use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::run::{self, quietly};
use crate::llm::text_of;
use crate::service::rust_services::{RustServiceFn, rust_service};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, get_audit_view)?;
    rust_service!(map, cancel_run_admin)?;
    Ok(())
}

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, Error>;

/// A transcript line or a payload is cut here.
const TEXT_CHARS: usize = 8000;
/// Rows of each list read at most.
const MAX_ROWS: i64 = 2000;

fn text(parameters: &Parameters, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn cut(text: &str) -> String {
    if text.chars().count() <= TEXT_CHARS {
        return text.to_string();
    }
    let mut out: String = text.chars().take(TEXT_CHARS).collect();
    out.push('…');
    out
}

/// A row as a map, its JSON text fields pretty-printed and its long texts cut.
fn row_map(row: &EntityValue) -> Value {
    let mut out = indexmap::IndexMap::new();
    for (name, value) in row.fields() {
        let value = match value {
            Value::String(text) if name.ends_with("Json") => Value::string(cut(&serde_json::from_str::<Json>(text)
                .ok()
                .and_then(|json| serde_json::to_string_pretty(&json).ok())
                .unwrap_or_else(|| text.clone()))),
            Value::String(text) => Value::string(cut(text)),
            other => other.clone(),
        };
        out.insert(name.to_string(), value);
    }
    Value::Map(out)
}

fn list(facade: &EntityFacade, entity: &str, field: &str, id: &str, order: &[&str]) -> Result<Vec<EntityValue>, Error> {
    let mut find = facade
        .find(entity)?
        .eq(field, Value::string(id))
        .limit(MAX_ROWS)
        .disable_authz();
    for name in order {
        find = find.order_by(name, false);
    }
    Ok(find.list()?.values().to_vec())
}

/// Whether the viewer may read the content of `target`'s rows: their own, or with `AgentAudit`, which is logged.
/// Out: (shown, audited).
fn may_read(
    facade: &EntityFacade,
    target: Option<&str>,
    conversation_id: Option<&str>,
    run_id: Option<&str>,
    reason: &str,
) -> Result<(bool, bool), Error> {
    let viewer = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty());
    let Some(viewer) = viewer else {
        return Ok((false, false));
    };
    if target == Some(viewer.as_str()) {
        return Ok((true, false));
    }
    if !crate::user::has_permission("AgentAudit").unwrap_or(false) {
        return Ok((false, false));
    }
    let mut log = facade.make_value("moqui.agent.AgentAuditLog")?;
    log.set_sequenced_id_primary(facade)?;
    log.set("auditorUserId", Value::string(&viewer));
    if let Some(target) = target {
        log.set("targetUserId", Value::string(target));
    }
    if let Some(conversation_id) = conversation_id {
        log.set("conversationId", Value::string(conversation_id));
    }
    if let Some(run_id) = run_id {
        log.set("runId", Value::string(run_id));
    }
    log.set("reason", Value::string(reason));
    log.set("auditDate", crate::user::now_value());
    log.create(facade)?;
    Ok((true, true))
}

/// One transcript line of a message: its text parts, its tool calls as `→ name(arguments)` and its tool results as
/// `← name: text`, redacted.
fn message_line(facade: &EntityFacade, message: &EntityValue, extra: &[String]) -> Result<Value, Error> {
    let message_id = text_of(message, "messageId").unwrap_or_default();
    let parts = list(facade, run::PART, "messageId", &message_id, &["partSeq"])?;
    let mut lines = Vec::new();
    for part in &parts {
        let data = text_of(part, "dataJson").and_then(|text| serde_json::from_str::<Json>(&text).ok());
        match text_of(part, "partKindEnumId").as_deref() {
            Some("AgpkToolCall") => {
                let data = data.unwrap_or(Json::Null);
                let arguments = data.get("arguments").map(Json::to_string).unwrap_or_default();
                lines.push(format!(
                    "→ {}({})",
                    data.get("name").and_then(Json::as_str).unwrap_or_default(),
                    crate::llm::usage::redact(&arguments, extra)
                ));
            }
            Some("AgpkToolResult") => {
                let meta = text_of(part, "metadataJson")
                    .and_then(|text| serde_json::from_str::<Json>(&text).ok())
                    .unwrap_or(Json::Null);
                lines.push(format!(
                    "← {}: {}",
                    meta.get("name").and_then(Json::as_str).unwrap_or_default(),
                    crate::llm::usage::redact(&text_of(part, "textValue").unwrap_or_default(), extra)
                ));
            }
            Some("AgpkReasoning") => {
                lines.push(format!("(thinking) {}", text_of(part, "textValue").unwrap_or_default()))
            }
            _ => {
                if let Some(text) = text_of(part, "textValue") {
                    lines.push(text);
                } else if let Some(data) = data {
                    lines.push(data.to_string());
                }
            }
        }
    }
    let mut out = indexmap::IndexMap::new();
    out.insert("messageId".to_string(), Value::string(&message_id));
    out.insert(
        "messageSeq".to_string(),
        message.get("messageSeq").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "role".to_string(),
        Value::string(text_of(message, "role").unwrap_or_default()),
    );
    out.insert(
        "runId".to_string(),
        text_of(message, "runId").map_or(Value::Null, Value::string),
    );
    out.insert(
        "createdDate".to_string(),
        message.get("createdDate").cloned().unwrap_or(Value::Null),
    );
    out.insert("text".to_string(), Value::string(cut(&lines.join("\n"))));
    Ok(Value::Map(out))
}

/// The fields of a run another user's admin without `AgentAudit` sees: no answer (`resultJson`), status reason or error
/// text, which carry what the run said.
const RUN_METADATA: &[&str] = &[
    "runId",
    "conversationId",
    "agentId",
    "userId",
    "surfaceEnumId",
    "statusId",
    "pauseKindEnumId",
    "stepCount",
    "toolCallCount",
    "inputTokens",
    "cachedInputTokens",
    "outputTokens",
    "cost",
    "tier",
    "activeSeconds",
    "submittedDate",
    "startedDate",
    "endedDate",
    "errorCode",
];

/// The fields of a tool call shown without `AgentAudit`: what was called and how it ended, no arguments or results.
const CALL_METADATA: &[&str] = &[
    "toolCallId",
    "stepSeq",
    "toolName",
    "classificationEnumId",
    "statusId",
    "startedDate",
    "endedDate",
    "approvalId",
];

/// A map of `fields` of `row`.
fn fields_of(row: &EntityValue, fields: &[&str]) -> Value {
    let mut map = indexmap::IndexMap::new();
    for field in fields {
        map.insert((*field).to_string(), row.get(field).cloned().unwrap_or(Value::Null));
    }
    Value::Map(map)
}

fn conversation_view(facade: &EntityFacade, conversation_id: &str, reason: &str, extra: &[String]) -> Out {
    let Some(row) = facade
        .find(run::CONVERSATION)?
        .eq("conversationId", Value::string(conversation_id))
        .disable_authz()
        .one()?
    else {
        add_error(format!("there is no conversation {conversation_id}"));
        return Ok(BTreeMap::new());
    };
    let target = text_of(&row, "userId");
    let (shown, audited) = may_read(facade, target.as_deref(), Some(conversation_id), None, reason)?;
    let mut out = BTreeMap::new();
    out.insert("shown".to_string(), Value::Bool(shown));
    out.insert("audited".to_string(), Value::Bool(audited));
    let runs = list(
        facade,
        run::RUN,
        "conversationId",
        conversation_id,
        &["submittedDate", "runId"],
    )?;
    if !shown {
        // who, which agent and how each run went, with no answer, reasons or error text
        out.insert(
            "runList".to_string(),
            Value::List(runs.iter().map(|run| fields_of(run, RUN_METADATA)).collect()),
        );
        out.insert(
            "reason".to_string(),
            Value::string("The content of another user's conversation needs the AgentAudit permission."),
        );
        return Ok(out);
    }
    out.insert("runList".to_string(), Value::List(runs.iter().map(row_map).collect()));
    let messages = list(facade, run::MESSAGE, "conversationId", conversation_id, &["messageSeq"])?;
    let lines = messages
        .iter()
        .map(|message| message_line(facade, message, extra))
        .collect::<Result<Vec<_>, _>>()?;
    out.insert("messageList".to_string(), Value::List(lines));
    out.insert("conversation".to_string(), row_map(&row));
    Ok(out)
}

fn run_view(facade: &EntityFacade, run_id: &str, reason: &str, extra: &[String]) -> Out {
    let Some(row) = facade
        .find(run::RUN)?
        .eq("runId", Value::string(run_id))
        .disable_authz()
        .one()?
    else {
        add_error(format!("there is no agent run {run_id}"));
        return Ok(BTreeMap::new());
    };
    let target = text_of(&row, "userId");
    let conversation_id = text_of(&row, "conversationId");
    let (shown, audited) = may_read(
        facade,
        target.as_deref(),
        conversation_id.as_deref(),
        Some(run_id),
        reason,
    )?;
    let mut out = BTreeMap::new();
    out.insert("shown".to_string(), Value::Bool(shown));
    out.insert("audited".to_string(), Value::Bool(audited));
    let steps = list(facade, "moqui.agent.AgentRunStep", "runId", run_id, &["stepSeq"])?;
    out.insert("stepList".to_string(), Value::List(steps.iter().map(row_map).collect()));
    let calls = list(facade, run::TOOL_CALL, "runId", run_id, &["stepSeq", "startedDate"])?;
    if !shown {
        // what was called and how it ended, with no arguments or results
        let bare = calls.iter().map(|call| fields_of(call, CALL_METADATA)).collect();
        out.insert("toolCallList".to_string(), Value::List(bare));
        out.insert(
            "reason".to_string(),
            Value::string("The content of another user's run needs the AgentAudit permission."),
        );
        return Ok(out);
    }
    let redacted = |row: &EntityValue| {
        let mut map = match row_map(row) {
            Value::Map(map) => map,
            _ => indexmap::IndexMap::new(),
        };
        for field in ["argumentsJson", "resultSummary"] {
            if let Some(Value::String(text)) = map.get(field) {
                let redacted = crate::llm::usage::redact(text, extra);
                map.insert(field.to_string(), Value::string(redacted));
            }
        }
        Value::Map(map)
    };
    out.insert(
        "toolCallList".to_string(),
        Value::List(calls.iter().map(redacted).collect()),
    );
    let events = list(facade, "moqui.agent.AgentRunEvent", "runId", run_id, &["eventSeq"])?;
    out.insert(
        "eventList".to_string(),
        Value::List(events.iter().map(row_map).collect()),
    );
    let approvals = list(facade, "moqui.agent.AgentApproval", "runId", run_id, &["createdDate"])?;
    out.insert(
        "approvalList".to_string(),
        Value::List(approvals.iter().map(row_map).collect()),
    );
    Ok(out)
}

fn approval_view(facade: &EntityFacade, approval_id: &str, reason: &str) -> Out {
    let Some(row) = facade
        .find("moqui.agent.AgentApproval")?
        .eq("approvalId", Value::string(approval_id))
        .disable_authz()
        .one()?
    else {
        add_error(format!("there is no approval {approval_id}"));
        return Ok(BTreeMap::new());
    };
    let target = text_of(&row, "userId");
    let run_id = text_of(&row, "runId");
    let conversation_id = match &run_id {
        Some(run_id) => run::load_run(facade, run_id)?.map(|run| run.conversation_id),
        None => None,
    };
    let (shown, audited) = may_read(
        facade,
        target.as_deref(),
        conversation_id.as_deref(),
        run_id.as_deref(),
        reason,
    )?;
    let mut out = BTreeMap::new();
    out.insert("shown".to_string(), Value::Bool(shown));
    out.insert("audited".to_string(), Value::Bool(audited));
    if !shown {
        out.insert(
            "reason".to_string(),
            Value::string("The arguments of another user's approval need the AgentAudit permission."),
        );
        return Ok(out);
    }
    let view = crate::llm::approval::ApprovalView::of(&row);
    let changes: Vec<String> = view
        .changes()
        .iter()
        .map(|change| format!("{}: {} → {}", change.label, change.current, change.proposed))
        .collect();
    let mut approval = match row_map(&row) {
        Value::Map(map) => map,
        _ => indexmap::IndexMap::new(),
    };
    approval.insert("summary".to_string(), Value::string(view.summary()));
    approval.insert("changesText".to_string(), Value::string(changes.join("\n")));
    let edited = view
        .snapshot
        .get("edited")
        .filter(|edited| !edited.is_null())
        .and_then(|edited| serde_json::to_string_pretty(edited).ok());
    approval.insert("editedText".to_string(), edited.map_or(Value::Null, Value::string));
    out.insert("approval".to_string(), Value::Map(approval));
    Ok(out)
}

/// `get#AgentAuditView`: the content of a conversation (its transcript and runs), a run (steps, tool calls with their
/// arguments and results, events, approvals) or an approval (arguments, snapshot, the edited arguments, preview and
/// result), for its own user or a user with `AgentAudit` (each read logged in `AgentAuditLog` with `reason`); for
/// anyone else `shown` is false and only the metadata lists come back.
fn get_audit_view(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let extra =
        ExecutionContext::with_current(|ec| crate::llm::facade(ec.factory()).conf().content_log.redact_keys.clone())
            .unwrap_or_default();
    let reason = text(parameters, "reason").unwrap_or_else(|| "System › LLM".to_string());
    quietly(|| {
        if let Some(conversation_id) = text(parameters, "conversationId") {
            conversation_view(facade, &conversation_id, &reason, &extra)
        } else if let Some(run_id) = text(parameters, "runId") {
            run_view(facade, &run_id, &reason, &extra)
        } else if let Some(approval_id) = text(parameters, "approvalId") {
            approval_view(facade, &approval_id, &reason)
        } else {
            add_error("conversationId, runId or approvalId is required");
            Ok(BTreeMap::new())
        }
    })
}

/// `cancel#AgentRun` (admin): stop any user's run, as its own Stop does; the partial answer is kept. Needs
/// `AgentAdmin`.
fn cancel_run_admin(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    if !crate::user::has_permission("AgentAdmin").unwrap_or(false) {
        add_error("this needs the AgentAdmin permission");
        return Ok(BTreeMap::new());
    }
    let Some(run_id) = text(parameters, "runId") else {
        add_error("runId is required");
        return Ok(BTreeMap::new());
    };
    let Some(factory) = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory())) else {
        return Ok(BTreeMap::new());
    };
    match run::cancel_run(&factory, &run_id, None) {
        Ok(state) => Ok(BTreeMap::from([(
            "statusId".to_string(),
            Value::string(state.status_id()),
        )])),
        Err(error) => {
            add_error(error.to_string());
            Ok(BTreeMap::new())
        }
    }
}
