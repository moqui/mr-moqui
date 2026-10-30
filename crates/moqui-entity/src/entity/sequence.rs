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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_db::sql::{is_mysql, is_sqlite, quote_column, quote_physical_ident};
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, Value};
use moqui_lib::sync::lock;

use super::exec::{query_conn, with_reserve_tx};
use super::facade::EntityFacade;
use super::value::FieldMap;
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
        let mut names = lock(&self.names);
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
        let mut bank = lock(&bank);
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
        *lock(&bank) = SequenceBank {
            current: next,
            end: next.saturating_add(bank_size),
        };
    }

    /// Java's `tempResetSequencedIdPrimary`: drop the bank, so the next id comes from a database refill.
    fn temp_reset(&self, seq_name: &str) {
        let bank = self.bank(seq_name);
        *lock(&bank) = SequenceBank { current: 0, end: -1 };
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
    values: &FieldMap,
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
    let id = facade.inner.banks.next_id(&spec.seq_name, spec.stagger, |hint| {
        refill_bank(facade, &spec.seq_name, spec.bank_size, hint)
    })?;
    let id = apply_prefix(facade.inner.sequenced_id_prefix.as_deref(), &id.to_string());
    Ok((spec.pk_field, apply_prefix(prefix.as_deref(), &id)))
}

/// The entity's `sequence-primary-prefix`, with `${}` run against the value's fields.
pub fn expand_prefix(raw: Option<&str>, values: &FieldMap) -> Result<Option<String>, Error> {
    match raw {
        Some(raw) if raw.contains("${") => {
            let _frame = crate::hooks::ScriptFrame::push();
            for (name, value) in values {
                crate::hooks::set_name(name, value.clone())?;
            }
            Ok(Some(crate::hooks::expand(raw)?))
        }
        other => Ok(other.map(str::to_string)),
    }
}

/// `EntityFacade.sequencedIdPrimary(seqName, staggerMax, bankSize)`: the next id of any sequence name, as text, with the
/// global `sequenced-id-prefix`. A name that is an entity with a UUID primary sequence (its own or its datasource's)
/// gets a UUID. No stagger is 0 and no bank size is the default of 50 (also for zero or less), as Java.
pub fn sequenced_id_primary(
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
    let id = facade
        .inner
        .banks
        .next_id(seq_name, stagger, |hint| refill_bank(facade, seq_name, bank_size, hint))?;
    Ok(apply_prefix(
        facade.inner.sequenced_id_prefix.as_deref(),
        &id.to_string(),
    ))
}

/// A database refill of `seq_name`'s bank. The SQL is rendered here, as most ids come from the bank without one.
fn refill_bank(facade: &EntityFacade, seq_name: &str, bank_size: u64, hint: i64) -> Result<SequenceBank, Error> {
    let prepared = facade.with_entity(SEQUENCE_ENTITY, |entity, dialect, _| seq_sql(entity, dialect))?;
    let tx = facade.transactions_for(SEQUENCE_ENTITY)?;
    refill(&tx, prepared, seq_name, bank_size, hint)
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
    prepared: SeqSql,
    seq_name: &str,
    bank_size: u64,
    hint: i64,
) -> Result<SequenceBank, Error> {
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
        params.push(basic_convert(&Value::I64(crate::hooks::now_millis()), "java.sql.Timestamp").value);
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

pub fn uuid_v4() -> Result<String, Error> {
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
#[path = "sequence_tests.rs"]
mod tests;
