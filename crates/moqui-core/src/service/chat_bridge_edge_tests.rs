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
        url: "/channels/SMS1/webhook?attempt=1".to_string(),
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
    let item = webhook_item("SMS1", &key, &request(), None);
    assert_eq!(item.kind, KIND_WEBHOOK);
    assert_eq!(item.idempotency_key, "chatWebhook:SMS1:ev-1");
    assert_eq!(item.conversation_key, "chatWebhook:SMS1:+15557778888");
    let payload: serde_json::Value = serde_json::from_str(item.payload.as_deref().unwrap()).unwrap();
    let again = request_of(&payload).unwrap();
    assert_eq!(again.body, request().body, "the raw bytes, as signed");
    assert_eq!(again.method, "POST");
    assert_eq!(
        again.url, "/channels/SMS1/webhook?attempt=1",
        "the path and query it came to"
    );
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
    let item = webhook_item("SMS1", &long, &request(), None);
    assert_eq!(item.conversation_key, "chatWebhook:SMS1");
    assert!(item.idempotency_key.starts_with("chatWebhook:SMS1:sha256:"));
    assert!(item.idempotency_key.len() < 120);

    // a request the edge node did not verify: keyed by its signature too, so a forged body naming a real event id
    // cannot take that event's key; the same signature again is the same item
    let signed = webhook_item("SMS1", &key, &request(), Some("sig-a"));
    let forged = webhook_item("SMS1", &key, &request(), Some("sig-b"));
    assert!(
        signed.idempotency_key.starts_with("chatWebhook:SMS1:ev-1:sig:"),
        "{}",
        signed.idempotency_key
    );
    assert_ne!(signed.idempotency_key, forged.idempotency_key);
    assert_ne!(signed.idempotency_key, "chatWebhook:SMS1:ev-1");
    assert_eq!(
        webhook_item("SMS1", &key, &request(), Some("sig-a")).idempotency_key,
        signed.idempotency_key
    );
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

/// The signature header of [`TestAdapter`]'s network, which signs with a secret only the local node holds.
const DEFERRED_HEADER: &str = "x-test-signature";

/// A network signed `<secret> <public URL>` ([`crate::service::chat_bridge::webhook_public_url`]), so a request
/// queued without its URL, or checked over another, does not verify. `edge` says where it is verified; it counts the
/// checks and what it applies, and answers the edge node's 2xx as `<Response/>` (a provider that wants its own shape).
struct TestAdapter {
    name: &'static str,
    edge: bool,
    checks: std::sync::Mutex<u32>,
    applied: std::sync::Mutex<Vec<String>>,
}

impl TestAdapter {
    fn new(name: &'static str, edge: bool) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            name,
            edge,
            checks: std::sync::Mutex::new(0),
            applied: std::sync::Mutex::new(Vec::new()),
        })
    }
}

impl crate::service::chat_bridge::ChatBridgeAdapter for TestAdapter {
    fn name(&self) -> &str {
        self.name
    }

    fn capabilities(&self, _bridge: &BridgeInfo) -> crate::service::chat_bridge::Capabilities {
        crate::service::chat_bridge::Capabilities::default()
    }

    fn deliver(
        &self,
        _bridge: &BridgeInfo,
        _request: &DeliveryRequest,
    ) -> crate::service::chat_bridge::DeliveryOutcome {
        crate::service::chat_bridge::DeliveryOutcome::Uncertain {
            error: "not sent in this test".to_string(),
        }
    }

    fn webhook_verification(&self) -> WebhookVerification {
        if self.edge {
            WebhookVerification::Edge
        } else {
            WebhookVerification::Core {
                signature_header: DEFERRED_HEADER,
            }
        }
    }

    fn webhook_verifiable(&self, bridge: &BridgeInfo) -> Result<(), String> {
        match bridge.config["secret"].as_str() {
            Some(secret) if !secret.is_empty() => Ok(()),
            _ => Err("the bridge has no secret".to_string()),
        }
    }

    fn verify_webhook(&self, bridge: &BridgeInfo, request: &WebhookRequest) -> Option<WebhookResponse> {
        *self.checks.lock().unwrap() += 1;
        let secret = bridge.config["secret"].as_str().unwrap_or_default();
        let url = crate::service::chat_bridge::webhook_public_url(bridge, request).unwrap_or_default();
        let expected = format!("{secret} {url}");
        (secret.is_empty() || request.header(DEFERRED_HEADER) != Some(expected.as_str()))
            .then(WebhookResponse::not_signed)
    }

    fn webhook_event_id(&self, _bridge: &BridgeInfo, request: &WebhookRequest) -> Option<WebhookEventKey> {
        Some(WebhookEventKey {
            event_id: String::from_utf8_lossy(&request.body).to_string(),
            conversation: None,
        })
    }

    fn apply_webhook(&self, _bridge: &BridgeInfo, request: &WebhookRequest) -> WebhookResponse {
        self.applied
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(&request.body).to_string());
        WebhookResponse::json(200, &serde_json::json!({"outcome": "accepted"}))
    }

    fn edge_answer(&self, _bridge: &BridgeInfo, response: WebhookResponse) -> WebhookResponse {
        if (200..300).contains(&response.status) {
            WebhookResponse::text(200, "<Response/>")
        } else {
            response
        }
    }
}

/// The public base the tests' signatures are over (the value chat_bridge_web's test sets too).
const PUBLIC_BASE: &str = "https://example.test/";

fn deferred_request(signature: Option<&str>, body: &str, now_millis: i64) -> WebhookRequest {
    let mut headers = vec![(
        "content-type".to_string(),
        "application/x-www-form-urlencoded".to_string(),
    )];
    if let Some(signature) = signature {
        headers.push((DEFERRED_HEADER.to_string(), signature.to_string()));
    }
    WebhookRequest {
        method: "POST".to_string(),
        url: "/channels/TW1/webhook?kind=status".to_string(),
        headers,
        body: body.as_bytes().to_vec(),
        client_ip: "198.51.100.7".to_string(),
        now_millis,
    }
}

fn limits(waiting: i64) -> UnverifiedLimits {
    UnverifiedLimits {
        max_bytes: 64,
        per_minute: 3,
        per_client_minute: 2,
        waiting,
    }
}

/// An edge node cannot check the signature of a network that signs with a secret: it refuses a request that is too
/// large or has no signature header at all (as an unsigned one), more than a client address's share of the bridge's
/// rate and more than the bridge's rate (429, the network tries again), and queues the rest, unverified.
#[test]
fn an_edge_node_queues_an_unverifiable_webhook_only_after_its_cheap_checks() {
    let rates = EdgeRates::default();
    let now = 1_700_000_000_000;
    let limits = limits(10);
    let check = |request: &WebhookRequest, key: &str| deferred_checks(&rates, key, request, DEFERRED_HEADER, &limits);
    let from = |ip: &str, at: i64| {
        let mut request = deferred_request(Some("wrong"), "a", at);
        request.client_ip = ip.to_string();
        request
    };
    let large = deferred_request(Some("any"), &"x".repeat(65), now);
    assert_eq!(check(&large, "n:TW1").map(|answer| answer.status), Some(413));
    assert_eq!(
        check(&deferred_request(None, "a", now), "n:TW1"),
        Some(WebhookResponse::not_signed())
    );
    assert_eq!(
        check(&deferred_request(Some(" "), "a", now), "n:TW1"),
        Some(WebhookResponse::not_signed())
    );
    // a wrong signature is the local node's to find: it goes in the queue, within one address's share (2) ...
    assert_eq!(check(&from("192.0.2.1", now), "n:TW1"), None);
    assert_eq!(check(&from("192.0.2.1", now), "n:TW1"), None);
    let over = check(&from("192.0.2.1", now), "n:TW1").unwrap();
    assert_eq!(over.status, 429);
    assert!(!over.auth_failure, "the network's own rate is not a refused login");
    // ... which did not spend the bridge's (3): another address takes the last one, then the bridge is full
    assert_eq!(check(&from("192.0.2.2", now), "n:TW1"), None);
    assert_eq!(check(&from("192.0.2.3", now), "n:TW1").map(|a| a.status), Some(429));
    // another bridge has its own buckets; this one takes one again after a third of a minute (3 a minute)
    assert_eq!(check(&from("192.0.2.4", now), "n:OTHER"), None);
    assert_eq!(
        check(&from("192.0.2.4", now + 19_000), "n:TW1").map(|a| a.status),
        Some(429)
    );
    assert_eq!(check(&from("192.0.2.4", now + 20_000), "n:TW1"), None);
}

/// The edge node's whole path for a network it cannot verify: the cheap checks, then the item queued with its URL and
/// keyed by its signature, its answer in the network's shape, never a signature check; and a cap on what waits.
#[test]
fn an_edge_node_queues_an_unverified_webhook_without_checking_it() {
    let ctx = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><edge><queue-kind name="chatWebhook" service="org.moqui.chat.ChatBridgeServices.apply#EdgeWebhook"/></edge></moqui-conf>"#,
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let adapter = TestAdapter::new("core-edge-test", false);
    crate::service::chat_bridge::register_adapter(ctx.factory(), adapter.clone());
    // the node the items go to (on a node that takes no links, its own outbox's)
    let node = queue::local_node_id(ctx.factory());
    let fields =
        serde_json::json!({"typeEnumId": "CbtSms", "adapterName": "core-edge-test", "statusId": "CbsConnected"});
    let now = now_millis();
    let post = |signature: Option<&str>, body: &str| {
        paired_webhook_within(
            ctx.factory(),
            &facade,
            "TW1",
            &deferred_request(signature, body, now),
            &node,
            &fields,
            &UnverifiedLimits {
                per_minute: 10,
                per_client_minute: 10,
                ..limits(2)
            },
        )
    };
    assert_eq!(post(None, "ev-1"), WebhookResponse::not_signed());
    let queued = post(Some("forged"), "ev-1");
    assert_eq!(
        (queued.status, queued.body.as_slice()),
        (200, b"<Response/>".as_slice())
    );
    assert_eq!(*adapter.checks.lock().unwrap(), 0, "never checked on the edge node");
    let items = facade
        .find(queue::QUEUE_ITEM)
        .unwrap()
        .eq("kind", Value::string(KIND_WEBHOOK))
        .disable_authz()
        .list()
        .unwrap();
    let items = items.values();
    assert_eq!(items.len(), 1);
    let key = text_of(&items[0], "idempotencyKey").unwrap();
    assert!(key.starts_with("chatWebhook:TW1:ev-1:sig:"), "{key}");
    let payload: serde_json::Value = serde_json::from_str(&text_of(&items[0], "payload").unwrap()).unwrap();
    assert_eq!(payload["url"], "/channels/TW1/webhook?kind=status");
    // the same event under another signature is another item (the forged one cannot hold the real one's key) ...
    assert_eq!(post(Some("real"), "ev-1").status, 200);
    // ... and with two of the bridge's items waiting, the cap: refused until they go
    let full = post(Some("third"), "ev-2");
    assert_eq!(
        (full.status, full.body.as_slice()),
        (429, b"too many requests wait; try again later".as_slice())
    );
}

/// The local node checks every queued request's signature with the bridge's whole configuration, over the URL the
/// network posted to (`webhookUrl`, else `channels@public-url` and the path and query), whichever node checked it
/// before: a good one is applied; a bad one is refused, counted and kept (the item done, so it holds nothing up), and
/// applied by replay#RefusedWebhooks once the configuration is fixed; one it cannot check (no secret) waits. A bridge
/// whose adapter now verifies on the edge node is checked here too.
#[test]
fn a_queued_webhook_is_checked_on_the_local_node_and_a_bad_one_refused_and_kept() {
    moqui_lib::env::set_property("moqui_conf__channels__public_url", PUBLIC_BASE);
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let core = TestAdapter::new("core-test", false);
    let edge = TestAdapter::new("edge-test", true);
    crate::service::chat_bridge::register_adapter(ctx.factory(), core.clone());
    crate::service::chat_bridge::register_adapter(ctx.factory(), edge.clone());
    let create = |entity: &str, fields: &[(&str, &str)]| {
        let mut row = facade.make_value(entity).unwrap();
        for (name, value) in fields {
            row.set(*name, Value::string(*value));
        }
        row.create(&facade).unwrap();
    };
    for (id, adapter, config) in [
        ("TW1", "core-test", r#"{"secret": "s3cret"}"#),
        (
            "TW2",
            "core-test",
            r#"{"secret": "s3cret", "webhookUrl": "https://hooks.example/tw2"}"#,
        ),
        ("TW3", "core-test", r#"{}"#),
        ("TE1", "edge-test", r#"{"secret": "s3cret"}"#),
    ] {
        create(
            BRIDGE,
            &[
                ("chatBridgeId", id),
                ("typeEnumId", "CbtSms"),
                ("adapterName", adapter),
                ("statusId", "CbsConnected"),
                ("configJson", config),
            ],
        );
    }
    create(
        "moqui.security.UserAccount",
        &[
            ("userId", "EDGE_DELEGATE"),
            ("username", crate::edge::delegate::EDGE_USERNAME),
        ],
    );
    crate::user::internal_login_user(crate::edge::delegate::EDGE_USERNAME).unwrap();
    let apply = |bridge: &str, item: &str, request: &WebhookRequest| {
        create(queue::RECEIPT, &[("pairedNodeId", "edge-1"), ("queueItemId", item)]);
        let key = WebhookEventKey {
            event_id: item.to_string(),
            conversation: None,
        };
        let queued = webhook_item(bridge, &key, request, Some("sig"));
        let parameters = BTreeMap::from([
            ("queueItemId".to_string(), Value::string(item)),
            ("pairedNodeId".to_string(), Value::string("edge-1")),
            ("payload".to_string(), Value::string(queued.payload.unwrap())),
        ]);
        apply_edge_webhook(&parameters, &facade).unwrap()
    };
    let received = now_millis() - 3_600_000;
    let signed = |signature: &str, body: &str| deferred_request(Some(signature), body, received);
    // over channels@public-url and the path and query as received
    let good = signed("s3cret https://example.test/channels/TW1/webhook?kind=status", "good");
    assert!(apply("TW1", "item-1", &good).is_empty(), "applied");
    // a bridge's webhookUrl, as configured, wins
    let configured = signed("s3cret https://hooks.example/tw2", "configured");
    assert!(apply("TW2", "item-2", &configured).is_empty(), "applied");
    assert_eq!(
        *core.applied.lock().unwrap(),
        vec!["good".to_string(), "configured".to_string()]
    );

    // signed over another URL, or with a wrong secret: refused, kept, not applied, the item done
    let other_url = signed("s3cret https://example.test/channels/TW1/webhook", "moved");
    let wrong = signed("guess https://example.test/channels/TW1/webhook?kind=status", "forged");
    for (item, request) in [("item-3", &other_url), ("item-4", &wrong)] {
        let answer = apply("TW1", item, request);
        assert!(answer.contains_key("refused"), "{answer:?}");
        assert!(!answer.contains_key("retryLater"));
    }
    assert_eq!(core.applied.lock().unwrap().len(), 2, "nothing refused was applied");
    assert_eq!(refused_webhooks(ctx.factory(), "TW1"), 2);
    let kept = |bridge: &str| {
        facade
            .find(REFUSED)
            .unwrap()
            .eq("chatBridgeId", Value::string(bridge))
            .disable_authz()
            .count()
            .unwrap()
    };
    assert_eq!(kept("TW1"), 2);

    // a bridge with no secret here cannot check: the item waits, nothing kept as refused
    let unchecked = apply("TW3", "item-5", &signed("anything", "unchecked"));
    assert!(
        unchecked
            .get("retryLater")
            .is_some_and(|why| moqui_lib::object::to_plain_string(why).contains("TW3")),
        "{unchecked:?}"
    );
    assert_eq!(kept("TW3"), 0);

    // an adapter that verifies on the edge node is checked here as well: a request queued unverified (its adapter changed
    // since) is refused when it is not the network's
    let edge_bad = deferred_request(Some("guess"), "edge-forged", received);
    assert!(apply("TE1", "item-6", &edge_bad).contains_key("refused"));
    assert!(edge.applied.lock().unwrap().is_empty());

    // the secret fixed (it was "guess"): the replay applies the one now signed right, keeps the other
    {
        let mut row = facade
            .find(BRIDGE)
            .unwrap()
            .eq("chatBridgeId", Value::string("TW1"))
            .disable_authz()
            .one()
            .unwrap()
            .unwrap();
        row.set("configJson", Value::string(r#"{"secret": "guess"}"#));
        row.update(&facade).unwrap();
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let replayed = replay_refused_webhooks(
        &BTreeMap::from([("chatBridgeId".to_string(), Value::string("TW1"))]),
        &facade,
    )
    .unwrap();
    let count = |name: &str| moqui_lib::object::to_plain_string(&replayed[name]);
    assert_eq!(
        (count("appliedCount"), count("refusedCount")),
        ("1".to_string(), "1".to_string())
    );
    assert_eq!(core.applied.lock().unwrap().last().map(String::as_str), Some("forged"));
    assert_eq!(kept("TW1"), 1);
}
