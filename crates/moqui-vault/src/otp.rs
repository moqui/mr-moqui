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

//! One-time codes: HOTP (RFC 4226) and TOTP (RFC 6238) on HMAC-SHA1, SHA-256 or SHA-512, the base32 of authenticator
//! secrets, the `otpauth://totp/` URI an app scans, and the `otpauth-migration://` export of Google Authenticator. The
//! server's sign-in factor checks codes with it (`moqui-core` `user::totp`), and the vault module makes the codes of a
//! vault item's seed with it (`ops` `item_otp`), so a page shows a code and never holds the seed
//! (docs/security/Vaults.md, One-time codes).

use std::fmt;

use zeroize::Zeroizing;

use crate::Error;

/// RFC 4648 base32, the alphabet of authenticator secrets.
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// The longest period accepted, in seconds: an hour.
const MAX_PERIOD: u64 = 3600;
/// The longest secret accepted, in bytes: past any real authenticator's.
const MAX_SECRET: usize = 128;
/// The most entries one migration export holds: past Google Authenticator's batch size.
const MAX_ENTRIES: usize = 1000;

/// The HMAC a code is made with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    Sha1,
    Sha256,
    Sha512,
}

impl Algorithm {
    /// The name an `otpauth` URI uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Algorithm::Sha1 => "SHA1",
            Algorithm::Sha256 => "SHA256",
            Algorithm::Sha512 => "SHA512",
        }
    }

    fn of(name: &str) -> Option<Algorithm> {
        match name.to_ascii_uppercase().replace('-', "").as_str() {
            "SHA1" => Some(Algorithm::Sha1),
            "SHA256" => Some(Algorithm::Sha256),
            "SHA512" => Some(Algorithm::Sha512),
            _ => None,
        }
    }
}

/// A seed: the key and how its codes are made, with the names an app shows it under. `Debug` leaves out the key.
#[derive(Clone, PartialEq, Eq)]
pub struct Seed {
    pub key: Zeroizing<Vec<u8>>,
    pub algorithm: Algorithm,
    /// 6 to 8.
    pub digits: u32,
    /// Seconds, 1 to an hour.
    pub period: u64,
    pub issuer: Option<String>,
    pub account: Option<String>,
}

impl fmt::Debug for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Seed")
            .field("key", &"<redacted>")
            .field("algorithm", &self.algorithm)
            .field("digits", &self.digits)
            .field("period", &self.period)
            .field("issuer", &self.issuer)
            .field("account", &self.account)
            .finish()
    }
}

/// The codes of a seed at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Codes {
    pub code: String,
    /// The code of the period after.
    pub next: String,
    /// Until the code changes.
    pub remaining_ms: u64,
}

/// `bytes` as base32, without padding.
#[must_use]
pub fn base32(bytes: &[u8]) -> String {
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
#[must_use]
pub fn from_base32(text: &str) -> Option<Vec<u8>> {
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

/// The code of `key` for the step `counter`: RFC 4226's HOTP with its dynamic truncation, zero padded to `digits`.
#[must_use]
pub fn hotp(key: &[u8], counter: u64, algorithm: Algorithm, digits: u32) -> String {
    let message = counter.to_be_bytes();
    let digest: Vec<u8> = match algorithm {
        Algorithm::Sha1 => mac::<hmac::Hmac<sha1::Sha1>>(key, &message),
        Algorithm::Sha256 => mac::<hmac::Hmac<sha2::Sha256>>(key, &message),
        Algorithm::Sha512 => mac::<hmac::Hmac<sha2::Sha512>>(key, &message),
    };
    let Some(&last) = digest.last() else {
        return String::new();
    };
    let offset = usize::from(last & 0x0F);
    let Some(window) = digest.get(offset..offset + 4) else {
        return String::new();
    };
    let value = u32::from_be_bytes([window[0], window[1], window[2], window[3]]) & 0x7FFF_FFFF;
    let digits = digits.clamp(1, 9);
    format!("{:0width$}", value % 10u32.pow(digits), width = digits as usize)
}

fn mac<M: hmac::Mac + hmac::KeyInit>(key: &[u8], message: &[u8]) -> Vec<u8> {
    let Ok(mut mac) = <M as hmac::KeyInit>::new_from_slice(key) else {
        return Vec::new();
    };
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

/// The codes of `seed` at `now_ms` (milliseconds since the epoch; a time before it counts as the epoch).
#[must_use]
pub fn codes(seed: &Seed, now_ms: i64) -> Codes {
    let now = u64::try_from(now_ms).unwrap_or(0);
    let period_ms = seed.period * 1000;
    let step = now / period_ms;
    Codes {
        code: hotp(&seed.key, step, seed.algorithm, seed.digits),
        next: hotp(&seed.key, step + 1, seed.algorithm, seed.digits),
        remaining_ms: period_ms - now % period_ms,
    }
}

/// A seed as an authenticator takes it: an `otpauth://totp/` URI, or a bare base32 secret (any case, with spaces or
/// dashes between its groups, as sites print it) with the defaults of SHA1, six digits and 30 seconds.
///
/// # Errors
///
/// [`Error::Format`] for text that is neither, a counter (`hotp`) or Steam seed, or parameters out of range.
pub fn parse(text: &str) -> Result<Seed, Error> {
    let text = text.trim();
    if starts_with_ignore_case(text, "otpauth-migration:") {
        return Err(Error::Format("a migration export holds several seeds: import it"));
    }
    if starts_with_ignore_case(text, "otpauth://hotp") {
        return Err(Error::Format("counter (HOTP) codes are not supported"));
    }
    if starts_with_ignore_case(text, "otpauth://totp/") {
        return parse_uri(&text["otpauth://totp/".len()..]);
    }
    if starts_with_ignore_case(text, "otpauth:") {
        return Err(Error::Format("an otpauth URI of another kind"));
    }
    Ok(Seed {
        key: secret_of(text)?,
        algorithm: Algorithm::Sha1,
        digits: 6,
        period: 30,
        issuer: None,
        account: None,
    })
}

/// The `otpauth://totp/` URI of `seed`, the form a vault item keeps: the label (`Issuer:account`) and issuer percent
/// encoded, the secret as base32, and the algorithm, digits and period only where they are not an app's defaults.
#[must_use]
pub fn uri(seed: &Seed) -> String {
    let issuer = seed.issuer.as_deref().filter(|text| !text.is_empty());
    let account = seed.account.as_deref().unwrap_or("");
    let label = match issuer {
        Some(issuer) => format!("{}:{}", percent_encoded(issuer), percent_encoded(account)),
        None => percent_encoded(account),
    };
    let mut out = format!("otpauth://totp/{label}?secret={}", base32(&seed.key));
    if let Some(issuer) = issuer {
        out.push_str(&format!("&issuer={}", percent_encoded(issuer)));
    }
    if seed.algorithm != Algorithm::Sha1 {
        out.push_str(&format!("&algorithm={}", seed.algorithm.as_str()));
    }
    if seed.digits != 6 {
        out.push_str(&format!("&digits={}", seed.digits));
    }
    if seed.period != 30 {
        out.push_str(&format!("&period={}", seed.period));
    }
    out
}

/// The time-based seeds of a Google Authenticator export, `otpauth-migration://offline?data=` (base64 of its
/// `MigrationPayload`). A counter seed in it is left out, never refused: the others still import.
///
/// # Errors
///
/// [`Error::Format`] when the URI or its payload does not read.
pub fn migration(text: &str) -> Result<Vec<Seed>, Error> {
    let text = text.trim();
    if !starts_with_ignore_case(text, "otpauth-migration://") {
        return Err(Error::Format("not an otpauth-migration URI"));
    }
    let query = text.split_once('?').map_or("", |(_, query)| query);
    let data = query_value(query, "data").ok_or(Error::Format("a migration URI without its data"))?;
    let payload = Zeroizing::new(base64(&data).ok_or(Error::Format("a migration's data is not base64"))?);
    let mut seeds = Vec::new();
    let mut reader = Proto::new(&payload);
    while let Some((field, value)) = reader.next()? {
        if let (1, Wire::Bytes(entry)) = (field, value) {
            if seeds.len() >= MAX_ENTRIES {
                return Err(Error::Format("a migration of too many seeds"));
            }
            if let Some(seed) = migration_entry(entry)? {
                seeds.push(seed);
            }
        }
    }
    Ok(seeds)
}

/// One `OtpParameters`: secret 1, name 2, issuer 3, algorithm 4 (1 SHA1, 2 SHA256, 3 SHA512), digits 5 (1 six,
/// 2 eight), type 6 (1 HOTP, 2 TOTP). `None` for a counter seed or an algorithm this does not make.
fn migration_entry(bytes: &[u8]) -> Result<Option<Seed>, Error> {
    let mut key = Zeroizing::new(Vec::new());
    let (mut name, mut issuer) = (String::new(), String::new());
    let (mut algorithm, mut digits, mut kind) = (Some(Algorithm::Sha1), 6, 2);
    let mut reader = Proto::new(bytes);
    while let Some((field, value)) = reader.next()? {
        match (field, value) {
            (1, Wire::Bytes(secret)) => key = Zeroizing::new(secret.to_vec()),
            (2, Wire::Bytes(text)) => name = String::from_utf8_lossy(text).into_owned(),
            (3, Wire::Bytes(text)) => issuer = String::from_utf8_lossy(text).into_owned(),
            (4, Wire::Varint(value)) => {
                algorithm = match value {
                    0 | 1 => Some(Algorithm::Sha1),
                    2 => Some(Algorithm::Sha256),
                    3 => Some(Algorithm::Sha512),
                    _ => None,
                }
            }
            (5, Wire::Varint(value)) => digits = if value == 2 { 8 } else { 6 },
            (6, Wire::Varint(value)) => kind = value,
            _ => {}
        }
    }
    let Some(algorithm) = algorithm.filter(|_| kind != 1) else {
        return Ok(None);
    };
    if key.is_empty() || key.len() > MAX_SECRET {
        return Err(Error::Format("a migration seed's secret"));
    }
    let (label_issuer, account) = label_parts(&name);
    Ok(Some(Seed {
        key,
        algorithm,
        digits,
        period: 30,
        issuer: Some(issuer).filter(|text| !text.is_empty()).or(label_issuer),
        account: Some(account).filter(|text| !text.is_empty()),
    }))
}

/// `Issuer:account` or `account`, the label of a URI or of a migration's name.
fn label_parts(label: &str) -> (Option<String>, String) {
    match label.split_once(':') {
        Some((issuer, account)) if !issuer.trim().is_empty() => {
            (Some(issuer.trim().to_string()), account.trim().to_string())
        }
        Some((_, account)) => (None, account.trim().to_string()),
        None => (None, label.trim().to_string()),
    }
}

/// `rest`, the URI after `otpauth://totp/`: `label?query`.
fn parse_uri(rest: &str) -> Result<Seed, Error> {
    let (label, query) = rest.split_once('?').unwrap_or((rest, ""));
    let label = percent_decoded(label).ok_or(Error::Format("an otpauth label"))?;
    let (label_issuer, account) = label_parts(&label);
    let secret = query_value(query, "secret").ok_or(Error::Format("an otpauth URI without its secret"))?;
    let secret = Zeroizing::new(secret);
    let algorithm = match query_value(query, "algorithm") {
        Some(name) => {
            Algorithm::of(&name).ok_or(Error::Format("an otpauth algorithm other than SHA1, SHA256, SHA512"))?
        }
        None => Algorithm::Sha1,
    };
    let digits = match query_value(query, "digits") {
        Some(text) => text
            .parse::<u32>()
            .ok()
            .filter(|digits| (6..=8).contains(digits))
            .ok_or(Error::Format("otpauth digits other than 6 to 8"))?,
        None => 6,
    };
    let period = match query_value(query, "period") {
        Some(text) => text
            .parse::<u64>()
            .ok()
            .filter(|period| (1..=MAX_PERIOD).contains(period))
            .ok_or(Error::Format("an otpauth period other than 1 to 3600 seconds"))?,
        None => 30,
    };
    if query_value(query, "encoder").is_some_and(|encoder| encoder.eq_ignore_ascii_case("steam")) {
        return Err(Error::Format("Steam codes are not supported"));
    }
    let issuer = query_value(query, "issuer").filter(|text| !text.trim().is_empty());
    Ok(Seed {
        key: secret_of(&secret)?,
        algorithm,
        digits,
        period,
        issuer: issuer.map(|text| text.trim().to_string()).or(label_issuer),
        account: Some(account).filter(|text| !text.is_empty()),
    })
}

/// The key of a base32 secret as people copy it: any case, `=` padding, and spaces or dashes between groups.
fn secret_of(text: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    let cleaned: Zeroizing<String> = Zeroizing::new(
        text.chars()
            .filter(|ch| !ch.is_whitespace() && *ch != '-')
            .map(|ch| ch.to_ascii_uppercase())
            .collect(),
    );
    let key = Zeroizing::new(from_base32(&cleaned).ok_or(Error::Format("a seed that is not base32"))?);
    if key.is_empty() || key.len() > MAX_SECRET {
        return Err(Error::Format("a seed of no key, or too long a key"));
    }
    Ok(key)
}

/// The decoded value of `name` in a URI query (`+` is a space, as a form encodes it).
fn query_value(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key.eq_ignore_ascii_case(name) {
            percent_decoded(&value.replace('+', " "))
        } else {
            None
        }
    })
}

/// `text` with its `%XX` escapes decoded; `None` for a bad escape or bytes that are not UTF-8.
fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = text.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
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

/// Base64 in either alphabet (standard or URL safe; a space is a `+` a URL lost), padding optional.
fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for ch in text.trim().trim_end_matches('=').bytes() {
        let value = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' | b'-' | b' ' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

/// A protobuf field's value, of the two wire types an export uses (fixed-width ones are skipped).
enum Wire<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Skipped,
}

/// Reads protobuf fields one at a time, refusing a truncated or malformed message.
struct Proto<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Proto<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Proto { bytes, at: 0 }
    }

    fn varint(&mut self) -> Result<u64, Error> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self
                .bytes
                .get(self.at)
                .ok_or(Error::Format("a truncated migration payload"))?;
            self.at += 1;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::Format("a migration payload's varint"))
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let end = self
            .at
            .checked_add(length)
            .ok_or(Error::Format("a migration payload's length"))?;
        let bytes = self
            .bytes
            .get(self.at..end)
            .ok_or(Error::Format("a truncated migration payload"))?;
        self.at = end;
        Ok(bytes)
    }

    /// The next field's number and value, `None` at the end.
    fn next(&mut self) -> Result<Option<(u64, Wire<'a>)>, Error> {
        if self.at >= self.bytes.len() {
            return Ok(None);
        }
        let tag = self.varint()?;
        let value = match tag & 7 {
            0 => Wire::Varint(self.varint()?),
            1 => {
                self.take(8)?;
                Wire::Skipped
            }
            2 => {
                let length =
                    usize::try_from(self.varint()?).map_err(|_| Error::Format("a migration payload's length"))?;
                Wire::Bytes(self.take(length)?)
            }
            5 => {
                self.take(4)?;
                Wire::Skipped
            }
            _ => return Err(Error::Format("a migration payload's wire type")),
        };
        Ok(Some((tag >> 3, value)))
    }
}

#[cfg(test)]
#[path = "otp_tests.rs"]
mod tests;
