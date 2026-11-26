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

//! A user's geographic file as a map overlay (plans/Maps.md §4.1, §8): a GeoJSON, TopoJSON, EsriJSON, GPX, KML, GML,
//! WKT, WKB, delimited text (a WKT, WKB or latitude and longitude column) or zipped shapefile read, checked and made a
//! sanitized GeoJSON FeatureCollection in WGS84 for the browser, which draws only that. The readers of the formats added
//! after GeoJSON, GPX and KML are the submodules ([`topojson`], [`esrijson`], [`wkt`] with the delimited text, [`wkb`],
//! [`gml`], [`shapefile`]); each hands its geometries to the one [`Reader`], so every format has the same bounds, checks
//! and plain-text rule. A GeoPackage is not read: it is an SQLite database, and the server links no SQLite reader
//! (docs/tools/Maps.md, Overlays and places).
//!
//! - Bounded: a file over `max_bytes`, or with more than `max_features` features, is refused whole. A zipped shapefile's
//!   members are inflated to at most four times `max_bytes` in all, at most a hundred times their compressed size, and
//!   a TopoJSON file's arcs, which features may share, to at most `max_bytes / 4` positions in all.
//! - Coordinates are WGS84 longitude and latitude, each in range and finite. GPX and KML are WGS84 by definition; a
//!   GeoJSON or TopoJSON file is WGS84 unless its (pre-2016) `crs` member names EPSG:4326 or CRS84 (the same), or
//!   EPSG:3857 (Web Mercator, converted here); an EsriJSON file's `spatialReference`, a GML geometry's `srsName` (an
//!   EPSG:4326 URN or URL read latitude first, as GML has it), an EWKT or EWKB SRID and a shapefile's `.prj` may name
//!   the same two. Any other CRS is refused (CRS_UNSUPPORTED), since no reprojection is made here (§4.1). The CRS
//!   recorded is the file's (its first geometry's).
//! - Each feature is checked by itself: a geometry that is not sane (a position out of range or not a number, a line of
//!   fewer than two positions, a ring of fewer than four, a nesting that is not its type's) rejects that feature, which
//!   is counted and named in the notes, and the rest are kept. A file with no feature left is refused.
//! - Sanitized: a feature keeps only its label and description as plain text, and a GeoJSON feature its own id when it
//!   is short, plain and unique (else `f` and its number) (markup, scripts and styles removed,
//!   entities decoded, control characters dropped, length cut); every other property is left out and counted. KML
//!   network links, overlays of images, styles and icons and every other reference to another file or host are
//!   never followed: they are counted in the notes. The XML reader ([`crate::node`]) refuses a DOCTYPE and entity
//!   declarations, so no external entity is read.
//!
//! No ExecutionContext: `moqui_tools::maps_overlay` reads the file through the resource facade and serves the result.

mod esrijson;
mod gml;
mod shapefile;
mod topojson;
mod wkb;
mod wkt;

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::node::MNode;

/// The formats read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayFormat {
    GeoJson,
    TopoJson,
    EsriJson,
    Gpx,
    Kml,
    Gml,
    /// One WKT or EWKT geometry a line.
    Wkt,
    /// WKB or EWKB: binary geometries one after another, or hex text, one geometry a line.
    Wkb,
    /// Delimited text with a header: a WKT or hex WKB column, or latitude and longitude columns.
    Csv,
    /// A zip of a shapefile: its `.shp`, with its `.dbf`, `.prj` and `.cpg` when it has them.
    Shapefile,
}

impl OverlayFormat {
    /// Every format, in the order a list of them is shown.
    pub const ALL: [Self; 10] = [
        Self::GeoJson,
        Self::TopoJson,
        Self::EsriJson,
        Self::Gpx,
        Self::Kml,
        Self::Gml,
        Self::Wkt,
        Self::Wkb,
        Self::Csv,
        Self::Shapefile,
    ];

    /// A format by its `MapLayerFormat` enum ID (`MLF_GEOJSON`) or its short name (`geojson`, `json`, `gpx`, `kml`, ...).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let lower = name.trim().to_ascii_lowercase();
        let short = lower.strip_prefix("mlf_").unwrap_or(&lower);
        match short {
            "geojson" | "json" => Some(Self::GeoJson),
            "topojson" => Some(Self::TopoJson),
            "esrijson" => Some(Self::EsriJson),
            "gpx" => Some(Self::Gpx),
            "kml" => Some(Self::Kml),
            "gml" => Some(Self::Gml),
            "wkt" => Some(Self::Wkt),
            "wkb" => Some(Self::Wkb),
            "csv" => Some(Self::Csv),
            "shapefile" | "shp" => Some(Self::Shapefile),
            _ => None,
        }
    }

    /// Its `MapLayerFormat` enum ID.
    #[must_use]
    pub fn enum_id(self) -> &'static str {
        match self {
            Self::GeoJson => "MLF_GEOJSON",
            Self::TopoJson => "MLF_TOPOJSON",
            Self::EsriJson => "MLF_ESRIJSON",
            Self::Gpx => "MLF_GPX",
            Self::Kml => "MLF_KML",
            Self::Gml => "MLF_GML",
            Self::Wkt => "MLF_WKT",
            Self::Wkb => "MLF_WKB",
            Self::Csv => "MLF_CSV",
            Self::Shapefile => "MLF_SHAPEFILE",
        }
    }

    /// The format of a file name by its extension (`.esri.json` before `.json`; a shapefile is a `.zip`).
    #[must_use]
    pub fn of_file_name(name: &str) -> Option<Self> {
        let lower = name.trim().to_ascii_lowercase();
        if lower.ends_with(".esri.json") {
            return Some(Self::EsriJson);
        }
        let extension = lower.rsplit_once('.').map(|(_, extension)| extension)?;
        match extension {
            "geojson" | "json" => Some(Self::GeoJson),
            "topojson" => Some(Self::TopoJson),
            "esrijson" => Some(Self::EsriJson),
            "gpx" => Some(Self::Gpx),
            "kml" => Some(Self::Kml),
            "gml" => Some(Self::Gml),
            "wkt" => Some(Self::Wkt),
            "wkb" => Some(Self::Wkb),
            "csv" | "tsv" => Some(Self::Csv),
            "zip" => Some(Self::Shapefile),
            _ => None,
        }
    }

    /// Whether the file is read as bytes rather than UTF-8 text.
    #[must_use]
    pub fn is_binary(self) -> bool {
        matches!(self, Self::Wkb | Self::Shapefile)
    }
}

/// What a file may hold.
#[derive(Clone, Copy, Debug)]
pub struct OverlayLimits {
    pub max_bytes: usize,
    pub max_features: usize,
}

/// A whole file refused: `code` is one of TOO_LARGE, NOT_TEXT, NOT_PARSED, NOT_FORMAT, CRS_UNSUPPORTED,
/// TOO_MANY_FEATURES, NO_FEATURES; `message` says why in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayError {
    pub code: &'static str,
    pub message: String,
}

impl OverlayError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// A file read as an overlay.
#[derive(Clone, Debug)]
pub struct Overlay {
    /// GeoJSON Feature objects, WGS84, each with an `id` and the properties `label` and `description` (plain text).
    pub features: Vec<Value>,
    /// The features left out for a geometry that is not sane.
    pub rejected: usize,
    /// west, south, east, north of every position kept.
    pub bounds: Option<[f64; 4]>,
    /// The file's CRS as it declared it (its first geometry's): EPSG:4326, or EPSG:3857, whose positions are converted.
    pub crs: &'static str,
    /// What was not read or kept, in words: each kind once with its count, and the first rejected features by number.
    pub notes: Vec<String>,
}

impl Overlay {
    /// The features as a GeoJSON FeatureCollection.
    #[must_use]
    pub fn feature_collection(&self) -> Value {
        json!({"type": "FeatureCollection", "features": self.features})
    }
}

/// The most label and description characters kept.
const LABEL_MAX: usize = 200;
const DESCRIPTION_MAX: usize = 2000;
/// The rejected features named one by one in the notes; the rest are counted.
const REJECTS_NAMED: usize = 10;
/// The half-circumference of the Web Mercator sphere, in meters.
const MERCATOR_EDGE: f64 = 20_037_508.342_789_244;
const EARTH_RADIUS: f64 = 6_378_137.0;

/// Read `text` as a file of `format`.
///
/// # Errors
/// An [`OverlayError`] when the file as a whole cannot be an overlay; a feature that cannot be is rejected by itself.
pub fn convert_overlay(text: &str, format: OverlayFormat, limits: &OverlayLimits) -> Result<Overlay, OverlayError> {
    convert_overlay_bytes(text.as_bytes(), format, limits)
}

/// Read `bytes` as a file of `format`: a text format must be UTF-8 (NOT_TEXT otherwise).
///
/// # Errors
/// As [`convert_overlay`].
pub fn convert_overlay_bytes(
    bytes: &[u8],
    format: OverlayFormat,
    limits: &OverlayLimits,
) -> Result<Overlay, OverlayError> {
    if bytes.len() > limits.max_bytes {
        return Err(OverlayError::new(
            "TOO_LARGE",
            format!(
                "the file is {} bytes, more than the {} an overlay may be",
                bytes.len(),
                limits.max_bytes
            ),
        ));
    }
    let mut reader = Reader::new(limits);
    match format {
        OverlayFormat::Shapefile => reader.shapefile(bytes, limits.max_bytes)?,
        OverlayFormat::Wkb => reader.wkb_file(bytes)?,
        _ => {
            // delimited text is often saved as Windows-1252 by a spreadsheet: read so, and said, when it is not UTF-8
            let text: std::borrow::Cow<'_, str> = match std::str::from_utf8(bytes) {
                Ok(text) => text.into(),
                Err(_) if format == OverlayFormat::Csv => {
                    reader.count("the file is not UTF-8, so it was read as Windows-1252");
                    decode_cp1252(bytes).into()
                }
                Err(_) => return Err(OverlayError::new("NOT_TEXT", "the file is not UTF-8 text")),
            };
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            match format {
                OverlayFormat::GeoJson => reader.geojson(text, limits.max_bytes)?,
                OverlayFormat::TopoJson => {
                    let document = parse_json(text)?;
                    reader.topojson(&document, limits.max_bytes)?;
                }
                OverlayFormat::EsriJson => {
                    let document = parse_json(text)?;
                    reader.esrijson(&document)?;
                }
                OverlayFormat::Gpx => reader.gpx(text)?,
                OverlayFormat::Kml => reader.kml(text)?,
                OverlayFormat::Gml => reader.gml(text)?,
                OverlayFormat::Wkt => reader.wkt_file(text)?,
                OverlayFormat::Csv => reader.delimited(text)?,
                OverlayFormat::Wkb | OverlayFormat::Shapefile => {}
            }
        }
    }
    reader.finish()
}

/// Windows-1252's characters for 0x80 to 0x9F (Latin-1 for the rest); an unassigned one is a replacement character.
const CP1252_HIGH: [char; 32] = [
    '€', '\u{FFFD}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{FFFD}', 'Ž', '\u{FFFD}', '\u{FFFD}',
    '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{FFFD}', 'ž', 'Ÿ',
];

/// Bytes read as Windows-1252.
fn decode_cp1252(raw: &[u8]) -> String {
    raw.iter()
        .map(|byte| match byte {
            0x80..=0x9F => CP1252_HIGH[usize::from(byte - 0x80)],
            other => char::from(*other),
        })
        .collect()
}

fn parse_json(text: &str) -> Result<Value, OverlayError> {
    serde_json::from_str(text).map_err(|err| OverlayError::new("NOT_PARSED", format!("the file is not JSON: {err}")))
}

/// A position as kept: longitude, latitude.
type Position = [f64; 2];

/// A geometry as kept.
#[derive(Clone, Debug)]
enum Geometry {
    Point(Position),
    MultiPoint(Vec<Position>),
    LineString(Vec<Position>),
    MultiLineString(Vec<Vec<Position>>),
    Polygon(Vec<Vec<Position>>),
    MultiPolygon(Vec<Vec<Vec<Position>>>),
}

impl Geometry {
    fn to_json(&self) -> Value {
        let (kind, coordinates) = match self {
            Self::Point(point) => ("Point", json!(point)),
            Self::MultiPoint(points) => ("MultiPoint", json!(points)),
            Self::LineString(line) => ("LineString", json!(line)),
            Self::MultiLineString(lines) => ("MultiLineString", json!(lines)),
            Self::Polygon(rings) => ("Polygon", json!(rings)),
            Self::MultiPolygon(polygons) => ("MultiPolygon", json!(polygons)),
        };
        json!({"type": kind, "coordinates": coordinates})
    }

    fn for_each_position(&self, mut body: impl FnMut(&Position)) {
        match self {
            Self::Point(point) => body(point),
            Self::MultiPoint(points) | Self::LineString(points) => points.iter().for_each(body),
            Self::MultiLineString(lines) | Self::Polygon(lines) => lines.iter().flatten().for_each(body),
            Self::MultiPolygon(polygons) => polygons.iter().flatten().flatten().for_each(body),
        }
    }
}

/// The reading of one file: the features kept, the rejections and the notes.
struct Reader {
    max_features: usize,
    /// the positions all features may have: a file's bytes give at most a position for every four (`max_bytes / 4`), so
    /// only a format whose features share parts could pass it, and memory stays bounded whatever the format
    positions_left: usize,
    features: Vec<Value>,
    rejected: usize,
    rejects_named: Vec<String>,
    /// a note's text to the times it happened
    counted: BTreeMap<&'static str, usize>,
    bounds: Option<[f64; 4]>,
    crs: &'static str,
    /// whether `crs` was set by the file (its first geometry's), so a later geometry in another is noted
    crs_given: bool,
    /// the geometry read now is in EPSG:3857: each position converted
    mercator: bool,
    /// the feature number, 1 first, as the file holds them
    number: usize,
    /// a GeoJSON feature's own id, kept for the next feature added when it is short, plain and not used yet
    given_id: Option<String>,
    ids: std::collections::HashSet<String>,
}

impl Reader {
    fn new(limits: &OverlayLimits) -> Self {
        Self {
            max_features: limits.max_features,
            positions_left: (limits.max_bytes / 4).max(1000),
            features: Vec::new(),
            rejected: 0,
            rejects_named: Vec::new(),
            counted: BTreeMap::new(),
            bounds: None,
            crs: "EPSG:4326",
            crs_given: false,
            mercator: false,
            number: 0,
            given_id: None,
            ids: std::collections::HashSet::new(),
        }
    }

    fn count(&mut self, note: &'static str) {
        *self.counted.entry(note).or_insert(0) += 1;
    }

    fn reject(&mut self, reason: &str) {
        self.given_id = None;
        self.rejected += 1;
        if self.rejects_named.len() < REJECTS_NAMED {
            self.rejects_named
                .push(format!("feature {} left out: {reason}", self.number));
        }
    }

    fn add(&mut self, geometry: &Geometry, label: &str, description: &str) -> Result<(), OverlayError> {
        if self.features.len() >= self.max_features {
            return Err(OverlayError::new(
                "TOO_MANY_FEATURES",
                format!(
                    "the file has more than the {} features an overlay may have",
                    self.max_features
                ),
            ));
        }
        let mut count = 0;
        geometry.for_each_position(|_| count += 1);
        if count > self.positions_left {
            return Err(OverlayError::new(
                "TOO_LARGE",
                "the file's features have more positions than an overlay may have",
            ));
        }
        self.positions_left -= count;
        geometry.for_each_position(|&[lon, lat]| {
            self.bounds = Some(match self.bounds {
                None => [lon, lat, lon, lat],
                Some([west, south, east, north]) => [west.min(lon), south.min(lat), east.max(lon), north.max(lat)],
            });
        });
        let mut properties = Map::new();
        let label = plain_text(label, LABEL_MAX, false);
        if !label.is_empty() {
            properties.insert("label".to_string(), Value::String(label));
        }
        let description = plain_text(description, DESCRIPTION_MAX, true);
        if !description.is_empty() {
            properties.insert("description".to_string(), Value::String(description));
        }
        let given = self.given_id.take().filter(|id| {
            !id.is_empty()
                && id.len() <= 64
                && id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "_.:-".contains(character))
                && !self.ids.contains(id)
        });
        let id = given.unwrap_or_else(|| {
            let mut number = self.features.len() + 1;
            while self.ids.contains(&format!("f{number}")) {
                number += 1;
            }
            format!("f{number}")
        });
        self.ids.insert(id.clone());
        self.features
            .push(json!({"type": "Feature", "id": id, "geometry": geometry.to_json(), "properties": properties}));
        Ok(())
    }

    fn finish(self) -> Result<Overlay, OverlayError> {
        if self.features.is_empty() {
            let mut message = "the file has no feature a map can draw".to_string();
            if let Some(first) = self.rejects_named.first() {
                message.push_str(&format!(" ({first})"));
            }
            return Err(OverlayError::new("NO_FEATURES", message));
        }
        let mut notes = self.rejects_named;
        if self.rejected > notes.len() {
            notes.push(format!("{} more features left out", self.rejected - notes.len()));
        }
        for (note, times) in self.counted {
            notes.push(note.replace("{count}", &times.to_string()));
        }
        Ok(Overlay {
            features: self.features,
            rejected: self.rejected,
            bounds: self.bounds,
            crs: self.crs,
            notes,
        })
    }

    // ===== positions =====
    fn position(&self, lon: f64, lat: f64) -> Result<Position, String> {
        if !lon.is_finite() || !lat.is_finite() {
            return Err("a coordinate is not a number".to_string());
        }
        let (lon, lat) = if self.mercator {
            if lon.abs() > MERCATOR_EDGE * 1.000_001 || lat.abs() > MERCATOR_EDGE * 1.01 {
                return Err("a Web Mercator coordinate is outside the projection".to_string());
            }
            let longitude = (lon / EARTH_RADIUS).to_degrees();
            let latitude = (2.0 * (lat / EARTH_RADIUS).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
            (longitude.clamp(-180.0, 180.0), latitude)
        } else {
            (lon, lat)
        };
        if !(-180.0..=180.0).contains(&lon) {
            return Err(format!("a longitude out of range ({lon})"));
        }
        if !(-90.0..=90.0).contains(&lat) {
            return Err(format!("a latitude out of range ({lat})"));
        }
        Ok([lon, lat])
    }

    fn line(&self, positions: Vec<Position>) -> Result<Vec<Position>, String> {
        if positions.len() < 2 {
            return Err("a line of fewer than two positions".to_string());
        }
        Ok(positions)
    }

    fn ring(&mut self, mut positions: Vec<Position>) -> Result<Vec<Position>, String> {
        if positions.first() != positions.last() {
            if let Some(&first) = positions.first() {
                positions.push(first);
                self.count("{count} polygon rings that were not closed were closed");
            }
        }
        if positions.len() < 4 {
            return Err("a polygon ring of fewer than four positions".to_string());
        }
        Ok(positions)
    }

    /// The CRS of the geometry read next: the file's is its first geometry's, and a geometry in another is noted.
    fn use_crs(&mut self, crs: Crs) {
        let name = crs.code();
        if !self.crs_given {
            self.crs = name;
            self.crs_given = true;
        } else if self.crs != name {
            self.count("{count} geometries were in another CRS than the file's first, and converted");
        }
        self.mercator = crs == Crs::WebMercator;
    }

    /// A feature's label and description from its properties (the first of `name`, `title`, `label`, and of
    /// `description`, `desc`, by those names in any case), with the other properties counted, not kept.
    fn pick_properties(&mut self, properties: Option<&Map<String, Value>>) -> (String, String) {
        const LABELS: [&str; 3] = ["name", "title", "label"];
        const DESCRIPTIONS: [&str; 2] = ["description", "desc"];
        let Some(properties) = properties else {
            return (String::new(), String::new());
        };
        let text_of = |names: &[&str]| -> String {
            names
                .iter()
                .find_map(|name| {
                    properties
                        .iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .and_then(|(_, value)| match value {
                            Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
                            Value::Number(number) => Some(number.to_string()),
                            _ => None,
                        })
                })
                .unwrap_or_default()
        };
        let label = text_of(&LABELS);
        let description = text_of(&DESCRIPTIONS);
        let others = properties
            .keys()
            .filter(|key| {
                !LABELS
                    .iter()
                    .chain(DESCRIPTIONS.iter())
                    .any(|kept| key.eq_ignore_ascii_case(kept))
            })
            .count();
        if others > 0 {
            self.count("{count} features had other properties, which are not shown");
        }
        (label, description)
    }

    /// A geometry read in its source's coordinates, checked and kept: a collection's parts are each a feature of their
    /// own, with the same label; a part that is not sane rejects the feature whole.
    fn add_raw(&mut self, raw: Result<Raw, String>, label: &str, description: &str) -> Result<(), OverlayError> {
        let geometries = raw.and_then(|raw| {
            let mut out = Vec::new();
            self.raw_geometries(raw, &mut out)?;
            if out.is_empty() {
                return Err("an empty geometry collection".to_string());
            }
            Ok(out)
        });
        match geometries {
            Ok(geometries) => {
                if geometries.len() > 1 {
                    self.count("{count} geometry collections were drawn as their parts");
                }
                for geometry in &geometries {
                    self.add(geometry, label, description)?;
                }
            }
            Err(reason) => self.reject(&reason),
        }
        Ok(())
    }

    fn raw_positions(&self, positions: Vec<Position>) -> Result<Vec<Position>, String> {
        positions.into_iter().map(|[x, y]| self.position(x, y)).collect()
    }

    fn raw_polygon(&mut self, rings: Vec<Vec<Position>>) -> Result<Vec<Vec<Position>>, String> {
        if rings.is_empty() {
            return Err("a polygon with no ring".to_string());
        }
        let mut out = Vec::with_capacity(rings.len());
        for ring in rings {
            let positions = self.raw_positions(ring)?;
            out.push(self.ring(positions)?);
        }
        Ok(out)
    }

    fn raw_geometries(&mut self, raw: Raw, out: &mut Vec<Geometry>) -> Result<(), String> {
        match raw {
            Raw::Point([x, y]) => out.push(Geometry::Point(self.position(x, y)?)),
            Raw::MultiPoint(points) => {
                if points.is_empty() {
                    return Err("a multipoint with no points".to_string());
                }
                out.push(Geometry::MultiPoint(self.raw_positions(points)?));
            }
            Raw::Line(line) => {
                let positions = self.raw_positions(line)?;
                out.push(Geometry::LineString(self.line(positions)?));
            }
            Raw::MultiLine(lines) => {
                if lines.is_empty() {
                    return Err("a multiline with no lines".to_string());
                }
                let mut kept = Vec::with_capacity(lines.len());
                for line in lines {
                    let positions = self.raw_positions(line)?;
                    kept.push(self.line(positions)?);
                }
                out.push(if kept.len() == 1 {
                    Geometry::LineString(kept.remove(0))
                } else {
                    Geometry::MultiLineString(kept)
                });
            }
            Raw::Polygon(rings) => out.push(Geometry::Polygon(self.raw_polygon(rings)?)),
            Raw::MultiPolygon(polygons) => {
                if polygons.is_empty() {
                    return Err("a multipolygon with no polygons".to_string());
                }
                let mut kept = Vec::with_capacity(polygons.len());
                for rings in polygons {
                    kept.push(self.raw_polygon(rings)?);
                }
                out.push(if kept.len() == 1 {
                    Geometry::Polygon(kept.remove(0))
                } else {
                    Geometry::MultiPolygon(kept)
                });
            }
            Raw::Collection(parts) => {
                for part in parts {
                    self.raw_geometries(part, out)?;
                }
            }
        }
        Ok(())
    }

    // ===== GeoJSON =====
    /// A GeoJSON file; a TopoJSON or EsriJSON one given the GeoJSON format (a `.json` file) is read as that.
    fn geojson(&mut self, text: &str, max_bytes: usize) -> Result<(), OverlayError> {
        let document = parse_json(text)?;
        let Some(object) = document.as_object() else {
            return Err(OverlayError::new("NOT_FORMAT", "the file is not a GeoJSON object"));
        };
        if object.get("type").and_then(Value::as_str) == Some("Topology") {
            return self.topojson(&document, max_bytes);
        }
        if esrijson::is_esrijson(object) {
            return self.esrijson(&document);
        }
        self.declared_crs(object.get("crs"))?;
        match object.get("type").and_then(Value::as_str) {
            Some("FeatureCollection") => {
                let Some(features) = object.get("features").and_then(Value::as_array) else {
                    return Err(OverlayError::new(
                        "NOT_FORMAT",
                        "the FeatureCollection has no features list",
                    ));
                };
                for feature in features {
                    self.geojson_feature(feature)?;
                }
            }
            Some("Feature") => self.geojson_feature(&document)?,
            Some(
                "Point" | "MultiPoint" | "LineString" | "MultiLineString" | "Polygon" | "MultiPolygon"
                | "GeometryCollection",
            ) => self.geojson_feature(&json!({"type": "Feature", "geometry": document, "properties": {}}))?,
            _ => {
                return Err(OverlayError::new(
                    "NOT_FORMAT",
                    "the file is not GeoJSON (it has no GeoJSON type)",
                ))
            }
        }
        Ok(())
    }

    /// The legacy `crs` member: none, WGS84 or Web Mercator.
    fn declared_crs(&mut self, crs: Option<&Value>) -> Result<(), OverlayError> {
        let Some(crs) = crs.filter(|crs| !crs.is_null()) else {
            return Ok(());
        };
        let name = crs
            .pointer("/properties/name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match Crs::of_name(name) {
            // a GeoJSON position is longitude first whatever its CRS's axis order
            Some(Crs::Wgs84 | Crs::Wgs84LatFirst) => {
                self.use_crs(Crs::Wgs84);
                Ok(())
            }
            Some(Crs::WebMercator) => {
                self.use_crs(Crs::WebMercator);
                Ok(())
            }
            None => Err(crs_unsupported(name)),
        }
    }

    fn geojson_feature(&mut self, feature: &Value) -> Result<(), OverlayError> {
        self.number += 1;
        if feature.get("type").and_then(Value::as_str) != Some("Feature") {
            self.reject("it is not a GeoJSON Feature");
            return Ok(());
        }
        let geometry = feature.get("geometry").filter(|geometry| !geometry.is_null());
        let Some(geometry) = geometry else {
            self.reject("it has no geometry");
            return Ok(());
        };
        self.given_id = match feature.get("id") {
            Some(Value::String(id)) => Some(id.clone()),
            Some(Value::Number(id)) => Some(id.to_string()),
            _ => None,
        };
        let (label, description) = self.pick_properties(feature.get("properties").and_then(Value::as_object));
        if geometry.get("type").and_then(Value::as_str) == Some("GeometryCollection") {
            // each member drawn as a feature of its own, with the same label
            let members = geometry
                .get("geometries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if members.is_empty() {
                self.reject("an empty GeometryCollection");
                return Ok(());
            }
            self.count("{count} GeometryCollections were drawn as their parts");
            for member in &members {
                match self.geojson_geometry(member) {
                    Ok(geometry) => self.add(&geometry, &label, &description)?,
                    Err(reason) => self.reject(&reason),
                }
            }
            return Ok(());
        }
        match self.geojson_geometry(geometry) {
            Ok(geometry) => self.add(&geometry, &label, &description)?,
            Err(reason) => self.reject(&reason),
        }
        Ok(())
    }

    fn geojson_position(&self, value: &Value) -> Result<Position, String> {
        let Some(items) = value.as_array().filter(|items| items.len() >= 2) else {
            return Err("a position that is not a longitude and a latitude".to_string());
        };
        match (items[0].as_f64(), items[1].as_f64()) {
            (Some(lon), Some(lat)) => self.position(lon, lat),
            _ => Err("a coordinate is not a number".to_string()),
        }
    }

    fn geojson_positions(&self, value: &Value) -> Result<Vec<Position>, String> {
        let Some(items) = value.as_array() else {
            return Err("coordinates that are not a list".to_string());
        };
        items.iter().map(|item| self.geojson_position(item)).collect()
    }

    fn geojson_lists<'a>(value: &'a Value, what: &str) -> Result<&'a Vec<Value>, String> {
        value
            .as_array()
            .filter(|items| !items.is_empty())
            .ok_or_else(|| format!("{what} that is not a list of its parts"))
    }

    fn geojson_polygon(&mut self, value: &Value) -> Result<Vec<Vec<Position>>, String> {
        let mut rings = Vec::new();
        for ring in Self::geojson_lists(value, "a polygon")? {
            let positions = self.geojson_positions(ring)?;
            rings.push(self.ring(positions)?);
        }
        Ok(rings)
    }

    fn geojson_geometry(&mut self, geometry: &Value) -> Result<Geometry, String> {
        let coordinates = geometry.get("coordinates").unwrap_or(&Value::Null);
        match geometry.get("type").and_then(Value::as_str) {
            Some("Point") => Ok(Geometry::Point(self.geojson_position(coordinates)?)),
            Some("MultiPoint") => {
                let points = self.geojson_positions(coordinates)?;
                if points.is_empty() {
                    return Err("a MultiPoint with no points".to_string());
                }
                Ok(Geometry::MultiPoint(points))
            }
            Some("LineString") => {
                let positions = self.geojson_positions(coordinates)?;
                Ok(Geometry::LineString(self.line(positions)?))
            }
            Some("MultiLineString") => {
                let mut lines = Vec::new();
                for line in Self::geojson_lists(coordinates, "a MultiLineString")? {
                    let positions = self.geojson_positions(line)?;
                    lines.push(self.line(positions)?);
                }
                Ok(Geometry::MultiLineString(lines))
            }
            Some("Polygon") => Ok(Geometry::Polygon(self.geojson_polygon(coordinates)?)),
            Some("MultiPolygon") => {
                let mut polygons = Vec::new();
                for polygon in Self::geojson_lists(coordinates, "a MultiPolygon")? {
                    polygons.push(self.geojson_polygon(polygon)?);
                }
                Ok(Geometry::MultiPolygon(polygons))
            }
            Some("GeometryCollection") => Err("a GeometryCollection inside another".to_string()),
            Some(other) => Err(format!(
                "a geometry type that is not GeoJSON's ({})",
                plain_text(other, 40, false)
            )),
            None => Err("a geometry with no type".to_string()),
        }
    }

    // ===== XML =====
    fn xml_root(text: &str, root: &str, what: &str) -> Result<MNode, OverlayError> {
        let node = MNode::parse_text("overlay", text).map_err(|err| {
            OverlayError::new(
                "NOT_PARSED",
                format!("the file is not XML a map reads: {}", err.message()),
            )
        })?;
        if local(&node.name()) != root {
            return Err(OverlayError::new(
                "NOT_FORMAT",
                format!("the file is not {what} (its root is not <{root}>)"),
            ));
        }
        Ok(node)
    }

    // ===== GPX (WGS84 by definition) =====
    fn gpx(&mut self, text: &str) -> Result<(), OverlayError> {
        let root = Self::xml_root(text, "gpx", "GPX")?;
        for child in root.children() {
            match local(&child.name()) {
                "wpt" => {
                    self.number += 1;
                    match self.gpx_point(&child) {
                        Ok(point) => self.add(
                            &Geometry::Point(point),
                            &child_text(&child, "name"),
                            &gpx_description(&child),
                        )?,
                        Err(reason) => self.reject(&reason),
                    }
                }
                "rte" => {
                    self.number += 1;
                    let points: Result<Vec<Position>, String> = named(&child, "rtept")
                        .iter()
                        .map(|point| self.gpx_point(point))
                        .collect();
                    match points.and_then(|points| self.line(points)) {
                        Ok(line) => self.add(
                            &Geometry::LineString(line),
                            &child_text(&child, "name"),
                            &gpx_description(&child),
                        )?,
                        Err(reason) => self.reject(&reason),
                    }
                }
                "trk" => {
                    self.number += 1;
                    let mut lines = Vec::new();
                    let mut failure = None;
                    for segment in named(&child, "trkseg") {
                        let points: Result<Vec<Position>, String> = named(&segment, "trkpt")
                            .iter()
                            .map(|point| self.gpx_point(point))
                            .collect();
                        match points {
                            // a segment of one point is not a line: passed over, the track kept
                            Ok(points) if points.len() < 2 => {
                                self.count("{count} track segments of fewer than two points were left out")
                            }
                            Ok(points) => lines.push(points),
                            Err(reason) => failure = Some(reason),
                        }
                    }
                    let label = child_text(&child, "name");
                    let description = gpx_description(&child);
                    match (failure, lines.len()) {
                        (Some(reason), _) => self.reject(&reason),
                        (None, 0) => self.reject("a track with no segment of two points or more"),
                        (None, 1) => self.add(&Geometry::LineString(lines.remove(0)), &label, &description)?,
                        (None, _) => self.add(&Geometry::MultiLineString(lines), &label, &description)?,
                    }
                }
                "extensions" | "metadata" => {}
                _ => self.count("{count} GPX elements that are not waypoints, routes or tracks were not read"),
            }
            if child.first_where(|node| local(&node.name()) == "extensions").is_some() {
                self.count("{count} GPX extensions were not read");
            }
        }
        Ok(())
    }

    fn gpx_point(&self, node: &MNode) -> Result<Position, String> {
        let number = |name: &str| node.attribute(name).and_then(|value| value.trim().parse::<f64>().ok());
        match (number("lon"), number("lat")) {
            (Some(lon), Some(lat)) => self.position(lon, lat),
            _ => Err("a point with no lat and lon".to_string()),
        }
    }

    // ===== KML (WGS84 by definition) =====
    fn kml(&mut self, text: &str) -> Result<(), OverlayError> {
        let root = Self::xml_root(text, "kml", "KML")?;
        self.kml_container(&root)
    }

    fn kml_container(&mut self, node: &MNode) -> Result<(), OverlayError> {
        for child in node.children() {
            match local(&child.name()) {
                "Document" | "Folder" => self.kml_container(&child)?,
                "Placemark" => self.kml_placemark(&child)?,
                "NetworkLink" => self.count("{count} network links were not followed"),
                "GroundOverlay" | "ScreenOverlay" | "PhotoOverlay" => {
                    self.count("{count} image overlays were not drawn (their images are not fetched)");
                }
                "Style" | "StyleMap" => {
                    self.count("{count} styles were not read (icons are not fetched; the map draws its own)")
                }
                "Tour" => self.count("{count} tours were not read"),
                "ExtendedData" | "Schema" | "name" | "description" | "open" | "visibility" | "Snippet" | "snippet"
                | "LookAt" | "Camera" | "Region" | "styleUrl" | "atom:author" | "author" | "link" | "address" => {}
                _ => self.count("{count} KML elements that are not placemarks were not read"),
            }
        }
        Ok(())
    }

    fn kml_placemark(&mut self, placemark: &MNode) -> Result<(), OverlayError> {
        self.number += 1;
        let label = child_text(placemark, "name");
        let description = child_text(placemark, "description");
        if placemark
            .first_where(|node| local(&node.name()) == "ExtendedData")
            .is_some()
        {
            self.count("{count} placemarks had extended data, which is not shown");
        }
        if placemark
            .first_where(|node| matches!(local(&node.name()), "Style" | "StyleMap"))
            .is_some()
        {
            self.count("{count} styles were not read (icons are not fetched; the map draws its own)");
        }
        let mut geometries = Vec::new();
        let mut failure = None;
        for child in placemark.children() {
            if is_kml_geometry(&child) {
                if let Err(reason) = self.kml_geometry(&child, &mut geometries) {
                    failure = Some(reason);
                }
            }
        }
        if let Some(reason) = failure {
            self.reject(&reason);
            return Ok(());
        }
        if geometries.is_empty() {
            self.reject("it has no geometry a map draws");
            return Ok(());
        }
        // one geometry, or a MultiGeometry's parts: as a multi geometry when they are of one kind, else each by itself
        for geometry in combine(geometries) {
            self.add(&geometry, &label, &description)?;
        }
        Ok(())
    }

    fn kml_coordinates(&self, node: &MNode) -> Result<Vec<Position>, String> {
        let text = child_text(node, "coordinates");
        text.split_whitespace()
            .map(|tuple| {
                let mut parts = tuple.split(',');
                let lon = parts.next().and_then(|part| part.trim().parse::<f64>().ok());
                let lat = parts.next().and_then(|part| part.trim().parse::<f64>().ok());
                match (lon, lat) {
                    (Some(lon), Some(lat)) => self.position(lon, lat),
                    _ => Err("coordinates that are not longitude,latitude".to_string()),
                }
            })
            .collect()
    }

    fn kml_ring(&mut self, boundary: &MNode) -> Result<Vec<Position>, String> {
        let ring =
            first_named(boundary, "LinearRing").ok_or_else(|| "a polygon boundary with no LinearRing".to_string())?;
        let positions = self.kml_coordinates(&ring)?;
        self.ring(positions)
    }

    fn kml_geometry(&mut self, node: &MNode, out: &mut Vec<Geometry>) -> Result<(), String> {
        match local(&node.name()) {
            "Point" => {
                let positions = self.kml_coordinates(node)?;
                let point = positions
                    .first()
                    .copied()
                    .ok_or_else(|| "a point with no coordinates".to_string())?;
                out.push(Geometry::Point(point));
            }
            "LineString" => {
                let positions = self.kml_coordinates(node)?;
                out.push(Geometry::LineString(self.line(positions)?));
            }
            "LinearRing" => {
                let positions = self.kml_coordinates(node)?;
                out.push(Geometry::Polygon(vec![self.ring(positions)?]));
            }
            "Polygon" => {
                let outer = first_named(node, "outerBoundaryIs")
                    .ok_or_else(|| "a polygon with no outer boundary".to_string())?;
                let mut rings = vec![self.kml_ring(&outer)?];
                for inner in named(node, "innerBoundaryIs") {
                    rings.push(self.kml_ring(&inner)?);
                }
                out.push(Geometry::Polygon(rings));
            }
            "MultiGeometry" => {
                for child in node.children() {
                    if is_kml_geometry(&child) {
                        self.kml_geometry(&child, out)?;
                    }
                }
            }
            "Track" => out.push(Geometry::LineString(self.kml_track(node)?)),
            "MultiTrack" => {
                let mut lines = Vec::new();
                for track in named(node, "Track") {
                    lines.push(self.kml_track(&track)?);
                }
                if lines.is_empty() {
                    return Err("a MultiTrack with no track".to_string());
                }
                out.push(Geometry::MultiLineString(lines));
            }
            _ => {
                // a Model: a 3D file is not fetched
                self.count("{count} 3D models were not drawn");
            }
        }
        Ok(())
    }

    /// A gx:Track: its gx:coord children, "longitude latitude altitude".
    fn kml_track(&mut self, node: &MNode) -> Result<Vec<Position>, String> {
        let positions: Result<Vec<Position>, String> = named(node, "coord")
            .iter()
            .map(|coord| {
                let text = coord.text().unwrap_or_default();
                let mut parts = text.split_whitespace();
                let lon = parts.next().and_then(|part| part.parse::<f64>().ok());
                let lat = parts.next().and_then(|part| part.parse::<f64>().ok());
                match (lon, lat) {
                    (Some(lon), Some(lat)) => self.position(lon, lat),
                    _ => Err("a track coordinate that is not longitude latitude".to_string()),
                }
            })
            .collect();
        self.line(positions?)
    }
}

/// A geometry as a reader found it, in its source's coordinates (x, y: longitude and latitude, or Web Mercator meters),
/// before [`Reader::add_raw`] checks and converts it.
#[derive(Clone, Debug)]
enum Raw {
    Point(Position),
    MultiPoint(Vec<Position>),
    Line(Vec<Position>),
    MultiLine(Vec<Vec<Position>>),
    Polygon(Vec<Vec<Position>>),
    MultiPolygon(Vec<Vec<Vec<Position>>>),
    Collection(Vec<Raw>),
}

/// The CRSs read: WGS84 (longitude first, or latitude first as a GML EPSG:4326 URN gives it) and Web Mercator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Crs {
    Wgs84,
    Wgs84LatFirst,
    WebMercator,
}

impl Crs {
    /// A CRS by its EPSG code; None for one not read here (0, no SRID, is WGS84).
    fn of_code(code: u32) -> Option<Self> {
        match code {
            0 | 4326 => Some(Self::Wgs84),
            3857 | 900_913 | 3785 | 102_100 | 102_113 => Some(Self::WebMercator),
            _ => None,
        }
    }

    /// A CRS by its name: `EPSG:4326`, `CRS:84`, `urn:ogc:def:crs:OGC:1.3:CRS84`, `urn:ogc:def:crs:EPSG::4326` and
    /// `http://www.opengis.net/def/crs/EPSG/0/4326` (these two latitude first), the legacy
    /// `http://www.opengis.net/gml/srs/epsg.xml#4326`, and the same for 3857 and its older codes. None for another.
    fn of_name(name: &str) -> Option<Self> {
        let upper = name.trim().to_ascii_uppercase();
        if upper.ends_with("CRS84") || upper.ends_with("CRS:84") {
            return Some(Self::Wgs84);
        }
        let code: u32 = upper
            .rsplit([':', '/', '#'])
            .find(|part| !part.is_empty())
            .and_then(|code| code.parse().ok())?;
        let crs = Self::of_code(code).filter(|_| code != 0)?;
        let lat_first = upper.starts_with("URN:")
            || upper.starts_with("HTTP://WWW.OPENGIS.NET/DEF/CRS/")
            || upper.starts_with("HTTPS://WWW.OPENGIS.NET/DEF/CRS/");
        Some(if crs == Self::Wgs84 && lat_first {
            Self::Wgs84LatFirst
        } else {
            crs
        })
    }

    fn code(self) -> &'static str {
        match self {
            Self::Wgs84 | Self::Wgs84LatFirst => "EPSG:4326",
            Self::WebMercator => "EPSG:3857",
        }
    }
}

fn crs_unsupported(name: &str) -> OverlayError {
    let name = plain_text(name, 120, false);
    OverlayError::new(
        "CRS_UNSUPPORTED",
        format!(
            "the file's coordinates are in {}, which is not read here: give it in WGS84 (EPSG:4326) or Web Mercator (EPSG:3857)",
            if name.is_empty() { "a CRS that is not named".to_string() } else { name }
        ),
    )
}

/// The most rings one Esri or shapefile geometry may have, and the edge tests grouping its holes may take: past either the
/// feature is left out, so one feature of many rings costs no more than these.
const MAX_RINGS: usize = 10_000;
const MAX_RING_WORK: usize = 20_000_000;

/// Rings of the Esri and shapefile kind as polygons: a clockwise ring starts a polygon and a counterclockwise one is a
/// hole of the first polygon whose outer ring holds its first position, an outer ring tried only when its box holds it
/// (a hole that no outer ring holds is taken as an outer ring itself, for a file whose rings are not wound as the format
/// says, and is not tried for the holes after it). Bounded by [`MAX_RINGS`] and [`MAX_RING_WORK`].
fn rings_to_polygons(rings: Vec<Vec<Position>>) -> Result<Vec<Vec<Vec<Position>>>, String> {
    if rings.len() > MAX_RINGS {
        return Err(format!(
            "a geometry of {} rings, more than the {MAX_RINGS} read",
            rings.len()
        ));
    }
    let mut polygons: Vec<Vec<Vec<Position>>> = Vec::new();
    let mut boxes: Vec<[f64; 4]> = Vec::new();
    let mut holes = Vec::new();
    for ring in rings {
        if signed_area(&ring) <= 0.0 {
            boxes.push(ring_box(&ring));
            polygons.push(vec![ring]);
        } else {
            holes.push(ring);
        }
    }
    // only the outer rings are tried (boxes is theirs): a hole taken as an outer ring is not
    let mut work = 0;
    for hole in holes {
        let mut held = None;
        if let Some(&[x, y]) = hole.first() {
            for (index, [west, south, east, north]) in boxes.iter().enumerate() {
                if x < *west || x > *east || y < *south || y > *north {
                    continue;
                }
                let outer = &polygons[index][0];
                work += outer.len();
                if work > MAX_RING_WORK {
                    return Err("a polygon whose rings are too many to group".to_string());
                }
                if ring_holds(outer, [x, y]) {
                    held = Some(index);
                    break;
                }
            }
        }
        match held {
            Some(index) => polygons[index].push(hole),
            None => polygons.push(vec![hole]),
        }
    }
    Ok(polygons)
}

/// A ring's west, south, east and north.
fn ring_box(ring: &[Position]) -> [f64; 4] {
    ring.iter().fold(
        [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY],
        |[west, south, east, north], [x, y]| [west.min(*x), south.min(*y), east.max(*x), north.max(*y)],
    )
}

/// Twice a ring's signed area: positive counterclockwise, negative clockwise (x east, y north).
fn signed_area(ring: &[Position]) -> f64 {
    edges(ring).map(|([x1, y1], [x2, y2])| x1 * y2 - x2 * y1).sum()
}

/// A ring's edges, the last position back to the first included (nothing more for a closed ring).
fn edges(ring: &[Position]) -> impl Iterator<Item = (Position, Position)> + '_ {
    ring.iter().copied().zip(ring.iter().copied().cycle().skip(1))
}

/// Whether a ring holds a position (even-odd).
fn ring_holds(ring: &[Position], [x, y]: Position) -> bool {
    let mut inside = false;
    for ([x1, y1], [x2, y2]) in edges(ring) {
        if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
    }
    inside
}

fn is_kml_geometry(node: &MNode) -> bool {
    matches!(
        local(&node.name()),
        "Point" | "LineString" | "LinearRing" | "Polygon" | "MultiGeometry" | "Track" | "MultiTrack" | "Model"
    )
}

/// Geometries of one placemark as few features as their kinds allow.
fn combine(geometries: Vec<Geometry>) -> Vec<Geometry> {
    if geometries.len() == 1 {
        return geometries;
    }
    let points = geometries.iter().all(|geometry| matches!(geometry, Geometry::Point(_)));
    let lines = geometries
        .iter()
        .all(|geometry| matches!(geometry, Geometry::LineString(_)));
    let polygons = geometries
        .iter()
        .all(|geometry| matches!(geometry, Geometry::Polygon(_)));
    if points {
        let all = geometries
            .into_iter()
            .filter_map(|geometry| {
                if let Geometry::Point(point) = geometry {
                    Some(point)
                } else {
                    None
                }
            })
            .collect();
        vec![Geometry::MultiPoint(all)]
    } else if lines {
        let all = geometries
            .into_iter()
            .filter_map(|geometry| {
                if let Geometry::LineString(line) = geometry {
                    Some(line)
                } else {
                    None
                }
            })
            .collect();
        vec![Geometry::MultiLineString(all)]
    } else if polygons {
        let all = geometries
            .into_iter()
            .filter_map(|geometry| {
                if let Geometry::Polygon(rings) = geometry {
                    Some(rings)
                } else {
                    None
                }
            })
            .collect();
        vec![Geometry::MultiPolygon(all)]
    } else {
        geometries
    }
}

/// An element's name without its namespace prefix (`gx:Track` is `Track`).
fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, local)| local)
}

fn named(node: &MNode, name: &str) -> Vec<MNode> {
    node.children()
        .into_iter()
        .filter(|child| local(&child.name()) == name)
        .collect()
}

fn first_named(node: &MNode, name: &str) -> Option<MNode> {
    node.children().into_iter().find(|child| local(&child.name()) == name)
}

/// A child's text with the text of the elements in it (markup written as elements rather than escaped or in CDATA),
/// its own first.
fn child_text(node: &MNode, name: &str) -> String {
    fn deep(node: &MNode, out: &mut String) {
        if let Some(text) = node.text() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&text);
        }
        for child in node.children() {
            deep(&child, out);
        }
    }
    let mut out = String::new();
    if let Some(child) = first_named(node, name) {
        deep(&child, &mut out);
    }
    out
}

fn gpx_description(node: &MNode) -> String {
    let description = child_text(node, "desc");
    if description.is_empty() {
        child_text(node, "cmt")
    } else {
        description
    }
}

/// Text as plain text: script, style, iframe and object elements removed with what they hold, comments and every tag
/// removed, the common entities decoded, control characters dropped, white space collapsed (line breaks kept when
/// `lines`), at most `max` characters. What it gives is shown as text, never as markup: a `<` that does not start a tag
/// closed by `>` (`a<b`, `3 < 4`) is kept as text, and an entity decoded to `<` stays text too.
///
/// Linear in its input, and the input is first cut to `max * 8` bytes (on a character boundary): a field of a large file
/// costs no more than a short one.
#[must_use]
pub fn plain_text(text: &str, max: usize, lines: bool) -> String {
    let bounded = bounded(text, max.saturating_mul(8));
    let decoded = decode_entities(&strip_markup(bounded));
    let mut out = String::with_capacity(decoded.len().min(max.saturating_mul(4)));
    let mut pending_space = false;
    let mut pending_line = false;
    let mut count = 0;
    for character in decoded.chars() {
        if character == '\n' && lines {
            pending_line = true;
            continue;
        }
        if character.is_whitespace() {
            pending_space = true;
            continue;
        }
        if character.is_control()
            || matches!(character, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            continue;
        }
        if !out.is_empty() {
            if pending_line {
                out.push('\n');
                count += 1;
            } else if pending_space {
                out.push(' ');
                count += 1;
            }
        }
        pending_space = false;
        pending_line = false;
        if count >= max {
            out.push('…');
            break;
        }
        out.push(character);
        count += 1;
    }
    out
}

/// At most `limit` bytes of `text`, cut on a character boundary.
fn bounded(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The elements whose content is removed with them.
const BLOCKS: [&str; 4] = ["script", "style", "iframe", "object"];

/// `text` with its markup removed in one forward scan: a comment, and a script, style, iframe or object element with
/// what it holds (to the end of the text when it is not closed), become a space; any other tag closed by `>` a space, or
/// a line break for `<br>` and `</p>`; a `<` that starts no tag is kept. Each `>` and closing tag is looked for once
/// from where the scan is, so the scan is linear.
fn strip_markup(text: &str) -> String {
    // ASCII lower case keeps every byte offset
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    // the next '>' at or after a position, found once and kept while the scan is before it; None when there is none
    let mut next_gt: Option<Option<usize>> = None;
    while let Some(offset) = lower[at..].find('<') {
        let start = at + offset;
        out.push_str(&text[at..start]);
        let after = &lower[start + 1..];
        if after.starts_with("!--") {
            at = lower[start + 4..]
                .find("-->")
                .map_or(text.len(), |end| start + 4 + end + 3);
            out.push(' ');
            continue;
        }
        let block = BLOCKS.iter().find(|name| {
            after.starts_with(*name)
                && !after[name.len()..]
                    .chars()
                    .next()
                    .is_some_and(|next| next.is_ascii_alphanumeric())
        });
        if let Some(name) = block {
            let close = format!("</{name}");
            at = lower[start + 1..].find(&close).map_or(text.len(), |end| {
                let close_end = start + 1 + end + close.len();
                lower[close_end..].find('>').map_or(text.len(), |gt| close_end + gt + 1)
            });
            next_gt = None;
            out.push(' ');
            continue;
        }
        let is_tag = after
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, '/' | '!' | '?'));
        let gt = if is_tag {
            match next_gt {
                Some(Some(found)) if found > start => Some(found),
                Some(None) => None,
                _ => {
                    let found = lower[start + 1..].find('>').map(|gt| start + 1 + gt);
                    next_gt = Some(found);
                    found
                }
            }
        } else {
            None
        };
        match gt {
            Some(end) => {
                let line = after.starts_with("br") || after.starts_with("/p");
                out.push(if line { '\n' } else { ' ' });
                at = end + 1;
            }
            None => {
                out.push('<');
                at = start + 1;
            }
        }
    }
    out.push_str(&text[at..]);
    out
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        // a ';' only within an entity's length: never a scan of the rest of the text for each '&'
        let Some(end) = after.as_bytes().iter().take(11).position(|byte| *byte == b';') else {
            out.push('&');
            rest = after;
            continue;
        };
        let name = &after[..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|digits| digits.parse::<u32>().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(character) => {
                out.push(character);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
#[path = "overlay_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "overlay/formats_tests.rs"]
mod formats_tests;
