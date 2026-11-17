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

//! Group texts on Twilio's Conversations API (docs/interfaces/SMS.md "Twilio"): Twilio's Messages API has no group
//! MMS, so a group room is one Twilio Conversation, between +1 (US and Canadian) long codes, at most 10 participants
//! (our number and [`super::GROUP_MAX`] others).
//!
//! - **Out.** The room's Conversation is its mapping's thread (`ChatRoomBridge.externalThreadId`, the `CH...` SID).
//!   A room with none gets one on its first send: looked up by its unique name first ([`unique_name`], of the bridge and
//!   the room's numbers, so a send after a lost answer finds the one made before and never makes a second), else made
//!   with it; then our number joins as the chat participant (`Identity` and `MessagingBinding.ProjectedAddress` our
//!   number), each other number as an SMS participant (`MessagingBinding.Address`; when Twilio answers that another
//!   Conversation has these numbers, 50438, as one it made for a group text that came in, that one is found by its
//!   participants and taken instead), and a webhook of the Conversation
//!   names the bridge's webhook URL for `onMessageAdded` and `onDeliveryUpdated`; then the mapping keeps the SID
//!   (`store#MappingThread`). Nothing has reached anyone's phone until the message is posted, so a failure or a lost
//!   answer in these steps is tried again. The message is `POST .../Conversations/{CH}/Messages` (`Author` our number,
//!   `Body`): its `sid` (`IM...`) is the delivery's remote id, and an answer lost there is uncertain, as for any text.
//!   Files are not sent in a group text: Twilio takes a Conversation's files only as uploads to its Media Content
//!   Service, which this adapter does not do yet (`groupMediaNotSupported`).
//! - **In.** The Conversation's webhooks are form posts signed as the Messages API's are: `onMessageAdded` is a text
//!   (`Author` the sender's number, `Body`, `Media` the files in the Media Content Service), with every number of the
//!   Conversation as its recipients; `onDeliveryUpdated` is one recipient's receipt (`ParticipantSid`, `Status`,
//!   `ErrorCode`) of the message (`MessageSid`). A Conversation's participants (their ids and numbers) are read from
//!   Twilio once a process and kept ([`Participants`]).

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use moqui_core::service::chat_bridge::{BridgeInfo, DeliveryOutcome, WebhookResponse};
use moqui_lib::http::{Method, RestClient, RestResponse};
use moqui_lib::object::Value;
use serde_json::Value as Json;

use super::{auth_headers, is_sid, Config, Form, Twilio};
use crate::sms::{
    call, exchange, never_sent, ApiBody, ApiRequest, Inbound, InboundMedia, Outgoing, Receipt, ReceiptStatus, Settings,
    SmsEvent, SmsProvider as _,
};

/// The participants of each Conversation a process knows: by `CH...` SID, each participant's `MB...` SID and number
/// (our number's is its projected address).
#[derive(Default)]
pub struct Participants {
    by_conversation: Mutex<HashMap<String, Roster>>,
}

/// A Conversation's participants.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roster {
    /// Our number: the chat participant's projected address.
    pub ours: Option<String>,
    /// Each participant's SID and number.
    pub numbers: BTreeMap<String, String>,
}

impl Participants {
    pub fn get(&self, conversation: &str) -> Option<Roster> {
        moqui_lib::sync::lock(&self.by_conversation).get(conversation).cloned()
    }

    pub fn put(&self, conversation: &str, roster: Roster) {
        let mut known = moqui_lib::sync::lock(&self.by_conversation);
        // a process keeps at most this many; a Conversation it forgot is read again
        if known.len() >= 10_000 {
            known.clear();
        }
        known.insert(conversation.to_string(), roster);
    }
}

/// The unique name of a group room's Conversation: a hash of the bridge and the room's numbers, so the same room finds
/// the same Conversation.
pub fn unique_name(bridge_id: &str, external_room_id: &str) -> String {
    let digest = aws_lc_rs::digest::digest(
        &aws_lc_rs::digest::SHA256,
        format!("{bridge_id}|{external_room_id}").as_bytes(),
    );
    format!("moqui-sms-{}", &moqui_lib::codec::hex_encode(digest.as_ref())[..40])
}

/// The Conversations API's base for the bridge's service: `.../v1/Services/{IS}` or `.../v1`.
pub fn conversations_base(config: &Config) -> String {
    match &config.conversation_service_sid {
        Some(service) => format!("{}/v1/Services/{service}", config.conversations_base_url),
        None => format!("{}/v1", config.conversations_base_url),
    }
}

fn failed(reason: &str, error: impl Into<String>) -> DeliveryOutcome {
    DeliveryOutcome::Failed {
        reason_code: reason.to_string(),
        error: error.into(),
    }
}

fn retry(error: impl Into<String>, not_connected: bool) -> DeliveryOutcome {
    DeliveryOutcome::Retry {
        error: error.into(),
        not_connected,
    }
}

/// Whether `url` may carry the API's credentials: https, or (a test's fake) http when internal addresses are allowed.
fn secure(url: &str, settings: &Settings) -> bool {
    url.starts_with("https://") || (settings.allow_internal && url.starts_with("http://"))
}

/// A form request to the Conversations API.
fn form_request(config: &Config, method: Method, url: String, form: Vec<(String, String)>) -> ApiRequest {
    ApiRequest {
        method,
        url,
        headers: auth_headers(config),
        body: if form.is_empty() {
            ApiBody::None
        } else {
            ApiBody::Form(form)
        },
    }
}

/// One call of the steps before the message: its answer, or what the send is when it cannot go on. Nothing has
/// reached a phone yet, so a lost answer, a 5xx and a rate are tried again; the key refused is not connected.
fn setup_call(client: &RestClient, request: &ApiRequest, what: &str) -> Result<RestResponse, DeliveryOutcome> {
    let response = match call(client, request) {
        Ok(response) => response,
        Err(error) => return Err(retry(format!("Twilio {what}: {}", error.message()), never_sent(&error))),
    };
    match response.status() {
        401 | 403 => Err(retry(
            format!("Twilio refused the API key ({})", response.status()),
            true,
        )),
        429 | 500..=599 => Err(retry(
            format!("Twilio {what}: it answered {}", response.status()),
            false,
        )),
        _ => Ok(response),
    }
}

fn json_of(response: &RestResponse) -> Json {
    serde_json::from_str(response.body()).unwrap_or(Json::Null)
}

/// Send a group text: the room's Conversation (made on its first send), then the message.
pub fn send(
    provider: &Twilio,
    client: &RestClient,
    config: &Config,
    settings: &Settings,
    out: &Outgoing<'_>,
) -> DeliveryOutcome {
    let plus_one = |number: &str| number.starts_with("+1") && number.len() == 12;
    if !plus_one(out.ours) || !out.theirs.iter().all(|number| plus_one(number)) {
        return failed(
            "groupTextsNotSupported",
            "Twilio carries group texts between US and Canadian (+1) numbers only",
        );
    }
    if !out.media_urls.is_empty() {
        return failed(
            "groupMediaNotSupported",
            "A group text through Twilio carries words only: send the files to each number on its own",
        );
    }
    if !secure(&config.conversations_base_url, settings) {
        return failed(
            "providerRefused",
            "The Twilio bridge's conversationsBaseUrl is not an https address",
        );
    }
    let conversation = match thread_of(out).filter(|sid| is_sid(sid, "CH")) {
        Some(sid) => sid.to_string(),
        None => match conversation_of(provider, client, config, out) {
            Ok(sid) => sid,
            Err(outcome) => return outcome,
        },
    };
    let mut form = vec![("Author".to_string(), out.ours.to_string())];
    if let Some(text) = out.text {
        form.push(("Body".to_string(), text.to_string()));
    }
    let request = form_request(
        config,
        Method::Post,
        format!("{}/Conversations/{conversation}/Messages", conversations_base(config)),
        form,
    );
    exchange(provider, client, out, &request)
}

/// The room's Conversation, found by its unique name or made, with its participants and its webhook, and kept on the
/// room's mapping.
fn conversation_of(
    provider: &Twilio,
    client: &RestClient,
    config: &Config,
    out: &Outgoing<'_>,
) -> Result<String, DeliveryOutcome> {
    let base = conversations_base(config);
    let bridge = out.bridge;
    let unique = unique_name(&bridge.chat_bridge_id, &out.request.external_room_id);
    let find = || -> Result<Option<String>, DeliveryOutcome> {
        let found = setup_call(
            client,
            &form_request(
                config,
                Method::Get,
                format!("{base}/Conversations/{unique}"),
                Vec::new(),
            ),
            "did not say whether the group's Conversation is there",
        )?;
        Ok(match found.status() {
            200 => json_of(&found)
                .get("sid")
                .and_then(Json::as_str)
                .filter(|sid| is_sid(sid, "CH"))
                .map(str::to_string),
            _ => None,
        })
    };
    let sid = match find()? {
        Some(sid) => sid,
        None => {
            let names = out.theirs.join(", ");
            let made = setup_call(
                client,
                &form_request(
                    config,
                    Method::Post,
                    format!("{base}/Conversations"),
                    vec![
                        ("UniqueName".to_string(), unique.clone()),
                        ("FriendlyName".to_string(), names.chars().take(200).collect()),
                    ],
                ),
                "did not make the group's Conversation",
            )?;
            let sid = json_of(&made)
                .get("sid")
                .and_then(Json::as_str)
                .filter(|sid| is_sid(sid, "CH"))
                .map(str::to_string);
            match (made.status(), sid) {
                (200..=299, Some(sid)) => sid,
                // made at once by another send: found by its name
                (409, _) => find()?.ok_or_else(|| retry("Twilio's group Conversation is not there yet", false))?,
                (status, _) => {
                    let error = provider_error(&made);
                    return Err(failed(
                        "providerRefused",
                        format!("Twilio did not make the group's Conversation ({status}): {error}"),
                    ));
                }
            }
        }
    };
    let mut roster = Roster {
        ours: Some(out.ours.to_string()),
        numbers: BTreeMap::new(),
    };
    // our number as the chat participant, its projected address; each other number by its own address. One there
    // already (a send before this one got this far) is 409
    let mut joins = vec![(
        out.ours.to_string(),
        vec![
            ("Identity".to_string(), out.ours.to_string()),
            ("MessagingBinding.ProjectedAddress".to_string(), out.ours.to_string()),
        ],
    )];
    for number in out.theirs {
        joins.push((
            number.clone(),
            vec![("MessagingBinding.Address".to_string(), number.clone())],
        ));
    }
    for (number, form) in joins {
        let joined = setup_call(
            client,
            &form_request(
                config,
                Method::Post,
                format!("{base}/Conversations/{sid}/Participants"),
                form,
            ),
            "did not add a number to the group's Conversation",
        )?;
        match joined.status() {
            200..=299 => {
                if let Some(participant) = json_of(&joined).get("sid").and_then(Json::as_str) {
                    roster.numbers.insert(participant.to_string(), number);
                }
            }
            // there already: a send before this one got this far (409; Twilio's 50416, 50433, 50437)
            409 => {}
            status => {
                let error = provider.error_of(joined.body());
                match error.code.as_deref() {
                    Some("50416" | "50433" | "50437") => {}
                    // another Conversation has these numbers (Twilio made it for a text that came in): that one is the
                    // room's, and the one made here goes
                    Some("50438") => {
                        let Some(other) = existing_group(provider, client, config, out) else {
                            return Err(failed(
                                "providerRefused",
                                "Twilio has another Conversation of this group, and it was not found",
                            ));
                        };
                        let _ = call(
                            client,
                            &form_request(config, Method::Delete, format!("{base}/Conversations/{sid}"), Vec::new()),
                        );
                        keep_thread(bridge, out, &other)?;
                        return Ok(other);
                    }
                    _ => {
                        return Err(failed(
                            "providerRefused",
                            format!(
                                "Twilio did not add {number} to the group's Conversation ({status}): {}",
                                error.text
                            ),
                        ))
                    }
                }
            }
        }
    }
    // its receipts and replies to the bridge's webhook, whatever the service's own webhooks are
    if let Some(callback) = crate::sms::callback_url(bridge) {
        let hooked = setup_call(
            client,
            &form_request(
                config,
                Method::Post,
                format!("{base}/Conversations/{sid}/Webhooks"),
                vec![
                    ("Target".to_string(), "webhook".to_string()),
                    ("Configuration.Url".to_string(), callback),
                    ("Configuration.Method".to_string(), "POST".to_string()),
                    ("Configuration.Filters".to_string(), "onMessageAdded".to_string()),
                    ("Configuration.Filters".to_string(), "onDeliveryUpdated".to_string()),
                ],
            ),
            "did not add the group Conversation's webhook",
        );
        if !matches!(&hooked, Ok(response) if (200..300).contains(&response.status())) {
            tracing::warn!(
                "SMS bridge {}: the webhook of Twilio Conversation {sid} was not added; its replies and receipts come \
                 only through the Conversations service's own webhook",
                bridge.chat_bridge_id
            );
        }
    }
    // a roster missing a participant that was there already is read again when a receipt needs it
    if roster.numbers.len() == out.theirs.len() + 1 {
        provider.participants.put(&sid, roster);
    }
    keep_thread(bridge, out, &sid)?;
    Ok(sid)
}

/// The Conversation of exactly the group's numbers that Twilio has already (`ParticipantConversations` of the first
/// other number, each one's participants compared), when there is one.
fn existing_group(provider: &Twilio, client: &RestClient, config: &Config, out: &Outgoing<'_>) -> Option<String> {
    let first = out.theirs.first()?;
    let request = form_request(
        config,
        Method::Get,
        format!(
            "{}/ParticipantConversations?Address={}",
            conversations_base(config),
            first.replace('+', "%2B")
        ),
        Vec::new(),
    );
    let response = call(client, &request)
        .ok()
        .filter(|response| response.status() == 200)?;
    let mut wanted: Vec<&str> = out.theirs.iter().map(String::as_str).collect();
    wanted.push(out.ours);
    wanted.sort_unstable();
    let listed = json_of(&response);
    let candidates = listed.get("conversations").and_then(Json::as_array)?;
    candidates
        .iter()
        .filter(|entry| entry.get("conversation_state").and_then(Json::as_str) != Some("closed"))
        .filter_map(|entry| entry.get("conversation_sid").and_then(Json::as_str))
        .filter(|sid| is_sid(sid, "CH"))
        .find(|sid| {
            roster_of(provider, client, config, sid).is_ok_and(|roster| {
                let mut numbers: Vec<&str> = roster.numbers.values().map(String::as_str).collect();
                numbers.sort_unstable();
                numbers.dedup();
                numbers == wanted
            })
        })
        .map(str::to_string)
}

fn provider_error(response: &RestResponse) -> String {
    Twilio::default().error_of(response.body()).text
}

/// Keep the Conversation's SID as the room's mapping's thread (`store#MappingThread`, in a transaction of its own).
fn keep_thread(bridge: &BridgeInfo, out: &Outgoing<'_>, sid: &str) -> Result<(), DeliveryOutcome> {
    let parameters = BTreeMap::from([
        ("chatBridgeId".to_string(), Value::string(&bridge.chat_bridge_id)),
        (
            "externalRoomId".to_string(),
            Value::string(&out.request.external_room_id),
        ),
        ("externalThreadId".to_string(), Value::string(sid)),
    ]);
    crate::call("org.moqui.chat.ChatBridgeServices.store#MappingThread", parameters)
        .map(|_| ())
        .map_err(|error| {
            // not kept: the next try finds the Conversation by its name
            retry(
                format!("The group's Twilio Conversation {sid} was not kept on its room: {error}"),
                false,
            )
        })
}

/// A Conversation's participants, read from Twilio (`GET .../Participants`), kept for the process.
pub fn roster_of(
    provider: &Twilio,
    client: &RestClient,
    config: &Config,
    conversation: &str,
) -> Result<Roster, String> {
    if let Some(known) = provider.participants.get(conversation) {
        return Ok(known);
    }
    if !is_sid(conversation, "CH") {
        return Err(format!("{conversation} is not a Conversation's SID"));
    }
    let request = form_request(
        config,
        Method::Get,
        format!(
            "{}/Conversations/{conversation}/Participants?PageSize=50",
            conversations_base(config)
        ),
        Vec::new(),
    );
    let response = call(client, &request).map_err(|error| error.message().to_string())?;
    if response.status() != 200 {
        return Err(format!(
            "Twilio answered {} for Conversation {conversation}'s participants",
            response.status()
        ));
    }
    let mut roster = Roster::default();
    for participant in json_of(&response)
        .get("participants")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        let Some(sid) = participant.get("sid").and_then(Json::as_str) else {
            continue;
        };
        let binding = participant.get("messaging_binding");
        let text = |key: &str| {
            binding
                .and_then(|binding| binding.get(key))
                .and_then(Json::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        if let Some(projected) = text("projected_address") {
            roster.ours = Some(projected.clone());
            roster.numbers.insert(sid.to_string(), projected);
        } else if let Some(address) = text("address") {
            roster.numbers.insert(sid.to_string(), address);
        }
    }
    provider.participants.put(conversation, roster.clone());
    Ok(roster)
}

/// The number of a participant of a Conversation, read again from Twilio when the kept roster does not have it.
pub fn number_of(
    provider: &Twilio,
    client: &RestClient,
    config: &Config,
    conversation: &str,
    participant: &str,
) -> Result<Option<String>, String> {
    // a participant the kept roster does not have: it is read again (one added since, or a roster kept short)
    if let Some(known) = provider.participants.get(conversation) {
        if let Some(number) = known.numbers.get(participant) {
            return Ok(Some(number.clone()));
        }
        let _ = moqui_lib::sync::lock(&provider.participants.by_conversation).remove(conversation);
    }
    Ok(roster_of(provider, client, config, conversation)?
        .numbers
        .get(participant)
        .cloned())
}

/// The room's Conversation, its mapping's thread.
fn thread_of(out: &Outgoing<'_>) -> Option<String> {
    let _ = out;
    None
}

/// A Conversations webhook (its `EventType`): a text of the group or one recipient's receipt, with the Conversation's
/// numbers (read from Twilio when the process does not have them: 503 when it cannot, so Twilio's retry finds them).
pub fn event(
    provider: &Twilio,
    client: &RestClient,
    bridge: &BridgeInfo,
    config: &Config,
    form: &Form,
) -> Result<SmsEvent, WebhookResponse> {
    let kind = form.get("EventType").unwrap_or_default();
    if !matches!(kind, "onMessageAdded" | "onDeliveryUpdated") {
        return Ok(SmsEvent::Ignored(kind.to_string()));
    }
    let (Some(conversation), Some(message)) = (form.get("ConversationSid"), form.get("MessageSid")) else {
        return Err(WebhookResponse::text(
            400,
            "a Conversation's event names it and its message",
        ));
    };
    let unread = |error: String| {
        tracing::warn!(
            "SMS bridge {}: Twilio Conversation {conversation}'s participants were not read: {error}",
            bridge.chat_bridge_id
        );
        WebhookResponse::text(503, "the Conversation's participants were not read; try again later")
    };
    if kind == "onDeliveryUpdated" {
        let Some(participant) = form.get("ParticipantSid") else {
            return Ok(SmsEvent::Ignored("a receipt of no participant".to_string()));
        };
        let Some(number) = number_of(provider, client, config, conversation, participant).map_err(unread)? else {
            return Ok(SmsEvent::Ignored(
                "a receipt of a participant not in the Conversation".to_string(),
            ));
        };
        let status = match form.get("Status").unwrap_or_default() {
            "sent" => ReceiptStatus::Sent,
            "delivered" | "read" => ReceiptStatus::Delivered,
            "undelivered" | "failed" => ReceiptStatus::Failed,
            other => return Ok(SmsEvent::Ignored(other.to_string())),
        };
        let error = form.get("ErrorCode").filter(|code| *code != "0").map(super::error_text);
        return Ok(SmsEvent::Receipt(Receipt {
            remote_event_id: message.to_string(),
            status,
            error: error
                .or_else(|| (status == ReceiptStatus::Failed).then(|| "Twilio could not deliver it".to_string())),
            recipient_number: Some(number),
            other_remote_event_id: form.get("ChannelMessageSid").map(str::to_string),
            receipt_millis: None,
        }));
    }
    let roster = roster_of(provider, client, config, conversation).map_err(unread)?;
    let Some(ours) = roster.ours.clone() else {
        return Ok(SmsEvent::Ignored("a Conversation with no number of ours".to_string()));
    };
    let Some(author) = form.get("Author") else {
        return Err(WebhookResponse::text(400, "a Conversation's message names its author"));
    };
    // ours (the chat participant's identity is our number), or a chat identity of another system on the account
    if author == ours || !author.starts_with('+') {
        return Ok(SmsEvent::Ignored("a message of ours".to_string()));
    }
    let mut recipients: Vec<String> = Vec::new();
    for number in roster.numbers.values() {
        if !recipients.contains(number) {
            recipients.push(number.clone());
        }
    }
    // two numbers (ours and theirs) are a pair's text
    if recipients.len() <= 2 {
        recipients.clear();
    }
    Ok(SmsEvent::Inbound(Inbound {
        event_id: message.to_string(),
        message_id: message.to_string(),
        our_number: ours,
        their_number: author.to_string(),
        text: form
            .pairs
            .iter()
            .find(|(key, _)| key == "Body")
            .map(|(_, body)| body.clone()),
        media: media_of(config, form),
        recipients,
        origin_millis: form
            .get("DateCreated")
            .and_then(|date| date.parse::<jiff::Timestamp>().ok())
            .map(|stamp| stamp.as_millisecond()),
    }))
}

/// The files of a Conversation's message (`Media`, a JSON list of the Media Content Service's files), each by its
/// content address there.
fn media_of(config: &Config, form: &Form) -> Vec<InboundMedia> {
    let Some(service) = form.get("ChatServiceSid").filter(|sid| is_sid(sid, "IS")) else {
        return Vec::new();
    };
    let list: Json = form
        .get("Media")
        .and_then(|media| serde_json::from_str(media).ok())
        .unwrap_or(Json::Null);
    let field = |file: &Json, names: &[&str]| {
        names
            .iter()
            .find_map(|name| file.get(*name).and_then(Json::as_str))
            .map(str::to_string)
    };
    list.as_array()
        .into_iter()
        .flatten()
        .take(10)
        .filter_map(|file| {
            let sid = field(file, &["Sid", "sid"]).filter(|sid| is_sid(sid, "ME"))?;
            Some(InboundMedia {
                url: format!(
                    "{}/v1/Services/{service}/Media/{sid}/Content",
                    config.media_content_base_url
                ),
                content_type: field(file, &["ContentType", "content_type"]),
            })
        })
        .collect()
}
