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

    // A second run updates the row, with one upsert.
    let again = root.make_file("again.xml").unwrap();
    again
        .write_text(
            r#"<data><moqui.basic.Enumeration enumId="EtlOne" enumTypeId="EtlType" description="Updated"/></data>"#,
        )
        .unwrap();
    let mut etl = SimpleEtl::new(FlatXmlExtractor::new(again), facade.make_etl_loader());
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

/// Extracts `count` EnumerationType rows, `EtlBatch1` … in order.
struct Numbered(usize);

impl moqui_lib::etl::Extractor for Numbered {
    fn extract(&mut self, etl: &mut SimpleEtl) -> Result<(), moqui_lib::error::Error> {
        for number in 1..=self.0 {
            let mut values = std::collections::BTreeMap::new();
            values.insert("enumTypeId".to_string(), Value::string(format!("EtlBatch{number}")));
            values.insert("description".to_string(), Value::string("batched"));
            let _ = etl.process_entry(moqui_lib::etl::SimpleEntry::new("moqui.basic.EnumerationType", values));
        }
        Ok(())
    }
}

/// Batches of three: the entry that fails rolls its batch back, the next entry starts a new batch, and the last
/// batch commits; the batches before stay.
#[test]
fn a_failed_entry_rolls_back_its_batch_and_the_rest_commit() {
    let facade = testing::open();
    let writes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&writes);
    let loader = facade
        .make_etl_loader()
        .batch_size(3)
        .writer(Box::new(move |facade, entry| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if entry.values.get("enumTypeId") == Some(&Value::string("EtlBatch8")) {
                return Err(moqui_lib::error::Error::new("bad row"));
            }
            let mut value = facade.make_value("moqui.basic.EnumerationType")?;
            for (name, field) in &entry.values {
                value.set(name, field.clone());
            }
            value.create(facade)
        }));
    let mut etl = SimpleEtl::new(Numbered(11), loader);
    etl.process();
    assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 11);
    let present = |number: usize| {
        text(
            &facade,
            "moqui.basic.EnumerationType",
            ("enumTypeId", &format!("EtlBatch{number}")),
            "description",
        )
        .is_some()
    };
    // 1-3 and 4-6 committed; 7 was in the batch that 8 failed; 9-11 the last batch
    for number in [1, 2, 3, 4, 5, 6, 9, 10, 11] {
        assert!(present(number), "row {number}");
    }
    for number in [7, 8] {
        assert!(!present(number), "row {number}");
    }
}

/// Extracts `count` EnumerationType rows, `<prefix>1` … in order, stopping when the run stops.
struct Prefixed(&'static str, usize);

impl moqui_lib::etl::Extractor for Prefixed {
    fn extract(&mut self, etl: &mut SimpleEtl) -> Result<(), moqui_lib::error::Error> {
        for number in 1..=self.1 {
            let mut values = std::collections::BTreeMap::new();
            values.insert("enumTypeId".to_string(), Value::string(format!("{}{number}", self.0)));
            values.insert("description".to_string(), Value::string("batched"));
            if etl
                .process_entry(moqui_lib::etl::SimpleEntry::new("moqui.basic.EnumerationType", values))
                .is_err()
            {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// With stop-on-error, a transform error ends the run and the batch it was in is rolled back, not committed.
#[test]
fn a_transform_error_that_stops_the_run_rolls_back_its_batch() {
    let facade = testing::open();
    let loader = facade.make_etl_loader().batch_size(3);
    let mut etl = SimpleEtl::new(Prefixed("EtlStop", 8), loader);
    etl.add_transformer(|transform: &mut moqui_lib::etl::EntryTransform| {
        if transform.entry().values.get("enumTypeId") == Some(&Value::string("EtlStop5")) {
            return Err(moqui_lib::error::Error::new("bad row"));
        }
        Ok(())
    });
    etl.stop_on_error();
    etl.process();
    assert!(etl.stopped());
    let present = |number: usize| {
        text(
            &facade,
            "moqui.basic.EnumerationType",
            ("enumTypeId", &format!("EtlStop{number}")),
            "description",
        )
        .is_some()
    };
    for number in [1, 2, 3] {
        assert!(present(number), "row {number}");
    }
    for number in [4, 5, 6, 7, 8] {
        assert!(!present(number), "row {number}");
    }
}

/// Joining the caller's transaction: nothing is begun.
#[test]
fn a_joining_loader_begins_no_transaction() {
    use moqui_lib::etl::Loader;

    let facade = testing::open();
    let tx = facade.transactions_for("moqui.basic.Enumeration").unwrap();
    let mut loader = facade
        .make_etl_loader()
        .transactions(crate::entity::EtlTransactions::JoinCaller);
    loader.init(None);
    assert!(!tx.in_transaction(), "a joining loader begins no transaction");
}
