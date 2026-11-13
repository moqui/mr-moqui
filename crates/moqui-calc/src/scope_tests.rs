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

use moqui_lib::object::Value;

use super::*;

/// The organizations of a filter in either order, and its terms in either order, are one scope; another organization is
/// another scope.
#[test]
fn a_condition_is_the_same_text_whatever_order_it_was_made_in() {
    let orgs = |ids: &[&str]| Condition::is_in("vendorPartyId", ids.iter().map(|id| Value::string(*id)).collect());
    let status = Condition::equals("statusId", Value::string("OrderPlaced"));
    let one = Condition::and(vec![orgs(&["ORG_A", "ORG_B"]), status.clone()]);
    let other_order = Condition::and(vec![status.clone(), orgs(&["ORG_B", "ORG_A", "ORG_A"])]);
    assert_eq!(canonical(&one), canonical(&other_order));

    let another_org = Condition::and(vec![orgs(&["ORG_A", "ORG_C"]), status.clone()]);
    assert_ne!(canonical(&one), canonical(&another_org));
    // and and or of the same parts differ, as do a value and its negation
    assert_ne!(
        canonical(&one),
        canonical(&Condition::or(vec![orgs(&["ORG_A", "ORG_B"]), status.clone()]))
    );
    assert_ne!(canonical(&status), canonical(&Condition::negate(status.clone())));
    // a filter that matches nothing is its own scope, not the open one
    assert_ne!(canonical(&Condition::where_sql("1 = 0")), "");
}
