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
//! - **Out.** The room's Conversation is its mapping's thread (`ChatRoomBridge.externalThreadId`, the `CH...` SID,
//!   which the delivery carries). A room with none gets one on its first send: looked up by its unique name first
//!   ([`unique_name`], of the bridge and the room's numbers, so a send after a lost answer finds the one made before and
//!   never makes a second; one found closed is deleted and made again), else made in one request with all its numbers
//!   (`ConversationWithParticipants`: our number by its projected address, each other number by its address), and
//!   kept on the mapping at once ([`crate::sms::store_thread`]). When Twilio answers that another Conversation has
//!   these numbers (50438: one it made for a group text that came in), that one is found by its participants
//!   (`ParticipantConversations`, paged) and kept instead. Before a send uses a Conversation (once a process), it is
//!   checked: one that is gone or closed is let go from the mapping and a new one made on the next try; one still
//!   `initializing` (its participants are added after the answer) waits a few seconds; and the bridge's webhook is on
//!   it (`onMessageAdded`, `onDeliveryUpdated`; added when missing, at most 5 a Conversation). Nothing has reached
//!   anyone's phone until the message is posted, so a failure or a lost answer in these steps is tried again. The
//!   message is `POST .../Conversations/{CH}/Messages` (`Author` our number, `Body`): its `sid` (`IM...`) is the
//!   delivery's remote id, and an answer lost there is uncertain, as for any text. Files are not sent in a group text:
//!   Twilio takes a Conversation's files only as uploads to its Media Content Service, which this adapter does not do
//!   yet (`groupMediaNotSupported`).
//! - **In.** The Conversation's webhooks are form posts signed as the Messages API's are: `onMessageAdded` is a text
//!   (`Author` the sender's number, `Body`, `Media` the files in the Media Content Service) of the Conversation
//!   (`Inbound.thread_id`: the room whose mapping keeps it), with every number of the Conversation as its recipients
//!   (a Conversation of one outside number, as Twilio's autocreation makes with `proxy_address` our number, is the
//!   pair's); `onDeliveryUpdated` is one recipient's receipt (`ParticipantSid`, `Status`, `ErrorCode`) of the message
//!   (`MessageSid`). A Conversation's participants (their ids and numbers) are read from Twilio once a process and kept
//!   by bridge ([`Participants`]), read again for a receipt of one it does not have.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use moqui_core::service::chat_bridge::{BridgeInfo, DeliveryOutcome, WebhookResponse};
use moqui_lib::http::{Method, RestClient, RestResponse};
use serde_json::{json, Value as Json};

use super::{auth_headers, is_sid, Config, Form, Twilio};
use crate::sms::{
    call, exchange, never_sent, ApiBody, ApiRequest, Inbound, InboundMedia, Outgoing, Receipt, ReceiptStatus, Settings,
    SmsEvent, SmsProvider as _,
};

/// The most webhooks Twilio takes on one Conversation.
const HOOKS_MAX: usize = 5;
/// The pages of `ParticipantConversations` read to find a group Twilio has already.
const PAGES_READ: usize = 10;
/// How long a Conversation still `initializing` is waited for.
const INITIALIZING_WAIT_MILLIS: i64 = 5_000;
/// The bound of a participants read in a webhook's work.
const ROSTER_TIMEOUT: Duration = Duration::from_secs(5);

/// What a process knows of Conversations, by bridge and `CH...` SID (shared by every bridge of the adapter): each one's
/// participants (`MB...` SID and number; our number's is its projected or proxy address), and those a send found
/// ready (active, the bridge's webhook on it).
#[derive(Default)]
pub struct Participants {
    by_conversation: Mutex<HashMap<String, Roster>>,
    ready: Mutex<HashSet<String>>,
}

/// A Conversation's participants.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roster {
    /// Our number: the chat participant's projected address, or the proxy address of a Conversation of one number.
    pub ours: Option<String>,
    /// Each participant's SID and number.
    pub numbers: BTreeMap<String, String>,
}

fn key(bridge_id: &str, conversation: &str) -> String {
    format!("{bridge_id}:{conversation}")
}

impl Participants {
    pub fn get(&self, bridge_id: &str, conversation: &str) -> Option<Roster> {
        moqui_lib::sync::lock(&self.by_conversation)
            .get(&key(bridge_id, conversation))
            .cloned()
    }

    fn forget(&self, bridge_id: &str, conversation: &str) {
        let _ = moqui_lib::sync::lock(&self.by_conversation).remove(&key(bridge_id, conversation));
        let _ = moqui_lib::sync::lock(&self.ready).remove(&key(bridge_id, conversation));
    }

    pub fn put(&self, bridge_id: &str, conversation: &str, roster: Roster) {
        let mut known = moqui_lib::sync::lock(&self.by_conversation);
        // a process keeps at most this many; a Conversation it forgot is read again
        if known.len() >= 10_000 {
            known.clear();
        }
        known.insert(key(bridge_id, conversation), roster);
    }

    fn is_ready(&self, bridge_id: &str, conversation: &str) -> bool {
        moqui_lib::sync::lock(&self.ready).contains(&key(bridge_id, conversation))
    }

    fn set_ready(&self, bridge_id: &str, conversation: &str) {
        let mut ready = moqui_lib::sync::lock(&self.ready);
        if ready.len() >= 10_000 {
            ready.clear();
        }
        ready.insert(key(bridge_id, conversation));
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

/// A Conversation's SID and state from its resource (a state not named is taken as active).
fn sid_and_state(value: &Json) -> Option<(String, String)> {
    let sid = value
        .get("sid")
        .and_then(Json::as_str)
        .filter(|sid| is_sid(sid, "CH"))?;
    let state = value.get("state").and_then(Json::as_str).unwrap_or("active");
    Some((sid.to_string(), state.to_string()))
}

/// Send a group text: the room's Conversation (made on its first send, checked once a process), then the message.
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
    let conversation = match conversation_of(provider, client, config, out) {
        Ok(sid) => sid,
        Err(outcome) => return outcome,
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
    let outcome = exchange(provider, client, out, &request);
    // refused (the Conversation gone or closed since it was checked): checked again on the next send
    if matches!(outcome, DeliveryOutcome::Failed { .. }) {
        provider.participants.forget(&out.bridge.chat_bridge_id, &conversation);
    }
    outcome
}

/// The room's Conversation, ready for a message: its mapping's thread, else found by its unique name or made and kept
/// on the mapping; checked once a process (gone or closed: let go, a new one on the next try; initializing: wait; the
/// bridge's webhook on it).
fn conversation_of(
    provider: &Twilio,
    client: &RestClient,
    config: &Config,
    out: &Outgoing<'_>,
) -> Result<String, DeliveryOutcome> {
    let bridge_id = &out.bridge.chat_bridge_id;
    let (sid, state) = match out
        .request
        .external_thread_id
        .as_deref()
        .filter(|sid| is_sid(sid, "CH"))
    {
        Some(sid) if provider.participants.is_ready(bridge_id, sid) => return Ok(sid.to_string()),
        Some(sid) => {
            let state = state_of(client, config, sid)?;
            match state.as_deref() {
                None | Some("closed") => {
                    provider.participants.forget(bridge_id, sid);
                    let_go(out, sid);
                    return Err(retry(
                        format!(
                            "Twilio's Conversation {sid} of this group is {}; a new one is made on the next try",
                            if state.is_none() { "gone" } else { "closed" }
                        ),
                        false,
                    ));
                }
                Some(state) => (sid.to_string(), state.to_string()),
            }
        }
        None => {
            let (sid, state) = found_or_made(provider, client, config, out)?;
            keep_thread(out, &sid)?;
            (sid, state)
        }
    };
    if state == "initializing" {
        return Err(DeliveryOutcome::Wait {
            error: format!("Twilio is still adding the group's numbers to Conversation {sid}"),
            retry_after_ms: INITIALIZING_WAIT_MILLIS,
        });
    }
    hooked(client, config, out, &sid)?;
    provider.participants.set_ready(bridge_id, &sid);
    Ok(sid)
}

/// The state of Conversation `sid`; none when it is not there (404).
fn state_of(client: &RestClient, config: &Config, sid: &str) -> Result<Option<String>, DeliveryOutcome> {
    let found = setup_call(
        client,
        &form_request(
            config,
            Method::Get,
            format!("{}/Conversations/{sid}", conversations_base(config)),
            Vec::new(),
        ),
        "did not say whether the group's Conversation is there",
    )?;
    match found.status() {
        200 => Ok(Some(
            sid_and_state(&json_of(&found)).map_or_else(|| "active".to_string(), |(_, state)| state),
        )),
        404 => Ok(None),
        status => Err(retry(
            format!("Twilio answered {status} for the group's Conversation {sid}"),
            false,
        )),
    }
}

/// The Conversation of the room's unique name: found (one made before an answer was lost; one found closed is
/// deleted, freeing its name), else made with all its numbers, else (50438) the one Twilio has of these numbers.
fn found_or_made(
    provider: &Twilio,
    client: &RestClient,
    config: &Config,
    out: &Outgoing<'_>,
) -> Result<(String, String), DeliveryOutcome> {
    let base = conversations_base(config);
    let unique = unique_name(&out.bridge.chat_bridge_id, &out.request.external_room_id);
    let find = || -> Result<Option<(String, String)>, DeliveryOutcome> {
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
            200 => sid_and_state(&json_of(&found)),
            _ => None,
        })
    };
    match find()? {
        Some((sid, state)) if state == "closed" => delete(client, config, out, &sid),
        Some(found) => return Ok(found),
        None => {}
    }
    let names = out.theirs.join(", ");
    let mut form = vec![
        ("UniqueName".to_string(), unique.clone()),
        ("FriendlyName".to_string(), names.chars().take(200).collect()),
        (
            "Participant".to_string(),
            json!({"messaging_binding": {"projected_address": out.ours}}).to_string(),
        ),
    ];
    for number in out.theirs {
        form.push((
            "Participant".to_string(),
            json!({"messaging_binding": {"address": number}}).to_string(),
        ));
    }
    let made = setup_call(
        client,
        &form_request(
            config,
            Method::Post,
            format!("{base}/ConversationWithParticipants"),
            form,
        ),
        "did not make the group's Conversation",
    )?;
    if (200..300).contains(&made.status()) {
        return sid_and_state(&json_of(&made))
            .ok_or_else(|| retry("Twilio made the group's Conversation and named no SID", false));
    }
    let error = provider.error_of(made.body());
    match (made.status(), error.code.as_deref()) {
        // made at once by another send: found by its name
        (409, _) => find()?.ok_or_else(|| retry("Twilio's group Conversation is not there yet", false)),
        // another Conversation has these numbers (Twilio made it for a group text that came in): that one is the room's
        (_, Some("50438")) => existing_group(provider, client, config, out)
            .map(|sid| (sid, "active".to_string()))
            .ok_or_else(|| {
                failed(
                    "providerRefused",
                    "Twilio has another Conversation of this group's numbers, and it was not found",
                )
            }),
        (status, _) => Err(failed(
            "providerRefused",
            format!(
                "Twilio did not make the group's Conversation ({status}): {}",
                error.text
            ),
        )),
    }
}

/// Delete a closed Conversation of the room's unique name, so a new one can take it. A failure is logged: the make
/// that follows says why it cannot.
fn delete(client: &RestClient, config: &Config, out: &Outgoing<'_>, sid: &str) {
    let request = form_request(
        config,
        Method::Delete,
        format!("{}/Conversations/{sid}", conversations_base(config)),
        Vec::new(),
    );
    match call(client, &request) {
        Ok(response) if (200..300).contains(&response.status()) || response.status() == 404 => {}
        Ok(response) => tracing::warn!(
            "SMS bridge {}: Twilio's closed Conversation {sid} was not deleted ({})",
            out.bridge.chat_bridge_id,
            response.status()
        ),
        Err(error) => tracing::warn!(
            "SMS bridge {}: Twilio's closed Conversation {sid} was not deleted: {error}",
            out.bridge.chat_bridge_id
        ),
    }
}

/// The bridge's webhook on Conversation `sid`: its webhooks read, and one added naming the bridge's webhook URL when
/// none does (at most [`HOOKS_MAX`] a Conversation: one that has them all is logged and left).
fn hooked(client: &RestClient, config: &Config, out: &Outgoing<'_>, sid: &str) -> Result<(), DeliveryOutcome> {
    let Some(callback) = crate::sms::callback_url(out.bridge) else {
        tracing::warn!(
            "SMS bridge {}: no webhook URL (channels_public_url or webhookUrl) for Twilio Conversation {sid}: its \
             replies and receipts come only through the Conversations service's own webhook",
            out.bridge.chat_bridge_id
        );
        return Ok(());
    };
    let base = format!("{}/Conversations/{sid}/Webhooks", conversations_base(config));
    let listed = setup_call(
        client,
        &form_request(config, Method::Get, base.clone(), Vec::new()),
        "did not list the group Conversation's webhooks",
    )?;
    let hooks = if listed.status() == 200 {
        json_of(&listed)
            .get("webhooks")
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if hooks
        .iter()
        .any(|hook| hook.pointer("/configuration/url").and_then(Json::as_str) == Some(callback.as_str()))
    {
        return Ok(());
    }
    if hooks.len() >= HOOKS_MAX {
        tracing::warn!(
            "SMS bridge {}: Twilio Conversation {sid} has {HOOKS_MAX} webhooks and none is the bridge's; its replies \
             and receipts come only through the Conversations service's own webhook",
            out.bridge.chat_bridge_id
        );
        return Ok(());
    }
    let added = setup_call(
        client,
        &form_request(
            config,
            Method::Post,
            base,
            vec![
                ("Target".to_string(), "webhook".to_string()),
                ("Configuration.Url".to_string(), callback),
                ("Configuration.Method".to_string(), "post".to_string()),
                ("Configuration.Filters".to_string(), "onMessageAdded".to_string()),
                ("Configuration.Filters".to_string(), "onDeliveryUpdated".to_string()),
            ],
        ),
        "did not add the group Conversation's webhook",
    )?;
    if (200..300).contains(&added.status()) {
        Ok(())
    } else {
        Err(retry(
            format!(
                "Twilio did not add the group Conversation's webhook ({}): {}",
                added.status(),
                Twilio::default().error_of(added.body()).text
            ),
            false,
        ))
    }
}

/// The Conversation of exactly the group's numbers that Twilio has already: the `ParticipantConversations` of the first
/// other number, at most [`PAGES_READ`] pages, each open one's participants compared. Only pages under the bridge's
/// Conversations address are read (they carry its credentials).
fn existing_group(provider: &Twilio, client: &RestClient, config: &Config, out: &Outgoing<'_>) -> Option<String> {
    let first = out.theirs.first()?;
    let mut wanted: Vec<&str> = out.theirs.iter().map(String::as_str).collect();
    wanted.push(out.ours);
    wanted.sort_unstable();
    let mut page = Some(format!(
        "{}/ParticipantConversations?Address={}&PageSize=50",
        conversations_base(config),
        first.replace('+', "%2B")
    ));
    for _ in 0..PAGES_READ {
        let url = page.take()?;
        let response = call(client, &form_request(config, Method::Get, url, Vec::new()))
            .ok()
            .filter(|response| response.status() == 200)?;
        let listed = json_of(&response);
        let found = listed
            .get("conversations")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter(|entry| entry.get("conversation_state").and_then(Json::as_str) != Some("closed"))
            .filter_map(|entry| entry.get("conversation_sid").and_then(Json::as_str))
            .filter(|sid| is_sid(sid, "CH"))
            .find(|sid| {
                roster_of(provider, client, config, &out.bridge.chat_bridge_id, sid).is_ok_and(|roster| {
                    let mut numbers: Vec<&str> = roster.numbers.values().map(String::as_str).collect();
                    numbers.sort_unstable();
                    numbers.dedup();
                    numbers == wanted
                })
            })
            .map(str::to_string);
        if found.is_some() {
            return found;
        }
        page = listed
            .pointer("/meta/next_page_url")
            .and_then(Json::as_str)
            .filter(|next| next.starts_with(&config.conversations_base_url))
            .map(str::to_string);
    }
    None
}

/// Keep the Conversation's SID as the room's mapping's thread, at once (`store#MappingThread`, in a transaction of its
/// own): a later try carries it (`DeliveryRequest.external_thread_id`) and makes no other.
fn keep_thread(out: &Outgoing<'_>, sid: &str) -> Result<(), DeliveryOutcome> {
    crate::sms::store_thread(out.bridge, &out.request.external_room_id, sid).map_err(|error| {
        retry(
            format!("The group's Twilio Conversation {sid} was not kept on its room: {error}"),
            false,
        )
    })
}

/// Let a gone or closed Conversation go from the room's mapping, so the next try makes another. A failure is logged:
/// the next try finds the same and lets it go again.
fn let_go(out: &Outgoing<'_>, sid: &str) {
    if let Err(error) = crate::sms::store_thread(out.bridge, &out.request.external_room_id, "") {
        tracing::warn!(
            "SMS bridge {}: Twilio's Conversation {sid} was not let go from its room: {error}",
            out.bridge.chat_bridge_id
        );
    }
}

/// A Conversation's participants, read from Twilio (`GET .../Participants`, within [`ROSTER_TIMEOUT`]), kept for the
/// process.
pub fn roster_of(
    provider: &Twilio,
    client: &RestClient,
    config: &Config,
    bridge_id: &str,
    conversation: &str,
) -> Result<Roster, String> {
    if let Some(known) = provider.participants.get(bridge_id, conversation) {
        return Ok(known);
    }
    if !is_sid(conversation, "CH") {
        return Err(format!("{conversation} is not a Conversation's SID"));
    }
    let url = format!(
        "{}/Conversations/{conversation}/Participants?PageSize=50",
        conversations_base(config)
    );
    let mut read = client.get(url).timeout(ROSTER_TIMEOUT);
    for (name, value) in auth_headers(config) {
        read = read.header(name, value);
    }
    let response = read.send(false).map_err(|error| error.message().to_string())?;
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
            // a Conversation of one number (Twilio's autocreation): our number is its proxy address
            if let Some(proxy) = text("proxy_address") {
                roster.ours.get_or_insert(proxy);
            }
            roster.numbers.insert(sid.to_string(), address);
        }
    }
    provider.participants.put(bridge_id, conversation, roster.clone());
    Ok(roster)
}

/// The number of a participant of a Conversation, read again from Twilio when the kept roster does not have it.
pub fn number_of(
    provider: &Twilio,
    client: &RestClient,
    config: &Config,
    bridge_id: &str,
    conversation: &str,
    participant: &str,
) -> Result<Option<String>, String> {
    // a participant the kept roster does not have: it is read again (one added since)
    if let Some(known) = provider.participants.get(bridge_id, conversation) {
        if let Some(number) = known.numbers.get(participant) {
            return Ok(Some(number.clone()));
        }
        provider.participants.forget(bridge_id, conversation);
    }
    Ok(roster_of(provider, client, config, bridge_id, conversation)?
        .numbers
        .get(participant)
        .cloned())
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
        let Some(number) = number_of(
            provider,
            client,
            config,
            &bridge.chat_bridge_id,
            conversation,
            participant,
        )
        .map_err(unread)?
        else {
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
            opt_signal: None,
            path: None,
        }));
    }
    let roster = roster_of(provider, client, config, &bridge.chat_bridge_id, conversation).map_err(unread)?;
    let Some(ours) = roster.ours.clone() else {
        tracing::error!(
            "SMS bridge {}: a text in Twilio Conversation {conversation} was left: none of its participants is a \
             number of ours (no projected_address or proxy_address); is it a Conversation of another system on the \
             account?",
            bridge.chat_bridge_id
        );
        return Ok(SmsEvent::Ignored("a Conversation with no number of ours".to_string()));
    };
    let Some(author) = form.get("Author") else {
        return Err(WebhookResponse::text(400, "a Conversation's message names its author"));
    };
    // ours (the chat participant's identity is our number), or a chat identity of another system on the account
    if author == ours || !author.starts_with('+') {
        return Ok(SmsEvent::Ignored("a message of ours".to_string()));
    }
    let mut recipients: Vec<String> = vec![ours.clone()];
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
        // the room of the mapping that keeps it, when one does (receive#Sms); else the group's room by its numbers
        thread_id: Some(conversation.to_string()),
        origin_millis: form
            .get("DateCreated")
            .and_then(|date| date.parse::<jiff::Timestamp>().ok())
            .map(|stamp| stamp.as_millisecond()),
        ..Inbound::default()
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
