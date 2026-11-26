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

//! The US Census files (plans/Maps.md §6.2, §6.4): the Gazetteer's ZCTA, Counties and Places files, and the 2020
//! ZCTA-to-county relationship file. Each is delimited text with a header line, read by column name: pipe-delimited
//! (the 2026 files; the delimiter is told from the header, so the older tab-delimited years read too), no quoting, a
//! byte order mark on the relationship file's header. A ZCTA's GEOID keeps its leading zeros.

use std::collections::BTreeMap;
use std::io::BufRead;

use crate::error::Error;

use super::postal::PostalRecord;
use super::text::{Line, Lines};
use super::{non_empty, normalize_postal_code, parse_point, Point, Precision, RowError, RowKey};

/// What reads a rejected row's key from its fields and the header's columns.
type KeyOf = dyn Fn(&[&str], &BTreeMap<String, usize>) -> Option<RowKey>;

/// A ZCTA's key, when its GEOID reads as a ZIP code.
fn zcta_key(geoid: &str) -> Option<RowKey> {
    normalize_postal_code("US", geoid).ok().map(|code| RowKey::Postal {
        country: "US".to_string(),
        code,
    })
}

/// A county's or place's key, when its GEOID is digits.
fn fips_key(geoid: &str) -> Option<RowKey> {
    (!geoid.is_empty() && geoid.chars().all(|c| c.is_ascii_digit())).then(|| RowKey::Fips(geoid.to_string()))
}

/// A county or place of the Gazetteer: its state's USPS code, its FIPS GEOID, its name and its internal point.
#[derive(Clone, Debug, PartialEq)]
pub struct DivisionRecord {
    pub line: u64,
    pub state: String,
    pub geoid: String,
    pub name: String,
    pub point: Point,
}

/// A row of the ZCTA-to-county relationship file: a ZCTA, a county it overlaps and the land area of the overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZctaCountyRecord {
    pub line: u64,
    pub zcta: String,
    pub county: String,
    pub land_part: u64,
}

/// A header-led file: its columns by name (upper case, trimmed) and its delimiter.
struct Headed<'a> {
    lines: Lines<'a>,
    columns: BTreeMap<String, usize>,
    delimiter: char,
}

impl<'a> Headed<'a> {
    /// Read the header; `required` columns must be in it.
    fn open(reader: &'a mut dyn BufRead, required: &[&str]) -> Result<Self, Error> {
        let mut lines = Lines::new(reader);
        let header = loop {
            match lines.next_line()? {
                Some(Line::Text(_, text)) if text.trim().is_empty() => {}
                Some(Line::Text(_, text)) => break text,
                Some(Line::Rejected(error)) => {
                    return Err(Error::new(format!(
                        "the header line could not be read: {}",
                        error.message
                    )))
                }
                None => return Err(Error::new("the file is empty: it has no header line")),
            }
        };
        let delimiter = if header.contains('|') { '|' } else { '\t' };
        let columns: BTreeMap<String, usize> = header
            .split(delimiter)
            .enumerate()
            .map(|(index, name)| (name.trim().trim_start_matches('\u{feff}').to_ascii_uppercase(), index))
            .collect();
        let missing: Vec<&str> = required
            .iter()
            .copied()
            .filter(|name| !columns.contains_key(*name))
            .collect();
        if !missing.is_empty() {
            return Err(Error::new(format!("the header has no {} column", missing.join(", "))));
        }
        Ok(Self {
            lines,
            columns,
            delimiter,
        })
    }

    /// Each data row's line number, text and fields, or its rejection; blank lines are left out. Returns the rows read.
    /// `key_of` reads what a rejected row was about from its fields (of a line too long, its start), where `each` did
    /// not say.
    fn rows(
        &mut self,
        mut each: impl FnMut(u64, &str, &[&str], &BTreeMap<String, usize>) -> Option<RowError>,
        key_of: &KeyOf,
        rejected: &mut dyn FnMut(RowError),
    ) -> Result<u64, Error> {
        let mut count = 0;
        while let Some(line) = self.lines.next_line()? {
            match line {
                Line::Text(_, text) if text.trim().is_empty() => {}
                Line::Text(number, text) => {
                    count += 1;
                    let fields: Vec<&str> = text.split(self.delimiter).collect();
                    if let Some(mut error) = each(number, &text, &fields, &self.columns) {
                        if error.key.is_none() {
                            error.key = key_of(&fields, &self.columns).map(Box::new);
                        }
                        rejected(error);
                    }
                }
                Line::Rejected(mut error) => {
                    count += 1;
                    let key = {
                        let fields: Vec<&str> = error.text.split(self.delimiter).collect();
                        key_of(&fields, &self.columns)
                    };
                    error.key = key.map(Box::new);
                    rejected(error);
                }
            }
        }
        Ok(count)
    }
}

/// The field of `column`, trimmed; empty when the row is short.
fn field<'f>(fields: &[&'f str], columns: &BTreeMap<String, usize>, column: &str) -> &'f str {
    columns
        .get(column)
        .and_then(|index| fields.get(*index))
        .map_or("", |text| text.trim())
}

/// The Gazetteer's ZCTA file: each ZCTA as a US postal code (full, leading zeros kept) with its internal point, a
/// postal-area point.
///
/// # Errors
///
/// The header lacks GEOID, INTPTLAT or INTPTLONG, or the file does not read.
pub fn parse_zcta(
    reader: &mut dyn BufRead,
    each: &mut dyn FnMut(Result<PostalRecord, RowError>),
) -> Result<u64, Error> {
    let mut headed = Headed::open(reader, &["GEOID", "INTPTLAT", "INTPTLONG"])?;
    let each = std::cell::RefCell::new(each);
    headed.rows(
        |line, text, fields, columns| {
            let code = match normalize_postal_code("US", field(fields, columns, "GEOID")) {
                Ok(code) => code,
                Err(message) => return Some(RowError::new(line, "GEOID", message, text)),
            };
            let point = match parse_point(field(fields, columns, "INTPTLAT"), field(fields, columns, "INTPTLONG")) {
                Ok(point) => point,
                Err((name, message)) => return Some(RowError::new(line, name, message, text)),
            };
            (each.borrow_mut())(Ok(PostalRecord {
                line,
                country: "US".to_string(),
                code,
                place_name: None,
                admin_name1: None,
                admin_code1: None,
                admin_name2: None,
                admin_code2: None,
                point: Some(point),
                accuracy: None,
                precision: Precision::PostalArea,
            }));
            None
        },
        &|fields, columns| zcta_key(field(fields, columns, "GEOID")),
        &mut |error| (each.borrow_mut())(Err(error)),
    )
}

/// The Gazetteer's Counties or Places file: each row's state (USPS), FIPS GEOID, name and internal point.
///
/// # Errors
///
/// The header lacks USPS, GEOID, NAME, INTPTLAT or INTPTLONG, or the file does not read.
pub fn parse_divisions(
    reader: &mut dyn BufRead,
    each: &mut dyn FnMut(Result<DivisionRecord, RowError>),
) -> Result<u64, Error> {
    let mut headed = Headed::open(reader, &["USPS", "GEOID", "NAME", "INTPTLAT", "INTPTLONG"])?;
    let each = std::cell::RefCell::new(each);
    headed.rows(
        |line, text, fields, columns| {
            let state = field(fields, columns, "USPS").to_ascii_uppercase();
            if state.len() != 2 || !state.chars().all(|c| c.is_ascii_alphabetic()) {
                return Some(RowError::new(
                    line,
                    "USPS",
                    format!("state {state:?} is not a USPS code"),
                    text,
                ));
            }
            let geoid = field(fields, columns, "GEOID");
            if geoid.is_empty() || !geoid.chars().all(|c| c.is_ascii_digit()) {
                return Some(RowError::new(
                    line,
                    "GEOID",
                    format!("GEOID {geoid:?} is not digits"),
                    text,
                ));
            }
            let Some(name) = non_empty(field(fields, columns, "NAME")) else {
                return Some(RowError::new(line, "NAME", "the name is empty", text));
            };
            let point = match parse_point(field(fields, columns, "INTPTLAT"), field(fields, columns, "INTPTLONG")) {
                Ok(point) => point,
                Err((name, message)) => return Some(RowError::new(line, name, message, text)),
            };
            (each.borrow_mut())(Ok(DivisionRecord {
                line,
                state,
                geoid: geoid.to_string(),
                name,
                point,
            }));
            None
        },
        &|fields, columns| fips_key(field(fields, columns, "GEOID")),
        &mut |error| (each.borrow_mut())(Err(error)),
    )
}

/// The 2020 ZCTA-to-county relationship file: each row's ZCTA (`GEOID_ZCTA5_20`), county (`GEOID_COUNTY_20`) and the
/// land area of their overlap (`AREALAND_PART`). A row with the ZCTA empty (the part of a county outside every ZCTA) is
/// left out, neither a record nor a rejection.
///
/// # Errors
///
/// The header lacks one of those columns, or the file does not read.
pub fn parse_zcta_county(
    reader: &mut dyn BufRead,
    each: &mut dyn FnMut(Result<ZctaCountyRecord, RowError>),
) -> Result<u64, Error> {
    let mut headed = Headed::open(reader, &["GEOID_ZCTA5_20", "GEOID_COUNTY_20", "AREALAND_PART"])?;
    let each = std::cell::RefCell::new(each);
    headed.rows(
        |line, text, fields, columns| {
            let zcta = field(fields, columns, "GEOID_ZCTA5_20");
            if zcta.is_empty() {
                return None;
            }
            let zcta = match normalize_postal_code("US", zcta) {
                Ok(code) => code.normalized,
                Err(message) => return Some(RowError::new(line, "GEOID_ZCTA5_20", message, text)),
            };
            let county = field(fields, columns, "GEOID_COUNTY_20");
            if county.len() != 5 || !county.chars().all(|c| c.is_ascii_digit()) {
                return Some(RowError::new(
                    line,
                    "GEOID_COUNTY_20",
                    format!("county {county:?} is not a five-digit FIPS code"),
                    text,
                ));
            }
            let land = field(fields, columns, "AREALAND_PART");
            let Ok(land_part) = land.parse::<u64>() else {
                return Some(RowError::new(
                    line,
                    "AREALAND_PART",
                    format!("land area {land:?} is not a whole number"),
                    text,
                ));
            };
            (each.borrow_mut())(Ok(ZctaCountyRecord {
                line,
                zcta,
                county: county.to_string(),
                land_part,
            }));
            None
        },
        &|_, _| None,
        &mut |error| (each.borrow_mut())(Err(error)),
    )
}

/// The county of each ZCTA: the one with the largest land share, the first in the file among equals. In ZCTA order.
#[must_use]
pub fn largest_share(records: &[ZctaCountyRecord]) -> Vec<&ZctaCountyRecord> {
    let mut best: BTreeMap<&str, &ZctaCountyRecord> = BTreeMap::new();
    for record in records {
        best.entry(record.zcta.as_str())
            .and_modify(|kept| {
                if record.land_part > kept.land_part {
                    *kept = record;
                }
            })
            .or_insert(record);
    }
    best.into_values().collect()
}
