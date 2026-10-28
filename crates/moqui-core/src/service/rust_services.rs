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

//! Function registry for `type="rust"` services. Each function registers under its Rust path, down to the function
//! name (`moqui_core::service::mail::send_template`). The service element's `location` is that path.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::entity::EntityFacade;

/// A Rust service body. The service name stays on the definition; the pointer is the `location`.
pub type RustServiceFn = fn(&BTreeMap<String, Value>, &EntityFacade) -> Result<BTreeMap<String, Value>, LibError>;

/// Register `$function` under `module_path!() + "::" + its name`. A second insert of that path is an error.
/// Exported for moqui-llm's services (crate::llm_hooks).
#[doc(hidden)]
#[macro_export]
macro_rules! rust_service {
    ($map:expr, $function:ident) => {{
        let location: &'static str = concat!(module_path!(), "::", stringify!($function));
        debug_assert_eq!(location, ::std::any::type_name_of_val(&$function));
        if $map.insert(location, $function).is_some() {
            Err(format!("duplicate rust service {location}"))
        } else {
            Ok(())
        }
    }};
}

static REGISTRY: OnceLock<BTreeMap<&'static str, RustServiceFn>> = OnceLock::new();

/// The Rust service locations of the features a build may leave out (docs/Packaging.md), by path prefix; the first
/// prefix that matches decides. A service at one of these locations still loads in a build without the feature, and a
/// call reports that the feature is not built into this host.
const FEATURE_LOCATIONS: &[(&str, &str, bool)] = &[
    ("moqui_core::service::git::", "git", cfg!(feature = "git")),
    ("moqui_core::service::git_hosting::", "git", cfg!(feature = "git")),
    ("moqui_core::service::images::", "image", cfg!(feature = "image")),
    ("moqui_core::service::spreadsheet::", "xlsx", cfg!(feature = "xlsx")),
    (
        "moqui_core::service::mail::send_template",
        "smtp",
        cfg!(feature = "smtp"),
    ),
    (
        "moqui_core::service::mail::send_message",
        "smtp",
        cfg!(feature = "smtp"),
    ),
    ("moqui_core::service::mail::test_server", "smtp", cfg!(feature = "smtp")),
    (
        "moqui_core::service::mail::push_remote_changes",
        "smtp",
        cfg!(feature = "smtp"),
    ),
    // the app's contacts hook, with or without mail built in
    ("moqui_core::service::mail::filter_contact_addresses", "", true),
    ("moqui_core::service::mail::", "imap", cfg!(feature = "imap")),
];

/// The feature `location` belongs to when this build leaves that feature out.
pub(crate) fn missing_feature(location: &str) -> Option<&'static str> {
    FEATURE_LOCATIONS
        .iter()
        .find(|(prefix, _, _)| location.starts_with(prefix))
        .and_then(|(_, feature, built)| (!built).then_some(*feature))
        .or_else(|| (location.starts_with("moqui_llm::") && !crate::llm_hooks::linked()).then_some("llm"))
}

/// The function registered at `location`, once every module has registered.
pub(crate) fn get(location: &str) -> Option<RustServiceFn> {
    registry().get(location).copied()
}

fn registry() -> &'static BTreeMap<&'static str, RustServiceFn> {
    REGISTRY.get_or_init(build)
}

fn build() -> BTreeMap<&'static str, RustServiceFn> {
    let mut map = BTreeMap::new();
    let mut errors = Vec::new();
    for result in [
        super::backup::register(&mut map),
        super::content::register(&mut map),
        super::mail::register(&mut map),
        super::system_sync::register(&mut map),
        super::elfinder::register(&mut map),
        super::sso::register(&mut map),
        super::tools::register(&mut map),
        #[cfg(feature = "image")]
        super::images::register(&mut map),
        #[cfg(feature = "xlsx")]
        super::spreadsheet::register(&mut map),
        super::shell::register(&mut map),
        super::user_keys::register(&mut map),
        super::dav::register(&mut map),
        super::collab::register(&mut map),
        super::devtools::register(&mut map),
        super::chat::register(&mut map),
        super::chat_link::register(&mut map),
        #[cfg(feature = "git")]
        super::git::register(&mut map),
        #[cfg(feature = "git")]
        super::git_hosting::register(&mut map),
        crate::llm_hooks::get().map_or(Ok(()), |hooks| (hooks.register_services)(&mut map)),
    ] {
        if let Err(error) = result {
            errors.push(error);
        }
    }
    assert!(errors.is_empty(), "rust service registry: {}", errors.join("; "));
    map
}

#[cfg(test)]
#[path = "rust_services_tests.rs"]
mod tests;
