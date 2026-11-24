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

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;

fn setup() -> (ExecutionContext, EntityFacade) {
    setup_with_conf("")
}

fn setup_with_conf(conf: &str) -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        conf,
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
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
    moqui_core::user::internal_login_user(user_id).unwrap();
}

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let out = moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"));
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
        "org.moqui.resource.WikiServices.get#WikiPageInfo",
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

/// The board settings of the default conf, which a test conf does not read.
const BOARD_CONF: &str = r#"<moqui-conf><board max-document-bytes="20971520" max-images="200" max-image-bytes="10485760" max-update-bytes="1048576"/></moqui-conf>"#;

/// The rooms and their events are the process's: the tests that read events take turns.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn a_page_is_edited_together_saved_as_one_version_and_read_again_after_a_change_elsewhere() {
    let _serial = serial();
    let (context, facade) = setup();
    let _entered = context.enter();
    for id in ["alice", "bob"] {
        user(&facade, id);
    }
    login("alice");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", Value::string("TEAM")),
            ("description", Value::string("Team")),
        ],
    );
    let page = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
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
        "org.moqui.resource.WikiServices.get#WikiPageHistory",
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
        "org.moqui.resource.CollabServices.get#CollabDocument",
        &[("documentKey", Value::string(&key))],
    );
    assert_eq!(stored.get("exists"), Some(&Value::Bool(true)));
    super::leave(&key);
    super::leave(&key);
    super::tick();
    assert!(!super::open_documents().contains(&key));

    // the page is changed outside co-editing; the next join reads it again
    call(
        "org.moqui.resource.WikiServices.update#WikiPage",
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
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("TEAM")),
            ("pageName", Value::string("TEAM")),
            ("pageText", Value::string("Home.\n")),
        ],
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

/// Another server's editor types `words` into its room, which it stores (a flush), and the update it relays here is
/// lost on the way.
fn another_server_types(facade: &EntityFacade, key: &str, words: &str) {
    // a stamp of its own, as another server's write has
    std::thread::sleep(std::time::Duration::from_millis(5));
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade
        .find(super::COLLAB_DOCUMENT)
        .unwrap()
        .eq("documentKey", Value::string(key))
        .one()
        .unwrap()
        .unwrap();
    let doc = Doc::new();
    doc.transact_mut()
        .apply_update(Update::decode_v1(&super::bytes_of(&row, "stateData")).unwrap())
        .unwrap();
    type_paragraph(&doc, words);
    let state = doc.transact().encode_state_as_update_v1(&StateVector::default());
    row.set("stateData", Value::Bytes(state));
    row.update(facade).unwrap();
}

/// The updates the rooms gave `key`'s clients since the last call.
fn updates_for(key: &str) -> Vec<Vec<u8>> {
    super::take_events()
        .into_iter()
        .filter_map(|event| match event {
            super::Event::Update { document_key, update } if document_key == key => Some(update),
            _ => None,
        })
        .collect()
}

/// HL-018: an update relayed from another server that has the document open is lost, and that server stored it. The
/// next tick merges the stored state into the room here and gives its clients what they lack, once; a version saved
/// here before any tick has another lost edit too.
#[test]
fn an_update_lost_between_servers_is_merged_from_the_stored_state_and_saved() {
    let _serial = serial();
    let (context, facade) = setup();
    let _entered = context.enter();
    user(&facade, "carol");
    login("carol");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", Value::string("LOST")),
            ("description", Value::string("Lost")),
        ],
    );
    let page = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("LOST")),
            ("pageName", Value::string("Plan")),
            ("pageText", Value::string("# Plan\n\nStart.\n")),
        ],
    );
    let key = format!("WikiPage:{}", to_plain_string(page.get("wikiPageId").unwrap()));
    let joined = super::join(&key, "carol", &[]).unwrap();
    let carol_doc = client(&joined);
    super::tick();
    assert!(updates_for(&key).is_empty(), "nothing stored elsewhere yet");

    another_server_types(&facade, &key, "From the other server.");
    super::tick();
    let updates = updates_for(&key);
    assert_eq!(updates.len(), 1, "the room's clients get what it lacked");
    carol_doc
        .transact_mut()
        .apply_update(Update::decode_v1(&updates[0]).unwrap())
        .unwrap();
    assert!(super::markdown::render(&carol_doc).contains("From the other server."));
    assert_eq!(super::snapshot(&key).unwrap(), super::markdown::render(&carol_doc));
    super::tick();
    assert!(updates_for(&key).is_empty(), "a row read once is not merged again");

    // another lost edit, saved before a tick reads it: the version has both
    another_server_types(&facade, &key, "Saved at once.");
    assert!(super::save(&key, Some("Both lines")).unwrap().is_some());
    let written = page_text("LOST", "Plan");
    assert!(
        written.contains("From the other server.") && written.contains("Saved at once."),
        "{written}"
    );
    let updates = updates_for(&key);
    assert_eq!(updates.len(), 1, "{updates:?}");
    carol_doc
        .transact_mut()
        .apply_update(Update::decode_v1(&updates[0]).unwrap())
        .unwrap();
    assert_eq!(super::markdown::render(&carol_doc), written);
    super::leave(&key);
    super::tick();
    let _ = super::take_events();
}

/// The Rust services register at the `moqui_collab::` locations the service file names, and at no other.
#[test]
fn the_services_register_at_the_locations_the_service_file_names() {
    let mut map = BTreeMap::new();
    crate::service::register(&mut map).unwrap();
    let registered: Vec<&str> = map.keys().copied().collect();
    let file = moqui_core::context::framework_dir().join("service/org/moqui/resource/CollabServices.xml");
    let text = std::fs::read_to_string(file).unwrap();
    let mut named: Vec<String> = text
        .split("location=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(location, _)| location.to_string()))
        .filter(|location| location.starts_with("moqui_collab::"))
        .collect();
    named.sort_unstable();
    named.dedup();
    assert_eq!(registered, named);
}

/// An update whose attribute value nests arrays far past any editor's, two bytes a level: yrs decoded a value by a
/// recursion once a level, so applying it overflowed the worker's stack and aborted the process. yrs at moqui's fork
/// (docs/Build-Test.md Forks) refuses a value nested deeper than 128, so the update is refused as not one, on a small
/// stack too (plans/Security-Review-Medium.md SEC-TLS-95).
#[test]
fn an_update_nested_far_past_the_decode_limit_is_refused_on_a_small_stack() {
    use yrs::{Any, Xml};
    let _serial = serial();
    let (context, facade) = setup();
    let _entered = context.enter();
    user(&facade, "deep");
    login("deep");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", Value::string("DEEP")),
            ("description", Value::string("Deep")),
        ],
    );
    let page = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("DEEP")),
            ("pageName", Value::string("Nested")),
            ("pageText", Value::string("Text.\n")),
        ],
    );
    let key = format!("WikiPage:{}", to_plain_string(page.get("wikiPageId").unwrap()));
    let joined = super::join(&key, "deep", &[]).unwrap();
    let doc = client(&joined);
    // an attribute whose value is an array holding a marker string: [117 (array), 1 (length), 119 (string), 12, marker]
    let before = doc.transact().state_vector();
    {
        let fragment = doc.get_or_insert_xml_fragment(super::markdown::FRAGMENT);
        let mut txn = doc.transact_mut();
        let paragraph = fragment.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        paragraph.insert_attribute(&mut txn, "deep", Any::Array(vec![Any::from("NESTEDMARKER")].into()));
    }
    let plain = doc.transact().encode_state_as_update_v1(&before);
    let marker: Vec<u8> = [117, 1, 119, 12].iter().copied().chain(*b"NESTEDMARKER").collect();
    let at = plain
        .windows(marker.len())
        .position(|window| window == marker.as_slice())
        .expect("the attribute's value in the update");
    // the same update with the array nested 100,000 deep around the marker
    let mut nested = plain[..at].to_vec();
    for _ in 0..100_000 {
        nested.extend_from_slice(&[117, 1]);
    }
    nested.extend_from_slice(&marker[2..]);
    nested.extend_from_slice(&plain[at + marker.len()..]);
    // just past the limit is refused too, and the plain update is taken
    let mut past = plain[..at].to_vec();
    for _ in 0..129 {
        past.extend_from_slice(&[117, 1]);
    }
    past.extend_from_slice(&marker[2..]);
    past.extend_from_slice(&plain[at + marker.len()..]);
    // a quarter of a release worker's stack: yrs, built optimized and not incremental as a dependency from git, takes
    // about 2 KiB a level to read its 128 levels, so 256 KiB is not enough; 100,000 levels would need far more
    let applied = {
        let key = key.clone();
        std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(move || {
                (
                    super::apply(&key, "deep", &nested).map_err(|refusal| refusal.code),
                    super::apply(&key, "deep", &past).map_err(|refusal| refusal.code),
                    super::apply(&key, "deep", &plain).map_err(|refusal| refusal.code),
                )
            })
            .unwrap()
            .join()
            .expect("no stack overflow")
    };
    assert_eq!(applied, (Err(400), Err(400), Ok(())));
    // the room is written and closed, as each test leaves the shared rooms
    super::leave(&key);
    super::tick();
    let _ = super::take_events();
}

/// A state vector or an update of a few bytes that claims billions of clients or blocks: yrs reserved what was claimed
/// before reading it, which took gigabytes or failed to allocate, and a granted reservation of a hash map cost its size
/// in memory before the input ran out. yrs at moqui's fork reserves at most 1024 entries, so each is refused at the end
/// of its few bytes (plans/Security-Review-Medium.md SEC-TLS-117).
#[test]
fn a_claimed_length_past_the_input_is_refused_at_its_end() {
    let _serial = serial();
    let (context, facade) = setup();
    let _entered = context.enter();
    user(&facade, "claims");
    login("claims");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", Value::string("CLAIMS")),
            ("description", Value::string("Claims")),
        ],
    );
    let page = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("CLAIMS")),
            ("pageName", Value::string("Claimed")),
            ("pageText", Value::string("Text.\n")),
        ],
    );
    let key = format!("WikiPage:{}", to_plain_string(page.get("wikiPageId").unwrap()));
    // about 4 billion clients, each with a clock
    let vector = [0xff_u8, 0xff, 0xff, 0xff, 0x0f, 1];
    let refused = super::join(&key, "claims", &vector).unwrap_err();
    assert_eq!(refused.code, 400);
    assert!(refused.message.contains("end of buffer"), "{}", refused.message);
    super::join(&key, "claims", &[]).unwrap();
    // about 4 billion clients; one client with about 4 billion blocks
    for update in [
        [0xff_u8, 0xff, 0xff, 0xff, 0x0f, 0, 0, 0],
        [0x01, 0xff, 0xff, 0xff, 0xff, 0x0f, 0, 0],
    ] {
        let refused = super::apply(&key, "claims", &update).unwrap_err();
        assert_eq!(refused.code, 400);
        assert!(refused.message.contains("end of buffer"), "{}", refused.message);
    }
    super::leave(&key);
    super::tick();
    let _ = super::take_events();
}

/// SEC-TLS-111: a join whose state vector does not decode is no member, so the room still closes when it is left.
#[test]
fn a_refused_join_is_no_member() {
    let _serial = serial();
    let (context, facade) = setup();
    let _entered = context.enter();
    user(&facade, "rejoin");
    login("rejoin");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", Value::string("REJOIN")),
            ("description", Value::string("Rejoin")),
        ],
    );
    let page = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("REJOIN")),
            ("pageName", Value::string("Kept")),
            ("pageText", Value::string("Text.\n")),
        ],
    );
    let key = format!("WikiPage:{}", to_plain_string(page.get("wikiPageId").unwrap()));
    let bad = [0xff_u8; 11];
    assert_eq!(super::join(&key, "rejoin", &bad).unwrap_err().code, 400);
    super::tick();
    assert!(
        !super::open_documents().contains(&key),
        "the refused join left the room open"
    );
    let _ = super::take_events();
}

/// SEC-TLS-112: an editor's access is asked again by the tick once a minute old, and an answer the tick did not renew
/// is not used for an update.
#[test]
fn an_editors_access_is_asked_again_and_a_stale_answer_takes_no_update() {
    let _serial = serial();
    let (context, facade) = setup();
    let _entered = context.enter();
    user(&facade, "recheck");
    login("recheck");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", Value::string("RECHECK")),
            ("description", Value::string("Recheck")),
        ],
    );
    let page = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("RECHECK")),
            ("pageName", Value::string("Kept")),
            ("pageText", Value::string("Text.\n")),
        ],
    );
    let key = format!("WikiPage:{}", to_plain_string(page.get("wikiPageId").unwrap()));
    let joined = super::join(&key, "recheck", &[]).unwrap();
    let doc = client(&joined);
    super::age_grants(&key, super::ACCESS_STALE + std::time::Duration::from_secs(1));
    let typed = type_paragraph(&doc, "Late.");
    assert_eq!(super::apply(&key, "recheck", &typed).unwrap_err().code, 403);
    // the tick asks again: the editor may still change the page
    super::tick();
    super::apply(&key, "recheck", &typed).unwrap();
    super::leave(&key);
    super::tick();
    let _ = super::take_events();
}

/// SEC-TLS-113: an update that would take the document's state past resource-facade@collab-max-state-bytes is refused.
#[test]
fn an_update_past_the_state_limit_is_refused() {
    let _serial = serial();
    let (context, facade) = setup();
    let _entered = context.enter();
    user(&facade, "sizer");
    login("sizer");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", Value::string("SIZER")),
            ("description", Value::string("Sizer")),
        ],
    );
    let page = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("SIZER")),
            ("pageName", Value::string("Small")),
            ("pageText", Value::string("Text.\n")),
        ],
    );
    let key = format!("WikiPage:{}", to_plain_string(page.get("wikiPageId").unwrap()));
    let joined = super::join(&key, "sizer", &[]).unwrap();
    let doc = client(&joined);
    let typed = type_paragraph(&doc, &"many words ".repeat(400));
    let setting = moqui_core::context::conf_env::env_name_of("resource-facade@collab-max-state-bytes");
    moqui_lib::env::set_property(&setting, "2048");
    let refused = super::apply(&key, "sizer", &typed);
    moqui_lib::env::clear_property(&setting);
    assert_eq!(refused.unwrap_err().code, 413);
    super::apply(&key, "sizer", &typed).unwrap();
    super::leave(&key);
    super::tick();
    let _ = super::take_events();
}

/// SEC-TLS-112: a user who may no longer view the page is taken out at the tick's check (Revoked, then 409 on an
/// update), one who may view and not change is told (CanEdit false, then 403); a check that keeps failing takes the
/// member out once the answer is stale; and the answer goes with the user's last client.
#[test]
fn a_revoked_editor_is_dropped_and_a_lost_edit_is_told() {
    let _serial = serial();
    let (context, facade) = setup();
    let _entered = context.enter();
    user(&facade, "revoked");
    login("revoked");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", Value::string("REVOKE")),
            ("description", Value::string("Revoke")),
        ],
    );
    let page = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("REVOKE")),
            ("pageName", Value::string("Kept")),
            ("pageText", Value::string("Text.\n")),
        ],
    );
    let key = format!("WikiPage:{}", to_plain_string(page.get("wikiPageId").unwrap()));
    // the wiki's answer as the test sets it, put back when the test ends, a panic included
    struct Answer;
    impl Drop for Answer {
        fn drop(&mut self) {
            *super::lock(&super::ACCESS_FOR_TEST) = None;
        }
    }
    let _answer = Answer;
    let set = |answer: Option<Result<(bool, bool), u16>>| *super::lock(&super::ACCESS_FOR_TEST) = answer;
    let joined = super::join(&key, "revoked", &[]).unwrap();
    let doc = client(&joined);
    let typed = type_paragraph(&doc, "Edit.");
    let _ = super::take_events();

    // view kept, edit lost
    set(Some(Ok((true, false))));
    super::age_grants(&key, super::ACCESS_RECHECK + std::time::Duration::from_secs(1));
    super::tick();
    assert!(super::take_events().iter().any(|event| matches!(event,
        super::Event::CanEdit { user_id, can_edit: false, .. } if user_id == "revoked")));
    assert_eq!(super::apply(&key, "revoked", &typed).unwrap_err().code, 403);

    // the check fails: kept until stale, then taken out
    set(Some(Err(500)));
    super::age_grants(&key, super::ACCESS_RECHECK + std::time::Duration::from_secs(1));
    super::tick();
    assert!(!super::take_events()
        .iter()
        .any(|event| matches!(event, super::Event::Revoked { .. })));
    super::age_grants(&key, super::ACCESS_STALE + std::time::Duration::from_secs(120));
    super::tick();
    assert!(super::take_events().iter().any(|event| matches!(event,
        super::Event::Revoked { user_id, .. } if user_id == "revoked")));
    assert_eq!(super::apply(&key, "revoked", &typed).unwrap_err().code, 409);

    // view lost: taken out at the check
    set(None);
    let again = super::join(&key, "revoked", &[]).unwrap();
    let _ = again;
    set(Some(Ok((false, false))));
    super::age_grants(&key, super::ACCESS_RECHECK + std::time::Duration::from_secs(1));
    super::tick();
    assert!(super::take_events().iter().any(|event| matches!(event,
        super::Event::Revoked { user_id, .. } if user_id == "revoked")));
    assert_eq!(super::apply(&key, "revoked", &typed).unwrap_err().code, 409);

    // the answer goes with the user's last client
    set(None);
    super::join(&key, "revoked", &[]).unwrap();
    super::leave_as(&key, Some("revoked"));
    assert_eq!(super::apply(&key, "revoked", &typed).unwrap_err().code, 409);
    super::leave(&key);
    super::leave(&key);
    super::tick();
    let _ = super::take_events();
}

#[test]
fn board_versions_keep_the_scene_while_the_search_snapshot_has_its_words() {
    use serde_json::{json, Value as Json};
    let _serial = serial();
    let (ctx, facade) = setup_with_conf(BOARD_CONF);
    let _entered = ctx.enter();
    user(&facade, "artist");
    login("artist");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[("wikiSpaceId", Value::string("ARTS"))],
    );
    let scene = json!({"type":"excalidraw","version":2,"elements":[
        {"id":"label","type":"text","version":1,"versionNonce":7,"index":"a0","text":"Draft","x":1.5,"y":2}
    ],"appState":{"viewBackgroundColor":"#ffffff"},"files":{}});
    let saved = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("ARTS")),
            ("pageName", Value::string("Sketch")),
            ("wikiType", Value::string("excalidraw")),
            ("pageText", Value::string(scene.to_string())),
        ],
    );
    let id = to_plain_string(saved.get("wikiPageId").unwrap());
    let key = format!("WikiPage:{id}");
    let document = client(&super::join(&key, "artist", &[]).unwrap());
    assert_eq!(super::board::search_text(&document), "Draft");
    // An inline image is refused before it changes the server's document or clocks.
    // A concurrent valid edit can still be saved after that refusal.
    let invalid = Doc::new();
    let files = invalid.get_or_insert_map("files");
    yrs::Map::insert(
        &files,
        &mut invalid.transact_mut(),
        "inline",
        yrs::Any::from_json(r#"{"dataURL":"data:image/png;base64,AA=="}"#).unwrap(),
    );
    let bad = invalid.transact().encode_state_as_update_v1(&StateVector::default());
    assert_eq!(super::apply(&key, "artist", &bad).unwrap_err().code, 422);
    let rejected = super::room_of(&key).unwrap();
    let rejected = moqui_lib::sync::lock(&rejected);
    assert_eq!(super::board::render(&rejected.doc), super::board::render(&document));
    drop(rejected);
    let before = document.transact().state_vector();
    let mut edit = scene;
    edit["elements"][0]["version"] = json!(2);
    edit["elements"][0]["text"] = json!("Final <caption>");
    super::board::seed(&document, &edit.to_string()).unwrap();
    super::apply(&key, "artist", &document.transact().encode_state_as_update_v1(&before)).unwrap();
    let version = super::save(&key, Some("Meeting notes")).unwrap().unwrap();
    let text = page_text("ARTS", "Sketch");
    let saved: Json = serde_json::from_str(&text).unwrap();
    assert_eq!(saved["elements"][0]["text"], "Final <caption>");
    assert_eq!(saved["elements"][0]["x"], 1.5);
    let persisted = super::stored(&key).unwrap().unwrap();
    assert_eq!(
        persisted.get("snapshotText").map(to_plain_string).as_deref(),
        Some("Final <caption>")
    );
    let info = call(
        "org.moqui.resource.WikiServices.get#WikiPageInfoById",
        &[("wikiPageId", Value::string(&id))],
    );
    let location = super::text(&info, "pageLocation");
    assert!(location.ends_with("Sketch.excalidraw"));
    let stem = location.strip_suffix(".excalidraw").unwrap();
    let preview = ctx
        .factory()
        .resource()
        .location_text(&format!("{stem}/_attachments/_board-{version}.svg"))
        .unwrap();
    assert!(preview.contains("Final &lt;caption&gt;"), "{preview}");
    super::leave_as(&key, Some("artist"));
    super::tick();
}

#[test]
fn a_dirty_room_cannot_recreate_a_deleted_document() {
    let _serial = serial();
    let (context, facade) = setup();
    let _entered = context.enter();
    user(&facade, "retired");
    login("retired");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[
            ("wikiSpaceId", Value::string("REMOVE")),
            ("description", Value::string("Removed room")),
        ],
    );
    let page = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("REMOVE")),
            ("pageName", Value::string("Sketch")),
            ("pageText", Value::string("Before")),
        ],
    );
    let page_id = to_plain_string(&page["wikiPageId"]);
    let key = format!("WikiPage:{page_id}");
    let joined = super::join(&key, "retired", &[]).unwrap();
    super::apply(&key, "retired", &type_paragraph(&client(&joined), "Unflushed")).unwrap();
    let room = super::room_of(&key).unwrap();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find(super::COLLAB_DOCUMENT)
        .unwrap()
        .eq("documentKey", Value::string(&key))
        .delete_all()
        .unwrap();
    // The deleting transaction has dropped the document. A worker that had
    // retained the room before the next tick must not insert its snapshot again.
    assert!(super::flush(&key, &room).is_err());
    super::tick();
    assert!(super::room_of(&key).is_none());
    assert!(super::stored(&key).unwrap().is_none());
    assert!(super::take_events().iter().any(|event| matches!(event, super::Event::Revoked {document_key,user_id} if document_key == &key && user_id == "retired")));
}

/// A meeting member's frames are answered from the room's grant, with no grant service call per frame: asked at the
/// join and by the tick every 10 seconds, or at the next tick once the call changed (`recheck_meeting`, the
/// `collab-recheck` topic), which tells a lost edit right and drops a member who lost the board.
#[test]
fn a_meeting_grant_is_answered_in_memory_and_asked_again_when_the_call_changes() {
    use serde_json::json;
    use std::sync::atomic::Ordering;
    let _serial = serial();
    let (context, facade) = setup_with_conf(BOARD_CONF);
    let _entered = context.enter();
    user(&facade, "convener");
    login("convener");
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[("wikiSpaceId", Value::string("MEET"))],
    );
    let scene = json!({"type":"excalidraw","version":2,"elements":[
        {"id":"note","type":"text","version":1,"versionNonce":3,"index":"a0","text":"Agenda"}
    ],"appState":{},"files":{}});
    let saved = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string("MEET")),
            ("pageName", Value::string("Board")),
            ("wikiType", Value::string("excalidraw")),
            ("pageText", Value::string(scene.to_string())),
        ],
    );
    let key = format!("WikiPage:{}", to_plain_string(saved.get("wikiPageId").unwrap()));
    struct Answer;
    impl Drop for Answer {
        fn drop(&mut self) {
            *super::lock(&super::meeting::MEETING_GRANT_FOR_TEST) = None;
        }
    }
    let _answer = Answer;
    let set = |answer: Result<(bool, bool), u16>| *super::lock(&super::meeting::MEETING_GRANT_FOR_TEST) = Some(answer);
    let asked = || super::meeting::MEETING_GRANTS_ASKED.load(Ordering::SeqCst);
    set(Ok((true, true)));
    let meeting = super::Meeting {
        call_id: "MEETCALL".into(),
        leg_id: "LEG1".into(),
        user_id: "convener".into(),
        credential: None,
    };
    let actor = meeting.key();
    let before = asked();
    let joined = super::join_scoped(&key, "convener", &[], Some(meeting)).unwrap();
    assert!(joined.can_edit);
    assert_eq!(asked(), before + 1, "the join asks once");

    // frames and an update go by the grant in memory; a tick within 10 seconds asks nothing
    let document = client(&joined);
    assert!(super::meeting_allows(&key, &actor, false));
    assert!(super::meeting_allows(&key, &actor, true));
    assert!(
        !super::meeting_allows(&key, "\0meeting/OTHER/LEG1", false),
        "no grant, no frames"
    );
    let before_edit = document.transact().state_vector();
    let mut edit = scene.clone();
    edit["elements"][0]["version"] = json!(2);
    edit["elements"][0]["text"] = json!("Agenda, revised");
    super::board::seed(&document, &edit.to_string()).unwrap();
    super::apply(
        &key,
        &actor,
        &document.transact().encode_state_as_update_v1(&before_edit),
    )
    .unwrap();
    super::tick();
    super::recheck_meeting("ANOTHERCALL");
    super::tick();
    assert_eq!(asked(), before + 1, "no grant call per frame or per tick");
    let _ = super::take_events();

    // a stale grant allows nothing; the tick past 10 seconds asks again
    super::age_grants(&key, super::ACCESS_STALE + std::time::Duration::from_secs(1));
    assert!(!super::meeting_allows(&key, &actor, false));
    super::tick();
    assert_eq!(asked(), before + 2);
    assert!(super::meeting_allows(&key, &actor, false));

    // the call changed: drawing turned off, told at the next tick
    set(Ok((true, false)));
    super::recheck_meeting("MEETCALL");
    super::tick();
    assert_eq!(asked(), before + 3);
    assert!(super::take_events().iter().any(|event| matches!(event,
        super::Event::CanEdit { user_id, can_edit: false, .. } if *user_id == actor)));
    assert!(super::meeting_allows(&key, &actor, false));
    assert!(!super::meeting_allows(&key, &actor, true));
    assert_eq!(super::apply(&key, &actor, &[0, 0]).unwrap_err().code, 403);

    // the leg left: dropped at the next tick
    set(Ok((false, false)));
    super::recheck_meeting("MEETCALL");
    super::tick();
    assert!(super::take_events().iter().any(|event| matches!(event,
        super::Event::Revoked { user_id, .. } if *user_id == actor)));
    assert!(!super::meeting_allows(&key, &actor, false));
    super::leave(&key);
    super::tick();
    let _ = super::take_events();
}

/// A board page in a space of its own, and its document key.
fn board_page(space: &str, scene: &serde_json::Value) -> String {
    call(
        "org.moqui.resource.WikiServices.create#WikiSpace",
        &[("wikiSpaceId", Value::string(space))],
    );
    let saved = call(
        "org.moqui.resource.WikiServices.update#WikiPage",
        &[
            ("wikiSpaceId", Value::string(space)),
            ("pageName", Value::string("Board")),
            ("wikiType", Value::string("excalidraw")),
            ("pageText", Value::string(scene.to_string())),
        ],
    );
    format!("WikiPage:{}", to_plain_string(saved.get("wikiPageId").unwrap()))
}

/// A client's change to a board: `edit` seeded over what it has, the update it sends.
fn board_edit(doc: &Doc, edit: &serde_json::Value) -> Vec<u8> {
    let before = doc.transact().state_vector();
    super::board::seed(doc, &edit.to_string()).unwrap();
    doc.transact().encode_state_as_update_v1(&before)
}

fn room_render(key: &str) -> String {
    super::board::render(&super::lock(&super::room_of(key).unwrap()).doc)
}

/// SEC-TLS-113: a board update past board@max-update-bytes, or one that takes the document past
/// board@max-document-bytes, is refused (413); an update that is not one leaves the room as it was, and the next good
/// update is taken.
#[test]
fn a_board_holds_its_update_and_document_limits_and_survives_a_malformed_update() {
    use serde_json::json;
    let _serial = serial();
    let (context, facade) = setup_with_conf(BOARD_CONF);
    let _entered = context.enter();
    user(&facade, "limiter");
    login("limiter");
    let scene = json!({"type":"excalidraw","elements":[{"id":"a","type":"text","text":"Start","index":"a0","version":1}],"files":{}});
    let key = board_page("LIMITS", &scene);
    let document = client(&super::join(&key, "limiter", &[]).unwrap());
    let before = room_render(&key);
    assert_eq!(
        super::apply(&key, "limiter", &[0xff, 0x01, 0x02]).unwrap_err().code,
        400
    );
    assert_eq!(room_render(&key), before, "the malformed update changed nothing");
    let big = json!({"type":"excalidraw","elements":[
        {"id":"b","type":"text","text":"x".repeat(3000),"index":"a1","version":1}
    ]});
    let large = board_edit(&document, &big);
    let limited = |selector: &str| {
        let setting = moqui_core::context::conf_env::env_name_of(selector);
        moqui_lib::env::set_property(&setting, "1024");
        let refused = super::apply(&key, "limiter", &large);
        moqui_lib::env::clear_property(&setting);
        refused.unwrap_err().code
    };
    assert_eq!(limited("board@max-update-bytes"), 413);
    assert_eq!(limited("board@max-document-bytes"), 413);
    super::apply(&key, "limiter", &large).unwrap();
    assert!(
        super::snapshot(&key).unwrap().contains("xxx"),
        "the next good update is taken"
    );
    super::leave_as(&key, Some("limiter"));
    super::tick();
    let _ = super::take_events();
}

/// A board room that closes with every change in a version has its row's state cleared, and the next join seeds the
/// board from the saved scene: its edit is there, and its deleted element stays deleted though a client that still
/// holds it sends it again.
#[test]
fn a_closed_board_is_seeded_again_from_its_saved_scene() {
    use serde_json::json;
    let _serial = serial();
    let (context, facade) = setup_with_conf(BOARD_CONF);
    let _entered = context.enter();
    user(&facade, "compactor");
    login("compactor");
    let scene = json!({"type":"excalidraw","elements":[
        {"id":"keep","type":"text","text":"Keep","index":"a0","version":1,"versionNonce":1},
        {"id":"drop","type":"text","text":"Drop","index":"a1","version":1,"versionNonce":1}
    ],"files":{}});
    let key = board_page("COMPACT", &scene);
    let document = client(&super::join(&key, "compactor", &[]).unwrap());
    let edit = json!({"type":"excalidraw","elements":[
        {"id":"keep","type":"text","text":"Kept","index":"a0","version":2,"versionNonce":1},
        {"id":"drop","type":"text","text":"Drop","index":"a1","version":2,"versionNonce":1,"isDeleted":true}
    ]});
    super::apply(&key, "compactor", &board_edit(&document, &edit)).unwrap();
    super::leave_as(&key, Some("compactor"));
    super::tick();
    assert!(super::room_of(&key).is_none(), "the room closed");
    let row = super::stored(&key).unwrap().unwrap();
    assert!(super::bytes_of(&row, "stateData").is_empty(), "its state was cleared");
    let again = client(&super::join(&key, "compactor", &[]).unwrap());
    assert_eq!(super::board::search_text(&again), "Kept");
    let stale = Doc::new();
    super::board::seed(&stale, &scene.to_string()).unwrap();
    let everything = stale.transact().encode_state_as_update_v1(&StateVector::default());
    super::apply(&key, "compactor", &everything).unwrap();
    assert_eq!(super::snapshot(&key).unwrap(), "Kept");
    super::leave_as(&key, Some("compactor"));
    super::tick();
    let _ = super::take_events();
}

/// A room's space changed (a member left its chat room, the room was archived): the editors of its boards open here are
/// asked again at the next tick, not a minute later (`recheck_space`). A board whose last editor can no longer save it
/// still closes when its last member leaves, its state kept in the row for the next save.
#[test]
fn a_space_recheck_asks_its_editors_again_and_a_board_closes_when_no_editor_can_save_it() {
    use serde_json::json;
    let _serial = serial();
    let (context, facade) = setup_with_conf(BOARD_CONF);
    let _entered = context.enter();
    user(&facade, "roomowner");
    login("roomowner");
    let scene = json!({"type":"excalidraw","elements":[{"id":"a","type":"text","text":"Start","index":"a0","version":1}],"files":{}});
    let key = board_page("ROOMBRD", &scene);
    struct Answer;
    impl Drop for Answer {
        fn drop(&mut self) {
            *super::lock(&super::ACCESS_FOR_TEST) = None;
        }
    }
    let _answer = Answer;
    let set = |answer: Option<Result<(bool, bool), u16>>| *super::lock(&super::ACCESS_FOR_TEST) = answer;
    set(Some(Ok((true, true))));
    // an editor with no account here: no version can be saved as them
    let joined = super::join(&key, "departed", &[]).unwrap();
    let doc = client(&joined);
    let mut edit = scene.clone();
    edit["elements"][0]["version"] = json!(2);
    edit["elements"][0]["text"] = json!("Changed");
    super::apply(&key, "departed", &board_edit(&doc, &edit)).unwrap();
    super::tick();
    let _ = super::take_events();

    // another space's change asks nothing; this one's tells the lost edit right at once
    set(Some(Ok((true, false))));
    super::recheck_space("OTHERBRD");
    super::tick();
    assert!(!super::take_events()
        .iter()
        .any(|event| matches!(event, super::Event::CanEdit { .. })));
    super::recheck_space("ROOMBRD");
    super::tick();
    assert!(super::take_events().iter().any(|event| matches!(event,
        super::Event::CanEdit { user_id, can_edit: false, .. } if user_id == "departed")));

    // the last member leaves: the save as them fails, the room closes anyway, and the change waits in the row
    super::leave_as(&key, Some("departed"));
    super::tick();
    assert!(super::room_of(&key).is_none(), "a room no editor can save still closes");
    let row = super::stored(&key).unwrap().expect("the state is kept");
    assert!(to_plain_string(row.get("editorUserIds").unwrap()).contains("departed"));
    set(None);
    super::join(&key, "roomowner", &[]).unwrap();
    assert!(room_render(&key).contains("Changed"), "the next join has the change");
    super::leave(&key);
    super::tick();
    let _ = super::take_events();
}
