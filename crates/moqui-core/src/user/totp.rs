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

use hmac::{KeyInit as _, Mac as _};

/// RFC 4648 base32, the alphabet of authenticator secrets.
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// The shortest key RFC 4226 allows: 128 bits.
const MIN_KEY_BYTES: usize = 16;

pub(super) const DIGITS: u32 = 6;
pub(super) const STEP: u64 = 30;
pub(super) const SKEW: u64 = 1;

/// `bytes` as base32, without padding.
pub(super) fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// The bytes of base32 `text` (upper case; trailing `=` padding allowed); `None` for any other character.
pub(super) fn from_base32(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for ch in text.bytes() {
        let value = ALPHABET.iter().position(|&letter| letter == ch)? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// The key of base32 `secret` (any case, spaces around it), when it is long enough to be one.
pub(super) fn key(secret: &str) -> Option<Vec<u8>> {
    from_base32(&secret.trim().to_ascii_uppercase()).filter(|key| key.len() >= MIN_KEY_BYTES)
}

/// The code of `key` for the step `counter` (RFC 4226's HOTP, dynamic truncation), zero padded to [`DIGITS`].
pub(super) fn code(key: &[u8], counter: u64) -> String {
    let Ok(mut mac) = hmac::Hmac::<sha1::Sha1>::new_from_slice(key) else {
        return String::new();
    };
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0F);
    let value = u32::from_be_bytes([
        digest[offset],
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]) & 0x7FFF_FFFF;
    format!("{:0width$}", value % 10u32.pow(DIGITS), width = DIGITS as usize)
}

/// Whether `entered` is the code of `key` at `time` (seconds since the epoch), or one step either side. Only
/// [`DIGITS`] ASCII digits can match, and each comparison takes the same time whatever it finds.
pub(super) fn matches(key: &[u8], entered: &str, time: u64) -> bool {
    if entered.len() != DIGITS as usize || !entered.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let step = time / STEP;
    let mut found = false;
    for counter in step.saturating_sub(SKEW)..=step + SKEW {
        let expected = code(key, counter);
        let differ = expected
            .bytes()
            .zip(entered.bytes())
            .fold(0u8, |differ, (left, right)| differ | (left ^ right));
        found |= differ == 0;
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
    let secret = base32(key);
    Some(match issuer {
        Some(issuer) => {
            let issuer = percent_encoded(issuer);
            format!(
                "otpauth://totp/{issuer}:{}?secret={secret}&issuer={issuer}",
                percent_encoded(label)
            )
        }
        None => format!("otpauth://totp/{}?secret={secret}", percent_encoded(label)),
    })
}

/// `text` with every byte but ASCII letters, digits and `-_.~` percent encoded.
fn percent_encoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{base32, code, from_base32, key, matches, uri};

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
