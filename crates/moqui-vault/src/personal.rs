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

//! The personal key (plans/Vault-Instances.md §3.5): an X25519 key pair to receive wraps and an Ed25519 key pair to
//! sign what the member makes. The two private halves are wrapped together, by the key passphrase through Argon2id
//! and by the recovery code through HKDF; the server stores both wraps and never sees either secret.

use zeroize::Zeroizing;

use crate::aead;
use crate::hpke;
use crate::kdf::{self, Argon2Params, PasswordWrap};
use crate::keys::Key32;
use crate::names::{refs, Names, Purpose};
use crate::rng::Rng;
use crate::sign::{SigningKey, VerifyingKey};
use crate::text;
use crate::Error;

/// Random bytes in a recovery code: 256 bits, so it cannot be guessed and needs no Argon2id.
const RECOVERY_CODE_BYTES: usize = 32;
const RECOVERY_SALT_LEN: usize = 16;
const RECOVERY_INFO: &[u8] = b"moqui-vault personal recovery code";

/// A personal key's two public halves, stored in the clear (`VaultPersonalKey`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersonalPublic {
    /// X25519, to wrap to.
    pub encrypt: hpke::PublicKey,
    /// Ed25519, to check what the member signs.
    pub verify: VerifyingKey,
}

impl PersonalPublic {
    /// Both keys, encrypt then verify: 64 bytes, as signed statements and fingerprints carry them.
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(self.encrypt.as_bytes());
        out[32..].copy_from_slice(self.verify.as_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let Some((encrypt, verify)) = bytes.split_at_checked(32) else {
            return Err(Error::Format("a personal public key is 64 bytes"));
        };
        if verify.len() != 32 {
            return Err(Error::Format("a personal public key is 64 bytes"));
        }
        Ok(PersonalPublic {
            encrypt: hpke::PublicKey::from_bytes(encrypt)?,
            verify: VerifyingKey::from_bytes(verify)?,
        })
    }
}

/// A personal key with its private halves. Cleared on drop; `Debug` is redacted.
#[derive(Clone, Debug)]
pub struct PersonalKey {
    encrypt: hpke::SecretKey,
    sign: SigningKey,
}

impl PersonalKey {
    pub fn generate(rng: &mut dyn Rng) -> Self {
        PersonalKey {
            encrypt: hpke::SecretKey::random(rng),
            sign: SigningKey::random(rng),
        }
    }

    /// The key from its private halves: the X25519 private key and the Ed25519 seed.
    pub fn from_secrets(encrypt: &Key32, sign_seed: &Key32) -> Self {
        PersonalKey {
            encrypt: hpke::SecretKey::from_key(encrypt.clone()),
            sign: SigningKey::from_seed(sign_seed),
        }
    }

    pub fn public(&self) -> PersonalPublic {
        PersonalPublic {
            encrypt: self.encrypt.public().expect("a stored X25519 private key is 32 bytes"),
            verify: self.sign.verifying_key(),
        }
    }

    pub fn encrypt_key(&self) -> &hpke::SecretKey {
        &self.encrypt
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.sign
    }

    /// The private halves, X25519 then the Ed25519 seed: what the wraps hold.
    fn private_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(64));
        out.extend_from_slice(self.encrypt.as_key().as_bytes());
        out.extend_from_slice(self.sign.seed().as_bytes());
        out
    }

    fn from_private_bytes(bytes: &[u8], expected: &PersonalPublic) -> Result<Self, Error> {
        let Some((encrypt, seed)) = bytes.split_at_checked(32) else {
            return Err(Error::Format("personal private key"));
        };
        let key = PersonalKey {
            encrypt: hpke::SecretKey::from_key(Key32::from_slice(encrypt)?),
            sign: SigningKey::from_seed(&Key32::from_slice(seed)?),
        };
        // The stored public halves are the ones others wrap to and check; private halves that do not match them
        // would sign and open as someone else.
        if key.public() != *expected {
            return Err(Error::Refused("the private key does not match its public keys"));
        }
        Ok(key)
    }
}

fn names(personal_key_id: &str, purpose: Purpose) -> Names<'_> {
    Names::new(purpose, "", personal_key_id, 0)
}

/// Wraps the private halves under the key passphrase (Argon2id at the target).
pub fn wrap_with_passphrase(
    key: &PersonalKey,
    personal_key_id: &str,
    passphrase: &[u8],
    rng: &mut dyn Rng,
) -> Result<PasswordWrap, Error> {
    let public = key.public().to_bytes();
    let parts = names(personal_key_id, Purpose::PersonalPassphrase).parts(&[&public]);
    kdf::wrap_with_password(&key.private_bytes(), passphrase, &refs(&parts), rng)
}

/// An opened personal key, and the wrap to store in place of the old one when that one was under the Argon2id target.
#[derive(Debug)]
pub struct Opened {
    pub key: PersonalKey,
    pub rewrapped: Option<PasswordWrap>,
}

/// Opens the passphrase wrap. A wrong passphrase is [`Error::Open`]; the server stores no hash of it.
pub fn open_with_passphrase(
    wrap: &PasswordWrap,
    personal_key_id: &str,
    expected: &PersonalPublic,
    passphrase: &[u8],
    rng: &mut dyn Rng,
) -> Result<Opened, Error> {
    open_with_passphrase_to(wrap, personal_key_id, expected, passphrase, Argon2Params::TARGET, rng)
}

/// [`open_with_passphrase`] against another Argon2id target, which the tests raise above the floor.
pub(crate) fn open_with_passphrase_to(
    wrap: &PasswordWrap,
    personal_key_id: &str,
    expected: &PersonalPublic,
    passphrase: &[u8],
    target: Argon2Params,
    rng: &mut dyn Rng,
) -> Result<Opened, Error> {
    let public = expected.to_bytes();
    let parts = names(personal_key_id, Purpose::PersonalPassphrase).parts(&[&public]);
    let private = kdf::open_with_password(wrap, passphrase, &refs(&parts))?;
    let key = PersonalKey::from_private_bytes(&private, expected)?;
    let rewrapped = if kdf::needs_rewrap_to(wrap.params, target) {
        Some(kdf::wrap_with_password_at(
            &key.private_bytes(),
            passphrase,
            &refs(&parts),
            target,
            rng,
        )?)
    } else {
        None
    };
    Ok(Opened { key, rewrapped })
}

/// A new recovery code: 256 random bits as grouped base32, to print or save.
pub fn new_recovery_code(rng: &mut dyn Rng) -> Zeroizing<String> {
    let mut bytes = Zeroizing::new([0u8; RECOVERY_CODE_BYTES]);
    rng.fill(&mut bytes[..]);
    text::base32_grouped(&bytes[..], 4)
}

/// The key a recovery code or generated key gives with `salt`: HKDF over its decoded bytes.
pub(crate) fn code_key(code: &str, salt: &[u8], info: &[u8]) -> Result<Key32, Error> {
    let bytes = text::base32_decode(code)?;
    if bytes.len() != RECOVERY_CODE_BYTES {
        return Err(Error::Format("a code is 52 base32 characters"));
    }
    Ok(kdf::hkdf_sha256(&bytes, salt, info))
}

/// Seals `secret` under a code: `salt (16) ‖ AEAD`.
pub(crate) fn seal_with_code(
    secret: &[u8],
    code: &str,
    info: &[u8],
    ad: &[&[u8]],
    rng: &mut dyn Rng,
) -> Result<Vec<u8>, Error> {
    let mut salt = [0u8; RECOVERY_SALT_LEN];
    rng.fill(&mut salt);
    let key = code_key(code, &salt, info)?;
    let mut out = salt.to_vec();
    out.extend_from_slice(&aead::seal(&key, ad, secret, rng));
    Ok(out)
}

pub(crate) fn open_with_code(
    sealed: &[u8],
    code: &str,
    info: &[u8],
    ad: &[&[u8]],
) -> Result<Zeroizing<Vec<u8>>, Error> {
    let Some((salt, rest)) = sealed.split_at_checked(RECOVERY_SALT_LEN) else {
        return Err(Error::Format("code wrap shorter than its salt"));
    };
    aead::open(&code_key(code, salt, info)?, ad, rest)
}

/// Wraps the private halves under the recovery code.
pub fn wrap_with_recovery_code(
    key: &PersonalKey,
    personal_key_id: &str,
    code: &str,
    rng: &mut dyn Rng,
) -> Result<Vec<u8>, Error> {
    let public = key.public().to_bytes();
    let parts = names(personal_key_id, Purpose::PersonalRecoveryCode).parts(&[&public]);
    seal_with_code(&key.private_bytes(), code, RECOVERY_INFO, &refs(&parts), rng)
}

/// Opens the recovery-code wrap; the user then sets a new passphrase ([`wrap_with_passphrase`]).
pub fn open_with_recovery_code(
    sealed: &[u8],
    personal_key_id: &str,
    expected: &PersonalPublic,
    code: &str,
) -> Result<PersonalKey, Error> {
    let public = expected.to_bytes();
    let parts = names(personal_key_id, Purpose::PersonalRecoveryCode).parts(&[&public]);
    let private = open_with_code(sealed, code, RECOVERY_INFO, &refs(&parts))?;
    PersonalKey::from_private_bytes(&private, expected)
}

/// What a device memory is (§3.5, V5): its id, whose it is, and when it ends (epoch millis). All three are in the wrap's
/// associated data, so a memory file whose user or time was changed does not open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceMemory<'a> {
    pub memory_id: &'a str,
    pub user_id: &'a str,
    pub until: i64,
}

const DEVICE_INFO: &[u8] = b"moqui-vault device memory key";

/// The key a device memory is wrapped by: HKDF over the device's own key (kept in its OS key store) and the server's
/// secret for the memory (which the server hands out only while its record is live), salted by the memory's id. Neither
/// half alone opens the memory, so the server's record ending (revoked, past its time, taken off the list) ends it.
pub fn device_wrap_key(device_key: &Key32, server_secret: &Key32, memory_id: &str) -> Key32 {
    let mut ikm = Zeroizing::new([0u8; 64]);
    ikm[..32].copy_from_slice(device_key.as_bytes());
    ikm[32..].copy_from_slice(server_secret.as_bytes());
    kdf::hkdf_sha256(&ikm[..], memory_id.as_bytes(), DEVICE_INFO)
}

fn device_parts(personal_key_id: &str, public: &[u8; 64], memory: DeviceMemory<'_>) -> Vec<Vec<u8>> {
    let until = memory.until.to_be_bytes();
    names(personal_key_id, Purpose::PersonalDevice).parts(&[
        public,
        memory.memory_id.as_bytes(),
        memory.user_id.as_bytes(),
        &until,
    ])
}

/// Wraps the private halves under a device memory's key ([`device_wrap_key`], 256 bits, so no Argon2id). The
/// associated data names the personal key, its public halves, the memory, its user and its time.
pub fn wrap_with_device_key(
    key: &PersonalKey,
    personal_key_id: &str,
    memory: DeviceMemory<'_>,
    wrap_key: &Key32,
    rng: &mut dyn Rng,
) -> Vec<u8> {
    let parts = device_parts(personal_key_id, &key.public().to_bytes(), memory);
    aead::seal(wrap_key, &refs(&parts), &key.private_bytes(), rng)
}

/// Opens a device memory wrap ([`wrap_with_device_key`]); a wrong key, memory, user, time or personal key is
/// [`Error::Open`].
pub fn open_with_device_key(
    sealed: &[u8],
    personal_key_id: &str,
    memory: DeviceMemory<'_>,
    expected: &PersonalPublic,
    wrap_key: &Key32,
) -> Result<PersonalKey, Error> {
    let parts = device_parts(personal_key_id, &expected.to_bytes(), memory);
    let private = aead::open(wrap_key, &refs(&parts), sealed)?;
    PersonalKey::from_private_bytes(&private, expected)
}

#[cfg(test)]
#[path = "personal_tests.rs"]
mod tests;
