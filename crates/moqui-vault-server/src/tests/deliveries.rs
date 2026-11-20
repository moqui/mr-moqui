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

//! Deliveries (§3.3).

use moqui_lib::object::Value;
use serde_json::json;

use super::{new_id, setup, sys, text, Client};

/// A delivery goes only to its recipient: the server takes one only signed by its caller's own key and sealed to the
/// recipient's active key, lists it only to the recipient, and only the recipient files it; the recipient's client
/// refuses one whose signature a database writer changed, and opens the right one once. Sending and filing are events.
#[test]
fn a_delivery_goes_only_to_its_recipient() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let mut bob = Client::new(&facade, "bob");
    let carol = Client::new(&facade, "carol");
    alice.check("bob");
    bob.check("alice");
    let send = |client: &mut Client, id: &str| {
        let checks = client.checks();
        client
            .op(
                json!({ "op": "delivery_send", "delivery_id": id, "to_user_id": bob.user, "to_key_id": bob.key_id,
                        "to_public": bob.public, "purpose": "item", "value": "{\"v\":1,\"name\":\"wifi\"}",
                        "key_checks": checks }),
            )
            .unwrap()
    };
    let id = new_id("dlv");
    let delivery = send(&mut alice, &id);
    // carol cannot send alice's delivery as hers, nor alice one with a changed signature
    assert!(carol
        .call("send#VaultDelivery", json!({ "delivery": delivery.clone() }))
        .unwrap_err()
        .contains("its signer"));
    let mut forged = delivery.clone();
    forged["purpose"] = json!("other");
    assert!(alice
        .call("send#VaultDelivery", json!({ "delivery": forged }))
        .unwrap_err()
        .contains("signature"));
    alice
        .call("send#VaultDelivery", json!({ "delivery": delivery }))
        .unwrap();

    assert_eq!(
        carol.call("list#IncomingDeliveries", json!({})).unwrap()["deliveryList"],
        json!([])
    );
    assert!(carol
        .call("file#VaultDelivery", json!({ "deliveryId": id }))
        .unwrap_err()
        .contains("No delivery"));
    let listed = bob.call("list#IncomingDeliveries", json!({})).unwrap();
    let incoming = listed["deliveryList"][0].clone();
    assert_eq!(text(&incoming, "senderPublic"), alice.public);

    // a signature changed in the database: bob's client refuses it
    let mut changed = incoming["delivery"].clone();
    let signature = text(&changed, "signature");
    let flipped = if signature.starts_with('0') { "1" } else { "0" };
    changed["signature"] = json!(format!("{flipped}{}", &signature[1..]));
    let checks = bob.checks();
    let open = |delivery: &serde_json::Value| json!({ "op": "delivery_open", "delivery": delivery, "sender_public": incoming["senderPublic"], "key_checks": checks });
    assert!(bob.op(open(&changed)).unwrap_err().contains("signature"));
    let opened = bob.op(open(&incoming["delivery"])).unwrap();
    assert_eq!(opened["value"], json!("{\"v\":1,\"name\":\"wifi\"}"));
    bob.call("file#VaultDelivery", json!({ "deliveryId": id })).unwrap();
    assert_eq!(
        bob.call("list#IncomingDeliveries", json!({})).unwrap()["deliveryList"],
        json!([])
    );

    let kinds: Vec<String> = sys(|| {
        facade
            .find("moqui.security.vault.VaultEvent")
            .unwrap()
            .eq("deliveryId", Value::string(&id))
            .list()
            .unwrap()
            .iter()
            .map(|row| super::super::common::field(row, "eventTypeEnumId"))
            .collect()
    });
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    assert!(kinds.contains(&"VeDeliverySent".to_string()) && kinds.contains(&"VeDeliveryFiled".to_string()));
}
