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

//! `org.moqui.maps.MapSourceServices.convert#Overlay` (plans/Maps.md §4.1, §8): a geographic file of one of the formats
//! [`moqui_lib::geo::overlay`] reads (GeoJSON, TopoJSON, EsriJSON, GPX, KML, GML, WKT, WKB, delimited text, a zipped
//! shapefile), at a resource location or given as text or bytes, read and checked there and answered as a sanitized
//! GeoJSON FeatureCollection in WGS84 with its feature count, bounds, CRS and notes. It writes nothing. It reads any
//! location it is given, so no group is granted it: the overlay services (MapOverlays.js) call it after their own
//! check of the caller's drive access.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::geo::overlay::{convert_overlay_with, OverlayFormat, OverlayLimits};
use moqui_lib::geo::proj::Registry;
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;

use crate::group_db::{flag, text, Out};

/// The largest overlay when the caller names none, and the window a file is read in.
const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
const READ_WINDOW: usize = 1024 * 1024;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, convert_overlay_service)?;
    rust_service!(map, check_projection_definition)?;
    Ok(())
}

/// The registry of projections an overlay may be converted from (`moqui.maps.MapProjection`, P4c): each row read with
/// its PROJ.4 definition; a row that does not read here (a method or grid not read) is logged by its code and left out,
/// so a file in it is refused by name, and the others are used.
pub(crate) fn projection_registry(facade: &EntityFacade) -> Registry {
    let mut registry = Registry::empty();
    let rows = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        facade
            .find("moqui.maps.MapProjection")
            .and_then(|find| find.limit(2000).list())
    });
    let rows = match rows {
        Some(Ok(rows)) => rows,
        Some(Err(err)) => {
            tracing::error!("Map projections not read: {}", err.message());
            return registry;
        }
        None => return registry,
    };
    for row in rows.iter() {
        let field = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
        if field("disabled") == "Y" {
            continue;
        }
        let code = field("projectionId");
        // WGS84 and Web Mercator are every reader's own
        if code == "EPSG:4326" || code == "EPSG:3857" {
            continue;
        }
        if let Err(err) = registry.add(&code, &field("label"), &field("proj4Definition"), &field("axisOrder")) {
            tracing::warn!("Map projection {code} is not used on the server: {err}");
        }
    }
    registry
}

/// `check#ProjectionDefinition` (P4c): whether the server reads a map projection's PROJ.4 `definition` for converting
/// overlays and a COG's bounds (`moqui-lib` `geo/proj.rs`). Out: `readable`, `reason` (why not, in words), `transform`
/// (what converting from it does, its provenance) and `geographic`. A definition the server does not read may still be
/// one a browser draws (proj4js reads every method); the caller says so.
pub(crate) fn check_projection_definition(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = Out::new();
    match moqui_lib::geo::proj::Projection::parse_proj4(&text(parameters, "definition")) {
        Ok(projection) => {
            out.insert("readable".to_string(), Value::Bool(true));
            out.insert("transform".to_string(), Value::string(projection.describe()));
            out.insert("geographic".to_string(), Value::Bool(projection.is_geographic()));
        }
        Err(reason) => {
            out.insert("readable".to_string(), Value::Bool(false));
            out.insert("reason".to_string(), Value::string(reason));
        }
    }
    Ok(out)
}

fn number(parameters: &BTreeMap<String, Value>, key: &str) -> Option<u64> {
    text(parameters, key).trim().parse::<u64>().ok()
}

fn refusal(mut out: Out, code: &str, reason: impl Into<String>) -> Result<Out, LibError> {
    out.insert("valid".to_string(), Value::Bool(false));
    out.insert("reasonCode".to_string(), Value::string(code));
    out.insert("reason".to_string(), Value::string(reason.into()));
    Ok(out)
}

/// The bytes at `location`, read a window at a time as the framework (the caller checked the user's access), at most
/// `max` of them: `Ok(None)` when nothing is there, `Err((code, reason))` when it cannot be read or is larger.
pub(crate) fn read_location(location: &str, max: u64) -> Result<Option<Vec<u8>>, (&'static str, String)> {
    let opened = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory().resource().open(location)
    })
    .ok_or(("NOT_READ", "no execution context to read the file".to_string()))?;
    let source = match opened {
        Ok(Some(opened)) => opened.source,
        Ok(None) => return Ok(None),
        Err(err) => return Err(("NOT_READ", format!("the file could not be read: {}", err.message()))),
    };
    let size = source.len();
    if size > max {
        return Err((
            "TOO_LARGE",
            format!("the file is {size} bytes, more than the {max} an overlay may be"),
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    while (bytes.len() as u64) < size {
        let window = source
            .read_at(bytes.len() as u64, READ_WINDOW)
            .map_err(|err| ("NOT_READ", format!("the file could not be read: {}", err.message())))?;
        if window.is_empty() {
            break;
        }
        bytes.extend_from_slice(&window);
    }
    Ok(Some(bytes))
}

/// The formats named in a refusal of a file of another.
pub(crate) const FORMATS_READ: &str = "GeoJSON, TopoJSON, EsriJSON, GPX, KML, GML, WKT, WKB, CSV with a WKT, WKB or \
    latitude and longitude column, or a zipped shapefile (.geojson, .json, .topojson, .esrijson, .gpx, .kml, .gml, .wkt, \
    .wkb, .csv, .zip)";

/// `convert#Overlay`. In: `location`, `text` or `bytes` (one of them), `format` (an MLF_ enum ID, or by `fileName`'s
/// extension), `maxBytes`, `maxFeatures`, `summaryOnly` (no `geojson` text). Out: `valid`; when not, `reasonCode` and
/// `reason` (FORMAT_UNKNOWN, NOT_FOUND, NOT_READ, NOT_TEXT, or the reader's: TOO_LARGE, NOT_PARSED, NOT_FORMAT,
/// CRS_UNSUPPORTED, TOO_MANY_FEATURES, NO_FEATURES); when it is, `format`, `featureCount`, `rejectedCount`, `bounds`
/// [west, south, east, north], `crs` with `transform` (what converting it to WGS84 did, empty for WGS84), `notes`, `size`
/// and `geojson`. A CRS of the projection registry ([`projection_registry`]) is converted, any other refused.
pub(crate) fn convert_overlay_service(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let out = Out::new();
    let format_name = text(parameters, "format");
    let format =
        OverlayFormat::parse(&format_name).or_else(|| OverlayFormat::of_file_name(&text(parameters, "fileName")));
    let Some(format) = format else {
        return refusal(out, "FORMAT_UNKNOWN", format!("the file is not {FORMATS_READ}"));
    };
    let max_bytes = number(parameters, "maxBytes")
        .filter(|max| *max > 0)
        .unwrap_or(DEFAULT_MAX_BYTES);
    let max_features = number(parameters, "maxFeatures").filter(|max| *max > 0).unwrap_or(5000);
    let location = text(parameters, "location");
    let content: Vec<u8> = if !location.is_empty() {
        match read_location(&location, max_bytes) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return refusal(out, "NOT_FOUND", "there is no file there"),
            Err((code, reason)) => return refusal(out, code, reason),
        }
    } else if let Some(Value::Bytes(bytes)) = parameters.get("bytes") {
        bytes.clone()
    } else {
        text(parameters, "text").into_bytes()
    };
    if content.len() as u64 > max_bytes {
        return refusal(
            out,
            "TOO_LARGE",
            format!(
                "the file is {} bytes, more than the {max_bytes} an overlay may be",
                content.len()
            ),
        );
    }
    let limits = OverlayLimits {
        max_bytes: usize::try_from(max_bytes).unwrap_or(usize::MAX),
        max_features: usize::try_from(max_features).unwrap_or(usize::MAX),
    };
    let overlay = match convert_overlay_with(&content, format, &limits, &projection_registry(facade)) {
        Ok(overlay) => overlay,
        Err(err) => return refusal(out, err.code, err.message),
    };
    let mut out = out;
    out.insert("valid".to_string(), Value::Bool(true));
    out.insert("format".to_string(), Value::string(format.enum_id()));
    out.insert(
        "featureCount".to_string(),
        Value::I64(i64::try_from(overlay.features.len()).unwrap_or(i64::MAX)),
    );
    out.insert(
        "rejectedCount".to_string(),
        Value::I64(i64::try_from(overlay.rejected).unwrap_or(i64::MAX)),
    );
    out.insert(
        "bounds".to_string(),
        overlay.bounds.map_or(Value::Null, |bounds| {
            Value::List(bounds.iter().map(|value| Value::F64(*value)).collect())
        }),
    );
    out.insert("crs".to_string(), Value::string(overlay.crs.clone()));
    out.insert("transform".to_string(), Value::string(overlay.transform.clone()));
    out.insert(
        "notes".to_string(),
        Value::List(overlay.notes.iter().cloned().map(Value::string).collect()),
    );
    out.insert(
        "size".to_string(),
        Value::I64(i64::try_from(content.len()).unwrap_or(i64::MAX)),
    );
    if !flag(parameters, "summaryOnly") {
        out.insert(
            "geojson".to_string(),
            Value::string(serde_json::to_string(&overlay.feature_collection()).unwrap_or_default()),
        );
    }
    Ok(out)
}
