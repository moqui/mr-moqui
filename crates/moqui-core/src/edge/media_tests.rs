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

use super::super::owner::Owner;
use super::{paired_audio_location, room_key, room_owner, split_room};

#[test]
fn a_room_key_keeps_the_servers_own_calls_bare_and_a_paired_nodes_apart() {
    assert_eq!(room_key(None, "100010"), "100010");
    assert_eq!(room_key(Some(""), "100010"), "100010");
    let paired = room_key(Some("local-1"), "100010");
    assert_ne!(paired, "100010");
    assert_ne!(paired, room_key(Some("local-2"), "100010"));
    assert_eq!(split_room(&paired), (Some("local-1"), "100010"));
    assert_eq!(split_room("100010"), (None, "100010"));
    assert_eq!(room_owner(&paired), Owner::paired("local-1"));
    assert_eq!(room_owner("100010"), Owner::Own);
}

#[test]
fn a_call_id_is_given_back_whole() {
    // the separator is the first one only: a call id is never split
    let key = room_key(Some("local-1"), "edge-fallback-1");
    assert_eq!(split_room(&key).1, "edge-fallback-1");
    assert_eq!(split_room("\u{1f}100010"), (None, "100010"));
}

#[test]
fn a_call_id_from_a_link_stays_its_nodes() {
    // a paired node's call id that holds the separator, naming another node's call or the server's own
    for call in ["local-2\u{1f}100010", "\u{1f}100010", "100010\u{1f}"] {
        let key = room_key(Some("local-1"), call);
        assert_eq!(room_owner(&key), Owner::paired("local-1"), "{call:?}");
        assert_eq!(split_room(&key), (Some("local-1"), call));
        assert_ne!(key, room_key(Some("local-2"), "100010"));
        assert_ne!(key, "100010");
    }
}

#[test]
fn a_paired_nodes_audio_is_kept_apart_from_the_servers() {
    let own = "dbresource://moqui/call/greeting/100010/greeting.ogg";
    let kept = paired_audio_location("local-1", own);
    assert_eq!(
        kept,
        "dbresource://moqui/edge-node/local-1/moqui/call/greeting/100010/greeting.ogg"
    );
    assert_ne!(
        paired_audio_location("local/2", own),
        paired_audio_location("local-2x", own)
    );
    assert!(paired_audio_location("../x", own).starts_with("dbresource://moqui/edge-node/___x/"));
}
