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

use yrs::types::xml::{XmlElementPrelim, XmlFragment as _, XmlTextPrelim};
use yrs::updates::decoder::Decode as _;
use yrs::{
    Doc, Map as _, MapPrelim, ReadTxn as _, StateVector, Text as _, Transact as _, Update, WriteTxn as _, XmlOut,
};

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

/// Check `update`, and when it passes apply it and keep its depths, as a room does.
fn take(nesting: &mut Nesting, doc: &Doc, update: &[u8]) -> Result<(), (u16, String)> {
    let checked = nesting.check(doc, PAGE, update)?;
    apply(doc, update);
    nesting.keep(checked);
    Ok(())
}

/// How deep the chain of first elements under the page's fragment is.
fn depth(doc: &Doc) -> u32 {
    let fragment = doc.get_or_insert_xml_fragment(crate::markdown::FRAGMENT);
    let txn = doc.transact();
    let mut levels = 0;
    let mut next = fragment.get(&txn, 0);
    while let Some(XmlOut::Element(element)) = next {
        levels += 1;
        next = element.get(&txn, 0);
    }
    levels
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
    let _ = Nesting::default()
        .check(&server, PAGE, &chain(MAX_TYPE_DEPTH as usize))
        .unwrap();

    let page = Doc::new();
    crate::markdown::seed(
        &page,
        "# Title\n\n> quote\n> > nested\n\n- a\n  - b\n    - c\n\n| x | y |\n| - | - |\n| 1 | 2 |\n\ntext *b* **c**\n",
    );
    let _ = Nesting::default().check(&server, PAGE, &whole(&page)).unwrap();
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
    take(&mut nesting, &server, &first).unwrap();
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
        match take(&mut nesting, &server, &update) {
            Ok(()) => {}
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
    let _ = Nesting::default().check(&server, BOARD, &plain).unwrap();

    // an element (another editor's) under one the server's document does not have: its place is unknown
    let editor = Doc::new();
    let fragment = editor.get_or_insert_xml_fragment(crate::markdown::FRAGMENT);
    let outer = change(&editor, |txn| {
        fragment.push_back(txn, XmlElementPrelim::empty("blockquote"));
    });
    let other = Doc::new();
    apply(&other, &outer);
    let others = other.get_or_insert_xml_fragment(crate::markdown::FRAGMENT);
    let inner = change(&other, |txn| {
        if let Some(yrs::XmlOut::Element(element)) = others.get(txn, 0) {
            element.push_back(txn, XmlElementPrelim::empty("paragraph"));
        }
    });
    let (status, why) = Nesting::default().check(&server, PAGE, &inner).unwrap_err();
    assert_eq!(status, 422, "{why}");
    assert!(why.contains("unknown"), "{why}");
    // once the document has it (applied some other way: a seed, a merge), the whole state is read and it is placed
    apply(&server, &outer);
    let _ = Nesting::default().check(&server, PAGE, &inner).unwrap();
    assert_eq!(Nesting::default().check(&server, PAGE, b"\x05not").unwrap_err().0, 400);
}

/// An update that claims an item the document has (yrs skips it) never changes that item's depth: a copy of the deepest
/// element's block placed at the root, sent before each next level, does not let the levels past the bound (the
/// review's proof reached 768 levels this way). The copy and the next level in one update are refused the same way.
#[test]
fn a_copy_of_a_known_block_does_not_reset_its_depth() {
    const EDITOR: u64 = 4242;
    let editor = Doc::with_client_id(EDITOR);
    let server = Doc::new();
    let mut nesting = Nesting::default();
    let fragment = editor.get_or_insert_xml_fragment(crate::markdown::FRAGMENT);
    let first = change(&editor, |txn| {
        fragment.push_back(txn, XmlElementPrelim::empty("blockquote"));
    });
    take(&mut nesting, &server, &first).unwrap();
    let mut deepest = {
        let txn = editor.transact();
        match fragment.get(&txn, 0) {
            Some(XmlOut::Element(element)) => element,
            _ => unreachable!(),
        }
    };
    // a copy of the editor's block at `clock`, claimed directly under the root
    let copy_at_root = |clock: u32| {
        let fake = Doc::with_client_id(EDITOR);
        if clock > 0 {
            let filler = fake.get_or_insert_text("filler");
            let mut txn = fake.transact_mut();
            filler.insert(&mut txn, 0, &"a".repeat(clock as usize));
        }
        let root = fake.get_or_insert_xml_fragment(crate::markdown::FRAGMENT);
        let mut since = StateVector::default();
        since.set_max(yrs::block::ClientID::new(EDITOR), clock);
        {
            let mut txn = fake.transact_mut();
            root.push_back(&mut txn, XmlElementPrelim::empty("p"));
        }
        let copy = fake.transact().encode_state_as_update_v1(&since);
        copy
    };
    // the bound is reached with copies sent all along; with the depths written from the copies it was not
    let mut refused = None;
    for level in 2..=3 * MAX_TYPE_DEPTH {
        if level > 8 {
            // the copy itself passes (yrs skips it) and changes nothing kept
            take(&mut nesting, &server, &copy_at_root(level - 2)).unwrap();
        }
        let mut added = None;
        let update = change(&editor, |txn| {
            added = Some(deepest.push_back(txn, XmlElementPrelim::empty("blockquote")));
        });
        if let Err(err) = take(&mut nesting, &server, &update) {
            refused = Some((level, err, update));
            break;
        }
        deepest = added.unwrap();
    }
    let (level, (status, _), next) = refused.expect("refused past the bound");
    assert_eq!((level, status), (MAX_TYPE_DEPTH + 1, 422));
    assert_eq!(depth(&server), MAX_TYPE_DEPTH);

    // the copy of the deepest block and the level under it in one update: the level is placed by the true depth
    let copy = copy_at_root(MAX_TYPE_DEPTH - 2);
    let merged = yrs::merge_updates_v1([copy.as_slice(), next.as_slice()]).unwrap();
    let (status, why) = nesting.check(&server, PAGE, &merged).unwrap_err();
    assert_eq!(status, 422, "{why}");
    assert!(why.contains("nest at most"), "{why}");
}

/// An update that names a client twice, or that skips clocks past the document's state, is refused: yrs would leave a
/// hole a later item fills.
#[test]
fn a_client_named_twice_and_skipped_clocks_are_refused() {
    let editor = Doc::with_client_id(7);
    let server = Doc::new();
    let mut nesting = Nesting::default();
    let fragment = editor.get_or_insert_xml_fragment(crate::markdown::FRAGMENT);
    let first = change(&editor, |txn| {
        fragment.push_back(txn, XmlElementPrelim::empty("paragraph"));
    });
    let second = change(&editor, |txn| {
        fragment.push_back(txn, XmlElementPrelim::empty("paragraph"));
    });
    // the second without the first: its clock is past the server's state for the editor
    let (status, why) = nesting.check(&server, PAGE, &second).unwrap_err();
    assert_eq!(status, 422, "{why}");
    assert!(why.contains("skips clocks"), "{why}");
    // one client, twice: [clients=2][blocks=0][client 7][clock 0] twice
    let twice = [2u8, 0, 7, 0, 0, 7, 0, 0];
    let (status, why) = nesting.check(&server, PAGE, &twice).unwrap_err();
    assert_eq!(status, 400, "{why}");
    take(&mut nesting, &server, &first).unwrap();
    take(&mut nesting, &server, &second).unwrap();
}

/// Text depths are kept, so a type an editor puts beside text (an inline embed in a text) is placed; text typed in one
/// go is one kept run.
#[test]
fn a_type_beside_text_is_placed() {
    let editor = Doc::with_client_id(9);
    let server = Doc::new();
    let mut nesting = Nesting::default();
    let fragment = editor.get_or_insert_xml_fragment(crate::markdown::FRAGMENT);
    let paragraph = change(&editor, |txn| {
        let element = fragment.push_back(txn, XmlElementPrelim::empty("paragraph"));
        element.push_back(txn, XmlTextPrelim::new(""));
    });
    take(&mut nesting, &server, &paragraph).unwrap();
    let text = {
        let txn = editor.transact();
        match fragment.get(&txn, 0) {
            Some(XmlOut::Element(element)) => match element.get(&txn, 0) {
                Some(XmlOut::Text(text)) => text,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    };
    for at in 0..20 {
        let typed = change(&editor, |txn| text.insert(txn, at, "x"));
        take(&mut nesting, &server, &typed).unwrap();
    }
    assert!(
        nesting.parents[&yrs::block::ClientID::new(9)].len() <= 3,
        "{:?}",
        nesting.parents
    );
    let embed = change(&editor, |txn| {
        text.insert_embed(txn, 10, XmlElementPrelim::empty("mention"));
    });
    take(&mut nesting, &server, &embed).unwrap();
}
