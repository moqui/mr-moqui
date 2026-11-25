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

//! Validating and loading a Geo data file (plans/Maps.md §6.3, §6.4, steps 3 to 5).
//!
//! - **Validate** ([`prepare`]): the whole file read with `moqui_lib::geo`, writing nothing: each row's country (the
//!   run's, or one the Geo table has), its code's shape and computed IDs (at most 40 characters), its coordinates, and
//!   duplicates; a code's rows grouped, its representative chosen (highest accuracy, the first in the file among
//!   equals).
//! - **Load** ([`Loader::load_batch`]): a batch of items in one transaction, create-or-update by computed ID with each
//!   batch's rows read first, a row that has not changed left alone. A batch that fails is loaded again an item at a
//!   time, so one bad item is rejected and the rest go in.
//! - **Retire** ([`Loader::retire`]), only after a whole load: the codes, place names and points this source key owns
//!   in the run's scope that the new edition no longer has are end-dated (`thruDate`), never deleted.
//!
//! An import never changes a row another source key owns (a `GeoPostalCode` of another `sourceKey`, or empty: made by
//! hand), a point of a user's confirmation (`GPP_USER_CONFIRMED`), a record's own location (`GPT_RECORD_LOCATION`),
//! or a Geo whose ID is not of a form it computes (Mantle's `USA_CA`: a state found by lookup is linked to, not
//! renamed). Its own point is the Geo's ID and its source key, so where two sources have a code each writes its own
//! point, and the `priority` of its data set (lower first) makes the active one: in the US the Census point.

use std::collections::{BTreeSet, HashMap, HashSet};

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::geo::census::ZctaCountyRecord;
use moqui_lib::geo::geonames::{Admin1Record, Admin2Record, CityRecord, CountryRecord};
use moqui_lib::geo::postal::PostalRecord;
use moqui_lib::geo::{self, ids, Format, Granularity, Point, PostalCode, Precision, Record, RowError, RowKey};
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;

pub const GEO: &str = "moqui.basic.Geo";
pub const GEO_ASSOC: &str = "moqui.basic.GeoAssoc";
pub const GEO_POINT: &str = "moqui.basic.GeoPoint";
pub const POSTAL_CODE: &str = "moqui.basic.GeoPostalCode";
pub const POSTAL_PLACE: &str = "moqui.basic.GeoPostalCodePlace";
pub const POINT_ASSOC: &str = "moqui.basic.GeoPointAssoc";

const REGIONS: &str = "GAT_REGIONS";
const USER_CONFIRMED: &str = "GPP_USER_CONFIRMED";
const RECORD_LOCATION: &str = "GPT_RECORD_LOCATION";
/// The values of an `In` condition per query.
pub(crate) const IN_CHUNK: usize = 500;
/// The countries whose GeoNames first-level codes are ISO 3166-2 (download.geonames.org/export/dump/readme.txt); the
/// others' are FIPS, matched by name, never written to an ISO field.
const ISO_ADMIN1: [&str; 4] = ["US", "CH", "BE", "ME"];

/// The data set a run loads.
#[derive(Clone, Debug)]
pub struct DataSet {
    pub data_set_id: String,
    pub source_key: String,
    pub data_source_id: Option<String>,
    pub priority: i64,
    pub format: Format,
    /// Every data set with this source key (a mirror, a later year): the points they wrote are this source's.
    pub owned_set_ids: Vec<String>,
}

/// A country Geo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Country {
    pub geo_id: String,
    pub alpha2: String,
    pub alpha3: String,
}

/// A run's counts, by code (or other record): read is the file's rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub read: u64,
    pub created: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub retired: u64,
    pub rejected: u64,
}

/// What loading one item came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Created,
    Updated,
    Unchanged,
}

impl Change {
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Created, _) | (_, Self::Created) => Self::Created,
            (Self::Updated, _) | (_, Self::Updated) => Self::Updated,
            _ => Self::Unchanged,
        }
    }
}

/// A postal code's rows from the file.
#[derive(Clone, Debug)]
pub struct CodeGroup {
    pub geo_id: String,
    pub country_geo_id: String,
    pub code: PostalCode,
    pub point_id: String,
    pub records: Vec<PostalRecord>,
    pub representative: usize,
}

/// One thing a batch loads.
#[derive(Clone, Debug)]
pub enum Item {
    Code(CodeGroup),
    /// A Census county (`county` true) or place.
    Division {
        geo_id: String,
        point_id: String,
        county: bool,
        record: geo::census::DivisionRecord,
    },
    ZctaCounty {
        county_geo_id: String,
        postal_geo_id: String,
        line: u64,
    },
    Country(CountryRecord),
    Admin1(Admin1Record),
    Admin2(Admin2Record),
    City {
        geo_id: String,
        point_id: String,
        record: CityRecord,
    },
}

impl Item {
    fn line(&self) -> u64 {
        match self {
            Self::Code(group) => group.records.first().map_or(0, |record| record.line),
            Self::Division { record, .. } => record.line,
            Self::ZctaCounty { line, .. } => *line,
            Self::Country(record) => record.line,
            Self::Admin1(record) => record.line,
            Self::Admin2(record) => record.line,
            Self::City { record, .. } => record.line,
        }
    }
}

/// A file read and checked: the items to load, the rows passed over, and what the edition has (for retiring).
#[derive(Debug, Default)]
pub struct Prepared {
    pub items: Vec<Item>,
    pub rejected: Rejections,
    pub rows: u64,
    /// The Geo IDs the file has, with those of rows passed over for another fault whose code (county, place, city)
    /// could still be read ([`RowKey`]): a code is retired only when the new edition does not have it at all.
    pub seen: HashSet<String>,
    /// The (code Geo ID, place name) pairs the file has.
    pub seen_places: HashSet<(String, String)>,
    /// The country Geos the file has.
    pub countries: BTreeSet<String>,
}

/// Read the file at `path` (the zip member `member`, or the file) as `format`.
///
/// # Errors
///
/// The file as a whole does not read: not the member, a missing header column, a decompression limit.
pub fn read_file(
    path: &std::path::Path,
    member: Option<&str>,
    limits: &geo::text::Limits,
    format: Format,
    heartbeat: &mut dyn FnMut(),
) -> Result<FileRead, LibError> {
    let mut read = FileRead::default();
    let mut seen = 0_u64;
    let rows = geo::text::read_file(path, member, limits, |reader| {
        geo::parse(format, reader, &mut |row| {
            seen += 1;
            if seen.is_multiple_of(HEARTBEAT_ROWS) {
                heartbeat();
            }
            match row {
                Ok(record) => read.records.push(record),
                Err(mut error) => {
                    if let Some(key) = error.key.take() {
                        read.keys.push(*key);
                    }
                    read.rejected.push(error);
                }
            }
        })
    })?;
    read.rows = rows;
    Ok(read)
}

/// How often, in rows, a long read calls its heartbeat.
const HEARTBEAT_ROWS: u64 = 50_000;

/// A file read: its records, its rejected rows, what those rows were about where they could be read, and its rows.
#[derive(Debug, Default)]
pub struct FileRead {
    pub records: Vec<Record>,
    pub rejected: Rejections,
    pub keys: Vec<RowKey>,
    pub rows: u64,
}

/// The rejected rows of a run: every one counted, the first [`super::MAX_KEPT_REJECTIONS`] kept for the error file.
#[derive(Debug, Default)]
pub struct Rejections {
    pub kept: Vec<RowError>,
    pub total: u64,
}

impl Rejections {
    pub fn push(&mut self, error: RowError) {
        self.total += 1;
        if self.kept.len() < super::MAX_KEPT_REJECTIONS {
            self.kept.push(error);
        }
    }

    pub fn extend(&mut self, errors: impl IntoIterator<Item = RowError>) {
        for error in errors {
            self.push(error);
        }
    }
}

/// The country Geo of an ISO alpha-2 or alpha-3 code, as the framework.
fn find_country(facade: &EntityFacade, code: &str) -> Result<Option<Country>, LibError> {
    let code = code.trim().to_ascii_uppercase();
    let field = if code.len() == 2 {
        "geoCodeAlpha2"
    } else {
        "geoCodeAlpha3"
    };
    let found = facade
        .find(GEO)?
        .eq("geoTypeEnumId", Value::string("GEOT_COUNTRY"))
        .eq(field, Value::string(&code))
        .list()?;
    let country = found.iter().next().map(country_of);
    Ok(country)
}

fn country_of(row: &EntityValue) -> Country {
    Country {
        geo_id: text_of(row, "geoId"),
        alpha2: text_of(row, "geoCodeAlpha2"),
        alpha3: text_of(row, "geoCodeAlpha3"),
    }
}

/// The country Geo of `geo_id`, when it is one.
///
/// # Errors
///
/// A database error.
pub fn country_by_id(facade: &EntityFacade, geo_id: &str) -> Result<Option<Country>, LibError> {
    let row = facade.find(GEO)?.eq("geoId", Value::string(geo_id)).one()?;
    Ok(row
        .filter(|row| text_of(row, "geoTypeEnumId") == "GEOT_COUNTRY")
        .map(|row| country_of(&row)))
}

fn text_of(row: &EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// Check the records of a file, writing nothing. `country` is the run's: a row of another country is passed over.
///
/// # Errors
///
/// A database error while looking a country up.
pub fn prepare(
    facade: &EntityFacade,
    set: &DataSet,
    country: Option<&Country>,
    read: FileRead,
) -> Result<Prepared, LibError> {
    let FileRead {
        records,
        rejected,
        keys,
        rows,
    } = read;
    let mut prepared = Prepared {
        rejected,
        rows,
        ..Prepared::default()
    };
    let mut countries: HashMap<String, Option<Country>> = HashMap::new();
    if let Some(country) = country {
        countries.insert(country.alpha2.clone(), Some(country.clone()));
        countries.insert(country.alpha3.clone(), Some(country.clone()));
    }
    let mut lookup = |code: &str| -> Result<Option<Country>, LibError> {
        let code = code.to_ascii_uppercase();
        if let Some(found) = countries.get(&code) {
            return Ok(found.clone());
        }
        let found = find_country(facade, &code)?;
        countries.insert(code, found.clone());
        Ok(found)
    };
    // a row passed over whose code could be read keeps that code in the edition
    for key in keys {
        let geo_id = match key {
            RowKey::Postal {
                country: code_country,
                code,
            } => match lookup(&code_country)? {
                Some(found) if country.is_none_or(|run| run.geo_id == found.geo_id) => {
                    ids::postal_code_geo_id(&found.geo_id, &code).ok()
                }
                _ => None,
            },
            RowKey::Fips(geoid) if set.format == Format::CensusCounties => ids::county_geo_id(&geoid).ok(),
            RowKey::Fips(geoid) if set.format == Format::CensusPlaces => ids::place_geo_id(&geoid).ok(),
            RowKey::Fips(_) => None,
            RowKey::City {
                country: city_country,
                geonameid,
            } => match lookup(&city_country)? {
                Some(found) => ids::city_geo_id(&found.geo_id, &geonameid).ok(),
                None => None,
            },
        };
        if let Some(geo_id) = geo_id {
            prepared.seen.insert(geo_id);
        }
    }
    let mut groups: IndexMap<String, CodeGroup> = IndexMap::new();
    let mut zcta_counties: Vec<ZctaCountyRecord> = Vec::new();
    let mut lines: HashMap<String, u64> = HashMap::new();
    for record in records {
        match record {
            Record::Postal(record) => {
                let row_country = match lookup(&record.country)? {
                    Some(found) if country.is_none_or(|run| run.geo_id == found.geo_id) => found,
                    Some(found) => {
                        prepared.rejected.push(row_error(
                            record.line,
                            "country",
                            format!(
                                "the row's country {} is not the run's ({})",
                                found.geo_id,
                                country.map_or("", |run| run.geo_id.as_str())
                            ),
                        ));
                        continue;
                    }
                    None => {
                        prepared.rejected.push(row_error(
                            record.line,
                            "country",
                            format!("there is no country Geo for {}", record.country),
                        ));
                        continue;
                    }
                };
                add_postal(&mut prepared, &mut groups, set, &row_country, record);
            }
            Record::County(record) | Record::Place(record) => {
                let county = set.format == Format::CensusCounties;
                let ids = if county {
                    ids::county_geo_id(&record.geoid)
                } else {
                    ids::place_geo_id(&record.geoid)
                }
                .and_then(|geo_id| ids::point_id(&geo_id, &set.source_key).map(|point_id| (geo_id, point_id)));
                let (geo_id, point_id) = match ids {
                    Ok(ids) => ids,
                    Err(message) => {
                        prepared.rejected.push(row_error(record.line, "GEOID", message));
                        continue;
                    }
                };
                if !prepared.seen.insert(geo_id.clone()) {
                    prepared
                        .rejected
                        .push(duplicate(record.line, "GEOID", lines.get(&geo_id)));
                    continue;
                }
                lines.insert(geo_id.clone(), record.line);
                prepared.countries.insert("USA".to_string());
                prepared.items.push(Item::Division {
                    geo_id,
                    point_id,
                    county,
                    record,
                });
            }
            Record::ZctaCounty(record) => zcta_counties.push(record),
            Record::Country(record) => {
                if !prepared.seen.insert(record.iso2.clone()) {
                    prepared
                        .rejected
                        .push(duplicate(record.line, "ISO", lines.get(&record.iso2)));
                    continue;
                }
                lines.insert(record.iso2.clone(), record.line);
                prepared.items.push(Item::Country(record));
            }
            Record::Admin1(record) => {
                let key = format!("{}.{}", record.country, record.code);
                if !prepared.seen.insert(key.clone()) {
                    prepared.rejected.push(duplicate(record.line, "code", lines.get(&key)));
                    continue;
                }
                lines.insert(key, record.line);
                prepared.items.push(Item::Admin1(record));
            }
            Record::Admin2(record) => {
                let key = format!("{}.{}.{}", record.country, record.admin1, record.code);
                if !prepared.seen.insert(key.clone()) {
                    prepared.rejected.push(duplicate(record.line, "code", lines.get(&key)));
                    continue;
                }
                lines.insert(key, record.line);
                prepared.items.push(Item::Admin2(record));
            }
            Record::City(record) => {
                let Some(row_country) = lookup(&record.country)? else {
                    prepared.rejected.push(row_error(
                        record.line,
                        "country code",
                        format!("there is no country Geo for {}", record.country),
                    ));
                    continue;
                };
                let ids = ids::city_geo_id(&row_country.geo_id, &record.geonameid)
                    .and_then(|geo_id| ids::point_id(&geo_id, &set.source_key).map(|point_id| (geo_id, point_id)));
                let (geo_id, point_id) = match ids {
                    Ok(ids) => ids,
                    Err(message) => {
                        prepared.rejected.push(row_error(record.line, "geonameid", message));
                        continue;
                    }
                };
                if !prepared.seen.insert(geo_id.clone()) {
                    prepared
                        .rejected
                        .push(duplicate(record.line, "geonameid", lines.get(&geo_id)));
                    continue;
                }
                lines.insert(geo_id.clone(), record.line);
                prepared.countries.insert(row_country.geo_id.clone());
                prepared.items.push(Item::City {
                    geo_id,
                    point_id,
                    record,
                });
            }
        }
    }
    for (_, mut group) in groups {
        let accuracies: Vec<Option<i32>> = group.records.iter().map(|record| record.accuracy).collect();
        group.representative = geo::representative_index(&accuracies).unwrap_or(0);
        // a place with a point of its own needs that point's ID within 40 characters; one at the representative's
        // point shares the code's point and needs none
        let representative = group.records.get(group.representative).and_then(|record| record.point);
        let mut kept = Vec::with_capacity(group.records.len());
        let mut kept_representative = 0;
        for (index, record) in std::mem::take(&mut group.records).into_iter().enumerate() {
            let sequence = u32::try_from(kept.len() + 1).unwrap_or(u32::MAX);
            let own_point = index != group.representative && record.point.is_some() && record.point != representative;
            if own_point && record.place_name.is_some() {
                if let Err(message) = ids::place_point_id(&group.geo_id, &set.source_key, sequence) {
                    if let Some(place) = &record.place_name {
                        prepared.seen_places.remove(&(group.geo_id.clone(), place.clone()));
                    }
                    prepared.rejected.push(row_error(record.line, "place", message));
                    continue;
                }
            }
            if index == group.representative {
                kept_representative = kept.len();
            }
            kept.push(record);
        }
        group.records = kept;
        group.representative = kept_representative;
        prepared.items.push(Item::Code(group));
    }
    for record in geo::census::largest_share(&zcta_counties) {
        let code = PostalCode {
            normalized: record.zcta.clone(),
            display: record.zcta.clone(),
            granularity: Granularity::Full,
        };
        let ids = ids::postal_code_geo_id("USA", &code)
            .and_then(|postal| ids::county_geo_id(&record.county).map(|county| (postal, county)));
        match ids {
            Ok((postal_geo_id, county_geo_id)) => {
                prepared.seen.insert(postal_geo_id.clone());
                prepared.items.push(Item::ZctaCounty {
                    county_geo_id,
                    postal_geo_id,
                    line: record.line,
                });
            }
            Err(message) => prepared
                .rejected
                .push(row_error(record.line, "GEOID_ZCTA5_20", message)),
        }
    }
    prepared.rejected.kept.sort_by_key(|error| error.line);
    Ok(prepared)
}

fn row_error(line: u64, field: &str, message: String) -> RowError {
    RowError::new(line, field, message, "")
}

fn duplicate(line: u64, field: &str, first: Option<&u64>) -> RowError {
    row_error(
        line,
        field,
        match first {
            Some(first) => format!("a duplicate of line {first}"),
            None => "a duplicate".to_string(),
        },
    )
}

/// Add a postal row to its code's group: a second row with no place name, or the same place name, is a duplicate.
fn add_postal(
    prepared: &mut Prepared,
    groups: &mut IndexMap<String, CodeGroup>,
    set: &DataSet,
    country: &Country,
    record: PostalRecord,
) {
    let geo_id = match ids::postal_code_geo_id(&country.geo_id, &record.code) {
        Ok(geo_id) => geo_id,
        Err(message) => {
            prepared.rejected.push(row_error(record.line, "code", message));
            return;
        }
    };
    prepared.seen.insert(geo_id.clone());
    prepared.countries.insert(country.geo_id.clone());
    let point_id = match ids::point_id(&geo_id, &set.source_key) {
        Ok(point_id) => point_id,
        Err(message) => {
            prepared.rejected.push(row_error(record.line, "code", message));
            return;
        }
    };
    let group = groups.entry(geo_id.clone()).or_insert_with(|| CodeGroup {
        geo_id: geo_id.clone(),
        country_geo_id: country.geo_id.clone(),
        code: record.code.clone(),
        point_id,
        records: Vec::new(),
        representative: 0,
    });
    let place = record.place_name.clone().unwrap_or_default();
    if let Some(first) = group
        .records
        .iter()
        .find(|kept| kept.place_name.clone().unwrap_or_default() == place)
    {
        prepared.rejected.push(row_error(
            record.line,
            if place.is_empty() { "code" } else { "place" },
            format!("a duplicate of line {}", first.line),
        ));
        return;
    }
    if !place.is_empty() {
        prepared.seen_places.insert((geo_id, place));
    }
    group.records.push(record);
}

/// The rows of `entity` whose `field` is one of `values`, as the framework, read a chunk at a time; `extra` narrows
/// each query.
pub(crate) fn rows_in(
    facade: &EntityFacade,
    entity: &str,
    field: &str,
    values: &[String],
    extra: &[(&str, &str)],
) -> Result<Vec<EntityValue>, LibError> {
    let mut found = Vec::new();
    for chunk in values.chunks(IN_CHUNK) {
        if chunk.is_empty() {
            continue;
        }
        let mut find = facade
            .find(entity)?
            .condition(Condition::In(field.into(), chunk.iter().map(Value::string).collect()));
        for (name, value) in extra {
            find = find.eq(name, Value::string(*value));
        }
        found.extend(find.list()?.into_values());
    }
    Ok(found)
}

fn keyed(rows: Vec<EntityValue>, field: &str) -> HashMap<String, EntityValue> {
    rows.into_iter().map(|row| (text_of(&row, field), row)).collect()
}

/// Whether the stored `old` is the value `new` would write.
fn same(old: Option<&Value>, new: &Value) -> bool {
    let old_text = old.map(to_plain_string).unwrap_or_default();
    match new {
        Value::Null => old_text.is_empty(),
        Value::F64(number) => old_text.parse::<f64>().is_ok_and(|old| (old - number).abs() < 1e-9),
        Value::I64(number) => old_text.parse::<f64>().is_ok_and(|old| old == *number as f64),
        _ => old_text == to_plain_string(new),
    }
}

/// The fields a [`write`] sets.
struct Fields<'a> {
    /// The primary key.
    pk: &'a [(&'a str, Value)],
    /// Compared with the row; a difference updates it.
    compared: &'a [(&'a str, Value)],
    /// Set only when the row is made.
    on_create: &'a [(&'a str, Value)],
    /// Set when the row is made or updated, not compared (the edition that last wrote it).
    on_write: &'a [(&'a str, Value)],
}

/// Create the row of `entity`, or update `existing` where `fields.compared` differs from it.
fn write(
    facade: &EntityFacade,
    entity: &str,
    existing: Option<&EntityValue>,
    fields: &Fields<'_>,
) -> Result<Change, LibError> {
    match existing {
        None => {
            let mut row = facade.make_value(entity)?;
            for (name, value) in fields
                .pk
                .iter()
                .chain(fields.compared)
                .chain(fields.on_create)
                .chain(fields.on_write)
            {
                if !matches!(value, Value::Null) {
                    row.set(*name, value.clone());
                }
            }
            row.create(facade)?;
            Ok(Change::Created)
        }
        Some(row) => {
            let changed: Vec<&(&str, Value)> = fields
                .compared
                .iter()
                .filter(|(name, value)| !same(row.get(name), value))
                .collect();
            if changed.is_empty() {
                return Ok(Change::Unchanged);
            }
            let mut row = row.clone();
            for (name, value) in changed.into_iter().chain(fields.on_write) {
                row.set(*name, value.clone());
            }
            row.update(facade)?;
            Ok(Change::Updated)
        }
    }
}

/// A child of a Geo by `GAT_REGIONS`.
#[derive(Clone, Debug)]
struct Child {
    geo_id: String,
    alpha2: String,
    numeric: String,
    name: String,
}

/// The rows a batch reads before it writes.
#[derive(Default)]
struct Existing {
    geos: HashMap<String, EntityValue>,
    codes: HashMap<String, EntityValue>,
    /// (country, normalized code, granularity) to the Geo of a code already there.
    code_keys: HashMap<(String, String, String), String>,
    points: HashMap<String, EntityValue>,
    assocs: HashMap<String, EntityValue>,
    places: HashMap<(String, String), EntityValue>,
    /// GeoAssoc rows of GAT_REGIONS, (geoId, toGeoId).
    regions: HashSet<(String, String)>,
}

/// A run's loading: what it writes and its lookups, kept across batches.
pub struct Loader<'a> {
    pub facade: &'a EntityFacade,
    pub set: &'a DataSet,
    pub country: Option<&'a Country>,
    pub edition: String,
    pub now: Value,
    /// The GAT_REGIONS children of a Geo, read once.
    children: HashMap<String, Vec<Child>>,
    /// Country Geos by ISO alpha-2.
    countries: HashMap<String, Option<Country>>,
}

/// What a batch came to: its counts and the items it passed over.
#[derive(Debug, Default)]
pub struct BatchResult {
    pub created: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub rejected: Vec<RowError>,
}

impl<'a> Loader<'a> {
    #[must_use]
    pub fn new(facade: &'a EntityFacade, set: &'a DataSet, country: Option<&'a Country>, edition: String) -> Self {
        Self {
            facade,
            set,
            country,
            edition,
            now: Value::Timestamp(jiff::Timestamp::now()),
            children: HashMap::new(),
            countries: HashMap::new(),
        }
    }

    /// Load `items` in one transaction; when it fails, an item at a time, each failure a rejected row.
    pub fn load_batch(&mut self, items: &[Item]) -> BatchResult {
        match self.transaction(items) {
            Ok(result) => result,
            Err(err) => {
                tracing::warn!(error = %err, data_set = %self.set.data_set_id, "A Geo data batch failed; loading it an item at a time");
                // a lookup made in the rolled-back transaction is gone
                self.children.clear();
                let mut result = BatchResult::default();
                for item in items {
                    match self.transaction(std::slice::from_ref(item)) {
                        Ok(one) => {
                            result.created += one.created;
                            result.updated += one.updated;
                            result.unchanged += one.unchanged;
                            result.rejected.extend(one.rejected);
                        }
                        Err(err) => {
                            self.children.clear();
                            result
                                .rejected
                                .push(row_error(item.line(), "", err.message().to_string()));
                        }
                    }
                }
                result
            }
        }
    }

    fn transaction(&mut self, items: &[Item]) -> Result<BatchResult, LibError> {
        let facade = self.facade;
        let tx = facade.transactions_for(GEO)?;
        tx.run(TransactionAttribute::ForceNew, None, || {
            let existing = self.read_existing(items)?;
            let mut result = BatchResult::default();
            for item in items {
                match self.load_item(item, &existing)? {
                    Ok(Change::Created) => result.created += 1,
                    Ok(Change::Updated) => result.updated += 1,
                    Ok(Change::Unchanged) => result.unchanged += 1,
                    Err(error) => result.rejected.push(error),
                }
            }
            Ok(result)
        })
    }

    fn read_existing(&self, items: &[Item]) -> Result<Existing, LibError> {
        let facade = self.facade;
        let mut geo_ids: Vec<String> = Vec::new();
        let mut point_ids: Vec<String> = Vec::new();
        let mut code_ids: Vec<String> = Vec::new();
        let mut normalized: HashMap<String, Vec<String>> = HashMap::new();
        for item in items {
            match item {
                Item::Code(group) => {
                    geo_ids.push(group.geo_id.clone());
                    code_ids.push(group.geo_id.clone());
                    normalized
                        .entry(group.country_geo_id.clone())
                        .or_default()
                        .push(group.code.normalized.clone());
                    point_ids.push(group.point_id.clone());
                    for (index, record) in group.records.iter().enumerate() {
                        if record.place_name.is_some() {
                            let sequence = u32::try_from(index + 1).unwrap_or(u32::MAX);
                            if let Ok(own) = ids::place_point_id(&group.geo_id, &self.set.source_key, sequence) {
                                point_ids.push(own);
                            }
                        }
                    }
                }
                Item::Division { geo_id, point_id, .. } | Item::City { geo_id, point_id, .. } => {
                    geo_ids.push(geo_id.clone());
                    point_ids.push(point_id.clone());
                }
                Item::ZctaCounty {
                    county_geo_id,
                    postal_geo_id,
                    ..
                } => {
                    geo_ids.push(county_geo_id.clone());
                    geo_ids.push(postal_geo_id.clone());
                }
                Item::Country(record) => geo_ids.push(record.iso3.clone()),
                Item::Admin1(_) | Item::Admin2(_) => {}
            }
        }
        let mut existing = Existing {
            geos: keyed(rows_in(facade, GEO, "geoId", &geo_ids, &[])?, "geoId"),
            points: keyed(rows_in(facade, GEO_POINT, "geoPointId", &point_ids, &[])?, "geoPointId"),
            assocs: keyed(
                rows_in(facade, POINT_ASSOC, "geoPointId", &point_ids, &[])?,
                "geoPointId",
            ),
            ..Existing::default()
        };
        if !code_ids.is_empty() {
            existing.codes = keyed(rows_in(facade, POSTAL_CODE, "geoId", &code_ids, &[])?, "geoId");
            for (country, codes) in &normalized {
                for row in rows_in(
                    facade,
                    POSTAL_CODE,
                    "normalizedCode",
                    codes,
                    &[("countryGeoId", country.as_str())],
                )? {
                    existing.code_keys.insert(
                        (
                            country.clone(),
                            text_of(&row, "normalizedCode"),
                            text_of(&row, "granularityEnumId"),
                        ),
                        text_of(&row, "geoId"),
                    );
                }
            }
            for row in rows_in(
                facade,
                POSTAL_PLACE,
                "geoId",
                &code_ids,
                &[("sourceKey", self.set.source_key.as_str())],
            )? {
                existing
                    .places
                    .insert((text_of(&row, "geoId"), text_of(&row, "placeName")), row);
            }
        }
        if self.set.format == Format::CensusZctaCounty {
            let postal: Vec<String> = items
                .iter()
                .filter_map(|item| match item {
                    Item::ZctaCounty { postal_geo_id, .. } => Some(postal_geo_id.clone()),
                    _ => None,
                })
                .collect();
            for row in rows_in(
                facade,
                GEO_ASSOC,
                "toGeoId",
                &postal,
                &[("geoAssocTypeEnumId", REGIONS)],
            )? {
                existing
                    .regions
                    .insert((text_of(&row, "geoId"), text_of(&row, "toGeoId")));
            }
        }
        Ok(existing)
    }

    /// One item: its change, or its rejection (`Ok(Err(..))`); `Err` is a database error, which fails the batch.
    fn load_item(&mut self, item: &Item, existing: &Existing) -> Result<Result<Change, RowError>, LibError> {
        match item {
            Item::Code(group) => self.load_code(group, existing),
            Item::Division {
                geo_id,
                point_id,
                county,
                record,
            } => {
                let geo_type = if *county { "GEOT_COUNTY" } else { "GEOT_CITY" };
                let change = match self.write_geo(
                    existing,
                    geo_id,
                    geo_type,
                    &[
                        ("geoName", Value::string(&record.name)),
                        ("geoCodeNumeric", Value::string(&record.geoid)),
                    ],
                    record.line,
                )? {
                    Ok(change) => change,
                    Err(error) => return Ok(Err(error)),
                };
                let state = self.admin1("USA", "US", &record.state, &[], Some(&record.state))?;
                let mut change = change;
                if let Some(state) = state {
                    change = change.and(self.link(&state, geo_id, existing)?);
                }
                match self.write_point(existing, geo_id, point_id, Some(record.point), Precision::PlaceEstimate)? {
                    Ok(point) => Ok(Ok(change.and(point))),
                    Err(message) => Ok(Err(row_error(record.line, "", message))),
                }
            }
            Item::ZctaCounty {
                county_geo_id,
                postal_geo_id,
                line,
            } => {
                for geo_id in [county_geo_id, postal_geo_id] {
                    if !existing.geos.contains_key(geo_id) {
                        return Ok(Err(row_error(
                            *line,
                            "",
                            format!("Geo {geo_id} is not there: import its Census data set first"),
                        )));
                    }
                }
                let mut change = Change::Unchanged;
                // a ZCTA is in one county: another county's link, the importer's own, goes
                for (from, to) in &existing.regions {
                    if to == postal_geo_id && from != county_geo_id && from.starts_with("USA_C_") {
                        self.facade
                            .find(GEO_ASSOC)?
                            .eq("geoId", Value::string(from))
                            .eq("toGeoId", Value::string(to))
                            .list()?
                            .iter()
                            .try_for_each(|row| row.delete(self.facade))?;
                        change = Change::Updated;
                    }
                }
                if !existing
                    .regions
                    .contains(&(county_geo_id.clone(), postal_geo_id.clone()))
                {
                    self.create_region(county_geo_id, postal_geo_id)?;
                    change = if change == Change::Updated {
                        Change::Updated
                    } else {
                        Change::Created
                    };
                }
                Ok(Ok(change))
            }
            Item::Country(record) => self.load_country(record, existing),
            Item::Admin1(record) => self.load_admin1(record),
            Item::Admin2(record) => self.load_admin2(record),
            Item::City {
                geo_id,
                point_id,
                record,
            } => {
                let Some(country) = self.country_of_alpha2(&record.country)? else {
                    return Ok(Err(row_error(
                        record.line,
                        "country code",
                        format!("there is no country Geo for {}", record.country),
                    )));
                };
                let change = match self.write_geo(
                    existing,
                    geo_id,
                    "GEOT_CITY",
                    &[("geoName", Value::string(&record.name))],
                    record.line,
                )? {
                    Ok(change) => change,
                    Err(error) => return Ok(Err(error)),
                };
                let mut change = change;
                if let Some(admin1) = &record.admin1 {
                    if let Some(parent) = self.admin1(&country.geo_id, &country.alpha2, admin1, &[], None)? {
                        change = change.and(self.link(&parent, geo_id, existing)?);
                    }
                }
                match self.write_point(existing, geo_id, point_id, Some(record.point), Precision::PlaceEstimate)? {
                    Ok(point) => Ok(Ok(change.and(point))),
                    Err(message) => Ok(Err(row_error(record.line, "", message))),
                }
            }
        }
    }

    /// Create or update the importer's own Geo `geo_id` of `geo_type`; refused when a Geo of that ID is of another
    /// type (another record's).
    fn write_geo(
        &self,
        existing: &Existing,
        geo_id: &str,
        geo_type: &str,
        compared: &[(&str, Value)],
        line: u64,
    ) -> Result<Result<Change, RowError>, LibError> {
        let found = existing.geos.get(geo_id);
        if let Some(row) = found {
            let kind = text_of(row, "geoTypeEnumId");
            if kind != geo_type {
                return Ok(Err(row_error(
                    line,
                    "",
                    format!("Geo {geo_id} is there as {kind}, not {geo_type}: another record's, left as it is"),
                )));
            }
        }
        let mut fields = vec![("geoTypeEnumId", Value::string(geo_type))];
        fields.extend(compared.iter().cloned());
        Ok(Ok(write(
            self.facade,
            GEO,
            found,
            &Fields {
                pk: &[("geoId", Value::string(geo_id))],
                compared: &fields,
                on_create: &[],
                on_write: &[],
            },
        )?))
    }

    /// The source's point `point_id` of `geo_id` and its association. A point of a user's confirmation is left as it
    /// is (unchanged); a record's own location under that ID is refused.
    fn write_point(
        &self,
        existing: &Existing,
        geo_id: &str,
        point_id: &str,
        point: Option<Point>,
        precision: Precision,
    ) -> Result<Result<Change, String>, LibError> {
        let Some(point) = point else {
            return Ok(Ok(Change::Unchanged));
        };
        let assoc = existing.assocs.get(point_id);
        if assoc.is_some_and(|row| text_of(row, "precisionEnumId") == USER_CONFIRMED) {
            return Ok(Ok(Change::Unchanged));
        }
        let change = match self.write_bare_point(existing, point_id, point)? {
            Ok(change) => change,
            Err(message) => return Ok(Err(message)),
        };
        let association = write(
            self.facade,
            POINT_ASSOC,
            assoc.filter(|row| text_of(row, "geoId") == geo_id),
            &Fields {
                pk: &[
                    ("geoId", Value::string(geo_id)),
                    ("geoPointId", Value::string(point_id)),
                ],
                compared: &[
                    ("pointRoleEnumId", Value::string("GPR_REPRESENTATIVE")),
                    ("precisionEnumId", Value::string(precision.enum_id())),
                    ("dataSetId", Value::string(&self.set.data_set_id)),
                    ("priority", Value::I64(self.set.priority)),
                    ("thruDate", Value::Null),
                ],
                on_create: &[("fromDate", self.now.clone())],
                on_write: &[("edition", Value::string(&self.edition))],
            },
        )?;
        Ok(Ok(change.and(association)))
    }

    /// A GeoPoint of this source, with no association (a place name's own point, or a code's before its association).
    fn write_bare_point(
        &self,
        existing: &Existing,
        point_id: &str,
        point: Point,
    ) -> Result<Result<Change, String>, LibError> {
        let found = existing.points.get(point_id);
        if found.is_some_and(|row| text_of(row, "geoPointTypeEnumId") == RECORD_LOCATION) {
            return Ok(Err(format!(
                "point {point_id} is a record's own location, left as it is"
            )));
        }
        Ok(Ok(write(
            self.facade,
            GEO_POINT,
            found,
            &Fields {
                pk: &[("geoPointId", Value::string(point_id))],
                compared: &[
                    ("geoPointTypeEnumId", Value::string("GPT_GEO_REFERENCE")),
                    (
                        "dataSourceId",
                        self.set.data_source_id.clone().map_or(Value::Null, Value::String),
                    ),
                    ("latitude", Value::F64(point.latitude)),
                    ("longitude", Value::F64(point.longitude)),
                ],
                on_create: &[],
                on_write: &[],
            },
        )?))
    }

    fn load_code(&mut self, group: &CodeGroup, existing: &Existing) -> Result<Result<Change, RowError>, LibError> {
        let line = group.records.first().map_or(0, |record| record.line);
        let geo_id = group.geo_id.as_str();
        let geo = existing.geos.get(geo_id);
        if let Some(row) = geo {
            let kind = text_of(row, "geoTypeEnumId");
            if kind != "GEOT_POSTAL_CODE" {
                return Ok(Err(row_error(
                    line,
                    "code",
                    format!("Geo {geo_id} is there as {kind}, not a postal code: another record's, left as it is"),
                )));
            }
        }
        let key = (
            group.country_geo_id.clone(),
            group.code.normalized.clone(),
            group.code.granularity.enum_id().to_string(),
        );
        if let Some(other) = existing.code_keys.get(&key).filter(|other| other.as_str() != geo_id) {
            return Ok(Err(row_error(
                line,
                "code",
                format!("the code is Geo {other} already, made by hand: left as it is"),
            )));
        }
        let owns = match (geo, existing.codes.get(geo_id)) {
            (None, _) => true,
            (Some(_), Some(code)) => text_of(code, "sourceKey") == self.set.source_key,
            (Some(_), None) => {
                return Ok(Err(row_error(
                    line,
                    "code",
                    format!("Geo {geo_id} has no GeoPostalCode row: made by hand, left as it is"),
                )))
            }
        };
        let mut change = Change::Unchanged;
        if owns {
            change = write(
                self.facade,
                GEO,
                geo,
                &Fields {
                    pk: &[("geoId", Value::string(geo_id))],
                    compared: &[
                        ("geoTypeEnumId", Value::string("GEOT_POSTAL_CODE")),
                        ("geoName", Value::string(&group.code.display)),
                    ],
                    on_create: &[],
                    on_write: &[],
                },
            )?;
            change = change.and(write(
                self.facade,
                POSTAL_CODE,
                existing.codes.get(geo_id),
                &Fields {
                    pk: &[("geoId", Value::string(geo_id))],
                    compared: &[
                        ("countryGeoId", Value::string(&group.country_geo_id)),
                        ("normalizedCode", Value::string(&group.code.normalized)),
                        ("displayCode", Value::string(&group.code.display)),
                        ("granularityEnumId", Value::string(group.code.granularity.enum_id())),
                        ("sourceKey", Value::string(&self.set.source_key)),
                        ("thruDate", Value::Null),
                    ],
                    on_create: &[],
                    on_write: &[],
                },
            )?);
        } else if let Some(code) = existing.codes.get(geo_id) {
            // another source's code, retired by its owner but in this edition: revived, its ownership kept (a code
            // made by hand, with no sourceKey, is left as it is)
            if !text_of(code, "sourceKey").is_empty() && !text_of(code, "thruDate").is_empty() {
                let mut code = code.clone();
                code.set("thruDate", Value::Null);
                code.update(self.facade)?;
                change = Change::Updated;
            }
        }
        let Some(representative) = group.records.get(group.representative) else {
            return Ok(Ok(change));
        };
        match self.write_point(
            existing,
            geo_id,
            &group.point_id,
            representative.point,
            representative.precision,
        )? {
            Ok(point) => change = change.and(point),
            Err(message) => return Ok(Err(row_error(line, "", message))),
        }
        for (index, record) in group.records.iter().enumerate() {
            let Some(place) = &record.place_name else {
                continue;
            };
            let sequence = u32::try_from(index + 1).unwrap_or(u32::MAX);
            let point_id = if index == group.representative || record.point == representative.point {
                representative.point.map(|_| group.point_id.clone())
            } else if let Some(point) = record.point {
                let own = ids::place_point_id(geo_id, &self.set.source_key, sequence).map_err(LibError::new)?;
                match self.write_bare_point(existing, &own, point)? {
                    Ok(point) => change = change.and(point),
                    Err(message) => return Ok(Err(row_error(record.line, "", message))),
                }
                Some(own)
            } else {
                None
            };
            change = change.and(write(
                self.facade,
                POSTAL_PLACE,
                existing.places.get(&(geo_id.to_string(), place.clone())),
                &Fields {
                    pk: &[
                        ("geoId", Value::string(geo_id)),
                        ("sourceKey", Value::string(&self.set.source_key)),
                        ("placeName", Value::string(place)),
                    ],
                    compared: &[
                        ("sequenceNum", Value::I64(i64::from(sequence))),
                        (
                            "adminName1",
                            record
                                .admin_name1
                                .clone()
                                .or_else(|| record.admin_code1.clone())
                                .map_or(Value::Null, Value::String),
                        ),
                        (
                            "adminName2",
                            record
                                .admin_name2
                                .clone()
                                .or_else(|| record.admin_code2.clone())
                                .map_or(Value::Null, Value::String),
                        ),
                        (
                            "accuracy",
                            record.accuracy.map_or(Value::Null, |a| Value::I64(i64::from(a))),
                        ),
                        ("geoPointId", point_id.map_or(Value::Null, Value::String)),
                        ("thruDate", Value::Null),
                    ],
                    on_create: &[],
                    on_write: &[],
                },
            )?);
        }
        Ok(Ok(change))
    }

    fn load_country(
        &mut self,
        record: &CountryRecord,
        existing: &Existing,
    ) -> Result<Result<Change, RowError>, LibError> {
        if self.country_of_alpha2(&record.iso2)?.is_some() {
            // another record's (the framework's country data): left as it is
            return Ok(Ok(Change::Unchanged));
        }
        if existing.geos.contains_key(&record.iso3) {
            return Ok(Err(row_error(
                record.line,
                "ISO3",
                format!("Geo {} is there and is not this country", record.iso3),
            )));
        }
        write(
            self.facade,
            GEO,
            None,
            &Fields {
                pk: &[("geoId", Value::string(&record.iso3))],
                compared: &[
                    ("geoTypeEnumId", Value::string("GEOT_COUNTRY")),
                    ("geoName", Value::string(&record.name)),
                    ("geoCodeAlpha2", Value::string(&record.iso2)),
                    ("geoCodeAlpha3", Value::string(&record.iso3)),
                    ("geoCodeNumeric", Value::string(&record.numeric)),
                ],
                on_create: &[],
                on_write: &[],
            },
        )?;
        self.countries.remove(&record.iso2);
        Ok(Ok(Change::Created))
    }

    fn load_admin1(&mut self, record: &Admin1Record) -> Result<Result<Change, RowError>, LibError> {
        let Some(country) = self.country_of_alpha2(&record.country)? else {
            return Ok(Err(row_error(
                record.line,
                "code",
                format!("there is no country Geo for {}", record.country),
            )));
        };
        let mut names = vec![record.name.as_str()];
        if let Some(ascii) = &record.ascii_name {
            names.push(ascii);
        }
        let before = self.children_of(&country.geo_id)?.len();
        let Some(found) = self.admin1(
            &country.geo_id,
            &country.alpha2,
            &record.code,
            &names,
            Some(&record.name),
        )?
        else {
            return Ok(Err(row_error(
                record.line,
                "code",
                "the first-level division could not be made".to_string(),
            )));
        };
        if self.children_of(&country.geo_id)?.len() > before {
            return Ok(Ok(Change::Created));
        }
        self.rename_computed(&country.geo_id, &found, &record.name)
    }

    fn load_admin2(&mut self, record: &Admin2Record) -> Result<Result<Change, RowError>, LibError> {
        let Some(country) = self.country_of_alpha2(&record.country)? else {
            return Ok(Err(row_error(
                record.line,
                "code",
                format!("there is no country Geo for {}", record.country),
            )));
        };
        let Some(parent) = self.admin1(&country.geo_id, &country.alpha2, &record.admin1, &[], None)? else {
            return Ok(Err(row_error(
                record.line,
                "code",
                format!(
                    "first-level division {} of {} is not there: import the first-level divisions first",
                    record.admin1, country.geo_id
                ),
            )));
        };
        let mut names = vec![record.name.as_str()];
        if let Some(ascii) = &record.ascii_name {
            names.push(ascii);
        }
        let made = ids::admin2_geo_id(&country.geo_id, &record.admin1, &record.code)
            .map_err(|message| row_error(record.line, "code", message));
        let made = match made {
            Ok(made) => made,
            Err(error) => return Ok(Err(error)),
        };
        let found = self
            .children_of(&parent)?
            .iter()
            .find(|child| {
                child.geo_id == made
                    || (!child.numeric.is_empty() && child.numeric == record.code)
                    || names.iter().any(|name| child.name.eq_ignore_ascii_case(name))
            })
            .map(|child| child.geo_id.clone());
        if let Some(found) = found {
            return self.rename_computed(&parent, &found, &record.name);
        }
        let geo = self.facade.find(GEO)?.eq("geoId", Value::string(&made)).one()?;
        if geo.is_some() {
            return Ok(Err(row_error(
                record.line,
                "code",
                format!("Geo {made} is there but not under {parent}"),
            )));
        }
        let mut row = self.facade.make_value(GEO)?;
        row.set("geoId", Value::string(&made));
        row.set("geoTypeEnumId", Value::string("GEOT_COUNTY"));
        row.set("geoName", Value::string(&record.name));
        row.create(self.facade)?;
        self.create_region(&parent, &made)?;
        self.children.remove(&parent);
        Ok(Ok(Change::Created))
    }

    /// Rename `geo_id`, a child of `parent`, to `name` when its ID is of a form the importer computes; another
    /// record's Geo is linked to, never renamed.
    fn rename_computed(
        &mut self,
        parent: &str,
        geo_id: &str,
        name: &str,
    ) -> Result<Result<Change, RowError>, LibError> {
        if !ids::is_computed(geo_id) {
            return Ok(Ok(Change::Unchanged));
        }
        let Some(row) = self.facade.find(GEO)?.eq("geoId", Value::string(geo_id)).one()? else {
            return Ok(Ok(Change::Unchanged));
        };
        let change = write(
            self.facade,
            GEO,
            Some(&row),
            &Fields {
                pk: &[("geoId", Value::string(geo_id))],
                compared: &[("geoName", Value::string(name))],
                on_create: &[],
                on_write: &[],
            },
        )?;
        if change != Change::Unchanged {
            self.children.remove(parent);
        }
        Ok(Ok(change))
    }

    /// The country Geo of ISO alpha-2 `code`.
    fn country_of_alpha2(&mut self, code: &str) -> Result<Option<Country>, LibError> {
        if let Some(found) = self.countries.get(code) {
            return Ok(found.clone());
        }
        let found = match self.country {
            Some(country) if country.alpha2 == code => Some(country.clone()),
            _ => find_country(self.facade, code)?,
        };
        self.countries.insert(code.to_string(), found.clone());
        Ok(found)
    }

    fn children_of(&mut self, parent: &str) -> Result<&Vec<Child>, LibError> {
        if !self.children.contains_key(parent) {
            let to_ids: Vec<String> = self
                .facade
                .find(GEO_ASSOC)?
                .eq("geoId", Value::string(parent))
                .eq("geoAssocTypeEnumId", Value::string(REGIONS))
                .list()?
                .iter()
                .map(|row| text_of(row, "toGeoId"))
                .collect();
            let children = rows_in(self.facade, GEO, "geoId", &to_ids, &[])?
                .iter()
                .map(|row| Child {
                    geo_id: text_of(row, "geoId"),
                    alpha2: text_of(row, "geoCodeAlpha2"),
                    numeric: text_of(row, "geoCodeNumeric"),
                    name: text_of(row, "geoName"),
                })
                .collect();
            self.children.insert(parent.to_string(), children);
        }
        Ok(self.children.entry(parent.to_string()).or_default())
    }

    /// The first-level division of country `country_geo_id` (ISO alpha-2 `alpha2`) for the source's `code`, found by
    /// lookup among the country's `GAT_REGIONS` children (plans/Maps.md §6.3): by the code in `geoCodeAlpha2` or
    /// `geoCodeNumeric` where the source's codes are ISO, by the ID the importer would have made, then by `names`.
    /// When nothing matches and `create_name` is given, `{country}_A1_{code}` is made with that name (and the code
    /// as `geoCodeAlpha2` where it is ISO) and linked to the country.
    fn admin1(
        &mut self,
        country_geo_id: &str,
        alpha2: &str,
        code: &str,
        names: &[&str],
        create_name: Option<&str>,
    ) -> Result<Option<String>, LibError> {
        let iso = ISO_ADMIN1.contains(&alpha2);
        let Ok(made) = ids::admin1_geo_id(country_geo_id, code) else {
            return Ok(None);
        };
        let children = self.children_of(country_geo_id)?;
        let found = children
            .iter()
            .find(|child| {
                (iso && (child.alpha2.eq_ignore_ascii_case(code) || child.numeric == code)) || child.geo_id == made
            })
            .or_else(|| {
                children
                    .iter()
                    .find(|child| names.iter().any(|name| child.name.eq_ignore_ascii_case(name)))
            })
            .map(|child| child.geo_id.clone());
        if found.is_some() {
            return Ok(found);
        }
        let Some(name) = create_name else {
            return Ok(None);
        };
        if self
            .facade
            .find(GEO)?
            .eq("geoId", Value::string(&made))
            .one()?
            .is_none()
        {
            let mut row = self.facade.make_value(GEO)?;
            row.set("geoId", Value::string(&made));
            row.set("geoTypeEnumId", Value::string("GEOT_STATE"));
            row.set("geoName", Value::string(name));
            if iso {
                row.set("geoCodeAlpha2", Value::string(code));
            }
            row.create(self.facade)?;
        }
        self.create_region(country_geo_id, &made)?;
        self.children.remove(country_geo_id);
        Ok(Some(made))
    }

    /// Link `child` under `parent` by `GAT_REGIONS`, when not already.
    fn link(&mut self, parent: &str, child: &str, existing: &Existing) -> Result<Change, LibError> {
        if existing.regions.contains(&(parent.to_string(), child.to_string()))
            || self.children_of(parent)?.iter().any(|known| known.geo_id == child)
        {
            return Ok(Change::Unchanged);
        }
        self.create_region(parent, child)?;
        if let Some(children) = self.children.get_mut(parent) {
            children.push(Child {
                geo_id: child.to_string(),
                alpha2: String::new(),
                numeric: String::new(),
                name: String::new(),
            });
        }
        Ok(Change::Updated)
    }

    fn create_region(&self, parent: &str, child: &str) -> Result<(), LibError> {
        let mut row = self.facade.make_value(GEO_ASSOC)?;
        row.set("geoId", Value::string(parent));
        row.set("toGeoId", Value::string(child));
        row.set("geoAssocTypeEnumId", Value::string(REGIONS));
        row.create_if_missing(self.facade)?;
        Ok(())
    }

    /// The Geo ID prefixes of the run's scope: its country's, or the countries the file has; none for a global set
    /// (every Geo of the source).
    fn scope(&self, prepared: &Prepared) -> Vec<String> {
        match self.country {
            Some(country) => vec![format!("{}_", country.geo_id)],
            None => prepared.countries.iter().map(|country| format!("{country}_")).collect(),
        }
    }

    /// After a whole load: end-date what this source key owns in the run's scope that the edition does not have, in
    /// one transaction. Returns the Geos retired (whose code, points or place names were end-dated).
    ///
    /// # Errors
    ///
    /// A database error: nothing is retired.
    pub fn retire(&self, prepared: &Prepared) -> Result<u64, LibError> {
        if matches!(
            self.set.format,
            Format::CensusZctaCounty | Format::GeoNamesCountries | Format::GeoNamesAdmin1 | Format::GeoNamesAdmin2
        ) {
            return Ok(0);
        }
        let scope = self.scope(prepared);
        let in_scope = |geo_id: &str| scope.is_empty() || scope.iter().any(|prefix| geo_id.starts_with(prefix));
        // each query narrowed to the scope's Geo IDs (LIKE's _ matches any character: in_scope checks exactly)
        let scoped = |find: moqui_core::entity::EntityFind| {
            if scope.is_empty() {
                find
            } else {
                find.condition(Condition::or(
                    scope
                        .iter()
                        .map(|prefix| Condition::Like("geoId".into(), Value::string(format!("{prefix}%"))))
                        .collect(),
                ))
            }
        };
        let countries: Vec<String> = match self.country {
            Some(country) => vec![country.geo_id.clone()],
            None => prepared.countries.iter().cloned().collect(),
        };
        let facade = self.facade;
        let tx = facade.transactions_for(GEO)?;
        tx.run(TransactionAttribute::ForceNew, None, || {
            let mut retired: HashSet<String> = HashSet::new();
            let mut assocs: Vec<EntityValue> = Vec::new();
            let owned: Vec<String> = self.set.owned_set_ids.clone();
            for chunk in owned.chunks(IN_CHUNK) {
                let find = facade
                    .find(POINT_ASSOC)?
                    .condition(Condition::In(
                        "dataSetId".into(),
                        chunk.iter().map(Value::string).collect(),
                    ))
                    .condition(Condition::is_null("thruDate"));
                assocs.extend(scoped(find).list()?.into_values());
            }
            if matches!(
                self.set.format,
                Format::CensusZcta | Format::GeoNamesPostal | Format::OperatorCsv
            ) && !countries.is_empty()
            {
                let codes = rows_in(
                    facade,
                    POSTAL_CODE,
                    "countryGeoId",
                    &countries,
                    &[("sourceKey", self.set.source_key.as_str())],
                )?;
                let gone: Vec<EntityValue> = codes
                    .into_iter()
                    .filter(|code| text_of(code, "thruDate").is_empty())
                    .filter(|code| {
                        let geo_id = text_of(code, "geoId");
                        !prepared.seen.contains(&geo_id) && in_scope(&geo_id)
                    })
                    .collect();
                // a code another source still has a point for stays in date
                let gone_ids: Vec<String> = gone.iter().map(|code| text_of(code, "geoId")).collect();
                let held: HashSet<String> = rows_in(facade, POINT_ASSOC, "geoId", &gone_ids, &[])?
                    .iter()
                    .filter(|row| text_of(row, "thruDate").is_empty())
                    .filter(|row| !self.set.owned_set_ids.contains(&text_of(row, "dataSetId")))
                    .map(|row| text_of(row, "geoId"))
                    .collect();
                for code in gone {
                    let geo_id = text_of(&code, "geoId");
                    if !held.contains(&geo_id) {
                        let mut code = code;
                        code.set("thruDate", self.now.clone());
                        code.update(facade)?;
                    }
                    retired.insert(geo_id);
                }
                let find = facade
                    .find(POSTAL_PLACE)?
                    .eq("sourceKey", Value::string(&self.set.source_key))
                    .condition(Condition::is_null("thruDate"));
                let places = scoped(find).list()?;
                for place in places.iter() {
                    let geo_id = text_of(place, "geoId");
                    let key = (geo_id.clone(), text_of(place, "placeName"));
                    if prepared.seen_places.contains(&key) || !in_scope(&geo_id) {
                        continue;
                    }
                    let mut place = place.clone();
                    place.set("thruDate", self.now.clone());
                    place.update(facade)?;
                    if !prepared.seen.contains(&geo_id) {
                        retired.insert(geo_id);
                    }
                }
            }
            for assoc in &assocs {
                let geo_id = text_of(assoc, "geoId");
                if prepared.seen.contains(&geo_id)
                    || !in_scope(&geo_id)
                    || text_of(assoc, "precisionEnumId") == USER_CONFIRMED
                {
                    continue;
                }
                let mut assoc = assoc.clone();
                assoc.set("thruDate", self.now.clone());
                assoc.update(facade)?;
                retired.insert(geo_id);
            }
            Ok(retired.len() as u64)
        })
    }
}
