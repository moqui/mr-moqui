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

use super::{entries, links, IDENTIFIER};

fn conf() -> serde_json::Value {
    serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap()
}

#[test]
fn the_identifier_name_and_schemes_are_those_of_tauri_conf() {
    let conf = conf();
    assert_eq!(conf["identifier"], IDENTIFIER);
    assert_eq!(conf["productName"], entries::PRODUCT_NAME);
    let schemes: Vec<&str> = conf["plugins"]["deep-link"]["desktop"]["schemes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|scheme| scheme.as_str().unwrap())
        .collect();
    assert_eq!(schemes, links::SCHEMES);
}
