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

/// SEC-CLU-90: a dedicated edge node keeps each pairing's greetings and prompts under that pairing's own directory, as
/// an all-in-one node does, never at the shared location another pairing's calls read; one file is at most 8 MiB, and a
/// pairing's kept audio stays within its queue cap.
#[test]
fn each_pairings_call_audio_is_kept_apart_and_capped() {
    use moqui_lib::object::Value;
    let greeting = "dbresource://moqui/call/voicemail/greeting/B1/greeting-1.ogg";
    assert_ne!(super::kept_audio_location("local-a", greeting), greeting);
    assert_ne!(
        super::kept_audio_location("local-a", greeting),
        super::kept_audio_location("local-b", greeting)
    );
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let settings = crate::edge::EdgeSettings {
        role: crate::edge::NodeRole::FrontEdge,
        max_bytes: 100,
        ..crate::edge::EdgeSettings::default()
    };
    ctx.factory().extension(|_| settings.clone());
    let mut client = facade.make_value("moqui.edge.EdgeClient").unwrap();
    client.set("pairedNodeId", Value::string("local-a"));
    client.set("keyFingerprint", Value::string("ab"));
    client.set("statusId", Value::string("EdgeClientActive"));
    client.create(&facade).unwrap();
    super::check_kept_audio(ctx.factory(), "local-a", 60).unwrap();
    assert!(super::check_kept_audio(ctx.factory(), "local-a", super::MAX_KEPT_AUDIO + 1).is_err());
    let kept = super::kept_audio_location("local-a", greeting);
    let _authz = ctx.disable_authz();
    let keep = |location: &str| {
        super::super::queue::transaction(&facade, crate::transaction::TransactionAttribute::UseOrBegin, || {
            ctx.factory()
                .resource()
                .write_from(location, &mut std::io::Cursor::new(vec![0_u8; 60]), Some(60))
                .map_err(moqui_lib::error::Error::from)
        })
        .unwrap();
    };
    // the pairing's fallback names the greeting: it is kept, and the cap holds against it
    let mut copy = facade.make_value(super::super::replica::VALUE).unwrap();
    copy.set("pairedNodeId", Value::string("local-a"));
    copy.set("setName", Value::string("voiceFallbacks"));
    copy.set("entityName", Value::string("moqui.call.PhoneNumberFallback"));
    copy.set("pkHash", Value::string("h1"));
    copy.set("pkJson", Value::string(r#"{"phoneNumber":"+15550160001"}"#));
    copy.set(
        "fieldsJson",
        Value::string(format!(r#"{{"greetingLocation":"{greeting}"}}"#)),
    );
    copy.create(&facade).unwrap();
    keep(&kept);
    let over = super::check_kept_audio(ctx.factory(), "local-a", 60).unwrap_err();
    assert!(over.contains("queue cap"), "{over}");
    // a greeting no fallback names any more (the box changed it) is removed to make room
    let stale = super::kept_audio_location("local-a", "dbresource://moqui/call/voicemail/greeting/B1/old.ogg");
    keep(&stale);
    copy.set("fieldsJson", Value::string(r#"{"greetingLocation":""}"#));
    copy.update(&facade).unwrap();
    super::check_kept_audio(ctx.factory(), "local-a", 60).unwrap();
    let exists = |location: &str| {
        super::super::queue::transaction(&facade, crate::transaction::TransactionAttribute::UseOrBegin, || {
            Ok(ctx.factory().resource().stat(location)?.is_some())
        })
        .unwrap()
    };
    assert!(!exists(&stale) && !exists(&kept), "neither is named now");
    assert!(
        super::check_kept_audio(ctx.factory(), "local-b", 60).is_ok(),
        "another pairing's own"
    );
}

/// A local node keeps the TURN secret of each edge cluster member its links reached (plans/Edge-Node2.md §4.6): each
/// member's calls sign their credentials with it; a member whose TURN is off is forgotten, and unpairing forgets all.
#[test]
fn each_edge_members_turn_secret_is_kept_apart() {
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let factory = ctx.factory();
    super::set_peer_turn(factory, "edge-1", "secret-a", "local-a");
    super::set_peer_turn(factory, "edge-2", "secret-a", "local-a");
    assert_eq!(
        super::turn_peer(factory, "edge-1"),
        Some(("secret-a".to_string(), "local-a".to_string()))
    );
    assert_eq!(super::turn_secret(factory, "edge-2").as_deref(), Some("secret-a"));
    assert_eq!(super::turn_secret(factory, "other"), None, "not a member");
    assert_eq!(super::turn_secret(factory, ""), None);
    super::set_peer_turn(factory, "edge-1", "", "local-a");
    assert_eq!(super::turn_secret(factory, "edge-1"), None, "its TURN is off");
    assert!(
        super::turn_secret(factory, "edge-2").is_some(),
        "the other member's stays"
    );
    super::clear_peer_turn(factory);
    assert_eq!(super::turn_secret(factory, "edge-2"), None);
}

/// A member gone from the edge cluster's list has its TURN secret forgotten; the others stay.
#[test]
fn a_member_gone_from_the_list_has_its_turn_secret_forgotten() {
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let factory = ctx.factory();
    super::set_peer_turn(factory, "edge-1", "secret-a", "local-a");
    super::set_peer_turn(factory, "edge-2", "secret-a", "local-a");
    super::retain_peer_turns(factory, &["edge-2".to_string()]);
    assert_eq!(super::turn_secret(factory, "edge-1"), None);
    assert!(super::turn_secret(factory, "edge-2").is_some());
}
