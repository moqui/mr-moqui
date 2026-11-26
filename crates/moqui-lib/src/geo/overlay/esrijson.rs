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

//! An EsriJSON file as an overlay (plans/Maps.md §4.1; the ArcGIS REST API's FeatureSet): its features' points,
//! multipoints, polylines and polygons (rings grouped as the format winds them: clockwise outer rings, counterclockwise
//! holes), their `attributes` read as properties. The `spatialReference` (the set's, or a geometry's own) is WGS84 when
//! it is absent; EPSG:4326 and Web Mercator (3857, 102100, ...) are read, any other refused. Curves (`curvePaths`,
//! `curveRings`) are not read.

use serde_json::{Map, Value};

use super::{crs_unsupported, rings_to_polygons, Crs, OverlayError, Position, Raw, Reader};

/// Whether a JSON object is EsriJSON rather than GeoJSON: a FeatureSet's members, or a feature with `attributes`.
pub(super) fn is_esrijson(object: &Map<String, Value>) -> bool {
    if object.contains_key("type") {
        return false;
    }
    object.contains_key("spatialReference")
        || object.contains_key("geometryType")
        || object.contains_key("attributes")
        || object
            .get("features")
            .and_then(Value::as_array)
            .and_then(|features| features.first())
            .is_some_and(|feature| feature.get("attributes").is_some() || feature.get("type").is_none())
}

/// A spatialReference's CRS: its `latestWkid` or `wkid`; None when it names neither (absent is WGS84).
fn reference_crs(reference: Option<&Value>) -> Result<Option<Crs>, OverlayError> {
    let Some(reference) = reference.filter(|reference| !reference.is_null()) else {
        return Ok(None);
    };
    let code = ["latestWkid", "wkid"]
        .iter()
        .find_map(|name| reference.get(*name).and_then(Value::as_u64));
    match code.and_then(|code| u32::try_from(code).ok()) {
        // a WKID not WGS84 or Web Mercator is an EPSG code of the registry, or refused when it is used
        Some(code) => Ok(Crs::of_code(code)),
        None if reference.get("wkt").is_some() => Err(crs_unsupported("a spatial reference given as WKT")),
        None => Ok(None),
    }
}

fn position(value: &Value) -> Result<Position, String> {
    let items = value
        .as_array()
        .filter(|items| items.len() >= 2)
        .ok_or_else(|| "a position that is not x and y".to_string())?;
    match (items[0].as_f64(), items[1].as_f64()) {
        (Some(x), Some(y)) => Ok([x, y]),
        _ => Err("a coordinate is not a number".to_string()),
    }
}

fn paths(value: &Value, what: &str) -> Result<Vec<Vec<Position>>, String> {
    let lists = value
        .as_array()
        .filter(|lists| !lists.is_empty())
        .ok_or_else(|| format!("{what} with no parts"))?;
    lists
        .iter()
        .map(|list| {
            list.as_array()
                .ok_or_else(|| format!("{what} part that is not a list of positions"))?
                .iter()
                .map(position)
                .collect()
        })
        .collect()
}

/// An Esri geometry by the members it has.
fn geometry(geometry: &Map<String, Value>) -> Result<Raw, String> {
    if geometry.contains_key("curvePaths") || geometry.contains_key("curveRings") {
        return Err("a geometry of curves, which is not read".to_string());
    }
    if let (Some(x), Some(y)) = (geometry.get("x"), geometry.get("y")) {
        return match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => Ok(Raw::Point([x, y])),
            _ => Err("an empty point".to_string()),
        };
    }
    if let Some(points) = geometry.get("points") {
        let points = points
            .as_array()
            .ok_or_else(|| "points that are not a list".to_string())?;
        return Ok(Raw::MultiPoint(points.iter().map(position).collect::<Result<_, _>>()?));
    }
    if let Some(lines) = geometry.get("paths") {
        return Ok(Raw::MultiLine(paths(lines, "a polyline")?));
    }
    if let Some(rings) = geometry.get("rings") {
        let mut polygons = rings_to_polygons(paths(rings, "a polygon")?)?;
        return Ok(if polygons.len() == 1 {
            Raw::Polygon(polygons.remove(0))
        } else {
            Raw::MultiPolygon(polygons)
        });
    }
    Err("a geometry that is not a point, multipoint, polyline or polygon".to_string())
}

impl Reader {
    /// An EsriJSON FeatureSet, or one feature.
    pub(super) fn esrijson(&mut self, document: &Value) -> Result<(), OverlayError> {
        let Some(object) = document.as_object() else {
            return Err(OverlayError::new("NOT_FORMAT", "the file is not an EsriJSON object"));
        };
        let set_crs = reference_crs(object.get("spatialReference"))?;
        if set_crs.is_none() {
            self.count("the file names no spatial reference, so it was read as WGS84 (EPSG:4326)");
        }
        match object.get("features") {
            Some(Value::Array(features)) => {
                for feature in features {
                    self.esri_feature(feature, set_crs)?;
                }
            }
            Some(_) => {
                return Err(OverlayError::new(
                    "NOT_FORMAT",
                    "the FeatureSet's features are not a list",
                ))
            }
            None if object.contains_key("geometry") => self.esri_feature(document, set_crs)?,
            None => {
                return Err(OverlayError::new(
                    "NOT_FORMAT",
                    "the file is not an EsriJSON FeatureSet",
                ))
            }
        }
        Ok(())
    }

    fn esri_feature(&mut self, feature: &Value, set_crs: Option<Crs>) -> Result<(), OverlayError> {
        self.number += 1;
        let Some(shape) = feature.get("geometry").and_then(Value::as_object) else {
            self.reject("it has no geometry");
            return Ok(());
        };
        let crs = reference_crs(shape.get("spatialReference"))?
            .or(set_crs)
            .unwrap_or(Crs::Wgs84);
        self.use_crs(crs)?;
        let (label, description) = self.pick_properties(feature.get("attributes").and_then(Value::as_object));
        self.add_raw(geometry(shape), &label, &description)
    }
}
