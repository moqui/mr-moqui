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

//! A Parquet file's footer (`FileMetaData`): the schema as its leaf columns, each with its path, physical type and
//! maximum definition and repetition levels, and the row groups with the column chunks of the leaves asked for. The
//! chunks of other leaves are read past and dropped, so a footer of thousands of chunks keeps only what a reader uses.

use moqui_lib::error::Error;

use super::thrift::{self, Reader};

/// A physical type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Physical {
    Boolean,
    Int32,
    Int64,
    Int96,
    Float,
    Double,
    ByteArray,
    FixedLenByteArray,
}

impl Physical {
    fn of(code: i32) -> Result<Self, Error> {
        Ok(match code {
            0 => Self::Boolean,
            1 => Self::Int32,
            2 => Self::Int64,
            3 => Self::Int96,
            4 => Self::Float,
            5 => Self::Double,
            6 => Self::ByteArray,
            7 => Self::FixedLenByteArray,
            other => return Err(Error::new(format!("a Parquet column of unknown physical type {other}"))),
        })
    }
}

/// A leaf column of the schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    /// The names from the root's child down to the leaf (`sources`, `list`, `element`, `license`).
    pub path: Vec<String>,
    pub physical: Physical,
    /// A fixed-length byte array's length.
    pub type_length: usize,
    pub max_def: i16,
    pub max_rep: i16,
    /// The definition level of each repeated field above or at the leaf, outermost first: an entry whose level reaches
    /// it is an element of that list.
    pub repeated_def: Vec<i16>,
}

impl Leaf {
    /// The path joined with dots, as Parquet tools print it.
    #[must_use]
    pub fn dotted(&self) -> String {
        self.path.join(".")
    }
}

/// A column chunk's statistics, the bytes as written (a double's eight little-endian bytes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Statistics {
    pub min: Option<Vec<u8>>,
    pub max: Option<Vec<u8>>,
    pub null_count: Option<i64>,
}

/// A compression codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Uncompressed,
    Snappy,
    Gzip,
    Zstd,
    /// One this reader does not read (LZO, Brotli, LZ4), by its name.
    Unsupported(&'static str),
}

impl Codec {
    fn of(code: i32) -> Self {
        match code {
            0 => Self::Uncompressed,
            1 => Self::Snappy,
            2 => Self::Gzip,
            3 => Self::Unsupported("LZO"),
            4 => Self::Unsupported("BROTLI"),
            5 => Self::Unsupported("LZ4"),
            6 => Self::Zstd,
            7 => Self::Unsupported("LZ4_RAW"),
            _ => Self::Unsupported("an unknown codec"),
        }
    }
}

/// A column chunk of a leaf asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub leaf: usize,
    pub codec: Codec,
    pub num_values: i64,
    /// Where its pages start (its dictionary page, when it has one) and their bytes, headers included.
    pub start: u64,
    pub compressed_size: u64,
    pub statistics: Option<Statistics>,
}

/// A row group: its rows, and the chunks of the leaves asked for, by leaf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowGroup {
    pub num_rows: i64,
    pub chunks: Vec<Chunk>,
}

impl RowGroup {
    #[must_use]
    pub fn chunk(&self, leaf: usize) -> Option<&Chunk> {
        self.chunks.iter().find(|chunk| chunk.leaf == leaf)
    }
}

/// A file's footer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileMeta {
    pub num_rows: i64,
    pub leaves: Vec<Leaf>,
    pub row_groups: Vec<RowGroup>,
    pub key_values: Vec<(String, String)>,
    pub created_by: Option<String>,
}

impl FileMeta {
    /// The index of the leaf at `path`.
    #[must_use]
    pub fn leaf(&self, path: &[&str]) -> Option<usize> {
        self.leaves
            .iter()
            .position(|leaf| leaf.path.len() == path.len() && leaf.path.iter().zip(path).all(|(a, b)| a == b))
    }

    /// The value of key-value metadata `key` (GeoParquet's `geo`).
    #[must_use]
    pub fn key_value(&self, key: &str) -> Option<&str> {
        self.key_values
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

/// A schema element as written.
#[derive(Default)]
struct Element {
    physical: Option<i32>,
    type_length: i32,
    repetition: i32,
    name: String,
    children: i32,
}

/// The most schema elements a footer may have (Overture's have about 100).
const MAX_ELEMENTS: usize = 20_000;
/// How deep the schema's groups may nest (Overture's go six deep).
const MAX_SCHEMA_DEPTH: usize = 32;
/// The longest leaf path, its names' bytes together: each leaf keeps its own.
const MAX_PATH_BYTES: usize = 1024;

fn element(reader: &mut Reader<'_>) -> Result<Element, Error> {
    let mut out = Element::default();
    reader.read_struct(&mut |r, id, kind| {
        match (id, kind) {
            (1, thrift::I32) => out.physical = Some(r.i32()?),
            (2, thrift::I32) => out.type_length = r.i32()?,
            (3, thrift::I32) => out.repetition = r.i32()?,
            (4, thrift::BINARY) => out.name = r.string()?,
            (5, thrift::I32) => out.children = r.i32()?,
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    Ok(out)
}

/// The leaves of the schema elements, depth first as written.
fn leaves(elements: &[Element]) -> Result<Vec<Leaf>, Error> {
    struct Frame {
        left: i32,
        def: i16,
        rep: i16,
        path_len: usize,
        repeated_len: usize,
    }
    let Some(root) = elements.first() else {
        return Err(Error::new("a Parquet file with no schema"));
    };
    let mut out = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut repeated: Vec<i16> = Vec::new();
    let mut stack = vec![Frame {
        left: root.children,
        def: 0,
        rep: 0,
        path_len: 0,
        repeated_len: 0,
    }];
    for element in elements.iter().skip(1) {
        // the frames whose children are all read
        while stack.last().is_some_and(|frame| frame.left == 0) {
            stack.pop();
        }
        let Some(parent) = stack.last_mut() else {
            return Err(Error::new("a Parquet schema with more elements than its root holds"));
        };
        parent.left -= 1;
        let (def, rep, path_len, repeated_len) = (parent.def, parent.rep, parent.path_len, parent.repeated_len);
        path.truncate(path_len);
        repeated.truncate(repeated_len);
        path.push(element.name.clone());
        if path.iter().map(String::len).sum::<usize>() > MAX_PATH_BYTES {
            return Err(Error::new("a Parquet schema with a field path longer than 1,024 bytes"));
        }
        let deeper = |level: i16| {
            level
                .checked_add(1)
                .ok_or_else(|| Error::new("a Parquet schema nested too deep"))
        };
        let (def, rep) = match element.repetition {
            0 => (def, rep),
            1 => (deeper(def)?, rep),
            2 => {
                let def = deeper(def)?;
                repeated.push(def);
                (def, deeper(rep)?)
            }
            other => return Err(Error::new(format!("a Parquet field of unknown repetition {other}"))),
        };
        if element.children > 0 {
            if stack.len() >= MAX_SCHEMA_DEPTH {
                return Err(Error::new(format!(
                    "a Parquet schema nested more than {MAX_SCHEMA_DEPTH} deep"
                )));
            }
            stack.push(Frame {
                left: element.children,
                def,
                rep,
                path_len: path.len(),
                repeated_len: repeated.len(),
            });
            continue;
        }
        let Some(physical) = element.physical else {
            return Err(Error::new(format!(
                "the Parquet group {} has no fields",
                path.join(".")
            )));
        };
        let physical = Physical::of(physical)?;
        let type_length = usize::try_from(element.type_length).unwrap_or(0);
        if physical == Physical::FixedLenByteArray && type_length == 0 {
            return Err(Error::new(format!(
                "the Parquet column {} is a fixed-length byte array of length {}",
                path.join("."),
                element.type_length
            )));
        }
        out.push(Leaf {
            path: path.clone(),
            physical,
            type_length,
            max_def: def,
            max_rep: rep,
            repeated_def: repeated.clone(),
        });
    }
    if stack.iter().any(|frame| frame.left > 0) {
        return Err(Error::new("a Parquet schema with fewer elements than its groups claim"));
    }
    Ok(out)
}

fn statistics(reader: &mut Reader<'_>) -> Result<Statistics, Error> {
    let mut out = Statistics::default();
    let (mut old_min, mut old_max) = (None, None);
    reader.read_struct(&mut |r, id, kind| {
        match (id, kind) {
            (1, thrift::BINARY) => old_max = Some(r.binary()?.to_vec()),
            (2, thrift::BINARY) => old_min = Some(r.binary()?.to_vec()),
            (3, thrift::I64) => out.null_count = Some(r.i64()?),
            (5, thrift::BINARY) => out.max = Some(r.binary()?.to_vec()),
            (6, thrift::BINARY) => out.min = Some(r.binary()?.to_vec()),
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    // the deprecated min and max are signed comparisons, right for the doubles statistics are read for here
    if out.min.is_none() {
        out.min = old_min;
    }
    if out.max.is_none() {
        out.max = old_max;
    }
    Ok(out)
}

/// A column chunk as written, before it is matched to a leaf.
#[derive(Default)]
struct RawChunk {
    path: Vec<String>,
    codec: i32,
    num_values: i64,
    data_page_offset: i64,
    dictionary_page_offset: Option<i64>,
    compressed_size: i64,
    statistics: Option<Statistics>,
    elsewhere: bool,
}

fn column_meta(
    reader: &mut Reader<'_>,
    out: &mut RawChunk,
    wanted: &dyn Fn(&[String]) -> Option<bool>,
) -> Result<(), Error> {
    // path_in_schema (3) is written before the statistics (12), so whether they are kept is known in time
    reader.read_struct(&mut |r, id, kind| {
        match (id, kind) {
            (3, thrift::LIST) => {
                let mut path = Vec::new();
                r.read_list(&mut |r, _| {
                    path.push(r.string()?);
                    Ok(())
                })?;
                out.path = path;
            }
            (4, thrift::I32) => out.codec = r.i32()?,
            (5, thrift::I64) => out.num_values = r.i64()?,
            (7, thrift::I64) => out.compressed_size = r.i64()?,
            (9, thrift::I64) => out.data_page_offset = r.i64()?,
            (11, thrift::I64) => out.dictionary_page_offset = Some(r.i64()?),
            (12, thrift::STRUCT) if wanted(&out.path) == Some(true) => out.statistics = Some(statistics(r)?),
            _ => return Ok(false),
        }
        Ok(true)
    })
}

/// Read a footer (the bytes between the data and the length and magic at the end), keeping the chunks of the leaves for
/// which `wanted` answers `Some` (and their statistics where it answers `Some(true)`).
///
/// # Errors
///
/// Metadata that does not read, a schema that does not add up, or a column whose data is in another file.
pub fn parse(bytes: &[u8], wanted: &dyn Fn(&[String]) -> Option<bool>) -> Result<FileMeta, Error> {
    let mut reader = Reader::new(bytes);
    let mut out = FileMeta::default();
    let mut elements = Vec::new();
    let mut groups: Vec<(i64, Vec<RawChunk>)> = Vec::new();
    reader.read_struct(&mut |r, id, kind| {
        match (id, kind) {
            (2, thrift::LIST) => r.read_list(&mut |r, _| {
                if elements.len() >= MAX_ELEMENTS {
                    return Err(Error::new("a Parquet schema of too many fields"));
                }
                elements.push(element(r)?);
                Ok(())
            })?,
            (3, thrift::I64) => out.num_rows = r.i64()?,
            (4, thrift::LIST) => r.read_list(&mut |r, _| {
                let mut num_rows = 0;
                let mut chunks = Vec::new();
                r.read_struct(&mut |r, id, kind| {
                    match (id, kind) {
                        (1, thrift::LIST) => r.read_list(&mut |r, _| {
                            let mut chunk = RawChunk::default();
                            r.read_struct(&mut |r, id, kind| {
                                match (id, kind) {
                                    (1, thrift::BINARY) => {
                                        r.binary()?;
                                        chunk.elsewhere = true;
                                    }
                                    (3, thrift::STRUCT) => column_meta(r, &mut chunk, wanted)?,
                                    _ => return Ok(false),
                                }
                                Ok(true)
                            })?;
                            if wanted(&chunk.path).is_some() {
                                chunks.push(chunk);
                            }
                            Ok(())
                        })?,
                        (3, thrift::I64) => num_rows = r.i64()?,
                        _ => return Ok(false),
                    }
                    Ok(true)
                })?;
                groups.push((num_rows, chunks));
                Ok(())
            })?,
            (5, thrift::LIST) => r.read_list(&mut |r, _| {
                let (mut key, mut value) = (String::new(), String::new());
                r.read_struct(&mut |r, id, kind| {
                    match (id, kind) {
                        (1, thrift::BINARY) => key = r.string()?,
                        (2, thrift::BINARY) => value = r.string()?,
                        _ => return Ok(false),
                    }
                    Ok(true)
                })?;
                out.key_values.push((key, value));
                Ok(())
            })?,
            (6, thrift::BINARY) => out.created_by = Some(r.string()?),
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    out.leaves = leaves(&elements)?;
    for (num_rows, raw) in groups {
        let mut chunks = Vec::with_capacity(raw.len());
        for chunk in raw {
            if chunk.elsewhere {
                return Err(Error::new(format!(
                    "the Parquet column {} is kept in another file",
                    chunk.path.join(".")
                )));
            }
            let path: Vec<&str> = chunk.path.iter().map(String::as_str).collect();
            let Some(leaf) = out.leaf(&path) else {
                return Err(Error::new(format!(
                    "a Parquet column chunk {} of no column of the schema",
                    chunk.path.join(".")
                )));
            };
            // a writer that has no dictionary page may write its offset as 0
            let start = match chunk.dictionary_page_offset {
                Some(offset) if offset > 0 && offset < chunk.data_page_offset => offset,
                _ => chunk.data_page_offset,
            };
            let (Ok(start), Ok(compressed_size)) = (u64::try_from(start), u64::try_from(chunk.compressed_size)) else {
                return Err(Error::new(format!(
                    "the Parquet column chunk {} has a negative offset or size",
                    chunk.path.join(".")
                )));
            };
            chunks.push(Chunk {
                leaf,
                codec: Codec::of(chunk.codec),
                num_values: chunk.num_values,
                start,
                compressed_size,
                statistics: chunk.statistics,
            });
        }
        out.row_groups.push(RowGroup { num_rows, chunks });
    }
    Ok(out)
}
