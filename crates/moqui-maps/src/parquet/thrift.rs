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

//! The Thrift compact protocol, read only: what Parquet's footer and page headers are written in. A struct is read field
//! by field through a callback, which reads the fields it knows and skips the rest. Every length is checked against the
//! bytes left before anything is kept, and structs, lists and maps nest at most [`MAX_DEPTH`] deep.

use moqui_lib::error::Error;

/// How deep structs, lists and maps may nest (Parquet's own go four deep).
pub const MAX_DEPTH: usize = 32;

pub const STOP: u8 = 0;
pub const TRUE: u8 = 1;
pub const FALSE: u8 = 2;
pub const BYTE: u8 = 3;
pub const I16: u8 = 4;
pub const I32: u8 = 5;
pub const I64: u8 = 6;
pub const DOUBLE: u8 = 7;
pub const BINARY: u8 = 8;
pub const LIST: u8 = 9;
pub const SET: u8 = 10;
pub const MAP: u8 = 11;
pub const STRUCT: u8 = 12;

/// Compact-protocol bytes read from the front.
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
}

fn short() -> Error {
    Error::new("Parquet metadata that ends inside a value")
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0, depth: 0 }
    }

    /// The bytes read so far.
    #[must_use]
    pub fn position(&self) -> usize {
        self.at
    }

    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }

    pub fn byte(&mut self) -> Result<u8, Error> {
        let byte = *self.bytes.get(self.at).ok_or_else(short)?;
        self.at += 1;
        Ok(byte)
    }

    fn varint(&mut self) -> Result<u64, Error> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::new("Parquet metadata with a varint longer than 64 bits"))
    }

    fn zigzag(&mut self) -> Result<i64, Error> {
        let value = self.varint()?;
        // zigzag: the low bit is the sign
        Ok(((value >> 1) as i64) ^ -((value & 1) as i64))
    }

    pub fn i32(&mut self) -> Result<i32, Error> {
        i32::try_from(self.zigzag()?).map_err(|_| Error::new("Parquet metadata with an i32 out of range"))
    }

    pub fn i64(&mut self) -> Result<i64, Error> {
        self.zigzag()
    }

    pub fn double(&mut self) -> Result<f64, Error> {
        let slice = self.bytes.get(self.at..self.at + 8).ok_or_else(short)?;
        self.at += 8;
        let mut raw = [0u8; 8];
        raw.copy_from_slice(slice);
        Ok(f64::from_le_bytes(raw))
    }

    pub fn binary(&mut self) -> Result<&'a [u8], Error> {
        let length = usize::try_from(self.varint()?).unwrap_or(usize::MAX);
        if length > self.left() {
            return Err(Error::new(format!(
                "Parquet metadata that claims {length} bytes, more than it has"
            )));
        }
        let slice = self.bytes.get(self.at..self.at + length).ok_or_else(short)?;
        self.at += length;
        Ok(slice)
    }

    pub fn string(&mut self) -> Result<String, Error> {
        Ok(String::from_utf8_lossy(self.binary()?).into_owned())
    }

    /// A field's boolean, whose value is its type.
    pub fn bool_of(kind: u8) -> bool {
        kind == TRUE
    }

    /// A list's header: its element type and count, the count checked against the bytes left (an element is a byte at
    /// least).
    pub fn list_header(&mut self) -> Result<(u8, usize), Error> {
        let header = self.byte()?;
        let kind = header & 0x0f;
        let count = if header >> 4 == 0x0f {
            usize::try_from(self.varint()?).unwrap_or(usize::MAX)
        } else {
            usize::from(header >> 4)
        };
        if count > self.left() {
            return Err(Error::new(format!(
                "Parquet metadata with a list of {count} items, more than its bytes hold"
            )));
        }
        Ok((kind, count))
    }

    fn enter(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::new("Parquet metadata nested too deep"));
        }
        Ok(())
    }

    /// Read a struct: `field` is called with each field's ID and type, and reads its value or answers false to have it
    /// skipped.
    pub fn read_struct(
        &mut self,
        field: &mut dyn FnMut(&mut Self, i16, u8) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        self.enter()?;
        let mut last = 0i16;
        loop {
            let header = self.byte()?;
            let kind = header & 0x0f;
            if kind == STOP {
                break;
            }
            let delta = header >> 4;
            let id = if delta == 0 {
                i16::try_from(self.zigzag()?).map_err(|_| Error::new("Parquet metadata with a bad field ID"))?
            } else {
                last.saturating_add(i16::from(delta))
            };
            last = id;
            if !field(self, id, kind)? {
                self.skip(kind)?;
            }
        }
        self.depth -= 1;
        Ok(())
    }

    /// Read a list of `kind` elements with `element`, which reads each one.
    pub fn read_list(&mut self, element: &mut dyn FnMut(&mut Self, u8) -> Result<(), Error>) -> Result<(), Error> {
        self.enter()?;
        let (kind, count) = self.list_header()?;
        for _ in 0..count {
            element(self, kind)?;
        }
        self.depth -= 1;
        Ok(())
    }

    /// Skip a value of `kind`.
    pub fn skip(&mut self, kind: u8) -> Result<(), Error> {
        match kind {
            TRUE | FALSE => Ok(()),
            BYTE => self.byte().map(|_| ()),
            I16 | I32 | I64 => self.varint().map(|_| ()),
            DOUBLE => self.double().map(|_| ()),
            BINARY => self.binary().map(|_| ()),
            LIST | SET => {
                self.enter()?;
                let (element, count) = self.list_header()?;
                for _ in 0..count {
                    // a boolean element is a byte of its own in a list
                    if matches!(element, TRUE | FALSE) {
                        self.byte()?;
                    } else {
                        self.skip(element)?;
                    }
                }
                self.depth -= 1;
                Ok(())
            }
            MAP => {
                self.enter()?;
                let count = usize::try_from(self.varint()?).unwrap_or(usize::MAX);
                if count > 0 {
                    if count > self.left() {
                        return Err(Error::new("Parquet metadata with a map larger than its bytes"));
                    }
                    let kinds = self.byte()?;
                    for _ in 0..count {
                        // a boolean key or value is a byte of its own in a map, as in a list
                        for kind in [kinds >> 4, kinds & 0x0f] {
                            if matches!(kind, TRUE | FALSE) {
                                self.byte()?;
                            } else {
                                self.skip(kind)?;
                            }
                        }
                    }
                }
                self.depth -= 1;
                Ok(())
            }
            STRUCT => self.read_struct(&mut |_, _, _| Ok(false)),
            other => Err(Error::new(format!(
                "Parquet metadata with an unknown Thrift type {other}"
            ))),
        }
    }
}
