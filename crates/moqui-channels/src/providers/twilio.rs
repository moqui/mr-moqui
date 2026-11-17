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

//! SMS and MMS on Twilio: the `sms-twilio` adapter's provider (docs/interfaces/SMS.md "Twilio"), **not yet tested with
//! Twilio** (its requests and webhooks are from Twilio's documentation). A `ChatBridge` of type `CbtSms` with
//! `adapterName` `sms-twilio`; its `configJson` (kept encrypted) has, besides the neutral settings of
//! [`crate::sms::Settings`]:
//!
//! | Key | What |
//! | --- | --- |
//! | `accountSid` | The account (`AC...`), required |
//! | `apiKeySid`, `apiKeySecret` | An API key (`SK...`) for the API's Basic auth; else `accountSid` and `authToken` |
//! | `authToken` | The account's primary Auth Token: the webhooks' signature key, required |
//! | `messagingServiceSid` | A Messaging Service (`MG...`) a text goes from (its sender pool picks our number); else `From` is our number |
//! | `conversationServiceSid` | The Conversations service (`IS...`) group texts are kept in; else the account's default one |
//! | `conversationsBaseUrl` | `https://conversations.twilio.com` (a test's fake) |
//! | `mediaContentBaseUrl` | `https://mcs.us1.twilio.com`, where a group text's files are (a test's fake) |
//!
//! Its defaults: `apiBaseUrl` `https://api.twilio.com`, `maxTextLength` 1600, `maxMediaCount` 10 and `maxMediaBytes`
//! 5 MB in all (an MMS's limits), `maxInboundMediaBytes` 5 MiB.
//!
//! - **Out.** One number: `POST /2010-04-01/Accounts/{AccountSid}/Messages.json`, form-encoded, `To`, `From` (our
//!   number) or `MessagingServiceSid`, `Body`, `MediaUrl` (one a file), `StatusCallback` (the bridge's webhook URL); its
//!   `sid` is the delivery's remote id. Twilio has no idempotency key for it. Its error codes: 21610 (the person sent
//!   STOP) `optedOut`, and the pair is marked opted out; 21211, 21614 `badNumber`; 21408 `internationalNotEnabled`
//!   (the region is not enabled for the account); 30007 `filtered`; 30032, 30034 `notRegistered`; 20429 and 21611 tried
//!   again; any other `providerRefused`. A group text goes through the Conversations API ([`group`]).
//! - **In.** Form posts signed in `X-Twilio-Signature` ([`signature_valid`]: HMAC-SHA1 under `authToken` over the URL
//!   Twilio posted to and the sorted parameters), checked on the local node (the edge node never holds the token).
//!   Twilio's signature has no time in it: a copy is caught by the replay cache while it holds it, and after that by the
//!   ids (the message's `MessageSid` once, a receipt's status once). An inbound text (`MessageSid`, `From`, `To`,
//!   `Body`, `MediaUrl{N}`, its `OptOutType` when Advanced Opt-Out took it as a keyword) and a status callback
//!   (`MessageStatus`, `ErrorCode`), answered with empty TwiML. Files are fetched with the API's Basic auth.
//!
//! Its encryption is `None` (plans/Comms-Messaging.md §6.3): the API is reached over TLS, but Twilio, the carriers and
//! the phone network read every text.

pub mod group;

use aws_lc_rs::hmac;
use base64::Engine;
use moqui_core::service::chat_bridge::{
    webhook_public_url, BridgeInfo, DeliveryOutcome, WebhookEventKey, WebhookRequest, WebhookResponse,
};
use moqui_lib::http::{Method, RestClient};
use serde_json::Value as Json;

use crate::sms::{
    exchange, text_of, ApiBody, ApiRequest, Defaults, Inbound, InboundMedia, Outgoing, ProviderError, Receipt,
    ReceiptStatus, Refusal, Settings, SmsEvent, SmsProvider,
};

/// The most outside numbers of a group text: a Twilio group MMS Conversation has at most 10 participants, our number
/// (its projected address) one of them.
pub const GROUP_MAX: usize = 9;
/// The header Twilio signs a webhook in.
pub const SIGNATURE: &str = "x-twilio-signature";
/// The most files of a text that come in that are read (Twilio's MMS carries at most 10).
const MEDIA_READ: usize = 10;

/// A Twilio bridge's own settings, from its `configJson`.
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub account_sid: String,
    pub api_key_sid: Option<String>,
    pub api_key_secret: Option<String>,
    pub auth_token: Option<String>,
    pub messaging_service_sid: Option<String>,
    pub conversation_service_sid: Option<String>,
    /// No trailing `/`.
    pub conversations_base_url: String,
    /// No trailing `/`.
    pub media_content_base_url: String,
}

impl Config {
    /// The API's `Authorization` header: Basic with the API key when there is one, else the account and its token.
    pub fn authorization(&self) -> Option<String> {
        let (user, password) = match (&self.api_key_sid, &self.api_key_secret) {
            (Some(key), Some(secret)) => (key.as_str(), secret.as_str()),
            _ => (self.account_sid.as_str(), self.auth_token.as_deref()?),
        };
        let pair = format!("{user}:{password}");
        Some(format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(pair.as_bytes())
        ))
    }
}

/// The Twilio provider of the `sms-twilio` adapter.
#[derive(Default)]
pub struct Twilio {
    /// The participants of each group text's Conversation, read from Twilio once a process ([`group`]).
    pub(crate) participants: group::Participants,
}

/// Whether `sid` is a Twilio SID of `prefix` (two letters and 32 hex digits): never anything that could change a path.
pub fn is_sid(sid: &str, prefix: &str) -> bool {
    sid.len() == 34 && sid.starts_with(prefix) && sid[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

impl SmsProvider for Twilio {
    type Config = Config;

    fn name(&self) -> &'static str {
        "Twilio"
    }

    fn adapter_name(&self) -> &'static str {
        "sms-twilio"
    }

    fn defaults(&self) -> Defaults {
        Defaults {
            api_base_url: "https://api.twilio.com",
            max_text_length: 1600,
            max_media_count: 10,
            max_media_bytes: 5 * 1024 * 1024,
            max_inbound_media_bytes: 5 * 1024 * 1024,
        }
    }

    fn config(&self, config: &Json) -> Config {
        let base = |key: &str, default: &str| {
            text_of(config, key)
                .unwrap_or_else(|| default.to_string())
                .trim_end_matches('/')
                .to_string()
        };
        Config {
            account_sid: text_of(config, "accountSid").unwrap_or_default(),
            api_key_sid: text_of(config, "apiKeySid"),
            api_key_secret: text_of(config, "apiKeySecret"),
            auth_token: text_of(config, "authToken"),
            messaging_service_sid: text_of(config, "messagingServiceSid"),
            conversation_service_sid: text_of(config, "conversationServiceSid"),
            conversations_base_url: base("conversationsBaseUrl", "https://conversations.twilio.com"),
            media_content_base_url: base("mediaContentBaseUrl", "https://mcs.us1.twilio.com"),
        }
    }

    fn check(&self, config: &Config) -> Result<(), String> {
        if !is_sid(&config.account_sid, "AC") {
            return Err("The Twilio bridge has no accountSid (AC...) in its configuration".to_string());
        }
        if config.auth_token.is_none() {
            return Err(
                "The Twilio bridge has no authToken in its configuration: its webhooks' signatures are checked with it"
                    .to_string(),
            );
        }
        if config.api_key_sid.is_some() != config.api_key_secret.is_some() {
            return Err("The Twilio bridge has one of apiKeySid and apiKeySecret without the other".to_string());
        }
        for (key, sid, prefix) in [
            ("messagingServiceSid", &config.messaging_service_sid, "MG"),
            ("conversationServiceSid", &config.conversation_service_sid, "IS"),
        ] {
            if sid.as_deref().is_some_and(|sid| !is_sid(sid, prefix)) {
                return Err(format!("The Twilio bridge's {key} is not a {prefix}... id"));
            }
        }
        Ok(())
    }

    fn group_max(&self) -> usize {
        GROUP_MAX
    }

    /// A text to one number on the Messages API (a group text is [`group::send`]'s).
    fn request(&self, config: &Config, settings: &Settings, out: &Outgoing<'_>) -> ApiRequest {
        let to = out.theirs.first().cloned().unwrap_or_default();
        let mut form = vec![("To".to_string(), to)];
        match &config.messaging_service_sid {
            Some(service) => form.push(("MessagingServiceSid".to_string(), service.clone())),
            None => form.push(("From".to_string(), out.ours.to_string())),
        }
        if let Some(text) = out.text {
            form.push(("Body".to_string(), text.to_string()));
        }
        for url in out.media_urls {
            form.push(("MediaUrl".to_string(), url.clone()));
        }
        if let Some(callback) = crate::sms::callback_url(out.bridge) {
            form.push(("StatusCallback".to_string(), callback));
        }
        ApiRequest {
            method: Method::Post,
            url: format!(
                "{}/2010-04-01/Accounts/{}/Messages.json",
                settings.api_base_url, config.account_sid
            ),
            headers: auth_headers(config),
            body: ApiBody::Form(form),
        }
    }

    fn accepted_id(&self, _out: &Outgoing<'_>, body: &str) -> Option<String> {
        let parsed: Json = serde_json::from_str(body).ok()?;
        parsed.get("sid").and_then(Json::as_str).map(str::to_string)
    }

    /// Twilio's error answer: `{"code": 21610, "message": "...", "more_info": "...", "status": 400}`.
    fn error_of(&self, body: &str) -> ProviderError {
        let parsed: Json = serde_json::from_str(body).unwrap_or(Json::Null);
        let code = parsed.get("code").and_then(|code| {
            code.as_i64()
                .map(|n| n.to_string())
                .or_else(|| code.as_str().map(str::to_string))
        });
        let text = parsed
            .get("message")
            .and_then(Json::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| body.chars().take(300).collect());
        ProviderError { code, text }
    }

    fn refusal(&self, _status: u16, error: &ProviderError) -> Refusal {
        refusal_of(error.code.as_deref())
    }

    fn send(&self, client: &RestClient, config: &Config, settings: &Settings, out: &Outgoing<'_>) -> DeliveryOutcome {
        if out.group() {
            return group::send(self, client, config, settings, out);
        }
        let outcome = exchange(self, client, out, &self.request(config, settings, out));
        if let DeliveryOutcome::Failed { reason_code, .. } = &outcome {
            if reason_code == "optedOut" {
                if let Some(theirs) = out.theirs.first() {
                    opted_out(out.bridge, out.ours, theirs);
                }
            }
        }
        outcome
    }

    fn signature_header(&self) -> &'static str {
        SIGNATURE
    }

    fn verifies_on_edge(&self) -> bool {
        false
    }

    fn verify(&self, bridge: &BridgeInfo, config: &Config, _settings: &Settings, request: &WebhookRequest) -> bool {
        let Some(token) = config.auth_token.as_deref() else {
            tracing::error!(
                "SMS bridge {}: a webhook came and the bridge has no authToken to check it with",
                bridge.chat_bridge_id
            );
            return false;
        };
        let Some(signature) = request.header(SIGNATURE) else {
            return false;
        };
        let Some(url) = webhook_public_url(bridge, request) else {
            tracing::error!(
                "SMS bridge {}: a Twilio webhook cannot be checked: set channels_public_url or the bridge's webhookUrl \
                 to the URL Twilio posts to",
                bridge.chat_bridge_id
            );
            return false;
        };
        signature_valid(token, &url, &request.body, signature)
    }

    /// The event's id: an inbound text's or a Conversation message's `MessageSid`, a status callback's `MessageSid` and
    /// its status (each change is its own event), a Conversation receipt's `DeliveryReceiptSid` and status; else a hash
    /// of the body. Ordered by their number (or the Conversation).
    fn event_key(&self, request: &WebhookRequest) -> Option<WebhookEventKey> {
        let form = Form::parse(&request.body)?;
        let hash = || {
            let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &request.body);
            format!("sha256:{}", moqui_lib::codec::hex_encode(digest.as_ref()))
        };
        if let Some(kind) = form.get("EventType") {
            let id = match kind {
                "onMessageAdded" => form.get("MessageSid").map(str::to_string),
                "onDeliveryUpdated" => form
                    .get("DeliveryReceiptSid")
                    .zip(form.get("Status"))
                    .map(|(sid, status)| format!("{sid}:{status}")),
                _ => None,
            };
            return Some(WebhookEventKey {
                event_id: id.unwrap_or_else(hash),
                conversation: form.get("ConversationSid").map(str::to_string),
            });
        }
        let (event_id, conversation) = match status_of(&form) {
            Some(status) => (
                form.get("MessageSid").map(|sid| format!("{sid}:{status}")),
                form.get("To"),
            ),
            None => (form.get("MessageSid").map(str::to_string), form.get("From")),
        };
        Some(WebhookEventKey {
            event_id: event_id.unwrap_or_else(hash),
            conversation: conversation.map(str::to_string),
        })
    }

    fn parse(&self, request: &WebhookRequest) -> Result<SmsEvent, WebhookResponse> {
        let Some(form) = Form::parse(&request.body) else {
            return Err(WebhookResponse::text(400, "the body is not a form"));
        };
        if form.get("EventType").is_some() {
            // a Conversations event: its numbers are the local node's to find (parse_on)
            return Ok(SmsEvent::Ignored("a Conversations event".to_string()));
        }
        match status_of(&form) {
            Some(status) => Ok(receipt(&form, status)),
            None => received(&form),
        }
    }

    /// A Conversations event is the group's ([`group::event`]); the Messages API's are [`Self::parse`]'s.
    fn parse_on(
        &self,
        client: &RestClient,
        bridge: &BridgeInfo,
        config: &Config,
        _settings: &Settings,
        request: &WebhookRequest,
    ) -> Result<SmsEvent, WebhookResponse> {
        match Form::parse(&request.body) {
            Some(form) if form.get("EventType").is_some() => group::event(self, client, bridge, config, &form),
            _ => self.parse(request),
        }
    }

    /// Twilio reads an inbound text's answer as TwiML: an empty `<Response/>` sends no reply.
    fn respond(&self, response: WebhookResponse) -> WebhookResponse {
        if !(200..300).contains(&response.status) {
            return response;
        }
        WebhookResponse {
            status: response.status,
            content_type: "text/xml".to_string(),
            body: b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response/>".to_vec(),
            auth_failure: false,
        }
    }

    fn media_auth(&self, config: &Config) -> Option<(String, String)> {
        config.authorization().map(|value| ("authorization".to_string(), value))
    }
}

/// The API's auth header.
pub(crate) fn auth_headers(config: &Config) -> Vec<(String, String)> {
    config
        .authorization()
        .map(|value| vec![("authorization".to_string(), value)])
        .unwrap_or_default()
}

/// What a refused send means, by Twilio's error code.
pub fn refusal_of(code: Option<&str>) -> Refusal {
    Refusal::Failed(match code {
        // the person sent STOP to the number (or to the Messaging Service)
        Some("21610") => "optedOut",
        Some("21211" | "21614") => "badNumber",
        // the destination's region is not enabled for the account (Geo Permissions)
        Some("21408") => "internationalNotEnabled",
        Some("30007") => "filtered",
        Some("30032" | "30034") => "notRegistered",
        // a rate (too many requests at once) and the queue full: taken later
        Some("20429" | "21611") => return Refusal::Retry,
        _ => "providerRefused",
    })
}

/// A send Twilio refused because the person opted out (21610): the pair is marked as a STOP would mark it, so the next
/// send is refused before Twilio.
fn opted_out(bridge: &BridgeInfo, ours: &str, theirs: &str) {
    let parameters = std::collections::BTreeMap::from([
        ("ourNumber".to_string(), moqui_lib::object::Value::string(ours)),
        ("theirNumber".to_string(), moqui_lib::object::Value::string(theirs)),
        ("keyword".to_string(), moqui_lib::object::Value::string("STOP")),
    ]);
    if let Err(error) = crate::call("org.moqui.call.SmsServices.note#SmsKeyword", parameters) {
        tracing::warn!(
            "SMS bridge {}: Twilio refused a text to {theirs} as opted out, and the opt-out was not kept: {error}",
            bridge.chat_bridge_id
        );
    }
}

// ---- the signature ----

/// Whether `signature` (base64) is Twilio's signature of a request to `url` with `body`: HMAC-SHA1 under the Auth
/// Token of the URL and, for a form, each parameter's name and value sorted by name (case-sensitive), with no
/// separators; for another body Twilio puts `bodySHA256` (its hex SHA-256) in the URL's query and signs the URL alone.
/// Twilio signs the URL as it was set, which may or may not name the default port: both are tried. Credentials in the
/// URL are not part of it. The comparison is in constant time (aws-lc-rs).
pub fn signature_valid(auth_token: &str, url: &str, body: &[u8], signature: &str) -> bool {
    let Ok(signature) = base64::engine::general_purpose::STANDARD.decode(signature.trim()) else {
        return false;
    };
    let url = without_credentials(url);
    let pairs = match query_value(&url, "bodySHA256") {
        Some(expected) => {
            let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, body);
            let actual = moqui_lib::codec::hex_encode(digest.as_ref());
            if !actual.eq_ignore_ascii_case(&expected) {
                return false;
            }
            Vec::new()
        }
        None => match Form::parse(body) {
            Some(form) => form.sorted(),
            None => return false,
        },
    };
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, auth_token.as_bytes());
    let signed = |url: &str| {
        let mut data = url.to_string();
        for (name, value) in &pairs {
            data.push_str(name);
            data.push_str(value);
        }
        hmac::verify(&key, data.as_bytes(), &signature).is_ok()
    };
    signed(&url) || other_port_form(&url).is_some_and(|other| signed(&other))
}

/// The signature Twilio would send for a form post of `pairs` to `url` (a test's signer; [`signature_valid`] checks).
pub fn sign(auth_token: &str, url: &str, pairs: &[(String, String)]) -> String {
    let mut sorted = pairs.to_vec();
    sorted.sort();
    let mut data = url.to_string();
    for (name, value) in &sorted {
        data.push_str(name);
        data.push_str(value);
    }
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, auth_token.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hmac::sign(&key, data.as_bytes()).as_ref())
}

/// `url`'s scheme, authority and the rest: (`https`, `user@host:443`, `/path?query`).
fn split_url(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some((scheme, &rest[..end], &rest[end..]))
}

/// `url` without a `user:password@` in its authority.
fn without_credentials(url: &str) -> String {
    match split_url(url) {
        Some((scheme, authority, rest)) if authority.contains('@') => {
            let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
            format!("{scheme}://{host}{rest}")
        }
        _ => url.to_string(),
    }
}

/// `url` with its default port named when it names none, or without it when it names it (`https` 443, `http` 80);
/// none for another port or scheme.
fn other_port_form(url: &str) -> Option<String> {
    let (scheme, authority, rest) = split_url(url)?;
    let default = match scheme.to_ascii_lowercase().as_str() {
        "https" => "443",
        "http" => "80",
        _ => return None,
    };
    // a port follows the last `:` after any `]` of an IPv6 address
    let after_bracket = authority.rfind(']').map_or(0, |at| at + 1);
    match authority[after_bracket..].rfind(':') {
        Some(colon) => {
            let at = after_bracket + colon;
            (&authority[at + 1..] == default).then(|| format!("{scheme}://{}{rest}", &authority[..at]))
        }
        None => Some(format!("{scheme}://{authority}:{default}{rest}")),
    }
}

/// The decoded value of `name` in `url`'s query.
fn query_value(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or_default();
    Form::parse(query.as_bytes())?
        .pairs
        .into_iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

// ---- the webhooks ----

/// A form body (`application/x-www-form-urlencoded`), its pairs in order.
#[derive(Clone, Debug, Default)]
pub struct Form {
    pub pairs: Vec<(String, String)>,
}

impl Form {
    /// The pairs of `body`; none when it is not UTF-8 or a pair's escape is broken.
    pub fn parse(body: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(body).ok()?;
        let mut pairs = Vec::new();
        for part in text.split('&').filter(|part| !part.is_empty()) {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            pairs.push((form_decode(name)?, form_decode(value)?));
        }
        Some(Self { pairs })
    }

    /// The first value of `name`, when it is not empty.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    }

    /// The pairs sorted by name, then value (byte order: case-sensitive).
    pub fn sorted(&self) -> Vec<(String, String)> {
        let mut pairs = self.pairs.clone();
        pairs.sort();
        pairs
    }
}

/// One form-encoded name or value decoded (`+` a space, `%XX` a byte); none when it is not UTF-8 or an escape is
/// broken.
fn form_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = text.get(at + 1..at + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                at += 2;
            }
            byte => out.push(byte),
        }
        at += 1;
    }
    String::from_utf8(out).ok()
}

/// A status callback's `MessageStatus` (an inbound text's is `received`, or it has none).
fn status_of(form: &Form) -> Option<&str> {
    form.get("MessageStatus")
        .or_else(|| form.get("SmsStatus"))
        .filter(|status| !matches!(*status, "received" | "receiving"))
}

/// Twilio's words for some delivery error codes, for the delivery's reason.
pub(crate) fn error_text(code: &str) -> String {
    let what = match code {
        "30003" => "the phone is unreachable",
        "30004" => "the message was blocked",
        "30005" => "the number is unknown or no longer in service",
        "30006" => "the number is a landline or cannot take texts",
        "30007" => "the carrier filtered it",
        "30008" => "an unknown error",
        "30032" => "the toll-free number is not verified",
        "30034" => "the number is not registered for A2P 10DLC",
        "21610" => "the person opted out (STOP)",
        _ => "",
    };
    if what.is_empty() {
        format!("Twilio error {code}")
    } else {
        format!("Twilio error {code}: {what}")
    }
}

/// An inbound text: `MessageSid`, `From`, `To`, `Body`, `NumMedia` and `MediaUrl{N}`, `MediaContentType{N}` (from 0).
fn received(form: &Form) -> Result<SmsEvent, WebhookResponse> {
    let (Some(message_id), Some(theirs), Some(ours)) = (form.get("MessageSid"), form.get("From"), form.get("To"))
    else {
        return Err(WebhookResponse::text(400, "a text names its message and both numbers"));
    };
    let count = form
        .get("NumMedia")
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .min(MEDIA_READ);
    let media = (0..count)
        .filter_map(|index| {
            Some(InboundMedia {
                url: form.get(&format!("MediaUrl{index}"))?.to_string(),
                content_type: form.get(&format!("MediaContentType{index}")).map(str::to_string),
            })
        })
        .collect();
    Ok(SmsEvent::Inbound(Inbound {
        event_id: message_id.to_string(),
        message_id: message_id.to_string(),
        our_number: ours.to_string(),
        their_number: theirs.to_string(),
        text: form
            .pairs
            .iter()
            .find(|(key, _)| key == "Body")
            .map(|(_, body)| body.clone()),
        media,
        recipients: Vec::new(),
        origin_millis: None,
    }))
}

/// A status callback: one a change of the message's state, in no set order (`note#DeliveryReceipt` never takes a
/// delivered text back to sent). `queued`, `accepted`, `scheduled` and `sending` are before the carrier: left.
fn receipt(form: &Form, status: &str) -> SmsEvent {
    let Some(message_id) = form.get("MessageSid") else {
        return SmsEvent::Ignored("a status with no message".to_string());
    };
    let status = match status {
        "sent" => ReceiptStatus::Sent,
        // read: an RCS or WhatsApp read receipt, which is past delivered
        "delivered" | "read" => ReceiptStatus::Delivered,
        "undelivered" | "failed" | "canceled" => ReceiptStatus::Failed,
        other => return SmsEvent::Ignored(other.to_string()),
    };
    let error = form.get("ErrorCode").filter(|code| *code != "0").map(error_text);
    SmsEvent::Receipt(Receipt {
        remote_event_id: message_id.to_string(),
        status,
        error: error.or_else(|| (status == ReceiptStatus::Failed).then(|| "Twilio could not deliver it".to_string())),
        recipient_number: None,
        other_remote_event_id: None,
        receipt_millis: None,
    })
}
