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

//! A Parquet reader of the subset GeoParquet writers use (plans/archive/Maps-Overture.md §1), over a [`RangeSource`] that reads
//! a part of a file at a time (an HTTP range, a resource's `read_at`): the footer ([`meta`], in the Thrift compact
//! protocol, [`thrift`]) with only the column chunks asked for, then one column chunk of one row group at a time, its
//! pages decompressed and decoded ([`decode`]) into the column's levels and values ([`ColumnData`]). Data pages v1 and
//! v2, dictionary pages, and the encodings PLAIN, the dictionary's, RLE (booleans), the three DELTA ones and
//! BYTE_STREAM_SPLIT; codecs none, Snappy, GZIP and ZSTD. Encrypted files, columns in other files, and other codecs are
//! refused by name. Every size is bounded by [`Limits`], so a lying header costs nothing.

pub mod decode;
pub mod meta;
pub mod snappy;
pub mod thrift;

use moqui_lib::error::Error;

pub use decode::{ByteArrays, Values};
pub use meta::{Chunk, Codec, FileMeta, Leaf, Physical, RowGroup, Statistics};

/// The bytes of a file, a part at a time.
pub trait RangeSource {
    /// The file's length.
    fn size(&self) -> u64;
    /// `length` bytes from `offset` (fewer only at the end).
    ///
    /// # Errors
    ///
    /// The read fails.
    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>, Error>;
}

/// A file in memory, as a source (tests, a small file).
impl RangeSource for Vec<u8> {
    fn size(&self) -> u64 {
        self.len() as u64
    }

    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>, Error> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(self.len());
        let end = start.saturating_add(length).min(self.len());
        Ok(self.get(start..end).unwrap_or_default().to_vec())
    }
}

/// What reading may take.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The footer's bytes.
    pub max_footer: usize,
    /// One column chunk's compressed bytes.
    pub max_chunk: usize,
    /// One page's decompressed bytes.
    pub max_page: usize,
    /// The level entries (values and nulls) of one column chunk, and the rows of a row group.
    pub max_values: usize,
    /// The bytes one column chunk's values decode to.
    pub max_decoded: usize,
    /// The bytes the columns of one row group decode to, all of them read (the caller keeps the sum).
    pub max_group_decoded: usize,
}

impl Limits {
    /// Sized to Overture's files (row groups of 5,000 to 30,000 rows, a few MB a chunk) with room to spare: 64 MiB of
    /// footer, 64 MiB a chunk, 32 MiB a page, two million entries a chunk, 128 MiB decoded a chunk and 256 MiB a row
    /// group.
    #[must_use]
    pub fn standard() -> Self {
        Self {
            max_footer: 64 << 20,
            max_chunk: 64 << 20,
            max_page: 32 << 20,
            max_values: 2_000_000,
            max_decoded: 128 << 20,
            max_group_decoded: 256 << 20,
        }
    }
}

const MAGIC: &[u8; 4] = b"PAR1";
/// The first read of a file's end, which holds most footers of a small file and the length of any.
const TAIL: usize = 64 * 1024;

/// A file's footer, read.
#[derive(Clone, Debug)]
pub struct ParquetFile {
    pub meta: FileMeta,
    /// The bytes the footer took to read.
    pub footer_bytes: u64,
}

impl ParquetFile {
    /// Read the footer of `source`, keeping the chunks of the leaves for which `wanted` answers `Some` (and their
    /// statistics where it answers `Some(true)`).
    ///
    /// # Errors
    ///
    /// Not a Parquet file, an encrypted one, a footer past [`Limits::max_footer`] or one that does not read.
    pub fn open(
        source: &dyn RangeSource,
        limits: &Limits,
        wanted: &dyn Fn(&[String]) -> Option<bool>,
    ) -> Result<Self, Error> {
        let length = source.size();
        if length < 12 {
            return Err(Error::new("not a Parquet file: too short"));
        }
        let tail_size = usize::try_from(length).unwrap_or(usize::MAX).min(TAIL);
        let tail = source.read(length - tail_size as u64, tail_size)?;
        if tail.len() != tail_size {
            return Err(Error::new("a Parquet file shorter than its length"));
        }
        let (Some(end), Some(size_bytes)) = (tail.get(tail_size - 4..), tail.get(tail_size - 8..tail_size - 4)) else {
            return Err(Error::new("not a Parquet file: too short"));
        };
        if end == b"PARE" {
            return Err(Error::new("an encrypted Parquet file, which this reader does not read"));
        }
        if end != MAGIC {
            return Err(Error::new("not a Parquet file: no PAR1 at its end"));
        }
        let footer_size = u32::from_le_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]]) as usize;
        if footer_size > limits.max_footer {
            return Err(Error::new(format!(
                "a Parquet footer of {footer_size} bytes, more than {} allowed",
                limits.max_footer
            )));
        }
        if footer_size as u64 + 12 > length {
            return Err(Error::new("a Parquet footer longer than its file"));
        }
        let footer = if footer_size + 8 <= tail_size {
            tail.get(tail_size - 8 - footer_size..tail_size - 8)
                .unwrap_or_default()
                .to_vec()
        } else {
            // the part before the tail read, then the tail's part
            let before = footer_size + 8 - tail_size;
            let mut footer = source.read(length - (footer_size as u64 + 8), before)?;
            if footer.len() != before {
                return Err(Error::new("a Parquet file shorter than its length"));
            }
            footer.extend_from_slice(tail.get(..tail_size - 8).unwrap_or_default());
            footer
        };
        let meta = meta::parse(&footer, wanted)?;
        Ok(Self {
            meta,
            footer_bytes: (footer_size + 8).max(tail_size) as u64,
        })
    }

    /// The column chunk of leaf `leaf` in row group `group`: its pages read in one range and decoded.
    ///
    /// # Errors
    ///
    /// No such chunk, one past the limits, or pages that do not read.
    pub fn read_column(
        &self,
        source: &dyn RangeSource,
        group: usize,
        leaf: usize,
        limits: &Limits,
    ) -> Result<ColumnData, Error> {
        let (Some(row_group), Some(column)) = (self.meta.row_groups.get(group), self.meta.leaves.get(leaf)) else {
            return Err(Error::new(format!("no Parquet row group {group} or column {leaf}")));
        };
        let Some(chunk) = row_group.chunk(leaf) else {
            return Err(Error::new(format!(
                "the Parquet column {} has no chunk in row group {group}",
                column.dotted()
            )));
        };
        let size = usize::try_from(chunk.compressed_size).unwrap_or(usize::MAX);
        if size > limits.max_chunk {
            return Err(Error::new(format!(
                "the Parquet column chunk {} is {size} bytes, more than {} allowed",
                column.dotted(),
                limits.max_chunk
            )));
        }
        let bytes = source.read(chunk.start, size)?;
        if bytes.len() != size {
            return Err(Error::new(format!(
                "the Parquet column chunk {} ends early",
                column.dotted()
            )));
        }
        read_chunk(&bytes, column, chunk, limits)
            .map_err(|err| Error::new(format!("the Parquet column {}: {}", column.dotted(), err.message())))
    }
}

/// A column chunk's level entries and its values (the nulls left out): entry `i` is a value when `def[i]` is the
/// column's maximum, and starts a new row when `rep[i]` is 0. A column with no repetition has no `rep`, and one that is
/// required no `def`.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnData {
    pub max_def: i16,
    pub max_rep: i16,
    pub repeated_def: Vec<i16>,
    pub def: Vec<i16>,
    pub rep: Vec<i16>,
    pub values: Values,
}

/// A value of a row, by its index in [`ColumnData::values`].
pub type ValueIndex = usize;

impl ColumnData {
    /// The number of level entries.
    #[must_use]
    pub fn entries(&self) -> usize {
        if self.def.is_empty() {
            self.values.len()
        } else {
            self.def.len()
        }
    }

    /// For a column of no repetition: each row's value index, or `None` for a null.
    #[must_use]
    pub fn scalars(&self) -> Vec<Option<ValueIndex>> {
        if self.def.is_empty() {
            return (0..self.values.len()).map(Some).collect();
        }
        let mut next = 0;
        self.def
            .iter()
            .map(|def| {
                if *def == self.max_def {
                    next += 1;
                    Some(next - 1)
                } else {
                    None
                }
            })
            .collect()
    }

    /// For a column under one list: each row's elements, each a value index or `None` for a null element (a null or
    /// empty list has none).
    #[must_use]
    pub fn lists(&self) -> Vec<Vec<Option<ValueIndex>>> {
        let element_def = self.repeated_def.first().copied().unwrap_or(0);
        let mut rows: Vec<Vec<Option<ValueIndex>>> = Vec::new();
        let mut next = 0;
        for (index, def) in self.def.iter().enumerate() {
            if self.rep.get(index).copied().unwrap_or(0) == 0 {
                rows.push(Vec::new());
            }
            let Some(row) = rows.last_mut() else { continue };
            if *def >= element_def {
                if *def == self.max_def {
                    row.push(Some(next));
                    next += 1;
                } else {
                    row.push(None);
                }
            }
        }
        rows
    }
}

/// A page header, the fields read.
#[derive(Default)]
struct PageHeader {
    kind: i32,
    uncompressed: i32,
    compressed: i32,
    num_values: i32,
    encoding: i32,
    def_encoding: i32,
    rep_encoding: i32,
    v2_def_length: i32,
    v2_rep_length: i32,
    v2_compressed: bool,
}

fn page_header(reader: &mut thrift::Reader<'_>) -> Result<PageHeader, Error> {
    let mut out = PageHeader {
        v2_compressed: true,
        ..PageHeader::default()
    };
    reader.read_struct(&mut |r, id, kind| {
        match (id, kind) {
            (1, thrift::I32) => out.kind = r.i32()?,
            (2, thrift::I32) => out.uncompressed = r.i32()?,
            (3, thrift::I32) => out.compressed = r.i32()?,
            // DataPageHeader
            (5, thrift::STRUCT) => r.read_struct(&mut |r, id, kind| {
                match (id, kind) {
                    (1, thrift::I32) => out.num_values = r.i32()?,
                    (2, thrift::I32) => out.encoding = r.i32()?,
                    (3, thrift::I32) => out.def_encoding = r.i32()?,
                    (4, thrift::I32) => out.rep_encoding = r.i32()?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?,
            // DictionaryPageHeader
            (7, thrift::STRUCT) => r.read_struct(&mut |r, id, kind| {
                match (id, kind) {
                    (1, thrift::I32) => out.num_values = r.i32()?,
                    (2, thrift::I32) => out.encoding = r.i32()?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?,
            // DataPageHeaderV2
            (8, thrift::STRUCT) => r.read_struct(&mut |r, id, kind| {
                match (id, kind) {
                    (1, thrift::I32) => out.num_values = r.i32()?,
                    (4, thrift::I32) => out.encoding = r.i32()?,
                    (5, thrift::I32) => out.v2_def_length = r.i32()?,
                    (6, thrift::I32) => out.v2_rep_length = r.i32()?,
                    (7, thrift::TRUE | thrift::FALSE) => out.v2_compressed = thrift::Reader::bool_of(kind),
                    _ => return Ok(false),
                }
                Ok(true)
            })?,
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    Ok(out)
}

const DATA_PAGE: i32 = 0;
const INDEX_PAGE: i32 = 1;
const DICTIONARY_PAGE: i32 = 2;
const DATA_PAGE_V2: i32 = 3;

const PLAIN: i32 = 0;
const PLAIN_DICTIONARY: i32 = 2;
const RLE: i32 = 3;
const DELTA_BINARY_PACKED: i32 = 5;
const DELTA_LENGTH_BYTE_ARRAY: i32 = 6;
const DELTA_BYTE_ARRAY: i32 = 7;
const RLE_DICTIONARY: i32 = 8;
const BYTE_STREAM_SPLIT: i32 = 9;

fn size_of(value: i32, what: &str, max: usize) -> Result<usize, Error> {
    usize::try_from(value)
        .ok()
        .filter(|size| *size <= max)
        .ok_or_else(|| Error::new(format!("a page whose {what} is {value} bytes (at most {max})")))
}

/// Decode the pages of a column chunk.
fn read_chunk(bytes: &[u8], leaf: &Leaf, chunk: &Chunk, limits: &Limits) -> Result<ColumnData, Error> {
    let total = usize::try_from(chunk.num_values).unwrap_or(usize::MAX);
    if total > limits.max_values {
        return Err(Error::new(format!(
            "a chunk of {total} values, more than {} allowed",
            limits.max_values
        )));
    }
    let mut out = ColumnData {
        max_def: leaf.max_def,
        max_rep: leaf.max_rep,
        repeated_def: leaf.repeated_def.clone(),
        def: Vec::new(),
        rep: Vec::new(),
        values: Values::empty(leaf.physical),
    };
    let mut dictionary: Option<Values> = None;
    let mut at = 0usize;
    let mut entries = 0usize;
    while entries < total {
        let Some(rest) = bytes.get(at..).filter(|rest| !rest.is_empty()) else {
            return Err(Error::new(format!(
                "pages of {entries} values where the chunk says {total}"
            )));
        };
        let mut reader = thrift::Reader::new(rest);
        let header = page_header(&mut reader)?;
        at += reader.position();
        let compressed = size_of(header.compressed, "compressed size", limits.max_chunk)?;
        let uncompressed = size_of(header.uncompressed, "size", limits.max_page)?;
        let body = bytes
            .get(at..at + compressed)
            .ok_or_else(|| Error::new("a page that ends past its chunk"))?;
        at += compressed;
        match header.kind {
            DICTIONARY_PAGE => {
                if !matches!(header.encoding, PLAIN | PLAIN_DICTIONARY) {
                    return Err(Error::new(format!("a dictionary page of encoding {}", header.encoding)));
                }
                let count = usize::try_from(header.num_values).unwrap_or(usize::MAX);
                if count > limits.max_values {
                    return Err(Error::new("a dictionary of too many values"));
                }
                let page = decode::decompress(chunk.codec, body, uncompressed)?;
                dictionary = Some(decode::plain(&page, leaf.physical, leaf.type_length, count)?);
            }
            DATA_PAGE | DATA_PAGE_V2 => {
                let count = usize::try_from(header.num_values).unwrap_or(usize::MAX);
                if count > total - entries {
                    return Err(Error::new(format!("a page of {count} values past the chunk's {total}")));
                }
                let (rep, def, values_bytes) = if header.kind == DATA_PAGE {
                    let page = decode::decompress(chunk.codec, body, uncompressed)?;
                    let mut offset = 0;
                    let rep = if leaf.max_rep > 0 {
                        if header.rep_encoding != RLE {
                            return Err(Error::new(format!(
                                "repetition levels of encoding {}",
                                header.rep_encoding
                            )));
                        }
                        let (levels, used) = decode::levels_v1(&page, leaf.max_rep, count)?;
                        offset += used;
                        levels
                    } else {
                        Vec::new()
                    };
                    let def = if leaf.max_def > 0 {
                        if header.def_encoding != RLE {
                            return Err(Error::new(format!(
                                "definition levels of encoding {}",
                                header.def_encoding
                            )));
                        }
                        let (levels, used) =
                            decode::levels_v1(page.get(offset..).unwrap_or_default(), leaf.max_def, count)?;
                        offset += used;
                        levels
                    } else {
                        Vec::new()
                    };
                    (rep, def, page.get(offset..).unwrap_or_default().to_vec())
                } else {
                    let rep_length = size_of(header.v2_rep_length, "repetition levels", compressed)?;
                    let def_length = size_of(header.v2_def_length, "definition levels", compressed)?;
                    let levels_length = rep_length + def_length;
                    if levels_length > compressed || levels_length > uncompressed {
                        return Err(Error::new("a page whose levels are longer than the page"));
                    }
                    let rep = if leaf.max_rep > 0 {
                        decode::levels(body.get(..rep_length).unwrap_or_default(), leaf.max_rep, count)?
                    } else {
                        Vec::new()
                    };
                    let def = if leaf.max_def > 0 {
                        decode::levels(
                            body.get(rep_length..levels_length).unwrap_or_default(),
                            leaf.max_def,
                            count,
                        )?
                    } else {
                        Vec::new()
                    };
                    let values = body.get(levels_length..).unwrap_or_default();
                    let values = if header.v2_compressed {
                        decode::decompress(chunk.codec, values, uncompressed - levels_length)?
                    } else {
                        values.to_vec()
                    };
                    (rep, def, values)
                };
                let present = if leaf.max_def > 0 {
                    def.iter().filter(|level| **level == leaf.max_def).count()
                } else {
                    count
                };
                let values = match header.encoding {
                    PLAIN => decode::plain(&values_bytes, leaf.physical, leaf.type_length, present)?,
                    PLAIN_DICTIONARY | RLE_DICTIONARY => {
                        let Some(dictionary) = &dictionary else {
                            return Err(Error::new("dictionary indices with no dictionary page"));
                        };
                        let room = limits.max_decoded.saturating_sub(out.values.decoded_bytes());
                        decode::dictionary_indices(&values_bytes, dictionary, present, room)?
                    }
                    RLE if leaf.physical == Physical::Boolean => {
                        let mut cursor = decode::Cursor::new(&values_bytes);
                        let length = cursor.u32_le()? as usize;
                        let bits = decode::hybrid(cursor.take(length)?, 1, present)?;
                        Values::Boolean(bits.into_iter().map(|bit| bit == 1).collect())
                    }
                    DELTA_BINARY_PACKED => {
                        let mut cursor = decode::Cursor::new(&values_bytes);
                        let numbers = decode::delta_binary_packed(&mut cursor, present)?;
                        match leaf.physical {
                            // an INT32 column's deltas wrap within 32 bits
                            #[allow(clippy::cast_possible_truncation)]
                            Physical::Int32 => Values::Int32(numbers.into_iter().map(|value| value as i32).collect()),
                            Physical::Int64 => Values::Int64(numbers),
                            other => return Err(Error::new(format!("DELTA_BINARY_PACKED {other:?} values"))),
                        }
                    }
                    DELTA_LENGTH_BYTE_ARRAY => decode::delta_length_byte_array(&values_bytes, present)?,
                    DELTA_BYTE_ARRAY => decode::delta_byte_array(&values_bytes, present, limits.max_page)?,
                    BYTE_STREAM_SPLIT => {
                        decode::byte_stream_split(&values_bytes, leaf.physical, leaf.type_length, present)?
                    }
                    other => {
                        return Err(Error::new(format!(
                            "values of encoding {other}, which this reader does not read"
                        )))
                    }
                };
                if values.len() != present {
                    return Err(Error::new(format!(
                        "a page of {} values where its levels say {present}",
                        values.len()
                    )));
                }
                out.rep.extend(rep);
                out.def.extend(def);
                out.values.extend(values, limits.max_decoded)?;
                entries += count;
            }
            INDEX_PAGE => {}
            other => return Err(Error::new(format!("a page of unknown type {other}"))),
        }
    }
    Ok(out)
}

/// A row group's statistics of a double column (GeoParquet's `bbox.xmin` and its kind): its least and greatest value.
#[must_use]
pub fn double_range(statistics: Option<&Statistics>, physical: Physical) -> Option<(f64, f64)> {
    let statistics = statistics?;
    let value = |bytes: &Vec<u8>| match physical {
        Physical::Double => bytes
            .get(..8)
            .map(|b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])),
        Physical::Float => bytes
            .get(..4)
            .map(|b| f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
        _ => None,
    };
    let (min, max) = (value(statistics.min.as_ref()?)?, value(statistics.max.as_ref()?)?);
    (min.is_finite() && max.is_finite()).then_some((min, max))
}

#[cfg(test)]
mod tests;
