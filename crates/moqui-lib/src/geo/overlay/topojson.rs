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

//! A TopoJSON file as an overlay (plans/archive/Maps.md §4.1; the TopoJSON 1.0 specification): its objects' geometries, each a
//! feature (a GeometryCollection's members each one), their lines and rings stitched from the shared arcs, quantized
//! positions read through the topology's transform. Features may share an arc any number of times, so the positions the
//! stitching makes are bounded in all ([`super::OverlayLimits::max_bytes`] / 4), and a file that would make more is
//! refused whole (TOO_LARGE).

use serde_json::{Map, Value};

use super::{OverlayError, Position, Raw, Reader};

/// Why a geometry's arcs did not make a line: a reason the feature is rejected for, or the file's budget spent.
enum Stitch {
    Bad(String),
    Spent,
}

impl From<String> for Stitch {
    fn from(reason: String) -> Self {
        Self::Bad(reason)
    }
}

/// The topology's arcs, decoded, and what stitching may still make.
struct Topology {
    /// each arc's positions in the file's coordinates, or why it cannot be read
    arcs: Vec<Result<Vec<Position>, String>>,
    transform: Option<([f64; 2], [f64; 2])>,
    budget: usize,
}

impl Topology {
    fn point(&self, value: &Value) -> Result<Position, String> {
        let pair = number_pair(value).ok_or_else(|| "a position that is not two numbers".to_string())?;
        Ok(match self.transform {
            Some(([sx, sy], [tx, ty])) => [pair[0] * sx + tx, pair[1] * sy + ty],
            None => pair,
        })
    }

    fn points(&self, value: &Value) -> Result<Vec<Position>, String> {
        let items = value
            .as_array()
            .ok_or_else(|| "coordinates that are not a list".to_string())?;
        items.iter().map(|item| self.point(item)).collect()
    }

    /// A line or ring of arc indexes (a negative one, `!index`, reversed), each arc after the first without its first
    /// position, which is the last of the one before.
    fn stitch(&mut self, indexes: &Value) -> Result<Vec<Position>, Stitch> {
        let Some(indexes) = indexes.as_array() else {
            return Err(Stitch::Bad("arcs that are not a list".to_string()));
        };
        let mut out: Vec<Position> = Vec::new();
        for index in indexes {
            let Some(index) = index.as_i64() else {
                return Err(Stitch::Bad("an arc index that is not a whole number".to_string()));
            };
            let (at, reversed) = if index < 0 { (!index, true) } else { (index, false) };
            let arc = usize::try_from(at)
                .ok()
                .and_then(|at| self.arcs.get(at))
                .ok_or_else(|| format!("an arc index past the topology's arcs ({index})"))?;
            let arc = arc.as_ref().map_err(|reason| Stitch::Bad(reason.clone()))?;
            if arc.len() > self.budget {
                return Err(Stitch::Spent);
            }
            self.budget -= arc.len();
            let skip = usize::from(!out.is_empty());
            if reversed {
                out.extend(arc.iter().rev().skip(skip));
            } else {
                out.extend(arc.iter().skip(skip));
            }
        }
        Ok(out)
    }

    fn rings(&mut self, value: &Value) -> Result<Vec<Vec<Position>>, Stitch> {
        let Some(rings) = value.as_array() else {
            return Err(Stitch::Bad("polygon arcs that are not a list of rings".to_string()));
        };
        rings.iter().map(|ring| self.stitch(ring)).collect()
    }

    fn geometry(&mut self, object: &Map<String, Value>) -> Result<Raw, Stitch> {
        let arcs = object.get("arcs").unwrap_or(&Value::Null);
        let coordinates = object.get("coordinates").unwrap_or(&Value::Null);
        Ok(match object.get("type").and_then(Value::as_str) {
            Some("Point") => Raw::Point(self.point(coordinates)?),
            Some("MultiPoint") => Raw::MultiPoint(self.points(coordinates)?),
            Some("LineString") => Raw::Line(self.stitch(arcs)?),
            Some("MultiLineString") => {
                let lines = arcs
                    .as_array()
                    .ok_or_else(|| "arcs that are not a list of lines".to_string())?;
                Raw::MultiLine(lines.iter().map(|line| self.stitch(line)).collect::<Result<_, _>>()?)
            }
            Some("Polygon") => Raw::Polygon(self.rings(arcs)?),
            Some("MultiPolygon") => {
                let polygons = arcs
                    .as_array()
                    .ok_or_else(|| "arcs that are not a list of polygons".to_string())?;
                Raw::MultiPolygon(
                    polygons
                        .iter()
                        .map(|polygon| self.rings(polygon))
                        .collect::<Result<_, _>>()?,
                )
            }
            Some(other) => {
                return Err(Stitch::Bad(format!(
                    "a geometry type that is not TopoJSON's ({})",
                    super::plain_text(other, 40, false)
                )))
            }
            None => return Err(Stitch::Bad("a geometry with no type".to_string())),
        })
    }
}

fn number_pair(value: &Value) -> Option<Position> {
    let items = value.as_array().filter(|items| items.len() >= 2)?;
    Some([items[0].as_f64()?, items[1].as_f64()?])
}

fn spent(max_bytes: usize) -> OverlayError {
    OverlayError::new(
        "TOO_LARGE",
        format!(
            "the topology's shared arcs make more than the {} positions an overlay may have",
            max_bytes / 4
        ),
    )
}

impl Reader {
    /// A TopoJSON Topology: every object, in the order the file names them.
    pub(super) fn topojson(&mut self, document: &Value, max_bytes: usize) -> Result<(), OverlayError> {
        let Some(topology) = document
            .as_object()
            .filter(|object| object.get("type").and_then(Value::as_str) == Some("Topology"))
        else {
            return Err(OverlayError::new("NOT_FORMAT", "the file is not a TopoJSON Topology"));
        };
        let Some(objects) = topology
            .get("objects")
            .and_then(Value::as_object)
            .filter(|objects| !objects.is_empty())
        else {
            return Err(OverlayError::new("NOT_FORMAT", "the topology has no objects"));
        };
        self.declared_crs(topology.get("crs"))?;
        let transform = topology.get("transform").and_then(|transform| {
            Some((
                number_pair(transform.get("scale")?)?,
                number_pair(transform.get("translate")?)?,
            ))
        });
        if topology.get("transform").is_some_and(|value| !value.is_null()) && transform.is_none() {
            return Err(OverlayError::new(
                "NOT_FORMAT",
                "the topology's transform is not a scale and a translate",
            ));
        }
        let mut decoded = Topology {
            arcs: Vec::new(),
            transform,
            budget: max_bytes / 4,
        };
        for arc in topology
            .get("arcs")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            decoded.arcs.push(decode_arc(arc, transform));
        }
        for object in objects.values() {
            if object.get("type").and_then(Value::as_str) == Some("GeometryCollection") {
                let members = object
                    .get("geometries")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                for member in members {
                    self.topojson_feature(&mut decoded, member, max_bytes)?;
                }
            } else {
                self.topojson_feature(&mut decoded, object, max_bytes)?;
            }
        }
        Ok(())
    }

    fn topojson_feature(
        &mut self,
        topology: &mut Topology,
        member: &Value,
        max_bytes: usize,
    ) -> Result<(), OverlayError> {
        self.number += 1;
        let Some(object) = member.as_object() else {
            self.reject("a member that is not a geometry");
            return Ok(());
        };
        if object.get("type").is_none_or(Value::is_null) {
            // a null geometry: a feature with nothing to draw
            self.reject("it has no geometry");
            return Ok(());
        }
        let (label, description) = self.pick_properties(object.get("properties").and_then(Value::as_object));
        let raw = match topology.geometry(object) {
            Ok(raw) => Ok(raw),
            Err(Stitch::Spent) => return Err(spent(max_bytes)),
            Err(Stitch::Bad(reason)) => Err(reason),
        };
        self.given_id = match object.get("id") {
            Some(Value::String(id)) => Some(id.clone()),
            Some(Value::Number(id)) => Some(id.to_string()),
            _ => None,
        };
        self.add_raw(raw, &label, &description)
    }
}

/// An arc's positions: delta-encoded quantized integers through the transform when the topology has one, else as they are.
fn decode_arc(arc: &Value, transform: Option<([f64; 2], [f64; 2])>) -> Result<Vec<Position>, String> {
    let items = arc
        .as_array()
        .ok_or_else(|| "an arc that is not a list of positions".to_string())?;
    let mut out = Vec::with_capacity(items.len());
    let (mut x, mut y) = (0.0, 0.0);
    for item in items {
        let [dx, dy] = number_pair(item).ok_or_else(|| "an arc position that is not two numbers".to_string())?;
        out.push(match transform {
            Some(([sx, sy], [tx, ty])) => {
                x += dx;
                y += dy;
                [x * sx + tx, y * sy + ty]
            }
            None => [dx, dy],
        });
    }
    Ok(out)
}
