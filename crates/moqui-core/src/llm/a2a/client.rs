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

//! The A2A 1.0 client (plans/LLM-A2A-Assist.md 4.15 "Client"; docs/A2A.md "Delegating to remote agents"): what the
//! delegate tools ([`crate::llm::tools::delegate`]) call a remote agent with.
//!
//! A remote is an `AgentRemoteA2a` row: its card URL (an admin's, checked as [`crate::llm::remote`] checks every
//! address), its credential and its timeout. The card is read from that URL and kept on the row for an hour; an
//! interface the card names on another origin than the card's is never called (card URL hardening), and of the ones
//! left JSON-RPC is preferred over HTTP+JSON, version 1.0 only. A message is sent as `SendStreamingMessage` when the
//! card says it streams, else as a blocking `SendMessage` followed by `GetTask` while the task works, until the task
//! ends or is interrupted (`TASK_STATE_INPUT_REQUIRED`, `TASK_STATE_AUTH_REQUIRED`), within the delegation's time;
//! past it, or when the local run is canceled, the remote task is canceled. A follow-up (the user's answer to the
//! remote's question) goes to the same task and context.

use std::time::{Duration, Instant};

use moqui_lib::http::Method;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::types;
use crate::entity::{fence, EntityFacade, EntityValue};
use crate::llm::remote::{self, Address, Auth, Call, Opened};
use crate::llm::{millis_of, text_of};

/// The largest card read.
const CARD_CAP: u64 = 256 * 1024;
/// The largest answer of one request, and of a whole stream.
const ANSWER_CAP: u64 = 4 * 1024 * 1024;
/// How long a card read stays good.
const CARD_TTL_MS: i64 = 3_600_000;
/// The key a kept card names the URL it was read from by.
const CARD_URL_KEY: &str = "_cardUrl";
/// How often a working task is read again after a blocking send.
const POLL: Duration = Duration::from_millis(500);

/// A remote agent as its row has it.
#[derive(Clone, Debug)]
pub struct Remote {
    pub remote_id: String,
    pub name: String,
    pub description: Option<String>,
    pub address: Address,
    pub auth: Auth,
    pub timeout: Duration,
    pub allowed_agents: Option<String>,
    pub disabled: bool,
    pub card: Option<Json>,
    pub card_read: Option<i64>,
}

impl Remote {
    /// The remote of a row; `timeout` when the row sets none.
    ///
    /// # Errors
    ///
    /// Why it cannot be called: its URL, its credential.
    pub fn of(row: &EntityValue, timeout: Duration) -> Result<Self, String> {
        let remote_id = text_of(row, "remoteId").unwrap_or_default();
        let url = text_of(row, "cardUrl").ok_or("it has no cardUrl")?;
        let address = Address::of(&url, text_of(row, "allowPrivateNetwork").as_deref() == Some("Y"))?;
        let auth = Auth::parse(text_of(row, "authJson").as_deref())?;
        Ok(Self {
            name: text_of(row, "remoteName").unwrap_or_else(|| remote_id.clone()),
            remote_id,
            description: text_of(row, "description"),
            address,
            auth,
            timeout: crate::llm::int_of(row, "timeoutSeconds")
                .filter(|seconds| *seconds > 0)
                .map_or(timeout, |seconds| Duration::from_secs(seconds.unsigned_abs())),
            allowed_agents: text_of(row, "allowedAgents"),
            disabled: text_of(row, "disabled").as_deref() == Some("Y"),
            // a card kept is the card of the URL it was read from (`_cardUrl`): another URL's is no card
            card: text_of(row, "cardJson")
                .and_then(|text| serde_json::from_str::<Json>(&text).ok())
                .filter(|card| card.get(CARD_URL_KEY).and_then(Json::as_str) == Some(url.trim())),
            card_read: row.get("cardReadDate").and_then(millis_of),
        })
    }

    fn what(&self) -> String {
        format!("the remote agent {}", self.name)
    }

    /// Text of the remote's own (an error it gave) as the model reads it: wrapped as untrusted.
    pub fn untrusted(&self, text: &str) -> String {
        crate::llm::prompt::untrusted(&format!("a2a:{}", self.remote_id), text)
    }
}

/// A binding a card offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    JsonRpc,
    HttpJson,
}

impl Binding {
    pub fn as_str(self) -> &'static str {
        match self {
            Binding::JsonRpc => "JSONRPC",
            Binding::HttpJson => "HTTP+JSON",
        }
    }
}

/// What the client needs of a card.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub name: String,
    pub description: String,
    /// The interface called, on the card's origin.
    pub url: String,
    pub binding: Binding,
    pub streaming: bool,
    /// Its skills' names and descriptions (at most 20).
    pub skills: Vec<(String, String)>,
}

/// The card of `raw`, read from `address`: its 1.0 interfaces on the card's own origin only (JSON-RPC first).
///
/// # Errors
///
/// It is not a card, or it names no 1.0 interface on that origin.
pub fn parse_card(raw: &Json, address: &Address) -> Result<Card, String> {
    let Some(card) = raw.as_object() else {
        return Err("its card is not a JSON object".to_string());
    };
    let interfaces = card
        .get("supportedInterfaces")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    let mut found: Option<(Binding, String)> = None;
    for interface in &interfaces {
        let url = interface.get("url").and_then(Json::as_str).unwrap_or_default();
        let version = interface.get("protocolVersion").and_then(Json::as_str).unwrap_or(types::VERSION);
        let binding = match interface.get("protocolBinding").and_then(Json::as_str) {
            Some("JSONRPC") => Binding::JsonRpc,
            Some("HTTP+JSON") => Binding::HttpJson,
            _ => continue,
        };
        if version != types::VERSION {
            continue;
        }
        if !address.same_origin(url) {
            tracing::warn!(
                "the card at {} names an interface on another origin ({}), which is not called",
                moqui_lib::http::redacted(&address.url),
                moqui_lib::http::redacted(url)
            );
            continue;
        }
        if found.as_ref().is_none_or(|(known, _)| *known == Binding::HttpJson && binding == Binding::JsonRpc) {
            found = Some((binding, url.trim_end_matches('/').to_string()));
        }
    }
    let Some((binding, url)) = found else {
        return Err(format!(
            "its card names no A2A {} interface on {}",
            types::VERSION,
            address.origin
        ));
    };
    let text = |key: &str| card.get(key).and_then(Json::as_str).unwrap_or_default().trim().to_string();
    Ok(Card {
        name: text("name"),
        description: text("description"),
        url,
        binding,
        streaming: card
            .get("capabilities")
            .and_then(|caps| caps.get("streaming"))
            .and_then(Json::as_bool)
            .unwrap_or(false),
        skills: card
            .get("skills")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .take(20)
            .map(|skill| {
                let text = |key: &str| skill.get(key).and_then(Json::as_str).unwrap_or_default().to_string();
                (text("name"), text("description"))
            })
            .collect(),
    })
}

fn headers(remote: &Remote) -> Vec<(String, String)> {
    let mut headers = vec![
        ("A2A-Version".to_string(), types::VERSION.to_string()),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    headers.extend(remote.auth.headers());
    headers
}

/// Read the remote's card now.
///
/// # Errors
///
/// The call may not go out or failed, or the card does not read.
pub fn fetch_card(remote: &Remote) -> Result<(Card, Json), String> {
    let what = remote.what();
    let answer = remote::exchange(
        &remote.address,
        Call {
            method: Method::Get,
            url: &remote.address.url,
            headers: headers(remote),
            body: None,
            timeout: remote.timeout.min(Duration::from_secs(20)),
            cap: CARD_CAP,
            what: &what,
        },
    )?;
    if answer.status != 200 {
        return Err(format!("{what}'s card answered {}", answer.status));
    }
    let raw: Json = serde_json::from_str(&answer.body).map_err(|_| format!("{what}'s card is not JSON"))?;
    Ok((parse_card(&raw, &remote.address)?, raw))
}

/// Keep a card read on the remote's row, with the URL it was read from (configuration the client writes, with the fence
/// lifted; a failure is logged).
pub fn keep_card(facade: &EntityFacade, remote_id: &str, card_url: &str, raw: &Json) {
    let mut raw = raw.clone();
    if let Some(card) = raw.as_object_mut() {
        card.insert(CARD_URL_KEY.to_string(), json!(card_url.trim()));
    }
    let _lifted = fence::lift();
    let kept = crate::llm::run::quietly(|| {
        crate::llm::run::in_tx(facade, || {
            facade
                .find(remote::REMOTE_A2A)?
                .eq("remoteId", Value::string(remote_id))
                .disable_authz()
                .update_all([
                    ("cardJson".to_string(), Value::string(raw.to_string())),
                    (
                        "cardReadDate".to_string(),
                        crate::llm::run::stamp(crate::llm::run::now_millis()),
                    ),
                ])
        })
    });
    if let Err(error) = kept {
        tracing::error!("the card of remote agent {remote_id} could not be kept: {error}");
    }
}

/// The remote's card: the one kept when it is fresh, else read now and kept; a card kept from before stands in when
/// the read fails (logged). Either way its interfaces are checked as the card is read.
///
/// # Errors
///
/// No card can be had.
pub fn card(facade: &EntityFacade, remote: &Remote) -> Result<Card, String> {
    let fresh = remote
        .card_read
        .is_some_and(|read| crate::llm::run::now_millis() - read < CARD_TTL_MS);
    if let (true, Some(raw)) = (fresh, &remote.card) {
        // a kept card that no longer reads is read again
        match parse_card(raw, &remote.address) {
            Ok(card) => return Ok(card),
            Err(why) => tracing::warn!("the kept card of remote agent {} does not read, so it is read again: {why}", remote.remote_id),
        }
    }
    match fetch_card(remote) {
        Ok((card, raw)) => {
            keep_card(facade, &remote.remote_id, &remote.address.url, &raw);
            Ok(card)
        }
        Err(why) => match &remote.card {
            Some(raw) => {
                tracing::warn!("the card of remote agent {} could not be read, so the one kept is used: {why}", remote.remote_id);
                parse_card(raw, &remote.address)
            }
            None => Err(why),
        },
    }
}

/// A remote task as the client has seen it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskView {
    pub id: String,
    pub context_id: String,
    /// The wire state (`TASK_STATE_COMPLETED` ...); empty when the remote answered with a message.
    pub state: String,
    pub status_text: String,
    pub status_data: Vec<Json>,
    /// Each artifact's id and its parts, in order.
    pub artifacts: Vec<(String, Vec<Json>)>,
    /// The remote answered with a message and no task.
    pub message_text: Option<String>,
}

const TERMINAL: [&str; 4] = [
    "TASK_STATE_COMPLETED",
    "TASK_STATE_FAILED",
    "TASK_STATE_CANCELED",
    "TASK_STATE_REJECTED",
];

fn parts_text(parts: &[Json]) -> (String, Vec<Json>) {
    let mut text = Vec::new();
    let mut data = Vec::new();
    for part in parts {
        if let Some(piece) = part.get("text").and_then(Json::as_str) {
            text.push(piece.to_string());
        } else if let Some(value) = part.get("data") {
            data.push(value.clone());
        } else if let Some(url) = part.get("url").and_then(Json::as_str) {
            text.push(format!("[file {url}]"));
        } else if part.get("raw").is_some() {
            let name = part.get("filename").and_then(Json::as_str).unwrap_or("file");
            text.push(format!("[{name}, not shown]"));
        }
    }
    (text.join(""), data)
}

impl TaskView {
    /// Apply a `SendMessageResponse` or a `StreamResponse`: a task, a message, a status update or an artifact update.
    pub fn apply(&mut self, response: &Json) {
        if let Some(task) = response.get("task") {
            let text = |key: &str| task.get(key).and_then(Json::as_str).unwrap_or_default().to_string();
            self.id = text("id");
            self.context_id = text("contextId");
            if let Some(status) = task.get("status") {
                self.status(status);
            }
            self.artifacts.clear();
            for artifact in task.get("artifacts").and_then(Json::as_array).into_iter().flatten() {
                self.artifact(artifact, false);
            }
        } else if let Some(message) = response.get("message") {
            let parts = message.get("parts").and_then(Json::as_array).cloned().unwrap_or_default();
            let (text, data) = parts_text(&parts);
            let mut all = text;
            for value in data {
                all.push_str(&format!("\n{value}"));
            }
            self.message_text = Some(all);
            if let Some(context) = message.get("contextId").and_then(Json::as_str) {
                self.context_id = context.to_string();
            }
        } else if let Some(update) = response.get("statusUpdate") {
            if let Some(task_id) = update.get("taskId").and_then(Json::as_str) {
                self.id = task_id.to_string();
            }
            if let Some(context) = update.get("contextId").and_then(Json::as_str) {
                self.context_id = context.to_string();
            }
            if let Some(status) = update.get("status") {
                self.status(status);
            }
        } else if let Some(update) = response.get("artifactUpdate") {
            if let Some(artifact) = update.get("artifact") {
                let append = update.get("append").and_then(Json::as_bool).unwrap_or(false);
                self.artifact(artifact, append);
            }
        }
    }

    fn status(&mut self, status: &Json) {
        self.state = status.get("state").and_then(Json::as_str).unwrap_or_default().to_string();
        let parts = status
            .get("message")
            .and_then(|message| message.get("parts"))
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default();
        let (text, data) = parts_text(&parts);
        self.status_text = text;
        self.status_data = data;
    }

    fn artifact(&mut self, artifact: &Json, append: bool) {
        let id = artifact.get("artifactId").and_then(Json::as_str).unwrap_or_default().to_string();
        let parts = artifact.get("parts").and_then(Json::as_array).cloned().unwrap_or_default();
        match self.artifacts.iter_mut().find(|(known, _)| *known == id) {
            Some((_, known)) if append => known.extend(parts),
            Some((_, known)) => *known = parts,
            None => self.artifacts.push((id, parts)),
        }
    }

    /// The task ended (or the remote answered with a message).
    pub fn ended(&self) -> bool {
        self.message_text.is_some() || TERMINAL.contains(&self.state.as_str())
    }

    /// The task waits for its caller: a question (`TASK_STATE_INPUT_REQUIRED`) or a sign-in or approval at a link
    /// (`TASK_STATE_AUTH_REQUIRED`).
    pub fn interrupted(&self) -> bool {
        matches!(self.state.as_str(), "TASK_STATE_INPUT_REQUIRED" | "TASK_STATE_AUTH_REQUIRED")
    }

    /// What the remote answered: its artifacts' text and data, else its status message, else its message.
    pub fn answer(&self) -> String {
        let mut out = Vec::new();
        for (_, parts) in &self.artifacts {
            let (text, data) = parts_text(parts);
            if !text.is_empty() {
                out.push(text);
            }
            out.extend(data.iter().map(Json::to_string));
        }
        if out.is_empty() && !self.status_text.is_empty() {
            out.push(self.status_text.clone());
        }
        if out.is_empty() {
            if let Some(text) = &self.message_text {
                out.push(text.clone());
            }
        }
        out.join("\n")
    }

    /// The link of an interrupted task: a `url` in its status data (the `moqui.approval` part of a Moqui remote), else
    /// the first http(s) URL in its status text.
    pub fn link(&self, address: &Address) -> Option<String> {
        let from_data = self
            .status_data
            .iter()
            .find_map(|data| data.get("url").and_then(Json::as_str).map(str::to_string));
        let link = from_data.or_else(|| {
            self.status_text
                .split_whitespace()
                .find(|word| word.starts_with("https://") || word.starts_with("http://"))
                .map(|word| word.trim_end_matches(['.', ',', ')', '>', '"', '\'']).to_string())
        })?;
        // https, or http only to an internal host of a row that allows a private network
        remote::link_allowed(&link, address).then_some(link)
    }
}

/// What a delegation sends: its text, and the remote context and task it goes on in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outgoing {
    pub text: String,
    pub context_id: Option<String>,
    pub task_id: Option<String>,
}

fn message_id() -> String {
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        return format!("m{}", crate::llm::run::now_millis());
    }
    moqui_lib::codec::hex_encode(&bytes)
}

fn message_json(out: &Outgoing) -> Json {
    let mut message = json!({"messageId": message_id(), "role": "ROLE_USER", "parts": [types::text_part(&out.text)]});
    if let Some(context) = &out.context_id {
        message["contextId"] = json!(context);
    }
    if let Some(task) = &out.task_id {
        message["taskId"] = json!(task);
    }
    message
}

/// The JSON-RPC result or the HTTP+JSON body of a call, or the remote's error.
fn read_answer(remote: &Remote, answer: &remote::Answer, method: &str, binding: Binding) -> Result<Json, String> {
    let what = remote.what();
    let body: Json = serde_json::from_str(&answer.body)
        .map_err(|_| format!("{what} answered {method} with {} and no JSON", answer.status))?;
    let error = |error: &Json| {
        let message = error.get("message").and_then(Json::as_str).unwrap_or("an error");
        format!(
            "{what} refused {method}: {}",
            remote.untrusted(&crate::llm::prompt::one_line(message, 300))
        )
    };
    match binding {
        Binding::JsonRpc => {
            if let Some(failed) = body.get("error") {
                return Err(error(failed));
            }
            if !(200..300).contains(&answer.status) {
                return Err(format!("{what} answered {method} with {}", answer.status));
            }
            body.get("result").cloned().ok_or_else(|| format!("{what} answered {method} with no result"))
        }
        Binding::HttpJson => {
            if !(200..300).contains(&answer.status) {
                return Err(body.get("error").map_or_else(
                    || format!("{what} answered {method} with {}", answer.status),
                    error,
                ));
            }
            Ok(body)
        }
    }
}

/// One unary call of `method` (SendMessage, GetTask, CancelTask) with `params`.
fn call(remote: &Remote, card: &Card, method: &str, params: &Json, timeout: Duration) -> Result<Json, String> {
    let what = remote.what();
    let task_id = params.get("id").and_then(Json::as_str).unwrap_or_default();
    let (http_method, url, body) = match card.binding {
        Binding::JsonRpc => (
            Method::Post,
            card.url.clone(),
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string()),
        ),
        Binding::HttpJson => match method {
            "SendMessage" => (Method::Post, format!("{}/message:send", card.url), Some(params.to_string())),
            "GetTask" => (Method::Get, format!("{}/tasks/{}", card.url, encode(task_id)), None),
            "CancelTask" => (Method::Post, format!("{}/tasks/{}:cancel", card.url, encode(task_id)), Some("{}".to_string())),
            _ => return Err(format!("{method} is not a call of this client")),
        },
    };
    let mut headers = headers(remote);
    if body.is_some() {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    let answer = remote::exchange(
        &remote.address,
        Call {
            method: http_method,
            url: &url,
            headers,
            body,
            timeout: timeout.max(Duration::from_secs(1)),
            cap: ANSWER_CAP,
            what: &what,
        },
    )?;
    read_answer(remote, &answer, method, card.binding)
}

/// A task id in a path: the ids a server makes are plain, anything else is encoded.
fn encode(id: &str) -> String {
    id.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The remote task now.
///
/// # Errors
///
/// The call failed or was refused.
pub fn get_task(remote: &Remote, card: &Card, task_id: &str, timeout: Duration) -> Result<TaskView, String> {
    let result = call(remote, card, "GetTask", &json!({"id": task_id, "historyLength": 0}), timeout)?;
    let mut view = TaskView::default();
    view.apply(&json!({"task": result}));
    Ok(view)
}

/// Ask the remote to cancel its task (best effort: a failure is logged).
pub fn cancel_task(remote: &Remote, card: &Card, task_id: &str) {
    if let Err(why) = call(remote, card, "CancelTask", &json!({"id": task_id}), Duration::from_secs(15)) {
        tracing::warn!("remote task {task_id} of {} could not be canceled: {why}", remote.remote_id);
    }
}

/// Why a delegation stopped before its task ended or was interrupted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The call failed (the remote, the transport, a limit): what the model reads.
    Failed(String),
    /// The local run was canceled; the remote task was asked to cancel too.
    Canceled,
    /// The delegation's time ran out; the remote task was asked to cancel.
    TimedOut,
}

/// Send `out` and follow the task until it ends or is interrupted, by the deadline; `canceled` is asked between reads.
/// Past the deadline, or once canceled, the remote task is canceled.
///
/// # Errors
///
/// Why it stopped.
pub fn send(
    remote: &Remote,
    card: &Card,
    out: &Outgoing,
    deadline: Instant,
    canceled: &dyn Fn() -> bool,
) -> Result<TaskView, Stopped> {
    let params = json!({"message": message_json(out),
        "configuration": {"acceptedOutputModes": types::OUTPUT_MODES, "historyLength": 0}});
    let mut view = TaskView::default();
    if card.streaming {
        stream(remote, card, &params, deadline, canceled, &mut view)?;
    } else {
        let left = deadline.saturating_duration_since(Instant::now());
        let result = call(remote, card, "SendMessage", &params, left).map_err(Stopped::Failed)?;
        view.apply(&result);
    }
    // a stream that ended early, or a blocking send answered before the task did: read it until it settles
    follow(remote, card, view, deadline, canceled)
}

/// Read a task again until it ends or is interrupted, by the deadline; `canceled` is asked between reads. Past the
/// deadline, or once canceled, the remote task is canceled.
///
/// # Errors
///
/// Why it stopped.
pub fn follow(
    remote: &Remote,
    card: &Card,
    mut view: TaskView,
    deadline: Instant,
    canceled: &dyn Fn() -> bool,
) -> Result<TaskView, Stopped> {
    while !view.ended() && !view.interrupted() {
        if view.id.is_empty() {
            return Err(Stopped::Failed(format!("{} answered with no task", remote.what())));
        }
        if canceled() {
            cancel_task(remote, card, &view.id);
            return Err(Stopped::Canceled);
        }
        if Instant::now() >= deadline {
            cancel_task(remote, card, &view.id);
            return Err(Stopped::TimedOut);
        }
        std::thread::sleep(POLL);
        let left = deadline.saturating_duration_since(Instant::now());
        let id = view.id.clone();
        view = get_task(remote, card, &id, left.clamp(Duration::from_secs(1), Duration::from_secs(30)))
            .map_err(Stopped::Failed)?;
    }
    Ok(view)
}

/// `SendStreamingMessage`: the events applied to `view` until the task ends or is interrupted, or the stream ends.
fn stream(
    remote: &Remote,
    card: &Card,
    params: &Json,
    deadline: Instant,
    canceled: &dyn Fn() -> bool,
    view: &mut TaskView,
) -> Result<(), Stopped> {
    let what = remote.what();
    let (url, body) = match card.binding {
        Binding::JsonRpc => (
            card.url.clone(),
            json!({"jsonrpc": "2.0", "id": 1, "method": "SendStreamingMessage", "params": params}).to_string(),
        ),
        Binding::HttpJson => (format!("{}/message:stream", card.url), params.to_string()),
    };
    let mut headers = headers(remote);
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("accept"));
    headers.push(("Accept".to_string(), "text/event-stream".to_string()));
    let opened = remote::open(
        &remote.address,
        Call {
            method: Method::Post,
            url: &url,
            headers,
            body: Some(body),
            timeout: deadline.saturating_duration_since(Instant::now()).max(Duration::from_secs(1)),
            cap: ANSWER_CAP,
            what: &what,
        },
    )
    .map_err(Stopped::Failed)?;
    let mut events = match opened {
        Opened::Events(events) => events,
        Opened::Whole(answer) => {
            // an error, or a server that answers a stream request whole
            let result = read_answer(remote, &answer, "SendStreamingMessage", card.binding).map_err(Stopped::Failed)?;
            view.apply(&result);
            return Ok(());
        }
    };
    loop {
        let data = match events.next_data() {
            Ok(Some(data)) => data,
            Ok(None) => return Ok(()),
            Err(why) if view.id.is_empty() => return Err(Stopped::Failed(why)),
            // the task is known: it is read again as it stands
            Err(why) => {
                tracing::warn!("the stream of remote task {} ended early: {why}", view.id);
                return Ok(());
            }
        };
        let Ok(frame) = serde_json::from_str::<Json>(&data) else {
            continue;
        };
        let response = match card.binding {
            Binding::JsonRpc => {
                if let Some(error) = frame.get("error") {
                    let message = error.get("message").and_then(Json::as_str).unwrap_or("an error");
                    return Err(Stopped::Failed(format!(
                        "{what} refused the message: {}",
                        remote.untrusted(&crate::llm::prompt::one_line(message, 300))
                    )));
                }
                frame.get("result").cloned().unwrap_or(Json::Null)
            }
            Binding::HttpJson => frame,
        };
        view.apply(&response);
        if view.ended() || view.interrupted() {
            return Ok(());
        }
        if canceled() {
            if !view.id.is_empty() {
                cancel_task(remote, card, &view.id);
            }
            return Err(Stopped::Canceled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_is_called_only_on_its_own_origin_json_rpc_first() {
        let address = Address::of("https://agent.example.com/.well-known/agent-card.json", false).unwrap();
        let card = json!({"name": "Helper", "description": "Helps", "capabilities": {"streaming": true},
            "supportedInterfaces": [
                {"url": "https://evil.example.com/a2a", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"},
                {"url": "https://agent.example.com/a2a/", "protocolBinding": "HTTP+JSON", "protocolVersion": "1.0"},
                {"url": "https://agent.example.com/v03", "protocolBinding": "JSONRPC", "protocolVersion": "0.3"}],
            "skills": [{"id": "s", "name": "Search", "description": "Finds"}]});
        let parsed = parse_card(&card, &address).unwrap();
        assert_eq!((parsed.binding, parsed.url.as_str()), (Binding::HttpJson, "https://agent.example.com/a2a"));
        assert!(parsed.streaming);
        assert_eq!(parsed.skills, vec![("Search".to_string(), "Finds".to_string())]);
        let only_elsewhere = json!({"supportedInterfaces": [
            {"url": "https://evil.example.com/a2a", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}]});
        assert!(parse_card(&only_elsewhere, &address).unwrap_err().contains("no A2A 1.0 interface"));
    }

    #[test]
    fn a_task_view_follows_snapshots_updates_and_appended_artifacts() {
        let mut view = TaskView::default();
        view.apply(&json!({"task": {"id": "t1", "contextId": "c1", "status": {"state": "TASK_STATE_WORKING"}}}));
        assert!(!view.ended() && !view.interrupted());
        view.apply(&json!({"artifactUpdate": {"taskId": "t1", "artifact": {"artifactId": "response",
            "parts": [{"text": "Hel"}]}, "append": false}}));
        view.apply(&json!({"artifactUpdate": {"taskId": "t1", "artifact": {"artifactId": "response",
            "parts": [{"text": "lo"}]}, "append": true}}));
        view.apply(&json!({"statusUpdate": {"taskId": "t1", "contextId": "c1",
            "status": {"state": "TASK_STATE_COMPLETED"}}}));
        assert!(view.ended());
        assert_eq!(view.answer(), "Hello");
        let mut auth = TaskView::default();
        auth.apply(&json!({"task": {"id": "t2", "contextId": "c2", "status": {"state": "TASK_STATE_AUTH_REQUIRED",
            "message": {"parts": [{"text": "Approve it in Moqui"}, {"data": {"type": "moqui.approval",
                "url": "https://moqui.example.com/qapps?assist=C1"}}]}}}}));
        assert!(auth.interrupted());
        let address = Address::of("https://moqui.example.com/a2a", false).unwrap();
        assert_eq!(auth.link(&address).as_deref(), Some("https://moqui.example.com/qapps?assist=C1"));
        let mut asked = TaskView::default();
        asked.apply(&json!({"statusUpdate": {"taskId": "t3", "status": {"state": "TASK_STATE_INPUT_REQUIRED",
            "message": {"parts": [{"text": "Sign in at https://idp.example.com/x."}]}}}}));
        assert_eq!(asked.link(&address).as_deref(), Some("https://idp.example.com/x"));
        // a cleartext link to a public host is never shown
        let mut plain = TaskView::default();
        plain.apply(&json!({"statusUpdate": {"taskId": "t5", "status": {"state": "TASK_STATE_AUTH_REQUIRED",
            "message": {"parts": [{"data": {"url": "http://phish.example.com/login"}}]}}}}));
        assert_eq!(plain.link(&address), None);
        let mut message = TaskView::default();
        message.apply(&json!({"message": {"contextId": "c4", "parts": [{"text": "Just this"}]}}));
        assert!(message.ended());
        assert_eq!(message.answer(), "Just this");
    }
}
