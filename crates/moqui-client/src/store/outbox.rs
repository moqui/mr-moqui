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

//! The outbox: operations made with no network (a chat send or mark-read, a mail send or state change), kept until
//! the server takes them (plans/Mobile-Desktop.md §6: stable ids, explicit states). The id is the operation's
//! `clientRef`, which the server uses to take a retried send once. The outbox is never evicted.
//!
//! The flusher takes the due operations with [`Store::outbox_claim`], which marks them `sending` in the same statement,
//! sends them in the order given, and moves each on with [`Store::outbox_update`] or removes it. The order is kept per
//! target: an operation is not due while an earlier one of its kind and target (a room, a message) waits (held, being
//! sent, or a retry not due yet), so a later message never overtakes an earlier one. The flusher relies on this and
//! keeps no order of its own.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{failed, files_json, now_ms, Store};
use moqui_lib::error::Error;

/// How long an acknowledged operation is kept for its pushed echo (which removes it), before it is dropped anyway.
const ACKED_KEEP_MS: i64 = 10 * 60 * 1000;

/// Where an operation is on its way to the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutboxState {
    /// Waiting for its first send.
    Queued,
    /// Being sent now. A crash leaves it here: the next open makes it a retry.
    Sending,
    /// Taken by the server; kept until its echo is seen, or [`ACKED_KEEP_MS`], then removed.
    Acked,
    /// Its send failed in a way that may pass (no network, 408, 429, 5xx): sent again at its next time.
    Retry,
    /// Waiting for the user to sign in again (401, 403); [`Store::outbox_release_held`] makes it a retry.
    Held,
    /// Refused by the server; kept for the user to see, retry or discard. It does not hold up its target.
    Failed,
}

impl OutboxState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Sending => "sending",
            Self::Acked => "acked",
            Self::Retry => "retry",
            Self::Held => "held",
            Self::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "queued" => Self::Queued,
            "sending" => Self::Sending,
            "acked" => Self::Acked,
            "retry" => Self::Retry,
            "held" => Self::Held,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

/// An operation in the outbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxOp {
    /// The operation's `clientRef`.
    pub id: String,
    /// What it does, for the flusher to send it: `chat-send`, `chat-read`, `mail-send`, `mail-state`.
    pub kind: String,
    /// What its order is kept within, with its kind: the room of a chat send, the message of a mail state change.
    /// Empty for none.
    pub target: String,
    pub state: OutboxState,
    /// Sends tried so far; each claim counts one.
    pub attempts: u32,
    /// Not sent before this, ms since the epoch.
    pub next_ms: i64,
    /// When it was made, ms since the epoch; the outbox is sent oldest first.
    pub created_ms: i64,
    /// When its state last changed, ms since the epoch.
    pub updated_ms: i64,
    /// The operation's JSON.
    pub payload: String,
    /// The last failure, for the user.
    pub error: Option<String>,
    /// The store's files it sends ([`Store::save_file`]), removed with it.
    pub files: Vec<String>,
}

impl OutboxOp {
    /// A new operation, queued now.
    pub fn new(id: &str, kind: &str, target: &str, payload: &str) -> Self {
        let now = now_ms();
        Self {
            id: id.to_string(),
            kind: kind.to_string(),
            target: target.to_string(),
            state: OutboxState::Queued,
            attempts: 0,
            next_ms: now,
            created_ms: now,
            updated_ms: now,
            payload: payload.to_string(),
            error: None,
            files: Vec::new(),
        }
    }
}

/// A change of an operation's state after a send, or by the user (retry, hold).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxUpdate {
    pub state: OutboxState,
    pub attempts: u32,
    pub next_ms: i64,
    pub error: Option<String>,
}

const COLUMNS: &str = "id, kind, target, state, attempts, next_ms, created_ms, updated_ms, payload, error, files";

/// The operations due at `?1`: queued or to retry with their time come, and no earlier one of their kind and target
/// waiting (held, being sent, or queued or to retry and not due).
const DUE: &str = "state IN ('queued', 'retry') AND next_ms <= ?1 AND (target = '' OR NOT EXISTS (
        SELECT 1 FROM outbox earlier WHERE earlier.kind = outbox.kind AND earlier.target = outbox.target
            AND (earlier.created_ms < outbox.created_ms
                OR (earlier.created_ms = outbox.created_ms AND earlier.id < outbox.id))
            AND (earlier.state IN ('held', 'sending')
                OR (earlier.state IN ('queued', 'retry') AND earlier.next_ms > ?1))))";

impl Store {
    /// Add an operation; false, and the kept one unchanged, when one of its id is there already, so adding again
    /// after a crash does not queue it twice.
    ///
    /// # Errors
    ///
    /// Returns an error for a file name that is not plain, or when the row cannot be written.
    pub fn outbox_add(&self, op: &OutboxOp) -> Result<bool, Error> {
        let files = files_json(&op.files).map_err(|err| Error::new(format!("outbox {}: {err}", op.id)))?;
        let added = self
            .conn
            .execute(
                &format!(
                    "INSERT OR IGNORE INTO outbox ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
                ),
                params![
                    op.id,
                    op.kind,
                    op.target,
                    op.state.as_str(),
                    op.attempts,
                    op.next_ms,
                    op.created_ms,
                    op.updated_ms,
                    op.payload,
                    op.error,
                    files
                ],
            )
            .map_err(failed("outbox operation not kept"))?;
        Ok(added > 0)
    }

    /// The operation `id`; none when there is none, or it cannot be read (logged).
    pub fn outbox_get(&self, id: &str) -> Option<OutboxOp> {
        let found = self
            .conn
            .query_row(&format!("SELECT {COLUMNS} FROM outbox WHERE id = ?1"), [id], |row| {
                Ok(op_of(row))
            })
            .optional();
        match found {
            Ok(Some(Ok(op))) => Some(op),
            Ok(Some(Err(bad))) => {
                tracing::warn!("Client store {}: outbox {id} not read: {bad}", self.account);
                None
            }
            Ok(None) => None,
            Err(err) => {
                tracing::warn!("Client store {}: outbox {id} not read: {err}", self.account);
                None
            }
        }
    }

    /// All operations, oldest first. One that cannot be read is logged and left out.
    pub fn outbox_list(&self) -> Vec<OutboxOp> {
        self.ops(
            &format!("SELECT {COLUMNS} FROM outbox ORDER BY created_ms, id"),
            params![],
        )
    }

    /// The operations due at `now_ms`, oldest first, at most `limit`, without claiming them (to show; the flusher
    /// uses [`Self::outbox_claim`]).
    pub fn outbox_due(&self, now_ms: i64, limit: u32) -> Vec<OutboxOp> {
        self.ops(
            &format!("SELECT {COLUMNS} FROM outbox WHERE {DUE} ORDER BY created_ms, id LIMIT ?2"),
            params![now_ms, limit],
        )
    }

    /// Take the operations due at `now_ms` to send, oldest first, at most `limit`: each marked `sending` with one more
    /// attempt in the same statement, so two flushers never take one. Acknowledged operations past their keep are
    /// dropped first.
    pub fn outbox_claim(&self, now_ms: i64, limit: u32) -> Vec<OutboxOp> {
        self.expire_acked(now_ms);
        let mut ops = self.ops(
            &format!(
                "UPDATE outbox SET state = 'sending', attempts = attempts + 1, updated_ms = ?1
                WHERE id IN (SELECT id FROM outbox WHERE {DUE} ORDER BY created_ms, id LIMIT ?2)
                RETURNING {COLUMNS}"
            ),
            params![now_ms, limit],
        );
        ops.sort_by(|one, other| (one.created_ms, &one.id).cmp(&(other.created_ms, &other.id)));
        ops
    }

    /// Change an operation's state; false when there is no such operation.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be written.
    pub fn outbox_update(&self, id: &str, update: &OutboxUpdate) -> Result<bool, Error> {
        let changed = self
            .conn
            .execute(
                "UPDATE outbox SET state = ?1, attempts = ?2, next_ms = ?3, error = ?4, updated_ms = ?5 WHERE id = ?6",
                params![
                    update.state.as_str(),
                    update.attempts,
                    update.next_ms,
                    update.error,
                    now_ms(),
                    id
                ],
            )
            .map_err(failed("outbox operation not changed"))?;
        Ok(changed > 0)
    }

    /// Make the held operations due now, after the user signed in again; gives how many.
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be written.
    pub fn outbox_release_held(&self) -> Result<usize, Error> {
        let now = now_ms();
        self.conn
            .execute(
                "UPDATE outbox SET state = 'retry', next_ms = ?1, updated_ms = ?1 WHERE state = 'held'",
                [now],
            )
            .map_err(failed("held operations not released"))
    }

    /// Remove an operation (acknowledged, or discarded) and the files it sends that nothing else refers to; false when
    /// there was none. The files column is read as it is, so a row that does not parse as an operation still goes.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be removed.
    pub fn outbox_remove(&self, id: &str) -> Result<bool, Error> {
        let files: Option<String> = self
            .conn
            .query_row("DELETE FROM outbox WHERE id = ?1 RETURNING files", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(failed("outbox operation not removed"))?;
        let Some(files) = files else { return Ok(false) };
        self.release_files(&files);
        Ok(true)
    }

    /// At open: what a crash left `sending` is due again (the server takes a `clientRef` once, so sending it again is
    /// safe), and acknowledged operations past their keep go.
    pub(super) fn recover_outbox(&self) {
        let now = now_ms();
        if let Err(err) = self.conn.execute(
            "UPDATE outbox SET state = 'retry', next_ms = ?1, updated_ms = ?1 WHERE state = 'sending'",
            [now],
        ) {
            tracing::warn!(
                "Client store {}: operations being sent not recovered: {err}",
                self.account
            );
        }
        self.expire_acked(now);
    }

    fn expire_acked(&self, now_ms: i64) {
        if let Err(err) = self.conn.execute(
            "DELETE FROM outbox WHERE state = 'acked' AND updated_ms < ?1",
            [now_ms - ACKED_KEEP_MS],
        ) {
            tracing::warn!(
                "Client store {}: acknowledged operations not dropped: {err}",
                self.account
            );
        }
    }

    /// The operations of a query; a row that cannot be read (an unknown state, files that do not parse) is logged and
    /// left out.
    fn ops<P: rusqlite::Params>(&self, sql: &str, params: P) -> Vec<OutboxOp> {
        let read = self.rows("outbox", sql, params, |row| Ok(op_of(row)));
        let mut ops = Vec::with_capacity(read.len());
        for op in read {
            match op {
                Ok(op) => ops.push(op),
                Err(bad) => tracing::warn!("Client store {}: an outbox operation left out: {bad}", self.account),
            }
        }
        ops
    }
}

/// An operation of a row of [`COLUMNS`]; an error naming it for a column that does not read.
fn op_of(row: &rusqlite::Row<'_>) -> Result<OutboxOp, String> {
    let id: String = row.get(0).map_err(|err| err.to_string())?;
    let read = || -> Result<OutboxOp, String> {
        let state: String = row.get(3).map_err(|err| err.to_string())?;
        let files: String = row.get(10).map_err(|err| err.to_string())?;
        Ok(OutboxOp {
            id: id.clone(),
            kind: row.get(1).map_err(|err| err.to_string())?,
            target: row.get(2).map_err(|err| err.to_string())?,
            state: OutboxState::parse(&state).ok_or_else(|| format!("state {state} is not known"))?,
            attempts: row.get(4).map_err(|err| err.to_string())?,
            next_ms: row.get(5).map_err(|err| err.to_string())?,
            created_ms: row.get(6).map_err(|err| err.to_string())?,
            updated_ms: row.get(7).map_err(|err| err.to_string())?,
            payload: row.get(8).map_err(|err| err.to_string())?,
            error: row.get(9).map_err(|err| err.to_string())?,
            files: serde_json::from_str(&files).map_err(|err| format!("files: {err}"))?,
        })
    };
    read().map_err(|err| format!("{id}: {err}"))
}
