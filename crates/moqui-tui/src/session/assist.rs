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

//! The session's side of the Assist pane (plans/LLM-A2A-Assist.md 4.12): its keys, the `/assist/*` calls through the
//! JSON lane (docs/Assist.md "The calls"), the stored events read after each sequence while a run works, and follow:
//! the agent's `ui.navigate`, `ui.fill`, `ui.dialog` and `ui.focus` applied to this session, and a hand-off's Save
//! focused. The reducer is [`crate::assist::state`].

use serde_json::Value;

use super::*;
use crate::assist::state::{is_ended, is_paused, Effect, Follow};
use crate::assist::{answered_error, error_text, Act, Area, Assist, Bootstrap, Card, Thread};
use crate::state::AssistApproval;

/// The events of a working run are read again this often.
const POLL_MILLIS: u64 = 1000;

impl Session {
    // ---- what the view reads ----

    pub fn assist(&self) -> &Assist {
        &self.assist
    }

    /// A field Assist filled that still holds what it was given: drawn with an "Assist" tag.
    pub fn assist_filled(&self, key: &str) -> bool {
        self.assist
            .filled
            .get(key)
            .is_some_and(|value| value == self.buffer(key))
    }

    /// This session's Assist client id (made on first use): the follow owner a turn's events are stamped with.
    pub fn assist_client(&mut self) -> String {
        self.assist.client_id()
    }

    /// The pane has the keys.
    pub fn assist_focused(&self) -> bool {
        self.assist.open && self.assist.focused
    }

    // ---- keys ----

    /// F2, Alt+A: show the pane and focus it; again, close it.
    pub(super) fn toggle_assist(&mut self) {
        if self.assist_focused() {
            self.assist.open = false;
            self.assist.focused = false;
            return;
        }
        self.assist.open = true;
        self.assist.focused = true;
        self.assist.area = Area::Composer;
        if self.assist.bootstrap.is_none() {
            self.assist_call("bootstrap", "bootstrap", "get", BTreeMap::new());
        }
    }

    /// Alt+X: stop the active run; the text so far is kept.
    pub(super) fn assist_stop(&mut self) {
        let Some(run_id) = self.assist.thread.active_run.clone() else {
            self.set_message("Assist is not running.".to_string(), false);
            return;
        };
        self.assist_call(
            "cancel",
            "cancel",
            "post",
            BTreeMap::from([("runId".to_string(), run_id)]),
        );
    }

    /// A key while the pane has the focus. Esc gives the keys back to the screen.
    pub(super) fn assist_key(&mut self, press: &KeyPress) -> bool {
        if !self.assist_focused() {
            return false;
        }
        let area = self.assist.area;
        match &press.key {
            Key::Escape | Key::F(12) => {
                self.assist.focused = false;
                return true;
            }
            Key::Tab => {
                self.assist.cycle(true);
                return true;
            }
            Key::BackTab => {
                self.assist.cycle(false);
                return true;
            }
            Key::F(10) => {
                self.assist_menu();
                return true;
            }
            Key::F(4) if area == Area::Composer => {
                self.overlays.push(Overlay::Prompt(Prompt {
                    title: "Message · Enter is a new line, Alt+Enter sends".to_string(),
                    text: std::mem::take(&mut self.assist.composer),
                    then: PromptThen::AssistText,
                }));
                self.assist.cursor = 0;
                return true;
            }
            Key::Enter if area == Area::Composer => {
                let text = std::mem::take(&mut self.assist.composer);
                self.assist.cursor = 0;
                self.assist_send(&text);
                return true;
            }
            Key::Enter if area == Area::Cards => {
                self.card_enter();
                return true;
            }
            Key::Up | Key::Down if area == Area::Cards => {
                let last = self.assist.cards().len().saturating_sub(1);
                let at = self.assist.card_at.min(last);
                self.assist.card_at = if press.key == Key::Up {
                    at.saturating_sub(1)
                } else {
                    (at + 1).min(last)
                };
                return true;
            }
            Key::Up | Key::Down | Key::PageUp | Key::PageDown if area == Area::Transcript => {
                let step = if matches!(press.key, Key::PageUp | Key::PageDown) {
                    10
                } else {
                    1
                };
                self.assist.scroll = if matches!(press.key, Key::Up | Key::PageUp) {
                    self.assist.scroll.saturating_add(step)
                } else {
                    self.assist.scroll.saturating_sub(step)
                };
                return true;
            }
            Key::Char('o') if area != Area::Composer && press.is_plain() => {
                self.open_canvas();
                return true;
            }
            _ => {}
        }
        if area == Area::Composer {
            let (mut text, mut cursor) = (std::mem::take(&mut self.assist.composer), self.assist.cursor);
            let edited = edit::apply(&mut text, &mut cursor, press, &Limits::default());
            self.assist.composer = text;
            self.assist.cursor = cursor;
            if edited != Edited::No {
                return true;
            }
        }
        if let Some(action) = self.keys.action(press) {
            if matches!(
                action,
                Action::Assist
                    | Action::AssistStop
                    | Action::Quit
                    | Action::Redraw
                    | Action::Help
                    | Action::Log
                    | Action::Messages
                    | Action::Mouse
            ) {
                self.act(action);
            }
        }
        true
    }

    /// Enter on a card: an approval opens its confirmation, a question its options, a hand-off goes to its form.
    fn card_enter(&mut self) {
        match self.assist.card() {
            Some(Card::Approval(at)) => {
                let card = self.assist.thread.approvals[at].clone();
                let mut lines = vec![card.summary.clone()];
                lines.extend(card.changes.iter().map(|change| {
                    format!(
                        "{}: {} → {}",
                        change.label,
                        show(&change.current),
                        show(&change.proposed)
                    )
                }));
                self.overlays.push(Overlay::Confirm(Confirm {
                    kind: ConfirmKind::Approval(AssistApproval {
                        approval_id: card.approval_id,
                        lines,
                        can_edit: card.can_edit,
                    }),
                    then: Then::Nothing,
                    at: 0,
                }));
            }
            Some(Card::Question(at)) => {
                let card = self.assist.thread.questions[at].clone();
                let mut items: Vec<MenuEntry> = card
                    .options
                    .iter()
                    .map(|(label, recommended)| MenuEntry {
                        label: if *recommended {
                            format!("{label} (recommended)")
                        } else {
                            label.clone()
                        },
                        act: MenuAct::Assist(Act::Answer {
                            run_id: card.run_id.clone(),
                            question_id: card.question_id.clone(),
                            choice: label.clone(),
                        }),
                    })
                    .collect();
                if card.free_text || items.is_empty() {
                    items.push(MenuEntry {
                        label: "Other: type an answer".to_string(),
                        act: MenuAct::Assist(Act::AnswerText {
                            run_id: card.run_id,
                            question_id: card.question_id,
                        }),
                    });
                }
                self.open_menu(&card.text, items);
            }
            Some(Card::Handoff(at)) => {
                let card = self.assist.thread.handoffs[at].clone();
                let path = crate::assist::state::safe_path(&card.path).unwrap_or_default();
                if path.is_empty() || path == self.path() {
                    self.focus_save(&card.form);
                } else {
                    self.assist.handoff = Some(card.form);
                    self.follow_url(&path);
                }
            }
            None => {}
        }
    }

    /// `o` in the pane: the conversation's canvas in the browser (`?assist=`).
    fn open_canvas(&mut self) {
        let id = self.assist.thread.conversation_id.clone();
        if self.assist.thread.canvas.is_none() || id.is_empty() {
            self.set_message("This conversation has no canvas.".to_string(), false);
            return;
        }
        let url = self.location_url();
        let url = format!(
            "{url}{}assist={}",
            if url.contains('?') { '&' } else { '?' },
            encode(&id)
        );
        self.set_message(format!("Open in browser: {url}"), false);
        self.open = Some(url);
    }

    /// F10 in the pane: New, History, Tasks, Agent and Follow.
    fn assist_menu(&mut self) {
        let follow = if self.assist.follow_off {
            "Follow: off (turn on)"
        } else {
            "Follow: on (turn off)"
        };
        let items = [
            ("New conversation", Act::New),
            ("History", Act::History),
            ("Tasks", Act::Tasks),
            ("Agent", Act::Agent),
            (follow, Act::Follow),
        ]
        .into_iter()
        .map(|(label, act)| MenuEntry {
            label: label.to_string(),
            act: MenuAct::Assist(act),
        })
        .collect();
        self.open_menu("Assist", items);
    }

    pub(super) fn run_assist_act(&mut self, act: Act) {
        match act {
            Act::New => {
                let version = self.assist.thread.context_version;
                self.assist.thread = Thread::default();
                self.assist.thread.context_version = version;
                self.assist.queue.clear();
                self.timers.remove(&Timer::Assist);
                self.assist.focused = true;
                self.assist.area = Area::Composer;
            }
            Act::History => self.assist_call(
                "conversations",
                "conversations",
                "get",
                BTreeMap::from([("pageSize".to_string(), "15".to_string())]),
            ),
            Act::Tasks => self.assist_call("tasks", "tasks", "get", BTreeMap::new()),
            Act::Agent => {
                let agents = self
                    .assist
                    .bootstrap
                    .as_ref()
                    .map(|boot| boot.agents.clone())
                    .unwrap_or_default();
                let items = agents
                    .into_iter()
                    .map(|(id, name)| MenuEntry {
                        label: if name.is_empty() { id.clone() } else { name },
                        act: MenuAct::Assist(Act::PickAgent(id)),
                    })
                    .collect();
                self.open_menu("Agent", items);
            }
            Act::PickAgent(id) => {
                self.assist.agent_id = id;
                self.run_assist_act(Act::New);
            }
            Act::Follow => {
                self.assist.follow_off = !self.assist.follow_off;
                let id = self.assist.thread.conversation_id.clone();
                if !id.is_empty() {
                    let mut params = self.assist_ids();
                    params.insert("conversationId".to_string(), id);
                    if self.assist.follow_off {
                        params.insert("release".to_string(), "true".to_string());
                    }
                    self.assist_call("follow", "follow", "post", params);
                }
                let text = if self.assist.follow_off {
                    "Follow off: Assist's screens show as notes."
                } else {
                    "Follow on: the screen follows Assist."
                };
                self.set_message(text.to_string(), false);
            }
            Act::Open(id) => self.read_conversation(&id),
            Act::Answer {
                run_id,
                question_id,
                choice,
            } => {
                let params = BTreeMap::from([
                    ("runId".to_string(), run_id),
                    ("questionId".to_string(), question_id),
                    ("choice".to_string(), choice),
                ]);
                self.assist_call("answer", "answer", "post", params);
            }
            Act::AnswerText { run_id, question_id } => self.overlays.push(Overlay::Prompt(Prompt {
                title: "Your answer".to_string(),
                text: String::new(),
                then: PromptThen::AssistAnswer { run_id, question_id },
            })),
        }
    }

    /// The pane's prompts: a message of several lines, a rejection's feedback, a question's answer.
    pub(super) fn assist_prompt_key(&mut self, mut prompt: Prompt, press: &KeyPress) {
        let multiline = prompt.then == PromptThen::AssistText;
        let send = match &press.key {
            Key::Escape | Key::F(12) => {
                if multiline {
                    self.assist.cursor = prompt.text.chars().count();
                    self.assist.composer = prompt.text;
                }
                return;
            }
            Key::Enter => !multiline || press.alt || press.ctrl,
            Key::Char('s') => press.ctrl,
            _ => false,
        };
        if !send {
            let mut cursor = prompt.text.chars().count();
            let limits = Limits {
                multiline,
                ..Limits::default()
            };
            edit::apply(&mut prompt.text, &mut cursor, press, &limits);
            self.overlays.push(Overlay::Prompt(prompt));
            return;
        }
        let text = prompt.text.trim().to_string();
        match prompt.then {
            PromptThen::AssistText => self.assist_send(&text),
            PromptThen::AssistReject(approval_id) => {
                let mut params = self.assist_ids();
                params.extend([
                    ("approvalId".to_string(), approval_id.clone()),
                    ("decision".to_string(), "reject".to_string()),
                    ("feedback".to_string(), text),
                ]);
                self.assist_call(&format!("decide:{approval_id}"), "decide", "post", params);
            }
            PromptThen::AssistAnswer { run_id, question_id } => {
                let params = BTreeMap::from([
                    ("runId".to_string(), run_id),
                    ("questionId".to_string(), question_id),
                    ("text".to_string(), text),
                ]);
                self.assist_call("answer", "answer", "post", params);
            }
            PromptThen::SaveFind(_) => {}
        }
    }

    /// The approval's confirmation answered: Approve, Edit in screen (a hand-off to this screen), or Reject, which asks
    /// for what Assist should do instead.
    pub(super) fn assist_decide(&mut self, approval_id: &str, choice: &str) {
        let decision = match choice {
            "Approve" => "approve",
            "Edit in screen" => "edit",
            "Reject" => {
                self.overlays.push(Overlay::Prompt(Prompt {
                    title: "Reject: what should Assist do instead?".to_string(),
                    text: String::new(),
                    then: PromptThen::AssistReject(approval_id.to_string()),
                }));
                return;
            }
            _ => return,
        };
        let mut params = self.assist_ids();
        params.insert("approvalId".to_string(), approval_id.to_string());
        params.insert("decision".to_string(), decision.to_string());
        self.assist_call(&format!("decide:{approval_id}"), "decide", "post", params);
    }

    // ---- the calls ----

    fn assist_call(&mut self, lane: &str, call: &str, method: &str, parameters: BTreeMap<String, String>) {
        self.lanes.push(
            Purpose::Assist(lane.to_string()),
            LinkCall::Json {
                url: format!("/assist/{call}"),
                method: method.to_string(),
                parameters,
            },
        );
    }

    /// The client's id and page version, which every turn, follow, context and decision carries.
    fn assist_ids(&mut self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("clientId".to_string(), self.assist.client_id()),
            (
                "contextVersion".to_string(),
                self.assist.thread.context_version.to_string(),
            ),
        ])
    }

    /// The screen as the turn's `state`: its path and parameters, as JSON text.
    fn page_state(&self) -> String {
        serde_json::json!({"path": self.path(), "parameters": self.parameters(), "title": self.model().screen.title})
            .to_string()
    }

    fn read_conversation(&mut self, id: &str) {
        let mut params = BTreeMap::from([("conversationId".to_string(), id.to_string())]);
        params.insert("clientId".to_string(), self.assist.client_id());
        self.assist_call("conversation", "conversation", "get", params);
    }

    /// Send a message: a new conversation's first, or the next of this one (queued while its run works).
    fn assist_send(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let max = self.assist.bootstrap.as_ref().map_or(16000, |boot| boot.max_chars);
        if text.chars().count() > max {
            self.assist.composer = text.to_string();
            self.set_message(format!("A message holds at most {max} characters."), true);
            return;
        }
        let message_id = self.assist.message_id();
        self.assist.thread.outgoing(&message_id, text);
        self.assist.scroll = 0;
        let mut params = self.assist_ids();
        params.extend([
            ("text".to_string(), text.to_string()),
            ("clientMessageId".to_string(), message_id.clone()),
            ("surface".to_string(), "tui".to_string()),
            ("state".to_string(), self.page_state()),
        ]);
        let conversation = self.assist.thread.conversation_id.clone();
        if conversation.is_empty() {
            let agent = if self.assist.agent_id.is_empty() {
                self.assist
                    .bootstrap
                    .as_ref()
                    .map_or_else(|| "assist".to_string(), |boot| boot.agent_id.clone())
            } else {
                self.assist.agent_id.clone()
            };
            params.insert("agentId".to_string(), agent);
        } else {
            params.insert("conversationId".to_string(), conversation);
        }
        self.assist_call(&format!("turn:{message_id}"), "turn", "post", params);
    }

    /// Read the active run's stored events after the last one applied: on the timer while it works, at once after a
    /// decision, an answer or a push (`force`, even while it waits).
    pub(super) fn assist_poll(&mut self, force: bool) {
        self.timers.remove(&Timer::Assist);
        let Some(run) = self.assist.thread.active().cloned() else {
            return;
        };
        if is_paused(&run.state) && !force {
            return;
        }
        let after = self.assist.thread.seq.get(&run.run_id).copied().unwrap_or(0);
        self.assist.polling = run.run_id.clone();
        self.assist_call(
            "events",
            "events",
            "get",
            BTreeMap::from([
                ("runId".to_string(), run.run_id),
                ("after".to_string(), after.to_string()),
            ]),
        );
    }

    fn poll_later(&mut self) {
        let waits = self
            .assist
            .thread
            .active()
            .is_some_and(|run| !is_paused(&run.state) && !is_ended(&run.state));
        if waits && !self.lanes.busy(&Purpose::Assist("events".to_string())) {
            self.set_timer(Timer::Assist, Instant::now() + Duration::from_millis(POLL_MILLIS));
        }
    }

    pub(super) fn assist_reply(&mut self, lane: &str, result: Result<Value, LinkError>) {
        let value = match result
            .map_err(|err| error_text(&err))
            .and_then(|value| match answered_error(&value) {
                Some(error) => Err(error),
                None => Ok(value),
            }) {
            Ok(value) => value,
            Err(error) => {
                if let Some(message_id) = lane.strip_prefix("turn:") {
                    self.assist.thread.turn_failed(message_id, &error);
                }
                if lane == "bootstrap" {
                    self.assist.bootstrap = Some(Bootstrap {
                        reason: format!("Assist is not available: {error}"),
                        ..Bootstrap::default()
                    });
                    self.set_message(format!("Assist is not available: {error}"), true);
                } else if lane == "events" {
                    // The poll goes on: one failed read is not the end of the run.
                    self.poll_later();
                } else {
                    self.set_message(error, true);
                }
                return;
            }
        };
        let client = self.assist.client_id();
        let call = lane.split(':').next().unwrap_or(lane);
        match call {
            "bootstrap" => {
                let boot = Bootstrap::of(&value);
                if !boot.enabled {
                    self.set_message(boot.reason.clone(), true);
                }
                if self.assist.agent_id.is_empty() {
                    self.assist.agent_id.clone_from(&boot.agent_id);
                }
                self.assist.bootstrap = Some(boot);
            }
            "turn" => {
                let message_id = &lane["turn:".len()..];
                self.assist.thread.turn_answered(message_id, &value, &client);
                self.assist_poll(true);
            }
            "conversation" => {
                self.assist.thread.snapshot(&value, &client);
                self.assist.card_at = 0;
                self.assist.scroll = 0;
                self.assist_poll(false);
            }
            "events" => {
                let run_id = self.assist.polling.clone();
                let events: Vec<Value> = value
                    .get("events")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.assist_apply(&run_id, &events);
                let state = value.get("state").and_then(Value::as_str).unwrap_or("");
                let stale = self.assist.thread.active_run.as_deref() == Some(run_id.as_str()) && is_ended(state);
                if stale && events.is_empty() {
                    // The run ended and its events say nothing more: the conversation has its outcome.
                    let id = self.assist.thread.conversation_id.clone();
                    self.read_conversation(&id);
                } else {
                    self.poll_later();
                }
            }
            "decide" | "answer" | "cancel" => {
                if let Some(follow) = value.get("follow").filter(|follow| follow.is_object()) {
                    self.assist.thread.take_follow(follow, &client);
                }
                if call == "cancel" {
                    self.set_message("Stopped.".to_string(), false);
                }
                self.assist_poll(true);
            }
            "follow" | "context" => {
                if let Some(follow) = value.get("follow").filter(|follow| follow.is_object()) {
                    self.assist.thread.take_follow(follow, &client);
                }
            }
            "conversations" => {
                let items = value
                    .get("conversations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|row| {
                        let title = shell::json_text(row, "title");
                        let mark = if row.get("waiting").and_then(Value::as_bool).unwrap_or(false) {
                            " · waiting"
                        } else if !shell::json_text(row, "activeRunId").is_empty() {
                            " · running"
                        } else {
                            ""
                        };
                        MenuEntry {
                            label: format!("{}{mark}", if title.is_empty() { "(untitled)" } else { &title }),
                            act: MenuAct::Assist(Act::Open(shell::json_text(row, "conversationId"))),
                        }
                    })
                    .collect();
                self.open_menu("Conversations", items);
            }
            "tasks" => {
                let items = value
                    .get("waiting")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|row| {
                        let kind = shell::json_text(row, "kind");
                        let summary = shell::json_text(row, "summary");
                        MenuEntry {
                            label: format!("{kind}: {summary} · {}", shell::json_text(row, "conversationTitle")),
                            act: MenuAct::Assist(Act::Open(shell::json_text(row, "conversationId"))),
                        }
                    })
                    .collect();
                self.open_menu("Waiting for you", items);
            }
            _ => {}
        }
    }

    /// Stored events of `run_id`, in order, through the reducer; the follow they ask for is done once the screen is
    /// not loading.
    fn assist_apply(&mut self, run_id: &str, events: &[Value]) {
        if run_id.is_empty() {
            return;
        }
        let mut refresh = false;
        for event in events {
            let ctx_client = self.assist.client_id();
            let ctx = crate::assist::state::Ctx {
                client_id: &ctx_client,
                follow_on: !self.assist.follow_off,
            };
            for effect in self.assist.thread.apply(run_id, event, ctx) {
                match effect {
                    Effect::Gap { .. } => {
                        // The next read starts after the last event applied.
                        self.assist_poll(true);
                        self.assist_drain();
                        return;
                    }
                    Effect::Follow(follow) => self.assist.queue.push_back(follow),
                    Effect::Refresh => refresh = true,
                    Effect::Paused | Effect::Ended => {}
                }
            }
        }
        if refresh {
            let id = self.assist.thread.conversation_id.clone();
            self.read_conversation(&id);
        }
        self.assist_drain();
    }

    /// A notification of topic `AgentRun` (a run started, waits, was decided or ended): this conversation's run is
    /// read now.
    pub(super) fn assist_push(&mut self, push: &crate::link::Push) {
        let id = self.assist.thread.conversation_id.clone();
        if id.is_empty() {
            return;
        }
        let conversation = shell::json_text(&push.message, "conversationId");
        let run = shell::json_text(&push.message, "runId");
        let known = !run.is_empty() && self.assist.thread.runs.contains_key(&run);
        if conversation != id && !known {
            return;
        }
        if self.assist.thread.active_run.is_some() && (known || run.is_empty()) {
            self.assist_poll(true);
        } else {
            self.read_conversation(&id);
        }
    }

    /// A screen came: after the user's own navigation the server hears of the new page (`context`, a higher page
    /// version, so the agent's older page events are dropped); then follow goes on.
    pub(super) fn assist_after_level(&mut self, navigate: bool) {
        if navigate {
            self.assist.filled.clear();
        }
        if std::mem::take(&mut self.assist.follow_nav) {
            self.assist_drain();
            return;
        }
        let id = self.assist.thread.conversation_id.clone();
        if navigate && !id.is_empty() {
            self.assist.thread.context_version += 1;
            let mut params = self.assist_ids();
            params.insert("conversationId".to_string(), id);
            params.insert("state".to_string(), self.page_state());
            self.assist_call("context", "context", "post", params);
        }
        self.assist_drain();
    }

    /// Do what follow asks, in order, while no screen is loading; a navigation waits for its screen.
    fn assist_drain(&mut self) {
        while !self.lanes.busy(&Purpose::Level) {
            let Some(follow) = self.assist.queue.pop_front() else {
                break;
            };
            match follow {
                Follow::Navigate {
                    path,
                    parameters,
                    title,
                } => {
                    if path == self.path() && &parameters == self.parameters() {
                        continue;
                    }
                    if self.edits.any_dirty() {
                        self.set_message(
                            format!("Assist opened {title}; not followed while this screen has unsaved edits."),
                            false,
                        );
                        self.assist.queue.clear();
                        break;
                    }
                    self.assist.follow_nav = true;
                    self.request_level(&path, parameters, Intent::Navigate { history: true });
                }
                Follow::Fill {
                    tui_id,
                    form,
                    field,
                    value,
                    label,
                } => {
                    let key = if self.index.fields.contains_key(tui_id.split('#').next().unwrap_or("")) {
                        Some(tui_id)
                    } else {
                        self.fill_key(&form, &field)
                    };
                    let outcome = match key {
                        Some(key) => self.fill(&key, &value),
                        None => Err(format!("no field {field} here")),
                    };
                    match outcome {
                        Ok(filled) => self.set_message(format!("Assist filled {filled}"), false),
                        Err(why) => self.set_message(format!("Assist could not fill {label}: {why}"), true),
                    }
                }
                Follow::Focus { tui_id } => {
                    self.focus_key(&tui_id);
                }
                Follow::Dialog { id, open } => {
                    if open && self.index.dialogs.contains_key(&id) {
                        self.open_dialog(&id);
                    } else if !open {
                        self.overlays.retain(|overlay| overlay != &Overlay::Dialog(id.clone()));
                        self.rebuild_focus();
                    }
                }
                Follow::Handoff { form } => self.assist.handoff = Some(form),
            }
        }
        if self.assist.queue.is_empty() && !self.lanes.busy(&Purpose::Level) {
            if let Some(form) = self.assist.handoff.take() {
                self.focus_save(&form);
            }
        }
    }

    /// Fill a field as Assist: the key is a buffer key (a field's id, or `id#part`). A password, file, hidden or
    /// display field is never filled. Answers the field's title.
    pub fn fill(&mut self, key: &str, value: &str) -> Result<String, String> {
        let node = key.split('#').next().unwrap_or(key);
        let Some(field) = self.index.fields.get(node) else {
            return Err("it is not on this screen".to_string());
        };
        if matches!(
            field.widget,
            Widget::Password {} | Widget::File { .. } | Widget::Hidden { .. } | Widget::Display { .. }
        ) || !forms::field_keys(field).iter().any(|own| own == key)
        {
            return Err("Assist does not fill this field".to_string());
        }
        let title = field.title.clone();
        self.set_buffer(key, value.to_string());
        self.assist.filled.insert(key.to_string(), value.to_string());
        Ok(title)
    }

    /// The buffer key of a form's field by its request name; the form by its id, qvt id or name.
    fn fill_key(&self, form: &str, field: &str) -> Option<String> {
        self.index
            .forms
            .values()
            .filter(|info| info.id == form || info.dom_id == form || info.name == form)
            .flat_map(|info| info.fields.iter())
            .filter_map(|id| self.index.fields.get(id))
            .flat_map(forms::parameter_keys)
            .find(|(name, _)| name == field)
            .map(|(_, key)| key)
    }

    /// A hand-off: the form's Save takes the focus, the keys go to the screen, and the message line says what to do.
    fn focus_save(&mut self, form: &str) {
        let save = self
            .index
            .forms
            .values()
            .filter(|info| info.id == form || info.dom_id == form || info.name == form)
            .flat_map(|info| info.fields.iter())
            .find(|id| {
                matches!(
                    self.index.fields.get(*id).map(|field| &field.widget),
                    Some(Widget::Submit { .. })
                )
            })
            .cloned();
        let title = Some(self.model().screen.title.clone())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| form.to_string());
        if let Some(save) = save {
            self.rebuild_focus();
            self.focus_key(&save);
        }
        self.assist.focused = false;
        self.set_message(format!("Assist filled {title} — review, then Enter on Save"), false);
    }
}

/// A value of an approval's line: empty shows as `(empty)`.
fn show(value: &str) -> &str {
    if value.is_empty() {
        "(empty)"
    } else {
        value
    }
}
