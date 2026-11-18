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

use moqui_core::service::chat_bridge::WebhookResponse;
use moqui_lib::http::Method;

use super::{auth_headers, is_sid, Config, Form};
use crate::sms::{ApiBody, ApiRequest, Location, Outgoing, Path, Settings};

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
