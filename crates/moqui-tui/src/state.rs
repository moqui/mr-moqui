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

//! Client state with no screen in it: the request lanes, the edit buffers, and the confirmations.

use std::collections::{BTreeMap, BTreeSet};

use crate::link::LinkCall;

/// Why a request was sent. Each purpose has its own lane (section 3.6).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Purpose {
    /// The leaf level of the screen.
    Level,
    /// A parent level, by its path.
    Parent(String),
    Transition,
    /// The options of a drop-down, by field id.
    Options(String),
    Shell,
    Login,
    /// The body of a dynamic dialog or container, by node id.
    Fragment(String),
    /// A display's value from its value url, by field id.
    ValueUrl(String),
    /// The palette's search.
    Palette,
    /// The title line's counters.
    Counts,
    /// A tree node's children, by tree id and node path.
    Tree(String),
    /// A user preference written (the drawer's state).
    Preference,
    /// The screen's documents for the help.
    Help,
    /// An export: a file to save, or text to read.
    Export,
    /// The data of a component, by node id.
    Component(String),
    /// The menu data of the screen: its documents and saved finds.
    Menu,
    /// An Assist call, by its lane: one per call kind, a turn and a decision each their own.
    Assist(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub purpose: Purpose,
    pub call: LinkCall,
}

#[derive(Clone, Debug, Default)]
struct Lane {
    inflight: Option<u64>,
    pending: Option<Request>,
}

/// At most one request in flight per purpose, and only the newest one waiting. A held PgDn at key-repeat speed sends
/// one page request, keeps the latest as pending, and drops the ones between, so it never fills the server's request
/// queue. A late answer to a superseded request is dropped.
#[derive(Clone, Debug, Default)]
pub struct Lanes {
    next: u64,
    lanes: BTreeMap<Purpose, Lane>,
    outbox: Vec<Request>,
}

/// What to do with an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finished {
    /// The newest request of its purpose: apply it.
    Apply(Purpose),
    /// A newer request of its purpose was waiting and is now sent: drop this answer.
    Superseded,
    /// Not a request this session sent, or one already answered.
    Unknown,
}

impl Lanes {
    /// Queue a call. With one of its purpose in flight it waits, replacing any call waiting before it. A transition or
    /// a login never waits: a second one while one runs is refused, so a submit is never sent twice.
    /// Returns the id the call goes out under, or None when it was refused.
    pub fn push(&mut self, purpose: Purpose, call: LinkCall) -> Option<u64> {
        self.next += 1;
        let request = Request {
            id: self.next,
            purpose: purpose.clone(),
            call,
        };
        let single = matches!(purpose, Purpose::Transition | Purpose::Login);
        let lane = self.lanes.entry(purpose).or_default();
        let id = request.id;
        if lane.inflight.is_some() {
            if single {
                return None;
            }
            lane.pending = Some(request);
            return Some(id);
        }
        lane.inflight = Some(id);
        self.outbox.push(request);
        Some(id)
    }

    /// The requests to send now, in the order they were queued.
    pub fn take(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.outbox)
    }

    pub fn finish(&mut self, id: u64) -> Finished {
        let Some((purpose, lane)) = self.lanes.iter_mut().find(|(_, lane)| lane.inflight == Some(id)) else {
            return Finished::Unknown;
        };
        let purpose = purpose.clone();
        lane.inflight = None;
        if let Some(next) = lane.pending.take() {
            lane.inflight = Some(next.id);
            self.outbox.push(next);
            return Finished::Superseded;
        }
        Finished::Apply(purpose)
    }

    /// Drop what waits and forget what is in flight for `purpose`, so its answer is not applied.
    pub fn cancel(&mut self, purpose: &Purpose) {
        self.lanes.remove(purpose);
        self.outbox.retain(|request| &request.purpose != purpose);
    }

    pub fn busy(&self, purpose: &Purpose) -> bool {
        self.lanes.get(purpose).is_some_and(|lane| lane.inflight.is_some())
    }

    /// A level, a transition, a login, a fragment or an export runs: the title line shows the spinner. The background
    /// calls (counters, suggestions) do not.
    pub fn any_busy(&self) -> bool {
        self.lanes.iter().any(|(purpose, lane)| {
            lane.inflight.is_some()
                && matches!(
                    purpose,
                    Purpose::Level | Purpose::Transition | Purpose::Login | Purpose::Fragment(_) | Purpose::Export
                )
        })
    }

    /// Requests in flight and waiting, for tests and the busy check.
    pub fn counts(&self) -> (usize, usize) {
        let inflight = self.lanes.values().filter(|lane| lane.inflight.is_some()).count();
        let pending = self.lanes.values().filter(|lane| lane.pending.is_some()).count();
        (inflight, pending)
    }
}

/// The edit buffers of every field on the screen, by key: a field id, or `id#part` for the parts of a find field
/// (`#from`, `#thru`, `#op`). `originals` are the values the server sent; a key is dirty while its buffer differs.
#[derive(Clone, Debug, Default)]
pub struct Edits {
    buffers: BTreeMap<String, String>,
    originals: BTreeMap<String, String>,
    dirty: BTreeSet<String>,
    marks: BTreeMap<String, String>,
}

impl Edits {
    pub fn get(&self, key: &str) -> &str {
        self.buffers.get(key).map(String::as_str).unwrap_or("")
    }

    pub fn set(&mut self, key: &str, value: String) {
        if self.originals.get(key).is_some_and(|original| original == &value) {
            self.dirty.remove(key);
        } else {
            self.dirty.insert(key.to_string());
        }
        self.buffers.insert(key.to_string(), value);
        self.marks.remove(key);
    }

    pub fn is_dirty(&self, key: &str) -> bool {
        self.dirty.contains(key)
    }

    /// The value the server sent for a key.
    pub fn original(&self, key: &str) -> &str {
        self.originals.get(key).map(String::as_str).unwrap_or("")
    }

    pub fn any_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    pub fn dirty_keys(&self) -> impl Iterator<Item = &String> {
        self.dirty.iter()
    }

    /// The values a new model sends. With `keep`, keys still dirty keep the user's text; every other key takes the
    /// model's value, which a reload may have changed.
    pub fn load(&mut self, fresh: BTreeMap<String, String>, keep: bool) {
        let old = std::mem::take(&mut self.buffers);
        let old_dirty = std::mem::take(&mut self.dirty);
        self.buffers.clone_from(&fresh);
        self.originals = fresh;
        if keep {
            for key in old_dirty {
                if let Some(value) = old.get(&key) {
                    if self.originals.get(&key) != Some(value) {
                        self.buffers.insert(key.clone(), value.clone());
                        self.dirty.insert(key);
                    }
                }
            }
        } else {
            self.marks.clear();
        }
    }

    /// Forget the edits of these keys, as after their form was saved.
    pub fn drop_keys<'a>(&mut self, keys: impl IntoIterator<Item = &'a String>) {
        for key in keys {
            self.dirty.remove(key);
            if let Some(original) = self.originals.get(key) {
                self.buffers.insert(key.clone(), original.clone());
            }
        }
    }

    pub fn reset(&mut self) {
        self.buffers.clone_from(&self.originals);
        self.dirty.clear();
        self.marks.clear();
    }

    pub fn mark(&self, key: &str) -> Option<&str> {
        self.marks.get(key).map(String::as_str)
    }

    pub fn marks(&self) -> &BTreeMap<String, String> {
        &self.marks
    }

    pub fn set_marks(&mut self, marks: impl IntoIterator<Item = (String, String)>) {
        self.marks = marks.into_iter().collect();
    }

    pub fn clear_marks(&mut self) {
        self.marks.clear();
    }
}

/// What a confirmation does when it is answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Then {
    Navigate(String),
    Back,
    Quit,
    /// Submit the form holding this field or button.
    Submit(String),
    /// Run a transition link by its node id.
    FollowLink(String),
    /// Only close: an Esc that leaves the form's edits.
    Nothing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    /// Unsaved edits: Stay, Discard or Save, starting at Stay.
    Leave,
    /// A `confirmation` on a button or link: No or Yes, starting at No.
    Ask(String),
    /// An Assist approval: its summary and "Field: current → proposed" lines; Approve, Edit in screen (when offered)
    /// or Reject.
    Approval(AssistApproval),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistApproval {
    pub approval_id: String,
    pub lines: Vec<String>,
    pub can_edit: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirm {
    pub kind: ConfirmKind,
    pub then: Then,
    pub at: usize,
}

impl Confirm {
    pub fn leave(then: Then) -> Self {
        Self {
            kind: ConfirmKind::Leave,
            then,
            at: 0,
        }
    }

    pub fn ask(text: String, then: Then) -> Self {
        Self {
            kind: ConfirmKind::Ask(text),
            then,
            at: 0,
        }
    }

    pub fn choices(&self) -> &'static [&'static str] {
        match &self.kind {
            ConfirmKind::Leave => &["Stay", "Discard", "Save"],
            ConfirmKind::Ask(_) => &["No", "Yes"],
            ConfirmKind::Approval(card) if card.can_edit => &["Approve", "Edit in screen", "Reject"],
            ConfirmKind::Approval(_) => &["Approve", "Reject"],
        }
    }

    pub fn choice(&self) -> &'static str {
        self.choices().get(self.at).copied().unwrap_or("Stay")
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.choices().len() as isize;
        self.at = (self.at as isize + delta).rem_euclid(len) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(n: u32) -> LinkCall {
        LinkCall::Options {
            url: "/states".to_string(),
            parameters: BTreeMap::from([("n".to_string(), n.to_string())]),
        }
    }

    #[test]
    fn a_held_key_sends_one_request_and_keeps_only_the_newest() {
        let mut lanes = Lanes::default();
        for n in 0..20 {
            lanes.push(Purpose::Level, options(n));
        }
        let sent = lanes.take();
        assert_eq!(sent.len(), 1);
        assert_eq!(lanes.counts(), (1, 1));
        assert_eq!(lanes.finish(sent[0].id), Finished::Superseded);
        let next = lanes.take();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].call, options(19));
        assert_eq!(lanes.finish(next[0].id), Finished::Apply(Purpose::Level));
        assert_eq!(lanes.counts(), (0, 0));
        assert_eq!(lanes.finish(next[0].id), Finished::Unknown);
    }

    #[test]
    fn a_second_submit_while_one_runs_is_refused() {
        let mut lanes = Lanes::default();
        assert!(lanes.push(Purpose::Transition, options(1)).is_some());
        assert!(lanes.push(Purpose::Transition, options(2)).is_none());
        assert_eq!(lanes.take().len(), 1);
        assert!(lanes.any_busy());
    }

    #[test]
    fn edits_keep_dirty_text_across_a_reload() {
        let mut edits = Edits::default();
        edits.load(
            BTreeMap::from([("a".into(), "1".into()), ("b".into(), "2".into())]),
            false,
        );
        edits.set("a", "9".to_string());
        assert!(edits.is_dirty("a"));
        edits.load(
            BTreeMap::from([("a".into(), "1".into()), ("b".into(), "3".into())]),
            true,
        );
        assert_eq!(edits.get("a"), "9");
        assert_eq!(edits.get("b"), "3");
        edits.set("a", "1".to_string());
        assert!(!edits.any_dirty());
    }
}
