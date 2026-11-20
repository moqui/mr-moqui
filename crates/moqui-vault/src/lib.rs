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

//! The vault crypto (plans/Vault-Instances.md §3.2): no I/O and no framework dependency, so the browser module builds
//! from this crate alone. Randomness comes in through [`rng::Rng`], never from a global source, so the wasm build
//! takes it from the page and the known-answer vectors from a fixed generator.
//!
//! - [`rng`], [`keys`], [`aead`], [`kdf`] and [`shamir`] are the primitives the sealed start of
//!   plans/Sealed-Secrets.md §3.3 shares with the vaults.
//! - [`names`] is what every associated data and HPKE `info` names; [`hpke`] the public-key wraps; [`sign`] the
//!   Ed25519 statements.
//! - [`personal`] (§3.5), [`vault`] (§3.2, §3.9), [`keyversion`] (§3.9), [`lease`] (§3.7), [`machine`] (§3.8),
//!   [`recovery`] (§3.10), [`delivery`] (§3.3) and [`fingerprint`] (§3.5) are the vaults' formats.
//! - [`managed`] (§4.4, §4.5) is a managed instance's sealed start: transit by its manager, its unsealed proof, and
//!   its recovery shares to holders (feature `json`).
//! - [`vectors`] writes the known-answer vectors of `testdata/vectors.json`, natively and in the wasm module.
//! - [`ops`] is the module's JSON calls (feature `json`, which `wasm` takes): the browser module, the `moqui vault`
//!   command and the test client's `vault` member drive the same [`ops::Session`], and the server reads the records'
//!   JSON shapes from [`ops::shapes`]. `wasm` is the module's `extern "C"` ABI (feature `wasm`); `scripts/vault-wasm.sh`
//!   builds it to `runtime/components/base/webroot/screen/webroot/js/vault/moqui_vault.wasm` on demand (`./moq build`,
//!   the test tasks), not kept in git, and rebuilds it only when this crate, its dependencies or rustc changed.

pub mod aead;
pub mod approvers;
pub mod delivery;
mod error;
pub mod fingerprint;
pub mod hpke;
pub mod kdf;
pub mod keys;
pub mod keyversion;
pub mod lease;
pub mod machine;
#[cfg(any(test, feature = "json"))]
pub mod managed;
pub mod names;
#[cfg(any(test, feature = "json"))]
pub mod ops;
pub mod personal;
pub mod recovery;
pub mod rng;
pub mod shamir;
pub mod sign;
pub mod text;
pub mod vault;
#[cfg(any(test, feature = "test-vectors"))]
pub mod vectors;
#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
mod wasm;

pub use error::Error;
pub use keys::Key32;
