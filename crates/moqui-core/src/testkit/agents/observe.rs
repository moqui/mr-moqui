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

//! What a run did, read back from its rows once it settled: the model's replies (its steps and assistant messages:
//! text, tool calls, finish, usage), the tool calls with their full results (the tool messages' parts), the writes
//! held for approval (the run's `AgentApproval` rows, with what became of each), the questions asked, the
//! compactions, the answer and its invented references (`AgentRun.resultJson`), and the conversation's screen state.

use std::collections::{BTreeMap, HashMap};

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde::Serialize;
use serde_json::{json, Value as Json};

use crate::entity::EntityFacade;
use crate::llm::run::{events, load_run, quietly, RunState};
use crate::llm::screen::ScreenState;
use crate::llm::tools::{self, Classification};
use crate::llm::{int_of, text_of};

/// One model reply: the text, the tool calls as the model wrote them, the finish and the usage of its step.
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReply {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tool_calls: Vec<ReplyCall>,
    #[serde(default)]
    pub finish: String,
    #[serde(default)]
    pub usage: Usage,
    /// JSON tool mode: the text the model wrote, which a replay plays (its calls and answer are read from it again).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// A tool call of a reply: the provider's id, the tool, and its arguments; `raw` holds the text of arguments that did
/// not parse.
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct ReplyCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Json,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub cached: u64,
    #[serde(default)]
    pub output: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.cached += other.cached;
        self.output += other.output;
    }
}

/// A tool call and what it answered.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CallSeen {
    pub tool: String,
    pub arguments: Json,
    /// done, failed, denied, unknown-outcome, pending.
    pub status: String,
    pub result: String,
    /// The call failed or its result is an error (`{"error":…`, `--- error …`).
    pub error: bool,
    /// read, write, destructive or external; empty for a tool not offered.
    pub classification: String,
    /// The arguments were repaired (a value converted, a name renamed, an argument dropped).
    pub repaired: bool,
}

/// A write held for the user's approval: an approval the run asked for.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct HeldWrite {
    pub tool: String,
    /// The screen transition, or the service or tool target.
    pub transition: String,
    /// The summary and the changes, `Label: current → proposed`.
    pub text: String,
    /// What became of it: pending, executed, rejected, handed-off, expired, conflict, failed.
    pub status: String,
    /// How it was decided: drawer, policy ...
    pub channel: String,
}

/// One turn: a user message and the run that answered it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct TurnSeen {
    pub say: String,
    pub run_id: String,
    pub state: String,
    pub answer: String,
    pub invented: Vec<String>,
    pub error: Option<String>,
    #[serde(skip)]
    pub replies: Vec<ModelReply>,
    pub calls: Vec<CallSeen>,
    pub held: Vec<HeldWrite>,
    pub questions: Vec<Json>,
    pub screen: Option<ScreenState>,
    #[serde(skip)]
    pub compactions: u32,
    /// What every model call of the run spent, its side calls (the intent hint, a summary, a schema repair) among them:
    /// the `LlmCall` rows.
    #[serde(skip)]
    pub spent: Usage,
    /// The system text of the conversation's frozen prefix, in characters (on the last turn).
    #[serde(skip)]
    pub prefix_chars: u64,
    #[serde(skip)]
    pub wall_ms: u64,
}

impl TurnSeen {
    /// The turn as the checks read it.
    pub fn to_check(&self) -> Json {
        let screen = self.screen.as_ref().map_or(Json::Null, |screen| {
            json!({"path": screen.path, "parameters": screen.parameters, "values": screen.values, "url": screen.url()})
        });
        json!({
            "say": self.say,
            "state": self.state,
            "answer": self.answer,
            "invented": self.invented,
            "error": self.error,
            "calls": self.calls,
            "held": self.held,
            "questions": self.questions,
            "screen": screen,
        })
    }
}

/// A run state's name, as the checks and the transcript write it: Completed, Failed, InputRequired …
pub fn state_name(state: RunState) -> String {
    format!("{state:?}")
}

/// Read the turn's run back. `say` is the user's message.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn observe(facade: &EntityFacade, conversation_id: &str, run_id: &str, say: &str) -> Result<TurnSeen, Error> {
    quietly(|| observe_quietly(facade, conversation_id, run_id, say))
}

fn observe_quietly(facade: &EntityFacade, conversation_id: &str, run_id: &str, say: &str) -> Result<TurnSeen, Error> {
    let run = load_run(facade, run_id)?.ok_or_else(|| Error::new(format!("there is no agent run {run_id}")))?;
    let run_row = facade
        .find("moqui.agent.AgentRun")?
        .eq("runId", Value::string(run_id))
        .disable_authz()
        .one()?;
    let mut turn = TurnSeen {
        say: say.to_string(),
        run_id: run_id.to_string(),
        state: state_name(run.state),
        ..TurnSeen::default()
    };
    if let Some(row) = &run_row {
        if let Some(result) = text_of(row, "resultJson").and_then(|text| serde_json::from_str::<Json>(&text).ok()) {
            turn.answer = result.get("text").and_then(Json::as_str).unwrap_or_default().to_string();
            turn.invented = result
                .get("invented")
                .and_then(Json::as_array)
                .map(|list| list.iter().filter_map(Json::as_str).map(str::to_string).collect())
                .unwrap_or_default();
        }
        let code = text_of(row, "errorCode");
        let text = text_of(row, "errorText").or_else(|| text_of(row, "statusReason"));
        if code.is_some() || (run.state == RunState::Failed && text.is_some()) {
            turn.error = Some(
                [code, text]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(": "),
            );
        }
    }

    // every call of the run, the side calls among them
    for row in facade
        .find("moqui.llm.LlmCall")?
        .eq("runId", Value::string(run_id))
        .disable_authz()
        .list()?
        .values()
    {
        let count = |field: &str| int_of(row, field).and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
        turn.spent.add(&Usage {
            input: count("inputTokens"),
            cached: count("cachedInputTokens"),
            output: count("outputTokens"),
        });
    }

    // the steps, for the finish and usage of each reply
    let steps = facade
        .find("moqui.agent.AgentRunStep")?
        .eq("runId", Value::string(run_id))
        .order_by("stepSeq", false)
        .disable_authz()
        .list()?;
    // in step order, as the replies are
    let step_info: Vec<(String, Usage)> = steps
        .values()
        .iter()
        .map(|row| {
            let count = |field: &str| int_of(row, field).and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
            (
                text_of(row, "finishReason").unwrap_or_default(),
                Usage {
                    input: count("inputTokens"),
                    cached: count("cachedInputTokens"),
                    output: count("outputTokens"),
                },
            )
        })
        .collect();

    // the messages of the run and their parts
    let messages = facade
        .find("moqui.agent.AgentMessage")?
        .eq("runId", Value::string(run_id))
        .order_by("messageSeq", false)
        .disable_authz()
        .list()?;
    let ids: Vec<Value> = messages
        .values()
        .iter()
        .filter_map(|row| text_of(row, "messageId"))
        .map(Value::string)
        .collect();
    let mut parts_of: HashMap<String, Vec<PartRow>> = HashMap::new();
    if !ids.is_empty() {
        let parts = facade
            .find("moqui.agent.AgentMessagePart")?
            .condition(Condition::is_in("messageId", ids))
            .disable_authz()
            .list()?;
        for row in parts.values() {
            parts_of.entry(text_of(row, "messageId").unwrap_or_default()).or_default().push(PartRow {
                seq: int_of(row, "partSeq").unwrap_or(0),
                kind: text_of(row, "partKindEnumId").unwrap_or_default(),
                text: row
                    .get("textValue")
                    .filter(|value| !value.is_null())
                    .map(moqui_lib::object::to_plain_string),
                data: text_of(row, "dataJson"),
                tool_call_id: text_of(row, "toolCallId"),
            });
        }
    }
    let mut results: HashMap<String, String> = HashMap::new();
    let mut step_index = 0;
    for message in messages.values() {
        let role = text_of(message, "role").unwrap_or_default();
        let mut parts = parts_of
            .remove(&text_of(message, "messageId").unwrap_or_default())
            .unwrap_or_default();
        parts.sort_by_key(|part| part.seq);
        match role.as_str() {
            "assistant" => {
                let mut reply = ModelReply {
                    raw: text_of(message, "metadataJson")
                        .and_then(|text| serde_json::from_str::<Json>(&text).ok())
                        .and_then(|meta| meta.get("raw").and_then(Json::as_str).map(str::to_string)),
                    ..ModelReply::default()
                };
                for part in parts {
                    match part.kind.as_str() {
                        "AgpkText" => reply.text.push_str(&part.text.unwrap_or_default()),
                        "AgpkToolCall" => {
                            let data: Json = part
                                .data
                                .and_then(|data| serde_json::from_str(&data).ok())
                                .unwrap_or(Json::Null);
                            reply.tool_calls.push(reply_call(&data));
                        }
                        _ => {}
                    }
                }
                if let Some((finish, usage)) = step_info.get(step_index) {
                    reply.finish.clone_from(finish);
                    reply.usage = *usage;
                }
                step_index += 1;
                turn.replies.push(reply);
            }
            "tool" => {
                for part in parts {
                    if part.kind == "AgpkToolResult" {
                        if let Some(id) = part.tool_call_id {
                            results.insert(id, part.text.unwrap_or_default());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // the tool calls in the order they were asked for
    let calls = facade
        .find("moqui.agent.AgentToolCall")?
        .eq("runId", Value::string(run_id))
        .order_by("stepSeq", false)
        .order_by("toolCallId", false)
        .disable_authz()
        .list()?;
    let specs: BTreeMap<String, Json> = tools::registered()
        .iter()
        .map(|tool| (tool.spec().name.clone(), tool.spec().parameters.clone()))
        .collect();
    for row in calls.values() {
        let tool_call_id = text_of(row, "toolCallId").unwrap_or_default();
        let tool = text_of(row, "toolName").unwrap_or_default();
        let arguments: Json = text_of(row, "argumentsJson")
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Json::Null);
        let result = results
            .remove(&tool_call_id)
            .or_else(|| text_of(row, "resultSummary"))
            .unwrap_or_default();
        let status = match text_of(row, "statusId").as_deref() {
            Some("AgtcDone") => "done",
            Some("AgtcFailed") => "failed",
            Some("AgtcDenied") => "denied",
            Some("AgtcUnknownOutcome") => "unknown-outcome",
            Some("AgtcAwaiting") => "awaiting",
            _ => "pending",
        }
        .to_string();
        let error = status == "failed" || is_error_text(&result);
        let repaired = specs.get(&tool).is_some_and(|schema| {
            let repaired = tools::repair::repair(&arguments, schema);
            !repaired.fixed.is_empty() || !repaired.ignored.is_empty()
        });
        turn.calls.push(CallSeen {
            classification: text_of(row, "classificationEnumId")
                .and_then(|id| Classification::parse(&id))
                .map(|class| class.as_str().to_string())
                .unwrap_or_default(),
            tool,
            arguments,
            status,
            result,
            error,
            repaired,
        });
    }

    // the approvals the run asked for, in order
    let approvals = facade
        .find("moqui.agent.AgentApproval")?
        .eq("runId", Value::string(run_id))
        .order_by("createdDate", false)
        .disable_authz()
        .list()?;
    let tools_of: BTreeMap<String, String> = calls
        .values()
        .iter()
        .filter_map(|row| Some((text_of(row, "toolCallId")?, text_of(row, "toolName")?)))
        .collect();
    for row in approvals.values() {
        let view = crate::llm::approval::ApprovalView::of(row);
        turn.held.push(HeldWrite {
            tool: tools_of.get(&view.tool_call_id).cloned().unwrap_or_default(),
            transition: view
                .target_name
                .rsplit_once('#')
                .map_or(view.target_name.as_str(), |(_, name)| name)
                .to_string(),
            text: held_text(&view),
            status: view.status.as_str().to_string(),
            channel: view.decision_channel.clone().unwrap_or_default(),
        });
    }

    for event in events::stored_after(facade, run_id, 0, 100_000)? {
        match event.event_type.as_str() {
            "question.asked" => turn.questions.push(event.payload),
            "compaction" => turn.compactions += 1,
            _ => {}
        }
    }
    turn.screen = crate::llm::screen::load_state(facade, conversation_id)?;
    Ok(turn)
}

/// A message part as read: its place, kind, text, data and tool call.
struct PartRow {
    seq: i64,
    kind: String,
    text: Option<String>,
    data: Option<String>,
    tool_call_id: Option<String>,
}

/// A tool call part's data, `{id, name, arguments}`, as a reply call; arguments that did not parse are stored as
/// `{"raw": text}`.
fn reply_call(data: &Json) -> ReplyCall {
    let text = |key: &str| data.get(key).and_then(Json::as_str).unwrap_or_default().to_string();
    let arguments = data.get("arguments").cloned().unwrap_or(Json::Null);
    let raw = arguments
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get("raw"))
        .and_then(Json::as_str)
        .map(str::to_string);
    ReplyCall {
        id: text("id"),
        name: text("name"),
        arguments: if raw.is_some() { Json::Null } else { arguments },
        raw,
    }
}

/// An error result: the teaching JSON (`{"error": …}`), a plain `Error: …`, or a screen answer ending `--- error …`.
pub fn is_error_text(text: &str) -> bool {
    let trimmed = text.trim();
    tools::format::is_error_json(trimmed)
        || trimmed.starts_with("Error:")
        || trimmed
            .lines()
            .rev()
            .find(|line| line.starts_with("--- "))
            .is_some_and(|line| line.starts_with("--- error"))
}

/// An approval as a line: its summary and its changes, `Label: current → proposed`.
fn held_text(view: &crate::llm::approval::ApprovalView) -> String {
    let changes: Vec<String> = view
        .changes()
        .iter()
        .map(|change| {
            let label = if change.label.is_empty() {
                &change.field
            } else {
                &change.label
            };
            let current = if change.current.is_empty() {
                "(empty)"
            } else {
                &change.current
            };
            format!("{label}: {current} → {}", change.proposed)
        })
        .collect();
    if changes.is_empty() {
        view.summary()
    } else {
        format!("{} ({})", view.summary(), changes.join(", "))
    }
}

/// The transition of a held write in a result line `approval pending: <transition> …`, as the recordings made before
/// approvals have it.
pub fn held_transition(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.split("approval pending:").nth(1)?;
        rest.split_whitespace().next().map(str::to_string)
    })
}

/// The system text of the conversation's frozen prefix, in characters.
pub fn prefix_chars(facade: &EntityFacade, conversation_id: &str) -> u64 {
    quietly(|| {
        facade
            .find("moqui.agent.AgentConversation")
            .and_then(|find| find.eq("conversationId", Value::string(conversation_id)).disable_authz().one())
            .ok()
            .flatten()
            .and_then(|row| text_of(&row, "systemText"))
            .map_or(0, |text| text.chars().count() as u64)
    })
}
