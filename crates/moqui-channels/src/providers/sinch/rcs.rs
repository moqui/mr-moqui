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

use serde_json::{json, Value as Json};

use super::Config;
use crate::sms::{Location, Outgoing, Path};

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
    let file = !out.media_urls.is_empty();
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
