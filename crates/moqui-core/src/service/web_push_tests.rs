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
fn a_push_service_answer_decides_the_subscription() {
    assert_eq!(outcome(Some(201)), Outcome::Delivered);
    assert_eq!(outcome(Some(200)), Outcome::Delivered);
    assert_eq!(outcome(Some(410)), Outcome::Gone);
    assert_eq!(outcome(Some(404)), Outcome::Gone);
    assert_eq!(outcome(Some(413)), Outcome::Failed);
    assert_eq!(outcome(Some(429)), Outcome::Failed);
    assert_eq!(outcome(Some(500)), Outcome::Failed);
    assert_eq!(outcome(None), Outcome::Failed);
}

#[test]
fn long_titles_are_cut_with_an_ellipsis() {
    assert_eq!(truncated("short", 10), "short");
    let cut = truncated(&"é".repeat(20), 10);
    assert_eq!(cut.chars().count(), 10);
    assert!(cut.ends_with('…'));
}

#[test]
fn an_endpoint_is_keyed_by_its_sha256() {
    assert_eq!(
        endpoint_hash("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
