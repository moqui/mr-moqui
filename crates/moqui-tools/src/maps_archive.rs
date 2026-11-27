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

//! `org.moqui.maps.MapSourceServices.read#ArchiveHeader` (plans/archive/Maps.md §4.1, §5): the header and metadata of a PMTiles
//! archive at a resource location, read by range through the resource facade (two reads: the 127-byte header, then
//! the metadata where the header puts it) and checked by [`moqui_lib::pmtiles`], for registering a map archive and for
//! its readiness. It writes nothing. Its grant is MAPS_ADMIN's (it reads any location it is given); the services that
//! register and check an archive call it.

use std::collections::BTreeMap;
use std::io::Read;

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::geo::tiff::{read_cog_header as read_cog_header_from, CogInfo};
use moqui_lib::object::Value;
use moqui_lib::pmtiles::{Compression, Header, Metadata, HEADER_LEN};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;

use crate::group_db::{text, Out};

/// The most metadata bytes read as stored, and as decompressed.
const METADATA_MAX: u64 = 1024 * 1024;
const METADATA_TEXT_MAX: u64 = 4 * 1024 * 1024;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, read_archive_header)?;
    rust_service!(map, read_cog_header)?;
    Ok(())
}

fn number(value: u64) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

fn header_map(header: &Header) -> Value {
    let mut map = IndexMap::default();
    map.insert("version".to_string(), Value::I64(i64::from(header.version)));
    map.insert("tileType".to_string(), Value::string(header.tile_type.name()));
    map.insert("vector".to_string(), Value::Bool(header.tile_type.is_vector()));
    map.insert("raster".to_string(), Value::Bool(header.tile_type.is_raster()));
    map.insert(
        "tileCompression".to_string(),
        Value::string(header.tile_compression.name()),
    );
    map.insert(
        "internalCompression".to_string(),
        Value::string(header.internal_compression.name()),
    );
    map.insert("minZoom".to_string(), Value::I64(i64::from(header.min_zoom)));
    map.insert("maxZoom".to_string(), Value::I64(i64::from(header.max_zoom)));
    map.insert("minLongitude".to_string(), Value::F64(header.min_lon));
    map.insert("minLatitude".to_string(), Value::F64(header.min_lat));
    map.insert("maxLongitude".to_string(), Value::F64(header.max_lon));
    map.insert("maxLatitude".to_string(), Value::F64(header.max_lat));
    map.insert("centerZoom".to_string(), Value::I64(i64::from(header.center_zoom)));
    map.insert("centerLongitude".to_string(), Value::F64(header.center_lon));
    map.insert("centerLatitude".to_string(), Value::F64(header.center_lat));
    map.insert("addressedTiles".to_string(), number(header.addressed_tiles));
    map.insert("tileContents".to_string(), number(header.tile_contents));
    map.insert("clustered".to_string(), Value::Bool(header.clustered));
    Value::Map(map)
}

fn metadata_map(metadata: &Metadata) -> Value {
    let mut map = IndexMap::default();
    map.insert("name".to_string(), Value::string(metadata.name.clone()));
    map.insert("description".to_string(), Value::string(metadata.description.clone()));
    map.insert("attribution".to_string(), Value::string(metadata.attribution.clone()));
    map.insert("version".to_string(), Value::string(metadata.version.clone()));
    map.insert("type".to_string(), Value::string(metadata.kind.clone()));
    map.insert(
        "vectorLayers".to_string(),
        Value::List(metadata.vector_layers.iter().cloned().map(Value::string).collect()),
    );
    Value::Map(map)
}

/// The metadata's JSON text from its stored bytes, by the archive's internal compression.
fn metadata_text(stored: &[u8], compression: Compression) -> Result<Vec<u8>, String> {
    match compression {
        Compression::None | Compression::Unknown => Ok(stored.to_vec()),
        Compression::Gzip => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(stored)
                .take(METADATA_TEXT_MAX + 1)
                .read_to_end(&mut out)
                .map_err(|err| format!("its metadata does not decompress: {err}"))?;
            if out.len() as u64 > METADATA_TEXT_MAX {
                return Err(format!(
                    "its metadata is more than {METADATA_TEXT_MAX} bytes decompressed"
                ));
            }
            Ok(out)
        }
        other => Err(format!(
            "its metadata is compressed with {}, which is not read here",
            other.name()
        )),
    }
}

/// `read#ArchiveHeader`: the PMTiles archive at `location`. Out: `found` (there is a file there), `valid` (a version 3
/// archive with no problem), `problems` (each reason in words), `size`, `tag` (the strong tag the storage gives it, empty
/// for a file on disk), `header` and `metadata`. Nothing there, a read that fails and a file that is not such an
/// archive are `problems`, not errors.
pub(crate) fn read_archive_header(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let location = text(parameters, "location");
    let mut out = Out::new();
    let mut problems: Vec<String> = Vec::new();
    let finish = |mut out: Out, problems: Vec<String>, valid: bool| {
        out.insert("valid".to_string(), Value::Bool(valid && problems.is_empty()));
        out.insert(
            "problems".to_string(),
            Value::List(problems.into_iter().map(Value::string).collect()),
        );
        Ok(out)
    };
    out.insert("found".to_string(), Value::Bool(false));
    if location.is_empty() {
        problems.push("no location was given".to_string());
        return finish(out, problems, false);
    }
    let opened = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory().resource().open(&location)
    })
    .ok_or_else(|| LibError::new("no execution context to read the archive"))?;
    let opened = match opened {
        Ok(Some(opened)) => opened,
        Ok(None) => {
            problems.push(format!("there is no file at {location}"));
            return finish(out, problems, false);
        }
        Err(err) => {
            problems.push(format!("{location} could not be read: {}", err.message()));
            return finish(out, problems, false);
        }
    };
    let source = opened.source;
    let size = source.len();
    out.insert("found".to_string(), Value::Bool(true));
    out.insert("size".to_string(), number(size));
    out.insert("tag".to_string(), Value::string(source.tag().unwrap_or_default()));
    let head = match source.read_at(0, HEADER_LEN) {
        Ok(head) => head,
        Err(err) => {
            problems.push(format!("its header could not be read: {}", err.message()));
            return finish(out, problems, false);
        }
    };
    let header = match Header::parse(&head) {
        Ok(header) => header,
        Err(err) => {
            problems.push(err.message().to_string());
            return finish(out, problems, false);
        }
    };
    problems.extend(header.problems(size));
    out.insert("header".to_string(), header_map(&header));
    let metadata_end = header.metadata_offset.checked_add(header.metadata_length);
    if header.metadata_length > METADATA_MAX {
        problems.push(format!(
            "its metadata is {} bytes, more than the {METADATA_MAX} read here",
            header.metadata_length
        ));
    } else if header.metadata_length > 0 && metadata_end.is_some_and(|end| end <= size) {
        let stored = usize::try_from(header.metadata_length)
            .map_err(|_| LibError::new("metadata length"))
            .and_then(|length| source.read_at(header.metadata_offset, length));
        match stored {
            Err(err) => problems.push(format!("its metadata could not be read: {}", err.message())),
            Ok(stored) => match metadata_text(&stored, header.internal_compression)
                .and_then(|json| Metadata::parse(&json).map_err(|err| err.message().to_string()))
            {
                Ok(metadata) => {
                    if header.tile_type.is_vector() && metadata.vector_layers.is_empty() {
                        problems.push("its metadata names no vector layers".to_string());
                    }
                    out.insert("metadata".to_string(), metadata_map(&metadata));
                }
                Err(problem) => problems.push(problem),
            },
        }
    }
    finish(out, problems, true)
}

/// The CRS of a COG's EPSG code as a projection: WGS84 and Web Mercator, else a row of the registry; `Err` with the
/// reason when the row is there but its definition is not one the server reads (a map in the browser may still draw it).
fn cog_projection(code: u32, facade: &EntityFacade) -> Option<Result<moqui_lib::geo::proj::Projection, String>> {
    if let Some(projection) = moqui_lib::geo::proj::builtin(code) {
        return Some(Ok(projection));
    }
    let registry = crate::maps_overlay::projection_registry(facade);
    match registry.get(code) {
        Some(entry) => Some(Ok(entry.projection.clone())),
        None => registry.unreadable(code).map(|reason| Err(reason.to_string())),
    }
}

fn cog_map(info: &CogInfo, wgs84: Option<[f64; 4]>) -> Value {
    let floats = |values: &[f64]| Value::List(values.iter().map(|value| Value::F64(*value)).collect());
    let mut map = IndexMap::default();
    map.insert("bigTiff".to_string(), Value::Bool(info.big_tiff));
    map.insert("width".to_string(), number(info.width));
    map.insert("height".to_string(), number(info.height));
    map.insert("bands".to_string(), Value::I64(i64::from(info.bands)));
    map.insert("bitsPerSample".to_string(), Value::I64(i64::from(info.bits_per_sample)));
    map.insert("sampleFormat".to_string(), Value::I64(i64::from(info.sample_format)));
    map.insert("sampleType".to_string(), Value::string(info.sample_type()));
    map.insert("compression".to_string(), Value::string(info.compression_name()));
    map.insert("photometric".to_string(), Value::I64(i64::from(info.photometric)));
    map.insert("tileWidth".to_string(), number(info.tile_width));
    map.insert("tileHeight".to_string(), number(info.tile_height));
    map.insert(
        "overviews".to_string(),
        Value::I64(i64::try_from(info.overviews.len()).unwrap_or(0)),
    );
    map.insert("masks".to_string(), Value::I64(i64::try_from(info.masks).unwrap_or(0)));
    map.insert(
        "epsg".to_string(),
        info.epsg
            .map_or(Value::Null, |code| Value::string(format!("EPSG:{code}"))),
    );
    map.insert("geographic".to_string(), Value::Bool(info.geographic));
    map.insert(
        "bounds".to_string(),
        info.bounds.map_or(Value::Null, |bounds| floats(&bounds)),
    );
    map.insert(
        "wgs84Bounds".to_string(),
        wgs84.map_or(Value::Null, |bounds| floats(&bounds)),
    );
    map.insert(
        "resolution".to_string(),
        info.resolution.map_or(Value::Null, |resolution| floats(&resolution)),
    );
    map.insert(
        "nodata".to_string(),
        info.nodata.clone().map_or(Value::Null, Value::string),
    );
    map.insert(
        "statistics".to_string(),
        Value::List(
            info.statistics
                .iter()
                .map(|(band, min, max)| floats(&[f64::from(*band), *min, *max]))
                .collect(),
        ),
    );
    Value::Map(map)
}

/// `read#CogHeader`: the GeoTIFF at `location` (read by range as the framework), or its first bytes given as `bytes`
/// with the file's `size` (a provider's COG, which the caller fetched by a range: nothing past them is read). Out:
/// `found`, `valid` (a map can draw it: no `problems`), `problems` and `notes` in words, `size`, `tag` (the storage's
/// strong tag, empty for a file on disk or given bytes) and `cog` (the header: its bands, sample type, compression, size,
/// tiles, overviews, EPSG code, bounds in its CRS and in WGS84 when its CRS is known here, resolution, no-data and
/// statistics). A CRS that is neither WGS84, Web Mercator nor a registered projection is a problem.
pub(crate) fn read_cog_header(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let location = text(parameters, "location");
    let mut out = Out::new();
    let finish = |mut out: Out, problems: Vec<String>, notes: Vec<String>| {
        out.insert("valid".to_string(), Value::Bool(problems.is_empty()));
        out.insert(
            "problems".to_string(),
            Value::List(problems.into_iter().map(Value::string).collect()),
        );
        out.insert(
            "notes".to_string(),
            Value::List(notes.into_iter().map(Value::string).collect()),
        );
        Ok(out)
    };
    out.insert("found".to_string(), Value::Bool(false));
    let given = match parameters.get("bytes") {
        Some(Value::Bytes(bytes)) => Some(bytes.clone()),
        _ => None,
    };
    let read = if let Some(head) = given {
        let size = text(parameters, "size")
            .trim()
            .parse::<u64>()
            .unwrap_or(head.len() as u64);
        out.insert("found".to_string(), Value::Bool(true));
        out.insert("size".to_string(), number(size));
        out.insert("tag".to_string(), Value::string(""));
        let mut read_at = |offset: u64, len: usize| -> Result<Vec<u8>, String> {
            let start = usize::try_from(offset).map_err(|_| "an offset past what was read".to_string())?;
            let end = start.saturating_add(len);
            if end > head.len() && (end as u64) <= size {
                return Err(format!(
                    "its directories are past the first {} bytes read from it: not the COG layout",
                    head.len()
                ));
            }
            Ok(head.get(start..end.min(head.len())).unwrap_or_default().to_vec())
        };
        read_cog_header_from(&mut read_at, size)
    } else {
        if location.is_empty() {
            return finish(out, vec!["no location was given".to_string()], Vec::new());
        }
        let opened = ExecutionContext::with_current(|ec| {
            let _authz = ec.disable_authz();
            ec.factory().resource().open(&location)
        })
        .ok_or_else(|| LibError::new("no execution context to read the GeoTIFF"))?;
        let opened = match opened {
            Ok(Some(opened)) => opened,
            Ok(None) => return finish(out, vec![format!("there is no file at {location}")], Vec::new()),
            Err(err) => {
                return finish(
                    out,
                    vec![format!("{location} could not be read: {}", err.message())],
                    Vec::new(),
                )
            }
        };
        let source = opened.source;
        let size = source.len();
        out.insert("found".to_string(), Value::Bool(true));
        out.insert("size".to_string(), number(size));
        out.insert("tag".to_string(), Value::string(source.tag().unwrap_or_default()));
        let mut read_at = |offset: u64, len: usize| -> Result<Vec<u8>, String> {
            source.read_at(offset, len).map_err(|err| err.message().to_string())
        };
        read_cog_header_from(&mut read_at, size)
    };
    let info = match read {
        Ok(info) => info,
        Err(problem) => return finish(out, vec![problem], Vec::new()),
    };
    let mut problems = info.problems.clone();
    let mut notes = info.notes.clone();
    let wgs84 = match (info.epsg, info.bounds) {
        (Some(code), Some(bounds)) => match cog_projection(code, facade) {
            Some(Ok(projection)) => moqui_lib::geo::proj::wgs84_bounds(&projection, bounds),
            Some(Err(reason)) => {
                notes.push(format!(
                    "its CRS, EPSG:{code}, is registered, but the server does not read its definition ({reason}): a map draws it, and its bounds in WGS84 are not recorded"
                ));
                None
            }
            None => {
                problems.push(format!(
                    "its CRS, EPSG:{code}, is not a registered map projection: an administrator adds it under Maps › Configuration first"
                ));
                None
            }
        },
        _ => None,
    };
    out.insert("cog".to_string(), cog_map(&info, wgs84));
    finish(out, problems, notes)
}

#[cfg(test)]
#[path = "maps_archive_tests.rs"]
mod tests;
