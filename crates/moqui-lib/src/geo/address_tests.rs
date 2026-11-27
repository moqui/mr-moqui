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

#[test]
fn street_keys_meet_across_case_diacritics_abbreviations_and_particles() {
    // English, with directionals and a saint
    assert_eq!(street_key("HEMENWAY STREET", "US"), street_key("Hemenway St.", "US"));
    assert_eq!(street_key("Pennsylvania Avenue Northwest", "US"), "pennsylvania ave nw");
    assert_eq!(street_key("Saint Paul Ave", "US"), street_key("St Paul Avenue", "US"));
    // Italian: particles, abbreviations, the apostrophe and a saint
    assert_eq!(street_key("VIA DEI FOSSI", "IT"), "via fossi");
    assert_eq!(
        street_key("Via de' Tornabuoni", "IT"),
        street_key("Via Tornabuoni", "IT")
    );
    assert_eq!(
        street_key("P.za della Signoria", "IT"),
        street_key("PIAZZA DELLA SIGNORIA", "IT")
    );
    assert_eq!(
        street_key("VIA SANT'AGOSTINO", "IT"),
        street_key("Via S. Agostino", "IT")
    );
    assert_eq!(
        street_key("VIA GIOSUE' CARDUCCI", "IT"),
        street_key("Via Giosuè Carducci", "IT")
    );
    // French, German, Dutch, Spanish and Portuguese
    assert_eq!(
        street_key("Bd Saint-Germain", "FR"),
        street_key("Boulevard St Germain", "FR")
    );
    assert_eq!(street_key("Hauptstraße", "DE"), street_key("Hauptstr.", "DE"));
    assert_eq!(street_key("Marktplatz", "DE"), "marktpl");
    assert_eq!(street_key("Kalverstraat", "NL"), street_key("Kalverstr", "NL"));
    assert_eq!(
        street_key("Avda. de la Constitución", "ES"),
        street_key("Avenida Constitucion", "ES")
    );
    assert_eq!(street_key("R. Augusta", "PT"), street_key("Rua Augusta", "PT"));
    // a street of particles alone keeps them
    assert_eq!(street_key("Via Della", "IT"), "via");
    assert_eq!(street_key("De La", "ES"), "de la");
    // a country's languages decide: str is a street in the US, a Straße in Germany
    assert_eq!(street_key("Main Str", "US"), "main st");
    assert_eq!(street_key("Main Str", "DE"), "main str");
}

#[test]
fn numbers_units_and_postal_codes_have_one_key_each() {
    assert_eq!(number_key("12/A"), "12a");
    assert_eq!(number_key("12 a"), "12a");
    assert_eq!(number_key("51 rosso"), "51r");
    assert_eq!(number_key("189 1/2"), "1891/2");
    assert_eq!(number_base("12r").as_deref(), Some("12"));
    assert_eq!(number_base("12"), None);
    assert_eq!(unit_key("Apt 4B"), "4b");
    assert_eq!(unit_key("#4b"), "4b");
    assert_eq!(unit_key("Interno 3"), "3");
    assert_eq!(postcode_key("02115-1234", "US"), "02115");
    assert_eq!(postcode_key("1012 ab", "NL"), "1012AB");
    assert_eq!(postcode_key("50122", "IT"), "50122");
    assert_eq!(postcode_key("sw1a 1aa", "GB"), "SW1A1AA");
}

#[test]
fn a_us_address_is_read_with_its_unit_locality_and_zip() {
    let query = parse("373 Commonwealth Ave Apt 503, Boston, MA 02115", "US").unwrap();
    assert_eq!(query.number.as_deref(), Some("373"));
    assert_eq!(query.words, ["commonwealth", "ave"]);
    assert!(query.street_only);
    assert_eq!(query.unit.as_deref(), Some("503"));
    assert_eq!(query.postcode.as_deref(), Some("02115"));
    assert_eq!(query.locality.as_deref(), Some("boston"));
    let sign = parse("52 Hemenway St #2, Boston MA", "US").unwrap();
    assert_eq!(sign.unit.as_deref(), Some("2"));
    assert_eq!(sign.locality.as_deref(), Some("boston"));
}

#[test]
fn a_european_address_is_read_with_the_number_after_the_street() {
    let query = parse("Via dei Fossi 51, 50123 Firenze", "IT").unwrap();
    assert_eq!(query.number.as_deref(), Some("51"));
    assert_eq!(query.words, ["via", "dei", "fossi"]);
    assert_eq!(query.postcode.as_deref(), Some("50123"));
    assert_eq!(query.locality.as_deref(), Some("firenze"));
    // one part: the number is the house number, the code after it the postal code, the rest the locality
    let one = parse("Via dei Fossi 51 r 50123 Firenze", "IT").unwrap();
    assert_eq!(
        (one.number.as_deref(), one.postcode.as_deref()),
        (Some("51r"), Some("50123"))
    );
    assert_eq!(one.locality.as_deref(), Some("firenze"));
    assert_eq!(one.words, ["via", "dei", "fossi"]);
    let dutch = parse("Kalverstraat 1, 1012 NX Amsterdam", "NL").unwrap();
    assert_eq!(dutch.postcode.as_deref(), Some("1012 NX"));
    assert_eq!(dutch.locality.as_deref(), Some("amsterdam"));
    let french = parse("12 bis rue de Rivoli, 75001 Paris, France", "FR").unwrap();
    assert_eq!(french.number.as_deref(), Some("12bis"));
    assert_eq!(french.words, ["rue", "de", "rivoli"]);
    assert_eq!(french.locality.as_deref(), Some("paris"));
}

#[test]
fn a_text_of_one_part_tries_the_longest_street_first() {
    let query = parse("52 Hemenway Street Boston MA", "US").unwrap();
    assert!(!query.street_only);
    let streets = query.streets("US");
    assert_eq!(streets[0].0, "hemenway st boston ma");
    assert!(streets.contains(&("hemenway st".to_string(), Some("boston".to_string()))));
    assert_eq!(streets.last().unwrap().0, "hemenway");
    // no street at all, or nothing but a number
    assert_eq!(parse("02115", "US"), None);
    // words of other scripts (several bytes a character) never cut a character
    assert!(parse("12中4 Main St, 1中2345", "").is_some());
    assert!(parse("Main 1ж3ж, 12中4中6789, 12中4", "NL").is_some());
    assert_eq!(parse("  , ", "US"), None);
}

fn point(id: &str, number: &str, unit: &str, postcode: &str, locality: &str, lon: f64, lat: f64) -> Point {
    Point {
        address_id: id.to_string(),
        country: "US".to_string(),
        street: "HEMENWAY STREET".to_string(),
        number: number.to_string(),
        number_key: number_key(number),
        unit: unit.to_string(),
        unit_key: unit_key(unit),
        postcode: postcode.to_string(),
        postcode_key: postcode_key(postcode, "US"),
        locality: locality.to_string(),
        region: "MA".to_string(),
        places: vec!["ma".to_string(), fold(locality)],
        lon,
        lat,
        ..Point::default()
    }
}

#[test]
fn the_units_of_one_address_are_one_candidate_and_the_postal_code_decides() {
    let query = parse("52 Hemenway St, Boston, MA 02115", "US").unwrap();
    let points = vec![
        point("1", "52", "1", "02115", "BOSTON", -71.0892, 42.3450),
        point("2", "52", "", "02115", "BOSTON", -71.08921, 42.34501),
        point("3", "52", "2", "02115", "BOSTON", -71.08919, 42.34502),
        // the same number and street in another town and ZIP code: left out by the ZIP code
        point("4", "52", "", "02446", "BROOKLINE", -71.12, 42.34),
    ];
    let ranked = rank(&query, query.locality.as_deref(), Tier::Address, points, None);
    assert!(!ranked.ambiguous);
    assert_eq!(ranked.candidates.len(), 1);
    let best = &ranked.candidates[0];
    assert_eq!(best.point.address_id, "2", "the address before its units");
    assert_eq!(best.points, 3);
    assert_eq!(best.postcode_agrees, Some(true));
    assert_eq!(best.tier.precision(), "GPP_ADDRESS_MATCH");
    assert_eq!(label(&best.point, best.tier), "52 Hemenway Street, Boston MA 02115");
}

#[test]
fn points_of_one_number_far_apart_are_choices() {
    // Florence's black and red numbers share a number in the Italian data: two points 100 m apart, neither picked
    let query = parse("Via dei Fossi 51, Firenze", "IT").unwrap();
    let mut a = point("1", "51", "", "", "Firenze", 11.2487, 43.7721);
    let mut b = point("2", "51", "", "", "Firenze", 11.2497, 43.7728);
    for p in [&mut a, &mut b] {
        p.country = "IT".to_string();
        p.street = "VIA DEI FOSSI".to_string();
        p.places = vec!["toscana".to_string(), "firenze".to_string(), "firenze".to_string()];
    }
    let ranked = rank(
        &query,
        query.locality.as_deref(),
        Tier::Address,
        vec![a, b],
        Some((11.2490, 43.7722)),
    );
    assert!(ranked.ambiguous);
    assert_eq!(ranked.candidates.len(), 2);
    assert_eq!(
        ranked.candidates[0].point.address_id, "1",
        "the nearer to the view's middle first"
    );
    assert_eq!(
        label(&ranked.candidates[0].point, Tier::Address),
        "Via Dei Fossi 51, Firenze"
    );
}

#[test]
fn a_unit_asked_for_is_preferred_and_a_wrong_locality_ranks_below_the_margin() {
    let query = parse("52 Hemenway St Apt 2, Boston", "US").unwrap();
    let ranked = rank(
        &query,
        query.locality.as_deref(),
        Tier::Address,
        vec![
            point("1", "52", "1", "02115", "BOSTON", -71.0892, 42.3450),
            point("3", "52", "2", "02115", "BOSTON", -71.08919, 42.34502),
            point("9", "52", "2", "02446", "BROOKLINE", -71.12, 42.34),
        ],
        None,
    );
    assert_eq!(ranked.candidates.len(), 1, "{:?}", ranked.candidates);
    assert_eq!(ranked.candidates[0].point.address_id, "3");
    assert_eq!(ranked.candidates[0].unit_agrees, Some(true));
}

#[test]
fn a_street_without_the_number_is_an_estimate_by_locality() {
    let query = parse("999 Hemenway St, Boston", "US").unwrap();
    let ranked = rank(
        &query,
        query.locality.as_deref(),
        Tier::Street,
        vec![
            point("1", "10", "", "02115", "BOSTON", -71.090, 42.344),
            point("2", "80", "", "02115", "BOSTON", -71.088, 42.346),
        ],
        None,
    );
    assert_eq!(ranked.candidates.len(), 1);
    let street = &ranked.candidates[0];
    assert_eq!(street.tier.precision(), "GPP_PLACE_ESTIMATE");
    assert_eq!(street.tier.match_kind(), "street");
    assert_eq!(street.extent, Some([-71.090, 42.344, -71.088, 42.346]));
    assert!(
        [(-71.090, 42.344), (-71.088, 42.346)].contains(&(street.point.lon, street.point.lat)),
        "one of its own points"
    );
    assert_eq!(label(&street.point, Tier::Street), "Hemenway Street, Boston MA 02115");
}

#[test]
fn a_number_found_without_its_suffix_is_never_an_address_match() {
    let query = parse("Via dei Fossi 51r, Firenze", "IT").unwrap();
    assert_eq!(query.number.as_deref(), Some("51r"));
    let ranked = rank(
        &query,
        None,
        Tier::NumberBase,
        vec![point("1", "51", "", "", "Firenze", 11.2487, 43.7721)],
        None,
    );
    assert!(ranked.ambiguous, "an estimate is offered as a choice");
    assert_eq!(ranked.candidates[0].tier.precision(), "GPP_PLACE_ESTIMATE");
}

#[test]
fn display_title_cases_capitals_only() {
    assert_eq!(display("VIA DELL'ANGUILLARA"), "Via Dell'Anguillara");
    assert_eq!(display("McDonald Street"), "McDonald Street");
    assert_eq!(display("SAINT-GERMAIN"), "Saint-Germain");
}

#[test]
fn a_saints_ste_an_ordinal_and_a_number_within_a_street_name_are_not_misread() {
    // French: Ste is a saint, not a suite
    let french = parse("12 Rue Ste Catherine, Bordeaux", "FR").unwrap();
    assert_eq!(french.unit, None);
    assert_eq!(french.words, ["rue", "ste", "catherine"]);
    let canadian = parse("1200 Rue Ste Catherine, Montreal", "CA").unwrap();
    assert_eq!(canadian.unit, None);
    // English: Ste with a unit's name after it is a suite, a word is not
    assert_eq!(
        parse("100 Main St Ste 200, Boston", "US").unwrap().unit.as_deref(),
        Some("200")
    );
    assert_eq!(parse("100 Main St Ste Marie, Boston", "US").unwrap().unit, None);
    // ordinals name a street
    let fifth = parse("5th Avenue, New York", "US").unwrap();
    assert_eq!(fifth.number, None);
    assert_eq!(fifth.words, ["5th", "avenue"]);
    let numbered = parse("725 5th Ave, New York", "US").unwrap();
    assert_eq!(numbered.number.as_deref(), Some("725"));
    assert_eq!(numbered.words, ["5th", "ave"]);
    // a number within a street's name, where a comma ends the street, is the name's
    let piazza = parse("Piazza 1 Maggio, Firenze", "IT").unwrap();
    assert_eq!(piazza.number, None);
    assert_eq!(piazza.words, ["piazza", "1", "maggio"]);
    let via = parse("Via 4 Novembre 12, Firenze", "IT").unwrap();
    assert_eq!(via.number.as_deref(), Some("12"));
    assert_eq!(via.words, ["via", "4", "novembre"]);
}

#[test]
fn a_match_whose_postal_code_or_locality_differs_is_an_estimate_offered_as_a_choice() {
    let query = parse("52 Hemenway St, Prato, MA 02199", "US").unwrap();
    let ranked = rank(
        &query,
        query.locality.as_deref(),
        Tier::Address,
        vec![point("1", "52", "", "02115", "BOSTON", -71.0892, 42.3450)],
        None,
    );
    assert_eq!(ranked.candidates.len(), 1);
    let only = &ranked.candidates[0];
    assert_eq!((only.postcode_agrees, only.locality_agrees), (Some(false), Some(false)));
    assert_eq!(only.precision(), "GPP_PLACE_ESTIMATE");
    assert!(ranked.ambiguous, "a disagreeing match is a choice, never chosen");
}

#[test]
fn several_units_are_the_address_and_a_unit_not_there_is_not_named() {
    let mut first = point("1", "52", "1", "02115", "BOSTON", -71.0892, 42.3450);
    first.gers_id = "gers-1".to_string();
    let mut second = point("2", "52", "2", "02115", "BOSTON", -71.08921, 42.34501);
    second.gers_id = "gers-2".to_string();
    second.datasets = "other".to_string();
    let query = parse("52 Hemenway St, Boston", "US").unwrap();
    let ranked = rank(
        &query,
        query.locality.as_deref(),
        Tier::Address,
        vec![first.clone(), second.clone()],
        None,
    );
    let address = &ranked.candidates[0];
    assert_eq!((address.points, address.units), (2, 2));
    assert!(
        address.point.unit.is_empty() && address.point.gers_id.is_empty(),
        "{address:?}"
    );
    assert_eq!(
        label(&address.point, address.tier),
        "52 Hemenway Street, Boston MA 02115"
    );
    assert_eq!(address.point.datasets, "other", "the sources of all its points");
    // a unit asked for and there: that unit, its GERS ID
    let query = parse("52 Hemenway St #2, Boston", "US").unwrap();
    let ranked = rank(
        &query,
        query.locality.as_deref(),
        Tier::Address,
        vec![first.clone(), second.clone()],
        None,
    );
    assert_eq!(ranked.candidates[0].point.gers_id, "gers-2");
    assert_eq!(ranked.candidates[0].point.unit, "2");
    // a unit asked for and not there: the address, no unit named
    let query = parse("52 Hemenway St #9, Boston", "US").unwrap();
    let ranked = rank(&query, query.locality.as_deref(), Tier::Address, vec![first], None);
    assert_eq!(ranked.candidates[0].unit_agrees, Some(false));
    assert!(ranked.candidates[0].point.unit.is_empty());
}

#[test]
fn a_streets_estimate_is_its_point_nearest_the_middle() {
    let query = parse("999 Hemenway St, Boston", "US").unwrap();
    let ranked = rank(
        &query,
        query.locality.as_deref(),
        Tier::Street,
        vec![
            point("1", "10", "", "02115", "BOSTON", -71.090, 42.344),
            point("2", "40", "", "02115", "BOSTON", -71.0891, 42.3451),
            point("3", "80", "", "02115", "BOSTON", -71.088, 42.346),
        ],
        None,
    );
    let street = &ranked.candidates[0];
    assert_eq!((street.point.lon, street.point.lat), (-71.0891, 42.3451));
}
