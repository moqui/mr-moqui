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

//! `org.moqui.maps.MapSourceServices.convert#Overlay` (plans/Maps.md §4.1, §8): a GeoJSON, GPX or KML file, at a
//! resource location or given as text, read and checked by [`moqui_lib::geo::overlay`] and answered as a sanitized
//! GeoJSON FeatureCollection in WGS84 with its feature count, bounds, CRS and notes. It writes nothing. It reads any
//! location it is given, so no group is granted it: the overlay services (MapOverlays.js) call it after their own
//! check of the caller's drive access.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::geo::overlay::{convert_overlay, OverlayFormat, OverlayLimits};
use moqui_lib::object::Value;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;

use crate::group_db::{flag, text, Out};

/// The largest overlay when the caller names none, and the window a file is read in.
const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
const READ_WINDOW: usize = 1024 * 1024;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, convert_overlay_service)?;
    Ok(())
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

/// `convert#Overlay`. In: `location` or `text`, `format` (MLF_GEOJSON, MLF_GPX, MLF_KML, or by `fileName`'s extension),
/// `maxBytes`, `maxFeatures`, `summaryOnly` (no `geojson` text). Out: `valid`; when not, `reasonCode` and `reason`
/// (FORMAT_UNKNOWN, NOT_FOUND, NOT_READ, NOT_TEXT, or the reader's: TOO_LARGE, NOT_PARSED, NOT_FORMAT, CRS_UNSUPPORTED,
/// TOO_MANY_FEATURES, NO_FEATURES); when it is, `format`, `featureCount`, `rejectedCount`, `bounds` [west, south, east,
/// north], `crs`, `notes`, `size` and `geojson`.
pub(crate) fn convert_overlay_service(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let out = Out::new();
    let format_name = text(parameters, "format");
    let format =
        OverlayFormat::parse(&format_name).or_else(|| OverlayFormat::of_file_name(&text(parameters, "fileName")));
    let Some(format) = format else {
        return refusal(
            out,
            "FORMAT_UNKNOWN",
            "the file is not GeoJSON, GPX or KML (.geojson, .json, .gpx or .kml)",
        );
    };
    let max_bytes = number(parameters, "maxBytes")
        .filter(|max| *max > 0)
        .unwrap_or(DEFAULT_MAX_BYTES);
    let max_features = number(parameters, "maxFeatures").filter(|max| *max > 0).unwrap_or(5000);
    let location = text(parameters, "location");
    let given;
    let read;
    let content: &str = if location.is_empty() {
        given = text(parameters, "text");
        if given.len() as u64 > max_bytes {
            return refusal(
                out,
                "TOO_LARGE",
                format!(
                    "the file is {} bytes, more than the {max_bytes} an overlay may be",
                    given.len()
                ),
            );
        }
        &given
    } else {
        match read_location(&location, max_bytes) {
            Ok(Some(bytes)) => match String::from_utf8(bytes) {
                Ok(found) => {
                    read = found;
                    &read
                }
                Err(_) => return refusal(out, "NOT_TEXT", "the file is not UTF-8 text"),
            },
            Ok(None) => return refusal(out, "NOT_FOUND", "there is no file there"),
            Err((code, reason)) => return refusal(out, code, reason),
        }
    };
    let limits = OverlayLimits {
        max_bytes: usize::try_from(max_bytes).unwrap_or(usize::MAX),
        max_features: usize::try_from(max_features).unwrap_or(usize::MAX),
    };
    let overlay = match convert_overlay(content, format, &limits) {
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
    out.insert("crs".to_string(), Value::string(overlay.crs));
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
