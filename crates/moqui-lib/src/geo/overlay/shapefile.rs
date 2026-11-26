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

//! A zipped shapefile as an overlay (plans/Maps.md §4.1, §7; the ESRI Shapefile Technical Description, 1998, and
//! dBASE III): the zip's one `.shp`, read record by record in order (its `.shx` index is not needed), with the `.dbf`
//! record of the same number as the feature's properties (read as a GeoJSON feature's are: name, title or label and
//! description or desc kept as plain text, the other fields counted), the `.prj` for its CRS and the `.cpg` for the
//! `.dbf`'s text encoding (UTF-8 or Windows-1252; with none, UTF-8 when a field is valid UTF-8 and Windows-1252 when
//! not; another code page's fields that are not UTF-8 are left out and counted). A dBASE 7 table (48-byte field
//! descriptors) is not read: its features have no names, and that is noted.
//!
//! - The CRS: a `.prj` of WGS84 (a GEOGCS of the WGS 84 datum, or WKT2's GEOGCRS of it or its ensemble) or Web
//!   Mercator (EPSG:3857 by its authority, or a projection named Web or Pseudo Mercator or Mercator_Auxiliary_Sphere)
//!   is read, and so is a CRS of the projection registry the `.prj` names by its EPSG authority, by ESRI's name for it
//!   (`WGS_1984_UTM_Zone_33N`, `British_National_Grid`) or by its parameters ([`crate::geo::proj::Registry::find_wkt`]);
//!   any other is refused by name (CRS_UNSUPPORTED); with no `.prj` the coordinates are read as WGS84 when the file's
//!   bounding box is within degrees, and refused when not.
//! - Bounded: at most 64 zip entries; each member read is inflated to at most what is left of four times the overlay's
//!   `max_bytes` for all of them, and at most a hundred times its compressed size (past a MiB). Every record's length,
//!   part count and point count is checked against the bytes it has before anything is read, so a claimed count costs
//!   nothing.
//! - Points, multipoints, polylines and polygons, with Z and M (dropped); polygon rings grouped as the format winds them
//!   (clockwise outer rings, counterclockwise holes); a null shape or a multipatch is a feature left out.
//!
//! Our own reader, not the `shapefile` crate (MIT, with `dbase`): the format is a few fixed headers, and the bounds above
//! are this reader's to keep.

use std::io::{Cursor, Read};

use serde_json::{Map, Value};

use super::{crs_unsupported, rings_to_polygons, Crs, OverlayError, Position, Raw, Reader};

const MAX_ENTRIES: usize = 64;
const MAX_RATIO: u64 = 100;
const RATIO_SLACK: u64 = 1 << 20;
/// The most of a `.prj` or `.cpg` read.
const SMALL_MEMBER: u64 = 64 * 1024;

/// A zip member's name as a message may show it: plain text, cut short.
fn shown(name: &str) -> String {
    super::plain_text(name, 120, false)
}

fn not_format(message: impl Into<String>) -> OverlayError {
    OverlayError::new("NOT_FORMAT", message)
}

/// The zip's members read, within what they may inflate to in all.
struct Members<'a> {
    archive: zip::ZipArchive<Cursor<&'a [u8]>>,
    /// bytes still allowed to inflate
    left: u64,
}

impl Members<'_> {
    fn read(&mut self, name: &str, cap: u64) -> Result<Vec<u8>, OverlayError> {
        let entry = self
            .archive
            .by_name(name)
            .map_err(|err| not_format(format!("the zip's {} does not read: {err}", shown(name))))?;
        let compressed = entry.compressed_size();
        let limit = cap
            .min(self.left)
            .min(compressed.saturating_mul(MAX_RATIO).saturating_add(RATIO_SLACK));
        let mut out = Vec::new();
        entry
            .take(limit.saturating_add(1))
            .read_to_end(&mut out)
            .map_err(|err| not_format(format!("the zip's {} does not inflate: {err}", shown(name))))?;
        if out.len() as u64 > limit {
            return Err(OverlayError::new(
                "TOO_LARGE",
                format!(
                    "the zip's {} inflates to more than a shapefile overlay may (at most {limit} bytes)",
                    shown(name)
                ),
            ));
        }
        self.left -= out.len() as u64;
        Ok(out)
    }
}

/// A `.prj`'s CRS: WGS84, Web Mercator or a registered one, else refused by the name it gives.
fn prj_crs(text: &str, projections: &crate::geo::proj::Registry) -> Result<Crs, OverlayError> {
    if let Some(found) = projections.find_wkt(text) {
        return Ok(Crs::of_code(found.code).unwrap_or(Crs::Registered(found.code)));
    }
    let upper = text.trim().to_ascii_uppercase();
    let name = text.split('"').nth(1).map_or_else(
        || super::plain_text(text, 80, false),
        |name| super::plain_text(name, 80, false),
    );
    let projected = upper.starts_with("PROJCS") || upper.starts_with("PROJCRS");
    if projected {
        let mercator = [
            "WEB_MERCATOR",
            "WEB MERCATOR",
            "PSEUDO-MERCATOR",
            "PSEUDO_MERCATOR",
            "MERCATOR_AUXILIARY_SPHERE",
        ]
        .iter()
        .any(|marker| upper.contains(marker))
            || upper
                .trim_end_matches([']', ' ', '\n', '\r'])
                .ends_with("AUTHORITY[\"EPSG\",\"3857\"")
            || upper.contains("ID[\"EPSG\",3857]");
        return if mercator {
            Ok(Crs::WebMercator)
        } else {
            Err(crs_unsupported(&name))
        };
    }
    let geographic = upper.starts_with("GEOGCS") || upper.starts_with("GEOGCRS") || upper.starts_with("GEODCRS");
    // WKT1's DATUM, or WKT2's (GEOGCRS) DATUM or datum ENSEMBLE ("World Geodetic System 1984 ensemble")
    let datum = upper
        .split_once("DATUM[")
        .or_else(|| upper.split_once("ENSEMBLE["))
        .map_or("", |(_, datum)| datum);
    let wgs84 = ["WGS_1984", "WGS 84", "WGS84", "WORLD GEODETIC SYSTEM 1984"]
        .iter()
        .any(|marker| datum.contains(marker));
    if geographic && wgs84 {
        Ok(Crs::Wgs84)
    } else {
        Err(crs_unsupported(&name))
    }
}

/// How a `.dbf`'s text is read: by its `.cpg`, UTF-8 or Windows-1252 (Latin-1 named so too); with none, UTF-8 where a
/// field is valid UTF-8 and Windows-1252 where not (what most writers without a `.cpg` meant); another code page is not
/// decoded: a field of it that is not UTF-8 is left out, and counted, rather than shown as the wrong letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Cp1252,
    Guess,
    Other,
}

impl Encoding {
    fn of_cpg(text: &str) -> Self {
        let page = text.trim().to_ascii_uppercase().replace(['-', '_', ' '], "");
        match page.as_str() {
            "UTF8" | "65001" => Self::Utf8,
            "1252" | "ANSI1252" | "CP1252" | "WINDOWS1252" | "LATIN1" | "ISO88591" | "88591" | "28591" => Self::Cp1252,
            "" => Self::Guess,
            _ => Self::Other,
        }
    }

    /// A field's text, or None when its code page is not read and it is not UTF-8.
    fn decode(self, raw: &[u8]) -> Option<String> {
        match (std::str::from_utf8(raw), self) {
            (Ok(text), _) if self != Self::Cp1252 => Some(text.to_string()),
            (_, Self::Utf8) => Some(String::from_utf8_lossy(raw).into_owned()),
            (_, Self::Cp1252 | Self::Guess) => Some(super::decode_cp1252(raw)),
            (_, Self::Other) => None,
        }
    }
}

/// A `.dbf` table (dBASE III or IV, 32-byte field descriptors): its fields and records, each read when asked for.
struct Table<'a> {
    bytes: &'a [u8],
    /// name, offset in the record (after the deletion flag), width
    fields: Vec<(String, usize, usize)>,
    header_len: usize,
    record_len: usize,
    records: usize,
    encoding: Encoding,
}

/// A record read: its properties, and whether a field was left out for its code page.
struct Record {
    properties: Map<String, Value>,
    undecoded: bool,
}

impl<'a> Table<'a> {
    fn parse(bytes: &'a [u8], encoding: Encoding) -> Result<Self, String> {
        if bytes.len() < 32 {
            return Err("a .dbf shorter than its header".to_string());
        }
        // dBASE 7 has 48-byte field descriptors, which are not read
        if bytes[0] & 0x07 == 4 {
            return Err("a dBASE 7 table, which is not read".to_string());
        }
        let records = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let header_len = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        let record_len = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
        if header_len < 33 || header_len > bytes.len() || record_len == 0 {
            return Err("a .dbf whose header lengths are not its own".to_string());
        }
        let mut fields = Vec::new();
        let mut offset = 0;
        let mut at = 32;
        while at + 32 <= header_len && bytes[at] != 0x0D {
            let descriptor = &bytes[at..at + 32];
            let name_end = descriptor[..11].iter().position(|byte| *byte == 0).unwrap_or(11);
            let name = String::from_utf8_lossy(&descriptor[..name_end]).trim().to_string();
            let width = usize::from(descriptor[16]);
            fields.push((name, offset, width));
            offset += width;
            at += 32;
        }
        if offset + 1 > record_len {
            return Err("a .dbf whose fields are wider than its records".to_string());
        }
        // the records the file holds, whatever its header claims
        let records = records.min((bytes.len() - header_len) / record_len);
        Ok(Self {
            bytes,
            fields,
            header_len,
            record_len,
            records,
            encoding,
        })
    }

    /// Record `index` (0 first); None when it is marked deleted or past the table.
    fn record(&self, index: usize) -> Option<Record> {
        if index >= self.records {
            return None;
        }
        let start = self.header_len + index * self.record_len;
        let record = self.bytes.get(start..start + self.record_len)?;
        if record[0] == b'*' {
            return None;
        }
        let mut properties = Map::new();
        let mut undecoded = false;
        for (name, offset, width) in &self.fields {
            let Some(raw) = record.get(1 + offset..1 + offset + width) else {
                continue;
            };
            let Some(text) = self.encoding.decode(raw) else {
                undecoded = true;
                continue;
            };
            let text = text.trim_matches(|ch: char| ch.is_whitespace() || ch == '\0');
            if !text.is_empty() {
                properties.insert(name.clone(), Value::String(text.to_string()));
            }
        }
        Some(Record { properties, undecoded })
    }
}

/// The `.shp` read from the front, little and big endian.
struct Shapes<'a> {
    bytes: &'a [u8],
    at: usize,
}

fn le_i32(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn be_i32(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn le_f64(bytes: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// A record's content: its shape type and geometry.
fn shape(content: &[u8]) -> Result<Raw, String> {
    let kind = le_i32(content, 0).ok_or_else(|| "a record with no shape type".to_string())?;
    let point = |at: usize| -> Result<Position, String> {
        match (le_f64(content, at), le_f64(content, at + 8)) {
            (Some(x), Some(y)) => Ok([x, y]),
            _ => Err("a record shorter than its points".to_string()),
        }
    };
    match kind {
        0 => Err("a null shape".to_string()),
        1 | 11 | 21 => Ok(Raw::Point(point(4)?)),
        8 | 18 | 28 => {
            let count = le_i32(content, 36)
                .and_then(|count| usize::try_from(count).ok())
                .ok_or("a multipoint with no count")?;
            if count.saturating_mul(16) > content.len().saturating_sub(40) {
                return Err(format!(
                    "a multipoint that claims {count} points, more than its record holds"
                ));
            }
            Ok(Raw::MultiPoint(
                (0..count)
                    .map(|index| point(40 + index * 16))
                    .collect::<Result<_, _>>()?,
            ))
        }
        3 | 13 | 23 | 5 | 15 | 25 => {
            let parts = le_i32(content, 36)
                .and_then(|count| usize::try_from(count).ok())
                .ok_or("a shape with no part count")?;
            let points = le_i32(content, 40)
                .and_then(|count| usize::try_from(count).ok())
                .ok_or("a shape with no point count")?;
            let points_at = 44usize.saturating_add(parts.saturating_mul(4));
            if points_at.saturating_add(points.saturating_mul(16)) > content.len() {
                return Err(format!(
                    "a shape that claims {parts} parts and {points} points, more than its record holds"
                ));
            }
            let starts: Vec<usize> = (0..parts)
                .map(|index| {
                    le_i32(content, 44 + index * 4)
                        .and_then(|start| usize::try_from(start).ok())
                        .unwrap_or(usize::MAX)
                })
                .collect();
            let mut lines = Vec::with_capacity(parts);
            for (index, start) in starts.iter().enumerate() {
                let end = starts.get(index + 1).copied().unwrap_or(points);
                if *start > end || end > points {
                    return Err("a shape whose parts are not in order within its points".to_string());
                }
                lines.push(
                    (*start..end)
                        .map(|at| point(points_at + at * 16))
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            if matches!(kind, 3 | 13 | 23) {
                Ok(Raw::MultiLine(lines))
            } else {
                let mut polygons = rings_to_polygons(lines)?;
                Ok(if polygons.len() == 1 {
                    Raw::Polygon(polygons.remove(0))
                } else {
                    Raw::MultiPolygon(polygons)
                })
            }
        }
        31 => Err("a multipatch, which is not read".to_string()),
        other => Err(format!("a shape type that is not read ({other})")),
    }
}

impl Shapes<'_> {
    /// The next record's content, or None at the end; an error when the record runs past the file.
    fn next(&mut self) -> Option<Result<&[u8], String>> {
        if self.at + 8 > self.bytes.len() {
            return None;
        }
        let words = be_i32(self.bytes, self.at + 4).and_then(|words| usize::try_from(words).ok());
        let Some(length) = words.and_then(|words| words.checked_mul(2)) else {
            return Some(Err("a record of a negative length".to_string()));
        };
        let start = self.at + 8;
        let Some(content) = self.bytes.get(start..start.saturating_add(length)) else {
            self.at = self.bytes.len();
            return Some(Err("a record that runs past the end of the .shp".to_string()));
        };
        self.at = start + length;
        Some(Ok(content))
    }
}

impl Reader {
    /// A zip of a shapefile.
    pub(super) fn shapefile(&mut self, bytes: &[u8], max_bytes: usize) -> Result<(), OverlayError> {
        if !bytes.starts_with(b"PK") {
            return Err(not_format(
                "the file is not a zip (a shapefile overlay is a .zip of its .shp, .dbf and .prj)",
            ));
        }
        let archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|err| OverlayError::new("NOT_PARSED", format!("the zip does not read: {err}")))?;
        if archive.len() > MAX_ENTRIES {
            return Err(not_format(format!(
                "the zip has {} entries, more than the {MAX_ENTRIES} a shapefile overlay may",
                archive.len()
            )));
        }
        let names: Vec<String> = archive
            .file_names()
            .filter_map(Result::ok)
            .map(std::borrow::Cow::into_owned)
            .filter(|name| !name.ends_with('/') && !name.starts_with("__MACOSX/") && !name.contains("/._"))
            .collect();
        let with = |extension: &str| -> Vec<&String> {
            names
                .iter()
                .filter(|name| name.to_ascii_lowercase().ends_with(extension))
                .collect()
        };
        let shp = match with(".shp").as_slice() {
            [one] => (*one).clone(),
            [] => return Err(not_format("the zip has no .shp file")),
            many => {
                let listed: Vec<&str> = many.iter().take(5).map(|name| name.as_str()).collect();
                return Err(not_format(format!(
                    "the zip has more than one shapefile ({}): zip one a file",
                    super::plain_text(&listed.join(", "), 200, false)
                )));
            }
        };
        let lower = shp.to_ascii_lowercase();
        let base = lower.strip_suffix(".shp").unwrap_or(&lower).to_string();
        let sibling = |extension: &str| {
            names
                .iter()
                .find(|name| name.to_ascii_lowercase() == format!("{base}{extension}"))
                .cloned()
        };
        let mut members = Members {
            archive,
            left: (max_bytes as u64).saturating_mul(4),
        };
        let shp_bytes = members.read(&shp, u64::MAX)?;
        let prj = match sibling(".prj") {
            Some(name) => Some(String::from_utf8_lossy(&members.read(&name, SMALL_MEMBER)?).into_owned()),
            None => None,
        };
        let encoding = match sibling(".cpg") {
            Some(name) => Encoding::of_cpg(&String::from_utf8_lossy(&members.read(&name, SMALL_MEMBER)?)),
            None => Encoding::Guess,
        };
        if encoding == Encoding::Other {
            self.count("the .cpg names a code page that is not read: names in it that are not UTF-8 were left out");
        }
        let dbf_bytes = match sibling(".dbf") {
            Some(name) => Some(members.read(&name, u64::MAX)?),
            None => None,
        };
        if shp_bytes.len() < 100 || be_i32(&shp_bytes, 0) != Some(9994) {
            return Err(not_format("the .shp is not a shapefile (its header is not one)"));
        }
        let crs = match &prj {
            Some(text) => prj_crs(text, &self.projections)?,
            None => {
                let in_degrees = [(36, 180.0), (44, 90.0), (52, 180.0), (60, 90.0)]
                    .iter()
                    .all(|(at, limit)| le_f64(&shp_bytes, *at).is_some_and(|value: f64| value.abs() <= *limit));
                if !in_degrees {
                    return Err(crs_unsupported(
                        "a CRS the zip does not name (it has no .prj) and that is not degrees",
                    ));
                }
                self.count("the zip has no .prj, so the shapefile was read as WGS84 (EPSG:4326)");
                Crs::Wgs84
            }
        };
        let table = match &dbf_bytes {
            Some(bytes) => match Table::parse(bytes, encoding) {
                Ok(table) => Some(table),
                Err(reason) => {
                    self.count(if reason.contains("dBASE 7") {
                        "the .dbf is a dBASE 7 table, which is not read, so the features have no names"
                    } else {
                        "the .dbf's header does not read, so the features have no names"
                    });
                    tracing::debug!("shapefile overlay: {reason}");
                    None
                }
            },
            None => {
                self.count("the zip has no .dbf, so the features have no names");
                None
            }
        };
        let mut shapes = Shapes {
            bytes: &shp_bytes,
            at: 100,
        };
        let mut index = 0;
        while let Some(record) = shapes.next() {
            self.number += 1;
            let found = table.as_ref().and_then(|table| table.record(index));
            let deleted = table
                .as_ref()
                .is_some_and(|table| index < table.records && found.is_none());
            index += 1;
            if deleted {
                self.count("{count} records marked deleted in the .dbf were left out");
                continue;
            }
            if found.as_ref().is_some_and(|record| record.undecoded) {
                self.count("{count} records had fields in the .cpg's code page, which were left out");
            }
            let (label, description) = self.pick_properties(found.as_ref().map(|record| &record.properties));
            self.use_crs(crs)?;
            match record {
                Ok(content) => self.add_raw(shape(content), &label, &description)?,
                Err(reason) => {
                    self.reject(&reason);
                    break;
                }
            }
        }
        if table.as_ref().is_some_and(|table| table.records != index) {
            self.count("the .dbf and the .shp have different record counts");
        }
        Ok(())
    }
}
