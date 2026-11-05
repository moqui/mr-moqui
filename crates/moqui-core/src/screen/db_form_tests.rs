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

fn parts(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The operators of a dependency: equals one of a list (any part of a multiple value), not-equals, empty, not-empty.
#[test]
fn conditions_hold_by_operator() {
    assert!(condition_holds("equals", "B,C", &parts(&["C"])));
    assert!(condition_holds("equals", "B", &parts(&["A", "B"])));
    assert!(!condition_holds("equals", "B", &parts(&["A"])));
    assert!(!condition_holds("equals", "B", &[]));
    assert!(condition_holds("not-equals", "B", &parts(&["A"])));
    assert!(!condition_holds("not-equals", "B, C", &parts(&["C"])));
    assert!(condition_holds("empty", "", &parts(&[""])));
    assert!(!condition_holds("empty", "", &parts(&["x"])));
    assert!(condition_holds("not-empty", "", &parts(&["x"])));
}
