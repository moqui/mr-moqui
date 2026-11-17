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

//! The Assist event streams' count per user (SEC-AI-06).

use super::*;

/// One user holds at most [`MAX_STREAMS_PER_USER`] streams of the node; another user still gets one, and a stream
/// that ends gives its user's slot back.
#[test]
fn one_user_holds_at_most_their_share_of_the_event_streams() {
    let mut held: Vec<Open> = (0..MAX_STREAMS_PER_USER)
        .map(|_| Open::take("sse-share-ann").expect("a stream within the user's share"))
        .collect();
    assert_eq!(Open::take("sse-share-ann").err(), Some(Full::User));
    let other = Open::take("sse-share-bob").expect("another user's stream");
    drop(other);
    held.pop();
    let again = Open::take("sse-share-ann").expect("a slot given back");
    drop(again);
    drop(held);
    assert!(lock(&STREAMS_BY_USER)
        .as_ref()
        .is_none_or(|by_user| !by_user.contains_key("sse-share-ann")));
}
