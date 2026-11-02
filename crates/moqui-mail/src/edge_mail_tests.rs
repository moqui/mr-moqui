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

//! Mail through an edge node ([`super`]), on a local node with no link: the queue item of a received message (its key
//! by Message-ID and recipients, addresses and no account ids), its delivery to the accounts resolved here and its
//! redelivery doing nothing, and the edge node's bounce of a sent message reaching the Outbox. The link and the relay
//! are `crates/moqui-host/tests/it/edge_mail.rs`.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use super::*;
use crate::delivery::{Envelope, RecipientDsn};
use crate::sync_tests::{alice, field, install, ok, one, out_text, rows, setup, setup_with, text};

fn envelope() -> Envelope {
    Envelope {
        mail_from: "dan@example.org".to_string(),
        helo: "edge.example.com".to_string(),
        client_ip: "127.0.0.1".to_string(),
        protocol: "LMTP".to_string(),
        session_id: "E1".to_string(),
        hostname: "edge.example.com".to_string(),
        ..Envelope::default()
    }
}

fn recipient(address: &str) -> RecipientDsn {
    RecipientDsn {
        address: address.to_string(),
        ..RecipientDsn::default()
    }
}

fn message(message_id: &str) -> Vec<u8> {
    format!(
        "Message-ID: {message_id}\r\nFrom: Dan <dan@example.org>\r\nTo: sales@example.com\r\nSubject: Through the edge\r\n\
         Date: Mon, 19 Oct 2026 12:00:00 +0000\r\n\r\nHello\r\n"
    )
    .into_bytes()
}

#[test]
fn a_received_message_is_one_item_by_message_id_and_recipients_with_addresses_only() {
    let both = [recipient("sales@example.com"), recipient("Help@Example.com")];
    let item = inbound_item(message("<m1@example.org>"), &envelope(), &both, "acct-1");
    assert_eq!(item.kind, KIND_IN);
    assert_eq!(item.conversation_key, "acct-1");
    assert_eq!(item.content.as_deref(), Some(message("<m1@example.org>").as_slice()));
    let payload: serde_json::Value = serde_json::from_str(item.payload.as_deref().unwrap()).unwrap();
    assert_eq!(payload["mailFrom"], "dan@example.org");
    assert_eq!(payload["recipients"][1]["address"], "Help@Example.com");
    assert!(payload.get("emailAccountId").is_none());
    // the gateway's retry, the recipients in another order and case: the same item
    let again = inbound_item(
        message("<m1@example.org>"),
        &envelope(),
        &[recipient("help@example.com"), recipient("SALES@example.com")],
        "acct-1",
    );
    assert_eq!(again.idempotency_key, item.idempotency_key);
    let other = inbound_item(message("<m1@example.org>"), &envelope(), &both[..1], "acct-1");
    assert_ne!(other.idempotency_key, item.idempotency_key, "other recipients");
    let other = inbound_item(message("<m2@example.org>"), &envelope(), &both, "acct-1");
    assert_ne!(other.idempotency_key, item.idempotency_key, "another message");
    // no Message-ID: the message's own hash
    let bare = b"From: dan@example.org\r\nSubject: x\r\n\r\nbody\r\n".to_vec();
    let first = inbound_item(bare.clone(), &envelope(), &both, "acct-1");
    let second = inbound_item(bare, &envelope(), &both, "acct-1");
    assert_eq!(first.idempotency_key, second.idempotency_key);
    assert_ne!(first.idempotency_key, item.idempotency_key);
    assert!(is_edge_host(" Edge: "));
    assert!(!is_edge_host("smtp.example.com"));
}

/// The parameters the queue gives a handler, with the message kept where `location` names.
fn handler_parameters(facade: &EntityFacade, item: &NewItem, location: &str) -> BTreeMap<String, Value> {
    facade
        .write_db_resource(location, item.content.as_deref().unwrap_or_default())
        .unwrap();
    BTreeMap::from([
        ("queueItemId".to_string(), Value::string("item-1")),
        ("pairedNodeId".to_string(), Value::string("local")),
        ("kind".to_string(), Value::string(&item.kind)),
        ("conversationKey".to_string(), Value::string(&item.conversation_key)),
        ("idempotencyKey".to_string(), Value::string(&item.idempotency_key)),
        (
            "payload".to_string(),
            Value::string(item.payload.as_deref().unwrap_or_default()),
        ),
        ("contentLocation".to_string(), Value::string(location)),
    ])
}

#[test]
fn a_queued_message_is_delivered_to_the_accounts_resolved_here_once() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let (ctx, facade) = setup_with(r#"<moqui-conf><smtp-server plus-separator="+"/></moqui-conf>"#);
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let sales = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("accountKindEnumId", text("EakHosted")),
                ("emailAddress", text("sales@example.com")),
            ],
        ),
        "emailAccountId",
    );
    // an address no account here takes (the edge node's copy was older) is left out; a plus tag resolves here
    let recipients = [recipient("sales+edge@example.com"), recipient("gone@example.com")];
    let item = inbound_item(message("<edge-1@example.org>"), &envelope(), &recipients, "edge-acct");
    let location = "dbresource://moqui/edge/content/in/local/item-1";
    let parameters = handler_parameters(&facade, &item, location);
    deliver_edge_mail(&parameters, &facade).unwrap();
    let stored = rows(
        &facade,
        "moqui.basic.email.EmailMessage",
        &[("messageId", "<edge-1@example.org>")],
    );
    assert_eq!(stored.len(), 1);
    assert_eq!(field(&stored[0], "emailAccountId"), sales);
    let headers = field(&stored[0], "headersString");
    assert!(headers.contains("Delivered-To: sales+edge@example.com"), "{headers}");
    assert!(headers.contains("with LMTP"), "{headers}");
    // delivered again (its receipt was lost): nothing more
    deliver_edge_mail(&parameters, &facade).unwrap();
    assert_eq!(
        rows(
            &facade,
            "moqui.basic.email.EmailMessage",
            &[("messageId", "<edge-1@example.org>")]
        )
        .len(),
        1
    );
    // no message where the item names it: an error, so the queue tries again
    let mut missing = parameters.clone();
    missing.insert(
        "contentLocation".to_string(),
        Value::string("dbresource://moqui/edge/content/in/local/none"),
    );
    assert!(deliver_edge_mail(&missing, &facade).is_err());
}

/// A `mailStatus` for this node's `mailOut` item `item`, as the edge node sends it: only the item's id, never the
/// message's.
fn status(item: &str, status: &str) -> BTreeMap<String, Value> {
    let payload = serde_json::json!({
        "queueItemId": item,
        "status": status,
        "reply": "550 5.1.1 <nobody@example.net>: Recipient address rejected",
    });
    BTreeMap::from([
        ("queueItemId".to_string(), Value::string("status-1")),
        ("payload".to_string(), Value::string(payload.to_string())),
    ])
}

fn sent_message(facade: &EntityFacade, id: &str, account: Option<&str>) {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let mut row = facade.make_value("moqui.basic.email.EmailMessage").unwrap();
    row.set("emailMessageId", Value::string(id));
    row.set("statusId", Value::string("ES_SENT"));
    row.set("messageId", Value::string(format!("<{id}@example.com>")));
    row.set("subject", Value::string("Out"));
    if let Some(account) = account {
        row.set("emailAccountId", Value::string(account));
        row.set("emailFolderId", Value::string("SENT-FOLDER"));
    }
    row.create(facade).unwrap();
}

/// This node's `mailOut` item `item` for message `id`, as `enqueue_outbound` leaves it.
fn outbox_item(facade: &EntityFacade, item: &str, id: &str) {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let mut row = facade.make_value(queue::QUEUE_ITEM).unwrap();
    row.set("queueItemId", Value::string(item));
    row.set("pairedNodeId", Value::string("local"));
    row.set("directionEnumId", Value::string(queue::TO_EDGE));
    row.set("sequenceNum", Value::I64(1));
    row.set("kind", Value::string(KIND_OUT));
    row.set("conversationKey", Value::string(format!("mailOut:{id}")));
    row.set("idempotencyKey", Value::string(item));
    row.set(
        "payload",
        Value::string(
            serde_json::json!({"emailMessageId": id, "messageId": format!("<{id}@example.com>")}).to_string(),
        ),
    );
    row.set("statusEnumId", Value::string(queue::DELIVERED));
    row.create(facade).unwrap();
}

fn status_of(facade: &EntityFacade, id: &str) -> (String, String) {
    let row = one(facade, "moqui.basic.email.EmailMessage", "emailMessageId", id);
    (field(&row, "statusId"), field(&row, "sendError"))
}

#[test]
fn a_bounce_from_the_edge_node_puts_a_client_message_in_the_outbox_and_marks_a_template_message() {
    let _lock = moqui_core::user::session::cookie_property_lock();
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    install();
    alice(&facade);
    let account = out_text(
        &ok(
            "create#EmailAccount",
            &[
                ("accountKindEnumId", text("EakHosted")),
                ("emailAddress", text("alice@example.com")),
            ],
        ),
        "emailAccountId",
    );
    for (item, id, account) in [
        ("item-1", "OUT1", Some(account.as_str())),
        ("item-2", "OUT2", None),
        ("item-3", "OUT3", None),
        ("item-4", "OUT4", None),
        ("item-5", "OUT5", None),
    ] {
        sent_message(&facade, id, account);
        outbox_item(&facade, item, id);
    }
    note_edge_mail_status(&status("item-1", "bounced"), &facade).unwrap();
    let client = one(&facade, "moqui.basic.email.EmailMessage", "emailMessageId", "OUT1");
    assert_eq!(field(&client, "statusId"), "ES_READY", "back in the Outbox");
    assert_eq!(field(&client, "emailFolderId"), "");
    assert!(field(&client, "sendError").starts_with("Bounced: 550 5.1.1"));
    note_edge_mail_status(&status("item-2", "bounced"), &facade).unwrap();
    assert_eq!(status_of(&facade, "OUT2").0, "ES_BOUNCED");
    // accepted and delayed: left as they are; an item not in this node's outbox: nothing
    note_edge_mail_status(&status("item-3", "accepted"), &facade).unwrap();
    note_edge_mail_status(&status("item-3", "delayed"), &facade).unwrap();
    note_edge_mail_status(&status("no-such-item", "bounced"), &facade).unwrap();
    assert_eq!(status_of(&facade, "OUT3"), ("ES_SENT".to_string(), String::new()));
    // a message sent again since (another Message-ID) or cancelled is not overwritten
    crate::sync_tests::update(
        &facade,
        "moqui.basic.email.EmailMessage",
        ("emailMessageId", "OUT4"),
        &[("messageId", text("<again@example.com>"))],
    );
    note_edge_mail_status(&status("item-4", "bounced"), &facade).unwrap();
    assert_eq!(status_of(&facade, "OUT4").0, "ES_SENT");
    // an item that failed for good on this node (never reached the edge node): back, with the reason
    let failed = BTreeMap::from([
        ("queueItemId".to_string(), Value::string("item-5")),
        (
            "payload".to_string(),
            Value::string(serde_json::json!({"emailMessageId": "OUT5", "messageId": "<OUT5@example.com>"}).to_string()),
        ),
        ("error".to_string(), Value::string("unpaired")),
    ]);
    fail_edge_outbound_mail(&failed, &facade).unwrap();
    assert_eq!(
        status_of(&facade, "OUT5"),
        (
            "ES_BOUNCED".to_string(),
            "Not sent through the edge node: unpaired".to_string()
        )
    );
}

const DSN: &str = "From: MAILER-DAEMON@mx.example.net\r
To: bounce+k1234abcd@bounce.example.com\r
Subject: Undelivered Mail Returned to Sender\r
MIME-Version: 1.0\r
Content-Type: multipart/report; report-type=delivery-status; boundary=\"b1\"\r
\r
--b1\r
Content-Type: text/plain\r
\r
I'm sorry to have to inform you that your message could not be delivered.\r
--b1\r
Content-Type: message/delivery-status\r
\r
Reporting-MTA: dns; mx.example.net\r
Arrival-Date: Mon, 19 Oct 2026 12:00:00 +0000\r
\r
Final-Recipient: rfc822; nobody@example.net\r
Action: failed\r
Status: 5.1.1\r
Diagnostic-Code: smtp; 550 5.1.1 <nobody@example.net>:\r
 Recipient address rejected: User unknown\r
\r
Final-Recipient: rfc822; later@example.net\r
Action: delayed\r
Status: 4.4.1\r
\r
--b1--\r
";

#[test]
fn a_delivery_status_notification_to_a_verp_address_names_its_item_and_failure() {
    assert_eq!(
        bounce_item_id("Bounce+k1234abcd@Bounce.Example.com", "bounce.example.com").as_deref(),
        Some("k1234abcd")
    );
    assert_eq!(
        bounce_item_id("bounce+k1234abcd@example.com", "bounce.example.com"),
        None
    );
    assert_eq!(
        bounce_item_id("bounce+../x@bounce.example.com", "bounce.example.com"),
        None
    );
    assert_eq!(bounce_item_id("bounce+@bounce.example.com", "bounce.example.com"), None);
    assert_eq!(bounce_item_id("bounce+k1@bounce.example.com", ""), None);
    let report = parse_dsn(DSN.as_bytes()).unwrap();
    assert_eq!(report.action, "failed", "failed outranks delayed");
    assert!(
        report
            .detail
            .contains("rfc822; nobody@example.net; 5.1.1; smtp; 550 5.1.1"),
        "{report:?}"
    );
    assert!(
        report.detail.contains("Recipient address rejected"),
        "folded lines: {report:?}"
    );
    let item = dsn_status_item("k1234abcd", &report);
    assert_eq!(item.kind, KIND_STATUS);
    let payload: serde_json::Value = serde_json::from_str(item.payload.as_deref().unwrap()).unwrap();
    assert_eq!(payload["queueItemId"], "k1234abcd");
    assert_eq!(payload["status"], "bounced");
    assert_eq!(
        dsn_status_item("k1234abcd", &report).idempotency_key,
        item.idempotency_key
    );
    let delayed = DSN.replace("Action: failed", "Action: relayed");
    assert_eq!(parse_dsn(delayed.as_bytes()).unwrap().action, "delayed");
    assert_eq!(parse_dsn(b"Subject: Out of office\r\n\r\nAway\r\n"), None);
}
