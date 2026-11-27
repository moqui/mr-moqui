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
use crate::testdata;

fn read_all(bytes: &Vec<u8>, theme: Theme, region: &Bbox) -> (Vec<Row>, Counts) {
    let file = open(bytes, theme, &Limits::standard()).unwrap();
    let mut rows = Vec::new();
    let mut counts = Counts::default();
    let finished = read_region(
        bytes,
        &file,
        theme,
        region,
        &Limits::standard(),
        &mut counts,
        &mut |row| {
            rows.push(row);
            Ok(true)
        },
        &mut |_| true,
    )
    .unwrap();
    assert!(finished);
    (rows, counts)
}

#[test]
fn places_in_the_region_are_read_with_their_sources() {
    let bytes = testdata::spec_places_r1();
    let (rows, counts) = read_all(&bytes, Theme::Places, &testdata::SPEC_REGION);
    let places: Vec<&Place> = rows
        .iter()
        .map(|row| match row {
            Row::Place(place) => place,
            Row::Building(_) | Row::Address(_) => panic!("not a place"),
        })
        .collect();
    let names: Vec<&str> = places.iter().map(|place| place.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Cattedrale di Santa Maria del Fiore",
            "Caffè Gilli",
            "Galleria degli Uffizi"
        ]
    );
    let duomo = places[0];
    assert_eq!(duomo.gers_id, "08f1e2a0-0000-4000-8000-000000000001");
    assert_eq!(duomo.basic_category.as_deref(), Some("religious_organization"));
    assert_eq!(duomo.taxonomy.as_deref(), Some("religious_organization"));
    assert_eq!(duomo.address.locality.as_deref(), Some("Firenze"));
    assert_eq!(duomo.licenses, vec!["Apache-2.0", "CDLA-Permissive-2.0"]);
    assert_eq!(duomo.datasets, vec!["Foursquare", "meta"]);
    assert_eq!((duomo.lon, duomo.lat), (11.2560, 43.7731));
    // the far row group skipped by its statistics; its five rows read, one outside the region, one not a point
    assert_eq!((counts.row_groups_read, counts.row_groups_skipped), (1, 1));
    assert_eq!((counts.rows_read, counts.rows_kept, counts.rows_rejected), (5, 3, 1));
    assert!(counts.rejections[0].contains("000000000004"), "{:?}", counts.rejections);
    assert_eq!(
        counts
            .licenses
            .get(&("Foursquare".to_string(), "Apache-2.0".to_string())),
        Some(&1)
    );
    assert_eq!(
        counts
            .licenses
            .get(&("meta".to_string(), "CDLA-Permissive-2.0".to_string())),
        Some(&2)
    );
    assert_eq!(
        counts
            .licenses
            .get(&("AllThePlaces".to_string(), "CC0-1.0".to_string())),
        Some(&1)
    );
}

#[test]
fn a_second_release_reads_from_v2_delta_and_snappy_pages() {
    let bytes = testdata::spec_places_r2();
    let (rows, counts) = read_all(&bytes, Theme::Places, &testdata::SPEC_REGION);
    assert_eq!(rows.len(), 3);
    let Row::Place(duomo) = &rows[0] else { panic!("a place") };
    assert_eq!((duomo.name.as_str(), duomo.version), ("Duomo di Firenze", Some(2)));
    assert_eq!(
        counts
            .licenses
            .get(&("example-dataset".to_string(), "CC-BY-4.0".to_string())),
        Some(&1)
    );
}

#[test]
fn buildings_are_read_as_polygons() {
    let bytes = testdata::spec_buildings_r1();
    let (rows, counts) = read_all(&bytes, Theme::Buildings, &testdata::SPEC_REGION);
    assert_eq!(counts.rows_kept, 3, "the one east of the region left out");
    let Row::Building(cathedral) = &rows[0] else {
        panic!("a building")
    };
    assert_eq!(cathedral.name.as_deref(), Some("Cattedrale"));
    assert_eq!((cathedral.height, cathedral.num_floors), (Some(90.0), Some(3)));
    assert_eq!(cathedral.polygons[0][0].len(), 5);
    assert_eq!(cathedral.licenses, vec!["ODbL-1.0"]);
}

#[test]
fn addresses_are_read_with_their_levels_and_sources() {
    let bytes = testdata::spec_addresses_r1();
    let (rows, counts) = read_all(&bytes, Theme::Addresses, &testdata::SPEC_REGION);
    let addresses: Vec<&AddressPoint> = rows
        .iter()
        .map(|row| match row {
            Row::Address(address) => address,
            _ => panic!("not an address"),
        })
        .collect();
    assert_eq!(addresses.len(), 7, "the one east of the region left out");
    let signoria = addresses[0];
    assert_eq!(signoria.country, "IT");
    assert_eq!(signoria.street.as_deref(), Some("PIAZZA DELLA SIGNORIA"));
    assert_eq!(
        (signoria.number.as_deref(), signoria.postcode.as_deref()),
        (Some("5"), Some("50122"))
    );
    assert_eq!(signoria.levels, ["Toscana", "Firenze", "Firenze"]);
    assert_eq!(signoria.licenses, ["CC-BY-4.0"]);
    assert_eq!(addresses[3].unit.as_deref(), Some("A"));
    assert_eq!((counts.row_groups_read, counts.row_groups_skipped), (1, 1));
    assert_eq!((counts.rows_kept, counts.rows_rejected), (7, 1));
    assert!(
        counts.rejections[0].contains("neither a street nor a number"),
        "{:?}",
        counts.rejections
    );
    assert_eq!(
        counts.licenses.get(&("example/cc0".to_string(), "CC0-1.0".to_string())),
        Some(&1)
    );
}

#[test]
fn a_file_without_the_themes_columns_is_refused() {
    let bytes = testdata::write(
        "message schema { OPTIONAL BYTE_ARRAY id (STRING); }",
        &[vec![testdata::Leaf::Bytes(vec![b"x".to_vec()], vec![1], Vec::new())]],
        &testdata::Writer::default(),
    );
    let error = open(&bytes, Theme::Places, &Limits::standard()).unwrap_err();
    assert!(
        error.message().contains("geometry") && error.message().contains("names.primary"),
        "{error}"
    );
}

#[test]
fn a_read_stops_when_asked_and_when_the_sink_is_full() {
    let bytes = testdata::spec_places_r1();
    let file = open(&bytes, Theme::Places, &Limits::standard()).unwrap();
    let mut counts = Counts::default();
    let go = read_region(
        &bytes,
        &file,
        Theme::Places,
        &testdata::SPEC_REGION,
        &Limits::standard(),
        &mut counts,
        &mut |_| Ok(true),
        &mut |_| false,
    )
    .unwrap();
    assert!(!go, "cancelled before the first row group");
    let mut taken = 0;
    let mut counts = Counts::default();
    read_region(
        &bytes,
        &file,
        Theme::Places,
        &testdata::SPEC_REGION,
        &Limits::standard(),
        &mut counts,
        &mut |_| {
            taken += 1;
            Ok(false)
        },
        &mut |_| true,
    )
    .unwrap();
    assert_eq!(taken, 1);
}

#[test]
fn wkb_reads_points_and_polygons_and_refuses_the_rest() {
    assert_eq!(
        wkb::read(&testdata::point_wkb(1.5, 2.5), 10),
        Ok(Geometry::Point([1.5, 2.5]))
    );
    let square = testdata::polygon_wkb(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]]);
    let Ok(Geometry::Polygons(polygons)) = wkb::read(&square, 10) else {
        panic!("a polygon")
    };
    assert_eq!(polygons[0][0].len(), 5);
    assert!(wkb::read(&square, 4).is_err(), "more positions than allowed");
    // a line string, a point past the poles, a ring that claims a billion positions, nothing
    let mut line = vec![1, 2, 0, 0, 0, 1, 0, 0, 0];
    line.extend_from_slice(&[0; 16]);
    assert!(wkb::read(&line, 10).is_err());
    assert!(wkb::read(&testdata::point_wkb(0.0, 95.0), 10).is_err());
    let mut lying = vec![1, 3, 0, 0, 0, 1, 0, 0, 0];
    lying.extend_from_slice(&1_000_000_000u32.to_le_bytes());
    assert!(wkb::read(&lying, usize::MAX).is_err());
    assert!(wkb::read(&[], 10).is_err());
    // EWKB with an SRID and Z
    let mut ewkb = vec![1];
    ewkb.extend_from_slice(&(1u32 | 0x8000_0000 | 0x2000_0000).to_le_bytes());
    ewkb.extend_from_slice(&4326u32.to_le_bytes());
    for value in [3.0f64, 4.0, 100.0] {
        ewkb.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(wkb::read(&ewkb, 10), Ok(Geometry::Point([3.0, 4.0])));
}

/// The spec's fixture files (framework/test/fixtures/overture) are these tests' files: rewritten with UPDATE_GOLDEN=1.
#[test]
fn the_spec_fixtures_are_current() {
    let dir = moqui_core::context::framework_dir()
        .join("test")
        .join("fixtures")
        .join("overture");
    let files = [
        ("places-r1.parquet", testdata::spec_places_r1()),
        ("places-r2.parquet", testdata::spec_places_r2()),
        ("buildings-r1.parquet", testdata::spec_buildings_r1()),
        ("addresses-r1.parquet", testdata::spec_addresses_r1()),
        ("addresses-r2.parquet", testdata::spec_addresses_r2()),
    ];
    let update = std::env::var("UPDATE_GOLDEN").is_ok_and(|value| value == "1");
    if update {
        std::fs::create_dir_all(&dir).unwrap();
    }
    for (name, bytes) in files {
        let path = dir.join(name);
        if update {
            std::fs::write(&path, &bytes).unwrap();
        } else {
            let golden = std::fs::read(&path).unwrap_or_default();
            assert!(
                golden == bytes,
                "{} is stale: UPDATE_GOLDEN=1 cargo test -p moqui-maps",
                path.display()
            );
        }
    }
}

#[test]
fn a_row_group_claiming_more_rows_than_allowed_is_refused_before_it_is_read() {
    let bytes = testdata::spec_places_r1();
    let file = open(&bytes, Theme::Places, &Limits::standard()).unwrap();
    let tight = Limits {
        max_values: 2,
        ..Limits::standard()
    };
    let mut counts = Counts::default();
    let error = read_region(
        &bytes,
        &file,
        Theme::Places,
        &testdata::SPEC_REGION,
        &tight,
        &mut counts,
        &mut |_| Ok(true),
        &mut |_| true,
    )
    .unwrap_err();
    assert!(error.message().contains("claims 5 rows"), "{error}");
    let small_group = Limits {
        max_group_decoded: 64,
        ..Limits::standard()
    };
    let error = read_region(
        &bytes,
        &file,
        Theme::Places,
        &testdata::SPEC_REGION,
        &small_group,
        &mut Counts::default(),
        &mut |_| Ok(true),
        &mut |_| true,
    )
    .unwrap_err();
    assert!(error.message().contains("decodes to more than"), "{error}");
}
