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

//! Twilio Lookup v2 for the spam filter (plans/Comms-Messaging.md §18.3 phase 11; docs/interfaces/SMS.md "Spam"):
//! `GET {lookupsBaseUrl}/v2/PhoneNumbers/{number}?Fields=line_type_intelligence` with the API's Basic auth, a paid
//! request the framework makes only when the answer can change a verdict (`SmsSpam.js`). The line type keeps Twilio's
//! names (`mobile`, `landline`, `fixedVoip`, `nonFixedVoip`, `tollFree`, ...) and the carrier its name. Not yet tested
//! with the provider.

use moqui_core::service::chat_bridge::NumberLookup;
use moqui_lib::http::Method;
use serde_json::Value as Json;

use super::{auth_headers, Config};
use crate::sms::{ApiBody, ApiRequest, Settings};

/// The lookup of `number` (E.164 only, so nothing else reaches the path), when the bridge has credentials and an
/// https address (or a test's fake).
pub fn request(config: &Config, settings: &Settings, number: &str) -> Option<ApiRequest> {
    let digits = number.strip_prefix('+')?;
    if digits.len() < 5 || digits.len() > 15 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let base = &config.lookups_base_url;
    if !(base.starts_with("https://") || (settings.allow_internal && base.starts_with("http://"))) {
        return None;
    }
    let headers = auth_headers(config);
    if headers.is_empty() {
        return None;
    }
    Some(ApiRequest {
        method: Method::Get,
        url: format!("{base}/v2/PhoneNumbers/%2B{digits}?Fields=line_type_intelligence"),
        headers,
        body: ApiBody::None,
    })
}

/// The line type and carrier of Lookup's answer; None when it has neither.
pub fn answer(body: &str) -> Option<NumberLookup> {
    let json: Json = serde_json::from_str(body).ok()?;
    let line = &json["line_type_intelligence"];
    let text = |value: &Json| {
        value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| text.chars().take(100).collect::<String>())
    };
    let found = NumberLookup {
        line_type: text(&line["type"]),
        carrier_name: text(&line["carrier_name"]),
        caller_name: text(&json["caller_name"]["caller_name"]),
    };
    (found.line_type.is_some() || found.carrier_name.is_some()).then_some(found)
}
