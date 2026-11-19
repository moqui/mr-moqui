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

use std::collections::BTreeMap;

use moqui_core::extension::RustServiceFn;
// The crypto, used by the services V1 adds.
use moqui_vault as _;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["vault"],
        register_services,
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn register_services(_map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    Ok(())
}
