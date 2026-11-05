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

//! `moqui.database.DatabaseAudit` (plans/archive/Tools-Database.md §14.2): a row for what ran outside the entity
//! facade, written in a transaction of its own so it survives a rollback of what it records. A row that cannot be
//! written is logged and the action goes on.
//!
//! The rows are this module's to write: the read fence does not allow the entity (a fenced agent could otherwise
//! change or delete audit rows), so the writes here lift it ([`moqui_core::entity::fence::lift`]), which is the trusted
//! internal path; a sim preview stays fenced. Every id is the server's ([`new_id`]); a row is updated only while it is
//! its writer's running mark (same user, no duration yet).

use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;

pub const DATABASE_AUDIT: &str = "moqui.database.DatabaseAudit";

/// One audit row; the user, visit and instance are the [`Actor`]'s.
#[derive(Clone, Debug, Default)]
pub struct AuditEntry {
    /// The row's id, from [`new_id`], when the caller wrote its running mark first; otherwise a new one.
    pub audit_id: Option<String>,
    /// A `DatabaseAuditAction` enumeration: `DbaSql`, `DbaJob`, `DbaCancel`, `DbaTerminate`, `DbaGridSave`, `DbaLoad`,
    /// `DbaEtlRun`, `DbaStatsReset`.
    pub action: &'static str,
    /// `group:<name>` or `ext:<connectionId>`, when the action has one database.
    pub db: Option<String>,
    /// A SQL run's id and the statement's place in its script.
    pub run_id: Option<String>,
    pub statement_seq: Option<i64>,
    pub statement_text: Option<String>,
    pub is_change: bool,
    pub rows_returned: Option<i64>,
    pub rows_affected: Option<i64>,
    /// Empty while the statement runs (a row [`mark_running`] wrote).
    pub duration_ms: Option<i64>,
    pub error_text: Option<String>,
    pub detail_json: Option<String>,
    /// When it started; now when not given.
    pub start_time: Option<jiff::Timestamp>,
}

/// The current user, visit and instance, read on the thread whose context is entered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Actor {
    pub user_id: Option<String>,
    pub visit_id: Option<String>,
    pub instance_id: Option<String>,
}

impl Actor {
    #[must_use]
    pub fn current() -> Self {
        Self {
            user_id: moqui_core::user::current_user_id().ok().flatten(),
            visit_id: moqui_core::user::visit_id().ok().flatten(),
            instance_id: ExecutionContext::with_current(|ec| moqui_core::drain::instance_id(ec.factory())),
        }
    }
}

/// A new row id, the server's.
///
/// # Errors
///
/// When the sequence cannot be read.
pub fn new_id(facade: &EntityFacade) -> Result<String, moqui_lib::error::Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let _lift = moqui_core::entity::fence::lift();
    let mut row = facade.make_value(DATABASE_AUDIT)?;
    row.set_sequenced_id_primary(facade)?;
    Ok(row.get("auditId").map(to_plain_string).unwrap_or_default())
}

/// Write `entry` as the current user. See [`record_as`].
pub fn record(facade: &EntityFacade, entry: &AuditEntry) {
    record_as(facade, entry, &Actor::current());
}

/// Write `entry` as `actor`, in a transaction of its own: a new row, or with `audit_id` the actor's own running mark
/// completed (any other row of that id is left as it is and a new row written). A failure is logged, not returned.
pub fn record_as(facade: &EntityFacade, entry: &AuditEntry, actor: &Actor) {
    if let Err(err) = write(facade, entry, actor, Mode::Complete) {
        tracing::error!(error = %err, action = entry.action, "Could not write the DatabaseAudit row");
    }
}

/// Write the row of a statement that has started, with no duration yet, unless the row is there already (the
/// statement has ended). Called from another thread while the statement runs, so a cancel from another instance can
/// find the session (`detail_json` has `backendId` and `backendStart`).
pub fn mark_running(facade: &EntityFacade, entry: &AuditEntry, actor: &Actor) {
    if let Err(err) = write(facade, entry, actor, Mode::Running) {
        tracing::debug!(error = %err, "The running mark of a SQL statement was not written");
    }
}

/// Mark the rows `ids` (the actor's) as rolled back: `rolledBack` in their `detailJson`.
pub fn mark_rolled_back(facade: &EntityFacade, ids: &[String], actor: &Actor) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let _lift = moqui_core::entity::fence::lift();
    let result = facade.transactions_for(DATABASE_AUDIT).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            for id in ids {
                let Some(mut row) = facade.find(DATABASE_AUDIT)?.eq("auditId", Value::string(id)).one()? else {
                    continue;
                };
                if row.get("userId").map(to_plain_string) != actor.user_id {
                    continue;
                }
                let detail = row.get("detailJson").map(to_plain_string).unwrap_or_default();
                let mut parsed: serde_json::Value =
                    serde_json::from_str(&detail).unwrap_or_else(|_| serde_json::json!({}));
                if let Some(map) = parsed.as_object_mut() {
                    map.insert("rolledBack".to_string(), serde_json::Value::Bool(true));
                }
                row.set("detailJson", Value::String(parsed.to_string()));
                row.update(facade)?;
            }
            Ok(())
        })
    });
    if let Err(err) = result {
        tracing::error!(error = %err, "Could not mark DatabaseAudit rows rolled back");
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Running,
    Complete,
}

fn write(facade: &EntityFacade, entry: &AuditEntry, actor: &Actor, mode: Mode) -> Result<(), moqui_lib::error::Error> {
    let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
    let number = |value: Option<i64>| value.map_or(Value::Null, Value::I64);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let _lift = moqui_core::entity::fence::lift();
    let tx = facade.transactions_for(DATABASE_AUDIT)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let existing = match &entry.audit_id {
            Some(id) => facade.find(DATABASE_AUDIT)?.eq("auditId", Value::string(id)).one()?,
            None => None,
        };
        let own_mark = existing.as_ref().is_some_and(|row| {
            row.get("userId").map(to_plain_string) == actor.user_id
                && row.get("durationMs").is_none_or(|value| matches!(value, Value::Null))
        });
        if mode == Mode::Running && existing.is_some() {
            return Ok(());
        }
        let mut row = facade.make_value(DATABASE_AUDIT)?;
        match (&entry.audit_id, &existing) {
            (Some(id), None) => row.set("auditId", Value::string(id)),
            (Some(_), Some(_)) if own_mark => row.set(
                "auditId",
                existing
                    .as_ref()
                    .and_then(|row| row.get("auditId").cloned())
                    .unwrap_or(Value::Null),
            ),
            _ => row.set_sequenced_id_primary(facade)?,
        }
        row.set("userId", text(&actor.user_id));
        row.set("visitId", text(&actor.visit_id));
        row.set("instanceId", text(&actor.instance_id));
        row.set("db", text(&entry.db));
        row.set("runId", text(&entry.run_id));
        row.set("statementSeq", number(entry.statement_seq));
        row.set("actionEnumId", Value::string(entry.action));
        row.set("statementText", text(&entry.statement_text));
        row.set("isChange", Value::string(if entry.is_change { "Y" } else { "N" }));
        row.set("rowsReturned", number(entry.rows_returned));
        row.set("rowsAffected", number(entry.rows_affected));
        row.set("durationMs", number(entry.duration_ms));
        row.set("errorText", text(&entry.error_text));
        row.set(
            "startTime",
            Value::Timestamp(entry.start_time.unwrap_or_else(jiff::Timestamp::now)),
        );
        row.set("detailJson", text(&entry.detail_json));
        if existing.is_some() && own_mark && mode == Mode::Complete {
            row.update(facade)
        } else {
            row.create(facade)
        }
    })
}
