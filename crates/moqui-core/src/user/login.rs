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

//! Login against `moqui.security.UserAccount`. A bound web session rotates on password success.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_db::sql::{select as sql_select, update as sql_update, Condition, FieldRef, LimitOffset};
use moqui_lib::object::{basic_convert, Value};

use super::password::{self, hash_password};
use super::policy::{self, password_acceptable};
use super::session;
use crate::context::{Error, ExecutionContext};
use crate::driver::{DbConnection, DbRow};
use crate::entity::{EntityFacade, EntityValue};

const USER_ACCOUNT: &str = "moqui.security.UserAccount";
const USER_LOGIN_KEY: &str = "moqui.security.UserLoginKey";
const USER_LOGIN_HISTORY: &str = "moqui.security.UserLoginHistory";
const PASSWORD_HISTORY: &str = "moqui.security.UserPasswordHistory";
const INVALID_LOGIN: &str = "The username or password is not valid";
const WEEK_MILLIS: i64 = 7 * 24 * 60 * 60 * 1000;
const HOUR_MILLIS: f64 = 60.0 * 60.0 * 1000.0;
/// Session attributes of a login that passed the password and is not finished, the names Java uses.
pub(crate) const PRE_AUTHC_USERNAME: &str = "moquiPreAuthcUsername";
pub(crate) const AUTHC_FACTOR_REQUIRED: &str = "moquiAuthcFactorRequired";
const PASSWORD_CHANGE_REQUIRED: &str = "moquiPasswordChangeRequired";
const EXPIRED_CREDENTIALS: &str = "moquiExpiredCredentials";

/// Check the password and, on success, set the in-memory user.
///
/// Unknown users, an empty username, and an empty password share one message.
/// As in Java, a bound web session is renewed before the password is checked: its visit is closed and it moves to
/// a new id and token, whether or not the password is right.
///
/// A user who needs a second factor is not logged in. The password was right, so the session
/// remembers the user in `moquiPreAuthcUsername` with `moquiAuthcFactorRequired`,
/// and this returns `Ok` with no current user, as Java's `loginUser` returns false without an
/// error. The code then comes through `validate#ExternalUserAuthcCode` and [`internal_login_user`].
pub fn login_user(username: &str, password: &str) -> Result<(), Error> {
    if !username.is_empty() && !password.is_empty() {
        session::renew_for_login()?;
    }
    password_login(username, password)
}

/// Check the password and set the user for one piece of work, as Java's `loginUser` paired with `logoutLocal`. The
/// bound session keeps its id, and dropping the returned guard puts back the user, locale and time zone the context
/// had before. A failed login is undone the same way.
///
/// # Errors
///
/// Returns the [`login_user`] errors.
pub(crate) fn login_user_local(username: &str, password: &str) -> Result<LocalLogin, Error> {
    let restore = LocalLogin(session::identity());
    password_login(username, password)?;
    Ok(restore)
}

/// The identity a [`login_user_local`] replaced. Dropping it puts that identity back.
#[must_use = "dropping the guard logs the local user out"]
pub(crate) struct LocalLogin(session::Identity);

impl Drop for LocalLogin {
    fn drop(&mut self) {
        session::restore_identity(&self.0);
    }
}

/// The password check and login of [`login_user`].
fn password_login(username: &str, password: &str) -> Result<(), Error> {
    clear_pre_authc();
    if username.is_empty() || password.is_empty() {
        return Err(Error::new(INVALID_LOGIN));
    }
    // The login reads and writes the user's own account whatever the caller may do.
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(account) = find_username(&facade, username)? else {
        password::burn_verify(password);
        return Err(Error::new(INVALID_LOGIN));
    };
    let now = now_millis();
    if let Err(err) = gate_account(&facade, &account, now) {
        password::burn_verify(password);
        return Err(err);
    }
    // As Java's realm, once the account passes the gate every outcome is kept, a pending second factor as a failure.
    let outcome = authenticate(&facade, &account, password, now);
    save_history(&text_field(&account, "userId"), password, matches!(outcome, Ok(true)));
    outcome.map(drop)
}

/// The password and post-password checks of [`password_login`]. True when the login finished, false when it waits
/// for a second factor.
fn authenticate(facade: &EntityFacade, account: &EntityValue, password: &str, now: i64) -> Result<bool, Error> {
    // A legacy hash checks in microseconds; the argon2 burn keeps it as slow as an unknown user, so
    // the time does not tell which usernames exist.
    if legacy_hash(account) {
        password::burn_verify(password);
    }
    if !password_ok(account, password)? {
        let user_id = text_field(account, "userId");
        if !user_id.is_empty() {
            increment_failed(facade, &user_id)?;
        }
        return Err(Error::new(INVALID_LOGIN));
    }
    post_password(account, now)?;
    let user_id = text_field(account, "userId");
    if !user_id.is_empty() && super::factor::second_factor_required(&user_id)? {
        rehash_legacy(facade, account, password)?;
        let account_username = text_field(account, "username");
        session::set_session_attribute(PRE_AUTHC_USERNAME, Some(&account_username));
        session::set_session_attribute(AUTHC_FACTOR_REQUIRED, Some("true"));
        return Ok(false);
    }
    finish_login(facade, account, Some(password), true)?;
    Ok(true)
}

/// Java `MoquiShiroRealm.loginSaveHistory`: the time and visit are taken now, and a worker then creates the
/// `UserLoginHistory` row unless the user has one from the last minute. A failure there is only logged.
fn save_history(user_id: &str, password: &str, successful: bool) {
    if user_id.is_empty() || !policy::login_history_store() {
        return;
    }
    let from_date = now_effective_millis();
    let mut fields = BTreeMap::from([
        ("userId".to_string(), Value::string(user_id)),
        ("fromDate".to_string(), stamp_value(from_date)),
        ("successfulLogin".to_string(), Value::string(if successful { "Y" } else { "N" })),
    ]);
    if let Some(visit_id) = session::visit_id().ok().flatten() {
        fields.insert("visitId".to_string(), Value::string(visit_id));
    }
    if !successful && policy::login_history_incorrect_password() {
        fields.insert("passwordUsed".to_string(), Value::string(password));
    }
    let user_id = user_id.to_string();
    let task = move || {
        if let Err(err) = store_history(&user_id, from_date, &fields) {
            tracing::warn!(error = %err, "UserLoginHistory create failed");
        }
    };
    let queued = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory()))
        .ok_or_else(|| Error::new("no execution context"))
        .and_then(|factory| factory.service_pools().run_in_worker(task));
    if let Err(err) = queued {
        tracing::warn!(error = %err, "UserLoginHistory create failed");
    }
}

fn store_history(user_id: &str, from_date: i64, fields: &BTreeMap<String, Value>) -> Result<(), Error> {
    let _authz = without_authz();
    let recent = entity_facade()?
        .find(USER_LOGIN_HISTORY)?
        .eq("userId", Value::string(user_id))
        .condition(Condition::greater("fromDate", stamp_value(from_date - 60_000)))
        .count()?;
    if recent == 0 {
        crate::service::call::sync("create#moqui.security.UserLoginHistory", fields)?;
    }
    Ok(())
}

/// Every login starts by forgetting an earlier pre-authenticated user, so a finished login does
/// not leave the second-factor state of another attempt behind.
fn clear_pre_authc() {
    session::set_session_attribute(PRE_AUTHC_USERNAME, None);
    session::set_session_attribute(AUTHC_FACTOR_REQUIRED, None);
}

/// Authorization is off while the login code reads and writes the user tables.
fn without_authz() -> Option<crate::context::AuthzGuard> {
    ExecutionContext::with_current(|ctx| ctx.disable_authz())
}

/// Set the in-memory user without checking the password. Does not rotate a bound session.
///
/// # Errors
///
/// Returns an error when the account is unknown, disabled, terminated, or needs a password change.
pub fn internal_login_user(username: &str) -> Result<(), Error> {
    if username.is_empty() {
        return Err(Error::new("No username specified"));
    }
    clear_pre_authc();
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(account) = find_username(&facade, username)? else {
        return Err(Error::new(INVALID_LOGIN));
    };
    let now = now_millis();
    gate_account(&facade, &account, now)?;
    post_password(&account, now)?;
    finish_login(&facade, &account, None, false)
}

/// A login an identity provider vouched for (SSO): [`internal_login_user`], then the session id is replaced as after
/// a password login, so a session id from before the redirect does not carry the new login.
///
/// # Errors
///
/// Returns an error when the account is unknown, disabled, terminated, or not allowed from this address.
pub fn login_external_user(username: &str) -> Result<(), Error> {
    internal_login_user(username)?;
    session::rotate_on_password_login()
}

/// Log in with a `UserLoginKey`. The stored `loginKey` is [`super::password::hash_login_key`].
///
/// A `thruDate` in the past is rejected. A null `thruDate` stays valid. Does not rotate the session id.
///
/// # Errors
///
/// Returns [`INVALID_LOGIN`](Error) when the key is empty, unknown, or expired, or the account cannot log in.
pub fn login_user_key(key: &str) -> Result<(), Error> {
    if key.is_empty() {
        return Err(Error::new(INVALID_LOGIN));
    }
    let hashed = password::hash_login_key(key)?;
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(row) = facade
        .find(USER_LOGIN_KEY)?
        .eq("loginKey", Value::string(&hashed))
        .one()?
    else {
        return Err(Error::new(INVALID_LOGIN));
    };
    if let Some(thru) = field_millis(&row, "thruDate") {
        if now_millis() > thru {
            return Err(Error::new(INVALID_LOGIN));
        }
    }
    let user_id = text_field(&row, "userId");
    let Some(account) = find_user_id(&facade, &user_id)? else {
        return Err(Error::new(INVALID_LOGIN));
    };
    internal_login_user(&text_field(&account, "username"))
}

/// `getLoginKey`: a new 40 character key for the logged-in user, good for `expire_hours`
/// (the conf `login-key/@expire-hours` when `None`). The key is returned once. Only its hash is
/// stored, in a transaction of its own, and the user's expired keys are removed.
///
/// This is how an API key is issued. Revoking one is deleting its `UserLoginKey` row or setting
/// its `thruDate`; [`login_user_key`] rejects either.
///
/// # Errors
///
/// Returns an error when nobody is logged in or a write fails.
pub fn get_login_key(expire_hours: Option<f64>) -> Result<String, Error> {
    let Some(user_id) = session::current_user_id()?.filter(|id| !id.is_empty()) else {
        return Err(Error::of_kind(
            crate::context::ErrorKind::AuthenticationRequired,
            "No active user, cannot get login key",
        ));
    };
    let login_key = super::factor::random_string_for("login-key", 40)?;
    let hashed = password::hash_login_key(&login_key)?;
    let hours = expire_hours.unwrap_or_else(policy::login_key_expire_hours);
    let from = now_millis();
    #[allow(clippy::cast_possible_truncation)] // An expiry past i64 milliseconds is not a real key.
    let thru = from.saturating_add((hours * HOUR_MILLIS).round() as i64);
    let _authz = without_authz();
    let facade = entity_facade()?;
    let mut row = facade.make_value(USER_LOGIN_KEY)?;
    row.set("loginKey", Value::string(hashed));
    row.set("userId", Value::string(&user_id));
    row.set("fromDate", stamp_value(from));
    row.set("thruDate", stamp_value(thru));
    // Java creates the key with requireNewTransaction, so a caller's rollback does not lose it.
    let tx = facade.transactions_for(USER_LOGIN_KEY)?;
    tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
        row.create(&facade)
    })?;
    let expired = facade
        .find(USER_LOGIN_KEY)?
        .eq("userId", Value::string(&user_id))
        .condition(moqui_db::sql::Condition::less("thruDate", stamp_value(from)))
        .list()?;
    for old in expired.iter() {
        old.delete(&facade)?;
    }
    Ok(login_key)
}

/// Header login. Uses `api_key` when it is present, otherwise `login_key`.
///
/// Does not read a query string. An existing username returns without a lookup.
///
/// # Errors
///
/// Returns the login error when the chosen key is rejected.
pub fn apply_header_keys(api_key: Option<&str>, login_key: Option<&str>) -> Result<(), Error> {
    if session::current_username()?.is_some() {
        return Ok(());
    }
    let Some(key) = present_key(api_key).or_else(|| present_key(login_key)) else {
        return Ok(());
    };
    login_user_key(key)
}

fn present_key(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// Mark the session anonymous when nobody is logged in. Returns true when it did, so the caller
/// can undo it with [`logout_anonymous_only`]. This does not use the `_NA_` account.
///
/// # Errors
///
/// Returns an error only if the session cannot be read.
pub fn login_anonymous() -> Result<bool, Error> {
    if session::current_username()?.is_some() || session::is_anonymous()? {
        return Ok(false);
    }
    session::set_anonymous(true);
    Ok(true)
}

/// Undo [`login_anonymous`] without touching a real login.
pub fn logout_anonymous_only() {
    if session::current_username().ok().flatten().is_none() {
        session::set_anonymous(false);
    }
}

/// Clear the in-memory user and anonymous flag. Sets `hasLoggedOut` on the account.
///
/// Guarantee mode flushes the bound session after the user is cleared.
///
/// # Errors
///
/// Returns an error when the account row cannot be updated. The session is cleared either way.
pub fn logout() -> Result<(), Error> {
    let _authz = without_authz();
    let update = if let Some(user_id) = session::current_user_id()? {
        match entity_facade() {
            Ok(facade) => mark_logged_out(&facade, &user_id),
            Err(_) => Ok(()),
        }
    } else {
        Ok(())
    };
    session::clear_login();
    update.and(session::flush_after_logout())
}

/// Replace the password with argon2 and push the old hash into `UserPasswordHistory`.
///
/// The history rows and the new password are one transaction, so a failure part way leaves both
/// as they were.
///
/// # Errors
///
/// Returns an error when the policy rejects the password, it was used recently, or a write fails.
pub fn change_password(user_id: &str, new_password: &str) -> Result<(), Error> {
    password_acceptable(new_password)?;
    let _authz = without_authz();
    let facade = entity_facade()?;
    let account = find_user_id(&facade, user_id)?
        .ok_or_else(|| Error::new(format!("Cannot update password, account not found with ID {user_id}")))?;
    if password_ok(&account, new_password)? {
        return Err(Error::new("New password is same as current password"));
    }
    reject_history(&facade, &account, user_id, new_password)?;
    let hashed = hash_password(new_password)?;
    let tx = facade.transactions_for(USER_ACCOUNT)?;
    tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
        remember_history(&facade, &account, user_id)?;
        trim_history(&facade, user_id)?;
        Ok(update_password_fields(&facade, user_id, &hashed)?)
    })?;
    Ok(())
}

fn update_password_fields(facade: &EntityFacade, user_id: &str, hashed: &str) -> Result<(), Error> {
    update_user(
        facade,
        user_id,
        &[
            ("currentPassword", Value::string(hashed)),
            ("passwordHashType", Value::string("argon2")),
            ("passwordSalt", Value::string("")),
            ("passwordBase64", Value::string("N")),
            ("passwordSetDate", now_value()),
            ("requirePasswordChange", Value::string("N")),
            ("disabled", Value::string("N")),
            ("resetPassword", Value::Null),
            ("resetPasswordSetDate", Value::Null),
        ],
    )
}

pub(crate) fn entity_facade() -> Result<EntityFacade, Error> {
    match ExecutionContext::with_current(|ctx| ctx.factory().entity()) {
        None => Err(Error::new("no execution context")),
        Some(None) => Err(Error::new("no entity facade")),
        Some(Some(facade)) => Ok(facade),
    }
}

pub(crate) fn now_value() -> Value {
    stamp_value(now_millis())
}

pub(crate) fn stamp_value(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

/// Java's `ec.user.nowTimestamp`: the entered context's effective time, or the clock.
pub(crate) fn now_effective_value() -> Value {
    stamp_value(now_effective_millis())
}

/// [`now_effective_value`] as epoch millis.
pub(crate) fn now_effective_millis() -> i64 {
    if crate::deterministic::enabled() {
        return now_millis();
    }
    ExecutionContext::with_current(ExecutionContext::effective_time)
        .flatten()
        .unwrap_or_else(now_millis)
}

pub(crate) fn now_millis() -> i64 {
    moqui_lib::clock::now_millis()
}

fn gate_account(facade: &EntityFacade, account: &EntityValue, now: i64) -> Result<(), Error> {
    let user_id = text_field(account, "userId");
    if still_disabled(account, now) {
        if !user_id.is_empty() {
            increment_failed(facade, &user_id)?;
        }
        return Err(Error::new(INVALID_LOGIN));
    }
    if terminated(account, now) {
        return Err(Error::new(INVALID_LOGIN));
    }
    Ok(())
}

/// A password that must change fails the login and remembers the user, so the change-password
/// form can name them, as Java's `internalLoginToken` does.
fn post_password(account: &EntityValue, now: i64) -> Result<(), Error> {
    if text_field(account, "requirePasswordChange") == "Y" {
        pre_authc(account, PASSWORD_CHANGE_REQUIRED);
        return Err(Error::new(
            "Authenticate failed because the account requires a password change",
        ));
    }
    if password_expired(account, now) {
        pre_authc(account, EXPIRED_CREDENTIALS);
        return Err(Error::new("Authenticate failed because the password must be changed"));
    }
    Ok(())
}

fn pre_authc(account: &EntityValue, flag: &str) {
    session::set_session_attribute(PRE_AUTHC_USERNAME, Some(&text_field(account, "username")));
    session::set_session_attribute(flag, Some("true"));
}

/// A legacy stored hash becomes argon2 once the password has been checked against it.
fn rehash_legacy(facade: &EntityFacade, account: &EntityValue, plain: &str) -> Result<(), Error> {
    if !legacy_hash(account) {
        return Ok(());
    }
    let hashed = hash_password(plain)?;
    update_user(
        facade,
        &text_field(account, "userId"),
        &[
            ("currentPassword", Value::string(hashed)),
            ("passwordHashType", Value::string("argon2")),
            ("passwordSalt", Value::string("")),
            ("passwordBase64", Value::string("N")),
        ],
    )
}

fn finish_login(facade: &EntityFacade, account: &EntityValue, plain: Option<&str>, rehash: bool) -> Result<(), Error> {
    let user_id = text_field(account, "userId");
    let username = text_field(account, "username");
    if user_id.is_empty() || username.is_empty() {
        return Err(Error::new(INVALID_LOGIN));
    }
    // After the second-factor check, as Java `loginPostPassword`: a code login reaches here through
    // `internal_login_user`, which checks the address too.
    check_ip_allowed(account, &crate::web::current_client_ip())?;
    let mut fields = Vec::new();
    if rehash && legacy_hash(account) {
        if let Some(plain) = plain {
            let hashed = hash_password(plain)?;
            fields.push(("currentPassword", Value::string(hashed)));
            fields.push(("passwordHashType", Value::string("argon2")));
            fields.push(("passwordSalt", Value::string("")));
            fields.push(("passwordBase64", Value::string("N")));
        }
    }
    if needs_reset(account) {
        fields.push(("successiveFailedLogins", Value::I64(0)));
        fields.push(("disabled", Value::string("N")));
        fields.push(("disabledDateTime", Value::Null));
        fields.push(("hasLoggedOut", Value::string("N")));
    }
    if !fields.is_empty() {
        update_user(facade, &user_id, &fields)?;
    }
    session::set_user(&user_id, &username);
    let locale = nonempty(text_field(account, "locale"));
    let zone = nonempty(text_field(account, "timeZone"));
    session::set_locale(locale.as_deref());
    session::set_time_zone(zone.as_deref());
    link_visit(facade, &user_id)
}

/// `ipAllowed` of the account and of every group it is in, `ALL_USERS` included: when any is set, the client address
/// must match one of them. With no address, as for a login outside a request, nothing is checked.
fn check_ip_allowed(account: &EntityValue, client_ip: &str) -> Result<(), Error> {
    if client_ip.is_empty() {
        return Ok(());
    }
    let mut allowed = Vec::new();
    if let Some(pattern) = nonempty(text_field(account, "ipAllowed")) {
        allowed.push(pattern);
    }
    let facade = entity_facade()?;
    for group in super::preference::user_group_ids(Some(&text_field(account, "userId")))? {
        let row = facade
            .find("moqui.security.UserGroup")?
            .eq("userGroupId", Value::string(&group))
            .one()?;
        if let Some(pattern) = row.and_then(|row| nonempty(text_field(&row, "ipAllowed"))) {
            allowed.push(pattern);
        }
    }
    if allowed.is_empty() || allowed.iter().any(|pattern| crate::web::ip_matches(pattern, client_ip)) {
        return Ok(());
    }
    Err(Error::new(format!(
        "Authenticate failed for user {} because client IP {client_ip} is not in allowed list for user or group.",
        text_field(account, "username")
    )))
}

/// The visit that began before login, or the one a session put off and starts now, belongs to the user.
fn link_visit(facade: &EntityFacade, user_id: &str) -> Result<(), Error> {
    session::start_deferred_visit()?;
    let Some(visit_id) = session::visit_id()?.filter(|id| !id.is_empty()) else {
        return Ok(());
    };
    let mut visit = facade.make_value("moqui.server.Visit")?;
    visit.set("visitId", Value::string(visit_id));
    visit.set("userId", Value::string(user_id));
    visit.update(facade)?;
    Ok(())
}

fn legacy_hash(account: &EntityValue) -> bool {
    !password::is_argon2(
        &text_field(account, "passwordHashType"),
        &text_field(account, "currentPassword"),
    )
}

fn needs_reset(account: &EntityValue) -> bool {
    field_i64(account, "successiveFailedLogins").unwrap_or(0) != 0
        || text_field(account, "disabled") == "Y"
        || field_millis(account, "disabledDateTime").is_some()
        || text_field(account, "hasLoggedOut") == "Y"
}

fn still_disabled(account: &EntityValue, now: i64) -> bool {
    if text_field(account, "disabled") != "Y" {
        return false;
    }
    let Some(disabled_at) = field_millis(account, "disabledDateTime") else {
        return true;
    };
    let window_ms = policy::disable_minutes().saturating_mul(60_000);
    disabled_at.saturating_add(window_ms) > now
}

fn terminated(account: &EntityValue, now: i64) -> bool {
    field_millis(account, "terminateDate").is_some_and(|term| now > term)
}

fn password_expired(account: &EntityValue, now: i64) -> bool {
    let Some(set_at) = field_millis(account, "passwordSetDate") else {
        return false;
    };
    let weeks = policy::change_weeks();
    if weeks <= 0 {
        return false;
    }
    now.saturating_sub(set_at) / WEEK_MILLIS > weeks
}

fn password_ok(account: &EntityValue, password: &str) -> Result<bool, Error> {
    password::passwords_match(
        password,
        &text_field(account, "currentPassword"),
        &text_field(account, "passwordHashType"),
        &text_field(account, "passwordSalt"),
        text_field(account, "passwordBase64") == "Y",
    )
}

fn reject_history(
    facade: &EntityFacade,
    account: &EntityValue,
    user_id: &str,
    new_password: &str,
) -> Result<(), Error> {
    let base64 = text_field(account, "passwordBase64") == "Y";
    let rows = facade
        .find(PASSWORD_HISTORY)?
        .eq("userId", Value::string(user_id))
        .list()?;
    for row in rows.iter() {
        let matched = password::passwords_match(
            new_password,
            &text_field(row, "password"),
            &text_field(row, "passwordHashType"),
            &text_field(row, "passwordSalt"),
            base64,
        )?;
        if matched {
            let limit = policy::history_limit();
            return Err(Error::new(format!("Password was used in last {limit} passwords")));
        }
    }
    Ok(())
}

fn remember_history(facade: &EntityFacade, account: &EntityValue, user_id: &str) -> Result<(), Error> {
    let mut row = facade.make_value(PASSWORD_HISTORY)?;
    row.set("userId", Value::string(user_id));
    row.set("fromDate", now_value());
    row.set("password", Value::string(text_field(account, "currentPassword")));
    row.set("passwordSalt", Value::string(text_field(account, "passwordSalt")));
    row.set(
        "passwordHashType",
        Value::string(text_field(account, "passwordHashType")),
    );
    row.create(facade)?;
    Ok(())
}

fn trim_history(facade: &EntityFacade, user_id: &str) -> Result<(), Error> {
    let rows = facade
        .find(PASSWORD_HISTORY)?
        .eq("userId", Value::string(user_id))
        .order_by("fromDate", false)
        .list()?;
    let limit = usize::try_from(policy::history_limit().max(0)).unwrap_or(0);
    let overflow = rows.len().saturating_sub(limit);
    for row in rows.iter().take(overflow) {
        row.delete(facade)?;
    }
    Ok(())
}

fn find_username(facade: &EntityFacade, username: &str) -> Result<Option<EntityValue>, Error> {
    Ok(facade
        .find(USER_ACCOUNT)?
        .eq("username", Value::string(username))
        .one()?)
}

fn find_user_id(facade: &EntityFacade, user_id: &str) -> Result<Option<EntityValue>, Error> {
    Ok(facade.find(USER_ACCOUNT)?.eq("userId", Value::string(user_id)).one()?)
}

fn update_user(facade: &EntityFacade, user_id: &str, fields: &[(&str, Value)]) -> Result<(), Error> {
    let mut value = facade.make_value(USER_ACCOUNT)?;
    value.set("userId", Value::string(user_id));
    for (name, field) in fields {
        value.set(*name, field.clone());
    }
    value.update(facade)?;
    Ok(())
}

fn mark_logged_out(facade: &EntityFacade, user_id: &str) -> Result<(), Error> {
    update_user(facade, user_id, &[("hasLoggedOut", Value::string("Y"))])
}

/// Failed-login count commits on a reserve connection, so the caller's rollback does not drop it.
///
/// The update only applies to the count that was read, and a lost race reads again, so parallel
/// guesses cannot count as one.
fn increment_failed(facade: &EntityFacade, user_id: &str) -> Result<(), Error> {
    let statements = facade.with_entity(USER_ACCOUNT, |entity, dialect, _| {
        let by_user = Condition::equals("userId", Value::string(user_id));
        let fields = [
            FieldRef::new("successiveFailedLogins"),
            FieldRef::new("disabled"),
            FieldRef::new("disabledDateTime"),
        ];
        let set = ["successiveFailedLogins", "disabled", "disabledDateTime"];
        let select_sql = sql_select(entity, dialect, &fields, Some(&by_user), &[], LimitOffset::default())?.sql;
        let counted = Condition::and(vec![
            by_user.clone(),
            Condition::equals("successiveFailedLogins", Value::I64(0)),
        ]);
        let uncounted = Condition::and(vec![by_user.clone(), Condition::is_null("successiveFailedLogins")]);
        Ok(IncrementSql {
            select: select_sql,
            update_counted: sql_update(entity, dialect, &set, Some(&counted))?.sql,
            update_uncounted: sql_update(entity, dialect, &set, Some(&uncounted))?.sql,
        })
    })?;
    let tx = facade.transactions_for(USER_ACCOUNT)?;
    let user_id = user_id.to_string();
    if crate::deterministic::enabled() {
        crate::entity::exec::with_reserve_tx(&tx, |conn| write_failed_increment(conn, &statements, &user_id))?;
    } else {
        tx.with_reserve(|conn| apply_failed_increment(conn, &statements, &user_id))?;
    }
    Ok(())
}

struct IncrementSql {
    select: String,
    update_counted: String,
    update_uncounted: String,
}

const INCREMENT_ATTEMPTS: usize = 8;

fn apply_failed_increment(
    conn: &mut dyn DbConnection,
    statements: &IncrementSql,
    user_id: &str,
) -> Result<(), moqui_lib::error::Error> {
    conn.begin()?;
    let outcome = write_failed_increment(conn, statements, user_id);
    match outcome {
        Ok(()) => conn.commit(),
        Err(err) => {
            let _ = conn.rollback();
            Err(err)
        }
    }
}

fn write_failed_increment(
    conn: &mut dyn DbConnection,
    statements: &IncrementSql,
    user_id: &str,
) -> Result<(), moqui_lib::error::Error> {
    let key = [Value::string(user_id)];
    for _ in 0..INCREMENT_ATTEMPTS {
        let rows = conn.query(&statements.select, &key)?;
        let Some(row) = rows.first() else {
            return Err(moqui_lib::error::Error::new(format!("no UserAccount {user_id}")));
        };
        let previous = row.get("successiveFailedLogins").filter(|value| !value.is_null());
        let next = row_i64(row, "successiveFailedLogins").saturating_add(1);
        let disable = next > policy::max_failures();
        let disabled = if disable {
            Value::string("Y")
        } else {
            row_value(row, "disabled")
        };
        let when = if disable {
            now_value()
        } else {
            row_value(row, "disabledDateTime")
        };
        let mut params = vec![Value::I64(next), disabled, when, Value::string(user_id)];
        let sql = match previous {
            Some(_) => {
                params.push(Value::I64(row_i64(row, "successiveFailedLogins")));
                &statements.update_counted
            }
            None => &statements.update_uncounted,
        };
        if conn.execute(sql, &params)? > 0 {
            return Ok(());
        }
    }
    Err(moqui_lib::error::Error::new(format!(
        "could not count a failed login for {user_id}: the account kept changing"
    )))
}

fn row_value(row: &DbRow, name: &str) -> Value {
    row.get(name).cloned().unwrap_or(Value::Null)
}

fn row_i64(row: &DbRow, name: &str) -> i64 {
    match row.get(name) {
        Some(Value::I64(number)) => *number,
        Some(Value::I32(number)) => i64::from(*number),
        _ => 0,
    }
}

fn text_field(value: &EntityValue, name: &str) -> String {
    value.get(name).and_then(Value::as_str).unwrap_or("").to_string()
}

fn field_i64(value: &EntityValue, name: &str) -> Option<i64> {
    value.get(name).and_then(value_i64)
}

fn field_millis(value: &EntityValue, name: &str) -> Option<i64> {
    value.get(name).and_then(value_millis)
}

fn value_i64(value: &Value) -> Option<i64> {
    match value {
        Value::I64(number) => Some(*number),
        Value::I32(number) => Some(i64::from(*number)),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

pub(crate) fn value_millis(value: &Value) -> Option<i64> {
    match value {
        Value::Timestamp(timestamp) => Some(timestamp.as_millisecond()),
        other => value_i64(other),
    }
}

fn nonempty(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use moqui_lib::object::{basic_convert, Value};

    use super::super::password::{passwords_match, simple_hash};
    use super::super::session;

    use super::*;
    use crate::context::ExecutionContextFactory;
    use crate::entity::EntityFacade;

    #[test]
    fn login_user_sets_current_username_and_logout_clears_it() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        session::add_flash_message("keep");
        assert_eq!(login_user("", "Secret1!").unwrap_err().to_string(), INVALID_LOGIN);
        assert_eq!(login_user("ada", "").unwrap_err().to_string(), INVALID_LOGIN);
        assert_eq!(
            login_user("missing", "Secret1!").unwrap_err().to_string(),
            INVALID_LOGIN
        );
        assert_eq!(session::flash_messages().unwrap(), vec!["keep".to_string()]);
        assert!(session::current_username().unwrap().is_none());

        login_anonymous().unwrap();
        assert!(session::current_username().unwrap().is_none());
        assert!(session::is_anonymous().unwrap());

        create_user(&facade, "ada", "Secret1!", |value| {
            value.set("locale", Value::string("en"));
            value.set("timeZone", Value::string("UTC"));
        });
        login_user("ada", "Secret1!").unwrap();
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
        assert!(!session::is_anonymous().unwrap());
        assert_eq!(session::locale().unwrap().as_deref(), Some("en"));
        assert_eq!(session::time_zone().unwrap().as_deref(), Some("UTC"));
        assert_eq!(session::csrf_token().unwrap().len(), 40);
        assert_eq!(session::flash_messages().unwrap(), vec!["keep".to_string()]);

        logout().unwrap();
        assert!(session::current_username().unwrap().is_none());
        assert!(!session::is_anonymous().unwrap());
        let row = load_user(&facade, "ada");
        assert_eq!(text(&row, "hasLoggedOut"), "Y");
        session::reset_session();
    }

    #[test]
    fn a_legacy_hash_verifies_and_the_stored_value_becomes_an_argon2_string() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        create_user(&facade, "ada", "Secret1!", |_| {});
        login_user("ada", "Secret1!").unwrap();
        let row = load_user(&facade, "ada");
        let stored = text(&row, "currentPassword");
        assert!(stored.starts_with("$argon2id$"), "{stored}");
        assert_eq!(text(&row, "passwordHashType"), "argon2");
        assert!(passwords_match("Secret1!", &stored, "argon2", "", false).unwrap());
        logout().unwrap();
        session::reset_session();
    }

    #[test]
    fn the_fourth_failed_login_disables_the_account_the_third_does_not_and_it_survives_rollback() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        session::add_flash_message("keep");
        create_user(&facade, "ada", "Secret1!", |_| {});
        for _ in 0..3 {
            assert_eq!(login_user("ada", "wrong").unwrap_err().to_string(), INVALID_LOGIN);
        }
        let before = load_user(&facade, "ada");
        assert_eq!(i64_field(&before, "successiveFailedLogins"), 3);
        assert_ne!(text(&before, "disabled"), "Y");

        facade.begin(USER_ACCOUNT, None).unwrap();
        let mut hinted = load_user(&facade, "ada");
        hinted.set("passwordHint", Value::string("changed"));
        hinted.update(&facade).unwrap();
        let failed = login_user("ada", "wrong");
        facade.rollback(USER_ACCOUNT).unwrap();
        assert_eq!(failed.unwrap_err().to_string(), INVALID_LOGIN);

        let after = load_user(&facade, "ada");
        assert_eq!(i64_field(&after, "successiveFailedLogins"), 4);
        assert_eq!(text(&after, "disabled"), "Y");
        assert!(!after.get("disabledDateTime").is_none_or(Value::is_null));
        assert_eq!(text(&after, "passwordHint"), "original");
        assert!(session::current_username().unwrap().is_none());
        assert_eq!(session::flash_messages().unwrap(), vec!["keep".to_string()]);
        session::reset_session();
    }

    #[test]
    fn disabled_without_a_timestamp_stays_disabled_and_terminate_does_not_increment() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        create_user(&facade, "stuck", "Secret1!", |value| {
            value.set("disabled", Value::string("Y"));
        });
        assert_eq!(login_user("stuck", "Secret1!").unwrap_err().to_string(), INVALID_LOGIN);
        let stuck = load_user(&facade, "stuck");
        assert_eq!(text(&stuck, "disabled"), "Y");
        assert_eq!(i64_field(&stuck, "successiveFailedLogins"), 1);
        assert!(!text(&stuck, "currentPassword").starts_with("$argon2"));

        create_user(&facade, "gone", "Secret1!", |value| {
            value.set("terminateDate", stamp(1));
        });
        assert_eq!(login_user("gone", "Secret1!").unwrap_err().to_string(), INVALID_LOGIN);
        let gone = load_user(&facade, "gone");
        assert_eq!(i64_field(&gone, "successiveFailedLogins"), 0);
        assert!(!text(&gone, "currentPassword").starts_with("$argon2"));
        session::reset_session();
    }

    #[test]
    fn a_correct_password_logs_in_after_the_disable_window() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        let opened = now_millis() - 6 * 60_000;
        create_user(&facade, "ada", "Secret1!", |value| {
            value.set("disabled", Value::string("Y"));
            value.set("disabledDateTime", stamp(opened));
            value.set("successiveFailedLogins", Value::I64(4));
        });
        login_user("ada", "Secret1!").unwrap();
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
        let row = load_user(&facade, "ada");
        assert_eq!(text(&row, "disabled"), "N");
        assert!(row.get("disabledDateTime").is_none_or(Value::is_null));
        assert_eq!(i64_field(&row, "successiveFailedLogins"), 0);
        assert!(text(&row, "currentPassword").starts_with("$argon2id$"));
        logout().unwrap();
        session::reset_session();
    }

    #[test]
    fn require_password_change_and_an_old_password_fail_without_rehash() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        create_user(&facade, "must", "Secret1!", |value| {
            value.set("requirePasswordChange", Value::string("Y"));
        });
        let err = login_user("must", "Secret1!").unwrap_err();
        assert!(err.to_string().contains("password change"), "{err}");
        assert_eq!(i64_field(&load_user(&facade, "must"), "successiveFailedLogins"), 0);
        assert!(!text(&load_user(&facade, "must"), "currentPassword").starts_with("$argon2"));

        let old = now_millis() - 13 * WEEK_MILLIS;
        create_user(&facade, "stale", "Secret1!", |value| {
            value.set("passwordSetDate", stamp(old));
        });
        let err = login_user("stale", "Secret1!").unwrap_err();
        assert!(err.to_string().contains("must be changed"), "{err}");
        assert!(!text(&load_user(&facade, "stale"), "currentPassword").starts_with("$argon2"));
        assert!(session::current_username().unwrap().is_none());
        session::reset_session();
    }

    #[test]
    fn a_login_once_the_account_is_known_leaves_one_history_row_a_minute() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        let history = |user: &str| {
            facade
                .find(USER_LOGIN_HISTORY)
                .unwrap()
                .eq("userId", Value::string(user))
                .list()
                .unwrap()
        };
        create_user(&facade, "ada", "Secret1!", |_| {});
        assert_eq!(login_user("ada", "wrong").unwrap_err().to_string(), INVALID_LOGIN);
        let rows = history("ada");
        let row = rows.iter().next().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(text(row, "successfulLogin"), "N");
        assert_eq!(text(row, "passwordUsed"), "wrong");
        // A second login within the minute is not kept.
        login_user("ada", "Secret1!").unwrap();
        assert_eq!(history("ada").len(), 1);
        logout().unwrap();

        create_user(&facade, "bob", "Secret1!", |_| {});
        login_user("bob", "Secret1!").unwrap();
        let rows = history("bob");
        let row = rows.iter().next().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(text(row, "successfulLogin"), "Y");
        assert!(row.get("passwordUsed").is_none_or(Value::is_null));
        logout().unwrap();

        // Java has no userId for an unknown or disabled account, so it keeps nothing.
        create_user(&facade, "stuck", "Secret1!", |value| {
            value.set("disabled", Value::string("Y"));
        });
        assert!(login_user("stuck", "Secret1!").is_err());
        assert!(login_user("nobody", "Secret1!").is_err());
        assert!(history("stuck").is_empty());
        assert!(history("nobody").is_empty());
        session::reset_session();
    }

    #[test]
    fn a_password_below_the_current_policy_still_logs_in() {
        // The policy applies when a password is set. Java does not check it at login, so a user
        // whose password predates the policy is not locked out.
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        create_user(&facade, "ada", "secret", |_| {});
        login_user("ada", "secret").unwrap();
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
        assert!(text(&load_user(&facade, "ada"), "currentPassword").starts_with("$argon2"));
        session::reset_session();
    }

    #[test]
    fn internal_login_skips_the_password_and_anonymous_does_not_set_a_username() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        create_user(&facade, "ada", "Secret1!", |_| {});
        internal_login_user("ada").unwrap();
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
        assert!(!text(&load_user(&facade, "ada"), "currentPassword").starts_with("$argon2"));
        login_anonymous().unwrap();
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
        logout().unwrap();
        login_anonymous().unwrap();
        assert!(session::current_username().unwrap().is_none());
        assert!(session::is_anonymous().unwrap());
        assert_eq!(
            internal_login_user("").unwrap_err().to_string(),
            "No username specified"
        );
        session::reset_session();
    }

    #[test]
    fn change_password_stores_argon2_and_rejects_history() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        create_user(&facade, "ada", "Secret1!", |_| {});
        for millis in 1..=5 {
            let mut row = facade.make_value(PASSWORD_HISTORY).unwrap();
            row.set("userId", Value::string("ada"));
            row.set("fromDate", stamp(millis));
            row.set("password", Value::string("not-a-real-hash"));
            row.set("passwordSalt", Value::string("s4lt"));
            row.set("passwordHashType", Value::string("SHA-256"));
            row.create(&facade).unwrap();
        }
        change_password("ada", "Secret2!").unwrap();
        let account = load_user(&facade, "ada");
        let stored = text(&account, "currentPassword");
        assert!(stored.starts_with("$argon2id$"), "{stored}");
        assert_eq!(text(&account, "passwordHashType"), "argon2");
        let history = facade
            .find(PASSWORD_HISTORY)
            .unwrap()
            .eq("userId", Value::string("ada"))
            .order_by("fromDate", false)
            .list()
            .unwrap();
        assert_eq!(history.len(), 5);
        assert!(history.iter().all(|row| field_millis(row, "fromDate") != Some(1)));
        let current = change_password("ada", "Secret2!").unwrap_err();
        assert!(current.to_string().contains("current"), "{current}");
        let reused = change_password("ada", "Secret1!").unwrap_err();
        assert!(reused.to_string().contains("last"), "{reused}");
        session::reset_session();
    }

    #[test]
    fn a_user_with_a_factor_or_a_requiring_group_is_pre_authenticated_not_logged_in() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        create_user(&facade, "ada", "Secret1!", |_| {});
        let mut factor = facade.make_value("moqui.security.UserAuthcFactor").unwrap();
        factor.set("factorId", Value::string("ADA_TOTP"));
        factor.set("userId", Value::string("ada"));
        factor.set("factorTypeEnumId", Value::string("UafTotp"));
        factor.set("factorOption", Value::string(super::super::factor::totp_secret()));
        factor.set("fromDate", stamp(1));
        factor.create(&facade).unwrap();
        login_user("ada", "Secret1!").unwrap();
        assert!(session::current_username().unwrap().is_none());
        assert_eq!(session::session_attribute(PRE_AUTHC_USERNAME).as_deref(), Some("ada"));
        assert!(session::session_attribute_flag(AUTHC_FACTOR_REQUIRED));
        // The password was right, so a legacy hash is replaced as on a full login.
        assert!(text(&load_user(&facade, "ada"), "currentPassword").starts_with("$argon2id$"));
        // A wrong password does not pre-authenticate, and clears the earlier attempt.
        assert_eq!(login_user("ada", "wrong").unwrap_err().to_string(), INVALID_LOGIN);
        assert_eq!(session::session_attribute(PRE_AUTHC_USERNAME), None);
        // The factor is the second step: the internal login after a verified code sets the user.
        internal_login_user("ada").unwrap();
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
        logout().unwrap();

        create_user(&facade, "bob", "Secret1!", |_| {});
        let mut group = facade.make_value("moqui.security.UserGroup").unwrap();
        group.set("userGroupId", Value::string("STRICT"));
        group.set("requireAuthcFactor", Value::string("Y"));
        group.create(&facade).unwrap();
        assert!(!super::super::factor::second_factor_required("bob").unwrap());
        let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
        member.set("userGroupId", Value::string("STRICT"));
        member.set("userId", Value::string("bob"));
        member.set("fromDate", stamp(1));
        member.create(&facade).unwrap();
        assert!(super::super::factor::second_factor_required("bob").unwrap());
        login_user("bob", "Secret1!").unwrap();
        assert!(session::current_username().unwrap().is_none());
        assert_eq!(session::session_attribute(PRE_AUTHC_USERNAME).as_deref(), Some("bob"));
        session::reset_session();
    }

    #[test]
    fn ip_allowed_on_the_account_or_a_group_refuses_another_client_ip() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        let _web = crate::web::install(std::collections::BTreeMap::new());
        let from = |address: &str| {
            let mut request = crate::web::WebRequest::new("POST", "/Login/login");
            request.peer_ip = address.to_string();
            crate::web::set_request_info(&request);
        };
        create_user(&facade, "ada", "Secret1!", |value| {
            value.set("ipAllowed", Value::string("10.1.*.*"));
        });
        from("10.1.2.3");
        login_user("ada", "Secret1!").unwrap();
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
        logout().unwrap();
        from("192.168.0.4");
        let refused = login_user("ada", "Secret1!").unwrap_err().to_string();
        assert!(refused.contains("not in allowed list"), "{refused}");
        assert!(session::current_username().unwrap().is_none());
        // Not a failed password: the count is not raised.
        assert_eq!(i64_field(&load_user(&facade, "ada"), "successiveFailedLogins"), 0);
        // The internal login after a second-factor code is checked as well.
        assert!(internal_login_user("ada").is_err());

        // A group the user is in adds its list; any one matching allows.
        create_user(&facade, "bob", "Secret1!", |_| {});
        let mut group = facade.make_value("moqui.security.UserGroup").unwrap();
        group.set("userGroupId", Value::string("OFFICE"));
        group.set("ipAllowed", Value::string("192.168.0.1-5"));
        group.create(&facade).unwrap();
        let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
        member.set("userGroupId", Value::string("OFFICE"));
        member.set("userId", Value::string("bob"));
        member.set("fromDate", stamp(1));
        member.create(&facade).unwrap();
        login_user("bob", "Secret1!").unwrap();
        logout().unwrap();
        from("192.168.0.9");
        assert!(login_user("bob", "Secret1!").is_err());
        // The account's own list matches although the group's does not.
        let mut bob = load_user(&facade, "bob");
        bob.set("ipAllowed", Value::string("192.168.0.9"));
        bob.update(&facade).unwrap();
        login_user("bob", "Secret1!").unwrap();
        logout().unwrap();
        session::reset_session();
    }

    #[test]
    fn an_api_key_logs_in_until_it_is_revoked_or_expires() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        assert!(get_login_key(None).is_err(), "a key needs a logged-in user");
        create_user(&facade, "ada", "Secret1!", |_| {});
        internal_login_user("ada").unwrap();
        let key = get_login_key(None).unwrap();
        assert_eq!(key.len(), 40);
        // A key that ends now is kept, and is expired a moment later.
        let short = get_login_key(Some(0.0)).unwrap();
        logout().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));

        let stored = facade
            .find(USER_LOGIN_KEY)
            .unwrap()
            .eq("userId", Value::string("ada"))
            .list()
            .unwrap();
        assert_eq!(stored.len(), 2);
        let hashed = password::hash_login_key(&key).unwrap();
        let row = stored
            .iter()
            .find(|row| text(row, "loginKey") == hashed)
            .expect("only the hash is stored");
        let thru = field_millis(row, "thruDate").unwrap() - field_millis(row, "fromDate").unwrap();
        assert_eq!(thru, 144 * 60 * 60 * 1000, "the conf default is six days");

        apply_header_keys(Some(&key), None).unwrap();
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
        logout().unwrap();
        assert_eq!(login_user_key(&short).unwrap_err().to_string(), INVALID_LOGIN);

        // Revoke: the admin deletes the key's row.
        row.delete(&facade).unwrap();
        assert_eq!(
            apply_header_keys(Some(&key), None).unwrap_err().to_string(),
            INVALID_LOGIN
        );
        assert!(session::current_username().unwrap().is_none());
        session::reset_session();
    }

    #[test]
    fn a_service_call_with_credentials_puts_the_callers_user_back() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        create_user(&facade, "ada", "Secret1!", |value| {
            value.set("locale", Value::string("en"));
        });
        create_user(&facade, "bob", "Secret2!", |value| {
            value.set("locale", Value::string("fr"));
        });
        let path = std::env::temp_dir().join(format!("moqui-login-local-{}.xml", std::process::id()));
        std::fs::write(
            &path,
            r#"<services>
                <service verb="who" noun="Am" authenticate="true" transaction="ignore">
                    <out-parameters><parameter name="who"/></out-parameters>
                    <actions><set field="who" from="ec.user.username"/></actions>
                </service>
            </services>"#,
        )
        .unwrap();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        login_user("ada", "Secret1!").unwrap();
        let token = session::csrf_token().unwrap();
        let mut parameters = std::collections::BTreeMap::new();
        parameters.insert("authUsername".to_string(), Value::string("bob"));
        parameters.insert("authPassword".to_string(), Value::string("Secret2!"));
        let out = crate::service::sync("who#Am", &parameters).unwrap();
        assert_eq!(out.get("who").and_then(Value::as_str), Some("bob"));
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
        assert_eq!(session::current_user_id().unwrap().as_deref(), Some("ada"));
        assert_eq!(session::locale().unwrap().as_deref(), Some("en"));
        assert_eq!(session::csrf_token().unwrap(), token);

        parameters.insert("authPassword".to_string(), Value::string("wrong"));
        assert!(crate::service::sync("who#Am", &parameters).unwrap().is_empty());
        assert_eq!(session::current_username().unwrap().as_deref(), Some("ada"));
    }

    fn open_ctx() -> (crate::context::ExecutionContext, EntityFacade) {
        session::reset_session();
        let ctx = ExecutionContextFactory::test_context();
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        (ctx, facade)
    }

    fn create_user(facade: &EntityFacade, username: &str, password: &str, fill: impl FnOnce(&mut EntityValue)) {
        let stored = simple_hash(password, "s4lt", "SHA-256", false).unwrap();
        let mut value = facade.make_value(USER_ACCOUNT).unwrap();
        value.set("userId", Value::string(username));
        value.set("username", Value::string(username));
        value.set("currentPassword", Value::string(stored));
        value.set("passwordSalt", Value::string("s4lt"));
        value.set("passwordHashType", Value::string("SHA-256"));
        value.set("passwordBase64", Value::string("N"));
        value.set("passwordHint", Value::string("original"));
        fill(&mut value);
        value.create(facade).unwrap();
    }

    fn load_user(facade: &EntityFacade, username: &str) -> EntityValue {
        facade
            .find(USER_ACCOUNT)
            .unwrap()
            .eq("username", Value::string(username))
            .one()
            .unwrap()
            .unwrap_or_else(|| panic!("missing {username}"))
    }

    fn text(value: &EntityValue, field: &str) -> String {
        value.get(field).and_then(Value::as_str).unwrap_or("").to_string()
    }

    fn i64_field(value: &EntityValue, field: &str) -> i64 {
        match value.get(field) {
            Some(Value::I64(number)) => *number,
            Some(Value::I32(number)) => i64::from(*number),
            _ => 0,
        }
    }

    fn stamp(millis: i64) -> Value {
        basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
    }

    fn now_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_millis()).ok())
            .unwrap_or(0)
    }
}
