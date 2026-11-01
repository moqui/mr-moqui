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

//! Password hashing. New passwords are argon2id PHC strings. Legacy checks use Shiro SimpleHash.

use moqui_lib::codec::{constant_time_eq, hex_encode};

use argon2::{
    password_hash::{PasswordHasher, PasswordVerifier},
    Argon2,
};
use base64::Engine;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::context::Error;

use super::policy;

const ARGON2: &str = "argon2";

/// Hash `plain` with argon2id. The salt comes from `getrandom` inside the argon2 crate.
pub fn hash_password(plain: &str) -> Result<String, Error> {
    let hashed = Argon2::default()
        .hash_password(plain.as_bytes())
        .map_err(|err| Error::new(format!("argon2 hash failed: {err}")))?;
    Ok(hashed.to_string())
}

/// Shiro `SimpleHash`: one iteration, salt bytes then password bytes. Hex is lowercase.
///
/// `SHA` is SHA-1, matching Java `MessageDigest`. An empty `hash_type` uses the conf default.
/// Base64 is the standard alphabet when `base64_encode` is set.
pub fn simple_hash(password: &str, salt: &str, hash_type: &str, base64_encode: bool) -> Result<String, Error> {
    let digest = digest_bytes(hash_type, salt.as_bytes(), password.as_bytes())?;
    if base64_encode {
        Ok(base64::engine::general_purpose::STANDARD.encode(digest))
    } else {
        Ok(hex_encode(&digest))
    }
}

/// The SCRAM-SHA-256 verifiers of a password (RFC 5802, RFC 7677): `StoredKey` and `ServerKey` with `salt` and
/// `iterations`. The XMPP server (moqui-xmpp `scram.rs`) checks a client's proof against them; the password is not kept.
pub fn scram_sha256_verifier(password: &str, salt: &[u8], iterations: u32) -> (Vec<u8>, Vec<u8>) {
    use hmac::{KeyInit, Mac};
    let salted = pbkdf2::pbkdf2_hmac_array::<Sha256, 32>(password.as_bytes(), salt, iterations);
    let keyed = |label: &[u8]| {
        let mut mac = hmac::Hmac::<Sha256>::new_from_slice(&salted).expect("HMAC takes a key of any length");
        mac.update(label);
        mac.finalize().into_bytes().to_vec()
    };
    (Sha256::digest(keyed(b"Client Key")).to_vec(), keyed(b"Server Key"))
}

/// Login-key hash. Uses `login-key/@encrypt-hash-type`, an empty salt, and hex. Never argon2.
pub fn hash_login_key(login_key: &str) -> Result<String, Error> {
    simple_hash(login_key, "", &policy::login_key_hash_type(), false)
}

pub(crate) fn is_argon2(hash_type: &str, stored: &str) -> bool {
    hash_type.eq_ignore_ascii_case(ARGON2) || stored.starts_with("$argon2")
}

/// True when `plain` matches `stored`. Argon2 is verified with the argon2 crate, not SimpleHash.
pub(crate) fn passwords_match(
    plain: &str,
    stored: &str,
    hash_type: &str,
    salt: &str,
    base64_encode: bool,
) -> Result<bool, Error> {
    if stored.is_empty() {
        return Ok(false);
    }
    if is_argon2(hash_type, stored) {
        return verify_argon2(plain, stored);
    }
    let hashed = simple_hash(plain, salt, hash_type, base64_encode)?;
    Ok(constant_time_eq(hashed.as_bytes(), stored.as_bytes()))
}

/// Run a verification that always fails, so an unknown or locked account takes as long as a wrong password.
pub(crate) fn burn_verify(plain: &str) {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let stored = DUMMY.get_or_init(|| hash_password("burn-verify-placeholder").unwrap_or_default());
    let _ = passwords_match(plain, stored, ARGON2, "", false);
}

fn verify_argon2(plain: &str, stored: &str) -> Result<bool, Error> {
    match Argon2::default().verify_password(plain.as_bytes(), stored) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::PasswordInvalid) => Ok(false),
        // A malformed stored hash is a failed login, the same as a wrong password.
        Err(err) => {
            tracing::warn!(error = %err, "stored argon2 hash could not be verified");
            Ok(false)
        }
    }
}

fn digest_bytes(hash_type: &str, salt: &[u8], password: &[u8]) -> Result<Vec<u8>, Error> {
    let name = if hash_type.trim().is_empty() {
        policy::password_hash_type()
    } else {
        hash_type.to_string()
    };
    match name.to_ascii_uppercase().as_str() {
        "SHA-256" | "SHA256" => Ok(digest_with::<Sha256>(salt, password)),
        "SHA" | "SHA-1" | "SHA1" => Ok(digest_with::<Sha1>(salt, password)),
        other => Err(Error::new(format!("unknown hash type {other}"))),
    }
}

fn digest_with<D: Digest>(salt: &[u8], password: &[u8]) -> Vec<u8> {
    let mut hasher = D::new();
    hasher.update(salt);
    hasher.update(password);
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_hash_matches_the_shiro_hex_and_base64_vectors() {
        let sha256 = simple_hash("secret", "s4lt", "SHA-256", false).unwrap();
        assert_eq!(
            sha256,
            "56fb59a0918a8e898cc46532428936acd1717e608c197eef668b7c6ef2f3c655"
        );
        let empty_salt = simple_hash("secret", "", "SHA-256", false).unwrap();
        assert_eq!(
            empty_salt,
            "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"
        );
        let sha1 = simple_hash("secret", "s4lt", "SHA", false).unwrap();
        assert_eq!(sha1, "29acd6287bbcbaa53dc5ed18803e514e97f10479");
        let encoded = simple_hash("secret", "s4lt", "SHA-256", true).unwrap();
        assert_eq!(encoded, "VvtZoJGKjomMxGUyQok2rNFxfmCMGX7vZot8bvLzxlU=");
    }

    #[test]
    fn hash_password_is_an_argon2id_phc_string() {
        let stored = hash_password("Secret1!").unwrap();
        assert!(stored.starts_with("$argon2id$"), "{stored}");
        assert!(passwords_match("Secret1!", &stored, "argon2", "", false).unwrap());
        assert!(!passwords_match("Secret2!", &stored, "argon2", "", false).unwrap());
    }

    #[test]
    fn hash_login_key_stays_sha256_hex() {
        let hashed = hash_login_key("secret").unwrap();
        assert_eq!(
            hashed,
            "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"
        );
        assert!(!hashed.starts_with("$argon2"), "{hashed}");
    }
}
