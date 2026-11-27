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

//! The GeoTIFF header reader on files made here: a COG's header in classic and BigTIFF, both byte orders, and the
//! malformed and hostile ones (a loop, a directory past the end, a count that overflows, too many directories or
//! entries, strips, a user-defined CRS).

use super::*;

/// A tag's value as written
#[derive(Clone)]
enum V {
    Short(Vec<u16>),
    Long(Vec<u32>),
    Double(Vec<f64>),
    Ascii(String),
}

struct Writer {
    little: bool,
    big: bool,
}

impl Writer {
    fn u16(&self, out: &mut Vec<u8>, value: u16) {
        out.extend(if self.little {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        });
    }
    fn u32(&self, out: &mut Vec<u8>, value: u32) {
        out.extend(if self.little {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        });
    }
    fn u64(&self, out: &mut Vec<u8>, value: u64) {
        out.extend(if self.little {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        });
    }
    fn offset(&self, out: &mut Vec<u8>, value: u64) {
        if self.big {
            self.u64(out, value);
        } else {
            self.u32(out, u32::try_from(value).expect("small"));
        }
    }

    fn value_bytes(&self, value: &V) -> (u16, u64, Vec<u8>) {
        let mut out = Vec::new();
        match value {
            V::Short(items) => {
                items.iter().for_each(|item| self.u16(&mut out, *item));
                (3, items.len() as u64, out)
            }
            V::Long(items) => {
                items.iter().for_each(|item| self.u32(&mut out, *item));
                (4, items.len() as u64, out)
            }
            V::Double(items) => {
                items.iter().for_each(|item| self.u64(&mut out, item.to_bits()));
                (12, items.len() as u64, out)
            }
            V::Ascii(text) => {
                out.extend(text.as_bytes());
                out.push(0);
                (2, out.len() as u64, out)
            }
        }
    }

    /// A file of these directories, one after another, each with its values after it; `next_override` replaces the
    /// next-directory offset of a directory by its index
    fn file(&self, ifds: &[Vec<(u16, V)>], next_override: &[(usize, u64)]) -> Vec<u8> {
        self.file_from(ifds, next_override, if self.big { 16 } else { 8 })
    }

    /// As [`Writer::file`], the first directory at `first` (zeros before it)
    fn file_from(&self, ifds: &[Vec<(u16, V)>], next_override: &[(usize, u64)], first: u64) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(if self.little { b"II" } else { b"MM" });
        if self.big {
            self.u16(&mut out, 43);
            self.u16(&mut out, 8);
            self.u16(&mut out, 0);
            self.u64(&mut out, first);
        } else {
            self.u16(&mut out, 42);
            self.u32(&mut out, u32::try_from(first).expect("small"));
        }
        out.resize(usize::try_from(first).expect("small"), 0);
        let (count_len, entry_len, field_len, next_len) = if self.big { (8, 20, 8, 8) } else { (2, 12, 4, 4) };
        for (index, ifd) in ifds.iter().enumerate() {
            let start = out.len() as u64;
            let table_len = count_len + ifd.len() as u64 * entry_len + next_len;
            let mut extra_at = start + table_len;
            let mut table = Vec::new();
            let mut extra = Vec::new();
            if self.big {
                self.u64(&mut table, ifd.len() as u64);
            } else {
                self.u16(&mut table, u16::try_from(ifd.len()).expect("small"));
            }
            for (tag, value) in ifd {
                let (kind, count, bytes) = self.value_bytes(value);
                self.u16(&mut table, *tag);
                self.u16(&mut table, kind);
                if self.big {
                    self.u64(&mut table, count);
                } else {
                    self.u32(&mut table, u32::try_from(count).expect("small"));
                }
                if bytes.len() <= field_len {
                    let mut field = bytes.clone();
                    field.resize(field_len, 0);
                    table.extend(field);
                } else {
                    self.offset(&mut table, extra_at);
                    extra_at += bytes.len() as u64;
                    extra.extend(bytes);
                }
            }
            let next = next_override
                .iter()
                .find(|(at, _)| *at == index)
                .map(|(_, next)| *next)
                .unwrap_or(if index + 1 < ifds.len() { extra_at } else { 0 });
            self.offset(&mut table, next);
            out.extend(table);
            out.extend(extra);
        }
        // the tiles' bytes would follow: a little of something
        out.extend([0u8; 64]);
        out
    }
}

/// The main image of a 4096 by 2048 RGB uint8 COG in UTM zone 33N, tiled 512, with GDAL's no-data and statistics
fn main_ifd(width: u32, height: u32, tile: u32) -> Vec<(u16, V)> {
    let tiles = width.div_ceil(tile) * height.div_ceil(tile);
    vec![
        (256, V::Long(vec![width])),
        (257, V::Long(vec![height])),
        (258, V::Short(vec![8, 8, 8])),
        (259, V::Short(vec![8])),
        (262, V::Short(vec![2])),
        (277, V::Short(vec![3])),
        (284, V::Short(vec![1])),
        (322, V::Short(vec![u16::try_from(tile).expect("tile")])),
        (323, V::Short(vec![u16::try_from(tile).expect("tile")])),
        (324, V::Long(vec![100; tiles as usize])),
        (325, V::Long(vec![10; tiles as usize])),
        (339, V::Short(vec![1, 1, 1])),
        (33550, V::Double(vec![10.0, 10.0, 0.0])),
        (33922, V::Double(vec![0.0, 0.0, 0.0, 500_000.0, 6_000_000.0, 0.0])),
        (34735, V::Short(vec![1, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, 1, 3072, 0, 1, 32633])),
        (
            42112,
            V::Ascii(
                "<GDALMetadata><Item name=\"STATISTICS_MAXIMUM\" sample=\"0\">250</Item><Item name=\"STATISTICS_MINIMUM\" sample=\"0\">3</Item></GDALMetadata>"
                    .to_string(),
            ),
        ),
        (42113, V::Ascii("0".to_string())),
    ]
}

fn overview(width: u32, height: u32, tile: u32) -> Vec<(u16, V)> {
    let tiles = width.div_ceil(tile) * height.div_ceil(tile);
    vec![
        (254, V::Long(vec![1])),
        (256, V::Long(vec![width])),
        (257, V::Long(vec![height])),
        (258, V::Short(vec![8, 8, 8])),
        (259, V::Short(vec![8])),
        (277, V::Short(vec![3])),
        (322, V::Short(vec![u16::try_from(tile).expect("tile")])),
        (323, V::Short(vec![u16::try_from(tile).expect("tile")])),
        (324, V::Long(vec![100; tiles as usize])),
        (325, V::Long(vec![10; tiles as usize])),
    ]
}

fn read(file: &[u8]) -> Result<CogInfo, String> {
    let mut reads = 0;
    let mut read_at = |offset: u64, len: usize| -> Result<Vec<u8>, String> {
        reads += 1;
        assert!(reads < 300, "a bounded number of reads");
        let start = usize::try_from(offset).map_err(|_| "offset".to_string())?;
        let end = start.saturating_add(len).min(file.len());
        Ok(file.get(start..end).unwrap_or_default().to_vec())
    };
    read_cog_header(&mut read_at, file.len() as u64)
}

fn with(mut ifd: Vec<(u16, V)>, tag: u16, value: Option<V>) -> Vec<(u16, V)> {
    ifd.retain(|(each, _)| *each != tag);
    if let Some(value) = value {
        ifd.push((tag, value));
        ifd.sort_by_key(|(each, _)| *each);
    }
    ifd
}

#[test]
fn a_cog_in_each_byte_order_and_bigtiff() {
    for (little, big) in [(true, false), (false, false), (true, true), (false, true)] {
        let writer = Writer { little, big };
        let file = writer.file(
            &[
                main_ifd(4096, 2048, 512),
                overview(2048, 1024, 512),
                overview(1024, 512, 512),
            ],
            &[],
        );
        let info = read(&file).expect("a TIFF");
        assert!(info.problems.is_empty(), "{little} {big}: {:?}", info.problems);
        assert_eq!(info.big_tiff, big);
        assert_eq!(
            (info.width, info.height, info.bands, info.bits_per_sample),
            (4096, 2048, 3, 8)
        );
        assert_eq!(info.sample_type(), "uint8");
        assert_eq!(info.compression_name(), "Deflate");
        assert_eq!((info.tile_width, info.tile_height), (512, 512));
        assert_eq!(info.overviews.len(), 2);
        assert_eq!(info.epsg, Some(32633));
        assert!(!info.geographic);
        assert_eq!(
            info.bounds,
            Some([500_000.0, 6_000_000.0 - 20_480.0, 500_000.0 + 40_960.0, 6_000_000.0])
        );
        assert_eq!(info.resolution, Some([10.0, 10.0]));
        assert_eq!(info.nodata.as_deref(), Some("0"));
        assert_eq!(info.statistics, vec![(1, 3.0, 250.0)]);
    }
}

#[test]
fn a_map_cannot_draw_these() {
    let writer = Writer {
        little: true,
        big: false,
    };
    let problems = |ifds: &[Vec<(u16, V)>]| read(&writer.file(ifds, &[])).expect("a TIFF").problems.join("; ");
    // strips, not tiles
    let striped = with(with(main_ifd(1024, 1024, 512), 322, None), 323, None);
    let striped = with(striped, 273, Some(V::Long(vec![100])));
    assert!(problems(&[striped]).contains("strips"));
    // a large image with no overview
    assert!(problems(&[main_ifd(4096, 2048, 512)]).contains("no overview"));
    // a small one needs none
    assert!(problems(&[main_ifd(1024, 512, 512)]).is_empty());
    // an odd tile size, an unknown compression, 12-bit samples
    assert!(problems(&[main_ifd(1000, 1000, 250)]).contains("multiple of 16"));
    assert!(problems(&[with(main_ifd(1024, 1024, 512), 259, Some(V::Short(vec![34713])))]).contains("compression"));
    assert!(problems(&[with(main_ifd(1024, 1024, 512), 258, Some(V::Short(vec![12, 12, 12])))]).contains("12 bits"));
    // no keys, a user-defined CRS, a rotated georeference, a wrong tile count
    assert!(problems(&[with(main_ifd(1024, 1024, 512), 34735, None)]).contains("no GeoTIFF keys"));
    let user = with(
        main_ifd(1024, 1024, 512),
        34735,
        Some(V::Short(vec![1, 1, 0, 2, 1024, 0, 1, 1, 3072, 0, 1, 32767])),
    );
    assert!(problems(&[user]).contains("user-defined"));
    let rotated = with(
        with(with(main_ifd(1024, 1024, 512), 33550, None), 33922, None),
        34264,
        Some(V::Double(vec![
            10.0, 1.0, 0.0, 0.0, 0.0, -10.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ])),
    );
    assert!(problems(&[rotated]).contains("rotated"));
    assert!(problems(&[with(main_ifd(1024, 1024, 512), 324, Some(V::Long(vec![1; 3])))]).contains("tile offsets"));
    // a geographic one: EPSG:4326 by its GeographicTypeGeoKey
    let geographic = with(
        main_ifd(1024, 512, 512),
        34735,
        Some(V::Short(vec![1, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, 4326])),
    );
    let info = read(&writer.file(&[geographic], &[])).expect("a TIFF");
    assert_eq!((info.epsg, info.geographic), (Some(4326), true));
}

#[test]
fn hostile_files_are_refused_within_bounds() {
    let writer = Writer {
        little: true,
        big: false,
    };
    assert!(read(b"II*\0").is_err(), "too short");
    assert!(read(b"PK\x03\x04aaaaaaaaaaaa")
        .expect_err("a zip")
        .contains("not a TIFF"));
    assert!(read(b"II\x2b\0\x04\0\0\0\x08\0\0\0\0\0\0\0")
        .expect_err("a bad BigTIFF")
        .contains("BigTIFF"));
    // a directory that points back to itself
    let looped = writer.file(&[main_ifd(1024, 1024, 512)], &[(0, 8)]);
    assert!(read(&looped).expect_err("a loop").contains("loop"));
    // a next directory past the end
    let past = writer.file(&[main_ifd(1024, 1024, 512)], &[(0, 1 << 30)]);
    assert!(read(&past).expect_err("past the end").contains("past the file's end"));
    // a first directory past the end
    let mut first = writer.file(&[main_ifd(1024, 1024, 512)], &[]);
    first[4..8].copy_from_slice(&0x7fff_fff0u32.to_le_bytes());
    assert!(read(&first).is_err());
    // a count that would overflow when multiplied by its type's size (BigTIFF's 64-bit counts)
    let big = Writer {
        little: true,
        big: true,
    };
    let mut huge = big.file(&[main_ifd(1024, 1024, 512)], &[]);
    // the first entry's count (ImageWidth): after the 16-byte header, the 8-byte entry count, then tag and type
    huge[16 + 8 + 4..16 + 8 + 12].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(read(&huge).is_err(), "a count past every limit");
    // a directory of too many entries
    let mut many = writer.file(&[main_ifd(1024, 1024, 512)], &[]);
    many[8..10].copy_from_slice(&5000u16.to_le_bytes());
    assert!(read(&many).expect_err("many entries").contains("entries"));
    // more directories than are read: a chain of tiny ones
    let tiny: Vec<Vec<(u16, V)>> = (0..80).map(|_| vec![(254, V::Long(vec![1]))]).collect();
    let mut chain = vec![main_ifd(1024, 1024, 512)];
    chain.extend(tiny);
    assert!(read(&writer.file(&chain, &[]))
        .expect_err("too many")
        .contains("more than"));
    // an image of absurd size
    assert!(read(&writer.file(
        &[with(main_ifd(1024, 1024, 512), 256, Some(V::Long(vec![u32::MAX])))],
        &[]
    ))
    .is_err());
    // a GeoKeyDirectory of LONG values, its count past every bound: not a GeoTIFF's, and no overflow
    let long_keys = with(
        main_ifd(1024, 1024, 512),
        34735,
        Some(V::Long(vec![1, 1, 0, u32::MAX, 1024, 0, 1, 1])),
    );
    let info = read(&writer.file(&[long_keys], &[])).expect("a TIFF");
    assert!(
        info.problems.iter().any(|problem| problem.contains("SHORT")),
        "{:?}",
        info.problems
    );
    // a SHORT one that says it has more keys than it holds
    let short_keys = with(
        main_ifd(1024, 1024, 512),
        34735,
        Some(V::Short(vec![1, 1, 0, 65535, 1024, 0, 1, 1])),
    );
    let info = read(&writer.file(&[short_keys], &[])).expect("a TIFF");
    assert!(
        info.problems
            .iter()
            .any(|problem| problem.contains("shorter than it says")),
        "{:?}",
        info.problems
    );
    // directories past the first 64 KiB that need more than 1 MiB read: refused at the budget, not read whole
    let big = Writer {
        little: true,
        big: true,
    };
    let wide: Vec<(u16, V)> = (0..1000u16).map(|n| (40000 + n, V::Short(vec![n]))).collect();
    let mut many_wide = vec![main_ifd(1024, 1024, 512)];
    many_wide.extend((0..60).map(|_| wide.clone()));
    let far = big.file_from(&many_wide, &[], 70_000);
    assert!(
        read(&far).expect_err("past the budget").contains("1048576"),
        "the read budget is named"
    );
    // a tile offsets array too long for its tag to be read is still only counted, never read
    let wide = main_ifd(16_384, 16_384, 16);
    assert!(!read(&writer.file(&[wide], &[]))
        .expect("counted, not read")
        .problems
        .iter()
        .any(|p| p.contains("offsets")));
}
