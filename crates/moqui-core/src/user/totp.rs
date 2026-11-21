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

//! TOTP (RFC 6238) on HMAC-SHA1, as Java's `DefaultCodeVerifier` and the authenticator apps use it: a base32 secret,
//! six digits, a 30 second step, and one step either side accepted. The `otpauth://totp/` URI an app scans.

use moqui_vault::otp::{self, Algorithm};

/// The shortest key RFC 4226 allows: 128 bits.
const MIN_KEY_BYTES: usize = 16;

pub(super) const DIGITS: u32 = 6;
pub(super) const STEP: u64 = 30;
pub(super) const SKEW: u64 = 1;

/// `bytes` as base32, without padding.
pub(super) fn base32(bytes: &[u8]) -> String {
    otp::base32(bytes)
}

/// The key of base32 `secret` (any case, spaces around it), when it is long enough to be one.
pub(super) fn key(secret: &str) -> Option<Vec<u8>> {
    otp::from_base32(&secret.trim().to_ascii_uppercase()).filter(|key| key.len() >= MIN_KEY_BYTES)
}

/// The code of `key` for the step `counter`, six digits on HMAC-SHA1 ([`otp::hotp`]).
pub(super) fn code(key: &[u8], counter: u64) -> String {
    otp::hotp(key, counter, Algorithm::Sha1, DIGITS)
}

/// Whether `entered` is the code of `key` at `time` (seconds since the epoch), or one step either side. Only
/// [`DIGITS`] ASCII digits can match, and each comparison takes the same time whatever it finds.
pub(super) fn matches(key: &[u8], entered: &str, time: u64) -> bool {
    matching_step(key, entered, time, None).is_some()
}

/// The step (`time / STEP`, one either side) whose code of `key` is `entered`, when it is after `after`: a code is taken
/// once, so a step at or before the last one accepted is refused (SEC-WEB-12). Only [`DIGITS`] ASCII digits can match,
/// and each comparison takes the same time whatever it finds.
pub(super) fn matching_step(key: &[u8], entered: &str, time: u64, after: Option<u64>) -> Option<u64> {
    if entered.len() != DIGITS as usize || !entered.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let step = time / STEP;
    let mut found = None;
    for counter in step.saturating_sub(SKEW)..=step + SKEW {
        let expected = code(key, counter);
        let differ = expected
            .bytes()
            .zip(entered.bytes())
            .fold(0u8, |differ, (left, right)| differ | (left ^ right));
        if differ == 0 && after.is_none_or(|last| counter > last) {
            found = Some(counter);
        }
    }
    found
}

/// The `otpauth://totp/` URI of `secret` for `label`, under `issuer` when there is one: the label and issuer percent
/// encoded, the secret as base32, and no `digits`, `algorithm` or `period` (six, SHA1 and 30 are an app's defaults).
/// `None` when the label is empty or either holds a `:`, which the URI's label cannot carry.
pub(super) fn uri(label: &str, issuer: Option<&str>, key: &[u8]) -> Option<String> {
    if label.is_empty() || label.contains(':') || issuer.is_some_and(|issuer| issuer.contains(':')) {
        return None;
    }
    Some(otp::uri(&otp::Seed {
        key: zeroize::Zeroizing::new(key.to_vec()),
        algorithm: Algorithm::Sha1,
        digits: DIGITS,
        period: STEP,
        issuer: issuer.map(str::to_string),
        account: Some(label.to_string()),
    }))
}

#[cfg(test)]
mod tests {
    use super::{base32, code, key, matches, matching_step, uri};
    use moqui_vault::otp::from_base32;

    /// RFC 6238's SHA1 test key, its codes cut to six digits.
    #[test]
    fn the_rfc_6238_vectors() {
        let key = b"12345678901234567890";
        for (time, eight) in [
            (59_u64, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ] {
            assert_eq!(code(key, time / 30), eight[2..], "{time}");
            assert!(matches(key, &eight[2..], time));
            assert!(matches(key, &eight[2..], time + 30), "one step after");
            assert!(!matches(key, &eight[2..], time + 90), "three steps after");
        }
        assert!(!matches(key, "+87082", 59));
        assert!(!matches(key, "2870820", 59));
    }

    /// A code is taken once (SEC-WEB-12): its step, and any step before the last one taken, is refused.
    #[test]
    fn a_code_is_taken_for_a_step_after_the_last_one() {
        let key = b"12345678901234567890";
        let now = 1_111_111_111;
        let step = now / 30;
        let current = code(key, step);
        assert_eq!(matching_step(key, &current, now, None), Some(step));
        assert_eq!(matching_step(key, &current, now, Some(step - 1)), Some(step));
        assert_eq!(
            matching_step(key, &current, now, Some(step)),
            None,
            "the same code again"
        );
        assert_eq!(
            matching_step(key, &code(key, step - 1), now, Some(step)),
            None,
            "an older code"
        );
        assert_eq!(
            matching_step(key, &code(key, step + 1), now, Some(step)),
            Some(step + 1)
        );
    }

    #[test]
    fn base32_round_trips_and_refuses_what_is_not_base32() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(from_base32("MZXW6YTBOI======").unwrap(), b"foobar");
        for length in 0..40 {
            let bytes: Vec<u8> = (0..length).map(|at| (at * 37 + 11) as u8).collect();
            assert_eq!(from_base32(&base32(&bytes)).unwrap(), bytes, "{length}");
        }
        assert!(from_base32("MZXW1").is_none());
        assert_eq!(key("  mzxw6ytboi  "), None, "too short a key");
        assert_eq!(key(&base32(&[7; 20]).to_ascii_lowercase()), Some(vec![7; 20]));
    }

    #[test]
    fn the_uri_an_app_scans() {
        let key = [0x48; 20];
        assert_eq!(
            uri("ada", None, &key).unwrap(),
            format!("otpauth://totp/ada?secret={}", base32(&key))
        );
        assert_eq!(
            uri("ada lovelace@example.com", Some("Moqui Demo"), &key).unwrap(),
            format!(
                "otpauth://totp/Moqui%20Demo:ada%20lovelace%40example.com?secret={}&issuer=Moqui%20Demo",
                base32(&key)
            )
        );
        assert!(uri("", None, &key).is_none());
        assert!(uri("a:b", None, &key).is_none());
        assert!(uri("ada", Some("x:y"), &key).is_none());
    }
}
