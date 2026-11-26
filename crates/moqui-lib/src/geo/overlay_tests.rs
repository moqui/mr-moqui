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

use super::*;

const LIMITS: OverlayLimits = OverlayLimits {
    max_bytes: 100_000,
    max_features: 10,
};

fn read(text: &str, format: OverlayFormat) -> Overlay {
    convert_overlay(text, format, &LIMITS).expect("the file reads")
}

fn refused(text: &str, format: OverlayFormat, limits: &OverlayLimits) -> OverlayError {
    convert_overlay(text, format, limits).expect_err("the file is refused")
}

#[test]
fn geojson_bad_geometry_rejected_alone() {
    let text = r#"{"type":"FeatureCollection","features":[
        {"type":"Feature","properties":{"name":"Good","other":1},"geometry":{"type":"Point","coordinates":[-71.06,42.36]}},
        {"type":"Feature","properties":{"name":"Far"},"geometry":{"type":"Point","coordinates":[10,95]}},
        {"type":"Feature","properties":{"name":"Short"},"geometry":{"type":"LineString","coordinates":[[1,2]]}},
        {"type":"Feature","properties":{"name":"Open"},"geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1]]]}},
        {"type":"Feature","properties":{"name":"Text"},"geometry":{"type":"Point","coordinates":["a",1]}},
        {"type":"Feature","properties":{},"geometry":null}
    ]}"#;
    let overlay = read(text, OverlayFormat::GeoJson);
    // the good point and the polygon, closed; the others each left out with its reason
    assert_eq!(overlay.features.len(), 2);
    assert_eq!(overlay.rejected, 4);
    assert_eq!(overlay.features[0]["properties"]["label"], "Good");
    assert_eq!(overlay.features[0]["id"], "f1");
    assert_eq!(overlay.features[1]["id"], "f2");
    let ring = overlay.features[1]["geometry"]["coordinates"][0].as_array().unwrap();
    assert_eq!(ring.len(), 4);
    assert_eq!(ring[0], ring[3]);
    let notes = overlay.notes.join("\n");
    assert!(notes.contains("feature 2 left out: a latitude out of range"), "{notes}");
    assert!(
        notes.contains("feature 3 left out: a line of fewer than two positions"),
        "{notes}"
    );
    assert!(
        notes.contains("feature 5 left out: a coordinate is not a number"),
        "{notes}"
    );
    assert!(notes.contains("feature 6 left out: it has no geometry"), "{notes}");
    assert!(
        notes.contains("1 polygon rings that were not closed were closed"),
        "{notes}"
    );
    assert!(notes.contains("other properties"), "{notes}");
    assert_eq!(overlay.bounds, Some([-71.06, 0.0, 1.0, 42.36]));
    assert_eq!(overlay.crs, "EPSG:4326");
}

#[test]
fn geojson_crs_web_mercator_converted_other_refused() {
    let mercator = r#"{"type":"Feature","crs":{"type":"name","properties":{"name":"urn:ogc:def:crs:EPSG::3857"}},
        "properties":{"name":"Origin"},"geometry":{"type":"Point","coordinates":[20037508.342789244,0]}}"#;
    let overlay = read(mercator, OverlayFormat::GeoJson);
    assert_eq!(overlay.crs, "EPSG:3857");
    let point = &overlay.features[0]["geometry"]["coordinates"];
    assert!((point[0].as_f64().unwrap() - 180.0).abs() < 1e-9);
    assert!(point[1].as_f64().unwrap().abs() < 1e-9);
    let other = r#"{"type":"Feature","crs":{"type":"name","properties":{"name":"EPSG:27700"}},
        "geometry":{"type":"Point","coordinates":[530000,180000]}}"#;
    assert_eq!(refused(other, OverlayFormat::GeoJson, &LIMITS).code, "CRS_UNSUPPORTED");
    // a feature's own short, plain id is kept; one used already or with other characters is numbered
    let ids = r#"{"type":"FeatureCollection","features":[
        {"type":"Feature","id":"p1","geometry":{"type":"Point","coordinates":[1,2]}},
        {"type":"Feature","id":"p1","geometry":{"type":"Point","coordinates":[1,2]}},
        {"type":"Feature","id":"<x>","geometry":{"type":"Point","coordinates":[1,2]}}]}"#;
    let numbered = read(ids, OverlayFormat::GeoJson);
    let found: Vec<&str> = numbered
        .features
        .iter()
        .map(|feature| feature["id"].as_str().unwrap())
        .collect();
    assert_eq!(found, ["p1", "f2", "f3"]);
    let crs84 = r#"{"type":"Point","crs":{"type":"name","properties":{"name":"urn:ogc:def:crs:OGC:1.3:CRS84"}},"coordinates":[2,48]}"#;
    assert_eq!(read(crs84, OverlayFormat::GeoJson).features.len(), 1);
}

#[test]
fn caps_and_whole_file_refusals() {
    let point = r#"{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[1,2]}}"#;
    let eleven = format!(
        r#"{{"type":"FeatureCollection","features":[{}]}}"#,
        [point; 11].join(",")
    );
    assert_eq!(
        refused(&eleven, OverlayFormat::GeoJson, &LIMITS).code,
        "TOO_MANY_FEATURES"
    );
    let small = OverlayLimits {
        max_bytes: 50,
        max_features: 10,
    };
    assert_eq!(refused(&eleven, OverlayFormat::GeoJson, &small).code, "TOO_LARGE");
    assert_eq!(refused("{not json", OverlayFormat::GeoJson, &LIMITS).code, "NOT_PARSED");
    assert_eq!(
        refused(r#"{"type":"Topology"}"#, OverlayFormat::GeoJson, &LIMITS).code,
        "NOT_FORMAT"
    );
    let none = r#"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"Point","coordinates":[1,99]}}]}"#;
    let error = refused(none, OverlayFormat::GeoJson, &LIMITS);
    assert_eq!(error.code, "NO_FEATURES");
    assert!(error.message.contains("latitude out of range"), "{}", error.message);
    assert_eq!(refused("<gpx><wpt", OverlayFormat::Gpx, &LIMITS).code, "NOT_PARSED");
    assert_eq!(refused("<kml/>", OverlayFormat::Gpx, &LIMITS).code, "NOT_FORMAT");
}

#[test]
fn kml_sanitized_and_references_not_followed() {
    let text = r##"<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2">
  <Document>
    <Style id="s"><IconStyle><Icon><href>https://tracker.example/icon.png</href></Icon></IconStyle></Style>
    <NetworkLink><Link><href>https://tracker.example/more.kml</href></Link></NetworkLink>
    <GroundOverlay><Icon><href>https://tracker.example/photo.jpg</href></Icon></GroundOverlay>
    <Folder>
      <Placemark>
        <name>Park &amp; <b>pond</b></name>
        <description><![CDATA[<p>Open daily</p><script>alert('x')</script><img src="https://tracker.example/p.gif" onerror="alert(1)">Free]]></description>
        <Point><coordinates>-71.06,42.36,0</coordinates></Point>
      </Placemark>
      <Placemark>
        <name>Paths</name>
        <MultiGeometry>
          <LineString><coordinates>0,0 1,1</coordinates></LineString>
          <LineString><coordinates>2,2 3,3</coordinates></LineString>
        </MultiGeometry>
      </Placemark>
      <Placemark>
        <name>Lake</name>
        <Polygon><outerBoundaryIs><LinearRing><coordinates>0,0 1,0 1,1 0,0</coordinates></LinearRing></outerBoundaryIs>
          <innerBoundaryIs><LinearRing><coordinates>0.2,0.2 0.3,0.2 0.3,0.3 0.2,0.2</coordinates></LinearRing></innerBoundaryIs></Polygon>
      </Placemark>
      <Placemark><name>Walk</name><gx:Track><gx:coord>5 6 0</gx:coord><gx:coord>5.1 6.1 0</gx:coord></gx:Track></Placemark>
      <Placemark><name>Broken</name><Point><coordinates>200,10</coordinates></Point></Placemark>
    </Folder>
  </Document>
</kml>"##;
    let overlay = read(text, OverlayFormat::Kml);
    assert_eq!(overlay.features.len(), 4);
    assert_eq!(overlay.rejected, 1);
    let first = &overlay.features[0]["properties"];
    assert_eq!(first["label"], "Park & pond");
    assert_eq!(first["description"], "Open daily\nFree");
    assert_eq!(overlay.features[1]["geometry"]["type"], "MultiLineString");
    assert_eq!(
        overlay.features[2]["geometry"]["coordinates"].as_array().unwrap().len(),
        2
    );
    assert_eq!(overlay.features[3]["geometry"]["type"], "LineString");
    let all = serde_json::to_string(&overlay.feature_collection()).unwrap();
    assert!(!all.contains("tracker.example"), "{all}");
    assert!(
        !all.contains("<script") && !all.contains("alert") && !all.contains("onerror"),
        "{all}"
    );
    let notes = overlay.notes.join("\n");
    assert!(notes.contains("1 network links were not followed"), "{notes}");
    assert!(notes.contains("1 image overlays were not drawn"), "{notes}");
    assert!(notes.contains("styles were not read"), "{notes}");
    assert!(
        notes.contains("feature 5 left out: a longitude out of range"),
        "{notes}"
    );
}

#[test]
fn kml_doctype_refused() {
    let text = r#"<?xml version="1.0"?><!DOCTYPE kml [<!ENTITY x SYSTEM "file:///etc/passwd">]><kml><Placemark><name>&x;</name></Placemark></kml>"#;
    assert_eq!(refused(text, OverlayFormat::Kml, &LIMITS).code, "NOT_PARSED");
}

#[test]
fn gpx_waypoints_routes_tracks() {
    let text = r#"<gpx version="1.1" creator="spec" xmlns="http://www.topografix.com/GPX/1/1">
  <metadata><name>Trip</name></metadata>
  <wpt lat="42.36" lon="-71.06"><name>Start</name><desc>Here</desc></wpt>
  <wpt lat="91" lon="0"><name>Wrong</name></wpt>
  <rte><name>Route</name><rtept lat="1" lon="1"/><rtept lat="2" lon="2"/></rte>
  <trk><name>Track</name><extensions><x/></extensions>
    <trkseg><trkpt lat="0" lon="0"/><trkpt lat="0.1" lon="0.1"/></trkseg>
    <trkseg><trkpt lat="1" lon="1"/><trkpt lat="1.1" lon="1.1"/></trkseg>
    <trkseg><trkpt lat="5" lon="5"/></trkseg>
  </trk>
</gpx>"#;
    let overlay = read(text, OverlayFormat::Gpx);
    assert_eq!(overlay.features.len(), 3);
    assert_eq!(overlay.rejected, 1);
    assert_eq!(overlay.features[0]["properties"]["description"], "Here");
    assert_eq!(overlay.features[0]["geometry"]["coordinates"][0], -71.06);
    assert_eq!(overlay.features[1]["geometry"]["type"], "LineString");
    assert_eq!(overlay.features[2]["geometry"]["type"], "MultiLineString");
    let notes = overlay.notes.join("\n");
    assert!(notes.contains("feature 2 left out: a latitude out of range"), "{notes}");
    assert!(notes.contains("track segments of fewer than two points"), "{notes}");
    assert!(notes.contains("GPX extensions were not read"), "{notes}");
}

#[test]
fn plain_text_rules() {
    assert_eq!(plain_text("  a <i>b</i>\n c ", 100, false), "a b c");
    assert_eq!(plain_text("3 < 4 &lt;ok&gt; &#65;&#x42;", 100, false), "3 < 4 <ok> AB");
    assert_eq!(
        plain_text("x<SCRIPT>bad()</SCRIPT>y<!-- hidden -->z", 100, false),
        "x y z"
    );
    assert_eq!(plain_text("abcdef", 3, false), "abc…");
    assert_eq!(plain_text("a\u{202e}b\u{0007}c", 100, false), "abc");
    assert_eq!(OverlayFormat::of_file_name("Trip.GPX"), Some(OverlayFormat::Gpx));
    assert_eq!(OverlayFormat::parse("MLF_KML"), Some(OverlayFormat::Kml));
    assert_eq!(OverlayFormat::of_file_name("notes.txt"), None);
}

#[test]
fn markup_stripped_in_linear_time() {
    // 100,000 comments and as many unclosed '<' and '&': each scan is one pass, and plain_text reads only max * 8 bytes
    let comments = "<!--x-->a".repeat(100_000);
    let started = std::time::Instant::now();
    assert_eq!(strip_markup(&comments).len(), 200_000);
    let opened = "a<b &c ".repeat(100_000);
    assert_eq!(strip_markup(&opened), opened);
    assert_eq!(decode_entities(&opened), opened);
    let scripts = "<script>x</script>".repeat(100_000);
    assert_eq!(strip_markup(&scripts).trim(), "");
    assert_eq!(plain_text(&comments, 2000, true).chars().count(), 2001);
    let description = format!(
        r#"{{"type":"Feature","properties":{{"name":"Many","description":"{comments}"}},"geometry":{{"type":"Point","coordinates":[1,2]}}}}"#
    );
    let limits = OverlayLimits {
        max_bytes: 10_000_000,
        max_features: 10,
    };
    let overlay = convert_overlay(&description, OverlayFormat::GeoJson, &limits).unwrap();
    assert!(overlay.features[0]["properties"]["description"]
        .as_str()
        .unwrap()
        .starts_with("a a a"));
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
}

#[test]
fn an_unclosed_lt_is_text() {
    assert_eq!(plain_text("a<b and more", 100, false), "a<b and more");
    assert_eq!(plain_text("x <i>y</i> 1<2", 100, false), "x y 1<2");
    assert_eq!(plain_text("before<script>never closed", 100, false), "before");
}
