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

use super::*;

#[test]
fn bank_starts_at_100000_and_end_is_inclusive() {
    let bank = fill_bank(None, 50, 0);
    assert_eq!(bank.current, 100_000);
    assert_eq!(bank.end, 100_050);
    let mut bank = bank;
    assert_eq!(take_id(&mut bank, 1, 0.0), Some(100_000));
    assert_eq!(bank.current, 100_001);
    bank.current = bank.end;
    assert_eq!(take_id(&mut bank, 1, 0.0), Some(100_050));
    assert_eq!(take_id(&mut bank, 1, 0.0), None);
}

#[test]
fn refill_starts_one_past_the_stored_last_id() {
    let bank = fill_bank(Some(100_050), 50, 0);
    assert_eq!(bank.current, 100_051);
    assert_eq!(bank.end, 100_101);
    let jumped = fill_bank(Some(100_050), 50, 100_053);
    assert_eq!(jumped.current, 100_053);
    assert_eq!(jumped.end, 100_103);
}

#[test]
fn stagger_step_rounds_and_is_at_least_one() {
    assert_eq!(stagger_step(4, 0.9), 4);
    assert_eq!(stagger_step(4, 0.0), 1);
    assert_eq!(stagger_step(1, 0.2), 1);
}

#[test]
fn sql_end_matches_fill_bank() {
    let (first_end, _, _) = refill_targets(50, 0);
    assert_eq!(bank_from_end(first_end, 50), fill_bank(None, 50, 0));
    let old = 100_050_i64;
    let (_, increment, hint_end) = refill_targets(50, 0);
    let bumped = old.saturating_add(increment).max(hint_end);
    assert_eq!(bank_from_end(bumped, 50), fill_bank(Some(old), 50, 0));
    let hint = 100_090;
    let (_, increment, hint_end) = refill_targets(50, hint);
    let jumped = old.saturating_add(increment).max(hint_end);
    assert_eq!(bank_from_end(jumped, 50), fill_bank(Some(old), 50, hint));
}

#[test]
fn next_id_refills_a_used_up_bank_from_its_hint() {
    let banks = SequenceBanks::default();
    let mut hints = Vec::new();
    let mut stored = None;
    let mut ids = Vec::new();
    for _ in 0..5 {
        let id = banks
            .next_id("A", 1, |hint| {
                hints.push(hint);
                let bank = fill_bank(stored, 1, hint);
                stored = Some(bank.end);
                Ok(bank)
            })
            .unwrap();
        ids.push(id);
    }
    assert_eq!(ids, vec![100_000, 100_001, 100_002, 100_003, 100_004]);
    assert_eq!(hints, vec![0, 100_002, 100_004]);
}

#[test]
fn a_blocked_refill_does_not_block_another_sequence() {
    use std::sync::mpsc;
    let banks = Arc::new(SequenceBanks::default());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let blocked = {
        let banks = Arc::clone(&banks);
        std::thread::spawn(move || {
            banks.next_id("Slow", 1, |hint| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(fill_bank(None, 10, hint))
            })
        })
    };
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("slow refill started");
    let (done_tx, done_rx) = mpsc::channel();
    let other = {
        let banks = Arc::clone(&banks);
        std::thread::spawn(move || {
            let id = banks.next_id("Fast", 1, |hint| Ok(fill_bank(None, 10, hint)));
            done_tx.send(id.unwrap()).unwrap();
        })
    };
    let id = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("another sequence is not held up by the slow refill");
    assert_eq!(id, 100_000);
    release_tx.send(()).unwrap();
    assert_eq!(blocked.join().unwrap().unwrap(), 100_000);
    other.join().unwrap();
}

#[test]
fn temp_set_pins_ids_and_reset_refills() {
    let banks = SequenceBanks::default();
    banks.temp_set("A", 55_500, 2);
    let mut refills = 0;
    let mut next = |banks: &SequenceBanks| {
        banks
            .next_id("A", 1, |hint| {
                refills += 1;
                Ok(fill_bank(Some(100_050), 10, hint))
            })
            .unwrap()
    };
    assert_eq!(next(&banks), 55_500);
    assert_eq!(next(&banks), 55_501);
    assert_eq!(next(&banks), 55_502);
    banks.temp_reset("A");
    assert_eq!(next(&banks), 100_051);
    drop(next);
    assert_eq!(refills, 1);
}

#[test]
fn a_failed_refill_leaves_the_bank_to_try_again() {
    let banks = SequenceBanks::default();
    let err = banks.next_id("A", 1, |_| Err(Error::new("db down"))).unwrap_err();
    assert!(err.to_string().contains("db down"), "{err}");
    let id = banks.next_id("A", 1, |hint| Ok(fill_bank(None, 10, hint))).unwrap();
    assert_eq!(id, 100_000);
}

#[test]
fn refill_sql_is_insert_on_conflict_or_for_update() {
    let framework = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let conf = moqui_lib::node::MNode::parse_file(framework.join("conf/MoquiDefaultConf.xml")).unwrap();
    let model = moqui_db::model::EntityModel::load_directory(&conf, &framework.join("entity")).unwrap();
    let dialects = moqui_db::dialect::Dialects::load(&conf).unwrap();
    let entity = model.entity("moqui.entity.SequenceValueItem").unwrap();
    let postgres = seq_sql(entity, dialects.get("postgres").unwrap()).unwrap();
    assert!(!postgres.mysql);
    assert_eq!(
        postgres.upsert_sql,
        "INSERT INTO \"sequence_value_item\" (\"seq_name\", \"seq_num\", \"last_updated_stamp\") VALUES (?, ?, ?) ON CONFLICT (\"seq_name\") DO UPDATE SET \"seq_num\" = GREATEST(\"sequence_value_item\".\"seq_num\" + ?, ?), \"last_updated_stamp\" = EXCLUDED.\"last_updated_stamp\" RETURNING \"seq_num\""
    );
    let mysql = seq_sql(entity, dialects.get("mysql").unwrap()).unwrap();
    assert!(mysql.mysql);
    assert_eq!(
        mysql.lock_sql,
        "SELECT `SEQ_NUM` FROM `SEQUENCE_VALUE_ITEM` WHERE `SEQ_NAME` = ? FOR UPDATE"
    );
    assert_eq!(
        mysql.mysql_upsert_sql,
        "INSERT INTO `SEQUENCE_VALUE_ITEM` (`SEQ_NAME`, `SEQ_NUM`, `LAST_UPDATED_STAMP`) VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE `SEQ_NUM` = GREATEST(`SEQ_NUM` + ?, ?), `LAST_UPDATED_STAMP` = VALUES(`LAST_UPDATED_STAMP`)"
    );
    let mut overridden = entity.clone();
    overridden.table_name = "SEQ_STORE".to_string();
    for field in &mut overridden.fields {
        field.column_name = format!("CUSTOM_{}", field.column_name);
    }
    let sql = seq_sql(&overridden, dialects.get("postgres").unwrap()).unwrap();
    assert!(sql.upsert_sql.starts_with(
        "INSERT INTO \"seq_store\" (\"custom_seq_name\", \"custom_seq_num\", \"custom_last_updated_stamp\")"
    ));
    assert!(sql.upsert_sql.ends_with("RETURNING \"custom_seq_num\""));
}
