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

//! The SQL services of `org.moqui.impl.DatabaseServices` (plans/Tools-Database.md §8, §7.2 Queries, §7.5, §14): runs,
//! their classification and cancel, explain, the editor's catalog, the database's own statement stats, saved queries,
//! the history (the user's own audit rows) and the audit purge. A refusal is a message error and no out-parameters; a
//! statement that fails is in its result (`error`), not a message error.

use std::collections::BTreeMap;
use std::time::Duration;

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{Condition, EntityFacade};
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;

use crate::service::{add_error, flag, text, TOOLS_SCREEN};
use crate::sql::{self, RunRequest};
use crate::{audit, catalog, DbRef, NOT_AVAILABLE};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, classify_sql)?;
    rust_service!(map, run_sql)?;
    rust_service!(map, cancel_sql_run)?;
    rust_service!(map, explain_sql)?;
    rust_service!(map, get_sql_catalog)?;
    rust_service!(map, list_database_statements)?;
    rust_service!(map, reset_database_statements)?;
    rust_service!(map, find_sql_history)?;
    rust_service!(map, find_saved_queries)?;
    rust_service!(map, store_saved_query)?;
    rust_service!(map, delete_saved_query)?;
    rust_service!(map, purge_database_audit)?;
    Ok(())
}

const SAVED_QUERY: &str = "moqui.database.SavedQuery";

type Out = Result<BTreeMap<String, Value>, LibError>;

fn none() -> Out {
    Ok(BTreeMap::new())
}

/// SQL needs `SQL_RUNNER_WEB` and authorization for all actions on the Tools app (§14.1), as the SQL screen checks:
/// asked here whatever calls the service.
pub(crate) fn may_run_sql() -> bool {
    if !moqui_core::user::has_permission("SQL_RUNNER_WEB").unwrap_or(false) {
        add_error("SQL needs the SQL_RUNNER_WEB permission");
        return false;
    }
    match moqui_core::user::authorize_own("AT_XML_SCREEN", TOOLS_SCREEN, "all") {
        Ok(()) => true,
        Err(err) => {
            tracing::info!(error = %err, "Refused a SQL service");
            add_error("SQL needs authorization for all actions on the Tools app");
            false
        }
    }
}

/// The entity group `db` names; an external database is refused (§4.3) until external connections are built.
fn group_of(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Option<String> {
    match DbRef::parse(&text(parameters, "db"), facade.default_group()) {
        Ok(DbRef::Group(group)) if facade.group_names().contains(&group) => Some(group),
        Ok(DbRef::Group(group)) => {
            add_error(format!("no entity group named {group}"));
            None
        }
        Ok(DbRef::External(id)) => {
            let group = crate::ext::external_group(facade, &id);
            if group.is_none() {
                add_error(NOT_AVAILABLE);
            }
            group
        }
        Err(message) => {
            add_error(message);
            None
        }
    }
}

pub(crate) fn number(parameters: &BTreeMap<String, Value>, key: &str) -> usize {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .and_then(|value| to_plain_string(value).trim().parse::<f64>().ok())
        .filter(|number| number.is_finite() && *number >= 0.0)
        // past any limit the callee takes; it refuses or caps what is too large
        .map_or(0, |number| number.min(1e12) as usize)
}

fn params_map(parameters: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    match parameters.get("parameters") {
        Some(Value::Map(map)) => map.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
        _ => BTreeMap::new(),
    }
}

pub(crate) fn current_user() -> Option<String> {
    moqui_core::user::current_user_id().ok().flatten()
}

fn int(value: usize) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// `classify#Sql`: the statements of `sqlText` and the one-time token a run with changes allowed needs.
fn classify_sql(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_run_sql() {
        return none();
    }
    let Some(group) = group_of(parameters, facade) else {
        return none();
    };
    let user = current_user();
    match sql::classify(
        facade,
        &group,
        &text(parameters, "sqlText"),
        user.as_deref(),
        moqui_core::web::is_production(),
    ) {
        Ok(outcome) => {
            let mut out = BTreeMap::new();
            out.insert(
                "statements".to_string(),
                Value::List(outcome.statements.iter().map(sql::Statement::to_value).collect()),
            );
            out.insert("changeCount".to_string(), int(outcome.change_count));
            out.insert("token".to_string(), Value::String(outcome.token));
            out.insert(
                "confirmDatabaseName".to_string(),
                outcome.confirm_database_name.map_or(Value::Null, Value::String),
            );
            out.insert("implicitCommit".to_string(), Value::Bool(outcome.implicit_commit));
            out.insert(
                "parameterNames".to_string(),
                Value::List(outcome.parameter_names.into_iter().map(Value::String).collect()),
            );
            Ok(out)
        }
        Err(refusal) => {
            add_error(refusal.0);
            none()
        }
    }
}

/// `run#Sql`: run `sqlText`, read only unless `allowChanges` with the token of `classify#Sql`.
fn run_sql(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_run_sql() {
        return none();
    }
    let Some(group) = group_of(parameters, facade) else {
        return none();
    };
    let request = RunRequest {
        run_id: text(parameters, "runId"),
        group,
        sql_text: text(parameters, "sqlText"),
        params: params_map(parameters),
        allow_changes: flag(parameters, "allowChanges"),
        token: text(parameters, "token"),
        confirm_database_name: text(parameters, "confirmDatabaseName"),
        each_on_own: flag(parameters, "eachOnOwn"),
        limit: number(parameters, "limit"),
        offset: number(parameters, "offset"),
        timeout: Duration::from_secs(number(parameters, "timeoutSeconds") as u64),
    };
    match sql::run(
        facade,
        &request,
        &audit::Actor::current(),
        moqui_core::web::is_production(),
    ) {
        Ok(outcome) => {
            let mut out = BTreeMap::new();
            out.insert("runId".to_string(), Value::String(outcome.run_id));
            out.insert(
                "results".to_string(),
                Value::List(outcome.results.iter().map(sql::StatementResult::to_value).collect()),
            );
            out.insert("rolledBack".to_string(), Value::Bool(outcome.rolled_back));
            out.insert("cancelled".to_string(), Value::Bool(outcome.cancelled));
            out.insert("timedOut".to_string(), Value::Bool(outcome.timed_out));
            Ok(out)
        }
        Err(refusal) => {
            add_error(refusal.0);
            none()
        }
    }
}

/// `cancel#SqlRun`: cancel the run `runId` (the user's own, or any with `DATABASE_ADMIN`).
fn cancel_sql_run(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_run_sql() {
        return none();
    }
    let run_id = text(parameters, "runId");
    if run_id.is_empty() {
        add_error("runId is required");
        return none();
    }
    let admin = moqui_core::user::has_permission("DATABASE_ADMIN").unwrap_or(false);
    let user = current_user();
    let this_instance = ExecutionContext::with_current(|ec| moqui_core::drain::instance_id(ec.factory()));
    match sql::cancel(facade, &run_id, user.as_deref(), admin, this_instance.as_deref()) {
        Ok(outcome) => {
            let mut out = BTreeMap::new();
            let (name, instance, backend) = match outcome {
                sql::CancelOutcome::Cancelled => ("cancelled", None, None),
                sql::CancelOutcome::CancelledRemote {
                    instance_id,
                    backend_id,
                } => ("cancelledRemote", Some(instance_id), Some(backend_id)),
                sql::CancelOutcome::NotRunning => ("notRunning", None, None),
            };
            out.insert("outcome".to_string(), Value::string(name));
            out.insert("instanceId".to_string(), instance.map_or(Value::Null, Value::String));
            out.insert("backendId".to_string(), backend.map_or(Value::Null, Value::I64));
            audit::record(
                facade,
                &audit::AuditEntry {
                    action: "DbaCancel",
                    detail_json: Some(serde_json::json!({"runId": run_id, "outcome": name}).to_string()),
                    ..audit::AuditEntry::default()
                },
            );
            Ok(out)
        }
        Err(refusal) => {
            add_error(refusal.0);
            none()
        }
    }
}

/// `explain#Sql`: the plan of one statement (§7.5).
fn explain_sql(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_run_sql() {
        return none();
    }
    let Some(group) = group_of(parameters, facade) else {
        return none();
    };
    let mariadb = catalog::group_is_mysql(facade, &group)
        && crate::capabilities::group_capabilities(facade, &group, || false, false).backend
            == Some(crate::capabilities::Backend::Mariadb);
    let request = sql::ExplainRequest {
        run_id: text(parameters, "runId"),
        group,
        sql_text: text(parameters, "sqlText"),
        params: params_map(parameters),
        analyze: flag(parameters, "analyze"),
        token: text(parameters, "token"),
        confirm_database_name: text(parameters, "confirmDatabaseName"),
        mariadb,
        timeout: Duration::from_secs(number(parameters, "timeoutSeconds") as u64),
    };
    match sql::explain(
        facade,
        &request,
        &audit::Actor::current(),
        moqui_core::web::is_production(),
    ) {
        Ok(outcome) => {
            let mut out = BTreeMap::new();
            out.insert("runId".to_string(), Value::String(outcome.run_id));
            out.insert("format".to_string(), Value::string(outcome.format));
            out.insert("plan".to_string(), Value::String(outcome.plan));
            out.insert("analyzed".to_string(), Value::Bool(outcome.analyzed));
            out.insert("rolledBack".to_string(), Value::Bool(outcome.rolled_back));
            Ok(out)
        }
        Err(refusal) => {
            add_error(refusal.0);
            none()
        }
    }
}

/// `get#SqlCatalog`: the editor's completion schema: `tables` (table name to its column names; a table outside the
/// connection's schema is `schema.table`) and `entities` (entity name to its table), for the group's entities.
fn get_sql_catalog(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_run_sql() {
        return none();
    }
    let Some(group) = group_of(parameters, facade) else {
        return none();
    };
    let mysql = catalog::group_is_mysql(facade, &group);
    let sql = if mysql {
        catalog::mysql::COLUMNS
    } else {
        catalog::postgres::COLUMNS
    };
    let rows = match catalog::query(facade, &group, sql) {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = %err, "Could not read the columns of group {group}");
            add_error("The tables of the database could not be read (the server log has the detail)");
            return none();
        }
    };
    let mut tables: IndexMap<String, Value> = IndexMap::new();
    for row in &rows {
        let schema = row.get("table_schema").map(to_plain_string).unwrap_or_default();
        let current = row.get("current_schema").map(to_plain_string).unwrap_or_default();
        let table = row.get("table_name").map(to_plain_string).unwrap_or_default();
        let column = row.get("column_name").map(to_plain_string).unwrap_or_default();
        let name = if schema == current || schema.is_empty() {
            table
        } else {
            format!("{schema}.{table}")
        };
        if let Value::List(columns) = tables.entry(name).or_insert_with(|| Value::List(Vec::new())) {
            columns.push(Value::String(column));
        }
    }
    let groups = facade.group_names();
    let default = facade.default_group().to_string();
    let entities: IndexMap<String, Value> = facade.with_model(|model| {
        model
            .entities()
            .iter()
            .filter(|entity| !entity.is_view && !entity.table_name.is_empty())
            .filter(|entity| {
                let own = entity
                    .group
                    .as_deref()
                    .filter(|name| groups.iter().any(|open| open == name))
                    .unwrap_or(&default);
                own == group
            })
            .map(|entity| {
                // as the database names the table: Postgres folds an unquoted name to lower case, MySQL keeps it
                let table = [entity.table_name.clone(), entity.table_name.to_lowercase()]
                    .into_iter()
                    .find(|name| tables.contains_key(name))
                    .unwrap_or_else(|| entity.table_name.clone());
                (entity.full_name.clone(), Value::String(table))
            })
            .collect()
    });
    let mut out = BTreeMap::new();
    out.insert(
        "dialect".to_string(),
        Value::string(if mysql { "mysql" } else { "postgres" }),
    );
    out.insert("tables".to_string(), Value::Map(tables));
    out.insert("entities".to_string(), Value::Map(entities));
    Ok(out)
}

/// `list#DatabaseStatements`: the database's own statement stats (§7.2): Postgres `pg_stat_statements`, MySQL and
/// MariaDB `events_statements_summary_by_digest`, the most costly first. Needs only the Tools app.
fn list_database_statements(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !crate::service::may_view_tools() {
        return none();
    }
    let Some(group) = group_of(parameters, facade) else {
        return none();
    };
    let limit = match number(parameters, "limit") {
        0 => 100,
        limit => limit.min(1000),
    };
    let sql = if catalog::group_is_mysql(facade, &group) {
        catalog::mysql::STATEMENT_STATS
    } else {
        catalog::postgres::STATEMENT_STATS
    };
    let mut out = BTreeMap::new();
    match catalog::query_with(facade, &group, sql, &[int(limit)]) {
        Ok(rows) => {
            out.insert("available".to_string(), Value::Bool(true));
            out.insert(
                "statements".to_string(),
                Value::List(rows.into_iter().map(Value::Map).collect()),
            );
        }
        Err(err) => {
            tracing::info!(error = %err, "The statement stats of group {group} do not read");
            let caps = crate::capabilities::group_capabilities(
                facade,
                &group,
                || crate::service::group_is_embedded(&group),
                false,
            );
            out.insert("available".to_string(), Value::Bool(false));
            out.insert(
                "reason".to_string(),
                caps.feature("statementStats")
                    .reason
                    .map_or_else(|| Value::string("the statement stats do not read"), Value::String),
            );
            out.insert("statements".to_string(), Value::List(Vec::new()));
        }
    }
    Ok(out)
}

/// `reset#DatabaseStatements`: reset the database's own statement stats, with `DATABASE_ADMIN`.
fn reset_database_statements(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !crate::service::may_view_tools() {
        return none();
    }
    if !moqui_core::user::has_permission("DATABASE_ADMIN").unwrap_or(false) {
        add_error("Resetting statement stats needs the DATABASE_ADMIN permission");
        return none();
    }
    let Some(group) = group_of(parameters, facade) else {
        return none();
    };
    if moqui_core::entity::fence::active() {
        add_error("Resetting statement stats is not done under the read fence");
        return none();
    }
    let (sql, result) = if catalog::group_is_mysql(facade, &group) {
        let sql = catalog::mysql::STATEMENT_STATS_RESET;
        (sql, catalog::execute(facade, &group, sql).map(|_| ()))
    } else {
        let sql = catalog::postgres::STATEMENT_STATS_RESET;
        (sql, catalog::query(facade, &group, sql).map(|_| ()))
    };
    let error = result.as_ref().err().map(ToString::to_string);
    audit::record(
        facade,
        &audit::AuditEntry {
            action: "DbaStatsReset",
            db: Some(format!("group:{group}")),
            statement_text: Some(sql.to_string()),
            is_change: true,
            error_text: error.clone(),
            ..audit::AuditEntry::default()
        },
    );
    if let Some(error) = error {
        tracing::warn!(error, "Could not reset the statement stats of group {group}");
        add_error("The statement stats could not be reset (the server log has the detail)");
    }
    none()
}

/// `find#SqlHistory`: the current user's own SQL audit rows, newest first: the history of the SQL screen.
fn find_sql_history(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_run_sql() {
        return none();
    }
    let Some(user) = current_user() else {
        add_error("The history is a signed-in user's");
        return none();
    };
    let limit = match number(parameters, "limit") {
        0 => 50,
        limit => limit.min(500),
    };
    let db = text(parameters, "db");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut find = facade
        .find(audit::DATABASE_AUDIT)?
        .eq("userId", Value::String(user))
        .eq("actionEnumId", Value::string("DbaSql"));
    if !db.is_empty() {
        find = find.eq("db", Value::String(db));
    }
    let rows = find.order_by("startTime", true).limit(limit as i64).list()?;
    let history = rows
        .iter()
        .map(|row| {
            let mut entry = IndexMap::new();
            for field in [
                "auditId",
                "db",
                "statementText",
                "isChange",
                "rowsReturned",
                "rowsAffected",
                "durationMs",
                "errorText",
                "startTime",
            ] {
                entry.insert(field.to_string(), row.get(field).cloned().unwrap_or(Value::Null));
            }
            Value::Map(entry)
        })
        .collect();
    let mut out = BTreeMap::new();
    out.insert("history".to_string(), Value::List(history));
    Ok(out)
}

fn saved_query_value(row: &moqui_core::entity::EntityValue) -> Value {
    let mut entry = IndexMap::new();
    for field in [
        "queryId",
        "description",
        "db",
        "sqlText",
        "ownerUserId",
        "isShared",
        "parametersJson",
    ] {
        entry.insert(field.to_string(), row.get(field).cloned().unwrap_or(Value::Null));
    }
    Value::Map(entry)
}

/// `find#SavedQueries`: the user's own saved queries and the shared ones, by description.
fn find_saved_queries(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_run_sql() {
        return none();
    }
    let user = current_user().unwrap_or_default();
    let db = text(parameters, "db");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut find = facade.find(SAVED_QUERY)?.condition(Condition::Or(vec![
        Condition::equals("ownerUserId", Value::String(user)),
        Condition::equals("isShared", Value::string("Y")),
    ]));
    if !db.is_empty() {
        find = find.eq("db", Value::String(db));
    }
    let rows = find.order_by("description", false).list()?;
    let mut out = BTreeMap::new();
    out.insert(
        "savedQueries".to_string(),
        Value::List(rows.iter().map(saved_query_value).collect()),
    );
    Ok(out)
}

/// The saved query `queryId` when the current user owns it; an error added when not.
fn owned_query(
    facade: &EntityFacade,
    query_id: &str,
    user: &str,
) -> Result<Option<moqui_core::entity::EntityValue>, LibError> {
    let row = facade.find(SAVED_QUERY)?.eq("queryId", Value::string(query_id)).one()?;
    match row {
        Some(row) if row.get("ownerUserId").map(to_plain_string).as_deref() == Some(user) => Ok(Some(row)),
        Some(_) => {
            add_error("That saved query is another user's");
            Ok(None)
        }
        None => {
            add_error(format!("No saved query {query_id}"));
            Ok(None)
        }
    }
}

/// `store#SavedQuery`: create a saved query (no `queryId`) or update one the user owns. Out: `queryId`.
fn store_saved_query(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_run_sql() {
        return none();
    }
    let Some(user) = current_user() else {
        add_error("Saved queries are a signed-in user's");
        return none();
    };
    let sql_text = text(parameters, "sqlText");
    if sql_text.trim().is_empty() {
        add_error("sqlText is required");
        return none();
    }
    let parameters_json = text(parameters, "parametersJson");
    if !parameters_json.is_empty() && serde_json::from_str::<serde_json::Value>(&parameters_json).is_err() {
        add_error("parametersJson is not JSON");
        return none();
    }
    let query_id = text(parameters, "queryId");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = if query_id.is_empty() {
        let mut row = facade.make_value(SAVED_QUERY)?;
        row.set_sequenced_id_primary(facade)?;
        row.set("ownerUserId", Value::string(&user));
        row
    } else {
        match owned_query(facade, &query_id, &user)? {
            Some(row) => row,
            None => return none(),
        }
    };
    row.set("description", Value::String(text(parameters, "description")));
    row.set("db", Value::String(text(parameters, "db")));
    row.set("sqlText", Value::String(sql_text));
    row.set(
        "isShared",
        Value::string(if flag(parameters, "isShared") { "Y" } else { "N" }),
    );
    row.set(
        "parametersJson",
        if parameters_json.is_empty() {
            Value::Null
        } else {
            Value::String(parameters_json)
        },
    );
    if query_id.is_empty() {
        row.create(facade)?;
    } else {
        row.update(facade)?;
    }
    let mut out = BTreeMap::new();
    out.insert(
        "queryId".to_string(),
        row.get("queryId").cloned().unwrap_or(Value::Null),
    );
    Ok(out)
}

/// `delete#SavedQuery`: delete a saved query the user owns.
fn delete_saved_query(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_run_sql() {
        return none();
    }
    let Some(user) = current_user() else {
        add_error("Saved queries are a signed-in user's");
        return none();
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    if let Some(row) = owned_query(facade, &text(parameters, "queryId"), &user)? {
        row.delete(facade)?;
    }
    none()
}

/// The days `DatabaseAudit` rows are kept: the `database_audit_days` property, 90 by default, at least 1.
#[must_use]
pub fn audit_days() -> i64 {
    moqui_lib::env::get_prop_or_env("database_audit_days")
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|days| *days >= 1)
        .unwrap_or(90)
}

/// `purge#DatabaseAudit` (the `DatabaseAuditPurge` ServiceJob): delete the audit rows older than [`audit_days`]. Out:
/// `deleted`.
fn purge_database_audit(_parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    // the ServiceJob runs it with no user signed in; a user calling it needs DATABASE_ADMIN
    if current_user().is_some() && !moqui_core::user::has_permission("DATABASE_ADMIN").unwrap_or(false) {
        add_error("Purging the database audit needs the DATABASE_ADMIN permission");
        return none();
    }
    let tokens = sql::purge_tokens(facade).unwrap_or_else(|err| {
        tracing::error!(error = %err, "Could not purge the expired SQL run tokens");
        0
    });
    if tokens > 0 {
        tracing::info!("Purged {tokens} expired SQL run tokens");
    }
    let days = audit_days();
    let cutoff = jiff::Timestamp::now()
        .checked_sub(jiff::SignedDuration::from_hours(24 * days))
        .map_err(|err| LibError::new(err.to_string()))?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let deleted = facade
        .find(audit::DATABASE_AUDIT)?
        .condition(Condition::less("startTime", Value::Timestamp(cutoff)))
        .delete_all()?;
    if deleted > 0 {
        tracing::info!("Purged {deleted} DatabaseAudit rows older than {days} days");
    }
    let mut out = BTreeMap::new();
    out.insert("deleted".to_string(), Value::I64(deleted));
    Ok(out)
}
