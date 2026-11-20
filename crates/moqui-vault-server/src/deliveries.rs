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

//! Deliveries (plans/Vault-Instances.md §3.3, `VaultDelivery`): a value sealed in the client to one user's personal
//! key and signed by its sender. The server takes one only signed by the caller's own active key, to the recipient's
//! active key, and lists it only to that recipient, who checks the signature again before filing the value. An
//! instance's deliveries (its recovery shares, signed with its link key, M6) are written by the manager's link
//! (`moqui_instance::shares`); listed, they carry the link key and the fingerprint the manager pinned for it.

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault::ops::shapes;
use moqui_vault::text::hex;
use serde_json::{json, Value as Json};

use crate::common::{
    authz_off, check_id, fail, field, find, flag, json, now, out, refused, required, user, value, Out, Params,
    DELIVERY, PERSONAL_KEY,
};
use crate::events::{self, Event};
use crate::personal::{active_key, my_key, public_of};

/// The largest sealed value, in hex characters.
const MAX_SEALED_HEX: usize = 2 * (64 * 1024 + 1024);

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, send_vault_delivery)?;
    moqui_core::rust_service!(map, list_incoming_deliveries)?;
    moqui_core::rust_service!(map, file_vault_delivery)?;
    Ok(())
}

/// `send#VaultDelivery`: a delivery the caller sealed and signed, to a user's active personal key.
fn send_vault_delivery(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let mine = my_key(facade, &me)?;
    let delivery = shapes::delivery_of(&json(params, "delivery")?).map_err(refused)?;
    check_id("delivery_id", &delivery.delivery_id)?;
    if delivery.sender_kind != "user" || delivery.sender_id != me {
        return Err(fail("A delivery is sent by its signer, as a user"));
    }
    delivery.verify(&public_of(&mine)?.verify).map_err(refused)?;
    let recipient = active_key(facade, &delivery.recipient_user_id)?
        .ok_or_else(|| fail(format!("{} has no personal vault key", delivery.recipient_user_id)))?;
    if field(&recipient, "personalKeyId") != delivery.recipient_key_id {
        return Err(fail("The delivery is not sealed to the recipient's active key"));
    }
    if delivery.purpose.is_empty() || delivery.purpose.len() > 60 {
        return Err(fail("A delivery's purpose is 1 to 60 characters"));
    }
    if 2 * delivery.sealed.len() > MAX_SEALED_HEX {
        return Err(fail("A delivery is at most 64 KiB"));
    }
    let taken = find(facade, DELIVERY)?
        .eq("deliveryId", Value::string(&delivery.delivery_id))
        .one()?
        .is_some();
    if taken || crate::common::id_has_history(facade, "deliveryId", &delivery.delivery_id)? {
        return Err(crate::common::id_not_available("Delivery", &delivery.delivery_id));
    }
    let mut row = facade.make_value(DELIVERY)?;
    row.set("deliveryId", Value::string(&delivery.delivery_id));
    row.set("recipientUserId", Value::string(&delivery.recipient_user_id));
    row.set("recipientKeyId", Value::string(&delivery.recipient_key_id));
    row.set("purpose", Value::string(&delivery.purpose));
    row.set("senderKind", Value::string(&delivery.sender_kind));
    row.set("senderId", Value::string(&delivery.sender_id));
    row.set("senderKeyId", Value::string(field(&mine, "personalKeyId")));
    row.set("sealedValue", Value::string(hex(&delivery.sealed)));
    row.set("signature", Value::string(hex(&delivery.signature)));
    row.set("sentDate", now());
    row.create(facade)?;
    events::write(
        facade,
        Event {
            kind: "VeDeliverySent",
            delivery_id: Some(&delivery.delivery_id),
            subject_user_id: Some(&delivery.recipient_user_id),
            detail: Some(delivery.purpose.clone()),
            ..Event::default()
        },
    )?;
    Ok(out([("deliveryId", Value::string(delivery.delivery_id))]))
}

/// A delivery as the vault crate reads it, with the sender key's public halves for the client to check (against a
/// key its user checked) and the dates.
fn delivery_json(facade: &EntityFacade, row: &EntityValue) -> Result<Json, LibError> {
    // an instance's delivery (a recovery share, M6): its link key's Ed25519 half and the fingerprint the manager pinned
    // for it, which the holder's client checks it against before it trusts the signature
    let (sender_public, sender_fingerprint) = if field(row, "senderKind") == "instance" {
        match find(facade, "moqui.server.instance.AppInstance")?
            .eq("appInstanceId", Value::string(field(row, "senderId")))
            .one()?
        {
            Some(instance) => (
                field(&instance, "linkSignPublic"),
                field(&instance, "linkKeyFingerprint"),
            ),
            None => (String::new(), String::new()),
        }
    } else {
        let public = match find(facade, PERSONAL_KEY)?
            .eq("personalKeyId", Value::string(field(row, "senderKeyId")))
            .one()?
        {
            Some(key) => hex(&public_of(&key)?.to_bytes()),
            None => String::new(),
        };
        (public, String::new())
    };
    Ok(json!({
        "deliveryId": field(row, "deliveryId"),
        "delivery": {
            "delivery_id": field(row, "deliveryId"), "recipient_user_id": field(row, "recipientUserId"),
            "recipient_key_id": field(row, "recipientKeyId"), "purpose": field(row, "purpose"),
            "sender_kind": field(row, "senderKind"), "sender_id": field(row, "senderId"),
            "sealed": field(row, "sealedValue"), "signature": field(row, "signature"),
        },
        "senderKeyId": field(row, "senderKeyId"),
        "senderPublic": sender_public,
        "senderFingerprint": sender_fingerprint,
        "sentDate": field(row, "sentDate"),
        "filedDate": field(row, "filedDate"),
    }))
}

/// `list#IncomingDeliveries`: the caller's deliveries, those not filed yet unless `includeFiled`.
fn list_incoming_deliveries(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let include_filed = flag(params, "includeFiled").unwrap_or(false);
    let rows = find(facade, DELIVERY)?
        .eq("recipientUserId", Value::string(&me))
        .order_by("sentDate", false)
        .list()?;
    let mut listed = Vec::new();
    for row in rows.iter() {
        if include_filed || field(row, "filedDate").is_empty() {
            // one row that does not read is logged and left out; the others are listed
            match delivery_json(facade, row) {
                Ok(shaped) => listed.push(shaped),
                Err(err) => tracing::error!("VaultDelivery {} left out of a list: {err}", field(row, "deliveryId")),
            }
        }
    }
    Ok(out([("deliveryList", value(&Json::Array(listed)))]))
}

/// `file#VaultDelivery`: the recipient filed the value into a vault (in their client); an event.
fn file_vault_delivery(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let delivery_id = required(params, "deliveryId")?;
    let mut row = find(facade, DELIVERY)?
        .eq("deliveryId", Value::string(&delivery_id))
        .one()?
        .filter(|row| field(row, "recipientUserId") == me)
        .ok_or_else(|| fail(format!("No delivery {delivery_id}")))?;
    if field(&row, "filedDate").is_empty() {
        row.set("filedDate", now());
        row.update(facade)?;
        events::write(
            facade,
            Event {
                kind: "VeDeliveryFiled",
                delivery_id: Some(&delivery_id),
                ..Event::default()
            },
        )?;
    }
    Ok(out([("deliveryId", Value::string(delivery_id))]))
}
