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

//! Shared devices (docs/security/Shared-Devices.md; mr-boss plan/design-crosscut.md CX-DEVICE): a browser registered
//! to open one route group as a display account, with no person signed in. No Java counterpart.
//!
//! - **The credential** is the `moqui.device` cookie, `<deviceId>.<secret>`: HttpOnly, SameSite=Strict, Secure on
//!   https. Only the secret's hash is kept (`DeviceCredential.credentialHash`). It is made when the registration code is
//!   redeemed ([`redeem`]): the code is shown once to the registrar, kept only as its hash, short-lived and good once.
//! - **A device's request** ([`begin_request`]) is checked against the stored row every time, on whichever server takes
//!   it: revoked, stopped, expired, idle too long, an address outside `ipAllowed`, a display account that is gone,
//!   closed, no longer a display account, or holding what a device may not (a privileged group, a sealed permission).
//!   Nothing is cached, so a revocation holds from the next request everywhere.
//! - **The identity is the request's, never the session's**: the display account is the context's user while the
//!   request runs ([`super::session::enter_device`]) and is never written to the web session, so no session row, live
//!   session, notification socket or other server ever holds a signed-in display account. There is no session to
//!   steal, list or end beside the credential.
//! - **The route group** is an allow-list ([`check_route`]): a device's request is refused unless its target screen or
//!   transition is a member of the route group's artifact group. What is admitted is still authorized as the display
//!   account. Header keys, Basic credentials and login parameters are not read on a device's request, and no login
//!   succeeds in one.
//! - **One browser**: the cookie is a bearer secret, which nothing binds to hardware. It is not readable by script,
//!   is sent only same-site, and is replaced every day ([`ROTATE_MILLIS`]); the secret before it works ten more minutes
//!   ([`ROTATE_GRACE_MILLIS`]) and then stops the device (`stopReason` `copied`), since a second holder is the only
//!   one left presenting it. A device may also be held to an address pattern (`ipAllowed`).

use std::sync::Arc;

use moqui_db::sql::Condition;
use moqui_lib::object::Value;

use super::login::{entity_facade, now_millis, stamp_value, value_millis};
use super::session;
use crate::context::{Error, ErrorKind, ExecutionContext};
use crate::entity::{EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;
use crate::web::WebRequest;

/// The cookie of a registered device.
pub const DEVICE_COOKIE_NAME: &str = "moqui.device";
/// The public page a device's browser enters its registration code on, the one screen outside its route group a
/// device's request may open (it says what the browser is registered as).
pub const DEVICE_SCREEN: &str = "component://webroot/screen/webroot/Device.xml";

const DEVICE: &str = "moqui.security.DeviceCredential";
const ROUTE_GROUP: &str = "moqui.security.DeviceRouteGroup";
const GROUP_MEMBER: &str = "moqui.security.ArtifactGroupMember";
const USER_ACCOUNT: &str = "moqui.security.UserAccount";
const AUDIT_LOG: &str = "moqui.entity.EntityAuditLog";

/// How long a device's secret lasts before the next request replaces it.
pub const ROTATE_MILLIS: i64 = 24 * 60 * 60 * 1000;
/// How long the secret before the current one still works: requests under way when it was replaced, and a browser
/// that has not stored the new cookie yet.
pub const ROTATE_GRACE_MILLIS: i64 = 10 * 60 * 1000;
/// The idle timeout of a device that names none: a week.
pub const DEFAULT_IDLE_MINUTES: i64 = 7 * 24 * 60;
const MINUTE: i64 = 60_000;
const SECRET_LENGTH: usize = 43;

/// What a device's request may open, and as what.
#[derive(Debug)]
pub struct DeviceScope {
    pub device_id: String,
    pub route_group_id: String,
    pub artifact_group_id: String,
    /// The request parameter the registration fixed (a wall display's playlist), when the route group names one.
    pub parameter: Option<(String, String)>,
}

/// Why a device's request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No such device, or not its secret.
    Unknown,
    Revoked,
    /// Stopped for good: expired, idle too long, or a copy of its cookie in use.
    Stopped,
    /// From an address the device is not allowed from.
    Address,
    /// The display account or the route group no longer serves a device.
    Account,
}

impl Refusal {
    /// The refused browser no longer has a use for its cookie.
    pub fn ends_cookie(self) -> bool {
        !matches!(self, Self::Address | Self::Account)
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Unknown => "This browser is not a registered device",
            Self::Revoked => "This device's registration was revoked",
            Self::Stopped => "This device's registration has ended; register it again",
            Self::Address => "This device is not allowed from this address",
            Self::Account => "This device's display account is not available",
        }
    }

    /// What the plain page of a refused device's browser says ([`refusal_page`]): not registered, revoked, ended or
    /// refused, and nothing else.
    pub fn page_text(self) -> &'static str {
        match self {
            Self::Unknown => "This display is not registered.",
            Self::Revoked => "This display's registration was revoked.",
            Self::Stopped => "This display's registration has ended. Register it again to use it.",
            Self::Address | Self::Account => "This display is refused for now.",
        }
    }
}

/// What the plain page says to a device's browser that asked for something outside its route group, or whose request
/// failed: no error text, which could name the instance's records, paths or setup.
pub const OUTSIDE_PAGE_TEXT: &str = "This page is not part of this display.";
/// The plain page of a device's request that failed for another reason.
pub const FAILED_PAGE_TEXT: &str = "This display cannot show this page now.";

/// The page a shared device's browser gets for a refusal it can show (a wall display has no one to read a JSON error):
/// one sentence and a link to `/Device`, where the browser is registered again. Nothing about the instance: no host,
/// title, theme, logo, user, path or error text; no script or style sheet of the instance's.
pub fn refusal_page(text: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex\"><title>Display not available</title></head>\
         <body style=\"font-family:sans-serif;margin:3rem;font-size:1.5rem;\"><p>{}</p>\
         <p><a href=\"/Device\">Register this display</a></p></body></html>\n",
        crate::screen::encode_html(text)
    )
}

/// What [`begin_request`] found.
pub enum DeviceRequest {
    /// No device cookie: an ordinary request.
    None,
    /// A device's request, running as its display account until the guard drops.
    Active(DeviceGuard),
    Refused(Refusal),
}

/// A device's request under way. Dropping it gives the context back the web session's own identity.
#[must_use = "dropping the guard ends the device's request"]
pub struct DeviceGuard {
    scope: Arc<DeviceScope>,
    /// The daily replacement of the secret, when it is due: made only by [`DeviceGuard::rotate`], once the response
    /// that will carry the new cookie is known.
    rotation: Option<Rotation>,
}

/// A replacement of a device's secret that is due ([`ROTATE_MILLIS`]).
struct Rotation {
    device_id: String,
    /// The hash the browser's secret has now: the write holds only while it is still the stored one.
    current: String,
    max_age_seconds: i64,
    https: bool,
}

impl DeviceGuard {
    pub fn scope(&self) -> &Arc<DeviceScope> {
        &self.scope
    }

    /// Replace the device's secret when its day is up, and the `Set-Cookie` header of the new one: called only for a
    /// response that will reach the browser with the header (not one already streaming, not one a shared cache may
    /// keep), since a browser that never gets the new secret is stopped as a copy once the grace ends. `None` when no
    /// replacement is due, or another request of the device made it first (that one's response carries the cookie).
    ///
    /// # Errors
    ///
    /// Returns an error when the random source fails or the device cannot be written.
    pub fn rotate(&mut self) -> Result<Option<String>, Error> {
        let Some(rotation) = self.rotation.take() else {
            return Ok(None);
        };
        let _authz = without_authz();
        let facade = entity_facade()?;
        let secret = super::factor::random_string(SECRET_LENGTH)?;
        let fields = vec![
            ("credentialHash", Value::string(secret_hash(&secret)?)),
            ("previousHash", Value::string(&rotation.current)),
            ("rotatedDate", stamp_value(now_millis())),
        ];
        let guard = Condition::equals("credentialHash", Value::string(&rotation.current));
        if write_device(&facade, &rotation.device_id, Some(guard), fields)? == 0 {
            return Ok(None);
        }
        Ok(Some(format_cookie(
            &rotation.device_id,
            &secret,
            rotation.max_age_seconds,
            rotation.https,
        )))
    }
}

impl Drop for DeviceGuard {
    fn drop(&mut self) {
        session::leave_device();
    }
}

/// The scope of the device whose request this is, when it is one.
pub fn current_scope() -> Option<Arc<DeviceScope>> {
    session::device_scope()
}

/// The device cookie of a request, when it sends one.
pub fn cookie_of(request: &WebRequest) -> Option<String> {
    request
        .header("cookie")
        .and_then(|header| crate::web::cookie_value(header, DEVICE_COOKIE_NAME))
        .filter(|value| !value.is_empty())
}

/// `moqui.device=<deviceId>.<secret>; HttpOnly; SameSite=Strict; Path=/; Max-Age=...` and `; Secure` on https.
pub fn format_cookie(device_id: &str, secret: &str, max_age_seconds: i64, https: bool) -> String {
    let mut out = format!(
        "{DEVICE_COOKIE_NAME}={device_id}.{secret}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        max_age_seconds.max(0)
    );
    if https {
        out.push_str("; Secure");
    }
    out
}

/// The header that removes the device cookie from a browser whose device is no more.
pub fn expired_cookie(https: bool) -> String {
    format_cookie("", "", 0, https)
}

/// A new registration code and its hash: 20 letters and digits not easily mistaken for each other (about 99 bits) in
/// groups of four, typed once on the device. Only the hash is stored.
///
/// # Errors
///
/// Returns an error when the system random source fails.
pub fn new_registration_code() -> Result<(String, String), Error> {
    let code = super::login::new_app_password()?;
    let hash = code_hash(&code)?;
    Ok((code, hash))
}

/// The stored form of a registration code: the single-use hash of the code without its dashes and spaces, in lower
/// case. `None` for text that is no code.
fn code_hash(code: &str) -> Result<String, Error> {
    super::factor::single_use_hash(&normal_code(code))
}

fn normal_code(code: &str) -> String {
    code.chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

fn secret_hash(secret: &str) -> Result<String, Error> {
    super::password::hash_login_key(secret)
}

/// Compare two stored hashes without stopping at the first difference.
fn same_hash(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && !left.is_empty()
        && left.bytes().zip(right.bytes()).fold(0_u8, |all, (a, b)| all | (a ^ b)) == 0
}

fn is_device_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn is_secret(secret: &str) -> bool {
    secret.len() == SECRET_LENGTH
        && secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn text(row: &EntityValue, name: &str) -> String {
    match row.get(name) {
        Some(Value::String(value)) => value.clone(),
        Some(other) if !other.is_null() => moqui_lib::object::to_plain_string(other),
        _ => String::new(),
    }
}

fn millis(row: &EntityValue, name: &str) -> Option<i64> {
    row.get(name).filter(|value| !value.is_null()).and_then(value_millis)
}

fn number(row: &EntityValue, name: &str) -> Option<i64> {
    match moqui_lib::object::basic_convert(row.get(name).filter(|value| !value.is_null())?, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

fn without_authz() -> Option<crate::context::AuthzGuard> {
    ExecutionContext::with_current(ExecutionContext::disable_authz)
}

fn find_device(facade: &EntityFacade, device_id: &str) -> Result<Option<EntityValue>, Error> {
    Ok(facade
        .find(DEVICE)?
        .eq("deviceId", Value::string(device_id))
        .use_cache(false)
        .one()?)
}

/// Write `fields` on the device in a transaction of its own, only while `guard` (a field and the value it must
/// still have) holds; how many rows changed. The request's own transaction, if any, is left alone, so a refusal
/// that rolls it back keeps what was learned of the device.
fn write_device(
    facade: &EntityFacade,
    device_id: &str,
    guard: Option<Condition>,
    fields: Vec<(&str, Value)>,
) -> Result<i64, Error> {
    let tx = facade.transactions_for(DEVICE)?;
    Ok(tx.run(TransactionAttribute::ForceNew, None, || {
        let mut find = facade
            .find(DEVICE)?
            .eq("deviceId", Value::string(device_id))
            .use_cache(false);
        if let Some(guard) = guard.clone() {
            find = find.condition(guard);
        }
        find.update_all(fields.iter().map(|(name, value)| ((*name).to_string(), value.clone())))
    })?)
}

/// A row of the audit log for something that happened to a device with no audit-logged field changing (redeemed,
/// refused, stopped): on its display account (`UserAccount`, the field `deviceRedeemed`, `deviceRefused` or
/// `deviceStopped`, the device the secondary key), as the administration services write theirs, so it is read with
/// the account's other evidence by whoever may read that account's. Never a code or a secret. A failure is logged and
/// changes nothing.
fn audit(facade: &EntityFacade, device: &EntityValue, field: &str, summary: &str) {
    let device_id = text(device, "deviceId");
    let written = facade.transactions_for(AUDIT_LOG).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            let mut row = facade.make_value(AUDIT_LOG)?;
            row.set_sequenced_id_primary(facade)?;
            row.set("changedEntityName", Value::string(USER_ACCOUNT));
            row.set("changedFieldName", Value::string(field));
            row.set("pkPrimaryValue", Value::string(text(device, "displayUserId")));
            row.set("pkSecondaryValue", Value::string(&device_id));
            row.set("newValueText", Value::string(summary));
            row.set("changedDate", super::login::now_value());
            if let Some(visit_id) = session::visit_id().ok().flatten() {
                row.set("changedInVisitId", Value::string(visit_id));
            }
            row.create(facade)
        })
    });
    if let Err(err) = written {
        tracing::error!(device_id = %device_id, "device audit row ({field}: {summary}) not written: {err}");
    }
}

/// Stop the device for good, once: the reason and when, with an audit row.
fn stop(facade: &EntityFacade, device: &EntityValue, reason: &str, summary: &str) {
    let device_id = text(device, "deviceId");
    let fields = vec![
        ("stoppedDate", super::login::now_value()),
        ("stopReason", Value::string(reason)),
    ];
    match write_device(facade, &device_id, Some(Condition::is_null("stoppedDate")), fields) {
        Ok(changed) if changed > 0 => {
            tracing::warn!(device_id = %device_id, "shared device stopped: {summary}");
            audit(facade, device, "deviceStopped", summary);
        }
        Ok(_) => {}
        Err(err) => tracing::error!(device_id = %device_id, "shared device not stopped ({reason}): {err}"),
    }
}

/// Why the display account may not serve a device now, if it may not: gone, closed (disabled for good or past its end
/// date), no longer a display account, or holding what no device may run with: a membership in a privileged group or a
/// group's grant of a sealed permission (the effective lists of `user-facade` `authz`). Read on every request of the
/// device, so a change holds from the next one.
///
/// # Errors
///
/// Returns an error when the records cannot be read.
pub fn display_account_refusal(facade: &EntityFacade, user_id: &str) -> Result<Option<String>, Error> {
    let _authz = without_authz();
    let Some(account) = facade.find(USER_ACCOUNT)?.eq("userId", Value::string(user_id)).one()? else {
        return Ok(Some("the display account does not exist".to_string()));
    };
    if !super::login::is_display_account(&account) {
        return Ok(Some("the account is not a display account".to_string()));
    }
    let now = now_millis();
    if super::login::account_closed(&account, now) {
        return Ok(Some("the display account is disabled or past its end date".to_string()));
    }
    if !text(&account, "currentPassword").is_empty() || !text(&account, "externalUserId").is_empty() {
        return Ok(Some(
            "the display account has a password or an external identity".to_string(),
        ));
    }
    let privileged = crate::service::privileged_groups();
    let sealed = crate::service::sealed_permissions();
    for group in super::preference::user_group_ids(Some(user_id))? {
        if privileged.contains(&group) {
            return Ok(Some(format!("the display account is in the privileged group {group}")));
        }
        let grants = facade
            .find("moqui.security.UserGroupPermission")?
            .eq("userGroupId", Value::string(&group))
            .use_cache(false)
            .list()?;
        for grant in grants.values() {
            let permission = text(grant, "userPermissionId");
            let open = millis(grant, "fromDate").is_none_or(|from| from <= now)
                && millis(grant, "thruDate").is_none_or(|thru| thru > now);
            if open && sealed.contains(&permission) {
                return Ok(Some(format!(
                    "the display account holds the sealed permission {permission} through {group}"
                )));
            }
        }
    }
    Ok(None)
}

/// The route group's scope for a device, or `None` when the group is missing, disabled or names no artifact group.
fn route_scope(facade: &EntityFacade, device: &EntityValue) -> Result<Option<DeviceScope>, Error> {
    let route_group_id = text(device, "routeGroupId");
    let Some(group) = facade
        .find(ROUTE_GROUP)?
        .eq("routeGroupId", Value::string(&route_group_id))
        .use_cache(true)
        .one()?
    else {
        return Ok(None);
    };
    let artifact_group_id = text(&group, "artifactGroupId");
    if text(&group, "disabled") == "Y" || artifact_group_id.is_empty() {
        return Ok(None);
    }
    let name = text(&group, "parameterName");
    let value = text(device, "parameterValue");
    Ok(Some(DeviceScope {
        device_id: text(device, "deviceId"),
        route_group_id,
        artifact_group_id,
        parameter: (!name.is_empty() && !value.is_empty()).then_some((name, value)),
    }))
}

/// Note a refusal that is not the device's end (its address, its account) on the device at most every ten minutes,
/// with an audit row then: a refused display asks again every few seconds.
fn note_refusal(facade: &EntityFacade, device: &EntityValue, summary: &str) {
    let device_id = text(device, "deviceId");
    let now = now_millis();
    // throttled by the newest refusal row of the device
    let recent = facade
        .find(AUDIT_LOG)
        .and_then(|find| {
            // the whole of ENTAUDLOG_FLD1PK (entity, field, display account), then the device
            find.eq("changedEntityName", Value::string(USER_ACCOUNT))
                .eq("changedFieldName", Value::string("deviceRefused"))
                .eq("pkPrimaryValue", Value::string(text(device, "displayUserId")))
                .eq("pkSecondaryValue", Value::string(&device_id))
                .condition(Condition::greater("changedDate", stamp_value(now - 10 * MINUTE)))
                .use_cache(false)
                .count()
        })
        .unwrap_or(1);
    if recent == 0 {
        tracing::warn!(device_id = %device_id, "shared device refused: {summary}");
        audit(facade, device, "deviceRefused", summary);
    }
}

/// Begin a request that may be a registered device's ([`cookie_of`]): check the credential and everything it rests on,
/// and make the display account the context's user for the request. `client_ip` and `agent` are the request's.
///
/// # Errors
///
/// Returns an error when the records cannot be read or written; a refusal is not an error.
pub fn begin_request(request: &WebRequest) -> Result<DeviceRequest, Error> {
    let Some(cookie) = cookie_of(request) else {
        return Ok(DeviceRequest::None);
    };
    begin_with_cookie(
        &cookie,
        &crate::web::client_ip(request),
        request.header("user-agent").unwrap_or(""),
        request.https,
    )
}

/// [`begin_request`] for a cookie value.
///
/// # Errors
///
/// Returns an error when the records cannot be read or written.
pub fn begin_with_cookie(cookie: &str, client_ip: &str, agent: &str, https: bool) -> Result<DeviceRequest, Error> {
    let Some((device_id, secret)) = cookie
        .split_once('.')
        .filter(|(id, secret)| is_device_id(id) && is_secret(secret))
    else {
        crate::web::note_auth_failure();
        return Ok(DeviceRequest::Refused(Refusal::Unknown));
    };
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(device) = find_device(&facade, device_id)? else {
        crate::web::note_auth_failure();
        return Ok(DeviceRequest::Refused(Refusal::Unknown));
    };
    let now = now_millis();
    // the secret first: a request that does not hold one learns nothing of the device, not even that it exists
    let current = text(&device, "credentialHash");
    let presented = secret_hash(secret)?;
    let is_current = same_hash(&presented, &current);
    if millis(&device, "redeemedDate").is_none()
        || !(is_current || same_hash(&presented, &text(&device, "previousHash")))
    {
        crate::web::note_auth_failure();
        return Ok(DeviceRequest::Refused(Refusal::Unknown));
    }
    if millis(&device, "revokedDate").is_some() {
        return Ok(DeviceRequest::Refused(Refusal::Revoked));
    }
    if millis(&device, "stoppedDate").is_some() {
        return Ok(DeviceRequest::Refused(Refusal::Stopped));
    }
    let rotated = millis(&device, "rotatedDate").unwrap_or(now);
    if !is_current && now - rotated > ROTATE_GRACE_MILLIS {
        // the browser the secret was replaced for has the new one; whoever still presents this holds a copy
        stop(
            &facade,
            &device,
            "copied",
            "An earlier secret of the device was presented after it was replaced: a copy of its cookie is in use",
        );
        return Ok(DeviceRequest::Refused(Refusal::Stopped));
    }
    if millis(&device, "expireDate").is_some_and(|expire| now >= expire) {
        stop(
            &facade,
            &device,
            "expired",
            "The device's registration reached its end date",
        );
        return Ok(DeviceRequest::Refused(Refusal::Stopped));
    }
    let idle = number(&device, "idleTimeoutMinutes")
        .filter(|minutes| *minutes > 0)
        .unwrap_or(DEFAULT_IDLE_MINUTES);
    let seen = millis(&device, "lastSeenDate")
        .or_else(|| millis(&device, "redeemedDate"))
        .unwrap_or(now);
    if now - seen > idle.saturating_mul(MINUTE) {
        stop(
            &facade,
            &device,
            "idle",
            &format!("The device was not seen for more than {idle} minutes"),
        );
        return Ok(DeviceRequest::Refused(Refusal::Stopped));
    }
    let allowed = text(&device, "ipAllowed");
    // an address that is not known matches no pattern: a device held to addresses fails closed
    if !allowed.is_empty() && !crate::web::ip_matches(&allowed, client_ip) {
        note_refusal(
            &facade,
            &device,
            "A request from an address the device is not allowed from",
        );
        return Ok(DeviceRequest::Refused(Refusal::Address));
    }
    let user_id = text(&device, "displayUserId");
    if let Some(reason) = display_account_refusal(&facade, &user_id)? {
        note_refusal(&facade, &device, &format!("The device was refused: {reason}"));
        return Ok(DeviceRequest::Refused(Refusal::Account));
    }
    let Some(scope) = route_scope(&facade, &device)? else {
        note_refusal(&facade, &device, "The device's route group is not available");
        return Ok(DeviceRequest::Refused(Refusal::Account));
    };
    let username = facade
        .find(USER_ACCOUNT)?
        .eq("userId", Value::string(&user_id))
        .one()?
        .map(|account| text(&account, "username"))
        .unwrap_or_default();
    if username.is_empty() {
        return Ok(DeviceRequest::Refused(Refusal::Account));
    }
    // last seen, at most once a minute
    if millis(&device, "lastSeenDate").is_none_or(|seen| now - seen > MINUTE) {
        let fields = vec![
            ("lastSeenDate", stamp_value(now)),
            ("lastSeenAddress", Value::string(client_ip)),
            (
                "lastSeenAgent",
                Value::string(agent.chars().take(255).collect::<String>()),
            ),
        ];
        if let Err(err) = write_device(&facade, device_id, None, fields) {
            tracing::warn!(device_id = %device_id, "shared device: last seen not written: {err}");
        }
    }
    // a new secret each day, for the browser that holds the current one, made when the response is known
    // (DeviceGuard::rotate)
    let rotation = (is_current && now - rotated > ROTATE_MILLIS).then(|| Rotation {
        device_id: device_id.to_string(),
        current: current.clone(),
        max_age_seconds: millis(&device, "expireDate").map_or(ROTATE_MILLIS * 400, |expire| expire - now) / 1000,
        https,
    });
    let scope = Arc::new(scope);
    session::enter_device(&user_id, &username, Arc::clone(&scope));
    Ok(DeviceRequest::Active(DeviceGuard { scope, rotation }))
}

/// What [`redeem`] answered.
#[derive(Debug)]
pub struct Redeemed {
    pub device_id: String,
    pub route_group_id: String,
    /// The `Set-Cookie` header that makes this browser the device.
    pub set_cookie: String,
}

/// Why a registration code was not redeemed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedeemRefusal {
    /// No registration has this code.
    Unknown,
    /// The code was redeemed before: it works once.
    Used,
    Expired,
    /// The registration was revoked before its code was used.
    Revoked,
}

impl RedeemRefusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unknown => "CODE_UNKNOWN",
            Self::Used => "CODE_USED",
            Self::Expired => "CODE_EXPIRED",
            Self::Revoked => "DEVICE_REVOKED",
        }
    }
}

/// Redeem a registration code for the browser whose request this is: once, before it expires. The device's secret is
/// made here and returned only inside the cookie header; its hash replaces nothing but an unredeemed registration
/// (the write holds only while `redeemedDate` is empty, so two redemptions at once make one device). A refusal is
/// noted for the web protection, which slows and blocks an address that guesses, and a code that was used, expired
/// or revoked leaves an audit row on its device. Any user signed in on the browser's session is signed out: a
/// device's browser holds no person's session.
///
/// # Errors
///
/// Returns an error when the records cannot be read or written.
pub fn redeem(code: &str, client_ip: &str, agent: &str, https: bool) -> Result<Result<Redeemed, RedeemRefusal>, Error> {
    let normal = normal_code(code);
    if normal.len() != 20 || !normal.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        crate::web::note_auth_failure();
        return Ok(Err(RedeemRefusal::Unknown));
    }
    let hash = code_hash(code)?;
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(device) = facade
        .find(DEVICE)?
        .eq("registrationCodeHash", Value::string(&hash))
        .use_cache(false)
        .one()?
    else {
        crate::web::note_auth_failure();
        return Ok(Err(RedeemRefusal::Unknown));
    };
    let device_id = text(&device, "deviceId");
    let now = now_millis();
    let refuse = |refusal: RedeemRefusal, summary: &str| {
        crate::web::note_auth_failure();
        audit(&facade, &device, "deviceRefused", summary);
        Ok(Err(refusal))
    };
    if millis(&device, "revokedDate").is_some() || millis(&device, "stoppedDate").is_some() {
        return refuse(
            RedeemRefusal::Revoked,
            "The registration code of a revoked device was presented",
        );
    }
    if millis(&device, "redeemedDate").is_some() {
        return refuse(
            RedeemRefusal::Used,
            "The registration code was presented again after it was used",
        );
    }
    if millis(&device, "codeExpireDate").is_none_or(|expire| now >= expire) {
        return refuse(
            RedeemRefusal::Expired,
            "The registration code was presented after it expired",
        );
    }
    let secret = super::factor::random_string(SECRET_LENGTH)?;
    let fields = vec![
        ("credentialHash", Value::string(secret_hash(&secret)?)),
        ("previousHash", Value::Null),
        ("redeemedDate", stamp_value(now)),
        ("redeemedAddress", Value::string(client_ip)),
        ("rotatedDate", stamp_value(now)),
        ("lastSeenDate", stamp_value(now)),
        ("lastSeenAddress", Value::string(client_ip)),
        (
            "lastSeenAgent",
            Value::string(agent.chars().take(255).collect::<String>()),
        ),
    ];
    let guard = Condition::and(vec![
        Condition::is_null("redeemedDate"),
        Condition::is_null("revokedDate"),
        Condition::equals("registrationCodeHash", Value::string(&hash)),
    ]);
    if write_device(&facade, &device_id, Some(guard), fields)? == 0 {
        return refuse(
            RedeemRefusal::Used,
            "The registration code was presented again after it was used",
        );
    }
    audit(
        &facade,
        &device,
        "deviceRedeemed",
        &format!("The registration code was redeemed from {client_ip}"),
    );
    if session::current_user_id()?.is_some_and(|id| !id.is_empty()) {
        session::clear_login();
    }
    let left = millis(&device, "expireDate").map_or(ROTATE_MILLIS * 400, |expire| expire - now);
    Ok(Ok(Redeemed {
        set_cookie: format_cookie(&device_id, &secret, left / 1000, https),
        route_group_id: text(&device, "routeGroupId"),
        device_id,
    }))
}

/// Whether a request under a device may open what `info` resolved to: the device page; a static file anyone may
/// have; a screen that is a member of the route group's artifact group, or below a member that is inheritable; a
/// transition that is a member, or of or below an inheritable screen member. Deny is the default: a screen, transition,
/// REST or service path, script servlet or file not admitted here is refused whatever the display account's own groups
/// are authorized for.
///
/// # Errors
///
/// Returns `ErrorKind::Unauthorized` for what the device may not open, or an error when the members cannot be read.
pub(crate) fn check_route(info: &crate::screen::ScreenUrlInfo) -> Result<(), Error> {
    let Some(scope) = current_scope() else {
        return Ok(());
    };
    if info.target_screen.location == DEVICE_SCREEN {
        return Ok(());
    }
    if info.file_resource.is_some()
        && !info
            .render_defs()
            .iter()
            .any(|def| def.authentication == crate::screen::Authentication::Required)
    {
        return Ok(());
    }
    let _authz = without_authz();
    let facade = entity_facade()?;
    let members = facade
        .find(GROUP_MEMBER)?
        .eq("artifactGroupId", Value::string(&scope.artifact_group_id))
        .use_cache(true)
        .list()?;
    let target = info.target_screen.location.as_str();
    let transition = info
        .target_transition_name
        .as_ref()
        .map(|name| format!("{target}/{name}"));
    let matches = |member: &EntityValue, name: &str| {
        let member_name = text(member, "artifactName");
        if text(member, "nameIsPattern") == "Y" {
            moqui_lib::object::compare(&Value::string(name), "matches", &Value::string(&member_name), "String")
                .unwrap_or(false)
        } else {
            member_name == name
        }
    };
    let admitted = members.values().iter().any(|member| {
        let inherit = text(member, "inheritAuthz") == "Y";
        match text(member, "artifactTypeEnumId").as_str() {
            "AT_XML_SCREEN" => {
                // the screen itself (its render; with inherit its transitions too), or a screen above it that inherits
                (matches(member, target) && (transition.is_none() || inherit))
                    || (inherit && info.path_defs.iter().any(|def| matches(member, &def.location)))
            }
            "AT_XML_SCREEN_TRANS" => transition.as_deref().is_some_and(|name| matches(member, name)),
            _ => false,
        }
    });
    if admitted {
        return Ok(());
    }
    Err(refused_route(&scope))
}

/// The refusal of something outside a device's route group.
pub(crate) fn refused_route(scope: &DeviceScope) -> Error {
    Error::of_kind(
        ErrorKind::Unauthorized,
        format!("This device opens only {}", scope.route_group_id),
    )
}

#[cfg(test)]
#[path = "device_tests.rs"]
mod tests;
