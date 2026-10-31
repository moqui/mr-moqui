//! Leases on a monitoring scope (`moqui.monitor.MonitorScopeLock`): the regression detector's pass, an incident's
//! recovery. A claim is a short `force-new` transaction that takes the row for update and sets a token and an expiry on
//! the wall clock, so a stopped server's lease lapses (plans/Monitoring.md, Later: Regressions and incidents,
//! Recovery). The row is made the first time a scope is claimed.

use super::{fail, Error, ExecutionContext, Value};
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;

const LOCK: &str = "moqui.monitor.MonitorScopeLock";

/// Claim `scope` for `ttl` milliseconds: the token, or None while another holds it.
pub fn claim(ec: &ExecutionContext, entity: &EntityFacade, scope: &str, ttl: i64) -> Result<Option<String>, Error> {
    let _authz = ec.disable_authz();
    let tx = entity.transactions_for(LOCK)?;
    let exists = tx.run(TransactionAttribute::ForceNew, None, || {
        entity.find(LOCK)?.eq("scopeKey", Value::string(scope)).one()
    })?;
    if exists.is_none() {
        // Two servers may both make it; the one that loses finds the row in the claim below.
        let made = tx.run(TransactionAttribute::ForceNew, None, || {
            let mut row = entity.make_value(LOCK)?;
            row.set("scopeKey", Value::string(scope));
            row.create(entity)
        });
        if let Err(error) = made {
            tracing::debug!(scope, %error, "scope lock row made by another server");
        }
    }
    tx.run(TransactionAttribute::ForceNew, None, || {
        let mut row = entity
            .find(LOCK)?
            .eq("scopeKey", Value::string(scope))
            .for_update(true)
            .one()?
            .ok_or_else(|| fail(format!("scope lock {scope} could not be made")))?;
        let wall = moqui_core::user::now_millis();
        let held = row.get("claimToken").is_some_and(|token| !token.is_null())
            && row
                .get("claimedUntil")
                .map(moqui_lib::object::to_plain_string)
                .and_then(|until| until.parse::<i64>().ok())
                .is_some_and(|until| until > wall);
        if held {
            return Ok(None);
        }
        let token = moqui_core::entity::uuid_v4()?;
        row.set("claimToken", Value::string(&token));
        row.set("claimedUntil", Value::I64(wall + ttl));
        row.update(entity)?;
        Ok(Some(token))
    })
}

/// Give the lease back, if `token` still holds it. A failure is logged: the lease lapses on its own.
pub fn release(ec: &ExecutionContext, entity: &EntityFacade, scope: &str, token: &str) {
    let _authz = ec.disable_authz();
    let released = entity.transactions_for(LOCK).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            let Some(mut row) = entity
                .find(LOCK)?
                .eq("scopeKey", Value::string(scope))
                .for_update(true)
                .one()?
            else {
                return Ok(());
            };
            if row.get("claimToken").map(moqui_lib::object::to_plain_string).as_deref() == Some(token) {
                row.set("claimToken", Value::Null);
                row.set("claimedUntil", Value::Null);
                row.update(entity)?;
            }
            Ok(())
        })
    });
    if let Err(error) = released {
        tracing::warn!(scope, %error, "could not give back a monitoring scope lease");
    }
}
