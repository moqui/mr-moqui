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

//! SQL runs of the Tools SQL screen (plans/Tools-Database.md §8), on an entity group's database through
//! `moqui_entity::entity::info::run_sql_with`.
//!
//! - **Read only by default:** each statement runs in a read-only transaction of its own, so the database refuses a
//!   write ([`RunRequest::allow_changes`] false).
//! - **Allow changes:** [`classify`] lists the statements that may change data and issues a one-time token
//!   ([`ClassifyOutcome::token`]), which [`run`] takes (for the same user, database and text) and spends. On a
//!   production instance the run also needs the database's name typed ([`RunRequest::confirm_database_name`]).
//!   A script runs in one transaction unless [`RunRequest::each_on_own`]; a statement that cannot run in one
//!   ([`no_transaction`]) runs on its own between the others.
//! - **Parameters:** a `:name` outside text and comments is bound as a value ([`bind_parameters`]).
//! - **Cancel:** a run is registered by its `runId` while it runs ([`cancel`]); another instance's run is found by its
//!   audit rows and cancelled with `pg_cancel_backend` / `KILL QUERY`.
//! - **Audit:** every statement gets a `DatabaseAudit` row (`<runId>_<seq>`), reads and failures included.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use moqui_core::driver::DbCancel;
use moqui_core::entity::exec::{blank_sql, split_sql_as, SqlSyntax};
use moqui_core::entity::info::{self, SqlResult, SqlRunOptions, SqlStart};
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;

use crate::audit::{self, Actor, AuditEntry};
use crate::catalog;

/// The rows a query returns unless the run asks for another limit (§8.1).
pub const DEFAULT_LIMIT: usize = 500;
/// The most rows one run may ask for.
pub const MAX_LIMIT: usize = 100_000;
/// How long a run may take unless it asks for less; past it the running statement is cancelled.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// The longest timeout a run may ask for.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(3600);
/// How long a confirmation token is good for.
pub const TOKEN_FOR: Duration = Duration::from_secs(600);

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
    /// On MySQL and MariaDB, DDL commits the transaction it runs in: a script with it is not atomic there.
    pub implicit_commit: bool,
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

/// MySQL and MariaDB DDL, which commits the transaction it runs in.
fn implicit_commit(sql: &str, syntax: SqlSyntax) -> bool {
    syntax == SqlSyntax::Mysql
        && words(sql, syntax).first().is_some_and(|word| {
            matches!(
                word.as_str(),
                "create" | "alter" | "drop" | "rename" | "truncate" | "grant" | "revoke"
            )
        })
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
            // `:name` parameters are placeholders to the database; their values do not change what it is
            let described = bind_parameters(sql, syntax, &BTreeMap::new(), true)
                .ok()
                .and_then(|(bound, _)| info::sql_gives_rows(facade, group, &bound).ok());
            let gives_rows = described.unwrap_or_else(|| info::is_query(sql));
            Statement {
                seq: index + 1,
                sql: sql.to_string(),
                changes: !(gives_rows && plain_read(sql, syntax)),
                own_transaction: no_transaction(sql, syntax),
                implicit_commit: implicit_commit(sql, syntax),
            }
        })
        .collect())
}

/// The byte ranges of the `:name` parameters of one statement, outside its text, quoted names and comments; each
/// range covers the colon and the name. A `::type` cast, MySQL's `:=`, `a.b:c` and an array slice `a[1:n]` are not
/// parameters.
fn parameter_spans(sql: &str, syntax: SqlSyntax) -> Vec<(usize, usize)> {
    let blank = blank_sql(sql, syntax);
    let code = blank.as_bytes();
    let is_ident = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut spans = Vec::new();
    let mut at = 0;
    while at < code.len() {
        let previous = at.checked_sub(1).map(|index| code[index]);
        let next = code.get(at + 1).copied();
        let starts = code[at] == b':'
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

// ---- the confirmation token ----

struct Token {
    user_id: Option<String>,
    db: String,
    text_hash: u64,
    issued: Instant,
}

fn tokens() -> &'static Mutex<HashMap<String, Token>> {
    static TOKENS: OnceLock<Mutex<HashMap<String, Token>>> = OnceLock::new();
    TOKENS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn text_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.trim().hash(&mut hasher);
    hasher.finish()
}

fn issue_token(user_id: Option<&str>, db: &str, sql_text: &str) -> Result<String, Error> {
    let token = moqui_core::entity::uuid_v4()?.replace('-', "");
    let mut map = lock(tokens());
    map.retain(|_, token| token.issued.elapsed() < TOKEN_FOR);
    map.insert(
        token.clone(),
        Token {
            user_id: user_id.map(str::to_string),
            db: db.to_string(),
            text_hash: text_hash(sql_text),
            issued: Instant::now(),
        },
    );
    Ok(token)
}

/// Spend `token`: true when it was issued to this user for this database and text in the last [`TOKEN_FOR`]. A token
/// is spent by any attempt, so a wrong text does not leave it for a second try.
fn take_token(token: &str, user_id: Option<&str>, db: &str, sql_text: &str) -> bool {
    let Some(found) = lock(tokens()).remove(token) else {
        return false;
    };
    found.issued.elapsed() < TOKEN_FOR
        && found.user_id.as_deref() == user_id
        && found.db == db
        && found.text_hash == text_hash(sql_text)
}

/// What [`classify`] answers: the statements, which of them may change data, and the token a run with changes needs.
#[derive(Clone, Debug)]
pub struct ClassifyOutcome {
    pub statements: Vec<Statement>,
    pub change_count: usize,
    pub token: String,
    /// On a production instance the run needs this typed as `confirmDatabaseName`.
    pub confirm_database_name: Option<String>,
    /// MySQL or MariaDB DDL is in the script, so one transaction does not make it atomic.
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
/// Returns the refusal: an unknown group or an empty script.
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
    let db = format!("group:{group}");
    let token = issue_token(user_id, &db, sql_text).map_err(|err| Refusal::new(err.to_string()))?;
    let confirm_database_name = if production {
        Some(database_name(facade, group).unwrap_or_else(|| group.to_string()))
    } else {
        None
    };
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
}

fn runs() -> &'static Mutex<HashMap<String, Arc<RunState>>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Arc<RunState>>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Unregisters a run when it ends, and stops its timer.
struct Registered {
    run_id: String,
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
/// them (a UUID), so `<runId>_<seq>` fits an audit id.
fn valid_run_id(run_id: &str) -> bool {
    (8..=36).contains(&run_id.len()) && run_id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
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

/// Cancel the run `run_id` for `user_id`, who must have started it unless `admin` (`DATABASE_ADMIN`). On this
/// instance through its connection's canceller; otherwise its running audit row names the instance and session.
///
/// # Errors
///
/// Returns the refusal: another user's run, or the cancel failed.
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
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let running = facade
        .find(audit::DATABASE_AUDIT)
        .and_then(|find| {
            find.condition(moqui_core::entity::Condition::Like(
                "auditId".into(),
                Value::string(format!("{run_id}\\_%")),
            ))
            .condition(moqui_core::entity::Condition::IsNull("durationMs".into()))
            .list()
        })
        .map_err(|err| {
            tracing::warn!(error = %err, "Could not look up SQL run {run_id}");
            Refusal::new("The run could not be looked up")
        })?;
    for row in running.iter() {
        let owner = row.get("userId").map(to_plain_string);
        if !admin && owner.as_deref() != user_id {
            return Err(Refusal::new("That run is another user's"));
        }
        let instance_id = row.get("instanceId").map(to_plain_string).unwrap_or_default();
        if this_instance.is_some_and(|this| this == instance_id) {
            // this instance's run that has ended since its row was written
            continue;
        }
        let detail = row.get("detailJson").map(to_plain_string).unwrap_or_default();
        let parsed: serde_json::Value = serde_json::from_str(&detail).unwrap_or_default();
        let (Some(backend_id), Some(group)) = (
            parsed.get("backendId").and_then(serde_json::Value::as_i64),
            row.get("db")
                .map(to_plain_string)
                .and_then(|db| db.strip_prefix("group:").map(str::to_string)),
        ) else {
            continue;
        };
        let sent = if catalog::group_is_mysql(facade, &group) {
            catalog::execute(facade, &group, &format!("{}{backend_id}", catalog::mysql::KILL_QUERY)).map(|_| ())
        } else {
            catalog::query_with(
                facade,
                &group,
                catalog::postgres::CANCEL_BACKEND,
                &[Value::I64(backend_id)],
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
    /// "Fetch more": skip this many rows of a run of one plain query, read only.
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
}

/// What [`run`] answers.
#[derive(Clone, Debug, Default)]
pub struct RunOutcome {
    pub run_id: String,
    pub results: Vec<StatementResult>,
    /// The script's transaction was rolled back (a statement in it failed).
    pub rolled_back: bool,
    pub cancelled: bool,
    pub timed_out: bool,
}

fn new_run_id() -> Result<String, Error> {
    moqui_core::entity::uuid_v4().map(|id| id.replace('-', ""))
}

/// Run `request` for the current user. Each statement gets an audit row.
///
/// # Errors
///
/// Returns the refusal of a run that did not start: an unknown group, an empty script, a missing or spent token, the
/// database name not typed on production, a parameter with no value, a run id in use.
pub fn run(facade: &EntityFacade, request: &RunRequest, production: bool) -> Result<RunOutcome, Refusal> {
    let actor = Actor::current();
    let group = request.group.as_str();
    let db = format!("group:{group}");
    let syntax = syntax_of(facade, group);
    let statements = statements(facade, group, &request.sql_text).map_err(|err| Refusal::new(err.to_string()))?;
    if statements.is_empty() {
        return Err(Refusal::new("There is no statement to run"));
    }
    if request.allow_changes {
        if request.token.is_empty() || !take_token(&request.token, actor.user_id.as_deref(), &db, &request.sql_text) {
            return Err(Refusal::new(
                "Running with changes allowed needs the confirmation of these statements; confirm them again",
            ));
        }
        if production {
            let name = database_name(facade, group).unwrap_or_else(|| group.to_string());
            if request.confirm_database_name.trim() != name {
                return Err(Refusal::new(format!(
                    "Type the database name ({name}) to run changes on this production instance"
                )));
            }
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
    let fetch_more = request.offset > 0;
    if fetch_more && !(read_only && statements.len() == 1 && !statements[0].changes) {
        return Err(Refusal::new(
            "Fetching more runs a single plain query again, read only; page over the rows already fetched instead",
        ));
    }
    let timeout = match request.timeout {
        Duration::ZERO => DEFAULT_TIMEOUT,
        timeout => timeout.min(MAX_TIMEOUT),
    };
    let run_id = if request.run_id.is_empty() {
        new_run_id().map_err(|err| Refusal::new(err.to_string()))?
    } else if valid_run_id(&request.run_id) {
        request.run_id.clone()
    } else {
        return Err(Refusal::new("runId is 8 to 36 letters, digits or -"));
    };
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
    let _registered = Registered {
        run_id: run_id.clone(),
        stop_timer: Some(stop),
    };
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
    let runner = Runner {
        facade,
        db: &db,
        run_id: &run_id,
        state: &state,
        actor: &actor,
        read_only,
        limit: if fetch_more { request.offset + limit } else { limit },
        offset: request.offset,
        syntax,
    };
    let mut outcome = RunOutcome {
        run_id: run_id.clone(),
        ..RunOutcome::default()
    };
    if read_only || request.each_on_own {
        for (statement, (sql, params)) in statements.iter().zip(&bound) {
            if state.cancelled.load(Ordering::Relaxed) {
                outcome.results.push(runner.skipped(statement));
                continue;
            }
            let result = if read_only || statement.own_transaction {
                runner.run_one(statement, sql, params)
            } else {
                runner
                    .in_transaction(&[(statement, sql, params)], &mut outcome.rolled_back)
                    .remove(0)
            };
            let failed = result.error.is_some();
            outcome.results.push(result);
            if failed && !request.each_on_own {
                // a read-only script stops at its first failure, as one with changes does
                for rest in &statements[outcome.results.len()..] {
                    outcome.results.push(runner.skipped(rest));
                }
                break;
            }
        }
    } else {
        // one transaction for the script, a statement that cannot run in one on its own between its parts
        let mut index = 0;
        while index < statements.len() {
            if outcome.results.iter().any(|result| result.error.is_some()) || state.cancelled.load(Ordering::Relaxed) {
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
            outcome
                .results
                .extend(runner.in_transaction(&part, &mut outcome.rolled_back));
        }
    }
    outcome.cancelled = state.cancelled.load(Ordering::Relaxed);
    outcome.timed_out = state.timed_out.load(Ordering::Relaxed);
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

impl Runner<'_> {
    fn audit_id(&self, statement: &Statement) -> String {
        format!("{}_{}", self.run_id, statement.seq)
    }

    fn skipped(&self, statement: &Statement) -> StatementResult {
        StatementResult {
            seq: statement.seq,
            sql: statement.sql.clone(),
            skipped: true,
            ..StatementResult::default()
        }
    }

    /// Statements in one transaction of their own, stopping at the first failure, which rolls the rest back. A
    /// statement that succeeded before the failure is reported, its changes rolled back with the others.
    fn in_transaction(
        &self,
        part: &[(&Statement, &String, &Vec<Value>)],
        rolled_back: &mut bool,
    ) -> Vec<StatementResult> {
        let mut results = Vec::new();
        let tx = match catalog::group_tx(self.facade, &self.state.group) {
            Ok(tx) => tx,
            Err(err) => {
                return part
                    .iter()
                    .map(|(statement, _, _)| StatementResult {
                        seq: statement.seq,
                        sql: statement.sql.clone(),
                        error: Some(err.to_string()),
                        ..StatementResult::default()
                    })
                    .collect()
            }
        };
        let ran = tx.run(TransactionAttribute::ForceNew, Some(MAX_TIMEOUT), || {
            for (statement, sql, params) in part {
                if self.state.cancelled.load(Ordering::Relaxed) {
                    results.push(self.skipped(statement));
                    continue;
                }
                let result = self.run_one(statement, sql, params);
                let failed = result.error.clone();
                results.push(result);
                if let Some(error) = failed {
                    return Err(Error::new(error));
                }
            }
            Ok(())
        });
        if let Err(err) = ran {
            *rolled_back = true;
            if results.iter().all(|result| result.error.is_none()) {
                // the commit failed
                if let Some(last) = results.last_mut() {
                    last.error = Some(format!("the transaction did not commit: {err}"));
                }
            }
            for (statement, _, _) in &part[results.len()..] {
                results.push(self.skipped(statement));
            }
        }
        results
    }

    /// Run one statement (in the transaction in place, if any), with its audit row.
    fn run_one(&self, statement: &Statement, sql: &str, params: &[Value]) -> StatementResult {
        let started = Instant::now();
        let start_time = jiff::Timestamp::now();
        let audit_id = self.audit_id(statement);
        let marker: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
        let mut on_start = |start: SqlStart| {
            *lock(&self.state.backend_id) = start.backend_id;
            *lock(&self.state.canceller) = Some(start.canceller);
            if self.state.cancelled.load(Ordering::Relaxed) {
                // cancelled between statements: this one stops as it starts
                let _ = self.state.cancel();
            }
            // the running mark, on its own thread and connection, for a cancel from another instance
            let facade = self.facade.clone();
            let actor = self.actor.clone();
            let entry = AuditEntry {
                audit_id: Some(audit_id.clone()),
                action: "DbaSql",
                db: Some(self.db.to_string()),
                statement_text: Some(statement.sql.clone()),
                is_change: !self.read_only && statement.changes,
                start_time: Some(start_time),
                detail_json: Some(
                    serde_json::json!({"runId": self.run_id, "seq": statement.seq, "backendId": start.backend_id,
                        "running": true})
                    .to_string(),
                ),
                ..AuditEntry::default()
            };
            *lock(&marker) = Some(std::thread::spawn(move || audit::mark_running(&facade, &entry, &actor)));
        };
        let result = info::run_sql_with(
            self.facade,
            &self.state.group,
            sql,
            SqlRunOptions {
                limit: self.limit,
                read_only: self.read_only,
                on_start: Some(&mut on_start),
                params,
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
                let message = if self.state.timed_out.load(Ordering::Relaxed) {
                    format!("cancelled past the run's timeout: {err}")
                } else {
                    err.to_string()
                };
                out.error = Some(message);
            }
        }
        let names: Vec<String> = parameter_names(&statement.sql, self.syntax);
        audit::record_as(
            self.facade,
            &AuditEntry {
                audit_id: Some(audit_id),
                action: "DbaSql",
                db: Some(self.db.to_string()),
                statement_text: Some(statement.sql.clone()),
                is_change: !self.read_only && statement.changes,
                rows_returned: out
                    .gives_rows
                    .then(|| i64::try_from(out.records.len()).unwrap_or(i64::MAX)),
                rows_affected: out.rows_affected.map(|rows| i64::try_from(rows).unwrap_or(i64::MAX)),
                duration_ms: Some(duration_ms),
                error_text: out.error.clone(),
                start_time: Some(start_time),
                detail_json: Some(
                    serde_json::json!({"runId": self.run_id, "seq": statement.seq, "readOnly": self.read_only,
                        "parameters": names, "backendId": *lock(&self.state.backend_id)})
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
    /// `rowsAffected`, `durationMs`, `error`, `rerunnable`, `skipped`.
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
        Value::Map(map)
    }
}

impl Statement {
    /// As the service answers it: `seq`, `sql`, `changes`, `ownTransaction`, `implicitCommit`.
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
        Value::Map(map)
    }
}

// ---- explain (§7.5) ----

/// What [`explain`] answers: the plan as the database gives it.
#[derive(Clone, Debug, Default)]
pub struct ExplainOutcome {
    /// `json` (Postgres, MySQL and MariaDB `FORMAT=JSON`, MariaDB `ANALYZE FORMAT=JSON`) or `text` (MySQL `EXPLAIN
    /// ANALYZE`, a tree).
    pub format: &'static str,
    pub plan: String,
    /// The statement ran (Explain Analyze), in a transaction rolled back when it may change data.
    pub analyzed: bool,
    pub rolled_back: bool,
}

/// Explain the one statement `sql_text` on `group`; with `analyze` it runs. A statement that may change data needs
/// `token` from [`classify`] to be analyzed, and runs in a transaction that is rolled back. `mariadb` picks MariaDB's
/// `ANALYZE FORMAT=JSON` over MySQL's `EXPLAIN ANALYZE`. The statement gets an audit row.
///
/// # Errors
///
/// Returns the refusal: not one statement, a missing token, a parameter with no value, or the database's error.
#[allow(clippy::too_many_arguments)]
pub fn explain(
    facade: &EntityFacade,
    group: &str,
    sql_text: &str,
    params: &BTreeMap<String, Value>,
    analyze: bool,
    token: &str,
    mariadb: bool,
) -> Result<ExplainOutcome, Refusal> {
    let actor = Actor::current();
    let db = format!("group:{group}");
    let syntax = syntax_of(facade, group);
    let statements = statements(facade, group, sql_text).map_err(|err| Refusal::new(err.to_string()))?;
    let [statement] = statements.as_slice() else {
        return Err(Refusal::new("Explain takes one statement"));
    };
    let changes = statement.changes;
    if analyze && changes && (token.is_empty() || !take_token(token, actor.user_id.as_deref(), &db, sql_text)) {
        return Err(Refusal::new(
            "Explain Analyze runs the statement, which may change data: it needs the confirmation of the statement",
        ));
    }
    let (sql, values) = bind_parameters(&statement.sql, syntax, params, false)?;
    let mysql = syntax == SqlSyntax::Mysql;
    let (text, format) = match (mysql, analyze) {
        (false, false) => (format!("EXPLAIN (FORMAT JSON, VERBOSE, BUFFERS) {sql}"), "json"),
        (false, true) => (
            format!("EXPLAIN (ANALYZE, FORMAT JSON, VERBOSE, BUFFERS) {sql}"),
            "json",
        ),
        (true, false) => (format!("EXPLAIN FORMAT=JSON {sql}"), "json"),
        (true, true) if mariadb => (format!("ANALYZE FORMAT=JSON {sql}"), "json"),
        (true, true) => (format!("EXPLAIN ANALYZE {sql}"), "text"),
    };
    let started = Instant::now();
    let start_time = jiff::Timestamp::now();
    let mut rolled_back = false;
    let result = if !analyze || !changes {
        // EXPLAIN plans without running, and an analyzed read only reads; MariaDB's ANALYZE is not a query to the
        // read-only check of MySQL and MariaDB, so an analyzed read runs there as it is
        let read_only = !(analyze && mysql && mariadb);
        info::run_sql_with(
            facade,
            group,
            &text,
            SqlRunOptions {
                read_only,
                params: &values,
                ..SqlRunOptions::default()
            },
        )
    } else {
        rolled_back = true;
        let tx = catalog::group_tx(facade, group).map_err(|err| Refusal::new(err.to_string()))?;
        let mut kept = None;
        let ran = tx.run(TransactionAttribute::ForceNew, Some(DEFAULT_TIMEOUT), || {
            kept = Some(info::run_sql_with(
                facade,
                group,
                &text,
                SqlRunOptions {
                    params: &values,
                    ..SqlRunOptions::default()
                },
            ));
            Err::<(), _>(Error::new("explain analyze: rolled back"))
        });
        drop(ran);
        kept.unwrap_or_else(|| Err(Error::new("the statement did not run")))
    };
    let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let error = result.as_ref().err().map(ToString::to_string);
    audit::record_as(
        facade,
        &AuditEntry {
            action: "DbaSql",
            db: Some(db),
            statement_text: Some(text.clone()),
            is_change: analyze && changes,
            duration_ms: Some(duration_ms),
            error_text: error.clone(),
            start_time: Some(start_time),
            detail_json: Some(
                serde_json::json!({"explain": true, "analyze": analyze, "rolledBack": rolled_back}).to_string(),
            ),
            ..AuditEntry::default()
        },
        &actor,
    );
    let result = result.map_err(|err| Refusal::new(err.to_string()))?;
    let plan = result
        .records
        .iter()
        .filter_map(|record| record.first())
        .map(to_plain_string)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ExplainOutcome {
        format,
        plan,
        analyzed: analyze,
        rolled_back,
    })
}

#[cfg(test)]
#[path = "sql_tests.rs"]
mod tests;
