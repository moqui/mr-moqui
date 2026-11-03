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

use super::*;
use crate::service::chat_bridge::WebhookEventKey;

fn request() -> WebhookRequest {
    WebhookRequest {
        method: "POST".to_string(),
        headers: vec![
            ("telnyx-signature-ed25519".to_string(), "c2ln".to_string()),
            ("telnyx-timestamp".to_string(), "1700000000".to_string()),
            ("cookie".to_string(), "session=1".to_string()),
            ("authorization".to_string(), "Bearer x".to_string()),
        ],
        body: b"{\"data\":{\"id\":\"ev-1\"}}\x00\xff".to_vec(),
        client_ip: "198.51.100.7".to_string(),
        now_millis: 1_700_000_000_123,
    }
}

#[test]
fn a_queued_webhook_is_the_request_again_without_credentials() {
    let key = WebhookEventKey {
        event_id: "ev-1".to_string(),
        conversation: Some("+15557778888".to_string()),
    };
    let item = webhook_item("SMS1", &key, &request());
    assert_eq!(item.kind, KIND_WEBHOOK);
    assert_eq!(item.idempotency_key, "chatWebhook:SMS1:ev-1");
    assert_eq!(item.conversation_key, "chatWebhook:SMS1:+15557778888");
    let payload: serde_json::Value = serde_json::from_str(item.payload.as_deref().unwrap()).unwrap();
    let again = request_of(&payload).unwrap();
    assert_eq!(again.body, request().body, "the raw bytes, as signed");
    assert_eq!(again.method, "POST");
    assert_eq!(again.client_ip, "198.51.100.7");
    assert_eq!(again.now_millis, 1_700_000_000_123, "checked at the time it came");
    assert_eq!(again.header("telnyx-timestamp"), Some("1700000000"));
    assert_eq!(again.header("cookie"), None);
    assert_eq!(again.header("authorization"), None);

    // no conversation: the bridge's; a long event id: its hash
    let long = WebhookEventKey {
        event_id: "x".repeat(300),
        conversation: None,
    };
    let item = webhook_item("SMS1", &long, &request());
    assert_eq!(item.conversation_key, "chatWebhook:SMS1");
    assert!(item.idempotency_key.starts_with("chatWebhook:SMS1:sha256:"));
    assert!(item.idempotency_key.len() < 120);
}

#[test]
fn a_malformed_queued_webhook_is_refused() {
    assert!(request_of(&serde_json::json!({"body": "not base64!", "headers": []})).is_err());
    assert!(request_of(&serde_json::json!({"body": ""})).is_err());
}

#[test]
fn a_file_token_is_the_same_for_one_send_and_no_other() {
    let one = media_token("key-1", "chatMedia:SMS1:d1:a1:7").unwrap();
    assert_eq!(media_token("key-1", "chatMedia:SMS1:d1:a1:7").unwrap(), one);
    assert_ne!(media_token("key-1", "chatMedia:SMS1:d1:a1:8").unwrap(), one);
    assert_ne!(media_token("key-2", "chatMedia:SMS1:d1:a1:7").unwrap(), one);
    assert_eq!(one.len(), 43, "base64url of 32 bytes: {one}");
    assert!(one
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
}

#[test]
fn a_file_is_kept_only_under_the_edge_nodes_own_root() {
    let hash = sha256_hex("token");
    assert_eq!(
        media_location("node-1", &hash).unwrap(),
        format!("dbresource://moqui/edge/media/node-1/{hash}")
    );
    assert!(media_location("../x", &hash).is_none());
    assert!(media_location("node-1", "a/b").is_none());
    assert!(media_location("", &hash).is_none());
}

#[test]
fn the_edge_node_keeps_files_within_its_limits() {
    assert_eq!(over_limits(1_000, 0, 1_000, 10_000), None);
    assert!(over_limits(1_001, 0, 1_000, 10_000).unwrap().contains(MEDIA_MAX_BYTES));
    assert_eq!(over_limits(1_000, 9_000, 1_000, 10_000), None);
    assert!(over_limits(1_000, 9_001, 1_000, 10_000)
        .unwrap()
        .contains(MEDIA_NODE_MEGABYTES));
}

#[test]
fn a_replicated_tolerance_is_a_positive_number() {
    let fields = serde_json::json!({"a": 120, "b": "90", "c": 0, "d": "x"});
    assert_eq!(number_field(&fields, "a"), Some(120));
    assert_eq!(number_field(&fields, "b"), Some(90));
    assert_eq!(number_field(&fields, "c"), None);
    assert_eq!(number_field(&fields, "d"), None);
    assert_eq!(number_field(&fields, "e"), None);
}

#[test]
fn an_all_in_one_node_takes_a_bridge_from_one_side_and_refuses_one_on_both() {
    let fields = serde_json::json!({"typeEnumId": "CbtSms"});
    let paired = || Some(("local-a".to_string(), fields.clone()));
    assert_eq!(owner_of(true, None), BridgeOwner::Own);
    assert_eq!(
        owner_of(false, paired()),
        BridgeOwner::Paired("local-a".to_string(), fields.clone())
    );
    assert_eq!(owner_of(true, paired()), BridgeOwner::Both("local-a".to_string()));
    assert_eq!(owner_of(false, None), BridgeOwner::None);
    let refused = both_refused("B1", "local-a");
    assert_eq!(refused.status, 409);
}

#[test]
fn the_paired_nodes_copy_is_read_for_the_owner() {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let mut client = facade.make_value("moqui.edge.EdgeClient").unwrap();
    client.set("pairedNodeId", Value::string("local-a"));
    client.set("keyFingerprint", Value::string("ab"));
    client.set("statusId", Value::string("EdgeClientActive"));
    client.create(&facade).unwrap();
    let batch = crate::edge::replica::Batch {
        set_name: REPLICA_SET.to_string(),
        epoch: "e1".to_string(),
        base_seq: 0,
        seq: 1,
        snapshot: true,
        snapshot_end: true,
        changes: vec![crate::edge::replica::Change {
            entity_name: BRIDGE.to_string(),
            pk_json: r#"{"chatBridgeId":"LOCAL_BRIDGE"}"#.to_string(),
            fields_json: Some(r#"{"typeEnumId":"CbtSms"}"#.to_string()),
        }],
    };
    crate::edge::replica::apply_batch(&facade, "local-a", &batch).unwrap();
    assert!(matches!(
        bridge_owner(&facade, "LOCAL_BRIDGE", false).unwrap(),
        BridgeOwner::Paired(node, _) if node == "local-a"
    ));
    assert_eq!(
        bridge_owner(&facade, "LOCAL_BRIDGE", true).unwrap(),
        BridgeOwner::Both("local-a".to_string())
    );
    assert_eq!(bridge_owner(&facade, "OWN_BRIDGE", true).unwrap(), BridgeOwner::Own);
    assert_eq!(bridge_owner(&facade, "NONE", false).unwrap(), BridgeOwner::None);
}
