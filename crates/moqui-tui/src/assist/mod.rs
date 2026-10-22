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

//! The Assist pane (plans/LLM-A2A-Assist.md 4.12): the drawer of the browser shell in the terminal. [`state`] is the
//! reducer, this module the pane's own state (open, focus, the composer, follow), and `draw` (with the `ui` feature) the
//! column or overlay. The session's side (keys, the `/assist/*` calls, follow on the screen) is
//! `crate::session::assist`; docs/TUI.md "Assist" says what the user sees.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::link::LinkError;

#[cfg(feature = "ui")]
pub(crate) mod draw;
pub mod state;

pub use state::{Follow, Thread};

/// The pane's columns when it docks, and the room the canvas keeps beside it: it docks while the canvas stays md.
pub const PANE_WIDTH: u16 = 48;

/// Where the focus is in the pane: Tab cycles them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Area {
    #[default]
    Composer,
    Cards,
    Transcript,
}

/// A card waiting for the user, by its index in its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    Approval(usize),
    Question(usize),
    Handoff(usize),
}

/// What an Assist menu entry does (F10 in the pane, a question's options).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    New,
    History,
    Tasks,
    Agent,
    Follow,
    /// Open a conversation of the user's.
    Open(String),
    /// Start a new conversation with this agent.
    PickAgent(String),
    /// Answer a question with one of its options.
    Answer {
        run_id: String,
        question_id: String,
        choice: String,
    },
    /// Answer a question in a line of text.
    AnswerText {
        run_id: String,
        question_id: String,
    },
}

/// What `GET bootstrap` said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bootstrap {
    pub enabled: bool,
    pub reason: String,
    pub agent_id: String,
    /// Each agent the user may use: its id and name.
    pub agents: Vec<(String, String)>,
    pub screen_follow: bool,
    pub max_chars: usize,
}

impl Bootstrap {
    pub fn of(value: &serde_json::Value) -> Self {
        let text = |value: &serde_json::Value, name: &str| {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let features = value.get("features").cloned().unwrap_or_default();
        let flag = |name: &str| features.get(name).and_then(serde_json::Value::as_bool);
        let agents: Vec<(String, String)> = value
            .get("agents")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .map(|agent| (text(agent, "agentId"), text(agent, "name")))
            .filter(|(id, _)| !id.is_empty())
            .collect();
        let agent_id = Some(text(value, "defaultAgentId"))
            .filter(|id| !id.is_empty())
            .or_else(|| agents.first().map(|(id, _)| id.clone()))
            .unwrap_or_else(|| "assist".to_string());
        let max_chars = value
            .get("limits")
            .and_then(|limits| limits.get("maxMessageChars"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(16000);
        let enabled = flag("enabled").unwrap_or(true) && !agents.is_empty();
        Self {
            enabled,
            reason: if enabled {
                String::new()
            } else {
                "Assist is not available: no agent serves this user.".to_string()
            },
            agent_id,
            agents,
            screen_follow: flag("screenFollow").unwrap_or(true),
            max_chars,
        }
    }
}

/// The pane's state in the session.
#[derive(Clone, Debug, Default)]
pub struct Assist {
    /// The pane shows; with `focused` the keys go to it.
    pub open: bool,
    pub focused: bool,
    pub area: Area,
    pub composer: String,
    pub cursor: usize,
    /// The card the Cards area is on, and the option of a question card.
    pub card_at: usize,
    pub option_at: usize,
    /// Rows the transcript is scrolled up from its end.
    pub scroll: usize,
    pub thread: Thread,
    pub bootstrap: Option<Bootstrap>,
    /// The agent of the next new conversation.
    pub agent_id: String,
    client_id: String,
    sent: u64,
    /// The screen follows the agent (F10 › Follow turns it off).
    pub follow_off: bool,
    /// What follow still has to do once the screen asked for has come.
    pub queue: VecDeque<Follow>,
    /// A hand-off's form, whose Save takes the focus once the queue is done.
    pub handoff: Option<String>,
    /// A screen asked for by follow: its answer is not the user's navigation.
    pub follow_nav: bool,
    /// Fields Assist filled and the value each was given: tagged "Assist" while they hold it.
    pub filled: BTreeMap<String, String>,
    /// The run whose events were asked for last.
    pub polling: String,
}

static MADE: AtomicU64 = AtomicU64::new(0);

impl Assist {
    /// This client's id, made once per session: letters, digits and `-`.
    pub fn client_id(&mut self) -> String {
        if self.client_id.is_empty() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos());
            let n = MADE.fetch_add(1, Ordering::Relaxed);
            self.client_id = format!("tui-{:x}-{:x}-{n}", nanos & 0xffff_ffff_ffff, std::process::id());
        }
        self.client_id.clone()
    }

    pub fn client_id_now(&self) -> &str {
        &self.client_id
    }

    /// A new message's id, unique with the client.
    pub fn message_id(&mut self) -> String {
        self.sent += 1;
        format!("{}-m{}", self.client_id(), self.sent)
    }

    pub fn follow_ctx(&self) -> state::Ctx<'_> {
        state::Ctx {
            client_id: &self.client_id,
            follow_on: !self.follow_off,
        }
    }

    /// The cards waiting: approvals, then questions, then hand-offs.
    pub fn cards(&self) -> Vec<Card> {
        let thread = &self.thread;
        (0..thread.approvals.len())
            .map(Card::Approval)
            .chain((0..thread.questions.len()).map(Card::Question))
            .chain((0..thread.handoffs.len()).map(Card::Handoff))
            .collect()
    }

    pub fn card(&self) -> Option<Card> {
        let cards = self.cards();
        cards.get(self.card_at.min(cards.len().saturating_sub(1))).copied()
    }

    /// Tab: the composer, the cards (when there are any), the transcript.
    pub fn cycle(&mut self, forward: bool) {
        let mut areas = vec![Area::Composer];
        if !self.cards().is_empty() {
            areas.push(Area::Cards);
        }
        areas.push(Area::Transcript);
        let at = areas.iter().position(|area| *area == self.area).unwrap_or(0);
        let len = areas.len();
        self.area = areas[if forward { (at + 1) % len } else { (at + len - 1) % len }];
        self.option_at = 0;
    }
}

/// The text of a failed `/assist/*` call: the server's `errors` from its `{errorCode, errors, code}` answer, else the
/// link's own words.
pub fn error_text(err: &LinkError) -> String {
    let said = err.to_string();
    let body = said.split_once(": ").map_or(said.as_str(), |(_, rest)| rest);
    serde_json::from_str::<serde_json::Value>(body.trim())
        .ok()
        .and_then(|json| {
            json.get("errors")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or(said)
}

/// A 200 answer that is an error all the same.
pub fn answered_error(value: &serde_json::Value) -> Option<String> {
    value.get("errorCode")?;
    Some(
        value
            .get("errors")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Assist refused the call")
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_servers_error_text_shows() {
        let err = LinkError::Remote(
            r#"HTTP 409: {"errorCode": 409, "errors": "The approval was decided already", "code": "conflict"}"#.into(),
        );
        assert_eq!(error_text(&err), "The approval was decided already");
        assert_eq!(
            error_text(&LinkError::Missing("/assist/x".into())),
            "no screen at /assist/x"
        );
        let mut assist = Assist::default();
        let first = assist.message_id();
        assert!(first.starts_with("tui-") && first.len() <= 100, "{first}");
        assert_ne!(first, assist.message_id());
        assert!(first.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-'));
    }
}
