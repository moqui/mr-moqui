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

//! GeoNames first-level codes to ISO 3166-2 (plans/Maps.md §6.3): GeoNames' admin1 codes are FIPS 10-4 (or its own
//! where FIPS has none) for every country but the US, Switzerland, Belgium and Montenegro, so a second-level division or
//! a city, which names its first-level division by that code, finds the division Mantle's install data already has
//! (`CAN_AB`, `JPN_01`, `IND_AN`) through the crosswalk in `admin1_iso.tsv` (GeoNames CC BY 4.0 joined with Wikidata's
//! CC0 ISO 3166-2 codes; the file is GeoNames-derived data under CC BY 4.0, not the repository's CC0, and its header says
//! how it was made). A code with no ISO code here is matched by name where the source gives one (compared folded:
//! [`same_name`]), else the importer makes `{country}_A1_{code}`.

use std::collections::HashMap;
use std::sync::LazyLock;

/// The countries whose GeoNames first-level codes are ISO 3166-2 already (download.geonames.org/export/dump/readme.txt).
pub const ISO_SOURCE: [&str; 4] = ["US", "CH", "BE", "ME"];

/// The crosswalk: `{country}.{code}` to the ISO 3166-2 code after `{country}-`.
static CROSSWALK: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    include_str!("admin1_iso.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('\t'))
        .collect()
});

/// Whether a country's GeoNames first-level codes are ISO 3166-2.
#[must_use]
pub fn source_is_iso(country_alpha2: &str) -> bool {
    ISO_SOURCE
        .iter()
        .any(|iso| iso.eq_ignore_ascii_case(country_alpha2.trim()))
}

/// The ISO 3166-2 code (after `{country}-`) of GeoNames first-level code `code` of the country of ISO alpha-2
/// `country_alpha2`: the code itself where the country's codes are ISO, else the crosswalk's; None when it has none.
#[must_use]
pub fn iso_code(country_alpha2: &str, code: &str) -> Option<String> {
    let (country, code) = (country_alpha2.trim().to_ascii_uppercase(), code.trim());
    if code.is_empty() {
        return None;
    }
    if source_is_iso(&country) {
        return Some(code.to_ascii_uppercase());
    }
    CROSSWALK
        .get(format!("{country}.{code}").as_str())
        .map(|iso| (*iso).to_string())
}

/// What a Geo has that an ISO 3166-2 code can match: its ID and its `geoCodeAlpha2` and `geoCodeNumeric`.
#[derive(Clone, Copy, Debug)]
pub struct DivisionCodes<'a> {
    pub geo_id: &'a str,
    pub alpha2: &'a str,
    pub numeric: &'a str,
}

/// Whether `division`, a child of country Geo `country_geo_id` (ISO alpha-2 `country_alpha2`), is ISO 3166-2
/// `{country}-{iso}`, in any form Mantle's install data writes one: the code in `geoCodeAlpha2` (`CA` of `USA_CA`, or
/// `VN-SG` whole) or in `geoCodeNumeric` (`01` of `JPN_01`), or an ID of the country's Geo ID and the code (`JPN_01`,
/// `VNM_VN-SG`).
#[must_use]
pub fn matches_iso(division: DivisionCodes<'_>, country_geo_id: &str, country_alpha2: &str, iso: &str) -> bool {
    let iso = iso.trim();
    if iso.is_empty() {
        return false;
    }
    let whole = format!("{}-{iso}", country_alpha2.trim().to_ascii_uppercase());
    division.alpha2.eq_ignore_ascii_case(iso)
        || division.alpha2.eq_ignore_ascii_case(&whole)
        || division.numeric == iso
        || division.geo_id.eq_ignore_ascii_case(&format!("{country_geo_id}_{iso}"))
        || division
            .geo_id
            .eq_ignore_ascii_case(&format!("{country_geo_id}_{whole}"))
}

/// The field a division the importer makes keeps its ISO code in, as Mantle's do: `geoCodeNumeric` for a code of digits
/// (`JPN_01`), else `geoCodeAlpha2`.
#[must_use]
pub fn iso_field(iso: &str) -> &'static str {
    if !iso.is_empty() && iso.chars().all(|c| c.is_ascii_digit()) {
        "geoCodeNumeric"
    } else {
        "geoCodeAlpha2"
    }
}

/// The ISO 3166-2 codes (after `{country}-`) the crosswalk gives the GeoNames codes of the country of ISO alpha-2
/// `country_alpha2`, in no order.
#[must_use]
pub fn iso_codes(country_alpha2: &str) -> Vec<&'static str> {
    let prefix = format!("{}.", country_alpha2.trim().to_ascii_uppercase());
    CROSSWALK
        .iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .map(|(_, iso)| *iso)
        .collect()
}

/// Whether two division names are the same once folded: case, the diacritics of Latin letters (`Telangāna`, `Hyōgo`,
/// `Setúbal`) and surrounding space set aside, as a source's ASCII name and Mantle's native spelling differ.
#[must_use]
pub fn same_name(one: &str, other: &str) -> bool {
    fold_name(one) == fold_name(other)
}

/// `name` lower case with the diacritics of Latin letters taken off (a combining mark dropped, a precomposed letter as
/// its base: `ā` as `a`, `ß` as `ss`) and surrounding space trimmed.
#[must_use]
pub fn fold_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if ('\u{300}'..='\u{36f}').contains(&c) {
            continue;
        }
        match base_letters(c) {
            Some(base) => out.push_str(base),
            None => out.push(c),
        }
    }
    out
}

/// The base letters of a lower-case Latin letter with a diacritic (Latin-1 and Latin Extended-A, and the comma-below
/// letters of Romanian); None for any other character.
fn base_letters(c: char) -> Option<&'static str> {
    const GROUPS: [(&str, &str); 26] = [
        ("àáâãäåāăą", "a"),
        ("çćĉċč", "c"),
        ("ďđð", "d"),
        ("èéêëēĕėęě", "e"),
        ("ĝğġģ", "g"),
        ("ĥħ", "h"),
        ("ìíîïĩīĭįı", "i"),
        ("ĵ", "j"),
        ("ķ", "k"),
        ("ĺļľŀł", "l"),
        ("ñńņňŉ", "n"),
        ("òóôõöøōŏő", "o"),
        ("ŕŗř", "r"),
        ("śŝşšș", "s"),
        ("ţťŧț", "t"),
        ("ùúûüũūŭůűų", "u"),
        ("ŵ", "w"),
        ("ýÿŷ", "y"),
        ("źżž", "z"),
        ("æ", "ae"),
        ("œ", "oe"),
        ("ß", "ss"),
        ("þ", "th"),
        ("ĳ", "ij"),
        ("ŋ", "n"),
        ("ĸ", "k"),
    ];
    GROUPS
        .iter()
        .find(|(letters, _)| letters.contains(c))
        .map(|(_, base)| *base)
}
