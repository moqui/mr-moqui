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

use std::io::Write;

use moqui_core::resource::source::MemorySource;
use moqui_lib::pmtiles::{write_archive, ArchiveOptions, TileType};

use super::*;

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// An archive of gzipped vector tiles at 0/0/0 and 1/1/0, gzipped directories, pinned by `tag`
fn archive(tag: &str) -> (MemorySource, u64) {
    let tiles = vec![
        (tile_id(0, 0, 0).unwrap(), gzip(b"world tile")),
        (tile_id(1, 1, 0).unwrap(), gzip(b"north east")),
    ];
    let options = ArchiveOptions {
        tile_type: TileType::Mvt,
        tile_compression: Compression::Gzip,
        internal_compression: Compression::Gzip,
        min_zoom: 0,
        max_zoom: 1,
        bounds: [-180.0, -85.0, 180.0, 85.0],
        center_zoom: 0,
    };
    let bytes = write_archive(tiles, br#"{"vector_layers":[{"id":"overlay"}]}"#, &options, &gzip).unwrap();
    let size = bytes.len() as u64;
    (MemorySource::new(bytes).with_tag(tag), size)
}

fn request(tag: &str, size: u64, (z, x, y): (u8, u32, u32), accept_gzip: bool) -> TileRequest<'_> {
    TileRequest {
        tag,
        size,
        z,
        x,
        y,
        accept_gzip,
        if_none_match: "",
    }
}

#[test]
fn answers_a_tile_gzipped_or_plain_by_what_the_client_takes() {
    let (source, size) = archive("sha256-tiles-one");
    let TileAnswer::Tile {
        bytes,
        content_type,
        gzip: sent_gzip,
        etag,
    } = answer_tile(&source, &request("sha256-tiles-one", size, (1, 1, 0), true)).unwrap()
    else {
        panic!("no tile");
    };
    assert!(sent_gzip && content_type == "application/vnd.mapbox-vector-tile");
    assert_eq!(bytes, gzip(b"north east"));
    assert_eq!(etag, format!("\"sha256-tiles-one.{}.gz\"", tile_id(1, 1, 0).unwrap()));
    // a client that takes no gzip gets the tile decompressed, under another tag
    let TileAnswer::Tile { bytes, gzip, etag, .. } =
        answer_tile(&source, &request("sha256-tiles-one", size, (0, 0, 0), false)).unwrap()
    else {
        panic!("no tile");
    };
    assert!(!gzip && bytes == b"world tile" && etag == "\"sha256-tiles-one.0\"");
}

#[test]
fn a_missing_tile_a_known_tag_and_a_changed_archive() {
    let (source, size) = archive("sha256-tiles-two");
    let tag = "sha256-tiles-two";
    // no tile there, and a zoom past the archive's: an empty vector tile
    assert_eq!(
        answer_tile(&source, &request(tag, size, (1, 0, 0), true)).unwrap(),
        TileAnswer::Missing { vector: true }
    );
    assert_eq!(
        answer_tile(&source, &request(tag, size, (5, 0, 0), true)).unwrap(),
        TileAnswer::Missing { vector: true }
    );
    assert!(matches!(
        answer_tile(&source, &request(tag, size, (1, 2, 0), true)).unwrap(),
        TileAnswer::Refused { status: 404, .. }
    ));
    let mut known = request(tag, size, (1, 1, 0), true);
    let etag = format!("\"{tag}.{}.gz\"", tile_id(1, 1, 0).unwrap());
    let header = format!("\"other\", {etag}");
    known.if_none_match = &header;
    assert_eq!(answer_tile(&source, &known).unwrap(), TileAnswer::NotModified { etag });
    // another tag or size than the source's: it is not the revision registered
    assert!(matches!(
        answer_tile(&source, &request("sha256-other", size, (0, 0, 0), true)).unwrap(),
        TileAnswer::Refused { status: 409, .. }
    ));
    assert!(matches!(
        answer_tile(&source, &request(tag, size + 1, (0, 0, 0), true)).unwrap(),
        TileAnswer::Refused { status: 409, .. }
    ));
}

#[test]
fn the_kept_directories_stay_within_their_bound() {
    let entries = Arc::new(vec![
        Entry {
            tile_id: 0,
            offset: 0,
            length: 1,
            run_length: 1
        };
        CACHED_ENTRIES / 2 + 1
    ]);
    keep_directory("bound@1", &entries);
    keep_directory("bound@2", &entries);
    // two of them pass the bound on entries: the older one is dropped
    assert!(kept_directory("bound@1").is_none());
    assert!(kept_directory("bound@2").is_some());
    with_kept(|kept| assert!(kept.entries <= CACHED_ENTRIES));
}

#[test]
fn reads_the_coordinates_of_a_template() {
    let mut parameters = BTreeMap::new();
    parameters.insert("y".to_string(), Value::string("12.mvt"));
    parameters.insert("x".to_string(), Value::string("-1"));
    assert_eq!(coordinate(&parameters, "y"), Some(12));
    assert_eq!(coordinate(&parameters, "x"), None);
    assert_eq!(coordinate(&parameters, "z"), None);
}

#[test]
fn a_directory_that_inflates_past_its_bound_is_refused() {
    // a megabyte of zeros gzips to about a kilobyte: read to the bound only
    let bomb = gzip(&vec![0u8; 1024 * 1024]);
    assert!(bomb.len() < 8 * 1024);
    let refused = decompress(bomb.clone(), Compression::Gzip, 64 * 1024, "directory").unwrap_err();
    assert!(refused.message().contains("more than"), "{}", refused.message());
    assert_eq!(
        decompress(bomb, Compression::Gzip, 2 * 1024 * 1024, "directory")
            .unwrap()
            .len(),
        1024 * 1024
    );
    assert!(decompress(vec![1, 2, 3], Compression::Zstd, 10, "directory").is_err());
}

#[test]
fn a_missing_tile_is_never_not_modified() {
    let (source, size) = archive("sha256-tiles-three");
    let mut any = request("sha256-tiles-three", size, (1, 0, 0), true);
    any.if_none_match = "*";
    assert_eq!(
        answer_tile(&source, &any).unwrap(),
        TileAnswer::Missing { vector: true }
    );
    any.z = 0;
    any.x = 0;
    assert!(matches!(
        answer_tile(&source, &any).unwrap(),
        TileAnswer::NotModified { .. }
    ));
}
