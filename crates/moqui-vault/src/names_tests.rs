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
fn names_without_a_label_never_encode_as_labelled_ones() {
    let label = ItemLabel {
        item_type: "t",
        binding: "b",
        name: "n",
    };
    let labelled = Names::new(Purpose::ItemKey, "v", "i", 1).with_label(Some(label));
    let plain = Names::new(Purpose::ItemKey, "v", "i", 1);
    // An extra that spells out a label, with or without the old marker word.
    assert_ne!(plain.encode(&[b"t", b"b", b"n"]), labelled.encode(&[]));
    assert_ne!(plain.encode(&[b"operational", b"t", b"b", b"n"]), labelled.encode(&[]));
    assert_ne!(plain.encode(&[&[1], b"t", b"b", b"n"]), labelled.encode(&[]));
    assert_eq!(plain.parts(&[])[5], vec![0]);
    assert_eq!(labelled.parts(&[])[5], vec![1]);
}
