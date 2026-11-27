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

use yrs::types::xml::{XmlElementPrelim, XmlFragment as _};
use yrs::updates::decoder::Decode as _;
use yrs::{Doc, Map as _, MapPrelim, ReadTxn as _, StateVector, Transact as _, Update, WriteTxn as _};

use super::{Nested, Nesting, Shape, MAX_TYPE_DEPTH};

const PAGE: Shape = Shape {
    roots: &[crate::markdown::FRAGMENT],
    nested: Nested::Xml,
};
const BOARD: Shape = Shape {
    roots: &["elements", "files", "app"],
    nested: Nested::None,
};

fn whole(doc: &Doc) -> Vec<u8> {
    doc.transact().encode_state_as_update_v1(&StateVector::default())
}

/// What an editor's change of `doc` sends: the update from the state before `change` to after it.
fn change(doc: &Doc, change: impl FnOnce(&mut yrs::TransactionMut)) -> Vec<u8> {
    let before = doc.transact().state_vector();
    let mut txn = doc.transact_mut();
    change(&mut txn);
    drop(txn);
    doc.transact().encode_state_as_update_v1(&before)
}

fn apply(doc: &Doc, update: &[u8]) {
    doc.transact_mut()
        .apply_update(Update::decode_v1(update).unwrap())
        .unwrap();
}

/// A page of `levels` XML elements, each the only child of the one before.
fn chain(levels: usize) -> Vec<u8> {
    let doc = Doc::new();
    let fragment = doc.get_or_insert_xml_fragment(crate::markdown::FRAGMENT);
    let mut txn = doc.transact_mut();
    let mut parent = fragment.push_back(&mut txn, XmlElementPrelim::empty("blockquote"));
    for _ in 1..levels {
        parent = parent.push_back(&mut txn, XmlElementPrelim::empty("blockquote"));
    }
    drop(txn);
    whole(&doc)
}

/// SEC-TLS-118: an update that nests shared types past the bound is refused before anything of it is applied, so the
/// delete that would overflow the stack never has the levels to walk (y-crdt issue #749: 40,000 nested types); one
/// within the bound, and a page as the editor writes it, are taken.
#[test]
fn an_update_that_nests_types_too_deep_is_refused() {
    let server = Doc::new();
    let deep = chain(40_000);
    let (status, why) = Nesting::default().check(&server, PAGE, &deep).unwrap_err();
    assert_eq!(status, 422);
    assert!(why.contains(&MAX_TYPE_DEPTH.to_string()), "{why}");
    assert!(Nesting::default()
        .check(&server, PAGE, &chain(MAX_TYPE_DEPTH as usize + 1))
        .is_err());
    Nesting::default()
        .check(&server, PAGE, &chain(MAX_TYPE_DEPTH as usize))
        .unwrap();

    let page = Doc::new();
    crate::markdown::seed(
        &page,
        "# Title\n\n> quote\n> > nested\n\n- a\n  - b\n    - c\n\n| x | y |\n| - | - |\n| 1 | 2 |\n\ntext *b* **c**\n",
    );
    Nesting::default().check(&server, PAGE, &whole(&page)).unwrap();
}

/// The levels may come one update at a time: each is placed under the one before, by the depths kept from the updates
/// taken, and the one past the bound is refused.
#[test]
fn levels_added_one_update_at_a_time_stop_at_the_bound() {
    let editor = Doc::new();
    let server = Doc::new();
    let mut nesting = Nesting::default();
    let fragment = editor.get_or_insert_xml_fragment(crate::markdown::FRAGMENT);
    let first = change(&editor, |txn| {
        fragment.push_back(txn, XmlElementPrelim::empty("blockquote"));
    });
    nesting.check(&server, PAGE, &first).unwrap();
    apply(&server, &first);
    let mut deepest = {
        let txn = editor.transact();
        match fragment.get(&txn, 0) {
            Some(yrs::XmlOut::Element(element)) => element,
            _ => unreachable!(),
        }
    };
    let mut refused = None;
    for level in 2..=MAX_TYPE_DEPTH + 1 {
        let mut added = None;
        let update = change(&editor, |txn| {
            added = Some(deepest.push_back(txn, XmlElementPrelim::empty("blockquote")));
        });
        match nesting.check(&server, PAGE, &update) {
            Ok(()) => apply(&server, &update),
            Err(err) => {
                refused = Some((level, err));
                break;
            }
        }
        deepest = added.unwrap();
    }
    let (level, (status, _)) = refused.expect("refused past the bound");
    assert_eq!((level, status), (MAX_TYPE_DEPTH + 1, 422));
}

/// A root the codec does not have, a nested type of a kind it does not use, and a type under an item the document does
/// not have are refused; a board takes no nested type at all, and its plain values are taken.
#[test]
fn roots_kinds_and_unknown_places_are_refused() {
    let server = Doc::new();
    let other = Doc::new();
    let root = change(&other, |txn| {
        txn.get_or_insert_map("planted").insert(txn, "x", 1);
    });
    let (status, why) = Nesting::default().check(&server, PAGE, &root).unwrap_err();
    assert_eq!(status, 422, "{why}");
    assert!(why.contains("root"), "{why}");

    let maps = Doc::new();
    let nested = change(&maps, |txn| {
        txn.get_or_insert_map("app").insert(txn, "nested", MapPrelim::default());
    });
    let (_, why) = Nesting::default().check(&server, BOARD, &nested).unwrap_err();
    assert!(why.contains("shared type"), "{why}");
    let plain = change(&maps, |txn| {
        txn.get_or_insert_map("app").insert(txn, "gridSize", 20);
    });
    Nesting::default().check(&server, BOARD, &plain).unwrap();

    // an element under one the server's document does not have: its place is unknown
    let editor = Doc::new();
    let fragment = editor.get_or_insert_xml_fragment(crate::markdown::FRAGMENT);
    let outer = change(&editor, |txn| {
        fragment.push_back(txn, XmlElementPrelim::empty("blockquote"));
    });
    let inner = change(&editor, |txn| {
        if let Some(yrs::XmlOut::Element(element)) = fragment.get(txn, 0) {
            element.push_back(txn, XmlElementPrelim::empty("paragraph"));
        }
    });
    let (status, why) = Nesting::default().check(&server, PAGE, &inner).unwrap_err();
    assert_eq!(status, 422, "{why}");
    assert!(why.contains("unknown"), "{why}");
    // once the document has it (applied some other way: a seed, a merge), the whole state is read and it is placed
    apply(&server, &outer);
    Nesting::default().check(&server, PAGE, &inner).unwrap();
    assert_eq!(Nesting::default().check(&server, PAGE, b"\x05not").unwrap_err().0, 400);
}
