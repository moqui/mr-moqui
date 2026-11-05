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

//! `moqui.database.DatabaseAudit` (plans/Tools-Database.md §14.2): a row for what ran outside the entity facade, written
//! in a transaction of its own so it survives a rollback of what it records. A row that cannot be written is logged and
//! the action goes on.

use moqui_lib::object::Value;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;

pub const DATABASE_AUDIT: &str = "moqui.database.DatabaseAudit";

/// One audit row; the user, visit and instance are the current ones.
#[derive(Clone, Debug, Default)]
pub struct AuditEntry {
    /// The row's id when the caller names it (a SQL run's statement, `<runId>_<seq>`, so a cancel from another
    /// instance finds it); otherwise a sequenced one.
    pub audit_id: Option<String>,
    /// A `DatabaseAuditAction` enumeration: `DbaSql`, `DbaJob`, `DbaCancel`, `DbaTerminate`, `DbaGridSave`, `DbaLoad`,
    /// `DbaEtlRun`, `DbaStatsReset`.
    pub action: &'static str,
    /// `group:<name>` or `ext:<connectionId>`, when the action has one database.
    pub db: Option<String>,
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
#[derive(Clone, Debug, Default)]
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

/// Write `entry`, as the current user, in a transaction of its own; a row with its `audit_id` already there is
/// updated. A failure is logged, not returned.
pub fn record(facade: &EntityFacade, entry: &AuditEntry) {
    record_as(facade, entry, &Actor::current());
}

/// [`record`] as `actor`, for a thread with no context entered.
pub fn record_as(facade: &EntityFacade, entry: &AuditEntry, actor: &Actor) {
    if let Err(err) = write(facade, entry, actor, true) {
        tracing::error!(error = %err, action = entry.action, "Could not write the DatabaseAudit row");
    }
}

/// Write the row of a statement that has started, with no duration yet, unless its row is there already (the
/// statement has ended). Called from another thread while the statement runs, so a cancel from another instance can
/// find the session (`detail_json` has `backendId`).
pub fn mark_running(facade: &EntityFacade, entry: &AuditEntry, actor: &Actor) {
    if let Err(err) = write(facade, entry, actor, false) {
        tracing::debug!(error = %err, "The running mark of a SQL statement was not written");
    }
}

fn write(
    facade: &EntityFacade,
    entry: &AuditEntry,
    actor: &Actor,
    replace: bool,
) -> Result<(), moqui_lib::error::Error> {
    let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
    let number = |value: Option<i64>| value.map_or(Value::Null, Value::I64);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(DATABASE_AUDIT)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let mut row = facade.make_value(DATABASE_AUDIT)?;
        match &entry.audit_id {
            Some(id) => row.set("auditId", Value::string(id)),
            None => row.set_sequenced_id_primary(facade)?,
        }
        row.set("userId", text(&actor.user_id));
        row.set("visitId", text(&actor.visit_id));
        row.set("instanceId", text(&actor.instance_id));
        row.set("db", text(&entry.db));
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
        if replace && entry.audit_id.is_some() {
            row.create_or_update(facade)
        } else if entry.audit_id.is_some() {
            row.create_if_missing(facade).map(|_| ())
        } else {
            row.create(facade)
        }
    })
}
