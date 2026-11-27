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

/// A row of `overture-addresses.json` as OF1's MapPackages.js writes it: the server's keys of the point beside its text.
fn row(number: &str, street: &str, postcode: &str, locality: &str, lon: f64, lat: f64) -> Value {
    json!({
        "gersId": format!("g-{number}-{street}"), "countryCode": "US", "streetName": street,
        "streetNorm": address::street_key(street, "US"), "houseNumber": number, "numberNorm": address::number_key(number),
        "unitText": null, "unitNorm": "", "postcode": postcode, "postcodeNorm": address::postcode_key(postcode, "US"),
        "postalCity": locality, "locality": locality, "region": "MA", "levelsText": "", "latitude": lat, "longitude": lon,
        "positionKind": null, "sourceDatasets": "OpenAddresses", "sourceLicenses": "CC0"
    })
}

fn rows() -> Vec<Value> {
    vec![
        row("12", "Hemenway Street", "02115", "Boston", -71.0894, 42.3445),
        row("14", "Hemenway Street", "02115", "Boston", -71.0896, 42.3447),
        row("3", "Boylston Street", "02116", "Boston", -71.07, 42.35),
        // a row with no point, one with its degrees as text, and one with fields of the wrong kind
        json!({ "countryCode": "US", "streetName": "No point", "streetNorm": "no point" }),
        json!({ "countryCode": "US", "streetName": "Text Street", "streetNorm": address::street_key("Text Street", "US"),
            "houseNumber": 7, "numberNorm": "7", "latitude": "42.3", "longitude": "-71.1" }),
        json!({ "countryCode": ["US"], "streetName": {"a": 1}, "streetNorm": true, "latitude": 1, "longitude": 1 }),
        // not a row at all: left out, the rest read
        json!("not a row"),
        json!(["not", "a", "row"]),
    ]
}

fn search(rows: &[Value], text: &str, country: &str) -> Value {
    search_rows(&json!({ "rows": rows }).to_string(), text, country, None, 5).unwrap()
}

#[test]
fn a_packages_addresses_are_matched_as_the_server_matches_them() {
    let rows = rows();
    // a number and its street, written another way: an address match
    let found = search(&rows, "12 Hemenway St, Boston", "");
    assert_eq!(found["valid"], true, "{found}");
    let first = &found["candidates"][0];
    assert_eq!(first["tier"], "address");
    assert_eq!(first["precisionEnumId"], "GPP_ADDRESS_MATCH");
    assert_eq!(first["number"], "12");
    assert!((first["lon"].as_f64().unwrap() + 71.0894).abs() < 1e-9);
    assert_eq!(first["sourceLicenses"], "CC0");
    // a number and degrees written as text are read
    assert_eq!(search(&rows, "7 Text Street", "US")["candidates"][0]["tier"], "address");
    // a number the street does not have: the street's estimate, with its points' extent
    let street = search(&rows, "99 Hemenway Street", "US");
    assert_eq!(street["candidates"][0]["tier"], "street", "{street}");
    assert_eq!(street["candidates"][0]["precisionEnumId"], "GPP_PLACE_ESTIMATE");
    assert_eq!(street["candidates"][0]["number"], "");
    // no such street; no street at all
    assert_eq!(search(&rows, "5 Nowhere Lane", "")["reasonCode"], "NO_MATCH");
    assert_eq!(search(&rows, "", "")["reasonCode"], "QUERY_UNREAD");
    assert_eq!(search(&[], "12 Hemenway St", "")["valid"], false);
    assert!(search_rows("not json", "12 Hemenway St", "", None, 5).is_err());
    // a field past its length is cut
    let long = json!({ "countryCode": "US", "streetName": "x".repeat(1000), "streetNorm": "long st", "latitude": 1, "longitude": 1 });
    let member: Member = serde_json::from_str(&json!({ "rows": [long] }).to_string()).unwrap();
    assert_eq!(member.rows[0].street_name.chars().count(), FIELD_CHARS);
}

/// A package installed with its addresses member, as a download leaves it.
fn install(packages_dir: &Path, revision: i64, rows: &[Value]) -> PathBuf {
    let dir = packages_dir.join("data/a.packages/P1").join(revision.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let member = json!({ "rows": rows }).to_string();
    std::fs::write(dir.join(ADDRESSES_MEMBER), &member).unwrap();
    let sha = format!("{:064x}", revision);
    let manifest = json!({ "package": { "packageId": "P1", "revision": revision },
        "members": [{ "path": ADDRESSES_MEMBER, "size": member.len(), "sha256": sha }] });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    dir
}

#[test]
fn a_packages_address_index_is_kept_for_its_revision_and_dropped_on_a_switch_or_delete() {
    let dir = std::env::temp_dir().join(format!("moqui-client-addresses-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let packages = Packages::new(&dir);
    assert_eq!(
        packages.search_addresses("a", "P1", "12 Main St", "", None, 5)["reasonCode"],
        "NO_DATA"
    );
    let first = install(&dir, 3, &rows());
    assert_eq!(
        packages.search_addresses("a", "P1", "12 Hemenway St", "", None, 5)["valid"],
        true
    );
    assert!(packages.address_index_kept());
    // kept: the member removed from under it is not read again for the same revision
    std::fs::write(first.join(ADDRESSES_MEMBER), "{}").unwrap();
    assert_eq!(
        packages.search_addresses("a", "P1", "12 Hemenway St", "", None, 5)["valid"],
        true
    );
    // another account's search never reads this one's index, and a switch drops it
    assert_eq!(
        packages.search_addresses("b", "P1", "12 Hemenway St", "", None, 5)["reasonCode"],
        "NO_DATA"
    );
    packages.drop_address_index(None);
    assert!(!packages.address_index_kept());
    // a member that cannot be read is said once for its revision, not read again on every search
    packages.drop_address_index(None);
    std::fs::write(first.join(ADDRESSES_MEMBER), "not json").unwrap();
    assert_eq!(
        packages.search_addresses("a", "P1", "12 Hemenway St", "", None, 5)["reasonCode"],
        "NO_DATA"
    );
    std::fs::write(first.join(ADDRESSES_MEMBER), json!({ "rows": rows() }).to_string()).unwrap();
    assert_eq!(
        packages.search_addresses("a", "P1", "12 Hemenway St", "", None, 5)["reasonCode"],
        "NO_DATA",
        "remembered for the revision"
    );
    packages.drop_address_index(None);
    // a new revision installed is read as it is
    install(&dir, 4, &rows()[2..3]);
    assert_eq!(
        packages.search_addresses("a", "P1", "12 Hemenway St", "", None, 5)["reasonCode"],
        "NO_MATCH"
    );
    assert_eq!(
        packages.search_addresses("a", "P1", "3 Boylston St", "", None, 5)["valid"],
        true
    );
    packages.delete("a", "P1").unwrap();
    assert!(!packages.address_index_kept(), "a delete drops it");
    let _ = std::fs::remove_dir_all(&dir);
}
