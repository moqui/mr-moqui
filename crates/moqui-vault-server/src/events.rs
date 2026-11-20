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

//! The `VaultEvent` rows (plans/Vault-Instances.md §3.13): each change writes its event in the transaction of the
//! change, so a change whose event cannot be written does not happen (the service fails and rolls back); a lease's use
//! writes its event in a transaction of its own before the value is returned ([`write_alone`]), so a use whose event
//! cannot be written returns nothing (fail closed). An event names who acted, whom it was about and what changed, in
//! words, never a key or a value. `list#VaultEvents` shows a vault's events to its members, and a user's own events to
//! them.

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::common::{authz_off, field, find, int, out, text, user, vault_for, Access, Out, Params, EVENT};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, list_vault_events)?;
    Ok(())
}

/// One event to write.
#[derive(Debug, Default)]
pub(crate) struct Event<'a> {
    pub(crate) kind: &'a str,
    pub(crate) vault_id: Option<&'a str>,
    pub(crate) item_id: Option<&'a str>,
    pub(crate) subject_user_id: Option<&'a str>,
    pub(crate) personal_key_id: Option<&'a str>,
    pub(crate) key_version: Option<i64>,
    pub(crate) delivery_id: Option<&'a str>,
    pub(crate) lease_id: Option<&'a str>,
    /// Who acted when no user is signed in (the server ending a lease at its time limit or at a restart): none
    /// leaves the row's user empty.
    pub(crate) actor: Option<&'a str>,
    pub(crate) detail: Option<String>,
}

#[cfg(test)]
thread_local! {
    /// The tests' fault switch: while set on a thread, every event write there fails, as a database that refuses the
    /// row would.
    pub(crate) static FAIL_WRITES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Writes `event` as the signed-in user (or the event's actor when none is), in the caller's transaction.
pub(crate) fn write(facade: &EntityFacade, event: Event) -> Result<(), LibError> {
    #[cfg(test)]
    if FAIL_WRITES.with(std::cell::Cell::get) {
        return Err(crate::common::fail("The vault event could not be written (test fault)"));
    }
    let actor = user().ok().or_else(|| event.actor.map(str::to_string));
    let mut row = facade.make_value(EVENT)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("eventTypeEnumId", Value::string(event.kind));
    if let Some(actor) = actor {
        row.set("userId", Value::string(actor));
    }
    let optional = [
        ("vaultId", event.vault_id),
        ("vaultItemId", event.item_id),
        ("subjectUserId", event.subject_user_id),
        ("personalKeyId", event.personal_key_id),
        ("deliveryId", event.delivery_id),
        ("leaseId", event.lease_id),
    ];
    for (name, given) in optional {
        if let Some(given) = given {
            row.set(name, Value::string(given));
        }
    }
    if let Some(version) = event.key_version {
        row.set("keyVersion", Value::I64(version));
    }
    if let Some(detail) = event.detail {
        row.set("detail", Value::string(detail));
    }
    row.set("eventDate", crate::common::now());
    row.create(facade)?;
    Ok(())
}

/// Writes `event` in a transaction of its own, committed before this returns: a lease's use, whose value goes back
/// only once its event is kept (§3.13, fail closed).
pub(crate) fn write_alone(facade: &EntityFacade, event: Event) -> Result<(), LibError> {
    let _authz = authz_off();
    let tx = facade.transactions_for(EVENT)?;
    tx.run(TransactionAttribute::ForceNew, None, || write(facade, event))
}

fn event_json(row: &EntityValue) -> Value {
    let names = [
        "vaultEventId",
        "eventTypeEnumId",
        "vaultId",
        "vaultItemId",
        "userId",
        "subjectUserId",
        "personalKeyId",
        "keyVersion",
        "deliveryId",
        "leaseId",
        "detail",
        "eventDate",
    ];
    let map: moqui_lib::object::ValueMap = names
        .iter()
        .filter_map(|name| row.get(name).map(|value| (name.to_string(), value.clone())))
        .collect();
    Value::Map(map)
}

/// `list#VaultEvents`: a vault's events, newest first, to a member of it (or a single-secret vault's owner); with no
/// vault, the events the caller made or that were about them.
fn list_vault_events(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = text(params, "vaultId");
    let limit = int(params, "limit")
        .filter(|limit| (1..=1000).contains(limit))
        .unwrap_or(200);
    let rows: Vec<EntityValue> = if vault_id.is_empty() {
        let mut rows = Vec::new();
        for name in ["userId", "subjectUserId"] {
            let found = find(facade, EVENT)?
                .eq(name, Value::string(&me))
                .order_by("eventDate", true)
                .order_by("vaultEventId", true)
                .limit(limit)
                .list()?;
            rows.extend(found.into_values());
        }
        rows.sort_by_key(|row| std::cmp::Reverse((field(row, "eventDate"), field(row, "vaultEventId"))));
        rows.dedup_by_key(|row| field(row, "vaultEventId"));
        rows.truncate(usize::try_from(limit).unwrap_or(200));
        rows
    } else {
        vault_for(facade, &vault_id, &me, Access::Read, false)?;
        find(facade, EVENT)?
            .eq("vaultId", Value::string(&vault_id))
            .order_by("eventDate", true)
            .order_by("vaultEventId", true)
            .limit(limit)
            .list()?
            .into_values()
    };
    Ok(out([("eventList", Value::List(rows.iter().map(event_json).collect()))]))
}
