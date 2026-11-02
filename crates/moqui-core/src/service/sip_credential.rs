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

//! `org.moqui.impl.SipDeviceServices.make#SipCredential` (plans/Comms-Framework.md §5.2 "Registrar for phones (V4)"):
//! a desk or soft phone's new SIP credentials. The password is random and strong, and only its digest verifiers are
//! kept: HA1 = H(username:realm:password) (RFC 7616 §3.4.2) for SHA-256, and for MD5 only for a legacy device
//! (RFC 8760: SHA-256 is preferred; MD5 only where a phone has nothing else). The password goes back once, for the
//! user to put in the phone. moqui-sip's `digest::ha1` computes the same.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use md5::Md5;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use sha2::{Digest, Sha256};

use crate::entity::EntityFacade;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, make_sip_credential)?;
    rust_service!(map, compute_sip_digest_response)?;
    Ok(())
}

/// `compute#SipDigestResponse` (plans/Edge-Node.md §5.4 "Voice (SIP)", E8): the response a phone's digest answer must
/// have, from its verifier `ha1` and what the edge node's registrar sent (`nonce`, `nc`, `cnonce`, `qop`, and `ha2`, its
/// H(method:uri)), as moqui-sip's `NonceBook::verify_ha1` computes it: H(ha1:nonce:nc:cnonce:auth:ha2) with qop auth,
/// H(ha1:nonce:ha2) with none. Empty for an algorithm other than SHA-256, SHA-512-256 or MD5, or another qop.
fn compute_sip_digest_response(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let text = |key: &str| {
        parameters
            .get(key)
            .filter(|v| !is_empty(v))
            .map(to_plain_string)
            .unwrap_or_default()
    };
    let response = digest_response(
        &text("algorithm"),
        &text("ha1").to_ascii_lowercase(),
        &text("nonce"),
        Some(text("qop")).filter(|qop| !qop.is_empty()).as_deref(),
        &text("nc"),
        &text("cnonce"),
        &text("ha2"),
    );
    Ok(BTreeMap::from([(
        "response".to_string(),
        Value::string(response.unwrap_or_default()),
    )]))
}

/// The digest of `text` under `algorithm` (RFC 7616's names), hex; none for another algorithm.
fn digest_hex(algorithm: &str, text: &str) -> Option<String> {
    Some(match algorithm.trim().to_ascii_uppercase().as_str() {
        "SHA-256" => hex(&Sha256::digest(text.as_bytes())),
        "SHA-512-256" => hex(&sha2::Sha512_256::digest(text.as_bytes())),
        "MD5" => hex(&Md5::digest(text.as_bytes())),
        _ => return None,
    })
}

/// The expected response of a digest answer, as moqui-sip's `NonceBook::verify_ha1` computes it.
fn digest_response(
    algorithm: &str,
    ha1: &str,
    nonce: &str,
    qop: Option<&str>,
    nc: &str,
    cnonce: &str,
    ha2: &str,
) -> Option<String> {
    match qop {
        Some(qop) if qop.eq_ignore_ascii_case("auth") && !nc.is_empty() && !cnonce.is_empty() => {
            digest_hex(algorithm, &format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"))
        }
        None => digest_hex(algorithm, &format!("{ha1}:{nonce}:{ha2}")),
        Some(_) => None,
    }
}

/// The password's characters: letters and digits without the ones read alike (0 O 1 l I), which a person types into a
/// phone's keypad or settings page.
const PASSWORD_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
/// 22 of 57 characters: about 128 bits.
const PASSWORD_LENGTH: usize = 22;
/// A generated username: digits, which every phone's settings take.
const USERNAME_LENGTH: usize = 10;

/// `count` characters of `alphabet`, uniform (rejection sampling over random bytes).
fn random_text(alphabet: &[u8], count: usize) -> Result<String, LibError> {
    let limit = 256 - (256 % alphabet.len());
    let mut out = String::with_capacity(count);
    let mut bytes = [0u8; 64];
    while out.len() < count {
        getrandom::fill(&mut bytes).map_err(|e| LibError::new(format!("no random bytes: {e}")))?;
        for byte in bytes {
            if usize::from(byte) < limit && out.len() < count {
                out.push(char::from(alphabet[usize::from(byte) % alphabet.len()]));
            }
        }
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// `make#SipCredential`: in `realm` (required), `username` (generated when empty: digits, not starting with 0) and
/// `legacy` (an MD5 verifier too); out `username`, `password` and `verifierList` ([{algorithm, ha1}]).
fn make_sip_credential(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    credential(parameters)
}

fn credential(parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, LibError> {
    let text = |key: &str| {
        parameters
            .get(key)
            .filter(|v| !is_empty(v))
            .map(to_plain_string)
            .unwrap_or_default()
    };
    let realm = text("realm");
    if realm.is_empty() {
        return Err(LibError::new("make#SipCredential needs a realm"));
    }
    let username = match text("username") {
        name if name.is_empty() => format!(
            "{}{}",
            random_text(b"123456789", 1)?,
            random_text(b"0123456789", USERNAME_LENGTH - 1)?
        ),
        name => name,
    };
    let legacy = matches!(text("legacy").as_str(), "true" | "Y" | "y");
    let password = random_text(PASSWORD_CHARS, PASSWORD_LENGTH)?;
    let secret = format!("{username}:{realm}:{password}");
    let mut verifiers = vec![("SHA-256", hex(&Sha256::digest(secret.as_bytes())))];
    if legacy {
        verifiers.push(("MD5", hex(&Md5::digest(secret.as_bytes()))));
    }
    let verifier_list = verifiers
        .into_iter()
        .map(|(algorithm, ha1)| {
            Value::Map(
                [
                    ("algorithm".to_string(), Value::string(algorithm)),
                    ("ha1".to_string(), Value::string(&ha1)),
                ]
                .into_iter()
                .collect(),
            )
        })
        .collect();
    let mut out = BTreeMap::new();
    out.insert("username".to_string(), Value::string(&username));
    out.insert("password".to_string(), Value::string(&password));
    out.insert("verifierList".to_string(), Value::List(verifier_list));
    Ok(out)
}

#[cfg(test)]
#[path = "sip_credential_tests.rs"]
mod tests;
