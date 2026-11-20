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

//! Fingerprints and key checks (plans/Vault-Instances.md §3.5). A fingerprint is a phrase two people compare by
//! another channel: 96 bits of SHA-256 over both public keys of a personal key (or over the server's lease key),
//! written as six proquints (pronounceable five-letter words, consonant-vowel-consonant-vowel-consonant, 16 bits
//! each). A check that was made is remembered as a [`KeyCheck`], signed by the checker, which a client verifies
//! before it relies on it.

use sha2::{Digest as _, Sha256};

use crate::names::refs;
use crate::personal::{PersonalKey, PersonalPublic};
use crate::sign::{Statement, VerifyingKey};
use crate::Error;

const CONSONANTS: &[u8; 16] = b"bdfghjklmnprstvz";
const VOWELS: &[u8; 4] = b"aiou";
const WORDS: usize = 6;

fn phrase(label: &[u8], keys: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(label);
    for key in keys {
        hasher.update(key);
    }
    let digest = hasher.finalize();
    let mut words = Vec::with_capacity(WORDS);
    for pair in digest.chunks(2).take(WORDS) {
        let n = u16::from_be_bytes([pair[0], pair[1]]);
        let c = |shift: u16| char::from(CONSONANTS[usize::from((n >> shift) & 15)]);
        let v = |shift: u16| char::from(VOWELS[usize::from((n >> shift) & 3)]);
        words.push([c(12), v(10), c(6), v(4), c(0)].iter().collect::<String>());
    }
    words.join("-")
}

/// The phrase for a personal key, over both its public keys.
pub fn personal(public: &PersonalPublic) -> String {
    phrase(
        b"moqui-vault fingerprint/1 personal",
        &[public.encrypt.as_bytes(), public.verify.as_bytes()],
    )
}

/// The phrase for the server's lease key.
pub fn lease_key(key: &VerifyingKey) -> String {
    phrase(b"moqui-vault fingerprint/1 lease-key", &[key.as_bytes()])
}

/// A checked key, as `VaultKeyCheck` stores it: who checked, with which personal key, which key (a personal key
/// with both its public halves, or the lease key, whose `checked_encrypt` is empty), when, and their signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyCheck {
    pub checker_user_id: String,
    pub checker_key_id: String,
    /// A `personalKeyId`, or `lease-key`.
    pub checked_key_id: String,
    pub checked_encrypt: Vec<u8>,
    pub checked_verify: Vec<u8>,
    /// Milliseconds since the epoch.
    pub checked_at: i64,
    pub signature: Vec<u8>,
}

impl KeyCheck {
    fn signed_parts(&self) -> Vec<Vec<u8>> {
        vec![
            self.checker_user_id.as_bytes().to_vec(),
            self.checker_key_id.as_bytes().to_vec(),
            self.checked_key_id.as_bytes().to_vec(),
            self.checked_encrypt.clone(),
            self.checked_verify.clone(),
            self.checked_at.to_be_bytes().to_vec(),
        ]
    }

    /// Records a check of a personal key.
    pub fn of_personal(
        checker_user_id: &str,
        checker_key_id: &str,
        checker: &PersonalKey,
        checked_key_id: &str,
        checked: &PersonalPublic,
        checked_at: i64,
    ) -> KeyCheck {
        Self::make(
            checker_user_id,
            checker_key_id,
            checker,
            checked_key_id,
            checked.encrypt.as_bytes().to_vec(),
            checked.verify.as_bytes().to_vec(),
            checked_at,
        )
    }

    /// Records a check of the server's lease key.
    pub fn of_lease_key(
        checker_user_id: &str,
        checker_key_id: &str,
        checker: &PersonalKey,
        lease_key: &VerifyingKey,
        checked_at: i64,
    ) -> KeyCheck {
        Self::make(
            checker_user_id,
            checker_key_id,
            checker,
            "lease-key",
            Vec::new(),
            lease_key.as_bytes().to_vec(),
            checked_at,
        )
    }

    fn make(
        checker_user_id: &str,
        checker_key_id: &str,
        checker: &PersonalKey,
        checked_key_id: &str,
        checked_encrypt: Vec<u8>,
        checked_verify: Vec<u8>,
        checked_at: i64,
    ) -> KeyCheck {
        let mut check = KeyCheck {
            checker_user_id: checker_user_id.to_string(),
            checker_key_id: checker_key_id.to_string(),
            checked_key_id: checked_key_id.to_string(),
            checked_encrypt,
            checked_verify,
            checked_at,
            signature: Vec::new(),
        };
        check.signature = checker
            .signing_key()
            .sign(Statement::KeyCheck, &refs(&check.signed_parts()))
            .to_vec();
        check
    }

    /// The client's check of a stored row with the checker's own key; then whether it covers `public`, the key it
    /// is about to rely on. A check is for one key: a changed key is checked again (§3.5).
    pub fn covers_personal(&self, checker: &VerifyingKey, public: &PersonalPublic) -> Result<(), Error> {
        checker.verify(Statement::KeyCheck, &refs(&self.signed_parts()), &self.signature)?;
        if self.checked_encrypt != public.encrypt.as_bytes() || self.checked_verify != public.verify.as_bytes() {
            return Err(Error::Refused("the key changed since it was checked"));
        }
        Ok(())
    }

    /// As [`KeyCheck::covers_personal`], for the lease key.
    pub fn covers_lease_key(&self, checker: &VerifyingKey, lease_key: &VerifyingKey) -> Result<(), Error> {
        checker.verify(Statement::KeyCheck, &refs(&self.signed_parts()), &self.signature)?;
        if self.checked_key_id != "lease-key" || self.checked_verify != lease_key.as_bytes() {
            return Err(Error::Refused("the lease key changed since it was checked"));
        }
        Ok(())
    }

    /// The signing key a row checked by `checker` names (a personal key's Ed25519 half, or the lease key): a trust
    /// anchor for a member-list chain ([`crate::keyversion::Chain::verify`]).
    pub fn checked_verify_key(&self, checker: &VerifyingKey) -> Result<VerifyingKey, Error> {
        checker.verify(Statement::KeyCheck, &refs(&self.signed_parts()), &self.signature)?;
        VerifyingKey::from_bytes(&self.checked_verify)
    }
}

/// Whether any of `checks`, signed by `checker`, covers `public`: nothing is wrapped to a personal key, or trusted
/// as one, before this holds (§3.5).
pub fn require_checked(checks: &[KeyCheck], checker: &VerifyingKey, public: &PersonalPublic) -> Result<(), Error> {
    if checks
        .iter()
        .any(|check| check.covers_personal(checker, public).is_ok())
    {
        Ok(())
    } else {
        Err(Error::Refused("this personal key has not been checked"))
    }
}

#[cfg(test)]
#[path = "fingerprint_tests.rs"]
mod tests;
