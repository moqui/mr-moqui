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
    use_try_insert: bool,
    dummy_fks: bool,
}

impl EntityFacade {
    pub fn make_etl_loader(&self) -> EtlLoader {
        EtlLoader {
            facade: self.clone(),
            began: Vec::new(),
            use_try_insert: false,
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
    /// Insert, or update on a primary-key conflict, instead of the entity-auto store.
    #[must_use]
    pub fn use_try_insert(mut self) -> Self {
        self.use_try_insert = true;
        self
    }

    /// Create a dummy record for a foreign key that has no related record.
    #[must_use]
    pub fn dummy_fks(mut self) -> Self {
        self.dummy_fks = true;
        self
    }

    fn write(&self, entity_name: &str, entry: &mut SimpleEntry) -> Result<(), Error> {
        let facade = &self.facade;
        // The same pattern as the data loader's record write.
        if self.dummy_fks || self.use_try_insert {
            let mut value = facade.make_value(entity_name)?;
            for (name, field) in &entry.values {
                value.set(name, field.clone());
            }
            // Java's ETL loader tries the insert even where the database sets `never-try-insert` (Postgres), where the
            // failed insert aborts the transaction and the update after it fails; here such a database stores with
            // `createOrUpdate`, as the data loader's `handleValue` does.
            let try_insert = self.use_try_insert
                && facade.with_entity(entity_name, |_, dialect, _| Ok(!dialect.never_try_insert))?;
            if try_insert {
                facade.upsert_record(value.entity_name(), value.fields())
            } else {
                value.create_or_update(facade)
            }
        } else {
            let results = crate::service::call::store_entity(facade, entity_name, &entry.values)?;
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
        let entry = SimpleEntry::new(value.entity_name(), value.fields().clone());
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
mod tests {
    use moqui_lib::etl::{EntryTransform, FlatXmlExtractor, SimpleEtl};
    use moqui_lib::object::Value;
    use moqui_lib::resource::{MemoryResource, Resource};

    use crate::testing;

    fn text(facade: &crate::entity::EntityFacade, entity: &str, key: (&str, &str), field: &str) -> Option<String> {
        let value = facade
            .find(entity)
            .unwrap()
            .eq(key.0, Value::string(key.1))
            .one()
            .unwrap()?;
        value.get(field).and_then(Value::as_str).map(str::to_string)
    }

    #[test]
    fn a_loader_dropped_mid_run_rolls_back_the_frames_it_began() {
        use moqui_lib::etl::Loader;

        let facade = testing::open();
        let tx = facade.transactions_for("moqui.basic.Enumeration").unwrap();
        let mut loader = facade.make_etl_loader();
        loader.init(None);
        assert!(tx.in_transaction());
        // a panic in a transformer unwinds past `complete`
        drop(loader);
        assert!(!tx.in_transaction());
    }

    #[test]
    fn flat_xml_extract_loads_entity_rows() {
        let facade = testing::open();
        let root = MemoryResource::new_root("mem://etl");
        let file = root.make_file("enums.xml").unwrap();
        file.write_text(
            r#"<data>
                <moqui.basic.EnumerationType enumTypeId="EtlType" description="ETL type"/>
                <moqui.basic.Enumeration enumId="EtlOne" enumTypeId="EtlType">
                    <description>First value</description>
                </moqui.basic.Enumeration>
                <Enumeration enumId="EtlTwo" enumTypeId="EtlType" description="raw"/>
                <not.an.Entity someId="X"/>
            </data>"#,
        )
        .unwrap();
        let mut etl = SimpleEtl::new(FlatXmlExtractor::new(file), facade.make_etl_loader());
        etl.add_type_transformer("Enumeration", |transform: &mut EntryTransform| {
            transform
                .entry_mut()
                .values
                .insert("description".to_string(), Value::string("transformed"));
            Ok(())
        });
        etl.process();
        assert!(
            !etl.has_error(),
            "{:?}",
            etl.single_error_cause().map(ToString::to_string)
        );
        assert_eq!(etl.extract_count(), 4);
        // The undefined entity is skipped by the loader, which still counts it as loaded.
        assert_eq!(etl.load_count(), 4);
        assert_eq!(
            text(
                &facade,
                "moqui.basic.EnumerationType",
                ("enumTypeId", "EtlType"),
                "description"
            )
            .as_deref(),
            Some("ETL type")
        );
        assert_eq!(
            text(&facade, "moqui.basic.Enumeration", ("enumId", "EtlOne"), "description").as_deref(),
            Some("First value")
        );
        assert_eq!(
            text(&facade, "moqui.basic.Enumeration", ("enumId", "EtlTwo"), "description").as_deref(),
            Some("transformed")
        );

        // A second run updates through the try-insert path.
        let again = root.make_file("again.xml").unwrap();
        again
            .write_text(
                r#"<data><moqui.basic.Enumeration enumId="EtlOne" enumTypeId="EtlType" description="Updated"/></data>"#,
            )
            .unwrap();
        let mut etl = SimpleEtl::new(FlatXmlExtractor::new(again), facade.make_etl_loader().use_try_insert());
        etl.process();
        assert!(
            !etl.has_error(),
            "{:?}",
            etl.single_error_cause().map(ToString::to_string)
        );
        assert_eq!(
            text(&facade, "moqui.basic.Enumeration", ("enumId", "EtlOne"), "description").as_deref(),
            Some("Updated")
        );
    }

    #[test]
    fn error_in_the_run_rolls_back_what_it_loaded() {
        let facade = testing::open();
        let root = MemoryResource::new_root("mem://etl");
        let file = root.make_file("bad.xml").unwrap();
        file.write_text(
            r#"<data>
                <moqui.basic.EnumerationType enumTypeId="EtlRolled" description="rolled back"/>
                <moqui.basic.EnumerationType enumTypeId="EtlBad" description="fails"/>
            </data>"#,
        )
        .unwrap();
        let mut etl = SimpleEtl::new(FlatXmlExtractor::new(file), facade.make_etl_loader());
        etl.add_transformer(|transform: &mut EntryTransform| {
            if transform.entry().values.get("enumTypeId") == Some(&Value::string("EtlBad")) {
                return Err(moqui_lib::error::Error::new("bad record"));
            }
            Ok(())
        });
        etl.process();
        assert_eq!(etl.transform_errors().len(), 1);
        assert_eq!(etl.load_count(), 1);
        assert_eq!(
            text(
                &facade,
                "moqui.basic.EnumerationType",
                ("enumTypeId", "EtlRolled"),
                "description"
            ),
            None
        );
    }

    #[test]
    fn error_in_the_run_rolls_back_every_entity_group() {
        let facade = testing::open_two_groups();
        let root = MemoryResource::new_root("mem://etl");
        let file = root.make_file("groups.xml").unwrap();
        file.write_text(
            r#"<data>
                <moqui.basic.EnumerationType enumTypeId="EtlRolled" description="transactional group"/>
                <moqui.test.TestNoSqlEntity testId="EtlLogged" testMedium="logging group"/>
                <moqui.basic.EnumerationType enumTypeId="EtlBad" description="fails"/>
            </data>"#,
        )
        .unwrap();
        let mut etl = SimpleEtl::new(FlatXmlExtractor::new(file), facade.make_etl_loader());
        etl.add_transformer(|transform: &mut EntryTransform| {
            if transform.entry().values.get("enumTypeId") == Some(&Value::string("EtlBad")) {
                return Err(moqui_lib::error::Error::new("bad record"));
            }
            Ok(())
        });
        etl.process();
        assert_eq!(etl.load_count(), 2);
        let logging = facade.transactions_for("moqui.test.TestNoSqlEntity").unwrap();
        let transactional = facade.transactions_for("moqui.basic.EnumerationType").unwrap();
        assert!(!logging.shares_stack(&transactional));
        assert!(!logging.in_transaction() && !transactional.in_transaction());
        assert_eq!(
            text(
                &facade,
                "moqui.basic.EnumerationType",
                ("enumTypeId", "EtlRolled"),
                "description"
            ),
            None
        );
        assert_eq!(
            text(
                &facade,
                "moqui.test.TestNoSqlEntity",
                ("testId", "EtlLogged"),
                "testMedium"
            ),
            None
        );
    }
}
