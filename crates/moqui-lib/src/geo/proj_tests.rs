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

//! The projections against the worked examples of the EPSG Guidance Note 7-2 (Coordinate Conversions and
//! Transformations including Formulas, IOGP) and Snyder's Map Projections: A Working Manual (USGS 1987), each forward to
//! the example's centimetre and back to the point.

use super::*;

fn assert_close(found: (f64, f64), expected: (f64, f64), tolerance: f64, what: &str) {
    assert!(
        (found.0 - expected.0).abs() <= tolerance && (found.1 - expected.1).abs() <= tolerance,
        "{what}: {found:?}, expected {expected:?} within {tolerance}"
    );
}

/// Forward from WGS84-less geodetic degrees on the definition's own datum (no shift is given in these examples), and back.
fn check(definition: &str, lon: f64, lat: f64, expected: (f64, f64), tolerance: f64) {
    let projection = Projection::parse_proj4(definition).unwrap_or_else(|err| panic!("{definition}: {err}"));
    assert!(
        projection.to_wgs84.is_none(),
        "{definition}: no shift in a guidance example"
    );
    let found = projection.from_wgs84(lon, lat).expect("forward");
    assert_close(found, expected, tolerance, definition);
    let back = projection.to_wgs84(found.0, found.1).expect("inverse");
    assert_close(back, (lon, lat), 1e-9, &format!("{definition} inverse"));
    // the example's own rounded coordinates convert back to within its rounding
    let rounded = projection
        .to_wgs84(expected.0, expected.1)
        .expect("inverse of the example");
    assert_close(
        rounded,
        (lon, lat),
        1e-5,
        &format!("{definition} inverse of the example"),
    );
}

#[test]
fn transverse_mercator_british_national_grid_example() {
    // GN 7-2 §3.5.3.1: OSGB 1936 / British National Grid, φ 50°30'N, λ 0°30'E (Airy 1830, no datum shift here)
    check(
        "+proj=tmerc +lat_0=49 +lon_0=-2 +k=0.9996012717 +x_0=400000 +y_0=-100000 +ellps=airy +towgs84=0,0,0 +units=m +no_defs",
        0.5,
        50.5,
        (577_274.99, 69_740.50),
        0.01,
    );
}

#[test]
fn utm_zone_centre_and_far_from_it() {
    let utm31 = Projection::parse_proj4("+proj=utm +zone=31 +datum=WGS84 +units=m +no_defs").expect("utm");
    assert_close(
        utm31.from_wgs84(3.0, 0.0).expect("origin"),
        (500_000.0, 0.0),
        1e-6,
        "zone 31 origin",
    );
    let south = Projection::parse_proj4("+proj=utm +zone=33 +south +datum=WGS84 +units=m +no_defs").expect("south");
    let (x, y) = south.from_wgs84(15.0, -0.000_001).expect("just south of the equator");
    assert!(
        (x - 500_000.0).abs() < 1e-3 && (y - 10_000_000.0).abs() < 1.0,
        "{x} {y}"
    );
    // a round trip 10° from the central meridian stays within a micrometre-degree (Krüger's series holds there)
    for (lon, lat) in [(-7.0, 60.0), (13.0, 45.0), (3.5, -35.0), (12.9, 78.0)] {
        let (x, y) = utm31.from_wgs84(lon, lat).expect("forward");
        assert_close(utm31.to_wgs84(x, y).expect("inverse"), (lon, lat), 1e-9, "round trip");
    }
    // a scale of 0.9996 on the central meridian: one degree of latitude near the equator is about 110.6 km there
    let (_, north) = utm31.from_wgs84(3.0, 1.0).expect("one degree north");
    assert!((north - 110_574.4 * 0.9996).abs() < 1.0, "{north}");
}

#[test]
fn lambert_conformal_conic_texas_example() {
    // GN 7-2 §3.2.1.1: NAD27 / Texas South Central, in US survey feet, Clarke 1866
    check(
        "+proj=lcc +lat_0=27.8333333333333 +lon_0=-99 +lat_1=28.3833333333333 +lat_2=30.2833333333333 \
         +x_0=609601.2192024384 +y_0=0 +a=6378206.4 +b=6356583.8 +towgs84=0,0,0 +units=us-ft +no_defs",
        -96.0,
        28.5,
        (2_963_503.91, 254_759.80),
        0.02,
    );
}

#[test]
fn lambert_azimuthal_equal_area_europe_example() {
    // GN 7-2 §3.10.2: ETRS89 / LAEA Europe (EPSG:3035), φ 50°N, λ 5°E
    check(
        "+proj=laea +lat_0=52 +lon_0=10 +x_0=4321000 +y_0=3210000 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
        5.0,
        50.0,
        (3_962_799.45, 2_999_718.85),
        0.01,
    );
}

#[test]
fn albers_equal_area_snyder_example() {
    // Snyder (1987) p. 292: Clarke 1866, standard parallels 29.5° and 45.5°, origin 23°N 96°W; φ 35°N, λ 75°W
    check(
        "+proj=aea +lat_0=23 +lon_0=-96 +lat_1=29.5 +lat_2=45.5 +x_0=0 +y_0=0 +ellps=clrk66 +towgs84=0,0,0 +units=m +no_defs",
        -75.0,
        35.0,
        (1_885_472.7, 1_535_925.0),
        0.1,
    );
}

#[test]
fn polar_stereographic_variant_b_example() {
    // GN 7-2 §3.5.2.2: WGS 84 / Australian Antarctic Polar Stereographic (EPSG:3032), φ 75°S, λ 120°E
    check(
        "+proj=stere +lat_0=-90 +lat_ts=-71 +lon_0=70 +k=1 +x_0=6000000 +y_0=6000000 +datum=WGS84 +units=m +no_defs",
        120.0,
        -75.0,
        (7_255_380.79, 7_053_389.56),
        0.01,
    );
    // the north: EPSG:3413 round trip and the pole at its origin
    let north =
        Projection::parse_proj4("+proj=stere +lat_0=90 +lat_ts=70 +lon_0=-45 +k=1 +x_0=0 +y_0=0 +datum=WGS84 +units=m")
            .expect("3413");
    assert_close(north.from_wgs84(0.0, 90.0).expect("pole"), (0.0, 0.0), 1e-6, "the pole");
    let (x, y) = north.from_wgs84(-45.0, 70.0).expect("70°N on the central meridian");
    assert!(x.abs() < 1e-6 && y < 0.0, "south of the pole along -45° is -y: {x} {y}");
    assert_close(
        north.to_wgs84(x, y).expect("back"),
        (-45.0, 70.0),
        1e-9,
        "3413 round trip",
    );
}

#[test]
fn web_mercator_and_geographic() {
    let mercator = Projection::parse_proj4(
        "+proj=merc +a=6378137 +b=6378137 +lat_ts=0 +lon_0=0 +x_0=0 +y_0=0 +k=1 +units=m +nadgrids=@null +wktext +no_defs",
    )
    .expect("3857");
    assert_close(
        mercator.from_wgs84(180.0, 0.0).expect("edge"),
        (20_037_508.342_789_244, 0.0),
        1e-6,
        "edge",
    );
    let nad83 = Projection::parse_proj4("+proj=longlat +datum=NAD83 +no_defs").expect("4269");
    assert!(nad83.is_geographic());
    assert_close(
        nad83.to_wgs84(-71.06, 42.36).expect("same"),
        (-71.06, 42.36),
        1e-12,
        "NAD83 taken as WGS84",
    );
    assert!(nad83.describe().contains("within about 2 m"), "{}", nad83.describe());
}

#[test]
fn geocentric_and_helmert_examples() {
    // GN 7-2 §4.1.1: WGS 84 φ 53°48'33.82"N, λ 2°07'46.38"E, h 73 m
    let lat = (53.0 + 48.0 / 60.0 + 33.82 / 3600.0_f64).to_radians();
    let lon = (2.0 + 7.0 / 60.0 + 46.38 / 3600.0_f64).to_radians();
    let [x, y, z] = geodetic_to_geocentric(Ellipsoid::WGS84, lon, lat, 73.0);
    assert!((x - 3_771_793.968).abs() < 0.01 && (y - 140_253.342).abs() < 0.01 && (z - 5_124_304.349).abs() < 0.01);
    let (lon_back, lat_back) = geocentric_to_geodetic(
        Ellipsoid::WGS84,
        geodetic_to_geocentric(Ellipsoid::WGS84, lon, lat, 0.0),
    );
    assert!((lon_back - lon).abs() < 1e-12 && (lat_back - lat).abs() < 1e-12);
    // GN 7-2 §4.4.3.1: WGS 72 to WGS 84 by the position vector convention
    let helmert = Helmert::parse("0,0,4.5,0,0,0.554,0.219").expect("seven");
    let [x, y, z] = helmert.forward([3_657_660.66, 255_768.55, 5_201_382.11]);
    assert!(
        (x - 3_657_660.78).abs() < 0.01 && (y - 255_778.43).abs() < 0.01 && (z - 5_201_387.75).abs() < 0.01,
        "{x} {y} {z}"
    );
    let back = helmert.inverse([x, y, z]);
    assert!(
        (back[0] - 3_657_660.66).abs() < 1e-6
            && (back[1] - 255_768.55).abs() < 1e-6
            && (back[2] - 5_201_382.11).abs() < 1e-6
    );
}

#[test]
fn british_national_grid_with_its_datum_shift() {
    let bng = Projection::parse_proj4(
        "+proj=tmerc +lat_0=49 +lon_0=-2 +k=0.9996012717 +x_0=400000 +y_0=-100000 +ellps=airy \
         +towgs84=446.448,-125.157,542.06,0.15,0.247,0.842,-20.489 +units=m +no_defs",
    )
    .expect("27700");
    assert!(bng.describe().contains("OSGB36"), "{}", bng.describe());
    // Greenwich's grid square: the shift from OSGB36 to WGS84 there is about 100 m, mostly in longitude
    let (lon, lat) = bng.to_wgs84(538_874.0, 177_344.0).expect("inverse");
    assert!(
        (lat - 51.477_8).abs() < 0.002 && lon.abs() < 0.003,
        "near the Royal Observatory: {lon} {lat}"
    );
    let without = Projection {
        to_wgs84: None,
        ..bng.clone()
    };
    let (lon_osgb, lat_osgb) = without.to_wgs84(538_874.0, 177_344.0).expect("no shift");
    let metres = ((lon - lon_osgb).to_radians() * 6_378_137.0 * lat.to_radians().cos())
        .hypot((lat - lat_osgb).to_radians() * 6_378_137.0);
    assert!(
        (60.0..160.0).contains(&metres),
        "the OSGB36 to WGS84 shift is {metres} m"
    );
    let (x, y) = bng.from_wgs84(lon, lat).expect("forward");
    assert!(
        (x - 538_874.0).abs() < 1e-3 && (y - 177_344.0).abs() < 1e-3,
        "round trip {x} {y}"
    );
}

#[test]
fn definitions_refused_by_name() {
    for (definition, said) in [
        ("+proj=utm +zone=18 +datum=NAD27 +units=m", "NAD27"),
        ("+proj=tmerc +lat_0=0 +lon_0=0 +ellps=GRS80 +nadgrids=conus", "grid"),
        ("+proj=robin +lon_0=0 +datum=WGS84", "robin"),
        ("+proj=utm +zone=61 +datum=WGS84", "zone"),
        ("+proj=longlat +datum=WGS84 +pm=paris", "pm"),
        ("+proj=tmerc +lat_0=0 +lon_0=0 +ellps=WGS84 +bogus=1", "bogus"),
        ("+proj=tmerc +ellps=mars", "mars"),
        ("+proj=tmerc +a=3396190 +rf=169.894", "Earth"),
        ("+proj=laea +lat_0=90 +lon_0=0 +datum=WGS84", "pole"),
        ("+proj=stere +lat_0=45 +lon_0=0 +datum=WGS84", "pole"),
        ("+proj=utm +zone=33 +towgs84=1,2 +datum=WGS84", "towgs84"),
        ("proj=utm", "parameter"),
        // an ellipsoid of another datum with no shift is not taken as WGS84 (100 to 200 m off)
        (
            "+proj=tmerc +lat_0=0 +lon_0=9 +k=0.9996 +x_0=500000 +ellps=intl +units=m",
            "WGS 84's",
        ),
        ("+proj=longlat +ellps=bessel", "WGS 84's"),
        ("+proj=utm +zone=33 +a=6378206.4 +b=6356583.8", "WGS 84's"),
        // a parameter its method does not use, a latitude past the pole, two scales
        ("+proj=tmerc +zone=33 +datum=WGS84", "+zone"),
        ("+proj=lcc +lat_1=40 +lat_2=50 +south +datum=WGS84", "+south"),
        ("+proj=tmerc +lat_ts=10 +datum=WGS84", "+lat_ts"),
        ("+proj=merc +lat_ts=10 +k=0.9 +datum=WGS84", "+k and +lat_ts"),
        ("+proj=laea +lat_0=52 +k=1 +datum=WGS84", "+k"),
        ("+proj=tmerc +lat_0=95 +datum=WGS84", "within 90"),
    ] {
        let err = Projection::parse_proj4(definition).expect_err(definition);
        assert!(err.contains(said), "{definition}: {err}");
    }
}

#[test]
fn the_registry_finds_a_prj_by_authority_name_or_parameters() {
    let mut registry = Registry::empty();
    registry
        .add(
            "EPSG:32619",
            "WGS 84 / UTM zone 19N",
            "+proj=utm +zone=19 +datum=WGS84 +units=m +no_defs",
            "enu",
        )
        .expect("utm");
    registry
        .add(
            "EPSG:27700",
            "British National Grid",
            "+proj=tmerc +lat_0=49 +lon_0=-2 +k=0.9996012717 +x_0=400000 +y_0=-100000 +ellps=airy +towgs84=446.448,-125.157,542.06,0.15,0.247,0.842,-20.489 +units=m",
            "",
        )
        .expect("bng");
    registry
        .add("EPSG:4269", "NAD83", "+proj=longlat +datum=NAD83 +no_defs", "")
        .expect("nad83");
    assert!(
        registry.get(4269).expect("4269").north_first,
        "a geographic CRS's authority order is latitude first"
    );
    assert!(!registry.get(27700).expect("27700").north_first);
    assert!(registry
        .add("EPSG:4267", "NAD27", "+proj=longlat +datum=NAD27", "")
        .is_err());
    // a row the server does not read says why, and is not one it converts
    assert!(registry.get(4267).is_none());
    assert!(registry.unreadable(4267).is_some_and(|reason| reason.contains("NAD27")));
    assert!(registry.unreadable(32619).is_none());
    // an explicit zero shift on another ellipsoid is taken as the definition says, and said so
    let zero =
        Projection::parse_proj4("+proj=tmerc +lat_0=0 +lon_0=9 +k=0.9996 +x_0=500000 +ellps=intl +towgs84=0,0,0")
            .expect("zero");
    assert!(zero.describe().contains("+towgs84 of zeros"), "{}", zero.describe());
    // by the authority at its top level (not a GEOGCS's inside it)
    let ogc = r#"PROJCS["WGS 84 / UTM zone 19N",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]],AUTHORITY["EPSG","6326"]],AUTHORITY["EPSG","4326"]],PROJECTION["Transverse_Mercator"],UNIT["metre",1],AUTHORITY["EPSG","32619"]]"#;
    assert_eq!(registry.find_wkt(ogc).map(|found| found.code), Some(32619));
    // by ESRI's name
    let esri = r#"PROJCS["WGS_1984_UTM_Zone_19N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",-69.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
    assert_eq!(registry.find_wkt(esri).map(|found| found.code), Some(32619));
    // by its parameters, under a name of its own
    let renamed = esri.replace("WGS_1984_UTM_Zone_19N", "My survey grid");
    assert_eq!(registry.find_wkt(&renamed).map(|found| found.code), Some(32619));
    let bng = r#"PROJCS["British_National_Grid",GEOGCS["GCS_OSGB_1936",DATUM["D_OSGB_1936",SPHEROID["Airy_1830",6377563.396,299.3249646]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",400000.0],PARAMETER["False_Northing",-100000.0],PARAMETER["Central_Meridian",-2.0],PARAMETER["Scale_Factor",0.9996012717],PARAMETER["Latitude_Of_Origin",49.0],UNIT["Meter",1.0]]"#;
    assert_eq!(registry.find_wkt(bng).map(|found| found.code), Some(27700));
    // another zone's parameters match nothing registered
    let other = esri
        .replace("-69.0", "-63.0")
        .replace("WGS_1984_UTM_Zone_19N", "Unnamed");
    assert!(registry.find_wkt(&other).is_none());
    assert_eq!(epsg_number("urn:ogc:def:crs:EPSG::27700"), Some(27700));
    assert_eq!(epsg_number("http://www.opengis.net/def/crs/EPSG/0/3035"), Some(3035));
    assert_eq!(epsg_number("CRS:84"), None);
}

#[test]
fn a_projected_box_in_degrees() {
    let utm = Projection::parse_proj4("+proj=utm +zone=33 +datum=WGS84 +units=m").expect("utm");
    let [west, south, east, north] = wgs84_bounds(&utm, [400_000.0, 5_000_000.0, 600_000.0, 5_200_000.0]).expect("box");
    assert!(
        west > 13.0 && west < 14.0 && east > 16.0 && east < 17.0 && south > 45.0 && north < 47.0,
        "{west} {south} {east} {north}"
    );
    let polar =
        Projection::parse_proj4("+proj=stere +lat_0=-90 +lat_ts=-71 +lon_0=0 +k=1 +x_0=0 +y_0=0 +datum=WGS84 +units=m")
            .expect("3031");
    let around = wgs84_bounds(&polar, [-1_000_000.0, -1_000_000.0, 1_000_000.0, 1_000_000.0]).expect("polar box");
    assert_eq!(
        (around[0], around[1], around[2]),
        (-180.0, -90.0, 180.0),
        "the pole inside it: {around:?}"
    );
    assert!(builtin(3857).is_some() && builtin(4326).expect("4326").is_geographic() && builtin(32633).is_none());
}
