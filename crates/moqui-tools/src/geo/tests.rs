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

use super::fetch::{copy_capped, expand_template, header, location_allowed, location_allowed_for, PROGRESS_BYTES};
use super::*;

#[test]
fn templates_fill_country_and_year() {
    assert_eq!(
        expand_template(
            "https://www2.census.gov/geo/docs/maps-data/data/gazetteer/{year}_Gazetteer/{year}_Gaz_zcta_national.zip",
            "",
            "2026"
        )
        .as_deref(),
        Ok("https://www2.census.gov/geo/docs/maps-data/data/gazetteer/2026_Gazetteer/2026_Gaz_zcta_national.zip")
    );
    assert_eq!(expand_template("{country}.txt", "US", "").as_deref(), Ok("US.txt"));
    // nothing but a code of that shape reaches the URL's path
    assert!(expand_template("{country}.zip", "../x", "").is_err());
    assert!(expand_template("{country}.zip", "", "").is_err());
    assert!(expand_template("{year}_x.zip", "", "26").is_err());
    assert!(expand_template("{year}_x.zip", "", "2026/../").is_err());
}

#[test]
fn a_copy_is_capped_and_hashed() {
    let mut out = Vec::new();
    let (bytes, checksum) = copy_capped(&mut &b"abc"[..], &mut out, 3, &mut |_| {}).unwrap();
    assert_eq!((bytes, out.as_slice()), (3, &b"abc"[..]));
    assert_eq!(
        checksum,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let error = copy_capped(&mut &b"abcd"[..], &mut Vec::new(), 3, &mut |_| {}).unwrap_err();
    assert!(error.message().contains("maps@import-max-bytes"), "{}", error.message());
    let big = vec![0_u8; (PROGRESS_BYTES * 2 + 1) as usize];
    let mut told = Vec::new();
    copy_capped(&mut big.as_slice(), &mut std::io::sink(), u64::MAX, &mut |so_far| {
        told.push(so_far)
    })
    .unwrap();
    assert_eq!(told.len(), 2, "{told:?}");
}

#[test]
fn headers_and_locations() {
    let headers = vec![
        ("etag".to_string(), " \"abc\" ".to_string()),
        ("Last-Modified".to_string(), String::new()),
    ];
    assert_eq!(header(&headers, "ETag").as_deref(), Some("\"abc\""));
    assert_eq!(header(&headers, "last-modified"), None);
    // the Maps import folder: only the caller's own folder of it
    assert!(!location_allowed("dbresource://moqui/maps/import/zips.csv"));
    assert!(location_allowed_for(
        "dbresource://moqui/maps/import/EX_JOHN_DOE/a/zips.csv",
        Some("EX_JOHN_DOE")
    ));
    assert!(!location_allowed_for(
        "dbresource://moqui/maps/import/OTHER/a/zips.csv",
        Some("EX_JOHN_DOE")
    ));
    assert!(!location_allowed_for(
        "dbresource://moqui/maps/import/EX_JOHN_DOE/",
        Some("EX_JOHN_DOE")
    ));
    assert!(location_allowed_for(
        "dbresource://moqui/database/upload/EX_JOHN_DOE/1/z.csv",
        Some("EX_JOHN_DOE")
    ));
    assert!(location_allowed("runtime://import/geo/zips.csv"));
    assert!(!location_allowed("dbresource://moqui/maps/import/"));
    assert!(!location_allowed("dbresource://moqui/maps/import/../etl/x.csv"));
    assert!(!location_allowed("component://framework/data/x.csv"));
    assert!(!location_allowed("file:/etc/passwd"));
    // with no one signed in, no upload folder is the caller's
    assert!(!location_allowed(
        "dbresource://moqui/database/upload/EX_JOHN_DOE/zips.csv"
    ));
}

#[test]
fn statuses_in_progress() {
    for status in [PENDING, DOWNLOADING, VALIDATING, LOADING] {
        assert!(in_progress(status), "{status}");
    }
    for status in [COMPLETED, UNCHANGED, FAILED, CANCELLED] {
        assert!(!in_progress(status), "{status}");
    }
}
