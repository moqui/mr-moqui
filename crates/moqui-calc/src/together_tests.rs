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

use serde_json::json;

use super::*;
use crate::fill::{self, CellData, SheetRows};
use crate::room::{DataSheet, Opening};

fn orders(amounts: &[f64], region: &str) -> SheetRows {
    SheetRows {
        name: "Orders".to_string(),
        columns: vec!["region".to_string(), "amount".to_string()],
        rows: amounts
            .iter()
            .map(|amount| vec![CellData::Text(format!("{region} customer")), CellData::Number(*amount)])
            .collect(),
        truncated: false,
    }
}

/// A room of the document `doc-1` for a scope, holding `rows`.
fn scope_room(rooms: &Rooms, user: &str, scope: &str, rows: SheetRows) -> SharedRoom {
    rooms
        .open(Opening {
            user_id: user.to_string(),
            shared: Some((crate::room::document_room_id("doc-1", Some("sales"), scope), 0)),
            profile_id: Some("sales".to_string()),
            definition_id: Some("doc-1".to_string()),
            version: Some(1),
            title: "Orders".to_string(),
            data_sheets: vec![DataSheet {
                name: "Orders".to_string(),
                spec: json!({"name": "Orders", "source": {"type": "entity", "entityName": "test.Order"}}),
            }],
            charts: Vec::new(),
            base: fill::new_model("Orders").unwrap(),
            rows: vec![rows],
        })
        .unwrap()
}

/// The rows the log would hold as two people of two scopes edit, applied by each room the way an edit of the log's
/// writer is (its own [`Room::apply_ops`]) or another's is ([`Room::apply_row`]).
#[test]
fn rooms_of_two_scopes_follow_one_log_each_over_its_own_rows_and_undo_with_what_each_took() {
    let rooms = Rooms::default();
    let west = scope_room(&rooms, "ann", "west", orders(&[10.0, 20.0, 30.0], "West"));
    let east = scope_room(
        &rooms,
        "otto",
        "east",
        orders(&[100.0, 200.0, 300.0, 400.0, 500.0], "East"),
    );
    assert_eq!(rooms.document_rooms("doc-1").len(), 2);
    let mut log: Vec<LogRow> = Vec::new();
    // write a row as the writer's room applies it, and have the other room follow
    let mut write = |writer: &SharedRoom,
                     follower: &SharedRoom,
                     user: &str,
                     kind: &str,
                     ops: Vec<serde_json::Value>,
                     target: Option<i64>| {
        let seq = log.len() as i64 + 1;
        {
            let mut room = lock(writer);
            room.pause();
            if kind == log::UNDO {
                room.undo_row(target.unwrap()).unwrap();
            } else {
                let (applied, inverse, refused) = room.apply_ops(&ops);
                assert!(refused.is_none());
                assert_eq!(applied.len(), ops.len());
                room.record(seq, inverse);
            }
            room.seq = seq;
            room.evaluate();
        }
        let row = LogRow {
            seq,
            user_id: user.to_string(),
            kind: kind.to_string(),
            ops,
            target,
            undone: None,
        };
        let head = log::Head {
            last_seq: seq,
            saved_seq: 0,
            saved_version: Some(1),
        };
        follow(&mut lock(follower), std::slice::from_ref(&row), &head).unwrap();
        log.push(row);
    };
    // Ann (west) adds a summary over the table; each room sums its own rows
    write(
        &west,
        &east,
        "ann",
        log::EDIT,
        vec![
            json!({"op": "addSheet"}),
            json!({"op": "renameSheet", "sheet": 1, "name": "Summary"}),
            json!({"op": "input", "sheet": 1, "row": 1, "column": 1, "value": "=SUM(Orders[amount])"}),
            json!({"op": "input", "sheet": 1, "row": 2, "column": 1, "value": "Total"}),
        ],
        None,
    );
    let cell = |room: &SharedRoom, row, column| lock(room).model().get_formatted_cell_value(1, row, column).unwrap();
    assert_eq!(cell(&west, 1, 1), "60");
    assert_eq!(cell(&east, 1, 1), "1500");
    // Otto (east) overwrites the label; Ann sees it
    write(
        &east,
        &west,
        "otto",
        log::EDIT,
        vec![json!({"op": "input", "sheet": 1, "row": 2, "column": 1, "value": "Sum of orders"})],
        None,
    );
    assert_eq!(cell(&west, 2, 1), "Sum of orders");
    // Otto takes his edit back: each room restores the label from what it took, the same text in both
    write(&east, &west, "otto", log::UNDO, Vec::new(), Some(2));
    assert_eq!(cell(&west, 2, 1), "Total");
    assert_eq!(cell(&east, 2, 1), "Total");
    // and the totals are still each room's own, with none of the other's rows in either
    assert_eq!(cell(&west, 1, 1), "60");
    assert_eq!(cell(&east, 1, 1), "1500");
    for (room, other) in [(&west, "East customer"), (&east, "West customer")] {
        let stored = fill::open_stored(&lock(room).stored_form().unwrap()).unwrap();
        assert!(stored.workbook.shared_strings.iter().all(|text| text != other));
        assert_eq!(lock(room).seq, 3);
    }
    // the log rows hold the edits as typed, and nothing read from a room's data
    let written = serde_json::to_string(&log.iter().map(|row| &row.ops).collect::<Vec<_>>()).unwrap();
    assert!(written.contains("SUM(Orders[amount])"));
    assert!(!written.contains("customer") && !written.contains("1500") && !written.contains("60"));

    // a room behind a row it no longer has, or a revert, cannot follow: it is opened anew
    let head = log::Head {
        last_seq: 9,
        saved_seq: 0,
        saved_version: Some(1),
    };
    let gap = LogRow {
        seq: 9,
        ..log[0].clone()
    };
    assert!(follow(&mut lock(&west), &[gap], &head).is_err());
    let revert = LogRow {
        seq: 4,
        kind: log::REVERT.to_string(),
        ..log[0].clone()
    };
    assert!(follow(&mut lock(&east), &[revert], &head).is_err());
}

#[test]
fn the_others_in_a_document_are_seen_by_name_and_place_and_one_who_left_is_not() {
    let presence = Presence::default();
    presence.set(
        "doc-1",
        "c1",
        "ann",
        "Ann",
        Some(1),
        json!({"row": 2, "column": 3, "width": 1, "height": 1}),
    );
    presence.set("doc-1", "c2", "otto", "Otto", Some(0), json!(null));
    presence.set("doc-2", "c3", "vic", "Vic", None, json!(null));
    let seen = presence.peers("doc-1", "c1");
    assert_eq!(seen.len(), 1, "not oneself, nor another document's");
    assert_eq!(seen[0]["name"], "Otto");
    assert_eq!(seen[0]["sheet"], 0);
    let ann = &presence.peers("doc-1", "c2")[0];
    assert_eq!(
        (ann["userId"].as_str(), ann["area"]["column"].as_i64()),
        (Some("ann"), Some(3))
    );
    presence.leave("doc-1", "c2");
    assert!(presence.peers("doc-1", "c1").is_empty());
}
