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

//! The provider-neutral SMS and MMS adapter (docs/interfaces/SMS.md, docs/interfaces/Chat-Bridges.md "The adapter
//! contract"): [`SmsAdapter`] is the chat bridge adapter of one provider, and the provider's module
//! ([`crate::providers`]) implements [`SmsProvider`], what differs: its settings, its API's requests and answers, its
//! webhook signature and events, its file fetch.
//!
//! What is the same for every provider is here:
//!
//! - **Out.** A text in a number pair's room (or a group's, `ours/a,b,c`) is checked before the provider by
//!   `org.moqui.call.SmsServices.check#SmsSend`, once a recipient, each reserving its place in the day's count, let go
//!   again when the text does not reach the provider (refused, `Failed`, `Retry`, `Wait`). Then the bridge's limits (the
//!   text's length, the files' count and bytes), the files' addresses ([`media_urls`]: by the edge node on a paired
//!   node, `Wait` while they are on their way), and one send through a guarded client: public addresses only, at most
//!   three redirects (the client drops `Authorization` on a redirect to another host), `allowInternalAddresses` only off
//!   production (a test's fake provider, SEC-COM-10). Its answer by class: 2xx `Accepted` (the provider's id, else
//!   `Uncertain`); 429 and 503 `Retry`; 401 and 403 `Retry` with the bridge not connected; another 4xx as the provider
//!   reads it ([`SmsProvider::refusal`]); 5xx `Uncertain`. No provider dedupes a send, so a request whose answer was lost
//!   is `Uncertain` and never sent again on its own; only one that never reached the provider (a refused connection) is
//!   `Retry`. A refusal read as `optedOut` marks the pair opted out (the STOP table agrees with the provider).
//! - **In.** A webhook is a POST; its signature is the provider's to check ([`SmsProvider::verify`]); the replay cache
//!   (`replay_take`, by the signature header's value) answers a copy worked before 200 `duplicate` and one being worked
//!   409; the provider parses the body into a neutral [`SmsEvent`], worked under `catch_unwind`: a text goes to
//!   `SmsServices.receive#Sms` (in a provider's conversation, `thread_id`, to its mapping's room), a receipt to
//!   `ChatBridgeServices.note#DeliveryReceipt`, an opt-out or opt-in to `SmsServices.note#SmsOptSignal` (once per event). A failure of the server's own is 503 (the provider, or an edge node's queue, tries
//!   again); a text `ingest#Event` refuses for what it is ([`REFUSALS`]) is 422.
//! - **Behind an edge node** (plans/archive/Edge-Node.md §5.4): a provider that signs with a public key the bridge's
//!   copy holds (Telnyx) is verified on the edge node; one that signs with a secret (Twilio, Sinch) on the local node
//!   only ([`WebhookVerification::Core`], `chat_bridge_edge.rs`); the local node checks every queued request again
//!   ([`SmsProvider::can_verify`] says when it cannot). The edge node's answers are the provider's shape
//!   ([`SmsProvider::respond`]).
//!
//! The settings every provider reads from the bridge's `configJson` ([`Settings`]): `apiBaseUrl`, `maxTextLength`,
//! `maxMediaCount`, `maxMediaBytes`, `maxInboundMediaBytes`, `webhookToleranceSeconds`, `sendsPerMinute` (the bridge's
//! own `sendsPerMinute` wins), `allowInternalAddresses`; and `webhookUrl`, the URL the provider posts to, read only by
//! [`moqui_core::service::chat_bridge::webhook_public_url`] (a signature over the URL) and [`callback_url`].

pub mod rich;

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use moqui_core::service::chat_bridge::{
    media_urls, replay_finish, replay_take, webhook_public_url, BridgeInfo, Capabilities, ChatBridgeAdapter,
    DeliveryKind, DeliveryOutcome, DeliveryRequest, Encryption, FetchedMedia, MediaUrls, NumberLookup, Replay,
    WebhookEventKey, WebhookRequest, WebhookResponse, WebhookVerification, MEDIA_RETRY_MILLIS, WEBHOOK_MARK,
};
use moqui_lib::http::{Method, RestClient, RestResponse};
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};

pub use rich::Rich;

/// The most pictures a rich message (a carousel's cards) carries.
pub const RCS_MEDIA_COUNT: usize = 10;
/// The most bytes a rich message's pictures are together (`maxRcsTotalBytes` overrides).
pub const RCS_TOTAL_BYTES: i64 = 64 * 1024 * 1024;

/// The most a file fetched from a provider may be, whatever a bridge says.
pub const MEDIA_CAP: u64 = 20 * 1024 * 1024;
const DEFAULT_TOLERANCE_SECONDS: i64 = 300;
const DEFAULT_SENDS_PER_MINUTE: u32 = 60;
/// The most numbers a mapping's external room id is read with (a group's `ours/a,b,c`); a provider's
/// [`SmsProvider::group_max`] is the real bound.
const NUMBERS_READ: usize = 32;

// ---- the settings ----

/// A provider's defaults for the neutral settings.
#[derive(Clone, Copy, Debug)]
pub struct Defaults {
    pub api_base_url: &'static str,
    pub max_text_length: usize,
    pub max_media_count: usize,
    pub max_media_bytes: i64,
    pub max_inbound_media_bytes: u64,
}

/// The settings of a bridge that are the same for every provider, from its `configJson` (see the module notes).
#[derive(Clone, Debug)]
pub struct Settings {
    /// No trailing `/`.
    pub api_base_url: String,
    pub max_text_length: usize,
    pub max_media_count: usize,
    pub max_media_bytes: i64,
    /// At most [`MEDIA_CAP`].
    pub max_inbound_media_bytes: u64,
    pub tolerance_seconds: i64,
    pub sends_per_minute: u32,
    /// The API and file addresses may be internal (a test's fake provider); never on a production instance, where an
    /// inbound text's media URL would otherwise make the server fetch an internal address (SEC-COM-10).
    pub allow_internal: bool,
}

impl Settings {
    pub fn of(config: &Json, defaults: &Defaults) -> Self {
        Self {
            api_base_url: text_of(config, "apiBaseUrl")
                .unwrap_or_else(|| defaults.api_base_url.to_string())
                .trim_end_matches('/')
                .to_string(),
            max_text_length: number_of(config, "maxTextLength")
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n > 0)
                .unwrap_or(defaults.max_text_length),
            max_media_count: number_of(config, "maxMediaCount")
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(defaults.max_media_count),
            max_media_bytes: number_of(config, "maxMediaBytes")
                .filter(|n| *n > 0)
                .unwrap_or(defaults.max_media_bytes),
            max_inbound_media_bytes: number_of(config, "maxInboundMediaBytes")
                .and_then(|n| u64::try_from(n).ok())
                .filter(|n| *n > 0)
                .unwrap_or(defaults.max_inbound_media_bytes)
                .min(MEDIA_CAP),
            tolerance_seconds: number_of(config, "webhookToleranceSeconds")
                .filter(|n| *n > 0)
                .unwrap_or(DEFAULT_TOLERANCE_SECONDS),
            sends_per_minute: number_of(config, "sendsPerMinute")
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(DEFAULT_SENDS_PER_MINUTE),
            allow_internal: !moqui_core::web::is_production() && flag_of(config, "allowInternalAddresses"),
        }
    }
}

/// A text setting of a `configJson`, trimmed; none when empty.
pub fn text_of(config: &Json, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// A number setting of a `configJson`, a number or its text.
pub fn number_of(config: &Json, key: &str) -> Option<i64> {
    config.get(key).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
    })
}

/// A yes/no setting of a `configJson`: true, or the text `true`.
pub fn flag_of(config: &Json, key: &str) -> bool {
    config
        .get(key)
        .is_some_and(|value| value.as_bool() == Some(true) || value.as_str() == Some("true"))
}

/// The URL the provider posts the bridge's webhooks to, for a send that names its status callback (Twilio's
/// `StatusCallback`, Sinch's `callback_url`): the bridge's `webhookUrl`, else `channels_public_url` and
/// `/channels/{chatBridgeId}/webhook`. None when neither is set.
pub fn callback_url(bridge: &BridgeInfo) -> Option<String> {
    let request = WebhookRequest {
        url: format!("/channels/{}/webhook", bridge.chat_bridge_id),
        ..WebhookRequest::default()
    };
    webhook_public_url(bridge, &request)
}

// ---- what a provider gives ----

/// One checked text to send, as the provider sees it.
#[derive(Clone, Copy, Debug)]
pub struct Outgoing<'a> {
    pub bridge: &'a BridgeInfo,
    pub request: &'a DeliveryRequest,
    /// Our number, in E.164.
    pub ours: &'a str,
    /// The outside numbers: one, or a group text's (at most the provider's [`SmsProvider::group_max`]).
    pub theirs: &'a [String],
    /// The words, when there are any.
    pub text: Option<&'a str>,
    /// The files' addresses, one per attachment in order, for the provider to fetch.
    pub media_urls: &'a [String],
    /// An RCS send ([`Channel::Rcs`]) may go as SMS or MMS from our number when the phone takes no RCS: the bridge's
    /// `smsFallback` (true when not set). Always true for an SMS provider, where it means nothing.
    pub fallback: bool,
    /// An RCS send's cards and suggestions ([`rich`]), its `text` the fallback; none for an SMS provider, which sends
    /// the fallback and the first picture.
    pub rich: Option<&'a Rich>,
    /// A rich send's fallback picture (MMS): the index in [`Self::media_urls`] of its first, when that is within the
    /// MMS limit (`maxMediaBytes`); none sends the fallback as text alone.
    pub fallback_image: Option<usize>,
}

impl Outgoing<'_> {
    /// Whether it goes to several numbers as one group text.
    pub fn group(&self) -> bool {
        self.theirs.len() > 1
    }
}

/// An HTTP request to the provider's API, built by the provider and sent by the core.
#[derive(Clone, Debug)]
pub struct ApiRequest {
    pub method: Method,
    /// The whole URL (the bridge's `apiBaseUrl` and the provider's path).
    pub url: String,
    /// The auth header and any other; the body's content type is set by its encoding.
    pub headers: Vec<(String, String)>,
    pub body: ApiBody,
}

/// The body of an [`ApiRequest`].
#[derive(Clone, Debug)]
pub enum ApiBody {
    None,
    Json(Json),
    /// `application/x-www-form-urlencoded`, the pairs in order (a name may repeat, as Twilio's `MediaUrl`).
    Form(Vec<(String, String)>),
}

/// A provider's error answer: its code, when it names one, and its text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderError {
    pub code: Option<String>,
    pub text: String,
}

/// What a refused send (a 4xx other than 401, 403 and 429) means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Never sent, and another try changes nothing: the delivery's reason code (`optedOut`, `badNumber`, ...).
    Failed(&'static str),
    /// Not taken now (a provider's own rate code): tried again later.
    Retry,
}

/// The network a provider's adapter speaks (plans/Comms-Messaging.md §16): SMS and MMS, or business RCS (the brand's
/// agent, with SMS and MMS from our number as its fallback).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Channel {
    #[default]
    Sms,
    Rcs,
}

/// The way an RCS bridge's message went, as the provider's answer or report tells it (`ChatBridgeDelivery.deliveryPath`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    Rcs,
    Sms,
    Mms,
    /// SMS or MMS, the report does not say which: `note#DeliveryPath` decides by the message's files.
    Fallback,
}

impl Path {
    /// Its name in `note#DeliveryPath`.
    pub fn as_str(self) -> &'static str {
        match self {
            Path::Rcs => "rcs",
            Path::Sms => "sms",
            Path::Mms => "mms",
            Path::Fallback => "fallback",
        }
    }
}

/// A place a person shared (an RCS location message): checked numbers, the words cut to [`LOCATION_TEXT_MAX`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
    pub label: Option<String>,
    pub address: Option<String>,
}

/// The most characters of a location's label or address kept.
pub const LOCATION_TEXT_MAX: usize = 200;
/// The most characters of a tapped suggestion's payload kept.
pub const POSTBACK_MAX: usize = 1024;

impl Location {
    /// A location from a provider's latitude, longitude and words; none when either number is missing or out of range.
    pub fn of(
        latitude: Option<f64>,
        longitude: Option<f64>,
        label: Option<&str>,
        address: Option<&str>,
    ) -> Option<Self> {
        let (latitude, longitude) = (latitude?, longitude?);
        if !(latitude.is_finite() && longitude.is_finite() && latitude.abs() <= 90.0 && longitude.abs() <= 180.0) {
            return None;
        }
        let words = |text: Option<&str>| {
            text.map(str::trim)
                .filter(|text| !text.is_empty())
                .map(|text| text.chars().take(LOCATION_TEXT_MAX).collect::<String>())
        };
        Some(Self {
            latitude,
            longitude,
            label: words(label),
            address: words(address),
        })
    }
}

/// A tapped suggestion's payload, cut to [`POSTBACK_MAX`]; none when empty.
pub fn postback_of(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(POSTBACK_MAX).collect())
}

/// A text a person sent us.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inbound {
    /// The webhook event's id: the ingest transaction, so a repeated webhook does nothing again.
    pub event_id: String,
    /// The provider's id of the message: the room event's external id, so the same message in another event is one.
    pub message_id: String,
    pub our_number: String,
    pub their_number: String,
    pub text: Option<String>,
    pub media: Vec<InboundMedia>,
    /// Every number a group text went to, our number among them (none or one for a text to one number).
    pub recipients: Vec<String>,
    pub origin_millis: Option<i64>,
    /// The provider's conversation it came in (a Twilio Conversation of a group text): the room of the mapping that keeps
    /// it (`ChatRoomBridge.externalThreadId`, [`store_thread`]), whoever writes.
    pub thread_id: Option<String>,
    /// A place the person shared (RCS).
    pub location: Option<Location>,
    /// The payload of a suggestion the person tapped (RCS; R2's chips answer to it).
    pub postback: Option<String>,
    /// The provider account the payload names (Twilio's `AccountSid`, Sinch's `project_id/app_id`): an RCS bridge hands
    /// a text to a number of an SMS bridge only of this same account ([`SmsProvider::account_of`]).
    pub account: Option<String>,
}

/// A file a text that came in names, for the core to fetch ([`SmsProvider::media_auth`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InboundMedia {
    pub url: String,
    pub content_type: Option<String>,
}

/// A sent text's state at its recipient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptStatus {
    Sent,
    Delivered,
    /// Read on the phone (an RCS bridge's: Twilio's `read`, Sinch's `READ`).
    Read,
    Unconfirmed,
    Failed,
}

impl ReceiptStatus {
    /// Its name in `note#DeliveryReceipt`.
    pub fn as_str(self) -> &'static str {
        match self {
            ReceiptStatus::Sent => "sent",
            ReceiptStatus::Delivered => "delivered",
            ReceiptStatus::Read => "read",
            ReceiptStatus::Unconfirmed => "unconfirmed",
            ReceiptStatus::Failed => "failed",
        }
    }
}

/// A delivery receipt of a text we sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// The delivery's remote id ([`SmsProvider::accepted_id`]: a group text's own).
    pub remote_event_id: String,
    pub status: ReceiptStatus,
    pub error: Option<String>,
    /// A group text's: the one recipient this receipt is of.
    pub recipient_number: Option<String>,
    /// A group text's: the recipient's own message id.
    pub other_remote_event_id: Option<String>,
    pub receipt_millis: Option<i64>,
    /// An opt-out the receipt tells of (Twilio's failed status with 21610: the person sent STOP), applied after it,
    /// once by its event id.
    pub opt_signal: Option<OptSignal>,
    /// The way the message went, when the report tells it (an RCS bridge's: RCS, or the fallback's SMS or MMS).
    pub path: Option<Path>,
}

/// A keyword a provider tells of apart from a text (an opt-out it took itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    Stop,
    Start,
}

/// A person opted out of our number, or back in, as the provider tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptSignal {
    /// The provider's id of the event: applied once (`SmsServices.note#SmsOptSignal` keeps it), so a copy the replay
    /// cache no longer holds changes nothing.
    pub event_id: String,
    pub our_number: String,
    pub their_number: String,
    pub keyword: Keyword,
}

/// What a verified webhook carries, in neutral terms.
#[derive(Clone, Debug, PartialEq)]
pub enum SmsEvent {
    Inbound(Inbound),
    Receipt(Receipt),
    OptSignal(OptSignal),
    /// A report that tells only the way a message went (Sinch's `SWITCHING_CHANNEL`: on to the fallback).
    Path {
        remote_event_id: String,
        path: Path,
    },
    /// Nothing to do: answered 200 `{"ignored": ...}` so the provider stops.
    Ignored(String),
}

/// What differs between SMS providers. One value serves every bridge of its adapter; each bridge's settings are its own
/// (`Self::Config`, parsed from the bridge's `configJson` on each call). State a provider keeps in itself (an OAuth
/// token cache) is shared by every bridge of the adapter and every thread: key it by the bridge id or the credential's
/// id, never by nothing, and guard it with `moqui_lib::sync`.
///
/// Replay: the core's replay cache drops a copy of a signature it saw within its window; past that, texts dedupe by
/// their ids (`ingest#Event`), receipts never move a delivery back, and an [`OptSignal`] is applied once by its event
/// id. A provider whose signature has a timestamp (Telnyx, Sinch) also refuses an old one in [`Self::verify`]; one
/// whose signature has none (Twilio) relies on those.
pub trait SmsProvider: Send + Sync + Sized + 'static {
    /// The provider's own settings: keys, account ids, the webhook's key or secret.
    type Config;

    /// Its name in messages (`Telnyx`).
    fn name(&self) -> &'static str;
    /// The network its adapter speaks: [`Channel::Sms`] by default. An RCS adapter's messages are encrypted in transit
    /// only (`Encryption::Transport`), and each sent message's path is recorded (`note#DeliveryPath`).
    fn channel(&self) -> Channel {
        Channel::Sms
    }
    /// The most bytes of one picture of a rich message on its RCS channel (`maxRcsMediaBytes` overrides): Twilio's 16 MB
    /// by default.
    fn rcs_media_bytes(&self) -> i64 {
        16 * 1024 * 1024
    }
    /// The adapter name a `ChatBridge` selects it by (`adapterName`; Telnyx's is `sms`).
    fn adapter_name(&self) -> &'static str;
    /// An RCS provider's SMS adapter of the same provider (`sms-twilio` for `rcs-twilio`): a reply to a fallback text
    /// that comes to a number of a bridge of it goes to that number's room ([`SmsAdapter`]'s hand-over). None by default.
    fn sms_adapter_name(&self) -> Option<&'static str> {
        None
    }
    /// The provider account a bridge's `configJson` names, in the form of [`Inbound::account`] (Twilio's `accountSid`,
    /// Sinch's `projectId/appId`). None by default: no text is handed over.
    fn account_of(&self, config: &Json) -> Option<String> {
        let _ = config;
        None
    }
    /// Its defaults for the neutral [`Settings`].
    fn defaults(&self) -> Defaults;
    fn config(&self, config: &Json) -> Self::Config;
    /// Why a bridge cannot start with these settings (a missing key); the core checks the API base is https after it.
    ///
    /// # Errors
    ///
    /// Returns the bridge's error, shown as its last error.
    fn check(&self, config: &Self::Config) -> Result<(), String>;
    /// The most outside numbers of a group text (Telnyx 8); 0 when it sends none, and a group room then refuses.
    fn group_max(&self) -> usize;

    /// The request that sends `out` (to one number, or a group text to several).
    fn request(&self, config: &Self::Config, settings: &Settings, out: &Outgoing<'_>) -> ApiRequest;
    /// The provider's id of an accepted send, from its answer's body (a group text's own id, which its receipts name).
    fn accepted_id(&self, out: &Outgoing<'_>, body: &str) -> Option<String>;
    /// The error an answer's body carries.
    fn error_of(&self, body: &str) -> ProviderError;
    /// What a refused send means, by its status and error.
    fn refusal(&self, status: u16, error: &ProviderError) -> Refusal;
    /// Send `out`: by default the one [`Self::request`], its answer read by the core's classes ([`exchange`]). A
    /// provider whose group text takes several calls overrides it, sending each through `client` (the guarded one).
    fn send(
        &self,
        client: &RestClient,
        config: &Self::Config,
        settings: &Settings,
        out: &Outgoing<'_>,
    ) -> DeliveryOutcome {
        exchange(self, client, out, &self.request(config, settings, out))
    }

    /// The header its webhooks are signed in: the replay cache's key, and what an edge node checks is there.
    fn signature_header(&self) -> &'static str;
    /// Whether an edge node can check its signature with the bridge's replicated copy (a public key: Telnyx); else the
    /// local node checks it, with the secret only it holds.
    fn verifies_on_edge(&self) -> bool;
    /// Whether `request` is the provider's: its signature (over [`webhook_public_url`] when the provider signs the URL),
    /// and when the signature has a timestamp, that it is within `settings.tolerance_seconds` of `request.now_millis`
    /// (when the request came, also for one an edge node queued). It keeps no replay state (the core's replay cache
    /// does), so it can check a queued request again.
    fn verify(&self, bridge: &BridgeInfo, config: &Self::Config, settings: &Settings, request: &WebhookRequest)
        -> bool;
    /// Whether this node has what [`Self::verify`] needs for the bridge (its key or secret): an error names what is
    /// missing, and a queued request waits instead of being refused as forged. The default has all it needs.
    ///
    /// # Errors
    ///
    /// Returns what is missing.
    fn can_verify(&self, config: &Self::Config) -> Result<(), String> {
        let _ = config;
        Ok(())
    }
    /// The event a webhook carries, for an edge node's queue: its id (a repeat has the same) and their number. None when
    /// the body cannot be read. For a provider that verifies on the local node it reads a body nobody verified yet (at
    /// most `channels_edge_unverified_max_bytes`): read only the key.
    fn event_key(&self, request: &WebhookRequest) -> Option<WebhookEventKey>;
    /// The event a verified webhook carries; or the answer, when it is malformed (400).
    ///
    /// # Errors
    ///
    /// Returns the answer to a body that names no event this adapter can work.
    fn parse(&self, request: &WebhookRequest) -> Result<SmsEvent, WebhookResponse>;
    /// [`Self::parse`] with the bridge's settings, for a provider whose events need them (Sinch's opt-outs name no
    /// number of ours: its `smsSender` is); by default [`Self::parse`].
    ///
    /// # Errors
    ///
    /// Returns the answer to a body that names no event this adapter can work.
    fn parse_with(&self, config: &Self::Config, request: &WebhookRequest) -> Result<SmsEvent, WebhookResponse> {
        let _ = config;
        self.parse(request)
    }
    /// [`Self::parse`] on the local node with the bridge's whole configuration and the guarded client, for a provider
    /// whose event needs what it keeps or what its API says (Twilio's Conversations: a group's numbers). The default is
    /// [`Self::parse_with`].
    ///
    /// # Errors
    ///
    /// Returns the answer to a body that names no event this adapter can work, or 503 when what it needs cannot be read
    /// now.
    fn parse_on(
        &self,
        client: &RestClient,
        bridge: &BridgeInfo,
        config: &Self::Config,
        settings: &Settings,
        request: &WebhookRequest,
    ) -> Result<SmsEvent, WebhookResponse> {
        let _ = (client, bridge, settings);
        self.parse_with(config, request)
    }
    /// The answer the provider gets, from the core's (Twilio wants TwiML): on this node, and an edge node's to a request
    /// it queued (2xx) or refused for its rate (429). It leaves an unsigned request's 401 as it is. The default is the
    /// core's.
    fn respond(&self, response: WebhookResponse) -> WebhookResponse {
        response
    }

    /// The header a file a text names is fetched with (Basic auth), when the provider's files need one. The client
    /// sends it only to the file's own host: a redirect to another drops it.
    fn media_auth(&self, config: &Self::Config) -> Option<(String, String)> {
        let _ = config;
        None
    }

    /// Show `theirs` that a user of ours is typing, from `ours` (an RCS provider's typing indicator), through `client`
    /// (the guarded one; [`plain`] sends one request); nothing when the provider has none or the bridge lacks what it
    /// needs. Nothing by default.
    ///
    /// # Errors
    ///
    /// Returns why it was not sent.
    fn send_typing(
        &self,
        client: &RestClient,
        config: &Self::Config,
        settings: &Settings,
        ours: &str,
        theirs: &str,
    ) -> Result<(), String> {
        let _ = (client, config, settings, ours, theirs);
        Ok(())
    }
    /// The contents this provider made for sends before `before_millis` (ours by their name: Twilio's `moqui-...`
    /// Contents), for the sweep of those no delivery keeps. None by default.
    ///
    /// # Errors
    ///
    /// Returns why they could not be read.
    fn orphan_contents(
        &self,
        client: &RestClient,
        config: &Self::Config,
        settings: &Settings,
        before_millis: i64,
    ) -> Result<Vec<String>, String> {
        let _ = (client, config, settings, before_millis);
        Ok(Vec::new())
    }
    /// Delete a content this provider made for a send (a Twilio Content resource, [`keep_content`]); one it no longer
    /// has is deleted. Nothing by default.
    ///
    /// # Errors
    ///
    /// Returns why it was not deleted.
    fn delete_content(
        &self,
        client: &RestClient,
        config: &Self::Config,
        settings: &Settings,
        content_id: &str,
    ) -> Result<(), String> {
        let _ = (client, config, settings, content_id);
        Ok(())
    }
    /// The request of the provider's paid number lookup of `number`, for the spam filter (plans/Comms-Messaging.md
    /// §18.3 phase 11); None, the default, when it has none.
    fn lookup_request(&self, config: &Self::Config, settings: &Settings, number: &str) -> Option<ApiRequest> {
        let _ = (config, settings, number);
        None
    }
    /// What the lookup's answer says of the number; None when it says nothing this filter reads.
    fn lookup_of(&self, body: &str) -> Option<NumberLookup> {
        let _ = body;
        None
    }
}

// ---- the adapter ----

/// The chat bridge adapter of one SMS provider.
pub struct SmsAdapter<P> {
    provider: P,
    /// Clients for public addresses only, and (a test's fake provider, `allowInternalAddresses`) for any.
    public: OnceLock<RestClient>,
    internal: OnceLock<RestClient>,
}

impl<P: SmsProvider> SmsAdapter<P> {
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            public: OnceLock::new(),
            internal: OnceLock::new(),
        }
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }

    /// The bridge's neutral settings.
    pub fn settings(&self, bridge: &BridgeInfo) -> Settings {
        Settings::of(&bridge.config, &self.provider.defaults())
    }

    fn client(&self, allow_internal: bool) -> &RestClient {
        let make = |deny: bool| {
            RestClient::new()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .max_response_bytes(MEDIA_CAP)
                // to any host, as a provider's file store answers from another; reqwest drops Authorization on a hop
                // to another host
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

/// Our number and the outside numbers, from a mapping's external room id: a pair's (`ours/theirs`) or a group text's
/// (`ours/a,b,c`, plans/Comms-Messaging.md §9), at most [`NUMBERS_READ`] of them; a text member's (`ours/theirs#room`,
/// [`member_of`]) is its pair. None for anything else.
pub fn numbers_of(external_room_id: &str) -> Option<(String, Vec<String>)> {
    let numbers = external_room_id
        .split_once('#')
        .map_or(external_room_id, |(numbers, _)| numbers);
    let (ours, theirs) = numbers.split_once('/')?;
    let valid = |number: &str| {
        number
            .strip_prefix('+')
            .is_some_and(|digits| digits.len() > 3 && digits.bytes().all(|b| b.is_ascii_digit()))
    };
    let theirs: Vec<String> = theirs.split(',').map(str::to_string).collect();
    let fits = !theirs.is_empty() && theirs.len() <= NUMBERS_READ && theirs.iter().all(|number| valid(number));
    (valid(ours) && fits && !theirs.iter().any(|number| number == ours)).then(|| (ours.to_string(), theirs))
}

/// A text member's mapping (plans/Comms-Messaging.md §18.3 phase 18): our number, theirs and the room, from its external
/// room id `ours/theirs#chatRoomId`. None for any other mapping.
pub fn member_of(external_room_id: &str) -> Option<(String, String, String)> {
    let (numbers, room) = external_room_id.split_once('#')?;
    let room = room.trim();
    if room.is_empty()
        || room.len() > 64
        || !room
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return None;
    }
    let (ours, theirs) = pair_of(numbers)?;
    Some((ours, theirs, room.to_string()))
}

/// What a text member gets of a room's message: `<sender>: <text>` (the sender's name cut to 30 characters, the whole cut
/// to `max_text` characters, ending `…`), the photos, videos and sounds the bridge's MMS limits take, and a line for each
/// other file, which a text member could not open from a link. No rich cards: their text is the fallback.
pub fn member_request(request: &DeliveryRequest, max_text: usize, max_count: usize, max_bytes: i64) -> DeliveryRequest {
    let name: String = request
        .sender_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("?")
        .chars()
        .take(30)
        .collect();
    let mut lines: Vec<String> = Vec::new();
    let mut kept = Vec::new();
    let mut bytes = 0_i64;
    for file in &request.attachments {
        let media = file
            .content_type
            .as_deref()
            .is_some_and(|kind| kind.starts_with("image/") || kind.starts_with("video/") || kind.starts_with("audio/"));
        let size = file.file_size.unwrap_or(0);
        if media && kept.len() < max_count && bytes + size <= max_bytes {
            bytes += size;
            kept.push(file.clone());
        } else {
            lines.push(format!(
                "[{}: a file, not sent by text]",
                file.file_name.as_deref().unwrap_or("a file")
            ));
        }
    }
    let body = request.text.as_deref().map(str::trim).unwrap_or_default();
    let mut text = if body.is_empty() && !kept.is_empty() && lines.is_empty() {
        format!("{name}:")
    } else {
        let words = [body.to_string()]
            .into_iter()
            .chain(lines)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        format!("{name}: {words}")
    };
    if text.chars().count() > max_text.max(2) {
        text = text.chars().take(max_text.max(2) - 1).collect::<String>() + "…";
    }
    DeliveryRequest {
        text: Some(text),
        html: None,
        rich: None,
        attachments: kept,
        ..request.clone()
    }
}

/// Our number and theirs, from a mapping's external room id (`ours/theirs`); none for a group's.
pub fn pair_of(external_room_id: &str) -> Option<(String, String)> {
    match numbers_of(external_room_id)? {
        (ours, theirs) if theirs.len() == 1 => Some((ours, theirs.into_iter().next()?)),
        _ => None,
    }
}

/// A text that did not reach the provider: its places in the day's count (check#SmsSend reserve, one a recipient) go
/// back.
fn release(bridge: &BridgeInfo, reserved: Vec<BTreeMap<String, Value>>) {
    for parameters in reserved {
        if let Err(error) = crate::call("org.moqui.call.SmsServices.release#SmsSend", parameters) {
            tracing::warn!(
                "a text of bridge {} was not taken off the day's count: {error}",
                bridge.chat_bridge_id
            );
        }
    }
}

fn failed(reason: &str, error: impl Into<String>) -> DeliveryOutcome {
    DeliveryOutcome::Failed {
        reason_code: reason.to_string(),
        error: error.into(),
    }
}

/// An RCS bridge's `smsFallback`: true unless it is false (or the text `false`).
pub fn sms_fallback(config: &Json) -> bool {
    !config
        .get("smsFallback")
        .is_some_and(|value| value.as_bool() == Some(false) || value.as_str() == Some("false"))
}

/// Whether an HTTP error is one the provider never got: the connection could not be made.
pub fn never_sent(error: &moqui_lib::error::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(cause) = source {
        if let Some(http) = cause.downcast_ref::<reqwest::Error>() {
            return http.is_connect() || http.is_builder();
        }
        source = cause.source();
    }
    false
}

/// `pairs` form-encoded (`application/x-www-form-urlencoded`, a space as `+`).
pub fn form_encode(pairs: &[(String, String)]) -> String {
    let encode = |text: &str| {
        let mut out = String::with_capacity(text.len());
        for byte in text.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => out.push(char::from(byte)),
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
        out
    };
    let parts: Vec<String> = pairs
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect();
    parts.join("&")
}

/// Send `request`, whose answer is only its status (a typing indicator, a delete), through `client`: 2xx is done, and
/// 404 too when `gone_is_done`.
///
/// # Errors
///
/// Returns the provider's answer, or why it was not reached.
pub fn plain<P: SmsProvider>(
    provider: &P,
    client: &RestClient,
    request: &ApiRequest,
    gone_is_done: bool,
) -> Result<(), String> {
    let response = call(client, request).map_err(|error| error.message().to_string())?;
    let status = response.status();
    if (200..300).contains(&status) || (gone_is_done && status == 404) {
        Ok(())
    } else {
        Err(format!(
            "{} answered {status}: {}",
            provider.name(),
            provider.error_of(response.body()).text
        ))
    }
}

/// Send `request` through `client` (no answer kept but the response).
///
/// # Errors
///
/// Returns the client's error: the request could not be made, or its answer was lost ([`never_sent`] tells them apart).
pub fn call(client: &RestClient, request: &ApiRequest) -> Result<RestResponse, moqui_lib::error::Error> {
    let mut call = client.request(request.method, request.url.clone());
    for (name, value) in &request.headers {
        call = call.header(name.clone(), value.clone());
    }
    call = match &request.body {
        ApiBody::None => call,
        ApiBody::Json(body) => call.json(body),
        ApiBody::Form(pairs) => call.text("application/x-www-form-urlencoded", form_encode(pairs)),
    };
    call.send(false)
}

/// The provider refused a text to `out`'s one number as opted out (Twilio 21610, Telnyx 40300): the pair's opt-out
/// (`SmsServices.note#SmsKeyword` STOP), so the STOP table agrees with the provider.
fn mark_opted_out(out: &Outgoing<'_>) {
    let Some(theirs) = out.theirs.first() else {
        return;
    };
    let parameters = BTreeMap::from([
        ("ourNumber".to_string(), Value::string(out.ours)),
        ("theirNumber".to_string(), Value::string(theirs)),
        ("keyword".to_string(), Value::string("STOP")),
    ]);
    if let Err(error) = crate::call("org.moqui.call.SmsServices.note#SmsKeyword", parameters) {
        tracing::warn!(
            "SMS bridge {}: the provider's opt-out of {theirs} was not kept: {error}",
            out.bridge.chat_bridge_id
        );
    }
}

/// Keep `thread_id`, the conversation the provider made for the mapping of `external_room_id` (a Twilio Conversation
/// of a group text), on that mapping (`ChatBridgeServices.store#MappingThread`, in a transaction of its own): later
/// sends carry it ([`DeliveryRequest::external_thread_id`]) and a text that names it ([`Inbound::thread_id`]) comes to
/// the mapping's room. An empty `thread_id` lets the mapping's go (the conversation is gone or closed).
///
/// # Errors
///
/// Returns why it was not kept (no active mapping, another mapping of the bridge has it).
pub fn store_thread(bridge: &BridgeInfo, external_room_id: &str, thread_id: &str) -> Result<(), String> {
    let parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("externalRoomId".to_string(), Value::string(external_room_id)),
        ("externalThreadId".to_string(), Value::string(thread_id)),
    ]);
    crate::call("org.moqui.chat.ChatBridgeServices.store#MappingThread", parameters).map(|_| ())
}

/// Send `request` for `out` and read its answer by class (see the module notes): the outcome of one send.
pub fn exchange<P: SmsProvider>(
    provider: &P,
    client: &RestClient,
    out: &Outgoing<'_>,
    request: &ApiRequest,
) -> DeliveryOutcome {
    answer(provider, out, call(client, request))
}

/// The outcome of one send from what [`call`] gave, by class (see the module notes): for a provider that reads some
/// answers itself first (Sinch's 401, 403 and 404) and leaves the rest to the core.
pub fn answer<P: SmsProvider>(
    provider: &P,
    out: &Outgoing<'_>,
    result: Result<RestResponse, moqui_lib::error::Error>,
) -> DeliveryOutcome {
    let name = provider.name();
    let response = match result {
        Err(error) if never_sent(&error) => {
            return DeliveryOutcome::Retry {
                error: format!("{name} could not be reached: {}", error.message()),
                not_connected: true,
            }
        }
        Err(error) => {
            return DeliveryOutcome::Uncertain {
                error: format!("{name}'s answer was lost: {}", error.message()),
            }
        }
        Ok(response) => response,
    };
    let status = response.status();
    match status {
        200..=299 => match provider.accepted_id(out, response.body()) {
            Some(id) if !id.is_empty() => DeliveryOutcome::Accepted { remote_event_id: id },
            _ => DeliveryOutcome::Uncertain {
                error: format!("{name} took the text but named no message id"),
            },
        },
        // not taken: a rate limit, the service away
        429 | 503 => DeliveryOutcome::Retry {
            error: format!("{name} answered {status}: {}", provider.error_of(response.body()).text),
            not_connected: false,
        },
        // the key: nothing goes until an administrator fixes it
        401 | 403 => DeliveryOutcome::Retry {
            error: format!("{name} refused the API key ({status})"),
            not_connected: true,
        },
        400..=499 => {
            let error = provider.error_of(response.body());
            match provider.refusal(status, &error) {
                Refusal::Failed(reason) => {
                    // the provider holds the person's STOP: the pair's opt-out says so too, so the next text is
                    // refused before the provider (a group text's refusal names no one recipient)
                    if reason == "optedOut" && !out.group() {
                        mark_opted_out(out);
                    }
                    failed(reason, error.text)
                }
                Refusal::Retry => DeliveryOutcome::Retry {
                    error: format!("{name} answered {status}: {}", error.text),
                    not_connected: false,
                },
            }
        }
        _ => DeliveryOutcome::Uncertain {
            error: format!(
                "{name} answered {status}; the text may have been sent: {}",
                provider.error_of(response.body()).text
            ),
        },
    }
}

impl<P: SmsProvider> ChatBridgeAdapter for SmsAdapter<P> {
    fn name(&self) -> &str {
        self.provider.adapter_name()
    }

    fn capabilities(&self, bridge: &BridgeInfo) -> Capabilities {
        Capabilities {
            edit: false,
            delete: false,
            reaction: false,
            idempotent: false,
            sends_per_minute: self.settings(bridge).sends_per_minute,
            // SMS: not even Transport, a text crosses the phone network in the clear, whatever TLS reaches the provider
            // (plans/Comms-Messaging.md §6.3); business RCS is encrypted in transit only, never end to end (§16.1)
            encryption: match self.provider.channel() {
                Channel::Sms => Encryption::None,
                Channel::Rcs => Encryption::Transport,
            },
            group_max: u32::try_from(self.provider.group_max()).unwrap_or(u32::MAX),
        }
    }

    fn start(&self, bridge: &BridgeInfo) -> Result<(), String> {
        self.provider.check(&self.provider.config(&bridge.config))?;
        let settings = self.settings(bridge);
        // the API key goes with every send, so never in clear text, except to a test's fake (SEC-COM-10)
        if !(settings.api_base_url.starts_with("https://")
            || (settings.allow_internal && settings.api_base_url.starts_with("http://")))
        {
            return Err("The SMS bridge's apiBaseUrl is not an https address".to_string());
        }
        Ok(())
    }

    fn deliver(&self, bridge: &BridgeInfo, request: &DeliveryRequest) -> DeliveryOutcome {
        if request.kind != DeliveryKind::Message {
            return failed("bridgeUnsupported", "A text cannot be changed once sent");
        }
        let Some((ours, theirs)) = numbers_of(&request.external_room_id) else {
            return failed("badMapping", "The room's mapping is not a pair of numbers");
        };
        if theirs.len() > 1 {
            let max = self.provider.group_max();
            if max == 0 {
                return failed(
                    "groupTextsNotSupported",
                    format!(
                        "{} does not carry group texts: this group's texts are not sent from this number",
                        self.provider.name()
                    ),
                );
            }
            if theirs.len() > max {
                return failed(
                    "tooManyNumbers",
                    format!("A group text has at most {max} other numbers"),
                );
            }
        }
        // a direct send (send#BridgeDirect) has no room: its caller checked it and holds its place in the day's count
        let in_room = !request.chat_room_id.is_empty();
        // the day's counts this send holds (check#SmsSend reserve, one a recipient), let go when it does not reach the
        // provider
        let mut reserved: Vec<BTreeMap<String, Value>> = Vec::new();
        // a room's text member (plans/Comms-Messaging.md §18.3 phase 18): the room sends, whoever wrote, checked in room
        // mode (the number carries room texts, the pair, the room's and all users' day), and the text names its sender
        let member = in_room && member_of(&request.external_room_id).is_some();
        let shaped;
        let request = if member {
            if request.kind != DeliveryKind::Message {
                return failed("bridgeUnsupported", "A text member gets messages only");
            }
            let parameters = BTreeMap::from([
                ("fromNumber".to_string(), Value::string(&ours)),
                ("toNumber".to_string(), Value::string(&theirs[0])),
                ("chatRoomId".to_string(), Value::string(&request.chat_room_id)),
                ("reserve".to_string(), Value::Bool(true)),
            ]);
            match crate::call("org.moqui.call.SmsServices.check#SmsSend", parameters) {
                Ok(out) if out.get("allowed") == Some(&Value::Bool(true)) => reserved.push(BTreeMap::from([
                    ("chatRoomId".to_string(), Value::string(&request.chat_room_id)),
                    ("fromNumber".to_string(), Value::string(&ours)),
                    // the adder's day it counted on too
                    (
                        "userId".to_string(),
                        out.get("counterUserId").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "counterDay".to_string(),
                        out.get("counterDay").cloned().unwrap_or(Value::Null),
                    ),
                ])),
                Ok(out) => {
                    let text = |key: &str| out.get(key).map(to_plain_string).unwrap_or_default();
                    return failed(&text("reasonCode"), text("message"));
                }
                Err(error) => {
                    return DeliveryOutcome::Retry {
                        error: format!("The send could not be checked: {error}"),
                        not_connected: false,
                    }
                }
            }
            let settings = self.settings(bridge);
            shaped = member_request(
                request,
                settings.max_text_length,
                settings.max_media_count,
                settings.max_media_bytes,
            );
            &shaped
        } else {
            request
        };
        if in_room && !member {
            // a text in a room goes from a user granted our number, never as the system
            let Some(user) = request.sender_user_id.as_deref().filter(|user| !user.is_empty()) else {
                return failed("numberNotGranted", "A text goes from a user granted the number");
            };
            // each number a group text goes to is checked as a text to it would be, and counts as one: the carrier
            // bills a group MMS for each recipient, so the day's limits count each
            for to in &theirs {
                let parameters = BTreeMap::from([
                    ("fromNumber".to_string(), Value::string(&ours)),
                    ("toNumber".to_string(), Value::string(to)),
                    ("userId".to_string(), Value::string(user)),
                    ("reserve".to_string(), Value::Bool(true)),
                    // a pair room's text: refused while the number is a text member of a room on ours
                    // (inRoomByText); a group text with them goes as ever, since its replies come to the group
                    ("inRoom".to_string(), Value::Bool(theirs.len() == 1)),
                ]);
                let refusal = match crate::call("org.moqui.call.SmsServices.check#SmsSend", parameters) {
                    Ok(out) if out.get("allowed") == Some(&Value::Bool(true)) => {
                        reserved.push(BTreeMap::from([
                            ("userId".to_string(), Value::string(user)),
                            (
                                "counterDay".to_string(),
                                out.get("counterDay").cloned().unwrap_or(Value::Null),
                            ),
                        ]));
                        None
                    }
                    Ok(out) => {
                        let text = |key: &str| out.get(key).map(to_plain_string).unwrap_or_default();
                        Some(failed(&text("reasonCode"), text("message")))
                    }
                    Err(error) => Some(DeliveryOutcome::Retry {
                        error: format!("The send could not be checked: {error}"),
                        not_connected: false,
                    }),
                };
                if let Some(refusal) = refusal {
                    release(bridge, std::mem::take(&mut reserved));
                    return refusal;
                }
            }
        }
        let outcome = self.send(bridge, request, &ours, &theirs);
        // not sent (refused here or by the provider, or to be tried again): its place in the day's count goes back
        if matches!(
            outcome,
            DeliveryOutcome::Failed { .. } | DeliveryOutcome::Retry { .. } | DeliveryOutcome::Wait { .. }
        ) {
            release(bridge, reserved);
        }
        outcome
    }

    fn webhook(&self, bridge: &BridgeInfo, request: &WebhookRequest) -> WebhookResponse {
        self.handle(bridge, request)
    }

    fn verify_webhook(&self, bridge: &BridgeInfo, request: &WebhookRequest) -> Option<WebhookResponse> {
        let config = self.provider.config(&bridge.config);
        (!self.provider.verify(bridge, &config, &self.settings(bridge), request)).then(WebhookResponse::not_signed)
    }

    fn webhook_verifiable(&self, bridge: &BridgeInfo) -> Result<(), String> {
        self.provider.can_verify(&self.provider.config(&bridge.config))
    }

    fn edge_answer(&self, _bridge: &BridgeInfo, response: WebhookResponse) -> WebhookResponse {
        self.provider.respond(response)
    }

    fn webhook_verification(&self) -> WebhookVerification {
        if self.provider.verifies_on_edge() {
            WebhookVerification::Edge
        } else {
            WebhookVerification::Core {
                signature_header: self.provider.signature_header(),
            }
        }
    }

    fn webhook_event_id(&self, _bridge: &BridgeInfo, request: &WebhookRequest) -> Option<WebhookEventKey> {
        self.provider.event_key(request)
    }

    fn apply_webhook(&self, bridge: &BridgeInfo, request: &WebhookRequest) -> WebhookResponse {
        self.apply(bridge, request)
    }

    fn fetch_media(&self, bridge: &BridgeInfo, url: &str, max_bytes: u64) -> Result<FetchedMedia, String> {
        self.fetch(bridge, url, max_bytes)
    }

    fn typing(&self, bridge: &BridgeInfo, external_room_id: &str) -> Result<(), String> {
        if self.provider.channel() != Channel::Rcs || !typing_on(&bridge.config) {
            return Ok(());
        }
        // a pair only: RBM has no groups; and no typing to a room's text member
        if member_of(external_room_id).is_some() {
            return Ok(());
        }
        let Some((ours, theirs)) = pair_of(external_room_id) else {
            return Ok(());
        };
        // a STOP ends every contact, a typing indicator too; and none to a person we never sent a message
        if opted_out(&ours, &theirs)? || !messaged(&bridge.chat_bridge_id, external_room_id)? {
            return Ok(());
        }
        let settings = self.settings(bridge);
        let config = self.provider.config(&bridge.config);
        self.provider
            .send_typing(self.client(settings.allow_internal), &config, &settings, &ours, &theirs)
    }

    fn delete_content(&self, bridge: &BridgeInfo, content_id: &str) -> Result<(), String> {
        let settings = self.settings(bridge);
        let config = self.provider.config(&bridge.config);
        self.provider
            .delete_content(self.client(settings.allow_internal), &config, &settings, content_id)
    }

    fn orphan_contents(&self, bridge: &BridgeInfo, before_millis: i64) -> Result<Vec<String>, String> {
        let settings = self.settings(bridge);
        let config = self.provider.config(&bridge.config);
        self.provider
            .orphan_contents(self.client(settings.allow_internal), &config, &settings, before_millis)
    }

    fn lookup_number(&self, bridge: &BridgeInfo, number: &str) -> Result<Option<NumberLookup>, String> {
        self.lookup(bridge, number)
    }
}

/// How long a number lookup may take: the text it judges waits for it.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

impl<P: SmsProvider> SmsAdapter<P> {
    /// The provider's number lookup ([`SmsProvider::lookup_request`]) through the guarded client, in
    /// [`LOOKUP_TIMEOUT`]: None when the provider has none; an error for no answer or one that is not a 2xx.
    fn lookup(&self, bridge: &BridgeInfo, number: &str) -> Result<Option<NumberLookup>, String> {
        let config = self.provider.config(&bridge.config);
        let settings = self.settings(bridge);
        let Some(request) = self.provider.lookup_request(&config, &settings, number) else {
            return Ok(None);
        };
        let name = self.provider.name();
        let mut call = self
            .client(settings.allow_internal)
            .request(request.method, request.url.clone())
            .timeout(LOOKUP_TIMEOUT);
        for (header, value) in &request.headers {
            call = call.header(header.clone(), value.clone());
        }
        let response = call
            .send(false)
            .map_err(|error| format!("{name}'s number lookup: {}", error.message()))?;
        if !(200..300).contains(&response.status()) {
            return Err(format!("{name}'s number lookup answered {}", response.status()));
        }
        Ok(self.provider.lookup_of(response.body()))
    }
}

impl<P: SmsProvider> SmsAdapter<P> {
    /// A checked text, to the provider, within the bridge's limits and with its files' addresses.
    fn send(&self, bridge: &BridgeInfo, request: &DeliveryRequest, ours: &str, theirs: &[String]) -> DeliveryOutcome {
        let settings = self.settings(bridge);
        let text = request.text.as_deref().unwrap_or_default();
        if text.chars().count() > settings.max_text_length {
            return failed(
                "textTooLong",
                format!("A text is at most {} characters", settings.max_text_length),
            );
        }
        let rcs = self.provider.channel() == Channel::Rcs;
        // a rich message (its files are its cards' pictures): the cards go only to an RCS provider; any other sends the
        // fallback (the text) with only the first picture, so a carousel never fails for its count
        let rich = request
            .rich
            .as_ref()
            .and_then(|value| Rich::of(value, request.attachments.len()));
        let trimmed;
        let request = match &rich {
            Some(rich) if !rcs => {
                trimmed = DeliveryRequest {
                    attachments: rich
                        .first_image()
                        .and_then(|index| request.attachments.get(index))
                        .cloned()
                        .into_iter()
                        .collect(),
                    ..request.clone()
                };
                &trimmed
            }
            _ => request,
        };
        let rich = rich.filter(|_| rcs);
        // the fallback's picture (MMS): the first, when it is within the MMS limit; else the fallback is text alone
        let mut fallback_image = None;
        if let Some(rich) = &rich {
            // each picture within the provider's RCS limit, all within a sane total; never the MMS total, which only the
            // fallback's one picture meets
            let per_file = number_of(&bridge.config, "maxRcsMediaBytes")
                .filter(|n| *n > 0)
                .unwrap_or_else(|| self.provider.rcs_media_bytes());
            let all = number_of(&bridge.config, "maxRcsTotalBytes")
                .filter(|n| *n > 0)
                .unwrap_or(RCS_TOTAL_BYTES);
            if request.attachments.len() > RCS_MEDIA_COUNT {
                return failed(
                    "tooManyFiles",
                    format!("A rich message has at most {RCS_MEDIA_COUNT} pictures"),
                );
            }
            if let Some(file) = request
                .attachments
                .iter()
                .find(|file| file.file_size.unwrap_or(0) > per_file)
            {
                return failed(
                    "mediaTooLarge",
                    format!(
                        "A card's picture is at most {per_file} bytes: {}",
                        file.file_name.as_deref().unwrap_or("a picture")
                    ),
                );
            }
            let total: i64 = request.attachments.iter().map(|file| file.file_size.unwrap_or(0)).sum();
            if total > all {
                return failed(
                    "mediaTooLarge",
                    format!("A rich message's pictures are at most {all} bytes in all"),
                );
            }
            fallback_image = rich.first_image().filter(|index| {
                request
                    .attachments
                    .get(*index)
                    .is_some_and(|file| file.file_size.unwrap_or(0) <= settings.max_media_bytes)
            });
        } else {
            if request.attachments.len() > settings.max_media_count {
                return failed(
                    "tooManyFiles",
                    format!("A text carries at most {} files", settings.max_media_count),
                );
            }
            let total: i64 = request.attachments.iter().map(|file| file.file_size.unwrap_or(0)).sum();
            if total > settings.max_media_bytes {
                return failed(
                    "mediaTooLarge",
                    format!("A text's files are at most {} bytes in all", settings.max_media_bytes),
                );
            }
        }
        let now = jiff::Timestamp::now().as_millisecond();
        // by address, the edge node's on a paired local node: not sent until the files are there
        let media_urls = match media_urls(&bridge.chat_bridge_id, request, now) {
            MediaUrls::Ready(urls) => urls,
            // not counted: the files are on their way to the edge node
            MediaUrls::Wait(error) => {
                return DeliveryOutcome::Wait {
                    error,
                    retry_after_ms: MEDIA_RETRY_MILLIS,
                }
            }
            MediaUrls::Unreachable(error) => return failed("mediaUnreachable", error),
        };
        let words = Some(text).filter(|text| !text.trim().is_empty());
        if words.is_none() && media_urls.is_empty() {
            return failed("empty", "A text has words or a file");
        }
        let out = Outgoing {
            bridge,
            request,
            ours,
            theirs,
            text: words,
            media_urls: &media_urls,
            fallback: !rcs || sms_fallback(&bridge.config),
            rich: rich.as_ref(),
            fallback_image,
        };
        let config = self.provider.config(&bridge.config);
        let outcome = self
            .provider
            .send(self.client(settings.allow_internal), &config, &settings, &out);
        // an RCS send that cannot fall back went as RCS: its path is known now (one that can is told by its reports)
        if rcs && !out.fallback && matches!(outcome, DeliveryOutcome::Accepted { .. }) {
            note_path(
                bridge,
                &[("chatBridgeDeliveryId", &request.chat_bridge_delivery_id)],
                Path::Rcs,
            );
        }
        outcome
    }

    /// Work a webhook request of an SMS bridge: the POST, the signature, the replay cache, then [`Self::apply`].
    fn handle(&self, bridge: &BridgeInfo, request: &WebhookRequest) -> WebhookResponse {
        if !request.method.eq_ignore_ascii_case("POST") {
            return WebhookResponse::text(405, "a webhook is a POST");
        }
        if let Some(refused) = self.verify_webhook(bridge, request) {
            return refused;
        }
        let signature = request.header(self.provider.signature_header()).unwrap_or_default();
        match replay_take(&bridge.chat_bridge_id, signature) {
            Ok(Replay::Done) => {
                return self
                    .provider
                    .respond(WebhookResponse::json(200, &json!({"outcome": "duplicate"})))
            }
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
        let response = self.apply(bridge, request);
        replay_finish(&bridge.chat_bridge_id, signature, (200..300).contains(&response.status));
        response
    }

    /// The event step of [`Self::handle`] alone, after the signature and replay checks: on any node after them, and on
    /// a local node for a request its edge node queued (verified there, or by `apply#EdgeWebhook` here).
    fn apply(&self, bridge: &BridgeInfo, request: &WebhookRequest) -> WebhookResponse {
        let worked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let settings = self.settings(bridge);
            let config = self.provider.config(&bridge.config);
            let client = self.client(settings.allow_internal);
            match self.provider.parse_on(client, bridge, &config, &settings, request) {
                Ok(event) => self.work(bridge, event),
                Err(answer) => answer,
            }
        }));
        let response = worked.unwrap_or_else(|_| WebhookResponse::text(500, "the request failed"));
        self.provider.respond(response)
    }

    fn work(&self, bridge: &BridgeInfo, event: SmsEvent) -> WebhookResponse {
        match event {
            SmsEvent::Inbound(text) => match self.handover(bridge, &text) {
                Handover::Here => received(bridge, &self.settings(bridge), &text),
                // the RCS bridge's verified webhook speaks for the number's own bridge, for this one text
                Handover::To(target) => with_mark(&target.chat_bridge_id, || {
                    received(&target, &self.settings(bridge), &text)
                }),
                Handover::Left(reason) => {
                    WebhookResponse::json(200, &json!({"outcome": "ignored", "reasonCode": reason, "message": ""}))
                }
                Handover::Unreadable => WebhookResponse::text(503, "the text was not taken; try again later"),
            },
            SmsEvent::Receipt(receipt) => {
                let (answer, content) = noted(bridge, &receipt);
                // the message is done: the content made for its send goes (best effort; the daily sweep has the rest)
                if let Some(content) = content {
                    self.drop_content(bridge, &receipt.remote_event_id, &content);
                }
                answer
            }
            SmsEvent::OptSignal(signal) => opted(bridge, &signal),
            SmsEvent::Path { remote_event_id, path } => {
                let found = note_path(bridge, &[("remoteEventId", &remote_event_id)], path);
                WebhookResponse::json(200, &json!({"found": found, "path": path.as_str()}))
            }
            SmsEvent::Ignored(what) => WebhookResponse::json(200, &json!({"ignored": what})),
        }
    }

    /// Where a text an RCS bridge took goes (plans/Comms-Messaging.md §16 step 11): here, unless its number of ours is
    /// on another bridge. Then to that bridge only when it is an enabled SMS bridge of the same provider and account
    /// (the payload's, the RCS bridge's and the SMS bridge's account the same), and only inside this bridge's verified
    /// webhook (its mark); else it is left, before anything is kept, and logged.
    fn handover(&self, bridge: &BridgeInfo, text: &Inbound) -> Handover {
        if self.provider.channel() != Channel::Rcs {
            return Handover::Here;
        }
        let read = || -> Result<Option<(BridgeInfo, bool)>, moqui_lib::error::Error> {
            let Some(facade) =
                moqui_core::context::ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()
            else {
                return Ok(None);
            };
            let number = facade
                .find("moqui.call.PhoneNumber")?
                .eq("phoneNumber", Value::string(&text.our_number))
                .disable_authz()
                .one()?;
            let Some(target) = number
                .and_then(|row| row.get("messagingBridgeId").map(to_plain_string))
                .filter(|id| !id.is_empty() && *id != bridge.chat_bridge_id)
            else {
                return Ok(None);
            };
            let Some(row) = facade
                .find("moqui.chat.ChatBridge")?
                .eq("chatBridgeId", Value::string(&target))
                .disable_authz()
                .one()?
            else {
                return Ok(None);
            };
            let field = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
            let disabled = field("statusId") == "CbsDisabled";
            Ok(Some((
                BridgeInfo {
                    chat_bridge_id: target,
                    type_enum_id: field("typeEnumId"),
                    adapter_name: field("adapterName"),
                    name: field("name"),
                    config: serde_json::from_str(&field("configJson")).unwrap_or(Json::Null),
                },
                disabled,
            )))
        };
        let (target, disabled) = match read() {
            Ok(Some(found)) => found,
            Ok(None) => return Handover::Here,
            Err(error) => {
                tracing::error!(
                    "RCS bridge {}: the bridge of {} could not be read: {error}",
                    bridge.chat_bridge_id,
                    text.our_number
                );
                return Handover::Unreadable;
            }
        };
        let leave = |reason: &'static str, why: &str| {
            tracing::warn!(
                "RCS bridge {}: a text to {} was left ({reason}): its number is on bridge {}, {why}",
                bridge.chat_bridge_id,
                text.our_number,
                target.chat_bridge_id
            );
            Handover::Left(reason)
        };
        if disabled {
            return leave("bridgeDisabled", "which is disabled");
        }
        if target.type_enum_id != "CbtSms" || Some(target.adapter_name.as_str()) != self.provider.sms_adapter_name() {
            return leave("otherProvider", "not an SMS bridge of this provider");
        }
        let account = text.account.as_deref().filter(|account| !account.is_empty());
        let same = account.is_some()
            && self.provider.account_of(&bridge.config).as_deref() == account
            && self.provider.account_of(&target.config).as_deref() == account;
        if !same {
            return leave("otherAccount", "of another provider account");
        }
        let marked = moqui_core::context::ExecutionContext::with_current(|ec| {
            ec.shared_map().get(WEBHOOK_MARK).map(to_plain_string)
        })
        .flatten();
        if marked.as_deref() != Some(bridge.chat_bridge_id.as_str()) {
            return leave("notInWebhook", "and this is not the RCS bridge's own webhook");
        }
        Handover::To(target)
    }

    /// Delete `content`, made for the send of `remote_event_id`, now its message is done, and clear it from the delivery;
    /// one not deleted is logged and stays for the sweep.
    fn drop_content(&self, bridge: &BridgeInfo, remote_event_id: &str, content: &str) {
        match ChatBridgeAdapter::delete_content(self, bridge, content) {
            Ok(()) => {
                if let Err(error) = clear_content(bridge, remote_event_id, content) {
                    tracing::warn!(
                        "{} bridge {}: content {content} was deleted and stays named on its delivery: {error}",
                        self.provider.name(),
                        bridge.chat_bridge_id
                    );
                }
            }
            Err(error) => tracing::warn!(
                "{} bridge {}: content {content} was not deleted (the daily sweep tries again): {error}",
                self.provider.name(),
                bridge.chat_bridge_id
            ),
        }
    }

    fn fetch(&self, bridge: &BridgeInfo, url: &str, max_bytes: u64) -> Result<FetchedMedia, String> {
        let settings = self.settings(bridge);
        let max = max_bytes.min(settings.max_inbound_media_bytes);
        let mut call = self
            .client(settings.allow_internal)
            .get(url)
            .timeout(Duration::from_secs(20))
            // read no further than the bridge's bound (the client's own is the 20 MiB cap)
            .max_response_bytes(max);
        if let Some((name, value)) = self.provider.media_auth(&self.provider.config(&bridge.config)) {
            call = call.header(name, value);
        }
        let response = call.send(false).map_err(|error| {
            if error.message().contains("larger than the limit") {
                format!("the file is over {max} bytes")
            } else {
                error.message().to_string()
            }
        })?;
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

// ---- the events ----

/// Where a text an RCS bridge took goes ([`SmsAdapter::handover`]).
#[derive(Debug)]
enum Handover {
    Here,
    /// To the room of its number on this bridge, an SMS bridge of the same provider and account.
    To(BridgeInfo),
    /// Left, with the reason code.
    Left(&'static str),
    /// The number's bridge could not be read now: tried again.
    Unreadable,
}

/// Run `work` with the context's webhook mark (`chat_bridge::WEBHOOK_MARK`, what `ingest#Event` takes a
/// transport-verified item by) set to `bridge_id`, and put back as it was after, also when `work` fails or panics.
pub(crate) fn with_mark<T>(bridge_id: &str, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<Value>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            let _ = moqui_core::context::ExecutionContext::with_current(|ec| match previous {
                Some(value) => {
                    ec.shared_map().insert(WEBHOOK_MARK.to_string(), value);
                }
                None => {
                    ec.shared_map().shift_remove(WEBHOOK_MARK);
                }
            });
        }
    }
    let previous = moqui_core::context::ExecutionContext::with_current(|ec| {
        ec.shared_map()
            .insert(WEBHOOK_MARK.to_string(), Value::string(bridge_id))
    })
    .flatten();
    let _restore = Restore(previous);
    work()
}

/// The reason codes of `ingest#Event` that refuse a text for what it is; any other failure is the server's own.
pub const REFUSALS: &[&str] = &[
    "unknownBridge",
    "badToken",
    "badTransaction",
    "batchTooLarge",
    "localIdentity",
];

/// A text that came in, to `SmsServices.receive#Sms` (the pair or group room, then `ingest#Event`, stored before this
/// answers).
fn received(bridge: &BridgeInfo, settings: &Settings, text: &Inbound) -> WebhookResponse {
    let message_id = &text.message_id;
    let media: Vec<Value> = text
        .media
        .iter()
        .map(|file| {
            moqui_lib::json::from_json(&json!({
                "url": file.url,
                "contentType": file.content_type,
                "maxBytes": settings.max_inbound_media_bytes,
            }))
        })
        .collect();
    let mut parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("eventId".to_string(), Value::string(&text.event_id)),
        ("messageId".to_string(), Value::string(message_id)),
        ("ourNumber".to_string(), Value::string(&text.our_number)),
        ("theirNumber".to_string(), Value::string(&text.their_number)),
        ("mediaList".to_string(), Value::List(media)),
    ]);
    // a group text went to several numbers: every one of them, our number among them
    if text.recipients.len() > 1 {
        parameters.insert(
            "recipientList".to_string(),
            Value::List(text.recipients.iter().map(Value::string).collect()),
        );
    }
    if let Some(words) = &text.text {
        parameters.insert("text".to_string(), Value::string(words));
    }
    if let Some(thread) = text.thread_id.as_deref().filter(|thread| !thread.is_empty()) {
        parameters.insert("externalThreadId".to_string(), Value::string(thread));
    }
    if let Some(at) = text.origin_millis {
        parameters.insert("originMillis".to_string(), Value::I64(at));
    }
    // RCS: a place and a tapped suggestion, kept on the message (Sms.js makes a place with no words its text)
    let mut metadata = serde_json::Map::new();
    if let Some(place) = &text.location {
        metadata.insert(
            "location".to_string(),
            // the coordinates as text: a JSON number is a decimal in the service scripts
            json!({"latitude": place.latitude.to_string(), "longitude": place.longitude.to_string(), "label": place.label,
                "address": place.address}),
        );
    }
    if let Some(postback) = &text.postback {
        metadata.insert("postback".to_string(), Json::String(postback.clone()));
    }
    if !metadata.is_empty() {
        parameters.insert(
            "metadata".to_string(),
            moqui_lib::json::from_json(&Json::Object(metadata)),
        );
    }
    match crate::call("org.moqui.call.SmsServices.receive#Sms", parameters) {
        Ok(out) => {
            let field = |key: &str| out.get(key).map(to_plain_string).unwrap_or_default();
            let outcome = field("outcome");
            if outcome == "failed" {
                let reason = field("reasonCode");
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
                    "SMS bridge {}: a text to {} was left: {}",
                    bridge.chat_bridge_id,
                    text.our_number,
                    field("reasonCode")
                );
            }
            WebhookResponse::json(
                200,
                &json!({"outcome": outcome, "reasonCode": field("reasonCode"), "message": field("message")}),
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

/// A delivery receipt, to `ChatBridgeServices.note#DeliveryReceipt`: the message's state at its recipient. A group
/// text's names the group's message (the delivery's remote id) and its one recipient. With the answer, the content made
/// for the send ([`keep_content`]) when the receipt is final: its message is done with it.
fn noted(bridge: &BridgeInfo, receipt: &Receipt) -> (WebhookResponse, Option<String>) {
    let status = receipt.status.as_str();
    let mut parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("remoteEventId".to_string(), Value::string(&receipt.remote_event_id)),
        ("status".to_string(), Value::string(status)),
    ]);
    if let Some(error) = &receipt.error {
        parameters.insert("error".to_string(), Value::string(error));
    }
    if let Some(number) = &receipt.recipient_number {
        parameters.insert("recipientNumber".to_string(), Value::string(number));
    }
    if let Some(own) = &receipt.other_remote_event_id {
        parameters.insert("otherRemoteEventId".to_string(), Value::string(own));
    }
    if let Some(stamp) = receipt
        .receipt_millis
        .and_then(|at| jiff::Timestamp::from_millisecond(at).ok())
    {
        parameters.insert("receiptDate".to_string(), Value::Timestamp(stamp));
    }
    match crate::call("org.moqui.chat.ChatBridgeServices.note#DeliveryReceipt", parameters) {
        Ok(out) => {
            if let Some(path) = receipt.path {
                note_path(bridge, &[("remoteEventId", &receipt.remote_event_id)], path);
            }
            if let Some(signal) = &receipt.opt_signal {
                let answer = opted(bridge, signal);
                if !(200..300).contains(&answer.status) {
                    return (answer, None);
                }
            }
            let content = out
                .get("providerContentId")
                .map(to_plain_string)
                .filter(|content| !content.is_empty());
            (
                WebhookResponse::json(
                    200,
                    &json!({"found": out.get("found") == Some(&Value::Bool(true)), "status": status}),
                ),
                content,
            )
        }
        Err(error) => {
            tracing::error!(
                "SMS bridge {}: the receipt of {} was not kept: {error}",
                bridge.chat_bridge_id,
                receipt.remote_event_id
            );
            (
                WebhookResponse::text(503, "the receipt was not kept; try again later"),
                None,
            )
        }
    }
}

/// Keep `content`, a content the provider made for the send of delivery `delivery_id` (a Twilio Content resource of a
/// rich message), on the delivery (`ChatBridgeServices.note#DeliveryContent`, in a transaction of its own): its final
/// receipt deletes it, else the daily sweep (`clean#ProviderContent`). Called before the send, so a send that never got
/// through leaves it for the sweep.
///
/// # Errors
///
/// Returns why it was not kept.
pub fn keep_content(bridge: &BridgeInfo, delivery_id: &str, content: &str) -> Result<(), String> {
    if delivery_id.is_empty() {
        return Err("a direct send has no delivery to keep its content on".to_string());
    }
    let parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("chatBridgeDeliveryId".to_string(), Value::string(delivery_id)),
        ("providerContentId".to_string(), Value::string(content)),
    ]);
    crate::call("org.moqui.chat.ChatBridgeServices.note#DeliveryContent", parameters).map(|_| ())
}

/// The content an earlier attempt kept on delivery `delivery_id` ([`keep_content`]), for a retry to send again or
/// delete; none when there is none or it cannot be read.
pub fn kept_content(delivery_id: &str) -> Option<String> {
    if delivery_id.is_empty() {
        return None;
    }
    let facade = moqui_core::context::ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()?;
    facade
        .find("moqui.chat.ChatBridgeDelivery")
        .and_then(|find| {
            find.eq("chatBridgeDeliveryId", Value::string(delivery_id))
                .use_cache(false)
                .disable_authz()
                .one()
        })
        .ok()
        .flatten()
        .and_then(|row| row.get("providerContentId").map(to_plain_string))
        .filter(|content| !content.is_empty())
}

/// Clear `content` from delivery `delivery_id` once it is deleted.
///
/// # Errors
///
/// Returns why it was not cleared.
pub fn clear_content_of(bridge: &BridgeInfo, delivery_id: &str, content: &str) -> Result<(), String> {
    let parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("chatBridgeDeliveryId".to_string(), Value::string(delivery_id)),
        ("deletedContentId".to_string(), Value::string(content)),
    ]);
    crate::call("org.moqui.chat.ChatBridgeServices.note#DeliveryContent", parameters).map(|_| ())
}

/// Renew delivery `delivery_id`'s lease between the steps of a send of several calls (a Twilio Content, then the
/// message), so a slow send is never taken for one cut off; one not renewed is logged.
pub fn renew_lease(bridge: &BridgeInfo, delivery_id: &str) {
    if delivery_id.is_empty() {
        return;
    }
    let parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("chatBridgeDeliveryId".to_string(), Value::string(delivery_id)),
        ("renewLease".to_string(), Value::Bool(true)),
    ]);
    if let Err(error) = crate::call("org.moqui.chat.ChatBridgeServices.note#DeliveryContent", parameters) {
        tracing::warn!("delivery {delivery_id}: its lease was not renewed: {error}");
    }
}

/// Note on delivery `delivery_id` what the adapter changed of its send (`richRefused`: the provider refused its cards
/// and their text went), shown in the message's Info; one not kept is logged.
pub fn note_delivery(bridge: &BridgeInfo, delivery_id: &str, code: &str, text: &str) {
    if delivery_id.is_empty() {
        return;
    }
    let parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("chatBridgeDeliveryId".to_string(), Value::string(delivery_id)),
        ("noteCode".to_string(), Value::string(code)),
        ("noteText".to_string(), Value::string(text)),
    ]);
    if let Err(error) = crate::call("org.moqui.chat.ChatBridgeServices.note#DeliveryContent", parameters) {
        tracing::warn!("delivery {delivery_id}: its note {code} was not kept: {error}");
    }
}

/// Whether a message of ours ever reached the network for the mapping `external_room_id` of bridge `bridge_id` (an
/// accepted delivery): a typing indicator goes only to a person we wrote to.
fn messaged(bridge_id: &str, external_room_id: &str) -> Result<bool, String> {
    let Some(facade) = moqui_core::context::ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Err("no entity facade to check the room".to_string());
    };
    let read = || -> Result<bool, moqui_lib::error::Error> {
        let mappings = facade
            .find("moqui.chat.ChatRoomBridge")?
            .eq("chatBridgeId", Value::string(bridge_id))
            .eq("externalRoomId", Value::string(external_room_id))
            .disable_authz()
            .list()?;
        for mapping in mappings.values() {
            let Some(id) = mapping.get("chatRoomBridgeId").map(to_plain_string) else {
                continue;
            };
            let accepted = facade
                .find("moqui.chat.ChatBridgeDelivery")?
                .eq("chatRoomBridgeId", Value::string(&id))
                .eq("statusEnumId", Value::string("CdsAccepted"))
                .disable_authz()
                .count()?;
            if accepted > 0 {
                return Ok(true);
            }
        }
        Ok(false)
    };
    read().map_err(|error| format!("the room's messages were not read: {error}"))
}

/// Clear `content` from the delivery of `remote_event_id` once it is deleted.
fn clear_content(bridge: &BridgeInfo, remote_event_id: &str, content: &str) -> Result<(), String> {
    let parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("remoteEventId".to_string(), Value::string(remote_event_id)),
        ("deletedContentId".to_string(), Value::string(content)),
    ]);
    crate::call("org.moqui.chat.ChatBridgeServices.note#DeliveryContent", parameters).map(|_| ())
}

/// An RCS bridge's `typingIndicators`: true unless it is false (or the text `false`).
pub fn typing_on(config: &Json) -> bool {
    !config
        .get("typingIndicators")
        .is_some_and(|value| value.as_bool() == Some(false) || value.as_str() == Some("false"))
}

/// Whether the person at `theirs` opted out of texts from `ours` (`SmsOptOut`).
fn opted_out(ours: &str, theirs: &str) -> Result<bool, String> {
    let Some(facade) = moqui_core::context::ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Err("no entity facade to check the opt-out".to_string());
    };
    let row = facade
        .find("moqui.call.SmsOptOut")
        .and_then(|find| {
            find.eq("phoneNumber", Value::string(ours))
                .eq("otherNumber", Value::string(theirs))
                .disable_authz()
                .one()
        })
        .map_err(|error| format!("the opt-out was not read: {error}"))?;
    Ok(row.is_some_and(|row| row.get("optedOut").map(to_plain_string).as_deref() == Some("Y")))
}

/// The way a sent message went, to `ChatBridgeServices.note#DeliveryPath` by `key` (its delivery's id, or its remote
/// id): whether a delivery was found. One not kept is logged and changes nothing else: the path is what the room shows,
/// never what the send or its receipt depends on.
fn note_path(bridge: &BridgeInfo, key: &[(&str, &str)], path: Path) -> bool {
    if key.is_empty() || key.iter().any(|(_, value)| value.is_empty()) {
        return false;
    }
    let mut parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("path".to_string(), Value::string(path.as_str())),
    ]);
    for (name, value) in key {
        parameters.insert((*name).to_string(), Value::string(*value));
    }
    match crate::call("org.moqui.chat.ChatBridgeServices.note#DeliveryPath", parameters) {
        Ok(out) => out.get("found") == Some(&Value::Bool(true)),
        Err(error) => {
            tracing::warn!(
                "RCS bridge {}: the path {} of a message was not kept: {error}",
                bridge.chat_bridge_id,
                path.as_str()
            );
            false
        }
    }
}

/// An opt-out or opt-in the provider tells of, to `SmsServices.note#SmsOptSignal`: for a number of ours on this bridge
/// only, and once for its event.
fn opted(bridge: &BridgeInfo, signal: &OptSignal) -> WebhookResponse {
    let keyword = match signal.keyword {
        Keyword::Stop => "STOP",
        Keyword::Start => "START",
    };
    let parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        ("eventId".to_string(), Value::string(&signal.event_id)),
        ("ourNumber".to_string(), Value::string(&signal.our_number)),
        ("theirNumber".to_string(), Value::string(&signal.their_number)),
        ("keyword".to_string(), Value::string(keyword)),
    ]);
    match crate::call("org.moqui.call.SmsServices.note#SmsOptSignal", parameters) {
        Ok(out) => {
            let field = |key: &str| out.get(key).map(to_plain_string).unwrap_or_default();
            WebhookResponse::json(
                200,
                &json!({"outcome": field("outcome"), "reasonCode": field("reasonCode"), "keyword": keyword}),
            )
        }
        Err(error) => {
            tracing::error!(
                "SMS bridge {}: a {keyword} of {} was not kept: {error}",
                bridge.chat_bridge_id,
                signal.their_number
            );
            WebhookResponse::text(503, "the opt-out was not kept; try again later")
        }
    }
}
