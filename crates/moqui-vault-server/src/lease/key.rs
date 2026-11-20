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
//! every approver checks once. Until Sealed-Secrets S4 its seed is the generated secret `vault@lease-key`, kept in
//! `runtime/.env` as today's generated secrets are; S4 keeps it as a `SecretValue` (and S1 makes it the keyring's
//! `vault-lease` purpose key). [`seed`] is the one place that reads it, so that move changes only it.

use moqui_lib::error::Error as LibError;
use moqui_vault::sign::{SigningKey, VerifyingKey};
use zeroize::Zeroizing;

use crate::common::fail;

/// The setting the seed is kept under until Sealed-Secrets S4.
const SETTING: &str = "vault@lease-key";

/// The lease key's seed: the generated secret, as the boot generated it into `runtime/.env`.
fn seed() -> Result<Zeroizing<String>, LibError> {
    let value = Zeroizing::new(moqui_core::context::conf_env::setting(SETTING));
    if value.trim().is_empty() || value.ends_with("CHANGEME") {
        return Err(fail(format!(
            "The lease key is not set: {SETTING} is generated at the first start into runtime/.env"
        )));
    }
    Ok(value)
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
