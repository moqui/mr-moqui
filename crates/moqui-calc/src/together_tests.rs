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

/// A test context over the in-memory database, which holds the document's edit log.
fn log_context() -> (moqui_core::context::ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

/// plans/IronCalc-DB2.md CG1.3: under the log's lock the rows a room had not followed, and its own edit, are applied
/// with evaluation paused; the room evaluates them once, after the lock. Every room still ends with the same sheets.
#[test]
fn an_edit_and_a_save_follow_the_log_under_its_lock_and_evaluate_once_after_it() {
    let (_ctx, facade) = log_context();
    let rooms = Rooms::default();
    let west = scope_room(&rooms, "ann", "west", orders(&[10.0, 20.0, 30.0], "West"));
    let east = scope_room(&rooms, "otto", "east", orders(&[100.0, 200.0], "East"));
    let value = |room: &SharedRoom, row| lock(room).model().get_formatted_cell_value(1, row, 1).unwrap();
    // Ann adds a summary: a row of the log the east room has not followed
    let summary = [
        json!({"op": "addSheet"}),
        json!({"op": "renameSheet", "sheet": 1, "name": "Summary"}),
        json!({"op": "input", "sheet": 1, "row": 1, "column": 1, "value": "=SUM(Orders[amount])"}),
        json!({"op": "input", "sheet": 1, "row": 2, "column": 1, "value": "=A1*2"}),
    ];
    assert_eq!(edit(&facade, &mut lock(&west), "ann", &summary).unwrap().seq, 1);
    assert_eq!(value(&west, 2), "120");

    // Otto's edit follows Ann's row and applies his own under the lock, then the room evaluates both, once
    let before = lock(&east).evaluations();
    let otto = [json!({"op": "input", "sheet": 1, "row": 3, "column": 1, "value": "=A2+1"})];
    let edited = edit(&facade, &mut lock(&east), "otto", &otto).unwrap();
    assert_eq!((edited.seq, edited.kind), (2, log::EDIT));
    assert_eq!(
        lock(&east).evaluations(),
        before + 1,
        "one evaluation for the row followed and the edit"
    );
    // the row followed added a sheet: what the evaluation changed may be anything, and the views read everything again
    assert!(lock(&east).changed().is_none());
    assert_eq!(
        (value(&east, 1), value(&east, 2), value(&east, 3)),
        ("300".into(), "600".into(), "601".into())
    );
    // and his undo takes back his own edit with what the room took before it, the summary kept
    edit(&facade, &mut lock(&east), "otto", &[json!({"op": "undo"})]).unwrap();
    assert_eq!((value(&east, 2), value(&east, 3)), ("600".into(), String::new()));
    edit(&facade, &mut lock(&east), "otto", &[json!({"op": "redo"})]).unwrap();

    // a save follows Otto's rows with evaluation paused: no evaluation while the log is locked, one after
    let before = lock(&west).evaluations();
    let mut at_write = None;
    let saved = save(&facade, &mut lock(&west), "ann", |room, version| {
        at_write = Some((room.evaluations(), room.seq));
        Ok(version.map(|saved| saved + 1))
    })
    .unwrap();
    assert_eq!(saved, Some(2));
    assert_eq!(
        at_write,
        Some((before, 4)),
        "the rows followed, not yet evaluated, when the document is written"
    );
    assert_eq!(lock(&west).evaluations(), before + 1);
    assert_eq!(
        (value(&west, 1), value(&west, 2), value(&west, 3)),
        ("60".into(), "120".into(), "121".into())
    );
    // nothing left to follow: a save evaluates nothing
    let before = lock(&west).evaluations();
    save(&facade, &mut lock(&west), "ann", |_, version| Ok(version)).unwrap();
    assert_eq!(lock(&west).evaluations(), before);
    assert_eq!(unsaved(&facade, &lock(&west)), 0);

    // values only, the row followed and the edit: the one evaluation's changed areas hold both
    let input = |row: i32, value: &str| [json!({"op": "input", "sheet": 1, "row": row, "column": 1, "value": value})];
    edit(&facade, &mut lock(&west), "ann", &input(4, "5")).unwrap();
    edit(&facade, &mut lock(&east), "otto", &input(5, "6")).unwrap();
    let changed: Vec<Area> = lock(&east)
        .changed()
        .expect("value edits say what they changed")
        .to_vec();
    let covers = |row: i32| {
        changed
            .iter()
            .any(|area| area.sheet == 1 && area.column == 1 && area.row <= row && row < area.row + area.height)
    };
    assert!(covers(4) && covers(5), "{changed:?}");
}

/// plans/IronCalc-DB2.md CG2.4 to CG2.6: a border, a merge and a conditional format are edits of the document's log,
/// written by one room and followed by another scope's over its own rows, and taken back by their writer's undo in both.
#[test]
fn borders_merges_and_rules_go_through_the_log_and_their_undo_reaches_every_room() {
    let (_ctx, facade) = log_context();
    let rooms = Rooms::default();
    let west = scope_room(&rooms, "ann", "west", orders(&[10.0, 20.0, 30.0], "West"));
    let east = scope_room(&rooms, "otto", "east", orders(&[100.0, 200.0], "East"));
    let summary = [
        json!({"op": "addSheet"}),
        json!({"op": "renameSheet", "sheet": 1, "name": "Summary"}),
        json!({"op": "paste", "sheet": 1, "row": 1, "column": 1, "rows": [["Total", "=SUM(Orders[amount])"], ["Note"]]}),
    ];
    edit(&facade, &mut lock(&west), "ann", &summary).unwrap();
    let formats = [
        json!({"op": "border", "sheet": 1, "row": 1, "column": 1, "width": 2, "height": 1, "kind": "outer", "style": "medium"}),
        json!({"op": "merge", "sheet": 1, "row": 2, "column": 1, "width": 2, "height": 1}),
        json!({"op": "addRule", "sheet": 1, "range": "B1", "rule": {"type": "CellIs", "operator": "GreaterThan",
            "formula": "100", "format": {"fill": {"color": "#FFC7CE"}}, "stop_if_true": false}}),
    ];
    let edited = edit(&facade, &mut lock(&west), "ann", &formats).unwrap();
    assert!(edited.refused.is_none());
    assert!(catch_up(&facade, &mut lock(&east)).unwrap());
    let fill = |room: &SharedRoom| {
        let block: serde_json::Value = serde_json::from_str(&lock(room).block(1, 1, 2, 1, 1).unwrap()).unwrap();
        block["styles"][0]["style"]["fill"]["color"].clone()
    };
    // each room's rule over its own total: west's 60 is not over 100, east's 300 is
    assert!(fill(&west).is_null());
    assert_eq!(fill(&east), "#FFC7CE");
    for room in [&west, &east] {
        let room = lock(room);
        assert_eq!(room.merges_of(1, 1, 1, 10, 10).len(), 1);
        assert_eq!(room.layout()[1]["rules"].as_array().map(Vec::len), Some(1));
        let style = room.model().get_style_for_cell(1, 1, 1).unwrap();
        assert!(style.border.left.is_some() && style.border.right.is_none());
    }
    // the log holds the edits as sent, nothing of either room's data
    let written = serde_json::to_string(&log::rows_after(&facade, "doc-1", 0).unwrap()[1].ops).unwrap();
    assert!(written.contains("\"merge\"") && !written.contains("300") && !written.contains("customer"));

    // Ann's undo takes all three back, in her room and in Otto's
    edit(&facade, &mut lock(&west), "ann", &[json!({"op": "undo"})]).unwrap();
    assert!(catch_up(&facade, &mut lock(&east)).unwrap());
    for room in [&west, &east] {
        let room = lock(room);
        assert!(room.merges_of(1, 1, 1, 10, 10).is_empty());
        assert_eq!(room.layout()[1]["rules"], json!([]));
        assert!(room.model().get_style_for_cell(1, 1, 1).unwrap().border.left.is_none());
        assert_eq!(room.cell_content(1, 2, 1).unwrap(), "Note");
    }
    assert!(fill(&east).is_null());
}

/// An undo of a rule another has deleted since passes it by: the undo is written as usual, every room passes the same
/// step by, the person is told, and their next undo goes on.
#[test]
fn an_undo_of_a_rule_deleted_since_passes_it_by_and_the_stack_goes_on() {
    let (_ctx, facade) = log_context();
    let rooms = Rooms::default();
    let west = scope_room(&rooms, "ann", "west", orders(&[10.0, 20.0, 30.0], "West"));
    let east = scope_room(&rooms, "otto", "east", orders(&[100.0, 200.0], "East"));
    let rule = json!({"op": "addRule", "sheet": 1, "range": "A1:A3", "rule": {"type": "CellIs", "operator": "GreaterThan",
        "formula": "1", "format": {"fill": {"color": "#FFC7CE"}}, "stop_if_true": false}});
    edit(&facade, &mut lock(&west), "ann", &[json!({"op": "addSheet"})]).unwrap();
    edit(&facade, &mut lock(&west), "ann", &[rule]).unwrap();
    // Otto deletes Ann's rule, named by what his view shows of it
    catch_up(&facade, &mut lock(&east)).unwrap();
    let shown = lock(&east).rules_view(1)[0].clone();
    let expect = json!({"range": shown["range"], "rule": shown["rule"]});
    edit(
        &facade,
        &mut lock(&east),
        "otto",
        &[json!({"op": "deleteRule", "sheet": 1, "index": 0, "expect": expect})],
    )
    .unwrap();
    let undone = edit(&facade, &mut lock(&west), "ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!((undone.seq, undone.kind), (4, log::UNDO), "written as usual");
    assert_eq!(lock(&west).take_rules_passed(), 1);
    catch_up(&facade, &mut lock(&east)).unwrap();
    assert_eq!(
        lock(&east).take_rules_passed(),
        0,
        "another's undo is not told in Otto's room"
    );
    assert_eq!(lock(&east).seq, 4);
    // Ann's next undo takes back her sheet, in both rooms
    edit(&facade, &mut lock(&west), "ann", &[json!({"op": "undo"})]).unwrap();
    catch_up(&facade, &mut lock(&east)).unwrap();
    for room in [&west, &east] {
        assert_eq!(lock(room).model().workbook.worksheets.len(), 1);
    }
}

/// A room opened after a save numbers the rules of the saved version afresh, and follows another person's rule edit,
/// its undo and a redo from the log as the older rooms do, each by its own ids; a redo leaves out an add its undo
/// passed by, in every room.
#[test]
fn a_room_opened_after_a_save_follows_rule_edits_and_their_undo_by_its_own_ids() {
    let (_ctx, facade) = log_context();
    let rooms = Rooms::default();
    let west = scope_room(&rooms, "ann", "west", orders(&[10.0, 20.0, 30.0], "West"));
    let east = scope_room(&rooms, "otto", "east", orders(&[100.0, 200.0], "East"));
    let rule = |range: &str, fill: &str| {
        json!({"op": "addRule", "sheet": 1, "range": range, "rule": {"type": "CellIs",
        "operator": "GreaterThan", "formula": "1", "format": {"fill": {"color": fill}}, "stop_if_true": false}})
    };
    edit(
        &facade,
        &mut lock(&west),
        "ann",
        &[
            json!({"op": "addSheet"}),
            rule("A1:A3", "#FFC7CE"),
            rule("B1:B3", "#C8E6C9"),
        ],
    )
    .unwrap();
    save(&facade, &mut lock(&west), "ann", |_, version| {
        Ok(Some(version.unwrap_or(1) + 1))
    })
    .unwrap();
    let saved_seq = lock(&west).seq;
    let stored = lock(&west).stored_form().unwrap();
    let north = rooms
        .open(Opening {
            user_id: "nina".to_string(),
            shared: Some((
                crate::room::document_room_id("doc-1", Some("sales"), "north"),
                saved_seq,
            )),
            profile_id: Some("sales".to_string()),
            definition_id: Some("doc-1".to_string()),
            version: Some(2),
            title: "Orders".to_string(),
            data_sheets: vec![DataSheet {
                name: "Orders".to_string(),
                spec: json!({"name": "Orders", "source": {"type": "entity", "entityName": "test.Order"}}),
            }],
            charts: Vec::new(),
            base: fill::open_stored(&stored).unwrap(),
            rows: vec![orders(&[1.0], "North")],
        })
        .unwrap();
    // Otto changes the second rule and takes it back; Ann adds a rule, Otto changes it, Ann's undo passes it by and her
    // redo does not add it again
    catch_up(&facade, &mut lock(&east)).unwrap();
    let shown = |room: &SharedRoom, at: usize| {
        let views = lock(room).rules_view(1);
        let rule = views.iter().find(|rule| rule["index"] == at).unwrap().clone();
        json!({"range": rule["range"], "rule": rule["rule"]})
    };
    let change = |expect: Json, fill: &str| {
        json!({"op": "changeRule", "sheet": 1, "index": 1, "expect": expect, "range": "B1:B3",
        "rule": {"type": "CellIs", "operator": "GreaterThan", "formula": "1", "format": {"fill": {"color": fill}}, "stop_if_true": false}})
    };
    let blue = change(shown(&east, 1), "#BBDEFB");
    edit(&facade, &mut lock(&east), "otto", &[blue]).unwrap();
    edit(&facade, &mut lock(&east), "otto", &[json!({"op": "undo"})]).unwrap();
    edit(&facade, &mut lock(&west), "ann", &[rule("C1:C3", "#FFE0B2")]).unwrap();
    catch_up(&facade, &mut lock(&east)).unwrap();
    let mut third = change(shown(&east, 2), "#E1BEE7");
    third["index"] = json!(2);
    third["range"] = json!("C1:C3");
    edit(&facade, &mut lock(&east), "otto", &[third]).unwrap();
    edit(&facade, &mut lock(&west), "ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(lock(&west).take_rules_passed(), 1);
    edit(&facade, &mut lock(&west), "ann", &[json!({"op": "redo"})]).unwrap();
    for room in [&east, &north] {
        catch_up(&facade, &mut lock(room)).unwrap();
    }
    let rules = |room: &SharedRoom| {
        let mut rules = lock(room).rules_view(1);
        rules.reverse();
        rules
            .iter()
            .map(|rule| {
                format!(
                    "{} {}",
                    rule["range"].as_str().unwrap(),
                    rule["rule"]["format"]["fill"]["color"].as_str().unwrap()
                )
            })
            .collect::<Vec<_>>()
    };
    let expected = ["A1:A3 #FFC7CE", "B1:B3 #C8E6C9", "C1:C3 #E1BEE7"];
    for room in [&west, &east, &north] {
        assert_eq!(rules(room), expected);
    }
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

/// SEC-TLS-113: a view's place is four numbers, whatever else the client sent as its selection.
#[test]
fn a_place_is_four_numbers() {
    let place = super::place_of(&serde_json::json!({"row": 2, "column": 3, "width": 4, "height": 5,
        "label": "<img src=x>", "extra": [1, 2, 3]}));
    assert_eq!(
        place,
        serde_json::json!({"row": 2, "column": 3, "width": 4, "height": 5})
    );
}
