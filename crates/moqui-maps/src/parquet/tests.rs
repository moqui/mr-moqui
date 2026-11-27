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
use crate::testdata::{self, FixturePlace, Writer};

use ::parquet::basic::{Compression, Encoding};
use ::parquet::file::properties::WriterVersion;

fn all(_: &[String]) -> Option<bool> {
    Some(true)
}

fn places() -> Vec<FixturePlace> {
    let mut a = FixturePlace::new("a", 11.25, 43.77, "Alpha", "cafe");
    a.sources = vec![("meta", "CDLA-Permissive-2.0"), ("Foursquare", "Apache-2.0")];
    a.address = Some(("Via Uno 1", "Firenze", "IT"));
    let mut b = FixturePlace::new("b", 11.26, 43.775, "Beta", "museum");
    b.name = None;
    b.sources = Vec::new();
    b.confidence = None;
    let c = FixturePlace::new("c", 11.255, 43.771, "Gamma", "cafe");
    let mut d = FixturePlace::new("d", 11.257, 43.772, "Delta with a longer name", "bar");
    d.version = Some(7);
    vec![a, b, c, d]
}

fn read(bytes: &Vec<u8>, path: &[&str]) -> ColumnData {
    let file = ParquetFile::open(bytes, &Limits::standard(), &all).unwrap();
    let leaf = file.meta.leaf(path).unwrap();
    file.read_column(bytes, 0, leaf, &Limits::standard()).unwrap()
}

fn texts(data: &ColumnData) -> Vec<Option<String>> {
    data.scalars()
        .into_iter()
        .map(|index| index.and_then(|i| data.values.text(i)))
        .collect()
}

fn lists(data: &ColumnData) -> Vec<Vec<Option<String>>> {
    data.lists()
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|index| index.and_then(|i| data.values.text(i)))
                .collect()
        })
        .collect()
}

/// Each writer setting the reader must take: v1 and v2 pages, dictionary and plain, the delta encodings, Snappy.
fn writers() -> Vec<(&'static str, Writer)> {
    vec![
        ("v1 dictionary", Writer::default()),
        (
            "v1 plain snappy",
            Writer {
                dictionary: false,
                compression: Compression::SNAPPY,
                ..Writer::default()
            },
        ),
        (
            "v2 dictionary snappy",
            Writer {
                version: WriterVersion::PARQUET_2_0,
                compression: Compression::SNAPPY,
                ..Writer::default()
            },
        ),
        (
            "v2 delta",
            Writer {
                version: WriterVersion::PARQUET_2_0,
                dictionary: false,
                encodings: vec![
                    ("id", Encoding::DELTA_BYTE_ARRAY),
                    ("names.primary", Encoding::DELTA_LENGTH_BYTE_ARRAY),
                    ("version", Encoding::DELTA_BINARY_PACKED),
                    ("bbox.xmin", Encoding::BYTE_STREAM_SPLIT),
                ],
                page_rows: 2,
                ..Writer::default()
            },
        ),
    ]
}

#[test]
fn the_schema_reads_as_leaves_with_their_levels() {
    let bytes = testdata::write(
        testdata::PLACES_SCHEMA,
        &[testdata::place_leaves(&places())],
        &Writer::default(),
    );
    let file = ParquetFile::open(&bytes, &Limits::standard(), &all).unwrap();
    assert_eq!(file.meta.num_rows, 4);
    assert_eq!(file.meta.created_by.as_deref(), Some("moqui-maps fixtures"));
    let license = &file.meta.leaves[file.meta.leaf(&["sources", "list", "element", "license"]).unwrap()];
    assert_eq!(
        (license.max_def, license.max_rep, license.repeated_def.clone()),
        (4, 1, vec![2])
    );
    let primary = &file.meta.leaves[file.meta.leaf(&["names", "primary"]).unwrap()];
    assert_eq!(
        (primary.max_def, primary.max_rep, primary.physical),
        (2, 0, Physical::ByteArray)
    );
    assert_eq!(file.meta.leaves.len(), 16);
}

#[test]
fn every_writer_setting_reads_the_same_rows() {
    for (name, writer) in writers() {
        let bytes = testdata::write(testdata::PLACES_SCHEMA, &[testdata::place_leaves(&places())], &writer);
        let ids = texts(&read(&bytes, &["id"]));
        assert_eq!(ids, ["a", "b", "c", "d"].map(|id| Some(id.to_string())), "{name}");
        let names = texts(&read(&bytes, &["names", "primary"]));
        assert_eq!(names[1], None, "{name}: a null name");
        assert_eq!(names[3].as_deref(), Some("Delta with a longer name"), "{name}");
        let versions = read(&bytes, &["version"]);
        assert_eq!(versions.values, Values::Int32(vec![1, 1, 1, 7]), "{name}");
        let xmin = read(&bytes, &["bbox", "xmin"]);
        assert_eq!(
            xmin.values,
            Values::Double(vec![11.25, 11.26, 11.255, 11.257]),
            "{name}"
        );
        let confidence = read(&bytes, &["confidence"]);
        assert_eq!(
            confidence.scalars(),
            vec![Some(0), None, Some(1), Some(2)],
            "{name}: a null confidence"
        );
        let licenses = lists(&read(&bytes, &["sources", "list", "element", "license"]));
        assert_eq!(
            licenses,
            vec![
                vec![Some("CDLA-Permissive-2.0".to_string()), Some("Apache-2.0".to_string())],
                vec![],
                vec![Some("CDLA-Permissive-2.0".to_string())],
                vec![Some("CDLA-Permissive-2.0".to_string())],
            ],
            "{name}: lists, an empty one among them"
        );
        let geometry = read(&bytes, &["geometry"]);
        assert_eq!(
            geometry.values.bytes(0),
            Some(testdata::point_wkb(11.25, 43.77).as_slice()),
            "{name}"
        );
    }
}

#[test]
fn a_boolean_column_reads_plain_and_rle() {
    let buildings = vec![
        testdata::FixtureBuilding {
            id: "x",
            footprint: [0.0, 0.0, 1.0, 1.0],
            name: None,
            class: None,
            height: None,
            floors: None,
            sources: Vec::new(),
        };
        5
    ];
    for writer in [
        Writer::default(),
        Writer {
            version: WriterVersion::PARQUET_2_0,
            ..Writer::default()
        },
    ] {
        let bytes = testdata::write(
            testdata::BUILDINGS_SCHEMA,
            &[testdata::building_leaves(&buildings)],
            &writer,
        );
        let data = read(&bytes, &["is_underground"]);
        assert_eq!(data.values, Values::Boolean(vec![false; 5]));
    }
}

#[test]
fn row_group_statistics_place_each_group() {
    let far = FixturePlace::new("far", 12.49, 41.90, "Far", "cafe");
    let bytes = testdata::write(
        testdata::PLACES_SCHEMA,
        &[testdata::place_leaves(&places()), testdata::place_leaves(&[far])],
        &Writer::default(),
    );
    let file = ParquetFile::open(&bytes, &Limits::standard(), &all).unwrap();
    assert_eq!(file.meta.row_groups.len(), 2);
    let leaf = file.meta.leaf(&["bbox", "xmin"]).unwrap();
    let range = |group: usize| {
        double_range(
            file.meta.row_groups[group].chunk(leaf).unwrap().statistics.as_ref(),
            Physical::Double,
        )
    };
    assert_eq!(range(0), Some((11.25, 11.26)));
    assert_eq!(range(1), Some((12.49, 12.49)));
}

#[test]
fn only_the_chunks_asked_for_are_kept() {
    let bytes = testdata::write(
        testdata::PLACES_SCHEMA,
        &[testdata::place_leaves(&places())],
        &Writer::default(),
    );
    let wanted = |path: &[String]| (path.first().map(String::as_str) == Some("id")).then_some(false);
    let file = ParquetFile::open(&bytes, &Limits::standard(), &wanted).unwrap();
    assert_eq!(file.meta.row_groups[0].chunks.len(), 1);
    assert_eq!(file.meta.row_groups[0].chunks[0].statistics, None);
    let names = file.meta.leaf(&["names", "primary"]).unwrap();
    assert!(file.read_column(&bytes, 0, names, &Limits::standard()).is_err());
}

#[test]
fn a_file_that_is_not_parquet_or_cut_short_is_refused() {
    let bytes = testdata::write(
        testdata::PLACES_SCHEMA,
        &[testdata::place_leaves(&places())],
        &Writer::default(),
    );
    let cut = bytes[..bytes.len() - 10].to_vec();
    assert!(ParquetFile::open(&cut, &Limits::standard(), &all).is_err());
    assert!(ParquetFile::open(&b"PAR1 hello".to_vec(), &Limits::standard(), &all).is_err());
    let mut encrypted = bytes.clone();
    let end = encrypted.len();
    encrypted[end - 4..].copy_from_slice(b"PARE");
    let error = ParquetFile::open(&encrypted, &Limits::standard(), &all).unwrap_err();
    assert!(error.message().contains("encrypted"), "{error}");
}

#[test]
fn a_footer_or_chunk_past_the_limits_is_refused() {
    let bytes = testdata::write(
        testdata::PLACES_SCHEMA,
        &[testdata::place_leaves(&places())],
        &Writer::default(),
    );
    let small = Limits {
        max_footer: 16,
        ..Limits::standard()
    };
    assert!(ParquetFile::open(&bytes, &small, &all)
        .unwrap_err()
        .message()
        .contains("footer"));
    let file = ParquetFile::open(&bytes, &Limits::standard(), &all).unwrap();
    let tight = Limits {
        max_chunk: 8,
        ..Limits::standard()
    };
    let leaf = file.meta.leaf(&["id"]).unwrap();
    assert!(file.read_column(&bytes, 0, leaf, &tight).is_err());
}

#[test]
fn a_footer_that_lies_about_its_length_is_refused() {
    let mut bytes = testdata::write(
        testdata::PLACES_SCHEMA,
        &[testdata::place_leaves(&places())],
        &Writer::default(),
    );
    let end = bytes.len();
    bytes[end - 8..end - 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(ParquetFile::open(&bytes, &Limits::standard(), &all).is_err());
    let at = end - 8;
    bytes[at - 4..at].copy_from_slice(&[0xff; 4]);
    bytes[end - 8..end - 4].copy_from_slice(&40u32.to_le_bytes());
    assert!(ParquetFile::open(&bytes, &Limits::standard(), &all).is_err());
}

#[test]
fn a_corrupted_page_fails_without_a_panic() {
    let bytes = testdata::write(
        testdata::PLACES_SCHEMA,
        &[testdata::place_leaves(&places())],
        &Writer::default(),
    );
    let file = ParquetFile::open(&bytes, &Limits::standard(), &all).unwrap();
    for leaf in 0..file.meta.leaves.len() {
        let chunk = file.meta.row_groups[0].chunk(leaf).unwrap();
        let start = usize::try_from(chunk.start).unwrap();
        let size = usize::try_from(chunk.compressed_size).unwrap();
        for offset in 0..size {
            for value in [0x00, 0xff, 0x15] {
                let mut broken = bytes.clone();
                broken[start + offset] = value;
                // an error or other values, never a panic
                let _ = file.read_column(&broken, 0, leaf, &Limits::standard());
            }
        }
    }
}

#[test]
fn zstd_and_gzip_pages_decompress_to_their_declared_size() {
    let page: Vec<u8> = (0..5000u32).flat_map(|value| (value % 97).to_le_bytes()).collect();
    let zstd = ruzstd::encoding::compress_to_vec(page.as_slice(), ruzstd::encoding::CompressionLevel::Fastest);
    assert_eq!(decode::decompress(Codec::Zstd, &zstd, page.len()).unwrap(), page);
    assert!(decode::decompress(Codec::Zstd, &zstd, page.len() - 1).is_err());
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gzip, &page).unwrap();
    let gzip = gzip.finish().unwrap();
    assert_eq!(decode::decompress(Codec::Gzip, &gzip, page.len()).unwrap(), page);
    assert!(decode::decompress(Codec::Unsupported("BROTLI"), &gzip, page.len())
        .unwrap_err()
        .message()
        .contains("BROTLI"));
}

#[test]
fn snappy_refuses_a_copy_before_its_start_and_a_wrong_length() {
    // length 4, then a copy of 4 from offset 1 with nothing written
    assert!(snappy::decompress(&[4, 0b0000_0001, 1], 4).is_err());
    // a literal "ab" then a copy of 4 from offset 2: "ababab"
    assert_eq!(
        snappy::decompress(&[6, 0b0000_0100, b'a', b'b', 0b0000_0001, 2], 6).unwrap(),
        b"ababab"
    );
    assert!(snappy::decompress(&[6, 0b0000_0100, b'a', b'b'], 6).is_err());
    assert!(snappy::decompress(&[5, 0b0000_0100, b'a', b'b'], 6).is_err());
}

#[test]
fn hybrid_runs_and_delta_counts_are_bounded() {
    // an RLE run of 3 values of 5 (width 3), then a bit-packed group of 8 values
    let values = decode::hybrid(&[3 << 1, 5, 0b11, 0b1000_1000, 0b1100_0110, 0b1111_1010], 3, 11).unwrap();
    assert_eq!(values, vec![5, 5, 5, 0, 1, 2, 3, 4, 5, 6, 7]);
    assert!(decode::hybrid(&[0xff, 0xff, 0xff, 0xff, 0x0f], 1, 10).is_err());
    // a delta header claiming a million values in a page of ten
    let mut cursor = decode::Cursor::new(&[0x80, 0x01, 4, 0xc0, 0x84, 0x3d, 0]);
    assert!(decode::delta_binary_packed(&mut cursor, 10).is_err());
}

#[test]
fn a_dictionary_index_past_its_values_is_refused() {
    let dictionary = Values::Int32(vec![1, 2]);
    // width 2, an RLE run of one value 3
    assert!(decode::dictionary_indices(&[2, 1 << 1, 3], &dictionary, 1, 1 << 20).is_err());
    assert_eq!(
        decode::dictionary_indices(&[2, 1 << 1, 1], &dictionary, 1, 1 << 20).unwrap(),
        Values::Int32(vec![2])
    );
}

#[test]
fn many_rows_read_across_pages() {
    let rows: Vec<FixturePlace> = (0..500)
        .map(|index| {
            let mut place = FixturePlace::new("x", 11.0 + f64::from(index) / 1000.0, 43.0, "P", "cafe");
            place.version = Some(index);
            place
        })
        .collect();
    let leaves = testdata::place_leaves(&rows);
    let bytes = testdata::write(
        testdata::PLACES_SCHEMA,
        &[leaves],
        &Writer {
            page_rows: 37,
            ..Writer::default()
        },
    );
    let versions = read(&bytes, &["version"]);
    assert_eq!(versions.values, Values::Int32((0..500).collect()));
    let lists = read(&bytes, &["sources", "list", "element", "dataset"]).lists();
    assert_eq!(lists.len(), 500);
}

fn varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

#[test]
fn a_delta_header_claiming_a_huge_block_is_refused_before_any_allocation() {
    for (block, miniblocks) in [(1u64 << 40, 4u64), (1 << 20, 4), (128, 8), (128, 0), (256, 3)] {
        let mut bytes = Vec::new();
        varint(block, &mut bytes);
        varint(miniblocks, &mut bytes);
        varint(5, &mut bytes);
        varint(0, &mut bytes);
        bytes.extend_from_slice(&[0; 16]);
        let mut cursor = decode::Cursor::new(&bytes);
        assert!(
            decode::delta_binary_packed(&mut cursor, 10).is_err(),
            "{block} in {miniblocks}"
        );
    }
}

#[test]
fn a_width_zero_run_claiming_any_number_of_values_unpacks_only_those_wanted() {
    // a bit-packed run of 2^40 groups at width 0: no bytes, ten values wanted
    let mut bytes = Vec::new();
    varint((1u64 << 41) | 1, &mut bytes);
    assert_eq!(decode::hybrid(&bytes, 0, 10).unwrap(), vec![0; 10]);
    // a width past 32 bits
    assert!(decode::hybrid(&[2, 1], 40, 1).is_err());
}

#[test]
fn decoded_values_past_their_budget_are_refused() {
    let mut big = ByteArrays::default();
    big.push(&[7; 1000]);
    let dictionary = Values::Bytes(big);
    // ten indices of value 0: 10,000 bytes
    let indices = [1, 10 << 1, 0];
    assert!(decode::dictionary_indices(&indices, &dictionary, 10, 5000).is_err());
    assert_eq!(
        decode::dictionary_indices(&indices, &dictionary, 10, 20_000)
            .unwrap()
            .len(),
        10
    );
    let mut values = Values::Int64(vec![1; 100]);
    assert!(values.extend(Values::Int64(vec![2; 100]), 1000).is_err());
    assert!(values.extend(Values::Int64(vec![2; 100]), 2000).is_ok());
}

#[test]
fn a_schema_nested_too_deep_is_refused() {
    let depth = 40;
    let mut schema = String::from("message schema {");
    for level in 0..depth {
        schema.push_str(&format!(" OPTIONAL group g{level} {{"));
    }
    schema.push_str(" OPTIONAL INT32 x;");
    for _ in 0..=depth {
        schema.push_str(" }");
    }
    let bytes = testdata::write(
        &schema,
        &[vec![testdata::Leaf::Int32(vec![1], vec![depth + 1], Vec::new())]],
        &Writer::default(),
    );
    let error = ParquetFile::open(&bytes, &Limits::standard(), &all).unwrap_err();
    assert!(error.message().contains("nested"), "{error}");
}

#[test]
fn a_fixed_length_column_of_length_zero_is_refused() {
    // FileMetaData { version 1, schema [root "schema" of 1 child, FIXED_LEN_BYTE_ARRAY "x" of length 0], 0 rows, no groups }
    let mut footer = vec![0x15, 0x02, 0x19, 0x2c];
    footer.extend_from_slice(&[0x48, 6]);
    footer.extend_from_slice(b"schema");
    footer.extend_from_slice(&[0x15, 0x02, 0x00]);
    footer.extend_from_slice(&[0x15, 0x0e, 0x15, 0x00, 0x15, 0x02, 0x18, 1, b'x', 0x00]);
    footer.extend_from_slice(&[0x16, 0x00, 0x19, 0x0c, 0x00]);
    let error = meta::parse(&footer, &all).unwrap_err();
    assert!(error.message().contains("length 0"), "{error}");
}

#[test]
fn a_thrift_map_of_booleans_is_skipped_a_byte_each() {
    // a struct whose field 1 is a map<bool, bool> of two entries, then field 2 an i32 of 21
    let bytes = [0x1b, 2, 0x11, 1, 2, 2, 1, 0x15, 42, 0x00];
    let mut reader = thrift::Reader::new(&bytes);
    let mut seen = None;
    reader
        .read_struct(&mut |r, id, kind| {
            if id == 2 && kind == thrift::I32 {
                seen = Some(r.i32()?);
                return Ok(true);
            }
            Ok(false)
        })
        .unwrap();
    assert_eq!(seen, Some(21));
}
