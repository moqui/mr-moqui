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

//! Key derivation (plans/Vault-Instances.md §3.2): Argon2id for passwords and passphrases, with a floor that is
//! refused, a target that a wrap under it is wrapped again to, and a ceiling that is refused; HKDF-SHA256 for keys
//! from keys.

use argon2::{Algorithm, Argon2, Params, Version};
use hkdf::Hkdf;
use sha2::Sha256;

use zeroize::Zeroizing;

use crate::aead;
use crate::keys::Key32;
use crate::rng::Rng;
use crate::Error;

/// The shortest salt [`argon2id`] takes. The crate's formats write 16 random bytes.
pub const MIN_SALT_LEN: usize = 16;

/// Argon2id's cost: memory in KiB, passes and lanes. Stored beside each wrap, so a stronger target opens older wraps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Argon2Params {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl Argon2Params {
    /// The least any client writes or opens: 64 MiB, 3 passes, 1 lane, so a phone opens a vault in about a second.
    pub const FLOOR: Self = Self {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };
    /// What a client writes, and wraps again to when it opens a wrap under it. Starts at the floor; raised over time.
    pub const TARGET: Self = Self::FLOOR;
    /// The most a client opens: the server stores the parameters, and a huge value would stall a phone.
    pub const CEILING: Self = Self {
        m_kib: 1024 * 1024,
        t: 10,
        p: 4,
    };

    /// Refuses parameters under the floor or over the ceiling. Fewer lanes than the floor's one is not possible, and
    /// lanes do not weaken a derivation, so only memory and passes are held to the floor.
    pub fn check(&self) -> Result<(), Error> {
        if self.m_kib < Self::FLOOR.m_kib || self.t < Self::FLOOR.t || self.p < Self::FLOOR.p {
            return Err(Error::Params("under the floor (m=64 MiB, t=3, p=1)"));
        }
        if self.m_kib > Self::CEILING.m_kib || self.t > Self::CEILING.t || self.p > Self::CEILING.p {
            return Err(Error::Params("over the ceiling (m=1 GiB, t=10, p=4)"));
        }
        Ok(())
    }
}

/// Whether a wrap made with `params` should be made again with [`Argon2Params::TARGET`] on this open.
pub fn needs_rewrap(params: Argon2Params) -> bool {
    needs_rewrap_to(params, Argon2Params::TARGET)
}

/// [`needs_rewrap`] against another target: the tests raise it above the floor to see a rewrap.
pub(crate) fn needs_rewrap_to(params: Argon2Params, target: Argon2Params) -> bool {
    params.m_kib < target.m_kib || params.t < target.t
}

/// The key Argon2id (v1.3) derives from `secret` and `salt`, refusing parameters outside the floor and the ceiling.
pub fn argon2id(secret: &[u8], salt: &[u8], params: Argon2Params) -> Result<Key32, Error> {
    params.check()?;
    if salt.len() < MIN_SALT_LEN {
        return Err(Error::Params("salt under 16 bytes"));
    }
    let argon_params = Params::new(params.m_kib, params.t, params.p, Some(32))
        .map_err(|_| Error::Params("not valid Argon2 parameters"))?;
    let mut key = Key32::zeroed();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params)
        .hash_password_into(secret, salt, key.bytes_mut())
        .map_err(|_| Error::Params("Argon2 refused the input"))?;
    Ok(key)
}

/// A value wrapped under a key Argon2id derives from a password or passphrase. The parameters and salt are stored
/// beside it, so a raised target still opens older wraps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordWrap {
    pub params: Argon2Params,
    pub salt: [u8; MIN_SALT_LEN],
    pub sealed: Vec<u8>,
}

/// Wraps `secret` under `password` with today's [`Argon2Params::TARGET`] and a new salt.
pub fn wrap_with_password(
    secret: &[u8],
    password: &[u8],
    ad: &[&[u8]],
    rng: &mut dyn Rng,
) -> Result<PasswordWrap, Error> {
    wrap_with_password_at(secret, password, ad, Argon2Params::TARGET, rng)
}

/// [`wrap_with_password`] with other parameters, which must be within the floor and the ceiling.
pub(crate) fn wrap_with_password_at(
    secret: &[u8],
    password: &[u8],
    ad: &[&[u8]],
    params: Argon2Params,
    rng: &mut dyn Rng,
) -> Result<PasswordWrap, Error> {
    let mut salt = [0u8; MIN_SALT_LEN];
    rng.fill(&mut salt);
    let key = argon2id(password, &salt, params)?;
    Ok(PasswordWrap {
        params,
        salt,
        sealed: aead::seal(&key, ad, secret, rng),
    })
}

/// Opens a [`PasswordWrap`]; a wrong password is [`Error::Open`], parameters outside the floor and ceiling
/// [`Error::Params`]. The caller wraps again when [`needs_rewrap`] says so.
pub fn open_with_password(wrap: &PasswordWrap, password: &[u8], ad: &[&[u8]]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let key = argon2id(password, &wrap.salt, wrap.params)?;
    aead::open(&key, ad, &wrap.sealed)
}

/// HKDF-SHA256 (RFC 5869) to one 32-byte key.
pub fn hkdf_sha256(ikm: &[u8], salt: &[u8], info: &[u8]) -> Key32 {
    let mut key = Key32::zeroed();
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, key.bytes_mut())
        .expect("32 bytes is within HKDF-SHA256's output limit");
    key
}

#[cfg(test)]
#[path = "kdf_tests.rs"]
mod tests;
