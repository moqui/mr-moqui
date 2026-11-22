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

use super::backoff_millis;

#[test]
fn backoff_doubles_and_stops_at_ten_minutes() {
    assert_eq!(backoff_millis(0), 5_000);
    assert_eq!(backoff_millis(1), 10_000);
    assert_eq!(backoff_millis(3), 40_000);
    assert_eq!(backoff_millis(7), 600_000);
    assert_eq!(backoff_millis(50), 600_000);
    assert_eq!(backoff_millis(-1), 5_000);
}
