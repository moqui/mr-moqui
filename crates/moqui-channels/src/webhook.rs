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

//! A Telnyx webhook (`/channels/{chatBridgeId}/webhook`): checked before anything is parsed, then worked.
//!
//! 1. **The signature.** `telnyx-signature-ed25519` is the base64 Ed25519 signature of `{telnyx-timestamp}|{raw body}`
//!    under the account's public key (the bridge's `publicKey`), checked with aws-lc-rs. A missing or wrong signature, or a
//!    bridge with no key, is refused with the one answer an unknown bridge gets too
//!    ([`WebhookResponse::not_signed`]: 401, counted by the host's web protection). A disabled bridge checks the same
//!    ([`verify`]) before it answers its network 503.
//! 2. **The replay window.** The timestamp must be within `webhookToleranceSeconds` (300, at most half the
//!    time the replay cache holds a signature, so 150 under the default 300 s cache) of now, and a signature seen before in that window is not worked again: a copy of one worked is
//!    answered 200 (`duplicate`), one still being worked on another worker 409 (so the provider tries again later).
//!    Neither counts as a refused signature: the signature is the provider's, and counting it could block the
//!    provider's own address. A signature is held from the moment its request is taken and let go when it was not
//!    worked (not 2xx), so the provider's retry of one that failed goes through. The cache is the distributed one the
//!    system-message HMAC check uses ([`moqui_core::service::chat_bridge::replay_take`]), so a copy that reaches
//!    another node is caught there as far as that cache reaches (it is the node's own until a distributed cache is
//!    configured); the event's ids still dedupe on any node (`ingest#Event`'s transaction id, and
//!    `ChatEventExternal`).
//! 3. **The event.** `message.received` goes to `SmsServices.receive#Sms` (the pair room, then `ingest#Event`, stored
//!    before this answers); `message.sent` and `message.finalized` to `ChatBridgeServices.note#DeliveryReceipt`. Others are
//!    answered 200 and left. A failure of the server's own (a service error, a database away) is 503, so the provider
//!    (or an edge node's queue) tries again; a text `ingest#Event` refuses for what it is (an unknown bridge, a bad
//!    transaction id, a batch too large) is 422.
//!
//! Through an edge node (plans/Edge-Node.md §5.4) the steps split: the edge node checks the signature ([`verify`]) and
//! queues the request by its event id ([`event_key`]), which drops a repeat; the local node works the event
//! ([`apply`]) without checking again, since the window has passed by then.

use std::collections::BTreeMap;

use aws_lc_rs::signature::{UnparsedPublicKey, ED25519};
use base64::Engine;
use moqui_core::service::chat_bridge::{
    replay_finish, replay_take, replay_window_ms, BridgeInfo, Replay, WebhookEventKey, WebhookRequest, WebhookResponse,
};
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};

use crate::telnyx::Config;

/// Whether `signature` (base64) is the Ed25519 signature of `timestamp|body` under `public_key` (base64, 32 bytes).
pub fn signature_valid(public_key: &str, timestamp: &str, body: &[u8], signature: &str) -> bool {
    let engine = base64::engine::general_purpose::STANDARD;
    let (Ok(key), Ok(signature)) = (engine.decode(public_key.trim()), engine.decode(signature.trim())) else {
        return false;
    };
    if key.len() != 32 || signature.len() != 64 {
        return false;
    }
    let mut message = Vec::with_capacity(timestamp.len() + 1 + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.push(b'|');
    message.extend_from_slice(body);
    UnparsedPublicKey::new(&ED25519, &key)
        .verify(&message, &signature)
        .is_ok()
}

/// The signature step: None when the request is the provider's (signed under the bridge's key, within the window),
/// else the refusal, the same whatever was wrong ([`WebhookResponse::not_signed`]).
pub(crate) fn verify(bridge: &BridgeInfo, config: &Config, request: &WebhookRequest) -> Option<WebhookResponse> {
    let Some(public_key) = config.public_key.as_deref() else {
        tracing::error!(
            "SMS bridge {}: a webhook came and the bridge has no publicKey to check it with",
            bridge.chat_bridge_id
        );
        return Some(WebhookResponse::not_signed());
    };
    let (Some(signature), Some(timestamp)) = (
        request.header("telnyx-signature-ed25519"),
        request.header("telnyx-timestamp"),
    ) else {
        return Some(WebhookResponse::not_signed());
    };
    let window = replay_window_ms(config.tolerance_seconds.saturating_mul(1000));
    let Some(stamp) = timestamp
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(|seconds| seconds.checked_mul(1000))
    else {
        return Some(WebhookResponse::not_signed());
    };
    if request.now_millis.abs_diff(stamp) > window.unsigned_abs()
        || !signature_valid(public_key, timestamp.trim(), &request.body, signature)
    {
        return Some(WebhookResponse::not_signed());
    }
    None
}

/// Work a webhook request of an SMS bridge. See the module notes.
pub(crate) fn handle(bridge: &BridgeInfo, config: &Config, request: &WebhookRequest) -> WebhookResponse {
    if !request.method.eq_ignore_ascii_case("POST") {
        return WebhookResponse::text(405, "a webhook is a POST");
    }
    if let Some(refused) = verify(bridge, config, request) {
        return refused;
    }
    let signature = request.header("telnyx-signature-ed25519").unwrap_or_default();
    match replay_take(&bridge.chat_bridge_id, signature) {
        Ok(Replay::Done) => return WebhookResponse::json(200, &json!({"outcome": "duplicate"})),
        Ok(Replay::Working) => return WebhookResponse::text(409, "this request is being worked"),
        Ok(Replay::New) => {}
        Err(error) => {
            // strict: a request that cannot be checked for a replay is not worked; the provider tries again
            tracing::error!(
                "SMS bridge {}: a webhook could not be checked for a replay: {error}",
                bridge.chat_bridge_id
            );
            return WebhookResponse::text(503, "try again later");
        }
    }
    let response = apply(bridge, config, request);
    replay_finish(&bridge.chat_bridge_id, signature, (200..300).contains(&response.status));
    response
}

/// The event step of [`handle`] alone, after the signature and replay checks: on any node after them, and on a local
/// node for a request its edge node checked and queued (plans/Edge-Node.md §5.4).
pub(crate) fn apply(bridge: &BridgeInfo, config: &Config, request: &WebhookRequest) -> WebhookResponse {
    match serde_json::from_slice::<Json>(&request.body) {
        Ok(event) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(bridge, config, &event)))
            .unwrap_or_else(|_| WebhookResponse::text(500, "the request failed")),
        Err(_) => WebhookResponse::text(400, "the body is not JSON"),
    }
}

/// The reason codes of `ingest#Event` that refuse a text for what it is; any other failure is the server's own.
const REFUSALS: &[&str] = &[
    "unknownBridge",
    "badToken",
    "badTransaction",
    "batchTooLarge",
    "localIdentity",
];

/// The event a checked request carries, for an edge node's queue: Telnyx's event id (`/data/id`; when it has none, a
/// hash of the body, which a provider's retry sends unchanged, so a copy is still one item), ordered by their number (the sender of a text, the recipient of a receipt).
/// None when the body is not JSON.
pub(crate) fn event_key(request: &WebhookRequest) -> Option<WebhookEventKey> {
    let event: Json = serde_json::from_slice(&request.body).ok()?;
    let event_id = str_at(&event, "/data/id").map(str::to_string).unwrap_or_else(|| {
        let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &request.body);
        format!("sha256:{}", moqui_lib::codec::hex_encode(digest.as_ref()))
    });
    let conversation = match str_at(&event, "/data/event_type").unwrap_or_default() {
        "message.received" => str_at(&event, "/data/payload/from/phone_number"),
        _ => str_at(&event, "/data/payload/to/0/phone_number"),
    };
    Some(WebhookEventKey {
        event_id,
        conversation: conversation.map(str::to_string),
    })
}

fn str_at<'a>(value: &'a Json, pointer: &str) -> Option<&'a str> {
    value
        .pointer(pointer)
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
}

fn millis_of(text: Option<&str>) -> Option<i64> {
    text?
        .parse::<jiff::Timestamp>()
        .ok()
        .map(|stamp| stamp.as_millisecond())
}

/// A checked event, worked.
fn work(bridge: &BridgeInfo, config: &Config, event: &Json) -> WebhookResponse {
    let kind = str_at(event, "/data/event_type").unwrap_or_default();
    let Some(payload) = event.pointer("/data/payload") else {
        return WebhookResponse::json(200, &json!({"ignored": "no payload"}));
    };
    match kind {
        "message.received" => received(bridge, config, event, payload),
        "message.sent" | "message.finalized" => receipt(bridge, event, payload),
        _ => WebhookResponse::json(200, &json!({"ignored": kind})),
    }
}

fn received(bridge: &BridgeInfo, config: &Config, event: &Json, payload: &Json) -> WebhookResponse {
    let (Some(event_id), Some(message_id), Some(ours), Some(theirs)) = (
        str_at(event, "/data/id"),
        str_at(payload, "/id"),
        str_at(payload, "/to/0/phone_number"),
        str_at(payload, "/from/phone_number"),
    ) else {
        return WebhookResponse::text(400, "a text names its event, message, and both numbers");
    };
    let media: Vec<Value> = payload
        .get("media")
        .and_then(Json::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|file| {
                    let url = str_at(file, "/url")?;
                    Some(moqui_lib::json::from_json(&json!({
                        "url": url,
                        "contentType": str_at(file, "/content_type"),
                        "maxBytes": config.max_inbound_media_bytes,
                    })))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("eventId".to_string(), Value::string(event_id)),
        ("messageId".to_string(), Value::string(message_id)),
        ("ourNumber".to_string(), Value::string(ours)),
        ("theirNumber".to_string(), Value::string(theirs)),
        ("mediaList".to_string(), Value::List(media)),
    ]);
    if let Some(text) = payload.get("text").and_then(Json::as_str) {
        parameters.insert("text".to_string(), Value::string(text));
    }
    if let Some(at) = millis_of(str_at(payload, "/received_at").or_else(|| str_at(event, "/data/occurred_at"))) {
        parameters.insert("originMillis".to_string(), Value::I64(at));
    }
    match crate::call("org.moqui.impl.SmsServices.receive#Sms", parameters) {
        Ok(out) => {
            let text = |key: &str| out.get(key).map(to_plain_string).unwrap_or_default();
            let outcome = text("outcome");
            if outcome == "failed" {
                let reason = text("reasonCode");
                tracing::error!(
                    "SMS bridge {}: text {message_id} was not taken: {reason}",
                    bridge.chat_bridge_id
                );
                // refused for what it is: another try changes nothing; anything else is the server's own
                return if REFUSALS.contains(&reason.as_str()) {
                    WebhookResponse::text(422, &format!("the text was refused: {reason}"))
                } else {
                    WebhookResponse::text(503, "the text was not taken; try again later")
                };
            }
            if outcome == "ignored" {
                tracing::warn!(
                    "SMS bridge {}: a text to {ours} was left: {}",
                    bridge.chat_bridge_id,
                    text("reasonCode")
                );
            }
            WebhookResponse::json(
                200,
                &json!({"outcome": outcome, "reasonCode": text("reasonCode"), "message": text("message")}),
            )
        }
        Err(error) => {
            tracing::error!(
                "SMS bridge {}: text {message_id} was not taken: {error}",
                bridge.chat_bridge_id
            );
            WebhookResponse::text(503, "the text was not taken; try again later")
        }
    }
}

/// A delivery receipt: the message's state at its (first) recipient.
fn receipt(bridge: &BridgeInfo, event: &Json, payload: &Json) -> WebhookResponse {
    let Some(message_id) = str_at(payload, "/id") else {
        return WebhookResponse::text(400, "a receipt names its message");
    };
    let status = match str_at(payload, "/to/0/status").unwrap_or_default() {
        "sent" => "sent",
        "delivered" => "delivered",
        "delivery_unconfirmed" => "unconfirmed",
        "sending_failed" | "delivery_failed" => "failed",
        other => return WebhookResponse::json(200, &json!({"ignored": other})),
    };
    let mut parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("remoteEventId".to_string(), Value::string(message_id)),
        ("status".to_string(), Value::string(status)),
    ]);
    let error = str_at(payload, "/errors/0/detail").or_else(|| str_at(payload, "/errors/0/title"));
    if let Some(error) = error {
        parameters.insert("error".to_string(), Value::string(error));
    }
    if let Some(at) = millis_of(str_at(event, "/data/occurred_at")) {
        if let Ok(stamp) = jiff::Timestamp::from_millisecond(at) {
            parameters.insert("receiptDate".to_string(), Value::Timestamp(stamp));
        }
    }
    match crate::call("org.moqui.impl.ChatBridgeServices.note#DeliveryReceipt", parameters) {
        Ok(out) => WebhookResponse::json(
            200,
            &json!({"found": out.get("found") == Some(&Value::Bool(true)), "status": status}),
        ),
        Err(error) => {
            tracing::error!(
                "SMS bridge {}: the receipt of {message_id} was not kept: {error}",
                bridge.chat_bridge_id
            );
            WebhookResponse::text(503, "the receipt was not kept; try again later")
        }
    }
}
