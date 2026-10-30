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

//! The DAV server ([`dav`]: WebDAV over the drives, CalDAV and CardDAV over an application's adapter services), the
//! `VCard` and `ICalendar` tools ([`pim`]), `org.moqui.impl.DavServices` ([`service`]) and the elFinder connector
//! ([`elfinder`], `org.moqui.impl.ElFinderServices`), on top of moqui-core (plans/Crate-Split.md).
//!
//! Core reaches this crate only through the [`moqui_core::extension`] entry below: the feature `dav`, the Rust services
//! of `DavServices` and `run#ElFinderCommand` (at `moqui_dav::` locations), and the two tools, whose classes stay
//! `moqui_core::pim::VCardToolFactory` and `ICalendarToolFactory` as the conf names them. moqui-host links it in every
//! build and serves `/dav/` and SFTP over it; the browser build links it for the tools and elFinder. A binary without
//! it still loads its services, which report that `dav` is not built in.

use std::collections::BTreeMap;

use moqui_core::extension::RustServiceFn;

pub mod dav;
pub mod elfinder;
pub mod pim;
pub mod service;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["dav"],
        register_services,
        register_native: pim::register,
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn register_services(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    service::register(map)?;
    elfinder::register(map)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    /// The Rust services register at the `moqui_dav::` locations the service files name, and at no other.
    #[test]
    fn the_services_register_at_the_locations_the_service_files_name() {
        let mut map = BTreeMap::new();
        super::register_services(&mut map).unwrap();
        let registered: Vec<&str> = map.keys().copied().collect();
        let dir = moqui_core::context::framework_dir().join("service/org/moqui/impl");
        let mut named = Vec::new();
        for file in ["DavServices.xml", "ElFinderServices.xml"] {
            let text = std::fs::read_to_string(dir.join(file)).unwrap();
            named.extend(
                text.split("location=\"")
                    .skip(1)
                    .filter_map(|rest| rest.split_once('"').map(|(location, _)| location.to_string()))
                    .filter(|location| location.starts_with("moqui_dav::")),
            );
        }
        named.sort_unstable();
        named.dedup();
        assert_eq!(registered, named);
    }
}
