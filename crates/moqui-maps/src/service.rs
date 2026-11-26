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

//! `org.moqui.maps.OvertureServices`' Rust services (plans/Maps-Overture.md §3): `read#OvertureReleases` lists the
//! catalog's releases, and `read#OvertureTheme` reads one run's theme for its region: the release's catalog (its schema
//! version checked), the items whose extent meets the region, and each file by range ([`crate::overture`]); Places and
//! Addresses are written as the run's `OverturePlace` and `OvertureAddress` rows in batches, each in a transaction of
//! its own (Addresses then compared with the run active before, [`crate::address_service`]), and Buildings are cut into
//! vector tiles and answered as a PMTiles archive for OvertureData.js to store. `search#OvertureAddresses`, the local
//! geocoder's search, is [`crate::address_service`]'s. The counts are written to the run's row
//! after each file and at most once a second within one, and its `cancelRequested` read then. Both are no group's and
//! not remote: OvertureData.js calls them after its own checks (the caller's own MAPS_ADMIN grant, the license, outside
//! requests), and they refuse again while `maps@external-requests` is false.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error;
use moqui_lib::geo::tiles::{build_tiles, project, Shape, TileFeature, TilerLimits, LAYER, MAX_ZOOM};
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::pmtiles::{write_archive, ArchiveOptions, Compression, TileType};

use crate::address_service::{self, AddressSet, Keys};
use crate::http::{fetch_json, HttpSource};
use crate::overture::{self, AddressPoint, Building, Counts, Place, Row, Theme};
use crate::parquet::Limits;
use crate::stac::{self, meets, Bbox};

type Out = BTreeMap<String, Value>;

pub const IMPORT: &str = "moqui.maps.OvertureImport";
pub const REGION: &str = "moqui.maps.OvertureRegion";
pub const PLACE: &str = "moqui.maps.OverturePlace";
pub const THEME: &str = "moqui.maps.OvertureTheme";
/// The catalog when `maps@overture-catalog` is empty.
pub const DEFAULT_CATALOG: &str = "https://stac.overturemaps.org/catalog.json";
/// Places written in one transaction.
const BATCH: usize = 2000;
/// The most rows a run keeps of a theme: a region of a few cities, not a country.
const MAX_PLACES: u64 = 200_000;
const MAX_BUILDINGS: u64 = 250_000;
/// Address points: the O2 evaluation's Boston Back Bay has about 8,800 a km² (a point per unit), so about 115 km² of it.
const MAX_ADDRESSES: u64 = 1_000_000;
/// The most items read one by one when a collection's extent does not match its items.
const MAX_ITEMS: usize = 1024;
const REPORT_EVERY: Duration = Duration::from_secs(1);

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, find_overture_releases)?;
    rust_service!(map, read_overture_theme)?;
    address_service::register(map)
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> String {
    parameters
        .get(key)
        .map(to_plain_string)
        .unwrap_or_default()
        .trim()
        .to_string()
}

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

fn conf(name: &str) -> String {
    ExecutionContext::with_current(|ec| ec.factory().conf_setting("maps", name)).unwrap_or_default()
}

fn refusal(mut out: Out, code: &str, reason: impl Into<String>) -> Result<Out, Error> {
    out.insert("valid".to_string(), Value::Bool(false));
    out.insert("reasonCode".to_string(), Value::string(code));
    out.insert("reason".to_string(), Value::string(reason.into()));
    Ok(out)
}

/// The service whose grant a run needs: the user it runs as holds MAPS_ADMIN's authorization of it themselves.
const START_SERVICE: &str = "org.moqui.maps.OvertureServices.start#OvertureImport";

/// Whether the current user holds the grant of [`START_SERVICE`] themselves, asked with authorization required (also
/// where the caller turned it off: OvertureData.js calls this service so, and a job's script may have). With no context
/// entered (a unit test) nothing is checked.
fn authorized() -> bool {
    if !ExecutionContext::is_entered() {
        return true;
    }
    let _required = ExecutionContext::with_current(ExecutionContext::require_authz);
    match moqui_core::user::authorize_own("AT_SERVICE", START_SERVICE, "all") {
        Ok(()) => true,
        Err(err) => {
            tracing::info!(error = %err, "Refused an Overture import");
            false
        }
    }
}

/// The catalog's root: `maps@overture-catalog`, else Overture's.
fn catalog_url() -> String {
    let configured = conf("overture-catalog");
    if configured.trim().is_empty() {
        DEFAULT_CATALOG.to_string()
    } else {
        configured.trim().to_string()
    }
}

fn outside_requests_off() -> bool {
    conf("external-requests").trim().eq_ignore_ascii_case("false")
}

/// A document of the catalog, which must be on the root's origin.
fn catalog_document(root: &str, url: &str) -> Result<serde_json::Value, Error> {
    if stac::origin(url) != stac::origin(root) {
        return Err(Error::new(format!("the catalog links to {url}, outside its own host")));
    }
    fetch_json(url)
}

/// `read#OvertureReleases`. Out: `catalogUrl`, `releases` (each `{version, latest}`, the newest first), and when the
/// catalog cannot be read `valid` false with `reasonCode` (EXTERNAL_REQUESTS_OFF, CATALOG_UNREAD) and `reason`.
pub(crate) fn find_overture_releases(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<Out, Error> {
    let mut out = Out::new();
    let root = catalog_url();
    out.insert("catalogUrl".to_string(), Value::string(&root));
    if !root.starts_with("https://") {
        return refusal(out, "CATALOG_NOT_HTTPS", "maps@overture-catalog is not an https URL");
    }
    if outside_requests_off() {
        return refusal(
            out,
            "EXTERNAL_REQUESTS_OFF",
            "outside requests for maps are off (maps@external-requests)",
        );
    }
    let document = match fetch_json(&root) {
        Ok(document) => document,
        Err(err) => return refusal(out, "CATALOG_UNREAD", err.message().to_string()),
    };
    let releases: Vec<Value> = stac::releases(&root, &document)
        .into_iter()
        .map(|release| {
            let mut map = moqui_lib::object::ValueMap::default();
            map.insert("version".to_string(), Value::string(release.version));
            map.insert("latest".to_string(), Value::Bool(release.latest));
            Value::Map(map)
        })
        .collect();
    out.insert("valid".to_string(), Value::Bool(true));
    out.insert("releases".to_string(), Value::List(releases));
    Ok(out)
}

/// The files of `theme` in `release` that meet `region`: the release's catalog read (and its schema version checked),
/// the theme's collection, and the items whose extent meets the region, each read for its file.
struct Discovered {
    schema_version: String,
    files: Vec<String>,
}

fn discover(
    root: &str,
    release: &str,
    theme: Theme,
    region: &Bbox,
) -> Result<Result<Discovered, (&'static str, String)>, Error> {
    let document = catalog_document(root, root)?;
    let Some(entry) = stac::releases(root, &document)
        .into_iter()
        .find(|entry| entry.version == release)
    else {
        return Ok(Err((
            "RELEASE_NOT_FOUND",
            format!("the catalog lists no release {release}"),
        )));
    };
    let release_document = catalog_document(root, &entry.href)?;
    let catalog = stac::release(&entry.href, &release_document).map_err(Error::new)?;
    if !stac::schema_supported(&catalog.schema_version) {
        return Ok(Err((
            "SCHEMA_UNSUPPORTED",
            format!(
                "release {release} is of Overture schema {}, and this server reads schema 2",
                catalog.schema_version
            ),
        )));
    }
    let (theme_name, kind) = theme.overture();
    let Some((_, theme_href)) = catalog.themes.iter().find(|(name, _)| name == theme_name) else {
        return Ok(Err((
            "THEME_NOT_FOUND",
            format!("release {release} has no {theme_name} theme"),
        )));
    };
    let theme_document = catalog_document(root, theme_href)?;
    let Some(collection_href) = stac::theme(theme_href, &theme_document, kind) else {
        return Ok(Err((
            "THEME_NOT_FOUND",
            format!("release {release} has no {kind} collection"),
        )));
    };
    let collection = stac::collection(&collection_href, &catalog_document(root, &collection_href)?);
    if let Some(version) = collection
        .schema_version
        .as_deref()
        .filter(|version| !stac::schema_supported(version))
    {
        return Ok(Err((
            "SCHEMA_UNSUPPORTED",
            format!("the {kind} collection of release {release} is of Overture schema {version}, and this server reads schema 2"),
        )));
    }
    let extent_given = collection.items.iter().all(|item| item.bbox.is_some());
    // every item is read when the extent does not say each one's box: refused past MAX_ITEMS, never cut short
    let too_many = || {
        Err((
            "TOO_MANY_ITEMS",
            format!(
                "the {kind} collection of release {release} has {} items without their boxes in its extent, more than {MAX_ITEMS} this server reads one by one",
                collection.items.len()
            ),
        ))
    };
    if !extent_given && collection.items.len() > MAX_ITEMS {
        return Ok(too_many());
    }
    let candidates: Vec<&stac::ItemRef> = if extent_given {
        collection
            .items
            .iter()
            .filter(|item| item.bbox.is_some_and(|bbox| meets(&bbox, region)))
            .collect()
    } else {
        collection.items.iter().collect()
    };
    let mut files = Vec::new();
    let mut mismatch = false;
    for candidate in &candidates {
        let item = stac::item(&candidate.href, &catalog_document(root, &candidate.href)?).map_err(Error::new)?;
        if let Some(extent) = candidate.bbox {
            if extent.iter().zip(item.bbox.iter()).any(|(a, b)| (a - b).abs() > 1e-6) {
                mismatch = true;
            }
        }
        if meets(&item.bbox, region) {
            files.push(item.href);
        }
    }
    if mismatch && extent_given {
        // the extent's order is not the items': every item is read for its own box
        if collection.items.len() > MAX_ITEMS {
            return Ok(too_many());
        }
        files.clear();
        for candidate in &collection.items {
            let item = stac::item(&candidate.href, &catalog_document(root, &candidate.href)?).map_err(Error::new)?;
            if meets(&item.bbox, region) {
                files.push(item.href);
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(Ok(Discovered {
        schema_version: catalog.schema_version,
        files,
    }))
}

/// Write the counts onto run `import_id` in a transaction of its own and answer whether to go on (its
/// `cancelRequested` is not Y). A row that cannot be written is logged and the run goes on.
fn report(facade: &EntityFacade, import_id: &str, counts: &Counts) -> bool {
    let mut cancelled = false;
    let written = facade.transactions_for(IMPORT).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            let Some(mut row) = facade.find(IMPORT)?.eq("importId", Value::string(import_id)).one()? else {
                return Ok(());
            };
            cancelled = field(&row, "cancelRequested") == "Y";
            for (name, value) in counts_fields(counts) {
                row.set(name, value);
            }
            row.set("lastProgressDate", Value::Timestamp(jiff::Timestamp::now()));
            row.update(facade)
        })
    });
    if let Err(err) = written {
        tracing::error!(error = %err, import = %import_id, "Could not write an Overture import's progress");
    }
    !cancelled
}

fn as_i64(value: u64) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

fn counts_fields(counts: &Counts) -> Vec<(&'static str, Value)> {
    vec![
        ("filesRead", as_i64(counts.files)),
        ("rowGroupsRead", as_i64(counts.row_groups_read)),
        ("rowGroupsSkipped", as_i64(counts.row_groups_skipped)),
        ("bytesRead", as_i64(counts.bytes)),
        ("rowsRead", as_i64(counts.rows_read)),
        ("rowsKept", as_i64(counts.rows_kept)),
        ("rowsRejected", as_i64(counts.rows_rejected)),
    ]
}

fn place_fields(place: &Place, import_id: &str, index: u64) -> Vec<(&'static str, Value)> {
    let opt = |value: &Option<String>, max: usize| match value {
        Some(text) if !text.trim().is_empty() => Value::string(cut(text.trim(), max)),
        _ => Value::Null,
    };
    let name = cut(place.name.trim(), 255);
    vec![
        ("placeId", Value::string(format!("{import_id}-{index}"))),
        ("importId", Value::string(import_id)),
        ("gersId", Value::string(cut(&place.gers_id, 63))),
        ("featureVersion", place.version.map_or(Value::Null, Value::I64)),
        ("nameSearch", Value::string(name.to_lowercase())),
        ("placeName", Value::string(name)),
        ("basicCategory", opt(&place.basic_category, 63)),
        ("taxonomyPrimary", opt(&place.taxonomy, 63)),
        ("latitude", Value::F64(place.lat)),
        ("longitude", Value::F64(place.lon)),
        ("confidence", place.confidence.map_or(Value::Null, Value::F64)),
        ("operatingStatus", opt(&place.operating_status, 63)),
        ("brandName", opt(&place.brand, 255)),
        ("addressText", opt(&place.address.freeform, 255)),
        ("locality", opt(&place.address.locality, 255)),
        ("region", opt(&place.address.region, 63)),
        ("postcode", opt(&place.address.postcode, 63)),
        ("countryCode", opt(&place.address.country, 63)),
        ("sourceDatasets", Value::string(cut(&place.datasets.join(", "), 255))),
        ("sourceLicenses", Value::string(cut(&place.licenses.join(", "), 255))),
    ]
}

/// `text` cut to at most `max` bytes on a character boundary.
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

fn write_places(facade: &EntityFacade, import_id: &str, batch: &[(u64, Place)]) -> Result<(), Error> {
    if batch.is_empty() {
        return Ok(());
    }
    facade
        .transactions_for(PLACE)?
        .run(TransactionAttribute::ForceNew, None, || {
            for (index, place) in batch {
                let mut row = facade.make_value(PLACE)?;
                for (name, value) in place_fields(place, import_id, *index) {
                    row.set(name, value);
                }
                row.create(facade)?;
            }
            Ok(())
        })
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    // writing into a Vec does not fail
    let _ = encoder.write_all(bytes);
    encoder.finish().unwrap_or_default()
}

/// A building as a feature to tile: its footprint, its name or class as the label, and what is known of it.
fn building_feature(building: &Building, id: u64) -> Option<TileFeature> {
    let polygons: Vec<Vec<Vec<[f64; 2]>>> = building
        .polygons
        .iter()
        .map(|polygon| {
            polygon
                .iter()
                .map(|ring| ring.iter().map(|[lon, lat]| project(*lon, *lat)).collect())
                .collect()
        })
        .collect();
    let label = building
        .name
        .clone()
        .or_else(|| building.class.clone())
        .unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    if let Some(class) = building.class.as_ref().or(building.subtype.as_ref()) {
        parts.push(class.replace('_', " "));
    }
    if let Some(height) = building.height {
        parts.push(format!("{height:.0} m"));
    }
    if let Some(floors) = building.num_floors {
        parts.push(format!("{floors} floors"));
    }
    if !building.datasets.is_empty() {
        parts.push(building.datasets.join(", "));
    }
    TileFeature::new(
        id,
        Shape::Polygons(polygons),
        cut(&label, 120),
        cut(&parts.join(" · "), 200),
    )
}

/// `read#OvertureTheme`. In: `importId` (a run whose region, theme and release are set). Out: `valid`; when not,
/// `reasonCode` (EXTERNAL_REQUESTS_OFF, RUN_NOT_FOUND, REGION_INVALID, THEME_UNKNOWN, RELEASE_NOT_FOUND,
/// SCHEMA_UNSUPPORTED, THEME_NOT_FOUND, NO_FILES, READ_FAILED, TOO_MANY_ROWS, NO_TILES, TOO_LARGE_TO_TILE, CANCELLED with
/// `cancelled` true) and `reason`; always `schemaVersion` once known and the counts (`filesRead`, `rowGroupsRead`,
/// `rowGroupsSkipped`, `bytesRead`, `rowsRead`, `rowsKept`, `rowsRejected`), `licenses` (each `{dataset, license,
/// rows}`) and `rejections` (the first reasons). Places are written as rows of the run; Buildings answer `bytes` (the
/// archive), `size`, `tileCount`, `maxZoom`, `bounds` and `stoppedReason`.
#[allow(clippy::too_many_lines)]
pub(crate) fn read_overture_theme(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Result<Out, Error> {
    let out = Out::new();
    if outside_requests_off() {
        return refusal(
            out,
            "EXTERNAL_REQUESTS_OFF",
            "outside requests for maps are off (maps@external-requests)",
        );
    }
    if !authorized() {
        return refusal(
            out,
            "PERMISSION_DENIED",
            "Overture imports need the user's own Maps administration grant (MAPS_ADMIN)",
        );
    }
    let import_id = text(parameters, "importId");
    let (run, region_row, attribution) = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let run = facade.find(IMPORT)?.eq("importId", Value::string(&import_id)).one()?;
        let region_row = match &run {
            Some(run) => facade
                .find(REGION)?
                .eq("regionId", Value::string(field(run, "regionId")))
                .one()?,
            None => None,
        };
        // the theme's attribution, its sources' included, carried into an archive's metadata
        let attribution = match &run {
            Some(run) => facade
                .find(THEME)?
                .eq("themeId", Value::string(field(run, "themeId")))
                .one()?
                .map(|theme| field(&theme, "attribution"))
                .unwrap_or_default(),
            None => String::new(),
        };
        (run, region_row, attribution)
    };
    let (Some(run), Some(region_row)) = (run, region_row) else {
        return refusal(
            out,
            "RUN_NOT_FOUND",
            format!("there is no Overture import {import_id} with its region"),
        );
    };
    let Some(theme) = Theme::parse(&field(&run, "themeId")) else {
        return refusal(
            out,
            "THEME_UNKNOWN",
            format!("{} is not a theme this server imports", field(&run, "themeId")),
        );
    };
    let region = match (
        number(&region_row, "west"),
        number(&region_row, "south"),
        number(&region_row, "east"),
        number(&region_row, "north"),
    ) {
        (Some(west), Some(south), Some(east), Some(north)) if west < east && south < north => {
            [west, south, east, north]
        }
        _ => {
            return refusal(
                out,
                "REGION_INVALID",
                "the region's box is not west < east and south < north",
            )
        }
    };
    let release = field(&run, "releaseVersion");
    let root = catalog_url();
    if !root.starts_with("https://") {
        return refusal(out, "CATALOG_NOT_HTTPS", "maps@overture-catalog is not an https URL");
    }
    let discovered = match discover(&root, &release, theme, &region) {
        Ok(Ok(discovered)) => discovered,
        Ok(Err((code, reason))) => return refusal(out, code, reason),
        Err(err) => return refusal(out, "READ_FAILED", err.message().to_string()),
    };
    let mut out = out;
    out.insert("schemaVersion".to_string(), Value::string(&discovered.schema_version));
    if discovered.files.is_empty() {
        return refusal(
            out,
            "NO_FILES",
            format!(
                "release {release} has no {} file that meets the region",
                theme.overture().0
            ),
        );
    }
    let limits = Limits::standard();
    let mut counts = Counts::default();
    let mut places: Vec<(u64, Place)> = Vec::with_capacity(BATCH);
    let mut addresses: Vec<(u64, AddressPoint, Keys)> = Vec::new();
    let mut address_set = AddressSet::default();
    let mut countries: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut buildings: Vec<TileFeature> = Vec::new();
    let mut bounds: Bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut next = 0u64;
    let mut cancelled = false;
    let mut too_many = false;
    let mut last_report = Instant::now();
    let max_rows = match theme {
        Theme::Places => MAX_PLACES,
        Theme::Buildings => MAX_BUILDINGS,
        Theme::Addresses => MAX_ADDRESSES,
    };
    for url in &discovered.files {
        let source = match HttpSource::open(url) {
            Ok(source) => source,
            Err(err) => return finish_failed(out, &counts, "READ_FAILED", err.message()),
        };
        let file = match overture::open(&source, theme, &limits) {
            Ok(file) => file,
            Err(err) => {
                counts.bytes += source.bytes_read();
                return finish_failed(
                    out,
                    &counts,
                    "READ_FAILED",
                    &format!("{}: {}", moqui_core::outbound::redacted(url), err.message()),
                );
            }
        };
        let mut file_counts = Counts {
            files: 1,
            ..Counts::default()
        };
        let mut write_error: Option<Error> = None;
        let read = {
            let mut sink = |row: Row| -> Result<bool, Error> {
                next += 1;
                if next > max_rows {
                    too_many = true;
                    return Ok(false);
                }
                match row {
                    Row::Place(place) => {
                        bounds = [
                            bounds[0].min(place.lon),
                            bounds[1].min(place.lat),
                            bounds[2].max(place.lon),
                            bounds[3].max(place.lat),
                        ];
                        places.push((next, place));
                        if places.len() >= BATCH {
                            if let Err(err) = write_places(facade, &import_id, &places) {
                                write_error = Some(err);
                                return Ok(false);
                            }
                            places.clear();
                        }
                    }
                    Row::Building(building) => {
                        for [lon, lat] in building.polygons.iter().flatten().flatten() {
                            bounds = [
                                bounds[0].min(*lon),
                                bounds[1].min(*lat),
                                bounds[2].max(*lon),
                                bounds[3].max(*lat),
                            ];
                        }
                        if let Some(feature) = building_feature(&building, next) {
                            buildings.push(feature);
                        }
                    }
                    Row::Address(point) => {
                        bounds = [
                            bounds[0].min(point.lon),
                            bounds[1].min(point.lat),
                            bounds[2].max(point.lon),
                            bounds[3].max(point.lat),
                        ];
                        let keys = Keys::of(&point);
                        address_set.add(keys.digest(&point.country), point.lon, point.lat);
                        if !countries.contains(&point.country) {
                            countries.insert(point.country.clone());
                        }
                        addresses.push((next, point, keys));
                        if addresses.len() >= BATCH {
                            if let Err(err) = address_service::write_addresses(facade, &import_id, &addresses) {
                                write_error = Some(err);
                                return Ok(false);
                            }
                            addresses.clear();
                        }
                    }
                }
                Ok(true)
            };
            let base = counts.clone();
            let mut go_on = |so_far: &Counts| {
                if last_report.elapsed() < REPORT_EVERY {
                    return true;
                }
                last_report = Instant::now();
                let mut now = base.clone();
                now.add(so_far.clone());
                now.bytes += source.bytes_read();
                let go = report(facade, &import_id, &now);
                cancelled = !go;
                go
            };
            overture::read_region(
                &source,
                &file,
                theme,
                &region,
                &limits,
                &mut file_counts,
                &mut sink,
                &mut go_on,
            )
        };
        file_counts.bytes += source.bytes_read();
        counts.add(file_counts);
        match read {
            Ok(true) => {}
            Ok(false) => cancelled = true,
            Err(err) => {
                return finish_failed(
                    out,
                    &counts,
                    "READ_FAILED",
                    &format!("{}: {}", moqui_core::outbound::redacted(url), err.message()),
                )
            }
        }
        if let Some(err) = write_error {
            return finish_failed(out, &counts, "WRITE_FAILED", err.message());
        }
        if too_many {
            return finish_failed(
                out,
                &counts,
                "TOO_MANY_ROWS",
                &format!(
                    "the region has more than {max_rows} {}: choose a smaller one",
                    theme.overture().0
                ),
            );
        }
        if cancelled {
            report(facade, &import_id, &counts);
            out.insert("cancelled".to_string(), Value::Bool(true));
            return finish_failed(out, &counts, "CANCELLED", "the import was cancelled");
        }
        if !report(facade, &import_id, &counts) {
            out.insert("cancelled".to_string(), Value::Bool(true));
            return finish_failed(out, &counts, "CANCELLED", "the import was cancelled");
        }
    }
    if let Err(err) = write_places(facade, &import_id, &places) {
        return finish_failed(out, &counts, "WRITE_FAILED", err.message());
    }
    drop(places);
    if let Err(err) = address_service::write_addresses(facade, &import_id, &addresses) {
        return finish_failed(out, &counts, "WRITE_FAILED", err.message());
    }
    drop(addresses);
    let mut out = with_counts(out, &counts);
    if theme == Theme::Addresses {
        out.insert(
            "countryCodes".to_string(),
            Value::string(countries.iter().cloned().collect::<Vec<_>>().join(",")),
        );
        // what the release changed against the run active now, by address: a failure to compare is logged and the
        // run goes on, the comparison being a report, not a check
        let compared = {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            address_service::active_run(facade, &field(&region_row, "regionId")).and_then(|active| match active {
                Some(active) if active != import_id => address_service::read_set(facade, &active)
                    .map(|old| Some(address_service::compare(&address_set, &old, &active))),
                _ => Ok(None),
            })
        };
        match compared {
            Ok(Some(changes)) => {
                out.insert("changes".to_string(), address_service::changes_value(&changes));
            }
            Ok(None) => {}
            Err(err) => {
                tracing::error!(error = %err, import = %import_id, "Could not compare an Overture address run with the one active")
            }
        }
        drop(address_set);
    }
    if theme == Theme::Buildings {
        if buildings.is_empty() {
            return refusal(out, "NO_TILES", "no building of the release is in the region");
        }
        // tiling reports as reading does, so the run is not taken for abandoned, and reads its cancel
        let mut last_tile_report = Instant::now();
        let mut go_on = |_: moqui_lib::geo::tiles::Progress| {
            if last_tile_report.elapsed() < REPORT_EVERY {
                return true;
            }
            last_tile_report = Instant::now();
            report(facade, &import_id, &counts)
        };
        let set = match build_tiles(&buildings, &TilerLimits::standard(), &gzip, &mut go_on) {
            Ok(Some(set)) => set,
            Ok(None) => {
                out.insert("cancelled".to_string(), Value::Bool(true));
                return refusal(out, "CANCELLED", "the import was cancelled while its tiles were made");
            }
            Err(err) => return refusal(out, "TOO_LARGE_TO_TILE", err.message().to_string()),
        };
        let metadata = serde_json::json!({
            "name": "Overture buildings",
            "type": "overlay",
            "version": release,
            "attribution": if attribution.is_empty() { "© OpenStreetMap contributors, Overture Maps Foundation".to_string() } else { attribution.clone() },
            "description": "Building footprints of an Overture release for one region (ODbL)",
            "vector_layers": [{"id": LAYER, "minzoom": 0, "maxzoom": set.max_zoom,
                "fields": {"label": "String", "description": "String"}}]
        });
        let options = ArchiveOptions {
            tile_type: TileType::Mvt,
            tile_compression: Compression::Gzip,
            internal_compression: Compression::Gzip,
            min_zoom: 0,
            max_zoom: set.max_zoom.min(MAX_ZOOM),
            bounds,
            center_zoom: set.max_zoom.min(14),
        };
        let tile_count = set.tiles.len();
        let max_zoom = set.max_zoom;
        let stopped = set.stopped.clone();
        let archive = write_archive(set.tiles, metadata.to_string().as_bytes(), &options, &gzip)?;
        out.insert("size".to_string(), as_i64(archive.len() as u64));
        out.insert("bytes".to_string(), Value::Bytes(archive));
        out.insert("tileCount".to_string(), as_i64(tile_count as u64));
        out.insert("maxZoom".to_string(), Value::I64(i64::from(max_zoom)));
        out.insert("stoppedReason".to_string(), stopped.map_or(Value::Null, Value::string));
    }
    if bounds[0].is_finite() {
        out.insert(
            "bounds".to_string(),
            Value::List(bounds.iter().map(|value| Value::F64(*value)).collect()),
        );
    }
    out.insert("valid".to_string(), Value::Bool(true));
    Ok(out)
}

fn with_counts(mut out: Out, counts: &Counts) -> Out {
    for (name, value) in counts_fields(counts) {
        out.insert(name.to_string(), value);
    }
    let licenses: Vec<Value> = counts
        .licenses
        .iter()
        .map(|((dataset, license), rows)| {
            let mut map = moqui_lib::object::ValueMap::default();
            map.insert("dataset".to_string(), Value::string(dataset));
            map.insert("license".to_string(), Value::string(license));
            map.insert("rows".to_string(), as_i64(*rows));
            Value::Map(map)
        })
        .collect();
    out.insert("licenses".to_string(), Value::List(licenses));
    out.insert(
        "rejections".to_string(),
        Value::List(counts.rejections.iter().cloned().map(Value::string).collect()),
    );
    out
}

fn finish_failed(out: Out, counts: &Counts, code: &str, reason: &str) -> Result<Out, Error> {
    refusal(with_counts(out, counts), code, reason.to_string())
}
