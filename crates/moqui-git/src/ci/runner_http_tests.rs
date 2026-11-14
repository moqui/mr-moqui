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

use super::{Window, MAX_COUNTED};

/// A job's or runner's requests are counted per minute, all counts starting again when the minute turns; past
/// MAX_COUNTED keys a new key is not counted that minute (no sweep of the map on each call), and a key already counted
/// still is.
#[test]
fn the_runner_rate_window_counts_per_minute_and_caps_its_keys() {
    let mut window = Window::default();
    assert!(!window.count("job-1", 7, 2));
    assert!(!window.count("job-1", 7, 2));
    assert!(window.count("job-1", 7, 2), "the third in a minute is over 2");
    assert!(!window.count("job-1", 8, 2), "a new minute starts again");
    for index in 0..MAX_COUNTED - 1 {
        window.count(&format!("k{index}"), 8, 2);
    }
    assert_eq!(window.counts.len(), MAX_COUNTED);
    for _ in 0..5 {
        assert!(!window.count("one-too-many", 8, 2), "not counted");
    }
    assert!(!window.counts.contains_key("one-too-many"));
    assert!(window.count("job-1", 8, 1), "a counted key still is");
    assert!(!window.count("one-too-many", 9, 2));
    assert_eq!(window.counts.len(), 1);
}
