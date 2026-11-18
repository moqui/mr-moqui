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

use moqui_core::service::chat_bridge::{BridgeInfo, DeliveryOutcome, WebhookResponse};
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

/// The most characters of a carousel's own `body`: RCS drops it and Twilio documents no limit, so a safe one; the
/// whole fallback goes on the SMS path (`twilio/text`, `twilio/media`).
pub const CAROUSEL_BODY_MAX: usize = 1024;
/// The pages of our Contents a sweep or a retry reads at most.
const LIST_PAGES: usize = 3;

/// A suggestion as a Content action. `chip` places it in the chip list under the message (a text's: `Some(true)`) or
/// on the card (`Some(false)`); a carousel's cards name none (its page documents no `chip_list`).
fn action(suggestion: &Suggestion, chip: Option<bool>) -> Json {
    let mut action = match suggestion {
        Suggestion::Reply { text, postback } => json!({"type": "QUICK_REPLY", "title": text, "id": postback}),
        Suggestion::Url { text, url } => json!({"type": "URL", "title": text, "url": url}),
        Suggestion::Dial { text, phone } => json!({"type": "PHONE_NUMBER", "title": text, "phone": phone}),
    };
    if let Some(chip) = chip {
        action["chip_list"] = Json::Bool(chip);
    }
    action
}

/// Our name of the Contents of a delivery: `moqui-` and its id (letters, digits, - and _).
pub fn name_of_delivery(delivery_id: &str) -> String {
    let delivery: String = delivery_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    format!("moqui-{delivery}")
}

/// The Content of `rich` for `out` (its fallback the message's text, and its first image with it as MMS), with the
/// fallback's type only when the send may fall back.
pub fn content_body(config: &Config, out: &Outgoing<'_>, rich: &Rich) -> Json {
    let image = |index: Option<usize>| index.and_then(|index| out.media_urls.get(index)).cloned();
    let mut types = serde_json::Map::new();
    match rich.kind {
        RichKind::Text => {
            let actions: Vec<Json> = rich.suggestions.iter().map(|one| action(one, Some(true))).collect();
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
            let actions: Vec<Json> = card.suggestions.iter().map(|one| action(one, Some(false))).collect();
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
                        "actions": card.suggestions.iter().map(|one| action(one, None)).collect::<Vec<_>>(),
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
            let body: String = out.text.unwrap_or_default().chars().take(CAROUSEL_BODY_MAX).collect();
            types.insert("twilio/carousel".to_string(), json!({"body": body, "cards": cards}));
        }
    }
    if out.fallback {
        if let Some(fallback) = out.text {
            // the first picture when it is within the MMS limit, else the text alone
            match image(out.fallback_image) {
                Some(url) => types.insert("twilio/media".to_string(), json!({"body": fallback, "media": [url]})),
                None => types.insert("twilio/text".to_string(), json!({"body": fallback})),
            };
        }
    }
    // a name of ours for each attempt, so a Content whose answer was lost is found again ([`send_rich`])
    let name = format!(
        "{}-{}",
        name_of_delivery(&out.request.chat_bridge_delivery_id),
        out.request.attempt
    );
    json!({"friendly_name": name, "language": config.content_language, "types": types})
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

/// Send `rich`: its Content (or an app's own named), then the send with its `ContentSid`.
///
/// - **A retry.** The Content an earlier attempt kept on the delivery is sent again when it has no picture (its
///   addresses expire); else it is deleted and a new one made. With none kept, the Contents of the delivery's earlier
///   attempts (by name: one whose answer was lost) are deleted first.
/// - **The Content step.** A `sid` goes on, kept on the delivery and its lease renewed for the second step; none, a lost
///   answer, 429 and 5xx are tried again (no message went); 401 and 403 too, with the bridge not connected; another 4xx
///   (Twilio refuses the card) sends the fallback text and its picture instead, noted on the delivery (`richRefused`).
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
            let bridge = out.bridge;
            let delivery = &out.request.chat_bridge_delivery_id;
            let kept = crate::sms::kept_content(delivery);
            let reuse = kept.clone().filter(|_| out.media_urls.is_empty());
            let sid = match reuse {
                Some(sid) => sid,
                None => {
                    match &kept {
                        // its pictures' addresses may have run out: a new one, the old deleted first
                        Some(old) => drop_kept(twilio, client, config, bridge, delivery, old),
                        // an earlier attempt's Content whose answer was lost
                        None if out.request.attempt > 1 && !delivery.is_empty() => {
                            for old in named_contents(twilio, client, config, &name_of_delivery(delivery)) {
                                drop_kept(twilio, client, config, bridge, delivery, &old);
                            }
                            crate::sms::renew_lease(bridge, delivery);
                        }
                        None => {}
                    }
                    match made_content(twilio, client, config, out, rich) {
                        Ok(sid) => {
                            if let Err(error) = crate::sms::keep_content(bridge, delivery, &sid) {
                                tracing::warn!(
                                    "RCS bridge {}: content {sid} was not kept on its delivery; the sweep finds it by \
                                     name: {error}",
                                    bridge.chat_bridge_id
                                );
                            }
                            sid
                        }
                        Err(Made::Outcome(outcome)) => return outcome,
                        // Twilio refuses the card: its text (and picture) goes, said on the delivery
                        Err(Made::Refused(error)) => {
                            tracing::warn!(
                                "RCS bridge {}: Twilio refused a card, its text goes instead: {error}",
                                bridge.chat_bridge_id
                            );
                            crate::sms::note_delivery(bridge, delivery, "richRefused", &error);
                            let urls: Vec<String> = out
                                .fallback_image
                                .and_then(|index| out.media_urls.get(index))
                                .cloned()
                                .into_iter()
                                .collect();
                            let plain = Outgoing {
                                media_urls: &urls,
                                rich: None,
                                fallback_image: None,
                                ..*out
                            };
                            return exchange(twilio, client, &plain, &request(config, settings, &plain));
                        }
                    }
                }
            };
            what.push(("ContentSid".to_string(), sid));
        }
    }
    exchange(twilio, client, out, &message_request(config, settings, out, what))
}

/// Delete `old`, a Content of the delivery, and let the delivery's field go; one not deleted stays for the sweep.
fn drop_kept(twilio: &Twilio, client: &RestClient, config: &Config, bridge: &BridgeInfo, delivery: &str, old: &str) {
    let deleted = match delete_request(config, old) {
        Some(request) => crate::sms::plain(twilio, client, &request, true),
        None => Ok(()),
    };
    match deleted {
        Ok(()) => {
            if let Err(error) = crate::sms::clear_content_of(bridge, delivery, old) {
                tracing::warn!(
                    "RCS bridge {}: content {old} stays named: {error}",
                    bridge.chat_bridge_id
                );
            }
        }
        Err(error) => tracing::warn!(
            "RCS bridge {}: an earlier attempt's content {old} was not deleted (the sweep tries again): {error}",
            bridge.chat_bridge_id
        ),
    }
}

/// Why no Content was made.
enum Made {
    /// The send's outcome (tried again, or the bridge not connected).
    Outcome(DeliveryOutcome),
    /// Twilio refused the card (a 4xx): its text.
    Refused(String),
}

/// The `sid` of the Content made for `rich`, or why none was.
fn made_content(
    twilio: &Twilio,
    client: &RestClient,
    config: &Config,
    out: &Outgoing<'_>,
    rich: &Rich,
) -> Result<String, Made> {
    let retry = |error: String, not_connected: bool| Made::Outcome(DeliveryOutcome::Retry { error, not_connected });
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
        400..=499 if status != 429 => Err(Made::Refused(format!(
            "Twilio refused the card ({status}): {}",
            twilio.error_of(response.body()).text
        ))),
        _ => Err(retry(
            format!(
                "Twilio's content step answered {status}: {}",
                twilio.error_of(response.body()).text
            ),
            false,
        )),
    }
}

/// The search of our Contents named `name` (Content API v2, `ContentName`), created before `before` when given.
fn search_request(config: &Config, name: &str, before: Option<&str>) -> ApiRequest {
    let mut pairs = vec![
        ("PageSize".to_string(), "100".to_string()),
        ("ContentName".to_string(), name.to_string()),
    ];
    if let Some(before) = before {
        pairs.push(("DateCreatedBefore".to_string(), before.to_string()));
    }
    ApiRequest {
        method: Method::Get,
        url: format!(
            "{}/v2/Content?{}",
            config.content_base_url,
            crate::sms::form_encode(&pairs)
        ),
        headers: auth_headers(config),
        body: ApiBody::None,
    }
}

/// The Contents of `request`'s search whose name starts with `prefix`, at most [`LIST_PAGES`] pages (by its
/// `meta.next_page_url` on the Content API's own host only). A page that cannot be read ends the search.
fn listed(
    twilio: &Twilio,
    client: &RestClient,
    config: &Config,
    request: ApiRequest,
    prefix: &str,
) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    let mut next = Some(request);
    for _ in 0..LIST_PAGES {
        let Some(request) = next.take() else { break };
        let response = crate::sms::call(client, &request).map_err(|error| error.message().to_string())?;
        if !(200..300).contains(&response.status()) {
            return Err(format!(
                "Twilio's content search answered {}: {}",
                response.status(),
                twilio.error_of(response.body()).text
            ));
        }
        let page: Json = serde_json::from_str(response.body()).unwrap_or(Json::Null);
        for content in page.get("contents").and_then(Json::as_array).into_iter().flatten() {
            let name = content.get("friendly_name").and_then(Json::as_str).unwrap_or_default();
            if let Some(sid) = content
                .get("sid")
                .and_then(Json::as_str)
                .filter(|sid| is_sid(sid, "HX"))
            {
                if name.starts_with(prefix) {
                    found.push(sid.to_string());
                }
            }
        }
        next = page
            .pointer("/meta/next_page_url")
            .and_then(Json::as_str)
            .filter(|url| url.starts_with(&config.content_base_url))
            .map(|url| ApiRequest {
                url: url.to_string(),
                ..request.clone()
            });
    }
    Ok(found)
}

/// The Contents of a delivery's earlier attempts (`moqui-{delivery}-`), for a retry; none when they cannot be read.
fn named_contents(twilio: &Twilio, client: &RestClient, config: &Config, name: &str) -> Vec<String> {
    let prefix = format!("{name}-");
    match listed(twilio, client, config, search_request(config, name, None), &prefix) {
        Ok(found) => found,
        Err(error) => {
            tracing::warn!("Twilio's contents of {name} were not read (the sweep finds them later): {error}");
            Vec::new()
        }
    }
}

/// Our Contents (`moqui-...`) made before `before_millis`, for the sweep of those no delivery keeps.
///
/// # Errors
///
/// Returns why they could not be read.
pub fn our_contents(
    twilio: &Twilio,
    client: &RestClient,
    config: &Config,
    before_millis: i64,
) -> Result<Vec<String>, String> {
    let before = jiff::Timestamp::from_millisecond(before_millis)
        .map(|at| at.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
        .map_err(|error| error.to_string())?;
    listed(
        twilio,
        client,
        config,
        search_request(config, "moqui-", Some(&before)),
        "moqui-",
    )
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
