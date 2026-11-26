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

//! Login against `moqui.security.UserAccount`. A bound web session is renewed at a password login, before the check.

use std::collections::{BTreeMap, HashMap};
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
const USER_AUTHC_FACTOR: &str = "moqui.security.UserAuthcFactor";
const USER_LOGIN_KEY: &str = "moqui.security.UserLoginKey";
const USER_APP_PASSWORD: &str = "moqui.security.UserAppPassword";
/// The protocols an app password may be made for (`UserAppPassword.scope`, SEC-COM-21): DAV; SSH, SFTP and git over
/// SSH; git over HTTP; XMPP; the LLM gateway, A2A and MCP. A row with no scope, made before there were scopes, is taken
/// by each of them.
pub const APP_PASSWORD_SCOPES: [&str; 5] = ["dav", "ssh", "git", "xmpp", "ai"];
/// The scope of an XMPP sign-in ([`login_verified_app_password`]).
const XMPP_SCOPE: &str = "xmpp";

/// Whether an app password's row may be used where `scope` is asked (none: anywhere, for a check before the login that
/// asks its own): a row with no scope is taken everywhere.
fn scope_allows(row: &EntityValue, scope: Option<&str>) -> bool {
    let own = text_field(row, "scope");
    own.is_empty() || scope.is_none_or(|scope| own == scope)
}
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

/// The invalid login error for a credential that is not valid (an unknown, ended or another user's key or app
/// password), noted for the web protection. A valid key or app password the account's gate then refuses (a lock
/// window) tried nothing wrong and is not noted (SEC-WEB-03).
fn refused_credential() -> Error {
    crate::web::note_auth_failure();
    Error::new(INVALID_LOGIN)
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
    let (account, _checking) = gate_password(&facade, password, now, || find_username(&facade, username))?;
    // a display account has no sign-in of its own, whatever password someone gave it (user/device.rs)
    if is_display_account(&account) {
        password::burn_verify(password);
        return Err(Error::new(INVALID_LOGIN));
    }
    // an address outside the account's or its groups' ipAllowed is refused before the password is compared, with the
    // invalid login error, so a client there cannot tell a right password from a wrong one (SEC-WEB-90)
    if let Err(err) = check_ip_allowed(&account, &client_ip()) {
        password::burn_verify(password);
        save_history(&text_field(&account, "userId"), false);
        return Err(err);
    }
    // Once the account passes the gate every outcome is kept in the login history, a pending second factor as one
    // that did not log in.
    let outcome = authenticate(&facade, &account, username, password, now);
    save_history(
        &text_field(&account, "userId"),
        matches!(outcome, Ok(Checked::LoggedIn { .. })),
    );
    outcome.map(|checked| match checked {
        Checked::SecondFactor => true,
        Checked::LoggedIn { rotate } => rotate,
    })
}

/// The account gate in front of a password compare: the account `find` reads, when it is there and [`gate_account`]
/// passes it, with its place among the checks of that user running now. An unknown or refused account takes an argon2
/// verify, so the time does not tell it from a wrong password; a refusal during a lock window counts as a failed login.
/// No more of the user's checks run at once than the failures the account has left, so concurrent guesses cannot all
/// pass the gate before the first failure is counted. Another waits for one to end and reads the account again. One at
/// a time, the checks are Java's.
fn gate_password(
    facade: &EntityFacade,
    password: &str,
    now: i64,
    find: impl Fn() -> Result<Option<EntityValue>, Error>,
) -> Result<(EntityValue, LoginCheck), Error> {
    loop {
        let Some(account) = find()? else {
            password::burn_verify(password);
            return Err(Error::new(INVALID_LOGIN));
        };
        match gate_once(facade, &account, now) {
            Err(err) => {
                password::burn_verify(password);
                return Err(err);
            }
            Ok(Some(checking)) => return Ok((account, checking)),
            Ok(None) => LoginCheck::wait(&text_field(&account, "userId")),
        }
    }
}

/// One pass of the gate in front of a credential that counts as a login when wrong (a password, a second factor's
/// code): [`gate_account`], then a place among the user's checks running now, `None` when the failures the account has
/// left are all taken.
fn gate_once(facade: &EntityFacade, account: &EntityValue, now: i64) -> Result<Option<LoginCheck>, Error> {
    gate_account(facade, account, now, true)?;
    let left = policy::max_failures() + 1 - field_i64(account, "successiveFailedLogins").unwrap_or(0);
    Ok(LoginCheck::enter(&text_field(account, "userId"), left))
}

/// The old password of a password change (`update#Password`, `ec.user.checkOldPassword`), behind the gate of a password
/// login ([`gate_password`]): true when it is the account's current password, or its reset password while that is in
/// date (`password/@email-expire-hours`). Every refusal is the same false, whether the account is unknown (`user_id`
/// empty or not found), locked, terminated or the password wrong, and takes as long; a wrong password counts as a
/// failed login, and each refusal is noted for the web protection (SEC-WEB-02, SEC-ADM-02).
///
/// # Errors
///
/// Returns a storage error when the account cannot be read or the failure cannot be counted.
pub fn check_old_password(user_id: &str, old_password: &str) -> Result<bool, Error> {
    let checked = match old_password_ok(user_id, old_password) {
        Err(err) if err.message() == INVALID_LOGIN => Ok(false),
        other => other,
    };
    if matches!(checked, Ok(false)) {
        crate::web::note_auth_failure();
    }
    checked
}

fn old_password_ok(user_id: &str, old_password: &str) -> Result<bool, Error> {
    if old_password.is_empty() {
        return Ok(false);
    }
    let _authz = without_authz();
    let facade = entity_facade()?;
    let now = now_millis();
    let (account, _checking) = gate_password(&facade, old_password, now, || {
        if user_id.is_empty() {
            return Ok(None);
        }
        find_user_id(&facade, user_id)
    })?;
    if legacy_hash(&account) {
        password::burn_verify(old_password);
    }
    if password_ok(&account, old_password)? || reset_password_ok(&account, old_password, now)? {
        return Ok(true);
    }
    increment_failed(&facade, &text_field(&account, "userId"))?;
    Ok(false)
}

/// Count a failed login of `user_id` (`increment#UserAccountFailedLogins`, `ec.user.countFailedLogin`), as a wrong
/// password is counted ([`increment_failed`]): committed at once, and each of failures counted at once counted.
///
/// # Errors
///
/// Returns a storage error, and an error when there is no such account.
pub fn count_failed_login(user_id: &str) -> Result<(), Error> {
    let _authz = without_authz();
    increment_failed(&entity_facade()?, user_id)
}

/// The service that compares a code with the user's factors, with no count (`UserServices.xml`).
const COMPARE_CODE: &str = "org.moqui.impl.UserServices.compare#UserAuthcFactorCode";

/// A second factor's code that stands in for a login (`validate#UserAuthcFactorCode` with `countFailure`,
/// `ec.user.checkCountedCode`), behind the gate of a password login ([`gate_once`]): an unknown or terminated account
/// is refused, one still disabled is counted and refused with its code unchecked, no more of the user's codes are
/// compared at once than the failures the account has left, and a wrong code counts as a failed login by the password's
/// compare-and-set ([`increment_failed`]). A code past the failures left is counted and refused unchecked rather than
/// kept waiting, as a password is: a code is checked inside its caller's transaction, whose connection a wait would
/// hold. So codes sent at once are bounded by the lock as passwords are (SEC-WEB-87). A refusal and a wrong code are
/// noted for the web protection. The factor the code was for, or `None`.
///
/// # Errors
///
/// Returns a storage error when the account cannot be read or the failure cannot be counted, and the compare
/// service's errors.
pub fn check_counted_code(user_id: &str, code: &str) -> Result<Option<String>, Error> {
    if user_id.is_empty() {
        crate::web::note_auth_failure();
        return Ok(None);
    }
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(account) = find_user_id(&facade, user_id)? else {
        crate::web::note_auth_failure();
        return Ok(None);
    };
    let _checking = match gate_once(&facade, &account, now_millis()) {
        Ok(Some(checking)) => checking,
        Ok(None) => {
            tracing::warn!("A second factor code of user {user_id} came past the failures left; counted, not checked");
            increment_failed(&facade, user_id)?;
            crate::web::note_auth_failure();
            return Ok(None);
        }
        Err(err) if err.message() == INVALID_LOGIN => {
            crate::web::note_auth_failure();
            return Ok(None);
        }
        Err(err) => return Err(err),
    };
    let parameters = [("userId", user_id), ("code", code)]
        .into_iter()
        .map(|(name, value)| (name.to_string(), Value::string(value)))
        .collect();
    let compared = crate::service::sync(COMPARE_CODE, &parameters)?;
    let verified = compared.get("verified").is_some_and(super::factor::truthy);
    let factor_id = verified.then(|| {
        compared
            .get("factorId")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    });
    if factor_id.is_none() {
        increment_failed(&facade, user_id)?;
        crate::web::note_auth_failure();
    }
    Ok(factor_id)
}

/// Take a second factor's code once (`compare#UserAuthcFactorCode`, `ec.user.takeFactorCode`): a single-use code ends
/// (`thruDate` now) only while it has not ended, and an authenticator factor keeps `step` only while its last step taken
/// is before it. One conditional update in the caller's transaction (its held rows written first), so of two right
/// submissions of one code only one takes it: the database holds the second's update until the first's transaction
/// ends, and then it matches no row. A caller that rolls back gives the code back. True when this call took it.
///
/// # Errors
///
/// Returns a storage error.
pub fn take_factor_code(factor_id: &str, step: Option<i64>) -> Result<bool, Error> {
    let _authz = without_authz();
    let facade = entity_facade()?;
    let tx = facade.transactions_for(USER_AUTHC_FACTOR)?;
    // the caller's entity writes held in a transaction-cache frame go first, and its reads are forgotten, as for any
    // raw SQL in a frame (plans/archive/TX-Cache.md D1)
    crate::entity::cache::flush_before_sql(&facade, &tx)?;
    let Some(step) = step else {
        let Some(row) = facade
            .find(USER_AUTHC_FACTOR)?
            .eq("factorId", Value::string(factor_id))
            .one()?
        else {
            return Ok(false);
        };
        let read = row.get("thruDate").cloned().unwrap_or(Value::Null);
        return take_single_use(&facade, &tx, factor_id, &read, now_millis());
    };
    let condition = Condition::and(vec![
        Condition::equals("factorId", Value::string(factor_id)),
        Condition::or(vec![
            Condition::is_null("lastTotpStep"),
            Condition::less("lastTotpStep", Value::I64(step)),
        ]),
    ]);
    update_factor(&facade, &tx, "lastTotpStep", Value::I64(step), &condition)
}

/// Take a single-use code whose `thruDate` was read as `read`, at `now` (epoch millis): one already ended is not taken,
/// and the update matches only the `thruDate` read, so of two takes that read it only the first ends the code,
/// whatever the time of each (a printed code ends in a year and a sent one in a day, so the row's own date is not
/// enough: a later take's update, which the database re-checks once the first commits, would match a date set by the
/// first after its own `now`).
fn take_single_use(
    facade: &EntityFacade,
    tx: &crate::transaction::TransactionManager,
    factor_id: &str,
    read: &Value,
    now: i64,
) -> Result<bool, Error> {
    if value_millis(read).is_some_and(|thru| thru <= now) {
        return Ok(false);
    }
    let by_id = Condition::equals("factorId", Value::string(factor_id));
    let as_read = if read.is_null() {
        Condition::is_null("thruDate")
    } else {
        Condition::equals("thruDate", read.clone())
    };
    update_factor(
        facade,
        tx,
        "thruDate",
        stamp_value(now),
        &Condition::and(vec![by_id, as_read]),
    )
}

/// `UPDATE` one field of a factor (and its `lastUpdatedStamp`) where `condition` holds, in `tx`: true when a row changed.
fn update_factor(
    facade: &EntityFacade,
    tx: &crate::transaction::TransactionManager,
    column: &str,
    set: Value,
    condition: &Condition,
) -> Result<bool, Error> {
    let (statement, stamped) = facade.with_entity(USER_AUTHC_FACTOR, |entity, dialect, _| {
        let stamped = entity.has_field("lastUpdatedStamp");
        let columns: &[&str] = if stamped {
            &[column, "lastUpdatedStamp"]
        } else {
            &[column]
        };
        Ok((sql_update(entity, dialect, columns, Some(condition))?, stamped))
    })?;
    let mut params = vec![set];
    if stamped {
        params.push(now_value());
    }
    params.extend(statement.params);
    let updated = crate::entity::exec::execute_tx(tx, &statement.sql, &params)?;
    Ok(updated > 0)
}

/// Whether `plain` is the account's reset password (`reset#Password`, always argon2) and that is still in date; a
/// reset password with no set date is ended.
fn reset_password_ok(account: &EntityValue, plain: &str, now: i64) -> Result<bool, Error> {
    let reset = text_field(account, "resetPassword");
    let Some(set_at) = field_millis(account, "resetPasswordSetDate") else {
        return Ok(false);
    };
    if reset.is_empty() || now > set_at.saturating_add(policy::email_expire_hours().saturating_mul(3_600_000)) {
        return Ok(false);
    }
    password::passwords_match(plain, &reset, "argon2", "", false)
}

/// What [`authenticate`] ended with.
enum Checked {
    /// The password was right and the user waits for a second factor; the session id rotates.
    SecondFactor,
    /// The user is logged in; `rotate` when the session did not already hold this user.
    LoggedIn { rotate: bool },
}

/// The password and post-password checks of [`check_password_login`].
fn authenticate(
    facade: &EntityFacade,
    account: &EntityValue,
    username: &str,
    password: &str,
    now: i64,
) -> Result<Checked, Error> {
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
        return Ok(Checked::SecondFactor);
    }
    // A request repeating Basic credentials for the user its session already holds keeps its id.
    let same_user = session::current_username()?.as_deref() == Some(username);
    finish_login(facade, account, Some(password), true, true)?;
    note_recent_auth(&user_id);
    Ok(Checked::LoggedIn { rotate: !same_user })
}

/// Record the login in `UserLoginHistory` (`login/@history-store`): the time and visit are taken now, and a worker then
/// creates the row unless the user has one of its kind from the last minute: failures one a minute, successes one a
/// minute for each visit ([`HistoryKind`]). The minute is claimed on this node first ([`HistoryWritten`]), so logins at
/// once write one row; the worker's count covers other nodes and a restart. A failure there is only logged. Password,
/// second factor, login key, app password, SSH key and external (SSO) logins are recorded; the password tried is never
/// stored. The one row a minute also bounds what automation logging in with a key on every request writes; if those
/// volumes grow, key logins may be recorded differently.
fn save_history(user_id: &str, successful: bool) {
    if user_id.is_empty() || !policy::login_history_store() {
        return;
    }
    let from_date = now_effective_millis();
    let mut fields = BTreeMap::from([
        ("userId".to_string(), Value::string(user_id)),
        ("fromDate".to_string(), stamp_value(from_date)),
        (
            "successfulLogin".to_string(),
            Value::string(if successful { "Y" } else { "N" }),
        ),
    ]);
    let visit_id = session::visit_id().ok().flatten().filter(|id| !id.is_empty());
    if let Some(visit_id) = &visit_id {
        fields.insert("visitId".to_string(), Value::string(visit_id));
    }
    let Some(factory) = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory())) else {
        tracing::warn!("UserLoginHistory create failed: no execution context");
        return;
    };
    // a failure and a success claim minutes of their own, and a success one for each visit, so a wrong password first,
    // or a key's login every minute, does not keep a sign-in from another visit out of the history, nor from the
    // new-device notice its row starts (SEC-WEB-85); failures stay one a minute, whatever visits they come by
    let kind = if successful {
        HistoryKind::Success(visit_id)
    } else {
        HistoryKind::Failure
    };
    let Some(claim) = HistoryClaim::take(
        factory.extension(|_| HistoryWritten::default()),
        user_id,
        &kind,
        from_date,
    ) else {
        return;
    };
    factory.service_pools().spawn("UserLoginHistory create", move || {
        let stored = store_history(&claim.user_id, &kind, from_date, &fields);
        // a failure releases the minute (the claim's drop), so a later login is recorded
        claim.keep(stored.is_ok());
        stored
    });
}

/// Above this many users the claims older than a minute are pruned.
const HISTORY_CLAIMS_KEPT: usize = 1024;

/// Which minute of a user's login history a login claims: a failure's, or a success's of its visit (none for a login
/// with no visit, such as a key's).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum HistoryKind {
    Failure,
    Success(Option<String>),
}

/// The minute of login history each user's last login of each [`HistoryKind`] on this factory claimed: to its
/// `fromDate` millis. A login within a minute of a claim of its kind writes no row, without waiting for the row of the
/// other to be counted.
#[derive(Default)]
struct HistoryWritten {
    claimed: Mutex<HashMap<(String, HistoryKind), i64>>,
}

/// The minute a login claimed in [`HistoryWritten`]. Dropped without [`HistoryClaim::keep`] (the store failed, or
/// never ran), it is released.
struct HistoryClaim {
    written: Arc<HistoryWritten>,
    user_id: String,
    key: (String, HistoryKind),
    from_date: i64,
    kept: bool,
}

impl HistoryClaim {
    /// The minute from `from_date` for `user_id` and `kind`, or `None` when a login of that kind within the minute
    /// before claimed it.
    fn take(written: Arc<HistoryWritten>, user_id: &str, kind: &HistoryKind, from_date: i64) -> Option<Self> {
        let key = (user_id.to_string(), kind.clone());
        {
            let mut claimed = moqui_lib::sync::lock(&written.claimed);
            if claimed.get(&key).is_some_and(|stamp| *stamp > from_date - 60_000) {
                return None;
            }
            if claimed.len() >= HISTORY_CLAIMS_KEPT {
                claimed.retain(|_, stamp| *stamp > from_date - 60_000);
            }
            claimed.insert(key.clone(), from_date);
        }
        Some(Self {
            written,
            user_id: user_id.to_string(),
            key,
            from_date,
            kept: false,
        })
    }

    fn keep(mut self, kept: bool) {
        self.kept = kept;
    }
}

impl Drop for HistoryClaim {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        let mut claimed = moqui_lib::sync::lock(&self.written.claimed);
        if claimed.get(&self.key) == Some(&self.from_date) {
            claimed.remove(&self.key);
        }
    }
}

/// Create the history row in `fields` unless a row of the user and `kind` is there from the minute before `from_date`
/// (written by another node, or before a restart).
fn store_history(
    user_id: &str,
    kind: &HistoryKind,
    from_date: i64,
    fields: &BTreeMap<String, Value>,
) -> Result<(), Error> {
    let _authz = without_authz();
    let mut find = entity_facade()?
        .find(USER_LOGIN_HISTORY)?
        .eq("userId", Value::string(user_id))
        .condition(Condition::greater("fromDate", stamp_value(from_date - 60_000)));
    find = match kind {
        HistoryKind::Failure => find.eq("successfulLogin", Value::string("N")),
        HistoryKind::Success(Some(visit_id)) => find
            .eq("successfulLogin", Value::string("Y"))
            .eq("visitId", Value::string(visit_id)),
        HistoryKind::Success(None) => find
            .eq("successfulLogin", Value::string("Y"))
            .condition(Condition::is_null("visitId")),
    };
    let recent = find.count()?;
    if recent == 0 {
        crate::service::call::sync("create#moqui.security.UserLoginHistory", fields)?;
    }
    Ok(())
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
            let mut running = moqui_lib::sync::lock(&checking.running);
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
        let running = moqui_lib::sync::lock(&checking.running);
        if running.get(user_id).is_some_and(|count| *count > 0) {
            drop(moqui_lib::sync::wait_timeout(
                &checking.ended,
                running,
                Duration::from_secs(1),
            ));
        }
    }
}

impl Drop for LoginCheck {
    fn drop(&mut self) {
        if self.user_id.is_empty() {
            return;
        }
        let mut running = moqui_lib::sync::lock(&self.checking.running);
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

/// Set the in-memory user without checking the password. A bound web session that held another user or none moves to
/// a new id ([`internal_login`]).
///
/// # Errors
///
/// Returns an error when the account is unknown, disabled, terminated, or needs a password change.
pub fn internal_login_user(username: &str) -> Result<(), Error> {
    internal_login_user_verified(username, false)
}

/// [`internal_login_user`], where the caller says whether it verified the second factor's code of this sign-in
/// (`second_factor_verified`). Only such a login, finishing a sign-in whose password this session checked, is a strong
/// sign-in ([`LAST_STRONG_AUTH`]); the flag alone, or the session's flags alone, never are.
///
/// # Errors
///
/// As [`internal_login_user`].
pub fn internal_login_user_verified(username: &str, second_factor_verified: bool) -> Result<(), Error> {
    // The login that finishes a second factor: the password of this user was checked in this session, and the caller
    // verified the code. It is a sign-in (note_recent_auth); other internal logins (jobs, a verified callback) are not.
    let finishes_factor = !username.is_empty()
        && session::session_attribute_flag(AUTHC_FACTOR_REQUIRED)
        && session::session_attribute(PRE_AUTHC_USERNAME).as_deref() == Some(username);
    let (user_id, result) = internal_login(username, false, finishes_factor);
    // the password step of such a sign-in was recorded as not logged in; this is where it logs in (SEC-WEB-85)
    if finishes_factor {
        if let Some(user_id) = &user_id {
            save_history(user_id, result.is_ok());
        }
    }
    result?;
    if finishes_factor {
        if let Some(user_id) = session::current_user_id()?.filter(|id| !id.is_empty()) {
            note_recent_auth(&user_id);
            if second_factor_verified {
                note_strong_auth(&user_id);
            }
        }
    }
    Ok(())
}

/// [`internal_login_user`] for work a request of the user queued and a worker now runs as them (an async service call, a
/// job run): the one login a display account passes ([`is_display_account`]), since a device's request may queue work
/// as any request does. It signs nobody in: there is no client, and the worker's context ends with the work.
///
/// # Errors
///
/// Returns an error when the account is unknown, disabled, terminated, or needs a password change.
pub fn internal_login_queued_user(username: &str) -> Result<(), Error> {
    internal_login(username, true, false).1
}

/// The sign-in of a person by an SSH key the host checked to be theirs (an active `UserSshKey`), from `client_ip` (the
/// SSH peer): [`internal_login_user`] as a person's sign-in, which clears the account's `hasLoggedOut` as a password or
/// a login key does, so a sign-out elsewhere earlier does not end the sessions this sign-in starts. As the SSH password
/// login, it checks the account's and its groups' `ipAllowed` against `client_ip` and is recorded in the login history
/// (SEC-FIL-89). The operations of the session after it borrow the login ([`internal_login_user`], [`logout_local`])
/// and leave the flag as it is.
///
/// # Errors
///
/// Returns an error when the account is unknown, disabled, terminated, a display account, needs a password change, or
/// may not sign in from `client_ip`.
pub fn login_ssh_key_user(username: &str, client_ip: &str) -> Result<(), Error> {
    with_client_ip(client_ip, || recorded_login(username, true))
}

/// Whether the account is a display account (`UserAccount.displayAccount`, shared devices): it stands for no person
/// and nothing signs it in. Only a device credential runs as it, for one request at a time (`user/device.rs`).
pub(crate) fn is_display_account(account: &EntityValue) -> bool {
    text_field(account, "displayAccount") == "Y"
}

/// [`internal_login_user`], with the user id of the account once it passed the gate (None before), for the login history.
/// `queued` is [`internal_login_queued_user`]'s.
fn internal_login(username: &str, queued: bool, person: bool) -> (Option<String>, Result<(), Error>) {
    if username.is_empty() {
        return (None, Err(Error::new("No username specified")));
    }
    clear_pre_authc();
    let _authz = without_authz();
    let facade = match entity_facade() {
        Ok(facade) => facade,
        Err(err) => return (None, Err(err)),
    };
    // Through the entity cache: every async call and job run logs in this way as the user who queued it, and a write of
    // the account (disabling it, a new password) clears the cached row. A password login reads the row itself.
    let account = match find_username_cached(&facade, username) {
        Ok(Some(account)) => account,
        Ok(None) => return (None, Err(Error::new(INVALID_LOGIN))),
        Err(err) => return (None, Err(err)),
    };
    let now = now_millis();
    // as for a password login, an account the gate stops (disabled, terminated) has no history row; no credential of
    // the account was tried here, so nothing is counted
    if let Err(err) = gate_account(&facade, &account, now, false) {
        return (None, Err(err));
    }
    // a display account is never signed in: not by a key, an app password, an SSH key, an identity provider, a second
    // factor or Login as, which all end here
    if is_display_account(&account) && !queued {
        return (None, Err(Error::new(INVALID_LOGIN)));
    }
    let user_id = Some(text_field(&account, "userId")).filter(|id| !id.is_empty());
    let before = session::current_user_id().ok().flatten();
    let result = post_password(&account, now)
        .and_then(|()| finish_login(&facade, &account, None, false, person))
        .and_then(|()| {
            // A bound web session that now holds another user moves to a new id, whatever signed the user in (the
            // code of a second factor, a forced password change, a key, an identity provider), so an id planted
            // before the sign-in does not carry it (SEC-WEB-05). A queued worker has no session of its own, and a
            // session the server made for this request was never anyone else's.
            if queued || before == user_id || session::session_made_for_request() {
                return Ok(());
            }
            session::rotate_on_password_login()
        });
    (user_id, result)
}

/// [`internal_login`] for a key, app password or identity provider login, recorded in the login history when the
/// account is known. `person`: an interactive sign-in (an identity provider's), which clears the account's
/// `hasLoggedOut` as a password does; a key or an app password authenticates a request or a client and never does, so
/// a request made with one after a sign-out elsewhere does not bring back the sessions it ended (SEC-WEB-06).
fn recorded_login(username: &str, person: bool) -> Result<(), Error> {
    let (user_id, result) = internal_login(username, false, person);
    if let Some(user_id) = user_id {
        save_history(&user_id, result.is_ok());
    }
    result
}

/// A login an identity provider vouched for (SSO): [`internal_login_user`], which replaces the session id as a password
/// login does when the session held another user or none, so a session id from before the redirect does not carry the
/// new login.
///
/// # Errors
///
/// Returns an error when the account is unknown, disabled, terminated, or not allowed from this address.
pub fn login_external_user(username: &str, authenticated_at: Option<i64>) -> Result<(), Error> {
    recorded_login(username, true)?;
    if let Some(at) = authenticated_at {
        if let Some(user_id) = session::current_user_id()?.filter(|id| !id.is_empty()) {
            note_recent_auth_at(&user_id, at);
            note_strong_auth_at(&user_id, at);
        }
    }
    Ok(())
}

/// Log in with a `UserLoginKey`. The stored `loginKey` is [`super::password::hash_login_key`].
///
/// A `thruDate` in the past is rejected. A null `thruDate` stays valid. A cookie session that held another user or none
/// moves to a new id, as for every login ([`internal_login`]).
///
/// # Errors
///
/// Returns [`INVALID_LOGIN`](Error) when the key is empty, unknown, or expired, or the account cannot log in.
pub fn login_user_key(key: &str) -> Result<(), Error> {
    check_login_key(key, None, &[])
}

/// [`login_user_key`] for the one service `scope` names (`gateway`, the LLM gateway): an account's key, or a key made
/// for that scope alone ([`get_scoped_key`]), which every other login refuses.
///
/// # Errors
///
/// Returns [`INVALID_LOGIN`](Error) as [`login_user_key`] does, and for a key of another scope.
pub fn login_user_key_for(key: &str, scope: &str) -> Result<(), Error> {
    check_login_key(key, None, &[scope])
}

/// Recheck a socket's hashed login key without logging in, writing history, or changing its session.
/// Unknown, expired, revoked keys and disabled or terminated accounts have no user.
///
/// # Errors
/// Returns a storage error when the credential cannot be checked.
pub fn login_key_user_by_hash(hashed: &str) -> Result<Option<String>, Error> {
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(row) = facade
        .find(USER_LOGIN_KEY)?
        .eq("loginKey", Value::string(hashed))
        .one()?
    else {
        return Ok(None);
    };
    let now = now_millis();
    if field_millis(&row, "thruDate").is_some_and(|thru| key_ended(thru, now)) || !key_scope(&row).is_empty() {
        return Ok(None);
    }
    let user_id = text_field(&row, "userId");
    let Some(account) = find_user_id(&facade, &user_id)? else {
        return Ok(None);
    };
    Ok((!user_id.is_empty() && !still_disabled(&account, now) && !terminated(&account, now)).then_some(user_id))
}

/// The user a request's own credentials name, checked before its body is read, without logging in, writing history
/// or changing a session: the `api_key` or `login_key` header ([`login_key_user_by_hash`]), else `Authorization:
/// Basic` with an app password (one made for `scope`, or any when none is given) or, when `account_password`, the
/// account password. The host asks it of an upload larger than an anonymous request may send (SEC-RTE-07) and of a DAV
/// `PUT` (SEC-FIL-04). A wrong account password counts
/// as a failed login, as the login of the same request would; none when there are no credentials or they do not hold.
///
/// # Errors
///
/// Returns a storage error when the credential cannot be checked.
pub fn credential_user(
    api_key: Option<&str>,
    login_key: Option<&str>,
    authorization: Option<&str>,
    account_password: bool,
    scope: Option<&str>,
) -> Result<Option<String>, Error> {
    if let Some(key) = present_key(api_key).or_else(|| present_key(login_key)) {
        return login_key_user_by_hash(&password::hash_login_key(key)?);
    }
    let Some((username, secret)) = authorization.and_then(crate::web::basic_credentials) else {
        return Ok(None);
    };
    if username.trim().is_empty() || secret.is_empty() {
        return Ok(None);
    }
    let _authz = without_authz();
    let facade = entity_facade()?;
    if looks_like_app_password(&secret) {
        let hashed = password::hash_login_key(&app_password_key(&secret))?;
        if let Some(row) = facade
            .find(USER_APP_PASSWORD)?
            .eq("passwordHash", Value::string(&hashed))
            .one()?
            .filter(|row| field_millis(row, "revokedDate").is_none() && scope_allows(row, scope))
        {
            let user_id = text_field(&row, "userId");
            let owner = find_user_id(&facade, &user_id)?;
            if owner.is_some_and(|account| text_field(&account, "username").eq_ignore_ascii_case(username.trim())) {
                return Ok(Some(user_id));
            }
        }
    }
    if !account_password {
        return Ok(None);
    }
    let Some(account) = find_username(&facade, username.trim())? else {
        return Ok(None);
    };
    let user_id = text_field(&account, "userId");
    Ok(check_old_password(&user_id, &secret)?.then_some(user_id))
}

/// The kind of `UserLoginKey` a login by key used (`ec.user.loginKeyKind`), read from its row: what an application
/// asks of a request signed in by the `api_key` or `login_key` header, so it need not hash the key itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginKeyKind {
    /// A device's key (`deviceName`, [`get_device_key`]): the desktop or mobile client's.
    Device,
    /// An access token (`description`, `create#UserAccessToken`), for git alone ([`ACCESS_TOKEN_SCOPE`]).
    Token,
    /// A key from [`get_login_key`], with neither.
    Session,
    /// A key made for one service alone ([`get_scoped_key`]), which only that service's login takes.
    Scoped,
}

impl LoginKeyKind {
    /// The name scripts see: `device`, `token`, `session` or `scoped`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Token => "token",
            Self::Session => "session",
            Self::Scoped => "scoped",
        }
    }

    fn of(row: &EntityValue) -> Self {
        let scope = text_field(row, "scope");
        if scope == ACCESS_TOKEN_SCOPE || (scope.is_empty() && is_access_token(row)) {
            Self::Token
        } else if !scope.is_empty() {
            Self::Scoped
        } else if !text_field(row, "deviceName").is_empty() {
            Self::Device
        } else if !text_field(row, "description").is_empty() {
            Self::Token
        } else {
            Self::Session
        }
    }
}

/// The scope of an access token (`create#UserAccessToken`; SEC-PFS-61): git over HTTP and the git REST
/// (`/rest/s1/moqui/git/...`, `/rest/s1/moqui/hosting/...`) take it, and every other login refuses it, so a token made for
/// git is not the account's key on the web, the REST, a socket or a PASS page.
pub const ACCESS_TOKEN_SCOPE: &str = "git";

/// Whether `row` is an access token by its fields: a description and no device. One made before tokens had a scope has
/// an empty `scope` and is still git's alone ([`key_scope`]), so no stored row needs changing.
fn is_access_token(row: &EntityValue) -> bool {
    !text_field(row, "description").is_empty() && text_field(row, "deviceName").is_empty()
}

/// The one service a key opens alone: its `scope`, or [`ACCESS_TOKEN_SCOPE`] for an access token without one; empty for
/// the account's key (a session's or a device's), which every login takes.
fn key_scope(row: &EntityValue) -> String {
    let scope = text_field(row, "scope");
    if scope.is_empty() && is_access_token(row) {
        ACCESS_TOKEN_SCOPE.to_string()
    } else {
        scope
    }
}

/// `session_user`, when the session is already signed in, is the only user the key may log in: a key for another
/// user is refused before the login, so it never takes over that session or its attributes. A key made for one scope
/// (`UserLoginKey.scope`) logs in only where `scopes` names it.
fn check_login_key(key: &str, session_user: Option<&str>, scopes: &[&str]) -> Result<(), Error> {
    if key.is_empty() {
        return Err(refused_credential());
    }
    let hashed = password::hash_login_key(key)?;
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(row) = facade
        .find(USER_LOGIN_KEY)?
        .eq("loginKey", Value::string(&hashed))
        .one()?
    else {
        return Err(refused_credential());
    };
    let user_id = text_field(&row, "userId");
    if session_user.is_some_and(|signed_in| signed_in != user_id) {
        return Err(refused_credential());
    }
    let key_scope = key_scope(&row);
    if !key_scope.is_empty() && !scopes.contains(&key_scope.as_str()) {
        return Err(refused_credential());
    }
    if let Some(thru) = field_millis(&row, "thruDate") {
        if key_ended(thru, now_millis()) {
            save_history(&user_id, false);
            return Err(refused_credential());
        }
    }
    let Some(account) = find_user_id(&facade, &user_id)? else {
        return Err(refused_credential());
    };
    recorded_login(&text_field(&account, "username"), false)?;
    let kind = LoginKeyKind::of(&row);
    session::set_login_key_kind(Some(kind));
    if kind == LoginKeyKind::Device {
        session::set_device_key(Some(hashed.clone()));
        restore_key_stamps(&row, &user_id);
    }
    if !key_scope.is_empty() {
        session::set_scoped_key(Some(hashed.clone()));
    }
    // a device's key shows when it was last used, in the user's device list (plans/Mobile-Desktop.md, MD3)
    if !text_field(&row, "deviceName").is_empty() {
        note_key_used(&facade, &row, &hashed);
    }
    Ok(())
}

/// A re-authentication made in a request signed in by this device's key, which the key's row keeps, is this request's
/// too: the desktop and mobile clients sign every request in by the key, each a session of its own, so a
/// re-authentication kept only in the session never counted there, and every action behind `require#RecentAuth` failed
/// in the apps (plans/Security-Review.md SEC-CLI-107). The session's own later stamp stays.
fn restore_key_stamps(row: &EntityValue, user_id: &str) {
    if let Some(at) = field_millis(row, "lastAuthDate") {
        keep_later_stamp(LAST_AUTH, at, user_id);
    }
    // the strong sign-in that made the key, or the last strong re-authentication in a request it signed in
    if let Some(at) = field_millis(row, "strongAuthDate") {
        keep_later_stamp(LAST_STRONG_AUTH, at, user_id);
    }
    if let Some(at) = field_millis(row, "reauthPasswordDate") {
        if (0..=REAUTH_PASSWORD_MILLIS).contains(&(now_millis() - at)) {
            keep_later_stamp(REAUTH_PASSWORD, at, user_id);
        }
    }
}

/// Set the stamp attribute `name` to `at` for `user_id`, unless the session holds a later one of that user.
fn keep_later_stamp(name: &str, at: i64, user_id: &str) {
    let later = session::session_attribute(name)
        .and_then(|text| {
            text.split_once(' ')
                .map(|(millis, owner)| (millis.parse::<i64>().ok(), owner == user_id))
        })
        .is_some_and(|(noted, same)| same && noted.is_some_and(|noted| noted >= at));
    if !later {
        session::set_session_attribute(name, Some(&format!("{at} {user_id}")));
    }
}

/// Keep `fields`, a re-authentication's stamps, on the device's key this request signed in by, for the key's next
/// requests ([`restore_key_stamps`]); nothing for any other login. In a transaction of its own, as [`note_key_used`]: the
/// re-authentication stands whatever happens to the note.
fn note_on_device_key(fields: &[(&str, Value)]) {
    let Some(hashed) = session::device_key() else {
        return;
    };
    let _authz = without_authz();
    let facade = match entity_facade() {
        Ok(facade) => facade,
        Err(err) => {
            tracing::warn!("A device key's re-authentication was not kept for its next requests: {err}");
            return;
        }
    };
    let written = facade.transactions_for(USER_LOGIN_KEY).and_then(|tx| {
        tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
            facade
                .find(USER_LOGIN_KEY)?
                .eq("loginKey", Value::string(&hashed))
                .update_all(fields.iter().map(|(name, value)| ((*name).to_string(), value.clone())))
                .map(|_| ())
        })
    });
    if let Err(err) = written {
        tracing::warn!("A device key's re-authentication was not kept for its next requests: {err}");
    }
}

/// Write a device key's `lastUsedDate`, at most once a minute (a client signs every request in by its key), in a
/// transaction of its own; the login stands whatever happens to the note.
fn note_key_used(facade: &EntityFacade, row: &EntityValue, hashed: &str) {
    let now = now_millis();
    if field_millis(row, "lastUsedDate").is_some_and(|used| now - used <= 60_000) {
        return;
    }
    let written = facade.transactions_for(USER_LOGIN_KEY).and_then(|tx| {
        tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
            facade
                .find(USER_LOGIN_KEY)?
                .eq("loginKey", Value::string(hashed))
                .update_all([("lastUsedDate".to_string(), stamp_value(now))])
                .map(|_| ())
        })
    });
    if let Err(err) = written {
        tracing::warn!(
            "device key of {}: last use not written: {err}",
            text_field(row, "userId")
        );
    }
}

/// Log in with an app password (`UserAppPassword`), the password of one device's or app's client for one protocol,
/// which the web login does not take. The stored `passwordHash` is [`super::password::hash_login_key`] of the password
/// without its dashes and spaces, in lower case; the row must not be revoked, must be made for `scope` (one of
/// [`APP_PASSWORD_SCOPES`]) or for none, and must be the user's whose username is given. The account's own
/// checks then apply ([`internal_login_user`]), and `lastUsedDate` is written at most once a minute, in a transaction of
/// its own. A refusal is noted for the web protection; it is not an account failure, so guesses cannot lock the
/// account.
///
/// # Errors
///
/// Returns [`INVALID_LOGIN`](Error) when the password is empty, unknown, revoked, made for another protocol or another
/// user's, or the account cannot log in.
pub fn login_app_password(username: &str, password: &str, scope: &str) -> Result<(), Error> {
    check_app_password(username, password, scope)
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
pub fn app_password_key(password: &str) -> String {
    password
        .chars()
        .filter(|ch| *ch != '-' && !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn check_app_password(username: &str, password: &str, scope: &str) -> Result<(), Error> {
    let key = app_password_key(password);
    if username.is_empty() || key.is_empty() {
        return Err(refused_credential());
    }
    let hashed = password::hash_login_key(&key)?;
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(row) = facade
        .find(USER_APP_PASSWORD)?
        .eq("passwordHash", Value::string(&hashed))
        .one()?
    else {
        return Err(refused_credential());
    };
    let user_id = text_field(&row, "userId");
    if field_millis(&row, "revokedDate").is_some() {
        save_history(&user_id, false);
        return Err(refused_credential());
    }
    // made for another protocol (SEC-COM-21)
    if !scope_allows(&row, Some(scope)) {
        return Err(refused_credential());
    }
    let Some(account) = find_user_id(&facade, &user_id)? else {
        return Err(refused_credential());
    };
    let account_username = text_field(&account, "username");
    // another user's app password: not a login of this row's user, so nothing is recorded
    if !account_username.eq_ignore_ascii_case(username.trim()) {
        return Err(refused_credential());
    }
    recorded_login(&account_username, false)?;
    note_app_password_used(&facade, &row);
    Ok(())
}

/// Write an app password's `lastUsedDate`, at most once a minute, in a transaction of its own; the login stands
/// whatever happens to the note of its use.
fn note_app_password_used(facade: &EntityFacade, row: &EntityValue) {
    let now = now_millis();
    if field_millis(row, "lastUsedDate").is_some_and(|used| now - used <= 60_000) {
        return;
    }
    let written = facade.make_value(USER_APP_PASSWORD).and_then(|mut used| {
        used.set("appPasswordId", Value::string(text_field(row, "appPasswordId")));
        used.set("lastUsedDate", stamp_value(now));
        let tx = facade.transactions_for(USER_APP_PASSWORD)?;
        tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
            used.update(facade)
        })
    });
    if let Err(err) = written {
        tracing::warn!(
            "app password {}: last use not written: {err}",
            text_field(row, "appPasswordId")
        );
    }
}

/// The sign-in of a protocol session whose app password the protocol verified itself (XMPP SCRAM, or a FAST token of an
/// app password): the account's checks of a key login (lock window, terminate date, a display account, `ipAllowed`
/// against the [`with_client_ip`] address), recorded in the login history, and the app password's `lastUsedDate`
/// written, as a PLAIN sign-in ([`login_app_password`]) does (SEC-COM-11, SEC-COM-22). `app_password_id` must be the
/// user's, not revoked, and made for XMPP or for no protocol (SEC-COM-21).
///
/// # Errors
///
/// Returns [`INVALID_LOGIN`](Error) when the app password is not the user's or is revoked, or the account may not sign
/// in from here.
pub fn login_verified_app_password(username: &str, app_password_id: Option<&str>) -> Result<(), Error> {
    let row = match app_password_id.filter(|id| !id.is_empty()) {
        Some(id) => {
            let _authz = without_authz();
            let facade = entity_facade()?;
            let Some(row) = facade.find(USER_APP_PASSWORD)?.eq("appPasswordId", Value::string(id)).one()? else {
                return Err(Error::new(INVALID_LOGIN));
            };
            let owner = find_user_id(&facade, &text_field(&row, "userId"))?;
            if field_millis(&row, "revokedDate").is_some()
                || !scope_allows(&row, Some(XMPP_SCOPE))
                || !owner.is_some_and(|account| text_field(&account, "username").eq_ignore_ascii_case(username.trim()))
            {
                return Err(Error::new(INVALID_LOGIN));
            }
            Some(row)
        }
        None => None,
    };
    recorded_login(username, false)?;
    if let Some(row) = row {
        let _authz = without_authz();
        note_app_password_used(&entity_facade()?, &row);
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
    make_login_key(expire_hours.unwrap_or_else(policy::login_key_expire_hours), None)
}

/// A new login key for one device of the logged-in user, named `device` in the user's device list
/// (`get#UserDeviceKeys`) and good for the conf's `login-key/@device-expire-hours`. The desktop and mobile clients sign
/// in with it (plans/Mobile-Desktop.md §4, "Remote applications and authentication"); revoking it signs that device
/// out.
///
/// # Errors
///
/// Returns an error when nobody is logged in or a write fails.
pub fn get_device_key(device: &str) -> Result<String, Error> {
    let device: String = device.trim().chars().take(255).collect();
    make_login_key(
        policy::login_key_device_expire_hours(),
        Some(device.as_str()).filter(|name| !name.is_empty()),
    )
}

/// The scopes a key may be made for alone ([`get_scoped_key`]): `gateway`, the LLM gateway's `/llm/v1`; `git`
/// ([`ACCESS_TOKEN_SCOPE`]), git over HTTP and the REST resources that declare it (the framework's git and hosting
/// reads); `tasks`, the REST resources that declare it (Mantle's `my/tasks` and a repository's issues). A REST resource
/// names the scopes it takes with `key-scope` ([`request_key_scopes`]).
pub const KEY_SCOPES: [&str; 3] = ["gateway", ACCESS_TOKEN_SCOPE, "tasks"];

/// A new key of the logged-in user that opens only `scope` (one of [`KEY_SCOPES`]), named `device` in the user's
/// device list and good for the conf's `login-key/@device-expire-hours`: a dev instance keeps one for each of a group
/// server's git, tasks and LLM gateway (plans/design-dev-aide.md 21.2, question 18), where the account's own key would
/// open everything.
///
/// # Errors
///
/// Returns an error for a scope not in [`KEY_SCOPES`], when nobody is logged in, or when a write fails.
pub fn get_scoped_key(scope: &str, device: &str) -> Result<String, Error> {
    if !KEY_SCOPES.contains(&scope) {
        return Err(Error::new(format!("{scope} is not a scope a key is made for")));
    }
    let Some(user_id) = session::current_user_id()?.filter(|id| !id.is_empty()) else {
        return Err(Error::of_kind(
            crate::context::ErrorKind::AuthenticationRequired,
            "No active user, cannot get login key",
        ));
    };
    let device: String = device.trim().chars().take(255).collect();
    let device = if device.is_empty() { scope.to_string() } else { device };
    make_key(
        &user_id,
        policy::login_key_device_expire_hours(),
        Some(device.as_str()),
        Some(scope),
    )
}

/// End the logged-in user's login key `key` (as the client holds it), as a device signing out does. Answers whether
/// there was such a key of the user's.
///
/// # Errors
///
/// Returns an error when the key cannot be hashed or the row cannot be written.
pub fn revoke_login_key(key: &str) -> Result<bool, Error> {
    let Some(user_id) = session::current_user_id()?.filter(|id| !id.is_empty()) else {
        return Ok(false);
    };
    if key.is_empty() {
        return Ok(false);
    }
    let hashed = password::hash_login_key(key)?;
    // a request a key made for one service signed in ends that key alone, never another its holder knows (SEC-RTE-85)
    if session::scoped_key().is_some_and(|own| own != hashed) {
        return Ok(false);
    }
    let _authz = without_authz();
    let facade = entity_facade()?;
    let updated = facade
        .find(USER_LOGIN_KEY)?
        .eq("loginKey", Value::string(&hashed))
        .eq("userId", Value::string(&user_id))
        .update_all([("thruDate".to_string(), now_value())])?;
    if updated > 0 {
        // a device signing out gets no more pushes (plans/Mobile-Desktop.md §5, native push)
        crate::service::web_push::forget_device_key(&facade, &hashed);
    }
    Ok(updated > 0)
}

/// How long a renewed device's old key still signs in, so a request already on its way with it is not refused.
const RENEW_GRACE_MILLIS: i64 = 10 * 60 * 1000;

/// A new key for the logged-in user's device whose key is `key` (as the client holds it), with the same `deviceName`
/// and a full `login-key/@device-expire-hours`; the old key ends a few minutes later (plans/Mobile-Desktop.md §4,
/// "Remote applications and authentication"). The new key and its `thruDate` in milliseconds, or `None` when `key` is
/// not one of the user's unexpired device keys.
///
/// # Errors
///
/// Returns an error when the key cannot be hashed or a row cannot be read or written.
pub fn renew_device_key(key: &str) -> Result<Option<(String, i64)>, Error> {
    let Some(user_id) = session::current_user_id()?.filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    if key.is_empty() {
        return Ok(None);
    }
    let hashed = password::hash_login_key(key)?;
    let now = now_millis();
    let _authz = without_authz();
    let facade = entity_facade()?;
    let Some(mut old) = facade
        .find(USER_LOGIN_KEY)?
        .eq("loginKey", Value::string(&hashed))
        .one()?
    else {
        return Ok(None);
    };
    // A find by the primary key alone: the owner and the device are checked here.
    let device = text_field(&old, "deviceName");
    let old_thru = field_millis(&old, "thruDate");
    if text_field(&old, "userId") != user_id || device.is_empty() || old_thru.is_some_and(|thru| thru <= now) {
        return Ok(None);
    }
    let hours = policy::login_key_device_expire_hours();
    let renewed = make_login_key_for(&user_id, hours, Some(device.as_str()))?;
    let grace = now.saturating_add(RENEW_GRACE_MILLIS);
    old.set("thruDate", stamp_value(old_thru.map_or(grace, |thru| thru.min(grace))));
    old.update(&facade)?;
    // the device's push registration goes with it to the new key (plans/Mobile-Desktop.md §5, native push)
    crate::service::web_push::move_device_key(&facade, &hashed, &password::hash_login_key(&renewed)?);
    #[allow(clippy::cast_possible_truncation)] // As make_login_key_for's expiry.
    let thru = now.saturating_add((hours * HOUR_MILLIS).round() as i64);
    Ok(Some((renewed, thru)))
}

/// A device key for the user `user_id`, made with no session of theirs: an installed app's SSO hand-back
/// (`service/sso_app.rs`), where the browser signed in and the app takes the code. The account must be able to log
/// in now (not disabled or terminated), and the sign-in is in its login history, as a session's login is; no session
/// is logged in or out, so the user's other sessions are left as they are.
///
/// # Errors
///
/// Returns [`INVALID_LOGIN`](Error) for an account that is not there or cannot log in, or an error when a write fails.
pub fn issue_device_key(user_id: &str, device: &str) -> Result<String, Error> {
    {
        let _authz = without_authz();
        let facade = entity_facade()?;
        let Some(account) = find_user_id(&facade, user_id)? else {
            return Err(Error::new(INVALID_LOGIN));
        };
        let gated = gate_account(&facade, &account, now_millis(), false);
        save_history(user_id, gated.is_ok());
        gated?;
    }
    let device: String = device.trim().chars().take(255).collect();
    make_login_key_for(
        user_id,
        policy::login_key_device_expire_hours(),
        Some(device.as_str()).filter(|name| !name.is_empty()),
    )
}

fn make_login_key(hours: f64, device: Option<&str>) -> Result<String, Error> {
    let Some(user_id) = session::current_user_id()?.filter(|id| !id.is_empty()) else {
        return Err(Error::of_kind(
            crate::context::ErrorKind::AuthenticationRequired,
            "No active user, cannot get login key",
        ));
    };
    make_login_key_for(&user_id, hours, device)
}

fn make_login_key_for(user_id: &str, hours: f64, device: Option<&str>) -> Result<String, Error> {
    make_key(user_id, hours, device, None)
}

fn make_key(user_id: &str, hours: f64, device: Option<&str>, scope: Option<&str>) -> Result<String, Error> {
    let login_key = super::factor::random_string(40)?;
    let hashed = password::hash_login_key(&login_key)?;
    let from = now_millis();
    #[allow(clippy::cast_possible_truncation)] // An expiry past i64 milliseconds is not a real key.
    let thru = from.saturating_add((hours * HOUR_MILLIS).round() as i64);
    let _authz = without_authz();
    let facade = entity_facade()?;
    let mut row = facade.make_value(USER_LOGIN_KEY)?;
    row.set("loginKey", Value::string(hashed));
    row.set("userId", Value::string(user_id));
    row.set("fromDate", stamp_value(from));
    row.set("thruDate", stamp_value(thru));
    if let Some(device) = device {
        row.set("deviceName", Value::string(device));
    }
    if let Some(scope) = scope {
        row.set("scope", Value::string(scope));
    }
    // A device's key made right after a strong sign-in of this session (a second factor, SSO) is a strong one: it
    // counts toward strong_recent_auth with a recent sign-in on the device. One made after a password alone is not,
    // until a strong re-authentication in a request it signs in (note_strong_auth_at).
    if device.is_some() && scope.is_none() {
        if let Some(at) = last_strong_auth_millis().filter(|at| (0..=STRONG_KEY_MILLIS).contains(&(from - at))) {
            row.set("strongAuthDate", stamp_value(at));
        }
    }
    // Java creates the key with requireNewTransaction, so a caller's rollback does not lose it.
    let tx = facade.transactions_for(USER_LOGIN_KEY)?;
    tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
        row.create(&facade)
    })?;
    let expired = facade
        .find(USER_LOGIN_KEY)?
        .eq("userId", Value::string(user_id))
        .condition(moqui_db::sql::Condition::less("thruDate", stamp_value(from)))
        .list()?;
    for old in expired.iter() {
        old.delete(&facade)?;
    }
    Ok(login_key)
}

/// Header login. Uses `api_key` when it is present, otherwise `login_key`, and returns whether either was there.
///
/// Does not read a query string. A key is the request's authority even when the session is already signed in, so it
/// is checked every time, and a key for a user other than the session's is refused. `scope` is the one service the
/// request is for, when it is one a key may be made for alone (the git REST: [`ACCESS_TOKEN_SCOPE`]): a key of that scope
/// is taken there, and a scoped key anywhere else is refused.
///
/// # Errors
///
/// Returns the login error when the chosen key is rejected.
pub fn apply_header_keys(api_key: Option<&str>, login_key: Option<&str>, scope: Option<&str>) -> Result<bool, Error> {
    apply_header_keys_scoped(api_key, login_key, scope.as_slice())
}

/// [`apply_header_keys`] for a request whose path takes the keys of each of `scopes` ([`request_key_scopes`]).
///
/// # Errors
///
/// Returns the login error when the chosen key is rejected.
pub fn apply_header_keys_scoped(
    api_key: Option<&str>,
    login_key: Option<&str>,
    scopes: &[&str],
) -> Result<bool, Error> {
    let Some(key) = present_key(api_key).or_else(|| present_key(login_key)) else {
        return Ok(false);
    };
    let signed_in = session::current_user_id()?.filter(|id| !id.is_empty());
    check_login_key(key, signed_in.as_deref(), scopes)?;
    Ok(true)
}

/// The scopes of a request path ([`apply_header_keys_scoped`]), by the screen path the request reaches (`..` and empty
/// names resolved): for the REST (`/rest/s1/...`), the `key-scope` its resource declares, or the nearest resource above
/// it (`service::rest_key_scopes`); none for any other path. The framework's REST declares [`ACCESS_TOKEN_SCOPE`] on
/// its `git` resource and the read resources of its `hosting` (`offer`, `repository`, `mergeRequests`, `agreement`),
/// and every scope on `hosting/keyRevoke`, where such a key ends only itself ([`revoke_login_key`]); so a token for git
/// alone makes no key for another service (`hosting/gatewayKey`) and ends no other key (SEC-RTE-85). An application
/// declares its own (Mantle's `my/tasks` and a repository's issues take `tasks`).
pub fn request_key_scopes(path: &str) -> std::sync::Arc<[String]> {
    let names = crate::screen::screen_path_names(path);
    match names.split_first_chunk::<2>() {
        Some(([rest, s1], below)) if rest == "rest" && s1 == "s1" => crate::service::rest_key_scopes(below),
        _ => std::sync::Arc::from(Vec::new()),
    }
}

/// Whether this request signed in by a key made for one service alone that is not `key`: such a key ends only itself
/// ([`revoke_login_key`]).
///
/// # Errors
///
/// Returns an error when the key cannot be hashed.
pub fn scoped_login_is_not(key: &str) -> Result<bool, Error> {
    let Some(own) = session::scoped_key() else {
        return Ok(false);
    };
    Ok(own != password::hash_login_key(key)?)
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

/// End this context's login only (`ec.user.logoutLocal()`), as Java's `logoutLocal`: the end of a job's borrowed
/// sign-in (an internal login as a user to do work as them). Nothing is written to the account, so its `hasLoggedOut`
/// stays as it is and the person's own sessions go on; the user and anonymous flag are cleared here.
///
/// # Errors
///
/// Returns an error when the bound session cannot be flushed. The login is cleared either way.
pub fn logout_local() -> Result<(), Error> {
    session::set_session_attribute(LAST_AUTH, None);
    session::set_session_attribute(LAST_STRONG_AUTH, None);
    session::set_session_attribute(REAUTH_PASSWORD, None);
    session::clear_login();
    forget_shell_providers();
    session::flush_after_logout()
}

/// Clear the in-memory user and anonymous flag. Sets `hasLoggedOut` on the account. A bound web session is ended with
/// its attributes and replaced by a new empty one ([`session::renew_after_logout`]).
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
            Ok(facade) => {
                // The browser this visit subscribed gets no more pushes for this user.
                if let Some(visit) = session::visit_id().ok().flatten() {
                    crate::service::web_push::forget_visit(&facade, &user_id, &visit);
                }
                mark_logged_out(&facade, &user_id)
            }
            Err(_) => Ok(()),
        }
    } else {
        Ok(())
    };
    session::set_session_attribute(LAST_AUTH, None);
    session::set_session_attribute(LAST_STRONG_AUTH, None);
    session::set_session_attribute(REAUTH_PASSWORD, None);
    session::clear_login();
    forget_shell_providers();
    // the bound web session ends with its attributes, and a new empty one goes on (SEC-WEB-06)
    let renewed = session::renew_after_logout();
    update.and(renewed).and(session::flush_after_logout())
}

/// The session attribute of the session's last sign-in that proved who its user is: `<millis> <userId>`. A password
/// login (the form, an SSH or terminal password, Basic credentials), the code that finishes a second factor, an SSO
/// login and a re-authentication ([`super::reauthenticate`]) set it. A login key, an API key, an app password and an
/// internal login (jobs, `internalLoginUser`) do not: they are not a person signing in. A re-authentication in a
/// request a device's key signed in is kept on the key too, which its next requests take ([`restore_key_stamps`]).
pub(crate) const LAST_AUTH: &str = "moquiLastAuth";
/// The session attribute of the session's last strong sign-in, `<millis> <userId>`: a password and a second factor (the
/// code that finishes the sign-in, or a re-authentication with both), or an SSO login. A password alone, a login key,
/// an API key, an app password and an internal login are not; a paired device's key is, by its kind
/// ([`strong_recent_auth`]). Fetching a wrapped personal vault key needs one (plans/archive/Vault-Instances.md §9, Nov 19).
pub(crate) const LAST_STRONG_AUTH: &str = "moquiLastStrongAuth";
/// The password a re-authentication checked while it waits for the code: `<millis> <userId>`.
pub(crate) const REAUTH_PASSWORD: &str = "moquiReauthPassword";
/// How long a re-authentication's checked password waits for its code.
const REAUTH_PASSWORD_MILLIS: i64 = 5 * 60 * 1000;
/// How long after a strong sign-in a device's key made in the same session is a strong one.
const STRONG_KEY_MILLIS: i64 = 10 * 60 * 1000;

/// The session's user signed in (or confirmed who they are) now.
pub(crate) fn note_recent_auth(user_id: &str) {
    note_recent_auth_at(user_id, now_millis());
}

/// The session's user signed in at `at` (epoch millis; an identity provider's `auth_time`), never later than now.
fn note_recent_auth_at(user_id: &str, at: i64) {
    if user_id.is_empty() {
        return;
    }
    let at = at.min(now_millis());
    // a later sign-in of this user already noted stays (an SSO login answered from an older provider session after a
    // password login)
    let noted = session::session_attribute(LAST_AUTH).and_then(|text| {
        text.split_once(' ')
            .map(|(millis, owner)| (millis.parse::<i64>().ok(), owner == user_id))
    });
    if let Some((Some(noted), true)) = noted {
        if noted > at {
            session::set_session_attribute(REAUTH_PASSWORD, None);
            return;
        }
    }
    session::set_session_attribute(LAST_AUTH, Some(&format!("{at} {user_id}")));
    session::set_session_attribute(REAUTH_PASSWORD, None);
    note_on_device_key(&[("lastAuthDate", stamp_value(at)), ("reauthPasswordDate", Value::Null)]);
}

/// The session's user signed in strongly now ([`LAST_STRONG_AUTH`]).
pub(crate) fn note_strong_auth(user_id: &str) {
    note_strong_auth_at(user_id, now_millis());
}

/// The session's user signed in strongly at `at` (epoch millis), never later than now; a later stamp of this user
/// stays.
fn note_strong_auth_at(user_id: &str, at: i64) {
    if user_id.is_empty() {
        return;
    }
    let at = at.min(now_millis());
    let noted = session::session_attribute(LAST_STRONG_AUTH).and_then(|text| {
        text.split_once(' ')
            .map(|(millis, owner)| (millis.parse::<i64>().ok(), owner == user_id))
    });
    if let Some((Some(noted), true)) = noted {
        if noted > at {
            return;
        }
    }
    session::set_session_attribute(LAST_STRONG_AUTH, Some(&format!("{at} {user_id}")));
    // a strong re-authentication in a request a device's key signed in makes that key a strong one (a password-only
    // re-authentication refreshes its recency, never its strength)
    note_on_device_key(&[("strongAuthDate", stamp_value(at))]);
}

/// A login of `user_id` drops the sign-in stamps of another user, so an account switch in one session (a key, an
/// internal login, "login as") never finds an earlier user's sign-in when that user comes back without signing in.
fn drop_stamps_of_others(user_id: &str) {
    for name in [LAST_AUTH, LAST_STRONG_AUTH, REAUTH_PASSWORD] {
        let owner =
            session::session_attribute(name).and_then(|text| text.split_once(' ').map(|(_, owner)| owner.to_string()));
        if owner.is_some_and(|owner| owner != user_id) {
            session::set_session_attribute(name, None);
        }
    }
}

/// The time in the stamp attribute `name` when it is the current user's.
fn stamp_of(name: &str) -> Option<i64> {
    let user_id = session::current_user_id().ok().flatten().filter(|id| !id.is_empty())?;
    let text = session::session_attribute(name)?;
    let (millis, owner) = text.split_once(' ')?;
    (owner == user_id).then(|| millis.parse::<i64>().ok()).flatten()
}

/// When the session's user last signed in in a way that proves who they are ([`LAST_AUTH`]), in epoch millis; none
/// with nobody signed in, or when this session's user came only by a key, an app password or an internal login.
pub fn last_auth_millis() -> Option<i64> {
    stamp_of(LAST_AUTH)
}

/// Whether the session's user signed in (or confirmed who they are) in the last `max_age_millis`.
pub fn recent_auth(max_age_millis: i64) -> bool {
    last_auth_millis().is_some_and(|at| {
        let age = now_millis() - at;
        (0..=max_age_millis).contains(&age)
    })
}

/// When the session's user last signed in strongly ([`LAST_STRONG_AUTH`]), in epoch millis.
pub fn last_strong_auth_millis() -> Option<i64> {
    stamp_of(LAST_STRONG_AUTH)
}

/// Whether the session's user signed in strongly in the last `max_age_millis`: a password and a second factor, or an
/// SSO login ([`LAST_STRONG_AUTH`]); or this request came by a paired device's key (the desktop and mobile apps) that
/// a strong sign-in made or strongly re-authenticated (`strongAuthDate` on the key), a factor the device holds, with a
/// recent sign-in on that device ([`recent_auth`], kept on the key). Never an app password, an API or session login
/// key, a scoped key, a device's key made after a password alone, or a password alone. The vault services ask it
/// before they send a wrapped personal key (plans/archive/Vault-Instances.md §3.5, §9).
pub fn strong_recent_auth(max_age_millis: i64) -> bool {
    let recent = |at: i64| (0..=max_age_millis).contains(&(now_millis() - at));
    if last_strong_auth_millis().is_some_and(recent) {
        return true;
    }
    session::login_key_kind() == Some(LoginKeyKind::Device)
        && last_auth_millis().is_some_and(recent)
        && device_key_strong()
}

/// Whether this request's device key carries a strong sign-in (`strongAuthDate`).
fn device_key_strong() -> bool {
    let Some(hashed) = session::device_key() else {
        return false;
    };
    let _authz = without_authz();
    let row = entity_facade().and_then(|facade| {
        Ok(facade
            .find(USER_LOGIN_KEY)?
            .eq("loginKey", Value::string(&hashed))
            .one()?)
    });
    match row {
        Ok(row) => row.is_some_and(|row| field_millis(&row, "strongAuthDate").is_some()),
        Err(err) => {
            tracing::warn!("A device key's strength could not be read: {err}");
            false
        }
    }
}

/// The signed-in user's id, or the error that nobody is.
pub(super) fn current_user() -> Result<String, Error> {
    session::current_user_id()?
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::new("Sign in first"))
}

/// Whether a re-authentication checked `user_id`'s password in this session in the last five minutes.
pub(super) fn reauth_password_checked(user_id: &str) -> bool {
    session::current_user_id().ok().flatten().as_deref() == Some(user_id)
        && stamp_of(REAUTH_PASSWORD).is_some_and(|at| (0..=REAUTH_PASSWORD_MILLIS).contains(&(now_millis() - at)))
}

/// The password check of a re-authentication of the signed-in `user_id`: the account's gate and its password, a wrong one
/// counted as a failed login, as at the login form; the user, the session and its id do not change. A right one is
/// remembered for the code ([`reauth_password_checked`]).
pub(super) fn check_reauth_password(user_id: &str, password: &str) -> Result<(), Error> {
    noting_invalid(reauth_password(user_id, password))
}

fn reauth_password(user_id: &str, password: &str) -> Result<(), Error> {
    if password.is_empty() {
        return Err(Error::new(INVALID_LOGIN));
    }
    let _authz = without_authz();
    let facade = entity_facade()?;
    let now = now_millis();
    let (account, _checking) = loop {
        let Some(account) = find_user_id(&facade, user_id)? else {
            password::burn_verify(password);
            return Err(Error::new(INVALID_LOGIN));
        };
        if let Err(err) = gate_account(&facade, &account, now, true) {
            password::burn_verify(password);
            return Err(err);
        }
        if text_field(&account, "currentPassword").is_empty() {
            return Err(Error::new(
                "This account signs in through its identity provider: sign in with it again",
            ));
        }
        let left = policy::max_failures() + 1 - field_i64(&account, "successiveFailedLogins").unwrap_or(0);
        if let Some(checking) = LoginCheck::enter(user_id, left) {
            break (account, checking);
        }
        LoginCheck::wait(user_id);
    };
    if legacy_hash(&account) {
        password::burn_verify(password);
    }
    if !password_ok(&account, password)? {
        increment_failed(&facade, user_id)?;
        return Err(Error::new(INVALID_LOGIN));
    }
    if needs_reset(&account) {
        update_user(
            &facade,
            user_id,
            &[
                ("successiveFailedLogins", Value::I64(0)),
                ("disabled", Value::string("N")),
                ("disabledDateTime", Value::Null),
            ],
        )?;
    }
    session::set_session_attribute(REAUTH_PASSWORD, Some(&format!("{now} {user_id}")));
    note_on_device_key(&[("reauthPasswordDate", stamp_value(now))]);
    Ok(())
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
        Ok(update_password_fields(&facade, &account, &hashed)?)
    })?;
    Ok(())
}

/// The new password's fields. A password change ends a tarpit lock (`disabled` with a `disabledDateTime`), as Java's
/// does; an account disabled for good stays disabled (SEC-ADM-01), and only `enable#UserAccount` opens it.
fn update_password_fields(facade: &EntityFacade, account: &EntityValue, hashed: &str) -> Result<(), Error> {
    let mut fields = vec![
        ("currentPassword", Value::string(hashed)),
        ("passwordHashType", Value::string("argon2")),
        ("passwordSalt", Value::string("")),
        ("passwordBase64", Value::string("N")),
        ("passwordSetDate", now_value()),
        ("requirePasswordChange", Value::string("N")),
        ("resetPassword", Value::Null),
        ("resetPasswordSetDate", Value::Null),
    ];
    if !disabled_for_good(account) {
        fields.push(("disabled", Value::string("N")));
    }
    update_user(facade, &text_field(account, "userId"), &fields)
}

pub(crate) fn entity_facade() -> Result<EntityFacade, Error> {
    match ExecutionContext::with_current(|ctx| ctx.factory().entity()) {
        None => Err(Error::new("no execution context")),
        Some(None) => Err(Error::new("no entity facade")),
        Some(Some(facade)) => Ok(facade),
    }
}

/// The time now as a `Timestamp` value, for a date-time field.
pub fn now_value() -> Value {
    stamp_value(now_millis())
}

/// A `Timestamp` value of epoch `millis`.
pub fn stamp_value(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "Timestamp").value
}

/// Java's `ec.user.nowTimestamp`: the entered context's effective time, or the clock.
pub fn now_effective_value() -> Value {
    stamp_value(now_effective_millis())
}

/// [`now_effective_value`] as epoch millis.
pub fn now_effective_millis() -> i64 {
    ExecutionContext::with_current(ExecutionContext::effective_time)
        .flatten()
        .unwrap_or_else(now_millis)
}

/// Whether a login key whose `thruDate` is `thru` has ended at `now` (both milliseconds): in date while its thruDate is
/// after now, as every effective-dated row. A key ended now is ended: `revoke#UserHeldKey` sets its thruDate to now, and
/// a login in the same millisecond took the revoked key.
pub(crate) fn key_ended(thru: i64, now: i64) -> bool {
    thru <= now
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// Whether the account may sign in now: not in a lock window, not disabled for good, not terminated. `counted`: a
/// password was tried, so an attempt on a locked account counts as a failed login (the lock window does not restart,
/// [`write_failed_increment`]). An internal login (a job, a worker, a second factor's finish) and a valid key, app
/// password or identity provider tried no password of the account, and count nothing (SEC-WEB-03, SEC-COM-12).
fn gate_account(facade: &EntityFacade, account: &EntityValue, now: i64, counted: bool) -> Result<(), Error> {
    let user_id = text_field(account, "userId");
    if still_disabled(account, now) {
        if counted && !user_id.is_empty() {
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

/// The login of `account`. `person`: a person signs in interactively (a password, the code of a second factor, an SSH
/// key at a terminal, an identity provider), which clears the account's `hasLoggedOut`; a login key, an app password
/// (SEC-WEB-06) and a job's or a worker's internal login leave the flag as it is, so they do not bring back the
/// sessions a sign-out ended.
fn finish_login(
    facade: &EntityFacade,
    account: &EntityValue,
    plain: Option<&str>,
    rehash: bool,
    person: bool,
) -> Result<(), Error> {
    let user_id = text_field(account, "userId");
    let username = text_field(account, "username");
    if user_id.is_empty() || username.is_empty() {
        return Err(Error::new(INVALID_LOGIN));
    }
    // a shared device's request never becomes a signed-in session, as anyone (user/device.rs)
    if session::device_scope().is_some() {
        return Err(Error::new("A shared device does not sign in"));
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
    let logged_out = text_field(account, "hasLoggedOut") == "Y";
    if failed_or_disabled(account) {
        fields.push(("successiveFailedLogins", Value::I64(0)));
        fields.push(("disabled", Value::string("N")));
        fields.push(("disabledDateTime", Value::Null));
    }
    if person && logged_out {
        fields.push(("hasLoggedOut", Value::string("N")));
    }
    // The account's update and the visit's link to it in one transaction-cache frame (plans/archive/TX-Cache.md F3).
    session::in_cache_frame(facade, USER_ACCOUNT, || {
        if !fields.is_empty() {
            update_user(facade, &user_id, &fields)?;
        }
        drop_stamps_of_others(&user_id);
        session::set_user(&user_id, &username);
        forget_shell_providers();
        let locale = nonempty(text_field(account, "locale"));
        let zone = nonempty(text_field(account, "timeZone"));
        session::set_locale(locale.as_deref());
        session::set_time_zone(zone.as_deref());
        link_visit(facade, &user_id)
    })?;
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
pub(super) fn client_ip() -> String {
    let web = crate::web::current_client_ip();
    if !web.is_empty() {
        return web;
    }
    LOGIN_CLIENT_IP.with(|slot| slot.borrow().clone()).unwrap_or_default()
}

/// `ipAllowed` of the account and of every group it is in, `ALL_USERS` included: when any is set, the client address
/// must match one of them. With no address, as for a login outside a request, nothing is checked. A refusal is the
/// invalid login error, and the address and the account only in the server's log, so the answer tells the client
/// neither that its credential was right nor which address the server took for it (SEC-WEB-90).
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
    tracing::warn!(
        "Authenticate failed for user {} because client IP {client_ip} is not in allowed list for user or group.",
        text_field(account, "username")
    );
    Err(Error::new(INVALID_LOGIN))
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

/// Whether the account has failed logins counted or is disabled (what a login that passes resets).
fn failed_or_disabled(account: &EntityValue) -> bool {
    field_i64(account, "successiveFailedLogins").unwrap_or(0) != 0
        || text_field(account, "disabled") == "Y"
        || field_millis(account, "disabledDateTime").is_some()
}

fn needs_reset(account: &EntityValue) -> bool {
    failed_or_disabled(account) || text_field(account, "hasLoggedOut") == "Y"
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

/// Whether an account's live sign-ins are to end (impl-admin F4, F6, F42): it is disabled for good (`disabled` with no
/// `disabledDateTime`, as `disable#UserAccount` leaves it) or past its `terminateDate`, either of which stops a login. A
/// tarpit lock (disabled with a `disabledDateTime`) stops new logins only until it expires and does not end a session, so
/// failed logins by someone else cannot sign the account's owner out.
pub(crate) fn account_closed(account: &EntityValue, now: i64) -> bool {
    disabled_for_good(account) || terminated(account, now)
}

/// Disabled with no `disabledDateTime`, as `disable#UserAccount` leaves it: closed until an administrator enables it.
fn disabled_for_good(account: &EntityValue) -> bool {
    text_field(account, "disabled") == "Y" && field_millis(account, "disabledDateTime").is_none()
}

/// [`account_closed`] for the account `user_id`, read now; an unknown account is closed. For the notification socket's
/// recheck of a web session's user and a job's run-as user.
///
/// # Errors
///
/// Returns a storage error when the account cannot be read.
pub fn user_account_closed(user_id: &str) -> Result<bool, Error> {
    let _authz = without_authz();
    let facade = entity_facade()?;
    Ok(find_user_id(&facade, user_id)?.is_none_or(|account| account_closed(&account, now_millis())))
}

/// [`account_closed`] for the account of `username` (a job's run-as user), read now; an unknown account is closed.
///
/// # Errors
///
/// Returns a storage error when the account cannot be read.
pub fn username_account_closed(username: &str) -> Result<bool, Error> {
    let _authz = without_authz();
    let facade = entity_facade()?;
    Ok(find_username(&facade, username)?.is_none_or(|account| account_closed(&account, now_millis())))
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

/// [`find_username`] through the entity cache.
fn find_username_cached(facade: &EntityFacade, username: &str) -> Result<Option<EntityValue>, Error> {
    Ok(facade
        .find(USER_ACCOUNT)?
        .eq("username", Value::string(username))
        .use_cache(true)
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
        // An account disabled for good (`disabled` with no `disabledDateTime`) stays so: a lock time would turn it into a
        // tarpit lock that ends by itself (SEC-WEB-01). Only the count goes up.
        let disabled_flag = row.get("disabled").and_then(Value::as_str).map(str::trim) == Some("Y");
        let closed = disabled_flag && row_value(row, "disabledDateTime").is_null();
        // An attempt during a lock window counts but does not restart the window, so attempts that go on (or the owner's
        // integration) cannot keep the account locked for as long as they last (SEC-WEB-03).
        let window = policy::disable_minutes().saturating_mul(60_000);
        let locked = disabled_flag
            && value_millis(&row_value(row, "disabledDateTime"))
                .is_some_and(|at| at.saturating_add(window) > now_millis());
        let disable = next > policy::max_failures() && !closed && !locked;
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

pub fn value_millis(value: &Value) -> Option<i64> {
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
#[path = "login_tests.rs"]
mod tests;
