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
//!    bridge with no key, is refused (401, counted by the host's web protection).
//! 2. **The replay window.** The timestamp must be within `webhookToleranceSeconds` (300) of now, and a signature seen
//!    before in that window is not worked again: a copy of one worked is answered 200 (`duplicate`), one still being
//!    worked on another worker 409 (so the provider tries again later). Neither counts as a refused signature: the
//!    signature is the provider's, and counting it could block the provider's own address. A signature is held from
//!    the moment its request is taken and let go when it was not worked (not 2xx), so the provider's retry of one that
//!    failed goes through. The cache is this node's; another node's copy is still deduped by the event's ids
//!    (`ingest#Event`'s transaction id, and `ChatEventExternal`).
//! 3. **The event.** `message.received` goes to `SmsServices.receive#Sms` (the pair room, then `ingest#Event`, stored
//!    before this answers); `message.sent` and `message.finalized` to `ChatBridgeServices.note#DeliveryReceipt`. Others are
//!    answered 200 and left. A failure of the server's own is 500, so the provider tries again.

use std::collections::{BTreeMap, HashMap, VecDeque};

use aws_lc_rs::signature::{UnparsedPublicKey, ED25519};
use base64::Engine;
use moqui_core::service::chat_bridge::{BridgeInfo, WebhookRequest, WebhookResponse};
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use crate::telnyx::{Config, TelnyxSms};

/// Signatures kept per bridge at most; the oldest go first.
const REPLAY_CAP: usize = 20_000;

/// What the replay cache knows of a signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replay {
    /// Not seen in the window: it is now held for this request.
    New,
    /// Taken by another request that is still being worked.
    Working,
    /// Its request was worked.
    Done,
}

/// The signatures of requests taken within the replay window, per bridge: (when, signature, worked).
#[derive(Default)]
pub struct ReplayCache {
    seen: HashMap<String, VecDeque<(i64, String, bool)>>,
}

impl ReplayCache {
    fn prune(&mut self, bridge: &str, now: i64, window: i64) -> &mut VecDeque<(i64, String, bool)> {
        let list = self.seen.entry(bridge.to_string()).or_default();
        while list.front().is_some_and(|(at, _, _)| now.saturating_sub(*at) > window) || list.len() > REPLAY_CAP {
            list.pop_front();
        }
        list
    }

    /// Whether `signature` was taken on bridge `bridge` within the window; when not, it is held from now (one check and
    /// hold under the caller's lock, so two copies at once are not both worked).
    pub fn take(&mut self, bridge: &str, signature: &str, now: i64, window: i64) -> Replay {
        let list = self.prune(bridge, now, window);
        match list.iter().find(|(_, known, _)| known == signature) {
            Some((_, _, true)) => Replay::Done,
            Some((_, _, false)) => Replay::Working,
            None => {
                list.push_back((now, signature.to_string(), false));
                Replay::New
            }
        }
    }

    /// The request of `signature` was worked (`true`: kept as done) or not (`false`: let go, so a retry goes through).
    pub fn finish(&mut self, bridge: &str, signature: &str, worked: bool) {
        let Some(list) = self.seen.get_mut(bridge) else { return };
        if worked {
            if let Some(entry) = list.iter_mut().find(|(_, known, _)| known == signature) {
                entry.2 = true;
            }
        } else {
            list.retain(|(_, known, _)| known != signature);
        }
    }
}

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

/// Work a webhook request of an SMS bridge. See the module notes.
pub(crate) fn handle(
    adapter: &TelnyxSms,
    bridge: &BridgeInfo,
    config: &Config,
    request: &WebhookRequest,
) -> WebhookResponse {
    if !request.method.eq_ignore_ascii_case("POST") {
        return WebhookResponse::text(405, "a webhook is a POST");
    }
    let Some(public_key) = config.public_key.as_deref() else {
        tracing::error!(
            "SMS bridge {}: a webhook came and the bridge has no publicKey to check it with",
            bridge.chat_bridge_id
        );
        return WebhookResponse::refused(401, "not signed");
    };
    let (Some(signature), Some(timestamp)) = (
        request.header("telnyx-signature-ed25519"),
        request.header("telnyx-timestamp"),
    ) else {
        return WebhookResponse::refused(401, "not signed");
    };
    let window = config.tolerance_seconds.saturating_mul(1000);
    let Some(stamp) = timestamp
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(|seconds| seconds.checked_mul(1000))
    else {
        return WebhookResponse::refused(401, "not signed");
    };
    if request.now_millis.abs_diff(stamp) > window.unsigned_abs() {
        return WebhookResponse::refused(401, "the signature's time is out of its window");
    }
    if !signature_valid(public_key, timestamp.trim(), &request.body, signature) {
        return WebhookResponse::refused(401, "the signature does not match");
    }
    let replay_key = signature.trim().to_string();
    let taken = lock(&adapter.replays).take(&bridge.chat_bridge_id, &replay_key, request.now_millis, window);
    match taken {
        Replay::Done => return WebhookResponse::json(200, &json!({"outcome": "duplicate"})),
        Replay::Working => return WebhookResponse::text(409, "this request is being worked"),
        Replay::New => {}
    }
    let response = match serde_json::from_slice::<Json>(&request.body) {
        Ok(event) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(bridge, config, &event)))
            .unwrap_or_else(|_| WebhookResponse::text(500, "the request failed")),
        Err(_) => WebhookResponse::text(400, "the body is not JSON"),
    };
    lock(&adapter.replays).finish(
        &bridge.chat_bridge_id,
        &replay_key,
        (200..300).contains(&response.status),
    );
    response
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
                tracing::error!(
                    "SMS bridge {}: text {message_id} was not taken: {}",
                    bridge.chat_bridge_id,
                    text("reasonCode")
                );
                return WebhookResponse::text(500, "the text was not taken");
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
            WebhookResponse::text(500, "the text was not taken")
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
            WebhookResponse::text(500, "the receipt was not kept")
        }
    }
}
