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

use super::*;

/// A protobuf message's fields: (field, varint value or bytes)
fn fields(bytes: &[u8]) -> Vec<(u32, Result<u64, Vec<u8>>)> {
    let mut at = 0;
    let read = |at: &mut usize| {
        let (mut value, mut shift) = (0u64, 0);
        loop {
            let byte = bytes[*at];
            *at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
            shift += 7;
        }
    };
    let mut out = Vec::new();
    while at < bytes.len() {
        let key = read(&mut at);
        let field = u32::try_from(key >> 3).unwrap();
        if key & 7 == 0 {
            out.push((field, Ok(read(&mut at))));
        } else {
            let length = usize::try_from(read(&mut at)).unwrap();
            out.push((field, Err(bytes[at..at + length].to_vec())));
            at += length;
        }
    }
    out
}

fn packed(bytes: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let (mut value, mut shift) = (0u32, 0);
    for &byte in bytes {
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            out.push(value);
            value = 0;
            shift = 0;
        } else {
            shift += 7;
        }
    }
    out
}

fn bytes_of(fields: &[(u32, Result<u64, Vec<u8>>)], field: u32) -> Vec<Vec<u8>> {
    fields
        .iter()
        .filter(|(f, _)| *f == field)
        .filter_map(|(_, value)| value.clone().err())
        .collect()
}

#[test]
fn writes_a_layer_of_a_polygon_with_its_properties() {
    // a square drawn counter-clockwise on screen: written clockwise, the spec's outer ring
    let square = vec![(0, 0), (0, 10), (10, 10), (10, 0), (0, 0)];
    let feature = MvtFeature {
        id: 7,
        geometry: MvtGeometry::Polygons(vec![vec![square]]),
        properties: vec![("label", "Park"), ("description", "")],
    };
    let tile = encode_tile("overlay", &[feature]);
    let layers = bytes_of(&fields(&tile), 3);
    assert_eq!(layers.len(), 1);
    let layer = fields(&layers[0]);
    assert!(layer.contains(&(15, Ok(2))) && layer.contains(&(5, Ok(4096))));
    assert_eq!(bytes_of(&layer, 1), vec![b"overlay".to_vec()]);
    // the empty description is no property
    assert_eq!(bytes_of(&layer, 3), vec![b"label".to_vec()]);
    let feature = fields(&bytes_of(&layer, 2)[0]);
    assert!(feature.contains(&(1, Ok(7))) && feature.contains(&(3, Ok(3))));
    assert_eq!(packed(&bytes_of(&feature, 2)[0]), vec![0, 0]);
    // reversed: MoveTo (10,0), LineTo 3: (0,+10) (-10,0) (0,-10), ClosePath
    assert_eq!(
        packed(&bytes_of(&feature, 4)[0]),
        vec![9, 20, 0, 26, 0, 20, 19, 0, 0, 19, 15]
    );
}

#[test]
fn points_and_lines_and_what_is_not_drawn() {
    let points = MvtFeature {
        id: 1,
        geometry: MvtGeometry::Points(vec![(5, 5), (3, 8)]),
        properties: Vec::new(),
    };
    let line = MvtFeature {
        id: 2,
        geometry: MvtGeometry::Lines(vec![vec![(0, 0), (0, 0), (4, 0)], vec![(9, 9)]]),
        properties: Vec::new(),
    };
    let sliver = MvtFeature {
        id: 3,
        geometry: MvtGeometry::Polygons(vec![vec![vec![(0, 0), (5, 0), (10, 0)]]]),
        properties: Vec::new(),
    };
    let tile = encode_tile("overlay", &[points, line, sliver]);
    let layer = fields(&bytes_of(&fields(&tile), 3)[0]);
    let features = bytes_of(&layer, 2);
    assert_eq!(features.len(), 2, "the sliver has no area");
    // MoveTo 2: (5,5), then (-2,+3)
    assert_eq!(packed(&bytes_of(&fields(&features[0]), 4)[0]), vec![17, 10, 10, 3, 6]);
    // the repeated point dropped, the one-point line left out: MoveTo (0,0) LineTo (4,0)
    assert_eq!(packed(&bytes_of(&fields(&features[1]), 4)[0]), vec![9, 0, 0, 10, 8, 0]);
    assert!(encode_tile("overlay", &[]).is_empty());
}

#[test]
fn a_polygon_whose_outer_ring_is_not_drawn_is_left_out() {
    // the outer ring has no area: its hole is never drawn as an outer ring
    let polygon = MvtFeature {
        id: 4,
        geometry: MvtGeometry::Polygons(vec![vec![
            vec![(0, 0), (5, 0), (10, 0)],
            vec![(2, 2), (2, 8), (8, 8), (8, 2)],
        ]]),
        properties: Vec::new(),
    };
    assert!(encode_tile("overlay", &[polygon]).is_empty());
}
