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

//! The machine-access seal (plans/Vault-Instances.md §3.8): an item key an environment may use unattended, sealed by
//! the crate's AEAD under the server's machine key with the environment, vault and item ids as associated data, so a
//! row copied to another environment or item does not open. Until Sealed-Secrets S1 the machine key is derived by
//! HKDF from the field pass (`entity-facade@crypt-pass`) with the label `vault-machine`.

use crate::aead;
use crate::kdf;
use crate::keys::Key32;
use crate::names::{refs, Names, Purpose};
use crate::rng::Rng;
use crate::Error;

/// The machine key from today's field pass, until the keyring's `vault-machine` purpose key (Sealed-Secrets S1).
pub fn machine_key_from_crypt_pass(crypt_pass: &[u8]) -> Key32 {
    kdf::hkdf_sha256(crypt_pass, b"", b"vault-machine")
}

fn parts(environment_id: &str, vault_id: &str, item_id: &str) -> Vec<Vec<u8>> {
    Names::new(Purpose::Machine, vault_id, item_id, 0).parts(&[environment_id.as_bytes()])
}

/// Seals an item key for an environment's machine access.
pub fn seal(
    machine_key: &Key32,
    environment_id: &str,
    vault_id: &str,
    item_id: &str,
    item_key: &Key32,
    rng: &mut dyn Rng,
) -> Vec<u8> {
    aead::seal(
        machine_key,
        &refs(&parts(environment_id, vault_id, item_id)),
        item_key.as_bytes(),
        rng,
    )
}

/// Opens a machine-access item key; one moved to another environment, vault or item does not open.
pub fn open(
    machine_key: &Key32,
    environment_id: &str,
    vault_id: &str,
    item_id: &str,
    sealed: &[u8],
) -> Result<Key32, Error> {
    let opened = aead::open(machine_key, &refs(&parts(environment_id, vault_id, item_id)), sealed)?;
    Key32::from_slice(&opened)
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
