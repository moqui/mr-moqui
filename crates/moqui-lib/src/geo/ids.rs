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

//! The IDs of imported Geo data, computed from the country and the source's code with no lookup, so a re-import is a
//! create-or-update of the same rows (the comment above `GeoPostalCode` in BasicEntities.xml, plans/archive/Maps.md §6.3). An
//! ID longer than [`MAX_ID`] is not made: the importer sends its row to the error file. First- and second-level
//! divisions are found by lookup first; the forms here are for one the importer makes when nothing matches.

use super::postal::{Granularity, PostalCode};

/// The longest ID (an `id` field).
pub const MAX_ID: usize = 40;

/// `id` when it is at most [`MAX_ID`] characters of letters, digits, `_` and `-`.
fn checked(id: String) -> Result<String, String> {
    if id.len() > MAX_ID {
        return Err(format!("ID {id} is longer than {MAX_ID} characters"));
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(format!("ID {id:?} has characters other than letters, digits, _ and -"));
    }
    Ok(id)
}

/// A code part of an ID: letters, digits and `-`, not empty.
fn code_part<'a>(what: &str, code: &'a str) -> Result<&'a str, String> {
    let code = code.trim();
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("{what} {code:?} is not letters and digits"));
    }
    Ok(code)
}

/// A digits-only code of `len` digits (0 for any length).
fn digits<'a>(what: &str, code: &'a str, len: usize) -> Result<&'a str, String> {
    let code = code.trim();
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_digit()) || (len > 0 && code.len() != len) {
        return Err(if len > 0 {
            format!("{what} {code:?} is not {len} digits")
        } else {
            format!("{what} {code:?} is not digits")
        });
    }
    Ok(code)
}

/// A postal code: `{country}_Z_{code}`, a prefix code `{country}_ZP_{code}` (`USA_Z_02134`, `CAN_ZP_K1A`).
///
/// # Errors
///
/// The ID would be too long.
pub fn postal_code_geo_id(country_geo_id: &str, code: &PostalCode) -> Result<String, String> {
    let kind = match code.granularity {
        Granularity::Full => "Z",
        Granularity::Prefix => "ZP",
    };
    checked(format!("{country_geo_id}_{kind}_{}", code.normalized))
}

/// A US county by its five-digit FIPS state and county code: `USA_C_25025`.
///
/// # Errors
///
/// The code is not five digits.
pub fn county_geo_id(fips: &str) -> Result<String, String> {
    checked(format!("USA_C_{}", digits("county FIPS code", fips, 5)?))
}

/// A US place by its seven-digit FIPS state and place code: `USA_P_2507000`.
///
/// # Errors
///
/// The code is not seven digits.
pub fn place_geo_id(fips: &str) -> Result<String, String> {
    checked(format!("USA_P_{}", digits("place FIPS code", fips, 7)?))
}

/// A GeoNames city: `{country}_G_{geonameid}`.
///
/// # Errors
///
/// The geonameid is not digits, or the ID would be too long.
pub fn city_geo_id(country_geo_id: &str, geonameid: &str) -> Result<String, String> {
    checked(format!("{country_geo_id}_G_{}", digits("geonameid", geonameid, 0)?))
}

/// A first-level division made when none matched: `{country}_A1_{code}`.
///
/// # Errors
///
/// The code is not letters and digits, or the ID would be too long.
pub fn admin1_geo_id(country_geo_id: &str, code: &str) -> Result<String, String> {
    checked(format!("{country_geo_id}_A1_{}", code_part("first-level code", code)?))
}

/// A second-level division made when none matched: `{country}_A2_{admin1}_{admin2}`.
///
/// # Errors
///
/// A code is not letters and digits, or the ID would be too long.
pub fn admin2_geo_id(country_geo_id: &str, admin1: &str, admin2: &str) -> Result<String, String> {
    checked(format!(
        "{country_geo_id}_A2_{}_{}",
        code_part("first-level code", admin1)?,
        code_part("second-level code", admin2)?
    ))
}

/// A Geo's point from a source: the Geo's ID, `_` and the source key (`USA_Z_02134_CENSUS_ZCTA`).
///
/// # Errors
///
/// The ID would be too long.
pub fn point_id(geo_id: &str, source_key: &str) -> Result<String, String> {
    checked(format!("{geo_id}_{source_key}"))
}

/// A place name's own point: [`point_id`], `_` and the place's sequence number.
///
/// # Errors
///
/// The ID would be too long.
pub fn place_point_id(geo_id: &str, source_key: &str, sequence_num: u32) -> Result<String, String> {
    checked(format!("{geo_id}_{source_key}_{sequence_num}"))
}

/// Whether `geo_id` has a form the importer computes (a postal code, county, place, city, or a division it made), so
/// an import may update its name: never another record's Geo, such as Mantle's `USA_CA`.
#[must_use]
pub fn is_computed(geo_id: &str) -> bool {
    geo_id.split('_').nth(1).is_some_and(|kind| {
        matches!(kind, "Z" | "ZP" | "C" | "P" | "G" | "A1" | "A2") && geo_id.split('_').count() >= 3
    })
}
