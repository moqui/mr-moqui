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

//! An in-memory SQL database for unit tests. It understands the statements the entity layer writes
//! for the framework entities: single-table inserts, updates, deletes, and selects with `AND`,
//! `OR`, `IN`, `IS NULL`, ordering, and paging, view selects that join member tables on equal columns, and the
//! sequence refill.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_lib::error::Error;
use moqui_lib::object::{compare_obj, Value};
use moqui_lib::sync::lock;

use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};
use crate::entity::{EntityFacade, EntityGroup};
use crate::transaction::{PoolConfig, TransactionManager};

type Row = BTreeMap<String, Value>;
type Tables = BTreeMap<String, Vec<Row>>;

/// Options for an in-memory facade.
#[derive(Default)]
pub(crate) struct Options {
    /// Entity ECAs run when true.
    pub(crate) entity_eca: bool,
    /// Data feeds run when true.
    pub(crate) data_feed: bool,
    /// Every statement is appended here when set.
    pub(crate) tape: Option<Arc<Mutex<Vec<String>>>>,
}

pub(crate) fn open() -> EntityFacade {
    open_at(&framework_dir())
}

/// Seeds the framework's `StatusFlowTransition` rows, which the status check of an entity-auto update reads.
pub(crate) fn seed_status_flows(facade: &EntityFacade) {
    let entities = framework_dir().join("entity");
    for file in ["BasicEntities.xml", "ServiceEntities.xml"] {
        let text = std::fs::read_to_string(entities.join(file)).expect("read framework entities");
        for line in text
            .lines()
            .filter(|line| line.contains("<moqui.basic.StatusFlowTransition "))
        {
            let attribute = |name: &str| {
                let start = line.find(&format!(" {name}=\"")).expect("attribute") + name.len() + 3;
                line[start..].split('"').next().unwrap().to_string()
            };
            let mut row = facade.make_value("moqui.basic.StatusFlowTransition").expect("make");
            for name in ["statusFlowId", "statusId", "toStatusId"] {
                row.set(name, Value::string(attribute(name)));
            }
            // A transition can be listed twice across files; the first one wins.
            let _ = row.create(facade);
        }
    }
}

/// A facade whose statements are recorded on `tape`.
pub(crate) fn open_with_tape(tape: Arc<Mutex<Vec<String>>>) -> EntityFacade {
    open_with(
        &framework_dir(),
        Options {
            tape: Some(tape),
            ..Options::default()
        },
    )
}

/// The same database over the entity files under `framework_dir`.
pub(crate) fn open_at(framework_dir: &std::path::Path) -> EntityFacade {
    open_with(framework_dir, Options::default())
}

pub(crate) fn open_with(framework_dir: &std::path::Path, options: Options) -> EntityFacade {
    let driver: Arc<dyn DbDriver> = Arc::new(MemDriver {
        store: Arc::new(Mutex::new(BTreeMap::new())),
        tape: options.tape,
    });
    // Room for two service workers each two force-new levels deep: three ordinary connections apiece.
    let transactions = TransactionManager::new(
        driver,
        PoolConfig {
            min: 0,
            max: 8,
            reserve: 2,
            acquire_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .expect("memory pool");
    let facade = EntityFacade::open(
        framework_dir,
        "transactional",
        vec![EntityGroup::new("transactional", "postgres", transactions)],
    )
    .expect("entity facade");
    facade.set_disable_audit_log(true);
    facade.set_disable_entity_eca(!options.entity_eca);
    facade.set_disable_fk_create(true);
    facade.set_disable_data_feed(!options.data_feed);
    facade
}

/// [`open`] with `conf` in place of the framework's `MoquiDefaultConf.xml`.
pub(crate) fn open_with_conf(conf: &moqui_lib::node::MNode) -> EntityFacade {
    let driver: Arc<dyn DbDriver> = Arc::new(MemDriver {
        store: Arc::new(Mutex::new(BTreeMap::new())),
        tape: None,
    });
    let transactions = TransactionManager::new(
        driver,
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            acquire_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .expect("memory pool");
    let facade = EntityFacade::open_components_with_conf(
        &framework_dir(),
        &[],
        "transactional",
        vec![EntityGroup::new("transactional", "postgres", transactions)],
        conf,
    )
    .expect("entity facade");
    facade.set_disable_audit_log(true);
    facade.set_disable_entity_eca(true);
    facade.set_disable_fk_create(true);
    facade.set_disable_data_feed(true);
    facade
}

/// The `transactional` group and the `logging` group, each on its own database and transaction manager.
pub(crate) fn open_two_groups() -> EntityFacade {
    open_groups("logging")
}

/// The `transactional` group and `second`, each on its own database and transaction manager.
pub(crate) fn open_groups(second: &str) -> EntityFacade {
    let manager = || {
        let driver: Arc<dyn DbDriver> = Arc::new(MemDriver {
            store: Arc::new(Mutex::new(BTreeMap::new())),
            tape: None,
        });
        TransactionManager::new(
            driver,
            PoolConfig {
                min: 0,
                max: 4,
                reserve: 1,
                acquire_timeout: Duration::from_secs(5),
                ..Default::default()
            },
        )
        .expect("memory pool")
    };
    let facade = EntityFacade::open(
        &framework_dir(),
        "transactional",
        vec![
            EntityGroup::new("transactional", "postgres", manager()),
            EntityGroup::new(second, "postgres", manager()),
        ],
    )
    .expect("entity facade");
    facade.set_disable_audit_log(true);
    facade.set_disable_entity_eca(true);
    facade.set_disable_fk_create(true);
    facade.set_disable_data_feed(true);
    facade
}

pub(crate) fn framework_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework")
}

/// A log subscriber that keeps each event's fields as one line, for a test that checks what was logged.
pub(crate) struct LogSubscriber(pub(crate) Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for LogSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut visit = FieldVisit(String::new());
        event.record(&mut visit);
        self.0.lock().expect("lines").push(visit.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

struct FieldVisit(String);

impl tracing::field::Visit for FieldVisit {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }
}

struct MemDriver {
    store: Arc<Mutex<Tables>>,
    tape: Option<Arc<Mutex<Vec<String>>>>,
}

struct MemConn {
    store: Arc<Mutex<Tables>>,
    tape: Option<Arc<Mutex<Vec<String>>>>,
    local: Option<Tables>,
    dirty: BTreeSet<String>,
    /// The rows of the open stream not read yet. The select runs whole when the stream opens.
    stream: Option<std::collections::VecDeque<DbRow>>,
}

struct NoCancel;

impl DbCancel for NoCancel {
    fn cancel(&self) -> Result<(), Error> {
        Ok(())
    }
}

impl DbDriver for MemDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(MemConn {
            store: Arc::clone(&self.store),
            tape: self.tape.clone(),
            local: None,
            dirty: BTreeSet::new(),
            stream: None,
        }))
    }
}

impl DbConnection for MemConn {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, Error> {
        Ok(self.apply(sql, params)?.0)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        Ok(self.apply(sql, params)?.1)
    }

    fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query(sql, params)
    }

    fn open_stream(&mut self, sql: &str, params: &[Value]) -> Result<(), Error> {
        let rows = self.apply(sql, params)?.1;
        self.stream = Some(rows.into());
        Ok(())
    }

    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        Ok(self.stream.as_mut().and_then(std::collections::VecDeque::pop_front))
    }

    fn close_stream(&mut self) -> Result<(), Error> {
        self.stream = None;
        Ok(())
    }

    fn stream_open(&self) -> bool {
        self.stream.is_some()
    }

    fn begin(&mut self) -> Result<(), Error> {
        if self.local.is_none() {
            let committed = lock(&self.store).clone();
            self.local = Some(committed);
            self.dirty.clear();
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        let Some(local) = self.local.take() else {
            return Ok(());
        };
        let dirty = std::mem::take(&mut self.dirty);
        let mut committed = lock(&self.store);
        for table in dirty {
            let rows = local.get(&table).cloned().unwrap_or_default();
            committed.insert(table, rows);
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), Error> {
        self.local = None;
        self.dirty.clear();
        Ok(())
    }

    fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

impl MemConn {
    fn apply(&mut self, sql: &str, params: &[Value]) -> Result<(u64, Vec<DbRow>), Error> {
        let sql = sql.trim();
        // The pool checks out a connection with `SELECT 1`.
        if let Some(tape) = &self.tape {
            if !sql.eq_ignore_ascii_case("SELECT 1") {
                lock(tape).push(sql.to_string());
            }
        }
        if sql.eq_ignore_ascii_case("SELECT 1") {
            return Ok((
                1,
                vec![DbRow {
                    columns: vec![("1".to_string(), Value::I64(1))],
                }],
            ));
        }
        if sql.contains("ON CONFLICT") && sql.contains("sequence_value_item") {
            return self.sequence(params);
        }
        if sql.contains("xmin") && sql.contains("txid_current") {
            return self.xmin_rows(sql);
        }
        let stmt = parse(sql).map_err(|err| Error::new(format!("{err} sql={sql}")))?;
        self.exec(stmt, params)
    }

    /// The feed's "rows written by this transaction" query. Every row counts as written.
    fn xmin_rows(&self, sql: &str) -> Result<(u64, Vec<DbRow>), Error> {
        let Some(from_at) = sql.find(" FROM ") else {
            return Err(Error::new("xmin query has no table"));
        };
        let columns: Vec<_> = sql["SELECT ".len()..from_at]
            .split(", ")
            .map(|item| {
                let (column, alias) = item.split_once(" AS ").unwrap_or((item, item));
                (
                    column.trim_matches('"').to_string(),
                    alias.trim_matches('"').to_string(),
                )
            })
            .collect();
        let tail = sql[from_at + 6..].trim_start();
        let table = tail
            .strip_prefix('"')
            .and_then(|text| text.split('"').next())
            .unwrap_or("")
            .to_string();
        if table.is_empty() || columns.is_empty() {
            return Err(Error::new("xmin query is missing columns"));
        }
        let rows = self.read_table(&table);
        let projected: Vec<DbRow> = rows
            .iter()
            .map(|row| DbRow {
                columns: columns
                    .iter()
                    .map(|(column, alias)| (alias.clone(), row.get(column).cloned().unwrap_or(Value::Null)))
                    .collect(),
            })
            .collect();
        let count = u64::try_from(projected.len()).unwrap_or(u64::MAX);
        Ok((count, projected))
    }

    fn sequence(&mut self, params: &[Value]) -> Result<(u64, Vec<DbRow>), Error> {
        if params.len() < 4 {
            return Err(Error::new("sequence refill is missing parameters"));
        }
        let name = params[0].clone();
        let first_end = as_i64(&params[1])?;
        // With a lastUpdatedStamp column the stamp is the third insert value.
        let rest = &params[params.len() - 2..];
        let increment = as_i64(&rest[0])?;
        let hint_end = as_i64(&rest[1])?;
        let mut rows = self.read_table("sequence_value_item");
        let end = if let Some(row) = rows.iter_mut().find(|row| row.get("seq_name") == Some(&name)) {
            let old = row.get("seq_num").and_then(|value| as_i64(value).ok()).unwrap_or(0);
            let new_end = old.saturating_add(increment).max(hint_end);
            row.insert("seq_num".to_string(), Value::I64(new_end));
            new_end
        } else {
            let mut row = BTreeMap::new();
            row.insert("seq_name".to_string(), name);
            row.insert("seq_num".to_string(), Value::I64(first_end));
            rows.push(row);
            first_end
        };
        self.put_table("sequence_value_item", rows);
        Ok((
            1,
            vec![DbRow {
                columns: vec![("seq_num".to_string(), Value::I64(end))],
            }],
        ))
    }

    fn exec(&mut self, stmt: Stmt, params: &[Value]) -> Result<(u64, Vec<DbRow>), Error> {
        match stmt {
            Stmt::Insert { table, columns } => {
                let mut row = BTreeMap::new();
                for (index, column) in columns.iter().enumerate() {
                    let value = params
                        .get(index)
                        .cloned()
                        .ok_or_else(|| Error::new("insert is missing a parameter"))?;
                    row.insert(column.clone(), value);
                }
                let mut rows = self.read_table(&table);
                rows.push(row);
                self.put_table(&table, rows);
                Ok((1, Vec::new()))
            }
            Stmt::Update { table, columns, pred } => {
                let values = columns
                    .iter()
                    .enumerate()
                    .map(|(index, _)| {
                        params
                            .get(index)
                            .cloned()
                            .ok_or_else(|| Error::new("update is missing a parameter"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let start = columns.len();
                let mut rows = self.read_table(&table);
                let mut count = 0_u64;
                for row in &mut rows {
                    if keep(&pred, row, params, start)? {
                        for (column, value) in columns.iter().zip(&values) {
                            row.insert(column.clone(), value.clone());
                        }
                        count += 1;
                    }
                }
                self.put_table(&table, rows);
                Ok((count, Vec::new()))
            }
            Stmt::Delete { table, pred } => {
                let rows = self.read_table(&table);
                let mut kept = Vec::new();
                let mut count = 0_u64;
                for row in rows {
                    if keep(&pred, &row, params, 0)? {
                        count += 1;
                    } else {
                        kept.push(row);
                    }
                }
                self.put_table(&table, kept);
                Ok((count, Vec::new()))
            }
            Stmt::Select(query) => self.exec_select(&query, params),
        }
    }

    /// The rows of the select's table, and of each join, with every column also under `"ALIAS.column"`.
    fn select_rows(&self, query: &SelectQuery) -> Vec<Row> {
        let qualified = |row: &Row, alias: &str, into: &mut Row| {
            for (column, value) in row {
                into.insert(format!("{alias}.{column}"), value.clone());
            }
        };
        let mut rows: Vec<Row> = self
            .read_table(&query.table)
            .into_iter()
            .map(|row| {
                let mut out = row.clone();
                qualified(&row, query.alias.as_deref().unwrap_or(&query.table), &mut out);
                out
            })
            .collect();
        for join in &query.joins {
            let right = self.read_table(&join.table);
            let mut next = Vec::new();
            for left in rows {
                let mut found = false;
                for candidate in &right {
                    let matches = join.on.iter().all(|(one, two)| {
                        let mut probe = Row::new();
                        qualified(candidate, &join.alias, &mut probe);
                        let read = |name: &str| left.get(name).or_else(|| probe.get(name)).filter(|v| !v.is_null());
                        matches!((read(one), read(two)), (Some(a), Some(b)) if a == b)
                    });
                    if matches {
                        let mut merged = left.clone();
                        qualified(candidate, &join.alias, &mut merged);
                        next.push(merged);
                        found = true;
                    }
                }
                if !found && join.outer {
                    next.push(left);
                }
            }
            rows = next;
        }
        rows
    }

    fn exec_select(&mut self, query: &SelectQuery, params: &[Value]) -> Result<(u64, Vec<DbRow>), Error> {
        let mut matched = Vec::new();
        for row in self.select_rows(query) {
            if keep(&query.pred, &row, params, 0)? {
                matched.push(row);
            }
        }
        if query.count {
            let n = i64::try_from(matched.len()).unwrap_or(i64::MAX);
            return Ok((
                1,
                vec![DbRow {
                    columns: vec![("count".to_string(), Value::I64(n))],
                }],
            ));
        }
        matched.sort_by(|left, right| sort_rows(left, right, &query.order));
        let mut idx = pred_slots(&query.pred);
        // The statement reads LIMIT then OFFSET, and the parameters follow that order.
        let take_n = if query.limit {
            Some(as_i64(take_param(params, &mut idx)?)?)
        } else {
            None
        };
        let skip = if query.offset {
            as_i64(take_param(params, &mut idx)?)?
        } else {
            0
        };
        let start = usize::try_from(skip.max(0)).unwrap_or(0);
        let window: Vec<Row> = match take_n {
            Some(limit) => {
                let len = usize::try_from(limit.max(0)).unwrap_or(0);
                matched.into_iter().skip(start).take(len).collect()
            }
            None => matched.into_iter().skip(start).collect(),
        };
        let projected: Vec<DbRow> = window
            .iter()
            .map(|row| {
                let mut out = project(row, &query.columns);
                for (column, name) in out.columns.iter_mut().zip(&query.names) {
                    column.0.clone_from(name);
                }
                out
            })
            .collect();
        let n = u64::try_from(projected.len()).unwrap_or(u64::MAX);
        Ok((n, projected))
    }

    fn read_table(&self, table: &str) -> Vec<Row> {
        if let Some(local) = &self.local {
            return local.get(table).cloned().unwrap_or_default();
        }
        lock(&self.store).get(table).cloned().unwrap_or_default()
    }

    fn put_table(&mut self, table: &str, rows: Vec<Row>) {
        if let Some(local) = self.local.as_mut() {
            local.insert(table.to_string(), rows);
            self.dirty.insert(table.to_string());
            return;
        }
        lock(&self.store).insert(table.to_string(), rows);
    }
}

fn keep(pred: &Option<Pred>, row: &Row, params: &[Value], start: usize) -> Result<bool, Error> {
    let Some(pred) = pred else {
        return Ok(true);
    };
    let mut idx = start;
    eval_pred(pred, row, params, &mut idx)
}

fn pred_slots(pred: &Option<Pred>) -> usize {
    pred.as_ref().map(count_slots).unwrap_or(0)
}

fn count_slots(pred: &Pred) -> usize {
    match pred {
        Pred::Cmp { .. } => 1,
        Pred::In { slots, .. } => *slots,
        Pred::IsNull { .. } => 0,
        Pred::And(parts) | Pred::Or(parts) => parts.iter().map(count_slots).sum(),
    }
}

fn eval_pred(pred: &Pred, row: &Row, params: &[Value], idx: &mut usize) -> Result<bool, Error> {
    match pred {
        Pred::Cmp { field, op } => {
            let param = take_param(params, idx)?;
            let left = row.get(field).cloned().unwrap_or(Value::Null);
            Ok(compare(&left, *op, param))
        }
        Pred::In { field, slots, negated } => {
            let left = row.get(field).cloned().unwrap_or(Value::Null);
            let mut found = false;
            for _ in 0..*slots {
                if compare(&left, Op::Eq, take_param(params, idx)?) {
                    found = true;
                }
            }
            Ok(if *negated { !found && !left.is_null() } else { found })
        }
        Pred::IsNull { field, negated } => {
            let is_null = row.get(field).is_none_or(Value::is_null);
            Ok(if *negated { !is_null } else { is_null })
        }
        Pred::And(parts) => {
            let mut all = true;
            for part in parts {
                if !eval_pred(part, row, params, idx)? {
                    all = false;
                }
            }
            Ok(all)
        }
        Pred::Or(parts) => {
            let mut any = false;
            for part in parts {
                if eval_pred(part, row, params, idx)? {
                    any = true;
                }
            }
            Ok(any)
        }
    }
}

fn compare(left: &Value, op: Op, right: &Value) -> bool {
    let ordering = cmp_values(left, right);
    match op {
        Op::Eq => left == right,
        Op::Ne => left != right,
        Op::Lt => ordering == std::cmp::Ordering::Less,
        Op::Gt => ordering == std::cmp::Ordering::Greater,
        Op::Le => ordering != std::cmp::Ordering::Greater,
        Op::Ge => ordering != std::cmp::Ordering::Less,
        Op::Like => like_matches(left, right),
        Op::NotLike => !left.is_null() && !like_matches(left, right),
    }
}

/// SQL `LIKE` with `%` and `_`, on text.
fn like_matches(left: &Value, pattern: &Value) -> bool {
    let (Some(text), Some(pattern)) = (left.as_str(), pattern.as_str()) else {
        return false;
    };
    fn walk(text: &[char], pattern: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('%', rest)) => (0..=text.len()).any(|skip| walk(&text[skip..], rest)),
            Some((symbol, rest)) => text
                .split_first()
                .is_some_and(|(first, tail)| (*symbol == '_' || symbol == first) && walk(tail, rest)),
        }
    }
    walk(&text.chars().collect::<Vec<_>>(), &pattern.chars().collect::<Vec<_>>())
}

fn cmp_values(left: &Value, right: &Value) -> std::cmp::Ordering {
    if left.is_null() && right.is_null() {
        return std::cmp::Ordering::Equal;
    }
    if left.is_null() {
        return std::cmp::Ordering::Greater;
    }
    if right.is_null() {
        return std::cmp::Ordering::Less;
    }
    compare_obj(left, right).unwrap_or(std::cmp::Ordering::Equal)
}

/// Nulls sort last unless the key says first, in either direction, as the SQL is written.
fn sort_rows(left: &Row, right: &Row, order: &[OrderKey]) -> std::cmp::Ordering {
    for key in order {
        let read = |row: &Row| match row.get(&key.field).cloned().unwrap_or(Value::Null) {
            Value::String(text) if key.upper => Value::String(text.to_uppercase()),
            other => other,
        };
        let (left_value, right_value) = (read(left), read(right));
        let ordering = match (left_value.is_null(), right_value.is_null()) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) if key.nulls_first => std::cmp::Ordering::Less,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) if key.nulls_first => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) if key.desc => cmp_values(&left_value, &right_value).reverse(),
            (false, false) => cmp_values(&left_value, &right_value),
        };
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    std::cmp::Ordering::Equal
}

fn project(row: &Row, columns: &[String]) -> DbRow {
    DbRow {
        columns: columns
            .iter()
            .map(|name| (name.clone(), row.get(name).cloned().unwrap_or(Value::Null)))
            .collect(),
    }
}

fn take_param<'a>(params: &'a [Value], idx: &mut usize) -> Result<&'a Value, Error> {
    let value = params.get(*idx).ok_or_else(|| Error::new("sql parameter missing"))?;
    *idx += 1;
    Ok(value)
}

fn as_i64(value: &Value) -> Result<i64, Error> {
    match value {
        Value::I64(number) => Ok(*number),
        Value::I32(number) => Ok(i64::from(*number)),
        Value::String(text) => text.parse().map_err(|_| Error::new("parameter is not an integer")),
        _ => Err(Error::new("parameter is not an integer")),
    }
}

enum Stmt {
    Insert {
        table: String,
        columns: Vec<String>,
    },
    Update {
        table: String,
        columns: Vec<String>,
        pred: Option<Pred>,
    },
    Delete {
        table: String,
        pred: Option<Pred>,
    },
    Select(SelectQuery),
}

struct SelectQuery {
    count: bool,
    columns: Vec<String>,
    /// The name each column is returned as.
    names: Vec<String>,
    table: String,
    /// The alias of `table` in a view select.
    alias: Option<String>,
    joins: Vec<Join>,
    pred: Option<Pred>,
    order: Vec<OrderKey>,
    limit: bool,
    offset: bool,
}

enum Pred {
    Cmp {
        field: String,
        op: Op,
    },
    /// `IN (?, ...)` with `slots` placeholders, or `NOT IN` when `negated`. An empty list is false.
    /// A null never matches either, as in SQL.
    In {
        field: String,
        slots: usize,
        negated: bool,
    },
    IsNull {
        field: String,
        negated: bool,
    },
    And(Vec<Pred>),
    Or(Vec<Pred>),
}

#[derive(Clone, Copy)]
enum Op {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Like,
    NotLike,
}

/// A member table joined on equal columns, `ON "A"."x" = "B"."x" AND ...`.
struct Join {
    table: String,
    alias: String,
    outer: bool,
    on: Vec<(String, String)>,
}

struct OrderKey {
    field: String,
    desc: bool,
    nulls_first: bool,
    upper: bool,
}

fn parse(sql: &str) -> Result<Stmt, Error> {
    let mut cur = Cur { sql, i: 0 };
    let stmt = if cur.eat_keyword("INSERT") {
        parse_insert(&mut cur)?
    } else if cur.eat_keyword("UPDATE") {
        parse_update(&mut cur)?
    } else if cur.eat_keyword("DELETE") {
        parse_delete(&mut cur)?
    } else if cur.eat_keyword("SELECT") {
        parse_select(&mut cur)?
    } else {
        return Err(cur.fail("unsupported sql"));
    };
    Ok(stmt)
}

fn parse_insert(cur: &mut Cur) -> Result<Stmt, Error> {
    cur.expect_keyword("INTO")?;
    let table = cur.quoted()?;
    cur.expect_symbol(b'(')?;
    let mut columns = Vec::new();
    loop {
        columns.push(cur.quoted()?);
        if !cur.eat_symbol(b',') {
            break;
        }
    }
    cur.expect_symbol(b')')?;
    cur.expect_keyword("VALUES")?;
    cur.expect_symbol(b'(')?;
    for index in 0..columns.len() {
        if index > 0 {
            cur.expect_symbol(b',')?;
        }
        cur.expect_symbol(b'?')?;
    }
    cur.expect_symbol(b')')?;
    cur.expect_end()?;
    Ok(Stmt::Insert { table, columns })
}

fn parse_update(cur: &mut Cur) -> Result<Stmt, Error> {
    let table = cur.quoted()?;
    cur.expect_keyword("SET")?;
    let mut columns = Vec::new();
    loop {
        columns.push(cur.quoted()?);
        cur.expect_symbol(b'=')?;
        cur.expect_symbol(b'?')?;
        if !cur.eat_symbol(b',') {
            break;
        }
    }
    let pred = if cur.eat_keyword("WHERE") {
        Some(parse_or(cur)?)
    } else {
        None
    };
    cur.expect_end()?;
    Ok(Stmt::Update { table, columns, pred })
}

fn parse_delete(cur: &mut Cur) -> Result<Stmt, Error> {
    cur.expect_keyword("FROM")?;
    let table = cur.quoted()?;
    let pred = if cur.eat_keyword("WHERE") {
        Some(parse_or(cur)?)
    } else {
        None
    };
    cur.expect_end()?;
    Ok(Stmt::Delete { table, pred })
}

fn parse_select(cur: &mut Cur) -> Result<Stmt, Error> {
    let count = if cur.eat_keyword("COUNT") {
        cur.expect_symbol(b'(')?;
        cur.expect_symbol(b'*')?;
        cur.expect_symbol(b')')?;
        true
    } else {
        false
    };
    let (columns, names): (Vec<String>, Vec<String>) = if count {
        (Vec::new(), Vec::new())
    } else {
        parse_columns(cur)?.into_iter().unzip()
    };
    cur.expect_keyword("FROM")?;
    // A view of several members joins them in parentheses: FROM ("A" "X" INNER JOIN "B" "Y" ON ...).
    let joined = cur.eat_symbol(b'(');
    let table = cur.quoted_part()?;
    // A view select aliases its member table: FROM "Product" "PROD".
    cur.skip();
    let alias = if cur.peek_symbol(b'"') {
        Some(cur.quoted_part()?)
    } else {
        None
    };
    let mut joins = Vec::new();
    if joined {
        while !cur.eat_symbol(b')') {
            let outer = if cur.eat_keyword("LEFT") {
                cur.expect_keyword("OUTER")?;
                true
            } else {
                cur.expect_keyword("INNER")?;
                false
            };
            cur.expect_keyword("JOIN")?;
            let table = cur.quoted_part()?;
            let alias = cur.quoted_part()?;
            cur.expect_keyword("ON")?;
            let mut on = Vec::new();
            loop {
                let left = cur.quoted()?;
                cur.expect_symbol(b'=')?;
                on.push((left, cur.quoted()?));
                if !cur.eat_keyword("AND") {
                    break;
                }
            }
            joins.push(Join {
                table,
                alias,
                outer,
                on,
            });
        }
    }
    let pred = if cur.eat_keyword("WHERE") {
        Some(parse_or(cur)?)
    } else {
        None
    };
    let order = if cur.eat_keyword("ORDER") {
        cur.expect_keyword("BY")?;
        // `ORDER BY 1` sorts by the first column.
        if cur.eat_symbol(b'1') {
            columns
                .first()
                .map(|field| OrderKey {
                    field: field.clone(),
                    desc: false,
                    nulls_first: false,
                    upper: false,
                })
                .into_iter()
                .collect()
        } else {
            parse_order(cur)?
        }
    } else {
        Vec::new()
    };
    let limit = if cur.eat_keyword("LIMIT") {
        cur.expect_symbol(b'?')?;
        true
    } else {
        false
    };
    let offset = if cur.eat_keyword("OFFSET") {
        cur.expect_symbol(b'?')?;
        true
    } else {
        false
    };
    // One connection at a time holds the store, so a row lock has nothing more to do.
    if cur.eat_keyword("FOR") && !cur.eat_keyword("UPDATE") {
        return Err(cur.fail("unexpected sql"));
    }
    cur.expect_end()?;
    Ok(Stmt::Select(SelectQuery {
        count,
        columns,
        names,
        table,
        alias,
        joins,
        pred,
        order,
        limit,
        offset,
    }))
}

/// Each column and the name it is returned as: the `AS` alias, or its own without the table alias, as a database
/// labels `"I"."code"` as `code`.
fn parse_columns(cur: &mut Cur) -> Result<Vec<(String, String)>, Error> {
    let mut columns = Vec::new();
    loop {
        if cur.peek_keyword("FROM") {
            break;
        }
        let column = cur.quoted()?;
        let name = if cur.eat_keyword("AS") {
            cur.quoted()?
        } else {
            column.rsplit('.').next().unwrap_or(&column).to_string()
        };
        columns.push((column, name));
        if !cur.eat_symbol(b',') {
            break;
        }
    }
    if columns.is_empty() {
        return Err(cur.fail("select has no columns"));
    }
    Ok(columns)
}

fn parse_order(cur: &mut Cur) -> Result<Vec<OrderKey>, Error> {
    let mut order = Vec::new();
    loop {
        let upper = cur.eat_keyword("UPPER") || cur.eat_keyword("LOWER");
        if upper {
            cur.expect_symbol(b'(')?;
        }
        let field = cur.quoted()?;
        if upper {
            cur.expect_symbol(b')')?;
        }
        let desc = if cur.eat_keyword("DESC") {
            true
        } else if cur.eat_keyword("ASC") {
            false
        } else {
            return Err(cur.fail("expected ASC or DESC"));
        };
        let nulls_first = if cur.eat_keyword("NULLS") {
            if cur.eat_keyword("FIRST") {
                true
            } else {
                cur.expect_keyword("LAST")?;
                false
            }
        } else {
            false
        };
        order.push(OrderKey {
            field,
            desc,
            nulls_first,
            upper,
        });
        if !cur.eat_symbol(b',') {
            break;
        }
    }
    Ok(order)
}

fn parse_or(cur: &mut Cur) -> Result<Pred, Error> {
    let mut parts = vec![parse_and(cur)?];
    while cur.eat_keyword("OR") {
        parts.push(parse_and(cur)?);
    }
    Ok(fold(parts, false))
}

fn parse_and(cur: &mut Cur) -> Result<Pred, Error> {
    let mut parts = vec![parse_primary(cur)?];
    while cur.eat_keyword("AND") {
        parts.push(parse_primary(cur)?);
    }
    Ok(fold(parts, true))
}

fn parse_primary(cur: &mut Cur) -> Result<Pred, Error> {
    if cur.eat_symbol(b'(') {
        let pred = parse_or(cur)?;
        cur.expect_symbol(b')')?;
        return Ok(pred);
    }
    let field = cur.quoted()?;
    if cur.eat_keyword("IS") {
        let negated = cur.eat_keyword("NOT");
        cur.expect_keyword("NULL")?;
        return Ok(Pred::IsNull { field, negated });
    }
    let negated = cur.eat_keyword("NOT");
    if cur.eat_keyword("LIKE") {
        cur.expect_symbol(b'?')?;
        return Ok(Pred::Cmp {
            field,
            op: if negated { Op::NotLike } else { Op::Like },
        });
    }
    if cur.eat_keyword("IN") {
        cur.expect_symbol(b'(')?;
        let mut slots = 0;
        while !cur.eat_symbol(b')') {
            cur.expect_symbol(b'?')?;
            slots += 1;
            cur.eat_symbol(b',');
        }
        return Ok(Pred::In { field, slots, negated });
    }
    if negated {
        return Err(cur.fail("expected IN after NOT"));
    }
    let Some(op) = cur.eat_op() else {
        return Err(cur.fail("expected comparison"));
    };
    cur.expect_symbol(b'?')?;
    Ok(Pred::Cmp { field, op })
}

fn fold(mut parts: Vec<Pred>, and: bool) -> Pred {
    if parts.len() == 1 {
        return parts.remove(0);
    }
    if and {
        Pred::And(parts)
    } else {
        Pred::Or(parts)
    }
}

struct Cur<'a> {
    sql: &'a str,
    i: usize,
}

impl Cur<'_> {
    fn skip(&mut self) {
        while self.i < self.sql.len() && self.sql.as_bytes()[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat_keyword(&mut self, word: &str) -> bool {
        self.skip();
        let rest = &self.sql[self.i..];
        if rest.len() < word.len() || !rest.starts_with(word) {
            return false;
        }
        let end = self.i + word.len();
        if end < self.sql.len() {
            let next = self.sql.as_bytes()[end];
            if next.is_ascii_alphanumeric() || next == b'_' {
                return false;
            }
        }
        self.i = end;
        true
    }

    fn peek_keyword(&mut self, word: &str) -> bool {
        let saved = self.i;
        let found = self.eat_keyword(word);
        self.i = saved;
        found
    }

    fn eat_symbol(&mut self, symbol: u8) -> bool {
        self.skip();
        if self.i < self.sql.len() && self.sql.as_bytes()[self.i] == symbol {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn eat_op(&mut self) -> Option<Op> {
        self.skip();
        let rest = &self.sql[self.i..];
        let (op, len) = if rest.starts_with("<>") {
            (Op::Ne, 2)
        } else if rest.starts_with("<=") {
            (Op::Le, 2)
        } else if rest.starts_with(">=") {
            (Op::Ge, 2)
        } else if rest.starts_with('<') {
            (Op::Lt, 1)
        } else if rest.starts_with('>') {
            (Op::Gt, 1)
        } else if rest.starts_with('=') {
            (Op::Eq, 1)
        } else {
            return None;
        };
        self.i += len;
        Some(op)
    }

    fn peek_symbol(&mut self, symbol: u8) -> bool {
        self.skip();
        self.i < self.sql.len() && self.sql.as_bytes()[self.i] == symbol
    }

    /// A quoted name, or `"ALIAS"."name"`, which a select's rows also carry under `"ALIAS.name"`.
    fn quoted(&mut self) -> Result<String, Error> {
        let first = self.quoted_part()?;
        if self.i < self.sql.len() && self.sql.as_bytes()[self.i] == b'.' {
            self.i += 1;
            return Ok(format!("{first}.{}", self.quoted_part()?));
        }
        Ok(first)
    }

    fn quoted_part(&mut self) -> Result<String, Error> {
        if !self.eat_symbol(b'"') {
            return Err(self.fail("expected identifier"));
        }
        let start = self.i;
        while self.i < self.sql.len() && self.sql.as_bytes()[self.i] != b'"' {
            self.i += 1;
        }
        if self.i >= self.sql.len() {
            return Err(self.fail("unterminated identifier"));
        }
        let name = self.sql[start..self.i].to_string();
        self.i += 1;
        Ok(name)
    }

    fn expect_keyword(&mut self, word: &str) -> Result<(), Error> {
        if self.eat_keyword(word) {
            Ok(())
        } else {
            Err(self.fail(&format!("expected {word}")))
        }
    }

    fn expect_symbol(&mut self, symbol: u8) -> Result<(), Error> {
        if self.eat_symbol(symbol) {
            Ok(())
        } else {
            Err(self.fail(&format!("expected {}", char::from(symbol))))
        }
    }

    fn expect_end(&mut self) -> Result<(), Error> {
        self.skip();
        if self.i == self.sql.len() {
            Ok(())
        } else {
            Err(self.fail("unexpected sql"))
        }
    }

    fn fail(&self, message: &str) -> Error {
        let rest: String = self.sql[self.i..].chars().take(160).collect();
        Error::new(format!("{message} at '{rest}'"))
    }
}
