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

//! Zeroized key types (plans/Sealed-Secrets.md §3.5): cleared on drop, compared in constant time, and printed by
//! `Debug` as a redaction.

use std::fmt;

use subtle::ConstantTimeEq as _;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::rng::Rng;
use crate::Error;

/// A 256-bit symmetric key: a vault key, an item key, a root key, a key derived by HKDF or Argon2id.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Key32([u8; 32]);

impl Key32 {
    /// The key with these bytes. The caller zeroizes its own copy.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Key32(bytes)
    }

    /// The key from a slice of exactly 32 bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        let mut key = Key32::zeroed();
        if bytes.len() != key.0.len() {
            return Err(Error::Format("a key is 32 bytes"));
        }
        key.0.copy_from_slice(bytes);
        Ok(key)
    }

    /// A new random key.
    pub fn random(rng: &mut dyn Rng) -> Self {
        let mut key = Key32::zeroed();
        rng.fill(&mut key.0);
        key
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// An all-zero key to derive into in place, so the derived bytes are never in a buffer that is not cleared.
    pub(crate) fn zeroed() -> Self {
        Key32([0; 32])
    }

    pub(crate) fn bytes_mut(&mut self) -> &mut [u8; 32] {
        &mut self.0
    }
}

impl PartialEq for Key32 {
    fn eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl Eq for Key32 {}

impl fmt::Debug for Key32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key32(redacted)")
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
