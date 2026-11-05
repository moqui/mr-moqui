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

//! Database jobs (plans/Tools-Database.md §6): DDL and maintenance that run on a service worker, one statement after
//! another, each on a connection of its own outside a transaction, with no statement timeout and the backend id kept
//! on its row so any cluster member can cancel it. A statement that takes a strong lock waits at most the job's lock
//! timeout and is tried again [`LOCK_RETRIES`] times; `CONCURRENTLY` statements and `VALIDATE CONSTRAINT` wait as
//! long as they need. A failed statement does not stop the ones that do not depend on it; one whose dependency did not
//! finish is skipped. The statuses are written as the runner goes: there is no status flow for them.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use moqui_core::context::ExecutionContext;
use moqui_core::driver::{DbCancel, DbConnection};
use moqui_core::entity::exec::{blank_sql, SqlSyntax};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;

use crate::group_db::{add_error, cell_number, flag, number, require_admin, rows_value, text};
use crate::group_db::{GroupDb, Out, Server};

pub const JOB: &str = "moqui.database.DatabaseJob";
pub const STATEMENT: &str = "moqui.database.DatabaseJobStatement";
const RUN_SERVICE: &str = "org.moqui.impl.DatabaseServices.run#DatabaseJob";
/// The notification topic of a finished job.
pub const TOPIC: &str = "DatabaseJob";

/// A job's lock timeout when it names none.
pub const DEFAULT_LOCK_TIMEOUT_MS: i64 = 5000;
/// How many more times a statement that timed out on its lock is tried.
pub const LOCK_RETRIES: u32 = 3;

pub const PENDING: &str = "DbjsPending";
pub const RUNNING: &str = "DbjsRunning";
pub const DONE: &str = "DbjsDone";
pub const FAILED: &str = "DbjsFailed";
pub const CANCELLED: &str = "DbjsCancelled";
pub const SKIPPED: &str = "DbjsSkipped";

/// Whether `sql` takes a lock that blocks writes (or reads) for as long as it runs, so it gets the job's lock timeout:
/// DDL, `REINDEX`, `CLUSTER`, `VACUUM FULL`, `OPTIMIZE TABLE`. Not the `CONCURRENTLY` forms, `VALIDATE CONSTRAINT`,
/// a plain `VACUUM` or `ANALYZE`.
#[must_use]
pub fn takes_strong_lock(sql: &str) -> bool {
    let blank = blank_sql(sql, SqlSyntax::Postgres).to_ascii_uppercase();
    let words: Vec<&str> = blank
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|word| !word.is_empty())
        .collect();
    let has = |word: &str| words.contains(&word);
    if has("CONCURRENTLY") || words.windows(2).any(|pair| pair == ["VALIDATE", "CONSTRAINT"]) {
        return false;
    }
    match words.first().copied() {
        Some("VACUUM") => has("FULL"),
        Some("ALTER" | "CREATE" | "DROP" | "REINDEX" | "CLUSTER" | "OPTIMIZE" | "TRUNCATE" | "LOCK") => true,
        _ => false,
    }
}

/// The index a `CREATE [UNIQUE] INDEX CONCURRENTLY IF NOT EXISTS name` makes, unquoted: before a rerun an invalid one
/// of that name, left by a failed or cancelled build, is dropped, or `IF NOT EXISTS` would keep it.
#[must_use]
pub fn concurrent_index_name(sql: &str) -> Option<String> {
    let upper = sql.to_ascii_uppercase();
    let marker = "INDEX CONCURRENTLY IF NOT EXISTS ";
    let at = upper.find(marker)? + marker.len();
    if !upper.trim_start().starts_with("CREATE ") {
        return None;
    }
    let rest = sql.get(at..)?.trim_start();
    if let Some(quoted) = rest.strip_prefix('"') {
        let mut name = String::new();
        let mut chars = quoted.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    name.push('"');
                    continue;
                }
                return Some(name);
            }
            name.push(ch);
        }
        return None;
    }
    let name: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect();
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

/// The jobs running on this instance: the canceller of the statement running now (only while it runs), and whether
/// the job was cancelled here.
struct Live {
    canceller: Option<Box<dyn DbCancel>>,
    cancelled: bool,
}

fn live() -> &'static Mutex<HashMap<String, Live>> {
    static LIVE: OnceLock<Mutex<HashMap<String, Live>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn instance_id() -> String {
    ExecutionContext::with_current(|ec| moqui_core::drain::instance_id(ec.factory())).unwrap_or_default()
}

fn now() -> Value {
    Value::Timestamp(jiff::Timestamp::now())
}

/// Write `fields` onto the row of `entity` with primary key `pk`, as the framework, in a transaction of its own.
fn update_row(
    facade: &EntityFacade,
    entity: &str,
    pk: &[(&str, &str)],
    fields: Vec<(&str, Value)>,
) -> Result<(), LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(entity)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let mut find = facade.find(entity)?;
        for (field, value) in pk {
            find = find.eq(field, Value::string(*value));
        }
        let Some(mut row) = find.one()? else {
            return Err(LibError::new(format!("no {entity} {pk:?}")));
        };
        for (field, value) in fields {
            row.set(field, value);
        }
        row.update(facade)
    })
}

/// [`update_row`], a failure logged.
fn write_row(facade: &EntityFacade, entity: &str, pk: &[(&str, &str)], fields: Vec<(&str, Value)>) {
    if let Err(err) = update_row(facade, entity, pk, fields) {
        tracing::error!(error = %err, entity, ?pk, "Could not write a database job row");
    }
}

fn find_job(facade: &EntityFacade, job_id: &str) -> Result<Option<EntityValue>, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade.find(JOB)?.eq("jobId", Value::string(job_id)).one()
}

fn job_statements(facade: &EntityFacade, job_id: &str) -> Result<Vec<EntityValue>, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    Ok(facade
        .find(STATEMENT)?
        .eq("jobId", Value::string(job_id))
        .order_by("statementSeqId", false)
        .list()?
        .into_values())
}

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

fn row_map(row: &EntityValue) -> Value {
    Value::Map(
        row.fields()
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect(),
    )
}

/// Mark the jobs left Running by an instance that is gone (this one before it started again, or a cluster member
/// that is no longer live) Failed, with their Running statements, once per process: the first jobs service call
/// does it, since the extensions boot before the entity facade and the cluster are up.
fn recover_stale(facade: &EntityFacade) {
    static DONE_ONCE: OnceLock<()> = OnceLock::new();
    if DONE_ONCE.get().is_some() {
        return;
    }
    let _ = DONE_ONCE.set(());
    let me = instance_id();
    let running = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        facade
            .find(JOB)
            .and_then(|find| find.eq("statusId", Value::string(RUNNING)).list())
    };
    let running = match running {
        Ok(rows) => rows.into_values(),
        Err(err) => {
            tracing::warn!(error = %err, "Could not read the running database jobs");
            return;
        }
    };
    for job in running {
        let job_id = field(&job, "jobId");
        let owner = field(&job, "instanceId");
        if lock(live()).contains_key(&job_id) {
            continue;
        }
        let gone = owner == me
            || owner.is_empty()
            || !ExecutionContext::with_current(|ec| {
                moqui_core::cluster::is_live_member(ec.factory(), facade, &owner).unwrap_or(false)
            })
            .unwrap_or(false);
        if !gone {
            continue;
        }
        tracing::warn!(job = %job_id, instance = %owner, "Marking a database job Failed: the server stopped");
        if let Ok(statements) = job_statements(facade, &job_id) {
            for statement in statements.iter().filter(|row| field(row, "statusId") == RUNNING) {
                let seq = field(statement, "statementSeqId");
                write_row(
                    facade,
                    STATEMENT,
                    &[("jobId", &job_id), ("statementSeqId", &seq)],
                    vec![
                        ("statusId", Value::string(FAILED)),
                        ("endTime", now()),
                        ("errorText", Value::string("the server stopped")),
                    ],
                );
            }
        }
        write_row(
            facade,
            JOB,
            &[("jobId", &job_id)],
            vec![("statusId", Value::string(FAILED)), ("endTime", now())],
        );
    }
}

/// The statements of a `create#DatabaseJob`: a list of maps (`statementText`, and `statementSeqId` and
/// `dependsOnSeqId` as a plan gives them) or of texts.
fn input_statements(parameters: &BTreeMap<String, Value>) -> Vec<(String, String, Option<String>)> {
    let items = match parameters.get("statements") {
        Some(Value::List(items)) => items.clone(),
        Some(Value::String(text)) => crate::group_db::split_script(text),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let default_seq = format!("{:03}", index + 1);
        match item {
            Value::Map(map) => {
                let text = map.get("statementText").map(to_plain_string).unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                let seq = map
                    .get("statementSeqId")
                    .map(to_plain_string)
                    .filter(|seq| !seq.is_empty())
                    .unwrap_or(default_seq);
                let depends = map
                    .get("dependsOnSeqId")
                    .map(to_plain_string)
                    .filter(|seq| !seq.is_empty());
                out.push((seq, text, depends));
            }
            other => {
                let text = to_plain_string(other);
                if !text.trim().is_empty() {
                    out.push((default_seq, text, None));
                }
            }
        }
    }
    out
}

/// `create#DatabaseJob`: a job of `statements` on `db`, Pending, and run on a service worker unless `run` is false.
/// In: `db`, `description`, `kindEnumId` (`DbjDdl` default, `DbjMaintenance`), `statements`, `lockTimeoutMs`
/// (default 5000), `run` (default true). Out: `jobId`, `statementCount`. Needs `DATABASE_ADMIN`.
pub(crate) fn create_database_job(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() || !require_admin("A database job") {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of(parameters, facade) else {
        return Ok(Out::new());
    };
    recover_stale(facade);
    let statements = input_statements(parameters);
    if statements.is_empty() {
        add_error("A database job needs at least one statement");
        return Ok(Out::new());
    }
    let mut seqs = std::collections::BTreeSet::new();
    for (seq, _, depends) in &statements {
        if !seqs.insert(seq.clone()) {
            add_error(format!("statement {seq} is in the job twice"));
            return Ok(Out::new());
        }
        if let Some(depends) = depends {
            if !seqs.contains(depends) {
                add_error(format!(
                    "statement {seq} depends on {depends}, which is not before it in the job"
                ));
                return Ok(Out::new());
            }
        }
    }
    let kind = Some(text(parameters, "kindEnumId")).filter(|kind| !kind.is_empty());
    let kind = kind.unwrap_or_else(|| "DbjDdl".to_string());
    let lock_timeout = number(parameters, "lockTimeoutMs")
        .unwrap_or(DEFAULT_LOCK_TIMEOUT_MS)
        .max(0);
    let user_id = moqui_core::user::current_user_id().ok().flatten();
    let created = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let tx = facade.transactions_for(JOB)?;
        tx.run(TransactionAttribute::ForceNew, None, || {
            let mut job = facade.make_value(JOB)?;
            job.set_sequenced_id_primary(facade)?;
            let job_id = field(&job, "jobId");
            job.set("db", Value::string(db.db_text()));
            job.set("description", Value::string(text(parameters, "description")));
            job.set("kindEnumId", Value::string(&kind));
            job.set("statusId", Value::string(PENDING));
            job.set("userId", user_id.clone().map_or(Value::Null, Value::String));
            job.set("instanceId", Value::string(instance_id()));
            job.set("createdDate", now());
            job.set("lockTimeoutMs", Value::I64(lock_timeout));
            job.create(facade)?;
            for (seq, sql, depends) in &statements {
                let mut row = facade.make_value(STATEMENT)?;
                row.set("jobId", Value::string(&job_id));
                row.set("statementSeqId", Value::string(seq));
                row.set("statementText", Value::string(sql));
                row.set("dependsOnSeqId", depends.clone().map_or(Value::Null, Value::String));
                row.set("statusId", Value::string(PENDING));
                row.create(facade)?;
            }
            Ok(job_id)
        })
    };
    let job_id = match created {
        Ok(job_id) => job_id,
        Err(err) => {
            add_error(format!("Could not create the database job: {err}"));
            return Ok(Out::new());
        }
    };
    if parameters.get("run").is_none_or(|_| flag(parameters, "run")) {
        let mut run = BTreeMap::new();
        run.insert("jobId".to_string(), Value::string(&job_id));
        // this service checked the Tools app and DATABASE_ADMIN; run#DatabaseJob checks DATABASE_ADMIN again as the
        // user on the worker
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        if let Err(err) = moqui_core::service::call::async_call(RUN_SERVICE, &run) {
            add_error(format!("Could not start database job {job_id}: {err}"));
        }
    }
    let mut out = Out::new();
    out.insert("jobId".to_string(), Value::string(job_id));
    out.insert(
        "statementCount".to_string(),
        Value::I64(i64::try_from(statements.len()).unwrap_or(i64::MAX)),
    );
    Ok(out)
}

/// How one statement ended.
enum Outcome {
    Done(u64),
    Failed(String),
    Cancelled(String),
}

/// `run#DatabaseJob`: run (or run again) the job `jobId` here and now; `create#DatabaseJob` calls it on a service
/// worker. The statements not Done run in order. Out: `statusId`, `doneCount`, `failedCount`, `skippedCount`. Needs
/// `DATABASE_ADMIN`.
pub(crate) fn run_database_job(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !require_admin("Running a database job") {
        return Ok(Out::new());
    }
    recover_stale(facade);
    let job_id = text(parameters, "jobId");
    let Some(job) = find_job(facade, &job_id)? else {
        add_error(format!("no database job {job_id}"));
        return Ok(Out::new());
    };
    let status = field(&job, "statusId");
    if status == RUNNING {
        add_error(format!("database job {job_id} is running"));
        return Ok(Out::new());
    }
    // a job cancelled before it ever started stays cancelled
    if status == CANCELLED && job.get("startTime").is_none_or(Value::is_null) {
        return Ok(Out::new());
    }
    let mut db_parameters = BTreeMap::new();
    db_parameters.insert("db".to_string(), Value::string(field(&job, "db")));
    let Some(db) = GroupDb::of(&db_parameters, facade) else {
        return Ok(Out::new());
    };
    let lock_timeout = cell_number(job.get("lockTimeoutMs")).unwrap_or(DEFAULT_LOCK_TIMEOUT_MS);
    lock(live()).insert(
        job_id.clone(),
        Live {
            canceller: None,
            cancelled: false,
        },
    );
    write_row(
        facade,
        JOB,
        &[("jobId", &job_id)],
        vec![
            ("statusId", Value::string(RUNNING)),
            ("instanceId", Value::string(instance_id())),
            ("startTime", now()),
            ("endTime", Value::Null),
        ],
    );
    let started = Instant::now();
    let statements = job_statements(facade, &job_id).unwrap_or_default();
    let mut finished: HashMap<String, bool> = HashMap::new();
    let (mut done, mut failed, mut skipped, mut cancelled) = (0, 0, 0, false);
    for statement in &statements {
        let seq = field(statement, "statementSeqId");
        let pk = [("jobId", job_id.as_str()), ("statementSeqId", seq.as_str())];
        if field(statement, "statusId") == DONE {
            finished.insert(seq, true);
            done += 1;
            continue;
        }
        if cancelled || job_cancelled(facade, &job_id) {
            cancelled = true;
            write_row(facade, STATEMENT, &pk, vec![("statusId", Value::string(CANCELLED))]);
            continue;
        }
        let depends = field(statement, "dependsOnSeqId");
        if !depends.is_empty() && !finished.get(&depends).copied().unwrap_or(false) {
            skipped += 1;
            finished.insert(seq.clone(), false);
            write_row(
                facade,
                STATEMENT,
                &pk,
                vec![
                    ("statusId", Value::string(SKIPPED)),
                    (
                        "errorText",
                        Value::string(format!("statement {depends}, which this one needs, did not finish")),
                    ),
                ],
            );
            continue;
        }
        write_row(
            facade,
            STATEMENT,
            &pk,
            vec![
                ("statusId", Value::string(RUNNING)),
                ("startTime", now()),
                ("endTime", Value::Null),
                ("errorText", Value::Null),
                ("backendPid", Value::Null),
            ],
        );
        let sql = field(statement, "statementText");
        let outcome = run_statement(facade, &db, &job_id, &seq, &sql, lock_timeout);
        let (status, rows, error) = match outcome {
            Outcome::Done(rows) => {
                done += 1;
                (DONE, Some(rows), None)
            }
            Outcome::Failed(error) => {
                failed += 1;
                tracing::warn!(job = %job_id, statement = %seq, error = %error, "A database job statement failed");
                (FAILED, None, Some(error))
            }
            Outcome::Cancelled(error) => {
                cancelled = true;
                (CANCELLED, None, Some(error))
            }
        };
        finished.insert(seq.clone(), status == DONE);
        write_row(
            facade,
            STATEMENT,
            &pk,
            vec![
                ("statusId", Value::string(status)),
                ("endTime", now()),
                (
                    "rowsAffected",
                    rows.map_or(Value::Null, |rows| Value::I64(i64::try_from(rows).unwrap_or(i64::MAX))),
                ),
                ("errorText", error.map_or(Value::Null, Value::String)),
            ],
        );
    }
    let cancelled = cancelled || lock(live()).remove(&job_id).is_some_and(|live| live.cancelled);
    let status = if cancelled {
        CANCELLED
    } else if failed > 0 || skipped > 0 {
        FAILED
    } else {
        DONE
    };
    write_row(
        facade,
        JOB,
        &[("jobId", &job_id)],
        vec![("statusId", Value::string(status)), ("endTime", now())],
    );
    let elapsed_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    tracing::info!(
        job = %job_id,
        db = %db.db_text(),
        status,
        done,
        failed,
        skipped,
        elapsed_ms,
        "Database job {job_id} ended {status}: {done} done, {failed} failed, {skipped} skipped of {} statements",
        statements.len()
    );
    let summary = format!(
        r#"{{"jobId":"{job_id}","statusId":"{status}","statements":{},"done":{done},"failed":{failed},"skipped":{skipped}}}"#,
        statements.len()
    );
    crate::audit::record(
        facade,
        &crate::audit::AuditEntry {
            action: "DbaJob",
            db: Some(db.db_text()),
            statement_text: Some(
                statements
                    .iter()
                    .map(|row| field(row, "statementText"))
                    .collect::<Vec<_>>()
                    .join(";\n"),
            ),
            is_change: true,
            duration_ms: Some(elapsed_ms),
            error_text: (status != DONE).then(|| format!("job ended {status}")),
            detail_json: Some(summary),
            ..crate::audit::AuditEntry::default()
        },
    );
    notify(&job, &job_id, status, done, failed, skipped);
    let mut out = Out::new();
    out.insert("statusId".to_string(), Value::string(status));
    out.insert("doneCount".to_string(), Value::I64(done));
    out.insert("failedCount".to_string(), Value::I64(failed));
    out.insert("skippedCount".to_string(), Value::I64(skipped));
    Ok(out)
}

/// Tell the job's user how it ended, on topic [`TOPIC`].
fn notify(job: &EntityValue, job_id: &str, status: &str, done: i64, failed: i64, skipped: i64) {
    let user_id = field(job, "userId");
    if user_id.is_empty() {
        return;
    }
    let mut message = IndexMap::new();
    message.insert("jobId".to_string(), Value::string(job_id));
    message.insert("description".to_string(), Value::string(field(job, "description")));
    message.insert("statusId".to_string(), Value::string(status));
    message.insert("doneCount".to_string(), Value::I64(done));
    message.insert("failedCount".to_string(), Value::I64(failed));
    message.insert("skippedCount".to_string(), Value::I64(skipped));
    let mut note = moqui_core::notify::NotificationMessage::new();
    note.topic(TOPIC)
        .title(format!(
            "Database job {job_id} {}",
            status.trim_start_matches("Dbjs").to_ascii_lowercase()
        ))
        .notification_type(if status == DONE { "success" } else { "warning" })
        .message_map(message)
        .user_id(user_id);
    if let Err(err) = note.send() {
        tracing::warn!(job = %job_id, error = %err, "Could not send the database job notification");
    }
}

/// Whether the job's row says it was cancelled (by this instance or another).
fn job_cancelled(facade: &EntityFacade, job_id: &str) -> bool {
    find_job(facade, job_id)
        .ok()
        .flatten()
        .is_some_and(|job| field(&job, "statusId") == CANCELLED)
}

/// Whether `error` is a statement that waited its lock timeout out.
fn lock_timed_out(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("lock timeout") || error.contains("lock wait timeout") || error.contains("55p03")
}

/// Run one statement on a connection of its own, as [`run_database_job`] does, trying a lock timeout again.
fn run_statement(facade: &EntityFacade, db: &GroupDb, job_id: &str, seq: &str, sql: &str, lock_ms: i64) -> Outcome {
    let strong = takes_strong_lock(sql);
    let mut attempt = 0;
    loop {
        let result = db.tx.run(TransactionAttribute::IgnoreSuspend, None, || {
            db.tx
                .with_connection(|conn| on_connection(facade, db, conn, job_id, seq, sql, strong.then_some(lock_ms)))
        });
        let cancelled_here = lock(live()).get(job_id).is_some_and(|live| live.cancelled);
        match result {
            Ok(rows) => return Outcome::Done(rows),
            Err(err) => {
                let error = err.to_string();
                if cancelled_here || job_cancelled(facade, job_id) {
                    return Outcome::Cancelled(error);
                }
                if strong && attempt < LOCK_RETRIES && lock_timed_out(&error) {
                    attempt += 1;
                    let pause = Duration::from_millis(u64::try_from(lock_ms.clamp(100, 2000)).unwrap_or(2000));
                    tracing::info!(job = %job_id, statement = %seq, attempt, "Lock timeout, trying the statement again");
                    std::thread::sleep(pause);
                    continue;
                }
                return Outcome::Failed(error);
            }
        }
    }
}

/// The statement on `conn`: the session's timeouts set (none for the statement, the lock's when `lock_ms`), the
/// backend id written, the canceller live while it runs, an invalid index of the name it makes dropped first, and the
/// session's timeouts put back after.
fn on_connection(
    facade: &EntityFacade,
    db: &GroupDb,
    conn: &mut dyn DbConnection,
    job_id: &str,
    seq: &str,
    sql: &str,
    lock_ms: Option<i64>,
) -> Result<u64, LibError> {
    let (setup, reset) = session_settings(db.server, lock_ms);
    for statement in &setup {
        if let Err(err) = conn.execute(statement, &[]) {
            tracing::debug!(error = %err, statement, "A database job session setting was refused");
        }
    }
    let backend = conn.backend_id();
    write_row(
        facade,
        STATEMENT,
        &[("jobId", job_id), ("statementSeqId", seq)],
        vec![("backendPid", backend.map_or(Value::Null, Value::I64))],
    );
    let result = (|| {
        if db.is_postgres() {
            if let Some(index) = concurrent_index_name(sql) {
                drop_invalid_index(conn, db, &index)?;
            }
        }
        {
            let mut jobs = lock(live());
            if let Some(entry) = jobs.get_mut(job_id) {
                if entry.cancelled {
                    return Err(LibError::new("the job was cancelled"));
                }
                entry.canceller = Some(conn.canceller());
            }
        }
        let ran = conn.execute(sql, &[]);
        // disarmed under the lock a cancel holds, before the connection goes back
        if let Some(entry) = lock(live()).get_mut(job_id) {
            entry.canceller = None;
        }
        ran
    })();
    for statement in &reset {
        if let Err(err) = conn.execute(statement, &[]) {
            tracing::debug!(error = %err, statement, "A database job session setting was not put back");
        }
    }
    result
}

/// The session settings of a job statement, and the statements that put them back.
fn session_settings(server: Server, lock_ms: Option<i64>) -> (Vec<String>, Vec<String>) {
    match server {
        Server::Postgres => {
            let lock = lock_ms.map_or_else(|| "0".to_string(), |ms| format!("{ms}"));
            (
                vec![
                    "SET statement_timeout = 0".to_string(),
                    format!("SET lock_timeout = {lock}"),
                ],
                vec!["RESET statement_timeout".to_string(), "RESET lock_timeout".to_string()],
            )
        }
        Server::Mysql | Server::Mariadb => {
            let timeout = if server == Server::Mysql {
                "SET SESSION max_execution_time = 0"
            } else {
                "SET SESSION max_statement_time = 0"
            };
            let mut setup = vec![timeout.to_string()];
            if let Some(ms) = lock_ms {
                // seconds, at least one
                setup.push(format!(
                    "SET SESSION lock_wait_timeout = {}",
                    ((ms + 999) / 1000).max(1)
                ));
            }
            (setup, vec!["SET SESSION lock_wait_timeout = DEFAULT".to_string()])
        }
    }
}

/// Drop the invalid index `index` of the current schema, which a failed `CREATE INDEX CONCURRENTLY` leaves.
fn drop_invalid_index(conn: &mut dyn DbConnection, db: &GroupDb, index: &str) -> Result<(), LibError> {
    let rows = conn.query_once(INVALID_INDEX, &[Value::string(index)])?;
    if rows.is_empty() {
        return Ok(());
    }
    tracing::info!(
        index,
        "Dropping the invalid index a failed build left, before building it again"
    );
    let sql = format!(
        "DROP INDEX CONCURRENTLY IF EXISTS {}",
        moqui_db::sql::quote_ident(&db.dialect, index)
    );
    conn.execute(&sql, &[]).map(|_| ())
}

const INVALID_INDEX: &str = "SELECT i.relname AS index_name FROM pg_index x JOIN pg_class i ON i.oid = x.indexrelid \
     JOIN pg_namespace n ON n.oid = i.relnamespace WHERE i.relname = ? AND n.nspname = current_schema() \
     AND NOT x.indisvalid";

/// `cancel#DatabaseJob`: stop the job `jobId`: its row is marked Cancelled, so the statements not started do not
/// start, and the running statement is cancelled, with the local canceller when it runs here, else by its backend id
/// (`pg_cancel_backend`, `KILL QUERY`) on a connection of its own. Out: `cancelled` (Boolean), `how` (`local`,
/// `backend`, `notStarted` or `none`). Needs `DATABASE_ADMIN`; writes a `DbaCancel` audit row.
pub(crate) fn cancel_database_job(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() || !require_admin("Cancelling a database job") {
        return Ok(Out::new());
    }
    let job_id = text(parameters, "jobId");
    let Some(job) = find_job(facade, &job_id)? else {
        add_error(format!("no database job {job_id}"));
        return Ok(Out::new());
    };
    let status = field(&job, "statusId");
    let mut out = Out::new();
    if !matches!(status.as_str(), PENDING | RUNNING) {
        out.insert("cancelled".to_string(), Value::Bool(false));
        out.insert("how".to_string(), Value::string("none"));
        return Ok(out);
    }
    write_row(
        facade,
        JOB,
        &[("jobId", &job_id)],
        vec![("statusId", Value::string(CANCELLED))],
    );
    let mut how = "notStarted";
    // the job's row names the instance it runs on; this one cancels it in process only when that is this one
    let here = field(&job, "instanceId") == instance_id();
    let local = here && {
        let mut jobs = lock(live());
        match jobs.get_mut(&job_id) {
            Some(entry) => {
                entry.cancelled = true;
                if let Some(canceller) = &entry.canceller {
                    if let Err(err) = canceller.cancel() {
                        tracing::warn!(job = %job_id, error = %err, "Could not cancel the running job statement");
                    }
                }
                true
            }
            None => false,
        }
    };
    if local {
        how = "local";
    } else if status == RUNNING {
        let running = job_statements(facade, &job_id)
            .unwrap_or_default()
            .into_iter()
            .find(|row| field(row, "statusId") == RUNNING);
        let backend = running.as_ref().and_then(|row| cell_number(row.get("backendPid")));
        let mut db_parameters = BTreeMap::new();
        db_parameters.insert("db".to_string(), Value::string(field(&job, "db")));
        if let (Some(pid), Some(db)) = (backend, GroupDb::of(&db_parameters, facade)) {
            match signal_backend(&db, pid, false) {
                Ok(_) => how = "backend",
                Err(err) => add_error(format!("Could not cancel backend {pid}: {err}")),
            }
        }
    }
    crate::audit::record(
        facade,
        &crate::audit::AuditEntry {
            action: "DbaCancel",
            db: Some(field(&job, "db")),
            is_change: true,
            detail_json: Some(format!(r#"{{"jobId":"{job_id}","how":"{how}"}}"#)),
            ..crate::audit::AuditEntry::default()
        },
    );
    out.insert("cancelled".to_string(), Value::Bool(true));
    out.insert("how".to_string(), Value::string(how));
    Ok(out)
}

/// Cancel (or with `terminate`, end) the session `pid` of `db`'s server on a connection of its own: Postgres
/// `pg_cancel_backend` / `pg_terminate_backend`, MySQL `KILL QUERY` / `KILL`. Whether the server signalled it.
pub(crate) fn signal_backend(db: &GroupDb, pid: i64, terminate: bool) -> Result<bool, LibError> {
    if db.is_postgres() {
        let function = if terminate {
            "pg_terminate_backend"
        } else {
            "pg_cancel_backend"
        };
        let rows = db.query(&format!("SELECT {function}({pid}) AS signalled"), &[])?;
        Ok(rows
            .first()
            .and_then(|row| row.get("signalled"))
            .is_some_and(|value| matches!(value, Value::Bool(true))))
    } else {
        let kill = if terminate {
            format!("KILL {pid}")
        } else {
            format!("KILL QUERY {pid}")
        };
        db.tx
            .run(TransactionAttribute::IgnoreSuspend, None, || {
                db.tx.with_connection(|conn| conn.execute(&kill, &[]))
            })
            .map(|_| true)
    }
}

/// `get#DatabaseJobStatus`: the job `jobId`, its statements and, for a statement running now, the server's progress
/// and what blocks it. Out: `job` (Map), `statements` (List of Map), `progress` (List of Map: Postgres
/// `pg_stat_progress_create_index`, `_vacuum`, `_analyze`; MySQL `events_stages_current`; MariaDB `PROCESSLIST`
/// progress), `blocking` (List of Map: the sessions `pg_blocking_pids` names, from `pg_stat_activity`), `reasons` (Map:
/// what could not be read and why).
pub(crate) fn get_database_job_status(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    recover_stale(facade);
    let job_id = text(parameters, "jobId");
    let Some(job) = find_job(facade, &job_id)? else {
        add_error(format!("no database job {job_id}"));
        return Ok(Out::new());
    };
    let statements = job_statements(facade, &job_id)?;
    let mut progress = Vec::new();
    let mut blocking = Vec::new();
    let mut reasons = IndexMap::new();
    let mut db_parameters = BTreeMap::new();
    db_parameters.insert("db".to_string(), Value::string(field(&job, "db")));
    let running_pids: Vec<i64> = statements
        .iter()
        .filter(|row| field(row, "statusId") == RUNNING)
        .filter_map(|row| cell_number(row.get("backendPid")))
        .collect();
    if !running_pids.is_empty() {
        if let Some(db) = GroupDb::of(&db_parameters, facade) {
            for pid in running_pids {
                for sql in progress_queries(db.server, pid) {
                    progress.extend(db.query_or_note(&sql, &[], "progress", &mut reasons));
                }
                if db.is_postgres() {
                    blocking.extend(db.query_or_note(&blocking_query(pid), &[], "blocking", &mut reasons));
                }
            }
        }
    }
    let mut out = Out::new();
    out.insert("job".to_string(), row_map(&job));
    out.insert(
        "statements".to_string(),
        Value::List(statements.iter().map(row_map).collect()),
    );
    out.insert("progress".to_string(), rows_value(progress));
    out.insert("blocking".to_string(), rows_value(blocking));
    out.insert("reasons".to_string(), Value::Map(reasons));
    Ok(out)
}

/// The progress queries of the session `pid` (a number the runner recorded, written into the text).
fn progress_queries(server: Server, pid: i64) -> Vec<String> {
    match server {
        Server::Postgres => vec![
            format!(
                "SELECT p.pid, 'create index' AS command, p.phase, p.blocks_total, p.blocks_done, p.tuples_total, \
                 p.tuples_done, p.lockers_total, p.lockers_done, p.current_locker_pid, c.relname AS table_name, \
                 i.relname AS index_name FROM pg_stat_progress_create_index p \
                 LEFT JOIN pg_class c ON c.oid = p.relid LEFT JOIN pg_class i ON i.oid = p.index_relid \
                 WHERE p.pid = {pid}"
            ),
            format!(
                "SELECT p.pid, 'vacuum' AS command, p.phase, p.heap_blks_total AS blocks_total, \
                 p.heap_blks_vacuumed AS blocks_done, c.relname AS table_name FROM pg_stat_progress_vacuum p \
                 LEFT JOIN pg_class c ON c.oid = p.relid WHERE p.pid = {pid}"
            ),
            format!(
                "SELECT p.pid, 'analyze' AS command, p.phase, p.sample_blks_total AS blocks_total, \
                 p.sample_blks_scanned AS blocks_done, c.relname AS table_name FROM pg_stat_progress_analyze p \
                 LEFT JOIN pg_class c ON c.oid = p.relid WHERE p.pid = {pid}"
            ),
        ],
        Server::Mysql => vec![format!(
            "SELECT t.PROCESSLIST_ID AS pid, s.EVENT_NAME AS phase, s.WORK_COMPLETED AS blocks_done, \
             s.WORK_ESTIMATED AS blocks_total FROM performance_schema.events_stages_current s \
             JOIN performance_schema.threads t ON t.THREAD_ID = s.THREAD_ID WHERE t.PROCESSLIST_ID = {pid}"
        )],
        Server::Mariadb => vec![format!(
            "SELECT ID AS pid, STATE AS phase, STAGE AS stage, MAX_STAGE AS max_stage, PROGRESS AS progress \
             FROM information_schema.PROCESSLIST WHERE ID = {pid}"
        )],
    }
}

/// The sessions blocking `pid`.
fn blocking_query(pid: i64) -> String {
    format!(
        "SELECT {pid} AS blocked_pid, b.pid, b.usename AS user_name, b.application_name, b.state, b.query, \
         b.xact_start, b.query_start FROM pg_stat_activity b WHERE b.pid = ANY (pg_blocking_pids({pid}))"
    )
}

/// `get#DatabaseJobs`: the latest jobs, newest first. In: `statusId` (optional), `limit` (default 50). Out: `jobs`
/// (List of Map).
pub(crate) fn get_database_jobs(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    recover_stale(facade);
    let limit = number(parameters, "limit").unwrap_or(50).clamp(1, 1000);
    let status = text(parameters, "statusId");
    let jobs = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let mut find = facade.find(JOB)?.order_by("createdDate", true);
        if !status.is_empty() {
            find = find.eq("statusId", Value::string(status));
        }
        find.limit(limit).list()?
    };
    let mut out = Out::new();
    out.insert("jobs".to_string(), Value::List(jobs.iter().map(row_map).collect()));
    Ok(out)
}

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, create_database_job)?;
    rust_service!(map, run_database_job)?;
    rust_service!(map, cancel_database_job)?;
    rust_service!(map, get_database_job_status)?;
    rust_service!(map, get_database_jobs)?;
    Ok(())
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
