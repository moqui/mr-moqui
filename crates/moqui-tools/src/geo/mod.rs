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

//! The Geo data importer (plans/Maps.md §6.4): `org.moqui.maps.GeoDataServices` ([`service`]) brings a registered data
//! set (`moqui.maps.GeoDataSet`) in when an administrator asks: preflight, a capped and conditional download to a file
//! under `runtime/tmp`, or an operator's file ([`fetch`]); the whole file read and checked writing nothing, then loaded
//! in batches of a few thousand codes to a transaction, and retired only after a whole load ([`load`]). Each run is a
//! `moqui.maps.GeoDataImport` row with its counts, and its rejected rows go to an error file, as an ETL run's do. The
//! parsers, postal-code rules and derived IDs are `moqui_lib::geo`.

pub mod fetch;
pub mod load;
pub mod lookup;
pub mod remove;
pub mod service;

pub const DATA_SET: &str = "moqui.maps.GeoDataSet";
pub const IMPORT: &str = "moqui.maps.GeoDataImport";
pub const IMPORT_SERVICE: &str = "org.moqui.maps.GeoDataServices.import#GeoData";

/// Where a run's error file goes: `<root>/<userId>/<geoDataImportId>/errors.csv`.
pub const ERROR_ROOT: &str = "dbresource://moqui/maps/geo-import";
/// The postal lookup's own cache (the lookup service, Maps P2), cleared when an import finishes.
pub const POSTAL_LOOKUP_CACHE: &str = "maps.postal.lookup";
/// The codes (or other records) loaded in one transaction.
pub const BATCH: usize = 2000;
/// The rejected rows a run's out-parameters keep.
pub const MAX_ERRORS_OUT: usize = 100;
/// The rejected rows a run keeps for its error file; past them they are counted, and the file says how many more.
pub const MAX_KEPT_REJECTIONS: usize = 10_000;
/// A new file with fewer rows than this share of the edition in use is refused unless forced.
pub const MIN_ROW_SHARE: f64 = 0.8;
/// A run in progress whose row has not changed for this long is taken as abandoned (a server stopped under it).
pub const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(3600);

pub const PENDING: &str = "GdiPending";
pub const DOWNLOADING: &str = "GdiDownloading";
pub const VALIDATING: &str = "GdiValidating";
pub const LOADING: &str = "GdiLoading";
pub const COMPLETED: &str = "GdiCompleted";
pub const UNCHANGED: &str = "GdiUnchanged";
pub const FAILED: &str = "GdiFailed";
pub const CANCELLED: &str = "GdiCancelled";

/// Empty the postal lookup's cache (`maps.postal.lookup`): after a run that committed a change (a whole load, or the
/// batches of one that failed or was cancelled after them) and after a Remove. A failure is logged.
pub fn clear_postal_cache() {
    let cleared =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.factory().cache().clear(POSTAL_LOOKUP_CACHE));
    if let Some(Err(err)) = cleared {
        tracing::warn!(error = %err, "Could not clear the postal lookup cache");
    }
}

/// Whether a run in `status` is still going (or waiting to).
#[must_use]
pub fn in_progress(status: &str) -> bool {
    matches!(status, PENDING | DOWNLOADING | VALIDATING | LOADING)
}

#[cfg(test)]
mod tests;
