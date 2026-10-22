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

//! `send#SystemMessageRest`, from its Groovy script, and the `/rest/sm` receive, from `WebFacadeImpl.handleSystemMessage`.
//! The HMAC values are `WebUtilities.hmacSha256Base64`, `hmacSha256Hex`, and `hmacSha256TimestampHeader`.

use std::collections::BTreeMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hmac::{Hmac, Mac};
use indexmap::IndexMap;
use moqui_lib::codec::hex_encode;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use sha2::Sha256;

use super::{add_error, has_error, param_text, text_of};
use crate::context::{Error, ExecutionContext};
use crate::entity::{EntityFacade, EntityValue};
use crate::service::call::{sync_with, CallOptions};
use crate::web::{self, WebRequest};

pub(super) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, send_rest)?;
    Ok(())
}

const MESSAGE: &str = "moqui.service.message.SystemMessage";
const MESSAGE_TYPE: &str = "moqui.service.message.SystemMessageType";
const REMOTE: &str = "moqui.service.message.SystemMessageRemote";
const RECEIVE: &str = "org.moqui.impl.SystemMessageServices.receive#IncomingSystemMessage";
const REPLAY_CACHE: &str = "moqui.security.hmac.replay";
/// Clock skew allowed ahead of now for a timestamped HMAC, and how old one may be.
const SKEW_MILLIS: i64 = 10_000;
const MAX_AGE_MILLIS: i64 = 300_000;

type HmacSha256 = Hmac<Sha256>;

/// `send#SystemMessageRest`: POST the message text to the remote's `sendUrl` and read the remote message ID from a
/// JSON reply of `{systemMessageIdList: [...]}`. A failure is a message error, so `send#ProducedSystemMessage` puts the
/// status back.
pub(super) fn send_rest(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let message_id = param_text(parameters, "systemMessageId");
    let Some(message) = find_one(facade, MESSAGE, "systemMessageId", &message_id)? else {
        add_error(format!("System message {message_id} not found, not sending."));
        return Ok(BTreeMap::new());
    };
    let message_type = find_one(
        facade,
        MESSAGE_TYPE,
        "systemMessageTypeId",
        &field(&message, "systemMessageTypeId"),
    )?;
    let remote_id = field(&message, "systemMessageRemoteId");
    if remote_id.is_empty() {
        add_error(format!(
            "System message {message_id} has no systemMessageRemoteId, not sending."
        ));
        return Ok(BTreeMap::new());
    }
    let Some(remote) = find_one(facade, REMOTE, "systemMessageRemoteId", &remote_id)? else {
        add_error(format!(
            "System message {message_id} remote {remote_id} not found, not sending."
        ));
        return Ok(BTreeMap::new());
    };

    let url = {
        let _frame = crate::script::ContextFrame::push();
        crate::script::set_context_name("remoteMessageId", Value::string(&message_id))?;
        crate::script::set_context_name(
            "systemMessageTypeId",
            Value::string(field(&message, "systemMessageTypeId")),
        )?;
        crate::script::set_context_name("systemMessageRemoteId", Value::string(&remote_id))?;
        crate::script::expand(&field(&remote, "sendUrl"))?
    };
    let content_type = message_type
        .as_ref()
        .map(|row| field(row, "contentType"))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "text/plain".to_string());
    let body = field(&message, "messageText");
    let mut headers = vec![("content-type".to_string(), content_type)];
    let auth = field(&remote, "messageAuthEnumId");
    match auth.as_str() {
        "" | "SmatLogin" => {
            let credentials = format!("{}:{}", field(&remote, "username"), field(&remote, "password"));
            headers.push((
                "Authorization".to_string(),
                format!("Basic {}", STANDARD.encode(credentials)),
            ));
        }
        "SmatHmacSha256" | "SmatHmacSha256Timestamp" => {
            let header_name = field(&remote, "authHeaderName");
            let secret = field(&remote, "sharedSecret");
            if header_name.is_empty() || secret.is_empty() {
                add_error(format!(
                    "Send REST HMAC for remote {remote_id} needs authHeaderName and sharedSecret"
                ));
                return Ok(BTreeMap::new());
            }
            let value = if auth == "SmatHmacSha256" {
                hmac_base64(&secret, &body)
            } else {
                timestamp_header(&secret, &body, super::now_millis() / 1000)
            };
            headers.push((header_name, value));
        }
        "SmatNone" => {}
        other => {
            add_error(format!(
                "Send REST message auth type {other} not supported for remote {remote_id}"
            ));
            return Ok(BTreeMap::new());
        }
    }

    if facade.sim_active() {
        return Err(LibError::new(format!(
            "HTTP POST {} denied because sim mode is active",
            crate::outbound::redacted(&url)
        )));
    }
    crate::entity::fence::check_effect(
        crate::entity::fence::Effect::RemoteCall,
        &format!("HTTP POST {}", crate::outbound::redacted(&url)),
    )?;
    let response = crate::outbound::send(crate::outbound::Request {
        method: crate::outbound::Method::Post,
        url,
        headers,
        body: Some(body),
        timeout: None,
    })?;
    let status = response.status;
    if !(200..300).contains(&status) {
        add_error(format!(
            "System message {message_id} send error response ({status}): {}",
            response.body
        ));
        return Ok(BTreeMap::new());
    }
    // A body that is not JSON is still a successful send.
    let mut out = BTreeMap::new();
    if let Ok(Value::Map(reply)) = super::super::parse_json(&response.body) {
        match reply.get("systemMessageIdList") {
            Some(Value::List(ids)) if !ids.is_empty() => {
                out.insert("remoteMessageId".to_string(), ids[0].clone());
            }
            Some(Value::String(id)) => {
                out.insert("remoteMessageId".to_string(), Value::string(id));
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The `sm` transition of `rest.xml`: `/rest/sm/{systemMessageTypeId}/{systemMessageRemoteId}/{remoteMessageId}` with
/// the message as the request body. The remote's `messageAuthEnumId` picks the check: a logged-in user authorized for
/// `receive#IncomingSystemMessage` (SmatLogin or none set), an HMAC header (SmatHmacSha256, or SmatHmacSha256Timestamp
/// with a five minute window and a replay cache), or nothing (SmatNone). The reply is `{systemMessageIdList: [...]}`.
pub(crate) fn receive_rest_message(request: &WebRequest, extra: &[String]) -> Result<bool, Error> {
    if extra.len() < 2 {
        web::send_json_error(400, "No message type or remote system specified");
        return Ok(true);
    }
    let type_id = extra[0].as_str();
    let remote_id = extra[1].as_str();
    let remote_message_id = extra.get(2).map(String::as_str);
    // serve_at moves the body into the web state for web scripts; a direct call keeps it on the request
    let body = if request.body.is_empty() { web::request_body() } else { bytes::Bytes::from(request.body.clone()) };
    let text = String::from_utf8_lossy(&body).into_owned();
    if text.is_empty() {
        web::send_json_error(400, "Request body empty");
        return Ok(true);
    }
    if let Err(err) = receive_checked(request, type_id, remote_id, remote_message_id, &text) {
        tracing::error!(
            error = %err,
            "Error handling system message type {type_id} remote {remote_id} remote msg {}",
            remote_message_id.unwrap_or("")
        );
        web::send_json_error(500, &format!("Error receiving message: {err}"));
    }
    Ok(true)
}

fn receive_checked(
    request: &WebRequest,
    type_id: &str,
    remote_id: &str,
    remote_message_id: Option<&str>,
    text: &str,
) -> Result<(), Error> {
    let facade = ExecutionContext::with_current(|ctx| ctx.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade is installed"))?;
    // Make sure the type and the remote are valid before the service call.
    if find_one(&facade, MESSAGE_TYPE, "systemMessageTypeId", type_id)?.is_none() {
        web::send_json_error(400, &format!("Message type {type_id} not valid"));
        return Ok(());
    }
    let Some(remote) = find_one(&facade, REMOTE, "systemMessageRemoteId", remote_id)? else {
        web::send_json_error(400, &format!("Remote system {remote_id} not valid"));
        return Ok(());
    };

    let auth = field(&remote, "messageAuthEnumId");
    match auth.as_str() {
        "" | "SmatLogin" => {
            // The user is logged in by this point, by the request's credentials, or not at all.
            let Some(user_id) = crate::user::current_user_id()? else {
                web::send_json_error(
                    401,
                    &format!("Receive message for remote system {remote_id} requires login"),
                );
                return Ok(());
            };
            if let Err(err) = crate::user::authorize("AT_SERVICE", RECEIVE, "all") {
                tracing::warn!("Authz failure for system message receive from remote {remote_id}: {err}");
                web::send_json_error(
                    403,
                    &format!("Receive message for remote system {remote_id} not authorized for user with ID {user_id}"),
                );
                return Ok(());
            }
        }
        "SmatHmacSha256" => {
            let Some((header_name, secret)) = hmac_settings(&remote, remote_id) else {
                web::send_json_error(403, &format!("Remote system {remote_id} auth configuration not valid"));
                return Ok(());
            };
            let Some(header) = request.header(&header_name).filter(|value| !value.is_empty()) else {
                tracing::warn!(
                    "System message receive HMAC verify no header {header_name} value found, for remote {remote_id}"
                );
                web::send_json_error(
                    403,
                    &format!("No HMAC header {header_name} found for remote system {remote_id}"),
                );
                return Ok(());
            };
            let signature = hmac_base64(&secret, text);
            if !same_text(header, &signature) {
                tracing::warn!("System message receive HMAC verify did not match for remote {remote_id}");
                web::send_json_error(403, &format!("HMAC verify failed for remote system {remote_id}"));
                return Ok(());
            }
            crate::user::login_anonymous()?;
        }
        "SmatHmacSha256Timestamp" => {
            let Some((header_name, secret)) = hmac_settings(&remote, remote_id) else {
                web::send_json_error(403, &format!("Remote system {remote_id} auth configuration not valid"));
                return Ok(());
            };
            let Some(header) = request.header(&header_name).filter(|value| !value.is_empty()) else {
                tracing::warn!(
                    "System message receive HMAC verify no header {header_name} value found, for remote {remote_id}"
                );
                web::send_json_error(
                    403,
                    &format!("No HMAC header {header_name} found for remote system {remote_id}"),
                );
                return Ok(());
            };
            // A header like `t=1492774577,v1=5257a869...`, signed over the timestamp, a period, and the body.
            let (timestamp, incoming) = timestamp_parts(header);
            let signature = hmac_hex(&secret, &format!("{}.{text}", timestamp.as_deref().unwrap_or("null")));
            if !same_text(incoming.as_deref().unwrap_or(""), &signature) {
                tracing::warn!("System message receive HMAC verify did not match for remote {remote_id}");
                web::send_json_error(403, &format!("HMAC verify failed for remote system {remote_id}"));
                return Ok(());
            }
            let seconds: i64 = timestamp
                .as_deref()
                .unwrap_or("")
                .parse()
                .map_err(|_| Error::new(format!("HMAC timestamp {timestamp:?} is not a number")))?;
            let incoming_millis = seconds.saturating_mul(1000);
            let now = super::now_millis() + SKEW_MILLIS;
            if incoming_millis >= now || incoming_millis <= now - MAX_AGE_MILLIS {
                tracing::warn!(
                    "System message receive HMAC invalid incoming timestamp {incoming_millis} for remote {remote_id}"
                );
                web::send_json_error(403, "HMAC timestamp verification failed");
                return Ok(());
            }
            let replay_key = format!(
                "{remote_id}:{}:{}",
                timestamp.unwrap_or_default(),
                incoming.unwrap_or_default()
            );
            if !first_use(&replay_key)? {
                tracing::warn!("System message receive HMAC replay for remote {remote_id}");
                web::send_json_error(403, "HMAC replay rejected");
                return Ok(());
            }
            crate::user::login_anonymous()?;
        }
        // An explicit unauthenticated remote, with the same anonymous login as the HMAC paths.
        "SmatNone" => {
            crate::user::login_anonymous()?;
        }
        other => {
            tracing::error!("Got system message for remote {remote_id} with unsupported messageAuthEnumId {other}");
            web::send_json_error(400, &format!("Remote system {remote_id} auth configuration not valid"));
            return Ok(());
        }
    }

    // Authorization is disabled for the call since the check above already ran when it applies.
    let mut parameters = BTreeMap::new();
    parameters.insert("systemMessageTypeId".to_string(), Value::string(type_id));
    parameters.insert("systemMessageRemoteId".to_string(), Value::string(remote_id));
    if let Some(id) = remote_message_id {
        parameters.insert("remoteMessageId".to_string(), Value::string(id));
    }
    parameters.insert("messageText".to_string(), Value::string(text));
    let options = CallOptions {
        disable_authz: true,
        ..CallOptions::default()
    };
    let result = sync_with(RECEIVE, &parameters, &options);
    if has_error() {
        let errors = ExecutionContext::with_current(|ctx| ctx.message().errors().join("\n")).unwrap_or_default();
        web::send_json_error(500, &errors);
        return Ok(());
    }
    let ids = match result?.remove("systemMessageIdList") {
        Some(Value::List(ids)) => ids,
        _ => Vec::new(),
    };
    let mut reply = IndexMap::new();
    reply.insert("systemMessageIdList".to_string(), Value::List(ids));
    web::send_json_response(Value::Map(reply));
    Ok(())
}

fn find_one(facade: &EntityFacade, entity: &str, field: &str, value: &str) -> Result<Option<EntityValue>, LibError> {
    if value.is_empty() {
        return Ok(None);
    }
    facade
        .find(entity)?
        .disable_authz()
        .eq(field, Value::string(value))
        .one()
}

fn field(row: &EntityValue, name: &str) -> String {
    text_of(row.get(name))
}

/// The remote's HMAC header name and shared secret. Both are required: anyone can sign with an empty secret.
fn hmac_settings(remote: &EntityValue, remote_id: &str) -> Option<(String, String)> {
    let header_name = field(remote, "authHeaderName");
    let secret = field(remote, "sharedSecret");
    if header_name.is_empty() || secret.is_empty() {
        tracing::error!("System message receive HMAC for remote {remote_id} needs authHeaderName and sharedSecret");
        return None;
    }
    Some((header_name, secret))
}

fn hmac_bytes(secret: &str, message: &str) -> Vec<u8> {
    match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(mut mac) => {
            mac.update(message.as_bytes());
            mac.finalize().into_bytes().to_vec()
        }
        // HMAC takes a key of any length, so this is not reached, and an empty signature matches nothing.
        Err(_) => Vec::new(),
    }
}

fn hmac_base64(secret: &str, message: &str) -> String {
    STANDARD.encode(hmac_bytes(secret, message))
}

fn hmac_hex(secret: &str, message: &str) -> String {
    hex_encode(&hmac_bytes(secret, message))
}

/// The Stripe-style `t=<epochSeconds>,v1=<hex>` header of `SmatHmacSha256Timestamp`.
pub(super) fn timestamp_header(secret: &str, message: &str, epoch_seconds: i64) -> String {
    format!(
        "t={epoch_seconds},v1={}",
        hmac_hex(secret, &format!("{epoch_seconds}.{message}"))
    )
}

fn timestamp_parts(header: &str) -> (Option<String>, Option<String>) {
    let mut timestamp = None;
    let mut signature = None;
    for item in header.split(',') {
        let mut parts = item.split('=');
        let key = parts.next().unwrap_or("").trim();
        let value = parts.next().map(|value| value.trim().to_string());
        match key {
            "t" => timestamp = value,
            "v1" => signature = value,
            _ => {}
        }
    }
    (timestamp, signature)
}

/// Compare without stopping at the first difference, so the time does not show how much of a signature matched.
fn same_text(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// True the first time `key` is seen in the replay cache, which then holds it for the cache's live time.
///
/// A key must outlive the timestamp window, so a cache that expires entries sooner than [`MAX_AGE_MILLIS`] is an
/// error. Its `max-elements` can still evict a key early, but only after that many validly signed messages.
fn first_use(key: &str) -> Result<bool, Error> {
    let cache = ExecutionContext::with_current(|ctx| ctx.factory().cache().get(REPLAY_CACHE))
        .ok_or_else(|| Error::new("no execution context is entered"))??
        .ok_or_else(|| Error::new(format!("cache {REPLAY_CACHE} is not configured")))?;
    let info = cache.info();
    let short = |millis: i64| millis > 0 && millis < MAX_AGE_MILLIS;
    if short(info.expire_live_ms) || short(info.expire_idle_ms) {
        return Err(Error::new(format!(
            "cache {REPLAY_CACHE} expires entries before the {} second HMAC timestamp window",
            MAX_AGE_MILLIS / 1000
        )));
    }
    // One atomic check and store: two requests with the same signature cannot both see it unused.
    Ok(cache.put_if_absent(key.to_string(), Value::Bool(true))?)
}

#[cfg(test)]
pub(super) fn hmac_base64_for_test(secret: &str, message: &str) -> String {
    hmac_base64(secret, message)
}
