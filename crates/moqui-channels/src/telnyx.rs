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

//! SMS and MMS on the Telnyx Messaging API v2: the `sms` chat bridge adapter (docs/interfaces/SMS.md, the provider
//! profile there). A `ChatBridge` of type `CbtSms` names it; its `configJson` (kept encrypted) has:
//!
//! | Key | What |
//! | --- | --- |
//! | `apiKey` | The API key (`Authorization: Bearer`), required |
//! | `publicKey` | The account's webhook public key (base64 Ed25519), required for the webhook |
//! | `messagingProfileId` | Sent with each message when set |
//! | `campaignId` | The 10DLC campaign of the numbers (kept for the record; the registration is an operator's step) |
//! | `apiBaseUrl` | `https://api.telnyx.com` by default |
//! | `maxTextLength` | 1600 characters (Telnyx splits a long text into parts) |
//! | `maxMediaCount`, `maxMediaBytes` | 10 files and 1 MiB in all, an MMS's limits |
//! | `maxInboundMediaBytes` | 5 MiB, the largest file taken from a text that comes in |
//! | `webhookToleranceSeconds` | 300, how far a webhook's timestamp may be from now |
//! | `sendsPerMinute` | 60 (the bridge's own `sendsPerMinute` wins) |
//! | `allowInternalAddresses` | false; true lets the API and file addresses be internal (a test's fake Telnyx) |
//!
//! Each send carries the delivery's idempotency key in an `Idempotency-Key` header, but Telnyx does not dedupe on it,
//! so the adapter says it is not idempotent: an attempt whose answer was lost (a timeout, a reset after the request
//! went, a 5xx other than 503) is `Uncertain` and is not sent again unless someone asks (`retry#Delivery`).

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use moqui_core::service::chat_bridge::{
    media_url, BridgeInfo, Capabilities, ChatBridgeAdapter, DeliveryKind, DeliveryOutcome, DeliveryRequest,
    FetchedMedia, WebhookRequest, WebhookResponse,
};
use moqui_lib::http::RestClient;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::webhook::ReplayCache;

const DEFAULT_API: &str = "https://api.telnyx.com";
const DEFAULT_MAX_TEXT: usize = 1600;
const DEFAULT_MAX_MEDIA_COUNT: usize = 10;
const DEFAULT_MAX_MEDIA_BYTES: i64 = 1024 * 1024;
const DEFAULT_MAX_INBOUND_MEDIA: u64 = 5 * 1024 * 1024;
const DEFAULT_TOLERANCE_SECONDS: i64 = 300;
const DEFAULT_SENDS_PER_MINUTE: u32 = 60;
/// The most a file fetched from Telnyx may be, whatever a bridge says.
const MEDIA_CAP: u64 = 20 * 1024 * 1024;

/// A bridge's settings, from its `configJson`.
#[derive(Clone, Debug)]
pub struct Config {
    pub api_key: String,
    pub public_key: Option<String>,
    pub messaging_profile_id: Option<String>,
    pub campaign_id: Option<String>,
    pub api_base_url: String,
    pub max_text_length: usize,
    pub max_media_count: usize,
    pub max_media_bytes: i64,
    pub max_inbound_media_bytes: u64,
    pub tolerance_seconds: i64,
    pub sends_per_minute: u32,
    pub allow_internal: bool,
}

impl Config {
    pub fn of(config: &Json) -> Self {
        let text = |key: &str| {
            config
                .get(key)
                .and_then(Json::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let number = |key: &str| {
            config.get(key).and_then(|value| {
                value
                    .as_i64()
                    .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
            })
        };
        Self {
            api_key: text("apiKey").unwrap_or_default(),
            public_key: text("publicKey"),
            messaging_profile_id: text("messagingProfileId"),
            campaign_id: text("campaignId"),
            api_base_url: text("apiBaseUrl")
                .unwrap_or_else(|| DEFAULT_API.to_string())
                .trim_end_matches('/')
                .to_string(),
            max_text_length: number("maxTextLength")
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n > 0)
                .unwrap_or(DEFAULT_MAX_TEXT),
            max_media_count: number("maxMediaCount")
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(DEFAULT_MAX_MEDIA_COUNT),
            max_media_bytes: number("maxMediaBytes")
                .filter(|n| *n > 0)
                .unwrap_or(DEFAULT_MAX_MEDIA_BYTES),
            max_inbound_media_bytes: number("maxInboundMediaBytes")
                .and_then(|n| u64::try_from(n).ok())
                .filter(|n| *n > 0)
                .unwrap_or(DEFAULT_MAX_INBOUND_MEDIA)
                .min(MEDIA_CAP),
            tolerance_seconds: number("webhookToleranceSeconds")
                .filter(|n| *n > 0)
                .unwrap_or(DEFAULT_TOLERANCE_SECONDS),
            sends_per_minute: number("sendsPerMinute")
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(DEFAULT_SENDS_PER_MINUTE),
            allow_internal: config
                .get("allowInternalAddresses")
                .is_some_and(|value| value.as_bool() == Some(true) || value.as_str() == Some("true")),
        }
    }
}

/// The `sms` adapter. One serves every SMS bridge; each bridge's settings are its own.
pub struct TelnyxSms {
    /// Clients for public addresses only, and (a test's fake Telnyx, `allowInternalAddresses`) for any.
    public: OnceLock<RestClient>,
    internal: OnceLock<RestClient>,
    pub(crate) replays: Mutex<ReplayCache>,
}

impl Default for TelnyxSms {
    fn default() -> Self {
        Self::new()
    }
}

impl TelnyxSms {
    pub fn new() -> Self {
        Self {
            public: OnceLock::new(),
            internal: OnceLock::new(),
            replays: Mutex::new(ReplayCache::default()),
        }
    }

    fn client(&self, allow_internal: bool) -> &RestClient {
        let make = |deny: bool| {
            RestClient::new()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .max_response_bytes(MEDIA_CAP)
                .redirects(3, true)
                .deny_internal_addresses(deny)
        };
        if allow_internal {
            self.internal.get_or_init(|| make(false))
        } else {
            self.public.get_or_init(|| make(true))
        }
    }
}

/// Our number and theirs, from a mapping's external room id (`ours/theirs`).
pub fn pair_of(external_room_id: &str) -> Option<(String, String)> {
    let (ours, theirs) = external_room_id.split_once('/')?;
    let valid =
        |number: &str| number.starts_with('+') && number.len() > 4 && number[1..].bytes().all(|b| b.is_ascii_digit());
    (valid(ours) && valid(theirs)).then(|| (ours.to_string(), theirs.to_string()))
}

fn failed(reason: &str, error: impl Into<String>) -> DeliveryOutcome {
    DeliveryOutcome::Failed {
        reason_code: reason.to_string(),
        error: error.into(),
    }
}

/// Whether an HTTP error is one the provider never got: the connection could not be made.
fn never_sent(error: &moqui_lib::error::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(cause) = source {
        if let Some(http) = cause.downcast_ref::<reqwest::Error>() {
            return http.is_connect() || http.is_builder();
        }
        source = cause.source();
    }
    false
}

/// A Telnyx error's code and its text, from an error answer's `errors`.
fn provider_error(body: &str) -> (Option<String>, String) {
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
    (code, text)
}

/// The reason code of a send Telnyx refused (4xx), by its error code.
fn refusal_reason(code: Option<&str>) -> &'static str {
    match code {
        // blocked by a STOP the recipient sent
        Some("40300") => "optedOut",
        Some("40310" | "40311" | "40012" | "40013") => "badNumber",
        Some("40305" | "40306" | "40307") => "notRegistered",
        _ => "providerRefused",
    }
}

impl ChatBridgeAdapter for TelnyxSms {
    fn name(&self) -> &str {
        "sms"
    }

    fn capabilities(&self, bridge: &BridgeInfo) -> Capabilities {
        Capabilities {
            edit: false,
            delete: false,
            reaction: false,
            idempotent: false,
            sends_per_minute: Config::of(&bridge.config).sends_per_minute,
        }
    }

    fn start(&self, bridge: &BridgeInfo) -> Result<(), String> {
        let config = Config::of(&bridge.config);
        if config.api_key.is_empty() {
            return Err("The SMS bridge has no apiKey in its configuration".to_string());
        }
        if !(config.api_base_url.starts_with("https://") || config.api_base_url.starts_with("http://")) {
            return Err("The SMS bridge's apiBaseUrl is not an http address".to_string());
        }
        Ok(())
    }

    fn deliver(&self, bridge: &BridgeInfo, request: &DeliveryRequest) -> DeliveryOutcome {
        if request.kind != DeliveryKind::Message {
            return failed("bridgeUnsupported", "A text cannot be changed once sent");
        }
        let Some((ours, theirs)) = pair_of(&request.external_room_id) else {
            return failed("badMapping", "The room's mapping is not a pair of numbers");
        };
        let config = Config::of(&bridge.config);
        // a direct send (send#BridgeDirect) has no room: its caller checked it and holds its place in the day's count
        let in_room = !request.chat_room_id.is_empty();
        // the day's count this send holds (check#SmsSend reserve), let go when it does not reach the provider
        let mut reserved: Option<BTreeMap<String, Value>> = None;
        if in_room {
            // a text in a room goes from a user granted our number, never as the system
            let Some(user) = request.sender_user_id.as_deref().filter(|user| !user.is_empty()) else {
                return failed("numberNotGranted", "A text goes from a user granted the number");
            };
            let parameters = BTreeMap::from([
                ("fromNumber".to_string(), Value::string(&ours)),
                ("toNumber".to_string(), Value::string(&theirs)),
                ("userId".to_string(), Value::string(user)),
                ("reserve".to_string(), Value::Bool(true)),
            ]);
            match crate::call("org.moqui.impl.SmsServices.check#SmsSend", parameters) {
                Ok(out) if out.get("allowed") == Some(&Value::Bool(true)) => {
                    reserved = Some(BTreeMap::from([
                        ("userId".to_string(), Value::string(user)),
                        (
                            "counterDay".to_string(),
                            out.get("counterDay").cloned().unwrap_or(Value::Null),
                        ),
                    ]));
                }
                Ok(out) => {
                    let text = |key: &str| out.get(key).map(moqui_lib::object::to_plain_string).unwrap_or_default();
                    return failed(&text("reasonCode"), text("message"));
                }
                Err(error) => {
                    return DeliveryOutcome::Retry {
                        error: format!("The send could not be checked: {error}"),
                        not_connected: false,
                    }
                }
            }
        }
        let outcome = self.send(bridge, request, &config, &ours, &theirs);
        // not sent (refused here or by the provider, or to be tried again): its place in the day's count goes back
        if let Some(parameters) = reserved {
            if matches!(outcome, DeliveryOutcome::Failed { .. } | DeliveryOutcome::Retry { .. }) {
                if let Err(error) = crate::call("org.moqui.impl.SmsServices.release#SmsSend", parameters) {
                    tracing::warn!(
                        "a text of bridge {} was not taken off the day's count: {error}",
                        bridge.chat_bridge_id
                    );
                }
            }
        }
        outcome
    }

    fn webhook(&self, bridge: &BridgeInfo, request: &WebhookRequest) -> WebhookResponse {
        crate::webhook::handle(self, bridge, &Config::of(&bridge.config), request)
    }

    fn fetch_media(&self, bridge: &BridgeInfo, url: &str, max_bytes: u64) -> Result<FetchedMedia, String> {
        self.fetch(bridge, url, max_bytes)
    }
}

impl TelnyxSms {
    /// A checked text, to the provider.
    fn send(
        &self,
        bridge: &BridgeInfo,
        request: &DeliveryRequest,
        config: &Config,
        ours: &str,
        theirs: &str,
    ) -> DeliveryOutcome {
        let text = request.text.clone().unwrap_or_default();
        if text.chars().count() > config.max_text_length {
            return failed(
                "textTooLong",
                format!("A text is at most {} characters", config.max_text_length),
            );
        }
        if request.attachments.len() > config.max_media_count {
            return failed(
                "tooManyFiles",
                format!("A text carries at most {} files", config.max_media_count),
            );
        }
        let total: i64 = request.attachments.iter().map(|file| file.file_size.unwrap_or(0)).sum();
        if total > config.max_media_bytes {
            return failed(
                "mediaTooLarge",
                format!("A text's files are at most {} bytes in all", config.max_media_bytes),
            );
        }
        let now = jiff::Timestamp::now().as_millisecond();
        let mut media_urls = Vec::new();
        for file in &request.attachments {
            match media_url(&bridge.chat_bridge_id, &file.chat_attachment_id, now) {
                Some(url) => media_urls.push(url),
                None => {
                    return failed(
                        "mediaUnreachable",
                        "Files go by address: set channels_public_url (and channels_media_key)",
                    )
                }
            }
        }
        if text.trim().is_empty() && media_urls.is_empty() {
            return failed("empty", "A text has words or a file");
        }
        let mut body = json!({"from": ours, "to": theirs});
        if !text.trim().is_empty() {
            body["text"] = Json::String(text);
        }
        if !media_urls.is_empty() {
            body["media_urls"] = json!(media_urls);
        }
        if let Some(profile) = &config.messaging_profile_id {
            body["messaging_profile_id"] = Json::String(profile.clone());
        }
        let sent = self
            .client(config.allow_internal)
            .post(format!("{}/v2/messages", config.api_base_url))
            .header("authorization", format!("Bearer {}", config.api_key))
            .header("idempotency-key", request.idempotency_key.clone())
            .json(&body)
            .send(false);
        match sent {
            Err(error) if never_sent(&error) => DeliveryOutcome::Retry {
                error: format!("Telnyx could not be reached: {}", error.message()),
                not_connected: true,
            },
            Err(error) => DeliveryOutcome::Uncertain {
                error: format!("Telnyx's answer was lost: {}", error.message()),
            },
            Ok(response) => {
                let status = response.status();
                match status {
                    200..=299 => {
                        let parsed: Json = serde_json::from_str(response.body()).unwrap_or(Json::Null);
                        match parsed.pointer("/data/id").and_then(Json::as_str) {
                            Some(id) if !id.is_empty() => DeliveryOutcome::Accepted {
                                remote_event_id: id.to_string(),
                            },
                            _ => DeliveryOutcome::Uncertain {
                                error: "Telnyx took the text but named no message id".to_string(),
                            },
                        }
                    }
                    // not taken: a rate limit, the service away
                    429 | 503 => DeliveryOutcome::Retry {
                        error: format!("Telnyx answered {status}: {}", provider_error(response.body()).1),
                        not_connected: false,
                    },
                    // the key: nothing goes until an administrator fixes it
                    401 | 403 => DeliveryOutcome::Retry {
                        error: format!("Telnyx refused the API key ({status})"),
                        not_connected: true,
                    },
                    400..=499 => {
                        let (code, text) = provider_error(response.body());
                        failed(refusal_reason(code.as_deref()), text)
                    }
                    _ => DeliveryOutcome::Uncertain {
                        error: format!(
                            "Telnyx answered {status}; the text may have been sent: {}",
                            provider_error(response.body()).1
                        ),
                    },
                }
            }
        }
    }

    fn fetch(&self, bridge: &BridgeInfo, url: &str, max_bytes: u64) -> Result<FetchedMedia, String> {
        let config = Config::of(&bridge.config);
        let max = max_bytes.min(config.max_inbound_media_bytes);
        let response = self
            .client(config.allow_internal)
            .get(url)
            .timeout(Duration::from_secs(20))
            .send(false)
            .map_err(|error| error.message().to_string())?;
        if response.status() != 200 {
            return Err(format!("the file's address answered {}", response.status()));
        }
        if u64::try_from(response.bytes().len()).unwrap_or(u64::MAX) > max {
            return Err(format!("the file is over {max} bytes"));
        }
        let content_type = response
            .response_headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.clone());
        Ok(FetchedMedia {
            bytes: response.bytes().to_vec(),
            content_type,
        })
    }
}
