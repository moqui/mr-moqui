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

//! The replay keys of signed requests (a system message's HMAC, a chat bridge webhook's signature), kept in the
//! `moqui.security.SignatureReplay` table so every cluster member sees each one (SEC-DAT-90): the conf caches are each
//! member's own, so a key held there let a captured message through once on each member. A key is taken by an insert
//! of its hash, which only one member's transaction can make; a row past its time is taken again, and the rows past
//! their time are deleted now and then.

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use sha2::Digest as _;

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::transaction::TransactionAttribute;

const ENTITY: &str = "moqui.security.SignatureReplay";
/// One take in this many also deletes the rows past their time.
const PURGE_EVERY: u64 = 200;

/// What the table knows of a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Taken {
    /// Not seen in its time: it is now held, its request being worked.
    New,
    /// Taken by a request (on any member) still being worked.
    Working,
    /// Its request was worked.
    Done,
}

/// The row id of `key`: its SHA-256, the first 40 hex digits (an `id` field).
fn hash_of(key: &str) -> String {
    sha2::Sha256::digest(key.as_bytes())
        .iter()
        .take(20)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn facade() -> Result<EntityFacade, Error> {
    ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade for the signature replay keys"))
}

fn now() -> i64 {
    crate::user::now_millis()
}

/// Take `key` for `hold_ms`: [`Taken::New`] the first time it is seen in that time on any member, else whether its
/// request is still being worked. In a transaction of its own, so a caller's rollback does not give the key back.
///
/// # Errors
///
/// When the table cannot be read or written: the caller refuses the request.
pub(crate) fn take(key: &str, hold_ms: i64) -> Result<Taken, Error> {
    let facade = facade()?;
    let hash = hash_of(key);
    let at = now();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(ENTITY)?;
    let taken = tx.run(TransactionAttribute::ForceNew, None, || {
        let existing = facade
            .find(ENTITY)?
            .use_cache(false)
            .for_update(true)
            .eq("replayHash", Value::string(&hash))
            .one()?;
        match existing {
            Some(row) if millis(&row, "expireStamp") > at => Ok(if text(&row, "worked") == "Y" {
                Taken::Done
            } else {
                Taken::Working
            }),
            Some(mut row) => {
                row.set("expireStamp", Value::I64(at.saturating_add(hold_ms)));
                row.set("worked", Value::string("N"));
                row.update(&facade)?;
                Ok(Taken::New)
            }
            None => {
                let mut row = facade.make_value(ENTITY)?;
                row.set("replayHash", Value::string(&hash));
                row.set("expireStamp", Value::I64(at.saturating_add(hold_ms)));
                row.set("worked", Value::string("N"));
                row.create(&facade)?;
                Ok(Taken::New)
            }
        }
    });
    let taken = match taken {
        Ok(taken) => taken,
        // another member's insert of the same key won the race: it is theirs
        Err(err) if exists(&facade, &hash)? => {
            tracing::debug!("a signature replay key was taken at the same time elsewhere: {err}");
            Taken::Working
        }
        Err(err) => return Err(err),
    };
    if taken == Taken::New {
        purge_now_and_then(&facade, at);
    }
    Ok(taken)
}

/// The request of `key` was worked (`true`: kept as done until its time) or not (`false`: given back, so a retry goes
/// through). A failure is logged.
pub(crate) fn finish(key: &str, worked: bool) {
    let done = facade().and_then(|facade| {
        let hash = hash_of(key);
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let tx = facade.transactions_for(ENTITY)?;
        tx.run(TransactionAttribute::ForceNew, None, || {
            let find = facade.find(ENTITY)?.eq("replayHash", Value::string(&hash));
            if worked {
                find.update_all([("worked".into(), Value::string("Y"))]).map(|_| ())
            } else {
                find.delete_all().map(|_| ())
            }
        })
    });
    if let Err(err) = done {
        tracing::error!(
            "a signature replay key was not {}: {err}",
            if worked { "kept" } else { "given back" }
        );
    }
}

fn exists(facade: &EntityFacade, hash: &str) -> Result<bool, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    Ok(facade
        .find(ENTITY)?
        .use_cache(false)
        .eq("replayHash", Value::string(hash))
        .one()?
        .is_some())
}

/// Delete the rows past their time, once in [`PURGE_EVERY`] takes; a failure is logged.
fn purge_now_and_then(facade: &EntityFacade, at: i64) {
    static TAKES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if !TAKES
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .is_multiple_of(PURGE_EVERY)
    {
        return;
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let purged = facade.transactions_for(ENTITY).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            facade
                .find(ENTITY)?
                .condition(crate::entity::Condition::less("expireStamp", Value::I64(at)))
                .delete_all()
        })
    });
    if let Err(err) = purged {
        tracing::error!("the signature replay keys past their time were not deleted: {err}");
    }
}

fn millis(row: &crate::entity::EntityValue, name: &str) -> i64 {
    match row.get(name) {
        Some(Value::I64(number)) => *number,
        Some(Value::I32(number)) => i64::from(*number),
        Some(other) => moqui_lib::object::to_plain_string(other).parse().unwrap_or(0),
        None => 0,
    }
}

fn text(row: &crate::entity::EntityValue, name: &str) -> String {
    row.get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "replay_store_tests.rs"]
mod tests;
