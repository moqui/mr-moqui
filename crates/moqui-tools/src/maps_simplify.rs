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

//! `org.moqui.maps.MapSourceServices.generalize#Overlay` (plans/Maps.md §7 "Geometry/import jobs"; docs/tools/Maps.md
//! "Large overlays"): a user's overlay file too large to draw whole (past the 4 MiB of GeoJSON a map draws, or past
//! `maps@max-features`), read and sanitized as an overlay ([`moqui_lib::geo::overlay`], with a larger feature limit),
//! cut into simplified vector tiles for each zoom ([`moqui_lib::geo::tiles`]) and written as a PMTiles archive
//! ([`moqui_lib::pmtiles::write_archive`], tiles and directories gzipped), answered as bytes for MapOverlays.js to
//! store beside the file in its drive. It runs in the MapOverlayTiles ServiceJob: each zoom made is written to the
//! overlay's row as its progress (`tilesProgress`, in a transaction of its own) and the row's `tilesCancel` is read
//! between zooms, so a cancel stops it. Memory is bounded by the file's size limit and the tiles' limits. It reads
//! whatever location it is given, so it is no group's and not remote.

use std::collections::BTreeMap;
use std::io::Write;

use moqui_lib::error::Error as LibError;
use moqui_lib::geo::overlay::{convert_overlay, OverlayFormat, OverlayLimits};
use moqui_lib::geo::tiles::{build_tiles, Progress, TileFeature, TilerLimits, LAYER, MAX_ZOOM};
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::pmtiles::{write_archive, ArchiveOptions, Compression, TileType};

use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;

use crate::group_db::{number, text, Out};
use crate::maps_overlay::read_location;

const OVERLAY: &str = "moqui.maps.MapOverlay";
/// The file's size when the caller names none (it names maps@overlay-max-bytes, as store#Overlay reads it), and the
/// features a caller may lower; the tiles' limits are [`TilerLimits::standard`], which a caller may lower.
const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_MAX_FEATURES: u64 = 250_000;
/// How often at most a run writes its progress (and reads its cancel), so a zoom of many tiles costs few writes.
const REPORT_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, generalize_overlay)?;
    Ok(())
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    // writing into a Vec does not fail
    let _ = encoder.write_all(bytes);
    encoder.finish().unwrap_or_default()
}

fn refusal(mut out: Out, code: &str, reason: impl Into<String>) -> Result<Out, LibError> {
    out.insert("valid".to_string(), Value::Bool(false));
    out.insert("reasonCode".to_string(), Value::string(code));
    out.insert("reason".to_string(), Value::string(reason.into()));
    Ok(out)
}

fn limit(parameters: &BTreeMap<String, Value>, key: &str, default: u64) -> u64 {
    number(parameters, key)
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0)
        .map_or(default, |value| value.min(default))
}

/// Write `progress` (a percent) onto overlay `overlay_id` in a transaction of its own (a pending run is then running),
/// and answer whether to go on: its `tilesCancel` is not Y. A row that cannot be read or written is logged and the run goes on.
fn report(facade: &EntityFacade, overlay_id: &str, progress: i64) -> bool {
    let mut cancelled = false;
    let written = facade.transactions_for(OVERLAY).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            let Some(mut row) = facade.find(OVERLAY)?.eq("overlayId", Value::string(overlay_id)).one()? else {
                return Ok(());
            };
            cancelled = row.get("tilesCancel").map(to_plain_string).unwrap_or_default() == "Y";
            row.set("tilesProgress", Value::I64(progress));
            if row.get("tilesStatus").map(to_plain_string).unwrap_or_default() == "pending" {
                row.set("tilesStatus", Value::string("running"));
            }
            row.update(facade)
        })
    });
    if let Err(err) = written {
        tracing::error!(error = %err, overlay = %overlay_id, "Could not write a map overlay's tiling progress");
    }
    !cancelled
}

/// `generalize#Overlay`. In: `location`, `format` (MLF_GEOJSON, MLF_GPX, MLF_KML, else by `fileName`), `maxBytes`
/// (the file: the caller's maps@overlay-max-bytes, 10 MiB when none), `maxFeatures`, `maxTiles`, `maxArchiveBytes`,
/// `maxZoom` and `maxWork` (each at most its default: 250,000 and [`TilerLimits::standard`]'s), `overlayId` (the row
/// its progress is written to and its cancel read from). Out: `valid`; when not, `reasonCode` and `reason` (the overlay
/// reader's, NOT_FOUND, NOT_READ, NOT_TEXT, NO_TILES, TOO_LARGE_TO_TILE, CANCELLED with `cancelled` true); when it is,
/// `bytes` (the archive), `size`, `maxZoom`, `tileCount`, `featureCount`, `rejectedCount`, `bounds`, `notes` and
/// `stoppedReason` (a limit that ended it below zoom 14).
pub(crate) fn generalize_overlay(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let out = Out::new();
    let format = OverlayFormat::parse(&text(parameters, "format"))
        .or_else(|| OverlayFormat::of_file_name(&text(parameters, "fileName")));
    let Some(format) = format else {
        return refusal(out, "FORMAT_UNKNOWN", "the file is not GeoJSON, GPX or KML");
    };
    // the caller's limit of the file as it is (store#Overlay read the file within the same one), else the default
    let max_bytes = number(parameters, "maxBytes")
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_BYTES);
    let location = text(parameters, "location");
    let content = match read_location(&location, max_bytes) {
        Ok(Some(bytes)) => match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(_) => return refusal(out, "NOT_TEXT", "the file is not UTF-8 text"),
        },
        Ok(None) => return refusal(out, "NOT_FOUND", "there is no file there"),
        Err((code, reason)) => return refusal(out, code, reason),
    };
    let limits = OverlayLimits {
        max_bytes: usize::try_from(max_bytes).unwrap_or(usize::MAX),
        max_features: usize::try_from(limit(parameters, "maxFeatures", DEFAULT_MAX_FEATURES)).unwrap_or(usize::MAX),
    };
    let overlay = match convert_overlay(&content, format, &limits) {
        Ok(overlay) => overlay,
        Err(err) => return refusal(out, err.code, err.message),
    };
    drop(content);
    let features: Vec<TileFeature> = overlay
        .features
        .iter()
        .enumerate()
        .filter_map(|(index, feature)| TileFeature::from_geojson(feature, index as u64 + 1))
        .collect();
    let standard = TilerLimits::standard();
    let tiler = TilerLimits {
        max_zoom: u8::try_from(limit(parameters, "maxZoom", u64::from(MAX_ZOOM))).unwrap_or(MAX_ZOOM),
        max_tiles: usize::try_from(limit(parameters, "maxTiles", standard.max_tiles as u64)).unwrap_or(usize::MAX),
        max_bytes: usize::try_from(limit(parameters, "maxArchiveBytes", standard.max_bytes as u64))
            .unwrap_or(usize::MAX),
        max_work: limit(parameters, "maxWork", standard.max_work),
        ..standard
    };
    let overlay_id = text(parameters, "overlayId");
    // each zoom's end, and within a zoom at most once a second: the percent of the zooms, a zoom counted by its tiles
    let mut last_report: Option<std::time::Instant> = None;
    let mut progress = |at: Progress| {
        if overlay_id.is_empty() {
            return true;
        }
        let zoom_end = at.tiles_done == at.tiles;
        if !zoom_end && last_report.is_some_and(|last| last.elapsed() < REPORT_EVERY) {
            return true;
        }
        last_report = Some(std::time::Instant::now());
        let share = if at.tiles == 0 {
            1.0
        } else {
            at.tiles_done as f64 / at.tiles as f64
        };
        // a percent from 0 to 100
        #[allow(clippy::cast_possible_truncation)]
        let percent = ((f64::from(at.zoom) + share) * 100.0 / f64::from(at.top + 1)).round() as i64;
        report(facade, &overlay_id, percent.clamp(0, 100))
    };
    let set = match build_tiles(&features, &tiler, &gzip, &mut progress) {
        Ok(Some(set)) => set,
        Ok(None) => {
            let mut out = out;
            out.insert("cancelled".to_string(), Value::Bool(true));
            return refusal(out, "CANCELLED", "the simplification was cancelled");
        }
        Err(err) => return refusal(out, "TOO_LARGE_TO_TILE", err.message().to_string()),
    };
    if set.tiles.is_empty() {
        return refusal(out, "NO_TILES", "nothing of the file is left to draw");
    }
    let bounds = overlay.bounds.unwrap_or([-180.0, -85.0, 180.0, 85.0]);
    let metadata = serde_json::json!({
        "name": "overlay",
        "type": "overlay",
        "description": "A map overlay simplified for each zoom",
        "vector_layers": [{"id": LAYER, "minzoom": 0, "maxzoom": set.max_zoom,
            "fields": {"label": "String", "description": "String"}}]
    });
    let options = ArchiveOptions {
        tile_type: TileType::Mvt,
        tile_compression: Compression::Gzip,
        internal_compression: Compression::Gzip,
        min_zoom: 0,
        max_zoom: set.max_zoom,
        bounds,
        center_zoom: set.max_zoom.min(10),
    };
    let tile_count = set.tiles.len();
    let max_zoom = set.max_zoom;
    let stopped = set.stopped.clone();
    let archive = write_archive(set.tiles, metadata.to_string().as_bytes(), &options, &gzip)?;
    let mut out = out;
    out.insert("valid".to_string(), Value::Bool(true));
    out.insert(
        "size".to_string(),
        Value::I64(i64::try_from(archive.len()).unwrap_or(i64::MAX)),
    );
    out.insert("bytes".to_string(), Value::Bytes(archive));
    out.insert("maxZoom".to_string(), Value::I64(i64::from(max_zoom)));
    out.insert(
        "tileCount".to_string(),
        Value::I64(i64::try_from(tile_count).unwrap_or(i64::MAX)),
    );
    out.insert(
        "featureCount".to_string(),
        Value::I64(i64::try_from(features.len()).unwrap_or(i64::MAX)),
    );
    out.insert(
        "rejectedCount".to_string(),
        Value::I64(i64::try_from(overlay.rejected).unwrap_or(i64::MAX)),
    );
    out.insert(
        "bounds".to_string(),
        Value::List(bounds.iter().map(|value| Value::F64(*value)).collect()),
    );
    out.insert(
        "notes".to_string(),
        Value::List(overlay.notes.iter().cloned().map(Value::string).collect()),
    );
    // the run ended before its highest zoom by a limit: deeper zooms draw this one's tiles larger
    out.insert("stoppedReason".to_string(), stopped.map_or(Value::Null, Value::string));
    Ok(out)
}
