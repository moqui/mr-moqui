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

use super::{artifact_id, location_of};

#[test]
fn a_location_names_its_artifact_and_the_name_it_is_sent_as() {
    assert_eq!(location_of("A1", "app.tar.gz"), "ci-artifact://A1/app.tar.gz");
    assert_eq!(location_of("A1", "a/b\\c"), "ci-artifact://A1/a_b_c");
    assert_eq!(location_of("A1", ""), "ci-artifact://A1");
    assert_eq!(artifact_id("ci-artifact://A1/app.tar.gz").unwrap(), "A1");
    assert_eq!(artifact_id("ci-artifact://A1").unwrap(), "A1");
    assert!(artifact_id("ci-artifact:///x").is_err());
    assert!(artifact_id("git://A1").is_err());
}
