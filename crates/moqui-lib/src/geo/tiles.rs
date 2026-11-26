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

//! A user's overlay too large to draw whole cut into vector tiles (plans/Maps.md §7 "Geometry/import jobs"; docs/tools/
//! Maps.md "Large overlays"): its features (sanitized GeoJSON from [`super::overlay`]) projected to Web Mercator, and for
//! each zoom from 0 simplified to half a pixel of a 256-pixel tile ([`super::simplify`]), clipped to each tile they
//! touch with a small buffer, points thinned to one a pixel, and written as one MVT layer `overlay` with each feature's
//! `label` and `description` ([`super::mvt`]). It stops at the zoom past which nothing more is kept (at
//! [`DETAIL_ZOOM`] or above: a map draws the last zoom's tiles larger), at the limits' highest zoom, or before a zoom that
//! would pass a limit ([`TilerLimits`]). Before a zoom is made its cost is counted without making it: the feature and
//! tile pairs it would clip (each feature's tiles from its own points and segments, not only its box), the property text
//! those pairs carry and the positions they clip; a zoom over a per-zoom budget, or past the whole run's work, is not
//! made. A zoom is then made a tile at a time, its features clipped, encoded and compressed and dropped before the next
//! tile, so what is held is the features' simplified shapes (about the file's size), the zoom's pairs and the tiles made
//! (within the limits' bytes). The caller compresses each tile ([`build_tiles`]'s `compress`), is told the progress
//! every few tiles (and may stop the run there), and writes the archive (`crate::pmtiles::write_archive`).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use super::mvt::{encode_tile, MvtFeature, MvtGeometry, TileXy, EXTENT};
use super::simplify::{clip_line, clip_ring, douglas_peucker, simplify_ring, Rect, Xy};
use crate::error::Error;
use crate::pmtiles::tile_id;

/// The layer name of a generalized overlay's tiles.
pub const LAYER: &str = "overlay";
/// The zoom from which a zoom that keeps every position and point ends the tiles.
pub const DETAIL_ZOOM: u8 = 12;
/// The highest zoom tiles are made for.
pub const MAX_ZOOM: u8 = 14;
/// Web Mercator's latitude limit.
const MAX_LATITUDE: f64 = 85.051_128_779_806_59;
/// The buffer around a tile that features are clipped to, and the simplification tolerance, in tile units.
const BUFFER: f64 = 64.0;
const TOLERANCE: f64 = 8.0;
/// The cell a tile's points are thinned to (one point a cell): a pixel of a 256-pixel tile.
const POINT_CELL: i32 = 16;

/// A feature's geometry in Web Mercator's unit square (x east, y south, each from 0 to 1).
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Points(Vec<Xy>),
    Lines(Vec<Vec<Xy>>),
    /// Each polygon its rings, the outer ring first.
    Polygons(Vec<Vec<Vec<Xy>>>),
}

impl Shape {
    fn positions(&self) -> usize {
        match self {
            Self::Points(points) => points.len(),
            Self::Lines(lines) => lines.iter().map(Vec::len).sum(),
            Self::Polygons(polygons) => polygons.iter().flatten().map(Vec::len).sum(),
        }
    }
}

/// A feature to tile: its ID, shape, label and description, and the box of its shape (west, north, east, south).
#[derive(Clone, Debug, PartialEq)]
pub struct TileFeature {
    pub id: u64,
    pub shape: Shape,
    pub label: String,
    pub description: String,
    bbox: Rect,
}

/// A longitude and latitude in the unit square.
#[must_use]
pub fn project(lon: f64, lat: f64) -> Xy {
    let lat = lat.clamp(-MAX_LATITUDE, MAX_LATITUDE).to_radians();
    let x = (lon + 180.0) / 360.0;
    let y = 0.5 - (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln() / (2.0 * std::f64::consts::PI);
    [x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)]
}

fn position(value: &Value) -> Option<Xy> {
    let pair = value.as_array()?;
    let (lon, lat) = (pair.first()?.as_f64()?, pair.get(1)?.as_f64()?);
    (lon.is_finite() && lat.is_finite()).then(|| project(lon, lat))
}

fn positions(value: &Value) -> Option<Vec<Xy>> {
    value.as_array()?.iter().map(position).collect()
}

fn rings(value: &Value) -> Option<Vec<Vec<Xy>>> {
    value.as_array()?.iter().map(positions).collect()
}

impl TileFeature {
    /// A GeoJSON Feature (WGS84, as an overlay's reading gives it) as feature `id`; `None` for one with no geometry of a
    /// kind tiled (Point, LineString, Polygon and their Multi kinds).
    #[must_use]
    pub fn from_geojson(feature: &Value, id: u64) -> Option<Self> {
        let geometry = feature.get("geometry")?;
        let coordinates = geometry.get("coordinates")?;
        let shape = match geometry.get("type")?.as_str()? {
            "Point" => Shape::Points(vec![position(coordinates)?]),
            "MultiPoint" => Shape::Points(positions(coordinates)?),
            "LineString" => Shape::Lines(vec![positions(coordinates)?]),
            "MultiLineString" => Shape::Lines(rings(coordinates)?),
            "Polygon" => Shape::Polygons(vec![rings(coordinates)?]),
            "MultiPolygon" => Shape::Polygons(coordinates.as_array()?.iter().map(rings).collect::<Option<_>>()?),
            _ => return None,
        };
        let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        let mut grow = |point: &Xy| {
            bbox = [
                bbox[0].min(point[0]),
                bbox[1].min(point[1]),
                bbox[2].max(point[0]),
                bbox[3].max(point[1]),
            ];
        };
        match &shape {
            Shape::Points(points) => points.iter().for_each(&mut grow),
            Shape::Lines(lines) => lines.iter().flatten().for_each(&mut grow),
            Shape::Polygons(polygons) => polygons.iter().flatten().flatten().for_each(&mut grow),
        }
        if !bbox[0].is_finite() {
            return None;
        }
        let property = |name: &str| {
            feature
                .get("properties")
                .and_then(|properties| properties.get(name))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Some(Self {
            id,
            shape,
            label: property("label"),
            description: property("description"),
            bbox,
        })
    }
}

/// What tiling may make: the highest zoom, the most tiles and the most bytes (compressed) in all, and the budgets
/// counted before each zoom: its feature and tile pairs, the property text they carry, and the positions clipped over
/// the whole run.
#[derive(Clone, Copy, Debug)]
pub struct TilerLimits {
    pub max_zoom: u8,
    pub max_tiles: usize,
    pub max_bytes: usize,
    pub max_pairs: u64,
    pub max_property_bytes: u64,
    pub max_work: u64,
}

impl TilerLimits {
    /// The limits a server run takes: zoom 14, 50,000 tiles, 64 MiB, two million pairs and 256 MiB of property text a
    /// zoom, and 100 million positions clipped in all.
    #[must_use]
    pub fn standard() -> Self {
        Self {
            max_zoom: MAX_ZOOM,
            max_tiles: 50_000,
            max_bytes: 64 * 1024 * 1024,
            max_pairs: 2_000_000,
            max_property_bytes: 256 * 1024 * 1024,
            max_work: 100_000_000,
        }
    }
}

/// The tiles made: each by its tile ID (compressed), the highest zoom made, and their bytes; and why the run ended
/// before the limits' highest zoom, when it did.
#[derive(Debug, Default)]
pub struct TileSet {
    pub tiles: BTreeMap<u64, Vec<u8>>,
    pub max_zoom: u8,
    pub bytes: usize,
    pub stopped: Option<String>,
}

/// Where a run is, for its progress: the zoom being made, the highest it may make, and the tiles of the zoom made so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub zoom: u8,
    pub top: u8,
    pub tiles_done: usize,
    pub tiles: usize,
}

/// How often, in tiles, a zoom being made reports its progress.
const PROGRESS_TILES: usize = 64;

/// The tiles of zoom `z` that `bbox` touches with the buffer: x and y ranges.
fn tile_range(bbox: Rect, z: u8) -> (u32, u32, u32, u32) {
    let n = f64::from(1u32 << z);
    let buffer = BUFFER / f64::from(EXTENT);
    let cell = |value: f64| {
        // within 0 and 2^z - 1 by the clamp
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tile = (value * n).floor().clamp(0.0, n - 1.0) as u32;
        tile
    };
    let low = |value: f64| cell(value - buffer / n);
    let high = |value: f64| cell(value + buffer / n);
    (low(bbox[0]), low(bbox[1]), high(bbox[2]), high(bbox[3]))
}

fn range_count((x0, y0, x1, y1): (u32, u32, u32, u32)) -> u64 {
    (u64::from(x1 - x0) + 1) * (u64::from(y1 - y0) + 1)
}

fn box_of(points: &[Xy]) -> Rect {
    points.iter().fold(
        [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY],
        |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])],
    )
}

/// The tiles of zoom `z` a shape is clipped into: its box's for an area; for points each point's own; for lines each
/// segment's box, when those are fewer than the line's box (a long diagonal or a line across the antimeridian touches
/// far fewer tiles than its box holds). `count` is the same number worked out without listing the tiles, an upper bound
/// (a tile two points or segments share is counted for each).
enum Cover {
    Range((u32, u32, u32, u32)),
    Pieces(Vec<(u32, u32, u32, u32)>),
}

impl Cover {
    fn of(shape: &Shape, bbox: Rect, z: u8) -> Self {
        let whole = tile_range(bbox, z);
        let pieces: Vec<(u32, u32, u32, u32)> = match shape {
            Shape::Polygons(_) => return Self::Range(whole),
            Shape::Points(points) => points.iter().map(|p| tile_range([p[0], p[1], p[0], p[1]], z)).collect(),
            Shape::Lines(lines) => lines
                .iter()
                .flat_map(|line| line.windows(2).map(|pair| tile_range(box_of(pair), z)))
                .collect(),
        };
        let parts: u64 = pieces.iter().map(|piece| range_count(*piece)).sum();
        if parts < range_count(whole) {
            Self::Pieces(pieces)
        } else {
            Self::Range(whole)
        }
    }

    fn count(&self) -> u64 {
        match self {
            Self::Range(range) => range_count(*range),
            Self::Pieces(pieces) => pieces.iter().map(|piece| range_count(*piece)).sum(),
        }
    }

    fn for_each(&self, mut body: impl FnMut(u32, u32)) {
        let ranges: &[(u32, u32, u32, u32)] = match self {
            Self::Range(range) => std::slice::from_ref(range),
            Self::Pieces(pieces) => pieces,
        };
        for &(x0, y0, x1, y1) in ranges {
            for x in x0..=x1 {
                for y in y0..=y1 {
                    body(x, y);
                }
            }
        }
    }
}

/// The feature's shape simplified for zoom `z`.
fn simplified(shape: &Shape, z: u8) -> Shape {
    let tolerance = TOLERANCE / (f64::from(EXTENT) * f64::from(1u32 << z));
    match shape {
        Shape::Points(points) => Shape::Points(points.clone()),
        Shape::Lines(lines) => Shape::Lines(
            lines
                .iter()
                .map(|line| douglas_peucker(line, tolerance))
                .filter(|line| line.len() >= 2)
                .collect(),
        ),
        Shape::Polygons(polygons) => Shape::Polygons(
            polygons
                .iter()
                .filter_map(|rings| {
                    let mut kept = rings.iter().map(|ring| simplify_ring(ring, tolerance));
                    let outer = kept.next()??;
                    Some(std::iter::once(outer).chain(kept.flatten()).collect::<Vec<_>>())
                })
                .collect(),
        ),
    }
}

/// The shape's part in tile `x`/`y` of zoom `z`, in tile coordinates; `None` when nothing of it is there.
fn in_tile(shape: &Shape, z: u8, x: u32, y: u32) -> Option<MvtGeometry> {
    let n = f64::from(1u32 << z);
    let buffer = BUFFER / f64::from(EXTENT);
    let rect = [
        (f64::from(x) - buffer) / n,
        (f64::from(y) - buffer) / n,
        (f64::from(x) + 1.0 + buffer) / n,
        (f64::from(y) + 1.0 + buffer) / n,
    ];
    let extent = f64::from(EXTENT);
    let local = |point: &Xy| -> TileXy {
        // within the buffered tile by the clip, so well within an i32
        #[allow(clippy::cast_possible_truncation)]
        let at = (
            ((point[0] * n - f64::from(x)) * extent).round() as i32,
            ((point[1] * n - f64::from(y)) * extent).round() as i32,
        );
        at
    };
    let inside = |point: &Xy| point[0] >= rect[0] && point[0] <= rect[2] && point[1] >= rect[1] && point[1] <= rect[3];
    let geometry = match shape {
        Shape::Points(points) => {
            let kept: Vec<TileXy> = points.iter().filter(|point| inside(point)).map(local).collect();
            (!kept.is_empty()).then_some(MvtGeometry::Points(kept))?
        }
        Shape::Lines(lines) => {
            let kept: Vec<Vec<TileXy>> = lines
                .iter()
                .flat_map(|line| clip_line(line, rect))
                .map(|line| line.iter().map(local).collect())
                .collect();
            (!kept.is_empty()).then_some(MvtGeometry::Lines(kept))?
        }
        Shape::Polygons(polygons) => {
            let kept: Vec<Vec<Vec<TileXy>>> = polygons
                .iter()
                .filter_map(|rings| {
                    let mut clipped = rings.iter().map(|ring| clip_ring(ring, rect));
                    let outer = clipped.next()??;
                    Some(
                        std::iter::once(outer)
                            .chain(clipped.flatten())
                            .map(|ring| ring.iter().map(local).collect())
                            .collect(),
                    )
                })
                .collect();
            (!kept.is_empty()).then_some(MvtGeometry::Polygons(kept))?
        }
    };
    Some(geometry)
}

/// What a zoom would cost, counted before it is made, or why it may not be made.
fn zoom_cost(
    features: &[TileFeature],
    shapes: &[Shape],
    covers: &[Cover],
    limits: &TilerLimits,
    work_done: u64,
) -> Result<u64, String> {
    let (mut pairs, mut text, mut work) = (0u64, 0u64, 0u64);
    for ((feature, shape), cover) in features.iter().zip(shapes).zip(covers) {
        let tiles = cover.count();
        pairs = pairs.saturating_add(tiles);
        text = text.saturating_add(tiles.saturating_mul((feature.label.len() + feature.description.len()) as u64));
        work = work.saturating_add(tiles.saturating_mul(shape.positions() as u64));
    }
    if pairs > limits.max_pairs {
        return Err(format!(
            "it would clip {pairs} features into tiles, more than the {} a zoom may",
            limits.max_pairs
        ));
    }
    if text > limits.max_property_bytes {
        return Err(format!(
            "its tiles would carry {text} bytes of labels and descriptions, more than the {} a zoom may",
            limits.max_property_bytes
        ));
    }
    if work_done.saturating_add(work) > limits.max_work {
        return Err(format!(
            "it would pass the {} positions a run may clip",
            limits.max_work
        ));
    }
    Ok(work)
}

/// The tiles of `features` (see the module), each compressed by `compress`. `progress` is told where the run is every
/// few tiles and after each zoom, and answers whether to go on: false stops, and the answer is `Ok(None)`.
///
/// # Errors
///
/// When zoom 0 alone passes a limit: the file cannot be tiled within them.
pub fn build_tiles(
    features: &[TileFeature],
    limits: &TilerLimits,
    compress: &dyn Fn(&[u8]) -> Vec<u8>,
    progress: &mut dyn FnMut(Progress) -> bool,
) -> Result<Option<TileSet>, Error> {
    let mut set = TileSet::default();
    let top = limits.max_zoom.min(MAX_ZOOM);
    let positions: usize = features.iter().map(|feature| feature.shape.positions()).sum();
    let mut work_done = 0u64;
    let refuse = |z: u8, why: String| Error::new(format!("the overlay cannot be tiled at zoom {z}: {why}"));
    for z in 0..=top {
        let shapes: Vec<Shape> = features.iter().map(|feature| simplified(&feature.shape, z)).collect();
        let covers: Vec<Cover> = features
            .iter()
            .zip(&shapes)
            .map(|(feature, shape)| Cover::of(shape, feature.bbox, z))
            .collect();
        let work = match zoom_cost(features, &shapes, &covers, limits, work_done) {
            Ok(work) => work,
            Err(why) if z == 0 => return Err(refuse(z, why)),
            Err(why) => {
                set.stopped = Some(format!("zoom {z} was not made: {why}"));
                break;
            }
        };
        // the features of each tile, by index: as many entries as the pairs counted, at most
        let mut by_tile: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        for (index, cover) in covers.iter().enumerate() {
            let index = u32::try_from(index).map_err(|_| Error::new("too many features"))?;
            cover.for_each(|x, y| {
                let list = by_tile.entry((x, y)).or_default();
                if list.last() != Some(&index) {
                    list.push(index);
                }
            });
        }
        drop(covers);
        if set.tiles.len() + by_tile.len() > limits.max_tiles {
            let why = format!("it would pass the {} tiles allowed", limits.max_tiles);
            if z == 0 {
                return Err(refuse(z, why));
            }
            set.stopped = Some(format!("zoom {z} was not made: {why}"));
            break;
        }
        let mut order: Vec<(u64, u32, u32)> = Vec::with_capacity(by_tile.len());
        for &(x, y) in by_tile.keys() {
            order.push((tile_id(z, x, y)?, x, y));
        }
        order.sort_unstable();
        let tiles = order.len();
        let kept: usize = shapes.iter().map(Shape::positions).sum();
        let mut thinned = 0usize;
        let mut made: Vec<(u64, Vec<u8>)> = Vec::with_capacity(tiles);
        let mut bytes = 0usize;
        let mut over = None;
        for (done, (id, x, y)) in order.into_iter().enumerate() {
            if done % PROGRESS_TILES == 0
                && done > 0
                && !progress(Progress {
                    zoom: z,
                    top,
                    tiles_done: done,
                    tiles,
                })
            {
                return Ok(None);
            }
            let Some(indexes) = by_tile.remove(&(x, y)) else {
                continue;
            };
            let mut cells: HashSet<TileXy> = HashSet::new();
            let mut tile_features: Vec<MvtFeature<'_>> = Vec::with_capacity(indexes.len());
            for index in indexes {
                let index = index as usize;
                let feature = &features[index];
                let Some(mut geometry) = in_tile(&shapes[index], z, x, y) else {
                    continue;
                };
                if let MvtGeometry::Points(points) = &mut geometry {
                    let before = points.len();
                    points.retain(|&(px, py)| cells.insert((px.div_euclid(POINT_CELL), py.div_euclid(POINT_CELL))));
                    thinned += before - points.len();
                    if points.is_empty() {
                        continue;
                    }
                }
                let mut properties = vec![("label", feature.label.as_str())];
                if !feature.description.is_empty() {
                    properties.push(("description", feature.description.as_str()));
                }
                tile_features.push(MvtFeature {
                    id: feature.id,
                    geometry,
                    properties,
                });
            }
            let tile = encode_tile(LAYER, &tile_features);
            drop(tile_features);
            if tile.is_empty() {
                continue;
            }
            let tile = compress(&tile);
            bytes += tile.len();
            if set.bytes + bytes > limits.max_bytes {
                over = Some(format!("its tiles would pass the {} bytes allowed", limits.max_bytes));
                break;
            }
            made.push((id, tile));
        }
        if let Some(why) = over {
            if z == 0 {
                return Err(refuse(z, why));
            }
            set.stopped = Some(format!("zoom {z} was not made: {why}"));
            break;
        }
        work_done += work;
        set.bytes += bytes;
        set.tiles.extend(made);
        set.max_zoom = z;
        if !progress(Progress {
            zoom: z,
            top,
            tiles_done: tiles,
            tiles,
        }) {
            return Ok(None);
        }
        // every position and point kept at this zoom: a deeper zoom adds nothing a map would not draw from this one
        if z >= DETAIL_ZOOM && kept == positions && thinned == 0 {
            break;
        }
    }
    Ok(Some(set))
}

#[cfg(test)]
#[path = "tiles_tests.rs"]
mod tests;
