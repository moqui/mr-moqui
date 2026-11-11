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

//! Which session needs its user (plans/Terminal-Fancy.md §11.1): a session's status, in the words Rex's protocol and
//! herdr both use (working, blocked, done, idle, error), from the best source the session gives: the program's own
//! word (OSC 7501), else a bell or a notification (blocked, with its text, until the user types), else shell marks (OSC
//! 133: working while a command runs, then done or error by its exit status), else the output alone (working while it
//! flows; an agent's quiet reads as done, anything else's as idle). The server reads no screen, so it cannot tell a
//! question from an end: the text says "quiet". A change that wants the user, while nobody looks, is an attention.

use std::time::{Duration, Instant};

use super::track::Changes;

/// Output this recent is "working" for a session with no marks.
const FLOWING: Duration = Duration::from_secs(2);
/// An agent quiet this long is done.
const AGENT_QUIET: Duration = Duration::from_secs(5);

/// Why a session wants its user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Why {
    /// It asks for something: a permission, an answer, a sign-in.
    Blocked,
    /// It finished.
    Done,
    /// A command ran longer than the user's set time and finished.
    LongCommand,
    /// It rang the bell.
    Bell,
}

impl Why {
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::LongCommand => "command",
            Self::Bell => "bell",
        }
    }
}

/// An attention: why, and the text with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attention {
    pub why: Why,
    pub text: String,
    /// How long the command ran, for a long command.
    pub took: Option<Duration>,
}

#[derive(Debug, Default)]
pub(crate) struct Status {
    pub status: String,
    pub text: String,
    /// The program has spoken (OSC 7501): only it changes the status from now on.
    program: bool,
    /// Shell marks were seen: the output alone no longer says anything.
    marks: bool,
    /// Blocked by a bell or a notification, until the user types.
    rang: bool,
    last_output: Option<Instant>,
    command_started: Option<Instant>,
    /// An agent's session: quiet output reads as done.
    agent: bool,
}

impl Status {
    pub fn for_agent(agent: bool) -> Self {
        Self {
            agent,
            ..Self::default()
        }
    }

    fn set(&mut self, status: &str, text: &str) -> bool {
        if self.status == status && self.text == text {
            return false;
        }
        self.status = status.to_string();
        self.text = text.to_string();
        true
    }

    /// Output read at `now`, with what the tracker found in it. Answers whether the status changed and an attention.
    pub fn output(&mut self, changes: &Changes, now: Instant) -> (bool, Option<Attention>) {
        self.last_output = Some(now);
        let mut changed = false;
        let mut attention = None;
        if changes.program {
            self.program = true;
            let status = changes.status.clone().unwrap_or_default();
            let text = changes.status_text.clone().unwrap_or_default();
            let before = self.status.clone();
            changed |= self.set(&status, &text);
            if changed && before != status {
                attention = match status.as_str() {
                    "blocked" => Some(Attention {
                        why: Why::Blocked,
                        text,
                        took: None,
                    }),
                    "done" => Some(Attention {
                        why: Why::Done,
                        text,
                        took: None,
                    }),
                    _ => None,
                };
            }
            return (changed, attention);
        }
        if self.program {
            return (false, None);
        }
        if changes.marks.contains(&'C') {
            self.marks = true;
            self.rang = false;
            self.command_started = Some(now);
            changed |= self.set("working", "");
        }
        if changes.marks.contains(&'D') {
            self.marks = true;
            let status = changes.status.clone().unwrap_or_else(|| "done".to_string());
            let text = changes
                .exit_status
                .filter(|code| *code != 0)
                .map(|code| format!("exit {code}"))
                .unwrap_or_default();
            changed |= self.set(&status, &text);
            if let Some(started) = self.command_started.take() {
                attention = Some(Attention {
                    why: Why::LongCommand,
                    text: text.clone(),
                    took: Some(now - started),
                });
            }
        }
        if changes.marks.contains(&'A') {
            self.marks = true;
        }
        if let Some((title, body)) = &changes.notify {
            self.rang = true;
            let text = if title.is_empty() {
                body.clone()
            } else {
                format!("{title}: {body}")
            };
            changed |= self.set("blocked", &text);
            return (
                changed,
                Some(Attention {
                    why: Why::Blocked,
                    text,
                    took: None,
                }),
            );
        }
        if changes.bell {
            self.rang = true;
            changed |= self.set("blocked", "bell");
            return (
                changed,
                Some(Attention {
                    why: Why::Bell,
                    text: String::new(),
                    took: None,
                }),
            );
        }
        if !self.marks && !self.rang && self.status != "working" {
            changed |= self.set("working", "");
        }
        (changed, attention)
    }

    /// The user typed: a bell or a notification has had its answer.
    pub fn input(&mut self) -> bool {
        if std::mem::take(&mut self.rang) && !self.program {
            return self.set(if self.marks { "idle" } else { "working" }, "");
        }
        false
    }

    /// A tick with no output: a session with no marks that went quiet is idle, or done for an agent.
    pub fn tick(&mut self, now: Instant) -> (bool, Option<Attention>) {
        if self.program || self.marks || self.rang || self.status != "working" {
            return (false, None);
        }
        let quiet = self
            .last_output
            .map_or(Duration::MAX, |at| now.saturating_duration_since(at));
        if self.agent && quiet >= AGENT_QUIET {
            self.set("done", "quiet");
            return (
                true,
                Some(Attention {
                    why: Why::Done,
                    text: "quiet".to_string(),
                    took: None,
                }),
            );
        }
        if !self.agent && quiet >= FLOWING {
            self.set("idle", "");
            return (true, None);
        }
        (false, None)
    }
}

#[cfg(test)]
#[path = "attention_tests.rs"]
mod tests;
