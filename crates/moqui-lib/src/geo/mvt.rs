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

//! A Mapbox Vector Tile writer (version 2, <https://github.com/mapbox/vector-tile-spec/tree/master/2.1>): one layer of
//! features in tile coordinates with string properties, encoded as the spec's protobuf message by hand (a tile is a few
//! length-delimited fields and packed varints), for an overlay generalized into tiles on the server (plans/archive/Maps.md §7;
//! [`super::tiles`]). Written here rather than on the `mvt` crate (MIT or Apache-2.0), which brings a protobuf runtime
//! and its code generator's output for these few fields.

use std::collections::HashMap;

/// The tile's coordinate extent: positions from 0 to 4096 across the tile (and a little past it, the buffer).
pub const EXTENT: u32 = 4096;

/// A position in tile coordinates (y down).
pub type TileXy = (i32, i32);

/// A feature's geometry in tile coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MvtGeometry {
    Points(Vec<TileXy>),
    Lines(Vec<Vec<TileXy>>),
    /// Each polygon its rings, the outer ring first: closed or not, in either direction (the writer orients them).
    Polygons(Vec<Vec<Vec<TileXy>>>),
}

/// A feature of the layer: its ID, geometry and string properties (borrowed: a feature in many tiles shares its text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MvtFeature<'a> {
    pub id: u64,
    pub geometry: MvtGeometry,
    pub properties: Vec<(&'a str, &'a str)>,
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7f).unwrap_or_default() | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).unwrap_or_default());
}

fn key(out: &mut Vec<u8>, field: u32, wire: u32) {
    varint(out, u64::from((field << 3) | wire));
}

fn bytes_field(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    key(out, field, 2);
    varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn zigzag(value: i32) -> u32 {
    // the spec's (n << 1) ^ (n >> 31): the sign bit to the low bit
    #[allow(clippy::cast_sign_loss)]
    let encoded = ((value << 1) ^ (value >> 31)) as u32;
    encoded
}

fn command(id: u32, count: usize) -> u32 {
    (id & 0x7) | (u32::try_from(count).unwrap_or(0) << 3)
}

/// Twice the ring's area by the surveyor's formula in tile coordinates: positive for the spec's outer ring (clockwise
/// on screen, y down).
fn doubled_area(ring: &[TileXy]) -> i64 {
    let mut sum = 0i64;
    for index in 0..ring.len() {
        let (x0, y0) = ring[index];
        let (x1, y1) = ring[(index + 1) % ring.len()];
        sum += i64::from(x0) * i64::from(y1) - i64::from(x1) * i64::from(y0);
    }
    sum
}

/// The ring open (no closing point), with no repeated position, or `None` with fewer than three points or no area.
fn open_ring(ring: &[TileXy]) -> Option<Vec<TileXy>> {
    let mut points = ring.to_vec();
    points.dedup();
    if points.len() > 1 && points[0] == points[points.len() - 1] {
        points.pop();
    }
    (points.len() >= 3 && doubled_area(&points) != 0).then_some(points)
}

/// The geometry's command stream and its type (1 point, 2 line, 3 polygon); `None` when nothing of it is drawable.
fn geometry_commands(geometry: &MvtGeometry) -> Option<(Vec<u32>, u64)> {
    let mut out = Vec::new();
    let mut cursor: TileXy = (0, 0);
    let moves = |out: &mut Vec<u32>, points: &[TileXy], cursor: &mut TileXy| {
        for &(x, y) in points {
            out.push(zigzag(x.wrapping_sub(cursor.0)));
            out.push(zigzag(y.wrapping_sub(cursor.1)));
            *cursor = (x, y);
        }
    };
    let kind = match geometry {
        MvtGeometry::Points(points) => {
            if points.is_empty() {
                return None;
            }
            out.push(command(1, points.len()));
            moves(&mut out, points, &mut cursor);
            1
        }
        MvtGeometry::Lines(lines) => {
            for line in lines {
                let mut line = line.clone();
                line.dedup();
                if line.len() < 2 {
                    continue;
                }
                out.push(command(1, 1));
                moves(&mut out, &line[..1], &mut cursor);
                out.push(command(2, line.len() - 1));
                moves(&mut out, &line[1..], &mut cursor);
            }
            2
        }
        MvtGeometry::Polygons(polygons) => {
            for polygon in polygons {
                // a polygon whose outer ring is not drawable is left out: a hole is never made its outer ring
                let mut rings = polygon.iter();
                let Some(outer) = rings.next().and_then(|ring| open_ring(ring)) else {
                    continue;
                };
                let rings = rings.filter_map(|ring| open_ring(ring));
                for (index, mut ring) in std::iter::once(outer).chain(rings).enumerate() {
                    // the outer ring's area positive, each hole's negative
                    if (doubled_area(&ring) > 0) != (index == 0) {
                        ring.reverse();
                    }
                    out.push(command(1, 1));
                    moves(&mut out, &ring[..1], &mut cursor);
                    out.push(command(2, ring.len() - 1));
                    moves(&mut out, &ring[1..], &mut cursor);
                    out.push(command(7, 1));
                }
            }
            3
        }
    };
    (!out.is_empty()).then_some((out, kind))
}

/// A tile of one layer `name` of `features`: the protobuf bytes of the spec's `Tile`, with keys and values shared by
/// the features. A feature with nothing drawable is left out; no feature at all is an empty tile (no bytes).
#[must_use]
pub fn encode_tile<'a>(name: &str, features: &[MvtFeature<'a>]) -> Vec<u8> {
    let mut keys: Vec<&'a str> = Vec::new();
    let mut key_index: HashMap<&'a str, u32> = HashMap::new();
    let mut values: Vec<&'a str> = Vec::new();
    let mut value_index: HashMap<&'a str, u32> = HashMap::new();
    let mut encoded_features: Vec<Vec<u8>> = Vec::new();
    for feature in features {
        let Some((commands, kind)) = geometry_commands(&feature.geometry) else {
            continue;
        };
        let mut tags = Vec::new();
        for &(name, value) in &feature.properties {
            if value.is_empty() {
                continue;
            }
            let k = *key_index.entry(name).or_insert_with(|| {
                keys.push(name);
                u32::try_from(keys.len() - 1).unwrap_or(u32::MAX)
            });
            let v = *value_index.entry(value).or_insert_with(|| {
                values.push(value);
                u32::try_from(values.len() - 1).unwrap_or(u32::MAX)
            });
            varint(&mut tags, u64::from(k));
            varint(&mut tags, u64::from(v));
        }
        let mut body = Vec::new();
        key(&mut body, 1, 0);
        varint(&mut body, feature.id);
        if !tags.is_empty() {
            bytes_field(&mut body, 2, &tags);
        }
        key(&mut body, 3, 0);
        varint(&mut body, kind);
        let mut packed = Vec::with_capacity(commands.len() * 2);
        for value in commands {
            varint(&mut packed, u64::from(value));
        }
        bytes_field(&mut body, 4, &packed);
        encoded_features.push(body);
    }
    if encoded_features.is_empty() {
        return Vec::new();
    }
    let mut layer = Vec::new();
    key(&mut layer, 15, 0);
    varint(&mut layer, 2);
    bytes_field(&mut layer, 1, name.as_bytes());
    for feature in &encoded_features {
        bytes_field(&mut layer, 2, feature);
    }
    for name in keys {
        bytes_field(&mut layer, 3, name.as_bytes());
    }
    for value in values {
        let mut message = Vec::new();
        bytes_field(&mut message, 1, value.as_bytes());
        bytes_field(&mut layer, 4, &message);
    }
    key(&mut layer, 5, 0);
    varint(&mut layer, u64::from(EXTENT));
    let mut tile = Vec::with_capacity(layer.len() + 8);
    bytes_field(&mut tile, 3, &layer);
    tile
}

#[cfg(test)]
#[path = "mvt_tests.rs"]
mod tests;
