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

//! Geo data files read for the importer (plans/Maps.md §6.3, §6.4, §7): one delimited-text reader over a bare file or a
//! member of a zip, decompressed within limits ([`text`]); the parsers of the registered sources, each yielding typed
//! records ([`census`], [`geonames`], [`operator`]); postal-code normalization by country ([`postal`]); the derived Geo
//! and point IDs ([`ids`]); and the coordinate checks and precision rules here. No ExecutionContext: the importer that
//! writes the rows is `moqui_tools::geo`.
//!
//! A malformed row is a [`RowError`], counted and reported, and the rest of the file goes on; only a file that cannot
//! be read as a whole (no such member, a missing header column, a decompression limit passed) is an
//! [`Error`](crate::error::Error).

pub mod admin1;
pub mod census;
pub mod geonames;
pub mod ids;
pub mod operator;
pub mod overlay;
pub mod postal;
pub mod text;

use crate::error::Error;

pub use postal::{normalize_postal_code, Granularity, PostalCode};

/// A WGS84 position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub latitude: f64,
    pub longitude: f64,
}

/// How exact a point is (`GeoPointPrecision`, BasicEntities.xml). An import never makes a user-confirmed point, so
/// there is no such variant here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    AddressMatch,
    StreetRange,
    PostalArea,
    PlaceEstimate,
}

impl Precision {
    #[must_use]
    pub fn enum_id(self) -> &'static str {
        match self {
            Self::AddressMatch => "GPP_ADDRESS_MATCH",
            Self::StreetRange => "GPP_STREET_RANGE",
            Self::PostalArea => "GPP_POSTAL_AREA",
            Self::PlaceEstimate => "GPP_PLACE_ESTIMATE",
        }
    }

    /// An operator file's precision column: an enum ID or its word (`address`, `street`, `postal`, `place`); empty is a
    /// postal-area point.
    ///
    /// # Errors
    ///
    /// A user-confirmed precision (an import is never a user's confirmation) or a value that is none of these.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "postal" | "postal-area" | "gpp_postal_area" => Ok(Self::PostalArea),
            "place" | "gpp_place_estimate" => Ok(Self::PlaceEstimate),
            "address" | "gpp_address_match" => Ok(Self::AddressMatch),
            "street" | "street-range" | "gpp_street_range" => Ok(Self::StreetRange),
            "user" | "gpp_user_confirmed" => Err("an import never makes a user-confirmed point".to_string()),
            other => Err(format!("precision {other} is not address, street, postal or place")),
        }
    }
}

/// The precision of a GeoNames postal row from its accuracy (download.geonames.org/export/zip/readme.txt): 6, the
/// centroid of addresses or of the code's shape, is a postal-area point; 4 (the place's own point), 1 (estimated from
/// neighbouring codes) and none are place estimates.
#[must_use]
pub fn geonames_precision(accuracy: Option<i32>) -> Precision {
    match accuracy {
        Some(6) => Precision::PostalArea,
        _ => Precision::PlaceEstimate,
    }
}

/// The rank of a GeoNames accuracy for choosing a code's representative row (plans/Maps.md §6.3): 6, then 4, then 1,
/// then none (or any other value).
#[must_use]
pub fn accuracy_rank(accuracy: Option<i32>) -> u8 {
    match accuracy {
        Some(6) => 3,
        Some(4) => 2,
        Some(1) => 1,
        _ => 0,
    }
}

/// The index of the representative among a code's rows, given each row's accuracy in file order: the highest
/// [`accuracy_rank`], the first in the file among equals. None for no rows.
#[must_use]
pub fn representative_index(accuracies: &[Option<i32>]) -> Option<usize> {
    let mut best: Option<(usize, u8)> = None;
    for (index, accuracy) in accuracies.iter().enumerate() {
        let rank = accuracy_rank(*accuracy);
        if best.is_none_or(|(_, best_rank)| rank > best_rank) {
            best = Some((index, rank));
        }
    }
    best.map(|(index, _)| index)
}

/// A latitude and longitude read from text: decimal degrees, latitude within ±90 and longitude within ±180. Zero is a
/// position like any other.
///
/// # Errors
///
/// Text that is not a number, NaN or infinite, or out of range; the message names the field.
pub fn parse_point(latitude: &str, longitude: &str) -> Result<Point, (&'static str, String)> {
    let read = |field: &'static str, text: &str, limit: f64| -> Result<f64, (&'static str, String)> {
        let text = text.trim();
        let value: f64 = text
            .parse()
            .map_err(|_| (field, format!("{field} {text:?} is not a number")))?;
        if !value.is_finite() {
            return Err((field, format!("{field} {text:?} is not a finite number")));
        }
        if value.abs() > limit {
            return Err((field, format!("{field} {value} is outside ±{limit}")));
        }
        Ok(value)
    };
    Ok(Point {
        latitude: read("latitude", latitude, 90.0)?,
        longitude: read("longitude", longitude, 180.0)?,
    })
}

/// [`parse_point`], with no point when both are empty.
///
/// # Errors
///
/// One empty and not the other, or as [`parse_point`].
pub fn parse_optional_point(latitude: &str, longitude: &str) -> Result<Option<Point>, (&'static str, String)> {
    match (latitude.trim().is_empty(), longitude.trim().is_empty()) {
        (true, true) => Ok(None),
        (true, false) => Err(("latitude", "a longitude with no latitude".to_string())),
        (false, true) => Err(("longitude", "a latitude with no longitude".to_string())),
        (false, false) => parse_point(latitude, longitude).map(Some),
    }
}

/// The longest row text a [`RowError`] keeps: the error file names the row, it does not carry a runaway line.
pub const MAX_ROW_TEXT: usize = 1024;

/// What a rejected row was about, when that much of it could be read: so a code, county, place or city the new
/// edition still has, in a row passed over for another fault (a bad coordinate), is not retired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKey {
    /// A postal code, with the row's country as the row gives it.
    Postal { country: String, code: PostalCode },
    /// A Census county or place by its FIPS GEOID.
    Fips(String),
    /// A GeoNames city.
    City { country: String, geonameid: String },
}

/// A row that was passed over: its line in the file (from 1), the field at fault (empty for the row as a whole), why,
/// the row's text (at most [`MAX_ROW_TEXT`] bytes) and, where it could be read, what the row was about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowError {
    pub line: u64,
    pub field: String,
    pub message: String,
    pub text: String,
    /// Boxed: most rows have none, and a rejection is passed by value.
    pub key: Option<Box<RowKey>>,
}

impl RowError {
    #[must_use]
    pub fn new(line: u64, field: &str, message: impl Into<String>, text: &str) -> Self {
        Self {
            line,
            field: field.to_string(),
            message: message.into(),
            text: truncated(text, MAX_ROW_TEXT).to_string(),
            key: None,
        }
    }

    /// This error, about `key`.
    #[must_use]
    pub fn with_key(mut self, key: Option<RowKey>) -> Self {
        self.key = key.map(Box::new);
        self
    }
}

/// `text` cut to at most `max` bytes, on a character boundary.
#[must_use]
pub fn truncated(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default()
}

/// The files the parsers read, by the registry's parser and yields (`GeoDataParser`, `GeoDataYields`, MapEntities.xml).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Census Gazetteer ZCTAs: GEOID, INTPTLAT, INTPTLONG, and ALAND where it is there.
    CensusZcta,
    /// Census Gazetteer Counties: USPS, GEOID, NAME, INTPTLAT, INTPTLONG.
    CensusCounties,
    /// Census Gazetteer Places, as Counties.
    CensusPlaces,
    /// The Census 2020 ZCTA-to-county relationship file.
    CensusZctaCounty,
    /// GeoNames postal codes, a country's file.
    GeoNamesPostal,
    /// GeoNames countryInfo.txt.
    GeoNamesCountries,
    /// GeoNames admin1CodesASCII.txt.
    GeoNamesAdmin1,
    /// GeoNames admin2Codes.txt.
    GeoNamesAdmin2,
    /// GeoNames cities15000 (and the larger cities files).
    GeoNamesCities,
    /// An operator's CSV: country, code, place, admin1, admin2, latitude, longitude, precision.
    OperatorCsv,
}

impl Format {
    /// The format of a data set's parser and yields enum IDs; None for a pair no parser reads.
    #[must_use]
    pub fn of(parser_enum_id: &str, yields_enum_id: &str) -> Option<Self> {
        Some(match (parser_enum_id, yields_enum_id) {
            ("GDP_CENSUS", "GDY_POSTAL_POINTS") => Self::CensusZcta,
            ("GDP_CENSUS", "GDY_COUNTIES") => Self::CensusCounties,
            ("GDP_CENSUS", "GDY_PLACES") => Self::CensusPlaces,
            ("GDP_CENSUS", "GDY_POSTAL_COUNTY") => Self::CensusZctaCounty,
            ("GDP_GEONAMES", "GDY_POSTAL_POINTS") => Self::GeoNamesPostal,
            ("GDP_GEONAMES", "GDY_COUNTRIES") => Self::GeoNamesCountries,
            ("GDP_GEONAMES", "GDY_ADMIN1") => Self::GeoNamesAdmin1,
            ("GDP_GEONAMES", "GDY_ADMIN2") => Self::GeoNamesAdmin2,
            ("GDP_GEONAMES", "GDY_PLACES") => Self::GeoNamesCities,
            ("GDP_OPERATOR_CSV", _) => Self::OperatorCsv,
            _ => return None,
        })
    }
}

/// A record of any format, as [`parse`] yields it.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Postal(postal::PostalRecord),
    County(census::DivisionRecord),
    Place(census::DivisionRecord),
    ZctaCounty(census::ZctaCountyRecord),
    Country(geonames::CountryRecord),
    Admin1(geonames::Admin1Record),
    Admin2(geonames::Admin2Record),
    City(geonames::CityRecord),
}

/// Read every row of `reader` as `format`, calling `each` with each record or rejected row in file order. Returns the
/// rows read (data rows, not the header or comment lines).
///
/// # Errors
///
/// The file as a whole cannot be read: a header with a column missing, an I/O or decompression error.
pub fn parse(
    format: Format,
    reader: &mut dyn std::io::BufRead,
    each: &mut dyn FnMut(Result<Record, RowError>),
) -> Result<u64, Error> {
    match format {
        Format::CensusZcta => census::parse_zcta(reader, &mut |row| each(row.map(Record::Postal))),
        Format::CensusCounties => census::parse_divisions(reader, &mut |row| each(row.map(Record::County))),
        Format::CensusPlaces => census::parse_divisions(reader, &mut |row| each(row.map(Record::Place))),
        Format::CensusZctaCounty => census::parse_zcta_county(reader, &mut |row| each(row.map(Record::ZctaCounty))),
        Format::GeoNamesPostal => geonames::parse_postal(reader, &mut |row| each(row.map(Record::Postal))),
        Format::GeoNamesCountries => geonames::parse_countries(reader, &mut |row| each(row.map(Record::Country))),
        Format::GeoNamesAdmin1 => geonames::parse_admin1(reader, &mut |row| each(row.map(Record::Admin1))),
        Format::GeoNamesAdmin2 => geonames::parse_admin2(reader, &mut |row| each(row.map(Record::Admin2))),
        Format::GeoNamesCities => geonames::parse_cities(reader, &mut |row| each(row.map(Record::City))),
        Format::OperatorCsv => operator::parse(reader, &mut |row| each(row.map(Record::Postal))),
    }
}

/// Text that is empty after trimming as None.
pub(crate) fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests;
