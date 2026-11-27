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

//! An OGC service's GetCapabilities read for a map (plans/archive/Maps.md §4.1, P4a; the framework's `MapOgc.js` records what
//! this answers): WMS 1.3.0, WMTS 1.0.0 and WFS 2.0.0. The document is read by [`crate::node`], which refuses a DOCTYPE,
//! an entity declaration and an entity it does not know, so no DTD or external entity is ever read. What is kept is what
//! a map and the Configuration's Add OGC layer need, each text cut short: the service's version, its image (or GeoJSON)
//! formats and its operation's GET address, at most `max_layers` layers (a WMS's named layers with what they inherit, a
//! WMTS's layers linked to a matrix set in a CRS a map draws, a WFS's feature types), each with its title, styles,
//! formats, the CRSs of it a map draws, its WGS84 bounds and time dimension, and a WMTS layer's tile template and matrix
//! sets (each kept once, at most [`MAX_MATRICES`] matrices). A WMS layer's CRS and style lists are written once in
//! `crsLists` and `styleLists` and named by index, since children inherit them; an answer over [`MAX_JSON`] is refused. The
//! answer is JSON text, as the framework keeps it.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

use crate::error::Error;
use crate::node::MNode;

/// The matrices a WMTS matrix set keeps: zooms 0 to 29
pub const MAX_MATRICES: usize = 30;
const MAX_STYLES: usize = 30;
const MAX_CRS: usize = 12;
const MAX_DEPTH: usize = 12;
const MAX_HREF: usize = 1000;
/// The most JSON a provider keeps of its capabilities
pub const MAX_JSON: usize = 2 * 1024 * 1024;

/// The service a capabilities document is read as
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OgcService {
    Wms,
    Wmts,
    Wfs,
}

impl OgcService {
    /// `wms`, `wmts` or `wfs`
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "wms" => Some(Self::Wms),
            "wmts" => Some(Self::Wmts),
            "wfs" => Some(Self::Wfs),
            _ => None,
        }
    }
}

/// What a read finds: the capabilities as JSON, or why the document is not ones a map reads (a short fixed text)
#[derive(Debug)]
pub enum Capabilities {
    Read(Value),
    Refused(&'static str),
}

const WEB_MERCATOR: [&str; 6] = [
    "EPSG:3857",
    "EPSG:102100",
    "EPSG:102113",
    "EPSG:900913",
    "http://www.opengis.net/def/crs/EPSG/0/3857",
    "http://www.opengis.net/gml/srs/epsg.xml#3857",
];
const GEOGRAPHIC: [&str; 7] = [
    "CRS:84",
    "EPSG:4326",
    "urn:ogc:def:crs:OGC:1.3:CRS84",
    "urn:ogc:def:crs:OGC:2:84",
    "http://www.opengis.net/def/crs/OGC/1.3/CRS84",
    "http://www.opengis.net/gml/srs/epsg.xml#4326",
    "http://www.opengis.net/def/crs/EPSG/0/4326",
];

/// A `urn:ogc:def:crs:EPSG:[version]:code` (or `urn:x-ogc:...`) read as `EPSG:code`, as OpenLayers reads it (case as
/// written: OpenLayers' own match is case-sensitive)
fn ol_code(code: &str) -> String {
    let rest = code
        .strip_prefix("urn:ogc:def:crs:EPSG:")
        .or_else(|| code.strip_prefix("urn:x-ogc:def:crs:EPSG:"));
    if let Some(rest) = rest {
        if let Some((version, number)) = rest.rsplit_once(':') {
            let version_ok = version.chars().all(|ch| ch.is_ascii_digit() || ch == '.');
            if version_ok && !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit()) {
                return format!("EPSG:{number}");
            }
        }
    }
    code.to_string()
}

/// Whether a map draws a CRS without a projection of its own: the codes OpenLayers knows for EPSG:3857 and EPSG:4326
pub fn drawn_crs(code: &str) -> bool {
    let code = code.trim();
    let known = |name: &str| WEB_MERCATOR.contains(&name) || GEOGRAPHIC.contains(&name);
    known(code) || known(&ol_code(code))
}

/// An image format a map draws and the proxy passes (never SVG, TIFF or a document)
fn image_format(format: &str) -> bool {
    let lower = format.trim().to_ascii_lowercase();
    ["png", "jpeg", "jpg", "jpgpng", "webp", "gif", "avif"]
        .iter()
        .any(|kind| {
            lower
                .strip_prefix("image/")
                .and_then(|rest| rest.strip_prefix(kind))
                .is_some_and(|rest| rest.is_empty() || !rest.starts_with(|ch: char| ch.is_ascii_alphanumeric()))
        })
}

fn local(node: &MNode) -> String {
    node.with_name(|name| name.rsplit(':').next().unwrap_or(name).to_string())
}
fn kids(node: &MNode, name: &str) -> Vec<MNode> {
    node.children()
        .into_iter()
        .filter(|child| local(child) == name)
        .collect()
}
fn kid(node: &MNode, name: &str) -> Option<MNode> {
    node.child_iter().find(|child| local(child) == name)
}
fn path(node: &MNode, names: &[&str]) -> Option<MNode> {
    names.iter().try_fold(node.clone(), |at, name| kid(&at, name))
}
/// Text cut to `max` characters
fn cut(value: &str, max: usize) -> String {
    value.trim().chars().take(max).collect()
}
fn node_text(node: &MNode, max: usize) -> String {
    node.with_text(|text| cut(text.unwrap_or(""), max))
}
fn text_of(node: &MNode, name: &str, max: usize) -> String {
    kid(node, name).map(|found| node_text(&found, max)).unwrap_or_default()
}
/// An attribute by its local name (xlink:href is href)
fn attr(node: &MNode, name: &str) -> String {
    let mut found = String::new();
    node.for_each_attribute(|key, value| {
        if found.is_empty() && key.rsplit(':').next() == Some(name) {
            found = value.trim().to_string();
        }
    });
    found
}
/// The values `keep` holds, each once, in order, at most `cap` of them: linear in the input, which it stops reading at `cap`
fn unique_where(values: impl IntoIterator<Item = String>, keep: impl Fn(&str) -> bool, cap: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if out.len() >= cap {
            break;
        }
        if !value.is_empty() && keep(&value) && seen.insert(value.clone()) {
            out.push(value);
        }
    }
    out
}
fn numbers(value: &str) -> Vec<f64> {
    value
        .split_whitespace()
        .filter_map(|part| part.parse::<f64>().ok())
        .filter(|n| n.is_finite())
        .collect()
}
/// A WGS84 bounding box as [west, south, east, north]
fn bounds_of(node: Option<MNode>) -> Value {
    let Some(node) = node else { return Value::Null };
    if local(&node) == "EX_GeographicBoundingBox" {
        let found: Vec<f64> = [
            "westBoundLongitude",
            "southBoundLatitude",
            "eastBoundLongitude",
            "northBoundLatitude",
        ]
        .iter()
        .filter_map(|name| text_of(&node, name, 40).parse::<f64>().ok())
        .collect();
        return if found.len() == 4 { json!(found) } else { Value::Null };
    }
    let lower = numbers(&text_of(&node, "LowerCorner", 80));
    let upper = numbers(&text_of(&node, "UpperCorner", 80));
    if lower.len() == 2 && upper.len() == 2 {
        json!([lower[0], lower[1], upper[0], upper[1]])
    } else {
        Value::Null
    }
}
fn time(default: String, values: String) -> Value {
    json!({"default": cut(&default, 64), "values": cut(&values, 120)})
}

/// An OWS operation's GET address, its GetEncoding values (WMTS) and its parameters' allowed values (WFS outputFormat)
struct Operation {
    href: String,
    encodings: Vec<String>,
    output_formats: Vec<String>,
}
fn ows_operation(root: &MNode, name: &str) -> Option<Operation> {
    let operation = kid(root, "OperationsMetadata")?
        .child_iter()
        .find(|each| local(each) == "Operation" && attr(each, "name") == name)?;
    let gets = path(&operation, &["DCP", "HTTP"])
        .map(|http| kids(&http, "Get"))
        .unwrap_or_default();
    let values = |node: &MNode| -> Vec<String> {
        kid(node, "AllowedValues")
            .map(|allowed| {
                kids(&allowed, "Value")
                    .iter()
                    .take(30)
                    .map(|value| node_text(value, 128))
                    .collect()
            })
            .unwrap_or_default()
    };
    let encodings = gets
        .iter()
        .flat_map(|get| {
            kids(get, "Constraint")
                .into_iter()
                .filter(|each| attr(each, "name") == "GetEncoding")
                .flat_map(|each| values(&each))
        })
        .collect();
    let output_formats = kids(&operation, "Parameter")
        .iter()
        .filter(|each| attr(each, "name").eq_ignore_ascii_case("outputFormat"))
        .flat_map(values)
        .collect();
    Some(Operation {
        href: gets
            .first()
            .map(|get| cut(&attr(get, "href"), MAX_HREF))
            .unwrap_or_default(),
        encodings,
        output_formats,
    })
}

/// Lists many layers share (a WMS layer's inherited CRSs and styles), each kept once and named by its index
#[derive(Default)]
struct Shared {
    lists: Vec<Value>,
    index: HashMap<String, usize>,
}

impl Shared {
    fn id(&mut self, list: &[impl serde::Serialize]) -> usize {
        let value = json!(list);
        let key = value.to_string();
        if let Some(found) = self.index.get(&key) {
            return *found;
        }
        self.lists.push(value);
        self.index.insert(key, self.lists.len() - 1);
        self.lists.len() - 1
    }
}

struct Inherited {
    crs: Vec<String>,
    styles: Vec<Value>,
    bounds: Value,
    time: Value,
}

fn read_wms(root: &MNode, max_layers: usize) -> Capabilities {
    if local(root) != "WMS_Capabilities" || attr(root, "version") != "1.3.0" {
        return Capabilities::Refused("an answer that is not WMS 1.3.0 capabilities");
    }
    let capability = kid(root, "Capability");
    let get_map = capability.as_ref().and_then(|node| path(node, &["Request", "GetMap"]));
    let formats: Vec<String> = get_map
        .as_ref()
        .map(|node| {
            unique_where(
                kids(node, "Format").iter().map(|each| node_text(each, 128)),
                image_format,
                20,
            )
        })
        .unwrap_or_default();
    let href = get_map
        .as_ref()
        .and_then(|node| path(node, &["DCPType", "HTTP", "Get", "OnlineResource"]))
        .map(|node| cut(&attr(&node, "href"), MAX_HREF))
        .unwrap_or_default();
    struct Walk {
        layers: Vec<Value>,
        crs_lists: Shared,
        style_lists: Shared,
        max_layers: usize,
    }
    fn walk(node: &MNode, inherited: &Inherited, depth: usize, out: &mut Walk) {
        if depth > MAX_DEPTH || out.layers.len() >= out.max_layers {
            return;
        }
        let mut crs = inherited.crs.clone();
        for each in kids(node, "CRS") {
            let code = node_text(&each, 128);
            if drawn_crs(&code) && !crs.contains(&code) && crs.len() < MAX_CRS {
                crs.push(code);
            }
        }
        let mut styles = inherited.styles.clone();
        for style in kids(node, "Style").iter().take(MAX_STYLES) {
            let name = text_of(style, "Name", 128);
            if !name.is_empty() && styles.len() < MAX_STYLES {
                styles.push(json!({"name": name, "title": text_of(style, "Title", 128)}));
            }
        }
        let bounds = match bounds_of(kid(node, "EX_GeographicBoundingBox")) {
            Value::Null => inherited.bounds.clone(),
            found => found,
        };
        let time = kids(node, "Dimension")
            .into_iter()
            .find(|each| attr(each, "name").eq_ignore_ascii_case("time"))
            .map(|each| time(attr(&each, "default"), node_text(&each, 400)))
            .unwrap_or_else(|| inherited.time.clone());
        let name = text_of(node, "Name", 255);
        if !name.is_empty() {
            let title = text_of(node, "Title", 255);
            let (styles_id, crs_id) = (out.style_lists.id(&styles), out.crs_lists.id(&crs));
            out.layers.push(json!({
                "name": name,
                "title": if title.is_empty() { name.clone() } else { title },
                "styles": styles_id,
                "crs": crs_id,
                "bounds": bounds,
                "time": time,
                "opaque": attr(node, "opaque") == "1",
            }));
        }
        let next = Inherited {
            crs,
            styles,
            bounds,
            time,
        };
        for child in kids(node, "Layer") {
            walk(&child, &next, depth + 1, out);
        }
    }
    let top = Inherited {
        crs: Vec::new(),
        styles: Vec::new(),
        bounds: Value::Null,
        time: Value::Null,
    };
    let mut out = Walk {
        layers: Vec::new(),
        crs_lists: Shared::default(),
        style_lists: Shared::default(),
        max_layers,
    };
    for layer in capability.as_ref().map(|node| kids(node, "Layer")).unwrap_or_default() {
        walk(&layer, &top, 0, &mut out);
    }
    // a layer's crs and styles name a list of crsLists and styleLists: the lists children inherit are kept once
    Capabilities::Read(
        json!({"version": "1.3.0", "formats": formats, "href": href, "kvp": true, "layers": out.layers,
        "crsLists": out.crs_lists.lists, "styleLists": out.style_lists.lists, "matrixSets": {}}),
    )
}

fn read_wmts(root: &MNode, max_layers: usize) -> Capabilities {
    if local(root) != "Capabilities" || attr(root, "version") != "1.0.0" {
        return Capabilities::Refused("an answer that is not WMTS 1.0.0 capabilities");
    }
    let Some(contents) = kid(root, "Contents") else {
        return Capabilities::Read(
            json!({"version": "1.0.0", "formats": [], "href": "", "kvp": false, "layers": [], "matrixSets": {}}),
        );
    };
    let mut sets = Map::new();
    for set in kids(&contents, "TileMatrixSet").iter().take(200) {
        let id = text_of(set, "Identifier", 128);
        let crs = text_of(set, "SupportedCRS", 128);
        if id.is_empty() || !drawn_crs(&crs) {
            continue;
        }
        let mut matrices: Vec<Value> = Vec::new();
        let mut valid = true;
        for matrix in kids(set, "TileMatrix").iter().take(MAX_MATRICES) {
            let whole = |name: &str| text_of(matrix, name, 20).parse::<u64>().ok().filter(|n| *n > 0);
            let identifier = text_of(matrix, "Identifier", 64);
            let scale = text_of(matrix, "ScaleDenominator", 40)
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && *n > 0.0);
            let corner = numbers(&text_of(matrix, "TopLeftCorner", 80));
            match (
                scale,
                whole("TileWidth"),
                whole("TileHeight"),
                whole("MatrixWidth"),
                whole("MatrixHeight"),
            ) {
                (Some(scale), Some(tw), Some(th), Some(mw), Some(mh))
                    if !identifier.is_empty() && corner.len() == 2 =>
                {
                    matrices.push(
                        json!({"Identifier": identifier, "ScaleDenominator": scale, "TopLeftCorner": corner,
                        "TileWidth": tw, "TileHeight": th, "MatrixWidth": mw, "MatrixHeight": mh}),
                    );
                }
                _ => valid = false,
            }
        }
        if valid && !matrices.is_empty() {
            sets.insert(
                id.clone(),
                json!({"Identifier": id, "SupportedCRS": crs, "TileMatrix": matrices}),
            );
        }
    }
    let get_tile = ows_operation(root, "GetTile");
    let mut layers: Vec<Value> = Vec::new();
    for layer in kids(&contents, "Layer") {
        if layers.len() >= max_layers {
            break;
        }
        let name = text_of(&layer, "Identifier", 255);
        let linked: Vec<String> = unique_where(
            kids(&layer, "TileMatrixSetLink")
                .iter()
                .map(|link| text_of(link, "TileMatrixSet", 128)),
            |id| sets.contains_key(id),
            20,
        );
        if name.is_empty() || linked.is_empty() {
            continue;
        }
        let resource = kids(&layer, "ResourceURL")
            .into_iter()
            .find(|each| attr(each, "resourceType") == "tile");
        let time = kids(&layer, "Dimension")
            .into_iter()
            .find(|each| text_of(each, "Identifier", 64).eq_ignore_ascii_case("time"))
            .map(|each| {
                time(
                    text_of(&each, "Default", 64),
                    kids(&each, "Value")
                        .iter()
                        .map(|value| node_text(value, 64))
                        .collect::<Vec<_>>()
                        .join(","),
                )
            })
            .unwrap_or(Value::Null);
        let crs = unique_where(
            linked
                .iter()
                .map(|id| sets[id]["SupportedCRS"].as_str().unwrap_or("").to_string()),
            |_| true,
            MAX_CRS,
        );
        let title = text_of(&layer, "Title", 255);
        layers.push(json!({
            "name": name,
            "title": if title.is_empty() { name.clone() } else { title },
            "styles": kids(&layer, "Style").iter().take(MAX_STYLES).filter_map(|style| {
                let name = text_of(style, "Identifier", 128);
                (!name.is_empty()).then(|| json!({"name": name, "title": text_of(style, "Title", 128), "isDefault": attr(style, "isDefault") == "true"}))
            }).collect::<Vec<_>>(),
            "formats": unique_where(kids(&layer, "Format").iter().map(|each| node_text(each, 64)), image_format, 10),
            "matrixSets": linked,
            "crs": crs,
            "template": resource.as_ref().map(|each| cut(&attr(each, "template"), MAX_HREF)).unwrap_or_default(),
            "templateFormat": resource.as_ref().map(|each| cut(&attr(each, "format"), 64)).unwrap_or_default(),
            "bounds": bounds_of(kid(&layer, "WGS84BoundingBox")),
            "time": time,
        }));
    }
    // only the sets a recorded layer names
    let used: Vec<String> = layers
        .iter()
        .flat_map(|layer| layer["matrixSets"].as_array().cloned().unwrap_or_default())
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect();
    sets.retain(|id, _| used.contains(id));
    let formats = unique_where(
        layers
            .iter()
            .flat_map(|layer| layer["formats"].as_array().cloned().unwrap_or_default())
            .filter_map(|f| f.as_str().map(str::to_string)),
        |_| true,
        20,
    );
    let kvp = get_tile.as_ref().is_some_and(|op| {
        op.encodings.is_empty()
            || op
                .encodings
                .iter()
                .any(|value| value.to_ascii_lowercase().contains("kvp"))
    });
    Capabilities::Read(json!({
        "version": "1.0.0",
        "formats": formats.into_iter().take(20).collect::<Vec<_>>(),
        "href": get_tile.map(|op| op.href).unwrap_or_default(),
        "kvp": kvp,
        "layers": layers,
        "matrixSets": sets,
    }))
}

fn read_wfs(root: &MNode, max_layers: usize) -> Capabilities {
    if local(root) != "WFS_Capabilities" || attr(root, "version") != "2.0.0" {
        return Capabilities::Refused("an answer that is not WFS 2.0.0 capabilities");
    }
    let get_feature = ows_operation(root, "GetFeature");
    let json_formats = |values: Vec<String>| -> Vec<String> {
        unique_where(values, |format| format.to_ascii_lowercase().contains("json"), 10)
    };
    let shared = json_formats(
        get_feature
            .as_ref()
            .map(|op| op.output_formats.clone())
            .unwrap_or_default(),
    );
    let mut layers: Vec<Value> = Vec::new();
    for feature_type in kid(root, "FeatureTypeList")
        .map(|list| kids(&list, "FeatureType"))
        .unwrap_or_default()
    {
        if layers.len() >= max_layers {
            break;
        }
        let name = text_of(&feature_type, "Name", 255);
        if name.is_empty() {
            continue;
        }
        let crs: Vec<String> = unique_where(
            std::iter::once(text_of(&feature_type, "DefaultCRS", 128))
                .chain(kids(&feature_type, "OtherCRS").iter().map(|each| node_text(each, 128))),
            drawn_crs,
            MAX_CRS,
        );
        let own = json_formats(
            kid(&feature_type, "OutputFormats")
                .map(|list| kids(&list, "Format").iter().map(|each| node_text(each, 128)).collect())
                .unwrap_or_default(),
        );
        let title = text_of(&feature_type, "Title", 255);
        layers.push(json!({
            "name": name,
            "title": if title.is_empty() { name.clone() } else { title },
            "crs": crs,
            "formats": if own.is_empty() { shared.clone() } else { own },
            "bounds": bounds_of(kid(&feature_type, "WGS84BoundingBox")),
        }));
    }
    Capabilities::Read(json!({
        "version": "2.0.0",
        "formats": shared,
        "href": get_feature.map(|op| op.href).unwrap_or_default(),
        "kvp": true,
        "layers": layers,
        "matrixSets": {},
    }))
}

/// A GetCapabilities document read as `service`: [`Capabilities::Refused`] for one of another service or version, or a
/// service exception; an [`Error`] for text that is not XML the node reader reads (a DOCTYPE, an entity declaration and an
/// unknown entity among it).
pub fn read_capabilities(service: OgcService, xml: &str, max_layers: usize) -> Result<Capabilities, Error> {
    let root = MNode::parse_text("capabilities", xml)?;
    if local(&root).ends_with("ExceptionReport") {
        return Ok(Capabilities::Refused("a service exception, not its capabilities"));
    }
    let read = match service {
        OgcService::Wms => read_wms(&root, max_layers),
        OgcService::Wmts => read_wmts(&root, max_layers),
        OgcService::Wfs => read_wfs(&root, max_layers),
    };
    Ok(match read {
        Capabilities::Read(found) if found.to_string().len() > MAX_JSON => {
            Capabilities::Refused("capabilities larger than the 2 MB a provider keeps of them")
        }
        other => other,
    })
}

#[cfg(test)]
#[path = "ogc_tests.rs"]
mod tests;
