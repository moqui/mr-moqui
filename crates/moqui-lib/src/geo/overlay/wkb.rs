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

//! WKB as an overlay (plans/Maps.md §4.1): OGC and ISO well-known binary (Z, M and ZM types, by the ISO thousands or
//! EWKB's flags, the extra ordinates dropped), with EWKB's SRID read as WKT's is. A `.wkb` file is geometries one after
//! another, binary, or hex text with one geometry a line (a file of only hex digits and white space is read so); each is
//! a feature with no label. Every count a geometry claims is checked against the bytes left before anything is kept, so
//! a short file claiming millions of points costs nothing, and collections nest at most four deep. Binary geometries have
//! no separator, so the first that does not read ends the file: it is counted and the ones before it are kept.

use super::{OverlayError, Position, Raw, Reader};

const MAX_NESTING: usize = 4;
const EWKB_Z: u32 = 0x8000_0000;
const EWKB_M: u32 = 0x4000_0000;
const EWKB_SRID: u32 = 0x2000_0000;

/// WKB bytes read from the front.
struct Bytes<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Bytes<'_> {
    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let slice = self
            .bytes
            .get(self.at..self.at + N)
            .ok_or_else(|| "WKB that ends inside a geometry".to_string())?;
        self.at += N;
        let mut out = [0; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn u32(&mut self, little: bool) -> Result<u32, String> {
        let raw = self.take::<4>()?;
        Ok(if little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        })
    }

    fn f64(&mut self, little: bool) -> Result<f64, String> {
        let raw = self.take::<8>()?;
        Ok(if little {
            f64::from_le_bytes(raw)
        } else {
            f64::from_be_bytes(raw)
        })
    }

    /// A count of items each at least `size` bytes, refused when the bytes left cannot hold them.
    fn count(&mut self, little: bool, size: usize) -> Result<usize, String> {
        let count = usize::try_from(self.u32(little)?).unwrap_or(usize::MAX);
        if count.saturating_mul(size) > self.left() {
            return Err(format!("WKB that claims {count} items, more than its bytes hold"));
        }
        Ok(count)
    }
}

/// A geometry's header: its byte order, kind (1 to 7), the ordinates of a position and its SRID.
struct Header {
    little: bool,
    kind: u32,
    dimensions: usize,
    srid: Option<u32>,
}

fn header(input: &mut Bytes) -> Result<Header, String> {
    let little = match input.take::<1>()?[0] {
        0 => false,
        1 => true,
        _ => return Err("WKB with a byte order that is neither 0 nor 1".to_string()),
    };
    let code = input.u32(little)?;
    let srid = if code & EWKB_SRID != 0 {
        Some(input.u32(little)?)
    } else {
        None
    };
    let base = code & 0x0FFF_FFFF;
    let (kind, iso) = (base % 1000, base / 1000);
    if iso > 3 {
        return Err(format!("a WKB geometry type that is not read ({base})"));
    }
    let z = code & EWKB_Z != 0 || iso == 1 || iso == 3;
    let m = code & EWKB_M != 0 || iso == 2 || iso == 3;
    Ok(Header {
        little,
        kind,
        dimensions: 2 + usize::from(z) + usize::from(m),
        srid,
    })
}

fn position(input: &mut Bytes, header: &Header) -> Result<Position, String> {
    let x = input.f64(header.little)?;
    let y = input.f64(header.little)?;
    for _ in 2..header.dimensions {
        input.f64(header.little)?;
    }
    Ok([x, y])
}

fn positions(input: &mut Bytes, header: &Header) -> Result<Vec<Position>, String> {
    let count = input.count(header.little, header.dimensions * 8)?;
    (0..count).map(|_| position(input, header)).collect()
}

fn rings(input: &mut Bytes, header: &Header) -> Result<Vec<Vec<Position>>, String> {
    let count = input.count(header.little, 4)?;
    (0..count).map(|_| positions(input, header)).collect()
}

/// The members of a multi geometry or collection, each a geometry of its own header (`kind` the one a member must be,
/// or any for a collection).
fn members(input: &mut Bytes, header: &Header, kind: Option<u32>, depth: usize) -> Result<Vec<Raw>, String> {
    if depth >= MAX_NESTING {
        return Err(format!("WKB geometries nested deeper than {MAX_NESTING}"));
    }
    // a member is at least its byte order and type
    let count = input.count(header.little, 5)?;
    (0..count)
        .map(|_| {
            let (_, raw, member) = geometry(input, depth + 1)?;
            match kind {
                Some(kind) if member != kind => Err("a WKB multi geometry with a member of another type".to_string()),
                _ => Ok(raw),
            }
        })
        .collect()
}

/// One geometry from the front: its SRID, the geometry and its kind.
fn geometry(input: &mut Bytes, depth: usize) -> Result<(Option<u32>, Raw, u32), String> {
    let header = header(input)?;
    let raw = match header.kind {
        1 => {
            let [x, y] = position(input, &header)?;
            if x.is_nan() && y.is_nan() {
                return Err("an empty point".to_string());
            }
            Raw::Point([x, y])
        }
        2 => Raw::Line(positions(input, &header)?),
        3 => Raw::Polygon(rings(input, &header)?),
        4 => Raw::MultiPoint(
            members(input, &header, Some(1), depth)?
                .into_iter()
                .filter_map(|raw| if let Raw::Point(point) = raw { Some(point) } else { None })
                .collect(),
        ),
        5 => Raw::MultiLine(
            members(input, &header, Some(2), depth)?
                .into_iter()
                .filter_map(|raw| if let Raw::Line(line) = raw { Some(line) } else { None })
                .collect(),
        ),
        6 => Raw::MultiPolygon(
            members(input, &header, Some(3), depth)?
                .into_iter()
                .filter_map(|raw| {
                    if let Raw::Polygon(rings) = raw {
                        Some(rings)
                    } else {
                        None
                    }
                })
                .collect(),
        ),
        7 => Raw::Collection(members(input, &header, None, depth)?),
        other => return Err(format!("a WKB geometry type that is not read ({other})")),
    };
    Ok((header.srid, raw, header.kind))
}

/// One geometry in hex (a delimited file's cell, a line of a hex file), all of its bytes used.
pub(super) fn parse_hex(text: &str) -> Result<(Option<u32>, Raw), String> {
    let text = text.trim();
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err("hex WKB of an odd length".to_string());
    }
    let bytes: Result<Vec<u8>, String> = digits
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| "hex WKB with a character that is not a hex digit".to_string())
        })
        .collect();
    let bytes = bytes?;
    let mut input = Bytes { bytes: &bytes, at: 0 };
    let (srid, raw, _) = geometry(&mut input, 0)?;
    if input.left() > 0 {
        return Err("hex WKB with bytes after its geometry".to_string());
    }
    Ok((srid, raw))
}

impl Reader {
    /// A `.wkb` file: binary geometries one after another, or hex text, one a line.
    pub(super) fn wkb_file(&mut self, bytes: &[u8]) -> Result<(), OverlayError> {
        if bytes
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() || byte.is_ascii_whitespace())
        {
            let text = std::str::from_utf8(bytes).unwrap_or_default();
            for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                self.number += 1;
                self.add_with_srid(parse_hex(line), "", "")?;
            }
        } else {
            let mut input = Bytes { bytes, at: 0 };
            while input.left() > 0 {
                self.number += 1;
                match geometry(&mut input, 0) {
                    Ok((srid, raw, _)) => self.add_with_srid(Ok((srid, raw)), "", "")?,
                    Err(reason) => {
                        self.reject(&reason);
                        if input.left() > 0 {
                            self.count("the rest of the file was not read after a geometry that does not read");
                        }
                        break;
                    }
                }
            }
        }
        if self.number == 0 {
            return Err(OverlayError::new("NOT_FORMAT", "the file has no WKB geometry"));
        }
        Ok(())
    }
}
