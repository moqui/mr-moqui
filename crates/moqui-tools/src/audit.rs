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

/// One audit row; the user, visit, instance and start time are the current ones.
#[derive(Clone, Debug, Default)]
pub struct AuditEntry {
    /// A `DatabaseAuditAction` enumeration: `DbaSql`, `DbaJob`, `DbaCancel`, `DbaTerminate`, `DbaGridSave`, `DbaLoad`,
    /// `DbaEtlRun`, `DbaStatsReset`.
    pub action: &'static str,
    /// `group:<name>` or `ext:<connectionId>`, when the action has one database.
    pub db: Option<String>,
    pub statement_text: Option<String>,
    pub is_change: bool,
    pub rows_returned: Option<i64>,
    pub rows_affected: Option<i64>,
    pub duration_ms: Option<i64>,
    pub error_text: Option<String>,
    pub detail_json: Option<String>,
}

/// Write `entry`, as the current user, in a transaction of its own. A failure is logged, not returned.
pub fn record(facade: &EntityFacade, entry: &AuditEntry) {
    if let Err(err) = write(facade, entry) {
        tracing::error!(error = %err, action = entry.action, "Could not write the DatabaseAudit row");
    }
}

fn write(facade: &EntityFacade, entry: &AuditEntry) -> Result<(), moqui_lib::error::Error> {
    let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
    let number = |value: Option<i64>| value.map_or(Value::Null, Value::I64);
    let user_id = moqui_core::user::current_user_id().ok().flatten();
    let visit_id = moqui_core::user::visit_id().ok().flatten();
    let instance_id = ExecutionContext::with_current(|ec| moqui_core::drain::instance_id(ec.factory()));
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(DATABASE_AUDIT)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let mut row = facade.make_value(DATABASE_AUDIT)?;
        row.set_sequenced_id_primary(facade)?;
        row.set("userId", text(&user_id));
        row.set("visitId", text(&visit_id));
        row.set("instanceId", text(&instance_id));
        row.set("db", text(&entry.db));
        row.set("actionEnumId", Value::string(entry.action));
        row.set("statementText", text(&entry.statement_text));
        row.set("isChange", Value::string(if entry.is_change { "Y" } else { "N" }));
        row.set("rowsReturned", number(entry.rows_returned));
        row.set("rowsAffected", number(entry.rows_affected));
        row.set("durationMs", number(entry.duration_ms));
        row.set("errorText", text(&entry.error_text));
        row.set("startTime", Value::Timestamp(jiff::Timestamp::now()));
        row.set("detailJson", text(&entry.detail_json));
        row.create(facade)
    })
}
