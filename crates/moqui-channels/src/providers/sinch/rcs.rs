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

//! Business RCS on the Sinch Conversation API: the `rcs-sinch` adapter's part of [`super`] (docs/interfaces/RCS.md
//! "Sinch", plans/Comms-Messaging.md §16 RCS phase 1), **not yet tested with Sinch**: the shapes are from its
//! documentation. A `ChatBridge` of type `CbtRcs` with `adapterName` `rcs-sinch`, on a Conversation API app of its own
//! with the `RCS` channel (the agent) and the `SMS` channel (and `MMS` for files) set up; its `configJson` has the keys
//! of an `sms-sinch` bridge, `smsSender` required (the bridge's home number: our number of an RCS message, which names
//! none), and `smsFallback` (true by default; false sends RCS only).
//!
//! - **Out.** `messages:send` to the person's `RCS` identity and, with the fallback, their `SMS` identity (`MMS` for a
//!   file), `channel_priority_order` `["RCS","SMS"]` (or `"MMS"`; `["RCS"]` alone without the fallback), and the
//!   fallback's sender (`SMS_SENDER` or `MMS_SENDER`) the room's number. The message is one of the API's kinds (a text,
//!   a file, a card for words and a file), which Sinch renders for each channel.
//! - **In.** The `RCS` channel's `MESSAGE_INBOUND` too: `text_message`, `media_message`, `location_message` (a place)
//!   and `choice_response_message` (a tapped suggestion: its `postback_data`). Its `sender_id` is not read (it is no
//!   number): our number is `smsSender`. An `OPT_OUT` on the `RCS` channel is a STOP.
//! - **The path.** A delivery report's `channel_identity.channel` (`RCS`, `SMS`, `MMS`); `SWITCHING_CHANNEL` (RCS
//!   failed, the next channel is tried) is the fallback's.
//! - **Rich messages** (plans/Comms-Messaging.md §18.3 phase 13, [`rich_message`]): a text with chips is a
//!   `choice_message`, a card a `card_message`, cards a `carousel_message`; a reply a `text_message` choice with its
//!   `postback_data`, a link a `url_message`, a call a `call_message`. With the fallback, `explicit_channel_omni_message`
//!   gives the SMS channel our fallback text, or the MMS channel a card of it with the first image
//!   ([`fallback_override`]), not Sinch's own rendering of the card; it goes in the `message` (the AppMessage). Each
//!   picture is at most 100 MB (Sinch's RCS limit). `READ` is the message read on the phone.
//! - **Typing.** `events:send` with a `composing_event` to the person's `RCS` identity ([`typing_request`]). None comes
//!   in: Sinch's RCS channel documents no typing of the person.

use moqui_lib::http::Method;
use serde_json::{json, Value as Json};

use super::Config;
use crate::sms::rich::{Card, Rich, RichKind, Suggestion};
use crate::sms::{ApiBody, ApiRequest, Location, Outgoing, Path};

/// Why an `rcs-sinch` bridge cannot start with these settings.
///
/// # Errors
///
/// Returns the bridge's error, shown as its last error.
pub fn check(config: &Config) -> Result<(), String> {
    if config.sms_sender.is_none() {
        return Err(
            "The Sinch RCS bridge has no smsSender, its home number: our number of an RCS message that comes in"
                .to_string(),
        );
    }
    Ok(())
}

/// The recipient, the channel order and the fallback's sender of `out`, for [`super::Sinch::request`]'s body: (the
/// recipient's identities, `channel_priority_order`, `channel_properties`).
pub fn route(out: &Outgoing<'_>, theirs: &str) -> (Json, Json, Json) {
    // a rich message's fallback is MMS only with its picture (the first, within the MMS limit)
    let file = match out.rich {
        Some(_) => out.fallback_image.is_some(),
        None => !out.media_urls.is_empty(),
    };
    let (fallback, sender_key) = if file {
        ("MMS", "MMS_SENDER")
    } else {
        ("SMS", "SMS_SENDER")
    };
    if out.fallback {
        (
            json!([{"channel": "RCS", "identity": theirs}, {"channel": fallback, "identity": theirs}]),
            json!(["RCS", fallback]),
            json!({sender_key: out.ours}),
        )
    } else {
        (
            json!([{"channel": "RCS", "identity": theirs}]),
            json!(["RCS"]),
            json!({}),
        )
    }
}

/// The path a delivery report's channel names.
pub fn path_of(channel: Option<&str>) -> Option<Path> {
    match channel? {
        "RCS" => Some(Path::Rcs),
        "SMS" => Some(Path::Sms),
        "MMS" => Some(Path::Mms),
        _ => None,
    }
}

/// A place the person shared: `location_message` (`coordinates.latitude`, `coordinates.longitude`, `title`, `label`).
pub fn location_of(message: &Json) -> Option<Location> {
    let number = |pointer: &str| message.pointer(pointer).and_then(Json::as_f64);
    let text = |pointer: &str| message.pointer(pointer).and_then(Json::as_str);
    Location::of(
        number("/coordinates/latitude"),
        number("/coordinates/longitude"),
        text("/title").or_else(|| text("/label")),
        text("/label").filter(|_| text("/title").is_some()),
    )
}

/// A location message's place (a `DeliveryRequest`'s `location`: `latitude`, `longitude`, `label`, `address`) as a
/// Conversation API `location_message`: its `title` the label (else "Location"), its `label` the address. Sinch's RCS
/// page says RCS has no location message of its own and the API sends it as a text with a location choice (read
/// 2026-11-17); not yet tested with Sinch. None without both coordinates in range.
pub fn location_message(place: &Json) -> Option<Json> {
    let number = |key: &str| place.get(key).and_then(Json::as_f64);
    let text = |key: &str| place.get(key).and_then(Json::as_str);
    let place = Location::of(number("latitude"), number("longitude"), text("label"), text("address"))?;
    Some(json!({"location_message": {
        "coordinates": {"latitude": place.latitude, "longitude": place.longitude},
        "title": place.label.clone().unwrap_or_else(|| "Location".to_string()),
        "label": place.address.clone().unwrap_or_default(),
    }}))
}

/// A suggestion as a Conversation API choice.
fn choice(suggestion: &Suggestion) -> Json {
    match suggestion {
        Suggestion::Reply { text, postback } => json!({"text_message": {"text": text}, "postback_data": postback}),
        Suggestion::Url { text, url } => json!({"url_message": {"title": text, "url": url}}),
        Suggestion::Dial { text, phone } => json!({"call_message": {"title": text, "phone_number": phone}}),
    }
}

/// A card as a `card_message` (its image the message's file of `out`).
fn card(out: &Outgoing<'_>, card: &Card) -> Json {
    let mut body = json!({
        "title": card.title.clone().unwrap_or_default(),
        "choices": card.suggestions.iter().map(choice).collect::<Vec<_>>(),
    });
    if let Some(description) = &card.description {
        body["description"] = json!(description);
    }
    if let Some(url) = card.image.and_then(|index| out.media_urls.get(index)) {
        body["media_message"] = json!({"url": url});
    }
    body
}

/// The `message` of a rich send.
pub fn rich_message(out: &Outgoing<'_>, rich: &Rich) -> Json {
    match rich.kind {
        RichKind::Text => json!({"choice_message": {
            "text_message": {"text": rich.text.clone().unwrap_or_default()},
            "choices": rich.suggestions.iter().map(choice).collect::<Vec<_>>(),
        }}),
        RichKind::Card => json!({"card_message": rich.cards.first().map(|one| card(out, one)).unwrap_or_default()}),
        RichKind::Carousel => json!({"carousel_message": {
            "cards": rich.cards.iter().map(|one| card(out, one)).collect::<Vec<_>>(),
        }}),
    }
}

/// What the fallback's channel gets instead of Sinch's rendering of the cards: the SMS channel our fallback text, the
/// MMS channel (a rich message with an image) a card of it with the first image. None without the fallback.
pub fn fallback_override(out: &Outgoing<'_>, rich: &Rich) -> Option<Json> {
    if !out.fallback {
        return None;
    }
    let text = out.text?;
    Some(match out.fallback_image.and_then(|index| out.media_urls.get(index)) {
        Some(url) => {
            let title = rich
                .cards
                .iter()
                .find_map(|card| card.title.clone())
                .unwrap_or_default();
            json!({"MMS": {"card_message": {"title": title, "description": text, "media_message": {"url": url}}}})
        }
        None => json!({"SMS": {"text_message": {"text": text}}}),
    })
}

/// The typing indicator to `theirs` (`events:send`, a `composing_event` on the RCS channel), without its token.
pub fn typing_request(config: &Config, theirs: &str) -> ApiRequest {
    ApiRequest {
        method: Method::Post,
        url: format!("{}/v1/projects/{}/events:send", config.api_base, config.project_id),
        headers: Vec::new(),
        body: ApiBody::Json(json!({
            "app_id": config.app_id,
            "recipient": {"identified_by": {"channel_identities": [{"channel": "RCS", "identity": theirs}]}},
            "event": {"composing_event": {}},
            "channel_priority_order": ["RCS"],
        })),
    }
}
