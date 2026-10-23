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
pub use authz::{authorize, context_entered};
pub(crate) use authz::{denied, is_permitted, note_entity_write, AuthInfo, Permit};
pub use factor::{
    random_code, random_string, second_factor_required, single_use_hash, totp_code_valid, totp_secret, totp_uri,
};
pub(crate) use filter::apply_view_filters;
pub use filter::{apply_entity_filters, find_filters_for_user};
pub(crate) use login::login_user_local;
pub(crate) use login::stamp_value;
pub use login::{
    apply_header_keys, change_password, get_login_key, internal_login_user, login_anonymous, login_external_user,
    login_user, login_user_key, logout, logout_anonymous_only,
};
pub(crate) use login::{now_effective_millis, now_effective_value, now_millis, now_value, value_millis};
pub(crate) use login::{AUTHC_FACTOR_REQUIRED, PRE_AUTHC_USERNAME};
pub use password::hash_password;
pub use period::{period_days, period_range};
pub use policy::password_acceptable;
pub use preference::{
    get_preference, get_preference_for, get_preferences, has_permission, is_in_group, set_preference, user_group_ids,
};
pub(crate) use session::Memory;
pub use session::{
    bind, csrf_token, current_user_id, current_username, end_request, error_parameters, flash_messages,
    format_session_cookie, format_visitor_cookie, invalidate, is_anonymous, live_session_user_id, locale,
    saved_parameters, screen_history, time_zone, verify_session_cookie, visit_id, visitor_id, SessionGuard,
    SESSION_COOKIE_NAME, VISITOR_COOKIE_NAME,
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
