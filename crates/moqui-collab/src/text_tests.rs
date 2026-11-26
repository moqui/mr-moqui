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

//! A co-edited file's room (plans/design-dev-aide.md F15) with the dev-aide's answers stood in for: who may join, the
//! seed from the file's text, two editors converging, the bounds on an update, the document and a user's rooms, the
//! room dropped with its last member, and a writer who lost the right dropped at the next check.

use std::collections::HashMap;
use std::time::Duration;

use moqui_lib::sync::lock;
use yrs::updates::decoder::Decode;
use yrs::{Doc, GetString, ReadTxn, Text, Transact, Update};

use super::{TextAccess, ACCESS_FOR_TEST, ON_ACCESS_FOR_TEST, TEXT_NAME};
use crate::{Event, Joined};

/// The text rooms and the stood-in answers are the process's: these tests take turns.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn writer(text: &str) -> Result<TextAccess, u16> {
    Ok(TextAccess {
        can_edit: true,
        reason: String::new(),
        text: Some(text.to_string()),
        max_bytes: 64,
        max_rooms: 2,
        max_total_bytes: 1 << 20,
    })
}

fn not_a_writer() -> Result<TextAccess, u16> {
    Ok(TextAccess {
        can_edit: false,
        reason: "runtime/component/x is upstream: extend it in site".into(),
        text: None,
        max_bytes: 64,
        max_rooms: 2,
        max_total_bytes: 1 << 20,
    })
}

fn answers(list: &[(&str, Result<TextAccess, u16>)]) {
    *lock(&ACCESS_FOR_TEST) = Some(
        list.iter()
            .map(|(user, answer)| ((*user).to_string(), answer.clone()))
            .collect(),
    );
}

/// A browser's document: the server's update applied.
fn client(joined: &Joined) -> Doc {
    let doc = Doc::new();
    doc.transact_mut()
        .apply_update(Update::decode_v1(&joined.update).unwrap())
        .unwrap();
    doc
}

fn text_of(doc: &Doc) -> String {
    let text = doc.get_or_insert_text(TEXT_NAME);
    let txn = doc.transact();
    text.get_string(&txn)
}

/// `words` typed at `at` in a client's text; the update it sends.
fn type_at(doc: &Doc, at: u32, words: &str) -> Vec<u8> {
    let before = doc.transact().state_vector();
    let text = doc.get_or_insert_text(TEXT_NAME);
    text.insert(&mut doc.transact_mut(), at, words);
    doc.transact().encode_state_as_update_v1(&before)
}

fn merge(doc: &Doc, update: &[u8]) {
    doc.transact_mut()
        .apply_update(Update::decode_v1(update).unwrap())
        .unwrap();
}

#[test]
fn a_writer_joins_the_file_seeded_from_its_text_and_two_editors_converge() {
    let _serial = serial();
    let key = super::key_of("site", "service/Things.xml");
    answers(&[
        ("ann", writer("<services/>\n")),
        ("bob", writer("ignored: the room is seeded")),
        ("viewer", not_a_writer()),
    ]);

    // one who may not write the file is no member; the dev-aide's reason is the refusal's
    let refused = crate::join(&key, "viewer", &[]).unwrap_err();
    assert_eq!(refused.code, 403);
    assert!(refused.message.contains("upstream"), "{}", refused.message);
    assert!(super::room_of(&key).is_none());
    // a key that names no file, and a path that climbs to the root, are not co-edited
    assert_eq!(crate::join("DevFile:site", "ann", &[]).unwrap_err().code, 404);
    assert_eq!(
        crate::join("DevFile:site:/etc/passwd", "ann", &[]).unwrap_err().code,
        404
    );

    // the first writer seeds the room from the file; the second gets the room's text, not the file's
    let ann_join = crate::join(&key, "ann", &[]).unwrap();
    assert!(ann_join.can_edit);
    let ann = client(&ann_join);
    assert_eq!(text_of(&ann), "<services/>\n");
    let bob = client(&crate::join(&key, "bob", &[]).unwrap());
    assert_eq!(text_of(&bob), "<services/>\n");
    assert!(crate::open_documents().contains(&key));

    // both type at once: each update goes to the room and to the other, and the three converge
    let from_ann = type_at(&ann, 0, "<!-- a -->");
    let from_bob = type_at(&bob, 11, "<!-- b -->");
    crate::apply(&key, "ann", &from_ann).unwrap();
    crate::apply(&key, "bob", &from_bob).unwrap();
    merge(&ann, &from_bob);
    merge(&bob, &from_ann);
    let room = crate::snapshot(&key).unwrap();
    assert_eq!(text_of(&ann), room);
    assert_eq!(text_of(&bob), room);
    assert!(room.contains("<!-- a -->") && room.contains("<!-- b -->"), "{room}");
    // a user who did not join sends nothing
    assert_eq!(crate::apply(&key, "viewer", &from_ann).unwrap_err().code, 409);

    // the room goes with its last member, and the next join is seeded from the file again
    crate::leave_as(&key, Some("ann"));
    assert!(super::room_of(&key).is_some());
    crate::leave_as(&key, Some("bob"));
    assert!(super::room_of(&key).is_none());
    assert!(!crate::open_documents().contains(&key));
    let again = client(&crate::join(&key, "ann", &[]).unwrap());
    assert_eq!(text_of(&again), "<services/>\n");
    crate::leave_as(&key, Some("ann"));
    *lock(&ACCESS_FOR_TEST) = None;
}

#[test]
fn an_update_the_document_and_a_users_rooms_are_bounded() {
    let _serial = serial();
    let key = super::key_of("site", "notes.txt");
    answers(&[("ann", writer("short"))]);
    // a file over the most bytes is not co-edited
    *lock(&ACCESS_FOR_TEST) = Some(HashMap::from([(
        "ann".to_string(),
        Ok(TextAccess {
            text: Some("x".repeat(65)),
            ..writer("").unwrap()
        }),
    )]));
    assert_eq!(crate::join(&key, "ann", &[]).unwrap_err().code, 413);
    assert!(super::room_of(&key).is_none());

    answers(&[("ann", writer("short"))]);
    let ann = client(&crate::join(&key, "ann", &[]).unwrap());
    // one update past the most bytes is refused, and the room's text is as it was
    let paste = type_at(&ann, 0, &"y".repeat(80));
    assert_eq!(crate::apply(&key, "ann", &paste).unwrap_err().code, 413);
    assert_eq!(crate::snapshot(&key).unwrap(), "short");
    // the state, with its edits, at most STATE_FACTOR times the most bytes: small edits until it is full
    let mut refused = None;
    let fresh = client(&crate::join(&key, "ann", &[]).unwrap());
    for round in 0..64 {
        let edit = type_at(&fresh, 0, "0123456789");
        if let Err(refusal) = crate::apply(&key, "ann", &edit) {
            refused = Some((round, refusal.code));
            break;
        }
    }
    let (round, code) = refused.expect("the state bound refuses an edit");
    assert_eq!(code, 413);
    assert!(round > 0 && round < 64, "{round}");

    // a user co-edits at most max_rooms files at once; the file already open is not another
    let second = super::key_of("site", "second.txt");
    let third = super::key_of("site", "third.txt");
    crate::join(&second, "ann", &[]).unwrap();
    assert_eq!(crate::join(&third, "ann", &[]).unwrap_err().code, 429);
    assert!(super::room_of(&third).is_none());
    crate::join(&key, "ann", &[]).unwrap();
    for _ in 0..3 {
        crate::leave_as(&key, Some("ann"));
    }
    crate::leave_as(&second, Some("ann"));
    assert!(super::room_of(&key).is_none() && super::room_of(&second).is_none());
    *lock(&ACCESS_FOR_TEST) = None;
}

#[test]
fn a_writer_who_lost_the_right_is_dropped_and_a_stale_answer_takes_no_update() {
    let _serial = serial();
    let key = super::key_of("site", "screen/Things.xml");
    answers(&[("ann", writer("<screen/>")), ("bob", writer("<screen/>"))]);
    let ann = client(&crate::join(&key, "ann", &[]).unwrap());
    let bob = client(&crate::join(&key, "bob", &[]).unwrap());

    // a check before the answer is a minute old asks nothing
    answers(&[("ann", not_a_writer()), ("bob", Err(503))]);
    let mut events = Vec::new();
    super::tick(&mut events);
    assert!(events.is_empty());
    crate::apply(&key, "ann", &type_at(&ann, 0, "a")).unwrap();

    // a minute on: ann may no longer write the file and is dropped; bob's check failed and is tried again later
    super::age_grants(&key, Duration::from_secs(61));
    super::tick(&mut events);
    assert!(
        matches!(events.as_slice(), [Event::Revoked { document_key, user_id }] if *document_key == key && user_id == "ann"),
        "{events:?}"
    );
    assert_eq!(crate::apply(&key, "ann", &type_at(&ann, 1, "b")).unwrap_err().code, 409);
    crate::apply(&key, "bob", &type_at(&bob, 0, "c")).unwrap();

    // bob's answer could not be renewed for three minutes: his updates are refused, and the next check drops him
    super::age_grants(&key, Duration::from_secs(200));
    assert_eq!(crate::apply(&key, "bob", &type_at(&bob, 0, "d")).unwrap_err().code, 403);
    events.clear();
    super::tick(&mut events);
    assert!(
        matches!(events.as_slice(), [Event::Revoked { user_id, .. }] if user_id == "bob"),
        "{events:?}"
    );
    // the host's leave for each dropped client closes the room
    crate::leave_as(&key, Some("ann"));
    crate::leave_as(&key, Some("bob"));
    assert!(super::room_of(&key).is_none());
    *lock(&ACCESS_FOR_TEST) = None;
}

#[test]
fn a_room_closed_while_a_join_asks_is_seeded_again_from_the_file_never_from_no_text() {
    let _serial = serial();
    let key = super::key_of("site", "service/Race.xml");
    answers(&[("ann", writer("<services/>\n")), ("bob", writer("<services/>\n"))]);
    crate::join(&key, "ann", &[]).unwrap();
    // bob finds the room, so his answer has no text; while it is asked, ann's last client leaves and the room goes
    let gone = key.clone();
    *lock(&ON_ACCESS_FOR_TEST) = Some(Box::new(move || crate::leave_as(&gone, Some("ann"))));
    let bob = client(&crate::join(&key, "bob", &[]).unwrap());
    assert!(lock(&ON_ACCESS_FOR_TEST).is_none(), "the leave ran during bob's join");
    assert_eq!(
        text_of(&bob),
        "<services/>\n",
        "seeded from the file's text read again, not from none"
    );
    assert_eq!(crate::snapshot(&key).unwrap(), "<services/>\n");
    crate::leave_as(&key, Some("bob"));
    assert!(super::room_of(&key).is_none());
    *lock(&ACCESS_FOR_TEST) = None;
}

#[test]
fn every_rooms_bytes_together_are_bounded() {
    let _serial = serial();
    let first = super::key_of("site", "total/one.txt");
    let second = super::key_of("site", "total/two.txt");
    let bound = super::total_bytes() + 60;
    let small = |text: &str| {
        Ok(TextAccess {
            max_total_bytes: bound,
            ..writer(text).unwrap()
        })
    };
    *lock(&ACCESS_FOR_TEST) = Some(HashMap::from([("ann".to_string(), small(&"a".repeat(30)))]));
    let ann = client(&crate::join(&first, "ann", &[]).unwrap());
    // a second room over the server's total is refused, and is no room
    assert_eq!(crate::join(&second, "ann", &[]).unwrap_err().code, 503);
    assert!(super::room_of(&second).is_none());
    // an update over the total is refused too
    let edit = type_at(&ann, 0, &"b".repeat(40));
    assert_eq!(crate::apply(&first, "ann", &edit).unwrap_err().code, 413);
    // the room's bytes leave the total with it
    let before = super::total_bytes();
    crate::leave_as(&first, Some("ann"));
    assert!(super::total_bytes() < before);
    *lock(&ACCESS_FOR_TEST) = None;
}
