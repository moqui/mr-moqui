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
fn the_worker_idle_most_recently_is_woken_first_and_a_woken_one_leaves_once() {
    let (first, second) = (Arc::new(Condvar::new()), Arc::new(Condvar::new()));
    let mut idle = Idle::default();
    idle.enter(&first);
    idle.enter(&second);
    idle.wake_one();
    assert_eq!(idle.0.len(), 1);
    assert!(Arc::ptr_eq(&idle.0[0], &first), "the second, idle last, was woken");
    // the woken worker's leave finds nothing to take off; the other's takes it off
    idle.leave(&second);
    assert_eq!(idle.0.len(), 1);
    idle.leave(&first);
    assert!(idle.0.is_empty());
    idle.enter(&first);
    idle.enter(&second);
    idle.wake_all();
    assert!(idle.0.is_empty());
}
