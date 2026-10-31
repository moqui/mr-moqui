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

//! Web Push to the browsers a user subscribed (the installed PWA or a browser tab; plans/Comms-Framework.md §7.2 P0), in
//! `org.moqui.impl.NotificationServices`:
//!
//! - `store#WebPushSubscription`, `delete#WebPushSubscription` and `get#WebPushStatus` work on the current user's
//!   subscriptions only. A browser's endpoint stored for another user moves to them, so a push never crosses an
//!   account switch; logout deletes the visit's subscriptions ([`forget_visit`]).
//! - [`queue`], from [`crate::notify::NotificationMessage::send`] once a persisted message's rows are written, runs
//!   `send#WebPush` on a service worker for the recipients who want push on the topic and have a subscription.
//! - `send#WebPush` encrypts a small payload (title, link, topic, the message id; never the message body) for each
//!   subscription and posts it to its push service with a VAPID token. An endpoint answered 404 or 410 is disabled; one
//!   that fails [`MAX_FAILURES`] times in a row too. Each endpoint fails on its own, logged by subscription id.
//!
//! Push is a hint: the notification list and counts are the server's, and the client reads them again when it
//! reconnects. Without the `web-push` feature the services answer that push is not supported.

use std::collections::BTreeMap;
#[cfg(feature = "web-push")]
use std::time::Duration;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use sha2::Digest;

use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};
use crate::notify::push_crypto;
use crate::transaction::TransactionAttribute;

const SUBSCRIPTION: &str = "moqui.security.user.WebPushSubscription";
const SEND_SERVICE: &str = "org.moqui.impl.NotificationServices.send#WebPush";
const KEY_PROPERTY: &str = "web_push_vapid_key";
const SUBJECT_PROPERTY: &str = "web_push_subject";
/// A subscription is disabled after this many failed pushes in a row.
pub(crate) const MAX_FAILURES: i64 = 5;
/// How long a push service keeps an undelivered message, at most (a call's ring asks for less).
const TTL_SECONDS: u32 = 86_400;
#[cfg(feature = "web-push")]
const TIMEOUT: Duration = Duration::from_secs(10);
/// The longest title and link put in a payload, in characters.
const MAX_TITLE: usize = 300;
const MAX_LINK: usize = 1000;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, store_subscription)?;
    rust_service!(map, delete_subscription)?;
    rust_service!(map, get_status)?;
    rust_service!(map, send_web_push)?;
    Ok(())
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name)
        .filter(|value| !value.is_null())
        .map(to_plain_string)
        .unwrap_or_default()
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn current_user() -> Option<String> {
    crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
}

fn endpoint_hash(endpoint: &str) -> String {
    sha2::Sha256::digest(endpoint.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn vapid() -> Result<push_crypto::Vapid, String> {
    let seed = moqui_lib::env::get_prop_or_env(KEY_PROPERTY);
    if seed.is_empty() || seed.ends_with("CHANGEME") {
        return Err(format!("the {KEY_PROPERTY} property is not set"));
    }
    push_crypto::Vapid::from_seed(&seed)
}

/// `get#WebPushStatus`: whether this host can push (`supported`), has its key (`configured`, with the `publicKey` a
/// browser subscribes with), and whether the current user's browser with `endpoint` is subscribed. Each is answered on
/// its own; a host without push answers supported false and no error.
fn get_status(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::from([
        ("supported".to_string(), Value::Bool(push_crypto::BUILT)),
        ("configured".to_string(), Value::Bool(false)),
        ("publicKey".to_string(), Value::string("")),
        ("subscribed".to_string(), Value::Bool(false)),
        ("subscriptionCount".to_string(), Value::I64(0)),
    ]);
    if !push_crypto::BUILT {
        return Ok(out);
    }
    match vapid() {
        Ok(key) => {
            out.insert("configured".to_string(), Value::Bool(true));
            out.insert("publicKey".to_string(), Value::string(key.public_key()));
        }
        Err(reason) => {
            out.insert("unavailableReason".to_string(), Value::string(reason));
        }
    }
    let Some(user_id) = current_user() else {
        return Ok(out);
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let active = || {
        facade.find(SUBSCRIPTION).map(|find| {
            find.eq("userId", Value::string(&user_id))
                .condition(moqui_db::sql::Condition::is_null("disabledDate"))
        })
    };
    out.insert("subscriptionCount".to_string(), Value::I64(active()?.count()?));
    if let Some(endpoint) = text(parameters, "endpoint") {
        let found = active()?
            .eq("endpointHash", Value::string(endpoint_hash(&endpoint)))
            .count()?;
        out.insert("subscribed".to_string(), Value::Bool(found > 0));
    }
    Ok(out)
}

/// `store#WebPushSubscription`: keep the current user's browser subscription (endpoint, p256dhKey, authSecret). The
/// same endpoint stored before, for this user or another, is replaced: it is now this user's, enabled, with the new
/// keys and visit.
fn store_subscription(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        add_error("Sign in to turn on notifications");
        return Ok(BTreeMap::new());
    };
    if !push_crypto::BUILT {
        add_error("Notifications to this device are not available on this server");
        return Ok(BTreeMap::new());
    }
    let (Some(endpoint), Some(p256dh), Some(auth)) = (
        text(parameters, "endpoint"),
        text(parameters, "p256dhKey"),
        text(parameters, "authSecret"),
    ) else {
        add_error("endpoint, p256dhKey and authSecret are required");
        return Ok(BTreeMap::new());
    };
    if let Err(reason) = push_crypto::origin(&endpoint).and_then(|_| push_crypto::check_keys(&p256dh, &auth)) {
        add_error(format!("The browser's subscription is not valid: {reason}"));
        return Ok(BTreeMap::new());
    }
    let hash = endpoint_hash(&endpoint);
    let user_agent: String = text(parameters, "userAgent")
        .unwrap_or_default()
        .chars()
        .take(250)
        .collect();
    let visit_id = crate::user::visit_id().ok().flatten().unwrap_or_default();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let existing = facade
        .find(SUBSCRIPTION)?
        .eq("endpointHash", Value::string(&hash))
        .one()?;
    let mut row = match &existing {
        Some(row) => row.clone(),
        None => {
            let mut row = facade.make_value(SUBSCRIPTION)?;
            row.set_sequenced_id_primary(facade)?;
            row.set("createdDate", crate::user::now_value());
            row
        }
    };
    if let Some(previous) = existing
        .as_ref()
        .map(|row| field(row, "userId"))
        .filter(|id| id != &user_id)
    {
        tracing::info!(
            subscription_id = %field(&row, "subscriptionId"),
            "Web Push subscription moved from user {previous} to the user now signed in"
        );
    }
    row.set("userId", Value::string(&user_id));
    row.set("endpoint", Value::string(&endpoint));
    row.set("endpointHash", Value::string(&hash));
    row.set("p256dhKey", Value::string(&p256dh));
    row.set("authSecret", Value::string(&auth));
    row.set("userAgent", Value::string(&user_agent));
    row.set(
        "visitId",
        if visit_id.is_empty() {
            Value::Null
        } else {
            Value::string(&visit_id)
        },
    );
    row.set("failureCount", Value::I64(0));
    row.set("disabledDate", Value::Null);
    let subscription_id = field(&row, "subscriptionId");
    if existing.is_some() {
        row.update(facade)?;
    } else {
        row.create(facade)?;
    }
    Ok(BTreeMap::from([(
        "subscriptionId".to_string(),
        Value::string(subscription_id),
    )]))
}

/// `delete#WebPushSubscription`: forget the current user's subscription with `endpoint`. An endpoint that is not the
/// current user's is left alone, with no error, so the answer tells nothing about other users' browsers.
fn delete_subscription(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let (Some(user_id), Some(endpoint)) = (current_user(), text(parameters, "endpoint")) else {
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find(SUBSCRIPTION)?
        .eq("endpointHash", Value::string(endpoint_hash(&endpoint)))
        .eq("userId", Value::string(&user_id))
        .delete_all()?;
    Ok(BTreeMap::new())
}

/// Delete the subscriptions the visit stored, at logout. A failure is logged: the browser's own unsubscribe and the
/// move on the next sign-in still keep pushes to the right user.
pub(crate) fn forget_visit(facade: &EntityFacade, user_id: &str, visit_id: &str) {
    if user_id.is_empty() || visit_id.is_empty() || !defined(facade) {
        return;
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let deleted = facade.transactions_for(SUBSCRIPTION).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            facade
                .find(SUBSCRIPTION)?
                .eq("userId", Value::string(user_id))
                .eq("visitId", Value::string(visit_id))
                .delete_all()
        })
    });
    if let Err(err) = deleted {
        tracing::warn!(error = %err, "the visit's Web Push subscriptions were not deleted at logout");
    }
}

/// The subscription entity is in the model. A model without it (a test facade of a few entity files) sends no push,
/// and the caller's transaction sees no failed query.
fn defined(facade: &EntityFacade) -> bool {
    crate::entity::info::is_entity_defined(facade, SUBSCRIPTION)
}

/// The payload a push carries: the title, link and topic, and the message id as the tag that replaces an earlier
/// notice of the same message. The message body stays on the server.
pub(crate) struct PushNotice<'a> {
    pub title: &'a str,
    pub link: &'a str,
    pub topic: &'a str,
    pub tag: &'a str,
}

/// Run `send#WebPush` on a service worker for those of `user_ids` with an active subscription. Logged, never an error
/// for the caller: the notification is stored and on the socket already.
pub(crate) fn queue(facade: &EntityFacade, user_ids: &[String], notice: &PushNotice<'_>) {
    if !push_crypto::BUILT || user_ids.is_empty() || !defined(facade) {
        return;
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let ids: Vec<Value> = user_ids.iter().map(Value::string).collect();
    // in a transaction of its own: a failed read must not mark the sender's for rollback
    let subscribed = facade.transactions_for(SUBSCRIPTION).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            facade
                .find(SUBSCRIPTION)?
                .condition(moqui_db::sql::Condition::is_in("userId", ids.clone()))
                .condition(moqui_db::sql::Condition::is_null("disabledDate"))
                .select_field("userId")
                .distinct(true)
                .list()
        })
    });
    let users: Vec<Value> = match subscribed {
        Ok(rows) => rows.iter().map(|row| Value::string(field(row, "userId"))).collect(),
        Err(err) => {
            tracing::warn!(error = %err, "Web Push subscriptions could not be read; no push sent");
            return;
        }
    };
    if users.is_empty() {
        return;
    }
    let parameters = BTreeMap::from([
        ("userIdList".to_string(), Value::List(users)),
        ("title".to_string(), Value::string(notice.title)),
        ("link".to_string(), Value::string(notice.link)),
        ("topic".to_string(), Value::string(notice.topic)),
        ("tag".to_string(), Value::string(notice.tag)),
    ]);
    if let Err(err) = crate::service::call::async_call(SEND_SERVICE, &parameters) {
        tracing::warn!(error = %err, "send#WebPush was not started");
    }
}

/// What a push service's answer means for the subscription.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Delivered,
    /// 404 or 410: the subscription is gone; never send to it again.
    Gone,
    /// Any other answer, or no answer: count a failure.
    Failed,
}

pub(crate) fn outcome(status: Option<u16>) -> Outcome {
    match status {
        Some(200..=299) => Outcome::Delivered,
        Some(404 | 410) => Outcome::Gone,
        _ => Outcome::Failed,
    }
}

fn truncated(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// `send#WebPush`: push the notice to every active subscription of `userIdList`. Internal: run by [`queue`].
fn send_web_push(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut sent = 0_i64;
    let mut failed = 0_i64;
    let summary = |sent: i64, failed: i64| {
        BTreeMap::from([
            ("sentCount".to_string(), Value::I64(sent)),
            ("failedCount".to_string(), Value::I64(failed)),
        ])
    };
    let key = match vapid() {
        Ok(key) => key,
        Err(reason) => {
            tracing::warn!("Web Push not sent: {reason}");
            return Ok(summary(0, 0));
        }
    };
    let users = super::shell::user_ids(parameters.get("userIdList"));
    if users.is_empty() {
        return Ok(summary(0, 0));
    }
    let subject = Some(moqui_lib::env::get_prop_or_env(SUBJECT_PROPERTY))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "mailto:admin@localhost".to_string());
    let payload = serde_json::json!({
        "title": truncated(&text(parameters, "title").unwrap_or_default(), MAX_TITLE),
        "link": truncated(&text(parameters, "link").unwrap_or_default(), MAX_LINK),
        "topic": text(parameters, "topic").unwrap_or_default(),
        "tag": text(parameters, "tag").unwrap_or_default(),
    })
    .to_string();
    // A call's ring is worth nothing once the call stops ringing: it expires at the push service (§7.2).
    let ttl = parameters
        .get("ttlSeconds")
        .and_then(|v| to_plain_string(v).parse::<u32>().ok())
        .filter(|t| *t > 0)
        .map_or(TTL_SECONDS, |t| t.min(TTL_SECONDS));
    let urgency = if text(parameters, "urgency").as_deref() == Some("high") {
        "high"
    } else {
        "normal"
    };
    let delivery = Delivery { ttl, urgency };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let rows = facade
        .find(SUBSCRIPTION)?
        .condition(moqui_db::sql::Condition::is_in(
            "userId",
            users.iter().map(Value::string).collect(),
        ))
        .condition(moqui_db::sql::Condition::is_null("disabledDate"))
        .list()?;
    let now_secs = u64::try_from(jiff::Timestamp::now().as_second()).unwrap_or(0);
    for row in rows.iter() {
        let id = field(row, "subscriptionId");
        let endpoint = field(row, "endpoint");
        let status = push_crypto::encrypt(payload.as_bytes(), &field(row, "p256dhKey"), &field(row, "authSecret"))
            .and_then(|body| Ok((body, key.authorization(&endpoint, &subject, now_secs)?)))
            .and_then(|(body, authorization)| post(&endpoint, body, &authorization, &delivery));
        let result = match status {
            Ok(status) => outcome(Some(status)),
            Err(reason) => {
                tracing::warn!(subscription_id = %id, "Web Push failed: {reason}");
                Outcome::Failed
            }
        };
        if result == Outcome::Delivered {
            sent += 1;
        } else {
            failed += 1;
        }
        if let Err(err) = record(facade, row, &result) {
            tracing::warn!(subscription_id = %id, error = %err, "Web Push result was not stored");
        }
    }
    Ok(summary(sent, failed))
}

fn record(facade: &EntityFacade, row: &EntityValue, result: &Outcome) -> Result<(), LibError> {
    let mut row = row.clone();
    let now = crate::user::now_value();
    match result {
        Outcome::Delivered => {
            row.set("lastSuccessDate", now);
            row.set("failureCount", Value::I64(0));
        }
        Outcome::Gone => {
            tracing::info!(subscription_id = %field(&row, "subscriptionId"), "Web Push subscription expired; disabled");
            row.set("lastFailureDate", now.clone());
            row.set("disabledDate", now);
        }
        Outcome::Failed => {
            let failures = field(&row, "failureCount").parse::<i64>().unwrap_or(0) + 1;
            row.set("lastFailureDate", now.clone());
            row.set("failureCount", Value::I64(failures));
            if failures >= MAX_FAILURES {
                tracing::info!(
                    subscription_id = %field(&row, "subscriptionId"),
                    "Web Push subscription failed {failures} times; disabled"
                );
                row.set("disabledDate", now);
            }
        }
    }
    row.update(facade)?;
    Ok(())
}

/// How long the push service keeps a push for an offline device, and its urgency (RFC 8030 §5.2, §5.3).
#[cfg_attr(not(feature = "web-push"), allow(dead_code))]
struct Delivery {
    ttl: u32,
    urgency: &'static str,
}

/// POST the encrypted body; the push service's status. A push service is a public host: internal addresses are
/// refused, as for any URL a browser hands the server.
#[cfg(feature = "web-push")]
fn post(endpoint: &str, body: Vec<u8>, authorization: &str, delivery: &Delivery) -> Result<u16, String> {
    use std::sync::OnceLock;
    crate::entity::fence::check_effect(crate::entity::fence::Effect::RemoteCall, "a Web Push")
        .map_err(|err| err.message().to_string())?;
    static CLIENT: OnceLock<moqui_lib::http::RestClient> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        moqui_lib::http::RestClient::new()
            .deny_internal_addresses(true)
            .timeout(TIMEOUT)
            .redirects(0, false)
    });
    let response = client
        .open(moqui_lib::http::OpenRequest {
            method: moqui_lib::http::Method::Post,
            url: endpoint.to_string(),
            headers: vec![
                ("content-type".to_string(), "application/octet-stream".to_string()),
                ("content-encoding".to_string(), "aes128gcm".to_string()),
                ("ttl".to_string(), delivery.ttl.to_string()),
                ("urgency".to_string(), delivery.urgency.to_string()),
                ("authorization".to_string(), authorization.to_string()),
            ],
            body: Some(bytes::Bytes::from(body)),
            timeout: Some(TIMEOUT),
        })
        // the endpoint is a capability: the log names its push service only
        .map_err(|err| {
            let service = push_crypto::origin(endpoint).unwrap_or_default();
            let cause = std::error::Error::source(&err)
                .map(ToString::to_string)
                .unwrap_or_default();
            format!("the push service {service} did not answer: {cause}")
        })?;
    Ok(response.status)
}

#[cfg(not(feature = "web-push"))]
fn post(_endpoint: &str, _body: Vec<u8>, _authorization: &str, _delivery: &Delivery) -> Result<u16, String> {
    Err("Web Push is not built into this host".to_string())
}

#[cfg(test)]
#[path = "web_push_tests.rs"]
mod tests;
