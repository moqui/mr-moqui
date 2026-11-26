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

//! O2's address index (plans/Maps-Overture.md §O2): a run's address points written as `moqui.maps.OvertureAddress`
//! rows with their keys ([`write_addresses`]), what a refresh changed against the run active before it
//! ([`Changes`], counted by address, never by row or GERS ID: Overture's address IDs are not stable), and
//! `search#OvertureAddresses` ([`search_overture_addresses`]), the local geocoder's search: the rows of the active Addresses
//! runs read by their indexed street and number keys, ranked by [`crate::address`]. Nothing is asked of any outside
//! service, and no record, Geo or point outside these rows is read or written.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value, ValueMap};

use crate::address::{self, Candidate, Point, Query, Ranked, Tier};
use crate::overture::AddressPoint;

pub const ADDRESS: &str = "moqui.maps.OvertureAddress";
const REGION_THEME: &str = "moqui.maps.OvertureRegionTheme";
const IMPORT: &str = "moqui.maps.OvertureImport";
/// The theme's `OvertureTheme` ID.
pub const THEME_ID: &str = "ADDRESSES";
/// The most points read for one number of one street, and for one street.
const NUMBER_ROWS: i64 = 200;
const STREET_ROWS: i64 = 500;
/// The most street keys tried for a text of one part, countries for a text with none, and indexed queries a search sends
/// in all.
const MAX_STREETS: usize = 8;
const MAX_COUNTRIES: usize = 6;
const MAX_QUERIES: usize = 24;
/// An address whose point moved farther than this between two releases counts as moved.
const MOVED_M: f64 = 25.0;
/// The rows of the run before read at a time for the comparison.
const PAGE: i64 = 5000;

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

fn number(row: &EntityValue, name: &str) -> Option<f64> {
    field(row, name)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default().to_string()
}

/// The keys of an address point: its street, number, unit and postal code as the matching reads them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub street: String,
    pub number: String,
    pub unit: String,
    pub postcode: String,
}

impl Keys {
    /// The keys of `point`, each cut to the length its row keeps, so a refresh compares a new run with the rows of the run
    /// before on the same keys.
    #[must_use]
    pub fn of(point: &AddressPoint) -> Self {
        Self {
            street: cut(
                &point
                    .street
                    .as_deref()
                    .map(|street| address::street_key(street, &point.country))
                    .unwrap_or_default(),
                255,
            ),
            number: cut(
                &point.number.as_deref().map(address::number_key).unwrap_or_default(),
                63,
            ),
            unit: cut(&point.unit.as_deref().map(address::unit_key).unwrap_or_default(), 63),
            postcode: cut(
                &point
                    .postcode
                    .as_deref()
                    .map(|code| address::postcode_key(code, &point.country))
                    .unwrap_or_default(),
                63,
            ),
        }
    }

    /// The address it is for a refresh's comparison: its country and keys, hashed.
    #[must_use]
    pub fn digest(&self, country: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        (country, &self.street, &self.number, &self.unit, &self.postcode).hash(&mut hasher);
        hasher.finish()
    }
}

/// The fields of the row of `point`, the `index`th of run `import_id`.
#[must_use]
pub fn address_fields(point: &AddressPoint, keys: &Keys, import_id: &str, index: u64) -> Vec<(&'static str, Value)> {
    let opt = |value: &Option<String>, max: usize| match value {
        Some(text) if !text.trim().is_empty() => Value::string(cut(text.trim(), max)),
        _ => Value::Null,
    };
    let text = |value: &str, max: usize| {
        if value.is_empty() {
            Value::Null
        } else {
            Value::string(cut(value, max))
        }
    };
    let locality = point.levels.last().cloned();
    let region = (point.levels.len() > 1).then(|| point.levels[0].clone());
    vec![
        ("addressId", Value::string(format!("{import_id}-{index}"))),
        ("importId", Value::string(import_id)),
        ("gersId", Value::string(cut(&point.gers_id, 63))),
        ("featureVersion", point.version.map_or(Value::Null, Value::I64)),
        ("countryCode", Value::string(&point.country)),
        ("streetName", opt(&point.street, 255)),
        ("streetNorm", text(&keys.street, 255)),
        ("houseNumber", opt(&point.number, 63)),
        ("numberNorm", text(&keys.number, 63)),
        ("unitText", opt(&point.unit, 63)),
        ("unitNorm", text(&keys.unit, 63)),
        ("postcode", opt(&point.postcode, 63)),
        ("postcodeNorm", text(&keys.postcode, 63)),
        ("postalCity", opt(&point.postal_city, 255)),
        ("locality", opt(&locality, 255)),
        ("region", opt(&region, 63)),
        ("levelsText", text(&point.levels.join(" | "), 255)),
        ("latitude", Value::F64(point.lat)),
        ("longitude", Value::F64(point.lon)),
        ("sourceDatasets", Value::string(cut(&point.datasets.join(", "), 255))),
        ("sourceLicenses", Value::string(cut(&point.licenses.join(", "), 255))),
    ]
}

/// Write a batch of a run's address points in a transaction of its own.
///
/// # Errors
///
/// A row does not write.
pub fn write_addresses(
    facade: &EntityFacade,
    import_id: &str,
    batch: &[(u64, AddressPoint, Keys)],
) -> Result<(), Error> {
    if batch.is_empty() {
        return Ok(());
    }
    facade
        .transactions_for(ADDRESS)?
        .run(TransactionAttribute::ForceNew, None, || {
            for (index, point, keys) in batch {
                let mut row = facade.make_value(ADDRESS)?;
                for (name, value) in address_fields(point, keys, import_id, *index) {
                    row.set(name, value);
                }
                row.create(facade)?;
            }
            Ok(())
        })
}

/// A run's addresses, by digest: each address's mean point and its points (one address may have several: its units, or a
/// number written twice).
#[derive(Default)]
pub struct AddressSet {
    by_digest: HashMap<u64, (f32, f32, u32)>,
}

impl AddressSet {
    pub fn add(&mut self, digest: u64, lon: f64, lat: f64) {
        #[allow(clippy::cast_possible_truncation)]
        let (lon, lat) = (lon as f32, lat as f32);
        let entry = self.by_digest.entry(digest).or_insert((0.0, 0.0, 0));
        let count = entry.2 + 1;
        #[allow(clippy::cast_precision_loss)]
        let weight = 1.0 / count as f32;
        entry.0 += (lon - entry.0) * weight;
        entry.1 += (lat - entry.1) * weight;
        entry.2 = count;
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_digest.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_digest.is_empty()
    }
}

/// What a refresh changed, by address (country, street, number, unit and postal code): added, removed, moved more than
/// [`MOVED_M`], and unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub compared_with: String,
    pub added: u64,
    pub removed: u64,
    pub moved: u64,
    pub unchanged: u64,
}

/// Compare `new` with `old`.
#[must_use]
pub fn compare(new: &AddressSet, old: &AddressSet, compared_with: &str) -> Changes {
    let mut changes = Changes {
        compared_with: compared_with.to_string(),
        ..Changes::default()
    };
    for (digest, (lon, lat, _)) in &new.by_digest {
        match old.by_digest.get(digest) {
            None => changes.added += 1,
            Some((old_lon, old_lat, _)) => {
                let moved = address::distance_m(
                    (f64::from(*lon), f64::from(*lat)),
                    (f64::from(*old_lon), f64::from(*old_lat)),
                );
                if moved > MOVED_M {
                    changes.moved += 1;
                } else {
                    changes.unchanged += 1;
                }
            }
        }
    }
    changes.removed = old
        .by_digest
        .keys()
        .filter(|digest| !new.by_digest.contains_key(digest))
        .count() as u64;
    changes
}

/// The addresses of run `import_id`, read a page at a time.
///
/// # Errors
///
/// A page does not read.
pub fn read_set(facade: &EntityFacade, import_id: &str) -> Result<AddressSet, Error> {
    let mut set = AddressSet::default();
    let mut last = String::new();
    loop {
        let mut find = facade
            .find(ADDRESS)?
            .eq("importId", Value::string(import_id))
            .select_field("addressId,countryCode,streetNorm,numberNorm,unitNorm,postcodeNorm,latitude,longitude");
        if !last.is_empty() {
            find = find.condition(Condition::greater("addressId", Value::string(&last)));
        }
        let rows = find.order_by("addressId", false).limit(PAGE).list()?;
        let rows = rows.into_values();
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let keys = Keys {
                street: field(row, "streetNorm"),
                number: field(row, "numberNorm"),
                unit: field(row, "unitNorm"),
                postcode: field(row, "postcodeNorm"),
            };
            if let (Some(lon), Some(lat)) = (number(row, "longitude"), number(row, "latitude")) {
                set.add(keys.digest(&field(row, "countryCode")), lon, lat);
            }
        }
        last = rows.last().map(|row| field(row, "addressId")).unwrap_or_default();
        if rows.len() < usize::try_from(PAGE).unwrap_or(usize::MAX) {
            break;
        }
    }
    Ok(set)
}

/// The changes as a map for the run's results.
#[must_use]
pub fn changes_value(changes: &Changes) -> Value {
    let mut map = ValueMap::default();
    let count = |value: u64| Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
    map.insert("comparedWith".to_string(), Value::string(&changes.compared_with));
    map.insert("added".to_string(), count(changes.added));
    map.insert("removed".to_string(), count(changes.removed));
    map.insert("moved".to_string(), count(changes.moved));
    map.insert("unchanged".to_string(), count(changes.unchanged));
    Value::Map(map)
}

/// The run active for `region_id`'s addresses, if any.
///
/// # Errors
///
/// The row does not read.
pub fn active_run(facade: &EntityFacade, region_id: &str) -> Result<Option<String>, Error> {
    Ok(facade
        .find(REGION_THEME)?
        .eq("regionId", Value::string(region_id))
        .eq("themeId", Value::string(THEME_ID))
        .one()?
        .map(|row| field(&row, "activeImportId"))
        .filter(|id| !id.is_empty()))
}

// ===== search#OvertureAddresses =====

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, search_overture_addresses)
}

type Out = BTreeMap<String, Value>;

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> String {
    parameters
        .get(key)
        .map(to_plain_string)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The view's middle, from `viewport` (a list or a comma text of west, south, east, north).
fn middle(parameters: &BTreeMap<String, Value>) -> Option<(f64, f64)> {
    let values: Vec<f64> = match parameters.get("viewport") {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| to_plain_string(item).trim().parse::<f64>().ok())
            .collect(),
        Some(other) => to_plain_string(other)
            .split(',')
            .filter_map(|part| part.trim().parse::<f64>().ok())
            .collect(),
        None => Vec::new(),
    };
    match values.as_slice() {
        [west, south, east, north]
            if values.iter().all(|value| value.is_finite()) && west <= east && south <= north =>
        {
            Some(((west + east) / 2.0, (south + north) / 2.0))
        }
        _ => None,
    }
}

/// An active run: its ID, release and countries.
struct ActiveRun {
    import_id: String,
    release: String,
    countries: Vec<String>,
}

fn active_runs(facade: &EntityFacade) -> Result<Vec<ActiveRun>, Error> {
    let mut out = Vec::new();
    for row in facade
        .find(REGION_THEME)?
        .eq("themeId", Value::string(THEME_ID))
        .condition(Condition::is_not_null("activeImportId"))
        .limit(100)
        .list()?
        .iter()
    {
        let import_id = field(row, "activeImportId");
        let Some(run) = facade.find(IMPORT)?.eq("importId", Value::string(&import_id)).one()? else {
            continue;
        };
        out.push(ActiveRun {
            import_id,
            release: field(&run, "releaseVersion"),
            countries: field(&run, "countryCodes")
                .split(',')
                .map(|code| code.trim().to_ascii_uppercase())
                .filter(|code| code.len() == 2)
                .collect(),
        });
    }
    Ok(out)
}

fn point_of(row: &EntityValue, releases: &HashMap<String, String>) -> Option<Point> {
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
        address_id: field(row, "addressId"),
        country: field(row, "countryCode"),
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
        lon: number(row, "longitude")?,
        lat: number(row, "latitude")?,
        gers_id: field(row, "gersId"),
        release: releases.get(&field(row, "importId")).cloned().unwrap_or_default(),
        datasets: field(row, "sourceDatasets"),
        licenses: field(row, "sourceLicenses"),
    })
}

/// The rows of the active runs for `street` in `country`, with `number` when given.
fn rows(
    budget: &mut usize,
    facade: &EntityFacade,
    imports: &[Value],
    country: &str,
    street: &str,
    number: Option<&str>,
    releases: &HashMap<String, String>,
) -> Result<Vec<Point>, Error> {
    // a search sends at most MAX_QUERIES: past them it finds nothing more
    if *budget == 0 {
        return Ok(Vec::new());
    }
    *budget -= 1;
    let mut find = facade
        .find(ADDRESS)?
        .condition(Condition::is_in("importId", imports.to_vec()))
        .eq("streetNorm", Value::string(street))
        .eq("countryCode", Value::string(country));
    let limit = match number {
        Some(number) => {
            find = find.eq("numberNorm", Value::string(number));
            NUMBER_ROWS
        }
        None => STREET_ROWS,
    };
    Ok(find
        .order_by("addressId", false)
        .limit(limit)
        .list()?
        .iter()
        .filter_map(|row| point_of(row, releases))
        .collect())
}

/// The best of the tiers found for one country's reading of the text.
fn search_country(
    budget: &mut usize,
    facade: &EntityFacade,
    imports: &[Value],
    releases: &HashMap<String, String>,
    query: &Query,
    country: &str,
    near: Option<(f64, f64)>,
) -> Result<Option<(Tier, Ranked)>, Error> {
    let number = query
        .number
        .as_deref()
        .map(address::number_key)
        .filter(|key| !key.is_empty());
    for (street, locality) in query.streets(country).into_iter().take(MAX_STREETS) {
        if *budget == 0 {
            break;
        }
        let locality = locality.as_deref().map(address::fold);
        if let Some(number) = number.as_deref() {
            let found = rows(budget, facade, imports, country, &street, Some(number), releases)?;
            if !found.is_empty() {
                return Ok(Some((
                    Tier::Address,
                    address::rank(query, locality.as_deref(), Tier::Address, found, near),
                )));
            }
            if let Some(base) = address::number_base(number) {
                let found = rows(budget, facade, imports, country, &street, Some(&base), releases)?;
                // a letter after the number that the source keeps as the unit (Italy's `12/A`: number 12, unit A) is the
                // address; any other number found without its suffix is an estimate
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
                    return Ok(Some((
                        Tier::Address,
                        address::rank(&as_unit, locality.as_deref(), Tier::Address, units, near),
                    )));
                }
                if !found.is_empty() {
                    return Ok(Some((
                        Tier::NumberBase,
                        address::rank(query, locality.as_deref(), Tier::NumberBase, found, near),
                    )));
                }
            }
        }
        let found = rows(budget, facade, imports, country, &street, None, releases)?;
        if !found.is_empty() {
            return Ok(Some((
                Tier::Street,
                address::rank(query, locality.as_deref(), Tier::Street, found, near),
            )));
        }
    }
    Ok(None)
}

fn tier_order(tier: Tier) -> u8 {
    match tier {
        Tier::Address => 0,
        Tier::NumberBase => 1,
        Tier::Street => 2,
    }
}

fn agrees(value: Option<bool>) -> Value {
    value.map_or(Value::Null, Value::Bool)
}

fn candidate_value(candidate: &Candidate, query: &Query) -> Value {
    let point = &candidate.point;
    let mut map = ValueMap::default();
    let put = |map: &mut ValueMap, key: &str, value: &str| {
        map.insert(
            key.to_string(),
            if value.is_empty() {
                Value::Null
            } else {
                Value::string(value)
            },
        );
    };
    map.insert(
        "label".to_string(),
        Value::string(address::label(point, candidate.tier)),
    );
    map.insert("lon".to_string(), Value::F64(point.lon));
    map.insert("lat".to_string(), Value::F64(point.lat));
    map.insert(
        "tier".to_string(),
        Value::string(format!("{:?}", candidate.tier).to_lowercase()),
    );
    map.insert("matchKind".to_string(), Value::string(candidate.tier.match_kind()));
    map.insert("precisionEnumId".to_string(), Value::string(candidate.precision()));
    map.insert(
        "units".to_string(),
        Value::I64(i64::try_from(candidate.units).unwrap_or(i64::MAX)),
    );
    map.insert("score".to_string(), Value::I64(i64::from(candidate.score)));
    map.insert(
        "points".to_string(),
        Value::I64(i64::try_from(candidate.points).unwrap_or(i64::MAX)),
    );
    map.insert(
        "extent".to_string(),
        candidate.extent.map_or(Value::Null, |extent| {
            Value::List(extent.iter().map(|value| Value::F64(*value)).collect())
        }),
    );
    put(&mut map, "street", &address::display(&point.street));
    put(
        &mut map,
        "number",
        if candidate.tier == Tier::Street {
            ""
        } else {
            &point.number
        },
    );
    put(
        &mut map,
        "unit",
        if candidate.tier == Tier::Street {
            ""
        } else {
            &point.unit
        },
    );
    put(&mut map, "postcode", &point.postcode);
    put(&mut map, "locality", &address::display(&point.locality));
    put(&mut map, "countryCode", &point.country);
    put(
        &mut map,
        "gersId",
        if candidate.tier == Tier::Street {
            ""
        } else {
            &point.gers_id
        },
    );
    put(&mut map, "release", &point.release);
    put(&mut map, "sourceDatasets", &point.datasets);
    put(&mut map, "sourceLicenses", &point.licenses);
    put(&mut map, "numberAsked", query.number.as_deref().unwrap_or_default());
    map.insert("postcodeAgrees".to_string(), agrees(candidate.postcode_agrees));
    map.insert("localityAgrees".to_string(), agrees(candidate.locality_agrees));
    map.insert("unitAgrees".to_string(), agrees(candidate.unit_agrees));
    Value::Map(map)
}

fn parsed_value(query: &Query) -> Value {
    let mut map = ValueMap::default();
    let opt = |value: &Option<String>| value.as_ref().map_or(Value::Null, Value::string);
    map.insert("number".to_string(), opt(&query.number));
    map.insert("street".to_string(), Value::string(query.words.join(" ")));
    map.insert("unit".to_string(), opt(&query.unit));
    map.insert("postcode".to_string(), opt(&query.postcode));
    map.insert("locality".to_string(), opt(&query.locality));
    Value::Map(map)
}

/// `search#OvertureAddresses`: the local geocoder's search of the active Addresses runs. In: `query`, `country` (ISO
/// alpha-2: the country to read the text as; empty for each country of the active runs), `viewport` (its middle breaks
/// ties) and `limit` (at most 20). Out: `valid`; `candidates` (each `label`, `lon`, `lat`, `tier` (address, numberbase,
/// street), `matchKind`, `precisionEnumId`, `points`, `extent`, its parts, `gersId` and `release`, `sourceDatasets`,
/// `sourceLicenses`, and whether the postal code, locality and unit asked for agree), `ambiguous`, `parsed` (the parts read)
/// and `countries`; `reasonCode` NO_DATA (no active Addresses run), QUERY_UNREAD (no street in the text) or NO_MATCH.
pub(crate) fn search_overture_addresses(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<Out, Error> {
    let mut out = Out::new();
    out.insert("candidates".to_string(), Value::List(Vec::new()));
    out.insert("ambiguous".to_string(), Value::Bool(false));
    let refuse = |mut out: Out, code: &str, reason: &str| {
        out.insert("valid".to_string(), Value::Bool(false));
        out.insert("reasonCode".to_string(), Value::string(code));
        out.insert("reason".to_string(), Value::string(reason));
        Ok(out)
    };
    // the index is public reference data of this installation, read as the service's own step
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let runs = active_runs(facade)?;
    if runs.is_empty() {
        return refuse(out, "NO_DATA", "no region's Overture addresses are active");
    }
    let asked = text(parameters, "country").to_ascii_uppercase();
    let mut countries: BTreeSet<String> = BTreeSet::new();
    if asked.len() == 2 {
        countries.insert(asked);
    } else {
        countries.extend(runs.iter().flat_map(|run| run.countries.iter().cloned()));
    }
    let countries: Vec<String> = countries.into_iter().take(MAX_COUNTRIES).collect();
    out.insert(
        "countries".to_string(),
        Value::List(countries.iter().map(Value::string).collect()),
    );
    let imports: Vec<Value> = runs.iter().map(|run| Value::string(&run.import_id)).collect();
    let releases: HashMap<String, String> = runs
        .iter()
        .map(|run| (run.import_id.clone(), run.release.clone()))
        .collect();
    let limit = text(parameters, "limit").parse::<usize>().unwrap_or(10).clamp(1, 20);
    let near = middle(parameters);
    let q = text(parameters, "query");
    let mut budget = MAX_QUERIES;
    let mut best: Option<(Tier, Ranked, Query)> = None;
    let mut read_any: Option<Query> = None;
    for country in &countries {
        let Some(query) = address::parse(&q, country) else {
            continue;
        };
        read_any.get_or_insert_with(|| query.clone());
        if let Some((tier, ranked)) = search_country(&mut budget, facade, &imports, &releases, &query, country, near)? {
            let better = match &best {
                None => true,
                Some((best_tier, best_ranked, _)) => {
                    tier_order(tier) < tier_order(*best_tier)
                        || tier == *best_tier
                            && ranked.candidates.first().map_or(i32::MIN, |c| c.score)
                                > best_ranked.candidates.first().map_or(i32::MIN, |c| c.score)
                }
            };
            if better {
                best = Some((tier, ranked, query));
            }
        }
    }
    let Some(parsed) = best.as_ref().map(|(_, _, query)| query.clone()).or(read_any) else {
        return refuse(out, "QUERY_UNREAD", "the text has no street to look for");
    };
    out.insert("parsed".to_string(), parsed_value(&parsed));
    let Some((_, ranked, query)) = best else {
        return refuse(out, "NO_MATCH", "no address point of the active regions matches");
    };
    out.insert("ambiguous".to_string(), Value::Bool(ranked.ambiguous));
    out.insert(
        "candidates".to_string(),
        Value::List(
            ranked
                .candidates
                .iter()
                .take(limit)
                .map(|candidate| candidate_value(candidate, &query))
                .collect(),
        ),
    );
    out.insert("valid".to_string(), Value::Bool(true));
    Ok(out)
}

#[cfg(test)]
#[path = "address_service_tests.rs"]
mod tests;
