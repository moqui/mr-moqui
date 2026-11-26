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

//! WKT (OGC simple features, with EWKT's `SRID=n;` prefix) and delimited text as overlays (plans/Maps.md §4.1).
//!
//! - A `.wkt` file is one geometry a line (a line that is empty or starts with `#` is passed over), each a feature with no
//!   label: POINT, LINESTRING, POLYGON, their MULTI forms and GEOMETRYCOLLECTION, with Z, M or ZM ordinates (dropped).
//!   An SRID of 4326 or 3857 (and Web Mercator's older codes) is read; another refuses the file (CRS_UNSUPPORTED).
//! - A `.csv` (or `.tsv`) file has a header row and one feature a row, comma, semicolon or tab separated (the header's
//!   most frequent of them), quoted as RFC 4180 has it: its geometry is the column named WKT, geometry, geom, the_geom,
//!   shape or WKB (WKT, EWKT or hex WKB in it), or else the columns named latitude or lat and longitude, lon, lng or
//!   long; its label and description the columns a GeoJSON feature's properties would give them (name, title, label;
//!   description, desc), and the other columns are counted, not kept.

use serde_json::{Map, Value};

use super::{wkb, Crs, OverlayError, Position, Raw, Reader};

/// The deepest GEOMETRYCOLLECTION nesting read.
const MAX_NESTING: usize = 4;
const GEOMETRY_COLUMNS: [&str; 7] = ["wkt", "geometry", "geom", "the_geom", "shape", "wkb", "geometry_wkt"];
const LATITUDE_COLUMNS: [&str; 2] = ["latitude", "lat"];
const LONGITUDE_COLUMNS: [&str; 4] = ["longitude", "lon", "lng", "long"];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token<'a> {
    Word(&'a str),
    Number(f64),
    Open,
    Close,
    Comma,
}

/// A WKT text's tokens: words, numbers and the punctuation, nothing else.
fn tokenize(text: &str) -> Result<Vec<Token<'_>>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(first) = rest.chars().next() {
        if first.is_whitespace() {
            rest = &rest[first.len_utf8()..];
            continue;
        }
        let (token, used) = match first {
            '(' | '[' => (Token::Open, 1),
            ')' | ']' => (Token::Close, 1),
            ',' => (Token::Comma, 1),
            letter if letter.is_ascii_alphabetic() => {
                let end = rest
                    .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                    .unwrap_or(rest.len());
                (Token::Word(&rest[..end]), end)
            }
            digit if digit.is_ascii_digit() || matches!(digit, '-' | '+' | '.') => {
                let end = rest
                    .find(|ch: char| !(ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E')))
                    .unwrap_or(rest.len());
                let number: f64 = rest[..end].parse().map_err(|_| {
                    format!(
                        "a number that does not read ({})",
                        super::plain_text(&rest[..end], 30, false)
                    )
                })?;
                (Token::Number(number), end)
            }
            other => return Err(format!("a character WKT does not have ({other})")),
        };
        out.push(token);
        rest = &rest[used..];
    }
    Ok(out)
}

/// A WKT geometry's tokens read from the front.
struct Parser<'a> {
    tokens: Vec<Token<'a>>,
    at: usize,
}

impl<'a> Parser<'a> {
    fn next(&mut self) -> Option<Token<'a>> {
        let token = self.tokens.get(self.at).copied();
        self.at += 1;
        token
    }

    fn peek(&self) -> Option<Token<'a>> {
        self.tokens.get(self.at).copied()
    }

    fn expect(&mut self, wanted: Token<'static>, what: &str) -> Result<(), String> {
        match self.next() {
            Some(token) if token == wanted => Ok(()),
            _ => Err(format!("WKT that does not read: {what} expected")),
        }
    }

    /// A position: two to four numbers, the first two kept.
    fn position(&mut self) -> Result<Position, String> {
        let mut numbers = Vec::with_capacity(4);
        while let Some(Token::Number(number)) = self.peek() {
            self.at += 1;
            numbers.push(number);
        }
        match numbers.as_slice() {
            [x, y] | [x, y, _] | [x, y, _, _] => Ok([*x, *y]),
            _ => Err("a WKT position that is not two to four numbers".to_string()),
        }
    }

    /// Items of `body` between parentheses, separated by commas.
    fn list<T>(&mut self, mut body: impl FnMut(&mut Self) -> Result<T, String>) -> Result<Vec<T>, String> {
        self.expect(Token::Open, "(")?;
        let mut out = vec![body(self)?];
        loop {
            match self.next() {
                Some(Token::Comma) => out.push(body(self)?),
                Some(Token::Close) => return Ok(out),
                _ => return Err("WKT that does not read: , or ) expected".to_string()),
            }
        }
    }

    fn positions(&mut self) -> Result<Vec<Position>, String> {
        self.list(Self::position)
    }

    fn rings(&mut self) -> Result<Vec<Vec<Position>>, String> {
        self.list(Self::positions)
    }

    /// A MULTIPOINT's point: `(x y)` or a bare `x y`.
    fn point_item(&mut self) -> Result<Position, String> {
        if self.peek() == Some(Token::Open) {
            self.at += 1;
            let point = self.position()?;
            self.expect(Token::Close, ")")?;
            Ok(point)
        } else {
            self.position()
        }
    }

    fn geometry(&mut self, depth: usize) -> Result<Raw, String> {
        let Some(Token::Word(kind)) = self.next() else {
            return Err("WKT that does not start with a geometry type".to_string());
        };
        let kind = kind.to_ascii_uppercase();
        // Z, M or ZM, as a word of its own or written onto the type (POINTZ)
        let kind = kind
            .strip_suffix("ZM")
            .or_else(|| kind.strip_suffix('Z'))
            .or_else(|| kind.strip_suffix('M'))
            .filter(|base| {
                matches!(
                    *base,
                    "POINT"
                        | "LINESTRING"
                        | "POLYGON"
                        | "MULTIPOINT"
                        | "MULTILINESTRING"
                        | "MULTIPOLYGON"
                        | "GEOMETRYCOLLECTION"
                )
            })
            .map_or(kind.clone(), str::to_string);
        if let Some(Token::Word(word)) = self.peek() {
            if matches!(word.to_ascii_uppercase().as_str(), "Z" | "M" | "ZM") {
                self.at += 1;
            }
        }
        if let Some(Token::Word(word)) = self.peek() {
            if word.eq_ignore_ascii_case("EMPTY") {
                return Err("an empty geometry".to_string());
            }
        }
        Ok(match kind.as_str() {
            "POINT" => {
                self.expect(Token::Open, "(")?;
                let point = self.position()?;
                self.expect(Token::Close, ")")?;
                Raw::Point(point)
            }
            "LINESTRING" => Raw::Line(self.positions()?),
            "POLYGON" => Raw::Polygon(self.rings()?),
            "MULTIPOINT" => Raw::MultiPoint(self.list(Self::point_item)?),
            "MULTILINESTRING" => Raw::MultiLine(self.rings()?),
            "MULTIPOLYGON" => Raw::MultiPolygon(self.list(Self::rings)?),
            "GEOMETRYCOLLECTION" => {
                if depth >= MAX_NESTING {
                    return Err(format!("geometry collections nested deeper than {MAX_NESTING}"));
                }
                Raw::Collection(self.list(|parser| parser.geometry(depth + 1))?)
            }
            other => {
                return Err(format!(
                    "a WKT type that is not read ({})",
                    super::plain_text(other, 40, false)
                ))
            }
        })
    }
}

/// One WKT or EWKT geometry: its SRID (None when it names none) and the geometry.
pub(super) fn parse_wkt(text: &str) -> Result<(Option<u32>, Raw), String> {
    let text = text.trim();
    let srid_head = text
        .split_once(';')
        .and_then(|(head, body)| head.split_once('=').map(|(key, code)| (key, code, body)))
        .filter(|(key, _, _)| key.trim().eq_ignore_ascii_case("SRID"));
    let (srid, body) = match srid_head {
        Some((_, code, body)) => {
            let code: u32 = code
                .trim()
                .parse()
                .map_err(|_| "an EWKT SRID that is not a number".to_string())?;
            (Some(code), body)
        }
        None => (None, text),
    };
    let mut parser = Parser {
        tokens: tokenize(body)?,
        at: 0,
    };
    let raw = parser.geometry(0)?;
    if parser.at != parser.tokens.len() {
        return Err("WKT with more after its geometry".to_string());
    }
    Ok((srid, raw))
}

/// Whether a cell is hex WKB (an even count of hex digits, long enough for a point's header) rather than WKT.
fn is_hex_wkb(text: &str) -> bool {
    let text = text.trim();
    text.len() >= 18 && text.len().is_multiple_of(2) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Reader {
    /// A geometry with its SRID: an SRID not read here refuses the file, as a declared CRS does.
    pub(super) fn add_with_srid(
        &mut self,
        parsed: Result<(Option<u32>, Raw), String>,
        label: &str,
        description: &str,
    ) -> Result<(), OverlayError> {
        let raw = match parsed {
            Ok((srid, raw)) => {
                let crs = match srid {
                    Some(code) => Crs::of_code(code).ok_or_else(|| super::crs_unsupported(&format!("SRID {code}")))?,
                    None => {
                        self.count("geometries that name no SRID were read as WGS84 longitude, latitude");
                        Crs::Wgs84
                    }
                };
                self.use_crs(crs)?;
                Ok(raw)
            }
            Err(reason) => Err(reason),
        };
        self.add_raw(raw, label, description)
    }

    /// A `.wkt` file: one geometry a line.
    pub(super) fn wkt_file(&mut self, text: &str) -> Result<(), OverlayError> {
        let mut any = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            any = true;
            self.number += 1;
            self.add_with_srid(parse_wkt(line), "", "")?;
        }
        if !any {
            return Err(OverlayError::new("NOT_FORMAT", "the file has no WKT geometry"));
        }
        Ok(())
    }

    /// Delimited text with a header row: a geometry column, or latitude and longitude columns.
    pub(super) fn delimited(&mut self, text: &str) -> Result<(), OverlayError> {
        let header = text.lines().next().unwrap_or_default();
        let candidates: [u8; 3] = *b",;\t";
        let delimiter = candidates
            .into_iter()
            .max_by_key(|delimiter| header.bytes().filter(|byte| byte == delimiter).count())
            .filter(|delimiter| header.as_bytes().contains(delimiter))
            .unwrap_or(b',');
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(delimiter)
            .flexible(true)
            .has_headers(true)
            .from_reader(text.as_bytes());
        let headers: Vec<String> = reader
            .headers()
            .map_err(|err| OverlayError::new("NOT_PARSED", format!("the file's header row does not read: {err}")))?
            .iter()
            .map(|name| name.trim().to_string())
            .collect();
        let column = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| headers.iter().position(|header| header.eq_ignore_ascii_case(name)))
        };
        let geometry = column(&GEOMETRY_COLUMNS);
        let point = column(&LONGITUDE_COLUMNS).zip(column(&LATITUDE_COLUMNS));
        if geometry.is_none() && point.is_none() {
            return Err(OverlayError::new(
                "NOT_FORMAT",
                "a delimited file needs a column named WKT, geometry, geom, the_geom, shape or WKB, or latitude (lat) and \
                 longitude (lon, lng, long) columns",
            ));
        }
        let used: Vec<usize> = geometry
            .into_iter()
            .chain(point.into_iter().flat_map(|(lon, lat)| [lon, lat]))
            .collect();
        for record in reader.records() {
            self.number += 1;
            let record = match record {
                Ok(record) => record,
                Err(err) => {
                    self.reject(&format!("a row that does not read ({err})"));
                    continue;
                }
            };
            let mut properties = Map::new();
            for (index, value) in record.iter().enumerate() {
                if used.contains(&index) || value.trim().is_empty() {
                    continue;
                }
                if let Some(name) = headers.get(index) {
                    properties.insert(name.clone(), Value::String(value.to_string()));
                }
            }
            let (label, description) = self.pick_properties(Some(&properties));
            let cell = |index: usize| record.get(index).unwrap_or_default().trim();
            let parsed = match (geometry, point) {
                (Some(index), _) if !cell(index).is_empty() => {
                    let value = cell(index);
                    if is_hex_wkb(value) {
                        wkb::parse_hex(value)
                    } else {
                        parse_wkt(value)
                    }
                }
                (_, Some((lon, lat))) => match (cell(lon).parse::<f64>(), cell(lat).parse::<f64>()) {
                    (Ok(lon), Ok(lat)) => Ok((Some(4326), Raw::Point([lon, lat]))),
                    _ => Err("a latitude or longitude that is not a number".to_string()),
                },
                _ => Err("a row with no geometry".to_string()),
            };
            self.add_with_srid(parsed, &label, &description)?;
        }
        Ok(())
    }
}
