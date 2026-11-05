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

//! `org.moqui.impl.EtlServices` (plans/Tools-Database.md §10, §14.1): running a mapping (a job on the worker pool, or
//! here with `wait`), the check of a file's first rows for "Load into…", matching columns to target fields, storing,
//! copying and deleting mappings and value maps, the runs, and `load#SpreadsheetEntity`'s inline mapping. A refusal
//! is a message-facade error and no out parameters. Writes, checks and runs need authorization for all actions on the
//! Tools app; a SQL source also needs `SQL_RUNNER_WEB`, and an `expression` field `GROOVY_SHELL_WEB`. A run is the
//! user's own, with authorization on, also on a worker or in a ServiceJob ([`super::access::run_as_user`]). Only a
//! mapping's owner, or a `DATABASE_ADMIN` holder, may change, copy over or delete it.

use std::collections::BTreeMap;
use std::sync::Arc;

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::etl::{Loader, SimpleEntry, SimpleEtl, ROW_NUMBER_KEY};
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;

use super::access;
use super::mapping::{
    Mapping, MappingField, SourceKind, TargetKind, MAPPING, MAPPING_FIELD, VALUE_MAP, VALUE_MAP_ENTRY,
};
use super::run::{run, Progress, RunOptions, RunOutcome};
use super::source::{self, SourceOptions};
use super::transform::{row_json, row_number_of, ErrorLog, MappingTransformer};
use crate::group_db::{add_error, flag, list, number, text, Out};

pub const RUN: &str = "moqui.etl.EtlRun";
pub const RUN_ERROR: &str = "moqui.etl.EtlRunError";
const RUN_SERVICE: &str = "org.moqui.impl.EtlServices.run#EtlMapping";
/// Where a run's error file goes.
/// Where a run's error file goes: `<root>/<userId>/<runId>/errors.csv`, outside the import root, read through
/// `get#EtlRun` by the run's user or a `DATABASE_ADMIN` holder.
pub const ERROR_ROOT: &str = "dbresource://moqui/etl-run";
/// The rows the sample check reads by default.
pub const SAMPLE_ROWS: usize = 200;

pub const PENDING: &str = "EtlrPending";
pub const RUNNING: &str = "EtlrRunning";
pub const DONE: &str = "EtlrDone";
pub const FAILED: &str = "EtlrFailed";

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, run_etl_mapping)?;
    rust_service!(map, check_etl_sample)?;
    rust_service!(map, match_etl_fields)?;
    rust_service!(map, get_etl_mapping)?;
    rust_service!(map, store_etl_mapping)?;
    rust_service!(map, copy_etl_mapping)?;
    rust_service!(map, delete_etl_mapping)?;
    rust_service!(map, store_etl_value_map)?;
    rust_service!(map, delete_etl_value_map)?;
    rust_service!(map, get_etl_runs)?;
    rust_service!(map, get_etl_run)?;
    // load#SpreadsheetEntity maps its rows as a run does
    moqui_core::service::spreadsheet::install_inline_load(inline_spreadsheet_load);
    Ok(())
}

/// Writes and runs need authorization for all actions on the Tools app (§14.1); when not, an error naming `what`.
fn may_change(what: &str) -> bool {
    if !crate::service::may_view_tools() {
        return false;
    }
    match moqui_core::user::authorize_own("AT_XML_SCREEN", crate::service::TOOLS_SCREEN, "all") {
        Ok(()) => true,
        Err(err) => {
            tracing::info!(error = %err, "Refused an ETL change");
            add_error(format!("{what} needs authorization for all actions on the Tools app"));
            false
        }
    }
}

/// What every use of a mapping checks first: what it reaches ([`Mapping::validate`]), `SQL_RUNNER_WEB` for a SQL
/// source and `GROOVY_SHELL_WEB` for an expression. False with an error added.
fn mapping_allowed(facade: &EntityFacade, mapping: &Mapping) -> bool {
    if let Err(err) = mapping.validate_with(facade) {
        add_error(err.to_string());
        return false;
    }
    if mapping.source_kind == SourceKind::Sql && !moqui_core::user::has_permission("SQL_RUNNER_WEB").unwrap_or(false) {
        add_error("An ETL from a SQL source needs the SQL_RUNNER_WEB permission");
        return false;
    }
    if mapping.has_expression() && !access::may_run_scripts() {
        add_error(access::SCRIPTS_REFUSED);
        return false;
    }
    true
}

fn current_user() -> String {
    moqui_core::user::current_user_id().ok().flatten().unwrap_or_default()
}

fn is_database_admin() -> bool {
    moqui_core::user::has_permission("DATABASE_ADMIN").unwrap_or(false)
}

/// Whether the current user may change the row of `entity` whose `id_field` is `id`, its owner named by
/// `owner_field`: the owner, or a `DATABASE_ADMIN` holder. True for a row not saved yet. False with an error added.
fn may_change_owned(
    facade: &EntityFacade,
    entity: &str,
    id_field: &str,
    owner_field: &str,
    id: Option<&str>,
    what: &str,
) -> bool {
    let Some(id) = id.filter(|id| !id.is_empty()) else {
        return true;
    };
    let owner = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        facade
            .find(entity)
            .and_then(|find| find.eq(id_field, Value::string(id)).one())
    };
    let owner = match owner {
        Ok(Some(row)) => row.get(owner_field).map(to_plain_string).unwrap_or_default(),
        Ok(None) => return true,
        Err(err) => {
            add_error(err.to_string());
            return false;
        }
    };
    let me = current_user();
    if (!me.is_empty() && owner == me) || is_database_admin() {
        return true;
    }
    add_error(format!(
        "{what} needs to be its owner or to have the DATABASE_ADMIN permission"
    ));
    false
}

/// The identity before an owner sign-in ([`sign_in_owner`]), put back when dropped: a later `run#EtlMapping` in the
/// same job context signs in as its own mapping's owner.
#[must_use = "dropping the guard signs the owner out"]
struct OwnerSignIn(moqui_core::user::session::Identity);

impl Drop for OwnerSignIn {
    fn drop(&mut self) {
        moqui_core::user::session::restore_identity(&self.0);
    }
}

/// Sign in as the owner of the saved mapping `mapping_id`, for a ServiceJob's run with no user (a cron run of
/// `job_name`): then the owner's own checks apply as for a run of theirs. The owner must be a `ServiceJobUser` of the
/// job, so whoever writes a ServiceJob cannot run another user's mapping as that user. The account's own gate
/// (disabled until a time, terminated, a password that must change or has expired) is the login's.
///
/// # Errors
///
/// The mapping is not there or has no owner, the owner is not a user of the job, or the owner may not sign in.
fn sign_in_owner(facade: &EntityFacade, mapping_id: &str, job_name: &str) -> Result<OwnerSignIn, LibError> {
    let username = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let Some(mapping) = facade.find(MAPPING)?.eq("mappingId", Value::string(mapping_id)).one()? else {
            return Err(LibError::new(format!("no ETL mapping {mapping_id}")));
        };
        let owner = mapping.get("ownerUserId").map(to_plain_string).unwrap_or_default();
        if owner.is_empty() {
            return Err(LibError::new(format!(
                "ETL mapping {mapping_id} has no owner to run as"
            )));
        }
        let job_user = facade
            .find("moqui.service.job.ServiceJobUser")?
            .eq("jobName", Value::string(job_name))
            .eq("userId", Value::string(&owner))
            .one()?;
        if job_user.is_none() {
            return Err(LibError::new(format!(
                "The owner of ETL mapping {mapping_id} is not a user of ServiceJob {job_name}"
            )));
        }
        let account = facade
            .find("moqui.security.UserAccount")?
            .eq("userId", Value::string(&owner))
            .one()?;
        let username = account
            .and_then(|account| account.get("username").map(to_plain_string))
            .unwrap_or_default();
        if username.is_empty() {
            return Err(LibError::new(format!(
                "The owner of ETL mapping {mapping_id} is not a user"
            )));
        }
        username
    };
    let restore = OwnerSignIn(moqui_core::user::session::identity());
    moqui_core::user::internal_login_user(&username).map_err(|err| {
        LibError::new(format!(
            "The owner of ETL mapping {mapping_id} could not sign in: {err}"
        ))
    })?;
    Ok(restore)
}

/// [`may_change_owned`] of a saved mapping.
fn may_change_mapping(facade: &EntityFacade, mapping_id: Option<&str>, what: &str) -> bool {
    may_change_owned(facade, MAPPING, "mappingId", "ownerUserId", mapping_id, what)
}

fn now() -> Value {
    Value::Timestamp(jiff::Timestamp::now())
}

fn instance_id() -> String {
    ExecutionContext::with_current(|ec| moqui_core::drain::instance_id(ec.factory())).unwrap_or_default()
}

/// The mapping a service names: `mappingId`, else the inline `mapping`, its value maps read. None with an error added.
fn mapping_of(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Option<Mapping> {
    let mapping_id = text(parameters, "mappingId");
    let read = if mapping_id.is_empty() {
        match parameters.get("mapping") {
            Some(Value::Map(map)) => Mapping::from_map(map),
            _ => Err(LibError::new("An ETL run needs a mappingId or a mapping")),
        }
    } else {
        Mapping::load(facade, &mapping_id)
    };
    match read.and_then(|mut mapping| mapping.resolve(facade).map(|()| mapping)) {
        Ok(mapping) => Some(mapping),
        Err(err) => {
            add_error(err.to_string());
            None
        }
    }
}

fn source_options(parameters: &BTreeMap<String, Value>) -> SourceOptions {
    SourceOptions {
        location: Some(text(parameters, "location")).filter(|location| !location.is_empty()),
        content: match parameters.get("contentBytes") {
            Some(Value::Bytes(bytes)) if !bytes.is_empty() => Some(bytes.clone()),
            _ => None,
        },
        sheet_index: number(parameters, "sheetIndex").unwrap_or(0).max(0) as usize,
        start_row: number(parameters, "startRow").unwrap_or(0).max(0) as usize,
        max_rows: number(parameters, "maxRows")
            .filter(|rows| *rows > 0)
            .map(|rows| rows as usize),
        check: flag(parameters, "check"),
    }
}

/// Write `fields` onto the run's row, as the framework, in a transaction of its own; a failure is logged.
fn write_run(facade: &EntityFacade, run_id: &str, fields: Vec<(&str, Value)>) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let written = facade.transactions_for(RUN).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            let Some(mut row) = facade.find(RUN)?.eq("runId", Value::string(run_id)).one()? else {
                return Err(LibError::new(format!("no ETL run {run_id}")));
            };
            for (field, value) in fields {
                row.set(field, value);
            }
            row.update(facade)
        })
    });
    if let Err(err) = written {
        tracing::error!(error = %err, run = %run_id, "Could not write the ETL run row");
    }
}

/// A new `EtlRun` row: its id.
fn create_run(facade: &EntityFacade, mapping: &Mapping, check: bool, status: &str) -> Result<String, LibError> {
    let user_id = moqui_core::user::current_user_id().ok().flatten();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(RUN)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let mut row = facade.make_value(RUN)?;
        row.set_sequenced_id_primary(facade)?;
        row.set(
            "mappingId",
            mapping.mapping_id.clone().map_or(Value::Null, Value::String),
        );
        row.set("userId", user_id.clone().map_or(Value::Null, Value::String));
        row.set(
            "jobRunId",
            moqui_core::service::current_job_run_id().map_or(Value::Null, Value::String),
        );
        row.set("instanceId", Value::string(instance_id()));
        row.set("isCheck", Value::string(if check { "Y" } else { "N" }));
        row.set("statusId", Value::string(status));
        row.set("startTime", now());
        row.set("extractCount", Value::I64(0));
        row.set("loadCount", Value::I64(0));
        row.set("errorCount", Value::I64(0));
        row.create(facade)?;
        Ok(row.get("runId").map(to_plain_string).unwrap_or_default())
    })
}

/// The error rows (the first `MAX_ERROR_ROWS`) and the error file of a run.
fn record_errors(facade: &EntityFacade, run_id: &str, outcome: &RunOutcome) -> Option<String> {
    if outcome.errors.is_empty() {
        return None;
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let written = facade.transactions_for(RUN_ERROR).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            for (index, error) in outcome.errors.iter().enumerate() {
                let mut row = facade.make_value(RUN_ERROR)?;
                row.set("runId", Value::string(run_id));
                row.set("errorSeqId", Value::string(format!("{:06}", index + 1)));
                row.set("rowNumber", Value::I64(error.row_number));
                row.set("fieldName", Value::string(&error.field));
                row.set("message", Value::string(&error.message));
                row.set("rowJson", Value::string(row_json(&error.row)));
                row.create(facade)?;
            }
            Ok(())
        })
    });
    if let Err(err) = written {
        tracing::error!(error = %err, run = %run_id, "Could not write the ETL run's error rows");
    }
    let csv = outcome.error_csv.as_ref()?;
    let user = Some(current_user()).filter(|user| !user.is_empty() && !user.contains('/'));
    let location = format!("{ERROR_ROOT}/{}/{run_id}/errors.csv", user.as_deref().unwrap_or("_NA_"));
    // as the framework: the place is the run's own
    let saved = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory().resource().write_bytes(&location, csv.as_bytes())
    });
    match saved {
        Some(Ok(())) => Some(location),
        Some(Err(err)) => {
            tracing::error!(error = %err, run = %run_id, "Could not write the ETL run's error file");
            None
        }
        None => None,
    }
}

fn error_values(outcome: &RunOutcome, limit: usize) -> Value {
    Value::List(
        outcome
            .errors
            .iter()
            .take(limit)
            .map(|error| {
                let mut map = IndexMap::new();
                map.insert("rowNumber".to_string(), Value::I64(error.row_number));
                map.insert("fieldName".to_string(), Value::string(&error.field));
                map.insert("message".to_string(), Value::string(&error.message));
                map.insert(
                    "row".to_string(),
                    Value::Map(error.row.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
                );
                Value::Map(map)
            })
            .collect(),
    )
}

/// The status a run ended with: Failed when the source did not read or its load was not committed (with no batch size,
/// any error rolls the run back); Done otherwise, its `errorCount` the rows that failed (in batches rolled back).
fn end_status(outcome: &RunOutcome, check: bool) -> &'static str {
    if outcome.failure.is_some() {
        FAILED
    } else if check || outcome.committed {
        DONE
    } else {
        FAILED
    }
}

/// `run#EtlMapping`: run a mapping (`mappingId`, or an inline `mapping`) as a job on the worker pool, or here with
/// `wait`. In: `check` (extract and transform only: the row errors, the foreign keys with no related record, and for an
/// entity target the diff against the database), `startRow` (the first row read, from 1: a rerun from the last run's
/// `nextStartRow`), `location` (in place of the mapping's; `${date}`, `${yesterday}`, `${now}` and `${nowMillis}`
/// replaced), `contentBytes`, `sheetIndex`, `maxRows`, `wait`. Out: `runId`, `statusId` (`EtlrPending` when queued),
/// and with `wait` the counts (`extractCount` rows read, `skipCount`, `loadCount` rows committed, `errorCount` rows
/// with an error, `fieldErrorCount` errors, `lastCommittedRow`, `nextStartRow`, `committedBatches`, `failedBatches`),
/// `failedRanges` (each rolled-back batch's `fromRow`, `toRow`, `message`), `errors` (the first 100: `rowNumber`,
/// `fieldName`, `message`, `row`), `errorLocation` (the error file), `failure`, and for a check `diff`.
///
/// The run is the user's (also on the worker): authorization on for the run, its writes and its service calls, the
/// Tools app's authorization for all actions checked again as the user. Called from a ServiceJob with no user signed
/// in (a cron run) and a `mappingId`, it runs as the mapping's owner ([`sign_in_owner`]) from the mapping's own source:
/// `location` and `contentBytes` are ignored. An inline mapping there is refused. Called from a job it runs here, as
/// with `wait`, and the `EtlRun` names the job's run (`jobRunId`). With `wait` the caller's transaction is suspended,
/// so batches commit.
pub(crate) fn run_etl_mapping(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut parameters = parameters.clone();
    let mut _owner = None;
    if let Some(job_name) = moqui_core::service::current_job_name().filter(|_| current_user().is_empty()) {
        let mapping_id = text(&parameters, "mappingId");
        if mapping_id.is_empty() {
            tracing::error!(job = %job_name, "Refused an ETL run of an inline mapping with no user signed in");
            add_error("An ETL run of an inline mapping needs a signed-in user");
            return Ok(Out::new());
        }
        match sign_in_owner(facade, &mapping_id, &job_name) {
            Ok(signed_in) => _owner = Some(signed_in),
            Err(err) => {
                tracing::error!(job = %job_name, mapping = %mapping_id, error = %err, "Refused an ETL run with no user signed in");
                add_error(err.to_string());
                return Ok(Out::new());
            }
        }
        // the mapping's own source only: the job's writer does not pick what the owner's run reads
        parameters.remove("location");
        parameters.remove("contentBytes");
    }
    if moqui_core::service::current_job_run_id().is_some() {
        // a job's run is already on a worker: run here, so the job's run ends with it
        parameters.insert("wait".to_string(), Value::Bool(true));
    }
    let parameters = &parameters;
    let _user = match access::run_as_user() {
        Ok(user) => user,
        Err(err) => {
            add_error(err.to_string());
            return Ok(Out::new());
        }
    };
    if !may_change("Running an ETL mapping") {
        return Ok(Out::new());
    }
    let Some(mapping) = mapping_of(parameters, facade) else {
        return Ok(Out::new());
    };
    if !mapping_allowed(facade, &mapping) {
        return Ok(Out::new());
    }
    let check = flag(parameters, "check");
    let mut out = Out::new();
    if !flag(parameters, "wait") {
        let created = match create_run(facade, &mapping, check, PENDING) {
            Ok(run_id) => run_id,
            Err(err) => {
                add_error(format!("Could not create the ETL run: {err}"));
                return Ok(out);
            }
        };
        let mut queued = parameters.clone();
        queued.insert("runId".to_string(), Value::string(&created));
        queued.insert("wait".to_string(), Value::Bool(true));
        // the worker's call is made with authorization disabled; the run turns it on again, as the user
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        if let Err(err) = moqui_core::service::call::async_call(RUN_SERVICE, &queued) {
            add_error(format!("Could not start ETL run {created}: {err}"));
            write_run(
                facade,
                &created,
                vec![("statusId", Value::string(FAILED)), ("endTime", now())],
            );
            return Ok(out);
        }
        out.insert("runId".to_string(), Value::string(created));
        out.insert("statusId".to_string(), Value::string(PENDING));
        return Ok(out);
    }
    // the caller's transaction is set aside, so the run's batches commit on their own
    match facade.default_transactions() {
        Ok(tx) => tx.run(TransactionAttribute::IgnoreSuspend, None, || {
            Ok(run_waiting(parameters, facade, &mapping, check))
        }),
        Err(_) => Ok(run_waiting(parameters, facade, &mapping, check)),
    }
}

/// `run#EtlMapping` with `wait`: the run here, its `EtlRun` row written as it goes.
fn run_waiting(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
    mapping: &Mapping,
    check: bool,
) -> BTreeMap<String, Value> {
    let mut out = Out::new();
    let mut run_id = text(parameters, "runId");
    if run_id.is_empty() {
        run_id = match create_run(facade, mapping, check, RUNNING) {
            Ok(run_id) => run_id,
            Err(err) => {
                add_error(format!("Could not create the ETL run: {err}"));
                return out;
            }
        };
    } else {
        // a queued run: only its own user's, and only while it waits
        let row = {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            facade
                .find(RUN)
                .and_then(|find| find.eq("runId", Value::string(&run_id)).one())
        };
        let mine = matches!(&row, Ok(Some(row))
            if row.get("userId").map(to_plain_string).unwrap_or_default() == current_user()
                && row.get("statusId").map(to_plain_string).as_deref() == Some(PENDING));
        if !mine {
            add_error(format!("ETL run {run_id} is not a queued run of yours"));
            return out;
        }
        write_run(
            facade,
            &run_id,
            vec![("statusId", Value::string(RUNNING)), ("startTime", now())],
        );
    }
    let progress_facade = facade.clone();
    let progress_run = run_id.clone();
    let options = RunOptions {
        source: source_options(parameters),
        check,
        join_caller: false,
        progress: Some(Arc::new(move |progress: &Progress| {
            write_run(
                &progress_facade,
                &progress_run,
                vec![
                    ("extractCount", Value::I64(progress.extract_count as i64)),
                    ("loadCount", Value::I64(progress.load_count as i64)),
                    ("errorCount", Value::I64(progress.error_count as i64)),
                    ("lastCommittedRow", Value::I64(progress.last_committed_row)),
                ],
            );
        })),
    };
    let started = std::time::Instant::now();
    let outcome = run(facade, mapping, &options);
    let status = end_status(&outcome, check);
    let error_location = record_errors(facade, &run_id, &outcome);
    write_run(
        facade,
        &run_id,
        vec![
            ("statusId", Value::string(status)),
            ("endTime", now()),
            ("sourceDescription", Value::string(&outcome.resolved_source)),
            ("extractCount", Value::I64(outcome.extract_count as i64)),
            ("skipCount", Value::I64(outcome.skip_count as i64)),
            ("loadCount", Value::I64(outcome.load_count as i64)),
            ("errorCount", Value::I64(outcome.error_count as i64)),
            ("lastCommittedRow", Value::I64(outcome.last_committed_row)),
            (
                "errorLocation",
                error_location.clone().map_or(Value::Null, Value::String),
            ),
            (
                "failureText",
                outcome.failure.clone().map_or(Value::Null, Value::String),
            ),
        ],
    );
    crate::audit::record(
        facade,
        &crate::audit::AuditEntry {
            action: "DbaEtlRun",
            is_change: !check,
            rows_affected: Some(outcome.load_count as i64),
            duration_ms: Some(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)),
            error_text: outcome.failure.clone(),
            detail_json: Some(format!(
                r#"{{"runId":"{run_id}","mappingId":"{}","target":"{}","jobName":{},"check":{check},"loaded":{},"errors":{}}}"#,
                mapping.mapping_id.clone().unwrap_or_default(),
                serde_json::to_string(&moqui_core::service::current_job_name()).unwrap_or_else(|_| "null".to_string()),
                mapping.target_name,
                outcome.load_count,
                outcome.error_count
            )),
            ..crate::audit::AuditEntry::default()
        },
    );
    tracing::info!(
        run = %run_id,
        status,
        extracted = outcome.extract_count,
        loaded = outcome.load_count,
        errors = outcome.error_count,
        "ETL run {run_id} ended {status}"
    );
    out.insert("runId".to_string(), Value::string(&run_id));
    out.insert("statusId".to_string(), Value::string(status));
    out.insert("extractCount".to_string(), Value::I64(outcome.extract_count as i64));
    out.insert("skipCount".to_string(), Value::I64(outcome.skip_count as i64));
    out.insert("loadCount".to_string(), Value::I64(outcome.load_count as i64));
    out.insert("errorCount".to_string(), Value::I64(outcome.error_count as i64));
    out.insert(
        "fieldErrorCount".to_string(),
        Value::I64(outcome.field_error_count as i64),
    );
    out.insert("lastCommittedRow".to_string(), Value::I64(outcome.last_committed_row));
    out.insert("nextStartRow".to_string(), Value::I64(outcome.next_start_row));
    out.insert(
        "committedBatches".to_string(),
        Value::I64(outcome.committed_batches as i64),
    );
    out.insert("failedBatches".to_string(), Value::I64(outcome.failed_batches as i64));
    out.insert(
        "failedRanges".to_string(),
        Value::List(
            outcome
                .failed_ranges
                .iter()
                .map(super::run::FailedRange::to_value)
                .collect(),
        ),
    );
    out.insert("errors".to_string(), error_values(&outcome, 100));
    out.insert(
        "errorLocation".to_string(),
        error_location.map_or(Value::Null, Value::String),
    );
    out.insert(
        "failure".to_string(),
        outcome.failure.clone().map_or(Value::Null, Value::String),
    );
    if check {
        out.insert(
            "diff".to_string(),
            Value::List(outcome.diff.into_iter().map(Value::Map).collect()),
        );
    }
    out
}

/// The rows of a source as read, before any mapping.
struct Collect(std::rc::Rc<std::cell::RefCell<Vec<std::collections::BTreeMap<String, Value>>>>);

impl Loader for Collect {
    fn init(&mut self, _timeout: Option<u32>) {}
    fn load(&mut self, entry: &mut SimpleEntry) -> Result<(), LibError> {
        self.0.borrow_mut().push(entry.values.clone());
        Ok(())
    }
    fn complete(&mut self, _etl: &SimpleEtl) {}
}

/// `check#EtlSample`: the first rows of a source (`maxRows`, default 200) through a mapping (`mappingId` or `mapping`),
/// for "Load into…" in the grid (§10.4): each row's `rowNumber`, its `source` cells, the converted `values` by target
/// field, and `errors` by target field (`_row` for the row as a whole). Out: `columns` (the header), `rows`. Needs what
/// a run needs: authorization for all actions on the Tools app, as the user.
pub(crate) fn check_etl_sample(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let _user = match access::run_as_user() {
        Ok(user) => user,
        Err(err) => {
            add_error(err.to_string());
            return Ok(Out::new());
        }
    };
    if !may_change("Checking an ETL mapping") {
        return Ok(Out::new());
    }
    let Some(mapping) = mapping_of(parameters, facade) else {
        return Ok(Out::new());
    };
    if !mapping_allowed(facade, &mapping) {
        return Ok(Out::new());
    }
    let mut options = source_options(parameters);
    options.max_rows = Some(options.max_rows.unwrap_or(SAMPLE_ROWS).min(10_000));
    options.check = true;
    let source = match source::open(facade, &mapping, &options) {
        Ok(source) => source,
        Err(err) => {
            add_error(err.to_string());
            return Ok(Out::new());
        }
    };
    let headers = Arc::clone(&source.headers);
    let rows = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut etl = SimpleEtl::from_boxes(source.extractor, Box::new(Collect(std::rc::Rc::clone(&rows))));
    etl.process();
    if let Some(err) = etl.extract_error() {
        add_error(err.to_string());
        return Ok(Out::new());
    }
    let errors = Arc::new(std::sync::Mutex::new(ErrorLog::default()));
    let transformer = MappingTransformer::new(facade, &mapping, Arc::clone(&headers), errors);
    let mut out_rows = Vec::new();
    for (index, mut row) in rows.borrow().iter().cloned().enumerate() {
        let number = match row_number_of(&row) {
            0 => index as i64 + 1,
            number => number,
        };
        row.remove(ROW_NUMBER_KEY);
        let mut map = IndexMap::new();
        map.insert("rowNumber".to_string(), Value::I64(number));
        map.insert(
            "source".to_string(),
            Value::Map(row.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        );
        match transformer.transform_row(&row) {
            Ok(values) => {
                map.insert("values".to_string(), Value::Map(values.into_iter().collect()));
                map.insert("errors".to_string(), Value::Map(IndexMap::new()));
            }
            Err(failed) => {
                map.insert("values".to_string(), Value::Map(IndexMap::new()));
                map.insert(
                    "errors".to_string(),
                    Value::Map(
                        failed
                            .into_iter()
                            .map(|(field, message)| {
                                (
                                    if field.is_empty() { "_row".to_string() } else { field },
                                    Value::string(message),
                                )
                            })
                            .collect(),
                    ),
                );
            }
        }
        out_rows.push(Value::Map(map));
    }
    let mut out = Out::new();
    let columns = moqui_lib::sync::lock(&headers).clone();
    out.insert(
        "columns".to_string(),
        Value::List(columns.into_iter().map(Value::String).collect()),
    );
    out.insert("rows".to_string(), Value::List(out_rows));
    Ok(out)
}

/// The words of a name, lower case: `productName`, `Product Name`, `PRODUCT_NAME` all `product name`.
fn words(name: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for ch in name.chars() {
        if ch == '_' || ch == '-' || ch.is_whitespace() || ch == '.' {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            previous_lower = false;
            continue;
        }
        if ch.is_uppercase() && previous_lower && !out.ends_with(' ') {
            out.push(' ');
        }
        previous_lower = ch.is_lowercase() || ch.is_ascii_digit();
        out.extend(ch.to_lowercase());
    }
    out.trim().to_string()
}

/// The edit distance of two names, for the nearest match.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = (previous + usize::from(ca != cb)).min(row[j] + 1).min(current + 1);
            previous = current;
        }
    }
    row[b.len()]
}

/// A pass of the matching: its name, and whether a column and a field match in it.
type NameMatch<'a> = (&'a str, &'a dyn Fn(&str, &str) -> bool);

/// The target field of each column: the same name (ignoring case), then the same words (`Product Name` and
/// `productName`), then the nearest name within a third of its length; `how` is `name`, `prettyName`, `nearest` or
/// `none`. A field is matched once, to its best column.
#[must_use]
pub fn match_fields(columns: &[String], fields: &[String]) -> Vec<(String, Option<String>, &'static str)> {
    let mut taken: Vec<bool> = vec![false; fields.len()];
    let mut matches: Vec<(String, Option<String>, &'static str)> =
        columns.iter().map(|column| (column.clone(), None, "none")).collect();
    let passes: [NameMatch<'_>; 2] = [
        ("name", &|column: &str, field: &str| column.eq_ignore_ascii_case(field)),
        ("prettyName", &|column: &str, field: &str| words(column) == words(field)),
    ];
    for (how, matches_name) in passes {
        for (index, column) in columns.iter().enumerate() {
            if matches[index].1.is_some() {
                continue;
            }
            if let Some(at) = (0..fields.len()).find(|at| !taken[*at] && matches_name(column, &fields[*at])) {
                taken[at] = true;
                matches[index] = (column.clone(), Some(fields[at].clone()), how);
            }
        }
    }
    for (index, column) in columns.iter().enumerate() {
        if matches[index].1.is_some() {
            continue;
        }
        let column_words = words(column).replace(' ', "");
        let best = (0..fields.len())
            .filter(|at| !taken[*at])
            .map(|at| (at, distance(&column_words, &words(&fields[at]).replace(' ', ""))))
            .min_by_key(|(_, distance)| *distance);
        if let Some((at, distance)) = best {
            if distance > 0 && distance * 3 <= column_words.chars().count().max(1) {
                taken[at] = true;
                matches[index] = (column.clone(), Some(fields[at].clone()), "nearest");
            }
        }
    }
    matches
}

/// The fields of a target: an entity's field names, or a service's in-parameter names.
fn target_fields(facade: &EntityFacade, kind: TargetKind, name: &str) -> Result<Vec<String>, LibError> {
    match kind {
        TargetKind::Entity => facade.with_entity(name, |entity, _, _| {
            Ok(entity.fields.iter().map(|field| field.name.clone()).collect())
        }),
        TargetKind::Service => match moqui_core::service::info::service_definition_info(name) {
            Some(info) => Ok(match info.get("inParameterNames") {
                Some(Value::List(names)) => names.iter().map(to_plain_string).collect(),
                _ => Vec::new(),
            }),
            None => Err(LibError::new(format!("no service named {name}"))),
        },
    }
}

/// `match#EtlFields`: the target field of each of `columns` for the target `targetName` (`targetTypeEnumId`
/// `EtltEntity` or `EtltService`). Out: `matches` (each `column`, `targetField`, `how`), `targetFields`.
pub(crate) fn match_etl_fields(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let kind = if text(parameters, "targetTypeEnumId") == "EtltService" {
        TargetKind::Service
    } else {
        TargetKind::Entity
    };
    let fields = match target_fields(facade, kind, &text(parameters, "targetName")) {
        Ok(fields) => fields,
        Err(err) => {
            add_error(err.to_string());
            return Ok(Out::new());
        }
    };
    let columns = list(parameters, "columns");
    let matches = match_fields(&columns, &fields)
        .into_iter()
        .map(|(column, field, how)| {
            let mut map = IndexMap::new();
            map.insert("column".to_string(), Value::String(column));
            map.insert("targetField".to_string(), field.map_or(Value::Null, Value::String));
            map.insert("how".to_string(), Value::string(how));
            Value::Map(map)
        })
        .collect();
    let mut out = Out::new();
    out.insert("matches".to_string(), Value::List(matches));
    out.insert(
        "targetFields".to_string(),
        Value::List(fields.into_iter().map(Value::String).collect()),
    );
    Ok(out)
}

/// `get#EtlMapping`: the saved mapping `mappingId` as an inline map. Out: `mapping`.
pub(crate) fn get_etl_mapping(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let mut out = Out::new();
    match Mapping::load(facade, &text(parameters, "mappingId")) {
        Ok(mapping) => {
            out.insert("mapping".to_string(), mapping.to_value());
        }
        Err(err) => add_error(err.to_string()),
    }
    Ok(out)
}

/// Save `mapping` (an inline map) as `EtlMapping` and its `EtlMappingField` rows, its fields replaced. A new mapping
/// gets an id. Returns the id.
fn save_mapping(facade: &EntityFacade, mapping: &Mapping) -> Result<String, LibError> {
    let user_id = moqui_core::user::current_user_id().ok().flatten();
    let Value::Map(map) = mapping.to_value() else {
        return Err(LibError::new("a mapping is a map"));
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(MAPPING)?;
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        let existing = match &mapping.mapping_id {
            Some(id) => facade.find(MAPPING)?.eq("mappingId", Value::string(id)).one()?,
            None => None,
        };
        let creating = existing.is_none();
        let mut row = match existing {
            Some(row) => row,
            None => {
                let mut row = facade.make_value(MAPPING)?;
                match &mapping.mapping_id {
                    Some(id) => row.set("mappingId", Value::string(id)),
                    None => row.set_sequenced_id_primary(facade)?,
                }
                row.set("ownerUserId", user_id.clone().map_or(Value::Null, Value::String));
                row
            }
        };
        let mapping_id = row.get("mappingId").map(to_plain_string).unwrap_or_default();
        for (name, value) in &map {
            if matches!(
                name.as_str(),
                "mappingId" | "fields" | "sourceConditions" | "ownerUserId"
            ) {
                continue;
            }
            row.set(name.as_str(), value.clone());
        }
        let conditions: IndexMap<String, Value> = mapping.source_conditions.clone().into_iter().collect();
        row.set(
            "sourceConditionsJson",
            if conditions.is_empty() {
                Value::Null
            } else {
                Value::string(moqui_lib::json::to_json(&Value::Map(conditions)).to_string())
            },
        );
        if creating {
            row.create(facade)?;
        } else {
            row.update(facade)?;
        }
        facade
            .find(MAPPING_FIELD)?
            .eq("mappingId", Value::string(&mapping_id))
            .delete_all()?;
        for (index, field) in mapping.fields.iter().enumerate() {
            let Value::Map(mut values) = field.to_value() else {
                continue;
            };
            // an inline value map is saved as a value map of its own
            if let Some(Value::Map(entries)) = values.shift_remove("valueMap") {
                let mut value_map = facade.make_value(VALUE_MAP)?;
                value_map.set_sequenced_id_primary(facade)?;
                value_map.set(
                    "description",
                    Value::string(format!("{} {}", mapping.description, field.target_field).trim()),
                );
                value_map.set("ownerUserId", user_id.clone().map_or(Value::Null, Value::String));
                value_map.create(facade)?;
                let value_map_id = value_map.get("valueMapId").map(to_plain_string).unwrap_or_default();
                for (from, to) in entries {
                    let mut entry = facade.make_value(VALUE_MAP_ENTRY)?;
                    entry.set("valueMapId", Value::string(&value_map_id));
                    entry.set("fromValue", Value::String(from));
                    entry.set("toValue", to);
                    entry.create(facade)?;
                }
                values.insert("valueMapId".to_string(), Value::String(value_map_id));
            }
            let mut field_row = facade.make_value(MAPPING_FIELD)?;
            field_row.set("mappingId", Value::string(&mapping_id));
            field_row.set("fieldSeqId", Value::string(format!("{:06}", index + 1)));
            for (name, value) in values {
                field_row.set(name.as_str(), value);
            }
            field_row.create(facade)?;
        }
        Ok(mapping_id)
    })
}

/// `store#EtlMapping`: create or update a mapping and its fields from `mapping` (an inline map, `mappingId` to
/// update; an inline `valueMap` of a field is saved as an `EtlValueMap`). Out: `mappingId`. Needs authorization for
/// all actions on the Tools app; updating needs the mapping's owner or `DATABASE_ADMIN`; a mapping with an expression
/// needs `GROOVY_SHELL_WEB`.
pub(crate) fn store_etl_mapping(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !may_change("Saving an ETL mapping") {
        return Ok(Out::new());
    }
    let mapping = match parameters.get("mapping") {
        Some(Value::Map(map)) => Mapping::from_map(map),
        _ => Err(LibError::new("store#EtlMapping needs a mapping")),
    };
    let mapping = match mapping {
        Ok(mapping) => mapping,
        Err(err) => {
            add_error(err.to_string());
            return Ok(Out::new());
        }
    };
    if !may_change_mapping(facade, mapping.mapping_id.as_deref(), "Changing that ETL mapping")
        || !mapping_allowed(facade, &mapping)
    {
        return Ok(Out::new());
    }
    let mut out = Out::new();
    match save_mapping(facade, &mapping) {
        Ok(mapping_id) => {
            out.insert("mappingId".to_string(), Value::String(mapping_id));
        }
        Err(err) => add_error(err.to_string()),
    }
    Ok(out)
}

/// `copy#EtlMapping`: a copy of mapping `mappingId` with its fields, as `newMappingId` (or a new id), its description
/// `description` (or "Copy of …"). Out: `mappingId`.
pub(crate) fn copy_etl_mapping(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !may_change("Copying an ETL mapping") {
        return Ok(Out::new());
    }
    let new_id = text(parameters, "newMappingId");
    if !may_change_mapping(facade, Some(&new_id), "Copying over that ETL mapping") {
        return Ok(Out::new());
    }
    let mut mapping = match Mapping::load(facade, &text(parameters, "mappingId")) {
        Ok(mapping) => mapping,
        Err(err) => {
            add_error(err.to_string());
            return Ok(Out::new());
        }
    };
    if !mapping_allowed(facade, &mapping) {
        return Ok(Out::new());
    }
    let mut out = Out::new();
    mapping.mapping_id = Some(new_id).filter(|id| !id.is_empty());
    let description = text(parameters, "description");
    mapping.description = if description.is_empty() {
        format!("Copy of {}", mapping.description)
    } else {
        description
    };
    match save_mapping(facade, &mapping) {
        Ok(mapping_id) => {
            out.insert("mappingId".to_string(), Value::String(mapping_id));
        }
        Err(err) => add_error(err.to_string()),
    }
    Ok(out)
}

/// `delete#EtlMapping`: delete mapping `mappingId` and its fields; its runs stay, their `mappingId` naming it still
/// (`EtlRun.mappingId` has no foreign key). Needs the mapping's owner or `DATABASE_ADMIN`.
pub(crate) fn delete_etl_mapping(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !may_change("Deleting an ETL mapping") {
        return Ok(Out::new());
    }
    let mapping_id = text(parameters, "mappingId");
    if !may_change_mapping(facade, Some(&mapping_id), "Deleting that ETL mapping") {
        return Ok(Out::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let deleted = facade.transactions_for(MAPPING).and_then(|tx| {
        tx.run(TransactionAttribute::UseOrBegin, None, || {
            facade
                .find(MAPPING_FIELD)?
                .eq("mappingId", Value::string(&mapping_id))
                .delete_all()?;
            facade
                .find(MAPPING)?
                .eq("mappingId", Value::string(&mapping_id))
                .delete_all()
        })
    });
    if let Err(err) = deleted {
        add_error(err.to_string());
    }
    Ok(Out::new())
}

/// `store#EtlValueMap`: create or update value map `valueMapId` (a new id when empty) with `description` and its
/// `entries` (a map from value to value, or a list of `fromValue`/`toValue` maps), the entries replaced. Out:
/// `valueMapId`. Updating needs the value map's owner or `DATABASE_ADMIN`.
pub(crate) fn store_etl_value_map(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !may_change("Saving an ETL value map") {
        return Ok(Out::new());
    }
    if !may_change_owned(
        facade,
        VALUE_MAP,
        "valueMapId",
        "ownerUserId",
        Some(&text(parameters, "valueMapId")),
        "Changing that ETL value map",
    ) {
        return Ok(Out::new());
    }
    let owner = current_user();
    let entries: Vec<(String, String)> = match parameters.get("entries") {
        Some(Value::Map(map)) => map
            .iter()
            .map(|(from, to)| (from.clone(), to_plain_string(to)))
            .collect(),
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some((
                    map.get("fromValue").map(to_plain_string).unwrap_or_default(),
                    map.get("toValue").map(to_plain_string).unwrap_or_default(),
                )),
                _ => None,
            })
            .filter(|(from, _)| !from.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    let value_map_id = text(parameters, "valueMapId");
    let description = text(parameters, "description");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let saved = facade.transactions_for(VALUE_MAP).and_then(|tx| {
        tx.run(TransactionAttribute::UseOrBegin, None, || {
            let existing = if value_map_id.is_empty() {
                None
            } else {
                facade
                    .find(VALUE_MAP)?
                    .eq("valueMapId", Value::string(&value_map_id))
                    .one()?
            };
            let id = match existing {
                Some(mut row) => {
                    row.set("description", Value::string(&description));
                    row.update(facade)?;
                    value_map_id.clone()
                }
                None => {
                    let mut row = facade.make_value(VALUE_MAP)?;
                    if value_map_id.is_empty() {
                        row.set_sequenced_id_primary(facade)?;
                    } else {
                        row.set("valueMapId", Value::string(&value_map_id));
                    }
                    row.set("description", Value::string(&description));
                    row.set(
                        "ownerUserId",
                        Some(owner.clone())
                            .filter(|owner| !owner.is_empty())
                            .map_or(Value::Null, Value::String),
                    );
                    row.create(facade)?;
                    row.get("valueMapId").map(to_plain_string).unwrap_or_default()
                }
            };
            facade
                .find(VALUE_MAP_ENTRY)?
                .eq("valueMapId", Value::string(&id))
                .delete_all()?;
            for (from, to) in &entries {
                let mut row = facade.make_value(VALUE_MAP_ENTRY)?;
                row.set("valueMapId", Value::string(&id));
                row.set("fromValue", Value::string(from));
                row.set("toValue", Value::string(to));
                row.create(facade)?;
            }
            Ok(id)
        })
    });
    let mut out = Out::new();
    match saved {
        Ok(id) => {
            out.insert("valueMapId".to_string(), Value::String(id));
        }
        Err(err) => add_error(err.to_string()),
    }
    Ok(out)
}

/// `delete#EtlValueMap`: delete value map `valueMapId` and its entries. Needs its owner or `DATABASE_ADMIN`.
pub(crate) fn delete_etl_value_map(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !may_change("Deleting an ETL value map") {
        return Ok(Out::new());
    }
    let value_map_id = text(parameters, "valueMapId");
    if !may_change_owned(
        facade,
        VALUE_MAP,
        "valueMapId",
        "ownerUserId",
        Some(&value_map_id),
        "Deleting that ETL value map",
    ) {
        return Ok(Out::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let deleted = facade.transactions_for(VALUE_MAP).and_then(|tx| {
        tx.run(TransactionAttribute::UseOrBegin, None, || {
            facade
                .find(VALUE_MAP_ENTRY)?
                .eq("valueMapId", Value::string(&value_map_id))
                .delete_all()?;
            facade
                .find(VALUE_MAP)?
                .eq("valueMapId", Value::string(&value_map_id))
                .delete_all()
        })
    });
    if let Err(err) = deleted {
        add_error(err.to_string());
    }
    Ok(Out::new())
}

fn row_value(row: &moqui_core::entity::EntityValue) -> Value {
    Value::Map(
        row.fields()
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect(),
    )
}

/// `get#EtlRuns`: the latest runs, newest first, of `mappingId` when given: the user's own, or every user's for a
/// `DATABASE_ADMIN` holder. In: `limit` (default 50). Out: `runs`.
pub(crate) fn get_etl_runs(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut find = facade.find(RUN)?.order_by("startTime", true);
    let mapping_id = text(parameters, "mappingId");
    if !mapping_id.is_empty() {
        find = find.eq("mappingId", Value::string(mapping_id));
    }
    if !is_database_admin() {
        find = find.eq("userId", Value::string(current_user()));
    }
    let limit = number(parameters, "limit").unwrap_or(50).clamp(1, 1000);
    let runs = find.limit(limit).list()?;
    let mut out = Out::new();
    out.insert("runs".to_string(), Value::List(runs.iter().map(row_value).collect()));
    Ok(out)
}

/// `get#EtlRun`: run `runId` and its error rows (the first `MAX_ERROR_ROWS`), for the run's user or a
/// `DATABASE_ADMIN` holder. With `includeErrorFile`, `errorFile` is the error file's text (every failed row). Out:
/// `run`, `errors`, `errorFile`.
pub(crate) fn get_etl_run(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let run_id = text(parameters, "runId");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let run = facade.find(RUN)?.eq("runId", Value::string(&run_id)).one()?;
    let Some(run) = run.filter(|run| {
        is_database_admin() || run.get("userId").map(to_plain_string).unwrap_or_default() == current_user()
    }) else {
        add_error(format!("no ETL run {run_id}"));
        return Ok(Out::new());
    };
    let errors = facade
        .find(RUN_ERROR)?
        .eq("runId", Value::string(&run_id))
        .order_by("errorSeqId", false)
        .list()?;
    let mut out = Out::new();
    if flag(parameters, "includeErrorFile") {
        let location = run.get("errorLocation").map(to_plain_string).unwrap_or_default();
        if location.starts_with(ERROR_ROOT) {
            match source::read_location(&location) {
                Ok(bytes) => {
                    out.insert(
                        "errorFile".to_string(),
                        Value::String(String::from_utf8_lossy(&bytes).into_owned()),
                    );
                }
                Err(err) => tracing::warn!(error = %err, run = %run_id, "Could not read the ETL run's error file"),
            }
        }
    }
    out.insert("run".to_string(), row_value(&run));
    out.insert(
        "errors".to_string(),
        Value::List(errors.iter().map(row_value).collect()),
    );
    Ok(out)
}

/// `load#SpreadsheetEntity` as an inline mapping (§10.3): its sheet's header as the fields (`fieldMap` renaming a
/// column), `loadMode` the load mode, written with the entity-auto services in the caller's transaction, stopping at
/// the first error. A column that is not a field of the entity is not set, and an empty cell is left out. Out:
/// `recordCount`. Writes a `DbaLoad` audit row.
fn inline_spreadsheet_load(parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, LibError> {
    let facade = ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| LibError::new("the entity facade is not installed"))?;
    let entity_name = text(parameters, "entityName");
    if entity_name.is_empty() {
        return Err(LibError::new("load#SpreadsheetEntity needs an entityName"));
    }
    let load_mode = match text(parameters, "loadMode").as_str() {
        "" | "store" => "EtllStore",
        "create" => "EtllCreate",
        other => return Err(LibError::new(format!("loadMode {other} is not create or store"))),
    };
    let mut options = SourceOptions {
        sheet_index: number(parameters, "sheetIndex").unwrap_or(0).max(0) as usize,
        ..SourceOptions::default()
    };
    let bytes = match parameters.get("contentBytes") {
        Some(Value::Bytes(bytes)) if !bytes.is_empty() => bytes.clone(),
        _ => {
            let location = text(parameters, "location");
            if location.is_empty() {
                return Err(LibError::new("A spreadsheet needs contentBytes or a location"));
            }
            // the same places an ETL run reads (the caller's uploads, the import roots)
            access::readable(&location)?;
            access::read_bytes(&location)
                .map_err(|_| LibError::new(format!("Could not find spreadsheet at [{location}]")))?
        }
    };
    let sheet_name = text(parameters, "sheetName");
    let sheet = moqui_core::service::spreadsheet::read_sheet_bytes(&bytes, &sheet_name, options.sheet_index)?;
    let header: Vec<String> = match (sheet.rows.first(), sheet.row_numbers.first()) {
        (Some(cells), Some(_)) => cells
            .iter()
            .map(|cell| to_plain_string(cell).trim().to_string())
            .collect(),
        _ => Vec::new(),
    };
    let header_row = sheet.row_numbers.first().copied().unwrap_or(1) as usize;
    let field_map: BTreeMap<String, String> = match parameters.get("fieldMap") {
        Some(Value::Map(map)) => map
            .iter()
            .map(|(column, field)| (column.clone(), to_plain_string(field)))
            .collect(),
        _ => BTreeMap::new(),
    };
    let entity_fields: Vec<String> = facade.with_entity(&entity_name, |entity, _, _| {
        Ok(entity.fields.iter().map(|field| field.name.clone()).collect())
    })?;
    let mut seen = std::collections::HashSet::new();
    let fields: Vec<Value> = header
        .iter()
        .filter(|column| !column.is_empty())
        .filter_map(|column| {
            let target = field_map.get(column).cloned().unwrap_or_else(|| column.clone());
            (entity_fields.contains(&target) && seen.insert(target.clone())).then(|| {
                MappingField {
                    target_field: target,
                    source_column: Some(column.clone()),
                    skip_if_empty: true,
                    ..MappingField::default()
                }
                .to_value()
            })
        })
        .collect();
    let mut map = IndexMap::new();
    map.insert("sourceTypeEnumId".to_string(), Value::string("EtlsFile"));
    map.insert("fileFormat".to_string(), Value::string("xlsx"));
    map.insert("sheetName".to_string(), Value::string(&sheet.sheet_name));
    map.insert("headerRow".to_string(), Value::I64(header_row as i64));
    map.insert("targetTypeEnumId".to_string(), Value::string("EtltEntity"));
    map.insert("targetName".to_string(), Value::string(&entity_name));
    map.insert("loadModeEnumId".to_string(), Value::string(load_mode));
    map.insert("fields".to_string(), Value::List(fields));
    let mapping = Mapping::from_map(&map)?;
    options.content = Some(bytes);
    let started = std::time::Instant::now();
    let outcome = run(
        &facade,
        &mapping,
        &RunOptions {
            source: options,
            join_caller: true,
            ..RunOptions::default()
        },
    );
    crate::audit::record(
        &facade,
        &crate::audit::AuditEntry {
            action: "DbaLoad",
            is_change: true,
            rows_affected: Some(outcome.load_count as i64),
            duration_ms: Some(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)),
            error_text: outcome.failure.clone(),
            detail_json: Some(format!(
                r#"{{"service":"load#SpreadsheetEntity","entity":"{entity_name}","loaded":{}}}"#,
                outcome.load_count
            )),
            ..crate::audit::AuditEntry::default()
        },
    );
    if let Some(failure) = outcome.failure {
        return Err(LibError::new(failure));
    }
    let mut out = BTreeMap::new();
    out.insert("recordCount".to_string(), Value::I64(outcome.load_count as i64));
    Ok(out)
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
