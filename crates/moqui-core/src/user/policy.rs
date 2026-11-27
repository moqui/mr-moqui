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
    let PasswordRules {
        min_length,
        min_digits,
        min_others,
    } = password_rules();
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

/// The `user-facade/password` minimums: the length, the digits, and the characters that are neither letter nor digit.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PasswordRules {
    pub(crate) min_length: i64,
    pub(crate) min_digits: i64,
    pub(crate) min_others: i64,
}

pub(crate) fn password_rules() -> PasswordRules {
    PasswordRules {
        min_length: conf_number("password", "min-length", 8),
        min_digits: conf_number("password", "min-digits", 1),
        min_others: conf_number("password", "min-others", 1),
    }
}

pub(crate) fn history_limit() -> i64 {
    conf_number("password", "history-limit", 5)
}

/// `password/@change-weeks`, 104 as the shipped conf and the XSD give it (SEC-WEB-92: Java's 12 is not used).
pub(crate) fn change_weeks() -> i64 {
    conf_number("password", "change-weeks", 104)
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

/// `login-key/@device-expire-hours`, 2160 (90 days): a device's key (`get_device_key`), which a desktop or mobile
/// client keeps instead of the password, so it lasts as an access token does. A fraction is allowed.
pub(crate) fn login_key_device_expire_hours() -> f64 {
    conf_text_opt("login-key", "device-expire-hours")
        .and_then(|text| text.parse().ok())
        .unwrap_or(2160.0)
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

/// `login/@disable-minutes`, 5 as the shipped conf and the XSD give it.
pub(crate) fn disable_minutes() -> i64 {
    conf_number("login", "disable-minutes", 5)
}

/// `login/@history-store`: unless `false`, each login leaves a `UserLoginHistory` row.
/// `login/@reauth-email-codes-per-hour` (5): the re-authentication codes mailed to an account's address in an hour.
pub(crate) fn reauth_email_codes_per_hour() -> i64 {
    conf_number("login", "reauth-email-codes-per-hour", 5).max(0)
}

pub(crate) fn login_history_store() -> bool {
    conf_text("login", "history-store", "true") != "false"
}

/// `login/@sso-linked-service`: the service told of each SSO sign-in that links or confirms an account's provider id
/// (`org.moqui.sso.AuthServices.handle#Callback`); none when empty.
pub(crate) fn sso_linked_service() -> String {
    conf_text("login", "sso-linked-service", "").trim().to_string()
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

    /// The value `MoquiDefaultConf.xml` gives `attr` on the `user-facade` element `child`.
    fn shipped(conf: &str, child: &str, attr: &str) -> String {
        let at = conf
            .find(&format!("<{child} "))
            .unwrap_or_else(|| panic!("no <{child}> in the conf"));
        let element = &conf[at..at + conf[at..].find('>').unwrap()];
        let start = element
            .find(&format!(" {attr}=\""))
            .unwrap_or_else(|| panic!("no {child}@{attr}"))
            + attr.len()
            + 3;
        element[start..start + element[start..].find('"').unwrap()].to_string()
    }

    /// Each fallback the code uses when an attribute is missing (no execution context, here) is the default the
    /// shipped conf writes, so a missing attribute never means another policy: `change-weeks` was 12 in the code and the
    /// XSD and 104 in the conf (SEC-WEB-92).
    #[test]
    fn the_policy_fallbacks_are_the_shipped_conf_defaults() {
        let conf = include_str!("../../../../runtime/components/base/framework/conf/MoquiDefaultConf.xml");
        let xsd = include_str!("../../../../runtime/components/base/framework/xsd/mr-moqui-conf.xsd");
        let rules = super::password_rules();
        let numbers = [
            ("password", "min-length", rules.min_length),
            ("password", "min-digits", rules.min_digits),
            ("password", "min-others", rules.min_others),
            ("password", "history-limit", super::history_limit()),
            ("password", "change-weeks", super::change_weeks()),
            ("password", "email-expire-hours", super::email_expire_hours()),
            ("login", "max-failures", super::max_failures()),
            ("login", "disable-minutes", super::disable_minutes()),
            (
                "login",
                "reauth-email-codes-per-hour",
                super::reauth_email_codes_per_hour(),
            ),
        ];
        for (child, attr, fallback) in numbers {
            assert_eq!(shipped(conf, child, attr), fallback.to_string(), "{child}@{attr}");
            let declared = format!("name=\"{attr}\" type=");
            if let Some(at) = xsd.find(&declared) {
                let line = &xsd[at..at + xsd[at..].find('>').unwrap()];
                assert!(
                    line.contains(&format!("default=\"{fallback}\"")),
                    "the XSD default of {child}@{attr}: {line}"
                );
            }
        }
        assert_eq!(
            shipped(conf, "password", "encrypt-hash-type"),
            super::password_hash_type()
        );
        assert_eq!(
            shipped(conf, "login-key", "encrypt-hash-type"),
            super::login_key_hash_type()
        );
        assert_eq!(
            shipped(conf, "login-key", "expire-hours"),
            super::login_key_expire_hours().to_string()
        );
        assert_eq!(
            shipped(conf, "login-key", "device-expire-hours"),
            super::login_key_device_expire_hours().to_string()
        );
        assert_eq!(
            shipped(conf, "password", "email-require-change"),
            super::email_require_change().to_string()
        );
        assert_eq!(
            shipped(conf, "login", "history-store"),
            super::login_history_store().to_string()
        );
    }

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
