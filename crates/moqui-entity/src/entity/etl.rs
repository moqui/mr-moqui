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

//! The entity side of `moqui_lib::etl`: `EntityFacade.makeEtlLoader` and `EntityFind.extract`.
//!
//! An entity value is not itself an ETL entry here. The find extracts each value as an entry
//! whose type is the entity name and whose values are the fields.

use moqui_lib::error::Error;
use moqui_lib::etl::{Extractor, Loader, SimpleEntry, SimpleEtl};

use super::facade::EntityFacade;
use super::find::EntityFind;
use crate::transaction::{TransactionAttribute, TransactionManager};

/// Loads each entry into the entity named by its type. One transaction for the run, begun in
/// `init` when none is in place, committed in `complete`, or rolled back when the run had an error.
///
/// Java's transaction spans every datasource. Here each entity group with its own transaction
/// manager has its own frame; the facade links those managers, so the first begin makes a frame on
/// each and the others see a transaction in place. The run still begins on each manager not in a
/// transaction, for managers that are not linked. An error rolls back all of them. Commit is one
/// manager after another, with no two-phase commit, as everywhere in this port.
///
/// With [`Self::batch_size`] the transactions it began are committed after every N entries and begun again; an entry
/// that fails rolls back its batch only, and the next entry starts a new one (plans/archive/Tools-Database.md §10.3,
/// decision 7). With [`EtlTransactions::JoinCaller`] it begins nothing: each write joins the caller's transaction, as a
/// service call does, and an error marks that transaction rollback-only. [`Self::writer`] replaces the entity write,
/// for a loader that calls a service per row.
pub struct EtlLoader {
    facade: EntityFacade,
    began: Vec<TransactionManager>,
    dummy_fks: bool,
    transactions: EtlTransactions,
    batch_size: usize,
    in_batch: usize,
    timeout: Option<std::time::Duration>,
    commits: u64,
    failed_batches: u64,
    writer: Option<EtlWriter>,
}

/// A loader's own write of one entry, in place of the entity write.
pub type EtlWriter = Box<dyn FnMut(&EntityFacade, &mut SimpleEntry) -> Result<(), Error>>;

/// Whose transaction an [`EtlLoader`] writes in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EtlTransactions {
    /// One of its own on each group not in a transaction, joining the ones in progress (the default).
    #[default]
    Run,
    /// The caller's, begun by no one here: `load#SpreadsheetEntity` in the transaction of the service that calls it.
    JoinCaller,
}

impl EntityFacade {
    pub fn make_etl_loader(&self) -> EtlLoader {
        EtlLoader {
            facade: self.clone(),
            began: Vec::new(),
            dummy_fks: false,
            transactions: EtlTransactions::Run,
            batch_size: 0,
            in_batch: 0,
            timeout: None,
            commits: 0,
            failed_batches: 0,
            writer: None,
        }
    }

    /// The distinct transaction managers of the entity groups, the default group's first.
    fn group_transactions(&self) -> Vec<TransactionManager> {
        let mut managers: Vec<TransactionManager> = Vec::new();
        let default = self.inner.groups.get(&self.inner.default_group);
        let mut names: Vec<&String> = self.inner.groups.keys().collect();
        names.sort();
        for group in default
            .into_iter()
            .chain(names.into_iter().filter_map(|name| self.inner.groups.get(name)))
        {
            if !managers.iter().any(|known| known.shares_stack(&group.transactions)) {
                managers.push(group.transactions.clone());
            }
        }
        managers
    }
}

impl EtlLoader {
    /// Create a dummy record for a foreign key that has no related record.
    #[must_use]
    pub fn dummy_fks(mut self) -> Self {
        self.dummy_fks = true;
        self
    }

    /// Commit after every `size` entries loaded and begin again; 0 is one transaction for the run. Only the
    /// transactions this loader began are committed: in a caller's transaction there are no batches.
    #[must_use]
    pub fn batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Whose transaction the writes are in.
    #[must_use]
    pub fn transactions(mut self, transactions: EtlTransactions) -> Self {
        self.transactions = transactions;
        self
    }

    /// Write each entry with `writer` instead of into the entity its type names: a service call per row.
    #[must_use]
    pub fn writer(mut self, writer: EtlWriter) -> Self {
        self.writer = Some(writer);
        self
    }

    /// How many batches have been committed.
    #[must_use]
    pub fn commit_count(&self) -> u64 {
        self.commits
    }

    /// How many batches failed and were rolled back.
    #[must_use]
    pub fn failed_batch_count(&self) -> u64 {
        self.failed_batches
    }

    fn batching(&self) -> bool {
        self.batch_size > 0 && !self.began.is_empty()
    }

    /// End the batch on each transaction this loader began (commit, or roll back when `commit` is false) and begin
    /// the next. The first failure is returned; the rest still end and begin.
    fn end_batch(&mut self, commit: bool) -> Result<(), Error> {
        self.in_batch = 0;
        let mut first = None;
        for tx in &self.began {
            let ended = if commit { tx.commit() } else { tx.rollback() };
            if let Err(err) = ended {
                tracing::warn!(error = %err, "ETL batch did not end its transaction");
                first.get_or_insert(err);
            }
            if let Err(err) = tx.begin(self.timeout) {
                tracing::warn!(error = %err, "ETL batch could not begin the next transaction");
                first.get_or_insert(err);
            }
        }
        match first {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn write(&self, entity_name: &str, entry: &mut SimpleEntry) -> Result<(), Error> {
        let facade = &self.facade;
        // The same pattern as the data loader's record write: one upsert where it fits (Java's `useTryInsert`, which
        // this replaces), else create-or-update with dummy foreign keys or the entity-auto store.
        let upsert = facade.upsert_fits(entity_name, &entry.values)?;
        if upsert || self.dummy_fks {
            let mut value = facade.make_value(entity_name)?;
            for (name, field) in &entry.values {
                value.set(name, field.clone());
            }
            if upsert {
                facade.upsert_record(entity_name, value.into_fields())
            } else {
                value.create_or_update(facade)
            }
        } else {
            let results = (crate::hooks::get().store_entity)(facade, entity_name, &entry.values)?;
            entry.values.extend(results);
            Ok(())
        }
    }
}

/// A loader dropped with frames it began and not completed, as when a transformer panics mid-run, rolls them back
/// and does not leave them open on this thread. `complete` empties `began`, so a finished run does nothing here.
impl Drop for EtlLoader {
    fn drop(&mut self) {
        for tx in std::mem::take(&mut self.began) {
            if tx.in_transaction() {
                if let Err(err) = tx.rollback() {
                    tracing::warn!(error = %err, "ETL loader dropped mid-run could not roll back");
                }
            }
        }
    }
}

impl Loader for EtlLoader {
    fn init(&mut self, timeout: Option<u32>) {
        let timeout = timeout.map(|seconds| std::time::Duration::from_secs(u64::from(seconds)));
        self.timeout = timeout;
        if self.transactions == EtlTransactions::JoinCaller {
            return;
        }
        for tx in self.facade.group_transactions() {
            if tx.in_transaction() {
                continue;
            }
            match tx.begin(timeout) {
                Ok(true) => self.began.push(tx),
                Ok(false) => {}
                Err(err) => tracing::warn!(error = %err, "ETL load could not begin a transaction"),
            }
        }
    }

    fn load(&mut self, entry: &mut SimpleEntry) -> Result<(), Error> {
        let written = {
            // Dummy foreign keys are on for this thread and this write only, as for the data loader.
            let _scope = super::fk::FkScope::enter(self.dummy_fks, false);
            match self.writer.as_mut() {
                Some(writer) => writer(&self.facade, entry),
                None => {
                    let entity_name = entry.etl_type.clone();
                    if self.facade.with_entity(&entity_name, |_, _, _| Ok(())).is_err() {
                        tracing::info!(entity = %entity_name, "Tried to load ETL entry with invalid entity name");
                        return Ok(());
                    }
                    self.write(&entity_name, entry)
                }
            }
        };
        if !self.batching() {
            return written;
        }
        match written {
            Ok(()) => {
                self.in_batch += 1;
                if self.in_batch >= self.batch_size {
                    self.end_batch(true)?;
                    self.commits += 1;
                }
                Ok(())
            }
            Err(err) => {
                // the batch is rolled back; the next entry starts a new one
                self.failed_batches += 1;
                if let Err(ended) = self.end_batch(false) {
                    tracing::warn!(error = %ended, "ETL batch rollback after a failed entry did not complete");
                }
                Err(err)
            }
        }
    }

    fn complete(&mut self, etl: &SimpleEtl) {
        let batching = self.batching();
        let pending = self.in_batch;
        let began = std::mem::take(&mut self.began);
        // In batches each failure rolled its own batch back, and the last batch commits, unless an error stopped the
        // run: then the batch it was in is rolled back too, its entries not committed past the error.
        let stopped_batch = batching && pending > 0 && etl.stopped();
        let mut failed = !batching && etl.has_error();
        if stopped_batch {
            self.failed_batches += 1;
        } else if batching && pending > 0 {
            self.commits += 1;
        }
        for tx in self.facade.group_transactions() {
            let owned = began.iter().any(|known| known.shares_stack(&tx));
            let outcome = if stopped_batch && owned {
                tx.rollback()
            } else if failed {
                if owned {
                    tx.rollback()
                } else if tx.in_transaction() {
                    let cause = etl
                        .single_error_cause()
                        .map(|err| Error::new(err.to_string()))
                        .unwrap_or_else(|| Error::new("ETL error"));
                    tx.set_rollback_only_error("Error in ETL load", cause)
                } else {
                    Ok(())
                }
            } else if owned {
                tx.commit()
            } else {
                Ok(())
            };
            if let Err(err) = outcome {
                tracing::warn!(error = %err, "ETL load did not complete its transaction");
                // The groups left are rolled back: what is already committed cannot be.
                failed = true;
            }
        }
    }
}

impl EntityFind {
    /// Process each value this find returns, as an entry named for the entity.
    ///
    /// The cursor needs a transaction. One is begun around the extract when none is in place.
    pub fn extract(self, etl: &mut SimpleEtl) -> Result<(), Error> {
        let tx = self.facade.transactions_for(self.entity_name())?;
        if tx.in_transaction() {
            return extract_values(self, etl);
        }
        tx.run(TransactionAttribute::UseOrBegin, None, || extract_values(self, etl))
    }
}

fn extract_values(find: EntityFind, etl: &mut SimpleEtl) -> Result<(), Error> {
    let mut values = find.iterator()?;
    while let Some(value) = values.next_value()? {
        let entry = SimpleEntry::new(value.entity_name(), crate::entity::string_map(value.fields()));
        if let Err(stop) = etl.process_entry(entry) {
            tracing::warn!(cause = %stop.cause, "EntityFind extract stopped");
            break;
        }
    }
    values.close()
}

/// `EntityFind` is an extractor, as in Java. Each run extracts with a copy of the find.
impl Extractor for EntityFind {
    fn extract(&mut self, etl: &mut SimpleEtl) -> Result<(), Error> {
        EntityFind::extract(self.clone(), etl)
    }
}

#[cfg(test)]
#[path = "etl_tests.rs"]
mod tests;
