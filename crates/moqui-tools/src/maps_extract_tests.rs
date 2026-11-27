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

use moqui_core::resource::source::MemorySource;
use moqui_lib::pmtiles::{decode_directory, tile_zxy, TileType};

use super::*;

const SEA: u64 = u64::MAX;

/// An archive of every tile from zoom 0 to 4, gzipped directories: a tile's content names it, but the tiles of the
/// lower half of each zoom share one "sea" content (stored once, as a source's sea is), pinned by `tag`
fn archive(tag: &str) -> (MemorySource, u64) {
    let mut writer = ArchiveWriter::new(Vec::new());
    let mut ids: Vec<(u64, u8, u32, u32)> = Vec::new();
    for z in 0..=4u8 {
        for x in 0..(1u32 << z) {
            for y in 0..(1u32 << z) {
                ids.push((tile_id(z, x, y).unwrap(), z, x, y));
            }
        }
    }
    ids.sort_unstable();
    for (id, z, x, y) in ids {
        if z > 0 && y >= (1u32 << z) / 2 {
            writer.add_keyed(id, SEA, b"sea").unwrap();
        } else {
            writer.add_tile(id, format!("tile {z}/{x}/{y}").as_bytes()).unwrap();
        }
    }
    let options = ArchiveOptions {
        tile_type: TileType::Mvt,
        tile_compression: Compression::None,
        internal_compression: Compression::Gzip,
        min_zoom: 0,
        max_zoom: 4,
        bounds: [-180.0, -85.0, 180.0, 85.0],
        center_zoom: 2,
    };
    let metadata = br#"{"name":"spec","attribution":"Spec data","vector_layers":[{"id":"earth","fields":{"kind":"String"}}],"extra":{"kept":true}}"#;
    let finished = writer.finish(metadata, &options, &gzip).unwrap();
    let mut bytes = finished.prefix;
    bytes.extend_from_slice(&finished.data);
    let size = bytes.len() as u64;
    (MemorySource::new(bytes).with_tag(tag), size)
}

/// The tiles of an archive's bytes, by tile ID, read through its directories
fn tiles_of(archive: &[u8]) -> (Header, BTreeMap<u64, Vec<u8>>, Vec<u8>) {
    let header = Header::parse(archive).unwrap();
    assert!(
        header.problems(archive.len() as u64).is_empty(),
        "{:?}",
        header.problems(archive.len() as u64)
    );
    let section = |offset: u64, length: u64| {
        let start = usize::try_from(offset).unwrap();
        archive[start..start + usize::try_from(length).unwrap()].to_vec()
    };
    let gunzip = |bytes: Vec<u8>| decompress(bytes, header.internal_compression, 1 << 24, "spec").unwrap();
    let mut out = BTreeMap::new();
    let mut directories = vec![(header.root_offset, header.root_length)];
    while let Some((offset, length)) = directories.pop() {
        for entry in decode_directory(&gunzip(section(offset, length))).unwrap() {
            if entry.is_leaf() {
                directories.push((header.leaf_offset + entry.offset, u64::from(entry.length)));
                continue;
            }
            let content = section(header.tile_data_offset + entry.offset, u64::from(entry.length));
            for run in 0..u64::from(entry.run_length) {
                out.insert(entry.tile_id + run, content.clone());
            }
        }
    }
    let metadata = gunzip(section(header.metadata_offset, header.metadata_length));
    (header, out, metadata)
}

fn target(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-maps-extract-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{name}.pmtiles"))
}

fn cut(bbox: Bbox, min_zoom: u8, max_zoom: u8) -> Cut {
    Cut {
        bbox,
        min_zoom,
        max_zoom,
        max_tiles: MAX_TILES,
        max_bytes: 0,
    }
}

fn leftovers(path: &Path) -> Vec<String> {
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|found| found.starts_with(&name))
        .collect()
}

#[test]
fn the_tiles_of_a_box_at_zoom_0_are_the_world_tile() {
    let anywhere = Bbox::new(11.1, 43.7, 11.4, 43.9).unwrap();
    assert_eq!(bbox_tile_ids(&anywhere, 0).unwrap(), vec![0]);
    let across = Bbox::new(170.0, -10.0, -170.0, 10.0).unwrap();
    assert!(across.crosses_antimeridian());
    assert_eq!(bbox_tiles(&across, 0), (vec![(0, 0)], (0, 0)));
    assert_eq!(bbox_tile_ids(&across, 0).unwrap(), vec![0]);
    assert_eq!(bbox_tile_count(&across, 0), 1);
}

#[test]
fn a_box_across_the_antimeridian_takes_the_columns_on_both_sides() {
    let across = Bbox::new(170.0, -10.0, -170.0, 10.0).unwrap();
    // zoom 3: eight columns; 170 is in the last and -170 in the first, nothing between
    let (columns, (top, bottom)) = bbox_tiles(&across, 3);
    assert_eq!(columns, vec![(0, 0), (7, 7)]);
    assert_eq!((top, bottom), (3, 4));
    let ids = bbox_tile_ids(&across, 3).unwrap();
    assert_eq!(ids.len(), 4);
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "sorted by tile ID");
    let columns_of: Vec<u32> = ids.iter().map(|id| tile_zxy(*id).unwrap().1).collect();
    assert!(columns_of.iter().all(|x| *x == 0 || *x == 7), "{columns_of:?}");
    // zoom 1: the two halves meet, one run of both columns
    assert_eq!(bbox_tiles(&across, 1).0, vec![(0, 1)]);
    // the same width not across: the columns between
    let between = Bbox::new(-170.0, -10.0, 170.0, 10.0).unwrap();
    assert_eq!(bbox_tiles(&between, 3).0, vec![(0, 7)]);
}

#[test]
fn a_box_edge_on_a_tile_edge_does_not_take_the_tile_beyond() {
    // 0 to 90 degrees east is the third column of four at zoom 2, not the fourth
    let quarter = Bbox::new(0.0, 0.0, 90.0, 60.0).unwrap();
    assert_eq!(bbox_tiles(&quarter, 2).0, vec![(2, 2)]);
    // the equator is a row edge: the south edge on it ends at the row above
    assert_eq!(bbox_tiles(&quarter, 1).1, (0, 0));
    // the poles are cut at Web Mercator's edge
    let world = Bbox::new(-180.0, -90.0, 180.0, 90.0).unwrap();
    assert_eq!(bbox_tile_count(&world, 3), 64);
}

#[test]
fn a_box_that_is_no_box_is_refused() {
    assert!(Bbox::new(10.0, 5.0, 20.0, 5.0).is_err());
    assert!(Bbox::new(10.0, 6.0, 20.0, 5.0).is_err());
    assert!(Bbox::new(10.0, 0.0, 10.0, 5.0).is_err());
    assert!(Bbox::new(181.0, 0.0, 10.0, 5.0).is_err());
    assert!(Bbox::new(0.0, -91.0, 10.0, 5.0).is_err());
    assert!(Bbox::new(f64::NAN, 0.0, 10.0, 5.0).is_err());
}

#[test]
fn an_extract_holds_the_tiles_of_its_box_and_zooms_and_no_others() {
    let (source, size) = archive("sha256-extract-bounds");
    // the north-east quarter's upper half, zooms 1 to 3
    let bbox = Bbox::new(1.0, 1.0, 179.0, 80.0).unwrap();
    let asked = cut(bbox, 1, 3);
    let estimated = estimate(&source, "sha256-extract-bounds", size, &asked)
        .unwrap()
        .unwrap();
    let path = target("bounds");
    let mut calls = 0;
    let made = extract(
        &source,
        "sha256-extract-bounds",
        size,
        &asked,
        &path,
        &mut || true,
        &mut |_, _| {
            calls += 1;
            true
        },
    )
    .unwrap()
    .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(made.size, bytes.len() as u64);
    assert_eq!(made.sha256.len(), 64);
    let (header, tiles, metadata) = tiles_of(&bytes);
    assert_eq!((header.min_zoom, header.max_zoom), (1, 3));
    assert_eq!((made.min_zoom, made.max_zoom), (1, 3));
    assert!((header.min_lon - 1.0).abs() < 1e-6 && (header.max_lat - 80.0).abs() < 1e-6);
    // the source's metadata copied whole, what the parsed subset leaves out too
    let source_metadata: serde_json::Value = serde_json::from_slice(&metadata).unwrap();
    assert_eq!(source_metadata["extra"]["kept"], serde_json::Value::Bool(true));
    assert_eq!((made.attribution.as_str(), made.license.as_str()), ("Spec data", ""));
    let mut expected: Vec<u64> = (1..=3).flat_map(|z| bbox_tile_ids(&bbox, z).unwrap()).collect();
    expected.sort_unstable();
    assert_eq!(tiles.keys().copied().collect::<Vec<_>>(), expected);
    for (id, content) in &tiles {
        let (z, x, y) = tile_zxy(*id).unwrap();
        assert!((1..=3).contains(&z));
        assert_eq!(content, format!("tile {z}/{x}/{y}").as_bytes(), "the tile as stored");
    }
    assert_eq!(made.tiles, estimated.tiles);
    assert_eq!(made.tiles, expected.len() as u64);
    // neighbouring tiles are read together: fewer reads than tiles
    assert!(made.reads < made.tiles, "{} reads of {} tiles", made.reads, made.tiles);
    assert!(calls >= 2);
    assert!(
        leftovers(&path) == vec!["bounds.pmtiles".to_string()],
        "{:?}",
        leftovers(&path)
    );
}

#[test]
fn a_shared_content_is_cut_once_and_a_box_across_the_antimeridian_is_whole() {
    let (source, size) = archive("sha256-extract-sea");
    // the south of both sides of the antimeridian: all sea
    let bbox = Bbox::new(100.0, -60.0, -100.0, -5.0).unwrap();
    let path = target("sea");
    let made = extract(
        &source,
        "sha256-extract-sea",
        size,
        &cut(bbox, 2, 4),
        &path,
        &mut || true,
        &mut |_, _| true,
    )
    .unwrap()
    .unwrap();
    let (header, tiles, _) = tiles_of(&std::fs::read(&path).unwrap());
    assert_eq!(header.tile_contents, 1);
    assert_eq!(made.contents, 1);
    assert!(tiles.values().all(|content| content == b"sea"));
    // both sides, and the bounds of the whole width
    let columns: HashSet<u32> = tiles
        .keys()
        .filter_map(|id| tile_zxy(*id).ok())
        .filter(|(z, ..)| *z == 4)
        .map(|(_, x, _)| x)
        .collect();
    assert!(
        columns.contains(&0) && columns.contains(&15) && !columns.contains(&8),
        "{columns:?}"
    );
    assert!((header.min_lon + 180.0).abs() < 1e-6 && (header.max_lon - 180.0).abs() < 1e-6);
    let estimated = estimate(&source, "sha256-extract-sea", size, &cut(bbox, 2, 4))
        .unwrap()
        .unwrap();
    assert_eq!((estimated.contents, estimated.tile_bytes), (1, 3));
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn an_archive_replaced_while_it_is_cut_ends_the_cut_with_nothing_kept() {
    let (source, size) = archive("sha256-extract-replaced");
    let bbox = Bbox::new(-170.0, -60.0, 170.0, 60.0).unwrap();
    let path = target("replaced");
    // the location no longer the content when the cut ends
    let refused = extract(
        &source,
        "sha256-extract-replaced",
        size,
        &cut(bbox, 0, 3),
        &path,
        &mut || false,
        &mut |_, _| true,
    )
    .unwrap()
    .unwrap_err();
    assert_eq!(refused.code, "SOURCE_CHANGED");
    assert!(leftovers(&path).is_empty(), "{:?}", leftovers(&path));
    // another tag, or another size, when it starts
    let other = extract(
        &source,
        "sha256-another",
        size,
        &cut(bbox, 0, 3),
        &path,
        &mut || true,
        &mut |_, _| true,
    )
    .unwrap()
    .unwrap_err();
    assert_eq!(other.code, "SOURCE_CHANGED");
    let shorter = estimate(&source, "sha256-extract-replaced", size - 1, &cut(bbox, 0, 3))
        .unwrap()
        .unwrap_err();
    assert_eq!(shorter.code, "SOURCE_CHANGED");
}

#[test]
fn a_cancel_stops_the_cut_and_limits_refuse_it_before_it_starts() {
    let (source, size) = archive("sha256-extract-limits");
    let bbox = Bbox::new(-170.0, -60.0, 170.0, 60.0).unwrap();
    let path = target("cancelled");
    let stopped = extract(
        &source,
        "sha256-extract-limits",
        size,
        &cut(bbox, 0, 4),
        &path,
        &mut || true,
        &mut |done, _| done == 0,
    )
    .unwrap()
    .unwrap_err();
    assert_eq!(stopped.code, "CANCELLED");
    assert!(leftovers(&path).is_empty(), "{:?}", leftovers(&path));
    let few = Cut {
        max_tiles: 10,
        ..cut(bbox, 0, 4)
    };
    assert_eq!(
        estimate(&source, "sha256-extract-limits", size, &few)
            .unwrap()
            .unwrap_err()
            .code,
        "TOO_MANY_TILES"
    );
    let small = Cut {
        max_bytes: 100,
        ..cut(bbox, 0, 4)
    };
    let refused = estimate(&source, "sha256-extract-limits", size, &small)
        .unwrap()
        .unwrap_err();
    assert_eq!(refused.code, "TOO_LARGE");
    assert!(refused.reason.contains("maps@package-max-bytes"), "{}", refused.reason);
    assert_eq!(
        estimate(&source, "sha256-extract-limits", size, &cut(bbox, 6, 8))
            .unwrap()
            .unwrap_err()
            .code,
        "NO_ZOOMS"
    );
}
