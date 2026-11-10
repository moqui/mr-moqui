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

//! Database jobs (plans/archive/Tools-Database.md §6): DDL and maintenance that run on a service worker, one statement
//! after another. A job is only ever a plan the server made (`plan#SchemaChanges`, `plan#TableMaintenance`,
//! `plan#LoadWrap`, [`create_plan_job`]): no service takes statement text from the caller, so a job is not a way
//! around the SQL runner's own permission. Each statement runs on a connection of its own outside a transaction, which
//! is closed after it, with no statement timeout and its backend id and backend start kept on its row so any cluster
//! member can cancel it. A statement that is not one of the known light forms waits at most the job's lock timeout
//! and is tried again [`LOCK_RETRIES`] times. A failed statement does not stop the ones that do not depend on it; one
//! whose dependency did not finish is skipped. The statuses are written as the runner goes: there is no status flow.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
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

use crate::group_db::{add_error, cell_number, cell_text, flag, number, require_admin, rows_value, text};
use crate::group_db::{GroupDb, Out, Server};
use crate::schema::PlanStatement;

pub const JOB: &str = "moqui.database.DatabaseJob";
pub const STATEMENT: &str = "moqui.database.DatabaseJobStatement";
const RUN_SERVICE: &str = "org.moqui.dev.DatabaseServices.run#DatabaseJob";
/// The notification topic of a finished job.
pub const TOPIC: &str = "DatabaseJob";

/// A job's lock timeout when it names none.
pub const DEFAULT_LOCK_TIMEOUT_MS: i64 = 5000;
/// How many more times a statement that timed out on its lock is tried.
pub const LOCK_RETRIES: u32 = 3;
/// How often the stale-job pass runs again.
pub const RECOVER_EVERY: Duration = Duration::from_secs(300);
/// A plan job never started this long after it was made is purged.
pub const ABANDONED_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

pub const PENDING: &str = "DbjsPending";
pub const RUNNING: &str = "DbjsRunning";
pub const DONE: &str = "DbjsDone";
pub const FAILED: &str = "DbjsFailed";
pub const CANCELLED: &str = "DbjsCancelled";
pub const SKIPPED: &str = "DbjsSkipped";

/// The words and single punctuation of `sql` outside its text, quoted names and comments, upper case.
fn tokens(sql: &str) -> Vec<String> {
    let blank = blank_sql(sql, SqlSyntax::Postgres).to_ascii_uppercase();
    let mut out = Vec::new();
    let mut word = String::new();
    for ch in blank.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            word.push(ch);
            continue;
        }
        if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
        if matches!(ch, '(' | ')' | ',' | '"' | '`') {
            out.push(ch.to_string());
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// Whether `sql` is one of the known light forms, which take no lock that blocks writes for long, so it gets no lock
/// timeout: `CREATE [UNIQUE] INDEX CONCURRENTLY`, `DROP INDEX CONCURRENTLY`, `REINDEX … CONCURRENTLY`,
/// `ALTER TABLE t VALIDATE CONSTRAINT c` alone, a `VACUUM` without `FULL`, and `ANALYZE`. Matched on the statement's
/// leading form; every other statement takes the job's lock timeout.
#[must_use]
pub fn is_light(sql: &str) -> bool {
    let tokens = tokens(sql);
    let words: Vec<&str> = tokens.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["CREATE", "INDEX", "CONCURRENTLY", ..] | ["CREATE", "UNIQUE", "INDEX", "CONCURRENTLY", ..] => true,
        ["DROP", "INDEX", "CONCURRENTLY", ..] => true,
        ["REINDEX", rest @ ..] => {
            // REINDEX [(options)] {INDEX | TABLE | SCHEMA | DATABASE | SYSTEM} CONCURRENTLY name
            let rest = skip_options(rest);
            matches!(rest, [kind, "CONCURRENTLY", ..] if matches!(*kind, "INDEX" | "TABLE" | "SCHEMA" | "DATABASE"))
        }
        // ALTER TABLE [IF EXISTS] [ONLY] name VALIDATE CONSTRAINT name, and no other action: no comma, and no other
        // action's keyword outside the quoted names
        ["ALTER", "TABLE", rest @ ..] => {
            let validates = rest.windows(2).any(|pair| pair == ["VALIDATE", "CONSTRAINT"]);
            let other = rest.iter().any(|token| *token == "," || ALTER_ACTIONS.contains(token));
            validates && !other
        }
        ["VACUUM", rest @ ..] => {
            let options: &[&str] = match rest {
                ["(", ..] => {
                    let end = rest.iter().position(|token| *token == ")").unwrap_or(rest.len());
                    &rest[..end]
                }
                _ => {
                    let end = rest
                        .iter()
                        .position(|token| !matches!(*token, "FULL" | "FREEZE" | "VERBOSE" | "ANALYZE"))
                        .unwrap_or(rest.len());
                    &rest[..end]
                }
            };
            !options.contains(&"FULL")
        }
        ["ANALYZE", ..] => true,
        _ => false,
    }
}

/// The keywords that begin an `ALTER TABLE` action other than `VALIDATE CONSTRAINT`.
const ALTER_ACTIONS: &[&str] = &[
    "ADD", "DROP", "ALTER", "RENAME", "SET", "RESET", "OWNER", "ENABLE", "DISABLE", "CLUSTER", "INHERIT", "NO",
    "ATTACH", "DETACH", "REPLICA", "OF", "NOT", "TYPE", "USING", "FORCE",
];

/// Past a leading `( … )` option list.
fn skip_options<'a>(tokens: &'a [&'a str]) -> &'a [&'a str] {
    match tokens {
        ["(", rest @ ..] => {
            let end = rest
                .iter()
                .position(|token| *token == ")")
                .map_or(rest.len(), |at| at + 1);
            &rest[end..]
        }
        _ => tokens,
    }
}

/// Whether `sql` takes the job's lock timeout: every statement but the [`is_light`] forms.
#[must_use]
pub fn takes_strong_lock(sql: &str) -> bool {
    !is_light(sql)
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

/// The sequence id of the statement at `index` (from 0): six digits, so the ids sort as their numbers do.
#[must_use]
pub fn seq_id(index: usize) -> String {
    format!("{:06}", index + 1)
}

/// The jobs running on this instance: the canceller of the statement running now (only while it runs), and whether
/// the job was cancelled here. A job is in it from the claim of its run to its end, so a second run is refused.
struct Live {
    canceller: Option<Box<dyn DbCancel>>,
    cancelled: bool,
}

fn live() -> &'static Mutex<HashMap<String, Live>> {
    static LIVE: std::sync::OnceLock<Mutex<HashMap<String, Live>>> = std::sync::OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn instance_id() -> String {
    ExecutionContext::with_current(|ec| moqui_core::drain::instance_id(ec.factory())).unwrap_or_default()
}

fn now() -> Value {
    Value::Timestamp(jiff::Timestamp::now())
}

fn millis(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Timestamp(at) => Some(at.as_millisecond()),
        Value::Null => None,
        other => cell_number(Some(other)),
    }
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

/// Move the job from `from` to `to` (with `fields`) only when its row still has `from`: the number of rows changed.
fn transition(
    facade: &EntityFacade,
    job_id: &str,
    from: &str,
    to: &str,
    fields: Vec<(&str, Value)>,
) -> Result<i64, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(JOB)?;
    let mut set: Vec<(String, Value)> = vec![("statusId".to_string(), Value::string(to))];
    set.extend(fields.into_iter().map(|(name, value)| (name.to_string(), value)));
    tx.run(TransactionAttribute::ForceNew, None, || {
        facade
            .find(JOB)?
            .eq("jobId", Value::string(job_id))
            .eq("statusId", Value::string(from))
            .update_all(set)
    })
}

fn find_job(facade: &EntityFacade, job_id: &str) -> Result<Option<EntityValue>, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade.find(JOB)?.eq("jobId", Value::string(job_id)).one()
}

/// The job's statements in their order: by the number of their sequence id, so an id of another width sorts right.
fn job_statements(facade: &EntityFacade, job_id: &str) -> Result<Vec<EntityValue>, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut rows = facade
        .find(STATEMENT)?
        .eq("jobId", Value::string(job_id))
        .list()?
        .into_values();
    rows.sort_by_key(|row| {
        let seq = field(row, "statementSeqId");
        (seq.parse::<u64>().unwrap_or(u64::MAX), seq)
    });
    Ok(rows)
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

/// The stale-job pass, at most every [`RECOVER_EVERY`]: a job left Running by an instance that is gone is marked
/// Failed with its Running statements (this one, for a job started before this process started; or another that is no
/// longer a live cluster member, when the member rows say so), and a plan job never started for a day is purged.
fn recover_stale(facade: &EntityFacade) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = lock(&LAST);
        if last.is_some_and(|at| at.elapsed() < RECOVER_EVERY) {
            return;
        }
        *last = Some(Instant::now());
    }
    let me = instance_id();
    let process_start = ExecutionContext::with_current(|ec| ec.factory().started_millis()).unwrap_or(0);
    let rows = |status: &str| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        facade
            .find(JOB)
            .and_then(|find| find.eq("statusId", Value::string(status)).list())
            .map(moqui_core::entity::EntityList::into_values)
    };
    match rows(RUNNING) {
        Ok(running) => {
            for job in running {
                fail_if_gone(facade, &job, &me, process_start);
            }
        }
        Err(err) => tracing::warn!(error = %err, "Could not read the running database jobs"),
    }
    match rows(PENDING) {
        Ok(pending) => {
            let cutoff = jiff::Timestamp::now().as_millisecond() - ABANDONED_AFTER_MS;
            for job in pending {
                let never_started = job.get("startTime").is_none_or(Value::is_null);
                if never_started && millis(job.get("createdDate")).is_some_and(|created| created < cutoff) {
                    purge(facade, &field(&job, "jobId"));
                }
            }
        }
        Err(err) => tracing::warn!(error = %err, "Could not read the pending database jobs"),
    }
}

fn fail_if_gone(facade: &EntityFacade, job: &EntityValue, me: &str, process_start: i64) {
    let job_id = field(job, "jobId");
    let owner = field(job, "instanceId");
    if lock(live()).contains_key(&job_id) {
        return;
    }
    let gone = if owner == me {
        // this instance's, from before this process started
        millis(job.get("startTime")).is_some_and(|started| started < process_start)
    } else if owner.is_empty() {
        true
    } else {
        // gone only when the member rows say it is not live; an error or no answer keeps the job
        let live =
            ExecutionContext::with_current(|ec| moqui_core::cluster::is_live_member(ec.factory(), facade, &owner));
        matches!(live, Some(Ok(false)))
    };
    if !gone {
        return;
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
    if let Err(err) = transition(facade, &job_id, RUNNING, FAILED, vec![("endTime", now())]) {
        tracing::error!(error = %err, job = %job_id, "Could not mark the stale database job Failed");
    }
}

/// Delete a plan job that was never run, with its statements.
fn purge(facade: &EntityFacade, job_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let result = facade.transactions_for(JOB).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            facade
                .find(STATEMENT)?
                .eq("jobId", Value::string(job_id))
                .delete_all()?;
            facade
                .find(JOB)?
                .eq("jobId", Value::string(job_id))
                .eq("statusId", Value::string(PENDING))
                .delete_all()
        })
    });
    match result {
        Ok(_) => tracing::info!(job = %job_id, "Purged a database plan job that was never run"),
        Err(err) => tracing::warn!(error = %err, job = %job_id, "Could not purge an abandoned database plan job"),
    }
}

/// Store a plan the server made as a Pending job, with its statements, `db` and creator, and write a `DbaJob` audit
/// row of the texts. `None` (with an error added) when it could not be stored. Only the plan services call this.
pub(crate) fn create_plan_job(
    facade: &EntityFacade,
    db: &GroupDb,
    kind: &str,
    description: &str,
    lock_timeout_ms: Option<i64>,
    statements: &[PlanStatement],
) -> Option<String> {
    recover_stale(facade);
    let lock_timeout = lock_timeout_ms.unwrap_or(DEFAULT_LOCK_TIMEOUT_MS).max(0);
    let user_id = moqui_core::user::current_user_id().ok().flatten();
    let created = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        facade.transactions_for(JOB).and_then(|tx| {
            tx.run(TransactionAttribute::ForceNew, None, || {
                let mut job = facade.make_value(JOB)?;
                job.set_sequenced_id_primary(facade)?;
                let job_id = field(&job, "jobId");
                job.set("db", Value::string(db.db_text()));
                job.set("description", Value::string(description));
                job.set("kindEnumId", Value::string(kind));
                job.set("statusId", Value::string(PENDING));
                job.set("userId", user_id.clone().map_or(Value::Null, Value::String));
                job.set("instanceId", Value::string(instance_id()));
                job.set("createdDate", now());
                job.set("lockTimeoutMs", Value::I64(lock_timeout));
                job.create(facade)?;
                for statement in statements {
                    let mut row = facade.make_value(STATEMENT)?;
                    row.set("jobId", Value::string(&job_id));
                    row.set("statementSeqId", Value::string(&statement.statement_seq_id));
                    row.set("statementText", Value::string(&statement.statement_text));
                    row.set(
                        "dependsOnSeqId",
                        statement.depends_on_seq_id.clone().map_or(Value::Null, Value::String),
                    );
                    row.set("statusId", Value::string(PENDING));
                    row.create(facade)?;
                }
                Ok(job_id)
            })
        })
    };
    match created {
        Ok(job_id) => {
            crate::audit::record(
                facade,
                &crate::audit::AuditEntry {
                    action: "DbaJob",
                    db: Some(db.db_text()),
                    statement_text: Some(join_texts(statements.iter().map(|s| s.statement_text.as_str()))),
                    is_change: false,
                    detail_json: Some(format!(
                        r#"{{"jobId":"{job_id}","event":"created","kind":"{kind}","statements":{},"creator":"{}"}}"#,
                        statements.len(),
                        json_text(user_id.as_deref().unwrap_or(""))
                    )),
                    ..crate::audit::AuditEntry::default()
                },
            );
            Some(job_id)
        }
        Err(err) => {
            add_error(format!("Could not store the plan as a database job: {err}"));
            None
        }
    }
}

fn join_texts<'a>(texts: impl Iterator<Item = &'a str>) -> String {
    texts.collect::<Vec<_>>().join(";\n")
}

/// `text` as the inside of a JSON string.
fn json_text(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// How one statement ended.
enum Outcome {
    Done(u64),
    Failed(String),
    Cancelled(String),
}

/// `run#DatabaseJob`: run (or run again) the plan job `jobId`. With `wait` false (the default) it is queued on a
/// service worker and the out is `queued`; with `wait` true it runs here and the out is `statusId`, `doneCount`,
/// `failedCount`, `skippedCount`. The statements not Done run in order. Needs `DATABASE_ADMIN`; a run is refused while
/// the job runs, and a job cancelled before it ever started is not run.
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
    if status == RUNNING || lock(live()).contains_key(&job_id) {
        add_error(format!("database job {job_id} is running"));
        return Ok(Out::new());
    }
    if status == CANCELLED && job.get("startTime").is_none_or(Value::is_null) {
        add_error(format!("database job {job_id} was cancelled before it ran"));
        return Ok(Out::new());
    }
    if !flag(parameters, "wait") {
        let mut run = BTreeMap::new();
        run.insert("jobId".to_string(), Value::string(&job_id));
        run.insert("wait".to_string(), Value::Bool(true));
        // this call checked DATABASE_ADMIN; the run on the worker checks it again as the user
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let mut out = Out::new();
        match moqui_core::service::call::async_call(RUN_SERVICE, &run) {
            Ok(()) => {
                out.insert("queued".to_string(), Value::Bool(true));
            }
            Err(err) => add_error(format!("Could not start database job {job_id}: {err}")),
        }
        return Ok(out);
    }
    run_claimed(facade, &job, &job_id, &status)
}

/// Claim the job (in process, then on its row, from the status read) and run it.
fn run_claimed(facade: &EntityFacade, job: &EntityValue, job_id: &str, status: &str) -> Result<Out, LibError> {
    {
        let mut jobs = lock(live());
        if jobs.contains_key(job_id) {
            add_error(format!("database job {job_id} is running"));
            return Ok(Out::new());
        }
        jobs.insert(
            job_id.to_string(),
            Live {
                canceller: None,
                cancelled: false,
            },
        );
    }
    let claimed = transition(
        facade,
        job_id,
        status,
        RUNNING,
        vec![
            ("instanceId", Value::string(instance_id())),
            ("startTime", now()),
            ("endTime", Value::Null),
        ],
    );
    if !matches!(claimed, Ok(1)) {
        lock(live()).remove(job_id);
        match claimed {
            Err(err) => add_error(format!("Could not start database job {job_id}: {err}")),
            // another run or a cancel changed the row since it was read
            Ok(_) => add_error(format!("database job {job_id} changed while starting; read it again")),
        }
        return Ok(Out::new());
    }
    let mut db_parameters = BTreeMap::new();
    db_parameters.insert("db".to_string(), Value::string(field(job, "db")));
    let Some(db) = GroupDb::of_group(&db_parameters, facade) else {
        lock(live()).remove(job_id);
        let _ = transition(facade, job_id, RUNNING, FAILED, vec![("endTime", now())]);
        return Ok(Out::new());
    };
    let runner = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let lock_timeout = cell_number(job.get("lockTimeoutMs")).unwrap_or(DEFAULT_LOCK_TIMEOUT_MS);
    let started = Instant::now();
    let statements = job_statements(facade, job_id).unwrap_or_default();
    let mut finished: HashMap<String, bool> = HashMap::new();
    let (mut done, mut failed, mut skipped, mut cancelled) = (0, 0, 0, false);
    for statement in &statements {
        let seq = field(statement, "statementSeqId");
        let pk = [("jobId", job_id), ("statementSeqId", seq.as_str())];
        if field(statement, "statusId") == DONE {
            finished.insert(seq, true);
            done += 1;
            continue;
        }
        if cancelled || job_cancelled(facade, job_id) {
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
                ("backendStart", Value::Null),
            ],
        );
        let sql = field(statement, "statementText");
        let outcome = run_statement(facade, &db, job_id, &seq, &sql, lock_timeout);
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
    let cancelled =
        cancelled || lock(live()).get(job_id).is_some_and(|live| live.cancelled) || job_cancelled(facade, job_id);
    let status = if cancelled {
        CANCELLED
    } else if failed > 0 || skipped > 0 {
        FAILED
    } else {
        DONE
    };
    // Running, or Cancelled by a cancel while it ran: either becomes the end status
    let ended = transition(facade, job_id, RUNNING, status, vec![("endTime", now())]).unwrap_or(0);
    if ended == 0 {
        write_row(
            facade,
            JOB,
            &[("jobId", job_id)],
            vec![("statusId", Value::string(status)), ("endTime", now())],
        );
    }
    lock(live()).remove(job_id);
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
            detail_json: Some(format!(
                r#"{{"jobId":"{job_id}","event":"ran","statusId":"{status}","statements":{},"done":{done},"failed":{failed},"skipped":{skipped},"runner":"{}"}}"#,
                statements.len(),
                json_text(&runner)
            )),
            ..crate::audit::AuditEntry::default()
        },
    );
    notify(job, job_id, status, done, failed, skipped);
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
        let result =
            db.own_connection(|conn| on_connection(facade, db, conn, job_id, seq, sql, strong.then_some(lock_ms)));
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

/// The statement on `conn`, a connection closed after it: the session's timeouts set (none for the statement, the
/// lock's when `lock_ms`), the backend id and start written, the canceller live from before the drop of an invalid
/// index of the name it makes until the statement ends.
fn on_connection(
    facade: &EntityFacade,
    db: &GroupDb,
    conn: &mut dyn DbConnection,
    job_id: &str,
    seq: &str,
    sql: &str,
    lock_ms: Option<i64>,
) -> Result<u64, LibError> {
    for statement in session_settings(db.server, lock_ms) {
        if let Err(err) = conn.execute(&statement, &[]) {
            tracing::warn!(error = %err, statement, "A database job session setting was refused");
        }
    }
    let backend = conn.backend_id();
    let backend_start = if db.is_postgres() {
        conn.query_once(BACKEND_START, &[])
            .ok()
            .and_then(|rows| rows.into_iter().next())
            .and_then(|row| row.columns.into_iter().next())
            .map(|(_, value)| cell_text(Some(&value)))
    } else {
        None
    };
    write_row(
        facade,
        STATEMENT,
        &[("jobId", job_id), ("statementSeqId", seq)],
        vec![
            ("backendPid", backend.map_or(Value::Null, Value::I64)),
            ("backendStart", backend_start.map_or(Value::Null, Value::String)),
        ],
    );
    {
        let mut jobs = lock(live());
        if let Some(entry) = jobs.get_mut(job_id) {
            if entry.cancelled {
                return Err(LibError::new("the job was cancelled"));
            }
            entry.canceller = Some(conn.canceller());
        }
    }
    let result = (|| {
        if db.is_postgres() {
            if let Some(index) = concurrent_index_name(sql) {
                drop_invalid_index(conn, db, &index)?;
            }
        }
        conn.execute(sql, &[])
    })();
    // disarmed under the lock a cancel holds, before the connection is closed
    if let Some(entry) = lock(live()).get_mut(job_id) {
        entry.canceller = None;
    }
    result
}

const BACKEND_START: &str = "SELECT CAST(backend_start AS TEXT) AS backend_start FROM pg_stat_activity \
     WHERE pid = pg_backend_pid()";

/// The session settings of a job statement. The connection is closed after it, so nothing puts them back.
fn session_settings(server: Server, lock_ms: Option<i64>) -> Vec<String> {
    match server {
        Server::Postgres => {
            let lock = lock_ms.map_or_else(|| "0".to_string(), |ms| format!("{ms}"));
            vec![
                "SET statement_timeout = 0".to_string(),
                format!("SET lock_timeout = {lock}"),
            ]
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
            setup
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

/// `cancel#DatabaseJob`: stop the job `jobId`. A Pending job (a plan not run, or one queued) is marked Cancelled and
/// does not run (`how` `notStarted`). A Running one is marked Cancelled, so the statements not started do not start,
/// and its running statement is cancelled: with the canceller of this instance when the job runs here (`local`),
/// else by its backend pid and backend start on a connection of its own (`backend`). Out: `cancelled` (Boolean), `how`
/// (`local`, `backend`, `notStarted` or `none`). Needs `DATABASE_ADMIN`; writes a `DbaCancel` audit row.
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
    // only from the status read, so a run that claimed the job since is cancelled as running, not as pending
    let marked = matches!(status.as_str(), PENDING | RUNNING)
        && transition(facade, &job_id, &status, CANCELLED, Vec::new()).unwrap_or(0) == 1;
    if !marked {
        out.insert("cancelled".to_string(), Value::Bool(false));
        out.insert("how".to_string(), Value::string("none"));
        return Ok(out);
    }
    let mut how = "notStarted";
    if status == RUNNING {
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
        } else {
            let running = job_statements(facade, &job_id)
                .unwrap_or_default()
                .into_iter()
                .find(|row| field(row, "statusId") == RUNNING);
            let backend = running.as_ref().and_then(|row| cell_number(row.get("backendPid")));
            let started = running
                .as_ref()
                .map(|row| field(row, "backendStart"))
                .unwrap_or_default();
            let mut db_parameters = BTreeMap::new();
            db_parameters.insert("db".to_string(), Value::string(field(&job, "db")));
            if let (Some(pid), Some(db)) = (backend, GroupDb::of_group(&db_parameters, facade)) {
                let statement = running
                    .as_ref()
                    .map(|row| field(row, "statementText"))
                    .unwrap_or_default();
                match cancel_job_backend(&db, pid, &started, &statement) {
                    Ok(true) => how = "backend",
                    Ok(false) => tracing::info!(job = %job_id, pid, "The job's session was no longer running"),
                    Err(err) => add_error(format!("Could not cancel backend {pid}: {err}")),
                }
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

/// Cancel the job statement's session, and only that session: on Postgres the pid with the backend start the runner
/// recorded (a later session may have the pid); on MySQL the connection id whose current statement is the job's.
fn cancel_job_backend(db: &GroupDb, pid: i64, backend_start: &str, statement: &str) -> Result<bool, LibError> {
    if db.is_postgres() {
        if backend_start.is_empty() {
            return Ok(false);
        }
        let rows = db.query(
            &format!(
                "SELECT pg_cancel_backend(pid) AS signalled FROM pg_stat_activity \
                 WHERE pid = {pid} AND CAST(backend_start AS TEXT) = ?"
            ),
            &[Value::string(backend_start)],
        )?;
        return Ok(rows
            .first()
            .and_then(|row| row.get("signalled"))
            .is_some_and(|value| matches!(value, Value::Bool(true))));
    }
    let rows = db.query(
        "SELECT INFO AS query FROM information_schema.PROCESSLIST WHERE ID = ?",
        &[Value::I64(pid)],
    )?;
    let head: String = statement.chars().take(60).collect();
    let running = rows
        .first()
        .is_some_and(|row| cell_text(row.get("query")).starts_with(head.as_str()));
    if !running {
        return Ok(false);
    }
    signal_backend(db, pid, false)
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
        db.own_connection(|conn| conn.execute(&kill, &[])).map(|_| true)
    }
}

/// Whether `pid` is the session of a job statement running now.
pub(crate) fn is_job_backend(facade: &EntityFacade, pid: i64) -> bool {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find(STATEMENT)
        .and_then(|find| {
            find.eq("statusId", Value::string(RUNNING))
                .eq("backendPid", Value::I64(pid))
                .count()
        })
        .is_ok_and(|count| count > 0)
}

/// `get#DatabaseJobStatus`: the job `jobId`, its statements and, for a statement running now, the server's progress
/// and what blocks it. Out: `job` (Map), `statements` (List of Map, in order), `progress` (List of Map: Postgres
/// `pg_stat_progress_create_index`, `_vacuum`, `_analyze`, `_cluster`; MySQL `events_stages_current`; MariaDB
/// `PROCESSLIST` progress), `blocking` (List of Map: the sessions blocking it, from `pg_blocking_pids` with
/// `pg_stat_activity`, MySQL `sys.innodb_lock_waits`, MariaDB `INNODB_LOCK_WAITS`), `reasons` (Map: what could not be
/// read and why).
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
        if let Some(db) = GroupDb::of_group(&db_parameters, facade) {
            for pid in running_pids {
                for sql in progress_queries(db.server, pid) {
                    progress.extend(db.query_or_note(&sql, &[], "progress", &mut reasons));
                }
                blocking.extend(db.query_or_note(&blocking_query(db.server, pid), &[], "blocking", &mut reasons));
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
            // VACUUM FULL and CLUSTER
            format!(
                "SELECT p.pid, lower(p.command) AS command, p.phase, p.heap_blks_total AS blocks_total, \
                 p.heap_blks_scanned AS blocks_done, p.heap_tuples_written AS tuples_done, c.relname AS table_name \
                 FROM pg_stat_progress_cluster p LEFT JOIN pg_class c ON c.oid = p.relid WHERE p.pid = {pid}"
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
fn blocking_query(server: Server, pid: i64) -> String {
    match server {
        Server::Postgres => format!(
            "SELECT {pid} AS blocked_pid, b.pid, b.usename AS user_name, b.application_name, b.state, b.query, \
             b.xact_start, b.query_start FROM pg_stat_activity b WHERE b.pid = ANY (pg_blocking_pids({pid}))"
        ),
        Server::Mysql => format!(
            "SELECT waiting_pid AS blocked_pid, blocking_pid AS pid, blocking_query AS query, \
             wait_age_secs FROM sys.innodb_lock_waits WHERE waiting_pid = {pid}"
        ),
        Server::Mariadb => format!(
            "SELECT r.trx_mysql_thread_id AS blocked_pid, b.trx_mysql_thread_id AS pid, b.trx_query AS query, \
             b.trx_started AS xact_start FROM information_schema.INNODB_LOCK_WAITS w \
             JOIN information_schema.INNODB_TRX r ON r.trx_id = w.requesting_trx_id \
             JOIN information_schema.INNODB_TRX b ON b.trx_id = w.blocking_trx_id \
             WHERE r.trx_mysql_thread_id = {pid}"
        ),
    }
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
    rust_service!(map, run_database_job)?;
    rust_service!(map, cancel_database_job)?;
    rust_service!(map, get_database_job_status)?;
    rust_service!(map, get_database_jobs)?;
    Ok(())
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
