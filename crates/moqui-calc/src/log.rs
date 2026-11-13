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

//! The edit log of a saved document (plans/IronCalc-DB.md, editing together): what its editors changed since it was
//! saved, in one order for every room of it.
//!
//! A document is open in a room for each scope of its readers ([`crate::scope`]), on each server that has a reader of
//! it, and each room holds its own data. An edit locks the document's head row (`moqui.calc.CalcDocumentHead`), brings
//! its room up to the last edit, applies itself there and is written as the next row: so the rows are one order, and
//! every room applies them in it, each over its own data. A row holds the edits as the view sent them (a formula as
//! typed, so `Orders[amount]` is the table of the room that applies it) and never a value: the engine's own diffs carry
//! the cells they replaced, with values computed from one scope's data and indexes into its texts, so they are not
//! passed between rooms.
//!
//! Undo and redo are each person's own ([`undo_target`], [`redo_target`]): an undo is a row that names the edit it takes
//! back, and each room takes it back with what it took of that edit before applying it ([`crate::room::Room`]). An
//! edit at or before the saved one is not undone, as a room opened since has nothing of it.

use moqui_core::context::Error;
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::Value as Json;

const HEAD: &str = "moqui.calc.CalcDocumentHead";
const EDIT_ROW: &str = "moqui.calc.CalcDocumentEdit";
/// The most rows a room is brought forward by in one read; a room further behind is opened again.
pub const MAX_CATCH_UP: i64 = 5_000;
/// The rows at or before a save that stay, for a room still bringing itself up to them.
const KEPT_AFTER_SAVE: i64 = 1_000;

/// An edit as the view sent it.
pub const EDIT: &str = "edit";
/// The taking back of an edit or a redo (`target`).
pub const UNDO: &str = "undo";
/// An undone edit applied again (`target`: the undo).
pub const REDO: &str = "redo";
/// The saved version again: each room is opened anew.
pub const REVERT: &str = "revert";

/// A row of a document's log, or of a scratch room's own.
#[derive(Clone, Debug, PartialEq)]
pub struct LogRow {
    pub seq: i64,
    pub user_id: String,
    pub kind: String,
    pub ops: Vec<Json>,
    /// What an undo takes back, or the undo a redo takes back.
    pub target: Option<i64>,
    /// The undo or redo that took this row back.
    pub undone: Option<i64>,
}

/// The edit or redo of `user` an undo takes back: their newest after `base` not taken back already.
#[must_use]
pub fn undo_target<'a>(rows: &'a [LogRow], user: &str, base: i64) -> Option<&'a LogRow> {
    rows.iter()
        .rev()
        .filter(|row| row.seq > base && row.user_id == user && row.undone.is_none())
        .find(|row| row.kind == EDIT || row.kind == REDO)
}

/// The undo of `user` a redo takes back: their newest after `base` not redone, when they have made no edit since.
#[must_use]
pub fn redo_target<'a>(rows: &'a [LogRow], user: &str, base: i64) -> Option<&'a LogRow> {
    let newest_edit = rows
        .iter()
        .filter(|row| row.user_id == user && row.kind == EDIT)
        .map(|row| row.seq)
        .max()
        .unwrap_or(i64::MIN);
    rows.iter()
        .rev()
        .filter(|row| row.seq > base && row.seq > newest_edit && row.user_id == user)
        .find(|row| row.kind == UNDO && row.undone.is_none())
}

/// Whether `user` has an edit to undo, and an undo to redo.
#[must_use]
pub fn undo_state(rows: &[LogRow], user: &str, base: i64) -> (bool, bool) {
    (
        undo_target(rows, user, base).is_some(),
        redo_target(rows, user, base).is_some(),
    )
}

/// A document's head row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Head {
    pub last_seq: i64,
    pub saved_seq: i64,
    pub saved_version: Option<i64>,
}

fn number(row: &moqui_core::entity::EntityValue, name: &str) -> Option<i64> {
    row.get(name)
        .map(to_plain_string)
        .filter(|text| !text.is_empty())
        .and_then(|text| text.parse().ok())
}

fn head_of(row: &moqui_core::entity::EntityValue) -> Head {
    Head {
        last_seq: number(row, "lastSeq").unwrap_or(0),
        saved_seq: number(row, "savedSeq").unwrap_or(0),
        saved_version: number(row, "savedVersion"),
    }
}

fn row_of(row: &moqui_core::entity::EntityValue) -> LogRow {
    let ops = row
        .get("opsJson")
        .map(to_plain_string)
        .and_then(|text| serde_json::from_str::<Json>(&text).ok())
        .and_then(|json| json.as_array().cloned())
        .unwrap_or_default();
    LogRow {
        seq: number(row, "seq").unwrap_or(0),
        user_id: row.get("userId").map(to_plain_string).unwrap_or_default(),
        kind: row.get("kind").map(to_plain_string).unwrap_or_default(),
        ops,
        target: number(row, "targetSeq"),
        undone: number(row, "undoneSeq"),
    }
}

/// The head of `definition_id`'s log, read without a lock; None when nothing was ever edited.
///
/// # Errors
///
/// Returns the facade's error.
pub fn head(facade: &EntityFacade, definition_id: &str) -> Result<Option<Head>, Error> {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    Ok(facade
        .find(HEAD)?
        .eq("definitionId", Value::string(definition_id))
        .use_cache(false)
        .one()?
        .as_ref()
        .map(head_of))
}

/// The rows of `definition_id`'s log after `after`, in order, at most [`MAX_CATCH_UP`].
///
/// # Errors
///
/// Returns the facade's error.
pub fn rows_after(facade: &EntityFacade, definition_id: &str, after: i64) -> Result<Vec<LogRow>, Error> {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let found = facade
        .find(EDIT_ROW)?
        .eq("definitionId", Value::string(definition_id))
        .condition(moqui_core::entity::Condition::greater("seq", Value::I64(after)))
        .order_by("seq", false)
        .limit(MAX_CATCH_UP)
        .use_cache(false)
        .list()?;
    Ok(found.values().iter().map(row_of).collect())
}

/// The log of one document while its head row is locked ([`locked`]).
pub struct Locked<'a> {
    facade: &'a EntityFacade,
    definition_id: String,
    pub head: Head,
    user_id: String,
}

impl Locked<'_> {
    /// The rows after `after`, in order.
    ///
    /// # Errors
    ///
    /// Returns the facade's error.
    pub fn rows_after(&self, after: i64) -> Result<Vec<LogRow>, Error> {
        rows_after(self.facade, &self.definition_id, after)
    }

    /// Write the next row and move the head to it; its seq.
    ///
    /// # Errors
    ///
    /// Returns the facade's error.
    pub fn append(&mut self, kind: &str, ops: &[Json], target: Option<i64>) -> Result<i64, Error> {
        let seq = self.head.last_seq + 1;
        let mut row = self.facade.make_value(EDIT_ROW)?;
        row.set("definitionId", Value::string(&self.definition_id));
        row.set("seq", Value::I64(seq));
        row.set("kind", Value::string(kind));
        row.set(
            "opsJson",
            Value::string(serde_json::to_string(ops).map_err(|err| Error::new(err.to_string()))?),
        );
        row.set("targetSeq", target.map_or(Value::Null, Value::I64));
        row.set("userId", Value::string(&self.user_id));
        row.set("editDate", moqui_core::user::now_value());
        row.create(self.facade)?;
        self.head.last_seq = seq;
        Ok(seq)
    }

    /// Mark row `seq` taken back by row `by`.
    ///
    /// # Errors
    ///
    /// Returns the facade's error.
    pub fn mark_undone(&self, seq: i64, by: i64) -> Result<(), Error> {
        if let Some(mut row) = self
            .facade
            .find(EDIT_ROW)?
            .eq("definitionId", Value::string(&self.definition_id))
            .eq("seq", Value::I64(seq))
            .use_cache(false)
            .one()?
        {
            row.set("undoneSeq", Value::I64(by));
            row.update(self.facade)?;
        }
        Ok(())
    }

    /// The document was saved at `version` with every edit through `seq`: those are no longer undone, and the rows
    /// well before it are dropped (the recent ones stay for a room still bringing itself up to them).
    ///
    /// # Errors
    ///
    /// Returns the facade's error.
    pub fn saved(&mut self, seq: i64, version: Option<i64>) -> Result<(), Error> {
        self.facade
            .find(EDIT_ROW)?
            .eq("definitionId", Value::string(&self.definition_id))
            .condition(moqui_core::entity::Condition::less_equal(
                "seq",
                Value::I64(seq - KEPT_AFTER_SAVE),
            ))
            .delete_all()?;
        self.head.saved_seq = seq;
        self.head.saved_version = version;
        Ok(())
    }

    /// The saved version `version` holds, and no edit after it: every row is dropped and a revert row written, so the
    /// rooms open anew. Its seq.
    ///
    /// # Errors
    ///
    /// Returns the facade's error.
    pub fn begin_again(&mut self, version: Option<i64>) -> Result<i64, Error> {
        self.facade
            .find(EDIT_ROW)?
            .eq("definitionId", Value::string(&self.definition_id))
            .delete_all()?;
        let seq = self.append(REVERT, &[], None)?;
        self.head.saved_seq = seq;
        self.head.saved_version = version;
        Ok(seq)
    }
}

/// Run `body` with `definition_id`'s head row locked, in a transaction of its own, as `user_id`: edits of every room
/// and server are one order. The head is made the first time; what `body` changes of it is written when it returns.
///
/// # Errors
///
/// Returns `body`'s error, and the transaction is rolled back; or the facade's.
pub fn locked<T>(
    facade: &EntityFacade,
    definition_id: &str,
    user_id: &str,
    body: impl FnOnce(&mut Locked<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let tx = facade.transactions_for(HEAD)?;
    // the head made in a transaction of its own, so two servers that both make it do not fail the edit
    if head(facade, definition_id)?.is_none() {
        let made = tx.run(TransactionAttribute::ForceNew, None, || {
            let mut row = facade.make_value(HEAD)?;
            row.set("definitionId", Value::string(definition_id));
            row.set("lastSeq", Value::I64(0));
            row.set("savedSeq", Value::I64(0));
            row.create(facade)?;
            Ok(())
        });
        if let Err(err) = made {
            tracing::debug!(document = definition_id, error = %err, "A CalcGrid log head was made by another");
        }
    }
    let mut out = None;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let Some(mut row) = facade
            .find(HEAD)?
            .eq("definitionId", Value::string(definition_id))
            .for_update(true)
            .use_cache(false)
            .one()?
        else {
            return Err(moqui_lib::error::Error::new("The document's edit log is not readable"));
        };
        let mut locked = Locked {
            facade,
            definition_id: definition_id.to_string(),
            head: head_of(&row),
            user_id: user_id.to_string(),
        };
        let answer = body(&mut locked).map_err(|err| moqui_lib::error::Error::new(err.message()))?;
        row.set("lastSeq", Value::I64(locked.head.last_seq));
        row.set("savedSeq", Value::I64(locked.head.saved_seq));
        row.set(
            "savedVersion",
            locked.head.saved_version.map_or(Value::Null, Value::I64),
        );
        row.set("lastEditDate", moqui_core::user::now_value());
        row.set("lastEditUserId", Value::string(user_id));
        row.update(facade)?;
        out = Some(answer);
        Ok(())
    })
    .map_err(Error::from)?;
    out.ok_or_else(|| Error::new("The document's edit log was not written"))
}

/// Tell every server the document changed ([`moqui_core::notify::CALC_UPDATE`]): each brings its rooms of it up to
/// `seq` and tells their viewers. A failure is logged; a room is brought up at its next read anyway.
pub fn announce(facade: &EntityFacade, definition_id: &str, seq: i64, user_id: &str, kind: &str) {
    let body = serde_json::json!({"doc": definition_id, "seq": seq, "userId": user_id, "kind": kind});
    let sent = moqui_core::cluster::prepare_message(body)
        .and_then(|message| moqui_core::cluster::publish(facade, moqui_core::notify::CALC_UPDATE, &message));
    if let Err(err) = sent {
        tracing::warn!(document = definition_id, error = %err, "Could not announce a CalcGrid edit");
    }
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod tests;
