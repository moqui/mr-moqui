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
    pair(&facade, "local-a");
    replicate_bridge(&facade, "local-a", "LOCAL_BRIDGE");
    let plain = request();
    assert!(matches!(
        bridge_owner(&facade, "LOCAL_BRIDGE", false, &plain).unwrap(),
        BridgeOwner::Paired(node, _) if node == "local-a"
    ));
    assert_eq!(
        bridge_owner(&facade, "LOCAL_BRIDGE", true, &plain).unwrap(),
        BridgeOwner::Both("local-a".to_string())
    );
    assert_eq!(
        bridge_owner(&facade, "OWN_BRIDGE", true, &plain).unwrap(),
        BridgeOwner::Own
    );
    assert_eq!(bridge_owner(&facade, "NONE", false, &plain).unwrap(), BridgeOwner::None);
}

fn pair(facade: &EntityFacade, node: &str) {
    let mut client = facade.make_value("moqui.edge.EdgeClient").unwrap();
    client.set("pairedNodeId", Value::string(node));
    client.set("keyFingerprint", Value::string("ab"));
    client.set("statusId", Value::string("EdgeClientActive"));
    client.create(facade).unwrap();
}

fn replicate_bridge(facade: &EntityFacade, node: &str, bridge: &str) {
    let batch = crate::edge::replica::Batch {
        sender_token: 0,
        set_name: REPLICA_SET.to_string(),
        epoch: "e1".to_string(),
        base_seq: 0,
        seq: 1,
        snapshot: true,
        snapshot_end: true,
        changes: vec![crate::edge::replica::Change {
            entity_name: BRIDGE.to_string(),
            pk_json: format!(r#"{{"chatBridgeId":"{bridge}"}}"#),
            fields_json: Some(format!(r#"{{"typeEnumId":"CbtSms","adapterName":"{node}"}}"#)),
        }],
    };
    crate::edge::replica::apply_batch(facade, node, &batch).unwrap();
}

fn host(name: &str) -> WebhookRequest {
    let mut request = request();
    request.headers.push(("host".to_string(), name.to_string()));
    request
}

/// E16 (plans/Edge-Node2.md §4.1): two pairings with the same bridge id, from their separate databases. The request's
/// host name picks the pairing first: at a pairing's own host its copy alone; at a name of no pairing the id is
/// ambiguous and refused. An id one pairing has is found at any name.
#[test]
fn a_bridge_id_two_pairings_have_goes_by_the_requests_host_name() {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    for node in ["local-a", "local-b"] {
        pair(&facade, node);
        replicate_bridge(&facade, node, "100000");
    }
    replicate_bridge(&facade, "local-b", "ONLY_B");
    for (name, node) in [("a.example", "local-a"), ("b.example", "local-b")] {
        let mut row = facade.make_value("moqui.edge.EdgeHost").unwrap();
        row.set("hostName", Value::string(name));
        row.set("pairedNodeId", Value::string(node));
        row.set("modeEnumId", Value::string(crate::edge::HOST_TERMINATE));
        row.create(&facade).unwrap();
    }
    let at = |name: &str, bridge: &str| {
        replicated_bridge(&facade, bridge, host_pairing(&facade, &host(name)).unwrap().as_deref())
            .unwrap()
            .map(|(node, _)| node)
    };
    assert_eq!(at("A.example:443", "100000").as_deref(), Some("local-a"));
    assert_eq!(at("b.example.", "100000").as_deref(), Some("local-b"));
    assert_eq!(
        at("edge.example", "100000"),
        None,
        "ambiguous at the edge node's own name"
    );
    assert_eq!(at("edge.example", "ONLY_B").as_deref(), Some("local-b"));
    assert_eq!(
        at("a.example", "ONLY_B"),
        None,
        "never another pairing's at a pairing's name"
    );
    // on an all-in-one node too: the pairing's name goes to its copy, the server's own row aside
    assert!(matches!(
        bridge_owner(&facade, "100000", true, &host("b.example")).unwrap(),
        BridgeOwner::Paired(node, _) if node == "local-b"
    ));
}
