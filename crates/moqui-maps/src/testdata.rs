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

//! Test fixtures: GeoParquet files of Overture's schema 2.x written by the `parquet` crate (another implementation than
//! the reader under test), from places and buildings described here, in row groups, with the writer's settings each test
//! chooses; and the spec's fixture files, which are goldens of these (`UPDATE_GOLDEN=1 cargo test -p moqui-maps`).

use std::sync::Arc;

use parquet::basic::{Compression, Encoding};
use parquet::data_type::{BoolType, ByteArray, ByteArrayType, DoubleType, Int32Type, Int64Type};
use parquet::file::properties::{WriterProperties, WriterVersion};
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;
use parquet::schema::types::ColumnPath;

/// One leaf column's entries for a row group, in the schema's leaf order.
#[derive(Clone, Debug)]
pub enum Leaf {
    Bytes(Vec<Vec<u8>>, Vec<i16>, Vec<i16>),
    Double(Vec<f64>, Vec<i16>, Vec<i16>),
    Int32(Vec<i32>, Vec<i16>, Vec<i16>),
    Int64(Vec<i64>, Vec<i16>, Vec<i16>),
    Bool(Vec<bool>, Vec<i16>, Vec<i16>),
}

/// The writer's settings.
#[derive(Clone, Debug)]
pub struct Writer {
    pub version: WriterVersion,
    pub compression: Compression,
    pub dictionary: bool,
    /// An encoding by column path (`id`, `bbox.xmin`), the dictionary off for it.
    pub encodings: Vec<(&'static str, Encoding)>,
    /// The rows of a data page, so a chunk has several.
    pub page_rows: usize,
}

impl Default for Writer {
    fn default() -> Self {
        Self {
            version: WriterVersion::PARQUET_1_0,
            compression: Compression::UNCOMPRESSED,
            dictionary: true,
            encodings: Vec::new(),
            page_rows: 3,
        }
    }
}

/// A file of `schema` with each row group's leaves.
#[must_use]
pub fn write(schema: &str, groups: &[Vec<Leaf>], writer: &Writer) -> Vec<u8> {
    let schema = Arc::new(parse_message_type(schema).expect("the fixture schema parses"));
    let mut builder = WriterProperties::builder()
        .set_writer_version(writer.version)
        .set_compression(writer.compression)
        .set_dictionary_enabled(writer.dictionary)
        .set_data_page_row_count_limit(writer.page_rows)
        .set_write_batch_size(writer.page_rows)
        .set_created_by("moqui-maps fixtures".to_string());
    for (path, encoding) in &writer.encodings {
        let path = ColumnPath::from(path.split('.').map(str::to_string).collect::<Vec<_>>());
        builder = builder
            .set_column_dictionary_enabled(path.clone(), false)
            .set_column_encoding(path, *encoding);
    }
    let properties = Arc::new(builder.build());
    let mut out = Vec::new();
    let mut file = SerializedFileWriter::new(&mut out, schema, properties).expect("a fixture writer");
    for leaves in groups {
        let mut group = file.next_row_group().expect("a row group");
        let mut leaves = leaves.iter();
        while let Some(mut column) = group.next_column().expect("a column") {
            let leaf = leaves.next().expect("a leaf for each column of the schema");
            let opt = |levels: &Vec<i16>| if levels.is_empty() { None } else { Some(levels.clone()) };
            match leaf {
                Leaf::Bytes(values, def, rep) => {
                    let values: Vec<ByteArray> = values.iter().map(|value| ByteArray::from(value.clone())).collect();
                    column
                        .typed::<ByteArrayType>()
                        .write_batch(&values, opt(def).as_deref(), opt(rep).as_deref())
                }
                Leaf::Double(values, def, rep) => {
                    column
                        .typed::<DoubleType>()
                        .write_batch(values, opt(def).as_deref(), opt(rep).as_deref())
                }
                Leaf::Int32(values, def, rep) => {
                    column
                        .typed::<Int32Type>()
                        .write_batch(values, opt(def).as_deref(), opt(rep).as_deref())
                }
                Leaf::Int64(values, def, rep) => {
                    column
                        .typed::<Int64Type>()
                        .write_batch(values, opt(def).as_deref(), opt(rep).as_deref())
                }
                Leaf::Bool(values, def, rep) => {
                    column
                        .typed::<BoolType>()
                        .write_batch(values, opt(def).as_deref(), opt(rep).as_deref())
                }
            }
            .expect("the fixture values write");
            column.close().expect("a column closes");
        }
        group.close().expect("a row group closes");
    }
    file.close().expect("the fixture file closes");
    out
}

/// An optional scalar under `depth` optional fields (itself included): present at the full depth, else null at the
/// field just above it.
fn scalar<T: Clone>(values: &[Option<T>], depth: i16) -> (Vec<T>, Vec<i16>) {
    let mut out = Vec::new();
    let mut def = Vec::new();
    for value in values {
        match value {
            Some(value) => {
                out.push(value.clone());
                def.push(depth);
            }
            None => def.push(depth - 1),
        }
    }
    (out, def)
}

/// A field of the element struct of an optional list (`x.list.element.field`, max def 4): each row's elements.
fn list_field<T: Clone>(rows: &[Vec<Option<T>>]) -> (Vec<T>, Vec<i16>, Vec<i16>) {
    let (mut out, mut def, mut rep) = (Vec::new(), Vec::new(), Vec::new());
    for row in rows {
        if row.is_empty() {
            // the list there and empty
            def.push(1);
            rep.push(0);
            continue;
        }
        for (index, element) in row.iter().enumerate() {
            rep.push(i16::from(index > 0));
            match element {
                Some(value) => {
                    out.push(value.clone());
                    def.push(4);
                }
                None => def.push(3),
            }
        }
    }
    (out, def, rep)
}

fn bytes(values: &[Option<String>], depth: i16) -> Leaf {
    let (values, def) = scalar(values, depth);
    Leaf::Bytes(values.into_iter().map(String::into_bytes).collect(), def, Vec::new())
}

fn list_bytes(rows: &[Vec<Option<String>>]) -> Leaf {
    let (values, def, rep) = list_field(rows);
    Leaf::Bytes(values.into_iter().map(String::into_bytes).collect(), def, rep)
}

fn doubles(values: &[Option<f64>], depth: i16) -> Leaf {
    let (values, def) = scalar(values, depth);
    Leaf::Double(values, def, Vec::new())
}

fn int32s(values: &[Option<i32>]) -> Leaf {
    let (values, def) = scalar(values, 1);
    Leaf::Int32(values, def, Vec::new())
}

/// WKB of a point, little-endian.
#[must_use]
pub fn point_wkb(lon: f64, lat: f64) -> Vec<u8> {
    let mut out = vec![1, 1, 0, 0, 0];
    out.extend_from_slice(&lon.to_le_bytes());
    out.extend_from_slice(&lat.to_le_bytes());
    out
}

/// WKB of a polygon of one ring, big-endian (both orders are read).
#[must_use]
pub fn polygon_wkb(ring: &[[f64; 2]]) -> Vec<u8> {
    let mut out = vec![0];
    out.extend_from_slice(&3u32.to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&u32::try_from(ring.len()).expect("a small ring").to_be_bytes());
    for [lon, lat] in ring {
        out.extend_from_slice(&lon.to_be_bytes());
        out.extend_from_slice(&lat.to_be_bytes());
    }
    out
}

/// A place of a fixture.
#[derive(Clone, Debug)]
pub struct FixturePlace {
    pub id: Option<&'static str>,
    pub lon: f64,
    pub lat: f64,
    pub name: Option<&'static str>,
    pub category: Option<&'static str>,
    pub confidence: Option<f64>,
    /// Each source's dataset and license.
    pub sources: Vec<(&'static str, &'static str)>,
    pub address: Option<(&'static str, &'static str, &'static str)>,
    pub version: Option<i32>,
    /// The geometry's bytes in place of the point's (a row that does not read).
    pub geometry: Option<Vec<u8>>,
}

impl FixturePlace {
    #[must_use]
    pub fn new(id: &'static str, lon: f64, lat: f64, name: &'static str, category: &'static str) -> Self {
        Self {
            id: Some(id),
            lon,
            lat,
            name: Some(name),
            category: Some(category),
            confidence: Some(0.9),
            sources: vec![("meta", "CDLA-Permissive-2.0")],
            address: None,
            version: Some(1),
            geometry: None,
        }
    }
}

/// Overture places' schema 2.x, the columns the fixtures have.
pub const PLACES_SCHEMA: &str = "message schema {
  OPTIONAL BYTE_ARRAY id (STRING);
  OPTIONAL BYTE_ARRAY geometry;
  OPTIONAL DOUBLE confidence;
  OPTIONAL group names { OPTIONAL BYTE_ARRAY primary (STRING); }
  OPTIONAL group sources (LIST) { REPEATED group list { OPTIONAL group element {
    OPTIONAL BYTE_ARRAY dataset (STRING); OPTIONAL BYTE_ARRAY license (STRING); } } }
  OPTIONAL BYTE_ARRAY basic_category (STRING);
  OPTIONAL group taxonomy { OPTIONAL BYTE_ARRAY primary (STRING); }
  OPTIONAL group addresses (LIST) { REPEATED group list { OPTIONAL group element {
    OPTIONAL BYTE_ARRAY freeform (STRING); OPTIONAL BYTE_ARRAY locality (STRING); OPTIONAL BYTE_ARRAY country (STRING); } } }
  OPTIONAL INT32 version;
  OPTIONAL group bbox { OPTIONAL DOUBLE xmin; OPTIONAL DOUBLE xmax; OPTIONAL DOUBLE ymin; OPTIONAL DOUBLE ymax; }
}";

/// A row group of places in [`PLACES_SCHEMA`]'s leaf order.
#[must_use]
pub fn place_leaves(places: &[FixturePlace]) -> Vec<Leaf> {
    let text = |get: &dyn Fn(&FixturePlace) -> Option<&'static str>| -> Vec<Option<String>> {
        places.iter().map(|place| get(place).map(str::to_string)).collect()
    };
    let geometry: Vec<Option<Vec<u8>>> = places
        .iter()
        .map(|place| {
            Some(
                place
                    .geometry
                    .clone()
                    .unwrap_or_else(|| point_wkb(place.lon, place.lat)),
            )
        })
        .collect();
    let (geometry, geometry_def) = scalar(&geometry, 1);
    let sources = |pick: fn(&(&'static str, &'static str)) -> &'static str| -> Vec<Vec<Option<String>>> {
        places
            .iter()
            .map(|place| {
                place
                    .sources
                    .iter()
                    .map(|source| Some(pick(source).to_string()))
                    .collect()
            })
            .collect()
    };
    let address = |pick: fn(&(&'static str, &'static str, &'static str)) -> &'static str| -> Vec<Vec<Option<String>>> {
        places
            .iter()
            .map(|place| {
                place
                    .address
                    .iter()
                    .map(|address| Some(pick(address).to_string()))
                    .collect()
            })
            .collect()
    };
    let coordinate =
        |get: fn(&FixturePlace) -> f64| -> Vec<Option<f64>> { places.iter().map(|place| Some(get(place))).collect() };
    vec![
        bytes(&text(&|place| place.id), 1),
        Leaf::Bytes(geometry, geometry_def, Vec::new()),
        doubles(&places.iter().map(|place| place.confidence).collect::<Vec<_>>(), 1),
        bytes(&text(&|place| place.name), 2),
        list_bytes(&sources(|source| source.0)),
        list_bytes(&sources(|source| source.1)),
        bytes(&text(&|place| place.category), 1),
        bytes(&text(&|place| place.category), 2),
        list_bytes(&address(|address| address.0)),
        list_bytes(&address(|address| address.1)),
        list_bytes(&address(|address| address.2)),
        int32s(&places.iter().map(|place| place.version).collect::<Vec<_>>()),
        doubles(&coordinate(|place| place.lon), 2),
        doubles(&coordinate(|place| place.lon), 2),
        doubles(&coordinate(|place| place.lat), 2),
        doubles(&coordinate(|place| place.lat), 2),
    ]
}

/// A building of a fixture: an axis-aligned footprint.
#[derive(Clone, Debug)]
pub struct FixtureBuilding {
    pub id: &'static str,
    /// west, south, east, north
    pub footprint: [f64; 4],
    pub name: Option<&'static str>,
    pub class: Option<&'static str>,
    pub height: Option<f64>,
    pub floors: Option<i32>,
    pub sources: Vec<(&'static str, &'static str)>,
}

/// Overture buildings' schema 2.x, the columns the fixtures have.
pub const BUILDINGS_SCHEMA: &str = "message schema {
  OPTIONAL BYTE_ARRAY id (STRING);
  OPTIONAL group names { OPTIONAL BYTE_ARRAY primary (STRING); }
  OPTIONAL group sources (LIST) { REPEATED group list { OPTIONAL group element {
    OPTIONAL BYTE_ARRAY dataset (STRING); OPTIONAL BYTE_ARRAY license (STRING); } } }
  OPTIONAL DOUBLE height;
  OPTIONAL BOOLEAN is_underground;
  OPTIONAL INT32 num_floors;
  OPTIONAL BYTE_ARRAY class (STRING);
  OPTIONAL BYTE_ARRAY geometry;
  OPTIONAL INT32 version;
  OPTIONAL group bbox { OPTIONAL DOUBLE xmin; OPTIONAL DOUBLE xmax; OPTIONAL DOUBLE ymin; OPTIONAL DOUBLE ymax; }
}";

/// A row group of buildings in [`BUILDINGS_SCHEMA`]'s leaf order.
#[must_use]
pub fn building_leaves(buildings: &[FixtureBuilding]) -> Vec<Leaf> {
    let sources = |pick: fn(&(&'static str, &'static str)) -> &'static str| -> Vec<Vec<Option<String>>> {
        buildings
            .iter()
            .map(|building| {
                building
                    .sources
                    .iter()
                    .map(|source| Some(pick(source).to_string()))
                    .collect()
            })
            .collect()
    };
    let geometry: Vec<Vec<u8>> = buildings
        .iter()
        .map(|building| {
            let [w, s, e, n] = building.footprint;
            polygon_wkb(&[[w, s], [e, s], [e, n], [w, n], [w, s]])
        })
        .collect();
    let edge = |index: usize| -> Vec<Option<f64>> {
        buildings
            .iter()
            .map(|building| Some(building.footprint[index]))
            .collect()
    };
    let (underground, underground_def) = scalar(&vec![Some(false); buildings.len()], 1);
    vec![
        bytes(
            &buildings
                .iter()
                .map(|building| Some(building.id.to_string()))
                .collect::<Vec<_>>(),
            1,
        ),
        bytes(
            &buildings
                .iter()
                .map(|building| building.name.map(str::to_string))
                .collect::<Vec<_>>(),
            2,
        ),
        list_bytes(&sources(|source| source.0)),
        list_bytes(&sources(|source| source.1)),
        doubles(&buildings.iter().map(|building| building.height).collect::<Vec<_>>(), 1),
        Leaf::Bool(underground, underground_def, Vec::new()),
        int32s(&buildings.iter().map(|building| building.floors).collect::<Vec<_>>()),
        bytes(
            &buildings
                .iter()
                .map(|building| building.class.map(str::to_string))
                .collect::<Vec<_>>(),
            1,
        ),
        Leaf::Bytes(geometry, vec![1; buildings.len()], Vec::new()),
        int32s(&vec![Some(1); buildings.len()]),
        doubles(&edge(0), 2),
        doubles(&edge(2), 2),
        doubles(&edge(1), 2),
        doubles(&edge(3), 2),
    ]
}

/// The spec's region (central Florence): west, south, east, north.
pub const SPEC_REGION: [f64; 4] = [11.24, 43.76, 11.27, 43.78];

/// Places of release 1 in the spec's region (and a row group far away, which is the most of the file).
#[must_use]
pub fn spec_places_r1() -> Vec<u8> {
    let mut duomo = FixturePlace::new(
        "08f1e2a0-0000-4000-8000-000000000001",
        11.2560,
        43.7731,
        "Cattedrale di Santa Maria del Fiore",
        "religious_organization",
    );
    duomo.sources = vec![("meta", "CDLA-Permissive-2.0"), ("Foursquare", "Apache-2.0")];
    duomo.address = Some(("Piazza del Duomo", "Firenze", "IT"));
    let mut bar = FixturePlace::new(
        "08f1e2a0-0000-4000-8000-000000000002",
        11.2541,
        43.7702,
        "Caffè Gilli",
        "cafe",
    );
    bar.sources = vec![("AllThePlaces", "CC0-1.0")];
    let uffizi = FixturePlace::new(
        "08f1e2a0-0000-4000-8000-000000000003",
        11.2553,
        43.7678,
        "Galleria degli Uffizi",
        "museum",
    );
    let mut broken = FixturePlace::new(
        "08f1e2a0-0000-4000-8000-000000000004",
        11.2500,
        43.7700,
        "Not a point",
        "cafe",
    );
    broken.geometry = Some(vec![1, 2, 0, 0, 0]);
    let outside = FixturePlace::new(
        "08f1e2a0-0000-4000-8000-000000000005",
        11.3000,
        43.7700,
        "Outside the region",
        "cafe",
    );
    let far = FixturePlace::new(
        "08f1e2a0-0000-4000-8000-000000000006",
        12.4964,
        41.9028,
        "Colosseo",
        "landmark",
    );
    // the far group is large enough that the file is past the reader's first 64 KiB, so the region's group is read by
    // range, pinned to the file's ETag
    let mut rome = vec![far];
    for index in 0..1200u32 {
        let id: &'static str = Box::leak(format!("08f1e2a0-0000-4000-9000-{index:012}").into_boxed_str());
        let name: &'static str = Box::leak(format!("Roma {index}").into_boxed_str());
        rome.push(FixturePlace::new(id, 12.4 + f64::from(index) / 10_000.0, 41.9, name, "cafe"));
    }
    write(
        PLACES_SCHEMA,
        &[place_leaves(&[duomo, bar, uffizi, broken, outside]), place_leaves(&rome)],
        &Writer {
            page_rows: 500,
            ..Writer::default()
        },
    )
}

/// Places of release 2: one renamed, one new with a license the acknowledgement does not cover; written with v2
/// pages, delta encodings and Snappy.
#[must_use]
pub fn spec_places_r2() -> Vec<u8> {
    let mut duomo = FixturePlace::new(
        "08f1e2a0-0000-4000-8000-000000000001",
        11.2560,
        43.7731,
        "Duomo di Firenze",
        "religious_organization",
    );
    duomo.version = Some(2);
    let bar = FixturePlace::new(
        "08f1e2a0-0000-4000-8000-000000000002",
        11.2541,
        43.7702,
        "Caffè Gilli",
        "cafe",
    );
    let mut market = FixturePlace::new(
        "08f1e2a0-0000-4000-8000-000000000007",
        11.2537,
        43.7765,
        "Mercato Centrale",
        "market",
    );
    market.sources = vec![("example-dataset", "CC-BY-4.0")];
    write(
        PLACES_SCHEMA,
        &[place_leaves(&[duomo, bar, market])],
        &Writer {
            version: WriterVersion::PARQUET_2_0,
            compression: Compression::SNAPPY,
            dictionary: false,
            encodings: vec![
                ("id", Encoding::DELTA_BYTE_ARRAY),
                ("version", Encoding::DELTA_BINARY_PACKED),
            ],
            page_rows: 2,
        },
    )
}

/// Buildings of release 1 in the spec's region.
#[must_use]
pub fn spec_buildings_r1() -> Vec<u8> {
    let building = |id, footprint, name, class, height| FixtureBuilding {
        id,
        footprint,
        name,
        class,
        height,
        floors: Some(3),
        sources: vec![("OpenStreetMap", "ODbL-1.0")],
    };
    write(
        BUILDINGS_SCHEMA,
        &[vec![
            building(
                "b0000000-0000-4000-8000-000000000001",
                [11.2555, 43.7727, 11.2566, 43.7735],
                Some("Cattedrale"),
                Some("cathedral"),
                Some(90.0),
            ),
            building(
                "b0000000-0000-4000-8000-000000000002",
                [11.2549, 43.7674, 11.2558, 43.7684],
                Some("Uffizi"),
                Some("museum"),
                Some(25.0),
            ),
            building(
                "b0000000-0000-4000-8000-000000000003",
                [11.2600, 43.7700, 11.2603, 43.7703],
                None,
                Some("house"),
                None,
            ),
            building(
                "b0000000-0000-4000-8000-000000000004",
                [11.3100, 43.7700, 11.3103, 43.7703],
                None,
                Some("house"),
                None,
            ),
        ]]
        .into_iter()
        .map(|group| building_leaves(&group))
        .collect::<Vec<_>>(),
        &Writer::default(),
    )
}
