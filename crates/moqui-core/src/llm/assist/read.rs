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

//! The drawer's reads: bootstrap, the conversation list and one conversation with its items, cards and runs, the
//! tasks pane, suggested prompts and the @ pickers. Each reads only the caller's own rows.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use moqui_db::sql::Condition;
use moqui_lib::object::Value;
use serde_json::{Value as Json, json};

use super::{Answer, Caller, DrawerError, Params};
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::approval::{self, ApprovalView};
use crate::llm::prompt::Surface;
use crate::llm::run::{self, AgentDef, RunRow, RunState, quietly};
use crate::llm::tools::PauseKind;
use crate::llm::{int_of, millis_of, text_of};

const AGENT: &str = "moqui.agent.AgentDefinition";
const APPROVAL: &str = "moqui.agent.AgentApproval";
const HANDOFF: &str = "moqui.agent.AgentHandoff";
const SUGGESTED: &str = "moqui.agent.AgentSuggestedPrompt";

/// Conversations per page by default, and at most.
const PAGE_SIZE: i64 = 25;
const MAX_PAGE_SIZE: i64 = 100;
/// Messages a conversation answers by default, and at most.
const MESSAGES: i64 = 200;
const MAX_MESSAGES: i64 = 500;
/// A tool call's arguments and a tool result's text are cut here for the step rows.
const STEP_TEXT_CHARS: usize = 2000;
/// Suggested prompts and mentions answered at most.
const MAX_PROMPTS: usize = 8;
const MAX_MENTIONS: usize = 10;

fn millis(row: &EntityValue, field: &str) -> Json {
    row.get(field).and_then(millis_of).map_or(Json::Null, Json::from)
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// The `enumCode` of a pause kind, as the client names it.
pub(super) fn pause_code(kind: PauseKind) -> &'static str {
    match kind {
        PauseKind::Approval => "approval",
        PauseKind::Question => "question",
        PauseKind::Handoff => "handoff",
        PauseKind::RemoteTask => "remote-task",
        PauseKind::Auth => "auth",
        PauseKind::Budget => "budget",
    }
}

fn surface_code(row: &EntityValue) -> Json {
    text_of(row, "surfaceEnumId")
        .and_then(|surface| Surface::parse(&surface))
        .map_or(Json::Null, |surface| json!(surface.as_str()))
}

/// The agents the user may use in the drawer: not disabled, serving the drawer surface, in id order.
pub(super) fn drawer_agents(facade: &EntityFacade) -> Result<Vec<(AgentDef, EntityValue)>, DrawerError> {
    let rows = quietly(|| {
        facade
            .find(AGENT)?
            .order_by("agentId", false)
            .use_cache(true)
            .disable_authz()
            .list()
    })?;
    Ok(rows
        .values()
        .iter()
        .map(|row| (AgentDef::of(row), row.clone()))
        .filter(|(agent, _)| !agent.disabled && agent.serves(Surface::Drawer))
        .collect())
}

/// `GET bootstrap`.
pub(super) fn bootstrap(caller: &Caller) -> Answer {
    let llm = crate::llm::facade(&caller.factory);
    let registry = quietly(|| llm.registry(&caller.facade));
    let agents = drawer_agents(&caller.facade)?;
    // no development-only dialect is built yet (screen-xml is planned separately)
    let drawer_dialects = crate::llm::ui::offered(&crate::llm::tools::ClientCaps::for_surface(crate::llm::prompt::Surface::Drawer), false);
    let dialects_of = |agent: &crate::llm::run::AgentDef| -> Json {
        drawer_dialects
            .iter()
            .filter(|dialect| agent.allows_dialect(dialect.name()))
            .map(|dialect| json!({"name": dialect.name(), "label": dialect.label(), "skill": dialect.skill()}))
            .collect()
    };
    let any_canvas = agents.iter().any(|(agent, _)| !dialects_of(agent).as_array().is_none_or(Vec::is_empty));
    let agent_list: Vec<Json> = agents
        .iter()
        .map(|(agent, row)| {
            let tiers: Vec<&str> = registry
                .usable_tiers(agent.local_only)
                .into_iter()
                .map(|tier| tier.as_str())
                .collect();
            json!({
                "agentId": agent.agent_id,
                "name": if agent.name.is_empty() { agent.agent_id.clone() } else { agent.name.clone() },
                "description": text_of(row, "description"),
                "tier": agent.tier.map(|tier| tier.as_str()),
                "tierFixed": agent.tier.is_some(),
                "tiers": tiers,
                // the canvas dialects the drawer offers this agent (built here, with the drawer's capability, and the agent's)
                "dialects": dialects_of(agent),
                "startPath": agent.start_path,
            })
        })
        .collect();
    let default_agent = agents
        .iter()
        .find(|(agent, _)| agent.agent_id == "assist")
        .or_else(|| agents.first())
        .map(|(agent, _)| agent.agent_id.clone());
    let conf = llm.conf();
    let transcribe = registry.bindings.contains_key("transcribe") && registry.resolve_feature("transcribe").is_ok();
    let username = crate::user::current_username().ok().flatten();
    let locale = crate::user::locale().ok().flatten();
    Ok(json!({
        "user": {
            "userId": caller.user_id,
            "username": username,
            "locale": locale,
            "timeZone": crate::user::time_zone().ok().flatten(),
            "weekStart": super::state::week_start(locale.as_deref().unwrap_or_default()),
        },
        "agents": agent_list,
        "defaultAgentId": default_agent,
        "tiers": ["fast", "balanced", "thorough"],
        "features": {
            "enabled": llm.enabled(),
            "screenFollow": crate::llm::screen::installed(),
            "preview": quietly(|| approval::preview::available(&caller.facade)),
            "canvas": any_canvas,
            // the user lets a Vue SFC canvas open other sites and show their images (ASSIST_CANVAS_EXTERNAL)
            "canvasExternal": canvas_external(&caller.user_id),
            "attachments": true,
            // dictation through the model bound to transcribe; else the browser's speech API only when the admin allows it
            "transcribe": transcribe,
            "browserDictation": !transcribe && conf.assist.browser_dictation,
            "textActions": registry.resolve(&crate::llm::registry::Want {
                feature: Some("text-action"),
                tier: Some(crate::llm::conf::Tier::Fast),
                ..crate::llm::registry::Want::default()
            }).is_ok(),
            "saveSkill": caller.has_permission(crate::llm::skills::learn::AUTHOR_PERMISSION),
            "memory": true,
            "admin": caller.has_permission("AgentAdmin"),
            "audit": caller.has_permission("AgentAudit"),
        },
        "limits": {
            "maxMessageChars": super::MAX_MESSAGE_CHARS,
            "maxForms": super::state::MAX_FORMS,
            "maxFormValues": super::state::MAX_FORM_VALUES,
            "maxValueChars": super::state::MAX_VALUE_CHARS,
            "approvalTtlMinutes": conf.limits.approval_ttl_minutes,
            "maxSteps": conf.limits.max_steps,
            "maxRunSeconds": conf.limits.max_run_seconds,
            "maxAttachmentBytes": conf.assist.max_attachment_bytes,
            "maxAttachments": conf.assist.max_attachments,
            "attachmentTypes": conf.assist.attachment_types,
            "maxAudioBytes": conf.assist.max_audio_bytes,
            "maxTextActionChars": conf.assist.max_text_action_chars,
        },
        // the user's permanent memory gate (a conversation's own comes with it)
        "memory": {"preference": crate::llm::memory::preference(&caller.user_id, conf.assist.memory_default)},
        "secretFields": {
            "contains": ["password", "passwd", "token", "secret", "credential", "salt"],
            "endsWith": ["key"],
            "names": conf.content_log.redact_keys,
        },
        "paths": {
            "calls": "/assist",
            "events": "/assist/runs/{runId}/events",
            "catchUp": "/assist/runs/{runId}/events.json",
            "sandbox": "/assist/sandbox",
        },
    }))
}

/// The user preference that lets a Vue SFC canvas reach other sites (its links and images); off unless the user set it.
pub const CANVAS_EXTERNAL: &str = "ASSIST_CANVAS_EXTERNAL";

/// Whether the user lets a Vue SFC canvas reach other sites.
pub(crate) fn canvas_external(user_id: &str) -> bool {
    match crate::user::get_preference_for(CANVAS_EXTERNAL, Some(user_id)) {
        Ok(value) => value.as_deref().map(str::trim) == Some("true"),
        Err(error) => {
            tracing::error!("the preference {CANVAS_EXTERNAL} of {user_id} could not be read: {error}");
            false
        }
    }
}

/// The runs `ids` by id, read at once.
fn runs_by_id(facade: &EntityFacade, ids: &[String]) -> Result<HashMap<String, RunRow>, DrawerError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = quietly(|| {
        facade
            .find(run::RUN)?
            .condition(Condition::is_in("runId", ids.iter().map(Value::string).collect()))
            .disable_authz()
            .list()
    })?;
    Ok(rows
        .values()
        .iter()
        .map(RunRow::of)
        .map(|run| (run.run_id.clone(), run))
        .collect())
}

/// The conversations `ids` by id, read at once.
fn conversations_by_id(
    facade: &EntityFacade,
    ids: &BTreeSet<String>,
) -> Result<HashMap<String, EntityValue>, DrawerError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = quietly(|| {
        facade
            .find(run::CONVERSATION)?
            .condition(Condition::is_in(
                "conversationId",
                ids.iter().map(Value::string).collect(),
            ))
            .disable_authz()
            .list()
    })?;
    Ok(rows
        .values()
        .iter()
        .filter_map(|row| text_of(row, "conversationId").map(|id| (id, row.clone())))
        .collect())
}

/// `GET conversations`: the user's conversations, newest first, a page at a time.
pub(super) fn conversations(caller: &Caller, params: &Params) -> Answer {
    let facade = &caller.facade;
    let page_size = params
        .int("pageSize")
        .filter(|size| *size > 0)
        .map_or(PAGE_SIZE, |size| size.min(MAX_PAGE_SIZE));
    let page_index = params.int("pageIndex").filter(|index| *index >= 0).unwrap_or(0);
    let filter = params.text("filter").unwrap_or_else(|| "all".to_string());
    let mut find = quietly(|| facade.find(run::CONVERSATION))?
        .eq("userId", Value::string(&caller.user_id))
        .disable_authz();
    match filter.as_str() {
        "all" => {}
        "running" | "waiting" => {
            let states: Vec<RunState> = if filter == "running" {
                vec![RunState::Submitted, RunState::Working]
            } else {
                vec![RunState::InputRequired, RunState::AuthRequired]
            };
            let active = quietly(|| {
                facade
                    .find(run::RUN)?
                    .eq("userId", Value::string(&caller.user_id))
                    .condition(Condition::is_in(
                        "statusId",
                        states.iter().map(|state| Value::string(state.status_id())).collect(),
                    ))
                    .disable_authz()
                    .list()
            })?;
            let ids: Vec<Value> = active
                .values()
                .iter()
                .filter_map(|row| text_of(row, "runId"))
                .map(Value::string)
                .collect();
            if ids.is_empty() {
                return Ok(
                    json!({"conversations": [], "pageIndex": page_index, "pageSize": page_size, "hasMore": false}),
                );
            }
            find = find.condition(Condition::is_in("activeRunId", ids));
        }
        "record" => {
            let entity_name = params.required("recordEntityName")?;
            let record_id = params.required("recordId")?;
            find = find
                .eq("recordEntityName", Value::string(entity_name))
                .eq("recordId", Value::string(record_id));
        }
        other => {
            return Err(DrawerError::invalid(format!(
                "{other} is not a filter; it is all, running, waiting or record"
            )));
        }
    }
    if let Some(surface) = params.text("surface") {
        let surface =
            Surface::parse(&surface).ok_or_else(|| DrawerError::invalid(format!("{surface} is not a surface")))?;
        find = find.eq("surfaceEnumId", Value::string(surface.enum_id()));
    }
    if let Some(agent_id) = params.text("agentId") {
        find = find.eq("agentId", Value::string(agent_id));
    }
    if let Some(search) = params.text("search") {
        let pattern = format!("%{}%", search.replace(['%', '_'], " ").trim());
        find = find.condition(Condition::or(vec![
            Condition::ignore_case(Condition::like("title", Value::string(&pattern))),
            Condition::ignore_case(Condition::like("searchText", Value::string(&pattern))),
        ]));
    }
    let rows = quietly(|| {
        find.order_by("lastMessageDate", true)
            .order_by("createdDate", true)
            .offset(page_index * page_size)
            .limit(page_size + 1)
            .list()
    })?;
    let mut rows: Vec<EntityValue> = rows.values().to_vec();
    let has_more = i64::try_from(rows.len()).unwrap_or(0) > page_size;
    rows.truncate(usize::try_from(page_size).unwrap_or(0));
    let active: Vec<String> = rows.iter().filter_map(|row| text_of(row, "activeRunId")).collect();
    let runs = runs_by_id(facade, &active)?;
    let list: Vec<Json> = rows
        .iter()
        .map(|row| {
            let active = text_of(row, "activeRunId").and_then(|id| runs.get(&id));
            json!({
                "conversationId": text_of(row, "conversationId"),
                "title": text_of(row, "title"),
                "agentId": text_of(row, "agentId"),
                "surface": surface_code(row),
                "statusId": text_of(row, "statusId"),
                "activeRunId": active.map(|run| run.run_id.clone()),
                "activeRunState": active.map(|run| run.state.wire()),
                "waiting": active.is_some_and(|run| run.state.is_paused()),
                "lastMessageDate": millis(row, "lastMessageDate"),
                "createdDate": millis(row, "createdDate"),
                "recordEntityName": text_of(row, "recordEntityName"),
                "recordId": text_of(row, "recordId"),
            })
        })
        .collect();
    Ok(json!({"conversations": list, "pageIndex": page_index, "pageSize": page_size, "hasMore": has_more}))
}

/// A run as the drawer shows it; the cost only for an admin.
fn run_json(run: &RunRow, row: Option<&EntityValue>, admin: bool) -> Json {
    let field = |name: &str| row.and_then(|row| text_of(row, name));
    let date = |name: &str| row.map_or(Json::Null, |row| millis(row, name));
    json!({
        "runId": run.run_id,
        "agentId": run.agent_id,
        "state": run.state.wire(),
        "statusId": run.state.status_id(),
        "pauseKind": run.pause_kind.map(pause_code),
        "pauseRef": run.pause_ref,
        "lastEventSeq": row.and_then(|row| int_of(row, "lastEventSeq")).unwrap_or(0),
        "submittedDate": date("submittedDate"),
        "endedDate": date("endedDate"),
        "stepCount": run.step_count,
        "toolCallCount": run.tool_call_count,
        "inputTokens": run.input_tokens,
        "outputTokens": run.output_tokens,
        "activeSeconds": run.active_seconds,
        "tier": run.tier.map(|tier| tier.as_str()),
        "errorCode": field("errorCode"),
        "errorText": field("errorText"),
        "cost": if admin { json!(run.cost.to_string()) } else { Json::Null },
    })
}

/// The approval card of a row: [`ApprovalView::to_json`], the tool's name and whether it can be previewed.
pub(super) fn approval_card(row: &EntityValue, tool: Option<String>, can_preview: bool) -> Json {
    let view = ApprovalView::of(row);
    let mut card = view.to_json();
    card["tool"] = json!(tool);
    // a tool call's write (another system's, a canvas Mutation) has no sim preview
    card["canPreview"] = json!(can_preview && view.target_kind != approval::TargetKind::Tool);
    card
}

/// The question card of a row: its text, options (the first recommended), fields, and `kind` budget for a run's
/// "continue?".
pub(super) fn question_card(row: &EntityValue, budget: bool) -> Json {
    let json_of = |field: &str| {
        text_of(row, field)
            .and_then(|text| serde_json::from_str::<Json>(&text).ok())
            .unwrap_or(Json::Null)
    };
    let options: Vec<Json> = json_of("choicesJson")
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(at, option)| {
            let label = option
                .as_str()
                .map(str::to_string)
                .or_else(|| option.get("label").and_then(Json::as_str).map(str::to_string))?;
            Some(json!({"label": label, "recommended": at == 0}))
        })
        .collect();
    json!({
        "questionId": text_of(row, "questionId"),
        "runId": text_of(row, "runId"),
        "toolCallId": text_of(row, "toolCallId"),
        "text": text_of(row, "questionText"),
        "options": options,
        "multi": false,
        "fields": json_of("schemaJson").as_array().cloned().unwrap_or_default(),
        "freeText": !budget,
        "kind": if budget { "budget" } else { "ask" },
        "status": text_of(row, "statusId"),
        "createdDate": millis(row, "createdDate"),
        "expiresAt": millis(row, "expireDate"),
    })
}

/// A hand-off watch as the drawer's card has it: with the page, the form and the values of its approval, as
/// `handoff.started` gave them, so a tab that reads the conversation again marks the same form.
fn handoff_json(row: &EntityValue, approval: Option<&EntityValue>) -> Json {
    let view = approval.map(ApprovalView::of);
    let target = view.as_ref().and_then(ApprovalView::handoff_target);
    let values: Vec<Json> = view
        .as_ref()
        .map(|view| {
            view.changes()
                .iter()
                .map(|change| json!({"field": change.field, "label": change.label, "value": change.proposed}))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "path": target.as_ref().map(|target| target.path.clone()),
        "parameters": target.as_ref().map(|target| json!(target.parameters)),
        "form": target.as_ref().map(|target| target.form.clone()),
        "values": values,
        "handoffId": text_of(row, "handoffId"),
        "approvalId": text_of(row, "approvalId"),
        "runId": text_of(row, "runId"),
        "screenPath": text_of(row, "screenPath"),
        "transition": text_of(row, "transitionName"),
        "status": text_of(row, "statusId").map(|status| match status.as_str() {
            "AghWatching" => "watching",
            "AghSaved" => "saved",
            "AghFailed" => "failed",
            "AghAbandoned" => "abandoned",
            _ => "unknown",
        }),
        "createdDate": millis(row, "createdDate"),
        "expiresAt": millis(row, "expireDate"),
    })
}

/// A JSON text field of a row, or null.
fn json_field(row: &EntityValue, field: &str) -> Json {
    text_of(row, field)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Json::Null)
}

/// The part of a message as the thread shows it. A tool call with an approval carries the approval as it stands
/// (`decided`: id, status, summary), so a decided card is drawn collapsed after a reload or in another tab.
fn part_json(
    part: &EntityValue,
    calls: &HashMap<String, EntityValue>,
    decided: &HashMap<String, Json>,
    extra_keys: &[String],
) -> Json {
    let kind = text_of(part, "partKindEnumId").unwrap_or_default();
    let seq = int_of(part, "partSeq").unwrap_or(0);
    match kind.as_str() {
        "AgpkText" => json!({"part": seq, "kind": "text", "text": text_of(part, "textValue").unwrap_or_default()}),
        "AgpkReasoning" => {
            json!({"part": seq, "kind": "reasoning", "text": text_of(part, "textValue").unwrap_or_default()})
        }
        "AgpkToolCall" => {
            let data = json_field(part, "dataJson");
            let tool_call_id = text_of(part, "toolCallId").unwrap_or_default();
            let call = calls.get(&tool_call_id);
            let arguments = data.get("arguments").map(|arguments| {
                let text = crate::llm::usage::redact(&arguments.to_string(), extra_keys);
                cut(&text, STEP_TEXT_CHARS)
            });
            let classification = call
                .and_then(|call| text_of(call, "classificationEnumId"))
                .and_then(|class| crate::llm::tools::Classification::parse(&class))
                .map(|class| class.as_str());
            let approval_id = call.and_then(|call| text_of(call, "approvalId"));
            json!({
                "part": seq,
                "kind": "tool-call",
                "toolCallId": tool_call_id,
                "name": data.get("name").and_then(Json::as_str),
                "arguments": arguments,
                "status": call.and_then(|call| text_of(call, "statusId")),
                "classification": classification,
                "approvalId": approval_id,
                "approval": approval_id.as_ref().and_then(|id| decided.get(id)).cloned().unwrap_or(Json::Null),
                "startedDate": call.map_or(Json::Null, |call| millis(call, "startedDate")),
                "endedDate": call.map_or(Json::Null, |call| millis(call, "endedDate")),
            })
        }
        "AgpkToolResult" => {
            let meta = json_field(part, "metadataJson");
            let text = text_of(part, "textValue").unwrap_or_default();
            json!({
                "part": seq,
                "kind": "tool-result",
                "toolCallId": text_of(part, "toolCallId"),
                "name": meta.get("name"),
                "isError": meta.get("isError").and_then(Json::as_bool).unwrap_or(false),
                "text": cut(&crate::llm::usage::redact(&text, extra_keys), STEP_TEXT_CHARS),
            })
        }
        "AgpkFile" => {
            let data = json_field(part, "dataJson");
            json!({
                "part": seq,
                "kind": "file",
                "attachmentId": data.get("attachmentId"),
                "ref": data.get("ref"),
                "fileName": text_of(part, "fileName"),
                "mediaType": text_of(part, "mediaType"),
                "size": data.get("size"),
                "fileUrl": text_of(part, "fileUrl"),
            })
        }
        _ => json!({"part": seq, "kind": "data", "data": json_field(part, "dataJson")}),
    }
}

/// `GET conversation`: one conversation of the user's with its messages (the last `limit`, or those before
/// `beforeSeq`), its runs, the active run and its last event, the cards waiting for the user, the messages queued for
/// the active run, and who follows it.
pub(super) fn conversation(caller: &Caller, params: &Params) -> Answer {
    let facade = &caller.facade;
    let conversation_id = params.required("conversationId")?;
    let row = caller.conversation(&conversation_id)?;
    let admin = caller.has_permission("AgentAdmin");
    let extra_keys = crate::llm::facade(&caller.factory)
        .conf()
        .content_log
        .redact_keys
        .clone();
    let limit = params
        .int("limit")
        .filter(|limit| *limit > 0)
        .map_or(MESSAGES, |limit| limit.min(MAX_MESSAGES));
    let before = params.int("beforeSeq");

    // the runs, oldest first, with their rows for the dates and sequences
    let run_rows = quietly(|| {
        facade
            .find(run::RUN)?
            .eq("conversationId", Value::string(&conversation_id))
            .order_by("submittedDate", false)
            .order_by("runId", false)
            .disable_authz()
            .list()
    })?;
    let run_rows: Vec<EntityValue> = run_rows
        .values()
        .iter()
        .filter(|run| text_of(run, "userId").as_deref() == Some(caller.user_id.as_str()))
        .cloned()
        .collect();
    let run_ids: Vec<Value> = run_rows
        .iter()
        .filter_map(|run| text_of(run, "runId"))
        .map(Value::string)
        .collect();
    let runs: Vec<Json> = run_rows
        .iter()
        .map(|row| run_json(&RunRow::of(row), Some(row), admin))
        .collect();
    let active_id = text_of(&row, "activeRunId");
    let active_row = active_id
        .as_ref()
        .and_then(|id| run_rows.iter().find(|run| text_of(run, "runId").as_deref() == Some(id)));
    let active = active_row
        .map(RunRow::of)
        .filter(|run| !run.state.is_terminal())
        .map(|run| run_json(&run, active_row, admin));

    // the messages: the newest `limit` (before `beforeSeq`), in order
    let mut find = quietly(|| facade.find(run::MESSAGE))?
        .eq("conversationId", Value::string(&conversation_id))
        .disable_authz();
    if let Some(before) = before {
        find = find.condition(Condition::less("messageSeq", Value::I64(before)));
    }
    let messages = quietly(|| find.order_by("messageSeq", true).limit(limit + 1).list())?;
    let mut messages: Vec<EntityValue> = messages.values().to_vec();
    let has_earlier = i64::try_from(messages.len()).unwrap_or(0) > limit;
    messages.truncate(usize::try_from(limit).unwrap_or(0));
    messages.reverse();
    let message_ids: Vec<Value> = messages
        .iter()
        .filter_map(|message| text_of(message, "messageId"))
        .map(Value::string)
        .collect();
    let mut parts_of: BTreeMap<String, Vec<EntityValue>> = BTreeMap::new();
    if !message_ids.is_empty() {
        let parts = quietly(|| {
            facade
                .find(run::PART)?
                .condition(Condition::is_in("messageId", message_ids))
                .order_by("partSeq", false)
                .disable_authz()
                .list()
        })?;
        for part in parts.values() {
            if let Some(message_id) = text_of(part, "messageId") {
                parts_of.entry(message_id).or_default().push(part.clone());
            }
        }
    }
    let calls: HashMap<String, EntityValue> = if run_ids.is_empty() {
        HashMap::new()
    } else {
        let rows = quietly(|| {
            facade
                .find(run::TOOL_CALL)?
                .condition(Condition::is_in("runId", run_ids.clone()))
                .disable_authz()
                .list()
        })?;
        rows.values()
            .iter()
            .filter_map(|call| text_of(call, "toolCallId").map(|id| (id, call.clone())))
            .collect()
    };
    // the approvals of the runs: the pending ones are cards that wait for the user, a decided one goes on its tool call
    let can_preview = quietly(|| approval::preview::available(facade));
    let mut approvals = Vec::new();
    let mut decided: HashMap<String, Json> = HashMap::new();
    let mut approval_rows: HashMap<String, EntityValue> = HashMap::new();
    if !run_ids.is_empty() {
        let rows = quietly(|| {
            facade
                .find(APPROVAL)?
                .condition(Condition::is_in("runId", run_ids.clone()))
                .eq("userId", Value::string(&caller.user_id))
                .order_by("createdDate", false)
                .disable_authz()
                .list()
        })?;
        for approval_row in rows.values() {
            if let Some(id) = text_of(approval_row, "approvalId") {
                approval_rows.insert(id, approval_row.clone());
            }
            let tool = text_of(approval_row, "toolCallId")
                .and_then(|id| calls.get(&id))
                .and_then(|call| text_of(call, "toolName"));
            let card = approval_card(approval_row, tool, can_preview);
            if text_of(approval_row, "statusId").as_deref() == Some(approval::ApprovalStatus::Pending.status_id()) {
                approvals.push(card);
            } else if let Some(id) = text_of(approval_row, "approvalId") {
                let compact = json!({"approvalId": card["approvalId"], "runId": card["runId"], "toolCallId": card["toolCallId"],
                    "classification": card["classification"], "targetName": card["targetName"], "summary": card["summary"],
                    "status": card["status"], "decisionChannel": card["decisionChannel"]});
                decided.insert(id, compact);
            }
        }
    }
    let items: Vec<Json> = messages
        .iter()
        .map(|message| {
            let message_id = text_of(message, "messageId").unwrap_or_default();
            let metadata = json_field(message, "metadataJson");
            let parts: Vec<Json> = parts_of
                .get(&message_id)
                .map(|parts| {
                    parts
                        .iter()
                        .map(|part| part_json(part, &calls, &decided, &extra_keys))
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "messageId": message_id,
                "seq": int_of(message, "messageSeq"),
                "role": text_of(message, "role"),
                "runId": text_of(message, "runId"),
                "clientMessageId": text_of(message, "clientMessageId"),
                "createdDate": millis(message, "createdDate"),
                "compacted": text_of(message, "compacted").as_deref() == Some("Y"),
                "feedback": metadata.get("feedback").cloned().unwrap_or(Json::Null),
                "parts": parts,
            })
        })
        .collect();

    // the cards that wait for the user
    let (mut questions, mut handoffs) = (Vec::new(), Vec::new());
    if !run_ids.is_empty() {
        let budget_refs: BTreeSet<String> = run_rows
            .iter()
            .map(RunRow::of)
            .filter(|run| run.pause_kind == Some(PauseKind::Budget))
            .filter_map(|run| run.pause_ref)
            .collect();
        let rows = quietly(|| {
            facade
                .find(run::QUESTION)?
                .condition(Condition::is_in("runId", run_ids.clone()))
                .eq("userId", Value::string(&caller.user_id))
                .eq("statusId", Value::string("AgqOpen"))
                .order_by("createdDate", false)
                .disable_authz()
                .list()
        })?;
        for question in rows.values() {
            let budget = text_of(question, "questionId").is_some_and(|id| budget_refs.contains(&id));
            questions.push(question_card(question, budget));
        }
        let rows = quietly(|| {
            facade
                .find(HANDOFF)?
                .condition(Condition::is_in("runId", run_ids.clone()))
                .eq("userId", Value::string(&caller.user_id))
                .condition(Condition::is_in(
                    "statusId",
                    vec![Value::string("AghWatching"), Value::string("AghFailed")],
                ))
                .disable_authz()
                .list()
        })?;
        handoffs = rows
            .values()
            .iter()
            .map(|row| handoff_json(row, text_of(row, "approvalId").and_then(|id| approval_rows.get(&id))))
            .collect();
    }
    // the messages sent while the active run works, not folded in yet
    let queued: Vec<Json> = match &active_id {
        Some(run_id) if active.is_some() => {
            let rows = quietly(|| {
                facade
                    .find(run::INPUT)?
                    .eq("runId", Value::string(run_id))
                    .eq("inputKindEnumId", Value::string("AgikMessage"))
                    .condition(Condition::is_null("consumedDate"))
                    .order_by("receivedDate", false)
                    .disable_authz()
                    .list()
            })?;
            rows.values()
                .iter()
                .map(|input| {
                    json!({
                        "inputId": text_of(input, "inputId"),
                        "text": text_of(input, "messageText"),
                        "clientMessageId": text_of(input, "clientMessageId"),
                        "receivedDate": millis(input, "receivedDate"),
                    })
                })
                .collect()
        }
        _ => Vec::new(),
    };
    let follow_client = text_of(&row, "clientId");
    let client_id = params.text("clientId");
    Ok(json!({
        "conversation": {
            "conversationId": conversation_id,
            "title": text_of(&row, "title"),
            "agentId": text_of(&row, "agentId"),
            "surface": surface_code(&row),
            "statusId": text_of(&row, "statusId"),
            "recordEntityName": text_of(&row, "recordEntityName"),
            "recordId": text_of(&row, "recordId"),
            "createdDate": millis(&row, "createdDate"),
            "lastMessageDate": millis(&row, "lastMessageDate"),
            "lastMessageSeq": int_of(&row, "lastMessageSeq").unwrap_or(0),
            "memoryCreate": text_of(&row, "memoryCreate").map(|flag| flag == "Y"),
        },
        "activeRun": active,
        "runs": runs,
        "items": items,
        "hasEarlier": has_earlier,
        "pending": {
            "approvals": approvals,
            "questions": questions,
            "handoffs": handoffs,
            "queued": queued,
        },
        "canvas": super::canvas::conversation_canvas(caller, &conversation_id),
        "context": context_meter(caller, &run_ids),
        "memory": memory_gate(caller, &conversation_id),
        "follow": {
            "clientId": follow_client,
            "contextVersion": int_of(&row, "contextVersion").unwrap_or(0),
            "following": client_id.is_some() && client_id == follow_client,
        },
    }))
}

/// The context meter (plan 4.11): the tokens of the conversation's last model call (its prompt and its reply) against
/// the window of its model: `{usedTokens, windowTokens, budgetTokens, percent, compactAt, modelId, estimated}`, or null
/// before any call. A read that fails is logged and gives null.
fn context_meter(caller: &Caller, run_ids: &[Value]) -> Json {
    if run_ids.is_empty() {
        return Json::Null;
    }
    let last = quietly(|| {
        caller
            .facade
            .find("moqui.llm.LlmCall")?
            .condition(Condition::is_in("runId", run_ids.to_vec()))
            .order_by("startDate", true)
            .order_by("callId", true)
            .limit(10)
            .disable_authz()
            .list()
    });
    let rows = match last {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("the context meter could not read the model calls: {error}");
            return Json::Null;
        }
    };
    // the last call of the agent's own steps that reported its prompt (a summary or side call has no stepSeq)
    let Some(call) = rows
        .values()
        .iter()
        .find(|call| int_of(call, "inputTokens").is_some_and(|tokens| tokens > 0) && int_of(call, "stepSeq").is_some())
    else {
        return Json::Null;
    };
    let used = u64::try_from(int_of(call, "inputTokens").unwrap_or(0) + int_of(call, "outputTokens").unwrap_or(0)).unwrap_or(0);
    let model_id = text_of(call, "modelId");
    let llm = crate::llm::facade(&caller.factory);
    let registry = quietly(|| llm.registry(&caller.facade));
    let model = model_id.as_deref().and_then(|id| registry.models.get(id)).map(|model| model.conf.clone());
    let model = model.unwrap_or_else(|| crate::llm::conf::ModelConf::new("", "", ""));
    let window = crate::llm::tokens::Window::for_model(&model, None, false);
    let percent = (used * 100).checked_div(window.budget).unwrap_or(0).min(999);
    json!({
        "usedTokens": used,
        "windowTokens": window.context,
        "budgetTokens": window.budget,
        "percent": percent,
        "compactAt": window.high,
        "modelId": model_id,
        "estimated": text_of(call, "usageEstimated").as_deref() == Some("Y"),
    })
}

/// The memory gates of the user for a conversation (`{preference, conversation, effective}`); null when they cannot be
/// read (logged).
fn memory_gate(caller: &Caller, conversation_id: &str) -> Json {
    let default = crate::llm::facade(&caller.factory).conf().assist.memory_default;
    match crate::llm::memory::gate(&caller.facade, &caller.user_id, Some(conversation_id), default) {
        Ok(gate) => gate.to_json(),
        Err(error) => {
            tracing::error!("the memory gate of agent conversation {conversation_id} could not be read: {error}");
            Json::Null
        }
    }
}

/// `GET tasks`: what waits for the user across every surface (approvals, questions, hand-offs), what runs, what
/// failed in the last week and what finished in the last day.
pub(super) fn tasks(caller: &Caller) -> Answer {
    let facade = &caller.facade;
    let user = Value::string(&caller.user_id);
    let approvals = quietly(|| {
        facade
            .find(APPROVAL)?
            .eq("userId", user.clone())
            .eq("statusId", Value::string(approval::ApprovalStatus::Pending.status_id()))
            .order_by("createdDate", true)
            .limit(50)
            .disable_authz()
            .list()
    })?;
    let questions = quietly(|| {
        facade
            .find(run::QUESTION)?
            .eq("userId", user.clone())
            .eq("statusId", Value::string("AgqOpen"))
            .order_by("createdDate", true)
            .limit(50)
            .disable_authz()
            .list()
    })?;
    let handoffs = quietly(|| {
        facade
            .find(HANDOFF)?
            .eq("userId", user.clone())
            .condition(Condition::is_in(
                "statusId",
                vec![Value::string("AghWatching"), Value::string("AghFailed")],
            ))
            .order_by("createdDate", true)
            .limit(50)
            .disable_authz()
            .list()
    })?;
    let now = run::now_millis();
    let recent = |states: Vec<RunState>, since: Option<i64>, limit: i64| {
        quietly(|| {
            let mut find = facade
                .find(run::RUN)?
                .eq("userId", user.clone())
                .condition(Condition::is_in(
                    "statusId",
                    states.iter().map(|state| Value::string(state.status_id())).collect(),
                ))
                .disable_authz();
            if let Some(since) = since {
                find = find.condition(Condition::greater("submittedDate", run::stamp(since)));
            }
            find.order_by("submittedDate", true).limit(limit).list()
        })
    };
    let running = recent(vec![RunState::Submitted, RunState::Working], None, 50)?;
    let failed = recent(vec![RunState::Failed], Some(now - 7 * 86_400_000), 20)?;
    let done = recent(vec![RunState::Completed], Some(now - 86_400_000), 20)?;
    let waiting_runs = recent(vec![RunState::InputRequired, RunState::AuthRequired], None, 100)?;

    let mut run_ids: Vec<String> = Vec::new();
    for rows in [&approvals, &questions, &handoffs] {
        run_ids.extend(rows.values().iter().filter_map(|row| text_of(row, "runId")));
    }
    let mut runs = runs_by_id(facade, &run_ids)?;
    for rows in [&running, &failed, &done, &waiting_runs] {
        for row in rows.values() {
            let run = RunRow::of(row);
            runs.insert(run.run_id.clone(), run);
        }
    }
    let conversation_ids: BTreeSet<String> = runs.values().map(|run| run.conversation_id.clone()).collect();
    let conversations = conversations_by_id(facade, &conversation_ids)?;
    let context = |run_id: Option<String>| -> (Json, Json, Json) {
        let run = run_id.as_ref().and_then(|id| runs.get(id));
        let conversation_id = run.map(|run| run.conversation_id.clone());
        let title = conversation_id
            .as_ref()
            .and_then(|id| conversations.get(id))
            .and_then(|row| text_of(row, "title"));
        (
            json!(conversation_id),
            json!(title),
            json!(run.map(|run| run.surface.as_str())),
        )
    };
    let mut waiting = Vec::new();
    for row in approvals.values() {
        let (conversation_id, title, surface) = context(text_of(row, "runId"));
        let view = ApprovalView::of(row);
        waiting.push(json!({
            "kind": "approval", "approvalId": view.approval_id, "runId": view.run_id, "conversationId": conversation_id,
            "conversationTitle": title, "surface": surface, "summary": view.summary(), "targetName": view.target_name,
            "createdDate": view.created, "expiresAt": view.expires,
        }));
    }
    for row in questions.values() {
        let run_id = text_of(row, "runId");
        let budget = run_id.as_ref().and_then(|id| runs.get(id)).is_some_and(|run| {
            run.pause_kind == Some(PauseKind::Budget) && run.pause_ref == text_of(row, "questionId")
        });
        let (conversation_id, title, surface) = context(run_id.clone());
        waiting.push(json!({
            "kind": if budget { "budget" } else { "question" }, "questionId": text_of(row, "questionId"),
            "runId": run_id, "conversationId": conversation_id, "conversationTitle": title, "surface": surface,
            "summary": text_of(row, "questionText"), "createdDate": millis(row, "createdDate"),
            "expiresAt": millis(row, "expireDate"),
        }));
    }
    for row in handoffs.values() {
        let (conversation_id, title, surface) = context(text_of(row, "runId"));
        let mut item = handoff_json(row, None);
        item["kind"] = json!("handoff");
        item["conversationId"] = conversation_id;
        item["conversationTitle"] = title;
        item["surface"] = surface;
        waiting.push(item);
    }
    let run_items = |rows: &crate::entity::EntityList| -> Vec<Json> {
        rows.values()
            .iter()
            .map(|row| {
                let run = RunRow::of(row);
                let (conversation_id, title, surface) = context(Some(run.run_id.clone()));
                json!({
                    "kind": "run", "runId": run.run_id, "agentId": run.agent_id, "conversationId": conversation_id,
                    "conversationTitle": title, "surface": surface, "state": run.state.wire(),
                    "pauseKind": run.pause_kind.map(pause_code), "submittedDate": millis(row, "submittedDate"),
                    "endedDate": millis(row, "endedDate"), "errorCode": text_of(row, "errorCode"),
                    "errorText": text_of(row, "errorText"),
                })
            })
            .collect()
    };
    let count = waiting.len();
    Ok(json!({
        "waiting": waiting,
        "running": run_items(&running),
        "failed": run_items(&failed),
        "done": run_items(&done),
        // threads started by triggered agents come with phase 13.9
        "inbox": Json::Array(Vec::new()),
        "counts": {"waiting": count, "running": running.values().len()},
    }))
}

/// `GET prompts`: the suggested prompts for the page: the admin's rows for the agent (or any agent) whose screen path
/// pattern (a regular expression over the whole `/apps/…` path) and entity fit, then the registered providers'.
pub(super) fn prompts(caller: &Caller, params: &Params) -> Answer {
    let facade = &caller.facade;
    let agent_id = params.text("agentId").unwrap_or_else(|| "assist".to_string());
    let path = params.text("path").and_then(|path| super::state::session_path(&path));
    let entity_name = params.text("entityName");
    let rows = quietly(|| {
        facade
            .find(SUGGESTED)?
            .order_by("sequenceNum", false)
            .use_cache(true)
            .disable_authz()
            .list()
    })?;
    let mut out: Vec<Json> = Vec::new();
    for row in rows.values() {
        if text_of(row, "disabled").as_deref() == Some("Y") {
            continue;
        }
        if text_of(row, "agentId").is_some_and(|agent| agent != agent_id) {
            continue;
        }
        if let Some(pattern) = text_of(row, "screenPathPattern") {
            let fits = path.as_deref().is_some_and(|path| {
                moqui_lib::object::regex_full_match(path, &pattern).unwrap_or_else(|error| {
                    tracing::error!(
                        "suggested prompt {} has a screen path pattern that is not a regular expression: {error}",
                        text_of(row, "promptId").unwrap_or_default()
                    );
                    false
                })
            });
            if !fits {
                continue;
            }
        }
        if text_of(row, "entityName").is_some_and(|entity| Some(entity) != entity_name) {
            continue;
        }
        let (Some(label), Some(text)) = (text_of(row, "label"), text_of(row, "promptText")) else {
            continue;
        };
        out.push(
            json!({"promptId": text_of(row, "promptId"), "label": label, "text": text,
            "send": text_of(row, "sendMode").as_deref() == Some("send"), "source": "admin"}),
        );
    }
    // the apps' <shell-providers>: their assist-prompt elements for this page, and the response actions
    let shell = super::prompts::shell_providers();
    for (source, prompt) in &shell.prompts {
        if prompt_fits(path.as_deref(), entity_name.as_deref(), prompt) {
            out.push(json!({"promptId": Json::Null, "label": prompt.get("label"), "text": prompt.get("text"),
                "send": prompt.get("send").and_then(Json::as_bool).unwrap_or(false), "source": source}));
        }
    }
    let query = super::prompts::PromptQuery {
        user_id: caller.user_id.clone(),
        agent_id: agent_id.clone(),
        path: path.clone(),
        entity_name: entity_name.clone(),
        record_id: params.text("recordId"),
    };
    for (source, prompt) in super::prompts::from_providers(facade, &query) {
        out.push(
            json!({"promptId": Json::Null, "label": prompt.label, "text": prompt.text, "send": prompt.send,
            "source": source}),
        );
    }
    // procedure skills whose screens hold the page (plan 7.6): a chip that names the skill in the composer
    if let Some(path) = &path {
        let index = crate::llm::skills::SkillIndex::for_audience(
            &caller.factory,
            facade,
            &crate::llm::skills::Audience {
                user_id: Some(&caller.user_id),
                agent_id: Some(&agent_id),
                surface: Some(crate::llm::prompt::Surface::Drawer),
            },
        );
        for skill in &index.skills {
            let on_page = skill.kind == "procedure"
                && skill
                    .screens
                    .iter()
                    .any(|screen| !screen.is_empty() && (path == screen || path.starts_with(&format!("{screen}/"))));
            if on_page && out.len() < MAX_PROMPTS {
                out.push(json!({"promptId": Json::Null, "label": skill.title, "text": format!("@{} ", skill.name),
                    "send": false, "source": "skill"}));
            }
        }
    }
    out.truncate(MAX_PROMPTS);
    let mut actions = vec![json!({"key": "download", "label": "Download as Markdown", "icon": "sym_o_download", "builtin": true})];
    for (source, action) in &shell.actions {
        let mut action = action.clone();
        let label = action.get("label").and_then(Json::as_str).unwrap_or_default().to_string();
        action["key"] = json!(format!("app:{source}:{}", crate::llm::skills::slug(&label)));
        action["source"] = json!(source);
        actions.push(action);
    }
    Ok(json!({"prompts": out, "responseActions": actions}))
}

/// Whether an app's assist prompt applies on `path` (an `/apps` path) for a record of `entity`: its page is the path or
/// above it, and its entity, when it names one, is the page's.
fn prompt_fits(path: Option<&str>, entity: Option<&str>, prompt: &Json) -> bool {
    let wanted = prompt.get("entity").and_then(Json::as_str).filter(|entity| !entity.is_empty());
    if wanted.is_some() && wanted != entity {
        return false;
    }
    let Some(url) = prompt.get("url").and_then(Json::as_str) else {
        return true;
    };
    let Some(path) = path else {
        return false;
    };
    let page = url.split('?').next().unwrap_or(url);
    let page = super::state::session_path(page).unwrap_or_else(|| page.to_string());
    path == page || path.starts_with(&format!("{page}/"))
}

/// `GET mentions`: the @ picker's agents or skills matching `q`. Records come from the palette search
/// (`kind=records`, which the transition answers through `/shellSearch`'s sources).
pub(super) fn mentions(caller: &Caller, params: &Params) -> Answer {
    let kind = params.text("kind").unwrap_or_else(|| "agents".to_string());
    let query = params.text("q").unwrap_or_default().to_lowercase();
    let fits = |texts: &[Option<String>]| {
        query.is_empty() || texts.iter().flatten().any(|text| text.to_lowercase().contains(&query))
    };
    match kind.as_str() {
        "agents" => {
            let items: Vec<Json> = drawer_agents(&caller.facade)?
                .into_iter()
                .filter(|(agent, row)| {
                    fits(&[
                        Some(agent.agent_id.clone()),
                        Some(agent.name.clone()),
                        text_of(row, "description"),
                    ])
                })
                .take(MAX_MENTIONS)
                .map(|(agent, row)| {
                    json!({"kind": "agent", "agentId": agent.agent_id,
                        "label": if agent.name.is_empty() { agent.agent_id.clone() } else { agent.name.clone() },
                        "description": text_of(&row, "description")})
                })
                .collect();
            Ok(json!({"kind": kind, "items": items}))
        }
        "skills" => {
            // the user's index for the agent asked about (personal skills shadow the site's): a name that starts with
            // the words first, then retrieval's ranking
            let agent_id = params.text("agentId").unwrap_or_else(|| "assist".to_string());
            let index = crate::llm::skills::SkillIndex::for_audience(
                &caller.factory,
                &caller.facade,
                &crate::llm::skills::Audience {
                    user_id: Some(&caller.user_id),
                    agent_id: Some(&agent_id),
                    surface: Some(crate::llm::prompt::Surface::Drawer),
                },
            );
            let mut names: Vec<String> = index
                .skills
                .iter()
                .filter(|skill| query.is_empty() || skill.name.starts_with(&query))
                .map(|skill| skill.name.clone())
                .collect();
            if !query.is_empty() {
                for found in crate::llm::skills::retrieval::rank(&index, &query, None) {
                    if !names.contains(&found.name) {
                        names.push(found.name);
                    }
                }
            }
            let items: Vec<Json> = names
                .iter()
                .filter_map(|name| index.get(name))
                .take(MAX_MENTIONS)
                .map(|skill| {
                    json!({"kind": "skill", "name": skill.name, "label": skill.title,
                        "description": skill.description, "personal": skill.is_personal()})
                })
                .collect();
            Ok(json!({"kind": kind, "items": items}))
        }
        "records" => Err(DrawerError::invalid(
            "records come from the palette search: GET mentions?kind=records goes through it",
        )),
        other => Err(DrawerError::invalid(format!(
            "{other} is not a mention kind; it is agents, skills or records"
        ))),
    }
}
