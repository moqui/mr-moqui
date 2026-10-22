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

//! User facade: login, passwords, the second factor, preferences, the in-memory session, and
//! artifact authorization.

mod authz;
mod factor;
mod filter;
mod login;
mod password;
mod period;
mod policy;
mod preference;
pub(crate) mod session;
mod visit;

pub(crate) use authz::current_user_groups;
pub use authz::{authorize, authorize_own, context_entered, group_permits};
pub(crate) use authz::{denied, is_permitted, note_entity_write, AuthInfo, Permit};
pub use factor::{
    login_with_code, random_code, random_string, second_factor_required, send_login_code, single_use_hash,
    totp_code_valid, totp_secret, totp_uri, CodeLogin, SendableFactor,
};
pub(crate) use factor::{CODE_NOT_VALID, FACTOR_INFO, SEND_CODE, VALIDATE_CODE};
pub(crate) use filter::apply_view_filters;
pub use filter::{apply_entity_filters, find_filters_for_user};
pub(crate) use login::app_password_key;
pub(crate) use login::login_user_local;
pub(crate) use login::stamp_value;
pub use login::{
    apply_header_keys, change_password, get_login_key, header_login_session, internal_login_user, login_anonymous,
    login_app_password, login_external_user, login_user, login_user_key, logout, logout_anonymous_only,
    looks_like_app_password, new_app_password, note_header_login, with_client_ip,
};
pub(crate) use login::{now_effective_millis, now_effective_value, now_millis, now_value, value_millis};
pub(crate) use login::{AUTHC_FACTOR_REQUIRED, PRE_AUTHC_USERNAME};
pub use password::{hash_login_key, hash_password};
pub use period::period_range;
pub use policy::password_acceptable;
pub use preference::{
    current_user_external, get_preference, get_preference_for, get_preferences, has_permission, is_external_user,
    is_in_group, set_preference, user_group_ids, user_has_permission, EXTERNAL_USER, EXTERNAL_WITHHELD,
};
pub(crate) use session::Memory;
#[cfg(feature = "llm")]
pub(crate) use session::derived_cookie_key;
pub use session::{
    bind, csrf_token, current_log_actor, current_user_id, current_username, end_request, end_user_visit,
    error_parameters, flash_messages, format_session_cookie, format_visitor_cookie, http_log_identity, invalidate,
    is_anonymous, live_session_user_id, locale, saved_parameters, screen_history, set_current_log_actor, time_zone,
    verify_session_cookie, visit_id, visitor_id, SessionGuard, SESSION_COOKIE_NAME, VISITOR_COOKIE_NAME,
};
pub use visit::record_visit;

/// `(min length, min digits, min others)` of the password policy.
pub(crate) fn policy_password_rules() -> (i64, i64, i64) {
    policy::password_rules()
}

/// The `user-facade` settings the user services read: `(max-failures, disable-minutes,
/// history-limit, email-expire-hours, email-require-change)`.
pub(crate) fn policy_login_rules() -> (i64, i64, i64, i64, bool) {
    (
        policy::max_failures(),
        policy::disable_minutes(),
        policy::history_limit(),
        policy::email_expire_hours(),
        policy::email_require_change(),
    )
}

/// `passwordsMatch` for scripts: `plain` against a stored hash of any supported type.
pub(crate) fn stored_password_matches(
    plain: &str,
    stored: &str,
    hash_type: &str,
    salt: &str,
    base64: bool,
) -> Result<bool, crate::context::Error> {
    password::passwords_match(plain, stored, hash_type, salt, base64)
}

/// Run `work` as the user `user_id`, with authorization on: in a context of its own, entered on this thread, so it
/// shares the caller's transaction but not the caller's user, authorization or messages. None when there is no
/// context or the user is not found. For deciding what another user may see or do (the chat record rule, the
/// approvals a merge request counts).
pub(crate) fn as_user<T>(user_id: &str, work: impl FnOnce() -> T) -> Option<T> {
    use crate::context::ExecutionContext;
    let factory = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))?;
    let facade = factory.entity()?;
    let account = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        facade
            .find("moqui.security.UserAccount")
            .and_then(|find| find.eq("userId", moqui_lib::object::Value::string(user_id)).one())
    };
    let username = match account {
        Ok(Some(account)) => account
            .get("username")
            .map(moqui_lib::object::to_plain_string)
            .filter(|name| !name.is_empty()),
        Ok(None) => None,
        Err(err) => {
            tracing::error!("Could not read the account of user {user_id}: {err}");
            None
        }
    }?;
    let ctx = factory.new_context();
    let _entered = ctx.enter();
    session::set_user(user_id, &username);
    Some(work())
}
