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

//! The framework crates on top of this one (moqui-git, plans/Crate-Split.md). Each submits one [`Extension`], which the
//! framework calls to register the crate's Rust services and native implementations. A binary that does not link a
//! crate has no entry for it: a service at one of its locations still loads, and a call reports that the feature is
//! not built into this host ([`missing_feature`]).

use std::collections::BTreeMap;

pub use inventory::submit;

use crate::context::Error;
use crate::native::NativeRegistry;
pub use crate::service::rust_services::RustServiceFn;

/// What the framework calls in a framework crate. Start from [`Extension::EMPTY`] and set what the crate has:
///
/// ```ignore
/// moqui_core::extension::submit! {
///     moqui_core::extension::Extension { features: &["git"], register_services, ..moqui_core::extension::Extension::EMPTY }
/// }
/// ```
pub struct Extension {
    /// The build features the crate is, as `moqui version` lists them and a spec's `requires` names them.
    pub features: &'static [&'static str],
    /// Add the crate's Rust services (`rust_service!` locations) to the registry.
    pub register_services: fn(&mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String>,
    /// Add the crate's tools, resource providers, runners and template renderers.
    pub register_native: fn(&mut NativeRegistry) -> Result<(), Error>,
}
inventory::collect!(Extension);

impl Extension {
    /// No features and nothing to register.
    pub const EMPTY: Self = Self {
        features: &[],
        register_services: |_| Ok(()),
        register_native: |_| Ok(()),
    };
}

/// The framework crates' service locations by path prefix, with the feature each is; the first prefix that matches
/// decides.
const LOCATIONS: &[(&str, &str)] = &[("moqui_git::", "git")];

/// The linked framework crates.
pub fn linked() -> impl Iterator<Item = &'static Extension> {
    inventory::iter::<Extension>.into_iter()
}

/// The features of the linked framework crates.
pub fn features() -> impl Iterator<Item = &'static str> {
    linked().flat_map(|extension| extension.features.iter().copied())
}

/// The feature `location` belongs to when this binary does not link the crate that is it.
pub(crate) fn missing_feature(location: &str) -> Option<&'static str> {
    let (_, feature) = LOCATIONS.iter().find(|(prefix, _)| location.starts_with(prefix))?;
    (!features().any(|linked| linked == *feature)).then_some(*feature)
}
