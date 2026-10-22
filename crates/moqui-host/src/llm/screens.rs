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

//! The agents' screen sessions (plans/LLM-A2A-Assist.md 4.7): [`ScreenSessions`] over the in-process terminal client.
//! Each session is a `moqui_tui::Agent` on an [`AgentLink`], which wraps [`HostLink::weak_as_user`] the way the
//! terminal's gated link wraps one: every request renders as the agent (`RenderOrigin::Agent`), under the read fence,
//! with the transition gate holding every write, and with no push subscription. A held write is kept as the exact
//! request made, so an approval can send it again with its grant.
//!
//! The registry is shaped like `agent-apps`' (`crate::ssh::agent`): a session is found only for its owner, an idle one
//! is dropped on the next call, and its limits are its own (64 sessions, 4 per user, 30 minutes idle; a fifth session
//! of a user takes the place of the user's least recently used). Calls block the step thread and hop to the request
//! pool through the in-process transport. A session is a cache: the conversation keeps its state.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::llm::screen::{
    self, ApprovalGrant, BrowserState, FieldChange, FieldConflict, FollowEvent, OpenSpec, PendingWrite, ScreenAnswer,
    ScreenCommand, ScreenError, ScreenSessions, ScreenState,
};
use moqui_core::screen::{RenderOptions, RenderOrigin, TransitionGate};
use moqui_tui::{
    Agent, AgentAnswer, Choice, LinkError, Messages, PushSink, RawBody, ScreenLink, ScreenModel, ShellInfo,
    Subscription, TransitionOutcome,
};
use serde_json::{json, Value as Json};

use crate::tui::HostLink;

/// Sessions on this node in all.
const MAX_SESSIONS: usize = 64;
/// Sessions of one user; one more takes the place of the least recently used.
const MAX_PER_USER: usize = 4;
/// A session no command used for this long is dropped.
const IDLE: Duration = Duration::from_secs(30 * 60);

/// What a request the gate held asked for, exactly: sent again with an approval's grant.
#[derive(Clone, Debug)]
struct HeldRequest {
    url: String,
    method: String,
    fields: BTreeMap<String, String>,
    /// The 409 body: `heldWrite`, and `approvalMismatch` when a grant did not match.
    body: Json,
}

/// The link of an agent's session: the in-process link as the user, with no login and no pushes. A level the session
/// goes to and a transition it posts (a form's submit, a link it presses) go through `link`, whose gate holds a write:
/// the held request is kept and answered as a refusal the session shows. Lookups (a drop-down's options, a value, a
/// dialog's body, a file) go through `lookups`, which runs any transition under the read fence instead of holding it:
/// a lookup with actions that only read answers as for a person, and one that writes fails at the fence as a failed
/// lookup, never as a write waiting for approval.
#[derive(Clone)]
pub(crate) struct AgentLink {
    link: HostLink,
    lookups: HostLink,
    held: Arc<Mutex<Option<HeldRequest>>>,
}

impl AgentLink {
    fn new(factory: Weak<ExecutionContextFactory>, username: &str) -> Self {
        let link = HostLink::weak_as_user(factory, username);
        let lookups = link.sibling(RenderOptions::default());
        Self {
            link,
            lookups,
            held: Arc::new(Mutex::new(None)),
        }
    }

    /// The options of the next requests: as the agent of `run_id`, fenced; a level or a transition under `gate`, a
    /// lookup with the gate open, so its transition runs fenced.
    fn gate(&self, agent_id: &str, run_id: Option<&str>, gate: TransitionGate) {
        let origin = RenderOrigin::Agent {
            agent_id: agent_id.to_string(),
            run_id: run_id.map(str::to_string),
        };
        self.lookups.set_render_options(RenderOptions {
            origin: origin.clone(),
            gate: TransitionGate::Open,
            write_fence: true,
        });
        self.link.set_render_options(RenderOptions {
            origin,
            gate,
            write_fence: true,
        });
    }

    fn take_held(&self) -> Option<HeldRequest> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// Keep what the gate held on the request just made, if it held one: `true` when it did.
    fn note_held(&self, url: &str, method: &str, fields: &BTreeMap<String, String>) -> bool {
        let Some(body) = self.link.take_held() else {
            return false;
        };
        *self.held.lock().unwrap_or_else(PoisonError::into_inner) = Some(HeldRequest {
            url: self.link.resolve(url),
            method: method.to_ascii_uppercase(),
            fields: fields.clone(),
            body,
        });
        true
    }

    fn held_text(&self) -> String {
        let transition = self
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|held| held.body["heldWrite"]["transition"].as_str().map(str::to_string))
            .unwrap_or_default();
        format!("{transition} waits for the user's approval; nothing was saved")
    }
}

impl ScreenLink for AgentLink {
    fn level(&self, path: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        let result = self.link.level(path, parameters);
        if self.note_held(path, "GET", parameters) {
            return Err(LinkError::Remote(self.held_text()));
        }
        result
    }

    fn transition(
        &self,
        url: &str,
        method: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<TransitionOutcome, LinkError> {
        let method = if method.is_empty() { "POST" } else { method };
        let result = self.link.transition(url, method, fields);
        if self.note_held(url, method, fields) {
            // as a refused save: the edits stay on the screen
            return Ok(TransitionOutcome {
                screen_url: String::new(),
                messages: Messages {
                    errors: vec![self.held_text()],
                    ..Messages::default()
                },
                parameters: BTreeMap::new(),
            });
        }
        result
    }

    fn options(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<Vec<Choice>, LinkError> {
        self.lookups.options(url, parameters)
    }

    fn shell(&self, path: &str) -> Result<ShellInfo, LinkError> {
        self.lookups.shell(path)
    }

    fn login(&self, _username: &str, _password: &str, _code: &str) -> Result<(), LinkError> {
        Err(LinkError::Remote("an agent's screen session does not log in".into()))
    }

    fn json(&self, url: &str, method: &str, parameters: &BTreeMap<String, String>) -> Result<Json, LinkError> {
        self.lookups.json(url, method, parameters)
    }

    fn fragment(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        self.lookups.fragment(url, parameters)
    }

    fn raw(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<RawBody, LinkError> {
        self.lookups.raw(url, parameters)
    }

    fn subscribe(&self, _sink: PushSink) -> Option<Subscription> {
        None
    }
}

/// One session.
struct Held {
    /// The user's id.
    owner: String,
    username: String,
    agent_id: String,
    run_id: Option<String>,
    follow: bool,
    link: AgentLink,
    agent: Agent<AgentLink>,
    last_used: Instant,
}

/// A session in the registry: whose and for which conversation, readable while a command holds the session.
struct Entry {
    owner: String,
    conversation_id: String,
    held: Arc<Mutex<Held>>,
}

/// The sessions of this node, by id.
pub(crate) struct ScreenRegistry {
    factory: Weak<ExecutionContextFactory>,
    live: Mutex<HashMap<String, Entry>>,
}

/// Install the registry as this process's screen sessions.
pub(crate) fn install(factory: &Arc<ExecutionContextFactory>) {
    screen::install_screen_sessions(Arc::new(ScreenRegistry {
        factory: Arc::downgrade(factory),
        live: Mutex::new(HashMap::new()),
    }));
}

impl ScreenRegistry {
    /// Drop the sessions idle past [`IDLE`]; one in use is left.
    fn reap(&self) {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        live.retain(|id, entry| match entry.held.try_lock() {
            Ok(held) => {
                let keep = held.last_used.elapsed() < IDLE;
                if !keep {
                    tracing::info!(
                        "agent screen session {id} of {} ended after {IDLE:?} unused",
                        held.username
                    );
                }
                keep
            }
            Err(_) => true,
        });
    }

    /// The session `id` when it is `owner`'s.
    fn get(&self, id: &str, owner: &str) -> Option<Arc<Mutex<Held>>> {
        self.reap();
        let live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = live.get(id)?;
        (entry.owner == owner).then(|| Arc::clone(&entry.held))
    }

    /// Make room for one more session of `owner`: every session of the owner counts, and at [`MAX_PER_USER`] the
    /// least recently used one that no command holds now ends. When all of them are busy the new one is refused,
    /// never let past the limit; so is one past [`MAX_SESSIONS`] on the node.
    fn make_room(&self, owner: &str) -> Result<(), ScreenError> {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        let mine: Vec<(String, Option<Instant>)> = live
            .iter()
            .filter(|(_, entry)| entry.owner == owner)
            .map(|(id, entry)| (id.clone(), entry.held.try_lock().ok().map(|held| held.last_used)))
            .collect();
        if let Some(oldest) = evicted(&mine)? {
            tracing::info!("agent screen session {oldest} of {owner} ended for a newer one");
            live.remove(&oldest);
        }
        if live.len() >= MAX_SESSIONS {
            return Err(ScreenError::Refused(format!(
                "this server has {MAX_SESSIONS} screen sessions open; try again later"
            )));
        }
        Ok(())
    }
}

/// Which of a user's sessions ends for a new one: none under [`MAX_PER_USER`], else the least recently used of those
/// no command holds (`None` is one in use).
///
/// # Errors
///
/// [`ScreenError::Refused`] when the user has [`MAX_PER_USER`] sessions and every one is in use.
fn evicted(mine: &[(String, Option<Instant>)]) -> Result<Option<String>, ScreenError> {
    if mine.len() < MAX_PER_USER {
        return Ok(None);
    }
    mine.iter()
        .filter_map(|(id, used)| used.map(|used| (used, id)))
        .min()
        .map(|(_, id)| Some(id.clone()))
        .ok_or_else(|| {
            ScreenError::Refused(format!(
                "you have {MAX_PER_USER} screen sessions busy in other conversations; try again when one is done"
            ))
        })
}

impl ScreenSessions for ScreenRegistry {
    fn open(&self, spec: &OpenSpec) -> Result<String, ScreenError> {
        if let Some(id) = self.find(&spec.user_id, &spec.conversation_id) {
            return Ok(id);
        }
        self.make_room(&spec.user_id)?;
        let link = AgentLink::new(Weak::clone(&self.factory), &spec.username);
        link.gate(&spec.agent_id, spec.run_id.as_deref(), TransitionGate::Hold);
        let start = spec
            .state
            .as_ref()
            .filter(|state| screen::path_problem(&state.path).is_none())
            .map(ScreenState::url)
            .unwrap_or_else(|| spec.start_path.clone());
        let columns = if spec.columns == 0 {
            screen::DEFAULT_COLUMNS
        } else {
            spec.columns
        };
        let rows = if spec.rows == 0 {
            screen::DEFAULT_ROWS
        } else {
            spec.rows
        };
        let mut agent = Agent::open(link.clone(), &start, columns, rows);
        agent.set_marks_writes(true);
        if let Some(state) = &spec.state {
            for (dom_id, values) in &state.values {
                match agent.set_values(dom_id, values) {
                    Ok(ignored) if !ignored.is_empty() => tracing::warn!(
                        "the screen session of conversation {} left out the values {ignored:?} of {dom_id}",
                        spec.conversation_id
                    ),
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        "the screen session of conversation {} did not take its kept values: {error}",
                        spec.conversation_id
                    ),
                }
            }
        }
        let id = moqui_core::user::random_string(16)
            .map_err(|error| ScreenError::Failed(format!("no session id: {error}")))?;
        let held = Held {
            owner: spec.user_id.clone(),
            username: spec.username.clone(),
            agent_id: spec.agent_id.clone(),
            run_id: spec.run_id.clone(),
            follow: spec.follow,
            link,
            agent,
            last_used: Instant::now(),
        };
        tracing::info!(
            "agent screen session {id} of {} opened for conversation {} at {start}",
            spec.username,
            spec.conversation_id
        );
        self.live.lock().unwrap_or_else(PoisonError::into_inner).insert(
            id.clone(),
            Entry {
                owner: spec.user_id.clone(),
                conversation_id: spec.conversation_id.clone(),
                held: Arc::new(Mutex::new(held)),
            },
        );
        Ok(id)
    }

    fn find(&self, owner: &str, conversation_id: &str) -> Option<String> {
        self.reap();
        let live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        live.iter()
            .find(|(_, entry)| entry.owner == owner && entry.conversation_id == conversation_id)
            .map(|(id, _)| id.clone())
    }

    fn command(&self, id: &str, owner: &str, command: &ScreenCommand) -> Result<ScreenAnswer, ScreenError> {
        let held = self.get(id, owner).ok_or(ScreenError::NotFound)?;
        let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
        held.last_used = Instant::now();
        if !command.agent_id.is_empty() {
            held.agent_id.clone_from(&command.agent_id);
            held.run_id.clone_from(&command.run_id);
        }
        held.link
            .gate(&held.agent_id, held.run_id.as_deref(), TransitionGate::Hold);
        held.link.take_held();
        Ok(run(&mut held, command))
    }

    fn sync(&self, id: &str, owner: &str, state: &BrowserState) -> Result<Vec<FollowEvent>, ScreenError> {
        let held = self.get(id, owner).ok_or(ScreenError::NotFound)?;
        let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
        held.last_used = Instant::now();
        held.link
            .gate(&held.agent_id, held.run_id.as_deref(), TransitionGate::Hold);
        let wanted = ScreenState {
            path: state.path.clone(),
            parameters: state.parameters.clone(),
            values: BTreeMap::new(),
        };
        let session = held.agent.session();
        let elsewhere = session.path() != state.path || session.parameters() != &state.parameters;
        if elsewhere && screen::path_problem(&state.path).is_none() {
            let answer = held.agent.run(&format!("go {}", wanted.url()));
            if !answer.ok {
                tracing::warn!("the screen session {id} did not go to the user's {}", state.path);
            }
        }
        for form in &state.forms {
            let values = form.dirty_values();
            if values.is_empty() {
                continue;
            }
            match held.agent.set_values(&form.dom_id, &values) {
                Ok(ignored) if !ignored.is_empty() => {
                    tracing::warn!("the screen session {id} left out {ignored:?} of {}", form.dom_id);
                }
                Ok(_) => {}
                Err(error) => tracing::warn!("the screen session {id} did not take the user's values: {error}"),
            }
        }
        held.link.take_held();
        Ok(Vec::new())
    }

    fn recheck(&self, id: &str, owner: &str, write: &PendingWrite) -> Result<Vec<FieldConflict>, ScreenError> {
        let (username, agent_id) = self.identity(id, owner, write)?;
        let link = AgentLink::new(Weak::clone(&self.factory), &username);
        link.gate(&agent_id, None, TransitionGate::Hold);
        let url = ScreenState {
            path: write.path.clone(),
            parameters: write.parameters.clone(),
            values: BTreeMap::new(),
        }
        .url();
        let fresh = Agent::open(link, &url, screen::DEFAULT_COLUMNS, screen::DEFAULT_ROWS);
        if fresh.session().path() != write.path && fresh.session().message_is_error() {
            return Err(ScreenError::Failed(format!(
                "{} did not render again: {}",
                write.path,
                fresh.session().message()
            )));
        }
        Ok(write
            .changes
            .iter()
            .filter_map(|change| {
                let current = fresh.original_value(&write.form, &change.field)?;
                (current != change.original && current != change.proposed).then(|| FieldConflict {
                    field: change.field.clone(),
                    label: change.label.clone(),
                    original: change.original.clone(),
                    current,
                    proposed: change.proposed.clone(),
                })
            })
            .collect())
    }

    fn execute_approved(
        &self,
        id: &str,
        owner: &str,
        write: &PendingWrite,
        grant: &ApprovalGrant,
    ) -> Result<ScreenAnswer, ScreenError> {
        if write.owner != owner {
            return Err(ScreenError::NotFound);
        }
        let approved = TransitionGate::Approved {
            approval_id: grant.approval_id.clone(),
            args_hash: grant.args_hash.clone(),
        };
        if let Some(held) = self.get(id, owner) {
            let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
            held.last_used = Instant::now();
            held.agent_id.clone_from(&grant.agent_id);
            held.run_id.clone_from(&grant.run_id);
            held.link.gate(&grant.agent_id, grant.run_id.as_deref(), approved);
            let outcome = held.link.transition(&write.url, &write.method, &write.fields);
            held.link
                .gate(&grant.agent_id, grant.run_id.as_deref(), TransitionGate::Hold);
            let outcome = approved_outcome(&held.link, outcome)?;
            let before = held.agent.snapshot();
            let to = if outcome.screen_url.is_empty() {
                held.agent.session().location_url()
            } else {
                outcome.screen_url.clone()
            };
            let step = held.agent.run(&format!("go {to}"));
            let mut answer = answer_of(&held, &step, &before, None);
            answer.text = with_messages(&answer.text, &outcome.messages);
            return Ok(answer);
        }
        // the session is gone: the write runs on a link of its own
        let username = self.username_of(owner)?;
        let link = AgentLink::new(Weak::clone(&self.factory), &username);
        link.gate(&grant.agent_id, grant.run_id.as_deref(), approved);
        let outcome = approved_outcome(&link, link.transition(&write.url, &write.method, &write.fields))?;
        let text = with_messages(&format!("path {}\n--- ok", outcome.screen_url), &outcome.messages);
        Ok(ScreenAnswer {
            ok: outcome.messages.errors.is_empty(),
            text,
            path: outcome.screen_url.clone(),
            ..ScreenAnswer::default()
        })
    }

    fn close(&self, id: &str) {
        if self
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
            .is_some()
        {
            tracing::info!("agent screen session {id} closed");
        }
    }
}

impl ScreenRegistry {
    /// The username and agent of the owner's session `id`, or of the owner alone when this node no longer has it.
    /// The username and agent a held write is rechecked as: the write must be `owner`'s; the username is the owner's
    /// session's, or read when this node no longer has it; the agent is the write's.
    fn identity(&self, id: &str, owner: &str, write: &PendingWrite) -> Result<(String, String), ScreenError> {
        if write.owner != owner {
            return Err(ScreenError::NotFound);
        }
        let session = self.get(id, owner).map(|held| {
            let held = held.lock().unwrap_or_else(PoisonError::into_inner);
            (held.username.clone(), held.agent_id.clone())
        });
        let (username, session_agent) = match session {
            Some(found) => found,
            None => (self.username_of(owner)?, String::new()),
        };
        let agent_id = if write.agent_id.is_empty() {
            session_agent
        } else {
            write.agent_id.clone()
        };
        if agent_id.is_empty() {
            return Err(ScreenError::Failed("the held write names no agent".to_string()));
        }
        Ok((username, agent_id))
    }

    fn username_of(&self, owner: &str) -> Result<String, ScreenError> {
        let factory = self
            .factory
            .upgrade()
            .ok_or_else(|| ScreenError::Failed("the server is stopping".to_string()))?;
        let facade = factory
            .entity()
            .ok_or_else(|| ScreenError::Failed("the entity facade is not installed".to_string()))?;
        let row = facade
            .find("moqui.security.UserAccount")
            .and_then(|find| {
                find.eq("userId", moqui_lib::object::Value::string(owner))
                    .disable_authz()
                    .one()
            })
            .map_err(|error| ScreenError::Failed(format!("the user could not be read: {error}")))?;
        row.and_then(|row| row.get("username").map(moqui_lib::object::to_plain_string))
            .filter(|name| !name.is_empty())
            .ok_or(ScreenError::NotFound)
    }
}

/// A transition run with an approval's grant: a hold again is a mismatch, refused.
fn approved_outcome(
    link: &AgentLink,
    outcome: Result<TransitionOutcome, LinkError>,
) -> Result<TransitionOutcome, ScreenError> {
    if let Some(held) = link.take_held() {
        let mismatch = held.body.get("approvalMismatch").is_some();
        return Err(ScreenError::Refused(if mismatch {
            "the write is not the one approved, so it was held again; nothing was saved".to_string()
        } else {
            "the write was held again; nothing was saved".to_string()
        }));
    }
    outcome.map_err(|error| ScreenError::Failed(error.to_string()))
}

/// `text` with the transition's messages before its outcome line, and an error outcome when it reported errors.
fn with_messages(text: &str, messages: &Messages) -> String {
    let (body, outcome) = match text.rfind("\n--- ") {
        Some(at) => (&text[..at], &text[at + 1..]),
        None => (text, "--- ok"),
    };
    let mut lines = vec![body.to_string()];
    for info in &messages.info {
        lines.push(format!("message {info}"));
    }
    let mut errors = messages.errors.clone();
    errors.extend(
        messages
            .validation_errors
            .iter()
            .map(|error| format!("{}: {}", error.field, error.message)),
    );
    if errors.is_empty() {
        lines.push(outcome.to_string());
    } else {
        lines.push(format!("--- error {}", errors.join("; ")));
    }
    lines.join("\n")
}

/// Run a command's steps in order (the size, `go`, each `set`, `find`, `do`, `key`); the first that fails stops the
/// rest. A `set` of a drop-down whose choices wait on another field set later in the same call is tried once more at
/// the end.
fn run(held: &mut Held, command: &ScreenCommand) -> ScreenAnswer {
    let before = held.agent.snapshot();
    let mut last: Option<AgentAnswer> = None;
    let mut steps: Vec<String> = Vec::new();
    if let Some(rows) = command.rows {
        let (columns, _) = held.agent.drawing_size();
        steps.push(format!("size {columns}x{rows}"));
    }
    if let Some(path) = &command.go {
        steps.push(format!("go {path}"));
    }
    let mut failed = None;
    for line in &steps {
        let from = held.agent.session().path().to_string();
        let answer = held.agent.run(line);
        // a go that stayed where it was with an error, such as a screen the user may not open
        let session = held.agent.session();
        let nowhere = line.starts_with("go ") && session.message_is_error() && session.path() == from;
        if !answer.ok || nowhere {
            failed = Some(fail_go(&held.agent, answer, line));
            break;
        }
        last = Some(answer);
    }
    if failed.is_none() {
        let mut retry = Vec::new();
        // the name and the value go apart, so a value with spaces is never read as part of the name
        for (element, value) in &command.set {
            let answer = held.agent.set_field(element, value);
            if !answer.ok {
                retry.push((element.clone(), value.clone()));
            }
            last = Some(answer);
        }
        for (element, value) in retry {
            let answer = held.agent.set_field(&element, &value);
            let ok = answer.ok;
            last = Some(answer);
            if !ok {
                failed.clone_from(&last);
                break;
            }
        }
    }
    if failed.is_none() {
        for line in [
            command.find.as_ref().map(|text| format!("find {text}")),
            command.press.as_ref().map(|element| format!("do {element}")),
            command.key.as_ref().map(|keys| format!("key {keys}")),
        ]
        .into_iter()
        .flatten()
        {
            let answer = held.agent.run(&line);
            let ok = answer.ok;
            last = Some(answer);
            if !ok {
                failed.clone_from(&last);
                break;
            }
        }
    }
    let answer = failed.or(last).unwrap_or_else(|| held.agent.look());
    let mut screen = answer_of(held, &answer, &before, command.find.as_deref());
    if command.elements {
        let listed = held.agent.run("elements");
        let list = listed.text.trim_end_matches("--- ok").trim_end().to_string();
        let (body, outcome) = match screen.text.rfind("\n--- ") {
            Some(at) => (screen.text[..at].to_string(), screen.text[at + 1..].to_string()),
            None => (screen.text.clone(), String::new()),
        };
        screen.text = format!("{body}\nelements:\n{list}\n{outcome}");
    }
    screen
}

/// A step that failed, or a `go` that went nowhere, as an error answer.
fn fail_go(agent: &Agent<AgentLink>, answer: AgentAnswer, line: &str) -> AgentAnswer {
    if answer.ok && line.starts_with("go ") {
        let text = match answer.text.rfind("\n--- ") {
            Some(at) => answer.text[..at].to_string(),
            None => answer.text.clone(),
        };
        return AgentAnswer {
            ok: false,
            text: format!(
                "{text}\n--- error {}: {}",
                line.trim_start_matches("go "),
                agent.session().message()
            ),
        };
    }
    answer
}

/// The answer of a command: the screen's text, where it is, the write the gate held, the follow events since `before`
/// (with a `ui.highlight` for what `find` moved to) and the state to keep.
fn answer_of(held: &Held, answer: &AgentAnswer, before: &moqui_tui::AgentSnapshot, find: Option<&str>) -> ScreenAnswer {
    let agent = &held.agent;
    let session = agent.session();
    let pending = held.link.take_held().map(|request| pending_write(held, request));
    let mut follow = Vec::new();
    if held.follow {
        follow = agent
            .follow(before)
            .into_iter()
            .map(|(event_type, payload)| FollowEvent { event_type, payload })
            .collect();
        if find.is_some() && answer.ok {
            if let Some(target) = agent.focus_target() {
                follow.push(FollowEvent {
                    event_type: "ui.highlight".to_string(),
                    payload: target,
                });
            }
        }
    }
    ScreenAnswer {
        ok: answer.ok,
        text: answer.text.clone(),
        path: session.path().to_string(),
        parameters: session.parameters().clone(),
        title: session.crumbs().join(" ▸ "),
        held: pending,
        follow,
        state: ScreenState {
            path: session.path().to_string(),
            parameters: session.parameters().clone(),
            values: agent.unsaved(),
        },
    }
}

/// A held request as the approval keeps it: the request, the `heldWrite` and its hash, where it was, the element
/// pressed and the changed fields of its form.
fn pending_write(held_by: &Held, request: HeldRequest) -> PendingWrite {
    let agent = &held_by.agent;
    let session = agent.session();
    let focus = agent.focus_target().unwrap_or(Json::Null);
    let form = focus["domId"].as_str().unwrap_or_default().to_string();
    let changes = agent
        .changed_fields()
        .into_iter()
        .filter(|change| form.is_empty() || change.dom_id == form)
        .map(|change| FieldChange {
            field: change.name,
            label: change.label,
            original: change.original,
            proposed: change.proposed,
        })
        .collect();
    let held = request.body.get("heldWrite").cloned().unwrap_or_else(|| json!({}));
    PendingWrite {
        owner: held_by.owner.clone(),
        agent_id: held_by.agent_id.clone(),
        url: request.url,
        method: request.method,
        fields: request.fields,
        args_hash: held["argsHash"].as_str().unwrap_or_default().to_string(),
        held,
        path: session.path().to_string(),
        parameters: session.parameters().clone(),
        element: focus["tuiId"].as_str().unwrap_or_default().to_string(),
        form,
        changes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_session_takes_the_place_of_the_least_recently_used_idle_one_and_never_passes_the_limit() {
        let now = Instant::now();
        let at = |seconds: u64| Some(now.checked_sub(Duration::from_secs(seconds)).unwrap_or(now));
        let few = [("a".to_string(), at(10)), ("b".to_string(), None)];
        assert_eq!(evicted(&few), Ok(None));
        let full = [
            ("a".to_string(), at(10)),
            ("b".to_string(), None),
            ("c".to_string(), at(30)),
            ("d".to_string(), at(20)),
        ];
        assert_eq!(evicted(&full), Ok(Some("c".to_string())), "the oldest idle one");
        let busy: Vec<(String, Option<Instant>)> = (0..MAX_PER_USER).map(|n| (n.to_string(), None)).collect();
        assert!(matches!(evicted(&busy), Err(ScreenError::Refused(text)) if text.contains("busy")));
    }
}
