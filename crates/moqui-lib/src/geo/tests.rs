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

use std::io::{Cursor, Write};

use super::census::{largest_share, DivisionRecord};
use super::geonames::{Admin1Record, CityRecord};
use super::ids;
use super::postal::PostalRecord;
use super::text::{read_source, Limits};
use super::*;

/// Every record and rejection of `text` read as `format`, through the reader as the importer reads a bare file.
fn read(format: Format, text: &[u8]) -> (Vec<Record>, Vec<RowError>, u64) {
    read_with(format, text, None, &Limits::default()).expect("the file reads")
}

fn read_with(
    format: Format,
    bytes: &[u8],
    member: Option<&str>,
    limits: &Limits,
) -> Result<(Vec<Record>, Vec<RowError>, u64), crate::error::Error> {
    let mut records = Vec::new();
    let mut rejected = Vec::new();
    let rows = read_source(Cursor::new(bytes.to_vec()), "test", member, limits, |reader| {
        parse(format, reader, &mut |row| match row {
            Ok(record) => records.push(record),
            Err(error) => rejected.push(error),
        })
    })?;
    Ok((records, rejected, rows))
}

fn postal(records: &[Record]) -> Vec<&PostalRecord> {
    records
        .iter()
        .filter_map(|record| match record {
            Record::Postal(postal) => Some(postal),
            _ => None,
        })
        .collect()
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

const ZCTA: &str = "GEOID|ALAND|AWATER|ALAND_SQMI|AWATER_SQMI|INTPTLAT|INTPTLONG\n\
00601|166836392|799292|64.416|0.309|18.180555|-66.749961\n\
02134|2335958|158963|0.902|0.061|42.357579|-71.128389\n\
2135|1|1|0|0|42.35|-71.15\n\
02136|1|1|0|0|91.5|-71.1\n\
02138|1|1|0|0|42.38|abc\n\
\n\
99999|1|1|0|0|0|0\n";

#[test]
fn census_zcta_keeps_leading_zeros_and_passes_over_a_bad_row() {
    let (records, rejected, rows) = read(Format::CensusZcta, ZCTA.as_bytes());
    let codes: Vec<&str> = postal(&records).iter().map(|r| r.code.normalized.as_str()).collect();
    assert_eq!(codes, ["00601", "02134", "99999"]);
    assert_eq!(rows, 6, "the blank line is not a row");
    let first = postal(&records)[0];
    assert_eq!(first.country, "US");
    assert_eq!(first.code.granularity, Granularity::Full);
    assert_eq!(first.precision, Precision::PostalArea);
    assert_eq!(
        first.point,
        Some(Point {
            latitude: 18.180555,
            longitude: -66.749961
        })
    );
    // zero is a position like any other
    assert_eq!(
        postal(&records)[2].point,
        Some(Point {
            latitude: 0.0,
            longitude: 0.0
        })
    );
    let fields: Vec<(u64, &str)> = rejected.iter().map(|e| (e.line, e.field.as_str())).collect();
    assert_eq!(fields, [(4, "GEOID"), (5, "latitude"), (6, "longitude")]);
    assert!(rejected[0].message.contains("five-digit"), "{}", rejected[0].message);
    assert!(rejected[0].text.starts_with("2135|"));
}

#[test]
fn census_tab_delimited_older_years_read_by_the_same_header() {
    let text = "GEOID\tALAND\tINTPTLAT\tINTPTLONG                 \n01001\t1\t42.06\t-72.61\n";
    let (records, rejected, _) = read(Format::CensusZcta, text.as_bytes());
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(postal(&records)[0].code.normalized, "01001");
}

#[test]
fn census_header_missing_a_column_refuses_the_file() {
    let error = read_with(
        Format::CensusZcta,
        b"GEOID|INTPTLAT\n02134|42.3\n",
        None,
        &Limits::default(),
    )
    .unwrap_err();
    assert!(error.message().contains("INTPTLONG"), "{}", error.message());
}

#[test]
fn census_counties_and_places() {
    let text = "USPS|GEOID|ANSICODE|NAME|ALAND|AWATER|ALAND_SQMI|AWATER_SQMI|INTPTLAT|INTPTLONG\n\
MA|25025|00606939|Suffolk County|1|1|1|1|42.331960|-71.020173\n\
M1|25017|0|Middlesex County|1|1|1|1|42.48|-71.39\n\
MA|25021|0||1|1|1|1|42.17|-71.18\n";
    let (records, rejected, rows) = read(Format::CensusCounties, text.as_bytes());
    assert_eq!(rows, 3);
    assert_eq!(
        records,
        [Record::County(DivisionRecord {
            line: 2,
            state: "MA".to_string(),
            geoid: "25025".to_string(),
            name: "Suffolk County".to_string(),
            point: Point {
                latitude: 42.33196,
                longitude: -71.020173
            },
        })]
    );
    let fields: Vec<&str> = rejected.iter().map(|e| e.field.as_str()).collect();
    assert_eq!(fields, ["USPS", "NAME"]);
    let (places, _, _) = read(Format::CensusPlaces, text.as_bytes());
    assert!(matches!(&places[0], Record::Place(place) if place.geoid == "25025"));
}

#[test]
fn census_zcta_county_with_a_byte_order_mark_picks_the_largest_land_share() {
    let header = "\u{feff}OID_ZCTA5_20|GEOID_ZCTA5_20|NAMELSAD_ZCTA5_20|AREALAND_ZCTA5_20|AREAWATER_ZCTA5_20|\
MTFCC_ZCTA5_20|CLASSFP_ZCTA5_20|FUNCSTAT_ZCTA5_20|OID_COUNTY_20|GEOID_COUNTY_20|NAMELSAD_COUNTY_20|\
AREALAND_COUNTY_20|AREAWATER_COUNTY_20|MTFCC_COUNTY_20|CLASSFP_COUNTY_20|FUNCSTAT_COUNTY_20|AREALAND_PART|AREAWATER_PART";
    let row = |zcta: &str, county: &str, land: &str| {
        format!("1|{zcta}|ZCTA5 {zcta}|1|1|G6350|B5|S|2|{county}|Some County|1|1|G4020|H1|A|{land}|0")
    };
    let text = [
        header.to_string(),
        row("02134", "25017", "100"),
        row("02134", "25025", "900"),
        row("02134", "25021", "900"),
        row("", "25001", "5000"),
        row("01001", "25013", "abc"),
        row("01002", "25015", "10"),
    ]
    .join("\r\n");
    let (records, rejected, rows) = read(Format::CensusZctaCounty, text.as_bytes());
    assert_eq!(rows, 6);
    assert_eq!(rejected.len(), 1, "{rejected:?}");
    assert_eq!(rejected[0].field, "AREALAND_PART");
    let rows: Vec<_> = records
        .into_iter()
        .filter_map(|record| match record {
            Record::ZctaCounty(row) => Some(row),
            _ => None,
        })
        .collect();
    assert_eq!(rows.len(), 4, "the row with no ZCTA is left out");
    let best: Vec<(&str, &str)> = largest_share(&rows)
        .iter()
        .map(|row| (row.zcta.as_str(), row.county.as_str()))
        .collect();
    assert_eq!(
        best,
        [("01002", "25015"), ("02134", "25025")],
        "the first among equal shares"
    );
}

const GEONAMES_US: &str = "US\t02134\tAllston\tMassachusetts\tMA\tSuffolk\t025\t\t\t42.3539\t-71.1337\t4\n\
US\t02134\tBoston\tMassachusetts\tMA\tSuffolk\t025\t\t\t42.358\t-71.06\t6\n\
US\t00501\tHoltsville\tNew York\tNY\tSuffolk\t103\t\t\t40.8154\t-73.0451\t\n\
US\t1234\tShort\tX\tXX\t\t\t\t\t40\t-73\t1\n\
US\t02135\tBrighton \"Square\"\tMassachusetts\tMA\tSuffolk\t025\t\t\t42.3484\t-71.1528\t4\n\
US\t02136\tToo few\n";

#[test]
fn geonames_postal_rows_accuracy_and_precision() {
    let (records, rejected, rows) = read(Format::GeoNamesPostal, GEONAMES_US.as_bytes());
    assert_eq!(rows, 6);
    let records = postal(&records);
    let codes: Vec<&str> = records.iter().map(|r| r.code.normalized.as_str()).collect();
    assert_eq!(codes, ["02134", "02134", "00501", "02135"]);
    assert_eq!(records[0].accuracy, Some(4));
    assert_eq!(records[0].precision, Precision::PlaceEstimate);
    assert_eq!(records[1].precision, Precision::PostalArea);
    assert_eq!(records[2].accuracy, None);
    assert_eq!(records[1].admin_code1.as_deref(), Some("MA"));
    assert_eq!(records[1].admin_name2.as_deref(), Some("Suffolk"));
    assert_eq!(
        records[3].place_name.as_deref(),
        Some("Brighton \"Square\""),
        "no quoting"
    );
    let fields: Vec<(u64, &str)> = rejected.iter().map(|e| (e.line, e.field.as_str())).collect();
    assert_eq!(fields, [(4, "postalCode"), (6, "")]);
    // the representative: highest accuracy, then first in the file
    assert_eq!(representative_index(&[Some(4), Some(6), Some(6)]), Some(1));
    assert_eq!(representative_index(&[None, Some(1), Some(4), Some(1)]), Some(2));
    assert_eq!(representative_index(&[None, None]), Some(0));
    assert_eq!(representative_index(&[]), None);
}

#[test]
fn geonames_partial_code_countries_are_prefix_codes() {
    let text = "CA\tT0A\tEastern Alberta\tAlberta\tAB\t\t\t\t\t54.766\t-111.7174\t6\n\
GB\tSW1A\tLondon\tEngland\tENG\t\t\t\t\t51.5\t-0.14\t4\n\
GB\tSW1A 1AA\tLondon\tEngland\tENG\t\t\t\t\t51.501\t-0.1416\t6\n\
NL\t1012\tAmsterdam\tNoord-Holland\t07\t\t\t\t\t52.37\t4.89\t6\n\
NL\t1012 AB\tAmsterdam\tNoord-Holland\t07\t\t\t\t\t52.37\t4.89\t6\n\
CA\tK1A 0B1\tOttawa\tOntario\tON\t\t\t\t\t45.4\t-75.7\t6\n";
    let (records, rejected, _) = read(Format::GeoNamesPostal, text.as_bytes());
    assert!(rejected.is_empty(), "{rejected:?}");
    let summary: Vec<(&str, &str, Granularity, Precision)> = postal(&records)
        .iter()
        .map(|r| {
            (
                r.code.normalized.as_str(),
                r.code.display.as_str(),
                r.code.granularity,
                r.precision,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("T0A", "T0A", Granularity::Prefix, Precision::PlaceEstimate),
            ("SW1A", "SW1A", Granularity::Prefix, Precision::PlaceEstimate),
            ("SW1A1AA", "SW1A 1AA", Granularity::Full, Precision::PostalArea),
            ("1012", "1012", Granularity::Prefix, Precision::PlaceEstimate),
            ("1012AB", "1012 AB", Granularity::Full, Precision::PostalArea),
            ("K1A0B1", "K1A 0B1", Granularity::Full, Precision::PostalArea),
        ]
    );
}

#[test]
fn geonames_gazetteer_pieces() {
    let countries = "#ISO\tISO3\tISO-Numeric\tfips\tCountry\tCapital\tArea\tPopulation\tContinent\ttld\tCurrencyCode\tCurrencyName\tPhone\tPostal Code Format\tPostal Code Regex\tLanguages\tgeonameid\tneighbours\tEquivalentFipsCode\n\
# a comment\n\
US\tUSA\t840\tUS\tUnited States\tWashington\t9629091\t327167434\tNA\t.us\tUSD\tDollar\t1\t#####-####\t^\\d{5}(-\\d{4})?$\ten-US\t6252001\tCA,MX,CU\t\n\
CA\tCAN\t124\tCA\tCanada\tOttawa\t9984670\t37058856\tNA\t.ca\tCAD\tDollar\t1\t@#@ #@#\t^([ABCEGHJKLMNPRSTVXY]\\d[ABCEGHJKLMNPRSTV-Z]) ?(\\d[ABCEGHJKLMNPRSTV-Z]\\d)$ \ten-CA,fr-CA\t6251999\tUS\t\n\
XX\tXX\t1\n";
    let (records, rejected, rows) = read(Format::GeoNamesCountries, countries.as_bytes());
    assert_eq!(rows, 3, "comment lines are not rows");
    assert_eq!(rejected.len(), 1);
    let Record::Country(us) = &records[0] else {
        panic!("{records:?}")
    };
    assert_eq!(
        (us.iso2.as_str(), us.iso3.as_str(), us.numeric.as_str()),
        ("US", "USA", "840")
    );
    assert_eq!(us.postal_format.as_deref(), Some("#####-####"));
    assert_eq!(us.geonameid.as_deref(), Some("6252001"));

    let admin1 = "US.MA\tMassachusetts\tMassachusetts\t6254926\nCA.01\tAlberta\tAlberta\t5883102\nbad\tName\n";
    let (records, rejected, _) = read(Format::GeoNamesAdmin1, admin1.as_bytes());
    assert_eq!(rejected.len(), 1);
    assert_eq!(
        records[1],
        Record::Admin1(Admin1Record {
            line: 2,
            country: "CA".to_string(),
            code: "01".to_string(),
            name: "Alberta".to_string(),
            ascii_name: Some("Alberta".to_string()),
            geonameid: Some("5883102".to_string()),
        })
    );

    let admin2 = "US.MA.025\tSuffolk County\tSuffolk County\t4952349\nUS.MA\tNo county\t\t1\n";
    let (records, rejected, _) = read(Format::GeoNamesAdmin2, admin2.as_bytes());
    assert_eq!(rejected.len(), 1);
    assert!(matches!(&records[0], Record::Admin2(r) if r.admin1 == "MA" && r.code == "025"));

    let city = |id: &str, lat: &str| {
        format!("{id}\tBoston\tBoston\tBeantown,Boston\t{lat}\t-71.05977\tP\tPPLA\tUS\t\tMA\t025\t\t\t653833\t14\t38\tAmerica/New_York\t2024-01-01")
    };
    let cities = [city("4930956", "42.35843"), city("4930957", "NaN"), city("x", "1")].join("\n");
    let (records, rejected, _) = read(Format::GeoNamesCities, cities.as_bytes());
    let fields: Vec<&str> = rejected.iter().map(|e| e.field.as_str()).collect();
    assert_eq!(fields, ["latitude", "geonameid"], "NaN is refused");
    assert_eq!(
        records,
        [Record::City(CityRecord {
            line: 1,
            geonameid: "4930956".to_string(),
            name: "Boston".to_string(),
            ascii_name: Some("Boston".to_string()),
            point: Point {
                latitude: 42.35843,
                longitude: -71.05977
            },
            feature_code: Some("PPLA".to_string()),
            country: "US".to_string(),
            admin1: Some("MA".to_string()),
            admin2: Some("025".to_string()),
            population: Some(653833),
        })]
    );
}

#[test]
fn operator_csv_by_header_name() {
    let text = "\u{feff}Code,Country,Place,Admin1,Admin2,Latitude,Longitude,Precision\n\
02134,US,\"Allston, MA\",MA,025,42.35,-71.13,\n\
K1A 0B1,CAN,Ottawa,ON,,45.4,-75.7,address\n\
T0A,CA,Alberta,AB,,,,\n\
02135,US,Brighton,MA,,42.3,-71.1,user\n\
02136,US,Hyde Park,MA,,,-71.1,\n";
    let (records, rejected, rows) = read(Format::OperatorCsv, text.as_bytes());
    assert_eq!(rows, 5);
    let records = postal(&records);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].place_name.as_deref(), Some("Allston, MA"));
    assert_eq!(records[0].admin_code2.as_deref(), Some("025"));
    assert_eq!(records[0].precision, Precision::PostalArea);
    assert_eq!(records[1].country, "CAN");
    assert_eq!(records[1].code.normalized, "K1A0B1");
    assert_eq!(records[1].precision, Precision::AddressMatch);
    assert_eq!(records[2].code.granularity, Granularity::Prefix);
    assert_eq!(records[2].point, None, "a code with no point");
    assert_eq!(records[2].precision, Precision::PlaceEstimate);
    let fields: Vec<(u64, &str)> = rejected.iter().map(|e| (e.line, e.field.as_str())).collect();
    assert_eq!(fields, [(5, "precision"), (6, "latitude")]);
    let error = read_with(
        Format::OperatorCsv,
        b"country,code\nUS,02134\n",
        None,
        &Limits::default(),
    )
    .unwrap_err();
    assert!(error.message().contains("latitude, longitude"), "{}", error.message());
}

#[test]
fn a_zip_member_reads_and_a_missing_one_refuses_the_file() {
    let zip = zip_of(&[("readme.txt", b"not this one"), ("US.txt", GEONAMES_US.as_bytes())]);
    let (records, rejected, _) = read_with(Format::GeoNamesPostal, &zip, Some("US.txt"), &Limits::default()).unwrap();
    assert_eq!(postal(&records).len(), 4);
    assert_eq!(rejected.len(), 2);
    let error = read_with(Format::GeoNamesPostal, &zip, Some("CA.txt"), &Limits::default()).unwrap_err();
    assert!(error.message().contains("no member CA.txt"), "{}", error.message());
    let error = read_with(Format::GeoNamesPostal, &zip, None, &Limits::default()).unwrap_err();
    assert!(error.message().contains("more than one file"), "{}", error.message());
    // a zip of one file needs no member named
    let one = zip_of(&[("2026_Gaz_zcta_national.txt", ZCTA.as_bytes())]);
    let (records, _, _) = read_with(Format::CensusZcta, &one, None, &Limits::default()).unwrap();
    assert_eq!(postal(&records).len(), 3);
}

#[test]
fn a_zip_bomb_and_an_oversized_file_are_refused() {
    let zeros = vec![b'0'; 4 << 20];
    let bomb = zip_of(&[("US.txt", &zeros)]);
    assert!(bomb.len() < 64 << 10, "deflate packs the zeros small: {}", bomb.len());
    let error = read_with(Format::GeoNamesPostal, &bomb, Some("US.txt"), &Limits::default()).unwrap_err();
    assert!(error.message().contains("past the ratio"), "{}", error.message());
    let small = Limits {
        max_bytes: 1 << 20,
        ..Limits::default()
    };
    let error = read_with(Format::GeoNamesPostal, &bomb, Some("US.txt"), &small).unwrap_err();
    assert!(
        error.message().contains("more than the 1048576 allowed"),
        "{}",
        error.message()
    );
    let entries = Limits {
        max_entries: 1,
        ..Limits::default()
    };
    let two = zip_of(&[("a.txt", b"a"), ("b.txt", b"b")]);
    let error = read_with(Format::GeoNamesPostal, &two, Some("a.txt"), &entries).unwrap_err();
    assert!(error.message().contains("entries"), "{}", error.message());
    // a bare file past the limit ends the read: the file is not whole
    let tiny = Limits {
        max_bytes: 100,
        ..Limits::default()
    };
    let error = read_with(Format::GeoNamesPostal, GEONAMES_US.as_bytes(), None, &tiny).unwrap_err();
    assert!(format!("{error:?}").contains("larger than the 100 bytes"), "{error:?}");
}

#[test]
fn a_long_or_non_utf8_line_is_a_rejected_row() {
    let mut bytes = GEONAMES_US.lines().next().unwrap().as_bytes().to_vec();
    bytes.extend_from_slice(b"\nUS\t02139\tCambridge\xff\tMA\tMA\t\t\t\t\t42.36\t-71.1\t4\n");
    bytes.extend(std::iter::repeat_n(b'x', text::MAX_LINE + 10));
    bytes.extend_from_slice(b"\nUS\t02140\tCambridge\tMassachusetts\tMA\t\t\t\t\t42.39\t-71.13\t4\n");
    let (records, rejected, rows) = read(Format::GeoNamesPostal, &bytes);
    assert_eq!(rows, 4);
    assert_eq!(postal(&records).len(), 2);
    let messages: Vec<&str> = rejected.iter().map(|e| e.message.as_str()).collect();
    assert!(
        messages[0].contains("UTF-8") && messages[1].contains("longer"),
        "{messages:?}"
    );
}

#[test]
fn postal_codes_by_country() {
    let code = |country: &str, raw: &str| normalize_postal_code(country, raw);
    let us = code("US", " 02134 ").unwrap();
    assert_eq!(
        (us.normalized.as_str(), us.display.as_str(), us.granularity),
        ("02134", "02134", Granularity::Full)
    );
    assert!(code("USA", "2134").unwrap_err().contains("five-digit"));
    assert!(
        code("US", "02134-1234").unwrap_err().contains("ZIP+4"),
        "a ZIP+4 is not cut to its ZIP code"
    );
    assert_eq!(code("CA", "k1a0b1").unwrap().display, "K1A 0B1");
    assert!(code("CA", "K1A 0B").is_err());
    assert_eq!(code("GB", "ec1a1bb").unwrap().display, "EC1A 1BB");
    assert_eq!(code("GB", "M1 1AE").unwrap().normalized, "M11AE");
    assert_eq!(code("GB", "GIR 0AA").unwrap().granularity, Granularity::Full);
    assert_eq!(code("GB", "EC1A").unwrap().granularity, Granularity::Prefix);
    assert!(code("GB", "1AB 2CD").is_err());
    assert_eq!(code("NLD", "1012ab").unwrap().display, "1012 AB");
    // any other country: taken whole, its spelling kept, hyphens and spaces out of the normalized form
    let other = code("DE", "01067").unwrap();
    assert_eq!(
        (other.normalized.as_str(), other.granularity),
        ("01067", Granularity::Full)
    );
    let pt = code("PT", "1000-001").unwrap();
    assert_eq!((pt.normalized.as_str(), pt.display.as_str()), ("1000001", "1000-001"));
    assert!(code("DE", "").is_err());
    assert!(code("DE", "10/115").is_err());
    assert!(code("DE", "123456789012345678901").unwrap_err().contains("longer"));
}

#[test]
fn coordinates_and_precision() {
    assert_eq!(
        parse_point("0", "0"),
        Ok(Point {
            latitude: 0.0,
            longitude: 0.0
        })
    );
    assert_eq!(parse_point("-90", "180").unwrap().longitude, 180.0);
    assert_eq!(parse_point("90.0001", "0").unwrap_err().0, "latitude");
    assert_eq!(parse_point("0", "-180.5").unwrap_err().0, "longitude");
    assert!(parse_point("NaN", "0").is_err());
    assert!(parse_point("inf", "0").is_err());
    assert!(parse_point("", "0").is_err());
    assert_eq!(parse_optional_point(" ", ""), Ok(None));
    assert_eq!(parse_optional_point("1", "").unwrap_err().0, "longitude");
    assert_eq!(geonames_precision(Some(6)), Precision::PostalArea);
    assert_eq!(geonames_precision(Some(4)), Precision::PlaceEstimate);
    assert_eq!(geonames_precision(Some(1)), Precision::PlaceEstimate);
    assert_eq!(geonames_precision(None), Precision::PlaceEstimate);
    assert_eq!(Precision::parse("GPP_STREET_RANGE"), Ok(Precision::StreetRange));
    assert!(Precision::parse("GPP_USER_CONFIRMED").is_err());
    assert_eq!(
        Format::of("GDP_CENSUS", "GDY_POSTAL_COUNTY"),
        Some(Format::CensusZctaCounty)
    );
    assert_eq!(Format::of("GDP_GEONAMES", "GDY_PLACES"), Some(Format::GeoNamesCities));
    assert_eq!(Format::of("GDP_OPERATOR_CSV", ""), Some(Format::OperatorCsv));
    assert_eq!(Format::of("GDP_CENSUS", "GDY_COUNTRIES"), None);
}

#[test]
fn derived_ids() {
    let us = normalize_postal_code("US", "02134").unwrap();
    assert_eq!(ids::postal_code_geo_id("USA", &us).unwrap(), "USA_Z_02134");
    let fsa = normalize_postal_code("CA", "K1A").unwrap();
    assert_eq!(ids::postal_code_geo_id("CAN", &fsa).unwrap(), "CAN_ZP_K1A");
    assert_eq!(ids::county_geo_id("25025").unwrap(), "USA_C_25025");
    assert!(ids::county_geo_id("2502").is_err());
    assert_eq!(ids::place_geo_id("2507000").unwrap(), "USA_P_2507000");
    assert_eq!(ids::city_geo_id("USA", "4930956").unwrap(), "USA_G_4930956");
    assert_eq!(ids::admin1_geo_id("CAN", "01").unwrap(), "CAN_A1_01");
    assert_eq!(ids::admin2_geo_id("USA", "MA", "025").unwrap(), "USA_A2_MA_025");
    assert!(ids::admin1_geo_id("CAN", "0.1").is_err());
    assert_eq!(
        ids::point_id("USA_Z_02134", "CENSUS_ZCTA").unwrap(),
        "USA_Z_02134_CENSUS_ZCTA"
    );
    assert_eq!(
        ids::place_point_id("USA_Z_02134", "GN_POSTAL", 2).unwrap(),
        "USA_Z_02134_GN_POSTAL_2"
    );
    // a code long enough makes an ID past 40: refused, never cut
    let long = normalize_postal_code("DE", "ABCDEFGHIJ1234567890").unwrap();
    let geo_id = ids::postal_code_geo_id("DEU", &long).unwrap();
    assert_eq!(geo_id.len(), 26);
    assert!(ids::point_id(&geo_id, "GN_POSTAL").is_ok());
    assert!(ids::place_point_id(&geo_id, "OPERATOR_CSV", 10)
        .unwrap_err()
        .contains("longer than 40"));
    assert!(ids::is_computed("USA_Z_02134"));
    assert!(ids::is_computed("CAN_A1_01"));
    assert!(!ids::is_computed("USA_CA"));
    assert!(!ids::is_computed("USA"));
}

#[test]
fn a_rejected_row_names_its_code_where_it_reads() {
    let (_, rejected, _) = read(Format::CensusZcta, ZCTA.as_bytes());
    let keys: Vec<Option<&str>> = rejected
        .iter()
        .map(|e| match e.key.as_deref() {
            Some(RowKey::Postal { code, .. }) => Some(code.normalized.as_str()),
            _ => None,
        })
        .collect();
    // 2135 is no code; 02136 and 02138 are, passed over for a coordinate
    assert_eq!(keys, [None, Some("02136"), Some("02138")]);
    let mut long = "US\t02141\tCambridge\tMassachusetts\tMA\t\t\t\t\t42.37\t-71.08\t".to_string();
    long.push_str(&"9".repeat(text::MAX_LINE + 10));
    let text = format!("US\t02142\tCambridge\tMA\tMA\t\t\t\t\t42.36\tbad\t4\n{long}\n");
    let (_, rejected, _) = read(Format::GeoNamesPostal, text.as_bytes());
    let codes: Vec<String> = rejected
        .iter()
        .filter_map(|e| match e.key.as_deref() {
            Some(RowKey::Postal { country, code }) => Some(format!("{country}:{}", code.normalized)),
            _ => None,
        })
        .collect();
    assert_eq!(codes, ["US:02142", "US:02141"], "a line too long still names its code");
    assert!(rejected[1].text.len() <= MAX_ROW_TEXT, "the kept text is bounded");
    let cities = "1\tA\tA\t\tNaN\t1\tP\tPPL\tUS\t\tMA\t\t\t\t1\t\t\t\t\n";
    let (_, rejected, _) = read(Format::GeoNamesCities, cities.as_bytes());
    assert_eq!(
        rejected[0].key.as_deref(),
        Some(&RowKey::City {
            country: "US".to_string(),
            geonameid: "1".to_string()
        })
    );
    let counties = "USPS|GEOID|NAME|INTPTLAT|INTPTLONG\nMA|25025|Suffolk|x|1\n";
    let (_, rejected, _) = read(Format::CensusCounties, counties.as_bytes());
    assert_eq!(rejected[0].key.as_deref(), Some(&RowKey::Fips("25025".to_string())));
}

#[test]
fn an_unclosed_quote_spoils_its_own_operator_row_only() {
    let text = "country,code,place,latitude,longitude\n\
US,02134,\"Allston,42.35,-71.13\n\
US,02135,Brighton,42.3,-71.1\n\
US,02136,Hyde Park,42.25,x\n";
    let (records, rejected, rows) = read(Format::OperatorCsv, text.as_bytes());
    assert_eq!(rows, 3);
    let codes: Vec<&str> = postal(&records).iter().map(|r| r.code.normalized.as_str()).collect();
    assert!(codes.contains(&"02135"), "{codes:?} {rejected:?}");
    assert!(
        matches!(rejected.last().unwrap().key.as_deref(), Some(RowKey::Postal { code, .. }) if code.normalized == "02136")
    );
}

/// Set the declared uncompressed size of a one-entry zip's local header and central directory entry.
fn patch_sizes(zip: &mut [u8], size: u32) {
    let local = 0;
    let central = zip
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .expect("a central directory entry");
    zip[local + 22..local + 26].copy_from_slice(&size.to_le_bytes());
    zip[central + 24..central + 28].copy_from_slice(&size.to_le_bytes());
}

#[test]
fn a_member_whose_declared_size_lies_is_refused() {
    let zeros = vec![b'0'; 4 << 20];
    let mut lying = zip_of(&[("US.txt", &zeros)]);
    patch_sizes(&mut lying, 100);
    let result = read_with(Format::GeoNamesPostal, &lying, Some("US.txt"), &Limits::default());
    // the crate either stops at the declared size and fails its check, or our ratio bound ends the read
    let error = result.expect_err("a lying member is not read whole");
    assert!(!error.message().is_empty());
}
