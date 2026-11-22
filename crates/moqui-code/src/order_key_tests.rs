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

use super::{key_between, OrderKeyError};

#[test]
fn keys_match_the_fractional_indexing_package() {
    // the examples of the fractional-indexing README and tests
    assert_eq!(key_between(None, None).unwrap(), "a0");
    assert_eq!(key_between(None, Some("a0")).unwrap(), "Zz");
    assert_eq!(key_between(Some("a0"), None).unwrap(), "a1");
    assert_eq!(key_between(Some("a0"), Some("a1")).unwrap(), "a0V");
    assert_eq!(key_between(Some("a1"), Some("a2")).unwrap(), "a1V");
    assert_eq!(key_between(Some("a0V"), Some("a1")).unwrap(), "a0l");
    assert_eq!(key_between(Some("Zz"), Some("a0")).unwrap(), "ZzV");
    assert_eq!(key_between(Some("Zz"), Some("a01")).unwrap(), "a0");
    assert_eq!(key_between(None, Some("a0V")).unwrap(), "a0");
    assert_eq!(key_between(Some("b125"), None).unwrap(), "b13");
    assert_eq!(key_between(Some("az"), None).unwrap(), "b00");
    assert_eq!(key_between(Some("a0"), Some("a0V")).unwrap(), "a0G");
    assert_eq!(key_between(Some("a0"), Some("a0G")).unwrap(), "a08");
    assert_eq!(key_between(Some("b125"), Some("b129")).unwrap(), "b127");
    assert_eq!(key_between(Some("a0"), Some("a1V")).unwrap(), "a1");
    assert_eq!(key_between(Some("Zz"), Some("a01")).unwrap(), "a0");
    assert_eq!(key_between(None, Some("Y00")).unwrap(), "Xzzz");
    assert_eq!(key_between(Some("bzzz"), None).unwrap(), "c000");
    assert_eq!(key_between(Some("a0"), Some("a0V")).unwrap(), "a0G");
}

#[test]
fn keys_stay_ordered_when_inserted_repeatedly() {
    let mut low = key_between(None, None).unwrap();
    let high = key_between(Some(&low), None).unwrap();
    for _ in 0..200 {
        let mid = key_between(Some(&low), Some(&high)).unwrap();
        assert!(low < mid && mid < high, "{low} < {mid} < {high}");
        low = mid;
    }
    let mut top = key_between(None, None).unwrap();
    for _ in 0..200 {
        let next = key_between(None, Some(&top)).unwrap();
        assert!(next < top);
        top = next;
    }
}

#[test]
fn bad_keys_are_refused() {
    assert_eq!(key_between(Some("a1"), Some("a0")), Err(OrderKeyError::NotOrdered));
    assert_eq!(key_between(Some("a1"), Some("a1")), Err(OrderKeyError::NotOrdered));
    assert!(matches!(key_between(Some("a00"), Some("a1")), Err(OrderKeyError::Invalid(_))));
    assert!(matches!(key_between(Some("a10"), None), Err(OrderKeyError::Invalid(_))));
    assert!(matches!(key_between(Some("0"), None), Err(OrderKeyError::Invalid(_))));
    assert!(matches!(key_between(Some("a!"), None), Err(OrderKeyError::Invalid(_))));
}
