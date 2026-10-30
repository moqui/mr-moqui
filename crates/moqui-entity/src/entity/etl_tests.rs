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
