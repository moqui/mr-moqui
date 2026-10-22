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

//! The pane's reducer, with no Revue and no I/O: the drawer's contract (docs/Assist.md, `moqui.assist` in
//! `WebrootAssist.js`) in Rust. A conversation as `GET conversation` answers it is a [`Thread`]; a run's stored events
//! are applied in sequence ([`Thread::apply`]), each answering the [`Effect`]s the session acts on; [`Thread::rows`]
//! are the lines the pane draws.

use std::collections::BTreeMap;

use serde_json::Value;

const PAUSED: [&str; 2] = ["TASK_STATE_INPUT_REQUIRED", "TASK_STATE_AUTH_REQUIRED"];
const ENDED: [&str; 4] = [
    "TASK_STATE_COMPLETED",
    "TASK_STATE_FAILED",
    "TASK_STATE_CANCELED",
    "TASK_STATE_REJECTED",
];

pub fn is_paused(state: &str) -> bool {
    PAUSED.contains(&state)
}

pub fn is_ended(state: &str) -> bool {
    ENDED.contains(&state)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    User,
    Assistant,
    /// A line of the pane's own: a follow note, a decision, an error.
    Note,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    pub message_id: Option<String>,
    pub role: Role,
    pub run_id: Option<String>,
    pub client_message_id: Option<String>,
    /// A message of this client the server has not answered (`sending`), queued for the active run, or not sent.
    pub local: Option<String>,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Step(Step),
}

/// A tool call's step row: `· screen go /apps/…  ok`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub tool_call_id: String,
    pub name: String,
    pub arguments: String,
    /// `running`, `done`, `failed`, `denied`, `unknown-outcome`, or the stored call's status.
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub run_id: String,
    pub state: String,
    pub pause_kind: String,
    pub steps: u64,
    pub tokens: u64,
    pub seconds: u64,
    pub error: String,
    pub done: bool,
}

/// One "Field: current → proposed" line of an approval.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub label: String,
    pub current: String,
    pub proposed: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Approval {
    pub approval_id: String,
    pub run_id: String,
    pub tool_call_id: String,
    pub classification: String,
    pub summary: String,
    pub changes: Vec<Change>,
    pub status: String,
    /// Edit in screen is offered.
    pub can_edit: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Question {
    pub question_id: String,
    pub run_id: String,
    pub text: String,
    /// Each option's label and whether it is the recommended one.
    pub options: Vec<(String, bool)>,
    pub free_text: bool,
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handoff {
    pub handoff_id: String,
    pub path: String,
    pub form: String,
    pub values: Vec<(String, String)>,
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Queued {
    pub input_id: String,
    pub text: String,
    pub client_message_id: String,
}

/// The follow owner the server has, and whether it is this client.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Owner {
    pub client_id: String,
    pub context_version: u64,
    pub following: bool,
}

/// What the session does for an event this client follows (docs/Assist.md "Follow").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Follow {
    Navigate {
        path: String,
        parameters: BTreeMap<String, String>,
        title: String,
    },
    /// A field to fill: by its tui id (a buffer key), else by its form and request name.
    Fill {
        tui_id: String,
        form: String,
        field: String,
        value: String,
        label: String,
    },
    Focus {
        tui_id: String,
    },
    Dialog {
        id: String,
        open: bool,
    },
    /// The hand-off's form is filled: its Save takes the focus.
    Handoff {
        form: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Read the stored events of the run after this sequence.
    Gap {
        run_id: String,
        after: u64,
    },
    Follow(Follow),
    /// Read the conversation again: an event that cannot be placed.
    Refresh,
    /// The run waits for the user; the poll stops until the user acts.
    Paused,
    Ended,
}

/// Whose events apply: this client's id, and whether follow is on.
#[derive(Clone, Copy, Debug)]
pub struct Ctx<'a> {
    pub client_id: &'a str,
    pub follow_on: bool,
}

/// One line the pane draws, before wrapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    User,
    Assistant,
    Step,
    StepFailed,
    Note,
    Done,
    Error,
    Canvas,
}

/// A conversation as the pane holds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Thread {
    pub conversation_id: String,
    pub title: String,
    pub items: Vec<Item>,
    pub runs: BTreeMap<String, Run>,
    pub active_run: Option<String>,
    /// The last stored event applied, by run; ui events at or below a run's floor (what the snapshot had) never apply.
    pub seq: BTreeMap<String, u64>,
    pub floor: BTreeMap<String, u64>,
    pub approvals: Vec<Approval>,
    pub questions: Vec<Question>,
    pub handoffs: Vec<Handoff>,
    pub queued: Vec<Queued>,
    /// The conversation's canvas: its title, when it has one open.
    pub canvas: Option<String>,
    pub owner: Owner,
    /// This client's page version.
    pub context_version: u64,
    /// The tool the active run calls now, for the status line.
    pub tool: Option<String>,
}

fn text(value: &Value, name: &str) -> String {
    match value.get(name) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

fn number(value: &Value, name: &str) -> u64 {
    match value.get(name) {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(text)) => text.parse().unwrap_or(0),
        _ => 0,
    }
}

fn list<'a>(value: &'a Value, name: &str) -> impl Iterator<Item = &'a Value> {
    value.get(name).and_then(Value::as_array).into_iter().flatten()
}

fn flat(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(name, value)| match value {
                    Value::String(text) => (name.clone(), text.clone()),
                    other => (name.clone(), other.to_string()),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A path the pane may go to: `/apps/…` (or `/qapps/…`, read as `/apps/…`) with no scheme, host, `..`, `//`,
/// backslash or control character, as the drawer's `safePath`.
pub fn safe_path(path: &str) -> Option<String> {
    let path = path.trim();
    let bad = path.contains("..")
        || path.contains("//")
        || path.contains('\\')
        || path.contains("://")
        || path.chars().any(char::is_control);
    if bad {
        return None;
    }
    let path = match path.strip_prefix("/qapps") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("/apps{rest}"),
        _ => path.to_string(),
    };
    (path == "/apps" || path.starts_with("/apps/")).then_some(path)
}

fn approval_of(card: &Value, run_id: &str) -> Approval {
    Approval {
        approval_id: text(card, "approvalId"),
        run_id: Some(text(card, "runId"))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| run_id.to_string()),
        tool_call_id: text(card, "toolCallId"),
        classification: text(card, "classification"),
        summary: Some(text(card, "summary"))
            .filter(|summary| !summary.is_empty())
            .unwrap_or_else(|| text(card, "targetName")),
        changes: list(card, "changes")
            .map(|change| Change {
                label: Some(text(change, "label"))
                    .filter(|label| !label.is_empty())
                    .unwrap_or_else(|| text(change, "field")),
                current: text(change, "current"),
                proposed: text(change, "proposed"),
            })
            .collect(),
        status: Some(text(card, "status"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "pending".into()),
        can_edit: card.get("canEdit").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn question_of(card: &Value, run_id: &str) -> Question {
    let mut options: Vec<(String, bool)> = list(card, "options")
        .map(|option| match option {
            Value::String(label) => (label.clone(), false),
            _ => (
                text(option, "label"),
                option.get("recommended").and_then(Value::as_bool).unwrap_or(false),
            ),
        })
        .collect();
    if options.is_empty() && text(card, "kind") == "budget" {
        options = vec![("continue".into(), false), ("stop".into(), false)];
    }
    Question {
        question_id: text(card, "questionId"),
        run_id: Some(text(card, "runId"))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| run_id.to_string()),
        text: text(card, "text"),
        options,
        free_text: card.get("freeText").and_then(Value::as_bool).unwrap_or(false),
        status: "open".into(),
    }
}

fn handoff_of(card: &Value) -> Handoff {
    Handoff {
        handoff_id: text(card, "handoffId"),
        path: text(card, "path"),
        form: text(card, "form"),
        values: list(card, "values")
            .map(|value| {
                let label = Some(text(value, "label")).filter(|l| !l.is_empty());
                (label.unwrap_or_else(|| text(value, "field")), text(value, "value"))
            })
            .collect(),
        status: "watching".into(),
    }
}

fn run_of(value: &Value) -> Run {
    Run {
        run_id: text(value, "runId"),
        state: text(value, "state"),
        pause_kind: text(value, "pauseKind"),
        steps: number(value, "toolCallCount"),
        tokens: number(value, "inputTokens") + number(value, "outputTokens"),
        seconds: number(value, "activeSeconds"),
        error: text(value, "errorText"),
        done: is_ended(&text(value, "state")),
    }
}

impl Thread {
    /// The conversation as `GET conversation` answers it. A message of this client the server does not have yet is
    /// kept at the end; the active run's events are read after its `lastEventSeq`, and none of those at or below it
    /// ever moves the screen.
    pub fn snapshot(&mut self, json: &Value, client_id: &str) {
        let conversation = json.get("conversation").cloned().unwrap_or(Value::Null);
        let id = text(&conversation, "conversationId");
        if !self.conversation_id.is_empty() && !id.is_empty() && id != self.conversation_id {
            *self = Self::default();
        }
        if !id.is_empty() {
            self.conversation_id = id;
        }
        self.title = text(&conversation, "title");
        let mut items: Vec<Item> = Vec::new();
        let mut results: BTreeMap<String, String> = BTreeMap::new();
        for item in list(json, "items") {
            let role = match text(item, "role").as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => {
                    for part in list(item, "parts").filter(|part| text(part, "kind") == "tool-result") {
                        let failed = part.get("isError").and_then(Value::as_bool).unwrap_or(false);
                        results.insert(text(part, "toolCallId"), if failed { "failed" } else { "done" }.into());
                    }
                    continue;
                }
            };
            let parts = list(item, "parts")
                .filter_map(|part| match text(part, "kind").as_str() {
                    "text" => Some(Part::Text(text(part, "text"))),
                    "tool-call" => Some(Part::Step(Step {
                        tool_call_id: text(part, "toolCallId"),
                        name: text(part, "name"),
                        arguments: text(part, "arguments"),
                        status: text(part, "status"),
                    })),
                    _ => None,
                })
                .collect();
            items.push(Item {
                message_id: Some(text(item, "messageId")).filter(|id| !id.is_empty()),
                role,
                run_id: Some(text(item, "runId")).filter(|id| !id.is_empty()),
                client_message_id: Some(text(item, "clientMessageId")).filter(|id| !id.is_empty()),
                local: None,
                parts,
            });
        }
        for item in &mut items {
            for part in &mut item.parts {
                if let Part::Step(step) = part {
                    if let Some(status) = results.get(&step.tool_call_id) {
                        step.status.clone_from(status);
                    }
                }
            }
        }
        let pending = json.get("pending").cloned().unwrap_or(Value::Null);
        self.queued = list(&pending, "queued")
            .map(|entry| Queued {
                input_id: text(entry, "inputId"),
                text: text(entry, "text"),
                client_message_id: text(entry, "clientMessageId"),
            })
            .collect();
        let known = |client: &Option<String>| {
            client.as_ref().is_some_and(|id| {
                items.iter().any(|item| item.client_message_id.as_ref() == Some(id))
                    || self.queued.iter().any(|entry| &entry.client_message_id == id)
            })
        };
        let local: Vec<Item> = self
            .items
            .iter()
            .filter(|item| item.message_id.is_none() && item.role == Role::User && !known(&item.client_message_id))
            .cloned()
            .collect();
        items.extend(local);
        self.items = items;
        self.runs = list(json, "runs")
            .map(run_of)
            .map(|run| (run.run_id.clone(), run))
            .collect();
        self.active_run = None;
        if let Some(active) = json.get("activeRun").filter(|run| run.is_object()) {
            let run = run_of(active);
            let last = number(active, "lastEventSeq");
            let at = self.seq.entry(run.run_id.clone()).or_insert(last);
            *at = (*at).max(last);
            let floor = self.floor.entry(run.run_id.clone()).or_insert(last);
            *floor = (*floor).max(last);
            self.active_run = Some(run.run_id.clone());
            self.runs.insert(run.run_id.clone(), run);
        }
        self.approvals = list(&pending, "approvals").map(|card| approval_of(card, "")).collect();
        self.questions = list(&pending, "questions").map(|card| question_of(card, "")).collect();
        self.handoffs = list(&pending, "handoffs").map(handoff_of).collect();
        self.canvas = json
            .get("canvas")
            .filter(|canvas| canvas.is_object() && !canvas.get("closed").and_then(Value::as_bool).unwrap_or(false))
            .map(|canvas| text(canvas, "title"));
        if let Some(follow) = json.get("follow").filter(|follow| follow.is_object()) {
            self.take_follow(follow, client_id);
        }
    }

    /// The follow owner a call answered (turn, follow, context, decide): this client's version is at least its.
    pub fn take_follow(&mut self, follow: &Value, client_id: &str) {
        let owner = text(follow, "clientId");
        self.owner = Owner {
            following: !owner.is_empty() && owner == client_id,
            client_id: owner,
            context_version: number(follow, "contextVersion"),
        };
        self.context_version = self.context_version.max(self.owner.context_version);
    }

    /// A message of this client, before the server answers it.
    pub fn outgoing(&mut self, client_message_id: &str, text: &str) {
        // a new conversation's title is the first line of its first message, as the server makes it
        if self.title.is_empty() {
            self.title = text.lines().next().unwrap_or("").chars().take(80).collect();
        }
        self.items.push(Item {
            role: Role::User,
            client_message_id: Some(client_message_id.to_string()),
            local: Some("sending".into()),
            parts: vec![Part::Text(text.to_string())],
            ..Item::default()
        });
    }

    /// The turn's answer: the message is the run's, or queued for the active one; this client follows.
    pub fn turn_answered(&mut self, client_message_id: &str, json: &Value, client_id: &str) {
        let conversation = text(json, "conversationId");
        if !conversation.is_empty() {
            self.conversation_id = conversation;
        }
        let queued = json.get("queued").and_then(Value::as_bool).unwrap_or(false);
        let run_id = text(json, "runId");
        if let Some(item) = self.local_item(client_message_id) {
            item.local = queued.then(|| "queued".to_string());
            item.run_id = Some(run_id.clone()).filter(|id| !id.is_empty());
        }
        if let Some(follow) = json.get("follow").filter(|follow| follow.is_object()) {
            self.take_follow(follow, client_id);
        }
        if run_id.is_empty() || (queued && self.active_run.is_some()) {
            return;
        }
        let state = text(json, "state");
        let run = self.runs.entry(run_id.clone()).or_insert_with(|| Run {
            run_id: run_id.clone(),
            ..Run::default()
        });
        if run.state.is_empty() {
            run.state = state;
        }
        if !is_ended(&run.state) {
            self.active_run = Some(run_id.clone());
        }
        self.seq.entry(run_id.clone()).or_insert(0);
        self.floor.entry(run_id).or_insert(0);
    }

    pub fn turn_failed(&mut self, client_message_id: &str, error: &str) {
        if let Some(item) = self.local_item(client_message_id) {
            item.local = Some(format!("not sent: {error}"));
        }
    }

    fn local_item(&mut self, client_message_id: &str) -> Option<&mut Item> {
        self.items
            .iter_mut()
            .find(|item| item.message_id.is_none() && item.client_message_id.as_deref() == Some(client_message_id))
    }

    fn item_of(&mut self, message_id: &str, run_id: &str) -> &mut Item {
        let at = match self
            .items
            .iter()
            .position(|item| item.message_id.as_deref() == Some(message_id))
        {
            Some(at) => at,
            None => {
                self.items.push(Item {
                    message_id: Some(message_id.to_string()),
                    role: Role::Assistant,
                    run_id: Some(run_id.to_string()),
                    ..Item::default()
                });
                self.items.len() - 1
            }
        };
        &mut self.items[at]
    }

    /// The `part`-th text part of a message (the stream counts from 0), made when it is not there yet.
    fn text_part(&mut self, message_id: &str, part: usize, run_id: &str) -> Option<&mut String> {
        let item = self.item_of(message_id, run_id);
        let mut count = item.parts.iter().filter(|p| matches!(p, Part::Text(_))).count();
        while count <= part {
            item.parts.push(Part::Text(String::new()));
            count += 1;
        }
        item.parts
            .iter_mut()
            .filter_map(|p| match p {
                Part::Text(text) => Some(text),
                Part::Step(_) => None,
            })
            .nth(part)
    }

    fn step_mut(&mut self, tool_call_id: &str) -> Option<&mut Step> {
        self.items
            .iter_mut()
            .flat_map(|item| item.parts.iter_mut())
            .find_map(|part| match part {
                Part::Step(step) if step.tool_call_id == tool_call_id => Some(step),
                _ => None,
            })
    }

    fn note(&mut self, text: String) {
        self.items.push(Item {
            role: Role::Note,
            parts: vec![Part::Text(text)],
            ..Item::default()
        });
    }

    /// One stored event (`{seq, type, payload}`) of `run_id`. An event at or below the run's last sequence changes
    /// nothing; one past the next answers a gap and changes nothing.
    pub fn apply(&mut self, run_id: &str, event: &Value, ctx: Ctx) -> Vec<Effect> {
        let mut effects = Vec::new();
        let kind = text(event, "type");
        if kind.is_empty() {
            return effects;
        }
        let seq = event.get("seq").and_then(Value::as_u64);
        if let Some(seq) = seq {
            let last = self.seq.get(run_id).copied().unwrap_or(0);
            if seq <= last {
                return effects;
            }
            if seq > last + 1 {
                effects.push(Effect::Gap {
                    run_id: run_id.to_string(),
                    after: last,
                });
                return effects;
            }
            self.seq.insert(run_id.to_string(), seq);
        }
        let p = event.get("payload").cloned().unwrap_or(Value::Null);
        let run = self.runs.entry(run_id.to_string()).or_insert_with(|| Run {
            run_id: run_id.to_string(),
            ..Run::default()
        });
        match kind.as_str() {
            "run.state" => {
                let state = text(&p, "state");
                if !state.is_empty() {
                    run.state = state;
                }
                run.pause_kind = text(&p, "pauseKind");
                let (ended, paused) = (is_ended(&run.state), is_paused(&run.state));
                run.done |= ended;
                if ended {
                    self.end(run_id);
                    effects.push(Effect::Ended);
                } else {
                    self.active_run.get_or_insert_with(|| run_id.to_string());
                    if paused {
                        effects.push(Effect::Paused);
                    }
                }
            }
            "run.done" => {
                let state = text(&p, "state");
                if !state.is_empty() {
                    run.state = state;
                }
                let code = text(&p, "errorCode");
                if !code.is_empty() && run.error.is_empty() {
                    run.error = code;
                }
                run.done = true;
                self.end(run_id);
                effects.push(Effect::Ended);
            }
            "error" => {
                run.error = Some(text(&p, "message"))
                    .filter(|m| !m.is_empty())
                    .unwrap_or(text(&p, "code"))
            }
            "usage" => run.tokens += number(&p, "input") + number(&p, "output"),
            "message.start" if text(&p, "role") != "user" => {
                self.item_of(&text(&p, "messageId"), run_id);
            }
            "text.chunk" => {
                let message = text(&p, "messageId");
                if message.is_empty() {
                    return effects;
                }
                let part = usize::try_from(number(&p, "part")).unwrap_or(0);
                let offset = usize::try_from(number(&p, "offset")).unwrap_or(0);
                let piece = text(&p, "text");
                let Some(have) = self.text_part(&message, part, run_id) else {
                    return effects;
                };
                let length = have.chars().count();
                if offset > length {
                    effects.push(Effect::Refresh);
                } else if offset + piece.chars().count() > length {
                    have.extend(piece.chars().skip(length - offset));
                }
            }
            "part.done" => {
                let message = text(&p, "messageId");
                let part = usize::try_from(number(&p, "part")).unwrap_or(0);
                let length = usize::try_from(number(&p, "length")).unwrap_or(0);
                let have = if message.is_empty() {
                    0
                } else {
                    self.text_part(&message, part, run_id)
                        .map_or(0, |text| text.chars().count())
                };
                if length > have {
                    effects.push(Effect::Refresh);
                }
            }
            "step.restart" => {
                let message = text(&p, "messageId");
                if let Some(item) = self
                    .items
                    .iter_mut()
                    .find(|item| item.message_id.as_deref() == Some(&message))
                {
                    item.parts.retain(|part| !matches!(part, Part::Text(_)));
                }
            }
            "tool.call" => {
                let id = text(&p, "toolCallId");
                if self.step_mut(&id).is_none() {
                    let arguments = match p.get("arguments") {
                        Some(Value::String(text)) => text.clone(),
                        Some(other) => other.to_string(),
                        None => String::new(),
                    };
                    let item = self.item_of(&text(&p, "messageId"), run_id);
                    item.parts.push(Part::Step(Step {
                        tool_call_id: id,
                        name: text(&p, "name"),
                        arguments,
                        status: "running".into(),
                    }));
                    if let Some(run) = self.runs.get_mut(run_id) {
                        run.steps += 1;
                    }
                }
                self.tool = Some(text(&p, "name"));
            }
            "tool.result" => {
                let failed = p.get("isError").and_then(Value::as_bool).unwrap_or(false);
                let status = Some(text(&p, "status"))
                    .filter(|status| !status.is_empty())
                    .unwrap_or_else(|| if failed { "failed" } else { "done" }.into());
                if let Some(step) = self.step_mut(&text(&p, "toolCallId")) {
                    step.status = status;
                }
                self.tool = None;
            }
            "input.folded" => {
                let input = text(&p, "inputId");
                match self.queued.iter().position(|entry| entry.input_id == input) {
                    Some(at) => {
                        let entry = self.queued.remove(at);
                        if let Some(item) = self.local_item(&entry.client_message_id) {
                            item.local = None;
                            item.message_id = Some(format!("input:{input}"));
                        }
                    }
                    None => effects.push(Effect::Refresh),
                }
            }
            "approval.requested" => {
                let card = approval_of(&p, run_id);
                self.approvals.retain(|other| other.approval_id != card.approval_id);
                self.approvals.push(card);
            }
            "approval.decided" => {
                let id = text(&p, "approvalId");
                let status = Some(text(&p, "status"))
                    .filter(|s| !s.is_empty())
                    .unwrap_or(text(&p, "decision"));
                let summary = self
                    .approvals
                    .iter()
                    .find(|card| card.approval_id == id)
                    .map(|card| card.summary.clone())
                    .unwrap_or_default();
                self.approvals.retain(|card| card.approval_id != id);
                let word = match status.as_str() {
                    "approved" | "executing" | "executed" => "Approved",
                    "rejected" => "Rejected",
                    "handed-off" => "Handed to you",
                    "expired" => "Expired",
                    _ => "Decided",
                };
                self.note(if summary.is_empty() {
                    word.to_string()
                } else {
                    format!("{word}: {summary}")
                });
            }
            "question.asked" => {
                let card = question_of(&p, run_id);
                self.questions.retain(|other| other.question_id != card.question_id);
                self.questions.push(card);
            }
            "question.answered" => {
                let id = text(&p, "questionId");
                if id.is_empty() {
                    let run = run_id.to_string();
                    self.questions.retain(|card| card.run_id != run);
                } else {
                    self.questions.retain(|card| card.question_id != id);
                }
            }
            "handoff.started" => {
                let card = handoff_of(&p);
                let form = card.form.clone();
                self.handoffs.retain(|other| other.handoff_id != card.handoff_id);
                self.handoffs.push(card);
                if self.follows(run_id, seq, &p, ctx) {
                    effects.push(Effect::Follow(Follow::Handoff { form }));
                }
            }
            "handoff.done" => {
                let id = text(&p, "handoffId");
                self.handoffs.retain(|card| card.handoff_id != id);
                let outcome = text(&p, "outcome");
                self.note(if outcome == "saved" {
                    "Saved".into()
                } else {
                    format!("Hand-off {outcome}")
                });
            }
            "canvas.open" => self.canvas = Some(text(&p, "title")),
            "ui.navigate" | "ui.fill" | "ui.dialog" | "ui.focus" => {
                if self.follows(run_id, seq, &p, ctx) {
                    if let Some(follow) = follow_of(&kind, &p) {
                        effects.push(Effect::Follow(follow));
                    }
                } else if seq.is_some_and(|seq| seq > self.floor.get(run_id).copied().unwrap_or(0)) {
                    match kind.as_str() {
                        "ui.navigate" => {
                            let title = Some(text(&p, "title"))
                                .filter(|t| !t.is_empty())
                                .unwrap_or(text(&p, "path"));
                            self.note(format!("Assist opened {title} (not followed here)"));
                        }
                        "ui.fill" => self.note(format!("Assist filled {} (not followed here)", text(&p, "label"))),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        effects
    }

    /// Whether a ui event is this client's to apply: its owner, follow on, the same page version, a stored event past
    /// the floor (never one a reload replays).
    fn follows(&self, run_id: &str, seq: Option<u64>, payload: &Value, ctx: Ctx) -> bool {
        ctx.follow_on
            && !ctx.client_id.is_empty()
            && text(payload, "clientId") == ctx.client_id
            && number(payload, "contextVersion") == self.context_version
            && seq.is_some_and(|seq| seq > self.floor.get(run_id).copied().unwrap_or(0))
    }

    fn end(&mut self, run_id: &str) {
        if self.active_run.as_deref() == Some(run_id) {
            self.active_run = None;
        }
        self.tool = None;
    }

    /// The active run and whether it waits for the user.
    pub fn active(&self) -> Option<&Run> {
        self.active_run.as_ref().and_then(|id| self.runs.get(id))
    }

    /// The one status line: what the active run does or waits for.
    pub fn status(&self) -> String {
        let Some(run) = self.active() else {
            return String::new();
        };
        if is_paused(&run.state) {
            return match run.pause_kind.as_str() {
                "approval" | "AgpsApproval" => "Waiting for your approval",
                "handoff" | "AgpsHandoff" => "Waiting for your save",
                _ if run.state == "TASK_STATE_AUTH_REQUIRED" => "Waiting for your approval",
                _ => "Waiting for your answer",
            }
            .to_string();
        }
        match &self.tool {
            Some(tool) if !tool.is_empty() => format!("Working: {tool}"),
            _ => "Working".to_string(),
        }
    }

    /// The lines of the thread: messages, step rows, the done line of each ended run, and the canvas line.
    pub fn rows(&self) -> Vec<Row> {
        let row = |kind, text: String| Row { kind, text };
        let mut rows = Vec::new();
        let mut last_run: Option<&str> = None;
        let done_line = |rows: &mut Vec<Row>, run_id: Option<&str>| {
            if let Some(run) = run_id.and_then(|id| self.runs.get(id)).filter(|run| run.done) {
                rows.push(done_row(run));
            }
        };
        for item in &self.items {
            if item.run_id.as_deref() != last_run && item.role != Role::Note {
                if item.run_id.is_some() || item.role == Role::User {
                    done_line(&mut rows, last_run);
                }
                last_run = item.run_id.as_deref();
            }
            for part in &item.parts {
                match (part, &item.role) {
                    (Part::Text(text), Role::User) => {
                        let mark = match &item.local {
                            Some(local) => format!("  ({local})"),
                            None => String::new(),
                        };
                        rows.push(row(RowKind::User, format!("> {text}{mark}")));
                    }
                    (Part::Text(text), Role::Assistant) if !text.is_empty() => {
                        rows.push(row(RowKind::Assistant, text.clone()));
                    }
                    (Part::Text(text), Role::Note) => rows.push(row(RowKind::Note, text.clone())),
                    (Part::Step(step), _) => {
                        let failed = matches!(step.status.as_str(), "failed" | "denied" | "AgtcFailed");
                        let kind = if failed { RowKind::StepFailed } else { RowKind::Step };
                        rows.push(row(kind, step_line(step)));
                    }
                    _ => {}
                }
            }
        }
        done_line(&mut rows, last_run);
        for entry in &self.queued {
            if !self
                .items
                .iter()
                .any(|item| item.client_message_id.as_deref() == Some(&entry.client_message_id))
            {
                rows.push(row(RowKind::User, format!("> {}  (queued)", entry.text)));
            }
        }
        if let Some(title) = &self.canvas {
            let name = if title.is_empty() {
                String::new()
            } else {
                format!(" \"{title}\"")
            };
            rows.push(row(
                RowKind::Canvas,
                format!("This answer has a canvas{name} — o opens it in the browser"),
            ));
        }
        rows
    }
}

fn follow_of(kind: &str, p: &Value) -> Option<Follow> {
    Some(match kind {
        "ui.navigate" => Follow::Navigate {
            path: safe_path(&text(p, "path"))?,
            parameters: flat(p.get("parameters")),
            title: text(p, "title"),
        },
        "ui.fill" => Follow::Fill {
            tui_id: text(p, "tuiId"),
            form: Some(text(p, "domId"))
                .filter(|id| !id.is_empty())
                .unwrap_or(text(p, "form")),
            field: text(p, "field"),
            value: text(p, "value"),
            label: text(p, "label"),
        },
        "ui.focus" => Follow::Focus {
            tui_id: text(p, "tuiId"),
        },
        "ui.dialog" => Follow::Dialog {
            id: text(p, "id"),
            open: p.get("open").and_then(Value::as_bool).unwrap_or(false),
        },
        _ => return None,
    })
}

/// `· screen go /apps/…  ok`: the tool, its arguments cut short, and how it went.
fn step_line(step: &Step) -> String {
    let arguments: String = serde_json::from_str::<Value>(&step.arguments)
        .ok()
        .and_then(|value| {
            value.as_object().map(|map| {
                map.values()
                    .map(|value| match value {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        })
        .unwrap_or_else(|| step.arguments.clone());
    let status = match step.status.as_str() {
        "running" | "AgtcRunning" | "AgtcPending" => "…",
        "done" | "AgtcDone" => "ok",
        "failed" | "AgtcFailed" => "failed",
        "denied" => "denied",
        "" => "",
        other => other,
    };
    let arguments: String = arguments.chars().take(60).collect();
    format!("· {} {arguments}  {status}", step.name).replace("   ", "  ")
}

fn done_row(run: &Run) -> Row {
    let word = match run.state.as_str() {
        "TASK_STATE_COMPLETED" => "Done",
        "TASK_STATE_FAILED" => "Failed",
        "TASK_STATE_CANCELED" => "Stopped",
        "TASK_STATE_REJECTED" => "Not run",
        _ => "Ended",
    };
    let mut text = format!("✓ {word}");
    if run.steps > 0 {
        text.push_str(&format!(
            " · {} step{}",
            run.steps,
            if run.steps == 1 { "" } else { "s" }
        ));
    }
    if run.tokens > 0 {
        text.push_str(&format!(" · {} tokens", run.tokens));
    }
    if run.seconds > 0 {
        text.push_str(&format!(" · {} s", run.seconds));
    }
    let failed = run.state != "TASK_STATE_COMPLETED";
    if failed && !run.error.is_empty() {
        text.push_str(&format!(" · {}", run.error));
    }
    Row {
        kind: if failed { RowKind::Error } else { RowKind::Done },
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ME: Ctx = Ctx {
        client_id: "tui-1",
        follow_on: true,
    };

    fn event(seq: u64, kind: &str, payload: Value) -> Value {
        json!({"seq": seq, "type": kind, "payload": payload})
    }

    fn started() -> Thread {
        let mut thread = Thread::default();
        thread.outgoing("m-1", "Close the task");
        thread.turn_answered(
            "m-1",
            &json!({"conversationId": "c1", "runId": "r1", "state": "TASK_STATE_SUBMITTED", "queued": false,
                    "follow": {"clientId": "tui-1", "contextVersion": 2}}),
            "tui-1",
        );
        thread
    }

    #[test]
    fn text_chunks_append_only_their_new_suffix_and_a_gap_asks_for_a_catch_up() {
        let mut thread = started();
        assert_eq!(thread.active_run.as_deref(), Some("r1"));
        assert!(thread.owner.following);
        let chunk = |seq, offset, text: &str| {
            event(
                seq,
                "text.chunk",
                json!({"messageId": "a1", "part": 0, "offset": offset, "text": text}),
            )
        };
        assert!(thread.apply("r1", &chunk(1, 0, "Hel"), ME).is_empty());
        thread.apply("r1", &chunk(2, 0, "Hello"), ME);
        thread.apply("r1", &chunk(2, 0, "ignored, seen"), ME);
        assert_eq!(
            thread.apply("r1", &chunk(5, 5, "!"), ME),
            vec![Effect::Gap {
                run_id: "r1".into(),
                after: 2
            }]
        );
        thread.apply("r1", &chunk(3, 5, " there"), ME);
        let rows = thread.rows();
        assert_eq!(rows[0].text, "> Close the task");
        assert_eq!(rows[1].text, "Hello there");
        assert_eq!(thread.apply("r1", &chunk(4, 40, "lost"), ME), vec![Effect::Refresh]);
    }

    #[test]
    fn steps_cards_and_the_done_line() {
        let mut thread = started();
        thread.apply(
            "r1",
            &event(
                1,
                "tool.call",
                json!({"toolCallId": "t1", "messageId": "a1", "name": "screen",
                                         "arguments": "{\"go\": \"/apps/tasks/EditTask\"}"}),
            ),
            ME,
        );
        assert_eq!(thread.status(), "Working: screen");
        thread.apply(
            "r1",
            &event(2, "tool.result", json!({"toolCallId": "t1", "status": "done"})),
            ME,
        );
        thread.apply(
            "r1",
            &event(
                3,
                "approval.requested",
                json!({"approvalId": "ap1", "summary": "Update task Ship it",
                "classification": "write", "canEdit": true,
                "changes": [{"field": "statusId", "label": "Status", "current": "Open", "proposed": "Closed"}]}),
            ),
            ME,
        );
        let paused = event(
            4,
            "run.state",
            json!({"state": "TASK_STATE_INPUT_REQUIRED", "pauseKind": "approval"}),
        );
        assert_eq!(thread.apply("r1", &paused, ME), vec![Effect::Paused]);
        assert_eq!(thread.status(), "Waiting for your approval");
        assert_eq!(thread.approvals[0].changes[0].proposed, "Closed");
        thread.apply(
            "r1",
            &event(
                5,
                "approval.decided",
                json!({"approvalId": "ap1", "decision": "approve", "status": "executed"}),
            ),
            ME,
        );
        assert!(thread.approvals.is_empty());
        let done = event(6, "run.done", json!({"state": "TASK_STATE_COMPLETED"}));
        assert_eq!(thread.apply("r1", &done, ME), vec![Effect::Ended]);
        assert!(thread.active_run.is_none());
        let rows: Vec<String> = thread.rows().into_iter().map(|row| row.text).collect();
        assert_eq!(
            rows,
            [
                "> Close the task",
                "· screen /apps/tasks/EditTask  ok",
                "Approved: Update task Ship it",
                "✓ Done · 1 step"
            ]
        );
    }

    #[test]
    fn only_this_clients_events_at_its_page_version_move_the_screen() {
        let mut thread = started();
        let fill = |seq, client: &str, version| {
            event(
                seq,
                "ui.fill",
                json!({"form": "EditTask", "field": "statusId", "value": "Closed", "label": "Status",
                "tuiId": "EditTask.statusId", "clientId": client, "contextVersion": version}),
            )
        };
        let effects = thread.apply("r1", &fill(1, "tui-1", 2), ME);
        assert!(
            matches!(&effects[..], [Effect::Follow(Follow::Fill { tui_id, value, .. })]
            if tui_id == "EditTask.statusId" && value == "Closed")
        );
        // another client's, an older page version, or with follow off: a note, not a fill
        assert!(thread.apply("r1", &fill(2, "tab-a", 2), ME).is_empty());
        assert!(thread.apply("r1", &fill(3, "tui-1", 1), ME).is_empty());
        let off = Ctx { follow_on: false, ..ME };
        assert!(thread.apply("r1", &fill(4, "tui-1", 2), off).is_empty());
        assert_eq!(
            thread.rows().last().unwrap().text,
            "Assist filled Status (not followed here)"
        );
        // a path that is not a plain /apps path never navigates
        let nav = event(
            5,
            "ui.navigate",
            json!({"path": "https://evil.example/apps", "clientId": "tui-1", "contextVersion": 2}),
        );
        assert!(thread.apply("r1", &nav, ME).is_empty());
        assert_eq!(
            safe_path("/qapps/tasks/EditTask").as_deref(),
            Some("/apps/tasks/EditTask")
        );
        assert_eq!(safe_path("/apps/../x"), None);
    }

    #[test]
    fn a_snapshot_never_replays_its_page_events_and_keeps_an_unanswered_message() {
        let mut thread = Thread::default();
        thread.outgoing("m-9", "Also this");
        let json = json!({
            "conversation": {"conversationId": "c1", "title": "Close the task"},
            "activeRun": {"runId": "r1", "state": "TASK_STATE_WORKING", "lastEventSeq": 7},
            "items": [
                {"messageId": "u1", "role": "user", "runId": "r1", "clientMessageId": "m-1",
                 "parts": [{"part": 1, "kind": "text", "text": "Close the task"}]},
                {"messageId": "a1", "role": "assistant", "runId": "r1", "parts": [
                    {"part": 1, "kind": "tool-call", "toolCallId": "t1", "name": "screen", "arguments": "{}", "status": "AgtcDone"}]},
                {"messageId": "x1", "role": "tool", "runId": "r1", "parts": [
                    {"part": 1, "kind": "tool-result", "toolCallId": "t1", "isError": true, "text": "no"}]}],
            "pending": {"approvals": [], "questions": [{"questionId": "q1", "text": "Which one?",
                        "options": [{"label": "A", "recommended": true}, {"label": "B"}]}], "handoffs": [], "queued": []},
            "canvas": {"artifactId": "9", "title": "Hits", "closed": false},
            "follow": {"clientId": "tui-1", "contextVersion": 3, "following": true}
        });
        thread.snapshot(&json, "tui-1");
        assert_eq!(thread.context_version, 3);
        assert_eq!(thread.questions[0].options[0], ("A".to_string(), true));
        let old = event(
            7,
            "ui.navigate",
            json!({"path": "/apps/x", "clientId": "tui-1", "contextVersion": 3}),
        );
        assert!(thread.apply("r1", &old, ME).is_empty());
        let rows: Vec<(RowKind, String)> = thread.rows().into_iter().map(|row| (row.kind, row.text)).collect();
        assert_eq!(rows[1], (RowKind::StepFailed, "· screen   failed".replace("   ", "  ")));
        assert_eq!(rows[2].1, "> Also this  (sending)");
        assert!(rows[3].1.starts_with("This answer has a canvas \"Hits\""));
    }
}
