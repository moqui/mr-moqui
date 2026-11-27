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

#[test]
fn tile_ids_follow_the_hilbert_order() {
    // pmtiles' own test values
    assert_eq!(tile_id(0, 0, 0).unwrap(), 0);
    assert_eq!(tile_id(1, 0, 0).unwrap(), 1);
    assert_eq!(tile_id(1, 0, 1).unwrap(), 2);
    assert_eq!(tile_id(1, 1, 1).unwrap(), 3);
    assert_eq!(tile_id(1, 1, 0).unwrap(), 4);
    assert_eq!(tile_id(2, 0, 0).unwrap(), 5);
    assert_eq!(tile_id(3, 7, 0).unwrap(), 84);
    assert_eq!(tile_id(20, 0, 0).unwrap(), 366_503_875_925);
    for z in 0..=8u8 {
        for x in 0..(1u32 << z).min(40) {
            for y in [0, (1u32 << z) / 3, (1u32 << z) - 1] {
                let id = tile_id(z, x, y).unwrap();
                assert_eq!(tile_zxy(id).unwrap(), (z, x, y), "{z}/{x}/{y}");
            }
        }
    }
    let deep = tile_id(26, 12345, 67_108_863).unwrap();
    assert_eq!(tile_zxy(deep).unwrap(), (26, 12345, 67_108_863));
    assert!(tile_id(2, 4, 0).is_err() && tile_id(27, 0, 0).is_err());
}

fn entries() -> Vec<Entry> {
    vec![
        Entry {
            tile_id: 0,
            offset: 0,
            length: 10,
            run_length: 1,
        },
        Entry {
            tile_id: 1,
            offset: 10,
            length: 20,
            run_length: 3,
        },
        Entry {
            tile_id: 5,
            offset: 0,
            length: 10,
            run_length: 1,
        },
        Entry {
            tile_id: 9,
            offset: 30,
            length: 5,
            run_length: 0,
        },
    ]
}

#[test]
fn directories_decode_what_they_encode() {
    let bytes = encode_directory(&entries());
    // count 4; deltas 0 1 4 4; runs 1 3 1 0; lengths 10 20 10 5; offsets 1, 0 (follows), 1, 31
    assert_eq!(bytes, vec![4, 0, 1, 4, 4, 1, 3, 1, 0, 10, 20, 10, 5, 1, 0, 1, 31]);
    assert_eq!(decode_directory(&bytes).unwrap(), entries());
    assert!(decode_directory(&bytes[..16])
        .unwrap_err()
        .message()
        .contains("cut short"));
    assert!(decode_directory(&[200, 1]).unwrap_err().message().contains("claims"));
    // a delta of 0 after the first entry: out of order
    assert!(decode_directory(&[2, 3, 0, 1, 1, 1, 1, 1, 1]).is_err());
}

#[test]
fn finds_a_tile_in_a_run_or_a_leaf() {
    let entries = entries();
    assert_eq!(find_entry(&entries, 0).unwrap().length, 10);
    // tiles 1, 2 and 3 are one run of content at 10
    assert_eq!(find_entry(&entries, 3).unwrap().offset, 10);
    assert!(find_entry(&entries, 4).is_none());
    assert_eq!(find_entry(&entries, 5).unwrap().tile_id, 5);
    assert!(find_entry(&entries, 6).is_none());
    // from 9 on, a leaf may hold it
    assert!(find_entry(&entries, 9).unwrap().is_leaf());
    assert!(find_entry(&entries, 1000).unwrap().is_leaf());
    assert!(find_entry(&[], 0).is_none());
}

fn options() -> ArchiveOptions {
    ArchiveOptions {
        tile_type: TileType::Mvt,
        tile_compression: Compression::None,
        internal_compression: Compression::None,
        min_zoom: 0,
        max_zoom: 2,
        bounds: [-10.0, -5.0, 10.0, 5.0],
        center_zoom: 1,
    }
}

/// The tile `id` of `archive`, read through its directories as the endpoint reads them
fn read(archive: &[u8], id: u64) -> Option<Vec<u8>> {
    let header = Header::parse(archive).unwrap();
    let mut reads = 0;
    let found = locate_tile(&header, id, |offset, length| {
        reads += 1;
        let start = usize::try_from(offset).unwrap();
        let end = start + usize::try_from(length).unwrap();
        Ok(std::sync::Arc::new(decode_directory(&archive[start..end])?))
    })
    .unwrap();
    assert!(reads <= MAX_DEPTH);
    found.map(|(offset, length)| {
        let start = usize::try_from(offset).unwrap();
        archive[start..start + length as usize].to_vec()
    })
}

#[test]
fn writes_an_archive_its_directory_reads() {
    let tiles = vec![
        (0, b"world".to_vec()),
        (1, b"same".to_vec()),
        (2, b"same".to_vec()),
        (3, b"other".to_vec()),
        (7, b"z2".to_vec()),
    ];
    let archive = write_archive(tiles, br#"{"vector_layers":[{"id":"overlay"}]}"#, &options(), &|b| {
        b.to_vec()
    })
    .unwrap();
    let header = Header::parse(&archive).unwrap();
    assert!(
        header.problems(archive.len() as u64).is_empty(),
        "{:?}",
        header.problems(archive.len() as u64)
    );
    assert_eq!((header.addressed_tiles, header.tile_entries), (5, 4));
    assert!(header.clustered && header.leaf_length == 0);
    assert!((header.min_lon + 10.0).abs() < 1e-6 && (header.center_lat).abs() < 1e-6);
    assert_eq!(Header::parse(&header.to_bytes()).unwrap(), header);
    assert_eq!(read(&archive, 0).unwrap(), b"world");
    assert_eq!(read(&archive, 2).unwrap(), b"same");
    assert_eq!(read(&archive, 3).unwrap(), b"other");
    assert_eq!(read(&archive, 7).unwrap(), b"z2");
    assert!(read(&archive, 4).is_none() && read(&archive, 8).is_none());
    assert!(write_archive(vec![(3, vec![1]), (2, vec![2])], b"{}", &options(), &|b| b.to_vec()).is_err());
    assert!(write_archive(Vec::new(), b"{}", &options(), &|b| b.to_vec()).is_err());
}

#[test]
fn a_large_archive_has_leaf_directories() {
    // 40,000 tiles of distinct contents: the root alone would pass 16 KiB, so it points to leaves
    let tiles: Vec<(u64, Vec<u8>)> = (0..40_000u64).map(|id| (id * 2, id.to_le_bytes().to_vec())).collect();
    let archive = write_archive(tiles, b"{}", &options(), &|b| b.to_vec()).unwrap();
    let header = Header::parse(&archive).unwrap();
    assert!(header.leaf_length > 0 && header.root_offset + header.root_length <= ROOT_END_MAX);
    assert!(header.problems(archive.len() as u64).is_empty());
    assert_eq!(read(&archive, 0).unwrap(), 0u64.to_le_bytes());
    assert_eq!(read(&archive, 2 * 31_337).unwrap(), 31_337u64.to_le_bytes());
    assert_eq!(read(&archive, 2 * 39_999).unwrap(), 39_999u64.to_le_bytes());
    assert!(read(&archive, 2 * 31_337 + 1).is_none());
}

/// The header of `header_bytes` with a leaf section and tile data where a test puts them
fn sections(leaf_length: u64, tile_data_offset: u64, tile_data_length: u64) -> Header {
    let mut header = Header::parse(&header_bytes()).unwrap();
    header.leaf_offset = 1000;
    header.leaf_length = leaf_length;
    header.tile_data_offset = tile_data_offset;
    header.tile_data_length = tile_data_length;
    header
}

fn one(entry: Entry) -> impl FnMut(u64, u64) -> Result<std::sync::Arc<Vec<Entry>>, Error> {
    move |_, _| Ok(std::sync::Arc::new(vec![entry]))
}

#[test]
fn refuses_a_malicious_archive_s_directories() {
    // a leaf that points at itself: followed at most MAX_DEPTH times
    let leaf = Entry {
        tile_id: 0,
        offset: 0,
        length: 10,
        run_length: 0,
    };
    let mut reads = 0;
    let looping = locate_tile(&sections(100, 5000, 100), 7, |_, _| {
        reads += 1;
        Ok(std::sync::Arc::new(vec![leaf]))
    })
    .unwrap_err();
    assert!(
        looping.message().contains("nest deeper") && reads == MAX_DEPTH,
        "{}",
        looping.message()
    );
    // a leaf outside the leaf directories, a tile outside the tile data
    let far_leaf = Entry {
        tile_id: 0,
        offset: 95,
        length: 10,
        run_length: 0,
    };
    assert!(locate_tile(&sections(100, 5000, 100), 0, one(far_leaf))
        .unwrap_err()
        .message()
        .contains("leaf"));
    let far_tile = Entry {
        tile_id: 0,
        offset: 95,
        length: 10,
        run_length: 1,
    };
    assert!(locate_tile(&sections(100, 5000, 100), 0, one(far_tile))
        .unwrap_err()
        .message()
        .contains("tile data"));
    // offsets that overflow when added to their section's
    let tile = Entry {
        tile_id: 0,
        offset: 10,
        length: 10,
        run_length: 1,
    };
    let overflow = locate_tile(&sections(100, u64::MAX - 5, u64::MAX), 0, one(tile)).unwrap_err();
    assert!(overflow.message().contains("overflows"), "{}", overflow.message());
    let mut header = sections(u64::MAX, 5000, 100);
    header.leaf_offset = u64::MAX - 5;
    let leaf = Entry {
        tile_id: 0,
        offset: 10,
        length: 10,
        run_length: 0,
    };
    assert!(locate_tile(&header, 0, one(leaf))
        .unwrap_err()
        .message()
        .contains("overflows"));
    // a varint of more than 64 bits, and a count larger than the bytes hold
    assert!(decode_directory(&[0xff; 11])
        .unwrap_err()
        .message()
        .contains("too large"));
    assert!(decode_directory(&[0xff, 0xff, 0xff, 0x01])
        .unwrap_err()
        .message()
        .contains("claims"));
}

/// The archive writer as it was before [`ArchiveWriter`] (the whole archive built in one `Vec`), kept here so the
/// streaming writer is checked to give the same bytes
fn whole_archive(tiles: &[(u64, Vec<u8>)], metadata: &[u8], options: &ArchiveOptions) -> Vec<u8> {
    let compress = |b: &[u8]| b.to_vec();
    let mut entries: Vec<Entry> = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    let mut last_start = 0usize;
    let mut addressed = 0u64;
    for (tile_id, content) in tiles {
        if let Some(last) = entries.last_mut() {
            if *tile_id == last.tile_id + u64::from(last.run_length) && data[last_start..] == content[..] {
                last.run_length += 1;
                addressed += 1;
                continue;
            }
        }
        addressed += 1;
        last_start = data.len();
        entries.push(Entry {
            tile_id: *tile_id,
            offset: last_start as u64,
            length: u32::try_from(content.len()).unwrap(),
            run_length: 1,
        });
        data.extend_from_slice(content);
    }
    let (root, leaves) = directories(&entries, &compress);
    let metadata = compress(metadata);
    let root_offset = HEADER_LEN as u64;
    let metadata_offset = root_offset + root.len() as u64;
    let leaf_offset = metadata_offset + metadata.len() as u64;
    let [west, south, east, north] = options.bounds;
    let header = Header {
        version: 3,
        root_offset,
        root_length: root.len() as u64,
        metadata_offset,
        metadata_length: metadata.len() as u64,
        leaf_offset,
        leaf_length: leaves.len() as u64,
        tile_data_offset: leaf_offset + leaves.len() as u64,
        tile_data_length: data.len() as u64,
        addressed_tiles: addressed,
        tile_entries: entries.len() as u64,
        tile_contents: entries.len() as u64,
        clustered: true,
        internal_compression: options.internal_compression,
        tile_compression: options.tile_compression,
        tile_type: options.tile_type,
        min_zoom: options.min_zoom,
        max_zoom: options.max_zoom,
        min_lon: west,
        min_lat: south,
        max_lon: east,
        max_lat: north,
        center_zoom: options.center_zoom,
        center_lon: (west + east) / 2.0,
        center_lat: (south + north) / 2.0,
    };
    let mut out = header.to_bytes().to_vec();
    out.extend_from_slice(&root);
    out.extend_from_slice(&metadata);
    out.extend_from_slice(&leaves);
    out.extend_from_slice(&data);
    out
}

/// The archives the writer tests above write: a small one with a run, one of 40,000 tiles with leaf directories, and
/// runs broken by gaps with a content coming back after another
fn golden_tile_sets() -> Vec<Vec<(u64, Vec<u8>)>> {
    vec![
        vec![
            (0, b"world".to_vec()),
            (1, b"same".to_vec()),
            (2, b"same".to_vec()),
            (3, b"other".to_vec()),
            (7, b"z2".to_vec()),
        ],
        (0..40_000u64).map(|id| (id * 2, id.to_le_bytes().to_vec())).collect(),
        (0..3000u64)
            .map(|id| (id + id / 7, vec![u8::try_from(id % 5 / 3).unwrap(); 40]))
            .collect(),
    ]
}

#[test]
fn the_streaming_writer_gives_the_bytes_the_whole_archive_writer_gave() {
    let metadata = br#"{"vector_layers":[{"id":"overlay"}]}"#;
    for tiles in golden_tile_sets() {
        let expected = whole_archive(&tiles, metadata, &options());
        assert_eq!(
            write_archive(tiles.clone(), metadata, &options(), &|b| b.to_vec()).unwrap(),
            expected
        );
        // the tile data streamed to a file, the prefix written before it
        let path = std::env::temp_dir().join(format!("moqui-pmtiles-{}-{}.part", std::process::id(), tiles.len()));
        let part = std::fs::File::create(&path).unwrap();
        let mut writer = ArchiveWriter::new(std::io::BufWriter::new(part));
        for (tile_id, content) in &tiles {
            writer.add_tile(*tile_id, content).unwrap();
        }
        assert_eq!(writer.tile_count(), tiles.len() as u64);
        let finished = writer.finish(metadata, &options(), &|b| b.to_vec()).unwrap();
        drop(finished.data);
        let data = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut streamed = finished.prefix;
        streamed.extend_from_slice(&data);
        assert_eq!(streamed, expected);
        assert_eq!(finished.header, Header::parse(&expected).unwrap());
    }
}

#[test]
fn keyed_tiles_share_one_stored_content() {
    let mut writer = ArchiveWriter::new(Vec::new());
    // the sea at the source's offset 500, apart and then as a run; land at 900
    writer.add_keyed(0, 500, b"sea").unwrap();
    writer.add_keyed(1, 900, b"land").unwrap();
    writer
        .add_keyed(2, 500, b"ignored: the key's content is stored")
        .unwrap();
    writer.repeat_keyed(3, 500).unwrap();
    writer.add_tile(5, b"lake").unwrap();
    assert!(writer.has_key(500) && !writer.has_key(1));
    assert!(writer.repeat_keyed(6, 1).is_err());
    assert!(writer.repeat_keyed(4, 500).is_err(), "a tile ID before the last one");
    let finished = writer.finish(b"{}", &options(), &|b| b.to_vec()).unwrap();
    let mut archive = finished.prefix;
    archive.extend_from_slice(&finished.data);
    let header = Header::parse(&archive).unwrap();
    assert!(header.problems(archive.len() as u64).is_empty());
    // five tiles, four entries (2 and 3 one run), three contents
    assert_eq!(
        (header.addressed_tiles, header.tile_entries, header.tile_contents),
        (5, 4, 3)
    );
    assert_eq!(header.tile_data_length, 11);
    assert_eq!(read(&archive, 0).unwrap(), b"sea");
    assert_eq!(read(&archive, 1).unwrap(), b"land");
    assert_eq!(read(&archive, 2).unwrap(), b"sea");
    assert_eq!(read(&archive, 3).unwrap(), b"sea");
    assert!(read(&archive, 4).is_none());
    assert_eq!(read(&archive, 5).unwrap(), b"lake");
    assert!(ArchiveWriter::new(Vec::new())
        .finish(b"{}", &options(), &|b| b.to_vec())
        .is_err());
}

#[test]
fn the_keys_kept_are_bounded_and_a_forgotten_one_is_written_again() {
    let mut writer = ArchiveWriter::with_keys_max(Vec::new(), 4);
    // the sea, repeated, outlives the keys never repeated when the bound is reached
    writer.add_keyed(0, 100, b"sea").unwrap();
    writer.repeat_keyed(1, 100).unwrap();
    for (id, key) in [(2, 200), (3, 300), (4, 400), (5, 500)] {
        writer.add_keyed(id, key, format!("land {key}").as_bytes()).unwrap();
    }
    assert!(writer.has_key(100) && writer.has_key(500));
    assert!(!writer.has_key(200), "a key never repeated is let go past the bound");
    // a key let go is a new content again: written twice, read right
    writer.add_keyed(6, 200, b"land 200").unwrap();
    let finished = writer.finish(b"{}", &options(), &|b| b.to_vec()).unwrap();
    let mut archive = finished.prefix;
    archive.extend_from_slice(&finished.data);
    assert_eq!(read(&archive, 1).unwrap(), b"sea");
    assert_eq!(read(&archive, 6).unwrap(), b"land 200");
    assert_eq!(read(&archive, 2).unwrap(), b"land 200");
    assert_eq!(Header::parse(&archive).unwrap().tile_contents, 6);
}
