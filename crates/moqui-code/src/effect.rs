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

//! The effect outbox (plans/agent-code/02-Orchestration.md section 1, T3's `EffectWorker`): work a command asks for
//! after its commit, such as a title to generate or a harness session to stop. A command writes `AgentCodeEffect` rows
//! in its own transaction; [`run_due`] claims them with a lease, runs the handler of each type and marks the row done
//! or failed.
//!
//! Rules kept from T3:
//!
//! - A failure backs off ([`backoff_millis`]) and the row is tried again, up to [`MAX_ATTEMPTS`]; then it is failed
//!   and kept.
//! - While a thread's oldest open effect waits in backoff, the later effects of that thread wait too.
//! - Done rows are trimmed after [`DONE_KEEP_DAYS`] by `clean#AgentCodeEffects`; failed rows are kept.
//!
//! The worker runs from the `run#AgentCodeEffects` service: the `agent_code_effects` ServiceJob each minute and a
//! wake after each commit that queued an effect. A lease that ran out (a node stopped) is claimed again.

use std::collections::{BTreeMap, BTreeSet};

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use crate::support::{int_of, json_of, json_text, millis_of, now_millis, stamp, text_of};

pub(crate) const EFFECT: &str = "moqui.agent.code.AgentCodeEffect";

pub const QUEUED: &str = "AgceQueued";
pub const RUNNING: &str = "AgceRunning";
pub const DONE: &str = "AgceDone";
pub const FAILED: &str = "AgceFailed";

/// Tries before a row is failed for good.
pub const MAX_ATTEMPTS: i64 = 8;
/// How long a claim holds a row.
pub const LEASE_MILLIS: i64 = 5 * 60 * 1000;
/// Days a done row is kept.
pub const DONE_KEEP_DAYS: i64 = 7;
/// Rows one pass claims at most.
pub const CLAIM_LIMIT: i64 = 50;

/// One effect a command asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectSpec {
    pub effect_type: String,
    pub conversation_id: Option<String>,
    pub payload: serde_json::Value,
}

/// A claimed effect, as a handler gets it.
#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    pub effect_id: String,
    pub effect_type: String,
    pub conversation_id: Option<String>,
    pub payload: serde_json::Value,
    pub attempt: i64,
}

/// The handler of one effect type, registered with `inventory::submit!`.
pub struct EffectHandler {
    pub effect_type: &'static str,
    pub run: fn(&EntityFacade, &Effect) -> Result<(), Error>,
}

inventory::collect!(EffectHandler);

fn handler(effect_type: &str) -> Option<&'static EffectHandler> {
    inventory::iter::<EffectHandler>
        .into_iter()
        .find(|handler| handler.effect_type == effect_type)
}

/// The wait before try `attempt + 1`: 5 s doubled per try, at most 10 minutes.
#[must_use]
pub fn backoff_millis(attempt: i64) -> i64 {
    let shift = u32::try_from(attempt.clamp(0, 7)).unwrap_or(7);
    (5_000_i64 << shift).min(10 * 60 * 1000)
}

/// Queue `spec` in the open transaction.
///
/// # Errors
///
/// When the insert fails.
pub fn enqueue(facade: &EntityFacade, spec: &EffectSpec) -> Result<String, Error> {
    let now = now_millis();
    let mut row = facade.make_value(EFFECT)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("effectType", Value::string(&spec.effect_type));
    if let Some(id) = &spec.conversation_id {
        row.set("conversationId", Value::string(id));
    }
    row.set("payloadJson", Value::string(json_text(&spec.payload)));
    row.set("statusId", Value::string(QUEUED));
    row.set("attemptCount", Value::I64(0));
    row.set("createdDate", stamp(now));
    row.create(facade)?;
    Ok(text_of(&row, "effectId").unwrap_or_default())
}

/// The open rows due at `now`, oldest first, less those of a thread whose oldest open row waits in backoff.
fn due(rows: &[EntityValue], now: i64) -> Vec<&EntityValue> {
    let mut blocked = BTreeSet::new();
    let mut picked = Vec::new();
    for row in rows {
        let thread = text_of(row, "conversationId");
        if thread.as_ref().is_some_and(|thread| blocked.contains(thread)) {
            continue;
        }
        let status = text_of(row, "statusId").unwrap_or_default();
        let ready = match status.as_str() {
            QUEUED => millis_of(row, "nextAttemptDate").is_none_or(|next| next <= now),
            RUNNING => millis_of(row, "leaseExpireDate").is_none_or(|end| end <= now),
            _ => false,
        };
        if ready {
            picked.push(row);
        }
        // a thread runs its effects in order: one open row holds the later ones
        if let Some(thread) = thread {
            blocked.insert(thread);
        }
    }
    picked
}

/// Claim the due rows for `owner`. Each claim is a compare-and-set on the status and lease, so two nodes never run
/// one row.
fn claim(facade: &EntityFacade, owner: &str, now: i64) -> Result<Vec<Effect>, Error> {
    let rows = facade
        .find(EFFECT)?
        .condition(Condition::is_in(
            "statusId",
            vec![Value::string(QUEUED), Value::string(RUNNING)],
        ))
        .order_by("createdDate", false)
        .order_by("effectId", false)
        .limit(CLAIM_LIMIT * 4)
        .disable_authz()
        .list()?;
    let mut claimed = Vec::new();
    for row in due(rows.values(), now).into_iter().take(usize::try_from(CLAIM_LIMIT).unwrap_or(50)) {
        let effect_id = text_of(row, "effectId").unwrap_or_default();
        let status = text_of(row, "statusId").unwrap_or_default();
        let attempt = int_of(row, "attemptCount").unwrap_or(0) + 1;
        let mut find = facade
            .find(EFFECT)?
            .eq("effectId", Value::string(&effect_id))
            .eq("statusId", Value::string(&status))
            .disable_authz();
        find = match millis_of(row, "leaseExpireDate") {
            Some(end) => find.eq("leaseExpireDate", stamp(end)),
            None => find.condition(Condition::is_null("leaseExpireDate")),
        };
        let changed = find.update_all([
            ("statusId".to_string(), Value::string(RUNNING)),
            ("leaseOwner".to_string(), Value::string(owner)),
            ("leaseExpireDate".to_string(), stamp(now + LEASE_MILLIS)),
            ("attemptCount".to_string(), Value::I64(attempt)),
        ])?;
        if changed == 0 {
            continue;
        }
        claimed.push(Effect {
            effect_id,
            effect_type: text_of(row, "effectType").unwrap_or_default(),
            conversation_id: text_of(row, "conversationId"),
            payload: json_of(row, "payloadJson"),
            attempt,
        });
    }
    Ok(claimed)
}

fn finish(facade: &EntityFacade, effect: &Effect, owner: &str, outcome: Result<(), String>) -> Result<(), Error> {
    let now = now_millis();
    let mut fields: Vec<(String, Value)> = vec![
        ("leaseOwner".to_string(), Value::Null),
        ("leaseExpireDate".to_string(), Value::Null),
    ];
    match outcome {
        Ok(()) => {
            fields.push(("statusId".to_string(), Value::string(DONE)));
            fields.push(("doneDate".to_string(), stamp(now)));
        }
        Err(text) => {
            let status = if effect.attempt >= MAX_ATTEMPTS { FAILED } else { QUEUED };
            fields.push(("statusId".to_string(), Value::string(status)));
            fields.push(("lastErrorText".to_string(), Value::string(text)));
            fields.push(("nextAttemptDate".to_string(), stamp(now + backoff_millis(effect.attempt))));
        }
    }
    facade
        .find(EFFECT)?
        .eq("effectId", Value::string(&effect.effect_id))
        .eq("leaseOwner", Value::string(owner))
        .disable_authz()
        .update_all(fields)?;
    Ok(())
}

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pass {
    pub done: usize,
    pub retried: usize,
    pub failed: usize,
}

/// Claim and run the due effects as `owner`. A failing effect is logged with its id and type and the pass goes on.
///
/// # Errors
///
/// When the claim itself fails.
pub fn run_due(facade: &EntityFacade, owner: &str) -> Result<Pass, Error> {
    let effects = claim(facade, owner, now_millis())?;
    let mut pass = Pass::default();
    for effect in effects {
        let outcome = match handler(&effect.effect_type) {
            Some(handler) => (handler.run)(facade, &effect).map_err(|err| err.to_string()),
            None => Err(format!("no handler for effect type {}", effect.effect_type)),
        };
        if let Err(text) = &outcome {
            tracing::error!(
                "agent code: effect {} ({}) failed on try {}: {text}",
                effect.effect_id,
                effect.effect_type,
                effect.attempt
            );
            if effect.attempt >= MAX_ATTEMPTS {
                pass.failed += 1;
            } else {
                pass.retried += 1;
            }
        } else {
            pass.done += 1;
        }
        if let Err(err) = finish(facade, &effect, owner, outcome) {
            tracing::error!("agent code: effect {} was not marked: {err}", effect.effect_id);
        }
    }
    Ok(pass)
}

/// Delete done rows older than [`DONE_KEEP_DAYS`]; failed rows stay.
///
/// # Errors
///
/// When the delete fails.
pub fn clean(facade: &EntityFacade) -> Result<usize, Error> {
    let before = now_millis() - DONE_KEEP_DAYS * 24 * 60 * 60 * 1000;
    let removed = facade
        .find(EFFECT)?
        .eq("statusId", Value::string(DONE))
        .condition(Condition::less("doneDate", stamp(before)))
        .disable_authz()
        .delete_all()?;
    Ok(usize::try_from(removed).unwrap_or_default())
}

/// The open effect counts by type, for diagnostics.
///
/// # Errors
///
/// When the find fails.
pub fn open_counts(facade: &EntityFacade) -> Result<BTreeMap<String, i64>, Error> {
    let rows = facade
        .find(EFFECT)?
        .condition(Condition::is_in(
            "statusId",
            vec![Value::string(QUEUED), Value::string(RUNNING)],
        ))
        .disable_authz()
        .list()?;
    let mut counts = BTreeMap::new();
    for row in rows.iter() {
        *counts.entry(text_of(row, "effectType").unwrap_or_default()).or_insert(0) += 1;
    }
    Ok(counts)
}

#[cfg(test)]
#[path = "effect_tests.rs"]
mod tests;
