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

//! The expiry worker's clock: how long it sleeps for a due time, and the soonest of the wakes.

use std::time::Duration;

use super::{sooner, wait_for, SWEEP};

#[test]
fn the_worker_sleeps_until_the_next_due_time_and_never_past_the_sweep() {
    // nothing due: a sweep
    assert_eq!(wait_for(1_000, None), SWEEP);
    // due in 1.5 s
    assert_eq!(wait_for(1_000, Some(2_500)), Duration::from_millis(1_500));
    // due already: at once
    assert_eq!(wait_for(5_000, Some(2_500)), Duration::ZERO);
    // due in an hour: the sweep, so another node's rows are seen
    assert_eq!(wait_for(0, Some(3_600_000)), SWEEP);
}

#[test]
fn a_wake_keeps_the_soonest_due_time() {
    assert_eq!(sooner(None, 9_000), 9_000);
    assert_eq!(sooner(Some(5_000), 9_000), 5_000);
    assert_eq!(sooner(Some(5_000), 0), 0);
}
