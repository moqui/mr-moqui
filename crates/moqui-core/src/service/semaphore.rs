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

//! Service semaphore. The lock row is written with SQL on a reserve connection.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use moqui_db::sql::{insert, select, update, CompareOp, Condition, FieldRef, LimitOffset};
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::define::{SemaphoreMode, ServiceDef};
use super::util::{now_millis, now_stamp, reserve};
use crate::context::Error;
use crate::driver::DbRow;
use crate::entity::EntityFacade;
use crate::transaction::TransactionManager;

const COLUMNS: [&str; 4] = ["serviceName", "parameterValue", "lockThread", "lockTime"];

/// `None` when the service has no semaphore. The guard clears the row on drop.
pub(crate) fn take(
    def: &ServiceDef,
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<Option<Guard>, Error> {
    if def.semaphore == SemaphoreMode::None {
        return Ok(None);
    }
    let plan = plan(facade)?;
    let name = def.semaphore_name.clone().unwrap_or_else(|| def.name.clone());
    let scope = scope_value(def, parameters);
    let owner = owner_label();
    let started = Instant::now();
    let pause = if def.semaphore_sleep.is_zero() {
        Duration::from_millis(1)
    } else {
        def.semaphore_sleep
    };
    loop {
        let (thread, time) = match try_lock(&plan, &name, &scope, def.semaphore_ignore, &owner)? {
            None => {
                return Ok(Some(Guard {
                    tx: plan.tx.clone(),
                    clear_sql: plan.clear_sql.clone(),
                    owner,
                    name,
                    scope,
                }))
            }
            Some(holder) => holder,
        };
        if def.semaphore == SemaphoreMode::Fail {
            return Err(Error::new(format!(
                "An instance of service semaphore {name} with parameter value [{scope}] is already running (thread [{thread}], locked at {time}) and it is setup to fail on semaphore conflict."
            )));
        }
        if started.elapsed() >= def.semaphore_timeout {
            return Err(Error::new(format!(
                "An instance of service semaphore {name} with parameter value [{scope}] is already running (thread [{thread}], locked at {time}) and it is setup to wait on semaphore conflict, but the semaphore did not clear in {} seconds.",
                def.semaphore_timeout.as_secs()
            )));
        }
        std::thread::sleep(pause);
    }
}

pub(crate) struct Guard {
    tx: TransactionManager,
    clear_sql: String,
    /// The label this call wrote. The clear only applies while the row still carries it, so a
    /// holder whose lock was taken over after the ignore time does not free the new holder's lock.
    owner: Value,
    name: String,
    scope: String,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let params = vec![
            Value::Null,
            Value::Null,
            Value::string(&self.name),
            Value::string(&self.scope),
            self.owner.clone(),
        ];
        let cleared = reserve(&self.tx, |conn| {
            conn.execute(&self.clear_sql, &params)?;
            Ok(())
        });
        if let Err(err) = cleared {
            tracing::warn!(error = %err, "semaphore clear failed");
        }
    }
}

struct Plan {
    tx: TransactionManager,
    select_sql: String,
    insert_sql: String,
    /// Claim a row nobody holds. Applies only while `lockThread` is still null.
    claim_free_sql: String,
    /// Claim a row whose hold expired. Applies only while `lockThread` is still the expired one.
    claim_stale_sql: String,
    clear_sql: String,
}

fn plan(facade: &EntityFacade) -> Result<Plan, Error> {
    facade
        .with_group(ServiceDef::semaphore_entity(), |entity, dialect, _, tx| {
            let condition = Condition::and(vec![
                Condition::bound("serviceName", CompareOp::Equals),
                Condition::bound("parameterValue", CompareOp::Equals),
            ]);
            let fields: Vec<FieldRef> = COLUMNS.iter().copied().map(FieldRef::new).collect();
            let select_sql = select(entity, dialect, &fields, Some(&condition), &[], LimitOffset::default())?.sql;
            let insert_sql = insert(entity, dialect, &COLUMNS)?;
            let set = ["lockThread", "lockTime"];
            let free = Condition::and(vec![condition.clone(), Condition::is_null("lockThread")]);
            let owned = Condition::and(vec![
                condition.clone(),
                Condition::bound("lockThread", CompareOp::Equals),
            ]);
            Ok(Plan {
                tx: tx.clone(),
                select_sql,
                insert_sql,
                claim_free_sql: update(entity, dialect, &set, Some(&free))?.sql,
                claim_stale_sql: update(entity, dialect, &set, Some(&owned))?.sql,
                clear_sql: update(entity, dialect, &set, Some(&owned))?.sql,
            })
        })
        .map_err(Error::from)
}

/// None when this call owns the row, else the `lockThread` and `lockTime` of the holder, as text.
///
/// Every way to take the row is a conditional write, and one that changes no row means someone
/// else got there first. Two callers cannot both win, and a lost insert race reads as held.
fn try_lock(
    plan: &Plan,
    name: &str,
    scope: &str,
    ignore: Duration,
    owner: &Value,
) -> Result<Option<(String, String)>, Error> {
    let name_value = Value::string(name);
    let scope_value = Value::string(scope);
    // As Java's message prints them: the lock time as `Timestamp.toString` does, in the server zone.
    let holder = |row: Option<&DbRow>| {
        let text = |field: &str| match row.and_then(|row| row.get(field)) {
            None | Some(Value::Null) => "null".to_string(),
            Some(Value::Timestamp(stamp)) => moqui_lib::format::format_timestamp(
                *stamp,
                Some("yyyy-MM-dd HH:mm:ss.SSS"),
                &jiff::tz::TimeZone::system(),
            )
            .unwrap_or_else(|_| stamp.to_string()),
            Some(value) => to_plain_string(value),
        };
        Some((text("lockThread"), text("lockTime")))
    };
    reserve(&plan.tx, |conn| {
        let rows = conn.query(&plan.select_sql, &[name_value.clone(), scope_value.clone()])?;
        let now = now_millis();
        if let Some(row) = rows.first() {
            if held(row, ignore, now) {
                return Ok(holder(Some(row)));
            }
            let previous = row.get("lockThread").cloned().unwrap_or(Value::Null);
            let claimed = if previous.is_null() {
                let params = vec![owner.clone(), now_stamp(), name_value.clone(), scope_value.clone()];
                conn.execute(&plan.claim_free_sql, &params)? > 0
            } else {
                let params = vec![
                    owner.clone(),
                    now_stamp(),
                    name_value.clone(),
                    scope_value.clone(),
                    previous,
                ];
                conn.execute(&plan.claim_stale_sql, &params)? > 0
            };
            if claimed {
                return Ok(None);
            }
            let rows = conn.query(&plan.select_sql, &[name_value, scope_value])?;
            return Ok(holder(rows.first()));
        }
        let params = vec![name_value.clone(), scope_value.clone(), owner.clone(), now_stamp()];
        match conn.execute(&plan.insert_sql, &params) {
            Ok(_) => Ok(None),
            Err(err) => {
                // A concurrent insert of the same key is a held lock. Anything else is a failure.
                let _ = conn.rollback();
                conn.begin()?;
                let rows = conn.query(&plan.select_sql, &[name_value, scope_value])?;
                if rows.is_empty() {
                    return Err(err);
                }
                Ok(holder(rows.first()))
            }
        }
    })
}

fn held(row: &DbRow, ignore: Duration, now: i64) -> bool {
    let thread = row.get("lockThread").cloned().unwrap_or(Value::Null);
    let time = row.get("lockTime").cloned().unwrap_or(Value::Null);
    let thread_set = !thread.is_null() && !is_empty(&thread);
    let time_set = !time.is_null() && !is_empty(&time);
    if !thread_set && !time_set {
        return false;
    }
    if let Some(locked) = stamp_millis(&time) {
        let ignore_ms = i64::try_from(ignore.as_millis()).unwrap_or(i64::MAX);
        if now > locked.saturating_add(ignore_ms) {
            return false;
        }
    }
    true
}

/// Java's `parameterValue`: `_NA_` with no (or an empty) `semaphore-parameter`, `_NULL_` when that parameter is
/// missing or null, else its text, empty text included.
fn scope_value(def: &ServiceDef, parameters: &BTreeMap<String, Value>) -> String {
    let Some(name) = def.semaphore_parameter.as_deref().filter(|name| !name.is_empty()) else {
        return "_NA_".to_string();
    };
    match parameters.get(name) {
        None | Some(Value::Null) => "_NULL_".to_string(),
        Some(value) => to_plain_string(value),
    }
}

/// The thread, plus a number that is new on every call, so a hold can be told from a later hold
/// by the same thread.
fn owner_label() -> Value {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let current = std::thread::current();
    let label = current.name().map(str::to_string).unwrap_or_else(|| {
        let id = current.id();
        format!("{id:?}")
    });
    let number = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Value::string(format!("{label}#{number}"))
}

fn stamp_millis(value: &Value) -> Option<i64> {
    if value.is_null() || is_empty(value) {
        return None;
    }
    to_plain_string(value).parse().ok()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use moqui_lib::object::Value;

    use super::scope_value;
    use crate::service::define::ServiceDef;

    /// Java's `parameterValue`: `_NA_` with no parameter named, `_NULL_` when it is missing or null, else its text.
    #[test]
    fn the_scope_is_the_parameter_text_and_empty_text_is_kept() {
        let mut def = ServiceDef::implicit("lock#Row", "lock", "Row");
        let parameters = |value: Option<Value>| -> BTreeMap<String, Value> {
            value.map(|value| ("orderId".to_string(), value)).into_iter().collect()
        };
        assert_eq!(scope_value(&def, &parameters(None)), "_NA_");
        def.semaphore_parameter = Some(String::new());
        assert_eq!(scope_value(&def, &parameters(Some(Value::string("A")))), "_NA_");
        def.semaphore_parameter = Some("orderId".to_string());
        assert_eq!(scope_value(&def, &parameters(None)), "_NULL_");
        assert_eq!(scope_value(&def, &parameters(Some(Value::Null))), "_NULL_");
        assert_eq!(scope_value(&def, &parameters(Some(Value::string("")))), "");
        assert_eq!(scope_value(&def, &parameters(Some(Value::string("A")))), "A");
    }
}
