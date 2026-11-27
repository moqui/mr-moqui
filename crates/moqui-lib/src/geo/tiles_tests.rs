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

use serde_json::json;

use super::*;

fn features() -> Vec<TileFeature> {
    let square = json!({"type": "Feature", "properties": {"label": "Square", "description": "A park"},
        "geometry": {"type": "Polygon", "coordinates": [[[10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [10.0, 20.0], [10.0, 10.0]]]}});
    let line = json!({"type": "Feature", "properties": {"label": "Road"},
        "geometry": {"type": "LineString", "coordinates": [[-50.0, -10.0], [-40.0, -12.0], [-30.0, -10.0]]}});
    let points = json!({"type": "Feature", "properties": {"label": "Stops"},
        "geometry": {"type": "MultiPoint", "coordinates": [[100.0, 0.0], [100.000_001, 0.0]]}});
    let other =
        json!({"type": "Feature", "properties": {}, "geometry": {"type": "GeometryCollection", "geometries": []}});
    let mut out: Vec<TileFeature> = [square, line, points]
        .iter()
        .enumerate()
        .map(|(index, feature)| TileFeature::from_geojson(feature, index as u64 + 1).unwrap())
        .collect();
    assert!(TileFeature::from_geojson(&other, 9).is_none());
    out.truncate(3);
    out
}

fn limits(max_zoom: u8) -> TilerLimits {
    TilerLimits {
        max_zoom,
        max_tiles: 10_000,
        max_bytes: 10 * 1024 * 1024,
        ..TilerLimits::standard()
    }
}

fn go_on(_: Progress) -> bool {
    true
}

#[test]
fn projects_to_the_unit_square() {
    assert_eq!(project(-180.0, 0.0), [0.0, 0.5]);
    let [x, y] = project(0.0, 0.0);
    assert!((x - 0.5).abs() < 1e-12 && (y - 0.5).abs() < 1e-12);
    assert_eq!(project(0.0, 90.0)[1], 0.0);
    assert!(project(0.0, 45.0)[1] < 0.5);
}

#[test]
fn reads_a_feature_with_its_label() {
    let features = features();
    assert_eq!(features[0].label, "Square");
    assert_eq!(features[0].description, "A park");
    assert!(matches!(&features[1].shape, Shape::Lines(lines) if lines[0].len() == 3));
}

#[test]
fn tiles_each_zoom_within_the_features_tiles() {
    let mut zooms = Vec::new();
    let set = build_tiles(&features(), &limits(3), &|tile| tile.to_vec(), &mut |at: Progress| {
        if at.tiles_done == at.tiles {
            zooms.push((at.zoom, at.top));
        }
        true
    })
    .unwrap()
    .unwrap();
    assert_eq!(set.max_zoom, 3);
    assert_eq!(zooms, vec![(0, 3), (1, 3), (2, 3), (3, 3)]);
    // the world tile, and at zoom 1 the square's north east quarter, the road's south west and the stops' south east
    assert!(set.tiles.contains_key(&0));
    assert!(set.tiles.contains_key(&tile_id(1, 1, 0).unwrap()));
    assert!(set.tiles.contains_key(&tile_id(1, 0, 1).unwrap()));
    assert!(set.tiles.contains_key(&tile_id(1, 1, 1).unwrap()));
    assert!(!set.tiles.contains_key(&tile_id(1, 0, 0).unwrap()));
    // each tile names the layer
    assert!(set.tiles.values().all(|tile| tile.windows(7).any(|w| w == b"overlay")));
    assert_eq!(set.bytes, set.tiles.values().map(Vec::len).sum::<usize>());
}

#[test]
fn stops_at_the_limits_or_when_told() {
    // tiles: zooms 0 and 1 make 4; zoom 2 would pass 5
    let few = TilerLimits {
        max_tiles: 5,
        ..limits(8)
    };
    let set = build_tiles(&features(), &few, &|tile| tile.to_vec(), &mut go_on)
        .unwrap()
        .unwrap();
    assert_eq!(set.max_zoom, 1);
    let tiny = TilerLimits {
        max_bytes: 10,
        ..limits(8)
    };
    assert!(build_tiles(&features(), &tiny, &|tile| tile.to_vec(), &mut go_on).is_err());
    let cancelled = build_tiles(&features(), &limits(8), &|tile| tile.to_vec(), &mut |at: Progress| {
        at.zoom < 1
    })
    .unwrap();
    assert!(cancelled.is_none());
}

#[test]
fn ends_once_a_zoom_keeps_everything() {
    // one point is kept whole at every zoom: the tiles end at the detail zoom
    let point = TileFeature::from_geojson(
        &json!({"type": "Feature", "properties": {"label": "Here"}, "geometry": {"type": "Point", "coordinates": [2.35, 48.85]}}),
        1,
    )
    .unwrap();
    let set = build_tiles(&[point], &limits(14), &|tile| tile.to_vec(), &mut go_on)
        .unwrap()
        .unwrap();
    assert_eq!(set.max_zoom, DETAIL_ZOOM);
    // a tile a zoom, and its neighbor where the point is within the buffer of their edge
    assert!(set.tiles.len() > usize::from(DETAIL_ZOOM) && set.tiles.len() < 2 * (usize::from(DETAIL_ZOOM) + 1));
}

fn polygon(west: f64, south: f64, east: f64, north: f64) -> serde_json::Value {
    json!({"type": "Feature", "properties": {"label": "Area"}, "geometry": {"type": "Polygon",
        "coordinates": [[[west, south], [east, south], [east, north], [west, north], [west, south]]]}})
}

#[test]
fn a_pathological_file_stops_within_its_budget_quickly() {
    // 20,000 small polygons each spanning the world: zoom 4 alone would clip 5 million features into tiles
    let features: Vec<TileFeature> = (0..20_000u32)
        .map(|index| {
            let offset = f64::from(index % 100) / 1000.0;
            TileFeature::from_geojson(&polygon(-179.0 + offset, -80.0, 179.0 - offset, 80.0), u64::from(index)).unwrap()
        })
        .collect();
    let budget = TilerLimits {
        max_pairs: 200_000,
        ..TilerLimits::standard()
    };
    let started = std::time::Instant::now();
    let set = build_tiles(&features, &budget, &|tile| tile.to_vec(), &mut go_on)
        .unwrap()
        .unwrap();
    // zooms 0 to 2 clip 20,000, 80,000 and 320,000: zoom 2 is not made
    assert_eq!(set.max_zoom, 1);
    assert!(
        set.stopped
            .as_deref()
            .is_some_and(|why| why.contains("zoom 2") && why.contains("200000")),
        "{:?}",
        set.stopped
    );
    assert!(started.elapsed().as_secs() < 20, "{:?}", started.elapsed());
    // the work in all: zoom 0's alone past it refuses the file
    let little = TilerLimits {
        max_work: 1000,
        ..TilerLimits::standard()
    };
    assert!(build_tiles(&features, &little, &|tile| tile.to_vec(), &mut go_on).is_err());
}

#[test]
fn a_zoom_being_made_can_be_stopped() {
    let features: Vec<TileFeature> = vec![TileFeature::from_geojson(&polygon(-170.0, -80.0, 170.0, 80.0), 1).unwrap()];
    // zoom 4 has 256 tiles: told of its progress every 64, and stopped there
    let mut told = Vec::new();
    let stopped = build_tiles(&features, &limits(6), &|tile| tile.to_vec(), &mut |at: Progress| {
        told.push((at.zoom, at.tiles_done));
        !(at.zoom == 4 && at.tiles_done == 64)
    })
    .unwrap();
    assert!(stopped.is_none());
    assert!(told.contains(&(4, 64)) && !told.iter().any(|(z, _)| *z > 4));
}

#[test]
fn points_lines_and_the_poles_and_antimeridian_count_their_own_tiles() {
    // two points at opposite ends of the world and a line across the antimeridian: their tiles, not their boxes
    let points = TileFeature::from_geojson(
        &json!({"type": "Feature", "properties": {"label": "Ends"},
            "geometry": {"type": "MultiPoint", "coordinates": [[-179.9, 89.9], [179.9, -89.9]]}}),
        1,
    )
    .unwrap();
    let line = TileFeature::from_geojson(
        &json!({"type": "Feature", "properties": {"label": "Dateline"},
            "geometry": {"type": "LineString", "coordinates": [[179.0, 10.0], [179.9, 10.0], [-179.9, 11.0], [-179.0, 11.0]]}}),
        2,
    )
    .unwrap();
    let z = 10;
    let point_cover = Cover::of(&points.shape, points.bbox, z);
    assert!(matches!(point_cover, Cover::Pieces(_)) && point_cover.count() <= 8);
    // the middle segment crosses the map: the line's own tiles are its segments', a row across the world
    let line_cover = Cover::of(&line.shape, line.bbox, z);
    assert!(line_cover.count() <= 4 * 1024 + 8, "{}", line_cover.count());
    let budget = TilerLimits {
        max_pairs: 50_000,
        ..limits(14)
    };
    let set = build_tiles(&[points, line], &budget, &|tile| tile.to_vec(), &mut go_on)
        .unwrap()
        .unwrap();
    // the poles are clamped to the map's corners: their tiles are the first and last of each zoom
    assert!(set.tiles.contains_key(&tile_id(3, 0, 0).unwrap()));
    assert!(set.tiles.contains_key(&tile_id(3, 7, 7).unwrap()));
    assert!(set.max_zoom >= 3);
}
