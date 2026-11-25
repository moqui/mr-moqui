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

//! One rotation of a binding at a time (plans/archive/Vault-Instances.md §4.7), across a cluster: an `InstanceRotation` row per
//! binding, keyed by it, holds the claim of the rotation that runs or waits for its lease (a token, until a time). A
//! claim is taken in a transaction of its own, the row locked (`for_update`) or made (its key is the binding, so a
//! second member's create of the same row fails), and refused while another token holds it and its time has not
//! passed. The row also keeps how the last rotation ended, which the job at the policy age backs off from.

use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::common::{fail, field, find};

pub(crate) const ROTATION: &str = "moqui.server.instance.InstanceRotation";

/// Takes the claim of `binding`'s rotation for `token` until `until` (epoch milliseconds): free, past its time, or this
/// token's already; else refused, naming the action that holds it (reasonCode rotationRunning).
pub fn claim(facade: &EntityFacade, vault_id: &str, binding: &str, token: &str, until: i64) -> Result<(), LibError> {
    let tx = facade.transactions_for(ROTATION)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        let now = moqui_core::user::now_millis();
        let row = find(facade, ROTATION)?
            .eq("vaultId", Value::string(vault_id))
            .eq("bindingKey", Value::string(binding))
            .for_update(true)
            .one()?;
        let Some(mut row) = row else {
            let mut row = facade.make_value(ROTATION)?;
            row.set("vaultId", Value::string(vault_id));
            row.set("bindingKey", Value::string(binding));
            row.set("claimToken", Value::string(token));
            row.set("claimedUntil", moqui_core::user::stamp_value(until));
            return row.create(facade);
        };
        let held = field(&row, "claimToken");
        let live = row
            .get("claimedUntil")
            .and_then(moqui_core::user::value_millis)
            .is_some_and(|at| at > now);
        if !held.is_empty() && held != token && live {
            return Err(fail(format!(
                "{binding} is being rotated already (action {}): one rotation of an item at a time (reasonCode \
                 rotationRunning)",
                field(&row, "instanceActionId")
            )));
        }
        row.set("claimToken", Value::string(token));
        row.set("claimedUntil", moqui_core::user::stamp_value(until));
        row.update(facade)
    })
    .map_err(|err| {
        if err.message().contains("rotationRunning") {
            err
        } else {
            fail(format!(
                "{binding} is being rotated already, or its claim was not taken: {} (reasonCode rotationRunning)",
                err.message()
            ))
        }
    })
}

/// Whether another rotation holds `binding` now: a refusal naming its action, read before a lease is asked so a
/// rotation that would be refused asks the approvers for nothing (the claim itself still decides a race).
pub fn check_free(facade: &EntityFacade, vault_id: &str, binding: &str) -> Result<(), LibError> {
    let row = find(facade, ROTATION)?
        .eq("vaultId", Value::string(vault_id))
        .eq("bindingKey", Value::string(binding))
        .one()?;
    let now = moqui_core::user::now_millis();
    match row {
        Some(row)
            if !field(&row, "claimToken").is_empty()
                && row
                    .get("claimedUntil")
                    .and_then(moqui_core::user::value_millis)
                    .is_some_and(|at| at > now) =>
        {
            Err(fail(format!(
                "{binding} is being rotated already (action {}): one rotation of an item at a time (reasonCode \
                 rotationRunning)",
                field(&row, "instanceActionId")
            )))
        }
        _ => Ok(()),
    }
}

/// Notes the action that runs under a claim.
pub fn note_action(facade: &EntityFacade, vault_id: &str, binding: &str, token: &str, action_id: &str) {
    update(facade, vault_id, binding, token, |row| {
        row.set("instanceActionId", Value::string(action_id));
    });
}

/// Lets a claim go, when `token` holds it, with how the rotation ended when it ran (its action's status and id).
pub fn release(facade: &EntityFacade, vault_id: &str, binding: &str, token: &str, ended: Option<(&str, &str)>) {
    update(facade, vault_id, binding, token, |row| {
        row.set("claimToken", Value::Null);
        row.set("claimedUntil", Value::Null);
        if let Some((status, action_id)) = ended {
            row.set("lastStatusEnumId", Value::string(status));
            row.set("lastEndDate", moqui_core::user::now_value());
            row.set("lastActionId", Value::string(action_id));
        }
    });
}

fn update(
    facade: &EntityFacade,
    vault_id: &str,
    binding: &str,
    token: &str,
    change: impl FnOnce(&mut moqui_core::entity::EntityValue),
) {
    let done = facade.transactions_for(ROTATION).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            let Some(mut row) = find(facade, ROTATION)?
                .eq("vaultId", Value::string(vault_id))
                .eq("bindingKey", Value::string(binding))
                .for_update(true)
                .one()?
            else {
                return Ok(());
            };
            if field(&row, "claimToken") != token {
                return Ok(());
            }
            change(&mut row);
            row.update(facade)
        })
    });
    if let Err(err) = done {
        tracing::error!("Instances: the rotation claim of {binding} was not updated: {err}");
    }
}

/// When the last rotation of `binding` failed, at or after `since` (epoch milliseconds): its action.
pub fn failed_since(
    facade: &EntityFacade,
    vault_id: &str,
    binding: &str,
    since: i64,
) -> Result<Option<String>, LibError> {
    let row = find(facade, ROTATION)?
        .eq("vaultId", Value::string(vault_id))
        .eq("bindingKey", Value::string(binding))
        .one()?;
    Ok(row
        .filter(|row| {
            field(row, "lastStatusEnumId") == crate::actions::FAILED
                && row
                    .get("lastEndDate")
                    .and_then(moqui_core::user::value_millis)
                    .is_some_and(|at| at >= since)
        })
        .map(|row| field(&row, "lastActionId")))
}
