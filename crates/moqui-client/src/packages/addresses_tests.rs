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
        "sourceDatasets": "OpenAddresses", "sourceLicenses": "CC0"
    })
}

#[test]
fn a_packages_addresses_are_matched_as_the_server_matches_them() {
    let rows = vec![
        row("12", "Hemenway Street", "02115", "Boston", -71.0894, 42.3445),
        row("14", "Hemenway Street", "02115", "Boston", -71.0896, 42.3447),
        row("3", "Boylston Street", "02116", "Boston", -71.07, 42.35),
        json!({ "countryCode": "US", "streetName": "No point", "streetNorm": "no point" }),
        json!("not a row"),
    ];
    // a number and its street, written another way: an address match
    let found = search_rows(&rows, "", "12 Hemenway St, Boston", "", None, 5);
    assert_eq!(found["valid"], true, "{found}");
    let first = &found["candidates"][0];
    assert_eq!(first["tier"], "address");
    assert_eq!(first["precisionEnumId"], "GPP_ADDRESS_MATCH");
    assert_eq!(first["number"], "12");
    assert!((first["lon"].as_f64().unwrap() + 71.0894).abs() < 1e-9);
    assert_eq!(first["sourceLicenses"], "CC0");
    // a number the street does not have: the street's estimate, with its points' extent
    let street = search_rows(&rows, "", "99 Hemenway Street", "US", None, 5);
    assert_eq!(street["candidates"][0]["tier"], "street", "{street}");
    assert_eq!(street["candidates"][0]["precisionEnumId"], "GPP_PLACE_ESTIMATE");
    assert_eq!(street["candidates"][0]["number"], "");
    // no such street; no street at all
    assert_eq!(
        search_rows(&rows, "", "5 Nowhere Lane", "", None, 5)["reasonCode"],
        "NO_MATCH"
    );
    assert_eq!(search_rows(&rows, "", "", "", None, 5)["reasonCode"], "QUERY_UNREAD");
    assert_eq!(search_rows(&[], "", "12 Hemenway St", "", None, 5)["valid"], false);
}

#[test]
fn a_package_with_no_addresses_says_so() {
    let dir = std::env::temp_dir().join(format!("moqui-client-addresses-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let packages = Packages::new(&dir);
    let answer = packages.search_addresses("a", "P1", "12 Main St", "", None, 5);
    assert_eq!(answer["reasonCode"], "NO_DATA");
    let _ = std::fs::remove_dir_all(&dir);
}
