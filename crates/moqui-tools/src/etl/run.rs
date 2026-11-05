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

//! One ETL run (plans/Tools-Database.md §10.3, decision 7): the source's extractor, the [`MappingTransformer`], and a
//! loader of the target, an entity (`EtlLoader`) or a service (`ServiceLoader`: one call per row), each row's number
//! followed through the load.
//!
//! - **No batch size:** the run is one transaction. Any error rolls it all back; the extract and transform errors are
//!   all collected; the first database error ends the loading, since Postgres aborts the transaction then.
//! - **A batch size:** a batch that fails is rolled back and recorded; `stopOnError` decides whether the run goes on.
//!   `lastCommittedRow` is the last row of the last batch committed: where a rerun starts.
//! - **Check:** the extract and transform with no write: the per-row errors, the foreign keys with no related record
//!   (the rule `fk::enforce` uses), and for an entity target the diff against the database (`check_info_with`).

use std::collections::BTreeMap;
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
    pub extract_count: u64,
    pub load_count: u64,
    pub error_count: u64,
    pub last_committed_row: i64,
}

/// How a run ended.
#[derive(Debug, Default)]
pub struct RunOutcome {
    /// The source as read: a location (with its sheet), an entity, a SQL source.
    pub resolved_source: String,
    pub extract_count: u64,
    pub skip_count: u64,
    pub load_count: u64,
    pub error_count: u64,
    /// The last row of the last batch committed (with no batches, the last row once the run committed); 0 for none.
    pub last_committed_row: i64,
    pub committed_batches: u64,
    pub failed_batches: u64,
    /// The first [`super::transform::MAX_ERROR_ROWS`] errors, each with its row and field.
    pub errors: Vec<RowError>,
    /// Every failed row, for the error file.
    pub error_csv: Option<String>,
    /// The source could not be read, or the run stopped: why.
    pub failure: Option<String>,
    /// A check of an entity target: `entity`, `pk`, `notFound`, … rows (`CheckResult::diff_info`).
    pub diff: Vec<IndexMap<String, Value>>,
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
    loaded: u64,
    seen: u64,
    last_ok_row: i64,
    last_committed_row: i64,
    commits_seen: u64,
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
    committed: bool,
}

impl RowLoader {
    fn report(&self) {
        if let Some(progress) = &self.progress {
            progress(&Progress {
                extract_count: self.seen,
                load_count: self.loaded,
                error_count: lock(&self.errors).count,
                last_committed_row: self.last_committed_row,
            });
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
            return check_row(&self.facade, entity, row, entry, &self.errors);
        }
        let Some(inner) = self.inner.as_mut() else {
            return Ok(());
        };
        if self.dead {
            return Ok(());
        }
        let source = entry.values.clone();
        match inner.load(entry) {
            Ok(()) => {
                self.loaded += 1;
                self.last_ok_row = row;
                if inner.commit_count() > self.commits_seen {
                    self.commits_seen = inner.commit_count();
                    self.last_committed_row = row;
                }
                Ok(())
            }
            Err(err) => {
                let message = if self.batched {
                    format!("{err} (its batch was rolled back)")
                } else {
                    err.to_string()
                };
                lock(&self.errors).add(RowError {
                    row_number: row,
                    field: String::new(),
                    message,
                    row: source,
                });
                if !self.batched {
                    self.dead = true;
                }
                Err(err)
            }
        }
    }

    fn complete(&mut self, etl: &SimpleEtl) {
        let mut state = lock(&self.state);
        state.loaded = self.loaded;
        if let Some(inner) = self.inner.as_mut() {
            inner.complete(etl);
            if self.batched {
                if inner.commit_count() > self.commits_seen {
                    self.last_committed_row = self.last_ok_row;
                }
                state.committed = true;
            } else {
                state.committed = !etl.has_error() && !self.dead;
                if state.committed {
                    self.last_committed_row = self.last_ok_row;
                }
            }
            state.committed_batches = inner.commit_count();
            state.failed_batches = inner.failed_batch_count();
        }
        state.last_committed_row = self.last_committed_row;
    }
}

/// A check of one row of an entity target: its foreign keys with no related record, then its diff.
fn check_row(
    facade: &EntityFacade,
    entity: &str,
    row: i64,
    entry: &SimpleEntry,
    errors: &SharedErrors,
) -> Result<(), Error> {
    let fields = moqui_core::entity::field_map(entry.values.clone());
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
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
    moqui_core::entity::load_feed::check_entity_record(facade, entity, fields).map(|_| ())
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

/// The target's loader: the entity write, or a service call per row.
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
            let plain_store = mapping.load_mode == LoadMode::Store && mapping.match_fields.is_empty();
            if plain_store && !options.join_caller {
                // the data loader's write: one upsert where it fits, else the entity-auto store
                return loader;
            }
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

/// Run `mapping` (its value maps resolved). A source that does not open is the outcome's failure.
pub fn run(facade: &EntityFacade, mapping: &Mapping, options: &RunOptions) -> RunOutcome {
    let mut outcome = RunOutcome::default();
    let source = match source::open(facade, mapping, &options.source) {
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
    let loader = RowLoader {
        inner: (!options.check).then(|| target_loader(facade, mapping, options)),
        check_target,
        facade: facade.clone(),
        errors: Arc::clone(&errors),
        batched: mapping.batch_size > 0 && !options.join_caller && !options.check,
        dead: false,
        loaded: 0,
        seen: 0,
        last_ok_row: 0,
        last_committed_row: 0,
        commits_seen: 0,
        progress: options.progress.clone(),
        state: Arc::clone(&state),
    };
    let transformer = MappingTransformer::new(facade, mapping, Arc::clone(&source.headers), Arc::clone(&errors));
    let mut etl = SimpleEtl::from_boxes(source.extractor, Box::new(loader));
    // the entries of a source without row numbers are numbered in the order read
    let counter = Arc::new(std::sync::atomic::AtomicI64::new(0));
    etl.add_transformer(move |transform: &mut EntryTransform| {
        let number = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        transform
            .entry_mut()
            .values
            .entry(ROW_NUMBER_KEY.to_string())
            .or_insert(Value::I64(number));
        Ok(())
    });
    etl.add_transformer(transformer);
    if options.join_caller || (mapping.batch_size > 0 && mapping.stop_on_error) {
        etl.stop_on_error();
    }
    etl.set_timeout(Some(mapping.timeout_seconds.unwrap_or(3600)));
    if options.check {
        match moqui_core::entity::load_feed::check_info_with(|| {
            etl.process();
            Ok(())
        }) {
            Ok(((), result)) => {
                outcome.fields_checked = result.fields_checked;
                outcome.diff = result.diff_info.into_iter().take(MAX_DIFF_ROWS).collect();
            }
            Err(err) => outcome.failure = Some(err.to_string()),
        }
    } else {
        etl.process();
    }
    if let Some(err) = etl.extract_error() {
        outcome.failure.get_or_insert_with(|| err.to_string());
    }
    let state = lock(&state);
    let log = lock(&errors);
    outcome.extract_count = etl.extract_count();
    outcome.skip_count = etl.skip_count();
    outcome.load_count = if options.check { 0 } else { state.loaded };
    outcome.error_count = log.count;
    outcome.last_committed_row = state.last_committed_row;
    outcome.committed_batches = state.committed_batches;
    outcome.failed_batches = state.failed_batches;
    outcome.committed = state.committed;
    outcome.errors = log.kept.clone();
    if log.count > 0 {
        outcome.error_csv = Some(log.csv_text());
    }
    if outcome.failure.is_none() && options.join_caller && log.count > 0 {
        outcome.failure = log.kept.first().map(|error| error.message.clone());
    }
    if outcome.failure.is_none() && !options.check && mapping.stop_on_error && state.failed_batches > 0 {
        let failed = log.kept.iter().find(|error| error.field.is_empty());
        outcome.failure = Some(match failed {
            Some(error) => format!(
                "stopped at row {}, whose batch was rolled back: {}",
                error.row_number, error.message
            ),
            None => "stopped at a batch that was rolled back".to_string(),
        });
    }
    outcome
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
