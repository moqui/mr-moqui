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

//! The outbox of tx-commit rules (plans/archive/High-Level-Review.md HL-025): a `ServiceOutbox` row written in the
//! transaction that commits, so the rules of a committed call run though the process stops before they do.
//!
//! The commit hook runs the rules from memory as before, on a service worker, and that run deletes the row first in its
//! own transaction ([`take`]): the rules' effects and the delete commit together, and a run that finds the row gone
//! (another run took it) does nothing. A full or stopping service queue leaves the row. The scheduler's pass runs a
//! row still there after its `availableDate` ([`replay`]) on a job worker, claimed by a conditional update of its
//! `attemptCount`, so one member runs it; a failed run records its error and waits longer, up to [`MAX_ATTEMPTS`].
//! Delivery is at least once (a crash after a rule's email was sent but before its commit sends it again), so such
//! rules de-duplicate. The row is written only where the call's transaction is the default group's, the one the rules
//! run in, and never in a sim session.

use std::sync::Arc;

use moqui_db::sql::{delete, update, CompareOp, Condition};
use moqui_lib::object::{basic_convert, Value};

use super::jobs::QueuedCall;
use super::pool::{RulesJob, Shared, TxRules};
use super::util::{field_text, now_millis, reserve};
use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::transaction::{TransactionAttribute, TransactionManager};

pub(crate) const OUTBOX: &str = "moqui.service.job.ServiceOutbox";
/// How long after the commit the scheduler may run a row: the commit hook's run has this long to take it first.
const GRACE_MILLIS: i64 = 60_000;
/// Runs tried before a row is left for an administrator, its last error kept.
pub(crate) const MAX_ATTEMPTS: i64 = 5;
/// The longest wait between attempts.
const MAX_BACKOFF_MILLIS: i64 = 3_600_000;

/// A run of tx rules kept in the outbox: its row, and whether the scheduler's claim already counted this attempt.
#[derive(Clone, Debug)]
pub(crate) struct OutboxEntry {
    pub(crate) id: String,
    pub(crate) claimed: bool,
}

fn stamp(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "Timestamp").value
}

/// The wait before the attempt after `attempts` tried: a minute, doubling, at most an hour.
fn backoff_millis(attempts: i64) -> i64 {
    let doublings = u32::try_from(attempts.clamp(1, 7) - 1).unwrap_or(0);
    GRACE_MILLIS.saturating_mul(1_i64 << doublings).min(MAX_BACKOFF_MILLIS)
}

/// Write `rules` (tx-commit rules of a call, with the caller `user_name` and `origin`) to the outbox in the transaction
/// `tx` has in place. Its id, or None when that transaction is not the outbox's (not the default group's), so the rules
/// run from memory only, as before the outbox.
///
/// # Errors
///
/// Returns an error when the row cannot be written; the call's transaction then fails, as for any write in it.
pub(crate) fn write(
    facade: &EntityFacade,
    tx: &TransactionManager,
    rules: &TxRules,
    user_name: Option<&str>,
    origin: Option<&crate::context::AgentOrigin>,
) -> Result<Option<String>, Error> {
    let outbox_tx = facade.transactions_for(OUTBOX)?;
    if !outbox_tx.shares_stack(tx) || !outbox_tx.shares_stack(&facade.default_transactions()?) {
        tracing::debug!(service = %rules.service, "the call's transaction is not the outbox's; its tx-commit rules are not kept in the outbox");
        return Ok(None);
    }
    // Written by the framework for the call, whatever the caller may write.
    let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let mut row = facade.make_value(OUTBOX)?;
    row.set_sequenced_id_primary(facade)?;
    let id = field_text(&row, "outboxId");
    row.set("serviceName", Value::string(&rules.service));
    row.set("phase", Value::string(&rules.when));
    row.set(
        "queuedCall",
        Value::string(super::jobs::rules_json(rules, user_name, origin)),
    );
    row.set("availableDate", stamp(now_millis().saturating_add(GRACE_MILLIS)));
    row.set("attemptCount", Value::I64(0));
    row.create(facade)?;
    Ok(Some(id))
}

/// Delete the row `id` in the transaction in place, before its rules run in it: true when this run took it, false when
/// another run did (its rules ran, or are running and commit with its delete).
///
/// # Errors
///
/// Returns an error when the row cannot be deleted.
pub(crate) fn take(facade: &EntityFacade, id: &str) -> Result<bool, Error> {
    let (sql, tx) = facade.with_group(OUTBOX, |entity, dialect, _, tx| {
        let by_id = Condition::bound("outboxId", CompareOp::Equals);
        Ok((delete(entity, dialect, Some(&by_id))?.sql, tx.clone()))
    })?;
    let deleted = tx.with_connection(|conn| conn.execute(&sql, &[Value::string(id)]))?;
    Ok(deleted > 0)
}

/// Record that the run of `entry` failed with `error`: its error and time, an attempt counted unless the scheduler's
/// claim counted it, and a later `availableDate`. A row past [`MAX_ATTEMPTS`] is left, logged as an error. A failure
/// to record is logged.
pub(crate) fn record_failure(factory: &Arc<ExecutionContextFactory>, entry: &OutboxEntry, service: &str, error: &str) {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let recorded = (|| -> Result<Option<i64>, Error> {
        let facade = factory
            .entity()
            .ok_or_else(|| Error::new("no entity facade is installed"))?;
        let tx = facade.transactions_for(OUTBOX)?;
        let attempts = tx.run(TransactionAttribute::ForceNew, None, || {
            let Some(mut row) = facade.find(OUTBOX)?.eq("outboxId", Value::string(&entry.id)).one()? else {
                // Its delete committed: the rules' transaction committed after all.
                return Ok(None);
            };
            let mut attempts = number(&row, "attemptCount").unwrap_or(0);
            if !entry.claimed {
                attempts += 1;
            }
            let now = now_millis();
            row.set("attemptCount", Value::I64(attempts));
            row.set("lastAttemptDate", stamp(now));
            row.set("availableDate", stamp(now.saturating_add(backoff_millis(attempts))));
            row.set("lastError", Value::string(error.chars().take(4000).collect::<String>()));
            row.update(&facade)?;
            Ok(Some(attempts))
        })?;
        Ok(attempts)
    })();
    match recorded {
        Ok(Some(attempts)) if attempts >= MAX_ATTEMPTS => tracing::error!(
            outbox = %entry.id,
            service,
            attempts,
            "the tx-commit rules failed {attempts} times; their outbox row is kept and not tried again"
        ),
        Ok(_) => {}
        Err(err) => {
            tracing::error!(outbox = %entry.id, service, error = %err, "could not record the failed tx rules in the outbox")
        }
    }
}

/// The scheduler pass's part: run at most `slots` rows whose `availableDate` has passed and that have attempts left,
/// the longest waiting first, each claimed with a conditional update (so one member runs it) and queued for a job
/// worker. Out: the rows queued. A row that cannot be read as rules is recorded as failed; each failure is logged and the
/// others go on.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub(crate) fn replay(
    shared: &Shared,
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    slots: i64,
) -> Result<usize, Error> {
    if slots <= 0 {
        return Ok(0);
    }
    let now = now_millis();
    let rows = facade
        .find(OUTBOX)?
        .condition(Condition::less("availableDate", stamp(now)))
        .condition(Condition::or(vec![
            Condition::is_null("attemptCount"),
            Condition::less("attemptCount", Value::I64(MAX_ATTEMPTS)),
        ]))
        .order_by("availableDate", false)
        .order_by("outboxId", false)
        .limit(slots)
        .list()?
        .into_values();
    let mut queued = 0;
    for row in rows {
        let id = field_text(&row, "outboxId");
        let service = field_text(&row, "serviceName");
        let attempts = number(&row, "attemptCount");
        match claim(facade, &id, attempts, now) {
            Ok(true) => {}
            // another member claimed it first
            Ok(false) => continue,
            Err(err) => {
                tracing::error!(outbox = %id, service = %service, error = %err, "could not claim tx rules in the outbox");
                continue;
            }
        }
        let entry = OutboxEntry { id, claimed: true };
        let call = QueuedCall::parse(&field_text(&row, "queuedCall"));
        let job = match call {
            Ok(QueuedCall {
                parameters,
                user_name,
                origin,
                rules: Some(rules),
                ..
            }) => RulesJob::from_outbox(
                TxRules {
                    service: service.clone(),
                    when: rules.when,
                    parameters,
                    results: rules.results,
                    authz: rules.authz,
                },
                Arc::clone(factory),
                user_name,
                origin,
                entry.clone(),
            ),
            Ok(_) => {
                record_failure(factory, &entry, &service, "the outbox row names no rules");
                continue;
            }
            Err(err) => {
                record_failure(factory, &entry, &service, &err.to_string());
                continue;
            }
        };
        tracing::info!(outbox = %entry.id, service = %service, "running tx-commit rules kept in the outbox");
        if !shared.submit_rules_job(job) {
            // Stopping or full: the claim's later availableDate brings it back.
            tracing::info!(outbox = %entry.id, service = %service, "no job worker took the outbox's tx rules; they wait for a later pass");
            break;
        }
        queued += 1;
    }
    Ok(queued)
}

/// Count an attempt of row `id` and move its `availableDate` on, while its `attemptCount` is still `attempts`: true when
/// this call claimed it. A statement of its own, committed before the run.
fn claim(facade: &EntityFacade, id: &str, attempts: Option<i64>, now: i64) -> Result<bool, Error> {
    let by_count = match attempts {
        Some(_) => Condition::bound("attemptCount", CompareOp::Equals),
        None => Condition::is_null("attemptCount"),
    };
    let condition = Condition::and(vec![Condition::bound("outboxId", CompareOp::Equals), by_count]);
    let (sql, tx) = facade.with_group(OUTBOX, |entity, dialect, _, tx| {
        let set = ["attemptCount", "lastAttemptDate", "availableDate"];
        Ok((update(entity, dialect, &set, Some(&condition))?.sql, tx.clone()))
    })?;
    let next = attempts.unwrap_or(0).saturating_add(1);
    let mut params = vec![
        Value::I64(next),
        stamp(now),
        stamp(now.saturating_add(backoff_millis(next))),
        Value::string(id),
    ];
    if let Some(attempts) = attempts {
        params.push(Value::I64(attempts));
    }
    reserve(&tx, |conn| Ok(conn.execute(&sql, &params)? > 0))
}

fn number(row: &EntityValue, field: &str) -> Option<i64> {
    match basic_convert(row.get(field)?, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

/// The rows of the outbox, for a test.
#[cfg(test)]
pub(crate) fn rows(facade: &EntityFacade) -> Vec<EntityValue> {
    facade
        .find(OUTBOX)
        .and_then(|find| find.disable_authz().list())
        .map(|rows| rows.into_values())
        .unwrap_or_default()
}
