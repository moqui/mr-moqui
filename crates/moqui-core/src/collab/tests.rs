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

//! A co-edited wiki page on the real wiki services: two editors' clients (yrs documents standing in for the browser's
//! Yjs) join, edit at once and converge, a viewer may not change it, a save writes one version with its note, the room
//! closes when its editors leave, and a page changed outside co-editing is read again by the next join.

use std::collections::BTreeMap;

use moqui_lib::object::{to_plain_string, Value};
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

fn setup() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    ctx.factory().resource().install_git().unwrap();
    (ctx, facade)
}

fn user(facade: &EntityFacade, user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value("moqui.security.UserAccount").unwrap();
    row.set("userId", Value::string(user_id));
    row.set("username", Value::string(user_id));
    row.set("userFullName", Value::string(format!("{user_id} Example")));
    row.create(facade).unwrap();
}

fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    crate::user::internal_login_user(user_id).unwrap();
}

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let out = crate::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"));
    let errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

fn page_text(space: &str, path: &str) -> String {
    let info = call(
        "org.moqui.impl.WikiServices.get#WikiPageInfo",
        &[("wikiSpaceId", Value::string(space)), ("pagePath", Value::string(path))],
    );
    let location = to_plain_string(info.get("pageLocation").unwrap());
    ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory().resource().location_text(&location).unwrap()
    })
    .unwrap()
}

/// A browser's document: the server's update applied.
fn client(joined: &super::Joined) -> Doc {
    let doc = Doc::new();
    doc.transact_mut()
        .apply_update(Update::decode_v1(&joined.update).unwrap())
        .unwrap();
    doc
}

/// A paragraph typed at the end of a client's document; the update it sends.
fn type_paragraph(doc: &Doc, words: &str) -> Vec<u8> {
    let before = doc.transact().state_vector();
    let fragment = doc.get_or_insert_xml_fragment(super::markdown::FRAGMENT);
    {
        let mut txn = doc.transact_mut();
        let paragraph = fragment.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        paragraph.push_back(&mut txn, XmlTextPrelim::new(words));
    }
    doc.transact().encode_state_as_update_v1(&before)
}

#[test]
fn a_page_is_edited_together_saved_as_one_version_and_read_again_after_a_change_elsewhere() {
    let (context, facade) = setup();
    let _entered = context.enter();
    for id in ["alice", "bob"] {
        user(&facade, id);
    }
    login("alice");
    call(
        "org.moqui.impl.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", Value::string("TEAM")),
            ("description", Value::string("Team")),
        ],
    );
    let page = call(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("TEAM")),
            ("pageName", Value::string("Plan")),
            ("pageText", Value::string("# Plan\n\nFirst draft.\n")),
        ],
    );
    let page_id = to_plain_string(page.get("wikiPageId").unwrap());
    let key = format!("WikiPage:{page_id}");

    // alice and bob join; each client gets the page as the editor's structure
    let alice = super::join(&key, "alice", &[]).unwrap();
    assert!(alice.can_edit);
    assert_eq!(alice.user_name, "alice Example");
    let alice_doc = client(&alice);
    assert_eq!(super::markdown::render(&alice_doc), "# Plan\n\nFirst draft.\n");
    let bob_vector = Doc::new().transact().state_vector().encode_v1();
    let bob = super::join(&key, "bob", &bob_vector).unwrap();
    let bob_doc = client(&bob);

    // both type at once; the server takes each update, and each client the other's: all converge
    let from_alice = type_paragraph(&alice_doc, "Alice's line.");
    let from_bob = type_paragraph(&bob_doc, "Bob's line.");
    super::apply(&key, "alice", &from_alice).unwrap();
    super::apply(&key, "bob", &from_bob).unwrap();
    alice_doc
        .transact_mut()
        .apply_update(Update::decode_v1(&from_bob).unwrap())
        .unwrap();
    bob_doc
        .transact_mut()
        .apply_update(Update::decode_v1(&from_alice).unwrap())
        .unwrap();
    let server = super::snapshot(&key).unwrap();
    assert_eq!(super::markdown::render(&alice_doc), server);
    assert_eq!(super::markdown::render(&bob_doc), server);
    assert!(
        server.contains("Alice's line.") && server.contains("Bob's line."),
        "{server}"
    );
    // nothing is written to the page until a save
    assert_eq!(page_text("TEAM", "Plan"), "# Plan\n\nFirst draft.\n");

    // one who did not join, and an update that is not one, are refused
    assert_eq!(super::apply(&key, "carol", &from_alice).unwrap_err().code, 409);
    assert_eq!(super::apply(&key, "alice", b"not yjs").unwrap_err().code, 400);
    assert_eq!(super::join("Order:1", "alice", &[]).unwrap_err().code, 404);

    // alice saves: one version with her note, the text the document's
    let saved = super::save(&key, Some("Merged our lines")).unwrap();
    assert!(saved.is_some());
    let written = page_text("TEAM", "Plan");
    assert_eq!(written, server);
    let history = call(
        "org.moqui.impl.WikiServices.get#WikiPageHistory",
        &[("wikiPageId", Value::string(&page_id))],
    );
    assert!(format!("{history:?}").contains("Merged our lines"), "{history:?}");
    let events = super::take_events();
    assert!(
        matches!(
            events.as_slice(),
            [super::Event::Saved {
                version_name: Some(_),
                ..
            }]
        ),
        "{events:?}"
    );
    // a save with no change makes no version
    assert_eq!(super::save(&key, None).unwrap(), None);

    // the state is stored; when both leave, the room closes
    let stored = call(
        "org.moqui.impl.CollabServices.get#CollabDocument",
        &[("documentKey", Value::string(&key))],
    );
    assert_eq!(stored.get("exists"), Some(&Value::Bool(true)));
    super::leave(&key);
    super::leave(&key);
    super::tick();
    assert!(!super::open_documents().contains(&key));

    // the page is changed outside co-editing; the next join reads it again
    call(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("TEAM")),
            ("wikiPageId", Value::string(&page_id)),
            ("pageName", Value::string("Plan")),
            ("pageText", Value::string("# Plan\n\nRewritten in the text editor.\n")),
        ],
    );
    let again = super::join(&key, "bob", &[]).unwrap();
    assert_eq!(
        super::markdown::render(&client(&again)),
        "# Plan\n\nRewritten in the text editor.\n"
    );
    // the client's state vector gets only what it lacks
    let caught_up = client(&again).transact().state_vector().encode_v1();
    let nothing = super::join(&key, "bob", &caught_up).unwrap();
    assert!(
        Update::decode_v1(&nothing.update).unwrap().is_empty() || nothing.update.len() <= 2,
        "{:?}",
        nothing.update
    );
    let _ = StateVector::decode_v1(&nothing.state_vector).unwrap();
    super::leave(&key);
    super::leave(&key);
    super::tick();

    // the space's home page (no path) saves in place, not as a move
    let home = call(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &[("wikiSpaceId", Value::string("TEAM")), ("pageName", Value::string("TEAM")), ("pageText", Value::string("Home.\n"))],
    );
    let home_key = format!("WikiPage:{}", to_plain_string(home.get("wikiPageId").unwrap()));
    let joined = super::join(&home_key, "alice", &[]).unwrap();
    let home_doc = client(&joined);
    super::apply(&home_key, "alice", &type_paragraph(&home_doc, "More.")).unwrap();
    assert!(super::save(&home_key, Some("Home grows")).unwrap().is_some());
    assert_eq!(page_text("TEAM", ""), "Home.\n\nMore.\n");
    super::leave(&home_key);
    super::tick();
}
