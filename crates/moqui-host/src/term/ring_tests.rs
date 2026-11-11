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

use super::Ring;
use crate::term::track::Modes;

#[test]
fn a_client_inside_the_ring_goes_on_from_where_it_got() {
    let mut ring = Ring::new(16);
    ring.push(b"hello ", 0, &[]);
    ring.push(b"world", 6, &[]);
    assert_eq!(ring.since(6, 11).unwrap(), b"world");
    assert_eq!(ring.since(11, 11).unwrap(), b"");
    assert_eq!(ring.since(12, 11), None, "a count past the end");
}

#[test]
fn the_ring_keeps_its_last_bytes_and_a_client_behind_it_is_not_resumed() {
    let mut ring = Ring::new(8);
    ring.push(b"0123456789", 0, &[]);
    assert_eq!(ring.since(2, 10).unwrap(), b"23456789");
    assert_eq!(ring.since(1, 10), None, "dropped from the ring");
    ring.push(b"abc", 10, &[]);
    assert_eq!(ring.since(5, 13).unwrap(), b"56789abc");
    // a push larger than the whole ring keeps its own tail
    ring.push(b"ABCDEFGHIJKL", 13, &[]);
    assert_eq!(ring.since(17, 25).unwrap(), b"EFGHIJKL");
}

#[test]
fn a_replay_starts_at_the_oldest_rest_still_kept_with_its_modes() {
    let alternate = Modes {
        alternate: Some(1049),
        ..Modes::default()
    };
    let mut ring = Ring::new(10);
    // the stream starts at rest; a rest after "one\n", with the alternate screen on there
    ring.push(b"one\ntwo", 0, &[(4, alternate)]);
    let (from, bytes, modes) = ring.replay_from_rest(7, Modes::default());
    assert_eq!(
        (from, bytes.as_slice(), modes),
        (0, b"one\ntwo".as_slice(), Modes::default())
    );
    ring.push(b"three", 7, &[]);
    // the first rest (0) left the ring; the next is at 4
    let (from, bytes, modes) = ring.replay_from_rest(12, Modes::default());
    assert_eq!((from, bytes.as_slice(), modes), (4, b"twothree".as_slice(), alternate));
    ring.push(b"0123456789", 12, &[]);
    // no rest left in the ring: nothing to replay, the modes in force now
    let (from, bytes, modes) = ring.replay_from_rest(22, alternate);
    assert_eq!((from, bytes.len(), modes), (22, 0, alternate));
}
