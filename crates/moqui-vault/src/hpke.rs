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

//! Public-key wraps (plans/archive/Vault-Instances.md §3.2): HPKE (RFC 9180) base mode with DHKEM(X25519, HKDF-SHA256),
//! HKDF-SHA256 and AES-256-GCM, single shot. The `info` carries the [`Names`] of what is wrapped. Base mode names no
//! sender, so every wrap the vaults store is also signed ([`crate::sign`]).
//!
//! The wrapped form is `enc (32) ‖ ciphertext ‖ tag (16)`.

use std::convert::Infallible;
use std::fmt;

use hpke::aead::AesGcm256;
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{Deserializable as _, Kem as _, OpModeR, OpModeS, Serializable as _};
use zeroize::{Zeroize as _, Zeroizing};

use crate::keys::Key32;
use crate::names::Names;
use crate::rng::Rng;
use crate::text::hex;
use crate::Error;

type Kem = X25519HkdfSha256;
pub const PUBLIC_LEN: usize = 32;
pub const ENC_LEN: usize = 32;

/// An X25519 public key, to wrap to.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; PUBLIC_LEN]);

impl PublicKey {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let array: [u8; PUBLIC_LEN] = bytes
            .try_into()
            .map_err(|_| Error::Format("an X25519 public key is 32 bytes"))?;
        Ok(PublicKey(array))
    }

    pub fn as_bytes(&self) -> &[u8; PUBLIC_LEN] {
        &self.0
    }

    fn kem(&self) -> Result<<Kem as hpke::Kem>::PublicKey, Error> {
        <Kem as hpke::Kem>::PublicKey::from_bytes(&self.0).map_err(|_| Error::Format("X25519 public key"))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", hex(&self.0))
    }
}

/// An X25519 private key, to open wraps with. Cleared on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretKey(Key32);

impl SecretKey {
    /// A new key pair's private half, made as RFC 9180's GenerateKeyPair does: DeriveKeyPair over 32 random bytes.
    pub fn random(rng: &mut dyn Rng) -> Self {
        let ikm = Key32::random(rng);
        let (secret, _) = Kem::derive_keypair(ikm.as_bytes());
        Self::from_kem(&secret)
    }

    /// RFC 9180's DeriveKeyPair: the private key from input keying material (the test vectors' `ikmR`).
    pub fn derive(ikm: &[u8]) -> Self {
        let (secret, _) = Kem::derive_keypair(ikm);
        Self::from_kem(&secret)
    }

    pub fn from_key(key: Key32) -> Self {
        SecretKey(key)
    }

    pub fn as_key(&self) -> &Key32 {
        &self.0
    }

    pub fn public(&self) -> Result<PublicKey, Error> {
        let public = Kem::sk_to_pk(&self.kem()?);
        PublicKey::from_bytes(&public.to_bytes())
    }

    fn from_kem(secret: &<Kem as hpke::Kem>::PrivateKey) -> Self {
        let mut bytes = secret.to_bytes();
        let key = Key32::from_slice(&bytes).expect("an X25519 private key is 32 bytes");
        bytes.zeroize();
        SecretKey(key)
    }

    fn kem(&self) -> Result<<Kem as hpke::Kem>::PrivateKey, Error> {
        <Kem as hpke::Kem>::PrivateKey::from_bytes(self.0.as_bytes()).map_err(|_| Error::Format("X25519 private key"))
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey(redacted)")
    }
}

/// The crate's [`Rng`] as the `rand_core` generator the hpke crate takes.
struct HpkeRng<'a>(&'a mut dyn Rng);

impl hpke::rand_core::TryRng for HpkeRng<'_> {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut bytes = [0u8; 4];
        self.0.fill(&mut bytes);
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut bytes = [0u8; 8];
        self.0.fill(&mut bytes);
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        self.0.fill(dst);
        Ok(())
    }
}

impl hpke::rand_core::TryCryptoRng for HpkeRng<'_> {}

/// HPKE base-mode seal to `to` with this `info` and `aad`: `enc ‖ ciphertext`.
pub fn seal_raw(
    to: &PublicKey,
    info: &[u8],
    aad: &[u8],
    plaintext: &[u8],
    rng: &mut dyn Rng,
) -> Result<Vec<u8>, Error> {
    let (enc, ciphertext) = hpke::single_shot_seal_with_rng::<AesGcm256, HkdfSha256, Kem>(
        &OpModeS::Base,
        &to.kem()?,
        info,
        plaintext,
        aad,
        &mut HpkeRng(rng),
    )
    // A low-order public key gives an all-zero shared secret, which X25519 DHKEM refuses.
    .map_err(|_| Error::Refused("cannot wrap to this public key"))?;
    let mut out = Vec::with_capacity(ENC_LEN + ciphertext.len());
    out.extend_from_slice(&enc.to_bytes());
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Opens what [`seal_raw`] made.
pub fn open_raw(secret: &SecretKey, info: &[u8], aad: &[u8], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let Some((enc, ciphertext)) = sealed.split_at_checked(ENC_LEN) else {
        return Err(Error::Format("wrap shorter than its encapsulated key"));
    };
    let enc = <Kem as hpke::Kem>::EncappedKey::from_bytes(enc).map_err(|_| Error::Format("encapsulated key"))?;
    hpke::single_shot_open::<AesGcm256, HkdfSha256, Kem>(&OpModeR::Base, &secret.kem()?, &enc, info, ciphertext, aad)
        .map(Zeroizing::new)
        .map_err(|_| Error::Open)
}

/// Wraps `plaintext` to `to`, with `names` and `extra` as the HPKE `info`.
pub fn wrap(
    to: &PublicKey,
    names: &Names,
    extra: &[&[u8]],
    plaintext: &[u8],
    rng: &mut dyn Rng,
) -> Result<Vec<u8>, Error> {
    seal_raw(to, &names.encode(extra), &[], plaintext, rng)
}

/// Opens what [`wrap`] made, with the same names.
pub fn unwrap(secret: &SecretKey, names: &Names, extra: &[&[u8]], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    open_raw(secret, &names.encode(extra), &[], sealed)
}

/// [`unwrap`] of a 32-byte key.
pub(crate) fn unwrap_key(secret: &SecretKey, names: &Names, extra: &[&[u8]], sealed: &[u8]) -> Result<Key32, Error> {
    let opened = unwrap(secret, names, extra, sealed)?;
    Key32::from_slice(&opened)
}

#[cfg(test)]
#[path = "hpke_tests.rs"]
mod tests;
