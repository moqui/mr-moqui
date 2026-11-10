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

//! One ETL run (plans/archive/Tools-Database.md §10.3, decision 7): the source's extractor, the [`MappingTransformer`],
//! and a loader of the target, an entity (the entity-auto `create#`, `store#` or `update#` service per row) or a
//! service (one call per row), each row's number followed through the load. Every write is a service call as the user,
//! with authorization on: the entity's ECAs and the security rules run as they would for the user's own call.
//!
//! - **No batch size:** the run is one transaction. Any error rolls it all back; the extract and transform errors are
//!   all collected; the first database error ends the loading, since Postgres aborts the transaction then.
//! - **A batch size:** a batch that fails is rolled back and recorded (`failedRanges`); `stopOnError` decides whether
//!   the run goes on. With `stopOnError` any error ends the run as failed, and the batch it was in is rolled back.
//!   `lastCommittedRow` is the last row of the last batch committed before any batch failed, so a rerun from
//!   `nextStartRow` (`lastCommittedRow + 1`; `startRow` is the first row read) loads no committed row again and every
//!   failed one. `loadCount` counts committed rows only.
//! - **Check:** the extract and transform with no write, never stopping at an error: the per-row errors, the foreign
//!   keys with no related record (the rule `fk::enforce` uses), and for an entity target the diff against the database
//!   (`check_info_limited`, at most [`MAX_DIFF_ROWS`]). A known limit: a row whose related record an earlier row of the
//!   same source creates is reported as missing its foreign key, since a check writes nothing.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::etl::{EntryTransform, Loader, SimpleEntry, SimpleEtl, ROW_NUMBER_KEY};
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{EntityFacade, EtlLoader, EtlTransactions};

use super::mapping::{LoadMode, Mapping, TargetKind};
use super::source::{self, SourceOptions};
use super::transform::{row_number_of, ErrorLog, MappingTransformer, RowError, SharedErrors};

/// The most diff rows a check keeps.
pub const MAX_DIFF_ROWS: usize = 1000;
/// Progress is written every this many rows.
pub const PROGRESS_EVERY: u64 = 1000;

/// What a run is asked to do, beyond its mapping.
#[derive(Clone, Default)]
pub struct RunOptions {
    pub source: SourceOptions,
    /// Extract and transform only, and report.
    pub check: bool,
    /// Write in the caller's transaction (`load#SpreadsheetEntity`), stopping at the first error.
    pub join_caller: bool,
    /// Called every [`PROGRESS_EVERY`] rows with the counts so far.
    pub progress: Option<ProgressFn>,
}

/// Told of a run's counts as it goes.
pub type ProgressFn = Arc<dyn Fn(&Progress) + Send + Sync>;

/// The counts of a run so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// The rows read from the source.
    pub extract_count: u64,
    /// The rows committed.
    pub load_count: u64,
    /// The rows with an error.
    pub error_count: u64,
    pub last_committed_row: i64,
}

/// A batch that was rolled back: its first and last rows, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FailedRange {
    pub from_row: i64,
    pub to_row: i64,
    pub message: String,
}

impl FailedRange {
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = IndexMap::default();
        map.insert("fromRow".to_string(), Value::I64(self.from_row));
        map.insert("toRow".to_string(), Value::I64(self.to_row));
        map.insert("message".to_string(), Value::string(&self.message));
        Value::Map(map)
    }
}

/// How a run ended.
#[derive(Debug, Default)]
pub struct RunOutcome {
    /// The source as read: a location (with its sheet), an entity, a SQL source.
    pub resolved_source: String,
    /// The rows read from the source.
    pub extract_count: u64,
    pub skip_count: u64,
    /// The rows committed (or, joining, left in the caller's transaction).
    pub load_count: u64,
    /// The rows with an error.
    pub error_count: u64,
    /// The errors, one per field.
    pub field_error_count: u64,
    /// The last row of the last batch committed before any batch failed (with no batches, the last row once the run
    /// committed); 0 for none.
    pub last_committed_row: i64,
    /// Where a rerun starts: `last_committed_row + 1`, or the run's own start row when nothing committed.
    pub next_start_row: i64,
    pub committed_batches: u64,
    pub failed_batches: u64,
    /// The batches rolled back.
    pub failed_ranges: Vec<FailedRange>,
    /// The first [`super::transform::MAX_ERROR_ROWS`] errors, each with its row and field.
    pub errors: Vec<RowError>,
    /// Every failed row, for the error file.
    pub error_csv: Option<String>,
    /// The source could not be read, or the run stopped: why.
    pub failure: Option<String>,
    /// A check of an entity target: `entity`, `pk`, `notFound`, … rows (`CheckResult::diff_info`).
    pub diff: Vec<moqui_lib::object::ValueMap>,
    pub fields_checked: u64,
    /// Whether what loaded was committed (or, joining, left in the caller's transaction).
    pub committed: bool,
}

/// The loader of a run: the target's loader, each row's number followed, the rows' counts and errors kept.
struct RowLoader {
    inner: Option<EtlLoader>,
    check_target: Option<String>,
    facade: EntityFacade,
    errors: SharedErrors,
    batched: bool,
    /// With no batches, the first write error ended the loading: the rest are not written.
    dead: bool,
    /// The rows read from the source so far, counted by the run's numbering transformer.
    extracted: Arc<AtomicI64>,
    /// Rows the loader saw, for the progress cadence.
    seen: u64,
    /// Rows written and not yet committed: in the batch, or with no batches in the run.
    pending: u64,
    /// Rows committed.
    loaded: u64,
    last_ok_row: i64,
    last_committed_row: i64,
    /// A batch failed: `last_committed_row` stays where it was, so a rerun does not skip the failed rows.
    frozen: bool,
    /// The first row of the batch being written.
    batch_first: Option<i64>,
    commits_seen: u64,
    failed_ranges: Vec<FailedRange>,
    progress: Option<ProgressFn>,
    state: Arc<Mutex<LoaderState>>,
}

/// What a run reads back from its loader once the run moved it.
#[derive(Default)]
struct LoaderState {
    loaded: u64,
    last_committed_row: i64,
    committed_batches: u64,
    failed_batches: u64,
    failed_ranges: Vec<FailedRange>,
    committed: bool,
}

impl RowLoader {
    fn report(&self) {
        if let Some(progress) = &self.progress {
            progress(&Progress {
                extract_count: u64::try_from(self.extracted.load(Ordering::Relaxed)).unwrap_or(0),
                load_count: self.loaded,
                error_count: lock(&self.errors).rows,
                last_committed_row: self.last_committed_row,
            });
        }
    }

    /// A batch commit was seen: its rows count, and the last committed row moves unless a batch failed before.
    fn committed_to(&mut self, row: i64) {
        self.loaded += self.pending;
        self.pending = 0;
        self.batch_first = None;
        if !self.frozen {
            self.last_committed_row = row;
        }
    }
}

impl Loader for RowLoader {
    fn init(&mut self, timeout: Option<u32>) {
        if let Some(inner) = self.inner.as_mut() {
            inner.init(timeout);
        }
    }

    fn load(&mut self, entry: &mut SimpleEntry) -> Result<(), Error> {
        let row = row_number_of(&entry.values);
        entry.values.remove(ROW_NUMBER_KEY);
        self.seen += 1;
        if self.seen.is_multiple_of(PROGRESS_EVERY) {
            self.report();
        }
        if let Some(entity) = &self.check_target {
            check_row(&self.facade, entity, row, entry, &self.errors);
            return Ok(());
        }
        if self.dead || self.inner.is_none() {
            return Ok(());
        }
        self.batch_first.get_or_insert(row);
        let source = entry.values.clone();
        let Some(inner) = self.inner.as_mut() else {
            return Ok(());
        };
        let written = inner.load(entry);
        let commits = inner.commit_count();
        match written {
            Ok(()) => {
                self.pending += 1;
                self.last_ok_row = row;
                if commits > self.commits_seen {
                    self.commits_seen = commits;
                    self.committed_to(row);
                }
                Ok(())
            }
            Err(err) => {
                let message = if self.batched {
                    format!("{err} (its batch was rolled back)")
                } else {
                    err.to_string()
                };
                if self.batched {
                    self.failed_ranges.push(FailedRange {
                        from_row: self.batch_first.unwrap_or(row),
                        to_row: row,
                        message: err.to_string(),
                    });
                    self.pending = 0;
                    self.batch_first = None;
                    self.frozen = true;
                } else {
                    self.dead = true;
                }
                lock(&self.errors).add(RowError {
                    row_number: row,
                    field: String::new(),
                    message,
                    row: source,
                });
                Err(err)
            }
        }
    }

    fn complete(&mut self, etl: &SimpleEtl) {
        let shared = Arc::clone(&self.state);
        let mut state = lock(&shared);
        let ended = self.inner.as_mut().map(|inner| {
            inner.complete(etl);
            (inner.commit_count(), inner.failed_batch_count())
        });
        if let Some((commits, failed)) = ended {
            state.committed_batches = commits;
            state.failed_batches = failed;
            if self.batched {
                if commits > self.commits_seen {
                    self.commits_seen = commits;
                    let last = self.last_ok_row;
                    self.committed_to(last);
                } else if failed > self.failed_ranges.len() as u64 && self.pending > 0 {
                    // the run stopped at an error, and the batch it was in was rolled back
                    let first_error = lock(&self.errors).kept.first().map(|error| error.row_number);
                    self.failed_ranges.push(FailedRange {
                        from_row: self.batch_first.unwrap_or(self.last_ok_row),
                        to_row: first_error.unwrap_or(self.last_ok_row).max(self.last_ok_row),
                        message: "rolled back when the run stopped at an error".to_string(),
                    });
                    self.pending = 0;
                    self.frozen = true;
                }
                state.committed = true;
            } else {
                state.committed = !etl.has_error() && !self.dead;
                if state.committed {
                    let last = self.last_ok_row;
                    self.committed_to(last);
                }
            }
        }
        state.loaded = self.loaded;
        state.last_committed_row = self.last_committed_row;
        state.failed_ranges = std::mem::take(&mut self.failed_ranges);
    }
}

/// A check of one row of an entity target, as the user: its foreign keys with no related record, then its diff. Each
/// problem is an error of the row; a check goes on.
fn check_row(facade: &EntityFacade, entity: &str, row: i64, entry: &SimpleEntry, errors: &SharedErrors) {
    let fields = moqui_core::entity::field_map(entry.values.clone());
    match moqui_core::entity::missing_related(facade, entity, &fields) {
        Ok(missing) => {
            for key in missing {
                let keys = key
                    .keys
                    .iter()
                    .map(|(name, value)| format!("{name}={}", to_plain_string(value)))
                    .collect::<Vec<_>>()
                    .join(", ");
                lock(errors).add(RowError {
                    row_number: row,
                    field: key.keys.keys().next().map(ToString::to_string).unwrap_or_default(),
                    message: format!("foreign key {}: no {} with {keys}", key.relationship, key.related),
                    row: entry.values.clone(),
                });
            }
        }
        Err(err) => lock(errors).add(RowError {
            row_number: row,
            field: String::new(),
            message: format!("foreign keys: {err}"),
            row: entry.values.clone(),
        }),
    }
    if let Err(err) = moqui_core::entity::load_feed::check_entity_record(facade, entity, fields) {
        lock(errors).add(RowError {
            row_number: row,
            field: String::new(),
            message: err.to_string(),
            row: entry.values.clone(),
        });
    }
}

/// Find an existing record of `entity` by the mapping's match fields when `values` lacks its primary key, and fill
/// the key in from it (a natural key match).
fn match_existing(
    facade: &EntityFacade,
    entity: &str,
    match_fields: &[String],
    values: &mut BTreeMap<String, Value>,
) -> Result<(), Error> {
    if match_fields.is_empty() {
        return Ok(());
    }
    let pk: Vec<String> = facade.with_entity(entity, |definition, _, _| {
        Ok(definition
            .pk_field_names()
            .iter()
            .map(|name| (*name).to_string())
            .collect())
    })?;
    if pk
        .iter()
        .all(|field| values.get(field).is_some_and(|value| !value.is_null()))
    {
        return Ok(());
    }
    let mut find = facade.find(entity)?;
    for field in match_fields {
        let value = values.get(field).cloned().unwrap_or(Value::Null);
        find = find.eq(field, value);
    }
    let found = find.limit(2).list()?;
    match found.len() {
        0 => Ok(()),
        1 => {
            if let Some(existing) = found.iter().next() {
                for field in &pk {
                    if let Some(value) = existing.get(field) {
                        values.insert(field.clone(), value.clone());
                    }
                }
            }
            Ok(())
        }
        _ => Err(Error::new(format!(
            "more than one {entity} matches {}",
            match_fields.join(", ")
        ))),
    }
}

/// A service call per row, its errors taken out of the message facade into the row's error.
fn call_service(name: &str, values: &BTreeMap<String, Value>) -> Result<(), Error> {
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let called = moqui_core::service::call::sync_with(name, values, &moqui_core::service::call::CallOptions::default());
    let errors = match mark {
        Some(mark) => ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark)).unwrap_or_default(),
        None => Vec::new(),
    };
    match called {
        Err(err) => Err(Error::new(err.to_string())),
        Ok(_) if !errors.is_empty() => Err(Error::new(errors.join("; "))),
        Ok(_) => Ok(()),
    }
}

/// The target's loader: an entity-auto service call or the target service, per row, as the user.
fn target_loader(facade: &EntityFacade, mapping: &Mapping, options: &RunOptions) -> EtlLoader {
    let mut loader = facade.make_etl_loader();
    if mapping.dummy_fks {
        loader = loader.dummy_fks();
    }
    if options.join_caller {
        loader = loader.transactions(EtlTransactions::JoinCaller);
    } else {
        loader = loader.batch_size(mapping.batch_size);
    }
    let target = mapping.target_name.clone();
    match mapping.target_kind {
        TargetKind::Service => loader.writer(Box::new(move |_, entry| call_service(&target, &entry.values))),
        TargetKind::Entity => {
            let verb = match mapping.load_mode {
                LoadMode::Create => "create",
                LoadMode::Store => "store",
                LoadMode::Update => "update",
            };
            let match_fields = mapping.match_fields.clone();
            loader.writer(Box::new(move |facade, entry| {
                let mut values = entry.values.clone();
                match_existing(facade, &target, &match_fields, &mut values)?;
                values.retain(|_, value| !value.is_null() || verb != "create");
                call_service(&format!("{verb}#{target}"), &values)
            }))
        }
    }
}

/// Run `mapping` (its value maps resolved). A source that does not open, or a mapping that reaches what it may not
/// ([`Mapping::validate`], an expression without `GROOVY_SHELL_WEB`), is the outcome's failure.
pub fn run(facade: &EntityFacade, mapping: &Mapping, options: &RunOptions) -> RunOutcome {
    let mut outcome = RunOutcome {
        next_start_row: options.source.start_row as i64,
        ..RunOutcome::default()
    };
    if let Err(err) = mapping.validate_with(facade) {
        outcome.failure = Some(err.to_string());
        return outcome;
    }
    if mapping.has_expression() && !super::access::may_run_scripts() {
        outcome.failure = Some(super::access::SCRIPTS_REFUSED.to_string());
        return outcome;
    }
    let source_options = SourceOptions {
        check: options.check,
        ..options.source.clone()
    };
    let source = match source::open(facade, mapping, &source_options) {
        Ok(source) => source,
        Err(err) => {
            outcome.failure = Some(err.to_string());
            return outcome;
        }
    };
    outcome.resolved_source.clone_from(&source.resolved);
    let errors: SharedErrors = Arc::new(Mutex::new(ErrorLog::default()));
    let state = Arc::new(Mutex::new(LoaderState::default()));
    let check_target =
        (options.check && mapping.target_kind == TargetKind::Entity).then(|| mapping.target_name.clone());
    // the entries of a source without row numbers are numbered in the order read; the count is the rows read
    let counter = Arc::new(AtomicI64::new(0));
    let loader = RowLoader {
        inner: (!options.check).then(|| target_loader(facade, mapping, options)),
        check_target,
        facade: facade.clone(),
        errors: Arc::clone(&errors),
        batched: mapping.batch_size > 0 && !options.join_caller && !options.check,
        dead: false,
        extracted: Arc::clone(&counter),
        seen: 0,
        pending: 0,
        loaded: 0,
        last_ok_row: 0,
        last_committed_row: 0,
        frozen: false,
        batch_first: None,
        commits_seen: 0,
        failed_ranges: Vec::new(),
        progress: options.progress.clone(),
        state: Arc::clone(&state),
    };
    let transformer = MappingTransformer::new(facade, mapping, Arc::clone(&source.headers), Arc::clone(&errors));
    let mut etl = SimpleEtl::from_boxes(source.extractor, Box::new(loader));
    etl.add_transformer(move |transform: &mut EntryTransform| {
        let number = counter.fetch_add(1, Ordering::Relaxed) + 1;
        transform
            .entry_mut()
            .values
            .entry(ROW_NUMBER_KEY.to_string())
            .or_insert(Value::I64(number));
        Ok(())
    });
    etl.add_transformer(transformer);
    // a check never stops at an error
    if !options.check && (options.join_caller || (mapping.batch_size > 0 && mapping.stop_on_error)) {
        etl.stop_on_error();
    }
    etl.set_timeout(Some(mapping.timeout_seconds.unwrap_or(3600)));
    if options.check {
        match moqui_core::entity::load_feed::check_info_limited(MAX_DIFF_ROWS, || {
            etl.process();
            Ok(())
        }) {
            Ok(((), result)) => {
                outcome.fields_checked = result.fields_checked;
                outcome.diff = result.diff_info;
            }
            Err(err) => outcome.failure = Some(err.to_string()),
        }
    } else {
        etl.process();
    }
    if let Some(err) = etl.extract_error() {
        outcome.failure.get_or_insert_with(|| err.to_string());
    }
    let mut state = lock(&state);
    let log = lock(&errors);
    outcome.extract_count = etl.extract_count();
    outcome.skip_count = etl.skip_count();
    outcome.load_count = if options.check { 0 } else { state.loaded };
    outcome.error_count = log.rows;
    outcome.field_error_count = log.count;
    outcome.last_committed_row = state.last_committed_row;
    if state.last_committed_row > 0 {
        outcome.next_start_row = state.last_committed_row + 1;
    }
    outcome.committed_batches = state.committed_batches;
    outcome.failed_batches = state.failed_batches;
    outcome.failed_ranges = std::mem::take(&mut state.failed_ranges);
    outcome.committed = state.committed;
    outcome.errors = log.kept.clone();
    if log.count > 0 {
        outcome.error_csv = Some(log.csv_text());
    }
    if outcome.failure.is_none() && !options.check && (etl.stopped() || (options.join_caller && log.count > 0)) {
        outcome.failure = Some(match log.kept.first() {
            Some(error) if options.join_caller => error.message.clone(),
            Some(error) => format!("stopped at row {}: {}", error.row_number, error.message),
            None => "stopped at an error".to_string(),
        });
    }
    outcome
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
