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

use super::registers;

/// On a node that takes links a trunk registers only while this member holds the lock; elsewhere as its row says.
#[test]
fn a_trunk_registers_only_on_the_lock_holder_of_a_node_that_takes_links() {
    assert!(registers(true, true, true));
    assert!(!registers(true, true, false), "another member registers it");
    assert!(!registers(false, true, true), "its row says not to");
    assert!(registers(true, false, false), "a node that takes no links has no lock");
    assert!(!registers(false, false, false));
}

/// A holder whose claims fail keeps registering until two thirds of its lease passed since its last good claim, then
/// stops; one that did not hold it does not start.
#[test]
fn a_holder_whose_claims_fail_stops_before_its_lease_could_lapse() {
    use std::time::Duration;

    use super::holds_after;
    let lease = Duration::from_secs(30);
    assert!(holds_after(Some(true), false, Duration::ZERO, lease));
    assert!(
        !holds_after(Some(false), true, Duration::ZERO, lease),
        "another member holds it"
    );
    assert!(holds_after(None, true, Duration::from_secs(19), lease));
    assert!(
        !holds_after(None, true, Duration::from_secs(20), lease),
        "two thirds of the lease"
    );
    assert!(!holds_after(None, false, Duration::ZERO, lease), "never held");
}
