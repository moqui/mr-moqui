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

//! Parquet's page codecs and value encodings: decompression (ZSTD on `ruzstd`, GZIP on `flate2`, Snappy our own) to
//! exactly the size a page header declares; the RLE and bit-packed hybrid of levels and dictionary indices; and the
//! value encodings PLAIN, the dictionary's indices, `DELTA_BINARY_PACKED`, `DELTA_LENGTH_BYTE_ARRAY`,
//! `DELTA_BYTE_ARRAY` and `BYTE_STREAM_SPLIT`. Every count is checked against the bytes and the values a page claims.

use std::io::Read;

use moqui_lib::error::Error;

use super::meta::{Codec, Physical};

/// Decoded values of one column, the nulls left out.
#[derive(Clone, Debug, PartialEq)]
pub enum Values {
    Boolean(Vec<bool>),
    Int32(Vec<i32>),
    Int64(Vec<i64>),
    Float(Vec<f32>),
    Double(Vec<f64>),
    /// Byte arrays (and fixed-length ones, and INT96), one buffer with each value's end.
    Bytes(ByteArrays),
}

/// Byte arrays in one buffer: value `i` is `data[ends[i - 1]..ends[i]]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ByteArrays {
    pub data: Vec<u8>,
    pub ends: Vec<usize>,
}

impl ByteArrays {
    #[must_use]
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&[u8]> {
        let end = *self.ends.get(index)?;
        let start = if index == 0 { 0 } else { *self.ends.get(index - 1)? };
        self.data.get(start..end)
    }

    pub fn push(&mut self, value: &[u8]) {
        self.data.extend_from_slice(value);
        self.ends.push(self.data.len());
    }
}

impl Values {
    #[must_use]
    pub fn empty(physical: Physical) -> Self {
        match physical {
            Physical::Boolean => Self::Boolean(Vec::new()),
            Physical::Int32 => Self::Int32(Vec::new()),
            Physical::Int64 => Self::Int64(Vec::new()),
            Physical::Float => Self::Float(Vec::new()),
            Physical::Double => Self::Double(Vec::new()),
            Physical::Int96 | Physical::ByteArray | Physical::FixedLenByteArray => Self::Bytes(ByteArrays::default()),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Boolean(values) => values.len(),
            Self::Int32(values) => values.len(),
            Self::Int64(values) => values.len(),
            Self::Float(values) => values.len(),
            Self::Double(values) => values.len(),
            Self::Bytes(values) => values.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Value `index` as text (a byte array as UTF-8, lossily; a number written out).
    #[must_use]
    pub fn text(&self, index: usize) -> Option<String> {
        match self {
            Self::Bytes(values) => values
                .get(index)
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
            Self::Boolean(values) => values.get(index).map(ToString::to_string),
            Self::Int32(values) => values.get(index).map(ToString::to_string),
            Self::Int64(values) => values.get(index).map(ToString::to_string),
            Self::Float(values) => values.get(index).map(ToString::to_string),
            Self::Double(values) => values.get(index).map(ToString::to_string),
        }
    }

    /// Value `index` as a number, where it is one.
    #[must_use]
    pub fn number(&self, index: usize) -> Option<f64> {
        match self {
            Self::Int32(values) => values.get(index).map(|value| f64::from(*value)),
            #[allow(clippy::cast_precision_loss)]
            Self::Int64(values) => values.get(index).map(|value| *value as f64),
            Self::Float(values) => values.get(index).map(|value| f64::from(*value)),
            Self::Double(values) => values.get(index).copied(),
            Self::Boolean(_) | Self::Bytes(_) => None,
        }
    }

    #[must_use]
    pub fn bytes(&self, index: usize) -> Option<&[u8]> {
        match self {
            Self::Bytes(values) => values.get(index),
            _ => None,
        }
    }

    /// The bytes the values hold.
    #[must_use]
    pub fn decoded_bytes(&self) -> usize {
        match self {
            Self::Boolean(values) => values.len(),
            Self::Int32(values) => values.len() * 4,
            Self::Float(values) => values.len() * 4,
            Self::Int64(values) => values.len() * 8,
            Self::Double(values) => values.len() * 8,
            Self::Bytes(values) => values.data.len() + values.ends.len() * std::mem::size_of::<usize>(),
        }
    }

    /// Append `other`'s values (of the same type), refused when the two would hold more than `max_bytes`.
    ///
    /// # Errors
    ///
    /// Values of another type, or more than `max_bytes`.
    pub fn extend(&mut self, other: Self, max_bytes: usize) -> Result<(), Error> {
        if self.decoded_bytes().saturating_add(other.decoded_bytes()) > max_bytes {
            return Err(Error::new(format!(
                "a Parquet column chunk that decodes to more than {max_bytes} bytes"
            )));
        }
        match (self, other) {
            (Self::Boolean(a), Self::Boolean(b)) => a.extend(b),
            (Self::Int32(a), Self::Int32(b)) => a.extend(b),
            (Self::Int64(a), Self::Int64(b)) => a.extend(b),
            (Self::Float(a), Self::Float(b)) => a.extend(b),
            (Self::Double(a), Self::Double(b)) => a.extend(b),
            (Self::Bytes(a), Self::Bytes(b)) => {
                let base = a.data.len();
                a.data.extend_from_slice(&b.data);
                a.ends.extend(b.ends.iter().map(|end| base + end));
            }
            _ => return Err(Error::new("Parquet pages of one column with values of two types")),
        }
        Ok(())
    }
}

// ---------- decompression ----------

/// `input` decompressed by `codec` to exactly `size` bytes.
///
/// # Errors
///
/// A codec this reader does not read, data that does not decompress, or a size other than `size`.
pub fn decompress(codec: Codec, input: &[u8], size: usize) -> Result<Vec<u8>, Error> {
    let out = match codec {
        Codec::Uncompressed => input.to_vec(),
        Codec::Snappy => super::snappy::decompress(input, size)?,
        Codec::Gzip => read_capped(flate2::read::MultiGzDecoder::new(input), size, "GZIP")?,
        Codec::Zstd => {
            let mut source = input;
            let decoder = ruzstd::decoding::StreamingDecoder::new(&mut source)
                .map_err(|err| Error::new(format!("a ZSTD page that does not read: {err}")))?;
            read_capped(decoder, size, "ZSTD")?
        }
        Codec::Unsupported(name) => {
            return Err(Error::new(format!(
                "Parquet pages compressed with {name}, which this reader does not read (ZSTD, Snappy, GZIP and none)"
            )))
        }
    };
    if out.len() != size {
        return Err(Error::new(format!(
            "a Parquet page of {} bytes where its header says {size}",
            out.len()
        )));
    }
    Ok(out)
}

fn read_capped(reader: impl Read, size: usize, name: &str) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(size.min(1 << 24));
    reader
        .take(size as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|err| Error::new(format!("a {name} page that does not decompress")).with_source(err))?;
    Ok(out)
}

// ---------- bits ----------

/// The bits a level of at most `max` takes.
#[must_use]
pub fn bit_width(max: u32) -> u8 {
    // at most 32
    #[allow(clippy::cast_possible_truncation)]
    let width = (32 - max.leading_zeros()) as u8;
    width
}

fn short(what: &str) -> Error {
    Error::new(format!("Parquet {what} that end inside a value"))
}

/// Bytes read from the front.
pub struct Cursor<'a> {
    pub bytes: &'a [u8],
    pub at: usize,
}

impl<'a> Cursor<'a> {
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let slice = self
            .at
            .checked_add(count)
            .and_then(|end| self.bytes.get(self.at..end))
            .ok_or_else(|| short("values"))?;
        self.at += count;
        Ok(slice)
    }

    pub fn u32_le(&mut self) -> Result<u32, Error> {
        let raw = self.take(4)?;
        Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
    }

    pub fn varint(&mut self) -> Result<u64, Error> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.take(1)?.first().unwrap_or(&0);
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::new("a Parquet varint longer than 64 bits"))
    }

    pub fn zigzag(&mut self) -> Result<i64, Error> {
        let value = self.varint()?;
        Ok(((value >> 1) as i64) ^ -((value & 1) as i64))
    }

    #[must_use]
    pub fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.at..).unwrap_or_default()
    }
}

/// Unpack `count` values of `width` bits each, least significant bit first, from `bytes`.
fn unpack(bytes: &[u8], width: u8, count: usize, out: &mut Vec<u64>) -> Result<(), Error> {
    if width == 0 {
        out.extend(std::iter::repeat_n(0, count));
        return Ok(());
    }
    if width > 64 {
        return Err(Error::new(format!("a Parquet bit width of {width}")));
    }
    let needed = (count * usize::from(width)).div_ceil(8);
    if needed > bytes.len() {
        return Err(short("bit-packed values"));
    }
    let mask = if width == 64 { u64::MAX } else { (1u64 << width) - 1 };
    let mut bit = 0usize;
    for _ in 0..count {
        let mut value = 0u64;
        let mut got = 0u8;
        while got < width {
            let byte = bytes[bit / 8];
            let offset = (bit % 8) as u8;
            let take = (8 - offset).min(width - got);
            let part = (u64::from(byte) >> offset) & ((1u64 << take) - 1);
            value |= part << got;
            got += take;
            bit += usize::from(take);
        }
        out.push(value & mask);
    }
    Ok(())
}

/// The RLE and bit-packed hybrid: `count` values of `width` bits from `bytes` (with no length prefix).
///
/// # Errors
///
/// Runs that end early.
pub fn hybrid(bytes: &[u8], width: u8, count: usize) -> Result<Vec<u64>, Error> {
    // levels and dictionary indices are at most 32 bits wide
    if width > 32 {
        return Err(Error::new(format!("a Parquet RLE run of bit width {width}")));
    }
    let mut out = Vec::with_capacity(count.min(1 << 16));
    let mut cursor = Cursor::new(bytes);
    let value_bytes = usize::from(width).div_ceil(8);
    while out.len() < count {
        let header = cursor.varint()?;
        let run = usize::try_from(header >> 1).unwrap_or(usize::MAX);
        if header & 1 == 0 {
            let raw = cursor.take(value_bytes)?;
            let mut value = 0u64;
            for (index, byte) in raw.iter().enumerate() {
                value |= u64::from(*byte) << (8 * index);
            }
            let take = run.min(count - out.len());
            if run == 0 {
                return Err(Error::new("a Parquet RLE run of no values"));
            }
            out.extend(std::iter::repeat_n(value, take));
        } else {
            let values = run.checked_mul(8).ok_or_else(|| short("bit-packed values"))?;
            if values == 0 {
                return Err(Error::new("a Parquet bit-packed run of no values"));
            }
            let bytes = cursor.take(
                values
                    .checked_mul(usize::from(width))
                    .ok_or_else(|| short("bit-packed values"))?
                    / 8,
            )?;
            // the run's bytes are consumed whole; only the values still wanted are unpacked (a width-0 run claims any
            // number of values in no bytes)
            let take = values.min(count - out.len());
            unpack(bytes, width, take, &mut out)?;
        }
    }
    Ok(out)
}

/// Levels of a v1 data page: a four-byte length, then the hybrid. Answers them and the bytes they took.
pub fn levels_v1(bytes: &[u8], max: i16, count: usize) -> Result<(Vec<i16>, usize), Error> {
    let mut cursor = Cursor::new(bytes);
    let length = cursor.u32_le()? as usize;
    let data = cursor.take(length)?;
    Ok((levels(data, max, count)?, 4 + length))
}

/// Levels of `max` from hybrid `bytes`, each checked to be at most `max`.
pub fn levels(bytes: &[u8], max: i16, count: usize) -> Result<Vec<i16>, Error> {
    let width = bit_width(u32::try_from(max).unwrap_or(0));
    let raw = hybrid(bytes, width, count)?;
    raw.into_iter()
        .map(|value| {
            i16::try_from(value)
                .ok()
                .filter(|level| *level <= max)
                .ok_or_else(|| Error::new(format!("a Parquet level {value} above its column's {max}")))
        })
        .collect()
}

// ---------- values ----------

/// `count` PLAIN values of `physical` from `bytes`.
///
/// # Errors
///
/// Fewer bytes than the values need.
pub fn plain(bytes: &[u8], physical: Physical, type_length: usize, count: usize) -> Result<Values, Error> {
    let mut cursor = Cursor::new(bytes);
    let fixed = |size: usize| -> Result<(), Error> {
        if count.checked_mul(size).is_none_or(|needed| needed > bytes.len()) {
            return Err(short("PLAIN values"));
        }
        Ok(())
    };
    Ok(match physical {
        Physical::Boolean => {
            let mut bits = Vec::with_capacity(count);
            unpack(bytes, 1, count, &mut bits)?;
            Values::Boolean(bits.into_iter().map(|bit| bit == 1).collect())
        }
        Physical::Int32 => {
            fixed(4)?;
            Values::Int32(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .take(count)
                    .map(|b| i32::from_le_bytes(*b))
                    .collect(),
            )
        }
        Physical::Float => {
            fixed(4)?;
            Values::Float(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .take(count)
                    .map(|b| f32::from_le_bytes(*b))
                    .collect(),
            )
        }
        Physical::Int64 => {
            fixed(8)?;
            Values::Int64(
                bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .take(count)
                    .map(|b| i64::from_le_bytes(*b))
                    .collect(),
            )
        }
        Physical::Double => {
            fixed(8)?;
            Values::Double(
                bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .take(count)
                    .map(|b| f64::from_le_bytes(*b))
                    .collect(),
            )
        }
        Physical::Int96 | Physical::FixedLenByteArray => {
            let size = if physical == Physical::Int96 { 12 } else { type_length };
            fixed(size)?;
            let mut out = ByteArrays::default();
            for _ in 0..count {
                out.push(cursor.take(size)?);
            }
            Values::Bytes(out)
        }
        Physical::ByteArray => {
            let mut out = ByteArrays::default();
            for _ in 0..count {
                let length = cursor.u32_le()? as usize;
                out.push(cursor.take(length)?);
            }
            Values::Bytes(out)
        }
    })
}

/// `count` values looked up in `dictionary` by the indices in `bytes` (a bit width byte, then the hybrid).
pub fn dictionary_indices(bytes: &[u8], dictionary: &Values, count: usize, max_bytes: usize) -> Result<Values, Error> {
    let Some((&width, rest)) = bytes.split_first() else {
        return if count == 0 {
            Ok(Values::empty_like(dictionary))
        } else {
            Err(short("dictionary indices"))
        };
    };
    let indices = hybrid(rest, width, count)?;
    let size = dictionary.len();
    let index = |raw: u64| -> Result<usize, Error> {
        usize::try_from(raw)
            .ok()
            .filter(|index| *index < size)
            .ok_or_else(|| Error::new(format!("a Parquet dictionary index {raw} past its {size} values")))
    };
    Ok(match dictionary {
        Values::Boolean(values) => Values::Boolean(
            indices
                .into_iter()
                .map(|raw| index(raw).map(|i| values[i]))
                .collect::<Result<_, _>>()?,
        ),
        Values::Int32(values) => Values::Int32(
            indices
                .into_iter()
                .map(|raw| index(raw).map(|i| values[i]))
                .collect::<Result<_, _>>()?,
        ),
        Values::Int64(values) => Values::Int64(
            indices
                .into_iter()
                .map(|raw| index(raw).map(|i| values[i]))
                .collect::<Result<_, _>>()?,
        ),
        Values::Float(values) => Values::Float(
            indices
                .into_iter()
                .map(|raw| index(raw).map(|i| values[i]))
                .collect::<Result<_, _>>()?,
        ),
        Values::Double(values) => Values::Double(
            indices
                .into_iter()
                .map(|raw| index(raw).map(|i| values[i]))
                .collect::<Result<_, _>>()?,
        ),
        Values::Bytes(values) => {
            let mut out = ByteArrays::default();
            for raw in indices {
                let value = values.get(index(raw)?).unwrap_or_default();
                if out.data.len() + value.len() > max_bytes {
                    return Err(Error::new(format!(
                        "Parquet dictionary values that decode to more than {max_bytes} bytes"
                    )));
                }
                out.push(value);
            }
            Values::Bytes(out)
        }
    })
}

impl Values {
    fn empty_like(other: &Self) -> Self {
        match other {
            Self::Boolean(_) => Self::Boolean(Vec::new()),
            Self::Int32(_) => Self::Int32(Vec::new()),
            Self::Int64(_) => Self::Int64(Vec::new()),
            Self::Float(_) => Self::Float(Vec::new()),
            Self::Double(_) => Self::Double(Vec::new()),
            Self::Bytes(_) => Self::Bytes(ByteArrays::default()),
        }
    }
}

/// The largest delta block (the format's writers use 128 to 1,024 values).
const MAX_DELTA_BLOCK: usize = 1 << 16;

/// `DELTA_BINARY_PACKED` integers from `cursor` (left after them): at most `limit` values (the header's count is
/// checked against it).
pub fn delta_binary_packed(cursor: &mut Cursor<'_>, limit: usize) -> Result<Vec<i64>, Error> {
    let block_size = usize::try_from(cursor.varint()?).unwrap_or(0);
    let miniblocks = usize::try_from(cursor.varint()?).unwrap_or(0);
    let total = usize::try_from(cursor.varint()?).unwrap_or(usize::MAX);
    let first = cursor.zigzag()?;
    if total > limit {
        return Err(Error::new(format!(
            "Parquet delta-encoded values that claim {total}, more than the page's {limit}"
        )));
    }
    // a block of at most 65,536 values in miniblocks of a multiple of 32 values each
    if block_size == 0
        || block_size > MAX_DELTA_BLOCK
        || block_size % 128 != 0
        || miniblocks == 0
        || block_size % miniblocks != 0
        || (block_size / miniblocks) % 32 != 0
    {
        return Err(Error::new(format!(
            "a Parquet delta encoding of {block_size} values in {miniblocks} miniblocks"
        )));
    }
    let per_mini = block_size / miniblocks;
    let mut out = Vec::with_capacity(total.min(1 << 16));
    if total == 0 {
        return Ok(out);
    }
    out.push(first);
    let mut last = first;
    let mut unpacked = Vec::with_capacity(per_mini);
    while out.len() < total {
        let min_delta = cursor.zigzag()?;
        let widths = cursor.take(miniblocks)?;
        for &width in widths {
            if out.len() >= total {
                break;
            }
            let bytes = cursor.take(per_mini * usize::from(width) / 8)?;
            unpacked.clear();
            unpack(bytes, width, per_mini, &mut unpacked)?;
            for delta in &unpacked {
                if out.len() >= total {
                    break;
                }
                // wrapping, as the encoding is defined
                #[allow(clippy::cast_possible_wrap)]
                let delta = *delta as i64;
                last = last.wrapping_add(min_delta).wrapping_add(delta);
                out.push(last);
            }
        }
    }
    Ok(out)
}

fn lengths(values: Vec<i64>) -> Result<Vec<usize>, Error> {
    values
        .into_iter()
        .map(|length| usize::try_from(length).map_err(|_| Error::new(format!("a Parquet length of {length}"))))
        .collect()
}

/// `DELTA_LENGTH_BYTE_ARRAY`: the lengths, then the bytes.
pub fn delta_length_byte_array(bytes: &[u8], count: usize) -> Result<Values, Error> {
    let mut cursor = Cursor::new(bytes);
    let lengths = lengths(delta_binary_packed(&mut cursor, count)?)?;
    let mut out = ByteArrays::default();
    for length in lengths {
        out.push(cursor.take(length)?);
    }
    Ok(Values::Bytes(out))
}

/// `DELTA_BYTE_ARRAY`: prefix lengths, suffix lengths, then the suffixes, each value the previous one's prefix and its
/// suffix.
pub fn delta_byte_array(bytes: &[u8], count: usize, max_bytes: usize) -> Result<Values, Error> {
    let mut cursor = Cursor::new(bytes);
    let prefixes = lengths(delta_binary_packed(&mut cursor, count)?)?;
    let suffixes = lengths(delta_binary_packed(&mut cursor, count)?)?;
    if prefixes.len() != suffixes.len() {
        return Err(Error::new(
            "Parquet delta byte arrays with prefixes and suffixes of different counts",
        ));
    }
    let mut out = ByteArrays::default();
    let mut previous: Vec<u8> = Vec::new();
    for (prefix, suffix) in prefixes.into_iter().zip(suffixes) {
        if prefix > previous.len() {
            return Err(Error::new(
                "a Parquet delta byte array whose prefix is longer than the value before it",
            ));
        }
        previous.truncate(prefix);
        previous.extend_from_slice(cursor.take(suffix)?);
        if out.data.len() + previous.len() > max_bytes {
            return Err(Error::new("Parquet delta byte arrays larger than the page allows"));
        }
        out.push(&previous);
    }
    Ok(Values::Bytes(out))
}

/// `BYTE_STREAM_SPLIT`: byte `k` of every value in stream `k`.
pub fn byte_stream_split(bytes: &[u8], physical: Physical, type_length: usize, count: usize) -> Result<Values, Error> {
    let size = match physical {
        Physical::Int32 | Physical::Float => 4,
        Physical::Int64 | Physical::Double => 8,
        Physical::FixedLenByteArray => type_length,
        other => return Err(Error::new(format!("BYTE_STREAM_SPLIT of {other:?} values"))),
    };
    if count.checked_mul(size).is_none_or(|needed| needed > bytes.len()) {
        return Err(short("BYTE_STREAM_SPLIT values"));
    }
    let mut joined = vec![0u8; count * size];
    for (index, value) in joined.chunks_exact_mut(size).enumerate() {
        for (stream, byte) in value.iter_mut().enumerate() {
            *byte = bytes[stream * count + index];
        }
    }
    plain(&joined, physical, type_length, count)
}
