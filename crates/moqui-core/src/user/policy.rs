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

//! Password policy from `user-facade/password`. Missing conf uses the documented fallbacks.

use crate::context::{Error, ExecutionContext};

/// Reject a password that is short, has too few digits, or has no non-alphanumeric character.
pub fn password_acceptable(password: &str) -> Result<(), Error> {
    let (min_length, min_digits, min_others) = password_rules();
    let length = i64::try_from(password.chars().count()).unwrap_or(i64::MAX);
    if length < min_length {
        return Err(Error::new(format!("Password shorter than {min_length} characters")));
    }
    let digits = count_matching(password, |ch| ch.is_ascii_digit());
    if digits < min_digits {
        return Err(Error::new(format!(
            "Password needs {min_digits} digit/number characters"
        )));
    }
    let others = count_matching(password, |ch| !ch.is_alphabetic() && !ch.is_ascii_digit());
    if others < min_others {
        return Err(Error::new(format!(
            "Password needs {min_others} other characters (not letter or digit)"
        )));
    }
    Ok(())
}

pub(crate) fn password_rules() -> (i64, i64, i64) {
    (
        conf_number("password", "min-length", 8),
        conf_number("password", "min-digits", 1),
        conf_number("password", "min-others", 1),
    )
}

pub(crate) fn history_limit() -> i64 {
    conf_number("password", "history-limit", 5)
}

/// Java uses 12 when the attribute is missing. The XML default, when present, is 104.
pub(crate) fn change_weeks() -> i64 {
    conf_number("password", "change-weeks", 12)
}

pub(crate) fn password_hash_type() -> String {
    conf_text("password", "encrypt-hash-type", "SHA-256")
}

pub(crate) fn login_key_hash_type() -> String {
    conf_text("login-key", "encrypt-hash-type", "SHA-256")
}

/// `login-key/@expire-hours`, 144 (six days) as in the Java conf. A fraction is allowed.
pub(crate) fn login_key_expire_hours() -> f64 {
    conf_text_opt("login-key", "expire-hours")
        .and_then(|text| text.parse().ok())
        .unwrap_or(144.0)
}

/// How long an emailed reset password stays usable, `password/@email-expire-hours`.
pub(crate) fn email_expire_hours() -> i64 {
    conf_number("password", "email-expire-hours", 48)
}

/// `password/@email-require-change`: a reset sets `requirePasswordChange`.
pub(crate) fn email_require_change() -> bool {
    conf_text("password", "email-require-change", "false") == "true"
}

pub(crate) fn max_failures() -> i64 {
    conf_number("login", "max-failures", 3)
}

/// Conf default is 5. Java's in-code fallback when the attribute is missing is 30; this reads the attribute.
pub(crate) fn disable_minutes() -> i64 {
    conf_number("login", "disable-minutes", 5)
}

/// `login/@history-store`: unless `false`, each login leaves a `UserLoginHistory` row.
pub(crate) fn login_history_store() -> bool {
    conf_text("login", "history-store", "true") != "false"
}

/// `login/@history-incorrect-password`: unless `false`, a failed login's history row keeps the password tried.
pub(crate) fn login_history_incorrect_password() -> bool {
    conf_text("login", "history-incorrect-password", "true") != "false"
}

fn count_matching(password: &str, pred: impl Fn(char) -> bool) -> i64 {
    i64::try_from(password.chars().filter(|ch| pred(*ch)).count()).unwrap_or(i64::MAX)
}

fn conf_number(child: &str, name: &str, fallback: i64) -> i64 {
    conf_text_opt(child, name)
        .and_then(|text| text.parse().ok())
        .unwrap_or(fallback)
}

fn conf_text(child: &str, name: &str, fallback: &str) -> String {
    conf_text_opt(child, name).unwrap_or_else(|| fallback.to_string())
}

fn conf_text_opt(child: &str, name: &str) -> Option<String> {
    ExecutionContext::with_current(|ctx| ctx.factory().user_facade_attr(child, name)).flatten()
}

#[cfg(test)]
mod tests {
    use super::password_acceptable;

    #[test]
    fn password_acceptable_rejects_secret_and_accepts_a_mixed_password() {
        let short = password_acceptable("secret").unwrap_err();
        assert!(short.to_string().contains("shorter"), "{short}");
        let no_digit = password_acceptable("Secret!!").unwrap_err();
        assert!(no_digit.to_string().contains("digit"), "{no_digit}");
        let no_other = password_acceptable("Secret12").unwrap_err();
        assert!(no_other.to_string().contains("other"), "{no_other}");
        password_acceptable("Secret1!").unwrap();
    }
}
