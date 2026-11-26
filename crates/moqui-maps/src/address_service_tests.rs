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

/// A Florence address of the Italian countrywide source.
fn address(
    id: &str,
    street: &str,
    number: &str,
    unit: Option<&str>,
    postcode: Option<&str>,
    lon: f64,
    lat: f64,
) -> AddressPoint {
    AddressPoint {
        gers_id: id.to_string(),
        version: Some(1),
        country: "IT".to_string(),
        street: Some(street.to_string()),
        number: Some(number.to_string()),
        unit: unit.map(str::to_string),
        postcode: postcode.map(str::to_string),
        postal_city: None,
        levels: ["Toscana", "Firenze", "Firenze"]
            .iter()
            .map(|level| (*level).to_string())
            .collect(),
        lon,
        lat,
        datasets: vec!["it/countrywide".to_string()],
        licenses: vec!["CC-BY-4.0".to_string()],
    }
}

fn florence() -> Vec<AddressPoint> {
    vec![
        address("a1", "VIA DEI FOSSI", "51", None, None, 11.2487, 43.7721),
        // the same number again 100 m away: a red and a black number the source wrote alike
        address("a2", "VIA DEI FOSSI", "51", None, None, 11.2497, 43.7728),
        address("a3", "VIA DEI FOSSI", "12", Some("A"), None, 11.2490, 43.7724),
        address(
            "a4",
            "PIAZZA DELLA SIGNORIA",
            "5",
            None,
            Some("50122"),
            11.2556,
            43.7696,
        ),
    ]
}

fn stage(facade: &EntityFacade, import_id: &str, points: &[AddressPoint], active: bool) {
    let mut run = facade.make_value(IMPORT).unwrap();
    run.set("importId", Value::string(import_id));
    run.set("regionId", Value::string("R1"));
    run.set("themeId", Value::string(THEME_ID));
    run.set("releaseVersion", Value::string("2026-11-18.0"));
    run.set("countryCodes", Value::string("IT"));
    run.create(facade).unwrap();
    let batch: Vec<(u64, AddressPoint, Keys)> = points
        .iter()
        .enumerate()
        .map(|(index, point)| (index as u64 + 1, point.clone(), Keys::of(point)))
        .collect();
    write_addresses(facade, import_id, &batch).unwrap();
    if active {
        let mut theme = facade.make_value(REGION_THEME).unwrap();
        theme.set("regionId", Value::string("R1"));
        theme.set("themeId", Value::string(THEME_ID));
        theme.set("activeImportId", Value::string(import_id));
        theme.create(facade).unwrap();
    }
}

fn find(facade: &EntityFacade, query: &str) -> Out {
    let mut parameters = BTreeMap::new();
    parameters.insert("query".to_string(), Value::string(query));
    search_overture_addresses(&parameters, facade).unwrap()
}

fn candidates(out: &Out) -> Vec<ValueMap> {
    match out.get("candidates") {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some(map.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn get(map: &ValueMap, key: &str) -> String {
    map.get(key).map(to_plain_string).unwrap_or_default()
}

#[test]
fn the_index_answers_exact_ambiguous_estimated_and_missing_addresses() {
    let facade = moqui_core::testing::open();
    assert_eq!(get_text(&find(&facade, "Via dei Fossi 51"), "reasonCode"), "NO_DATA");
    stage(&facade, "100", &florence(), true);

    let exact = find(&facade, "Piazza della Signoria 5, 50122 Firenze");
    let found = candidates(&exact);
    assert_eq!(found.len(), 1, "{exact:?}");
    assert_eq!(get(&found[0], "precisionEnumId"), "GPP_ADDRESS_MATCH");
    assert_eq!(get(&found[0], "label"), "Piazza Della Signoria 5, 50122 Firenze");
    assert_eq!(get(&found[0], "postcodeAgrees"), "true");
    assert_eq!(get(&found[0], "release"), "2026-11-18.0");
    assert_eq!(get(&found[0], "sourceLicenses"), "CC-BY-4.0");
    assert_eq!(get_text(&exact, "ambiguous"), "false");

    let both = find(&facade, "Via de' Fossi 51, Firenze");
    assert_eq!(candidates(&both).len(), 2, "{both:?}");
    assert_eq!(get_text(&both, "ambiguous"), "true");

    // Italy's 12/A: the letter the source keeps as the unit
    let letter = find(&facade, "Via dei Fossi 12/A");
    let found = candidates(&letter);
    assert_eq!(get(&found[0], "precisionEnumId"), "GPP_ADDRESS_MATCH", "{letter:?}");
    assert_eq!(get(&found[0], "label"), "Via Dei Fossi 12 #A, Firenze");
    let other = find(&facade, "Via dei Fossi 12/B");
    assert_eq!(get(&candidates(&other)[0], "tier"), "numberbase");
    let red = find(&facade, "Via dei Fossi 51 r");
    let found = candidates(&red);
    assert_eq!(get(&found[0], "tier"), "numberbase");
    assert_eq!(get(&found[0], "precisionEnumId"), "GPP_PLACE_ESTIMATE");

    let street = find(&facade, "Via dei Fossi 999, Firenze");
    let found = candidates(&street);
    assert_eq!(found.len(), 1);
    assert_eq!(get(&found[0], "matchKind"), "street");
    assert_eq!(get(&found[0], "precisionEnumId"), "GPP_PLACE_ESTIMATE");
    assert!(!get(&found[0], "extent").is_empty());

    let missing = find(&facade, "Via Roma 1, 50123 Firenze");
    assert_eq!(get_text(&missing, "reasonCode"), "NO_MATCH");
    // the parts read go back for the postal fallback
    match missing.get("parsed") {
        Some(Value::Map(parsed)) => assert_eq!(get(parsed, "postcode"), "50123"),
        other => panic!("no parsed parts: {other:?}"),
    }
    assert_eq!(get_text(&find(&facade, "50123"), "reasonCode"), "QUERY_UNREAD");
}

fn get_text(out: &Out, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

#[test]
fn a_refresh_is_compared_by_address_not_by_id() {
    let facade = moqui_core::testing::open();
    let old = florence();
    stage(&facade, "200", &old, false);
    let mut new = florence();
    // one moved 200 m, one removed, one new; IDs all changed, which counts for nothing
    new[2].lon += 0.0025;
    new.remove(3);
    new.push(address("b9", "VIA PARIONE", "48", None, None, 11.2499, 43.7707));
    for point in &mut new {
        point.gers_id.push_str("-r2");
    }
    let mut set = AddressSet::default();
    for point in &new {
        set.add(Keys::of(point).digest(&point.country), point.lon, point.lat);
    }
    let previous = read_set(&facade, "200").unwrap();
    // the two points of Via dei Fossi 51 are one address
    assert_eq!(previous.len(), 3);
    let changes = compare(&set, &previous, "200");
    assert_eq!(
        changes,
        Changes {
            compared_with: "200".to_string(),
            added: 1,
            removed: 1,
            moved: 1,
            unchanged: 1,
        }
    );
}
