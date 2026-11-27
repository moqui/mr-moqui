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

//! Optional Overture data for Maps (plans/archive/Maps-Overture.md, stages O1 and O2 of plans/archive/Maps.md §4.3): a region's Places,
//! Buildings and Addresses from a pinned Overture release, and the local address matching over its Addresses.
//!
//! - [`parquet`]: our own reader of the GeoParquet subset Overture writes, a part of a file at a time, with ZSTD on
//!   `ruzstd` (no C library).
//! - [`stac`]: the release catalog: releases, schema versions, a theme's items and their boxes.
//! - [`http`]: catalog documents and files by range through `moqui_core::outbound`.
//! - [`wkb`]: the points and polygons of the `geometry` column.
//! - [`overture`]: the schema 2.x columns of each theme and a region read from one file.
//! - [`address`]: O2's address matching: a text read into its parts, the keys points and texts are compared by, and the
//!   ranking of the points found (plans/archive/Maps-Overture.md §O2).
//! - [`address_service`]: the address index (`OvertureAddress` rows), a refresh's changes, and `search#OvertureAddresses`,
//!   the local geocoder's search.
//! - [`service`]: `org.moqui.maps.OvertureServices`' Rust services; the rest of the import (preflight, validation,
//!   activation, rollback) is OvertureData.js.
//!
//! This crate is on moqui-core, which reaches it through the [`moqui_core::extension`] entry below. moqui-host links it
//! under its `overture` feature; a build without it reports that `overture` is not built in.

pub mod address;
pub mod address_service;
pub mod http;
pub mod overture;
pub mod parquet;
pub mod service;
pub mod stac;
#[cfg(test)]
pub mod testdata;
pub mod wkb;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["overture"],
        register_services: service::register,
        ..moqui_core::extension::Extension::EMPTY
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The registry has a function at each `moqui_maps::` location the framework's service files name, and no other.
    #[test]
    fn the_services_register_at_the_locations_the_service_files_name() {
        let mut map = std::collections::BTreeMap::new();
        super::service::register(&mut map).unwrap();
        let registered: Vec<&str> = map.keys().copied().collect();
        let mut files = Vec::new();
        service_files(&moqui_core::context::framework_dir().join("service"), &mut files);
        let mut named: Vec<String> = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            named.extend(
                text.split("location=\"")
                    .skip(1)
                    .filter_map(|rest| rest.split_once('"').map(|(location, _)| location.to_string()))
                    .filter(|location| location.starts_with("moqui_maps::")),
            );
        }
        named.sort_unstable();
        named.dedup();
        assert_eq!(registered, named);
    }

    fn service_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                service_files(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "xml") {
                out.push(path);
            }
        }
    }
}
