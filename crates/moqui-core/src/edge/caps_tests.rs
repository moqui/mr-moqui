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
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::edge::replica::Change;
use crate::edge::NodeRole;

const SET: &str = "groups";
const GROUP: &str = "moqui.security.UserGroup";
const MESSAGE: &str = "moqui.security.user.NotificationMessage";

/// An edge node's context with pairing `local-a` and the pause notice's topic.
fn open() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    let mut client = facade.make_value(CLIENT).unwrap();
    client.set("pairedNodeId", Value::string("local-a"));
    client.set("keyFingerprint", Value::string("ab"));
    client.set("statusId", Value::string("EdgeClientActive"));
    client.create(&facade).unwrap();
    for name in [PAUSED_TOPIC, REFUSED_TOPIC] {
        let mut topic = facade.make_value("moqui.security.user.NotificationTopic").unwrap();
        topic.set("topic", Value::string(name));
        topic.set("persistOnSend", Value::string("Y"));
        topic.create(&facade).unwrap();
    }
    (ctx, facade)
}

fn edge(max_rows: u64, max_bytes: u64) -> EdgeSettings {
    EdgeSettings {
        role: NodeRole::FrontEdge,
        replica_max_rows: max_rows,
        max_bytes,
        ..EdgeSettings::default()
    }
}

fn key(id: &str) -> String {
    format!(r#"{{"userGroupId":"{id}"}}"#)
}

fn write(id: &str) -> Change {
    Change {
        entity_name: GROUP.to_string(),
        pk_json: key(id),
        fields_json: Some(r#"{"description":"x"}"#.to_string()),
    }
}

fn delete(id: &str) -> Change {
    Change {
        entity_name: GROUP.to_string(),
        pk_json: key(id),
        fields_json: None,
    }
}

fn batch(changes: Vec<Change>) -> Batch {
    Batch {
        set_name: SET.to_string(),
        epoch: "e1".to_string(),
        base_seq: 1,
        seq: 2,
        snapshot: false,
        snapshot_end: false,
        changes,
        sender_token: 0,
    }
}

/// Keep record `id` for local-a, as an applied batch would.
fn keep(facade: &EntityFacade, id: &str) {
    let pk: serde_json::Value = serde_json::from_str(&key(id)).unwrap();
    let mut row = facade.make_value(VALUE).unwrap();
    row.set("pairedNodeId", Value::string("local-a"));
    row.set("setName", Value::string(SET));
    row.set("entityName", Value::string(GROUP));
    row.set("pkHash", Value::string(canonical_sha256(&pk)));
    row.set("pkJson", Value::string(key(id)));
    row.set("fieldsJson", Value::string(r#"{"description":"x"}"#));
    row.create(facade).unwrap();
}

fn notices(facade: &EntityFacade) -> i64 {
    notices_of(facade, PAUSED_TOPIC)
}

fn notices_of(facade: &EntityFacade, topic: &str) -> i64 {
    facade
        .find(MESSAGE)
        .unwrap()
        .eq("topic", Value::string(topic))
        .disable_authz()
        .count()
        .unwrap()
}

fn is_paused(admit: &Admit) -> bool {
    matches!(admit, Admit::Paused(_))
}

/// SEC-CLU-02: a batch that would take a pairing's replicated records past its cap pauses the pairing, with one
/// notice to the administrators; while paused every batch and every content is refused for now; a resume lets it
/// send again (and a send still over the cap pauses it again), as do new caps.
#[test]
fn a_pairing_at_its_record_cap_pauses_with_a_notice_until_an_administrator_resumes_it() {
    let (ctx, facade) = open();
    let _entered = ctx.enter();
    let settings = edge(3, 1024 * 1024);
    keep(&facade, "a");
    keep(&facade, "b");
    let take = |changes: Vec<Change>| admit_batch(&facade, &settings, "local-a", &batch(changes)).unwrap();
    assert_eq!(take(vec![write("c")]), Admit::Take, "3 of 3");
    keep(&facade, "c");
    assert_eq!(
        take(vec![write("a"), write("b")]),
        Admit::Take,
        "updates of kept records add none"
    );
    assert_eq!(take(vec![delete("a"), write("d")]), Admit::Take, "a delete makes room");
    assert_eq!(take(vec![delete("a")]), Admit::Take);
    assert_eq!(notices(&facade), 0);

    let over = take(vec![write("d")]);
    assert!(is_paused(&over), "{over:?}");
    assert!(paused(&facade, "local-a").unwrap().is_some());
    assert_eq!(notices(&facade), 1, "the administrators are told");
    // paused: nothing is taken, not even a batch that adds no record, nor content; no second notice
    assert!(is_paused(&take(vec![delete("a")])));
    assert!(is_paused(&take(vec![write("d")])));
    assert!(is_paused(
        &admit_content(ctx.factory(), &facade, &settings, "local-a", 1).unwrap()
    ));
    assert_eq!(notices(&facade), 1);
    // another pairing is not held by this one's pause
    assert_eq!(
        admit_batch(&facade, &settings, "local-b", &batch(vec![write("z")])).unwrap(),
        Admit::Take
    );

    assert!(resume(&facade, "local-a").unwrap());
    assert!(!resume(&facade, "local-a").unwrap(), "not paused now");
    assert_eq!(take(vec![delete("a")]), Admit::Take, "resumed");
    assert!(is_paused(&take(vec![write("d")])), "still over its cap: paused again");
    assert_eq!(notices(&facade), 2);
    crate::edge::hosts::set_caps(&facade, "local-a", None, Some(10)).unwrap();
    assert_eq!(paused(&facade, "local-a").unwrap(), None, "new caps resume it");
    assert_eq!(
        take(vec![write("d"), write("e")]),
        Admit::Take,
        "under its own cap of 10"
    );
    assert!(resume(&facade, "local-x").is_err(), "no such pairing");
}

/// SEC-CLU-02: the content kept for a pairing's deliveries counts against its queue cap; past it the pairing pauses.
/// A local node refuses content its edge node sends past its own cap, for now, with no pause; its administrators are
/// told once when it starts refusing and once when it takes content again.
#[test]
fn content_kept_for_a_pairing_counts_against_its_queue_cap() {
    let (ctx, facade) = open();
    let _entered = ctx.enter();
    let settings = edge(1000, 100);
    crate::edge::queue::store_received(ctx.factory(), &facade, "local-a", "item-1", &[7; 60]).unwrap();
    assert_eq!(
        admit_content(ctx.factory(), &facade, &settings, "local-a", 40).unwrap(),
        Admit::Take,
        "100 of 100"
    );
    let over = admit_content(ctx.factory(), &facade, &settings, "local-a", 41).unwrap();
    assert!(is_paused(&over), "{over:?}");
    assert!(paused(&facade, "local-a").unwrap().is_some());
    assert_eq!(notices(&facade), 1);
    assert!(
        is_paused(&admit_batch(&facade, &settings, "local-a", &batch(vec![write("a")])).unwrap()),
        "its replication waits too"
    );

    let local = EdgeSettings {
        max_bytes: 100,
        ..EdgeSettings::default()
    };
    crate::edge::queue::store_received(ctx.factory(), &facade, "me", "item-2", &[7; 90]).unwrap();
    assert_eq!(
        admit_content(ctx.factory(), &facade, &local, "me", 10).unwrap(),
        Admit::Take
    );
    assert_eq!(notices_of(&facade, REFUSED_TOPIC), 0);
    assert!(is_paused(
        &admit_content(ctx.factory(), &facade, &local, "me", 11).unwrap()
    ));
    assert!(is_paused(
        &admit_content(ctx.factory(), &facade, &local, "me", 12).unwrap()
    ));
    assert_eq!(notices_of(&facade, REFUSED_TOPIC), 1, "told once as refusing starts");
    assert_eq!(
        admit_content(ctx.factory(), &facade, &local, "me", 10).unwrap(),
        Admit::Take,
        "a local node has no pause of its own"
    );
    assert_eq!(
        notices_of(&facade, REFUSED_TOPIC),
        2,
        "and once as it takes content again"
    );
    admit_content(ctx.factory(), &facade, &local, "me", 1).unwrap();
    assert_eq!(notices_of(&facade, REFUSED_TOPIC), 2);
}

/// SEC-CLU-02: the content files kept for a pairing count against `queue-max-content-files` whatever their size, so
/// empty ones add up too; at it the pairing pauses as at its byte cap.
#[test]
fn content_files_kept_for_a_pairing_count_against_their_own_cap() {
    let (ctx, facade) = open();
    let _entered = ctx.enter();
    let settings = EdgeSettings {
        max_content_files: 2,
        ..edge(1000, 1_000_000)
    };
    crate::edge::queue::store_received(ctx.factory(), &facade, "local-a", "item-1", &[]).unwrap();
    assert_eq!(
        admit_content(ctx.factory(), &facade, &settings, "local-a", 0).unwrap(),
        Admit::Take,
        "the second file"
    );
    crate::edge::queue::store_received(ctx.factory(), &facade, "local-a", "item-2", &[]).unwrap();
    let over = admit_content(ctx.factory(), &facade, &settings, "local-a", 0).unwrap();
    assert!(matches!(&over, Admit::Paused(why) if why.contains("files")), "{over:?}");
    assert!(paused(&facade, "local-a").unwrap().is_some());
    assert_eq!(notices(&facade), 1);
    let uncapped = EdgeSettings {
        max_content_files: 0,
        ..edge(1000, 1_000_000)
    };
    resume(&facade, "local-a").unwrap();
    assert_eq!(
        admit_content(ctx.factory(), &facade, &uncapped, "local-a", 0).unwrap(),
        Admit::Take,
        "0 is no cap"
    );
}
