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

//! SMS and MMS on the Telnyx Messaging API v2: the `sms` adapter's provider (docs/interfaces/SMS.md, the provider
//! profile there). A `ChatBridge` of type `CbtSms` names it (`adapterName` `sms`, or none); its `configJson` (kept
//! encrypted) has, besides the neutral settings of [`crate::sms::Settings`]:
//!
//! | Key | What |
//! | --- | --- |
//! | `apiKey` | The API key (`Authorization: Bearer`), required |
//! | `publicKey` | The account's webhook public key (base64 Ed25519), required for the webhook |
//! | `messagingProfileId` | Sent with each message to one number when set |
//! | `campaignId` | The 10DLC campaign of the numbers (kept for the record; the registration is an operator's step) |
//!
//! Its defaults: `apiBaseUrl` `https://api.telnyx.com`, `maxTextLength` 1600 characters (Telnyx splits a long text into
//! parts), `maxMediaCount` 10 and `maxMediaBytes` 1 MiB in all (an MMS's limits), `maxInboundMediaBytes` 5 MiB.
//!
//! - **Out.** One number: `POST /v2/messages`; a group text: `POST /v2/messages/group_mms` (our number, and at most 8
//!   others: [`GROUP_MAX`]), whose receipts name its `group_message_id`. Each send carries the delivery's idempotency key
//!   in an `Idempotency-Key` header, but Telnyx does not dedupe on it. Its error codes: 40300 (a STOP) `optedOut`; 40310,
//!   40311, 40012, 40013 `badNumber`; 40305 to 40307 `notRegistered`; any other `providerRefused`.
//! - **In.** `telnyx-signature-ed25519` is the base64 Ed25519 signature of `{telnyx-timestamp}|{raw body}` under the
//!   bridge's `publicKey`, checked with aws-lc-rs, the timestamp within `webhookToleranceSeconds` (300, at most half the
//!   time the replay cache holds a signature). A bridge with no key refuses every webhook. The public key is in the
//!   bridge's replicated copy, so an edge node checks it. `message.received` is a text (a group MMS's numbers in `to`
//!   and `cc`), `message.sent` and `message.finalized` a receipt; others are left.
//!
//! Its encryption is `None` (plans/Comms-Messaging.md §6.3): the API is reached over TLS, but Telnyx, the carriers and
//! the phone network read every text.

use aws_lc_rs::signature::{UnparsedPublicKey, ED25519};
use base64::Engine;
use moqui_core::service::chat_bridge::{
    replay_window_ms, BridgeInfo, WebhookEventKey, WebhookRequest, WebhookResponse,
};
use moqui_lib::http::Method;
use serde_json::{json, Value as Json};

use crate::sms::{
    text_of, ApiBody, ApiRequest, Defaults, Inbound, InboundMedia, Outgoing, ProviderError, Receipt, ReceiptStatus,
    Refusal, Settings, SmsEvent, SmsProvider,
};

/// The most outside numbers of a group text (Telnyx's group MMS: our number, the sender, and eight recipients).
pub const GROUP_MAX: usize = 8;
/// The header Telnyx signs a webhook in.
const SIGNATURE: &str = "telnyx-signature-ed25519";
const TIMESTAMP: &str = "telnyx-timestamp";
/// The most recipients of a text that came in that are read: the server finds ours among them and keeps the sender and
/// at most a group's others (receive#Sms), so this bounds only the input.
const RECIPIENTS_READ: usize = 32;

/// A Telnyx bridge's own settings, from its `configJson`.
#[derive(Clone, Debug)]
pub struct Config {
    pub api_key: String,
    pub public_key: Option<String>,
    pub messaging_profile_id: Option<String>,
    pub campaign_id: Option<String>,
}

/// The Telnyx provider of the `sms` adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct Telnyx;

impl SmsProvider for Telnyx {
    type Config = Config;

    fn name(&self) -> &'static str {
        "Telnyx"
    }

    fn adapter_name(&self) -> &'static str {
        "sms"
    }

    fn defaults(&self) -> Defaults {
        Defaults {
            api_base_url: "https://api.telnyx.com",
            max_text_length: 1600,
            max_media_count: 10,
            max_media_bytes: 1024 * 1024,
            max_inbound_media_bytes: 5 * 1024 * 1024,
        }
    }

    fn config(&self, config: &Json) -> Config {
        Config {
            api_key: text_of(config, "apiKey").unwrap_or_default(),
            public_key: text_of(config, "publicKey"),
            messaging_profile_id: text_of(config, "messagingProfileId"),
            campaign_id: text_of(config, "campaignId"),
        }
    }

    fn check(&self, config: &Config) -> Result<(), String> {
        if config.api_key.is_empty() {
            return Err("The SMS bridge has no apiKey in its configuration".to_string());
        }
        Ok(())
    }

    fn group_max(&self) -> usize {
        GROUP_MAX
    }

    fn request(&self, config: &Config, settings: &Settings, out: &Outgoing<'_>) -> ApiRequest {
        let group = out.group();
        let mut body = if group {
            json!({"from": out.ours, "to": out.theirs})
        } else {
            json!({"from": out.ours, "to": out.theirs.first().cloned().unwrap_or_default()})
        };
        if let Some(text) = out.text {
            body["text"] = Json::String(text.to_string());
        }
        if !out.media_urls.is_empty() {
            body["media_urls"] = json!(out.media_urls);
        }
        // a group MMS goes from the number's own profile: the group endpoint takes no messaging_profile_id
        if let (Some(profile), false) = (&config.messaging_profile_id, group) {
            body["messaging_profile_id"] = Json::String(profile.clone());
        }
        let path = if group {
            "/v2/messages/group_mms"
        } else {
            "/v2/messages"
        };
        ApiRequest {
            method: Method::Post,
            url: format!("{}{path}", settings.api_base_url),
            headers: vec![
                ("authorization".to_string(), format!("Bearer {}", config.api_key)),
                ("idempotency-key".to_string(), out.request.idempotency_key.clone()),
            ],
            body: ApiBody::Json(body),
        }
    }

    fn accepted_id(&self, out: &Outgoing<'_>, body: &str) -> Option<String> {
        let parsed: Json = serde_json::from_str(body).unwrap_or(Json::Null);
        // a group MMS: its receipts, one a recipient, name it by its group_message_id
        parsed
            .pointer("/data/group_message_id")
            .and_then(Json::as_str)
            .filter(|id| out.group() && !id.is_empty())
            .or_else(|| parsed.pointer("/data/id").and_then(Json::as_str))
            .map(str::to_string)
    }

    /// A Telnyx error's code and its text, from an error answer's `errors`.
    fn error_of(&self, body: &str) -> ProviderError {
        let parsed: Json = serde_json::from_str(body).unwrap_or(Json::Null);
        let first = parsed.get("errors").and_then(|errors| errors.get(0));
        let code = first.and_then(|error| error.get("code")).and_then(|code| {
            code.as_str()
                .map(str::to_string)
                .or_else(|| code.as_i64().map(|n| n.to_string()))
        });
        let text = first
            .and_then(|error| error.get("detail").or_else(|| error.get("title")))
            .and_then(Json::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| body.chars().take(300).collect());
        ProviderError { code, text }
    }

    /// The reason code of a send Telnyx refused (4xx), by its error code.
    fn refusal(&self, _status: u16, error: &ProviderError) -> Refusal {
        Refusal::Failed(match error.code.as_deref() {
            // blocked by a STOP the recipient sent
            Some("40300") => "optedOut",
            Some("40310" | "40311" | "40012" | "40013") => "badNumber",
            Some("40305" | "40306" | "40307") => "notRegistered",
            _ => "providerRefused",
        })
    }

    fn signature_header(&self) -> &'static str {
        SIGNATURE
    }

    fn verifies_on_edge(&self) -> bool {
        true
    }

    fn can_verify(&self, config: &Config) -> Result<(), String> {
        match config.public_key {
            Some(_) => Ok(()),
            None => Err("the bridge has no publicKey in its configuration".to_string()),
        }
    }

    /// The signature under the bridge's key, within the window.
    fn verify(&self, bridge: &BridgeInfo, config: &Config, settings: &Settings, request: &WebhookRequest) -> bool {
        let Some(public_key) = config.public_key.as_deref() else {
            tracing::error!(
                "SMS bridge {}: a webhook came and the bridge has no publicKey to check it with",
                bridge.chat_bridge_id
            );
            return false;
        };
        let (Some(signature), Some(timestamp)) = (request.header(SIGNATURE), request.header(TIMESTAMP)) else {
            return false;
        };
        let window = replay_window_ms(settings.tolerance_seconds.saturating_mul(1000));
        let Some(stamp) = timestamp
            .trim()
            .parse::<i64>()
            .ok()
            .and_then(|seconds| seconds.checked_mul(1000))
        else {
            return false;
        };
        request.now_millis.abs_diff(stamp) <= window.unsigned_abs()
            && signature_valid(public_key, timestamp.trim(), &request.body, signature)
    }

    /// Telnyx's event id (`/data/id`; when it has none, a hash of the body, which a provider's retry sends unchanged, so
    /// a copy is still one item), ordered by their number (the sender of a text, the recipient of a receipt).
    fn event_key(&self, request: &WebhookRequest) -> Option<WebhookEventKey> {
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

    fn parse(&self, request: &WebhookRequest) -> Result<SmsEvent, WebhookResponse> {
        let Ok(event) = serde_json::from_slice::<Json>(&request.body) else {
            return Err(WebhookResponse::text(400, "the body is not JSON"));
        };
        let kind = str_at(&event, "/data/event_type").unwrap_or_default();
        let Some(payload) = event.pointer("/data/payload") else {
            return Ok(SmsEvent::Ignored("no payload".to_string()));
        };
        match kind {
            "message.received" => received(&event, payload),
            "message.sent" | "message.finalized" => receipt(&event, payload),
            _ => Ok(SmsEvent::Ignored(kind.to_string())),
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

/// The numbers a text that came in went to: `to` and, for a group text, `cc`, each once.
pub fn recipients_of(payload: &Json) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for list in ["to", "cc"] {
        for entry in payload.get(list).and_then(Json::as_array).into_iter().flatten() {
            let number = entry
                .get("phone_number")
                .and_then(Json::as_str)
                .or_else(|| entry.as_str());
            if let Some(number) = number.filter(|number| !number.is_empty() && number.len() <= 20) {
                if !out.iter().any(|known| known == number) {
                    out.push(number.to_string());
                }
            }
        }
    }
    out.truncate(RECIPIENTS_READ);
    out
}

fn received(event: &Json, payload: &Json) -> Result<SmsEvent, WebhookResponse> {
    let (Some(event_id), Some(message_id), Some(ours), Some(theirs)) = (
        str_at(event, "/data/id"),
        str_at(payload, "/id"),
        str_at(payload, "/to/0/phone_number"),
        str_at(payload, "/from/phone_number"),
    ) else {
        return Err(WebhookResponse::text(
            400,
            "a text names its event, message, and both numbers",
        ));
    };
    let media = payload
        .get("media")
        .and_then(Json::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|file| {
                    Some(InboundMedia {
                        url: str_at(file, "/url")?.to_string(),
                        content_type: str_at(file, "/content_type").map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(SmsEvent::Inbound(Inbound {
        event_id: event_id.to_string(),
        message_id: message_id.to_string(),
        our_number: ours.to_string(),
        their_number: theirs.to_string(),
        text: payload.get("text").and_then(Json::as_str).map(str::to_string),
        media,
        // a group text (group MMS) went to several numbers: every one of them, `to` and `cc`, our number among them
        recipients: recipients_of(payload),
        origin_millis: millis_of(str_at(payload, "/received_at").or_else(|| str_at(event, "/data/occurred_at"))),
        // a group MMS is its numbers, not a conversation of Telnyx's
        thread_id: None,
    }))
}

/// A delivery receipt: the message's state at its recipient. A group MMS has one a recipient, each naming the group's
/// message (`group_message_id`, the delivery's remote id) and its one recipient.
fn receipt(event: &Json, payload: &Json) -> Result<SmsEvent, WebhookResponse> {
    let Some(own_id) = str_at(payload, "/id") else {
        return Err(WebhookResponse::text(400, "a receipt names its message"));
    };
    let group_id = str_at(payload, "/group_message_id");
    let status = match str_at(payload, "/to/0/status").unwrap_or_default() {
        "sent" => ReceiptStatus::Sent,
        "delivered" => ReceiptStatus::Delivered,
        "delivery_unconfirmed" => ReceiptStatus::Unconfirmed,
        "sending_failed" | "delivery_failed" => ReceiptStatus::Failed,
        other => return Ok(SmsEvent::Ignored(other.to_string())),
    };
    let error = str_at(payload, "/errors/0/detail").or_else(|| str_at(payload, "/errors/0/title"));
    Ok(SmsEvent::Receipt(Receipt {
        remote_event_id: group_id.unwrap_or(own_id).to_string(),
        status,
        error: error.map(str::to_string),
        recipient_number: group_id
            .and_then(|_| str_at(payload, "/to/0/phone_number"))
            .map(str::to_string),
        other_remote_event_id: group_id.map(|_| own_id.to_string()),
        receipt_millis: millis_of(str_at(event, "/data/occurred_at")),
        opt_signal: None,
    }))
}
