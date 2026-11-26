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

/// A header of a small valid vector archive of `size` bytes over Florence, z0 to 14.
fn header_bytes() -> Vec<u8> {
    let mut bytes = vec![0u8; HEADER_LEN];
    bytes[..7].copy_from_slice(b"PMTiles");
    bytes[7] = 3;
    let words: [(usize, u64); 11] = [
        (8, 127),  // root offset
        (16, 40),  // root length
        (24, 167), // metadata offset
        (32, 60),  // metadata length
        (40, 0),   // leaf offset
        (48, 0),   // leaf length
        (56, 227), // tile data offset
        (64, 100), // tile data length
        (72, 5),   // addressed tiles
        (80, 5),   // tile entries
        (88, 5),   // tile contents
    ];
    for (at, value) in words {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes[96] = 1; // clustered
    bytes[97] = 2; // gzip directories
    bytes[98] = 2; // gzip tiles
    bytes[99] = 1; // mvt
    bytes[100] = 0;
    bytes[101] = 14;
    for (at, degrees) in [
        (102, 11.1),
        (106, 43.7),
        (110, 11.4),
        (114, 43.9),
        (119, 11.25),
        (123, 43.77),
    ] {
        #[allow(clippy::cast_possible_truncation)]
        let e7 = (degrees * 10_000_000.0_f64).round() as i32;
        bytes[at..at + 4].copy_from_slice(&e7.to_le_bytes());
    }
    bytes[118] = 12;
    bytes
}

const SIZE: u64 = 327;

#[test]
fn reads_a_version_3_header() {
    let header = Header::parse(&header_bytes()).unwrap();
    assert_eq!(header.version, 3);
    assert_eq!((header.root_offset, header.root_length), (127, 40));
    assert_eq!((header.metadata_offset, header.metadata_length), (167, 60));
    assert_eq!(header.tile_type, TileType::Mvt);
    assert!(header.tile_type.is_vector() && !header.tile_type.is_raster());
    assert_eq!(header.internal_compression, Compression::Gzip);
    assert_eq!((header.min_zoom, header.max_zoom, header.center_zoom), (0, 14, 12));
    assert!((header.min_lon - 11.1).abs() < 1e-6 && (header.max_lat - 43.9).abs() < 1e-6);
    assert!((header.center_lat - 43.77).abs() < 1e-6);
    assert!(header.clustered);
    assert!(header.problems(SIZE).is_empty(), "{:?}", header.problems(SIZE));
}

#[test]
fn refuses_what_is_not_a_version_3_archive() {
    let short = Header::parse(&header_bytes()[..100]).unwrap_err();
    assert!(short.message().contains("fewer than a header"), "{}", short.message());
    let mut other = header_bytes();
    other[..7].copy_from_slice(b"NOTPMTL");
    assert!(Header::parse(&other)
        .unwrap_err()
        .message()
        .contains("does not start with PMTiles"));
    let mut version = header_bytes();
    version[7] = 4;
    assert!(Header::parse(&version).unwrap_err().message().contains("version 4"));
    // a version 2 archive starts PM and its version
    let old = Header::parse(&[b'P', b'M', 2, 0, 0, 0, 0, 0]).unwrap_err();
    assert!(
        old.message().contains("version 2") && old.message().contains("pmtiles convert"),
        "{}",
        old.message()
    );
}

#[test]
fn names_each_problem_of_a_header() {
    let problems_of = |change: &dyn Fn(&mut Vec<u8>), size: u64| {
        let mut bytes = header_bytes();
        change(&mut bytes);
        Header::parse(&bytes).unwrap().problems(size)
    };
    let zoom = problems_of(
        &|b| {
            b[100] = 15;
        },
        SIZE,
    );
    assert!(
        zoom.iter()
            .any(|p| p.contains("minimum zoom 15 is above its maximum zoom 14")),
        "{zoom:?}"
    );
    let deep = problems_of(
        &|b| {
            b[101] = 40;
        },
        SIZE,
    );
    assert!(deep.iter().any(|p| p.contains("maximum zoom 40")), "{deep:?}");
    let bounds = problems_of(&|b| b[106..110].copy_from_slice(&950_000_000i32.to_le_bytes()), SIZE);
    assert!(
        bounds.iter().any(|p| p.contains("not longitudes and latitudes")),
        "{bounds:?}"
    );
    let swapped = problems_of(&|b| b[102..106].copy_from_slice(&120_000_000i32.to_le_bytes()), SIZE);
    assert!(
        swapped.iter().any(|p| p.contains("minimum above a maximum")),
        "{swapped:?}"
    );
    // cut short: the tile data runs past the end
    let cut = problems_of(&|_| {}, 300);
    assert!(
        cut.iter().any(|p| p.contains("tile data") && p.contains("cut short")),
        "{cut:?}"
    );
    let image = problems_of(
        &|b| {
            b[99] = 9;
        },
        SIZE,
    );
    assert!(image.iter().any(|p| p.contains("type 9")), "{image:?}");
    let brotli = problems_of(
        &|b| {
            b[97] = 3;
        },
        SIZE,
    );
    assert!(brotli.iter().any(|p| p.contains("brotli")), "{brotli:?}");
    let far_root = problems_of(&|b| b[8..16].copy_from_slice(&20_000u64.to_le_bytes()), 40_000);
    assert!(far_root.iter().any(|p| p.contains("first 16384 bytes")), "{far_root:?}");
    let zstd_tiles = problems_of(&|b| b[98] = 4, SIZE);
    assert!(
        zstd_tiles
            .iter()
            .any(|p| p.contains("mvt tiles are compressed with zstd")),
        "{zstd_tiles:?}"
    );
    let gzip_images = problems_of(
        &|b| {
            b[99] = 2;
            b[98] = 2;
        },
        SIZE,
    );
    assert!(
        gzip_images
            .iter()
            .any(|p| p.contains("png tiles are compressed with gzip")),
        "{gzip_images:?}"
    );
    let in_header = problems_of(&|b| b[24..32].copy_from_slice(&100u64.to_le_bytes()), SIZE);
    assert!(
        in_header
            .iter()
            .any(|p| p.contains("metadata") && p.contains("inside the header")),
        "{in_header:?}"
    );
    let too_deep = problems_of(&|b| b[101] = 27, SIZE);
    assert!(
        too_deep.iter().any(|p| p.contains("maximum zoom 27 is above 26")),
        "{too_deep:?}"
    );
    let empty = problems_of(&|b| b[72..80].copy_from_slice(&0u64.to_le_bytes()), SIZE);
    assert!(empty.iter().any(|p| p.contains("no tiles")), "{empty:?}");
}

#[test]
fn raster_tile_types() {
    let mut bytes = header_bytes();
    for (byte, name) in [(2u8, "png"), (3, "jpeg"), (4, "webp"), (5, "avif")] {
        bytes[99] = byte;
        let header = Header::parse(&bytes).unwrap();
        assert!(header.tile_type.is_raster() && !header.tile_type.is_vector());
        assert_eq!(header.tile_type.name(), name);
    }
}

#[test]
fn reads_the_metadata() {
    let metadata = Metadata::parse(
        br#"{"name":"Florence","attribution":"<a href=\"https://openstreetmap.org\">OSM</a>","version":4,
            "type":"baselayer","vector_layers":[{"id":"earth"},{"id":"water"},{"fields":{}}]}"#,
    )
    .unwrap();
    assert_eq!(metadata.name, "Florence");
    assert_eq!(metadata.version, "4");
    assert_eq!(metadata.kind, "baselayer");
    assert_eq!(metadata.vector_layers, vec!["earth".to_string(), "water".to_string()]);
    assert!(metadata.attribution.contains("OSM"));
    assert!(Metadata::parse(b"[1]")
        .unwrap_err()
        .message()
        .contains("not a JSON object"));
    assert!(Metadata::parse(b"{nope").unwrap_err().message().contains("not JSON"));
}
