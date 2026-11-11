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

//! `ec.transaction`: Java's `TransactionFacade` over the default entity group's transaction manager
//! (docs/dev/JS-Core-API.md). The functions are hidden functions of the globals shim, which builds `ec.transaction`.
//!
//! ```text
//! begin(timeoutSeconds) -> whether this call began a transaction (false when one is in place, which it joins)
//! commit(began)  commit()        commit when `began` is true or not given; nothing when it is false
//! rollback(began, message, error)   roll back when `began`, else mark the transaction in place rollback-only
//! rollback(message, error)          roll back the transaction in place
//! setRollbackOnly(message, error)   suspend() -> whether one was suspended   resume()
//! getStatus() -> 0 active, 1 marked rollback-only, 6 no transaction   getStatusString()
//! runUseOrBegin(timeoutSeconds, rollbackMessage, body)   body() in the transaction in place, or in one of its own
//! runRequireNew(timeoutSeconds, rollbackMessage, body)   body() in a new transaction, the one in place suspended
//! ```
//!
//! As in Java a `begin` that joins takes nothing that must be given back, so the usual
//! `const began = ec.transaction.begin(null); try { ... } finally { ec.transaction.commit(began) }` is balanced. The
//! The two run forms are the shim's, over these: a commit runs its hooks, which may run scripts, so no host call here
//! calls back into the script. XA and synchronization calls of Java's facade have no counterpart: there is one database transaction per group.

use std::time::Duration;

use moqui_lib::object::Value;

use super::{Call, HostError, Ret};
use crate::transaction::TransactionManager;

fn manager() -> Result<TransactionManager, HostError> {
    let facade = super::entity_facade()?;
    let group = facade.default_group();
    let found = facade
        .groups()
        .find(|(name, _, _)| *name == group)
        .map(|(_, _, transactions)| transactions.clone());
    found.ok_or_else(|| HostError::message(format!("no transaction manager for entity group {group}")))
}

/// The timeout argument in seconds, none for null, zero or less.
fn timeout(call: &Call<'_>, index: usize) -> Option<Duration> {
    let seconds = match call.value(index) {
        Some(Value::I32(seconds)) => f64::from(*seconds),
        #[allow(clippy::cast_precision_loss)] // a timeout in seconds is small
        Some(Value::I64(seconds)) => *seconds as f64,
        Some(Value::F64(seconds)) => *seconds,
        Some(Value::String(text)) => text.trim().parse().unwrap_or(0.0),
        _ => return None,
    };
    (seconds.is_finite() && seconds > 0.0).then(|| Duration::from_secs_f64(seconds))
}

/// The message argument, with the error's text after it when one is given.
fn message(call: &Call<'_>, index: usize, default: &str) -> String {
    let text = call.opt_text(index).filter(|text| !text.is_empty());
    let cause = call.opt_text(index + 1).filter(|text| !text.is_empty());
    match (text, cause) {
        (Some(text), Some(cause)) => format!("{text}: {cause}"),
        (Some(text), None) | (None, Some(text)) => text,
        (None, None) => default.to_string(),
    }
}

/// `begin(timeout)`: false with a transaction in place, else a new one and true.
pub(super) fn begin(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let manager = manager()?;
    if manager.in_transaction() {
        return Ok(false.into());
    }
    manager.begin(timeout(call, 0)).map(Ret::from).map_err(HostError::Lib)
}

/// `commit(began)`: nothing when `began` is false (the caller joined), else the transaction in place is committed. A
/// transaction marked rollback-only is rolled back, which is an error.
pub(super) fn commit(call: &mut Call<'_>) -> Result<Ret, HostError> {
    if matches!(call.value(0), Some(Value::Bool(false))) {
        return Ok(Ret::Undefined);
    }
    let manager = manager()?;
    if !manager.in_transaction() {
        return Ok(Ret::Undefined);
    }
    manager.commit().map_err(HostError::Lib)?;
    Ok(Ret::Undefined)
}

/// `rollback(began, message, error)` and `rollback(message, error)`.
pub(super) fn rollback(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let manager = manager()?;
    if !manager.in_transaction() {
        return Ok(Ret::Undefined);
    }
    match call.value(0) {
        Some(Value::Bool(false)) => {
            let message = message(call, 1, "rollback of a joined transaction");
            manager.set_rollback_only(message).map_err(HostError::Lib)?;
        }
        _ => manager.rollback().map_err(HostError::Lib)?,
    }
    Ok(Ret::Undefined)
}

pub(super) fn set_rollback_only(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let manager = manager()?;
    if !manager.in_transaction() {
        return Ok(Ret::Undefined);
    }
    let message = message(call, 0, "setRollbackOnly called by a script");
    manager.set_rollback_only(message).map_err(HostError::Lib)?;
    Ok(Ret::Undefined)
}

pub(super) fn suspend(_: &mut Call<'_>) -> Result<Ret, HostError> {
    manager()?.suspend().map(Ret::from).map_err(HostError::Lib)
}

pub(super) fn resume(_: &mut Call<'_>) -> Result<Ret, HostError> {
    manager()?.resume().map_err(HostError::Lib)?;
    Ok(Ret::Undefined)
}

/// Java's `javax.transaction.Status` of the transaction in place: 0 active, 1 marked rollback-only, 6 none.
pub(super) fn status(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let manager = manager()?;
    let status = if !manager.in_transaction() {
        6
    } else if manager.is_rollback_only() {
        1
    } else {
        0
    };
    Ok(Ret::Value(Value::I32(status)))
}

/// A new transaction with the one in place suspended until this one ends (`runRequireNew`'s begin): its commit or
/// rollback resumes the suspended one.
pub(super) fn begin_new(call: &mut Call<'_>) -> Result<Ret, HostError> {
    manager()?
        .force_new(timeout(call, 0))
        .map(Ret::from)
        .map_err(HostError::Lib)
}
