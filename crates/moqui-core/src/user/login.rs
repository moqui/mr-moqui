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

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
const USER_APP_PASSWORD: &str = "moqui.security.UserAppPassword";
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
/// A bound web session rotates its id only after this password check succeeds.
///
/// A user who needs a second factor is not logged in. The password was right, so the session
/// rotates and remembers the user in `moquiPreAuthcUsername` with `moquiAuthcFactorRequired`,
/// and this returns `Ok` with no current user, as Java's `loginUser` returns false without an
/// error. The code then comes through `validate#ExternalUserAuthcCode` and [`internal_login_user`].
pub fn login_user(username: &str, password: &str) -> Result<(), Error> {
    if password_login(username, password)? {
        session::rotate_on_password_login()?;
    }
    // a password login of its own (the login form) is a person's; a header login marks the session again after it
    session::set_session_attribute(HEADER_LOGIN, None);
    Ok(())
}

/// The session attribute that marks a session whose user came by a request header (Basic, `api_key`, `login_key`,
/// `authUsername`) rather than the login form: the user id. A program holding a key gets such a session and its token
/// from one request; it is never a person deciding an agent's approval ([`header_login_session`]).
pub(crate) const HEADER_LOGIN: &str = "moquiHeaderLogin";

/// Mark the bound session as one whose user came by a request header ([`HEADER_LOGIN`]).
pub fn note_header_login() {
    let user_id = crate::user::current_user_id().ok().flatten().unwrap_or_default();
    session::set_session_attribute(HEADER_LOGIN, Some(&user_id));
}

/// Whether the bound session's user came by a request header, not the login form: such a session never decides an
/// agent's approval, whatever cookie and token it later presents.
pub fn header_login_session() -> bool {
    session::session_attribute(HEADER_LOGIN).is_some_and(|user_id| !user_id.is_empty())
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

/// The password check and login of [`login_user`]. True when the session id should rotate: the password was right
/// and the session did not already hold this user. Credentials that are not valid are noted for the web protection.
fn password_login(username: &str, password: &str) -> Result<bool, Error> {
    // A blank form tried no password, so it is not a failure web protection counts.
    if username.is_empty() || password.is_empty() {
        clear_pre_authc();
        return Err(Error::new(INVALID_LOGIN));
    }
    noting_invalid(check_password_login(username, password))
}

/// `result`, noted with [`crate::web::note_auth_failure`] when it is the invalid login error.
fn noting_invalid<T>(result: Result<T, Error>) -> Result<T, Error> {
    if result.as_ref().is_err_and(|err| err.message() == INVALID_LOGIN) {
        crate::web::note_auth_failure();
    }
    result
}

fn check_password_login(username: &str, password: &str) -> Result<bool, Error> {
    clear_pre_authc();
    if username.is_empty() || password.is_empty() {
        return Err(Error::new(INVALID_LOGIN));
    }
    // The login reads and writes the user's own account whatever the caller may do.
    let _authz = without_authz();
    let facade = entity_facade()?;
    let now = now_millis();
    // No more of the user's logins are checked at once than the failures the account has left, so concurrent guesses
    // cannot all pass the gate before the first failure is counted. Another waits for one to end and reads the account
    // again. One at a time, the logins checked are Java's.
    let (account, _checking) = loop {
        let Some(account) = find_username(&facade, username)? else {
            password::burn_verify(password);
            return Err(Error::new(INVALID_LOGIN));
        };
        if let Err(err) = gate_account(&facade, &account, now) {
            password::burn_verify(password);
            return Err(err);
        }
        let left = policy::max_failures() + 1 - field_i64(&account, "successiveFailedLogins").unwrap_or(0);
        let user_id = text_field(&account, "userId");
        if let Some(checking) = LoginCheck::enter(&user_id, left) {
            break (account, checking);
        }
        LoginCheck::wait(&user_id);
    };
    // A legacy hash checks in microseconds; the argon2 burn keeps it as slow as an unknown user, so
    // the time does not tell which usernames exist.
    if legacy_hash(&account) {
        password::burn_verify(password);
    }
    if !password_ok(&account, password)? {
        let user_id = text_field(&account, "userId");
        if !user_id.is_empty() {
            increment_failed(&facade, &user_id)?;
        }
        return Err(Error::new(INVALID_LOGIN));
    }
    post_password(&account, now)?;
    let user_id = text_field(&account, "userId");
    if !user_id.is_empty() && super::factor::second_factor_required(&user_id)? {
        rehash_legacy(&facade, &account, password)?;
        let account_username = text_field(&account, "username");
        session::set_session_attribute(PRE_AUTHC_USERNAME, Some(&account_username));
        session::set_session_attribute(AUTHC_FACTOR_REQUIRED, Some("true"));
        return Ok(true);
    }
    // A request repeating Basic credentials for the user its session already holds keeps its id.
    let same_user = session::current_username()?.as_deref() == Some(username);
    finish_login(&facade, &account, Some(password), true)?;
    Ok(!same_user)
}

/// The password checks of each user running on this factory, and a signal when one ends. A Rust-only bound on concurrent
/// guesses: Java counts a failure only after the check, so every attempt that passed the gate before it is checked.
#[derive(Default)]
struct LoginsChecking {
    running: Mutex<HashMap<String, i64>>,
    ended: Condvar,
}

/// One password check of a user in [`LoginsChecking`], until it is dropped.
struct LoginCheck {
    checking: Arc<LoginsChecking>,
    user_id: String,
}

impl LoginCheck {
    fn checking() -> Arc<LoginsChecking> {
        ExecutionContext::with_current(|ctx| ctx.factory().extension(|_| LoginsChecking::default())).unwrap_or_default()
    }

    /// A check of `user_id`, or `None` when `left` (at least one) of them already run. An empty user id is not counted.
    fn enter(user_id: &str, left: i64) -> Option<Self> {
        let checking = Self::checking();
        if !user_id.is_empty() {
            let mut running = checking.running.lock().unwrap_or_else(|err| err.into_inner());
            let count = running.entry(user_id.to_string()).or_insert(0);
            if *count >= left.max(1) {
                return None;
            }
            *count += 1;
        }
        Some(Self {
            checking,
            user_id: user_id.to_string(),
        })
    }

    /// Wait until a check of `user_id` ends, or a second passes.
    fn wait(user_id: &str) {
        let checking = Self::checking();
        let running = checking.running.lock().unwrap_or_else(|err| err.into_inner());
        if running.get(user_id).is_some_and(|count| *count > 0) {
            drop(checking.ended.wait_timeout(running, Duration::from_secs(1)));
        }
    }
}

impl Drop for LoginCheck {
    fn drop(&mut self) {
        if self.user_id.is_empty() {
            return;
        }
        let mut running = self.checking.running.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(count) = running.get_mut(&self.user_id) {
            *count -= 1;
            if *count <= 0 {
                running.remove(&self.user_id);
            }
        }
        drop(running);
        self.checking.ended.notify_all();
    }
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
    noting_invalid(check_login_key(key))
}

fn check_login_key(key: &str) -> Result<(), Error> {
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

/// Log in with an app password (`UserAppPassword`), the password of one device's DAV or SFTP client, which the web login
/// does not take. The stored `passwordHash` is [`super::password::hash_login_key`] of the password without its dashes and
/// spaces, in lower case; the row must not be revoked and must be the user's whose username is given. The account's own
/// checks then apply ([`internal_login_user`]), and `lastUsedDate` is written at most once a minute, in a transaction of
/// its own. A refusal is noted for the web protection; it is not an account failure, so guesses cannot lock the
/// account.
///
/// # Errors
///
/// Returns [`INVALID_LOGIN`](Error) when the password is empty, unknown, revoked or another user's, or the account
/// cannot log in.
pub fn login_app_password(username: &str, password: &str) -> Result<(), Error> {
    noting_invalid(check_app_password(username, password))
}

/// Whether `password` has the form of an app password ([`new_app_password`]): 20 of its letters and digits once the
/// dashes and spaces are gone. A protocol server tries such a password as an app password before the account password.
pub fn looks_like_app_password(password: &str) -> bool {
    let key = app_password_key(password);
    key.len() == 20
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// The form an app password is compared in: no dashes or spaces, lower case.
pub(crate) fn app_password_key(password: &str) -> String {
    password
        .chars()
        .filter(|ch| *ch != '-' && !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn check_app_password(username: &str, password: &str) -> Result<(), Error> {
    let key = app_password_key(password);
    if username.is_empty() || key.is_empty() {
        return Err(Error::new(INVALID_LOGIN));
    }
    let hashed = password::hash_login_key(&key)?;
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(row) = facade
        .find(USER_APP_PASSWORD)?
        .eq("passwordHash", Value::string(&hashed))
        .one()?
    else {
        return Err(Error::new(INVALID_LOGIN));
    };
    if field_millis(&row, "revokedDate").is_some() {
        return Err(Error::new(INVALID_LOGIN));
    }
    let user_id = text_field(&row, "userId");
    let Some(account) = find_user_id(&facade, &user_id)? else {
        return Err(Error::new(INVALID_LOGIN));
    };
    let account_username = text_field(&account, "username");
    if !account_username.eq_ignore_ascii_case(username.trim()) {
        return Err(Error::new(INVALID_LOGIN));
    }
    internal_login_user(&account_username)?;
    let now = now_millis();
    if field_millis(&row, "lastUsedDate").is_none_or(|used| now - used > 60_000) {
        let mut used = facade.make_value(USER_APP_PASSWORD)?;
        used.set("appPasswordId", Value::string(text_field(&row, "appPasswordId")));
        used.set("lastUsedDate", stamp_value(now));
        let tx = facade.transactions_for(USER_APP_PASSWORD)?;
        // the login stands whatever happens to the note of its use
        if let Err(err) = tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
            used.update(&facade)
        }) {
            tracing::warn!(
                "app password {}: last use not written: {err}",
                text_field(&row, "appPasswordId")
            );
        }
    }
    Ok(())
}

/// A new app password: 20 letters and digits that are not easily mistaken for each other (about 99 bits), in groups of
/// four with dashes, as a device's settings screen shows it.
///
/// # Errors
///
/// Returns an error when the system random source fails.
pub fn new_app_password() -> Result<String, Error> {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut out = String::with_capacity(24);
    let mut count = 0;
    while count < 20 {
        let mut byte = [0_u8; 1];
        getrandom::fill(&mut byte).map_err(|err| Error::new(format!("random source failed: {err}")))?;
        // 248 is the largest multiple of 31 under 256, so every symbol is equally likely
        if byte[0] >= 248 {
            continue;
        }
        if count > 0 && count % 4 == 0 {
            out.push('-');
        }
        out.push(ALPHABET[usize::from(byte[0]) % ALPHABET.len()] as char);
        count += 1;
    }
    Ok(out)
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
    let login_key = super::factor::random_string(40)?;
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
    // As Java's `logoutLocal`, a web logout runs the webapp's before-logout actions while the user is still there.
    if crate::web::current_request().is_some() {
        if let Some(factory) = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) {
            factory
                .webapp_scripts()
                .run_actions_logged("before-logout", &Default::default());
        }
    }
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
    forget_shell_providers();
    update.and(session::flush_after_logout())
}

/// A login or logout changes what the session's user may open: the shell providers kept for it go.
fn forget_shell_providers() {
    let visit = session::visit_id().ok().flatten();
    ExecutionContext::with_current(|ec| crate::screen::shell::forget_shell_providers(ec.factory(), visit.as_deref()));
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
    ExecutionContext::with_current(ExecutionContext::effective_time)
        .flatten()
        .unwrap_or_else(now_millis)
}

pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
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
    check_ip_allowed(account, &client_ip())?;
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
    forget_shell_providers();
    let locale = nonempty(text_field(account, "locale"));
    let zone = nonempty(text_field(account, "timeZone"));
    session::set_locale(locale.as_deref());
    session::set_time_zone(zone.as_deref());
    link_visit(facade, &user_id)?;
    // As Java's `UserFacadeImpl`, a web login runs the webapp's after-login actions.
    if crate::web::current_request().is_some() {
        if let Some(factory) = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) {
            factory
                .webapp_scripts()
                .run_actions_logged("after-login", &Default::default());
        }
    }
    Ok(())
}

thread_local! {
    /// The address of a login outside an HTTP request (SSH, the terminal client's dialog): [`with_client_ip`].
    static LOGIN_CLIENT_IP: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Run `work` with `client_ip` as the address a login in it checks `ipAllowed` against, for a login with no HTTP
/// request (an SSH password, the terminal client's login dialog). A request's own address comes first.
pub fn with_client_ip<T>(client_ip: &str, work: impl FnOnce() -> T) -> T {
    let before = LOGIN_CLIENT_IP.with(|slot| slot.replace(Some(client_ip.to_string())));
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let before = self.0.take();
            LOGIN_CLIENT_IP.with(|slot| *slot.borrow_mut() = before);
        }
    }
    let _restore = Restore(before);
    work()
}

/// The request's address, else the one [`with_client_ip`] gave, else empty.
fn client_ip() -> String {
    let web = crate::web::current_client_ip();
    if !web.is_empty() {
        return web;
    }
    LOGIN_CLIENT_IP.with(|slot| slot.borrow().clone()).unwrap_or_default()
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
    tx.with_reserve(|conn| apply_failed_increment(conn, &statements, &user_id))?;
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

/// A count read straight from the driver: Postgres gives a `NUMERIC(20,0)` column as a decimal.
fn row_i64(row: &DbRow, name: &str) -> i64 {
    row.get(name).and_then(value_i64).unwrap_or(0)
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
        Value::Decimal(number) if number.is_integer() => i64::try_from(*number).ok(),
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
    /// No more of a user's password checks run at once than the account has failures left, and a check that ends
    /// frees its place; other users are not held by it.
    #[test]
    fn concurrent_checks_of_one_user_are_bounded_by_the_failures_left() {
        let ctx = ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        let first = LoginCheck::enter("ada", 2).expect("first");
        let second = LoginCheck::enter("ada", 2).expect("second");
        assert!(LoginCheck::enter("ada", 2).is_none());
        assert!(LoginCheck::enter("bob", 2).is_some());
        drop(first);
        let third = LoginCheck::enter("ada", 2).expect("a place came free");
        drop((second, third));
        // An account past its failures still gets one check, and an empty user id is not held.
        let one = LoginCheck::enter("ada", -3).expect("one");
        assert!(LoginCheck::enter("ada", -3).is_none());
        drop(one);
        assert!(LoginCheck::enter("", 0).is_some() && LoginCheck::enter("", 0).is_some());

        // A check past the bound waits for one to end, here on another thread.
        let held = LoginCheck::enter("ada", 1).expect("held");
        let factory = Arc::clone(ctx.factory());
        let started = std::time::Instant::now();
        let ender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(held);
            drop(factory);
        });
        LoginCheck::wait("ada");
        assert!(
            started.elapsed() < Duration::from_millis(900),
            "{:?}",
            started.elapsed()
        );
        assert!(LoginCheck::enter("ada", 1).is_some());
        ender.join().unwrap();
    }

    /// Concurrent logins of one user with the right password all succeed: those past the bound wait their turn.
    #[test]
    fn concurrent_logins_of_one_user_all_succeed() {
        let (ctx, facade) = open_ctx();
        create_user(&facade, "par", "Secret1!", |_| {});
        let factory = Arc::clone(ctx.factory());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let factory = Arc::clone(&factory);
                std::thread::spawn(move || {
                    let context = factory.new_context();
                    let _entered = context.enter();
                    login_user("par", "Secret1!").is_ok()
                })
            })
            .collect();
        for thread in threads {
            assert!(thread.join().unwrap());
        }
    }

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
    fn a_count_the_driver_gives_as_a_decimal_is_its_number() {
        // Postgres reads a NUMERIC(20,0) column, such as successiveFailedLogins, as a decimal
        assert_eq!(value_i64(&Value::Decimal("3".parse().unwrap())), Some(3));
        assert_eq!(value_i64(&Value::Decimal("3.5".parse().unwrap())), None);
        assert_eq!(value_i64(&Value::I32(2)), Some(2));
    }

    #[test]
    fn the_fourth_failed_login_disables_the_account_the_third_does_not_and_it_survives_rollback() {
        let (ctx, facade) = open_ctx();
        let _guard = ctx.enter();
        session::add_flash_message("keep");
        create_user(&facade, "ada", "Secret1!", |_| {});
        crate::web::take_auth_failure();
        for _ in 0..3 {
            assert_eq!(login_user("ada", "wrong").unwrap_err().to_string(), INVALID_LOGIN);
            // noted for the web protection, once taken
            assert!(crate::web::take_auth_failure());
            assert!(!crate::web::take_auth_failure());
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
