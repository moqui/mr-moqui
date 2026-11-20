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

pub mod aead;
mod error;
pub mod kdf;
pub mod keys;
pub mod rng;
pub mod shamir;

pub use error::Error;
pub use keys::Key32;
