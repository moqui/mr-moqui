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

//! Vaults on the server (plans/Vault-Instances.md §3): the services of `org.moqui.security.VaultServices`, leases,
//! deposits, machine access, recovery and events. Core reaches this crate only through the [`moqui_core::extension`]
//! entry below, with the feature `vault`; a binary without it still loads the vault services, which report that
//! `vault` is not built in.
//!
//! The server stores and serves what clients make with the vault crate (`crates/moqui-vault`): public keys, signed
//! records and ciphertext. It never decrypts a personal item or holds a vault key. What it can check, it checks: a
//! key version is signed by a manager of the version before, a wrap by a manager of its version, a key check and a
//! delivery by their maker's current personal key, and nothing is wrapped to a member's key that its signer has not
//! checked. A client checks all of these again (§3.9), since a server that has been changed would not.
//!
//! - [`personal`]: personal keys (§3.5) and key checks; the wrapped private key only after a strong, recent sign-in.
//! - [`vaults`]: vaults of both locks (§3.1, §3.11), made, listed, read and deleted.
//! - [`items`]: items with revisions and history, written only at the vault's current key version.
//! - [`members`]: the member-list chain, add, remove with rotation in one compare-and-swap, and the ack of a new
//!   key version (§3.9, decisions (a) and (b) of §9).
//! - [`deliveries`]: sealed values to one recipient (§3.3).
//! - [`deposit`]: an operational vault's signed deposit key, and the server's deposit of a new item with no lease
//!   (§3.2).
//! - [`lease`]: leases (§3.7): the lease key, the registry of the leases this cluster member holds, ask, approve,
//!   use, end and revoke, and the thread's lease a script reads a value from (`ec.vault.leased`).
//! - [`machine`]: machine access per environment (§3.8).
//! - [`reveal`]: one operational value to a member with the reveal role (§3.7).
//! - [`events`]: the `VaultEvent` rows of §3.13, written in the transaction of their change; a lease's use in its own,
//!   before the value is returned.
//! - [`recovery`]: the organization's recovery keys split k of n, recovery on a vault, and recoveries (§3.10).
//! - [`device_memory`]: the record of each desktop or mobile app that remembers its user's personal key (§3.5, V5).

use std::collections::BTreeMap;

use moqui_core::extension::RustServiceFn;

mod common;
pub mod deliveries;
pub mod deposit;
pub mod device_memory;
pub mod events;
pub mod items;
pub mod lease;
pub mod machine;
pub mod members;
pub mod personal;
pub mod recovery;
pub mod reveal;
pub mod vaults;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["vault"],
        register_services,
        after_cluster_start: lease::after_cluster_start,
        before_shutdown: lease::before_shutdown,
        ..moqui_core::extension::Extension::EMPTY
    }
}

moqui_core::vault_hooks::submit! {
    moqui_core::vault_hooks::VaultHooks {
        leased: lease::hook_leased,
        holds_leased: lease::hook_holds_leased,
        machine_run: machine::hook_machine_run,
    }
}

fn register_services(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    personal::register(map)?;
    vaults::register(map)?;
    items::register(map)?;
    members::register(map)?;
    deliveries::register(map)?;
    recovery::register(map)?;
    deposit::register(map)?;
    device_memory::register(map)?;
    lease::services::register(map)?;
    machine::register(map)?;
    reveal::register(map)?;
    events::register(map)
}

#[cfg(test)]
mod tests;
