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

use std::time::Duration;

use super::*;
use crate::context::ExecutionContextFactory;
use crate::edge::replica::{enable, Work};
use crate::edge::{ReplicaEntity, ReplicaSet};

const GROUP: &str = "moqui.security.UserGroup";

fn open() -> (crate::context::ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn groups() -> ReplicaSet {
    ReplicaSet {
        name: "groups".to_string(),
        entities: vec![ReplicaEntity {
            name: GROUP.to_string(),
            fields: vec!["description".to_string()],
        }],
    }
}

/// One member holds the lock at a time: it renews, the other is told who holds it, and takes it once it is let go or
/// its lease lapsed.
#[test]
fn one_member_holds_the_sender_lock_and_another_takes_it_over() {
    let (_ctx, facade) = open();
    let lease = Duration::from_secs(30);
    let Claim::Taken(first) = claim(&facade, REPLICATE, "m-1", lease).unwrap() else {
        panic!("taken")
    };
    assert_eq!(
        claim(&facade, REPLICATE, "m-1", lease).unwrap(),
        Claim::Renewed(first),
        "the same token"
    );
    assert_eq!(
        claim(&facade, REPLICATE, "m-2", lease).unwrap(),
        Claim::Held("m-1".to_string())
    );
    assert_eq!(
        holder(&facade, REPLICATE).unwrap().map(|(id, _)| id).as_deref(),
        Some("m-1")
    );
    // let go: the next claim takes it
    assert!(release(&facade, REPLICATE, "m-1").unwrap());
    assert!(!release(&facade, REPLICATE, "m-1").unwrap());
    std::thread::sleep(Duration::from_millis(5));
    let Claim::Taken(second) = claim(&facade, REPLICATE, "m-2", lease).unwrap() else {
        panic!("taken by m-2")
    };
    assert!(second > first, "a later sender's token is greater");
    // a lease that lapsed (the holder stopped without letting go) is taken over
    assert!(claim(&facade, REPLICATE, "m-2", Duration::from_millis(1))
        .unwrap()
        .holds());
    std::thread::sleep(Duration::from_millis(1100));
    assert!(matches!(
        claim(&facade, REPLICATE, "m-1", lease).unwrap(),
        Claim::Taken(_)
    ));
    assert!(claim(&facade, REPLICATE, "", lease).is_err());
}

/// A member keeps notes only while it is the sender, a new sender sends each set whole, and it takes the notes of
/// the other members (not its own again); one that is not the sender takes none.
#[test]
fn the_sender_keeps_notes_takes_the_others_and_starts_with_full_sends() {
    let (_ctx, facade) = open();
    let replica = enable(&facade, &[groups()]).unwrap();
    // the first full send, done
    let Work::Snapshot { epoch, resyncs } = replica.next(0) else {
        panic!("a first full send");
    };
    replica.snapshot_done(0, &epoch, 1, resyncs);
    replica.acked(0, &epoch, 1);
    let key = r#"{"userGroupId":"G1"}"#;
    let note = |from: &str| serde_json::json!({"from": from, "notes": [[GROUP, key]], "whole": []});

    // not the sender: nothing kept, the others' notes dropped
    assert!(!set_sender(&facade, None));
    assert!(!replica.keeps_notes());
    assert!(!is_sender(&facade));
    assert_eq!(take_notes(&facade, &note("m-2")), 0);
    // what it noted is gone: it is sent whole if this member is the sender again
    assert!(matches!(replica.next(0), Work::Snapshot { .. }));

    // the sender now: each set whole under a new epoch, then the others' notes are sent
    assert!(set_sender(&facade, Some("m-1")));
    assert!(is_sender(&facade) && replica.keeps_notes());
    let Work::Snapshot { epoch: second, resyncs } = replica.next(0) else {
        panic!("a new sender sends the set whole");
    };
    assert_ne!(second, epoch);
    replica.snapshot_done(0, &second, 1, resyncs);
    replica.acked(0, &second, 1);
    assert_eq!(take_notes(&facade, &note("m-2")), 1);
    assert_eq!(take_notes(&facade, &note("")), 0, "a note names its member");
    match replica.next(0) {
        Work::Changes { keys, .. } => assert_eq!(keys, vec![(GROUP.to_string(), key.to_string())]),
        other => panic!("the other member's note: {other:?}"),
    }
    // still the sender: no change, no full send
    assert!(!set_sender(&facade, Some("m-1")));
    // and letting go
    let_go(&facade);
    assert!(!is_sender(&facade));
    disable_for_test(&facade);
}

fn disable_for_test(facade: &EntityFacade) {
    super::super::disable(facade);
}
