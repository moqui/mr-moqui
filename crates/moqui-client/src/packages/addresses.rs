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

//! The offline search of a package's Overture addresses (plans/Maps-Offline.md §2.5): the server's address matching
//! (`moqui_lib::geo::address`, which builds with no Parquet reader or codec, the same code `search#OvertureAddresses`
//! ranks with) over the package's `overture-addresses.json`, whose rows carry the keys the server made of each point
//! (`streetNorm`, `numberNorm`, `unitNorm`, `postcodeNorm`). The text is read into its parts for each country of the
//! rows, its street's keys looked up, then the number, the number without its suffix and the street alone, as the
//! server's search does; the best tier and score of the countries wins. The file is read for each search (a search is
//! asked for, not typed through), and nothing is kept or sent.

use std::collections::{BTreeSet, HashMap};

use moqui_lib::geo::address::{self, Candidate, Point, Query, Ranked, Tier};
use serde_json::{json, Map, Value};

use super::Packages;

/// The member a package's addresses are in.
pub const ADDRESSES_MEMBER: &str = "overture-addresses.json";
/// The most street keys tried for a text, and countries for a text read as each of them, as the server's search.
const MAX_STREETS: usize = 8;
const MAX_COUNTRIES: usize = 6;
/// The most points of one number of a street, and of one street, as the server reads them.
const NUMBER_ROWS: usize = 200;
const STREET_ROWS: usize = 500;

/// The rows of a package's addresses, by country and street key.
struct Index {
    points: Vec<Point>,
    by_street: HashMap<(String, String), Vec<usize>>,
    countries: BTreeSet<String>,
}

fn field(row: &Map<String, Value>, name: &str) -> String {
    match row.get(name) {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn degrees(row: &Map<String, Value>, name: &str) -> Option<f64> {
    match row.get(name) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(text)) => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|value: &f64| value.is_finite())
}

/// A row of the member as the matcher's point (the fields `address_service::point_of` reads of a stored row).
fn point_of(row: &Map<String, Value>, release: &str) -> Option<Point> {
    let mut places: Vec<String> = field(row, "levelsText")
        .split(" | ")
        .map(address::fold)
        .filter(|place| !place.is_empty())
        .collect();
    let postal_city = address::fold(&field(row, "postalCity"));
    if !postal_city.is_empty() {
        places.push(postal_city);
    }
    Some(Point {
        address_id: field(row, "gersId"),
        country: field(row, "countryCode").to_ascii_uppercase(),
        street: field(row, "streetName"),
        number: field(row, "houseNumber"),
        number_key: field(row, "numberNorm"),
        unit: field(row, "unitText"),
        unit_key: field(row, "unitNorm"),
        postcode: field(row, "postcode"),
        postcode_key: field(row, "postcodeNorm"),
        locality: field(row, "locality"),
        region: field(row, "region"),
        places,
        lon: degrees(row, "longitude").filter(|lon| lon.abs() <= 180.0)?,
        lat: degrees(row, "latitude").filter(|lat| lat.abs() <= 90.0)?,
        gers_id: field(row, "gersId"),
        release: release.to_string(),
        datasets: field(row, "sourceDatasets"),
        licenses: field(row, "sourceLicenses"),
    })
}

impl Index {
    fn of(rows: &[Value], release: &str) -> Self {
        let mut index = Self {
            points: Vec::new(),
            by_street: HashMap::new(),
            countries: BTreeSet::new(),
        };
        for row in rows.iter().filter_map(Value::as_object) {
            let street = field(row, "streetNorm");
            let Some(point) = point_of(row, release).filter(|point| !street.is_empty() && point.country.len() == 2)
            else {
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
}

/// The best tier found for one country's reading of the text (`address_service::search_country`'s steps).
fn search_country(index: &Index, query: &Query, country: &str, near: Option<(f64, f64)>) -> Option<(Tier, Ranked)> {
    let number = query
        .number
        .as_deref()
        .map(address::number_key)
        .filter(|key| !key.is_empty());
    for (street, locality) in query.streets(country).into_iter().take(MAX_STREETS) {
        let locality = locality.as_deref().map(address::fold);
        if let Some(number) = number.as_deref() {
            let found = index.rows(country, &street, Some(number));
            if !found.is_empty() {
                return Some((
                    Tier::Address,
                    address::rank(query, locality.as_deref(), Tier::Address, found, near),
                ));
            }
            if let Some(base) = address::number_base(number) {
                let found = index.rows(country, &street, Some(&base));
                let suffix = number
                    .get(base.len()..)
                    .unwrap_or_default()
                    .trim_matches(['/', '-'])
                    .to_string();
                if query.unit.is_none() && !suffix.is_empty() && found.iter().any(|point| point.unit_key == suffix) {
                    let as_unit = Query {
                        unit: Some(suffix.clone()),
                        ..query.clone()
                    };
                    let units: Vec<Point> = found.into_iter().filter(|point| point.unit_key == suffix).collect();
                    return Some((
                        Tier::Address,
                        address::rank(&as_unit, locality.as_deref(), Tier::Address, units, near),
                    ));
                }
                if !found.is_empty() {
                    return Some((
                        Tier::NumberBase,
                        address::rank(query, locality.as_deref(), Tier::NumberBase, found, near),
                    ));
                }
            }
        }
        let found = index.rows(country, &street, None);
        if !found.is_empty() {
            return Some((
                Tier::Street,
                address::rank(query, locality.as_deref(), Tier::Street, found, near),
            ));
        }
    }
    None
}

fn tier_order(tier: Tier) -> u8 {
    match tier {
        Tier::Address => 0,
        Tier::NumberBase => 1,
        Tier::Street => 2,
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

/// Search the rows of a package's addresses member (`{rows: [...]}`) for `text`, read as `country` (ISO alpha-2) or as
/// each country of the rows; `near` (the map's middle) breaks ties. Answers `{valid, candidates, ambiguous}`, or
/// `valid: false` with `reasonCode` QUERY_UNREAD (no street in the text) or NO_MATCH, in the shape of the server's
/// `search#OvertureAddresses`.
pub fn search_rows(
    rows: &[Value],
    release: &str,
    text: &str,
    country: &str,
    near: Option<(f64, f64)>,
    limit: usize,
) -> Value {
    let index = Index::of(rows, release);
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
        if let Some((tier, ranked)) = search_country(&index, &query, country, near) {
            let better = match &best {
                None => true,
                Some((best_tier, best_ranked)) => {
                    tier_order(tier) < tier_order(*best_tier)
                        || tier == *best_tier
                            && ranked.candidates.first().map_or(i32::MIN, |c| c.score)
                                > best_ranked.candidates.first().map_or(i32::MIN, |c| c.score)
                }
            };
            if better {
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

impl Packages {
    /// Search the installed revision of an account's package for an address ([`search_rows`] over its
    /// `overture-addresses.json`): `valid: false` with `reasonCode` NO_DATA when the package has no addresses.
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
        if !manifest.members.contains_key(ADDRESSES_MEMBER) {
            return no_data();
        }
        let read = std::fs::read(dir.join(ADDRESSES_MEMBER))
            .map_err(|err| err.to_string())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|err| err.to_string()));
        let member = match read {
            Ok(member) => member,
            Err(err) => {
                tracing::error!("The addresses of the offline map {package_id} were not read: {err}");
                return no_data();
            }
        };
        let rows = member["rows"].as_array().map_or(&[][..], Vec::as_slice);
        search_rows(rows, "", text, country, near, limit)
    }
}

#[cfg(test)]
#[path = "addresses_tests.rs"]
mod tests;
