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

//! The WKB geometries of Overture's `geometry` column (GeoParquet's `WKB` encoding): a place's Point and a building's
//! Polygon or MultiPolygon, in WGS84 longitude and latitude, ISO or EWKB Z and M ordinates dropped. Every count is
//! checked against the bytes left before anything is kept; any other geometry is refused, which counts the row as
//! rejected.

/// A geometry read: each polygon its rings, the outer ring first.
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    Point([f64; 2]),
    Polygons(Vec<Vec<Vec<[f64; 2]>>>),
}

struct Bytes<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Bytes<'_> {
    fn left(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
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

    fn count(&mut self, little: bool, size: usize) -> Result<usize, String> {
        let count = usize::try_from(self.u32(little)?).unwrap_or(usize::MAX);
        if count.saturating_mul(size) > self.left() {
            return Err(format!("WKB that claims {count} items, more than its bytes hold"));
        }
        Ok(count)
    }
}

/// A geometry's header: its byte order, kind (1 to 7) and ordinates of a position.
fn header(bytes: &mut Bytes<'_>) -> Result<(bool, u32, usize), String> {
    let order = bytes.take::<1>()?[0];
    let little = match order {
        0 => false,
        1 => true,
        other => return Err(format!("WKB with byte order {other}")),
    };
    let raw = bytes.u32(little)?;
    let mut ordinates = 2;
    // EWKB flags
    if raw & 0x8000_0000 != 0 {
        ordinates += 1;
    }
    if raw & 0x4000_0000 != 0 {
        ordinates += 1;
    }
    if raw & 0x2000_0000 != 0 {
        bytes.u32(little)?;
    }
    let iso = raw & 0x0fff_ffff;
    let kind = iso % 1000;
    ordinates += match iso / 1000 {
        0 => 0,
        1 | 2 => 1,
        3 => 2,
        other => return Err(format!("WKB of dimension code {other}")),
    };
    Ok((little, kind, ordinates.min(4)))
}

fn position(bytes: &mut Bytes<'_>, little: bool, ordinates: usize) -> Result<[f64; 2], String> {
    let lon = bytes.f64(little)?;
    let lat = bytes.f64(little)?;
    for _ in 2..ordinates {
        bytes.f64(little)?;
    }
    if !(lon.is_finite() && lat.is_finite() && (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat)) {
        return Err(format!("a WKB position outside longitude and latitude: {lon}, {lat}"));
    }
    Ok([lon, lat])
}

fn polygon(
    bytes: &mut Bytes<'_>,
    little: bool,
    ordinates: usize,
    budget: &mut usize,
) -> Result<Vec<Vec<[f64; 2]>>, String> {
    let rings = bytes.count(little, 4)?;
    let mut out = Vec::with_capacity(rings.min(64));
    for _ in 0..rings {
        let points = bytes.count(little, 8 * ordinates)?;
        if points > *budget {
            return Err("a WKB geometry of too many positions".to_string());
        }
        *budget -= points;
        let mut ring = Vec::with_capacity(points);
        for _ in 0..points {
            ring.push(position(bytes, little, ordinates)?);
        }
        if ring.len() < 4 {
            return Err("a WKB polygon ring of fewer than four positions".to_string());
        }
        out.push(ring);
    }
    if out.is_empty() {
        return Err("a WKB polygon with no ring".to_string());
    }
    Ok(out)
}

/// Read one geometry of at most `max_positions` positions.
///
/// # Errors
///
/// Bytes that are not a WKB Point, Polygon or MultiPolygon in longitude and latitude, or more positions than allowed.
pub fn read(wkb: &[u8], max_positions: usize) -> Result<Geometry, String> {
    let mut bytes = Bytes { bytes: wkb, at: 0 };
    let (little, kind, ordinates) = header(&mut bytes)?;
    let mut budget = max_positions;
    match kind {
        1 => Ok(Geometry::Point(position(&mut bytes, little, ordinates)?)),
        3 => Ok(Geometry::Polygons(vec![polygon(
            &mut bytes,
            little,
            ordinates,
            &mut budget,
        )?])),
        6 => {
            let count = bytes.count(little, 9)?;
            let mut out = Vec::with_capacity(count.min(64));
            for _ in 0..count {
                let (little, kind, ordinates) = header(&mut bytes)?;
                if kind != 3 {
                    return Err(format!("a WKB MultiPolygon member of kind {kind}"));
                }
                out.push(polygon(&mut bytes, little, ordinates, &mut budget)?);
            }
            if out.is_empty() {
                return Err("an empty WKB MultiPolygon".to_string());
            }
            Ok(Geometry::Polygons(out))
        }
        other => Err(format!("a WKB geometry of kind {other}, not a point or polygon")),
    }
}
