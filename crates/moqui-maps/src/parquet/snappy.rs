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

//! Snappy's raw format, decompressed (Parquet's SNAPPY pages): a length, then literals and copies. The output is held to
//! the length the page header declares and every copy to the bytes already written.

use moqui_lib::error::Error;

use super::decode::Cursor;

fn bad(what: &str) -> Error {
    Error::new(format!("a Snappy page with {what}"))
}

/// `input` decompressed, which must come to `size` bytes.
///
/// # Errors
///
/// A length other than `size`, a copy before the start, or data that ends early.
pub fn decompress(input: &[u8], size: usize) -> Result<Vec<u8>, Error> {
    let mut cursor = Cursor::new(input);
    let declared = usize::try_from(cursor.varint()?).unwrap_or(usize::MAX);
    if declared != size {
        return Err(bad(&format!("a length of {declared} where its header says {size}")));
    }
    // reserved as it is written: a length is a claim until the data bears it out
    let mut out: Vec<u8> = Vec::with_capacity(size.min(input.len().saturating_mul(4)));
    while cursor.at < input.len() {
        let tag = *cursor.take(1)?.first().unwrap_or(&0);
        match tag & 3 {
            0 => {
                let mut length = usize::from(tag >> 2);
                if length >= 60 {
                    let extra = length - 59;
                    let raw = cursor.take(extra)?;
                    length = raw
                        .iter()
                        .enumerate()
                        .fold(0usize, |sum, (index, byte)| sum | (usize::from(*byte) << (8 * index)));
                }
                let length = length + 1;
                if out.len() + length > size {
                    return Err(bad("more bytes than it declares"));
                }
                out.extend_from_slice(cursor.take(length)?);
            }
            kind => {
                let (length, offset) = match kind {
                    1 => {
                        let low = usize::from(*cursor.take(1)?.first().unwrap_or(&0));
                        (4 + usize::from((tag >> 2) & 7), (usize::from(tag >> 5) << 8) | low)
                    }
                    2 => {
                        let raw = cursor.take(2)?;
                        (
                            usize::from(tag >> 2) + 1,
                            usize::from(u16::from_le_bytes([raw[0], raw[1]])),
                        )
                    }
                    _ => (usize::from(tag >> 2) + 1, cursor.u32_le()? as usize),
                };
                if offset == 0 || offset > out.len() {
                    return Err(bad("a copy from before its start"));
                }
                if out.len() + length > size {
                    return Err(bad("more bytes than it declares"));
                }
                // a copy may overlap what it writes: byte by byte
                let from = out.len() - offset;
                for index in 0..length {
                    let byte = out[from + index];
                    out.push(byte);
                }
            }
        }
    }
    if out.len() != size {
        return Err(bad("fewer bytes than it declares"));
    }
    Ok(out)
}
