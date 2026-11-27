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

//! The readers added after GeoJSON, GPX and KML (overlay/*.rs): each format's features, CRS and refusals, and the
//! malformed and oversized inputs each must refuse cheaply.

use std::io::Write;

use super::*;

const LIMITS: OverlayLimits = OverlayLimits {
    max_bytes: 200_000,
    max_features: 10,
};

fn read(bytes: &[u8], format: OverlayFormat) -> Overlay {
    convert_overlay_bytes(bytes, format, &LIMITS).expect("the file reads")
}

fn refused(bytes: &[u8], format: OverlayFormat) -> OverlayError {
    convert_overlay_bytes(bytes, format, &LIMITS).expect_err("the file is refused")
}

fn notes(overlay: &Overlay) -> String {
    overlay.notes.join("\n")
}

#[test]
fn formats_by_name_and_id() {
    for format in OverlayFormat::ALL {
        assert_eq!(OverlayFormat::parse(format.enum_id()), Some(format));
    }
    assert_eq!(OverlayFormat::of_file_name("roads.ZIP"), Some(OverlayFormat::Shapefile));
    assert_eq!(
        OverlayFormat::of_file_name("parcels.esri.json"),
        Some(OverlayFormat::EsriJson)
    );
    assert_eq!(OverlayFormat::of_file_name("a.json"), Some(OverlayFormat::GeoJson));
    assert_eq!(OverlayFormat::of_file_name("a.tsv"), Some(OverlayFormat::Csv));
    assert_eq!(OverlayFormat::of_file_name("layers.gpkg"), None);
    assert!(OverlayFormat::Wkb.is_binary() && !OverlayFormat::Gml.is_binary());
}

#[test]
fn topojson_arcs_stitched_quantized_and_shared() {
    // two squares sharing an edge (arc 1), quantized with a transform; a point and a feature of a bad arc index
    let text = r#"{"type":"Topology","transform":{"scale":[0.5,0.5],"translate":[10,20]},
        "arcs":[[[0,0],[0,2],[2,0]],[[2,2],[0,-2]],[[2,0],[-2,0],[0,2]],[[2,0],[2,0],[0,2],[-2,0]]],
        "objects":{"areas":{"type":"GeometryCollection","geometries":[
            {"type":"Polygon","arcs":[[0,1,2]],"id":"west","properties":{"name":"West <b>side</b>","pop":3}},
            {"type":"Polygon","arcs":[[-2,3]],"properties":{"name":"East"}},
            {"type":"LineString","arcs":[9],"properties":{"name":"Broken"}},
            {"type":"Point","coordinates":[4,4],"properties":{"title":"Well"}},
            {"type":null}]}}}"#;
    let overlay = read(text.as_bytes(), OverlayFormat::TopoJson);
    assert_eq!(overlay.features.len(), 3, "{:?}", overlay.notes);
    assert_eq!(overlay.rejected, 2);
    let west = &overlay.features[0];
    assert_eq!(west["id"], "west");
    assert_eq!(west["properties"]["label"], "West side");
    let ring = west["geometry"]["coordinates"][0].as_array().unwrap();
    assert_eq!(ring.first(), ring.last());
    assert_eq!(ring[0], json!([10.0, 20.0]));
    assert_eq!(ring[1], json!([10.0, 21.0]));
    assert_eq!(
        overlay.features[2]["geometry"],
        json!({"type": "Point", "coordinates": [12.0, 22.0]})
    );
    let notes = notes(&overlay);
    assert!(notes.contains("feature 3 left out: an arc index past"), "{notes}");
    assert!(notes.contains("other properties"), "{notes}");
    // read as GeoJSON too (a .json file)
    assert_eq!(read(text.as_bytes(), OverlayFormat::GeoJson).features.len(), 3);
}

#[test]
fn topojson_shared_arcs_bounded() {
    // one arc of 1,000 positions referenced 1,000 times by one line: a million positions from a small file
    let arc: Vec<String> = (0..1000).map(|_| "[0,0]".to_string()).collect();
    let indexes: Vec<String> = (0..1000).map(|_| "0".to_string()).collect();
    let text = format!(
        r#"{{"type":"Topology","arcs":[[{}]],"objects":{{"a":{{"type":"LineString","arcs":[{}]}}}}}}"#,
        arc.join(","),
        indexes.join(",")
    );
    let limits = OverlayLimits {
        max_bytes: 400_000,
        max_features: 10,
    };
    let error = convert_overlay(&text, OverlayFormat::TopoJson, &limits).expect_err("too many positions");
    assert_eq!(error.code, "TOO_LARGE");
    assert_eq!(
        refused(br#"{"type":"Topology","objects":{}}"#, OverlayFormat::TopoJson).code,
        "NOT_FORMAT"
    );
    assert_eq!(
        refused(br#"{"type":"FeatureCollection"}"#, OverlayFormat::TopoJson).code,
        "NOT_FORMAT"
    );
}

#[test]
fn esrijson_rings_wound_and_spatial_reference() {
    // an outer ring clockwise with a hole counterclockwise, a second outer ring, a polyline of two paths, a point in
    // Web Mercator (its own spatialReference), curves refused
    let text = r#"{"geometryType":"esriGeometryPolygon","spatialReference":{"wkid":4326},"features":[
        {"attributes":{"NAME":"Park","Description":"Open","OBJECTID":1},
         "geometry":{"rings":[[[0,0],[0,10],[10,10],[10,0],[0,0]],[[2,2],[4,2],[4,4],[2,4],[2,2]],[[20,0],[20,5],[25,5],[20,0]]]}},
        {"attributes":{"name":"Paths"},"geometry":{"paths":[[[0,0],[1,1]],[[2,2],[3,3]]]}},
        {"attributes":{"name":"Origin"},"geometry":{"x":0,"y":0,"spatialReference":{"wkid":102100}}},
        {"attributes":{"name":"Arc"},"geometry":{"curvePaths":[[[0,0],{"c":[[1,1],[0.5,0.5]]}]]}},
        {"attributes":{"name":"Far"},"geometry":{"x":0,"y":95}}]}"#;
    let overlay = read(text.as_bytes(), OverlayFormat::EsriJson);
    assert_eq!(overlay.features.len(), 3);
    assert_eq!(overlay.rejected, 2);
    let park = &overlay.features[0];
    assert_eq!(park["properties"], json!({"label": "Park", "description": "Open"}));
    assert_eq!(park["geometry"]["type"], "MultiPolygon");
    assert_eq!(
        park["geometry"]["coordinates"][0].as_array().unwrap().len(),
        2,
        "the hole is the first polygon's"
    );
    assert_eq!(overlay.features[1]["geometry"]["type"], "MultiLineString");
    assert_eq!(overlay.crs, "EPSG:4326");
    let notes = notes(&overlay);
    assert!(notes.contains("another CRS than the file's first"), "{notes}");
    assert!(notes.contains("feature 4 left out: a geometry of curves"), "{notes}");
    // detected in a .json file; another WKID refused
    assert_eq!(read(text.as_bytes(), OverlayFormat::GeoJson).features.len(), 3);
    let other = br#"{"spatialReference":{"wkid":2154},"features":[{"attributes":{},"geometry":{"x":1,"y":2}}]}"#;
    assert_eq!(refused(other, OverlayFormat::EsriJson).code, "CRS_UNSUPPORTED");
    let none = br#"{"features":[{"attributes":{},"geometry":{"x":1,"y":2}}]}"#;
    assert!(notes_of(none, OverlayFormat::EsriJson).contains("no spatial reference"));
}

fn notes_of(bytes: &[u8], format: OverlayFormat) -> String {
    notes(&read(bytes, format))
}

#[test]
fn wkt_lines_ewkt_and_refusals() {
    let text = "# parks\nPOINT (-71.06 42.36)\nLINESTRING Z (0 0 1, 1 1 2)\nPOLYGON ((0 0, 1 0, 1 1, 0 0), (0.2 0.2, 0.3 0.2, 0.3 0.3, 0.2 0.2))\n\
        MULTIPOINT ((1 2), (3 4))\nMULTIPOINT (5 6, 7 8)\nGEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 2 2))\n\
        POINT EMPTY\nTRIANGLE ((0 0, 1 0, 0 1, 0 0))\nPOINT (1 2) junk\n";
    let overlay = read(text.as_bytes(), OverlayFormat::Wkt);
    // the collection is two features
    assert_eq!(overlay.features.len(), 7, "{:?}", overlay.notes);
    assert_eq!(overlay.rejected, 3);
    assert_eq!(
        overlay.features[1]["geometry"]["coordinates"],
        json!([[0.0, 0.0], [1.0, 1.0]])
    );
    assert_eq!(
        overlay.features[2]["geometry"]["coordinates"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        overlay.features[4]["geometry"]["coordinates"],
        json!([[5.0, 6.0], [7.0, 8.0]])
    );
    let notes = notes(&overlay);
    assert!(notes.contains("left out: an empty geometry"), "{notes}");
    assert!(notes.contains("a WKT type that is not read (TRIANGLE)"), "{notes}");
    assert!(notes.contains("WKT with more after its geometry"), "{notes}");
    let mercator = read(b"SRID=3857;POINT(20037508.342789244 0)", OverlayFormat::Wkt);
    assert_eq!(mercator.crs, "EPSG:3857");
    assert!((mercator.features[0]["geometry"]["coordinates"][0].as_f64().unwrap() - 180.0).abs() < 1e-9);
    assert_eq!(
        refused(b"SRID=27700;POINT(530000 180000)", OverlayFormat::Wkt).code,
        "CRS_UNSUPPORTED"
    );
    assert_eq!(refused(b"\n# nothing\n", OverlayFormat::Wkt).code, "NOT_FORMAT");
    // nesting past four collections is refused, not recursed into
    let deep = format!("{}POINT (1 1){}", "GEOMETRYCOLLECTION (".repeat(10), ")".repeat(10));
    assert!(refused(deep.as_bytes(), OverlayFormat::Wkt)
        .message
        .contains("nested deeper"));
}

/// A little-endian WKB point, with an EWKB SRID when given.
fn wkb_point(x: f64, y: f64, srid: Option<u32>) -> Vec<u8> {
    let mut out = vec![1];
    let code: u32 = 1 | if srid.is_some() { 0x2000_0000 } else { 0 };
    out.extend(code.to_le_bytes());
    if let Some(srid) = srid {
        out.extend(srid.to_le_bytes());
    }
    out.extend(x.to_le_bytes());
    out.extend(y.to_le_bytes());
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

#[test]
fn wkb_binary_hex_and_claimed_counts() {
    // a big-endian ISO LineString Z, then a point, binary one after another
    let mut line = vec![0];
    line.extend(1002_u32.to_be_bytes());
    line.extend(2_u32.to_be_bytes());
    for value in [0.0_f64, 0.0, 5.0, 1.0, 1.0, 6.0] {
        line.extend(value.to_be_bytes());
    }
    let mut binary = line.clone();
    binary.extend(wkb_point(-71.06, 42.36, Some(4326)));
    let overlay = read(&binary, OverlayFormat::Wkb);
    assert_eq!(overlay.features.len(), 2);
    assert_eq!(
        overlay.features[0]["geometry"]["coordinates"],
        json!([[0.0, 0.0], [1.0, 1.0]])
    );
    // hex text, one a line; an SRID not read refuses the file
    let text = format!("{}\n{}\n", hex(&wkb_point(1.0, 2.0, None)), hex(&line));
    assert_eq!(read(text.as_bytes(), OverlayFormat::Wkb).features.len(), 2);
    let other = hex(&wkb_point(1.0, 2.0, Some(2154)));
    assert_eq!(refused(other.as_bytes(), OverlayFormat::Wkb).code, "CRS_UNSUPPORTED");
    // a polygon claiming four billion rings in a few bytes: refused before anything is made
    let mut bomb = vec![1];
    bomb.extend(3_u32.to_le_bytes());
    bomb.extend(u32::MAX.to_le_bytes());
    let error = refused(&bomb, OverlayFormat::Wkb);
    assert_eq!(error.code, "NO_FEATURES");
    assert!(error.message.contains("more than its bytes hold"), "{}", error.message);
    // a good geometry then a broken one: the first kept, the rest noted
    let mut broken = wkb_point(1.0, 2.0, None);
    broken.extend([1, 9, 0, 0, 0, 1, 2, 3]);
    let overlay = read(&broken, OverlayFormat::Wkb);
    assert_eq!((overlay.features.len(), overlay.rejected), (1, 1));
}

#[test]
fn delimited_wkt_wkb_and_point_columns() {
    let point_hex = hex(&wkb_point(3.0, 4.0, None));
    let text = format!(
        "id;Name;WKT;notes\n1;Pond;POINT (-71.06 42.36);x\n2;\"Trail; north\";\"LINESTRING (0 0, 1 1)\";\n3;Hex;{point_hex};\n4;Bad;POINT (1);\n5;Empty;;\n"
    );
    let overlay = read(text.as_bytes(), OverlayFormat::Csv);
    assert_eq!(overlay.features.len(), 3);
    assert_eq!(overlay.rejected, 2);
    assert_eq!(overlay.features[1]["properties"]["label"], "Trail; north");
    assert_eq!(overlay.features[2]["geometry"]["coordinates"], json!([3.0, 4.0]));
    assert!(notes(&overlay).contains("other properties"));
    let points = read(
        b"lat,lng,title,description\n42.36,-71.06,Start,<i>here</i>\n91,0,Wrong,\n",
        OverlayFormat::Csv,
    );
    assert_eq!(points.features.len(), 1);
    assert_eq!(
        points.features[0]["properties"],
        json!({"label": "Start", "description": "here"})
    );
    assert_eq!(points.features[0]["geometry"]["coordinates"], json!([-71.06, 42.36]));
    assert_eq!(refused(b"a,b\n1,2\n", OverlayFormat::Csv).code, "NOT_FORMAT");
}

#[test]
fn gml_features_axis_order_and_crs() {
    let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="urn:app">
  <wfs:member><app:park gml:id="park.1"><gml:name>Common</gml:name><app:area>20</app:area>
    <app:geom><gml:Polygon srsName="urn:ogc:def:crs:EPSG::4326"><gml:exterior><gml:LinearRing>
      <gml:posList>42.35 -71.07 42.36 -71.07 42.36 -71.06 42.35 -71.07</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></app:geom>
  </app:park></wfs:member>
  <wfs:member><app:stop><app:label>Stop</app:label><app:geom><gml:Point srsName="EPSG:4326"><gml:pos>-71.05 42.37</gml:pos></gml:Point></app:geom></app:stop></wfs:member>
  <wfs:member><app:route><gml:description>Route</gml:description><app:geom><gml:MultiCurve srsName="http://www.opengis.net/def/crs/EPSG/0/3857">
    <gml:curveMember><gml:LineString><gml:posList srsDimension="3">0 0 9 20037508.342789244 0 9</gml:posList></gml:LineString></gml:curveMember>
    <gml:curveMember><gml:Curve><gml:segments><gml:LineStringSegment><gml:posList>0 0 1 1</gml:posList></gml:LineStringSegment>
      <gml:LineStringSegment><gml:posList>1 1 2 2</gml:posList></gml:LineStringSegment></gml:segments></gml:Curve></gml:curveMember>
  </gml:MultiCurve></app:geom></app:route></wfs:member>
  <wfs:member><app:arc><app:geom><gml:Curve srsName="EPSG:4326"><gml:segments><gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc></gml:segments></gml:Curve></app:geom></app:arc></wfs:member>
  <wfs:member><app:empty><app:name>Nothing</app:name></app:empty></wfs:member>
</wfs:FeatureCollection>"#;
    let overlay = read(text.as_bytes(), OverlayFormat::Gml);
    assert_eq!(overlay.features.len(), 3, "{:?}", overlay.notes);
    assert_eq!(overlay.rejected, 2);
    let park = &overlay.features[0];
    assert_eq!(park["id"], "park.1");
    assert_eq!(park["properties"]["label"], "Common");
    // the URN is latitude first: swapped to longitude first
    assert_eq!(park["geometry"]["coordinates"][0][0], json!([-71.07, 42.35]));
    assert_eq!(overlay.features[1]["geometry"]["coordinates"], json!([-71.05, 42.37]));
    let route = &overlay.features[2]["geometry"];
    assert_eq!(route["type"], "MultiLineString");
    assert!((route["coordinates"][0][1][0].as_f64().unwrap() - 180.0).abs() < 1e-9);
    assert!(route["coordinates"][1][1][0].as_f64().unwrap() < 0.001);
    assert_eq!(route["coordinates"][1].as_array().unwrap().len(), 3, "segments joined");
    let notes = notes(&overlay);
    assert!(notes.contains("a GML curve segment that is not read (Arc)"), "{notes}");
    assert!(notes.contains("another CRS"), "{notes}");
    // GML 2 coordinates, a geometry root, no srsName
    let gml2 = r#"<gml:LineString xmlns:gml="http://www.opengis.net/gml"><gml:coordinates>1,2 3,4</gml:coordinates></gml:LineString>"#;
    let line = read(gml2.as_bytes(), OverlayFormat::Gml);
    assert_eq!(
        line.features[0]["geometry"]["coordinates"],
        json!([[1.0, 2.0], [3.0, 4.0]])
    );
    assert!(notes_of(gml2.as_bytes(), OverlayFormat::Gml).contains("no srsName"));
    let other = r#"<gml:Point xmlns:gml="http://www.opengis.net/gml/3.2" srsName="EPSG:27700"><gml:pos>1 2</gml:pos></gml:Point>"#;
    assert_eq!(refused(other.as_bytes(), OverlayFormat::Gml).code, "CRS_UNSUPPORTED");
    let entity = r#"<?xml version="1.0"?><!DOCTYPE x [<!ENTITY e SYSTEM "file:///etc/passwd">]><gml:Point><gml:pos>&e;</gml:pos></gml:Point>"#;
    assert_eq!(refused(entity.as_bytes(), OverlayFormat::Gml).code, "NOT_PARSED");
    assert_eq!(refused(b"<root><a/></root>", OverlayFormat::Gml).code, "NOT_FORMAT");
}

/// A shapefile's .shp of the given records' contents (shape type 0 in the header: a test does not need it).
fn shp(records: &[Vec<u8>], bbox: [f64; 4]) -> Vec<u8> {
    let mut out = vec![0_u8; 100];
    out[0..4].copy_from_slice(&9994_i32.to_be_bytes());
    out[28..32].copy_from_slice(&1000_i32.to_le_bytes());
    for (index, value) in bbox.iter().enumerate() {
        out[36 + index * 8..44 + index * 8].copy_from_slice(&value.to_le_bytes());
    }
    for (number, content) in records.iter().enumerate() {
        out.extend((number as i32 + 1).to_be_bytes());
        out.extend((content.len() as i32 / 2).to_be_bytes());
        out.extend(content);
    }
    let words = out.len() as i32 / 2;
    out[24..28].copy_from_slice(&words.to_be_bytes());
    out
}

fn point_record(x: f64, y: f64) -> Vec<u8> {
    let mut out = 1_i32.to_le_bytes().to_vec();
    out.extend(x.to_le_bytes());
    out.extend(y.to_le_bytes());
    out
}

/// A polygon record of rings (the box is not read here, so it is zero).
fn polygon_record(rings: &[&[[f64; 2]]]) -> Vec<u8> {
    let mut out = 5_i32.to_le_bytes().to_vec();
    out.extend([0_u8; 32]);
    let points: usize = rings.iter().map(|ring| ring.len()).sum();
    out.extend((rings.len() as i32).to_le_bytes());
    out.extend((points as i32).to_le_bytes());
    let mut start = 0;
    for ring in rings {
        out.extend((start as i32).to_le_bytes());
        start += ring.len();
    }
    for [x, y] in rings.iter().flat_map(|ring| ring.iter()) {
        out.extend(x.to_le_bytes());
        out.extend(y.to_le_bytes());
    }
    out
}

/// A dBASE III table of character fields.
fn dbf(fields: &[&str], rows: &[(bool, Vec<&str>)]) -> Vec<u8> {
    let width = 20_usize;
    let header_len = 32 + fields.len() * 32 + 1;
    let record_len = 1 + fields.len() * width;
    let mut out = vec![0_u8; 32];
    out[0] = 3;
    out[4..8].copy_from_slice(&(rows.len() as u32).to_le_bytes());
    out[8..10].copy_from_slice(&(header_len as u16).to_le_bytes());
    out[10..12].copy_from_slice(&(record_len as u16).to_le_bytes());
    for name in fields {
        let mut descriptor = [0_u8; 32];
        descriptor[..name.len()].copy_from_slice(name.as_bytes());
        descriptor[11] = b'C';
        descriptor[16] = width as u8;
        out.extend(descriptor);
    }
    out.push(0x0D);
    for (deleted, values) in rows {
        out.push(if *deleted { b'*' } else { b' ' });
        for value in values {
            let mut cell = vec![b' '; width];
            cell[..value.len()].copy_from_slice(value.as_bytes());
            out.extend(cell);
        }
    }
    out
}

fn zip_of(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in members {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

const WGS84_PRJ: &str = r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#;

#[test]
fn shapefile_records_with_dbf_and_prj() {
    let outer: &[[f64; 2]] = &[[0.0, 0.0], [0.0, 10.0], [10.0, 10.0], [10.0, 0.0], [0.0, 0.0]];
    let hole: &[[f64; 2]] = &[[2.0, 2.0], [4.0, 2.0], [4.0, 4.0], [2.0, 4.0], [2.0, 2.0]];
    let records = vec![
        point_record(-71.06, 42.36),
        polygon_record(&[outer, hole]),
        point_record(-71.0, 42.0),
        0_i32.to_le_bytes().to_vec(),
    ];
    let shp = shp(&records, [-72.0, 0.0, 10.0, 43.0]);
    let table = dbf(
        &["NAME", "KIND"],
        &[
            (false, vec!["Pond", "water"]),
            (false, vec!["Park <b>", "green"]),
            (true, vec!["Gone", ""]),
            (false, vec!["Null", ""]),
        ],
    );
    let zip = zip_of(&[
        ("parks/parks.shp", &shp),
        ("parks/parks.dbf", &table),
        ("parks/parks.prj", WGS84_PRJ.as_bytes()),
    ]);
    let overlay = read(&zip, OverlayFormat::Shapefile);
    assert_eq!(overlay.features.len(), 2, "{:?}", overlay.notes);
    assert_eq!(overlay.features[0]["properties"]["label"], "Pond");
    assert_eq!(overlay.features[1]["properties"]["label"], "Park");
    assert_eq!(overlay.features[1]["geometry"]["type"], "Polygon");
    assert_eq!(
        overlay.features[1]["geometry"]["coordinates"].as_array().unwrap().len(),
        2
    );
    assert_eq!(overlay.rejected, 1);
    let notes = notes(&overlay);
    assert!(notes.contains("1 records marked deleted"), "{notes}");
    assert!(notes.contains("left out: a null shape"), "{notes}");
    assert!(notes.contains("other properties"), "{notes}");
}

#[test]
fn shapefile_crs_and_refusals() {
    let shp_bytes = shp(&[point_record(1.0, 2.0)], [1.0, 2.0, 1.0, 2.0]);
    let mercator = r#"PROJCS["WGS_1984_Web_Mercator_Auxiliary_Sphere",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]]],PROJECTION["Mercator_Auxiliary_Sphere"]]"#;
    let read_mercator = read(
        &zip_of(&[("a.shp", &shp_bytes), ("a.prj", mercator.as_bytes())]),
        OverlayFormat::Shapefile,
    );
    assert_eq!(read_mercator.crs, "EPSG:3857");
    let utm = r#"PROJCS["NAD_1983_UTM_Zone_19N",GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983"]]]"#;
    let error = refused(
        &zip_of(&[("a.shp", &shp_bytes), ("a.prj", utm.as_bytes())]),
        OverlayFormat::Shapefile,
    );
    assert_eq!(error.code, "CRS_UNSUPPORTED");
    assert!(error.message.contains("NAD_1983_UTM_Zone_19N"), "{}", error.message);
    let nad83 = r#"GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983",SPHEROID["GRS_1980",6378137,298.257222101]]]"#;
    assert_eq!(
        refused(
            &zip_of(&[("a.shp", &shp_bytes), ("a.prj", nad83.as_bytes())]),
            OverlayFormat::Shapefile
        )
        .code,
        "CRS_UNSUPPORTED"
    );
    // no .prj: degrees read as WGS84, meters refused
    let bare = read(&zip_of(&[("a.shp", &shp_bytes)]), OverlayFormat::Shapefile);
    assert!(notes(&bare).contains("no .prj") && notes(&bare).contains("no .dbf"));
    let meters = shp(
        &[point_record(530_000.0, 180_000.0)],
        [530_000.0, 180_000.0, 530_000.0, 180_000.0],
    );
    assert_eq!(
        refused(&zip_of(&[("a.shp", &meters)]), OverlayFormat::Shapefile).code,
        "CRS_UNSUPPORTED"
    );
    assert_eq!(
        refused(&zip_of(&[("a.txt", b"x")]), OverlayFormat::Shapefile).code,
        "NOT_FORMAT"
    );
    assert_eq!(
        refused(
            &zip_of(&[("a.shp", &shp_bytes), ("b.shp", &shp_bytes)]),
            OverlayFormat::Shapefile
        )
        .code,
        "NOT_FORMAT"
    );
    assert_eq!(refused(b"not a zip", OverlayFormat::Shapefile).code, "NOT_FORMAT");
    assert_eq!(
        refused(&zip_of(&[("a.shp", b"short")]), OverlayFormat::Shapefile).code,
        "NOT_FORMAT"
    );
}

#[test]
fn shapefile_bombs_refused_cheaply() {
    // a record claiming a billion points in a few bytes, and a record running past the file
    let mut claim = 5_i32.to_le_bytes().to_vec();
    claim.extend([0_u8; 32]);
    claim.extend(1_i32.to_le_bytes());
    claim.extend(1_000_000_000_i32.to_le_bytes());
    claim.extend(0_i32.to_le_bytes());
    let bytes = shp(&[claim, point_record(1.0, 2.0)], [0.0, 0.0, 1.0, 2.0]);
    let overlay = read(&zip_of(&[("a.shp", &bytes)]), OverlayFormat::Shapefile);
    assert_eq!((overlay.features.len(), overlay.rejected), (1, 1));
    assert!(notes(&overlay).contains("more than its record holds"));
    let mut bytes = shp(&[point_record(1.0, 2.0), point_record(3.0, 4.0)], [0.0, 0.0, 3.0, 4.0]);
    let at = bytes.len() - 20;
    bytes[at - 4..at].copy_from_slice(&1_000_000_i32.to_be_bytes());
    let overlay = read(&zip_of(&[("a.shp", &bytes)]), OverlayFormat::Shapefile);
    assert!(notes(&overlay).contains("runs past the end"), "{:?}", overlay.notes);
    // a member that inflates past four times the overlay's bytes: refused as it inflates
    let zeros = vec![0_u8; 900_000];
    let small = OverlayLimits {
        max_bytes: 200_000,
        max_features: 10,
    };
    let zip = zip_of(&[("a.shp", &zeros)]);
    assert!(zip.len() < 200_000);
    let error = convert_overlay_bytes(&zip, OverlayFormat::Shapefile, &small).expect_err("inflates too far");
    assert_eq!(error.code, "TOO_LARGE");
}

#[test]
fn text_formats_must_be_utf8() {
    assert_eq!(refused(&[0xff, 0xfe, 0x00], OverlayFormat::GeoJson).code, "NOT_TEXT");
    assert_eq!(refused(&[0xff, 0xfe, 0x00], OverlayFormat::Gml).code, "NOT_TEXT");
}

#[test]
fn many_rings_grouped_in_bounded_time() {
    // 9,000 counterclockwise rings no outer ring holds, beside one clockwise ring: each is tried only against the outer
    // ring whose box holds it, and none against the rings taken as outer before it
    let mut rings = vec![vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]];
    for index in 0..9_000 {
        let x = 2.0 + f64::from(index) * 0.001;
        rings.push(vec![[x, 2.0], [x + 0.0005, 2.0], [x + 0.0005, 2.0005], [x, 2.0]]);
    }
    let started = std::time::Instant::now();
    let polygons = rings_to_polygons(rings.clone()).expect("grouped");
    assert_eq!(polygons.len(), 9_001);
    assert!(started.elapsed().as_secs() < 2, "{:?}", started.elapsed());
    // past the rings read, or the work allowed, the feature is left out
    rings.extend(rings.clone());
    assert!(rings_to_polygons(rings).unwrap_err().contains("rings, more than"));
    // one outer ring of 30,000 positions holding 1,000 holes: 30 million edge tests, past the work allowed
    let big: Vec<[f64; 2]> = (0..30_000)
        .map(|index| {
            let angle = -f64::from(index) / 30_000.0 * std::f64::consts::TAU;
            [angle.cos(), angle.sin()]
        })
        .collect();
    let mut nested = vec![big];
    for _ in 0..1_000 {
        nested.push(vec![[0.0, 0.0], [0.1, 0.0], [0.1, 0.1], [0.0, 0.0]]);
    }
    let started = std::time::Instant::now();
    assert!(rings_to_polygons(nested).unwrap_err().contains("too many to group"));
    assert!(started.elapsed().as_secs() < 2, "{:?}", started.elapsed());
}

#[test]
fn positions_bounded_for_every_format() {
    // 2,000 WKT points in a file of 40,000 bytes: a limit of 4,000 bytes' worth of positions is passed whatever the format
    let text: String = (0..2_000).map(|_| "MULTIPOINT ((1 2), (3 4), (5 6))\n").collect();
    let limits = OverlayLimits {
        max_bytes: 100_000,
        max_features: 10_000,
    };
    assert!(convert_overlay(&text, OverlayFormat::Wkt, &limits).is_ok());
    let tight = OverlayLimits {
        max_bytes: 100_000,
        max_features: 10_000,
    };
    let mut reader = Reader::new(&tight, &crate::geo::proj::Registry::empty());
    reader.positions_left = 3_000;
    let error = reader.wkt_file(&text).expect_err("too many positions");
    assert_eq!(error.code, "TOO_LARGE");
}

#[test]
fn encodings_crs_names_and_srid_notes() {
    // a delimited file saved as Windows-1252
    let overlay = read(b"name,lat,lon\nCaf\xe9 \x80,1,2\n", OverlayFormat::Csv);
    assert_eq!(overlay.features[0]["properties"]["label"], "Café €");
    assert!(notes(&overlay).contains("Windows-1252"));
    // WKT without an SRID is said to be read as WGS84; a GML member in another CRS rejects its feature
    assert!(notes_of(b"POINT (1 2)", OverlayFormat::Wkt).contains("name no SRID"));
    let mixed = r#"<gml:MultiPoint xmlns:gml="http://www.opengis.net/gml/3.2" srsName="EPSG:4326">
        <gml:pointMember><gml:Point><gml:pos>1 2</gml:pos></gml:Point></gml:pointMember>
        <gml:pointMember><gml:Point srsName="EPSG:3857"><gml:pos>100 200</gml:pos></gml:Point></gml:pointMember></gml:MultiPoint>"#;
    let error = refused(mixed.as_bytes(), OverlayFormat::Gml);
    assert!(
        error.message.contains("member geometry in another CRS"),
        "{}",
        error.message
    );
    // a member's own latitude-first URN of the same CRS is read latitude first
    let urn = r#"<gml:MultiPoint xmlns:gml="http://www.opengis.net/gml/3.2" srsName="EPSG:4326">
        <gml:pointMember><gml:Point srsName="urn:ogc:def:crs:EPSG::4326"><gml:pos>2 1</gml:pos></gml:Point></gml:pointMember></gml:MultiPoint>"#;
    assert_eq!(
        read(urn.as_bytes(), OverlayFormat::Gml).features[0]["geometry"]["coordinates"],
        json!([[1.0, 2.0]])
    );
    // WKT2's GEOGCRS of the WGS 84 ensemble
    let shp_bytes = shp(&[point_record(1.0, 2.0)], [1.0, 2.0, 1.0, 2.0]);
    let wkt2 = r#"GEOGCRS["WGS 84",ENSEMBLE["World Geodetic System 1984 ensemble",MEMBER["World Geodetic System 1984 (G2139)"]],ID["EPSG",4326]]"#;
    assert_eq!(
        read(
            &zip_of(&[("a.shp", &shp_bytes), ("a.prj", wkt2.as_bytes())]),
            OverlayFormat::Shapefile
        )
        .crs,
        "EPSG:4326"
    );
    // a .cpg of a code page not read: a field that is not UTF-8 is left out, not mis-read
    let table = dbf(&["NAME"], &[(false, vec!["\u{00e9}"])]);
    let mut latin = table.clone();
    let at = latin.len() - 20;
    latin.splice(at..at + 2, [0xC7, b' ']);
    let overlay = read(
        &zip_of(&[("a.shp", &shp_bytes), ("a.dbf", &latin), ("a.cpg", b"1251")]),
        OverlayFormat::Shapefile,
    );
    assert!(overlay.features[0]["properties"].get("label").is_none());
    assert!(
        notes(&overlay).contains("code page that is not read"),
        "{:?}",
        overlay.notes
    );
    // dBASE 7 is not read, and said
    let mut seven = table;
    seven[0] = 4;
    let overlay = read(
        &zip_of(&[("a.shp", &shp_bytes), ("a.dbf", &seven)]),
        OverlayFormat::Shapefile,
    );
    assert!(notes(&overlay).contains("dBASE 7"), "{:?}", overlay.notes);
}

/// The registry the specs and the seed have for these: WGS 84 / UTM zone 19N, the British National Grid, NAD83 and
/// LAEA Europe (northing first by its authority)
fn registry() -> crate::geo::proj::Registry {
    let mut registry = crate::geo::proj::Registry::empty();
    for (code, label, definition, axis) in [
        ("EPSG:32619", "WGS 84 / UTM zone 19N", "+proj=utm +zone=19 +datum=WGS84 +units=m +no_defs", "enu"),
        ("EPSG:26919", "NAD83 / UTM zone 19N", "+proj=utm +zone=19 +datum=NAD83 +units=m +no_defs", "enu"),
        (
            "EPSG:27700",
            "OSGB36 / British National Grid",
            "+proj=tmerc +lat_0=49 +lon_0=-2 +k=0.9996012717 +x_0=400000 +y_0=-100000 +ellps=airy +towgs84=446.448,-125.157,542.06,0.15,0.247,0.842,-20.489 +units=m +no_defs",
            "enu",
        ),
        ("EPSG:4269", "NAD83", "+proj=longlat +datum=NAD83 +no_defs", "neu"),
        (
            "EPSG:3035",
            "ETRS89-extended / LAEA Europe",
            "+proj=laea +lat_0=52 +lon_0=10 +x_0=4321000 +y_0=3210000 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
            "neu",
        ),
    ] {
        registry.add(code, label, definition, axis).expect(code);
    }
    registry
}

fn read_with(bytes: &[u8], format: OverlayFormat) -> Overlay {
    convert_overlay_with(bytes, format, &LIMITS, &registry()).expect("the file reads")
}

fn point_of(overlay: &Overlay, index: usize) -> (f64, f64) {
    let coordinates = &overlay.features[index]["geometry"]["coordinates"];
    (coordinates[0].as_f64().unwrap(), coordinates[1].as_f64().unwrap())
}

#[test]
fn registered_crs_reprojected_with_provenance() {
    // a NAD83 UTM 19N shapefile (Boston, about 42.36°N 71.06°W): its .prj named by ESRI, read through the registry
    let shp_bytes = shp(
        &[point_record(330_000.0, 4_691_000.0)],
        [330_000.0, 4_691_000.0, 330_000.0, 4_691_000.0],
    );
    let utm = r#"PROJCS["NAD_1983_UTM_Zone_19N",GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",-69.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
    let zip = zip_of(&[("a.shp", &shp_bytes), ("a.prj", utm.as_bytes())]);
    let overlay = read_with(&zip, OverlayFormat::Shapefile);
    assert_eq!(overlay.crs, "EPSG:26919");
    assert!(
        overlay.transform.contains("transverse Mercator") && overlay.transform.contains("within about 2 m"),
        "{}",
        overlay.transform
    );
    let (lon, lat) = point_of(&overlay, 0);
    assert!((lon + 71.06).abs() < 0.05 && (lat - 42.35).abs() < 0.05, "{lon} {lat}");
    // the same file is still refused with no registry
    assert_eq!(refused(&zip, OverlayFormat::Shapefile).code, "CRS_UNSUPPORTED");

    // a GeoJSON file in the British National Grid (its pre-2016 crs member), converted with the OSGB36 shift
    let bng = r#"{"type":"FeatureCollection","crs":{"type":"name","properties":{"name":"urn:ogc:def:crs:EPSG::27700"}},
        "features":[{"type":"Feature","properties":{"name":"Greenwich"},"geometry":{"type":"Point","coordinates":[538874,177344]}}]}"#;
    let overlay = read_with(bng.as_bytes(), OverlayFormat::GeoJson);
    assert_eq!(overlay.crs, "EPSG:27700");
    assert!(overlay.transform.contains("OSGB36"), "{}", overlay.transform);
    let (lon, lat) = point_of(&overlay, 0);
    assert!(lon.abs() < 0.01 && (lat - 51.478).abs() < 0.01, "{lon} {lat}");

    // GML: a URN of LAEA Europe gives northing first, its short name easting first; NAD83's URN latitude first
    let gml = |srs: &str, pos: &str| {
        format!(
            r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:x="urn:x">
            <wfs:member><x:p><x:geom><gml:Point srsName="{srs}"><gml:pos>{pos}</gml:pos></gml:Point></x:geom></x:p></wfs:member>
            </wfs:FeatureCollection>"#
        )
    };
    let north_first = read_with(
        gml("urn:ogc:def:crs:EPSG::3035", "2999718.85 3962799.45").as_bytes(),
        OverlayFormat::Gml,
    );
    let east_first = read_with(gml("EPSG:3035", "3962799.45 2999718.85").as_bytes(), OverlayFormat::Gml);
    for overlay in [&north_first, &east_first] {
        let (lon, lat) = point_of(overlay, 0);
        assert!(
            (lon - 5.0).abs() < 1e-6 && (lat - 50.0).abs() < 1e-6,
            "the GN 7-2 LAEA point: {lon} {lat}"
        );
    }
    let nad83 = read_with(
        gml("urn:ogc:def:crs:EPSG::4269", "42.36 -71.06").as_bytes(),
        OverlayFormat::Gml,
    );
    assert_eq!(point_of(&nad83, 0), (-71.06, 42.36));

    // EWKT's SRID and EsriJSON's WKID of a registered CRS; one not registered is refused by its code
    let ewkt = read_with(b"SRID=32619;POINT(330000 4691000)", OverlayFormat::Wkt);
    assert_eq!(ewkt.crs, "EPSG:32619");
    let esri = r#"{"spatialReference":{"wkid":32619},"features":[{"attributes":{"name":"a"},"geometry":{"x":330000,"y":4691000}}]}"#;
    assert_eq!(read_with(esri.as_bytes(), OverlayFormat::EsriJson).crs, "EPSG:32619");
    let unknown = convert_overlay_with(
        b"SRID=2056;POINT(2600000 1200000)",
        OverlayFormat::Wkt,
        &LIMITS,
        &registry(),
    )
    .expect_err("not registered");
    assert_eq!(unknown.code, "CRS_UNSUPPORTED");
    assert!(unknown.message.contains("2056"), "{}", unknown.message);
    // a position outside a projection's domain rejects its feature, not the file
    let far = r#"{"type":"FeatureCollection","crs":{"type":"name","properties":{"name":"EPSG:3035"}},"features":[
        {"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[1e12,1e12]}},
        {"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[3962799.45,2999718.85]}}]}"#;
    let overlay = read_with(far.as_bytes(), OverlayFormat::GeoJson);
    assert_eq!(
        (overlay.features.len(), overlay.rejected),
        (1, 1),
        "{:?}",
        overlay.notes
    );
}
