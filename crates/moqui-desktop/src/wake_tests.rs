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

use std::time::Duration;

use super::slept;

const GAP: Duration = Duration::from_secs(60);

#[test]
fn a_wall_clock_far_ahead_of_the_monotonic_one_is_a_sleep() {
    let slept_for = slept(Some(Duration::from_secs(3630)), Duration::from_secs(30), GAP);
    assert_eq!(slept_for, Some(Duration::from_secs(3600)));
}

#[test]
fn a_tick_in_step_or_a_clock_set_back_is_no_sleep() {
    assert_eq!(slept(Some(Duration::from_secs(31)), Duration::from_secs(30), GAP), None);
    assert_eq!(slept(Some(Duration::from_secs(90)), Duration::from_secs(30), GAP), None);
    assert_eq!(slept(Some(Duration::from_secs(10)), Duration::from_secs(30), GAP), None);
    assert_eq!(slept(None, Duration::from_secs(30), GAP), None);
}
