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

//! The change feed (plans/agent-code/02-Orchestration.md section 1, T3's shell stream sequence): one
//! `AgentCodeChange` row per change a command makes, in the command's transaction. The shell stream reads the rows
//! after its cursor, so a client that reconnects resumes from its last `changeSeq`.
//!
//! Ordering: `changeSeq` comes from the entity sequence with a bank of one, so each node takes the next number from
//! the database. Two transactions can still commit their numbers out of order (5 is taken first and committed last).
//! A [`Cursor`] therefore re-reads a window of [`REREAD_WINDOW`] numbers below its top and skips the ones it has seen,
//! so a late commit inside the window is not lost. A number whose transaction rolled back is never filled; the
//! cursor does not wait for it.
//!
//! Wake-up: after a change commits, [`wake`] calls every live [`on_change`] waker of the facade, so a stream reads at
//! once instead of on its next ping. Another node's commits are read on the stream's ping (cluster notify comes with
//! cluster forwarding, phase P6).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, Weak};

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::json;

use crate::support::{int_of, iso, json_of, json_text, millis_of, now_millis, stamp, text_of};

pub(crate) const CHANGE: &str = "moqui.agent.code.AgentCodeChange";

/// How many numbers below its top a cursor reads again for late commits.
pub const REREAD_WINDOW: i64 = 256;

/// The most rows one read returns; a stream reads again while a read is full.
pub const READ_LIMIT: i64 = 500;

/// One change a command makes.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeSpec {
    /// The T3 shell event type, such as `thread.upserted` or `thread.removed`.
    pub change_type: String,
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    /// The one user the change is for; `None` = every Agent Code user.
    pub user_id: Option<String>,
    pub payload: serde_json::Value,
}

/// A stored change, as the shell stream sends it.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub seq: i64,
    pub change_type: String,
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    pub user_id: Option<String>,
    pub payload: serde_json::Value,
    pub date: i64,
}

impl Change {
    fn from_row(row: &EntityValue) -> Self {
        Self {
            seq: int_of(row, "changeSeq").unwrap_or_default(),
            change_type: text_of(row, "changeType").unwrap_or_default(),
            conversation_id: text_of(row, "conversationId"),
            project_id: text_of(row, "projectId"),
            user_id: text_of(row, "userId"),
            payload: json_of(row, "payloadJson"),
            date: millis_of(row, "changeDate").unwrap_or_default(),
        }
    }

    /// The stream item of the change.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "sequence": self.seq,
            "type": self.change_type,
            "threadId": self.conversation_id,
            "projectId": self.project_id,
            "payload": self.payload,
            "occurredAt": iso(self.date),
        })
    }
}

/// Append `spec` in the open transaction and return its number.
///
/// # Errors
///
/// When the sequence or the insert fails.
pub fn append(facade: &EntityFacade, spec: &ChangeSpec) -> Result<i64, Error> {
    let seq_text = moqui_core::entity::sequenced_id_primary(facade, CHANGE, None, Some(1))?;
    let seq: i64 = seq_text
        .parse()
        .map_err(|_| Error::new(format!("the change sequence gave {seq_text}, not a number")))?;
    let mut row = facade.make_value(CHANGE)?;
    row.set("changeSeq", Value::I64(seq));
    row.set("changeType", Value::string(&spec.change_type));
    if let Some(id) = &spec.conversation_id {
        row.set("conversationId", Value::string(id));
    }
    if let Some(id) = &spec.project_id {
        row.set("projectId", Value::string(id));
    }
    if let Some(id) = &spec.user_id {
        row.set("userId", Value::string(id));
    }
    row.set("payloadJson", Value::string(json_text(&spec.payload)));
    row.set("changeDate", stamp(now_millis()));
    row.create(facade)?;
    wake_after_commit(facade);
    Ok(seq)
}

/// The highest change number stored, 0 when there is none.
///
/// # Errors
///
/// When the find fails.
pub fn latest(facade: &EntityFacade) -> Result<i64, Error> {
    let row = facade
        .find(CHANGE)?
        .order_by("changeSeq", true)
        .limit(1)
        .disable_authz()
        .list()?;
    let seq = row.iter().next().and_then(|row| int_of(row, "changeSeq")).unwrap_or(0);
    Ok(seq)
}

/// A shell stream's place in the feed: the numbers at or below `floor` are read (or in the snapshot); above it,
/// `seen` holds the ones read, so a late commit between them is still sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursor {
    floor: i64,
    top: i64,
    seen: BTreeSet<i64>,
}

impl Cursor {
    /// A cursor after `seq`: the rows at or below it count as read.
    #[must_use]
    pub fn after(seq: i64) -> Self {
        let seq = seq.max(0);
        Self {
            floor: seq,
            top: seq,
            seen: BTreeSet::new(),
        }
    }

    /// The highest number read.
    #[must_use]
    pub const fn top(&self) -> i64 {
        self.top
    }

    /// The changes for `user_id` that this cursor has not read, oldest first, at most [`READ_LIMIT`]; the second value
    /// is true when there may be more.
    ///
    /// # Errors
    ///
    /// When the find fails.
    pub fn read(&mut self, facade: &EntityFacade, user_id: &str) -> Result<(Vec<Change>, bool), Error> {
        let rows = facade
            .find(CHANGE)?
            .condition(Condition::greater("changeSeq", Value::I64(self.floor)))
            .condition(Condition::or(vec![
                Condition::is_null("userId"),
                Condition::equals("userId", Value::string(user_id)),
            ]))
            .order_by("changeSeq", false)
            .limit(READ_LIMIT)
            .disable_authz()
            .list()?;
        let found = rows.len();
        let changes = self.take(rows.iter().map(Change::from_row));
        // a full read that gave nothing new is the window itself; more rows come only above it
        let full = i64::try_from(found).unwrap_or(i64::MAX) >= READ_LIMIT && !changes.is_empty();
        Ok((changes, full))
    }

    /// The changes of `found` this cursor has not read; it moves the top, and the floor up to the window below it.
    fn take(&mut self, found: impl Iterator<Item = Change>) -> Vec<Change> {
        let mut changes = Vec::new();
        for change in found {
            if change.seq <= self.floor || !self.seen.insert(change.seq) {
                continue;
            }
            self.top = self.top.max(change.seq);
            changes.push(change);
        }
        self.floor = self.floor.max(self.top - REREAD_WINDOW);
        self.seen = self.seen.split_off(&(self.floor + 1));
        changes
    }
}

#[derive(Default)]
struct Wakers(Mutex<Vec<Weak<dyn Fn() + Send + Sync>>>);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Call `wake` after each change this node commits on `facade`, while the caller holds the `Arc`.
pub fn on_change(facade: &EntityFacade, wake: &Arc<dyn Fn() + Send + Sync>) {
    facade.with_slot(Wakers::default, |wakers| {
        let mut list = lock(&wakers.0);
        list.retain(|waker| waker.strong_count() > 0);
        list.push(Arc::downgrade(wake));
    });
}

/// Call the live wakers of `facade`.
pub fn wake(facade: &EntityFacade) {
    let wakers: Vec<_> = facade.with_slot_read(Wakers::default, |wakers| {
        lock(&wakers.0).iter().filter_map(Weak::upgrade).collect()
    });
    for wake in wakers {
        wake();
    }
}

fn wake_after_commit(facade: &EntityFacade) {
    let open = facade
        .transactions_for(CHANGE)
        .ok()
        .filter(|tx| tx.in_transaction());
    match open {
        Some(tx) => {
            let facade = facade.clone();
            if let Err(err) = tx.on_commit(move || wake(&facade)) {
                tracing::debug!("agent code: no wake after commit: {err}");
            }
        }
        None => wake(facade),
    }
}

#[cfg(test)]
#[path = "change_tests.rs"]
mod tests;
