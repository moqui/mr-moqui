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

//! The GeoNames files (plans/archive/Maps.md §6.2, §6.4): the postal codes of a country (download.geonames.org/export/zip,
//! twelve columns) and the gazetteer's countryInfo (`#` comment lines, nineteen columns), admin1CodesASCII,
//! admin2Codes and cities15000 (nineteen columns) from download.geonames.org/export/dump. All are UTF-8, tab-delimited,
//! with no header and no quoting: a `"` in a name is part of it. A first-level code is the source's own (FIPS for most
//! countries, ISO for the US, Switzerland, Belgium and Montenegro) and is kept as given.

use std::io::BufRead;

use crate::error::Error;

use super::postal::PostalRecord;
use super::text::{Line, Lines};
use super::{
    geonames_precision, non_empty, normalize_postal_code, parse_point, Granularity, Point, Precision, RowError, RowKey,
};

/// A country of countryInfo.txt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountryRecord {
    pub line: u64,
    pub iso2: String,
    pub iso3: String,
    pub numeric: String,
    pub fips: Option<String>,
    pub name: String,
    pub postal_format: Option<String>,
    pub postal_regex: Option<String>,
    pub geonameid: Option<String>,
}

/// A first-level division of admin1CodesASCII.txt (`US.CA`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admin1Record {
    pub line: u64,
    pub country: String,
    pub code: String,
    pub name: String,
    pub ascii_name: Option<String>,
    pub geonameid: Option<String>,
}

/// A second-level division of admin2Codes.txt (`US.CA.037`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admin2Record {
    pub line: u64,
    pub country: String,
    pub admin1: String,
    pub code: String,
    pub name: String,
    pub ascii_name: Option<String>,
    pub geonameid: Option<String>,
}

/// A city of cities15000.txt (and the larger cities files).
#[derive(Clone, Debug, PartialEq)]
pub struct CityRecord {
    pub line: u64,
    pub geonameid: String,
    pub name: String,
    pub ascii_name: Option<String>,
    pub point: Point,
    pub feature_code: Option<String>,
    pub country: String,
    pub admin1: Option<String>,
    pub admin2: Option<String>,
    pub population: Option<u64>,
}

/// Each line's number, text and tab-separated fields; blank lines and, with `comments`, lines that start with `#` are
/// left out. `each` returns the line's rejection, if any. Returns the rows read.
fn tab_rows(
    reader: &mut dyn BufRead,
    comments: bool,
    mut each: impl FnMut(u64, &str, &[&str]) -> Option<RowError>,
    key_of: &dyn Fn(&[&str]) -> Option<RowKey>,
    rejected: &mut dyn FnMut(RowError),
) -> Result<u64, Error> {
    let mut lines = Lines::new(reader);
    let mut count = 0;
    while let Some(line) = lines.next_line()? {
        match line {
            Line::Text(_, text) if text.trim().is_empty() || (comments && text.starts_with('#')) => {}
            Line::Text(number, text) => {
                count += 1;
                let fields: Vec<&str> = text.split('\t').collect();
                if let Some(mut error) = each(number, &text, &fields) {
                    if error.key.is_none() {
                        error.key = key_of(&fields).map(Box::new);
                    }
                    rejected(error);
                }
            }
            Line::Rejected(mut error) => {
                count += 1;
                let key = {
                    let fields: Vec<&str> = error.text.split('\t').collect();
                    key_of(&fields)
                };
                error.key = key.map(Box::new);
                rejected(error);
            }
        }
    }
    Ok(count)
}

/// A postal row's key, when its country and code read.
fn postal_key(fields: &[&str]) -> Option<RowKey> {
    let country = country_code(at(fields, 0))?;
    let code = normalize_postal_code(&country, at(fields, 1)).ok()?;
    Some(RowKey::Postal { country, code })
}

/// A city row's key, when its geonameid and country read.
fn city_key(fields: &[&str]) -> Option<RowKey> {
    let geonameid = at(fields, 0);
    if geonameid.is_empty() || !geonameid.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(RowKey::City {
        country: country_code(at(fields, 8))?,
        geonameid: geonameid.to_string(),
    })
}

/// The field at `index`, trimmed; empty when the row is short.
fn at<'f>(fields: &[&'f str], index: usize) -> &'f str {
    fields.get(index).map_or("", |text| text.trim())
}

/// A two-letter country code, upper case.
fn country_code(text: &str) -> Option<String> {
    let text = text.trim();
    (text.len() == 2 && text.chars().all(|c| c.is_ascii_alphabetic())).then(|| text.to_ascii_uppercase())
}

fn too_short(line: u64, text: &str, fields: &[&str], want: usize) -> Option<RowError> {
    (fields.len() < want).then(|| {
        RowError::new(
            line,
            "",
            format!("the row has {} columns, not {want}", fields.len()),
            text,
        )
    })
}

/// A country's postal file: country, code, place name, admin names and codes 1 to 3, latitude, longitude and accuracy
/// (1, 4 or 6, or empty). The code is normalized by its country's rules, so a Canadian, British or Dutch first part is
/// a prefix code, and a prefix's point is a place estimate whatever its accuracy.
///
/// # Errors
///
/// The file does not read.
pub fn parse_postal(
    reader: &mut dyn BufRead,
    each: &mut dyn FnMut(Result<PostalRecord, RowError>),
) -> Result<u64, Error> {
    let each = std::cell::RefCell::new(each);
    tab_rows(
        reader,
        false,
        |line, text, fields| {
            if let Some(error) = too_short(line, text, fields, 11) {
                return Some(error);
            }
            let Some(country) = country_code(at(fields, 0)) else {
                return Some(RowError::new(
                    line,
                    "country",
                    "the country is not a two-letter code",
                    text,
                ));
            };
            let code = match normalize_postal_code(&country, at(fields, 1)) {
                Ok(code) => code,
                Err(message) => return Some(RowError::new(line, "postalCode", message, text)),
            };
            let point = match parse_point(at(fields, 9), at(fields, 10)) {
                Ok(point) => point,
                Err((name, message)) => return Some(RowError::new(line, name, message, text)),
            };
            let accuracy = match at(fields, 11) {
                "" => None,
                value => match value.parse::<i32>() {
                    Ok(accuracy) => Some(accuracy),
                    Err(_) => {
                        return Some(RowError::new(
                            line,
                            "accuracy",
                            format!("accuracy {value:?} is not a whole number"),
                            text,
                        ))
                    }
                },
            };
            let precision = if code.granularity == Granularity::Prefix {
                Precision::PlaceEstimate
            } else {
                geonames_precision(accuracy)
            };
            (each.borrow_mut())(Ok(PostalRecord {
                line,
                country,
                code,
                place_name: non_empty(at(fields, 2)),
                admin_name1: non_empty(at(fields, 3)),
                admin_code1: non_empty(at(fields, 4)),
                admin_name2: non_empty(at(fields, 5)),
                admin_code2: non_empty(at(fields, 6)),
                point: Some(point),
                accuracy,
                precision,
                land_area: None,
            }));
            None
        },
        &|fields| postal_key(fields),
        &mut |error| (each.borrow_mut())(Err(error)),
    )
}

/// countryInfo.txt: ISO, ISO3, ISO numeric, FIPS, name, ..., postal code format and regular expression, ...,
/// geonameid. Lines starting with `#` are comments.
///
/// # Errors
///
/// The file does not read.
pub fn parse_countries(
    reader: &mut dyn BufRead,
    each: &mut dyn FnMut(Result<CountryRecord, RowError>),
) -> Result<u64, Error> {
    let each = std::cell::RefCell::new(each);
    tab_rows(
        reader,
        true,
        |line, text, fields| {
            if let Some(error) = too_short(line, text, fields, 17) {
                return Some(error);
            }
            let Some(iso2) = country_code(at(fields, 0)) else {
                return Some(RowError::new(line, "ISO", "the ISO code is not two letters", text));
            };
            let iso3 = at(fields, 1).to_ascii_uppercase();
            if iso3.len() != 3 || !iso3.chars().all(|c| c.is_ascii_alphabetic()) {
                return Some(RowError::new(line, "ISO3", "the ISO3 code is not three letters", text));
            }
            let numeric = at(fields, 2);
            if numeric.is_empty() || !numeric.chars().all(|c| c.is_ascii_digit()) {
                return Some(RowError::new(
                    line,
                    "ISO-Numeric",
                    "the numeric code is not digits",
                    text,
                ));
            }
            let Some(name) = non_empty(at(fields, 4)) else {
                return Some(RowError::new(line, "Country", "the name is empty", text));
            };
            (each.borrow_mut())(Ok(CountryRecord {
                line,
                iso2,
                iso3,
                numeric: format!("{numeric:0>3}"),
                fips: non_empty(at(fields, 3)),
                name,
                postal_format: non_empty(at(fields, 13)),
                postal_regex: non_empty(at(fields, 14)),
                geonameid: non_empty(at(fields, 16)),
            }));
            None
        },
        &|_| None,
        &mut |error| (each.borrow_mut())(Err(error)),
    )
}

/// The parts of a dotted GeoNames code (`US.CA.037`), each not empty, `parts` of them.
fn dotted(text: &str, parts: usize) -> Option<Vec<String>> {
    let split: Vec<String> = text.trim().split('.').map(str::to_string).collect();
    (split.len() == parts && split.iter().all(|part| !part.is_empty())).then_some(split)
}

/// admin1CodesASCII.txt: code (`US.CA`), name, ASCII name, geonameid.
///
/// # Errors
///
/// The file does not read.
pub fn parse_admin1(
    reader: &mut dyn BufRead,
    each: &mut dyn FnMut(Result<Admin1Record, RowError>),
) -> Result<u64, Error> {
    let each = std::cell::RefCell::new(each);
    tab_rows(
        reader,
        false,
        |line, text, fields| {
            if let Some(error) = too_short(line, text, fields, 2) {
                return Some(error);
            }
            let Some(parts) = dotted(at(fields, 0), 2) else {
                return Some(RowError::new(line, "code", "the code is not country.admin1", text));
            };
            let Some(country) = country_code(&parts[0]) else {
                return Some(RowError::new(
                    line,
                    "code",
                    "the country is not a two-letter code",
                    text,
                ));
            };
            let Some(name) = non_empty(at(fields, 1)) else {
                return Some(RowError::new(line, "name", "the name is empty", text));
            };
            (each.borrow_mut())(Ok(Admin1Record {
                line,
                country,
                code: parts[1].clone(),
                name,
                ascii_name: non_empty(at(fields, 2)),
                geonameid: non_empty(at(fields, 3)),
            }));
            None
        },
        &|_| None,
        &mut |error| (each.borrow_mut())(Err(error)),
    )
}

/// admin2Codes.txt: code (`US.CA.037`), name, ASCII name, geonameid.
///
/// # Errors
///
/// The file does not read.
pub fn parse_admin2(
    reader: &mut dyn BufRead,
    each: &mut dyn FnMut(Result<Admin2Record, RowError>),
) -> Result<u64, Error> {
    let each = std::cell::RefCell::new(each);
    tab_rows(
        reader,
        false,
        |line, text, fields| {
            if let Some(error) = too_short(line, text, fields, 2) {
                return Some(error);
            }
            let Some(parts) = dotted(at(fields, 0), 3) else {
                return Some(RowError::new(
                    line,
                    "code",
                    "the code is not country.admin1.admin2",
                    text,
                ));
            };
            let Some(country) = country_code(&parts[0]) else {
                return Some(RowError::new(
                    line,
                    "code",
                    "the country is not a two-letter code",
                    text,
                ));
            };
            let Some(name) = non_empty(at(fields, 1)) else {
                return Some(RowError::new(line, "name", "the name is empty", text));
            };
            (each.borrow_mut())(Ok(Admin2Record {
                line,
                country,
                admin1: parts[1].clone(),
                code: parts[2].clone(),
                name,
                ascii_name: non_empty(at(fields, 2)),
                geonameid: non_empty(at(fields, 3)),
            }));
            None
        },
        &|_| None,
        &mut |error| (each.borrow_mut())(Err(error)),
    )
}

/// cities15000.txt: geonameid, name, ASCII name, alternate names, latitude, longitude, feature class and code,
/// country, cc2, admin1 to admin4 codes, population, elevation, dem, time zone and modification date.
///
/// # Errors
///
/// The file does not read.
pub fn parse_cities(
    reader: &mut dyn BufRead,
    each: &mut dyn FnMut(Result<CityRecord, RowError>),
) -> Result<u64, Error> {
    let each = std::cell::RefCell::new(each);
    tab_rows(
        reader,
        false,
        |line, text, fields| {
            if let Some(error) = too_short(line, text, fields, 15) {
                return Some(error);
            }
            let geonameid = at(fields, 0);
            if geonameid.is_empty() || !geonameid.chars().all(|c| c.is_ascii_digit()) {
                return Some(RowError::new(line, "geonameid", "the geonameid is not digits", text));
            }
            let Some(name) = non_empty(at(fields, 1)) else {
                return Some(RowError::new(line, "name", "the name is empty", text));
            };
            let point = match parse_point(at(fields, 4), at(fields, 5)) {
                Ok(point) => point,
                Err((name, message)) => return Some(RowError::new(line, name, message, text)),
            };
            let Some(country) = country_code(at(fields, 8)) else {
                return Some(RowError::new(
                    line,
                    "country code",
                    "the country is not a two-letter code",
                    text,
                ));
            };
            let population = match at(fields, 14) {
                "" => None,
                value => match value.parse::<u64>() {
                    Ok(population) => Some(population),
                    Err(_) => {
                        return Some(RowError::new(
                            line,
                            "population",
                            format!("population {value:?} is not a whole number"),
                            text,
                        ))
                    }
                },
            };
            (each.borrow_mut())(Ok(CityRecord {
                line,
                geonameid: geonameid.to_string(),
                name,
                ascii_name: non_empty(at(fields, 2)),
                point,
                feature_code: non_empty(at(fields, 7)),
                country,
                admin1: non_empty(at(fields, 10)),
                admin2: non_empty(at(fields, 11)),
                population,
            }));
            None
        },
        &|fields| city_key(fields),
        &mut |error| (each.borrow_mut())(Err(error)),
    )
}
