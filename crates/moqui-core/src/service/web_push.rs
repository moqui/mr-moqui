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
//! - `register#PushDevice` and `unregister#PushDevice` keep a native app's FCM registration (plans/Mobile-Desktop.md §5,
//!   "Native push") in the same entity, as provider `PushFcm`, tied to the device key the request signed in with: the
//!   key's revoke deletes it ([`forget_device_key`]), its renewal moves it ([`move_device_key`]), and one whose key has
//!   ended is deleted at the next push. `send#WebPush` sends to these through [`super::fcm`], with a call's ring as a
//!   high-priority data message.
//!
//! Push is a hint: the notification list and counts are the server's, and the client reads them again when it
//! reconnects. Without the `web-push` feature the Web Push services answer that push is not supported; FCM needs only
//! HTTP.

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
const USER_LOGIN_KEY: &str = "moqui.security.UserLoginKey";
/// `pushProviderEnumId`: a browser's Web Push subscription, or a native app's FCM registration.
const PROVIDER_WEB: &str = "PushWeb";
const PROVIDER_FCM: &str = "PushFcm";
/// The longest FCM token and app id stored, in characters (a token is about 160).
const MAX_TOKEN: usize = 4096;
const MAX_APP_ID: usize = 255;
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
    rust_service!(map, register_device)?;
    rust_service!(map, unregister_device)?;
    rust_service!(map, get_device_status)?;
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

/// A browser's subscription: `PushWeb`, or no provider (the entity's default).
fn web_provider() -> moqui_db::sql::Condition {
    moqui_db::sql::Condition::or(vec![
        moqui_db::sql::Condition::equals("pushProviderEnumId", Value::string(PROVIDER_WEB)),
        moqui_db::sql::Condition::is_null("pushProviderEnumId"),
    ])
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
                .condition(web_provider())
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
    row.set("pushProviderEnumId", Value::string(PROVIDER_WEB));
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
        .condition(web_provider())
        .delete_all()?;
    Ok(BTreeMap::new())
}

/// The stored hash of the device key this request signed in with (its `login_key` header), when that is an unexpired
/// device key (one with a `deviceName`) of `user_id`. Authorization is the caller's to turn off.
fn request_device_key(facade: &EntityFacade, user_id: &str) -> Result<Option<String>, LibError> {
    let Some(hashed) = crate::web::current_request()
        .and_then(|request| request.header("login_key").map(str::to_string))
        .filter(|key| !key.is_empty())
        .and_then(|key| crate::user::hash_login_key(&key).ok())
    else {
        return Ok(None);
    };
    let Some(row) = facade
        .find(USER_LOGIN_KEY)?
        .eq("loginKey", Value::string(&hashed))
        .one()?
    else {
        return Ok(None);
    };
    let live = row
        .get("thruDate")
        .and_then(crate::user::value_millis)
        .is_none_or(|thru| thru > crate::user::now_millis());
    Ok((field(&row, "userId") == user_id && !field(&row, "deviceName").is_empty() && live).then_some(hashed))
}

/// `register#PushDevice`: keep the FCM registration `token` of the native app this request comes from, for the current
/// user, tied to the device key the request signed in with. A token stored before, for this user or another, becomes
/// this registration (enabled again); an older token of the same device key is replaced. Refused, with an error, for a
/// request not signed in by a device key of the user's, so a registration always ends with its device's key.
fn register_device(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        add_error("Sign in to turn on notifications");
        return Ok(BTreeMap::new());
    };
    let provider = text(parameters, "provider").unwrap_or_default().to_ascii_lowercase();
    if provider != "fcm" && provider != "pushfcm" {
        add_error("provider must be fcm");
        return Ok(BTreeMap::new());
    }
    let Some(token) = text(parameters, "token")
        .filter(|token| token.chars().count() <= MAX_TOKEN && !token.chars().any(char::is_whitespace))
    else {
        add_error("token is required, with no spaces");
        return Ok(BTreeMap::new());
    };
    let app_id: String = text(parameters, "appId")
        .unwrap_or_default()
        .chars()
        .take(MAX_APP_ID)
        .collect();
    let platform: String = text(parameters, "platform")
        .unwrap_or_else(|| "android".to_string())
        .to_ascii_lowercase()
        .chars()
        .take(20)
        .collect();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(login_key) = request_device_key(facade, &user_id)? else {
        add_error("Only an app signed in with its device key can register for push");
        return Ok(BTreeMap::new());
    };
    let hash = endpoint_hash(&token);
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
    if let Some(previous) = existing.as_ref().filter(|row| field(row, "userId") != user_id) {
        // One app holds one token. Another account signed in on the same app takes it over (an account switch that
        // did not unregister); whether the earlier account's key still works is logged, as a token seen under two live
        // keys at once means something other than a switch.
        let earlier_key = field(previous, "loginKey");
        let earlier_live = !earlier_key.is_empty()
            && facade
                .find(USER_LOGIN_KEY)?
                .eq("loginKey", Value::string(&earlier_key))
                .one()?
                .is_some_and(|key| {
                    key.get("thruDate")
                        .and_then(crate::user::value_millis)
                        .is_none_or(|thru| thru > crate::user::now_millis())
                });
        let subscription_id = field(&row, "subscriptionId");
        let earlier = field(previous, "userId");
        if earlier_live {
            tracing::warn!(
                subscription_id = %subscription_id,
                "FCM token moved from user {earlier}, whose device key is still good, to the user now signed in"
            );
        } else {
            tracing::info!(subscription_id = %subscription_id, "FCM registration moved from user {earlier} to the user now signed in");
        }
    }
    let subscription_id = field(&row, "subscriptionId");
    // one registration a device key: the app's token changed
    facade
        .find(SUBSCRIPTION)?
        .eq("loginKey", Value::string(&login_key))
        .condition(moqui_db::sql::Condition::not_equals(
            "subscriptionId",
            Value::string(&subscription_id),
        ))
        .delete_all()?;
    row.set("userId", Value::string(&user_id));
    row.set("pushProviderEnumId", Value::string(PROVIDER_FCM));
    row.set("endpoint", Value::string(&token));
    row.set("endpointHash", Value::string(&hash));
    row.set("loginKey", Value::string(&login_key));
    row.set("appId", Value::string(&app_id));
    row.set("platform", Value::string(&platform));
    for unused in ["p256dhKey", "authSecret", "userAgent", "visitId"] {
        row.set(unused, Value::Null);
    }
    row.set("failureCount", Value::I64(0));
    row.set("disabledDate", Value::Null);
    if existing.is_some() {
        row.update(facade)?;
        return Ok(BTreeMap::from([(
            "subscriptionId".to_string(),
            Value::string(subscription_id),
        )]));
    }
    // A new row in a transaction of its own: two registrations of one token at once (an app retrying) meet at the
    // unique endpointHash, and the one that loses updates the row the other made instead of failing the request.
    let created = facade
        .transactions_for(SUBSCRIPTION)?
        .run(TransactionAttribute::ForceNew, None, || row.create(facade));
    let subscription_id = match created {
        Ok(()) => subscription_id,
        Err(err) => {
            let Some(mut winner) = facade
                .find(SUBSCRIPTION)?
                .eq("endpointHash", Value::string(&hash))
                .one()?
            else {
                return Err(err);
            };
            for name in [
                "userId",
                "pushProviderEnumId",
                "endpoint",
                "loginKey",
                "appId",
                "platform",
                "failureCount",
                "disabledDate",
            ] {
                winner.set(name, row.get(name).cloned().unwrap_or(Value::Null));
            }
            winner.update(facade)?;
            field(&winner, "subscriptionId")
        }
    };
    Ok(BTreeMap::from([(
        "subscriptionId".to_string(),
        Value::string(subscription_id),
    )]))
}

/// `unregister#PushDevice`: forget the current user's FCM registration with `token`, or with no token every one of the
/// device key this request signed in with (the app turned notifications off, or is signing out). Another user's token
/// is left as it is, with no error.
fn unregister_device(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let find = facade
        .find(SUBSCRIPTION)?
        .eq("userId", Value::string(&user_id))
        .eq("pushProviderEnumId", Value::string(PROVIDER_FCM));
    let deleted = if let Some(token) = text(parameters, "token") {
        find.eq("endpointHash", Value::string(endpoint_hash(&token)))
            .delete_all()?
    } else if let Some(login_key) = request_device_key(facade, &user_id)? {
        find.eq("loginKey", Value::string(login_key)).delete_all()?
    } else {
        0
    };
    Ok(BTreeMap::from([("deletedCount".to_string(), Value::I64(deleted))]))
}

/// `get#PushDeviceStatus`: whether this server sends FCM (`fcmConfigured`: its service account is set) and whether the
/// device key of this request has an enabled registration (`registered`). No error.
fn get_device_status(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::from([
        ("fcmConfigured".to_string(), Value::Bool(super::fcm::configured())),
        ("registered".to_string(), Value::Bool(false)),
    ]);
    let Some(user_id) = current_user() else {
        return Ok(out);
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    if let Some(login_key) = request_device_key(facade, &user_id)? {
        let found = facade
            .find(SUBSCRIPTION)?
            .eq("loginKey", Value::string(login_key))
            .condition(moqui_db::sql::Condition::is_null("disabledDate"))
            .count()?;
        out.insert("registered".to_string(), Value::Bool(found > 0));
    }
    Ok(out)
}

/// Delete the registrations of the device key `hashed_key` (the stored hash), as the key is revoked. In a transaction
/// of its own and logged: it commits even if the revoke's own transaction later rolls back, which costs only a
/// re-registration on the device's next start, and when it fails the next push deletes them anyway, as their key has
/// ended.
pub(crate) fn forget_device_key(facade: &EntityFacade, hashed_key: &str) {
    if hashed_key.is_empty() || !defined(facade) {
        return;
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let deleted = facade.transactions_for(SUBSCRIPTION).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            facade
                .find(SUBSCRIPTION)?
                .eq("loginKey", Value::string(hashed_key))
                .delete_all()
        })
    });
    if let Err(err) = deleted {
        tracing::warn!(error = %err, "the push registrations of a revoked device key were not deleted");
    }
}

/// Move the registrations of the device key `old_key` to its renewal `new_key` (both stored hashes), so the device keeps
/// its push past the old key's end. In a transaction of its own, after the new key is made in one too (so the key it
/// points to exists); if the renewal's transaction then rolls back, the registration points at a key the device never
/// got and is deleted at a push once the old key ends. Logged: the app registers again with its key when it next
/// starts.
pub(crate) fn move_device_key(facade: &EntityFacade, old_key: &str, new_key: &str) {
    if old_key.is_empty() || new_key.is_empty() || !defined(facade) {
        return;
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let moved = facade.transactions_for(SUBSCRIPTION).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            facade
                .find(SUBSCRIPTION)?
                .eq("loginKey", Value::string(old_key))
                .update_all([("loginKey".to_string(), Value::string(new_key))])
        })
    });
    if let Err(err) = moved {
        tracing::warn!(error = %err, "the push registrations of a renewed device key were not moved");
    }
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

/// Run `send#WebPush` on a service worker for those of `user_ids` with an active subscription or FCM registration.
/// Logged, never an error for the caller: the notification is stored and on the socket already.
pub(crate) fn queue(facade: &EntityFacade, user_ids: &[String], notice: &PushNotice<'_>) {
    if user_ids.is_empty() || !defined(facade) {
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

/// What one send counted.
#[derive(Default)]
struct Tally {
    sent: i64,
    failed: i64,
}

impl Tally {
    fn count(&mut self, delivered: bool) {
        if delivered {
            self.sent += 1;
        } else {
            self.failed += 1;
        }
    }
}

/// The push of one `send#WebPush` call, from its parameters: the Web Push payload and its delivery, and the FCM notice.
struct Push {
    payload: String,
    delivery: Delivery,
    notice: super::fcm::Notice,
}

fn push_of(parameters: &BTreeMap<String, Value>) -> Push {
    let title = truncated(&text(parameters, "title").unwrap_or_default(), MAX_TITLE);
    let link = truncated(&text(parameters, "link").unwrap_or_default(), MAX_LINK);
    let topic = truncated(&text(parameters, "topic").unwrap_or_default(), MAX_TITLE);
    let tag = truncated(&text(parameters, "tag").unwrap_or_default(), MAX_TITLE);
    let payload = serde_json::json!({"title": title, "link": link, "topic": topic, "tag": tag}).to_string();
    // A call's ring is worth nothing once the call stops ringing: it expires at the push service (§7.2).
    let ttl = parameters
        .get("ttlSeconds")
        .and_then(|v| to_plain_string(v).parse::<u32>().ok())
        .filter(|t| *t > 0)
        .map_or(TTL_SECONDS, |t| t.min(TTL_SECONDS));
    let high = text(parameters, "urgency").as_deref() == Some("high");
    let kind = match (text(parameters, "kind").as_deref(), text(parameters, "callSessionId")) {
        (Some("ringEnded"), Some(call_session_id)) => super::fcm::Kind::RingEnded {
            call_session_id,
            reason: text(parameters, "reason").unwrap_or_default(),
        },
        (_, Some(call_session_id)) => super::fcm::Kind::Ring(super::fcm::Ring {
            call_session_id,
            chat_room_id: text(parameters, "chatRoomId").unwrap_or_default(),
            caller: truncated(&text(parameters, "fromName").unwrap_or_default(), MAX_TITLE),
            video: text(parameters, "video").is_some_and(|video| video == "true" || video == "Y"),
            expires_at: text(parameters, "expiresAt")
                .and_then(|at| at.parse::<i64>().ok())
                .unwrap_or(0),
        }),
        _ => super::fcm::Kind::Notification,
    };
    Push {
        payload,
        delivery: Delivery {
            ttl,
            urgency: if high { "high" } else { "normal" },
        },
        notice: super::fcm::Notice {
            title,
            link,
            topic,
            tag,
            kind,
            high,
            ttl,
        },
    }
}

/// `send#WebPush`: push the notice to every active subscription and FCM registration of `userIdList`. Internal: run by
/// [`queue`] and by a call's ring. Each provider and each device fails on its own: a server without its VAPID key still
/// sends FCM, and the other way around.
fn send_web_push(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut tally = Tally::default();
    let summary = |tally: &Tally| {
        BTreeMap::from([
            ("sentCount".to_string(), Value::I64(tally.sent)),
            ("failedCount".to_string(), Value::I64(tally.failed)),
        ])
    };
    let users = super::shell::user_ids(parameters.get("userIdList"));
    if users.is_empty() {
        return Ok(summary(&tally));
    }
    let push = push_of(parameters);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let rows = facade
        .find(SUBSCRIPTION)?
        .condition(moqui_db::sql::Condition::is_in(
            "userId",
            users.iter().map(Value::string).collect(),
        ))
        .condition(moqui_db::sql::Condition::is_null("disabledDate"))
        .list()?;
    let (native, browsers): (Vec<&EntityValue>, Vec<&EntityValue>) = rows
        .iter()
        .partition(|row| field(row, "pushProviderEnumId") == PROVIDER_FCM);
    // a ring's end is for an app's ring screen; a browser's notification of the ring is replaced by the call's state
    let ring_ended = matches!(push.notice.kind, super::fcm::Kind::RingEnded { .. });
    if push_crypto::BUILT && !ring_ended && !browsers.is_empty() {
        send_browsers(facade, &browsers, &push, &mut tally);
    }
    if !native.is_empty() {
        send_native(facade, &native, &push.notice, &mut tally);
    }
    Ok(summary(&tally))
}

/// Web Push to each browser, encrypted to it with the server's VAPID token.
fn send_browsers(facade: &EntityFacade, rows: &[&EntityValue], push: &Push, tally: &mut Tally) {
    let key = match vapid() {
        Ok(key) => key,
        Err(reason) => {
            tracing::warn!("Web Push not sent: {reason}");
            return;
        }
    };
    let subject = Some(moqui_lib::env::get_prop_or_env(SUBJECT_PROPERTY))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "mailto:admin@localhost".to_string());
    let now_secs = u64::try_from(jiff::Timestamp::now().as_second()).unwrap_or(0);
    for row in rows {
        let id = field(row, "subscriptionId");
        let endpoint = field(row, "endpoint");
        let status = push_crypto::encrypt(
            push.payload.as_bytes(),
            &field(row, "p256dhKey"),
            &field(row, "authSecret"),
        )
        .and_then(|body| Ok((body, key.authorization(&endpoint, &subject, now_secs)?)))
        .and_then(|(body, authorization)| post(&endpoint, body, &authorization, &push.delivery));
        let result = match status {
            Ok(status) => outcome(Some(status)),
            Err(reason) => {
                tracing::warn!(subscription_id = %id, "Web Push failed: {reason}");
                Outcome::Failed
            }
        };
        tally.count(result == Outcome::Delivered);
        if let Err(err) = record(facade, row, &result) {
            tracing::warn!(subscription_id = %id, error = %err, "Web Push result was not stored");
        }
    }
}

/// FCM to each native app whose device key is still good; a registration whose key has ended (revoked, expired, or
/// deleted) is deleted instead. Without `fcm_service_account` nothing is sent, with one info line in the log.
fn send_native(facade: &EntityFacade, rows: &[&EntityValue], notice: &super::fcm::Notice, tally: &mut Tally) {
    let keys: Vec<Value> = rows.iter().map(|row| Value::string(field(row, "loginKey"))).collect();
    let now = crate::user::now_millis();
    let live: std::collections::HashSet<String> = match facade
        .find(USER_LOGIN_KEY)
        .and_then(|find| find.condition(moqui_db::sql::Condition::is_in("loginKey", keys)).list())
    {
        Ok(found) => found
            .iter()
            .filter(|key| {
                key.get("thruDate")
                    .and_then(crate::user::value_millis)
                    .is_none_or(|thru| thru > now)
            })
            .map(|key| field(key, "loginKey"))
            .collect(),
        Err(err) => {
            tracing::warn!(error = %err, "device keys of the FCM registrations not read; no FCM push sent");
            return;
        }
    };
    let (rows, ended): (Vec<&EntityValue>, Vec<&EntityValue>) =
        rows.iter().partition(|row| live.contains(&field(row, "loginKey")));
    for row in ended {
        tracing::info!(subscription_id = %field(row, "subscriptionId"), "FCM registration's device key has ended; deleted");
        if let Err(err) = row.delete(facade) {
            tracing::warn!(subscription_id = %field(row, "subscriptionId"), error = %err, "FCM registration not deleted");
        }
    }
    if rows.is_empty() {
        return;
    }
    let config = match super::fcm::config() {
        Ok(Some(config)) => config,
        Ok(None) => {
            super::fcm::note_unset();
            return;
        }
        Err(reason) => {
            tracing::warn!("FCM push not sent: {reason}");
            return;
        }
    };
    let sender = super::fcm::sender(&config);
    let now_secs = jiff::Timestamp::now().as_second();
    for (index, row) in rows.iter().enumerate() {
        let id = field(row, "subscriptionId");
        let result = match sender.deliver_notice(&id, &field(row, "endpoint"), notice, now_secs) {
            super::fcm::Outcome::Delivered => Outcome::Delivered,
            super::fcm::Outcome::Gone => Outcome::Gone,
            // the message's fault, logged as an error: the device is left as it is
            super::fcm::Outcome::Rejected => {
                tally.count(false);
                continue;
            }
            // FCM or Google not taking messages (no access token, credentials refused, 429 or 5xx after the tries, no
            // answer), already logged: it would be the same for the rest, so they are not tried, and no device is
            // counted against.
            super::fcm::Outcome::Unsent => {
                tally.failed += i64::try_from(rows.len() - index).unwrap_or(i64::MAX);
                break;
            }
        };
        tally.count(result == Outcome::Delivered);
        if let Err(err) = record(facade, row, &result) {
            tracing::warn!(subscription_id = %id, error = %err, "FCM result was not stored");
        }
    }
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
            tracing::info!(subscription_id = %field(&row, "subscriptionId"), "push subscription expired (gone at its push service); disabled");
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
                    "push subscription failed {failures} times; disabled"
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
