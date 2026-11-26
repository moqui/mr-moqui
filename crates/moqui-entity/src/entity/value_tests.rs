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

//! [`EntityFacade::create_values`]: rows inserted several to a statement, each prepared as a create prepares it.

use std::sync::{Arc, Mutex};

use moqui_lib::object::Value;

use crate::entity::crypto::CryptConfig;
use crate::entity::exec::query_tx;
use crate::entity::{EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;

const TEST: &str = "moqui.test.TestEntity";

fn row(facade: &EntityFacade, index: usize) -> EntityValue {
    let mut row = facade.make_value(TEST).unwrap();
    row.set("testId", Value::string(format!("C{index:03}")));
    // a number given as text is converted as a create converts it; every other row leaves the medium out
    row.set("testNumberInteger", Value::string(index.to_string()));
    if index.is_multiple_of(2) {
        row.set("testMedium", Value::string("even"));
    }
    row
}

fn inserts(tape: &Arc<Mutex<Vec<String>>>, from: usize) -> Vec<String> {
    tape.lock().unwrap()[from..]
        .iter()
        .filter(|sql| sql.starts_with("INSERT") && sql.contains("\"test_entity\""))
        .cloned()
        .collect()
}

/// 250 rows go in five statements of 100, 100, 30, 10 and 10 rows, each naming every column a row sets; the rows read
/// back with their fields converted, the medium null where a row set none, and the stamp a create sets.
#[test]
fn rows_go_several_to_a_statement_as_creates_would_write_them() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    let rows: Vec<EntityValue> = (0..250).map(|index| row(&facade, index)).collect();
    let from = tape.lock().unwrap().len();
    facade.create_values(TEST, rows).unwrap();
    let inserts = inserts(&tape, from);
    let counts: Vec<usize> = inserts.iter().map(|sql| sql.matches("(?").count()).collect();
    assert_eq!(counts, [100, 100, 30, 10, 10], "{inserts:?}");
    for sql in &inserts {
        assert!(sql.contains("\"test_medium\""), "{sql}");
        assert!(sql.contains("\"last_updated_stamp\""), "{sql}");
    }
    assert_eq!(facade.find(TEST).unwrap().count().unwrap(), 250);
    let odd = facade
        .find(TEST)
        .unwrap()
        .eq("testId", Value::string("C007"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(odd.get("testNumberInteger"), Some(&Value::I64(7)));
    assert!(odd.get("testMedium").is_none_or(Value::is_null), "{odd:?}");
    assert!(odd.get("lastUpdatedStamp").is_some_and(|stamp| !stamp.is_null()));
    let even = facade
        .find(TEST)
        .unwrap()
        .eq("testId", Value::string("C008"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(even.get("testMedium"), Some(&Value::string("even")));
}

/// An encrypted field is sealed in each row as a create seals it, and reads back in the clear.
#[test]
fn encrypted_fields_are_sealed_in_each_row() {
    let facade = crate::testing::open();
    facade.set_crypt(
        CryptConfig {
            pass: "MoquiDefaultPassword:CHANGEME".to_string(),
            salt: "20201202".to_string(),
            iterations: 10,
            algorithm: "PBEWithHmacSHA256AndAES_128".to_string(),
        },
        Vec::new(),
    );
    let entity = "moqui.database.DatabaseConnection";
    let rows: Vec<EntityValue> = ["H1", "H2", "H3"]
        .iter()
        .map(|id| {
            let mut row = facade.make_value(entity).unwrap();
            row.set("connectionId", Value::string(*id));
            row.set("password", Value::string(format!("secret of {id}")));
            row
        })
        .collect();
    facade.create_values(entity, rows).unwrap();
    let tx = facade.transactions_for(entity).unwrap();
    let stored = query_tx(
        &tx,
        "SELECT \"password\" FROM \"database_connection\" WHERE \"connection_id\" = ?",
        &[Value::string("H2")],
    )
    .unwrap();
    let stored = stored[0].columns[0].1.as_str().unwrap().to_string();
    assert!(stored.starts_with(':'), "{stored}");
    let read = facade
        .find(entity)
        .unwrap()
        .eq("connectionId", Value::string("H2"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(read.get("password").and_then(Value::as_str), Some("secret of H2"));
}

/// A value of another entity is refused, and nothing is written.
#[test]
fn a_value_of_another_entity_is_refused() {
    let facade = crate::testing::open();
    let mut other = facade.make_value("moqui.test.TestEntity").unwrap();
    other.set("testId", Value::string("X"));
    let err = facade
        .create_values("moqui.basic.Enumeration", vec![other])
        .unwrap_err();
    assert!(err.to_string().contains("is not a value of"), "{err}");
    assert_eq!(facade.find(TEST).unwrap().count().unwrap(), 0);
}

/// A row without its primary key fails the call before any statement goes out.
#[test]
fn a_row_without_its_key_fails_the_call() {
    let facade = crate::testing::open();
    let mut keyless = facade.make_value(TEST).unwrap();
    keyless.set("testMedium", Value::string("no key"));
    let rows = vec![row(&facade, 1), keyless];
    assert!(facade.create_values(TEST, rows).is_err());
    assert_eq!(facade.find(TEST).unwrap().count().unwrap(), 0);
}

/// A cached list and count of the entity are cleared by the rows as by their creates: the next cached find sees them.
#[test]
fn the_rows_clear_the_cached_finds() {
    let facade = crate::testing::open();
    let cached_list = || {
        facade
            .find(TEST)
            .unwrap()
            .eq("testMedium", Value::string("even"))
            .use_cache(true)
            .list()
            .unwrap()
            .len()
    };
    let cached_count = || facade.find(TEST).unwrap().use_cache(true).count().unwrap();
    assert_eq!((cached_list(), cached_count()), (0, 0));
    facade
        .create_values(TEST, (0..4).map(|index| row(&facade, index)))
        .unwrap();
    assert_eq!((cached_list(), cached_count()), (2, 4));
}

/// In a transaction cache each row is held as its create would hold it, and written at the commit.
#[test]
fn a_transaction_cache_holds_each_row() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    let rows: Vec<EntityValue> = (0..3).map(|index| row(&facade, index)).collect();
    let tx = facade.transactions_for(TEST).unwrap();
    tx.run(TransactionAttribute::Cache, None, || {
        let from = tape.lock().unwrap().len();
        facade.create_values(TEST, rows)?;
        assert!(inserts(&tape, from).is_empty(), "held until the commit");
        assert_eq!(facade.find(TEST)?.count()?, 3);
        Ok(())
    })
    .unwrap();
    assert_eq!(facade.find(TEST).unwrap().count().unwrap(), 3);
}
