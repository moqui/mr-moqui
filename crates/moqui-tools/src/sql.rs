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

//! SQL runs of the Tools SQL screen (plans/archive/Tools-Database.md §8), on an entity group's database through
//! `moqui_entity::entity::info::run_sql_with`.
//!
//! - **Read only by default:** each statement runs in a read-only transaction of its own, so the database refuses a
//!   write ([`RunRequest::allow_changes`] false).
//! - **Allow changes:** [`classify`] lists the statements that may change data and issues a one-time token, kept in
//!   `moqui.database.SqlRunToken` so any instance can spend it, once ([`run`], [`explain`]). On a production instance
//!   the run also needs the database's name typed. A script runs in one transaction unless
//!   [`RunRequest::each_on_own`]; a statement that cannot run in one ([`no_transaction`]) runs on its own between the
//!   others. A cancel or timeout with statements left rolls the transaction back. Every connection a run with changes
//!   used is closed after it, so no session state (`SET`, `SET ROLE`, `USE`, temporary tables, advisory locks) reaches
//!   later work.
//! - **Transaction control** (`COMMIT`, `ROLLBACK`, `BEGIN`, `SAVEPOINT`, `SET TRANSACTION`, MySQL `LOCK TABLES`, …) is
//!   refused in every mode: a run is its own transaction ([`transaction_control`]).
//! - **Parameters:** a `:name` outside text and comments is bound as a value ([`bind_parameters`]).
//! - **Cancel:** a run is registered by its `runId` while it runs ([`cancel`]); another instance's run is found by its
//!   running audit row and cancelled with `pg_cancel_backend` / `KILL QUERY` once the session is checked to be the one
//!   that row names.
//! - **Audit:** every statement run gets a `DatabaseAudit` row with a server-made id, its `runId` and `statementSeq`,
//!   reads, failures and refused attempts included.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;
use sha2::Digest;

use moqui_core::context::ExecutionContext;
use moqui_core::driver::DbCancel;
use moqui_core::entity::exec::{blank_sql, split_sql_as, SqlSyntax};
use moqui_core::entity::info::{self, SqlResult, SqlRunOptions, SqlStart};
use moqui_core::entity::{Condition, EntityFacade};
use moqui_core::transaction::TransactionAttribute;

use crate::audit::{self, Actor, AuditEntry};
use crate::catalog;

/// The rows a query returns unless the run asks for another limit (§8.1).
pub const DEFAULT_LIMIT: usize = 500;
/// The most rows one run may ask for, and the most it may skip to fetch more.
pub const MAX_LIMIT: usize = 100_000;
/// How long a run may take unless it asks for less; past it the running statement is cancelled.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// The longest timeout a run may ask for; a running audit row older than this is stale.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(3600);
/// How long a confirmation token is good for.
pub const TOKEN_FOR: Duration = Duration::from_secs(600);

const SQL_RUN_TOKEN: &str = "moqui.database.SqlRunToken";

/// Why a request was refused before anything ran: the message the screen shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal(pub String);

impl Refusal {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// One statement of a script, as [`classify`] sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    /// From 1, in the order of the script.
    pub seq: usize,
    pub sql: String,
    /// It may change data: the database does not say it gives rows, or it is not a plain read (an `INSERT …
    /// RETURNING`, a `WITH` that writes).
    pub changes: bool,
    /// It cannot run inside a transaction (`VACUUM`, `CREATE INDEX CONCURRENTLY`, `CREATE DATABASE`): it runs on its
    /// own.
    pub own_transaction: bool,
    /// On MySQL and MariaDB, it commits the transaction it runs in (DDL, transaction control, `LOCK TABLES`): a script
    /// with it is not atomic there.
    pub implicit_commit: bool,
    /// Transaction control, which a run refuses ([`transaction_control`]).
    pub transaction_control: bool,
}

fn syntax_of(facade: &EntityFacade, group: &str) -> SqlSyntax {
    if catalog::group_is_mysql(facade, group) {
        SqlSyntax::Mysql
    } else {
        SqlSyntax::Postgres
    }
}

/// The words of `sql` outside its text, quoted names and comments, in lower case.
fn words(sql: &str, syntax: SqlSyntax) -> Vec<String> {
    blank_sql(sql, syntax)
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// A statement that cannot run inside a transaction, by its first words: Postgres `VACUUM`, `CREATE DATABASE`,
/// `DROP DATABASE`, `CREATE TABLESPACE`, `DROP TABLESPACE`, `ALTER SYSTEM`, and the `CONCURRENTLY` forms of
/// `CREATE INDEX`, `DROP INDEX` and `REINDEX`.
#[must_use]
pub fn no_transaction(sql: &str, syntax: SqlSyntax) -> bool {
    if syntax == SqlSyntax::Mysql {
        return false;
    }
    let words = words(sql, syntax);
    let first = |count: usize| words.iter().take(count).map(String::as_str).collect::<Vec<_>>();
    match first(2).as_slice() {
        ["vacuum", ..] => true,
        ["create" | "drop", "database" | "tablespace"] => true,
        ["alter", "system"] => true,
        ["create" | "drop" | "reindex", ..] => words.iter().take(8).any(|word| word == "concurrently"),
        _ => false,
    }
}

/// Transaction control, by its first words: `COMMIT`, `ROLLBACK`, `BEGIN`, `START TRANSACTION`, `SAVEPOINT`,
/// `RELEASE`, `END`, `ABORT`, `SET TRANSACTION`, `SET SESSION CHARACTERISTICS`, `PREPARE TRANSACTION`, and on MySQL and
/// MariaDB `LOCK TABLES`, `UNLOCK TABLES`, `SET autocommit` and `XA`. A run refuses it in every mode: a read-only run is
/// a read-only transaction of its own and a run with changes one transaction (or one per statement), so a statement
/// that ends or opens one would commit part of the run or leave a transaction open on a pooled connection.
#[must_use]
pub fn transaction_control(sql: &str, syntax: SqlSyntax) -> bool {
    let words = words(sql, syntax);
    let first = |count: usize| words.iter().take(count).map(String::as_str).collect::<Vec<_>>();
    match first(3).as_slice() {
        ["commit" | "rollback" | "begin" | "savepoint" | "release" | "end" | "abort", ..] => true,
        ["start", "transaction", ..] => true,
        ["set", "transaction", ..] | ["set", "session", "characteristics"] => true,
        ["prepare", "transaction", ..] => true,
        ["set", rest @ ..] if syntax == SqlSyntax::Mysql => rest
            .iter()
            .take(2)
            .any(|word| *word == "autocommit" || *word == "transaction"),
        ["lock" | "unlock", ..] | ["xa", ..] if syntax == SqlSyntax::Mysql => true,
        _ => false,
    }
}

/// MySQL and MariaDB statements that commit the transaction they run in: DDL, transaction control and `LOCK TABLES`.
fn implicit_commit(sql: &str, syntax: SqlSyntax) -> bool {
    syntax == SqlSyntax::Mysql
        && (transaction_control(sql, syntax)
            || words(sql, syntax).first().is_some_and(|word| {
                matches!(
                    word.as_str(),
                    "create" | "alter" | "drop" | "rename" | "truncate" | "grant" | "revoke" | "lock" | "unlock"
                )
            }))
}

/// A statement that only reads, by its words: a `SELECT`, `VALUES`, `TABLE`, `SHOW`, `DESCRIBE` or plain `EXPLAIN`,
/// or a `WITH` whose words have no `INSERT`, `DELETE`, `MERGE` or `UPDATE` other than a `FOR UPDATE` lock. A
/// `SELECT … INTO` table is a write.
fn plain_read(sql: &str, syntax: SqlSyntax) -> bool {
    let words = words(sql, syntax);
    let Some(first) = words.first() else {
        return false;
    };
    let writes = |words: &[String]| {
        words.iter().enumerate().any(|(index, word)| match word.as_str() {
            "insert" | "delete" | "merge" => true,
            "update" => !(index > 0 && (words[index - 1] == "for" || words[index - 1] == "key")),
            _ => false,
        })
    };
    match first.as_str() {
        "select" => !words.iter().any(|word| word == "into") || syntax == SqlSyntax::Mysql && !writes(&words),
        "values" | "table" | "show" | "describe" | "desc" => true,
        "explain" => !words.iter().take(4).any(|word| word == "analyze" || word == "analyse"),
        "with" => !writes(&words),
        _ => false,
    }
}

/// The statements of `sql_text` (split as the group's database writes SQL), each classified. A statement the database
/// cannot describe is taken by its first word.
///
/// # Errors
///
/// Returns an error for an unknown group.
pub fn statements(facade: &EntityFacade, group: &str, sql_text: &str) -> Result<Vec<Statement>, Error> {
    catalog::group_tx(facade, group)?;
    let syntax = syntax_of(facade, group);
    Ok(split_sql_as(sql_text, syntax)
        .into_iter()
        .enumerate()
        .map(|(index, sql)| {
            let control = transaction_control(sql, syntax);
            // `:name` parameters are placeholders to the database; their values do not change what it is. Transaction
            // control is not described: it is refused anyway.
            let described = (!control)
                .then(|| bind_parameters(sql, syntax, &BTreeMap::new(), true).ok())
                .flatten()
                .and_then(|(bound, _)| {
                    catalog::group_tx(facade, group)
                        .and_then(|tx| info::sql_gives_rows_on(&tx, &bound))
                        .ok()
                });
            let gives_rows = described.unwrap_or_else(|| info::is_query(sql));
            Statement {
                seq: index + 1,
                sql: sql.to_string(),
                changes: control || !(gives_rows && plain_read(sql, syntax)),
                own_transaction: no_transaction(sql, syntax),
                implicit_commit: implicit_commit(sql, syntax),
                transaction_control: control,
            }
        })
        .collect())
}

fn refuse_transaction_control(statements: &[Statement]) -> Result<(), Refusal> {
    match statements.iter().find(|statement| statement.transaction_control) {
        Some(statement) => Err(Refusal::new(format!(
            "Statement {} is transaction control, which is not run here: each run is a transaction of its own \
             (or one per statement)",
            statement.seq
        ))),
        None => Ok(()),
    }
}

/// The byte ranges of the `:name` parameters of one statement, outside its text, quoted names and comments; each
/// range covers the colon and the name. A `::type` cast, MySQL's `:=`, `a.b:c` and anything inside square brackets
/// (a Postgres slice `a[1:n]`, `a[:n]`, `a[1 : n]`) are not parameters.
fn parameter_spans(sql: &str, syntax: SqlSyntax) -> Vec<(usize, usize)> {
    let blank = blank_sql(sql, syntax);
    let code = blank.as_bytes();
    let is_ident = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut spans = Vec::new();
    let mut brackets = 0_usize;
    let mut at = 0;
    while at < code.len() {
        match code[at] {
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            _ => {}
        }
        let previous = at.checked_sub(1).map(|index| code[index]);
        let next = code.get(at + 1).copied();
        let starts = brackets == 0
            && code[at] == b':'
            && next.is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && !previous.is_some_and(|byte| is_ident(byte) || matches!(byte, b':' | b']' | b')' | b'.'));
        if !starts {
            at += 1;
            continue;
        }
        let mut end = at + 1;
        while end < code.len() && is_ident(code[end]) {
            end += 1;
        }
        spans.push((at, end));
        at = end;
    }
    spans
}

/// `sql` (one statement) with each `:name` parameter made a `?` placeholder, and the values in the order of the
/// placeholders. With `lenient`, a name with no value binds null (to describe a statement); otherwise it is refused.
/// A statement with parameters may not also have a `?` of its own, which the driver would take for a placeholder.
///
/// # Errors
///
/// Returns the refusal: a parameter with no value, or a `?` beside `:name` parameters.
pub fn bind_parameters(
    sql: &str,
    syntax: SqlSyntax,
    params: &BTreeMap<String, Value>,
    lenient: bool,
) -> Result<(String, Vec<Value>), Refusal> {
    let spans = parameter_spans(sql, syntax);
    if spans.is_empty() {
        return Ok((sql.to_string(), Vec::new()));
    }
    if blank_sql(sql, syntax).contains('?') {
        return Err(Refusal::new(
            "A statement with :name parameters cannot also use ? (the driver takes it for a placeholder)",
        ));
    }
    let mut out = String::with_capacity(sql.len());
    let mut values = Vec::with_capacity(spans.len());
    let mut copied = 0;
    for (start, end) in spans {
        let name = &sql[start + 1..end];
        let value = match params.get(name) {
            Some(value) => value.clone(),
            None if lenient => Value::Null,
            None => return Err(Refusal::new(format!("The parameter :{name} has no value"))),
        };
        out.push_str(&sql[copied..start]);
        out.push('?');
        values.push(value);
        copied = end;
    }
    out.push_str(&sql[copied..]);
    Ok((out, values))
}

/// The `:name` parameters of `sql_text`, in the order they first appear: what the screen asks values for.
#[must_use]
pub fn parameter_names(sql_text: &str, syntax: SqlSyntax) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for statement in split_sql_as(sql_text, syntax) {
        for (start, end) in parameter_spans(statement, syntax) {
            let name = &statement[start + 1..end];
            if !names.iter().any(|known| known == name) {
                names.push(name.to_string());
            }
        }
    }
    names
}

// ---- the confirmation token, in moqui.database.SqlRunToken ----

fn sha256_hex(text: &str) -> String {
    let digest = sha2::Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn text_hash(text: &str) -> String {
    sha256_hex(text.trim())
}

/// Issue a token for `user_id` to run `sql_text` on `db` with changes, good for [`TOKEN_FOR`]. Its SHA-256 is kept.
fn issue_token(facade: &EntityFacade, user_id: Option<&str>, db: &str, sql_text: &str) -> Result<String, Error> {
    let token = moqui_core::entity::uuid_v4()?.replace('-', "");
    let expire = jiff::Timestamp::now()
        .checked_add(jiff::SignedDuration::from_secs(TOKEN_FOR.as_secs() as i64))
        .map_err(|err| Error::new(err.to_string()))?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(SQL_RUN_TOKEN)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let mut row = facade.make_value(SQL_RUN_TOKEN)?;
        row.set("tokenHash", Value::String(sha256_hex(&token)));
        row.set("userId", user_id.map_or(Value::Null, Value::string));
        row.set("db", Value::string(db));
        row.set("textHash", Value::String(text_hash(sql_text)));
        row.set("expireTime", Value::Timestamp(expire));
        row.create(facade)
    })?;
    Ok(token)
}

/// Spend `token`: true when it was issued to this user for this database and text and has not expired. The spend is a
/// delete that must remove exactly the token's row, so only one run on any instance gets it; a wrong text or user
/// spends it too, so it is not left for a second try.
fn take_token(facade: &EntityFacade, token: &str, user_id: Option<&str>, db: &str, sql_text: &str) -> bool {
    if token.trim().is_empty() {
        return false;
    }
    let hash = sha256_hex(token.trim());
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let spent = facade.transactions_for(SQL_RUN_TOKEN).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            let row = facade
                .find(SQL_RUN_TOKEN)?
                .eq("tokenHash", Value::string(&hash))
                .one()?;
            let deleted = facade
                .find(SQL_RUN_TOKEN)?
                .eq("tokenHash", Value::string(&hash))
                .delete_all()?;
            Ok(row.filter(|_| deleted == 1))
        })
    });
    let row = match spent {
        Ok(Some(row)) => row,
        Ok(None) => return false,
        Err(err) => {
            tracing::warn!(error = %err, "Could not spend a SQL run token");
            return false;
        }
    };
    let field = |name: &str| row.get(name).map(to_plain_string);
    let fresh = matches!(row.get("expireTime"), Some(Value::Timestamp(expire)) if *expire > jiff::Timestamp::now());
    fresh
        && field("userId").filter(|user| !user.is_empty()).as_deref() == user_id
        && field("db").as_deref() == Some(db)
        && field("textHash") == Some(text_hash(sql_text))
}

/// Delete the expired tokens; the count deleted.
///
/// # Errors
///
/// When the rows cannot be deleted.
pub fn purge_tokens(facade: &EntityFacade) -> Result<i64, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find(SQL_RUN_TOKEN)?
        .condition(Condition::less("expireTime", Value::Timestamp(jiff::Timestamp::now())))
        .delete_all()
}

/// What [`classify`] answers: the statements, which of them may change data, and the token a run with changes needs.
#[derive(Clone, Debug)]
pub struct ClassifyOutcome {
    pub statements: Vec<Statement>,
    pub change_count: usize,
    pub token: String,
    /// On a production instance the run needs this typed as `confirmDatabaseName`.
    pub confirm_database_name: Option<String>,
    /// A statement commits the transaction it runs in (MySQL and MariaDB DDL), so one transaction does not make the
    /// script atomic.
    pub implicit_commit: bool,
    pub parameter_names: Vec<String>,
}

/// The database's name, as the connection sees it.
fn database_name(facade: &EntityFacade, group: &str) -> Option<String> {
    let sql = if catalog::group_is_mysql(facade, group) {
        catalog::mysql::CURRENT_DATABASE
    } else {
        catalog::postgres::CURRENT_DATABASE
    };
    match catalog::query(facade, group, sql) {
        Ok(rows) => rows
            .first()
            .and_then(|row| row.get("database_name"))
            .map(to_plain_string),
        Err(err) => {
            tracing::warn!(error = %err, "Could not read the database name of group {group}");
            None
        }
    }
}

/// Classify `sql_text` for a run with changes allowed, and issue its token for `user_id`.
///
/// # Errors
///
/// Returns the refusal: an unknown group, an empty script, or transaction control in it.
pub fn classify(
    facade: &EntityFacade,
    group: &str,
    sql_text: &str,
    user_id: Option<&str>,
    production: bool,
) -> Result<ClassifyOutcome, Refusal> {
    let statements = statements(facade, group, sql_text).map_err(|err| Refusal::new(err.to_string()))?;
    if statements.is_empty() {
        return Err(Refusal::new("There is no statement to run"));
    }
    refuse_transaction_control(&statements)?;
    let db = catalog::db_text(group);
    let token = issue_token(facade, user_id, &db, sql_text).map_err(|err| {
        tracing::warn!(error = %err, "Could not issue a SQL run token");
        Refusal::new("The confirmation could not be issued (the server log has the detail)")
    })?;
    let confirm_database_name = production.then(|| database_name(facade, group).unwrap_or_else(|| group.to_string()));
    Ok(ClassifyOutcome {
        change_count: statements.iter().filter(|statement| statement.changes).count(),
        implicit_commit: statements.iter().any(|statement| statement.implicit_commit),
        parameter_names: parameter_names(sql_text, syntax_of(facade, group)),
        statements,
        token,
        confirm_database_name,
    })
}

// ---- the cancel registry ----

/// A run while it runs: what [`cancel`] needs.
struct RunState {
    user_id: Option<String>,
    group: String,
    canceller: Mutex<Option<Box<dyn DbCancel>>>,
    backend_id: Mutex<Option<i64>>,
    cancelled: AtomicBool,
    timed_out: AtomicBool,
}

impl RunState {
    fn cancel(&self) -> Result<bool, Error> {
        self.cancelled.store(true, Ordering::Relaxed);
        match lock(&self.canceller).as_ref() {
            Some(canceller) => canceller.cancel().map(|()| true),
            None => Ok(false),
        }
    }

    fn stopped(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

fn runs() -> &'static Mutex<HashMap<String, Arc<RunState>>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Arc<RunState>>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A run's place in the registry: unregisters the run when it ends, and stops its timer.
struct Registered {
    run_id: String,
    state: Arc<RunState>,
    stop_timer: Option<mpsc::Sender<()>>,
}

impl Drop for Registered {
    fn drop(&mut self) {
        lock(runs()).remove(&self.run_id);
        if let Some(stop) = self.stop_timer.take() {
            let _ = stop.send(());
        }
    }
}

/// A run id the client names, so it can cancel the run before the run answers: letters, digits and `-`, 8 to 36 of
/// them (a UUID).
fn valid_run_id(run_id: &str) -> bool {
    (8..=36).contains(&run_id.len()) && run_id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

fn stale_before() -> Option<jiff::Timestamp> {
    jiff::Timestamp::now()
        .checked_sub(jiff::SignedDuration::from_secs(MAX_TIMEOUT.as_secs() as i64))
        .ok()
}

/// The running audit rows of `run_id` (no duration yet, started within [`MAX_TIMEOUT`]), on any instance.
fn running_rows(facade: &EntityFacade, run_id: &str) -> Result<Vec<moqui_core::entity::EntityValue>, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut find = facade
        .find(audit::DATABASE_AUDIT)?
        .eq("runId", Value::string(run_id))
        .condition(Condition::IsNull("durationMs".into()));
    if let Some(cutoff) = stale_before() {
        find = find.condition(Condition::greater("startTime", Value::Timestamp(cutoff)));
    }
    Ok(find.list()?.iter().cloned().collect())
}

/// Register the run `run_id` (a new id when empty) for `actor`, with a timer that cancels it after `timeout`. Refused
/// when the id is in use here, or by a running statement on another instance.
fn start_run(
    facade: &EntityFacade,
    run_id: &str,
    actor: &Actor,
    group: &str,
    timeout: Duration,
) -> Result<Registered, Refusal> {
    let run_id = if run_id.is_empty() {
        moqui_core::entity::uuid_v4()
            .map(|id| id.replace('-', ""))
            .map_err(|err| Refusal::new(err.to_string()))?
    } else if valid_run_id(run_id) {
        run_id.to_string()
    } else {
        return Err(Refusal::new("runId is 8 to 36 letters, digits or -"));
    };
    if running_rows(facade, &run_id).is_ok_and(|rows| !rows.is_empty()) {
        return Err(Refusal::new("A run of that runId is running"));
    }
    let state = Arc::new(RunState {
        user_id: actor.user_id.clone(),
        group: group.to_string(),
        canceller: Mutex::new(None),
        backend_id: Mutex::new(None),
        cancelled: AtomicBool::new(false),
        timed_out: AtomicBool::new(false),
    });
    {
        let mut map = lock(runs());
        if map.contains_key(&run_id) {
            return Err(Refusal::new("A run of that runId is running"));
        }
        map.insert(run_id.clone(), Arc::clone(&state));
    }
    let (stop, stopped) = mpsc::channel::<()>();
    {
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            if stopped.recv_timeout(timeout) == Err(mpsc::RecvTimeoutError::Timeout) {
                state.timed_out.store(true, Ordering::Relaxed);
                if let Err(err) = state.cancel() {
                    tracing::warn!(error = %err, "Could not cancel a SQL run past its timeout");
                }
            }
        });
    }
    Ok(Registered {
        run_id,
        state,
        stop_timer: Some(stop),
    })
}

/// What [`cancel`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    /// The statement of a run on this instance was cancelled.
    Cancelled,
    /// A run on another instance: its session was sent `pg_cancel_backend` / `KILL QUERY`.
    CancelledRemote { instance_id: String, backend_id: i64 },
    /// No run of that id is running.
    NotRunning,
}

/// The identity of the session `backend_id` of `group` now: Postgres its `backend_start`, MySQL and MariaDB the
/// server's start (connection ids start again only when the server does). Compared with what a running audit row
/// recorded, so a cancel never reaches a later session that reused the id.
fn backend_identity(facade: &EntityFacade, group: &str, backend_id: i64) -> Option<String> {
    let (sql, column) = if catalog::group_is_mysql(facade, group) {
        (catalog::mysql::SERVER_START.to_string(), "server_start")
    } else {
        (
            format!("{}{backend_id}", catalog::postgres::BACKEND_START),
            "backend_start",
        )
    };
    match catalog::query(facade, group, &sql) {
        Ok(rows) => rows.first().and_then(|row| row.get(column)).map(to_plain_string),
        Err(err) => {
            tracing::warn!(error = %err, "Could not read the identity of session {backend_id}");
            None
        }
    }
}

fn same_identity(recorded: &str, now: &str, mysql: bool) -> bool {
    if mysql {
        // the server start in epoch seconds, read twice a little apart
        match (recorded.trim().parse::<i64>(), now.trim().parse::<i64>()) {
            (Ok(recorded), Ok(now)) => (recorded - now).abs() <= 5,
            _ => false,
        }
    } else {
        recorded == now
    }
}

/// Cancel the run `run_id` for `user_id`, who must have started it unless `admin` (`DATABASE_ADMIN`). On this
/// instance through its connection's canceller. Otherwise its running audit row (on another instance, or on any when
/// `this_instance` is not known) names the session, which is cancelled when it is still the one the row recorded.
/// Refused under the read fence: a cancel acts on the database.
///
/// # Errors
///
/// Returns the refusal: another user's run, the read fence, or the cancel failed.
pub fn cancel(
    facade: &EntityFacade,
    run_id: &str,
    user_id: Option<&str>,
    admin: bool,
    this_instance: Option<&str>,
) -> Result<CancelOutcome, Refusal> {
    let local = lock(runs()).get(run_id).cloned();
    if let Some(state) = local {
        if !admin && state.user_id.as_deref() != user_id {
            return Err(Refusal::new("That run is another user's"));
        }
        return match state.cancel() {
            Ok(_) => Ok(CancelOutcome::Cancelled),
            Err(err) => {
                tracing::warn!(error = %err, "Could not cancel SQL run {run_id}");
                Err(Refusal::new("The cancel did not reach the database"))
            }
        };
    }
    if !valid_run_id(run_id) {
        return Ok(CancelOutcome::NotRunning);
    }
    if moqui_core::entity::fence::active() {
        return Err(Refusal::new(
            "A cancel of another instance's run is not made under the read fence",
        ));
    }
    let running = running_rows(facade, run_id).map_err(|err| {
        tracing::warn!(error = %err, "Could not look up SQL run {run_id}");
        Refusal::new("The run could not be looked up")
    })?;
    for row in &running {
        let owner = row.get("userId").map(to_plain_string).filter(|user| !user.is_empty());
        if !admin && owner.as_deref() != user_id {
            return Err(Refusal::new("That run is another user's"));
        }
        let instance_id = row.get("instanceId").map(to_plain_string).unwrap_or_default();
        if this_instance.is_some_and(|this| this == instance_id) {
            // this instance's run, ended since its row was written (the registry would have it otherwise)
            continue;
        }
        let detail = row.get("detailJson").map(to_plain_string).unwrap_or_default();
        let parsed: serde_json::Value = serde_json::from_str(&detail).unwrap_or_default();
        let backend_id = parsed.get("backendId").and_then(serde_json::Value::as_i64);
        let recorded = parsed.get("backendStart").map(|value| match value {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        });
        // an external connection's run is cancelled only through its gate
        let group = row.get("db").map(to_plain_string).and_then(|db| {
            if let Some(group) = db.strip_prefix("group:") {
                Some(group.to_string())
            } else {
                db.starts_with("ext:").then_some(db).filter(|_| crate::ext::gate())
            }
        });
        let (Some(backend_id), Some(recorded), Some(group)) = (backend_id, recorded, group) else {
            continue;
        };
        if !catalog::is_external(&group) && !facade.group_names().contains(&group) {
            continue;
        }
        let mysql = catalog::group_is_mysql(facade, &group);
        let Some(now) = backend_identity(facade, &group, backend_id) else {
            continue;
        };
        if !same_identity(&recorded, &now, mysql) {
            tracing::info!(
                "SQL run {run_id}: session {backend_id} is not the one its audit row recorded; not cancelled"
            );
            continue;
        }
        let sent = if mysql {
            catalog::execute(facade, &group, &format!("{}{backend_id}", catalog::mysql::KILL_QUERY)).map(|_| ())
        } else {
            catalog::query(
                facade,
                &group,
                &format!("{}{backend_id})", catalog::postgres::CANCEL_BACKEND),
            )
            .map(|_| ())
        };
        return match sent {
            Ok(()) => Ok(CancelOutcome::CancelledRemote {
                instance_id,
                backend_id,
            }),
            Err(err) => {
                tracing::warn!(error = %err, "Could not cancel SQL run {run_id} on instance {instance_id}");
                Err(Refusal::new("The cancel did not reach the database"))
            }
        };
    }
    Ok(CancelOutcome::NotRunning)
}

// ---- running ----

/// One run of [`run`].
#[derive(Clone, Debug, Default)]
pub struct RunRequest {
    /// Names the run for [`cancel`] and its audit rows; a new one when empty.
    pub run_id: String,
    pub group: String,
    pub sql_text: String,
    /// The values of the `:name` parameters.
    pub params: BTreeMap<String, Value>,
    /// Run with changes allowed: needs `token` from [`classify`].
    pub allow_changes: bool,
    pub token: String,
    /// On a production instance, with changes allowed: the database name, typed.
    pub confirm_database_name: String,
    /// Each statement in a transaction of its own, the run going on past a failed one; otherwise a script with
    /// changes is one transaction and stops at the first failure.
    pub each_on_own: bool,
    /// Rows per query; 0 is [`DEFAULT_LIMIT`].
    pub limit: usize,
    /// "Fetch more": skip this many rows (at most [`MAX_LIMIT`]) of a run of one plain query, read only.
    pub offset: usize,
    /// 0 is [`DEFAULT_TIMEOUT`].
    pub timeout: Duration,
}

/// One statement's result.
#[derive(Clone, Debug, Default)]
pub struct StatementResult {
    pub seq: usize,
    pub sql: String,
    pub gives_rows: bool,
    pub columns: Vec<String>,
    pub records: Vec<Vec<Value>>,
    pub limit_reached: bool,
    pub rows_affected: Option<u64>,
    pub duration_ms: i64,
    pub error: Option<String>,
    /// A plain query run read only: the screen may run it again to sort, filter or fetch more.
    pub rerunnable: bool,
    /// Not run: an earlier statement of the transaction failed, or the run was cancelled.
    pub skipped: bool,
    /// Ran, and its changes were rolled back with the transaction.
    pub rolled_back: bool,
    /// Its `DatabaseAudit` row.
    pub audit_id: Option<String>,
}

/// What [`run`] answers.
#[derive(Clone, Debug, Default)]
pub struct RunOutcome {
    pub run_id: String,
    pub results: Vec<StatementResult>,
    /// The script's transaction was rolled back (a statement in it failed, or the run was cancelled).
    pub rolled_back: bool,
    pub cancelled: bool,
    pub timed_out: bool,
}

/// Write the audit row of an attempt that was refused before anything ran.
fn audit_refusal(facade: &EntityFacade, actor: &Actor, db: &str, run_id: &str, sql_text: &str, refusal: &Refusal) {
    audit::record_as(
        facade,
        &AuditEntry {
            action: "DbaSql",
            db: Some(db.to_string()),
            run_id: valid_run_id(run_id).then(|| run_id.to_string()),
            statement_text: Some(sql_text.to_string()),
            is_change: true,
            duration_ms: Some(0),
            error_text: Some(refusal.0.clone()),
            detail_json: Some(serde_json::json!({"refused": true, "ran": false}).to_string()),
            ..AuditEntry::default()
        },
        actor,
    );
}

/// The typed database name a change on production needs.
fn check_database_name(facade: &EntityFacade, group: &str, typed: &str) -> Result<(), Refusal> {
    let name = database_name(facade, group).unwrap_or_else(|| group.to_string());
    if typed.trim() == name {
        Ok(())
    } else {
        Err(Refusal::new(format!(
            "Type the database name ({name}) to run changes on this production instance"
        )))
    }
}

/// Run `request` as `actor`. Each statement run gets an audit row, and so does a refused attempt with changes.
///
/// # Errors
///
/// Returns the refusal of a run that did not start: an unknown group, an empty script, transaction control, a missing
/// or spent token, the database name not typed on production, a parameter with no value, a run id in use.
pub fn run(
    facade: &EntityFacade,
    request: &RunRequest,
    actor: &Actor,
    production: bool,
) -> Result<RunOutcome, Refusal> {
    let group = request.group.as_str();
    let db = catalog::db_text(group);
    let syntax = syntax_of(facade, group);
    let statements = statements(facade, group, &request.sql_text).map_err(|err| Refusal::new(err.to_string()))?;
    if statements.is_empty() {
        return Err(Refusal::new("There is no statement to run"));
    }
    refuse_transaction_control(&statements)?;
    // the database once for the request; a read-only connection refuses changes before the token is spent, and a
    // database that cannot be resolved refuses, never taken as one that allows changes
    let target = catalog::target(facade, group).map_err(|err| {
        tracing::info!(error = %err, "The database {group} of a SQL run is not available");
        Refusal::new(crate::NOT_AVAILABLE)
    })?;
    if request.allow_changes && target.read_only {
        return Err(Refusal::new("This connection is read only"));
    }
    if request.allow_changes {
        let confirmed = if take_token(facade, &request.token, actor.user_id.as_deref(), &db, &request.sql_text) {
            if production {
                check_database_name(facade, group, &request.confirm_database_name)
            } else {
                Ok(())
            }
        } else {
            Err(Refusal::new(
                "Running with changes allowed needs the confirmation of these statements; confirm them again",
            ))
        };
        if let Err(refusal) = confirmed {
            audit_refusal(facade, actor, &db, &request.run_id, &request.sql_text, &refusal);
            return Err(refusal);
        }
    }
    let mut bound = Vec::with_capacity(statements.len());
    for statement in &statements {
        bound.push(bind_parameters(&statement.sql, syntax, &request.params, false)?);
    }
    let read_only = !request.allow_changes;
    let limit = match request.limit {
        0 => DEFAULT_LIMIT,
        limit => limit.min(MAX_LIMIT),
    };
    let offset = request.offset;
    if offset > MAX_LIMIT {
        return Err(Refusal::new(format!("Fetching more skips at most {MAX_LIMIT} rows")));
    }
    if offset > 0 && !(read_only && statements.len() == 1 && !statements[0].changes) {
        return Err(Refusal::new(
            "Fetching more runs a single plain query again, read only; page over the rows already fetched instead",
        ));
    }
    let timeout = match request.timeout {
        Duration::ZERO => DEFAULT_TIMEOUT,
        timeout => timeout.min(MAX_TIMEOUT),
    };
    let registered = start_run(facade, &request.run_id, actor, group, timeout)?;
    let runner = Runner {
        facade,
        db: &db,
        run_id: &registered.run_id,
        state: &registered.state,
        actor,
        read_only,
        limit: limit.saturating_add(offset),
        offset,
        syntax,
    };
    let state = &registered.state;
    let mut outcome = RunOutcome {
        run_id: registered.run_id.clone(),
        ..RunOutcome::default()
    };
    if read_only || request.each_on_own {
        for (index, (statement, (sql, params))) in statements.iter().zip(&bound).enumerate() {
            if state.stopped() {
                outcome.results.push(runner.skipped(statement));
                continue;
            }
            let result = if read_only || statement.own_transaction {
                runner.run_one(statement, sql, params)
            } else {
                let mut part = runner.in_transaction(&[(statement, sql, params)]);
                outcome.rolled_back |= part.rolled_back;
                part.results.remove(0)
            };
            let failed = result.error.is_some();
            outcome.results.push(result);
            if failed && !request.each_on_own {
                // a read-only script stops at its first failure, as one with changes does
                for rest in &statements[index + 1..] {
                    outcome.results.push(runner.skipped(rest));
                }
                break;
            }
        }
    } else {
        // one transaction for the script, a statement that cannot run in one on its own between its parts
        let mut index = 0;
        while index < statements.len() {
            if outcome.results.iter().any(|result| result.error.is_some()) || state.stopped() {
                outcome.results.push(runner.skipped(&statements[index]));
                index += 1;
                continue;
            }
            if statements[index].own_transaction {
                let (sql, params) = &bound[index];
                outcome.results.push(runner.run_one(&statements[index], sql, params));
                index += 1;
                continue;
            }
            let mut part = Vec::new();
            while index < statements.len() && !statements[index].own_transaction {
                let (sql, params) = &bound[index];
                part.push((&statements[index], sql, params));
                index += 1;
            }
            let ran = runner.in_transaction(&part);
            outcome.rolled_back |= ran.rolled_back;
            outcome.results.extend(ran.results);
        }
    }
    outcome.cancelled = state.stopped();
    outcome.timed_out = state.timed_out.load(Ordering::Relaxed);
    drop(registered);
    Ok(outcome)
}

struct Runner<'a> {
    facade: &'a EntityFacade,
    db: &'a str,
    run_id: &'a str,
    state: &'a Arc<RunState>,
    actor: &'a Actor,
    read_only: bool,
    limit: usize,
    offset: usize,
    syntax: SqlSyntax,
}

/// What [`Runner::in_transaction`] answers.
struct Part {
    results: Vec<StatementResult>,
    rolled_back: bool,
}

impl Runner<'_> {
    fn skipped(&self, statement: &Statement) -> StatementResult {
        StatementResult {
            seq: statement.seq,
            sql: statement.sql.clone(),
            skipped: true,
            ..StatementResult::default()
        }
    }

    /// Statements in one transaction of their own, stopping at the first failure or a cancel, either of which rolls
    /// the whole part back: a statement that ran before is reported with `rolled_back`, and its audit row marked.
    fn in_transaction(&self, part: &[(&Statement, &String, &Vec<Value>)]) -> Part {
        let mut results = Vec::new();
        let tx = match catalog::group_tx(self.facade, &self.state.group) {
            Ok(tx) => tx,
            Err(err) => {
                return Part {
                    results: part
                        .iter()
                        .map(|(statement, _, _)| StatementResult {
                            seq: statement.seq,
                            sql: statement.sql.clone(),
                            error: Some(err.to_string()),
                            ..StatementResult::default()
                        })
                        .collect(),
                    rolled_back: false,
                }
            }
        };
        let ran = tx.run(TransactionAttribute::ForceNew, Some(MAX_TIMEOUT), || {
            for (statement, sql, params) in part {
                if self.state.stopped() {
                    // cancelled or timed out between statements: none of the part commits
                    return Err(Error::new("the run was cancelled"));
                }
                let result = self.run_one(statement, sql, params);
                let failed = result.error.clone();
                results.push(result);
                if let Some(error) = failed {
                    return Err(Error::new(error));
                }
            }
            if self.state.stopped() {
                return Err(Error::new("the run was cancelled"));
            }
            Ok(())
        });
        let rolled_back = ran.is_err();
        if let Err(err) = ran {
            if results.iter().all(|result| result.error.is_none()) && !self.state.stopped() {
                // the commit failed
                if let Some(last) = results.last_mut() {
                    last.error = Some(format!("the transaction did not commit: {err}"));
                }
            }
            let mut ids = Vec::new();
            for result in &mut results {
                if result.error.is_none() {
                    result.rolled_back = true;
                }
                ids.extend(result.audit_id.clone());
            }
            audit::mark_rolled_back(self.facade, &ids, self.actor);
            for (statement, _, _) in &part[results.len()..] {
                results.push(self.skipped(statement));
            }
        }
        Part { results, rolled_back }
    }

    /// Run one statement (in the transaction in place, if any), with its audit row. With changes allowed its
    /// connection is closed after it (at the end of the transaction in place), so no session state it left behind
    /// reaches later work.
    fn run_one(&self, statement: &Statement, sql: &str, params: &[Value]) -> StatementResult {
        let started = Instant::now();
        let start_time = jiff::Timestamp::now();
        let audit_id = audit::new_id(self.facade)
            .map_err(|err| tracing::error!(error = %err, "Could not make a DatabaseAudit id"))
            .ok();
        let marker: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
        let mut on_start = |start: SqlStart| {
            *lock(&self.state.backend_id) = start.backend_id;
            *lock(&self.state.canceller) = Some(start.canceller);
            if self.state.stopped() {
                // cancelled while the statement was being set up: it stops as it starts
                let _ = self.state.cancel();
            }
            // the running mark, on its own thread and connection, for a cancel from another instance: with the
            // session's identity, so a later session with the same id is never cancelled
            let (Some(audit_id), Some(backend_id)) = (audit_id.clone(), start.backend_id) else {
                return;
            };
            let facade = self.facade.clone();
            let actor = self.actor.clone();
            let group = self.state.group.clone();
            let entry = AuditEntry {
                audit_id: Some(audit_id),
                action: "DbaSql",
                db: Some(self.db.to_string()),
                run_id: Some(self.run_id.to_string()),
                statement_seq: Some(i64::try_from(statement.seq).unwrap_or(i64::MAX)),
                statement_text: Some(statement.sql.clone()),
                is_change: !self.read_only && statement.changes,
                start_time: Some(start_time),
                ..AuditEntry::default()
            };
            *lock(&marker) = Some(std::thread::spawn(move || {
                let identity = backend_identity(&facade, &group, backend_id);
                let entry = AuditEntry {
                    detail_json: Some(
                        serde_json::json!({"backendId": backend_id, "backendStart": identity, "running": true})
                            .to_string(),
                    ),
                    ..entry
                };
                audit::mark_running(&facade, &entry, &actor);
            }));
        };
        let result = run_on(
            self.facade,
            &self.state.group,
            sql,
            SqlRunOptions {
                limit: self.limit,
                read_only: self.read_only,
                on_start: Some(&mut on_start),
                params,
                discard_connection: !self.read_only,
            },
        );
        *lock(&self.state.canceller) = None;
        let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
        if let Some(handle) = lock(&marker).take() {
            let _ = handle.join();
        }
        let mut out = StatementResult {
            seq: statement.seq,
            sql: statement.sql.clone(),
            duration_ms,
            audit_id: audit_id.clone(),
            ..StatementResult::default()
        };
        match result {
            Ok(SqlResult {
                columns,
                records,
                limit_reached,
                rows_affected,
                ..
            }) => {
                out.gives_rows = rows_affected.is_none();
                out.columns = columns;
                out.records = records.into_iter().skip(self.offset).collect();
                out.limit_reached = limit_reached;
                out.rows_affected = rows_affected;
                out.rerunnable = self.read_only && out.gives_rows && !statement.changes;
            }
            Err(err) => {
                out.error = Some(if self.state.timed_out.load(Ordering::Relaxed) {
                    format!("cancelled past the run's timeout: {err}")
                } else {
                    err.to_string()
                });
            }
        }
        audit::record_as(
            self.facade,
            &AuditEntry {
                audit_id,
                action: "DbaSql",
                db: Some(self.db.to_string()),
                run_id: Some(self.run_id.to_string()),
                statement_seq: Some(i64::try_from(statement.seq).unwrap_or(i64::MAX)),
                statement_text: Some(statement.sql.clone()),
                is_change: !self.read_only && statement.changes,
                rows_returned: out.gives_rows.then(|| i64::try_from(out.records.len()).unwrap_or(i64::MAX)),
                rows_affected: out.rows_affected.map(|rows| i64::try_from(rows).unwrap_or(i64::MAX)),
                duration_ms: Some(duration_ms),
                error_text: out.error.clone(),
                start_time: Some(start_time),
                detail_json: Some(
                    serde_json::json!({"readOnly": self.read_only, "parameters": parameter_names(&statement.sql, self.syntax),
                        "backendId": *lock(&self.state.backend_id), "cancelled": self.state.stopped()})
                    .to_string(),
                ),
            },
            self.actor,
        );
        out
    }
}

impl StatementResult {
    /// As the service answers it: `seq`, `sql`, `givesRows`, `columns`, `records` (lists of values), `limitReached`,
    /// `rowsAffected`, `durationMs`, `error`, `rerunnable`, `skipped`, `rolledBack`, `auditId`.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = IndexMap::new();
        map.insert(
            "seq".to_string(),
            Value::I64(i64::try_from(self.seq).unwrap_or(i64::MAX)),
        );
        map.insert("sql".to_string(), Value::string(&self.sql));
        map.insert("givesRows".to_string(), Value::Bool(self.gives_rows));
        map.insert(
            "columns".to_string(),
            Value::List(self.columns.iter().map(Value::string).collect()),
        );
        map.insert(
            "records".to_string(),
            Value::List(self.records.iter().map(|record| Value::List(record.clone())).collect()),
        );
        map.insert("limitReached".to_string(), Value::Bool(self.limit_reached));
        map.insert(
            "rowsAffected".to_string(),
            self.rows_affected
                .map_or(Value::Null, |rows| Value::I64(i64::try_from(rows).unwrap_or(i64::MAX))),
        );
        map.insert("durationMs".to_string(), Value::I64(self.duration_ms));
        map.insert(
            "error".to_string(),
            self.error.clone().map_or(Value::Null, Value::String),
        );
        map.insert("rerunnable".to_string(), Value::Bool(self.rerunnable));
        map.insert("skipped".to_string(), Value::Bool(self.skipped));
        map.insert("rolledBack".to_string(), Value::Bool(self.rolled_back));
        map.insert(
            "auditId".to_string(),
            self.audit_id.clone().map_or(Value::Null, Value::String),
        );
        Value::Map(map)
    }
}

impl Statement {
    /// As the service answers it: `seq`, `sql`, `changes`, `ownTransaction`, `implicitCommit`, `transactionControl`.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = IndexMap::new();
        map.insert(
            "seq".to_string(),
            Value::I64(i64::try_from(self.seq).unwrap_or(i64::MAX)),
        );
        map.insert("sql".to_string(), Value::string(&self.sql));
        map.insert("changes".to_string(), Value::Bool(self.changes));
        map.insert("ownTransaction".to_string(), Value::Bool(self.own_transaction));
        map.insert("implicitCommit".to_string(), Value::Bool(self.implicit_commit));
        map.insert("transactionControl".to_string(), Value::Bool(self.transaction_control));
        Value::Map(map)
    }
}

/// [`info::run_sql_on`] on the database `group` names: an entity group's, or an external connection's
/// (`ext:<connectionId>`).
/// On a read-only connection every statement runs read only, whatever the caller asked: this covers each path (an
/// analyzed statement, a rolled-back one) at once.
fn run_on(facade: &EntityFacade, group: &str, sql: &str, mut options: SqlRunOptions<'_>) -> Result<SqlResult, Error> {
    let db = catalog::target(facade, group)?;
    if db.read_only {
        options.read_only = true;
    }
    info::run_sql_on(&db.tx, &db.dialect, sql, options)
}

// ---- the data grid's queries (§9.1) ----

/// Run `sql` (one query the grid wrapped, with its `?` values) read only for `actor`: under the run timeout, registered
/// for [`cancel`] under a new run id, and audited with `purpose` (`rows`, `count`, `columns`, `export`). `limit` 0 reads
/// every row.
///
/// # Errors
///
/// Returns the database's error as the refusal.
pub fn grid_query(
    facade: &EntityFacade,
    group: &str,
    sql: &str,
    params: &[Value],
    limit: usize,
    actor: &Actor,
    purpose: &str,
) -> Result<SqlResult, Refusal> {
    let registered = start_run(facade, "", actor, group, DEFAULT_TIMEOUT)?;
    let state = Arc::clone(&registered.state);
    let mut on_start = |start: SqlStart| {
        *lock(&state.backend_id) = start.backend_id;
        *lock(&state.canceller) = Some(start.canceller);
        if state.stopped() {
            let _ = state.cancel();
        }
    };
    let started = Instant::now();
    let start_time = jiff::Timestamp::now();
    let result = run_on(
        facade,
        group,
        sql,
        SqlRunOptions {
            limit,
            read_only: true,
            on_start: Some(&mut on_start),
            params,
            ..SqlRunOptions::default()
        },
    );
    *lock(&registered.state.canceller) = None;
    audit::record_as(
        facade,
        &AuditEntry {
            action: "DbaSql",
            db: Some(catalog::db_text(group)),
            run_id: Some(registered.run_id.clone()),
            statement_seq: Some(1),
            statement_text: Some(sql.to_string()),
            rows_returned: result
                .as_ref()
                .ok()
                .filter(|result| result.rows_affected.is_none())
                .map(|result| i64::try_from(result.records.len()).unwrap_or(i64::MAX)),
            duration_ms: Some(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)),
            error_text: result.as_ref().err().map(ToString::to_string),
            start_time: Some(start_time),
            detail_json: Some(serde_json::json!({"grid": purpose, "readOnly": true}).to_string()),
            ..AuditEntry::default()
        },
        actor,
    );
    drop(registered);
    result.map_err(|err| Refusal::new(err.to_string()))
}

/// Whether `sql` is one plain query a grid may wrap: `SELECT`, `WITH` (that writes nothing), `VALUES` or `TABLE`.
#[must_use]
pub fn grid_source_query(sql: &str, syntax: SqlSyntax) -> bool {
    let statements = split_sql_as(sql, syntax);
    let [statement] = statements.as_slice() else {
        return false;
    };
    matches!(
        words(statement, syntax).first().map(String::as_str),
        Some("select" | "with" | "values" | "table")
    ) && plain_read(statement, syntax)
        && !transaction_control(statement, syntax)
}

/// The SQL syntax of `group`.
#[must_use]
pub fn group_syntax(facade: &EntityFacade, group: &str) -> SqlSyntax {
    syntax_of(facade, group)
}

// ---- explain (§7.5) ----

/// One explain of [`explain`].
#[derive(Clone, Debug, Default)]
pub struct ExplainRequest {
    /// Names the explain for [`cancel`]; a new one when empty.
    pub run_id: String,
    pub group: String,
    pub sql_text: String,
    pub params: BTreeMap<String, Value>,
    /// Run the statement (Explain Analyze).
    pub analyze: bool,
    /// From [`classify`], to analyze a statement that may change data.
    pub token: String,
    /// On a production instance, to analyze a statement that may change data.
    pub confirm_database_name: String,
    /// MariaDB's `ANALYZE FORMAT=JSON` over MySQL's `EXPLAIN ANALYZE`.
    pub mariadb: bool,
    /// 0 is [`DEFAULT_TIMEOUT`].
    pub timeout: Duration,
}

/// What [`explain`] answers: the plan as the database gives it.
#[derive(Clone, Debug, Default)]
pub struct ExplainOutcome {
    pub run_id: String,
    /// `json` (Postgres, MySQL and MariaDB `FORMAT=JSON`, MariaDB `ANALYZE FORMAT=JSON`) or `text` (MySQL `EXPLAIN
    /// ANALYZE`, a tree).
    pub format: &'static str,
    pub plan: String,
    /// The statement ran (Explain Analyze).
    pub analyzed: bool,
    /// It ran in a transaction that was rolled back.
    pub rolled_back: bool,
}

/// Explain the one statement of `request`; with `analyze` it runs. A statement that may change data needs the token
/// of [`classify`] to be analyzed (and on production the database name typed), and runs in a transaction that is
/// rolled back; on MySQL and MariaDB every analyzed statement does, since there an analyzed read is not run read only.
/// The explain is registered for [`cancel`] under its run id, runs under the run timeout, and gets an audit row.
///
/// # Errors
///
/// Returns the refusal: not one statement, transaction control, a missing token or name, a parameter with no value,
/// or the database's error.
pub fn explain(
    facade: &EntityFacade,
    request: &ExplainRequest,
    actor: &Actor,
    production: bool,
) -> Result<ExplainOutcome, Refusal> {
    let group = request.group.as_str();
    let db = catalog::db_text(group);
    let syntax = syntax_of(facade, group);
    let statements = statements(facade, group, &request.sql_text).map_err(|err| Refusal::new(err.to_string()))?;
    let [statement] = statements.as_slice() else {
        return Err(Refusal::new("Explain takes one statement"));
    };
    refuse_transaction_control(&statements)?;
    let analyze = request.analyze;
    let changes = statement.changes;
    let target = catalog::target(facade, group).map_err(|err| {
        tracing::info!(error = %err, "The database {group} of an explain is not available");
        Refusal::new(crate::NOT_AVAILABLE)
    })?;
    if analyze && changes && target.read_only {
        return Err(Refusal::new("This connection is read only"));
    }
    if analyze && changes {
        let confirmed = if take_token(facade, &request.token, actor.user_id.as_deref(), &db, &request.sql_text) {
            if production {
                check_database_name(facade, group, &request.confirm_database_name)
            } else {
                Ok(())
            }
        } else {
            Err(Refusal::new(
                "Explain Analyze runs the statement, which may change data: it needs the confirmation of the statement",
            ))
        };
        if let Err(refusal) = confirmed {
            audit_refusal(facade, actor, &db, &request.run_id, &request.sql_text, &refusal);
            return Err(refusal);
        }
    }
    let (sql, values) = bind_parameters(&statement.sql, syntax, &request.params, false)?;
    let mysql = syntax == SqlSyntax::Mysql;
    let (text, format) = match (mysql, analyze) {
        (false, false) => (format!("EXPLAIN (FORMAT JSON, VERBOSE, BUFFERS) {sql}"), "json"),
        (false, true) => (
            format!("EXPLAIN (ANALYZE, FORMAT JSON, VERBOSE, BUFFERS) {sql}"),
            "json",
        ),
        (true, false) => (format!("EXPLAIN FORMAT=JSON {sql}"), "json"),
        (true, true) if request.mariadb => (format!("ANALYZE FORMAT=JSON {sql}"), "json"),
        (true, true) => (format!("EXPLAIN ANALYZE {sql}"), "text"),
    };
    let timeout = match request.timeout {
        Duration::ZERO => DEFAULT_TIMEOUT,
        timeout => timeout.min(MAX_TIMEOUT),
    };
    let registered = start_run(facade, &request.run_id, actor, group, timeout)?;
    let state = Arc::clone(&registered.state);
    let mut on_start = |start: SqlStart| {
        *lock(&state.backend_id) = start.backend_id;
        *lock(&state.canceller) = Some(start.canceller);
        if state.stopped() {
            let _ = state.cancel();
        }
    };
    let started = Instant::now();
    let start_time = jiff::Timestamp::now();
    // EXPLAIN plans without running and a Postgres analyzed read runs read only; every other analyzed statement runs
    // in a transaction that is rolled back
    let rolled_back = analyze && (changes || mysql);
    let result = if rolled_back {
        match catalog::group_tx(facade, group) {
            Err(err) => Err(err),
            Ok(tx) => {
                let mut kept = None;
                let ran = tx.run(TransactionAttribute::ForceNew, Some(MAX_TIMEOUT), || {
                    kept = Some(run_on(
                        facade,
                        group,
                        &text,
                        SqlRunOptions {
                            params: &values,
                            on_start: Some(&mut on_start),
                            discard_connection: true,
                            ..SqlRunOptions::default()
                        },
                    ));
                    Err::<(), _>(Error::new("explain analyze: rolled back"))
                });
                drop(ran);
                kept.unwrap_or_else(|| Err(Error::new("the statement did not run")))
            }
        }
    } else {
        run_on(
            facade,
            group,
            &text,
            SqlRunOptions {
                read_only: true,
                params: &values,
                on_start: Some(&mut on_start),
                ..SqlRunOptions::default()
            },
        )
    };
    *lock(&registered.state.canceller) = None;
    let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let error = result.as_ref().err().map(ToString::to_string);
    audit::record_as(
        facade,
        &AuditEntry {
            action: "DbaSql",
            db: Some(db),
            run_id: Some(registered.run_id.clone()),
            statement_seq: Some(1),
            statement_text: Some(text.clone()),
            is_change: analyze && changes,
            duration_ms: Some(duration_ms),
            error_text: error,
            start_time: Some(start_time),
            detail_json: Some(
                serde_json::json!({"explain": true, "analyze": analyze, "rolledBack": rolled_back}).to_string(),
            ),
            ..AuditEntry::default()
        },
        actor,
    );
    let run_id = registered.run_id.clone();
    drop(registered);
    let result = result.map_err(|err| Refusal::new(err.to_string()))?;
    let plan = result
        .records
        .iter()
        .filter_map(|record| record.first())
        .map(to_plain_string)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ExplainOutcome {
        run_id,
        format,
        plan,
        analyzed: analyze,
        rolled_back,
    })
}

#[cfg(test)]
#[path = "sql_tests.rs"]
mod tests;
