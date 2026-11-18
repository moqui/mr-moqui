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

//! Business RCS on Twilio: the `rcs-twilio` adapter's part of [`super`] (docs/interfaces/RCS.md "Twilio",
//! plans/Comms-Messaging.md §16 RCS phase 1), **not yet tested with Twilio**: its requests and webhooks are from Twilio's
//! documentation. A `ChatBridge` of type `CbtRcs` with `adapterName` `rcs-twilio`; its `configJson` has the keys of an
//! `sms-twilio` bridge, and:
//!
//! | Key | What |
//! | --- | --- |
//! | `messagingServiceSid` | The Messaging Service (`MG...`) whose sender pool holds the RCS sender, required: Twilio falls back to SMS or MMS only through one |
//! | `fallbackFrom` | The bridge's home number (E.164), required: our number of an RCS message that comes in (it names the agent, not a number) |
//! | `smsFallback` | true (the default): a phone with no RCS gets the message as SMS or MMS from the room's number; false: RCS only |
//! | `contentLanguage` | The language of the contents made for rich messages (`en`) |
//! | `rcsSenderId` | The RCS agent's id (the `rcs:` address Twilio's inbound `To` names): the typing indicator's `from`; none sends no indicator |
//! | `typingIndicators` | true (the default): a user typing in a room shows the person the agent is typing; false: none |
//!
//! - **Out.** As an SMS: `POST .../Messages.json` with `MessagingServiceSid`, `To` the person's number (`rcs:` before
//!   it when `smsFallback` is false: Twilio then sends RCS only), `FallbackFrom` the room's number (Twilio's fallback
//!   goes from it, not from another number of the pool, so a reply comes to the room's number), `Body`, `MediaUrl`,
//!   `StatusCallback`. No group texts (RBM has none).
//! - **In.** An RCS message's `From` is `rcs:` and the person's number, its `To` `rcs:` and the sender's id (Twilio's
//!   "channel address"): the prefix is taken off their number, and our number is `fallbackFrom`. A reply to a fallback
//!   text is an SMS to the room's number, as for `sms-twilio`. A shared place (`Latitude`, `Longitude`, `Label`,
//!   `Address`) and a tapped suggestion (`ButtonPayload`; its `Body` is the button's words) are kept on the message.
//! - **The path.** A status callback's `From` is `rcs:` and the sender's id when the message went as RCS, else the
//!   number the fallback went from ([`path_of`]); the callbacks name no other field for it.
//! - **Rich messages** (plans/Comms-Messaging.md §18.3 phase 13, [`send_rich`]). Two calls: a Content made for the send
//!   (`POST {contentBaseUrl}/v1/Content`: `twilio/card` for a card or a text with chips, `twilio/carousel` for cards, and
//!   with the fallback `twilio/text` or, with an image, `twilio/media`, the message's own fallback), kept on the
//!   delivery ([`crate::sms::keep_content`]); then the send with its `ContentSid` in place of `Body` and `MediaUrl`. An
//!   app's own `providerContent.twilio.contentSid` (with `ContentVariables`) is sent as it is, never made or deleted.
//!   The Content is deleted ([`delete_request`]) once the message's receipt is final, else by the daily sweep.
//! - **Read.** A status callback's `read` is the message read on the phone.
//! - **Typing.** `POST {messagingBaseUrl}/v3/Indicators/Typing.json` ([`typing_request`]): `channel` RCS, `from`
//!   `rcs:` and `rcsSenderId`, `to` `rcs:` and the person's number, `event` START. Twilio shows it up to 20 seconds or
//!   until a message arrives. None comes in: Twilio documents no typing of the person.

use moqui_core::service::chat_bridge::{DeliveryOutcome, WebhookResponse};
use moqui_lib::http::{Method, RestClient};
use serde_json::{json, Value as Json};

use super::{auth_headers, is_sid, Config, Form, Twilio};
use crate::sms::rich::{Rich, RichKind, Suggestion};
use crate::sms::{exchange, ApiBody, ApiRequest, Location, Outgoing, Path, Settings, SmsProvider};

/// The prefix of an RCS address in Twilio's `From` and `To` (its "channel address").
pub const PREFIX: &str = "rcs:";

/// Whether `number` is in E.164 (`+` and 4 to 15 digits).
fn e164(number: &str) -> bool {
    number
        .strip_prefix('+')
        .is_some_and(|digits| (4..=15).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Why an `rcs-twilio` bridge cannot start with these settings.
///
/// # Errors
///
/// Returns the bridge's error, shown as its last error.
pub fn check(config: &Config) -> Result<(), String> {
    if !config
        .messaging_service_sid
        .as_deref()
        .is_some_and(|sid| is_sid(sid, "MG"))
    {
        return Err(
            "The Twilio RCS bridge has no messagingServiceSid (MG...): its RCS sender is in a Messaging Service"
                .to_string(),
        );
    }
    if !config.fallback_from.as_deref().is_some_and(e164) {
        return Err("The Twilio RCS bridge has no fallbackFrom, its home number in E.164".to_string());
    }
    Ok(())
}

/// The send of `out` to its one number: through the Messaging Service, its fallback from the room's number.
pub fn request(config: &Config, settings: &Settings, out: &Outgoing<'_>) -> ApiRequest {
    let mut what = Vec::new();
    if let Some(text) = out.text {
        what.push(("Body".to_string(), text.to_string()));
    }
    for url in out.media_urls {
        what.push(("MediaUrl".to_string(), url.clone()));
    }
    message_request(config, settings, out, what)
}

/// A Messages API send of `out` with `what` it carries (`Body` and `MediaUrl`, or `ContentSid`).
fn message_request(
    config: &Config,
    settings: &Settings,
    out: &Outgoing<'_>,
    what: Vec<(String, String)>,
) -> ApiRequest {
    let theirs = out.theirs.first().cloned().unwrap_or_default();
    let to = if out.fallback {
        theirs
    } else {
        format!("{PREFIX}{theirs}")
    };
    let mut form = vec![
        ("To".to_string(), to),
        (
            "MessagingServiceSid".to_string(),
            config.messaging_service_sid.clone().unwrap_or_default(),
        ),
    ];
    if out.fallback {
        form.push(("FallbackFrom".to_string(), out.ours.to_string()));
    }
    form.extend(what);
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

/// A suggestion as a Content action. `chip` places it in the chip list under the message (a text's), else on the card.
fn action(suggestion: &Suggestion, chip: bool) -> Json {
    let mut action = match suggestion {
        Suggestion::Reply { text, postback } => json!({"type": "QUICK_REPLY", "title": text, "id": postback}),
        Suggestion::Url { text, url } => json!({"type": "URL", "title": text, "url": url}),
        Suggestion::Dial { text, phone } => json!({"type": "PHONE_NUMBER", "title": text, "phone": phone}),
    };
    action["chip_list"] = Json::Bool(chip);
    action
}

/// The Content of `rich` for `out` (its fallback the message's text, and its first image with it as MMS), with the
/// fallback's type only when the send may fall back.
pub fn content_body(config: &Config, out: &Outgoing<'_>, rich: &Rich) -> Json {
    let image = |index: Option<usize>| index.and_then(|index| out.media_urls.get(index)).cloned();
    let mut types = serde_json::Map::new();
    match rich.kind {
        RichKind::Text => {
            let actions: Vec<Json> = rich.suggestions.iter().map(|one| action(one, true)).collect();
            types.insert(
                "twilio/card".to_string(),
                json!({"body": rich.text.clone().unwrap_or_default(), "actions": actions}),
            );
        }
        RichKind::Card => {
            let card = rich.cards.first().cloned().unwrap_or_default();
            let mut body = serde_json::Map::new();
            if let Some(title) = &card.title {
                body.insert("title".to_string(), json!(title));
            }
            if let Some(description) = &card.description {
                body.insert("body".to_string(), json!(description));
            }
            if let Some(url) = image(card.image) {
                body.insert("media".to_string(), json!([url]));
            }
            let actions: Vec<Json> = card.suggestions.iter().map(|one| action(one, false)).collect();
            if !actions.is_empty() {
                body.insert("actions".to_string(), Json::Array(actions));
            }
            types.insert("twilio/card".to_string(), Json::Object(body));
        }
        RichKind::Carousel => {
            let cards: Vec<Json> = rich
                .cards
                .iter()
                .map(|card| {
                    let mut body = json!({
                        "body": card.description.clone().or_else(|| card.title.clone()).unwrap_or_default(),
                        "actions": card.suggestions.iter().map(|one| action(one, false)).collect::<Vec<_>>(),
                    });
                    if let Some(title) = card.title.as_ref().filter(|_| card.description.is_some()) {
                        body["title"] = json!(title);
                    }
                    if let Some(url) = image(card.image) {
                        body["media"] = json!(url);
                    }
                    body
                })
                .collect();
            types.insert(
                "twilio/carousel".to_string(),
                json!({"body": out.text.unwrap_or_default(), "cards": cards}),
            );
        }
    }
    if out.fallback {
        if let Some(fallback) = out.text {
            match image(rich.first_image()) {
                Some(url) => types.insert("twilio/media".to_string(), json!({"body": fallback, "media": [url]})),
                None => types.insert("twilio/text".to_string(), json!({"body": fallback})),
            };
        }
    }
    // a name of ours, from the delivery's id (letters, digits, - and _)
    let delivery: String = out
        .request
        .chat_bridge_delivery_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    json!({"friendly_name": format!("moqui-{delivery}"), "language": config.content_language, "types": types})
}

/// The request that makes the Content of `rich` for `out`.
pub fn content_request(config: &Config, out: &Outgoing<'_>, rich: &Rich) -> ApiRequest {
    ApiRequest {
        method: Method::Post,
        url: format!("{}/v1/Content", config.content_base_url),
        headers: auth_headers(config),
        body: ApiBody::Json(content_body(config, out, rich)),
    }
}

/// Send `rich`: its Content made (or an app's own named), then the send with its `ContentSid`. The Content step's
/// answer: a `sid` goes on; none, a lost answer, 429 and 5xx are tried again (no message went: another Content is
/// harmless); 401 and 403 too, with the bridge not connected; another 4xx fails the send (`richRefused`).
pub fn send_rich(
    twilio: &Twilio,
    client: &RestClient,
    config: &Config,
    settings: &Settings,
    out: &Outgoing<'_>,
    rich: &Rich,
) -> DeliveryOutcome {
    let mut what = Vec::new();
    match &rich.twilio {
        // an app's own Content: as it is, never kept for deleting
        Some(own) => {
            what.push(("ContentSid".to_string(), own.content_sid.clone()));
            if let Some(variables) = &own.variables {
                what.push(("ContentVariables".to_string(), variables.clone()));
            }
        }
        None => {
            let sid = match made_content(twilio, client, config, out, rich) {
                Ok(sid) => sid,
                Err(outcome) => return outcome,
            };
            if let Err(error) = crate::sms::keep_content(out.bridge, &out.request.chat_bridge_delivery_id, &sid) {
                tracing::warn!(
                    "RCS bridge {}: content {sid} was not kept on its delivery, so it is not deleted: {error}",
                    out.bridge.chat_bridge_id
                );
            }
            what.push(("ContentSid".to_string(), sid));
        }
    }
    exchange(twilio, client, out, &message_request(config, settings, out, what))
}

/// The `sid` of the Content made for `rich`, or the send's outcome when it was not made.
fn made_content(
    twilio: &Twilio,
    client: &RestClient,
    config: &Config,
    out: &Outgoing<'_>,
    rich: &Rich,
) -> Result<String, DeliveryOutcome> {
    let retry = |error: String, not_connected: bool| DeliveryOutcome::Retry { error, not_connected };
    let response = crate::sms::call(client, &content_request(config, out, rich))
        .map_err(|error| retry(format!("Twilio's content step failed: {}", error.message()), false))?;
    let status = response.status();
    match status {
        200..=299 => serde_json::from_str::<Json>(response.body())
            .ok()
            .and_then(|body| body.get("sid").and_then(Json::as_str).map(str::to_string))
            .filter(|sid| is_sid(sid, "HX"))
            .ok_or_else(|| retry("Twilio made the content and named no id".to_string(), false)),
        401 | 403 => Err(retry(format!("Twilio refused the API key ({status})"), true)),
        400..=499 if status != 429 => Err(DeliveryOutcome::Failed {
            reason_code: "richRefused".to_string(),
            error: format!(
                "Twilio refused the card ({status}): {}",
                twilio.error_of(response.body()).text
            ),
        }),
        _ => Err(retry(
            format!(
                "Twilio's content step answered {status}: {}",
                twilio.error_of(response.body()).text
            ),
            false,
        )),
    }
}

/// The typing indicator to `theirs` from the agent; none without `rcsSenderId`.
pub fn typing_request(config: &Config, theirs: &str) -> Option<ApiRequest> {
    let sender = config.rcs_sender_id.as_deref()?;
    Some(ApiRequest {
        method: Method::Post,
        url: format!("{}/v3/Indicators/Typing.json", config.messaging_base_url),
        headers: auth_headers(config),
        body: ApiBody::Json(json!({
            "channel": "RCS",
            "from": format!("{PREFIX}{sender}"),
            "to": format!("{PREFIX}{}", bare(theirs)),
            "event": "START",
        })),
    })
}

/// The delete of a Content this adapter made; none for an id that is not a Content's (`HX...`).
pub fn delete_request(config: &Config, content_id: &str) -> Option<ApiRequest> {
    is_sid(content_id, "HX").then(|| ApiRequest {
        method: Method::Delete,
        url: format!("{}/v1/Content/{content_id}", config.content_base_url),
        headers: auth_headers(config),
        body: ApiBody::None,
    })
}

/// `address` without Twilio's `rcs:` prefix.
pub fn bare(address: &str) -> &str {
    address.strip_prefix(PREFIX).unwrap_or(address)
}

/// The person's number of an RCS address or a number (`rcs:+15557778888`, `+15557778888`), the prefix taken off. A
/// sender that is not in E.164 (a short code) is taken as `sms-twilio` takes it: `receive#Sms` gives it a room of its own.
pub fn their_number(address: &str) -> String {
    bare(address).to_string()
}

/// Our number of a message to `to`: the number itself, or for the agent (`rcs:` and the sender's id) the bridge's
/// `fallbackFrom`.
///
/// # Errors
///
/// Returns the answer when the message is to the agent and the bridge has no `fallbackFrom`.
pub fn our_number(config: &Config, to: &str) -> Result<String, WebhookResponse> {
    if !to.starts_with(PREFIX) {
        return Ok(to.to_string());
    }
    config.fallback_from.clone().ok_or_else(|| {
        WebhookResponse::text(
            400,
            "an RCS message came and the bridge has no fallbackFrom, its home number",
        )
    })
}

/// The path of a status callback: RCS when its `From` is the agent's, else the fallback's (SMS or MMS by the message's
/// files); none when it names no `From`.
pub fn path_of(form: &Form) -> Option<Path> {
    let from = form.get("From")?;
    Some(if from.starts_with(PREFIX) {
        Path::Rcs
    } else {
        Path::Fallback
    })
}

/// A place the person shared: `Latitude`, `Longitude`, `Label`, `Address`.
pub fn location_of(form: &Form) -> Option<Location> {
    let number = |name: &str| form.get(name).and_then(|value| value.trim().parse::<f64>().ok());
    Location::of(
        number("Latitude"),
        number("Longitude"),
        form.get("Label"),
        form.get("Address"),
    )
}
