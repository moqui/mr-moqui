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

//! Remote REST runner. The request goes out through [`crate::outbound`].

use std::collections::BTreeMap;

use moqui_lib::json::{from_json, to_json};
use moqui_lib::object::{to_plain_string, Value};

use crate::context::{Error, ExecutionContext};
use crate::outbound::{self, redacted, Method};

/// The reqwest client every outbound call shares when the `http` feature is built, so connections
/// and TLS state are reused.
#[cfg(feature = "http")]
pub fn shared_http_client() -> &'static moqui_lib::http::RestClient {
    static CLIENT: std::sync::OnceLock<moqui_lib::http::RestClient> = std::sync::OnceLock::new();
    CLIENT.get_or_init(moqui_lib::http::RestClient::new)
}

/// Call a `remote-rest` location. `sim` denies before anything is sent.
///
/// `method` overrides `verb`. The method is the one named when it is an HTTP method this client
/// sends (GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS) and POST otherwise. `allow_in_sim` stays false.
/// Only GET puts the parameters in the query; the other methods send them as a JSON body, which
/// HEAD and OPTIONS cannot carry. The response is the JSON object, or `{response: [...]}` for an
/// array; any other body fails the call, as Java's `jsonObject()`.
pub fn run_remote_rest(
    service_name: &str,
    location: &str,
    method: Option<&str>,
    verb: &str,
    parameters: &BTreeMap<String, Value>,
    sim: bool,
) -> Result<BTreeMap<String, Value>, Error> {
    if location.trim().is_empty() {
        return Err(Error::new(format!(
            "Location required to call remote service {service_name}"
        )));
    }
    let http_method = choose_method(method, verb);
    let mut url = expand_location(location, parameters)?;
    if http_method == Method::Get {
        url = with_query(&url, parameters);
    }
    let shown = redacted(&url);
    if sim {
        return Err(Error::new(format!(
            "HTTP {http_method} {shown} denied because sim mode is active"
        )));
    }
    let mut request = outbound::Request {
        method: http_method,
        url,
        headers: Vec::new(),
        body: None,
        timeout: None,
        deny_internal: false,
    };
    if http_method != Method::Get && !parameters.is_empty() {
        // Java's `RestClient.text` refuses a body for these.
        if matches!(http_method, Method::Head | Method::Options) {
            return Err(Error::new(format!("Cannot use body text with method {http_method}")));
        }
        let body = to_json(&Value::Map(
            parameters
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        ));
        request
            .headers
            .push(("content-type".to_string(), "application/json".to_string()));
        request.body = Some(body.to_string());
    }
    let response = outbound::send(request)?;
    let status = response.status;
    if !(200..300).contains(&status) {
        // The remote's body can hold anything, so it goes to the log and not into the message.
        tracing::warn!(service = service_name, status, url = %shown, body = %clip(&response.body), "remote REST error");
        let message = format!("Remote service error {status}: {}", outbound::reason_phrase(status));
        // As Java's `RemoteRestServiceRunner`, the error is a message and the call returns no results; with no context
        // to hold the message, it is the error.
        if ExecutionContext::with_current(|ctx| ctx.message().add_error(message.clone())).is_some() {
            return Ok(BTreeMap::new());
        }
        return Err(Error::new(message));
    }
    response_map(&response.body)
        .map_err(|()| Error::new(format!("Error parsing JSON response from request to {shown}")))
}

fn clip(text: &str) -> String {
    text.chars().take(500).collect()
}

fn choose_method(method: Option<&str>, verb: &str) -> Method {
    let named = method.map(str::trim).filter(|text| !text.is_empty());
    Method::parse(named.unwrap_or(verb)).unwrap_or(Method::Post)
}

/// Replace `${name}` with the parameter, percent-encoded, so a value cannot add a path segment, a
/// query, or a new host. Names that are not parameters stay for the environment expansion.
fn expand_location(location: &str, parameters: &BTreeMap<String, Value>) -> Result<String, Error> {
    let mut out = String::with_capacity(location.len());
    let mut rest = location;
    while let Some((before, after)) = rest.split_once("${") {
        out.push_str(before);
        let Some((name, tail)) = after.split_once('}') else {
            out.push_str("${");
            out.push_str(after);
            rest = "";
            break;
        };
        if let Some(value) = parameters.get(name) {
            out.push_str(&percent_encode(&to_plain_string(value)));
        } else {
            out.push_str("${");
            out.push_str(name);
            out.push('}');
        }
        rest = tail;
    }
    out.push_str(rest);
    Ok(moqui_lib::env::expand(&out)?)
}

fn with_query(url: &str, parameters: &BTreeMap<String, Value>) -> String {
    let query = encode_query(parameters);
    if query.is_empty() {
        return url.to_string();
    }
    let joiner = if url.contains('?') { '&' } else { '?' };
    format!("{url}{joiner}{query}")
}

/// Java's `RestClient.parametersMapToString`: every parameter that is not null, as its plain string, a list's items
/// joined with commas.
fn encode_query(parameters: &BTreeMap<String, Value>) -> String {
    let mut parts = Vec::new();
    for (key, value) in parameters {
        if value.is_null() {
            continue;
        }
        let text = to_plain_string(value);
        parts.push(format!("{}={}", percent_encode(key), percent_encode(&text)));
    }
    parts.join("&")
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(char::from(*byte)),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// A JSON object is the result and a JSON array is `response`. Anything else, an empty body too, is no JSON
/// payload to Java's `JsonSlurperClassic`, which takes only an object or an array.
fn response_map(body: &str) -> Result<BTreeMap<String, Value>, ()> {
    let parsed = serde_json::from_str::<serde_json::Value>(body.trim()).map(|json| from_json(&json));
    match parsed {
        Ok(Value::Map(map)) => Ok(map.into_iter().collect()),
        Ok(list @ Value::List(_)) => Ok(BTreeMap::from([("response".to_string(), list)])),
        _ => Err(()),
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
