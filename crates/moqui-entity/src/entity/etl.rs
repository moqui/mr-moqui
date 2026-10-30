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
/// manager has its own frame, so the run begins one on each, and an error rolls back all of them.
/// Groups that share a manager share that frame. Commit is one manager after another, with no
/// two-phase commit, as everywhere in this port.
pub struct EtlLoader {
    facade: EntityFacade,
    began: Vec<TransactionManager>,
    dummy_fks: bool,
}

impl EntityFacade {
    pub fn make_etl_loader(&self) -> EtlLoader {
        EtlLoader {
            facade: self.clone(),
            began: Vec::new(),
            dummy_fks: false,
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
        let entity_name = entry.etl_type.clone();
        if self.facade.with_entity(&entity_name, |_, _, _| Ok(())).is_err() {
            tracing::info!(entity = %entity_name, "Tried to load ETL entry with invalid entity name");
            return Ok(());
        }
        // Dummy foreign keys are on for this thread and this write only, as for the data loader.
        let _scope = super::fk::FkScope::enter(self.dummy_fks, false);
        self.write(&entity_name, entry)
    }

    fn complete(&mut self, etl: &SimpleEtl) {
        let began = std::mem::take(&mut self.began);
        let mut failed = etl.has_error();
        for tx in self.facade.group_transactions() {
            let owned = began.iter().any(|known| known.shares_stack(&tx));
            let outcome = if failed {
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
