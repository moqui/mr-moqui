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

//! Managed instances (plans/Vault-Instances.md §4): the services of `org.moqui.dev.InstanceServices` that are Rust,
//! instance kinds, provisioning, the reconcile job, deploys, rotation and the manager's transit. Core reaches this crate
//! only through the [`moqui_core::extension`] entry below, with the feature `instances`.

use std::collections::BTreeMap;

use moqui_core::extension::RustServiceFn;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["instances"],
        register_services,
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn register_services(_map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    Ok(())
}
