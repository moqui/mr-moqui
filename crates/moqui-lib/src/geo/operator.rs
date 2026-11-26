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

//! An operator's postal file (plans/Maps.md §6.4): CSV with a header of our own, read by column name in any order
//! and case: `country` (ISO alpha-2 or alpha-3), `code`, `latitude` and `longitude` required (a row may leave both
//! coordinates empty: a code with no point), `place`, `admin1`, `admin2` and `precision` (an enum ID or `address`,
//! `street`, `postal`, `place`; empty is postal) optional. Quoted fields as CSV has them, within one line: each line is
//! read by the bounded line reader and then as CSV, so an unclosed quote spoils its own row only and no record grows
//! past a line's limit. A byte order mark is ignored.

use std::collections::BTreeMap;
use std::io::BufRead;

use crate::error::Error;

use super::postal::PostalRecord;
use super::text::{Line, Lines};
use super::{non_empty, normalize_postal_code, parse_optional_point, Granularity, Precision, RowError, RowKey};

const REQUIRED: [&str; 4] = ["country", "code", "latitude", "longitude"];

/// The cells of one CSV line; None when it does not read as CSV.
fn cells_of(text: &str) -> Option<Vec<String>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut record = csv::StringRecord::new();
    match reader.read_record(&mut record) {
        Ok(true) => Some(record.iter().map(str::to_string).collect()),
        _ => None,
    }
}

/// Each row of an operator's file as a postal record.
///
/// # Errors
///
/// The header lacks a required column, or the file does not read.
pub fn parse(reader: &mut dyn BufRead, each: &mut dyn FnMut(Result<PostalRecord, RowError>)) -> Result<u64, Error> {
    let mut lines = Lines::new(reader);
    let header = loop {
        match lines.next_line()? {
            Some(Line::Text(_, text)) if text.trim().is_empty() => {}
            Some(Line::Text(_, text)) => {
                break cells_of(&text).ok_or_else(|| Error::new("the header line does not read as CSV"))?;
            }
            Some(Line::Rejected(error)) => {
                return Err(Error::new(format!(
                    "the header line could not be read: {}",
                    error.message
                )))
            }
            None => return Err(Error::new("the file is empty: it has no header line")),
        }
    };
    let columns: BTreeMap<String, usize> = header
        .iter()
        .enumerate()
        .map(|(index, name)| (name.trim().trim_start_matches('\u{feff}').to_ascii_lowercase(), index))
        .collect();
    let missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|name| !columns.contains_key(*name))
        .collect();
    if !missing.is_empty() {
        return Err(Error::new(format!("the header has no {} column", missing.join(", "))));
    }
    let mut count = 0;
    while let Some(line) = lines.next_line()? {
        match line {
            Line::Text(_, text) if text.trim().is_empty() => {}
            Line::Text(number, text) => {
                count += 1;
                let Some(cells) = cells_of(&text) else {
                    each(Err(RowError::new(number, "", "the row does not read as CSV", &text)));
                    continue;
                };
                let cells: Vec<&str> = cells.iter().map(String::as_str).collect();
                each(row(number, &text, &cells, &columns));
            }
            Line::Rejected(mut error) => {
                count += 1;
                let key = cells_of(&error.text).and_then(|cells| {
                    let cells: Vec<&str> = cells.iter().map(String::as_str).collect();
                    key_of(&cells, &columns)
                });
                error.key = key.map(Box::new);
                each(Err(error));
            }
        }
    }
    Ok(count)
}

fn cell<'c>(cells: &[&'c str], columns: &BTreeMap<String, usize>, name: &str) -> &'c str {
    columns
        .get(name)
        .and_then(|index| cells.get(*index))
        .map_or("", |text| text.trim())
}

/// A row's key, when its country and code read.
fn key_of(cells: &[&str], columns: &BTreeMap<String, usize>) -> Option<RowKey> {
    let country = cell(cells, columns, "country").to_ascii_uppercase();
    let code = normalize_postal_code(&country, cell(cells, columns, "code")).ok()?;
    (!country.is_empty()).then_some(RowKey::Postal { country, code })
}

fn row(line: u64, text: &str, cells: &[&str], columns: &BTreeMap<String, usize>) -> Result<PostalRecord, RowError> {
    let cell = |name: &str| cell(cells, columns, name);
    let country = cell("country").to_ascii_uppercase();
    if !(2..=3).contains(&country.len()) || !country.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(RowError::new(
            line,
            "country",
            format!("country {country:?} is not an ISO alpha-2 or alpha-3 code"),
            text,
        ));
    }
    let code =
        normalize_postal_code(&country, cell("code")).map_err(|message| RowError::new(line, "code", message, text))?;
    // past the code, a rejection names it, so the code is not retired for this row's fault
    let key = || {
        Some(RowKey::Postal {
            country: country.clone(),
            code: code.clone(),
        })
    };
    let point = parse_optional_point(cell("latitude"), cell("longitude"))
        .map_err(|(name, message)| RowError::new(line, name, message, text).with_key(key()))?;
    let mut precision = Precision::parse(cell("precision"))
        .map_err(|message| RowError::new(line, "precision", message, text).with_key(key()))?;
    if code.granularity == Granularity::Prefix && precision != Precision::PlaceEstimate {
        precision = Precision::PlaceEstimate;
    }
    Ok(PostalRecord {
        line,
        country,
        code,
        place_name: non_empty(cell("place")),
        admin_name1: None,
        admin_code1: non_empty(cell("admin1")),
        admin_name2: None,
        admin_code2: non_empty(cell("admin2")),
        point,
        accuracy: None,
        precision,
    })
}
