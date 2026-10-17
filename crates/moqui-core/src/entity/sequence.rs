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

//! In-memory sequenced-id bank. Refill uses a reserve connection.
//!
//! Postgres claims the next bank with one `INSERT ... ON CONFLICT ... RETURNING`.
//! MySQL runs one `INSERT ... ON DUPLICATE KEY UPDATE`, then reads the row it now holds with
//! `SELECT ... FOR UPDATE`. A bare locking read on a missing row would take a gap lock.
//! `seqNum` stays the last reserved id. There is no native sequence.
//!
//! Each sequence name has its own bank lock. A refill holds only that name's lock, so a slow or blocked
//! refill of one name does not stop ids for the others.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moqui_db::sql::{is_mysql, is_sqlite, quote_column, quote_physical_ident};
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, Value};

use super::exec::{query_conn, with_reserve_tx};
use super::facade::EntityFacade;
use crate::driver::DbConnection;

const SEQUENCE_START: i64 = 100_000;
/// The bank size when a call names none, Java's `defaultBankSize`.
const DEFAULT_BANK_SIZE: u64 = 50;
const SEQUENCE_ENTITY: &str = "moqui.entity.SequenceValueItem";
/// Bounds the refill statements, so a refill that waits on a row lock (such as the caller's own
/// uncommitted write of the `SequenceValueItem` row) fails instead of hanging.
const REFILL_TIMEOUT: Duration = Duration::from_secs(30);

/// One bank. `end` is the last reserved id, inclusive. The next id to issue is `current`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceBank {
    pub current: i64,
    pub end: i64,
}

/// Fill a bank. No stored row starts at 100000.
///
/// A stored `seqNum` is the last id reserved for this name. The next id is one past it.
/// The bank reserves `bank_size + 1` ids, matching `EntityFacadeImpl.dbSequencedIdPrimary`.
pub fn fill_bank(stored_last: Option<i64>, bank_size: u64, current_hint: i64) -> SequenceBank {
    let size = i64::try_from(bank_size.max(1)).unwrap_or(i64::MAX);
    let start = match stored_last {
        Some(stored) => stored.saturating_add(1).max(current_hint),
        None => SEQUENCE_START.max(current_hint),
    };
    SequenceBank {
        current: start,
        end: start.saturating_add(size),
    }
}

/// How many ids to skip. `roll` is in `0.0..=1.0`. The step is at least 1.
pub fn stagger_step(stagger_max: u64, roll: f64) -> i64 {
    let max = u64::max(stagger_max, 1);
    let scaled = roll.clamp(0.0, 1.0) * (max as f64);
    let rounded = scaled.round();
    let step = if rounded.is_sign_negative() || rounded > (i64::MAX as f64) {
        1
    } else {
        rounded as i64
    };
    step.max(1)
}

/// Issue one id, or `None` when `current` is past `end`.
pub fn take_id(bank: &mut SequenceBank, stagger_max: u64, roll: f64) -> Option<i64> {
    if bank.current > bank.end {
        return None;
    }
    let id = bank.current;
    bank.current = bank.current.saturating_add(stagger_step(stagger_max, roll));
    Some(id)
}

/// The banks of one facade, by sequence name.
#[derive(Default)]
pub(crate) struct SequenceBanks {
    names: Mutex<HashMap<String, Arc<Mutex<SequenceBank>>>>,
}

impl SequenceBanks {
    /// The bank of `seq_name`. The map lock is held only for this lookup.
    fn bank(&self, seq_name: &str) -> Arc<Mutex<SequenceBank>> {
        let mut names = self.names.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(bank) = names.get(seq_name) {
            return Arc::clone(bank);
        }
        let bank = Arc::new(Mutex::new(SequenceBank { current: 0, end: -1 }));
        names.insert(seq_name.to_string(), Arc::clone(&bank));
        bank
    }

    /// Issue the next id of `seq_name`. A used-up bank is replaced by `refill`, which gets the bank's
    /// `current` as the hint, while only this name's lock is held.
    fn next_id(
        &self,
        seq_name: &str,
        stagger: u64,
        mut refill: impl FnMut(i64) -> Result<SequenceBank, Error>,
    ) -> Result<i64, Error> {
        let bank = self.bank(seq_name);
        let mut bank = bank.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            let roll = roll_for(stagger)?;
            if let Some(id) = take_id(&mut bank, stagger, roll) {
                return Ok(id);
            }
            *bank = refill(bank.current)?;
        }
    }

    /// Java's `tempSetSequencedIdPrimary`: the next ids of `seq_name` are `next` through `next + bank_size`, then
    /// the bank refills from the database as usual. Used by tests to pin generated ids.
    fn temp_set(&self, seq_name: &str, next: i64, bank_size: i64) {
        let bank = self.bank(seq_name);
        *bank.lock().unwrap_or_else(PoisonError::into_inner) = SequenceBank {
            current: next,
            end: next.saturating_add(bank_size),
        };
    }

    /// Java's `tempResetSequencedIdPrimary`: drop the bank, so the next id comes from a database refill.
    fn temp_reset(&self, seq_name: &str) {
        let bank = self.bank(seq_name);
        *bank.lock().unwrap_or_else(PoisonError::into_inner) = SequenceBank { current: 0, end: -1 };
    }
}

/// `EntityFacade.tempSetSequencedIdPrimary(seqName, nextSeqNum, bankSize)`. Factory wide, as Java.
pub(crate) fn temp_set_sequenced_id_primary(facade: &EntityFacade, seq_name: &str, next: i64, bank_size: i64) {
    facade.inner.banks.temp_set(seq_name, next, bank_size);
}

/// `EntityFacade.tempResetSequencedIdPrimary(seqName)`.
pub(crate) fn temp_reset_sequenced_id_primary(facade: &EntityFacade, seq_name: &str) {
    facade.inner.banks.temp_reset(seq_name);
}

struct PrimarySpec {
    seq_name: String,
    bank_size: u64,
    stagger: u64,
    use_uuid: bool,
    prefix: Option<String>,
    pk_field: String,
}

#[derive(Clone)]
struct SeqSql {
    mysql: bool,
    /// The row has `lastUpdatedStamp`, set on each refill as Java's create or update of the row does.
    stamp: bool,
    lock_sql: String,
    mysql_upsert_sql: String,
    upsert_sql: String,
}

/// The first primary key and its next id for a value of `entity_name` with `values`: a UUID, the sim's own id, or the
/// next of the bank with the global `sequenced-id-prefix`, after the entity's `sequence-primary-prefix` (`${}` expanded
/// with the value's fields, as Java's `setSequencedIdPrimary`).
pub(crate) fn assign_primary(
    facade: &EntityFacade,
    entity_name: &str,
    values: &BTreeMap<String, Value>,
) -> Result<(String, String), Error> {
    let spec = facade.with_entity(entity_name, |entity, _, _| {
        let pk_field = entity
            .pk_field_names()
            .first()
            .copied()
            .ok_or_else(|| Error::new(format!("{} has no primary key", entity.full_name)))?
            .to_string();
        Ok(PrimarySpec {
            seq_name: entity.full_name.clone(),
            bank_size: entity.sequence_bank_size,
            stagger: entity.sequence_primary_stagger,
            use_uuid: entity.sequence_primary_use_uuid || facade.datasource_uses_uuid(entity.group.as_deref()),
            prefix: entity.sequence_primary_prefix.clone(),
            pk_field,
        })
    })?;
    let prefix = expand_prefix(spec.prefix.as_deref(), values)?;
    if spec.use_uuid {
        return Ok((spec.pk_field, apply_prefix(prefix.as_deref(), &uuid_v4()?)));
    }
    // A sim takes its ids from its own bank, as Java's `TransactionCacheDb.nextSeq`.
    if let Some(id) = super::cache::sim_next_seq(&spec.seq_name) {
        return Ok((spec.pk_field, apply_prefix(prefix.as_deref(), &id.to_string())));
    }
    let prepared = facade.with_entity(SEQUENCE_ENTITY, |entity, dialect, _| seq_sql(entity, dialect))?;
    let tx = facade.transactions_for(SEQUENCE_ENTITY)?;
    let id = facade.inner.banks.next_id(&spec.seq_name, spec.stagger, |hint| {
        refill(&tx, &prepared, &spec.seq_name, spec.bank_size, hint)
    })?;
    let id = apply_prefix(facade.inner.sequenced_id_prefix.as_deref(), &id.to_string());
    Ok((spec.pk_field, apply_prefix(prefix.as_deref(), &id)))
}

/// The entity's `sequence-primary-prefix`, with `${}` run against the value's fields.
pub(super) fn expand_prefix(raw: Option<&str>, values: &BTreeMap<String, Value>) -> Result<Option<String>, Error> {
    match raw {
        Some(raw) if raw.contains("${") => {
            let _frame = crate::script::ContextFrame::push();
            for (name, value) in values {
                crate::script::set_context_name(name, value.clone())?;
            }
            Ok(Some(crate::script::expand(raw)?))
        }
        other => Ok(other.map(str::to_string)),
    }
}

/// `EntityFacade.sequencedIdPrimary(seqName, staggerMax, bankSize)`: the next id of any sequence name, as text, with the
/// global `sequenced-id-prefix`. A name that is an entity with a UUID primary sequence (its own or its datasource's)
/// gets a UUID. No stagger is 0 and no bank size is the default of 50 (also for zero or less), as Java.
pub(crate) fn sequenced_id_primary(
    facade: &EntityFacade,
    seq_name: &str,
    stagger: Option<u64>,
    bank_size: Option<u64>,
) -> Result<String, Error> {
    let uuid = facade
        .with_entity(seq_name, |entity, _, _| {
            Ok(entity.sequence_primary_use_uuid || facade.datasource_uses_uuid(entity.group.as_deref()))
        })
        .unwrap_or(false);
    if uuid {
        return uuid_v4();
    }
    let stagger = stagger.unwrap_or(0);
    let bank_size = bank_size.filter(|size| *size > 0).unwrap_or(DEFAULT_BANK_SIZE);
    if let Some(id) = super::cache::sim_next_seq(seq_name) {
        return Ok(id.to_string());
    }
    let prepared = facade.with_entity(SEQUENCE_ENTITY, |entity, dialect, _| seq_sql(entity, dialect))?;
    let tx = facade.transactions_for(SEQUENCE_ENTITY)?;
    let id = facade.inner.banks.next_id(seq_name, stagger, |hint| {
        refill(&tx, &prepared, seq_name, bank_size, hint)
    })?;
    Ok(apply_prefix(
        facade.inner.sequenced_id_prefix.as_deref(),
        &id.to_string(),
    ))
}

fn seq_sql(entity: &moqui_db::model::EntityDefinition, dialect: &moqui_db::dialect::Dialect) -> Result<SeqSql, Error> {
    let table = quote_physical_ident(dialect, &entity.table_name);
    let column = |name| {
        entity
            .field(name)
            .map(|field| quote_column(dialect, &field.column_name))
            .ok_or_else(|| Error::new(format!("missing sequence field {name}")))
    };
    let name_col = column("seqName")?;
    let num_col = column("seqNum")?;
    let stamp_col = column("lastUpdatedStamp").ok();
    let mysql = is_mysql(dialect);
    // SQLite has the same upsert and RETURNING, but its two-argument max() is GREATEST.
    let greatest = if is_sqlite(dialect) { "max" } else { "GREATEST" };
    let (insert_cols, insert_values, mysql_stamp, stamp_set) = match &stamp_col {
        Some(stamp) => (
            format!("{name_col}, {num_col}, {stamp}"),
            "?, ?, ?",
            format!(", {stamp} = VALUES({stamp})"),
            format!(", {stamp} = EXCLUDED.{stamp}"),
        ),
        None => (format!("{name_col}, {num_col}"), "?, ?", String::new(), String::new()),
    };
    Ok(SeqSql {
        mysql,
        stamp: stamp_col.is_some(),
        lock_sql: format!("SELECT {num_col} FROM {table} WHERE {name_col} = ? FOR UPDATE"),
        mysql_upsert_sql: format!(
            "INSERT INTO {table} ({insert_cols}) VALUES ({insert_values}) ON DUPLICATE KEY UPDATE {num_col} = GREATEST({num_col} + ?, ?){mysql_stamp}"
        ),
        upsert_sql: format!(
            "INSERT INTO {table} ({insert_cols}) VALUES ({insert_values}) ON CONFLICT ({name_col}) DO UPDATE SET {num_col} = {greatest}({table}.{num_col} + ?, ?){stamp_set} RETURNING {num_col}"
        ),
    })
}

fn refill(
    tx: &crate::transaction::TransactionManager,
    prepared: &SeqSql,
    seq_name: &str,
    bank_size: u64,
    hint: i64,
) -> Result<SequenceBank, Error> {
    let prepared = prepared.clone();
    let seq_name = seq_name.to_string();
    let size = bank_width(bank_size);
    with_reserve_tx(tx, move |conn| {
        conn.set_statement_timeout(Some(REFILL_TIMEOUT))?;
        let (first_end, increment, hint_end) = refill_targets(size, hint);
        let end = if prepared.mysql {
            mysql_end(conn, &prepared, &seq_name, first_end, increment, hint_end)
        } else {
            postgres_end(conn, &prepared, &seq_name, first_end, increment, hint_end)
        };
        // Postgres ends the timeout with the transaction. A session setting (MySQL) must not stay on
        // the reserve connection.
        conn.set_statement_timeout(None)?;
        Ok(bank_from_end(end?, size))
    })
}

/// `end` is the stored `seqNum`. The bank starts `size` below it, matching [`fill_bank`].
fn bank_from_end(end: i64, size: i64) -> SequenceBank {
    SequenceBank {
        current: end.saturating_sub(size),
        end,
    }
}

fn bank_width(bank_size: u64) -> i64 {
    i64::try_from(bank_size.max(1)).unwrap_or(i64::MAX)
}

/// First insert stores `first_end`. A conflict stores `GREATEST(old + increment, hint_end)`.
fn refill_targets(size: i64, hint: i64) -> (i64, i64, i64) {
    let first_end = SEQUENCE_START.max(hint).saturating_add(size);
    let increment = size.saturating_add(1);
    let hint_end = hint.saturating_add(size);
    (first_end, increment, hint_end)
}

fn postgres_end(
    conn: &mut dyn DbConnection,
    prepared: &SeqSql,
    seq_name: &str,
    first_end: i64,
    increment: i64,
    hint_end: i64,
) -> Result<i64, Error> {
    let rows = query_conn(
        conn,
        &prepared.upsert_sql,
        &upsert_params(prepared, string_param(seq_name), first_end, increment, hint_end),
    )?;
    one_seq_num(&rows, seq_name)
}

fn mysql_end(
    conn: &mut dyn DbConnection,
    prepared: &SeqSql,
    seq_name: &str,
    first_end: i64,
    increment: i64,
    hint_end: i64,
) -> Result<i64, Error> {
    let name = string_param(seq_name);
    super::exec::execute_conn(
        conn,
        &prepared.mysql_upsert_sql,
        &upsert_params(prepared, name.clone(), first_end, increment, hint_end),
    )?;
    let rows = query_conn(conn, &prepared.lock_sql, std::slice::from_ref(&name))?;
    one_seq_num(&rows, seq_name)
}

/// Insert values (the name, the first end, and the stamp when the row has one), then the conflict increment and hint.
fn upsert_params(prepared: &SeqSql, name: Value, first_end: i64, increment: i64, hint_end: i64) -> Vec<Value> {
    let mut params = vec![name, long_param(first_end)];
    if prepared.stamp {
        params.push(basic_convert(&Value::I64(crate::user::now_millis()), "java.sql.Timestamp").value);
    }
    params.push(long_param(increment));
    params.push(long_param(hint_end));
    params
}

fn one_seq_num(rows: &[crate::driver::DbRow], seq_name: &str) -> Result<i64, Error> {
    rows.first()
        .and_then(|row| row.columns.first())
        .and_then(|(_, value)| as_i64(value))
        .ok_or_else(|| Error::new(format!("sequence refill for {seq_name} returned no seqNum")))
}

fn string_param(seq_name: &str) -> Value {
    basic_convert(&Value::string(seq_name), "String").value
}

fn long_param(number: i64) -> Value {
    basic_convert(&Value::I64(number), "Long").value
}

fn as_i64(value: &Value) -> Option<i64> {
    match basic_convert(value, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

fn apply_prefix(prefix: Option<&str>, id: &str) -> String {
    match prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}{id}"),
        _ => id.to_string(),
    }
}

fn roll_for(stagger: u64) -> Result<f64, Error> {
    if stagger <= 1 {
        return Ok(0.0);
    }
    unit_roll()
}

fn unit_roll() -> Result<f64, Error> {
    let mut bytes = [0_u8; 8];
    fill_random(&mut bytes)?;
    let number = u64::from_le_bytes(bytes);
    Ok((number as f64) / (u64::MAX as f64))
}

pub(crate) fn uuid_v4() -> Result<String, Error> {
    let mut bytes = [0_u8; 16];
    fill_random(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format_uuid(&bytes))
}

fn fill_random(bytes: &mut [u8]) -> Result<(), Error> {
    getrandom::fill(bytes).map_err(|err| Error::new(format!("could not read random bytes: {err}")))
}

fn format_uuid(bytes: &[u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if index == 4 || index == 6 || index == 8 || index == 10 {
            out.push('-');
        }
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
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
        let framework =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
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
}
