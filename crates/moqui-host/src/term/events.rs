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

//! A session's last events, numbered (plans/Terminal-Fancy2.md §4.3): what a program driving it reads to know what
//! happened since it last looked (`find#TerminalEvents`) and what it waits on (`wait#TerminalSession`). They are kept
//! beside the session's `Info`, which is cloned for every list and state message.

use std::collections::VecDeque;

use super::track::Changes;

/// The events a session keeps; older ones are dropped first.
pub(crate) const KEPT: usize = 256;

/// One event: its number, when (millis), its kind and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Event {
    pub number: u64,
    pub at: i64,
    /// title, cwd, status, prompt, command, bell, notify, attach, detach, ended
    pub kind: &'static str,
    pub text: String,
    /// A finished command's exit status, an ended session's.
    pub status: Option<i64>,
}

impl Event {
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "number": self.number, "at": self.at, "kind": self.kind, "text": self.text, "status": self.status,
        })
    }
}

#[derive(Debug, Default)]
pub(crate) struct Events {
    kept: VecDeque<Event>,
    /// The number the next event takes; the first is 1.
    next: u64,
}

impl Events {
    pub fn push(&mut self, at: i64, kind: &'static str, text: impl Into<String>, status: Option<i64>) {
        self.next += 1;
        self.kept.push_back(Event {
            number: self.next,
            at,
            kind,
            text: text.into(),
            status,
        });
        while self.kept.len() > KEPT {
            self.kept.pop_front();
        }
    }

    /// The events a read of the program's output brought: its title, directory, status, prompt marks, a finished
    /// command, a bell or a notification.
    pub fn changes(&mut self, at: i64, changes: &Changes) {
        if let Some(title) = &changes.title {
            self.push(at, "title", title.clone(), None);
        }
        if let Some(cwd) = &changes.cwd {
            self.push(at, "cwd", cwd.clone(), None);
        }
        if let Some(status) = &changes.status {
            let text = match &changes.status_text {
                Some(text) if !text.is_empty() => format!("{status}: {text}"),
                _ => status.clone(),
            };
            self.push(at, "status", text, None);
        }
        for mark in &changes.marks {
            match mark {
                'A' => self.push(at, "prompt", "", None),
                'D' => self.push(at, "command", "", changes.exit_status.map(i64::from)),
                _ => {}
            }
        }
        if changes.bell {
            self.push(at, "bell", "", None);
        }
        if let Some((title, body)) = &changes.notify {
            let text = if body.is_empty() {
                title.clone()
            } else {
                format!("{title}: {body}")
            };
            self.push(at, "notify", text, None);
        }
    }

    /// The events after number `after`, oldest first.
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    pub fn after(&self, after: u64) -> Vec<Event> {
        self.kept.iter().filter(|event| event.number > after).cloned().collect()
    }

    /// The number of the last event, 0 before any.
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    pub fn last(&self) -> u64 {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::{Events, KEPT};
    use crate::term::track::Changes;

    #[test]
    fn events_are_numbered_kept_to_their_count_and_read_after_a_number() {
        let mut events = Events::default();
        let changes = Changes {
            title: Some("vim".to_string()),
            marks: vec!['D', 'A'],
            exit_status: Some(2),
            bell: true,
            ..Changes::default()
        };
        events.changes(5, &changes);
        let kinds: Vec<&str> = events.after(0).iter().map(|event| event.kind).collect();
        assert_eq!(kinds, ["title", "command", "prompt", "bell"]);
        assert_eq!(events.after(1)[0].status, Some(2));
        assert_eq!(events.last(), 4);
        for _ in 0..KEPT + 10 {
            events.push(6, "attach", "", None);
        }
        let kept = events.after(0);
        assert_eq!(kept.len(), KEPT);
        assert_eq!(kept.last().map(|event| event.number), Some(events.last()));
        assert!(events.after(events.last()).is_empty());
    }
}
