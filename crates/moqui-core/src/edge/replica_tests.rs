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
use crate::context::ExecutionContextFactory;
use crate::edge::ReplicaEntity;

const GROUP: &str = "moqui.security.UserGroup";

fn open() -> (ExecutionContext, EntityFacade) {
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

/// Write a group in a transaction of its own, committed unless `roll_back`.
fn write_group(facade: &EntityFacade, id: &str, description: &str, roll_back: bool) {
    let tx = facade.transactions_for(GROUP).unwrap();
    let _ = tx.run(TransactionAttribute::UseOrBegin, None, || {
        let mut row = facade.make_value(GROUP)?;
        row.set("userGroupId", Value::string(id));
        row.set("description", Value::string(description));
        row.create_or_update(facade)?;
        if roll_back {
            return Err(moqui_lib::error::Error::new("roll back"));
        }
        Ok(())
    });
}

fn key(id: &str) -> String {
    format!(r#"{{"userGroupId":"{id}"}}"#)
}

#[test]
fn a_committed_write_is_noted_after_commit_and_a_rolled_back_one_is_not() {
    let (_ctx, facade) = open();
    let replica = enable(&facade, &[groups()]).unwrap();
    assert!(
        matches!(replica.next(0), Work::Snapshot { .. }),
        "the first send is whole"
    );
    replica.snapshot_done(0, "e1", 1, 0);
    assert_eq!(replica.next(0), Work::Idle, "probed by the full send");
    replica.link_up();
    assert_eq!(
        replica.next(0),
        Work::Probe {
            epoch: "e1".to_string(),
            acked: 1
        }
    );
    replica.acked(0, "e1", 1);

    write_group(&facade, "EDGE_T_ROLLED", "never", true);
    assert_eq!(replica.next(0), Work::Idle);
    write_group(&facade, "EDGE_T_ONE", "One", false);
    let Work::Changes { epoch, acked, keys } = replica.next(0) else {
        panic!("the committed write's key")
    };
    assert_eq!((epoch.as_str(), acked), ("e1", 1));
    assert_eq!(keys, [(GROUP.to_string(), key("EDGE_T_ONE"))]);
    let changes = read_changes(&facade, &groups(), &keys).unwrap();
    assert_eq!(changes[0].fields_json.as_deref(), Some(r#"{"description":"One"}"#));

    // a batch that was not acknowledged waits again, and a delete reads as gone
    replica.put_back(0);
    facade
        .find(GROUP)
        .unwrap()
        .eq("userGroupId", Value::string("EDGE_T_ONE"))
        .disable_authz()
        .delete_all()
        .unwrap();
    let Work::Changes { keys, .. } = replica.next(0) else {
        panic!("the key again")
    };
    assert_eq!(keys.len(), 1);
    let changes = read_changes(&facade, &groups(), &keys).unwrap();
    assert_eq!(changes[0].fields_json, None);
    assert_eq!(replica.status()[0].waiting, 1, "on its way");
    // its link ended before the answer: the next link sends it again
    replica.link_up();
    replica.acked(0, "e1", 1);
    let Work::Changes { keys: again, .. } = replica.next(0) else {
        panic!("the key a third time")
    };
    assert_eq!(again, keys);
    replica.acked(0, "e1", 2);
    assert_eq!(replica.status()[0].waiting, 0);

    replica.resync_all();
    assert!(matches!(replica.next(0), Work::Snapshot { .. }));
}

#[test]
fn a_full_send_reads_the_set_in_pages_after_the_last_key() {
    let (_ctx, facade) = open();
    for id in ["EDGE_T_P1", "EDGE_T_P2", "EDGE_T_P3"] {
        write_group(&facade, id, "Page", false);
    }
    let first = snapshot_page(&facade, &groups(), 0, None).unwrap();
    let ours = first
        .changes
        .iter()
        .find(|change| change.pk_json == key("EDGE_T_P1"))
        .unwrap();
    assert_eq!(ours.entity_name, GROUP);
    assert_eq!(ours.fields_json.as_deref(), Some(r#"{"description":"Page"}"#));
    // the page after a key starts past it, whatever was deleted before it meanwhile
    let after = vec![("userGroupId".to_string(), Value::string("EDGE_T_P1"))];
    facade
        .find(GROUP)
        .unwrap()
        .eq("userGroupId", Value::string("EDGE_T_P1"))
        .disable_authz()
        .delete_all()
        .unwrap();
    let next = snapshot_page(&facade, &groups(), 0, Some(&after)).unwrap();
    let keys: Vec<&str> = next.changes.iter().map(|change| change.pk_json.as_str()).collect();
    assert!(keys.contains(&key("EDGE_T_P2").as_str()) && keys.contains(&key("EDGE_T_P3").as_str()));
    assert!(next
        .changes
        .iter()
        .all(|change| change.pk_json.as_str() > key("EDGE_T_P1").as_str()));
    assert!(snapshot_page(&facade, &groups(), 1, None).unwrap().changes.is_empty());
}

#[test]
fn a_delete_or_a_resync_during_a_full_send_is_sent_after_it() {
    let (_ctx, facade) = open();
    let replica = enable(&facade, &[groups()]).unwrap();
    let Work::Snapshot { epoch, resyncs } = replica.next(0) else {
        panic!("a full send")
    };
    // while the pages go: a record is deleted, and the operator asks for a resync
    write_group(&facade, "EDGE_T_GONE", "Gone", false);
    facade
        .find(GROUP)
        .unwrap()
        .eq("userGroupId", Value::string("EDGE_T_GONE"))
        .disable_authz()
        .delete_all()
        .unwrap();
    replica.snapshot_done(0, &epoch, 3, resyncs);
    let Work::Changes { keys, acked, .. } = replica.next(0) else {
        panic!("the delete after the full send")
    };
    assert_eq!(acked, 3);
    assert_eq!(keys, [(GROUP.to_string(), key("EDGE_T_GONE"))]);
    let changes = read_changes(&facade, &groups(), &keys).unwrap();
    assert_eq!(changes[0].fields_json, None, "sent as a delete");
    replica.acked(0, &epoch, 4);
    assert_eq!(replica.next(0), Work::Idle);
    replica.resync_all();
    let Work::Snapshot { resyncs, .. } = replica.next(0) else {
        panic!("a full send asked for")
    };
    replica.resync(0);
    replica.snapshot_done(0, "e2", 1, resyncs);
    assert!(
        matches!(replica.next(0), Work::Snapshot { .. }),
        "a resync asked for during a full send sends the set whole again"
    );
}

#[test]
fn a_set_held_back_or_let_go_is_sent_whole_once_against_its_last_full_send() {
    let (_ctx, facade) = open();
    let replica = enable(&facade, &[groups()]).unwrap();
    assert!(!replica.hold(0, true), "no full send yet: one is due anyway");
    let Work::Snapshot { epoch, resyncs } = replica.next(0) else {
        panic!("a full send")
    };
    replica.sent_whole(0, false);
    replica.snapshot_done(0, &epoch, 2, resyncs);
    replica.acked(0, &epoch, 2);
    assert!(!replica.hold(0, false), "as last sent: nothing due");
    assert_eq!(replica.next(0), Work::Idle);
    // held back since (the setting changed, by Apply or by hand): a full send, once
    assert!(replica.hold(0, true));
    assert!(!replica.hold(0, true), "due already");
    let Work::Snapshot { epoch, resyncs } = replica.next(0) else {
        panic!("a full send, empty")
    };
    replica.sent_whole(0, true);
    replica.snapshot_done(0, &epoch, 1, resyncs);
    assert!(!replica.hold(0, true));
    assert!(replica.hold(0, false), "let go again: sent whole");
}

#[test]
fn a_batch_goes_in_parts_by_size() {
    let changes: Vec<Change> = (0..5)
        .map(|n| Change {
            entity_name: "e".to_string(),
            pk_json: format!("{n}"),
            fields_json: Some("x".repeat(8)),
        })
        .collect();
    let parts = split_by_size(changes, 20);
    let sizes: Vec<usize> = parts.iter().map(Vec::len).collect();
    assert_eq!(sizes, [2, 2, 1]);
    let big = vec![Change {
        entity_name: "e".to_string(),
        pk_json: "1".to_string(),
        fields_json: Some("x".repeat(100)),
    }];
    assert_eq!(
        split_by_size(big, 20).len(),
        1,
        "a record larger than a part is a part alone"
    );
}

fn change(id: &str, description: Option<&str>) -> Change {
    Change {
        entity_name: GROUP.to_string(),
        pk_json: key(id),
        fields_json: description.map(|text| format!(r#"{{"description":"{text}"}}"#)),
    }
}

fn batch(epoch: &str, base_seq: u64, seq: u64, snapshot: bool, changes: Vec<Change>) -> Batch {
    Batch {
        sender_token: 0,
        set_name: "groups".to_string(),
        epoch: epoch.to_string(),
        base_seq,
        seq,
        snapshot,
        snapshot_end: snapshot,
        changes,
    }
}

#[test]
fn the_edge_node_applies_batches_that_follow_its_cursor_and_asks_for_the_rest_whole() {
    let (_ctx, facade) = open();
    let mut client = facade.make_value("moqui.edge.EdgeClient").unwrap();
    client.set("pairedNodeId", Value::string("local-r"));
    client.set("keyFingerprint", Value::string("ab"));
    client.create(&facade).unwrap();
    let node = "local-r";
    // no cursor: only a full send is taken
    let early = batch("e1", 0, 1, false, vec![change("A", Some("a"))]);
    assert_eq!(apply_batch(&facade, node, &early).unwrap(), BatchAnswer::Resync);
    let full = batch("e1", 0, 1, true, vec![change("A", Some("a")), change("B", Some("b"))]);
    assert_eq!(apply_batch(&facade, node, &full).unwrap(), BatchAnswer::Applied);
    let next = batch("e1", 1, 2, false, vec![change("A", Some("a2")), change("B", None)]);
    assert_eq!(apply_batch(&facade, node, &next).unwrap(), BatchAnswer::Applied);
    assert_eq!(
        apply_batch(&facade, node, &next).unwrap(),
        BatchAnswer::Applied,
        "a batch applied before"
    );
    let gap = batch("e1", 5, 6, false, Vec::new());
    assert_eq!(apply_batch(&facade, node, &gap).unwrap(), BatchAnswer::Resync);
    let other_epoch = batch("e0", 2, 3, false, Vec::new());
    assert_eq!(apply_batch(&facade, node, &other_epoch).unwrap(), BatchAnswer::Resync);

    let pk = BTreeMap::from([("userGroupId".to_string(), "A".to_string())]);
    let a = get_value(&facade, Some(node), "groups", GROUP, &pk).unwrap().unwrap();
    assert_eq!(a.fields, serde_json::json!({"description": "a2"}));
    assert_eq!(list_values(&facade, None, "groups", None, 10).unwrap().len(), 1);

    // a new full send keeps only what it sent
    let again = batch("e2", 0, 1, true, vec![change("C", Some("c"))]);
    assert_eq!(apply_batch(&facade, node, &again).unwrap(), BatchAnswer::Applied);
    let values = list_values(&facade, Some(node), "groups", Some(GROUP), 10).unwrap();
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].pk, serde_json::json!({"userGroupId": "C"}));
    let cursors = cursors(&facade, Some(node)).unwrap();
    assert_eq!(cursors.len(), 1);
    assert_eq!(
        (cursors[0].epoch.as_str(), cursors[0].seq, cursors[0].values),
        ("e2", 1, 1)
    );
    assert!(cursors[0].snapshot_date.is_some());
}

/// A member that lost the sender lock and has not noticed yet cannot cross the new sender's batches (plans/Edge-Node2.md
/// §4.4): the edge node refuses a batch whose sender took the lock before the one that last wrote the set's cursor.
#[test]
fn the_edge_node_refuses_a_batch_from_an_earlier_sender() {
    let (_ctx, facade) = open();
    let mut client = facade.make_value("moqui.edge.EdgeClient").unwrap();
    client.set("pairedNodeId", Value::string("local-f"));
    client.set("keyFingerprint", Value::string("ab"));
    client.create(&facade).unwrap();
    let node = "local-f";
    let token = |token: u64, batch: Batch| Batch {
        sender_token: token,
        ..batch
    };
    // the old sender's full send, then the new sender's under a later token
    let old = token(100, batch("e1", 0, 1, true, vec![change("A", Some("a"))]));
    assert_eq!(apply_batch(&facade, node, &old).unwrap(), BatchAnswer::Applied);
    let new = token(200, batch("e2", 0, 1, true, vec![change("A", Some("b"))]));
    assert_eq!(apply_batch(&facade, node, &new).unwrap(), BatchAnswer::Applied);
    // the old sender's next batch, or a full send of its own, is refused
    let late = token(100, batch("e1", 1, 2, false, vec![change("A", Some("stale"))]));
    let err = apply_batch(&facade, node, &late).unwrap_err();
    assert!(err.message().contains("newer replication sender"), "{err}");
    let restart = token(100, batch("e3", 0, 1, true, vec![change("A", Some("stale"))]));
    assert!(apply_batch(&facade, node, &restart).is_err());
    // the new sender goes on
    let next = token(200, batch("e2", 1, 2, false, vec![change("A", Some("c"))]));
    assert_eq!(apply_batch(&facade, node, &next).unwrap(), BatchAnswer::Applied);
}
