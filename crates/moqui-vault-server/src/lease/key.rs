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

//! The server's lease key (plans/Vault-Instances.md §3.7): an Ed25519 key that signs each lease, whose fingerprint
//! every approver checks once. Its seed is the generated secret `vault@lease-key`, which Sealed-Secrets S4 keeps in the
//! seal's store as a `SecretValue` (made at its first read), read through the factory's secret handle; S1 makes it the
//! keyring's `vault-lease` purpose key. [`seed`] is the one place that reads it, so that move changes only it. A new
//! seed is a new key: the fingerprint changed once at the S4 upgrade (from a seed in `runtime/.env` to one in the
//! store), and an ephemeral boot (a store that does not outlive the process) has a new one each time.

use moqui_lib::error::Error as LibError;
use moqui_vault::sign::{SigningKey, VerifyingKey};
use zeroize::Zeroizing;

use crate::common::fail;

/// The secret the seed is kept under.
const SETTING: &str = "vault@lease-key";

/// The lease key's seed: the generated secret, from the seal's store.
fn seed() -> Result<Zeroizing<String>, LibError> {
    crate::common::server_secret(SETTING)?.ok_or_else(|| {
        fail(format!(
            "The lease key is not set: {SETTING} is generated into the seal's store at its first read"
        ))
    })
}

/// The lease key, from its seed by HKDF with the label `vault-lease`.
pub(crate) fn signing_key() -> Result<SigningKey, LibError> {
    let seed = seed()?;
    let derived = moqui_vault::kdf::hkdf_sha256(seed.as_bytes(), b"", b"vault-lease");
    Ok(SigningKey::from_seed(&derived))
}

/// The lease key's public half, which approvers check by its fingerprint.
pub(crate) fn verifying_key() -> Result<VerifyingKey, LibError> {
    Ok(signing_key()?.verifying_key())
}
