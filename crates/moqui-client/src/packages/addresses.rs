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

//! The offline search of a package's Overture addresses (plans/archive/Maps-Offline.md §2.5): the server's address matching
//! (`moqui_lib::geo::address`, which builds with no Parquet reader or codec; its `search_tiers` is the walk the server's
//! `search#OvertureAddresses` takes too) over the package's `overture-addresses.json`, whose rows carry the keys the
//! server made of each point (`streetNorm`, `numberNorm`, `unitNorm`, `postcodeNorm`). The text is read into its parts
//! for each country of the rows and the best tier and score of the countries wins.
//!
//! The member (up to 200,000 rows) is read once into typed rows of the fields used, streamed from the file, and kept as
//! an index by street key for one package revision of one account at a time ([`Packages`]' cache, keyed by the
//! revision's directory and the member's SHA-256): another package or account replaces it, and a delete, a new
//! revision installed, an account's data forgotten or an account switch drops it. Nothing is sent.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_lib::geo::address::{self, Candidate, Point, Ranked, Tier};
use serde::{Deserialize, Deserializer};
use serde_json::json;
use serde_json::Value;

use super::Packages;

/// The member a package's addresses are in.
pub const ADDRESSES_MEMBER: &str = "overture-addresses.json";
/// The most countries a text with none is read as, as the server's search.
const MAX_COUNTRIES: usize = 6;
/// The most points of one number of a street, and of one street, as the server reads them.
const NUMBER_ROWS: usize = 200;
const STREET_ROWS: usize = 500;
/// The most rows of a member read (the server writes at most 200,000), and the longest text of a field kept.
const MAX_ROWS: usize = 200_000;
const FIELD_CHARS: usize = 256;

/// A text field of a row: a string, a number written as text, or nothing (read as one value at a time, so a number keeps
/// its digits whatever serde_json's number features).
fn text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => text.trim().chars().take(FIELD_CHARS).collect(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}

/// A member's rows, at most [`MAX_ROWS`]: each read as one value and then as a row, so one that is not a row (not an
/// object, a field of no kind a row has) is logged and skipped and the rest are read (AGENTS.md, failure tolerance).
fn rows<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Row>, D::Error> {
    struct Rows;
    impl<'de> serde::de::Visitor<'de> for Rows {
        type Value = Vec<Row>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a list of address rows")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<Row>, A::Error> {
            let mut out = Vec::new();
            let (mut skipped, mut past) = (0usize, 0usize);
            while out.len() < MAX_ROWS {
                let Some(value) = seq.next_element::<Value>()? else {
                    break;
                };
                // a row is an object: anything else (an array, a text) is not one
                if !value.is_object() {
                    skipped += 1;
                    continue;
                }
                match Row::deserialize(value) {
                    Ok(row) => out.push(row),
                    Err(_) => skipped += 1,
                }
            }
            // past the cap the rows are passed over, not read
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                past += 1;
            }
            if skipped > 0 {
                tracing::warn!("{skipped} rows of an offline map's addresses are not rows, and were left out");
            }
            if past > 0 {
                tracing::warn!("{past} rows of an offline map's addresses past {MAX_ROWS} were left out");
            }
            Ok(out)
        }
    }
    deserializer.deserialize_seq(Rows)
}

/// A degree of a row: a number, or a number written as text.
fn degrees<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    let value = text(deserializer)?;
    Ok(value.parse::<f64>().ok().filter(|value| value.is_finite()))
}

/// A row of the member as OF1's MapPackages.js writes it: only the fields the matcher reads.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Row {
    #[serde(deserialize_with = "text")]
    gers_id: String,
    #[serde(deserialize_with = "text")]
    country_code: String,
    #[serde(deserialize_with = "text")]
    street_name: String,
    #[serde(deserialize_with = "text")]
    street_norm: String,
    #[serde(deserialize_with = "text")]
    house_number: String,
    #[serde(deserialize_with = "text")]
    number_norm: String,
    #[serde(deserialize_with = "text")]
    unit_text: String,
    #[serde(deserialize_with = "text")]
    unit_norm: String,
    #[serde(deserialize_with = "text")]
    postcode: String,
    #[serde(deserialize_with = "text")]
    postcode_norm: String,
    #[serde(deserialize_with = "text")]
    postal_city: String,
    #[serde(deserialize_with = "text")]
    locality: String,
    #[serde(deserialize_with = "text")]
    region: String,
    #[serde(deserialize_with = "text")]
    levels_text: String,
    #[serde(deserialize_with = "degrees")]
    latitude: Option<f64>,
    #[serde(deserialize_with = "degrees")]
    longitude: Option<f64>,
    #[serde(deserialize_with = "text")]
    source_datasets: String,
    #[serde(deserialize_with = "text")]
    source_licenses: String,
}

/// The member: its rows.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Member {
    #[serde(deserialize_with = "rows")]
    rows: Vec<Row>,
}

/// A row as the matcher's point (the fields `address_service::point_of` reads of a stored row), taking its texts.
fn point_of(row: Row) -> Option<Point> {
    let mut places: Vec<String> = row
        .levels_text
        .split(" | ")
        .map(address::fold)
        .filter(|place| !place.is_empty())
        .collect();
    let postal_city = address::fold(&row.postal_city);
    if !postal_city.is_empty() {
        places.push(postal_city);
    }
    Some(Point {
        address_id: row.gers_id.clone(),
        country: row.country_code.to_ascii_uppercase(),
        street: row.street_name,
        number: row.house_number,
        number_key: row.number_norm,
        unit: row.unit_text,
        unit_key: row.unit_norm,
        postcode: row.postcode,
        postcode_key: row.postcode_norm,
        locality: row.locality,
        region: row.region,
        places,
        lon: row.longitude.filter(|lon| lon.abs() <= 180.0)?,
        lat: row.latitude.filter(|lat| lat.abs() <= 90.0)?,
        gers_id: row.gers_id,
        release: String::new(),
        datasets: row.source_datasets,
        licenses: row.source_licenses,
    })
}

/// The points of a package's addresses, by country and street key.
pub(crate) struct Index {
    points: Vec<Point>,
    by_street: HashMap<(String, String), Vec<usize>>,
    countries: BTreeSet<String>,
}

impl Index {
    fn of(rows: Vec<Row>) -> Self {
        let mut index = Self {
            points: Vec::new(),
            by_street: HashMap::new(),
            countries: BTreeSet::new(),
        };
        for mut row in rows {
            let street = std::mem::take(&mut row.street_norm);
            let Some(point) = point_of(row).filter(|point| !street.is_empty() && point.country.len() == 2) else {
                continue;
            };
            index.countries.insert(point.country.clone());
            index
                .by_street
                .entry((point.country.clone(), street))
                .or_default()
                .push(index.points.len());
            index.points.push(point);
        }
        index
    }

    /// The points of a street of a country, those of one number key when given, at most as many as the server reads.
    fn rows(&self, country: &str, street: &str, number: Option<&str>) -> Vec<Point> {
        let Some(found) = self.by_street.get(&(country.to_string(), street.to_string())) else {
            return Vec::new();
        };
        let points = found.iter().map(|at| &self.points[*at]);
        match number {
            Some(number) => points
                .filter(|point| point.number_key == number)
                .take(NUMBER_ROWS)
                .cloned()
                .collect(),
            None => points.take(STREET_ROWS).cloned().collect(),
        }
    }

    /// Read from a member's file, streamed into typed rows.
    fn read(file: &Path) -> Result<Self, String> {
        let opened = std::fs::File::open(file).map_err(|err| err.to_string())?;
        let member: Member = serde_json::from_reader(std::io::BufReader::new(opened)).map_err(|err| err.to_string())?;
        Ok(Self::of(member.rows))
    }
}

/// A candidate as the offline screen shows it: its label and point, how good a match it is, and its sources.
fn candidate_json(candidate: &Candidate) -> Value {
    let point = &candidate.point;
    let street_only = candidate.tier == Tier::Street;
    json!({
        "label": address::label(point, candidate.tier),
        "lon": point.lon,
        "lat": point.lat,
        "tier": format!("{:?}", candidate.tier).to_lowercase(),
        "matchKind": candidate.tier.match_kind(),
        "precisionEnumId": candidate.precision(),
        "points": candidate.points,
        "extent": candidate.extent,
        "street": address::display(&point.street),
        "number": if street_only { "" } else { point.number.as_str() },
        "postcode": point.postcode,
        "locality": address::display(&point.locality),
        "countryCode": point.country,
        "sourceDatasets": point.datasets,
        "sourceLicenses": point.licenses,
        "postcodeAgrees": candidate.postcode_agrees,
        "localityAgrees": candidate.locality_agrees,
    })
}

/// Search an index for `text`, read as `country` (ISO alpha-2) or as each country of its rows; `near` (the map's
/// middle) breaks ties. Answers `{valid, candidates, ambiguous}`, or `valid: false` with `reasonCode` QUERY_UNREAD (no
/// street in the text) or NO_MATCH, in the shape of the server's `search#OvertureAddresses`.
pub(crate) fn search_index(index: &Index, text: &str, country: &str, near: Option<(f64, f64)>, limit: usize) -> Value {
    let asked = country.trim().to_ascii_uppercase();
    let countries: Vec<String> = if asked.len() == 2 {
        vec![asked]
    } else {
        index.countries.iter().take(MAX_COUNTRIES).cloned().collect()
    };
    let mut best: Option<(Tier, Ranked)> = None;
    let mut read_any = false;
    for country in &countries {
        let Some(query) = address::parse(text, country) else {
            continue;
        };
        read_any = true;
        let found = address::search_tiers::<std::convert::Infallible>(&query, country, near, &mut |street, number| {
            Ok(index.rows(country, street, number))
        });
        if let Ok(Some((tier, ranked))) = found {
            if address::better(tier, &ranked, best.as_ref().map(|(tier, ranked)| (*tier, ranked))) {
                best = Some((tier, ranked));
            }
        }
    }
    match best {
        Some((_, ranked)) => json!({
            "valid": true,
            "ambiguous": ranked.ambiguous,
            "candidates": ranked.candidates.iter().take(limit.clamp(1, 20)).map(candidate_json).collect::<Vec<_>>(),
        }),
        None if !read_any => json!({ "valid": false, "reasonCode": "QUERY_UNREAD", "candidates": [] }),
        None => json!({ "valid": false, "reasonCode": "NO_MATCH", "candidates": [] }),
    }
}

/// [`search_index`] over the rows of a member's JSON (`{rows: [...]}`), for a caller that holds them.
///
/// # Errors
///
/// Returns why the JSON is not a member's.
pub fn search_rows(
    member: &str,
    text: &str,
    country: &str,
    near: Option<(f64, f64)>,
    limit: usize,
) -> Result<Value, String> {
    let member: Member = serde_json::from_str(member).map_err(|err| err.to_string())?;
    Ok(search_index(&Index::of(member.rows), text, country, near, limit))
}

/// The one index the device keeps: the revision directory and the member's SHA-256 it was read from, and the index, or
/// none when that member could not be read (not read again for the same revision on every search).
pub(crate) type Cached = Option<(PathBuf, String, Option<Arc<Index>>)>;

impl Packages {
    /// Search the installed revision of an account's package for an address ([`search_index`] over its
    /// `overture-addresses.json`, read once and kept for that revision): `valid: false` with `reasonCode` NO_DATA when
    /// the package has no addresses.
    pub fn search_addresses(
        &self,
        account: &str,
        package_id: &str,
        text: &str,
        country: &str,
        near: Option<(f64, f64)>,
        limit: usize,
    ) -> Value {
        let no_data = || json!({ "valid": false, "reasonCode": "NO_DATA", "candidates": [] });
        let Some((_, dir)) = self.installed_dir(account, package_id) else {
            return no_data();
        };
        let Some(manifest) = self.manifest(&dir) else {
            return no_data();
        };
        let Some(member) = manifest.members.get(ADDRESSES_MEMBER) else {
            return no_data();
        };
        let kept = moqui_lib::sync::lock(&self.addresses)
            .as_ref()
            .filter(|(at, sha, _)| *at == dir && *sha == member.sha256)
            .map(|(_, _, index)| index.clone());
        let index = match kept {
            Some(Some(index)) => index,
            // its read failed before: said once, then NO_DATA until another revision
            Some(None) => return no_data(),
            None => {
                // the one kept goes first, so two are never held at once
                *moqui_lib::sync::lock(&self.addresses) = None;
                let read = Index::read(&dir.join(ADDRESSES_MEMBER)).map(Arc::new);
                if let Err(err) = &read {
                    tracing::error!("The addresses of the offline map {package_id} were not read: {err}");
                }
                let index = read.ok();
                *moqui_lib::sync::lock(&self.addresses) = Some((dir.clone(), member.sha256.clone(), index.clone()));
                match index {
                    Some(index) => index,
                    None => return no_data(),
                }
            }
        };
        search_index(&index, text, country, near, limit)
    }

    /// Drop the kept address index (an account switch; and of the packages under `dir`, a delete, a new revision, an
    /// account's data forgotten, when given).
    pub fn drop_address_index(&self, under: Option<&Path>) {
        let mut kept = moqui_lib::sync::lock(&self.addresses);
        if under.is_none_or(|dir| kept.as_ref().is_some_and(|(at, _, _)| at.starts_with(dir))) {
            *kept = None;
        }
    }

    /// Whether an address index is kept (a test's).
    #[cfg(test)]
    pub(crate) fn address_index_kept(&self) -> bool {
        moqui_lib::sync::lock(&self.addresses).is_some()
    }
}

#[cfg(test)]
#[path = "addresses_tests.rs"]
mod tests;
