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

//! SMS and MMS on the Sinch Conversation API: the `sms-sinch` adapter's provider (docs/interfaces/SMS.md "Sinch",
//! plans: Sinch on the Conversation API, not the end-of-sale XMS `/batches`). Not yet tested with Sinch: the shapes are
//! from its documentation. A `ChatBridge` of type `CbtSms` names it by `adapterName` `sms-sinch`; its `configJson`
//! (kept encrypted) has, besides the neutral settings of [`crate::sms::Settings`]:
//!
//! | Key | What |
//! | --- | --- |
//! | `projectId`, `appId` | The project and its Conversation API app, required |
//! | `keyId`, `keySecret` | An access key of the project (OAuth2 client credentials), required |
//! | `region` | `us` (the default), `eu` or `br`: where the app was made (`https://{region}.conversation.api.sinch.com`) |
//! | `webhookSecret` | The app webhook's secret (HMAC-SHA256), required |
//! | `smsSender` | Our number, for an event that names none of ours (an opt-out) |
//! | `mmsSender` | The MMS channel's sender; without it a text with a file is refused (`mmsNotSetUp`) |
//! | `authBaseUrl` | `https://auth.sinch.com`, the token service (a test's fake sets it) |
//!
//! `apiBaseUrl` set overrides the region's address (a test's fake). Its defaults: `maxTextLength` 1600 characters,
//! `maxMediaCount` 1 and `maxMediaBytes` 1 MiB (an MMS: the Conversation API sends one file a message),
//! `maxInboundMediaBytes` 5 MiB.
//!
//! - **Auth.** OAuth2 client credentials: `POST {authBaseUrl}/oauth2/token` (`grant_type=client_credentials`, Basic
//!   `keyId:keySecret`), the token kept for its key until a minute before it runs out ([`Sinch::token`]). A token that
//!   cannot be had is `Retry` (nothing was sent); a 401 on a send drops it and sends once more with a new one.
//! - **Out.** `POST /v1/projects/{projectId}/messages:send`, to one number (`group_max` 0: the Conversation API has no
//!   group texts): channel `SMS` with `SMS_SENDER` our number for words alone; channel `MMS` with `MMS_SENDER` for a
//!   file, a `media_message`, or a `card_message` (its `description` the words) for words and a file, so a text is one
//!   call and a lost answer is `Uncertain` as for any provider. More than one file is refused (`tooManyFiles`), and a
//!   file from a number that is not the bridge's `mmsSender` (`mmsNotSetUp`): a text never goes from another number than
//!   its room's. `correlation_id` is the delivery's id, `callback_url` the bridge's webhook URL. Its answers: 2xx
//!   `message_id`; a second 401 `Retry`, the bridge not connected; 403 `Failed` (`billingLimit` for
//!   `BILLING_CREDIT_LIMIT_BREACHED`, else `providerRefused`); 404 `Failed` `wrongRegion` (the app is in another
//!   region, or the ids are wrong); 429 and 503 `Retry`; another 4xx `providerRefused`; another 5xx `Uncertain` (the
//!   core's classes: Sinch says to retry a 500, but a text it may have taken is never sent again on its own).
//! - **In.** `x-sinch-webhook-signature` is the base64 HMAC-SHA256 under `webhookSecret` of
//!   `{raw body}.{x-sinch-webhook-signature-nonce}.{x-sinch-webhook-signature-timestamp}`, the algorithm header
//!   `HmacSHA256`, the timestamp (seconds) within `webhookToleranceSeconds`; the core's replay cache, by the signature,
//!   catches a nonce sent again. The secret is the local node's alone, so behind an edge node the local node checks it.
//!   The event is the body's key: `message` (`MESSAGE_INBOUND`: a text, a file, or a file with a caption; each file of
//!   an MMS comes as its own event, so each is its own message), `message_delivery_report` (`MESSAGE_DELIVERY`:
//!   `DELIVERED` and `READ` delivered, `FAILED` failed with its reason; `QUEUED_ON_CHANNEL` and `SWITCHING_CHANNEL`
//!   left), `opt_out_notification` and `opt_in_notification` (STOP and START for `smsSender`); others are left. Its files
//!   are kept by Sinch 7 days, and the core fetches them as the text comes in.
//!
//! Its encryption is `None` (plans/Comms-Messaging.md §6.3), as every SMS provider's.

use std::collections::HashMap;
use std::sync::Mutex;

use aws_lc_rs::hmac;
use base64::Engine;
use moqui_core::service::chat_bridge::{
    replay_window_ms, BridgeInfo, DeliveryOutcome, WebhookEventKey, WebhookRequest, WebhookResponse,
};
use moqui_lib::http::{Method, RestClient, RestResponse};
use serde_json::{json, Value as Json};

use crate::sms::{
    answer, call, callback_url, flag_of, text_of, ApiBody, ApiRequest, Defaults, Inbound, InboundMedia, Keyword,
    OptSignal, Outgoing, ProviderError, Receipt, ReceiptStatus, Refusal, Settings, SmsEvent, SmsProvider,
};

/// The header Sinch signs a webhook in, and the others of its signature.
pub const SIGNATURE: &str = "x-sinch-webhook-signature";
const NONCE: &str = "x-sinch-webhook-signature-nonce";
const TIMESTAMP: &str = "x-sinch-webhook-signature-timestamp";
const ALGORITHM: &str = "x-sinch-webhook-signature-algorithm";
const REGIONS: &[&str] = &["us", "eu", "br"];
const AUTH_BASE: &str = "https://auth.sinch.com";
/// A token is taken as run out this long before Sinch says, so a send never carries one that ends on the way.
const TOKEN_MARGIN_MILLIS: i64 = 60_000;
/// A token's life when the answer names none (Sinch's is an hour).
const TOKEN_SECONDS: i64 = 3600;

/// A Sinch bridge's own settings, from its `configJson`.
#[derive(Clone, Debug)]
pub struct Config {
    pub project_id: String,
    pub app_id: String,
    pub key_id: String,
    pub key_secret: String,
    /// Lower case; `us` when none is set.
    pub region: String,
    /// The Conversation API's address: `apiBaseUrl` when set, else the region's. No trailing `/`.
    pub api_base: String,
    /// No trailing `/`.
    pub auth_base: String,
    pub webhook_secret: Option<String>,
    pub sms_sender: Option<String>,
    pub mms_sender: Option<String>,
    /// The token service may be plain http (a test's fake), never on production.
    pub allow_internal: bool,
}

/// An access token and when it is taken as run out.
#[derive(Clone, Debug)]
struct Token {
    value: String,
    good_until_millis: i64,
}

/// The Sinch provider of the `sms-sinch` adapter: its access tokens, by token service and key.
#[derive(Debug, Default)]
pub struct Sinch {
    tokens: Mutex<HashMap<String, Token>>,
}

impl Sinch {
    fn token_key(config: &Config) -> String {
        format!("{}|{}", config.auth_base, config.key_id)
    }

    /// The access token of the bridge's key at `now_millis`: the one kept, else a new one from the token service, kept
    /// until [`TOKEN_MARGIN_MILLIS`] before it runs out. Two sends at once may each fetch one; either is good.
    ///
    /// # Errors
    ///
    /// Returns the send's outcome when no token could be had: `Retry` (nothing was sent), the bridge not connected.
    pub fn token(&self, client: &RestClient, config: &Config, now_millis: i64) -> Result<String, DeliveryOutcome> {
        let key = Self::token_key(config);
        if let Some(token) = moqui_lib::sync::lock(&self.tokens).get(&key) {
            if token.good_until_millis > now_millis {
                return Ok(token.value.clone());
            }
        }
        let basic =
            base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", config.key_id, config.key_secret));
        let request = ApiRequest {
            method: Method::Post,
            url: format!("{}/oauth2/token", config.auth_base),
            headers: vec![("authorization".to_string(), format!("Basic {basic}"))],
            body: ApiBody::Form(vec![("grant_type".to_string(), "client_credentials".to_string())]),
        };
        let retry = |error: String| DeliveryOutcome::Retry {
            error,
            not_connected: true,
        };
        let response = call(client, &request)
            .map_err(|error| retry(format!("Sinch's token service did not answer: {}", error.message())))?;
        match response.status() {
            200..=299 => {}
            401 | 403 => {
                return Err(retry(format!(
                    "Sinch's token service refused the bridge's keyId and keySecret ({})",
                    response.status()
                )))
            }
            status => return Err(retry(format!("Sinch's token service answered {status}"))),
        }
        let parsed: Json = serde_json::from_str(response.body()).unwrap_or(Json::Null);
        let Some(value) = parsed
            .get("access_token")
            .and_then(Json::as_str)
            .filter(|value| !value.is_empty())
        else {
            return Err(retry("Sinch's token service answered no access_token".to_string()));
        };
        let seconds = parsed.get("expires_in").and_then(Json::as_i64).unwrap_or(TOKEN_SECONDS);
        let token = Token {
            value: value.to_string(),
            good_until_millis: now_millis
                .saturating_add(seconds.saturating_mul(1000))
                .saturating_sub(TOKEN_MARGIN_MILLIS),
        };
        moqui_lib::sync::lock(&self.tokens).insert(key, token);
        Ok(value.to_string())
    }

    /// Drop the bridge key's token (Sinch refused it), so the next send fetches another.
    pub fn forget_token(&self, config: &Config) {
        moqui_lib::sync::lock(&self.tokens).remove(&Self::token_key(config));
    }
}

/// A number as Sinch gives it (`12039414555` or `+12039414555`), in E.164.
pub fn e164(number: &str) -> String {
    let number = number.trim();
    if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
        format!("+{number}")
    } else {
        number.to_string()
    }
}

fn failed(reason: &str, error: impl Into<String>) -> DeliveryOutcome {
    DeliveryOutcome::Failed {
        reason_code: reason.to_string(),
        error: error.into(),
    }
}

impl SmsProvider for Sinch {
    type Config = Config;

    fn name(&self) -> &'static str {
        "Sinch"
    }

    fn adapter_name(&self) -> &'static str {
        "sms-sinch"
    }

    fn defaults(&self) -> Defaults {
        Defaults {
            api_base_url: "https://us.conversation.api.sinch.com",
            max_text_length: 1600,
            max_media_count: 1,
            max_media_bytes: 1024 * 1024,
            max_inbound_media_bytes: 5 * 1024 * 1024,
        }
    }

    fn config(&self, config: &Json) -> Config {
        let region = text_of(config, "region")
            .unwrap_or_else(|| "us".to_string())
            .to_ascii_lowercase();
        let api_base = text_of(config, "apiBaseUrl")
            .unwrap_or_else(|| format!("https://{region}.conversation.api.sinch.com"))
            .trim_end_matches('/')
            .to_string();
        Config {
            project_id: text_of(config, "projectId").unwrap_or_default(),
            app_id: text_of(config, "appId").unwrap_or_default(),
            key_id: text_of(config, "keyId").unwrap_or_default(),
            key_secret: text_of(config, "keySecret").unwrap_or_default(),
            region,
            api_base,
            auth_base: text_of(config, "authBaseUrl")
                .unwrap_or_else(|| AUTH_BASE.to_string())
                .trim_end_matches('/')
                .to_string(),
            webhook_secret: text_of(config, "webhookSecret"),
            sms_sender: text_of(config, "smsSender").map(|number| e164(&number)),
            mms_sender: text_of(config, "mmsSender").map(|number| e164(&number)),
            allow_internal: !moqui_core::web::is_production() && flag_of(config, "allowInternalAddresses"),
        }
    }

    fn check(&self, config: &Config) -> Result<(), String> {
        for (key, value) in [
            ("projectId", &config.project_id),
            ("appId", &config.app_id),
            ("keyId", &config.key_id),
            ("keySecret", &config.key_secret),
        ] {
            if value.is_empty() {
                return Err(format!("The Sinch bridge has no {key} in its configuration"));
            }
        }
        if config.webhook_secret.is_none() {
            return Err("The Sinch bridge has no webhookSecret in its configuration".to_string());
        }
        if !REGIONS.contains(&config.region.as_str()) {
            return Err(format!(
                "The Sinch bridge's region {} is not one of us, eu or br",
                config.region
            ));
        }
        // the key and secret go to the token service, so never in clear text, except to a test's fake (SEC-COM-10)
        if !(config.auth_base.starts_with("https://")
            || (config.allow_internal && config.auth_base.starts_with("http://")))
        {
            return Err("The Sinch bridge's authBaseUrl is not an https address".to_string());
        }
        Ok(())
    }

    fn group_max(&self) -> usize {
        // the Conversation API sends to one number a message; it documents no group texts
        0
    }

    /// The send, with no `authorization`: [`Self::send`] adds the access token.
    fn request(&self, config: &Config, _settings: &Settings, out: &Outgoing<'_>) -> ApiRequest {
        let file = out.media_urls.first();
        let (channel, sender_key) = if file.is_some() {
            ("MMS", "MMS_SENDER")
        } else {
            ("SMS", "SMS_SENDER")
        };
        let message = match (file, out.text) {
            (Some(url), Some(text)) => json!({"card_message": {"description": text, "media_message": {"url": url}}}),
            (Some(url), None) => json!({"media_message": {"url": url}}),
            (None, text) => json!({"text_message": {"text": text.unwrap_or_default()}}),
        };
        let theirs = out.theirs.first().cloned().unwrap_or_default();
        let mut body = json!({
            "app_id": config.app_id,
            "recipient": {"identified_by": {"channel_identities": [{"channel": channel, "identity": theirs}]}},
            "message": message,
            "channel_properties": {sender_key: out.ours},
        });
        let delivery = &out.request.chat_bridge_delivery_id;
        if !delivery.is_empty() && delivery.len() <= 128 {
            body["correlation_id"] = Json::String(delivery.clone());
        }
        if let Some(url) = callback_url(out.bridge) {
            body["callback_url"] = Json::String(url);
        }
        ApiRequest {
            method: Method::Post,
            url: format!("{}/v1/projects/{}/messages:send", config.api_base, config.project_id),
            headers: Vec::new(),
            body: ApiBody::Json(body),
        }
    }

    fn accepted_id(&self, _out: &Outgoing<'_>, body: &str) -> Option<String> {
        let parsed: Json = serde_json::from_str(body).ok()?;
        parsed.get("message_id").and_then(Json::as_str).map(str::to_string)
    }

    /// A Sinch error's status (`INVALID_ARGUMENT`, ...) and its message, from `{"error": {code, message, status}}`.
    fn error_of(&self, body: &str) -> ProviderError {
        let parsed: Json = serde_json::from_str(body).unwrap_or(Json::Null);
        let error = parsed.get("error");
        let code = error
            .and_then(|error| error.get("status"))
            .and_then(Json::as_str)
            .map(str::to_string);
        let text = error
            .and_then(|error| error.get("message"))
            .and_then(Json::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| body.chars().take(300).collect());
        ProviderError { code, text }
    }

    fn refusal(&self, _status: u16, _error: &ProviderError) -> Refusal {
        Refusal::Failed("providerRefused")
    }

    /// One text, one call: refused here when Sinch cannot carry it, else the access token, the send, and its answer
    /// (401, 403 and 404 read here, the rest by the core's classes).
    fn send(&self, client: &RestClient, config: &Config, settings: &Settings, out: &Outgoing<'_>) -> DeliveryOutcome {
        if out.media_urls.len() > 1 {
            return failed(
                "tooManyFiles",
                "Sinch sends one file a text: send each file in a text of its own",
            );
        }
        if !out.media_urls.is_empty() && config.mms_sender.as_deref() != Some(out.ours) {
            let error = match &config.mms_sender {
                None => "The Sinch bridge has no mmsSender: its texts carry no files until its MMS channel is set up"
                    .to_string(),
                Some(sender) => format!(
                    "Sinch's MMS channel sends from {sender}, not {}: this number's texts carry no files",
                    out.ours
                ),
            };
            return failed("mmsNotSetUp", error);
        }
        let request = self.request(config, settings, out);
        let send = || -> Result<Result<RestResponse, moqui_lib::error::Error>, DeliveryOutcome> {
            let token = self.token(client, config, jiff::Timestamp::now().as_millisecond())?;
            let mut signed = request.clone();
            signed
                .headers
                .push(("authorization".to_string(), format!("Bearer {token}")));
            Ok(call(client, &signed))
        };
        let mut result = match send() {
            Ok(result) => result,
            Err(outcome) => return outcome,
        };
        // a 401 is a token Sinch no longer takes, and the text was not taken: once more with a new one
        if matches!(&result, Ok(response) if response.status() == 401) {
            self.forget_token(config);
            result = match send() {
                Ok(result) => result,
                Err(outcome) => return outcome,
            };
        }
        if let Ok(response) = &result {
            match response.status() {
                401 => {
                    return DeliveryOutcome::Retry {
                        error: "Sinch refused a new access token too (401); the next try fetches another".to_string(),
                        not_connected: true,
                    };
                }
                403 => {
                    let error = self.error_of(response.body());
                    return if response.body().contains("BILLING_CREDIT_LIMIT_BREACHED") {
                        failed(
                            "billingLimit",
                            format!(
                                "Sinch refused the text: the project's credit limit is reached ({})",
                                error.text
                            ),
                        )
                    } else {
                        failed(
                            "providerRefused",
                            format!("Sinch refused the text (403): {}", error.text),
                        )
                    };
                }
                404 => {
                    return failed(
                        "wrongRegion",
                        format!(
                            "Sinch answered 404: the app is not in region {} (the bridge's region), or its projectId or \
                             appId is wrong: {}",
                            config.region,
                            self.error_of(response.body()).text
                        ),
                    );
                }
                _ => {}
            }
        }
        answer(self, out, result)
    }

    fn signature_header(&self) -> &'static str {
        SIGNATURE
    }

    fn verifies_on_edge(&self) -> bool {
        // signed with a secret only the local node holds
        false
    }

    /// The signature under the bridge's `webhookSecret`, by `HmacSHA256`, its timestamp within the window.
    fn verify(&self, bridge: &BridgeInfo, config: &Config, settings: &Settings, request: &WebhookRequest) -> bool {
        let Some(secret) = config.webhook_secret.as_deref() else {
            tracing::error!(
                "SMS bridge {}: a webhook came and the bridge has no webhookSecret to check it with",
                bridge.chat_bridge_id
            );
            return false;
        };
        let (Some(signature), Some(nonce), Some(timestamp), Some(algorithm)) = (
            request.header(SIGNATURE),
            request.header(NONCE),
            request.header(TIMESTAMP),
            request.header(ALGORITHM),
        ) else {
            return false;
        };
        if !algorithm.trim().eq_ignore_ascii_case("HmacSHA256") {
            return false;
        }
        let Some(stamp) = timestamp
            .trim()
            .parse::<i64>()
            .ok()
            .and_then(|seconds| seconds.checked_mul(1000))
        else {
            return false;
        };
        let window = replay_window_ms(settings.tolerance_seconds.saturating_mul(1000));
        request.now_millis.abs_diff(stamp) <= window.unsigned_abs()
            && signature_valid(secret, &request.body, nonce, timestamp, signature)
    }

    /// The event's id (an inbound message's id, a delivery report's message and status, an opt-out's request id; when
    /// none, a hash of the body, which Sinch's retry sends unchanged) and their number.
    fn event_key(&self, request: &WebhookRequest) -> Option<WebhookEventKey> {
        let event: Json = serde_json::from_slice(&request.body).ok()?;
        let (id, conversation) = if let Some(message) = event.get("message") {
            (
                str_at(message, "/id").map(str::to_string),
                str_at(message, "/channel_identity/identity"),
            )
        } else if let Some(report) = event.get("message_delivery_report") {
            (
                str_at(report, "/message_id")
                    .map(|id| format!("{id}:{}", str_at(report, "/status").unwrap_or_default())),
                str_at(report, "/channel_identity/identity"),
            )
        } else if let Some(notice) = event
            .get("opt_out_notification")
            .or_else(|| event.get("opt_in_notification"))
        {
            (
                str_at(notice, "/request_id").map(str::to_string),
                str_at(notice, "/identity"),
            )
        } else {
            (None, None)
        };
        let event_id = id.unwrap_or_else(|| {
            let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &request.body);
            format!("sha256:{}", moqui_lib::codec::hex_encode(digest.as_ref()))
        });
        Some(WebhookEventKey {
            event_id,
            conversation: conversation.map(e164),
        })
    }

    fn parse(&self, request: &WebhookRequest) -> Result<SmsEvent, WebhookResponse> {
        self.parse_as(None, request)
    }

    fn parse_with(&self, config: &Config, request: &WebhookRequest) -> Result<SmsEvent, WebhookResponse> {
        self.parse_as(config.sms_sender.as_deref(), request)
    }
}

impl Sinch {
    /// The event of a verified webhook; `ours` the bridge's `smsSender`, for an event that names none of our numbers.
    fn parse_as(&self, ours: Option<&str>, request: &WebhookRequest) -> Result<SmsEvent, WebhookResponse> {
        let Ok(event) = serde_json::from_slice::<Json>(&request.body) else {
            return Err(WebhookResponse::text(400, "the body is not JSON"));
        };
        if let Some(message) = event.get("message") {
            return received(&event, message, ours);
        }
        if let Some(report) = event.get("message_delivery_report") {
            return receipt(&event, report);
        }
        for (key, keyword, succeeded) in [
            ("opt_out_notification", Keyword::Stop, "OPT_OUT_SUCCEEDED"),
            ("opt_in_notification", Keyword::Start, "OPT_IN_SUCCEEDED"),
        ] {
            if let Some(notice) = event.get(key) {
                return Ok(opted(notice, keyword, succeeded, ours));
            }
        }
        Ok(SmsEvent::Ignored(
            "not a message, a delivery report or an opt-out".to_string(),
        ))
    }
}

/// Whether `signature` (base64) is the HMAC-SHA256 under `secret` of `body.nonce.timestamp`, compared in constant time.
pub fn signature_valid(secret: &str, body: &[u8], nonce: &str, timestamp: &str, signature: &str) -> bool {
    let Ok(signature) = base64::engine::general_purpose::STANDARD.decode(signature.trim()) else {
        return false;
    };
    let mut message = Vec::with_capacity(body.len() + nonce.len() + timestamp.len() + 2);
    message.extend_from_slice(body);
    message.push(b'.');
    message.extend_from_slice(nonce.as_bytes());
    message.push(b'.');
    message.extend_from_slice(timestamp.as_bytes());
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    hmac::verify(&key, &message, &signature).is_ok()
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

/// A `MESSAGE_INBOUND`: a text, a file, or a file with its caption. Our number is the message's `sender_id` (the number
/// the person texted), else the bridge's `smsSender`.
fn received(event: &Json, message: &Json, ours: Option<&str>) -> Result<SmsEvent, WebhookResponse> {
    if str_at(message, "/direction").is_some_and(|direction| direction != "TO_APP") {
        return Ok(SmsEvent::Ignored("a message from the app".to_string()));
    }
    let (Some(id), Some(theirs)) = (str_at(message, "/id"), str_at(message, "/channel_identity/identity")) else {
        return Err(WebhookResponse::text(
            400,
            "a text names its message and the sender's number",
        ));
    };
    let Some(ours) = str_at(message, "/sender_id")
        .map(e164)
        .or_else(|| ours.map(str::to_string))
    else {
        return Err(WebhookResponse::text(
            400,
            "a text names no number of ours (sender_id), and the bridge has no smsSender",
        ));
    };
    let content = message.get("contact_message").unwrap_or(&Json::Null);
    let file = |url: Option<&str>| -> Vec<InboundMedia> {
        url.map(|url| InboundMedia {
            url: url.to_string(),
            content_type: None,
        })
        .into_iter()
        .collect()
    };
    let (text, media) = if let Some(words) = content.pointer("/text_message/text").and_then(Json::as_str) {
        (Some(words.to_string()), Vec::new())
    } else if let Some(media) = content.get("media_message") {
        (None, file(str_at(media, "/url")))
    } else if let Some(card) = content.get("media_card_message") {
        (str_at(card, "/caption").map(str::to_string), file(str_at(card, "/url")))
    } else {
        return Ok(SmsEvent::Ignored("a message that is not a text or a file".to_string()));
    };
    Ok(SmsEvent::Inbound(Inbound {
        event_id: id.to_string(),
        message_id: id.to_string(),
        our_number: ours,
        their_number: e164(theirs),
        text,
        media,
        recipients: Vec::new(),
        origin_millis: millis_of(str_at(message, "/accept_time").or_else(|| str_at(event, "/accepted_time"))),
    }))
}

/// A `MESSAGE_DELIVERY`: the message's state at its recipient.
fn receipt(event: &Json, report: &Json) -> Result<SmsEvent, WebhookResponse> {
    let Some(id) = str_at(report, "/message_id") else {
        return Err(WebhookResponse::text(400, "a delivery report names its message"));
    };
    let status = str_at(report, "/status").unwrap_or_default();
    let status = match status {
        "DELIVERED" | "READ" => ReceiptStatus::Delivered,
        "FAILED" => ReceiptStatus::Failed,
        // QUEUED_ON_CHANNEL (taken by the SMS channel: the text was accepted already), SWITCHING_CHANNEL (a fallback)
        other => return Ok(SmsEvent::Ignored(other.to_string())),
    };
    let error = match (str_at(report, "/reason/description"), str_at(report, "/reason/code")) {
        (Some(description), Some(code)) => Some(format!("{description} ({code})")),
        (description, code) => description.or(code).map(str::to_string),
    };
    Ok(SmsEvent::Receipt(Receipt {
        remote_event_id: id.to_string(),
        status,
        error: error.filter(|_| status == ReceiptStatus::Failed),
        recipient_number: None,
        other_remote_event_id: None,
        receipt_millis: millis_of(str_at(event, "/event_time").or_else(|| str_at(event, "/accepted_time"))),
    }))
}

/// An `OPT_OUT` or `OPT_IN` Sinch took: STOP or START from their number to the bridge's `smsSender` (the notice names
/// none of ours). Left when it did not succeed, is of another channel, or the bridge has no `smsSender`.
fn opted(notice: &Json, keyword: Keyword, succeeded: &str, ours: Option<&str>) -> SmsEvent {
    if str_at(notice, "/status") != Some(succeeded) {
        return SmsEvent::Ignored(str_at(notice, "/status").unwrap_or("no status").to_string());
    }
    if str_at(notice, "/channel").is_some_and(|channel| channel != "SMS" && channel != "MMS") {
        return SmsEvent::Ignored("an opt-out of another channel".to_string());
    }
    let (Some(theirs), Some(ours)) = (str_at(notice, "/identity"), ours) else {
        return SmsEvent::Ignored("an opt-out with no number, or no smsSender on the bridge".to_string());
    };
    SmsEvent::OptSignal(OptSignal {
        our_number: ours.to_string(),
        their_number: e164(theirs),
        keyword,
    })
}
