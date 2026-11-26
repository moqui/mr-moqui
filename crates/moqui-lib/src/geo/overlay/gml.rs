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

//! A GML file as an overlay (plans/Maps.md §4.1): GML 3.2 simple features (and GML 3.1 and 2, which differ in names),
//! as a WFS answer or an OGR export has them. Each child of a `featureMember`, `featureMembers` or `member` is a feature
//! (a file whose root is a geometry is one feature): its first geometry (Point, LineString, LinearRing, Polygon,
//! MultiPoint, MultiLineString, MultiCurve, MultiPolygon, MultiSurface, MultiGeometry, a Curve of line segments, a
//! Surface of polygon patches) and its simple properties (child elements with text: `gml:name` and `gml:description`
//! among them) read as a GeoJSON feature's are. Coordinates are `pos`, `posList` (by `srsDimension`), GML 2's
//! `coordinates` and `coord`. The CRS is the geometry's `srsName`, or the nearest element above it with one: EPSG:4326 as
//! a URN or an `http://www.opengis.net/def/crs` URL is latitude first, as GML 3.2 has it, the short `EPSG:4326` and the
//! legacy `.../epsg.xml#4326` longitude first (a member geometry's own `srsName` must be its geometry's CRS); Web
//! Mercator is read; none is WGS84 longitude first (noted); any other refuses the file. Arcs and other curves are not read. The XML reader refuses a DOCTYPE and entity declarations.

use serde_json::{Map, Value};

use crate::node::MNode;

use super::{crs_unsupported, local, Crs, OverlayError, Position, Raw, Reader};

const GEOMETRIES: [&str; 13] = [
    "Point",
    "LineString",
    "LinearRing",
    "Polygon",
    "MultiPoint",
    "MultiLineString",
    "MultiCurve",
    "MultiPolygon",
    "MultiSurface",
    "MultiGeometry",
    "Curve",
    "Surface",
    "CompositeCurve",
];

fn is_geometry(node: &MNode) -> bool {
    GEOMETRIES.contains(&local(&node.name()))
}

/// The first geometry at or below a node, depth first.
fn first_geometry(node: &MNode) -> Option<MNode> {
    if is_geometry(node) {
        return Some(node.clone());
    }
    node.children().iter().find_map(first_geometry)
}

/// The `srsName` of a node or of the nearest element above it.
fn srs_name(node: &MNode) -> Option<String> {
    let mut at = Some(node.clone());
    while let Some(node) = at {
        if let Some(name) = node.attribute("srsName").filter(|name| !name.trim().is_empty()) {
            return Some(name);
        }
        at = node.parent();
    }
    None
}

/// The ordinates of a position by `srsDimension` on the node or above it, 2 when none says.
fn dimension(node: &MNode) -> usize {
    let mut at = Some(node.clone());
    while let Some(node) = at {
        if let Some(found) = node
            .attribute("srsDimension")
            .and_then(|value| value.trim().parse::<usize>().ok())
        {
            return found.clamp(2, 4);
        }
        at = node.parent();
    }
    2
}

fn children_named(node: &MNode, names: &[&str]) -> Vec<MNode> {
    node.children()
        .into_iter()
        .filter(|child| names.contains(&local(&child.name())))
        .collect()
}

fn child_named(node: &MNode, names: &[&str]) -> Option<MNode> {
    node.children()
        .into_iter()
        .find(|child| names.contains(&local(&child.name())))
}

/// Reads a GML geometry's coordinates, latitude first or not.
struct Coordinates {
    crs: Crs,
    /// the positions are northing (latitude) first: an EPSG:4326 URN, or a registered CRS's URN whose order is so
    north_first: bool,
}

impl Coordinates {
    /// The reader of a member geometry: its own srsName's axis order, which must be its geometry's CRS (a member in
    /// another, or in one not read, rejects the feature: positions of one geometry are converted one way)
    fn member(&self, member: &MNode) -> Result<Self, String> {
        let Some(name) = member.attribute("srsName").filter(|name| !name.trim().is_empty()) else {
            return Ok(Self {
                crs: self.crs,
                north_first: self.north_first,
            });
        };
        match Crs::of_name(&name) {
            // the same CRS: its own form's order (a short name easting first, an authority's URN its order)
            Some(crs) if crs.code() == self.crs.code() => Ok(Self {
                crs,
                north_first: match (crs, self.crs) {
                    (Crs::Wgs84LatFirst, _) => true,
                    (Crs::RegisteredAuthority(_), Crs::RegisteredAuthority(_)) => self.north_first,
                    (Crs::RegisteredAuthority(_), _) => false,
                    _ => false,
                },
            }),
            Some(_) => Err("a member geometry in another CRS than its geometry's".to_string()),
            None => Err(format!(
                "a member geometry in a CRS that is not read ({})",
                super::plain_text(&name, 80, false)
            )),
        }
    }

    fn pair(&self, x: f64, y: f64) -> Position {
        if self.north_first {
            [y, x]
        } else {
            [x, y]
        }
    }

    fn numbers(text: &str) -> Result<Vec<f64>, String> {
        text.split_whitespace()
            .map(|part| {
                part.parse::<f64>()
                    .map_err(|_| "a GML coordinate that is not a number".to_string())
            })
            .collect()
    }

    /// The positions of a geometry element: its `posList`, its `pos` children, GML 2's `coordinates` or its `coord`s.
    fn positions(&self, node: &MNode) -> Result<Vec<Position>, String> {
        if let Some(list) = child_named(node, &["posList"]) {
            let size = dimension(&list);
            let numbers = Self::numbers(&list.text().unwrap_or_default())?;
            if numbers.len() % size != 0 {
                return Err("a GML posList whose numbers are not whole positions".to_string());
            }
            return Ok(numbers
                .chunks(size)
                .map(|chunk| self.pair(chunk[0], chunk[1]))
                .collect());
        }
        let points = children_named(node, &["pos"]);
        if !points.is_empty() {
            return points
                .iter()
                .map(|point| {
                    let numbers = Self::numbers(&point.text().unwrap_or_default())?;
                    match numbers.as_slice() {
                        [x, y, ..] => Ok(self.pair(*x, *y)),
                        _ => Err("a GML pos that is not a position".to_string()),
                    }
                })
                .collect();
        }
        if let Some(coordinates) = child_named(node, &["coordinates"]) {
            let tuple = coordinates.attribute("ts").filter(|ts| !ts.is_empty());
            let separator = coordinates
                .attribute("cs")
                .filter(|cs| !cs.is_empty())
                .unwrap_or_else(|| ",".to_string());
            let decimal = coordinates.attribute("decimal").filter(|decimal| !decimal.is_empty());
            let text = coordinates.text().unwrap_or_default();
            let tuples: Vec<&str> = match &tuple {
                Some(ts) if !ts.trim().is_empty() => text.split(ts.as_str()).collect(),
                _ => text.split_whitespace().collect(),
            };
            return tuples
                .iter()
                .filter(|tuple| !tuple.trim().is_empty())
                .map(|tuple| {
                    let mut parts = tuple.split(separator.as_str()).map(|part| {
                        let part = part.trim();
                        match &decimal {
                            Some(decimal) if decimal != "." => part.replace(decimal.as_str(), ".").parse::<f64>().ok(),
                            _ => part.parse::<f64>().ok(),
                        }
                    });
                    match (parts.next().flatten(), parts.next().flatten()) {
                        (Some(x), Some(y)) => Ok(self.pair(x, y)),
                        _ => Err("GML coordinates that are not x,y".to_string()),
                    }
                })
                .collect();
        }
        let coords = children_named(node, &["coord"]);
        if !coords.is_empty() {
            return coords
                .iter()
                .map(|coord| {
                    let value = |name: &str| {
                        child_named(coord, &[name])
                            .and_then(|axis| axis.text())
                            .and_then(|text| text.trim().parse::<f64>().ok())
                    };
                    match (value("X"), value("Y")) {
                        (Some(x), Some(y)) => Ok(self.pair(x, y)),
                        _ => Err("a GML coord with no X and Y".to_string()),
                    }
                })
                .collect();
        }
        Err("a GML geometry with no coordinates".to_string())
    }

    /// A ring: the LinearRing of an exterior or interior (outerBoundaryIs, innerBoundaryIs in GML 2).
    fn ring(&self, boundary: &MNode) -> Result<Vec<Position>, String> {
        let ring = child_named(boundary, &["LinearRing", "Ring"])
            .ok_or_else(|| "a GML boundary with no LinearRing".to_string())?;
        if local(&ring.name()) == "Ring" {
            // a Ring of curve members: their line segments, one after another
            let mut out = Vec::new();
            for member in children_named(&ring, &["curveMember"]) {
                let curve = first_geometry(&member).ok_or_else(|| "a GML ring member with no curve".to_string())?;
                out.extend(self.line(&curve)?);
            }
            return Ok(out);
        }
        self.positions(&ring)
    }

    fn polygon(&self, node: &MNode) -> Result<Vec<Vec<Position>>, String> {
        let exterior = child_named(node, &["exterior", "outerBoundaryIs"])
            .ok_or_else(|| "a GML polygon with no exterior".to_string())?;
        let mut rings = vec![self.ring(&exterior)?];
        for interior in children_named(node, &["interior", "innerBoundaryIs"]) {
            rings.push(self.ring(&interior)?);
        }
        Ok(rings)
    }

    /// A line: a LineString's positions, or a Curve's line segments one after another.
    fn line(&self, node: &MNode) -> Result<Vec<Position>, String> {
        match local(&node.name()) {
            "Curve" => {
                let segments =
                    child_named(node, &["segments"]).ok_or_else(|| "a GML curve with no segments".to_string())?;
                let mut out: Vec<Position> = Vec::new();
                for segment in segments.children() {
                    if !matches!(local(&segment.name()), "LineStringSegment" | "LineString") {
                        return Err(format!(
                            "a GML curve segment that is not read ({})",
                            local(&segment.name())
                        ));
                    }
                    let positions = self.positions(&segment)?;
                    let skip = usize::from(out.last().is_some() && out.last() == positions.first());
                    out.extend(positions.into_iter().skip(skip));
                }
                Ok(out)
            }
            "CompositeCurve" => {
                let mut out = Vec::new();
                for member in children_named(node, &["curveMember"]) {
                    let curve =
                        first_geometry(&member).ok_or_else(|| "a GML curve member with no curve".to_string())?;
                    out.extend(self.line(&curve)?);
                }
                Ok(out)
            }
            "LineString" => self.positions(node),
            other => Err(format!("a GML line that is not read ({other})")),
        }
    }

    /// A Polygon, or a Surface's polygon patches as one polygon each.
    fn polygons(&self, node: &MNode) -> Result<Vec<Vec<Vec<Position>>>, String> {
        match local(&node.name()) {
            "Polygon" => Ok(vec![self.polygon(node)?]),
            "Surface" => {
                let patches =
                    child_named(node, &["patches"]).ok_or_else(|| "a GML surface with no patches".to_string())?;
                patches.children().iter().map(|patch| self.polygon(patch)).collect()
            }
            other => Err(format!("a GML surface that is not read ({other})")),
        }
    }

    /// The geometries of a multi geometry's members (`pointMember`, `curveMember`, `surfaceMembers`, ...).
    fn members(node: &MNode) -> Vec<MNode> {
        let mut out = Vec::new();
        for member in node.children() {
            let full = member.name();
            let name = local(&full);
            if name.ends_with("Members") {
                out.extend(member.children().into_iter().filter(is_geometry));
            } else if name.ends_with("Member") {
                out.extend(member.children().into_iter().find(is_geometry));
            }
        }
        out
    }

    fn geometry(&self, node: &MNode, depth: usize) -> Result<Raw, String> {
        if depth > 4 {
            return Err("GML geometries nested deeper than four".to_string());
        }
        Ok(match local(&node.name()) {
            "Point" => {
                let positions = self.positions(node)?;
                Raw::Point(
                    *positions
                        .first()
                        .ok_or_else(|| "a GML point with no position".to_string())?,
                )
            }
            "LineString" | "Curve" | "CompositeCurve" => Raw::Line(self.line(node)?),
            "LinearRing" => Raw::Polygon(vec![self.positions(node)?]),
            "Polygon" | "Surface" => {
                let mut polygons = self.polygons(node)?;
                if polygons.len() == 1 {
                    Raw::Polygon(polygons.remove(0))
                } else {
                    Raw::MultiPolygon(polygons)
                }
            }
            "MultiPoint" => {
                let mut points = Vec::new();
                for member in Self::members(node) {
                    points.extend(self.member(&member)?.positions(&member)?.first().copied());
                }
                Raw::MultiPoint(points)
            }
            "MultiLineString" | "MultiCurve" => Raw::MultiLine(
                Self::members(node)
                    .iter()
                    .map(|member| self.member(member)?.line(member))
                    .collect::<Result<_, _>>()?,
            ),
            "MultiPolygon" | "MultiSurface" => {
                let mut polygons = Vec::new();
                for member in Self::members(node) {
                    polygons.extend(self.member(&member)?.polygons(&member)?);
                }
                Raw::MultiPolygon(polygons)
            }
            "MultiGeometry" => Raw::Collection(
                Self::members(node)
                    .iter()
                    .map(|member| self.member(member)?.geometry(member, depth + 1))
                    .collect::<Result<_, _>>()?,
            ),
            other => return Err(format!("a GML geometry that is not read ({other})")),
        })
    }
}

impl Reader {
    /// A GML document: its feature members, or the one geometry it is.
    pub(super) fn gml(&mut self, text: &str) -> Result<(), OverlayError> {
        let root = MNode::parse_text("overlay", text).map_err(|err| {
            OverlayError::new(
                "NOT_PARSED",
                format!("the file is not XML a map reads: {}", err.message()),
            )
        })?;
        if is_geometry(&root) {
            self.number += 1;
            return self.gml_geometry(&root, "", "");
        }
        let mut features = Vec::new();
        for child in root.children() {
            if matches!(local(&child.name()), "featureMember" | "featureMembers" | "member") {
                features.extend(child.children());
            }
        }
        if features.is_empty() {
            return Err(OverlayError::new(
                "NOT_FORMAT",
                "the file is not GML a map reads (no gml:featureMember, gml:featureMembers or wfs:member, and its root is no geometry)",
            ));
        }
        for feature in features {
            self.number += 1;
            let mut properties = Map::new();
            let mut others = 0;
            for property in feature.children() {
                let name = local(&property.name()).to_string();
                if property.children().is_empty() {
                    if let Some(text) = property.text().filter(|text| !text.trim().is_empty()) {
                        properties.insert(name, Value::String(text));
                    }
                } else if first_geometry(&property).is_none() && name != "boundedBy" {
                    others += 1;
                }
            }
            if others > 0 {
                self.count("{count} features had other properties, which are not shown");
            }
            let (label, description) = self.pick_properties(Some(&properties));
            self.given_id = feature
                .attribute("gml:id")
                .or_else(|| feature.attribute("fid"))
                .or_else(|| feature.attribute("id"));
            match first_geometry(&feature) {
                Some(geometry) => self.gml_geometry(&geometry, &label, &description)?,
                None => self.reject("it has no geometry a map draws"),
            }
        }
        Ok(())
    }

    fn gml_geometry(&mut self, node: &MNode, label: &str, description: &str) -> Result<(), OverlayError> {
        let crs = match srs_name(node) {
            Some(name) => Crs::of_name(&name).ok_or_else(|| crs_unsupported(&name))?,
            None => {
                self.count("geometries with no srsName were read as WGS84 longitude, latitude");
                Crs::Wgs84
            }
        };
        self.use_crs(crs)?;
        let reader = Coordinates {
            crs,
            north_first: self.north_first(crs),
        };
        self.add_raw(reader.geometry(node, 0), label, description)
    }
}
