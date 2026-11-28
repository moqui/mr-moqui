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

use serde_json::{json, Value as Json};

use super::*;
use crate::fill::{self, CellData, SheetRows};
use crate::log::{self, LogRow};
use crate::room::{range_cells, DataSheet, Opening, Rooms, SharedRoom};
use moqui_lib::sync::lock;

fn orders(count: usize) -> SheetRows {
    SheetRows {
        name: "Orders".to_string(),
        columns: vec!["region".to_string(), "amount".to_string()],
        rows: (1..=count)
            .map(|row| {
                #[allow(clippy::cast_precision_loss)]
                let amount = row as f64;
                vec![CellData::Text(format!("Region {row}")), CellData::Number(amount)]
            })
            .collect(),
        truncated: false,
    }
}

fn room(rooms: &Rooms, user: &str, count: usize) -> SharedRoom {
    rooms
        .open(Opening {
            user_id: user.to_string(),
            shared: None,
            profile_id: None,
            definition_id: None,
            version: None,
            title: "Orders".to_string(),
            data_sheets: vec![DataSheet {
                name: "Orders".to_string(),
                spec: json!({"name": "Orders", "source": {"type": "entity", "entityName": "test.Order"}}),
            }],
            charts: Vec::new(),
            base: fill::new_model("Orders").unwrap(),
            rows: vec![orders(count)],
        })
        .unwrap()
}

/// Two rooms of one document as two scopes hold it (their data of other row counts), following one log: an edit is
/// written by the first as its own ([`Room::apply_ops`]) and followed by the second as another's
/// ([`Room::apply_row`]), and an undo is taken back by each with what it took.
struct Pair {
    writer: SharedRoom,
    follower: SharedRoom,
    seq: i64,
}

impl Pair {
    fn new(rooms: &Rooms) -> Self {
        let mut pair = Self {
            writer: room(rooms, "ann", 3),
            follower: room(rooms, "otto", 5),
            seq: 0,
        };
        pair.write(&[
            json!({"op": "addSheet"}),
            json!({"op": "renameSheet", "sheet": 1, "name": "Summary"}),
        ]);
        pair
    }

    fn write(&mut self, ops: &[Json]) -> i64 {
        self.seq += 1;
        let seq = self.seq;
        {
            let mut writer = lock(&self.writer);
            writer.pause();
            let (applied, inverse, refused) = writer.apply_ops(ops);
            assert!(
                refused.is_none(),
                "{ops:?}: {:?}",
                refused.map(|err| err.message().to_string())
            );
            assert_eq!(applied.len(), ops.len());
            writer.record(seq, inverse);
            writer.seq = seq;
            writer.evaluate();
        }
        self.follow(log::EDIT, ops.to_vec(), None);
        seq
    }

    fn undo(&mut self, target: i64) {
        self.seq += 1;
        {
            let mut writer = lock(&self.writer);
            writer.pause();
            writer.undo_row(target).unwrap();
            writer.seq = self.seq;
            writer.evaluate();
        }
        self.follow(log::UNDO, Vec::new(), Some(target));
    }

    fn follow(&self, kind: &str, ops: Vec<Json>, target: Option<i64>) {
        let mut follower = lock(&self.follower);
        follower.pause();
        follower
            .apply_row(&LogRow {
                seq: self.seq,
                user_id: "ann".to_string(),
                kind: kind.to_string(),
                ops,
                target,
                undone: None,
            })
            .unwrap();
        follower.evaluate();
    }

    /// Both rooms' sheet 1, as [`state`] reads it.
    fn states(&self) -> (Json, Json) {
        (state(&lock(&self.writer)), state(&lock(&self.follower)))
    }
}

/// Sheet 1 as typed and styled: each cell's content and own style over A1:F10, its merges and its rules.
fn state(room: &Room) -> Json {
    let mut cells = Vec::new();
    for row in 1..=10 {
        for column in 1..=6 {
            let style = room.model().get_style_for_cell(1, row, column).unwrap();
            cells.push(json!([
                row,
                column,
                room.cell_content(1, row, column).unwrap(),
                serde_json::to_value(style).unwrap()
            ]));
        }
    }
    // the merges as a set: one taken back is put back at the end of the sheet's list
    let mut merges = room.merges_of(1, 1, 1, 1_048_576, 16_384);
    merges.sort_by_key(Json::to_string);
    // the rules in the order they apply, each its range and rule: an undo puts a rule back in that order, not at its
    // place in the sheet's list
    let mut rules = room.rules_of(1).unwrap();
    rules.sort_by_key(|rule| rule["priority"].as_u64());
    let rules: Vec<Json> = rules.iter().map(|rule| json!([rule["range"], rule["rule"]])).collect();
    json!({"cells": cells, "merges": merges, "rules": rules})
}

/// The style a view draws a cell in: its extended style from the room's block.
fn drawn(room: &Room, row: i32, column: i32) -> Json {
    let block: Json = serde_json::from_str(&room.block(1, row, column, 1, 1).unwrap()).unwrap();
    let index = block["style_indices"][0].as_u64().unwrap();
    block["styles"][usize::try_from(index).unwrap()].clone()
}

fn refusal(room: &SharedRoom, op: Json) -> String {
    lock(room)
        .edit("ann", std::slice::from_ref(&op))
        .expect_err("refused")
        .message()
        .to_string()
}

#[test]
fn a_border_changes_its_ring_and_its_undo_restores_both_in_every_room() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    pair.write(&[
        json!({"op": "paste", "sheet": 1, "row": 1, "column": 1, "rows": [["a", "1"], ["b", "2"], ["c", "3"]]}),
        json!({"op": "border", "sheet": 1, "row": 2, "column": 1, "width": 1, "height": 1, "kind": "right", "style": "thick"}),
    ]);
    let before = pair.states();
    let thin = pair.write(&[
        json!({"op": "border", "sheet": 1, "row": 2, "column": 2, "width": 2, "height": 2,
        "kind": "all", "style": "thin", "color": "#C62828"}),
    ]);
    for room in [&pair.writer, &pair.follower] {
        let room = lock(room);
        let style = room.model().get_style_for_cell(1, 2, 2).unwrap();
        assert_eq!(serde_json::to_value(&style.border.top).unwrap()["style"], "thin");
        assert_eq!(serde_json::to_value(&style.border.left).unwrap()["color"], "#C62828");
        // the neighbour's facing side, which was heavier and would be drawn over the new one, is the new one
        let left = room.model().get_style_for_cell(1, 2, 1).unwrap();
        assert_eq!(serde_json::to_value(&left.border.right).unwrap()["style"], "thin");
        assert_eq!(drawn(&room, 3, 3)["style"]["border"]["bottom"]["style"], "thin");
    }
    pair.undo(thin);
    assert_eq!(
        pair.states(),
        before,
        "the area and its ring as they were, in both rooms"
    );
    let room = lock(&pair.writer);
    let left = room.model().get_style_for_cell(1, 2, 1).unwrap();
    assert_eq!(serde_json::to_value(&left.border.right).unwrap()["style"], "thick");
    drop(room);

    // refused: on a data sheet, over whole rows, a border that is not one, an area past the cell bound
    let area = |sheet: u32, width: i64, height: i64, kind: &str| json!({"op": "border", "sheet": sheet, "row": 1, "column": 1, "width": width, "height": height, "kind": kind});
    assert!(refusal(&pair.writer, area(0, 1, 1, "all")).contains("is a data sheet"));
    assert!(refusal(&pair.writer, area(1, 16_384, 1, "all")).contains("not whole rows or columns"));
    assert!(refusal(&pair.writer, area(1, 1, 1, "diagonal")).contains("is not a border"));
    assert!(refusal(&pair.writer, area(1, 400, 400, "all")).contains("at most"));
    // a colour goes into the styles, and an export's XML, as it is: only #RRGGBB
    let mut injected = area(1, 1, 1, "all");
    injected["color"] = json!("000000\"/><x a=\"");
    assert!(refusal(&pair.writer, injected).contains("is not a colour"));
}

#[test]
fn a_merge_and_an_unmerge_are_taken_back_and_followed_by_another_room() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    pair.write(&[
        json!({"op": "paste", "sheet": 1, "row": 1, "column": 1, "rows": [["Title", "Sub"], [""], ["", "", "x"]]}),
        json!({"op": "style", "sheet": 1, "row": 3, "column": 3, "width": 1, "height": 1, "path": "font.b", "value": "true"}),
    ]);
    let before = pair.states();
    let merged = pair.write(&[json!({"op": "merge", "sheet": 1, "row": 2, "column": 2, "width": 2, "height": 2})]);
    for room in [&pair.writer, &pair.follower] {
        let room = lock(room);
        // the one cell with content moves to the anchor, with its style over the box
        assert_eq!(room.cell_content(1, 2, 2).unwrap(), "x");
        assert_eq!(room.cell_content(1, 3, 3).unwrap(), "");
        assert!(room.model().get_style_for_cell(1, 2, 3).unwrap().font.b);
        let box_ = json!([{"row": 2, "column": 2, "width": 2, "height": 2}]);
        assert_eq!(room.layout()[1]["merges"], box_);
        let block: Json = serde_json::from_str(&room.block(1, 3, 1, 4, 3).unwrap()).unwrap();
        assert_eq!(block["merges"], box_, "a block has the merges that meet it");
        let away: Json = serde_json::from_str(&room.block(1, 6, 1, 4, 3).unwrap()).unwrap();
        assert_eq!(away["merges"], json!([]));
    }
    // refused: over a merge, over two cells with content, on a data sheet
    let merge = |sheet: u32, row: i64, column: i64, width: i64, height: i64| json!({"op": "merge", "sheet": sheet, "row": row, "column": column, "width": width, "height": height});
    assert!(refusal(&pair.writer, merge(1, 1, 1, 2, 2)).contains("merged cell"));
    assert!(refusal(&pair.writer, merge(1, 1, 1, 2, 1)).contains("more than one cell has content"));
    assert!(refusal(&pair.writer, merge(1, 5, 5, 1, 1)).contains("single cell"));
    assert!(refusal(&pair.writer, merge(0, 2, 1, 2, 1)).contains("is a data sheet"));

    pair.undo(merged);
    assert_eq!(
        pair.states(),
        before,
        "unmerged, the content and styles back where they were"
    );

    // an unmerge is taken back by the merge put back, in both rooms
    let merged = pair.write(&[merge(1, 2, 2, 2, 2)]);
    let merged_state = pair.states();
    let unmerged = pair.write(&[json!({"op": "unmerge", "sheet": 1, "row": 3, "column": 3, "width": 1, "height": 1})]);
    assert_eq!(lock(&pair.writer).merges_of(1, 1, 1, 100, 100), Vec::<Json>::new());
    assert_eq!(lock(&pair.follower).merges_of(1, 1, 1, 100, 100), Vec::<Json>::new());
    pair.undo(unmerged);
    assert_eq!(pair.states(), merged_state);
    pair.undo(merged);
    assert_eq!(pair.states(), before);
}

/// A deletion of rows or columns cuts or drops the merges it meets, and a fill tiles the source's merges into its
/// target: each is taken back with the merges as they were.
#[test]
fn a_deletion_and_a_fill_are_taken_back_with_their_merges() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    pair.write(&[
        json!({"op": "paste", "sheet": 1, "row": 3, "column": 2, "rows": [["kept"]]}),
        json!({"op": "paste", "sheet": 1, "row": 4, "column": 5, "rows": [["gone"]]}),
        json!({"op": "merge", "sheet": 1, "row": 3, "column": 2, "width": 2, "height": 4}),
        json!({"op": "merge", "sheet": 1, "row": 4, "column": 5, "width": 2, "height": 1}),
        json!({"op": "paste", "sheet": 1, "row": 8, "column": 1, "rows": [["m"]]}),
        json!({"op": "merge", "sheet": 1, "row": 8, "column": 1, "width": 2, "height": 1}),
    ]);
    let before = pair.states();
    let deleted = pair.write(&[json!({"op": "deleteRows", "sheet": 1, "row": 2, "count": 4})]);
    assert_eq!(
        lock(&pair.writer).merges_of(1, 1, 1, 100, 100),
        [
            json!({"row": 2, "column": 2, "width": 2, "height": 1}),
            json!({"row": 4, "column": 1, "width": 2, "height": 1})
        ],
        "the first cut, the second gone, the third moved up"
    );
    pair.undo(deleted);
    assert_eq!(pair.states(), before);

    let columns = pair.write(&[json!({"op": "deleteColumns", "sheet": 1, "column": 3, "count": 3})]);
    pair.undo(columns);
    assert_eq!(pair.states(), before);

    let filled =
        pair.write(&[json!({"op": "fill", "sheet": 1, "row": 8, "column": 1, "width": 2, "height": 1, "toRow": 10})]);
    assert_eq!(
        lock(&pair.follower).merges_of(1, 9, 1, 10, 2).len(),
        2,
        "the source's merge tiled down"
    );
    pair.undo(filled);
    assert_eq!(pair.states(), before);

    let deleted = pair.write(&[json!({"op": "deleteSheet", "sheet": 1})]);
    pair.undo(deleted);
    assert_eq!(pair.states(), before, "a sheet deleted comes back with its merges");
}

fn cell_is(operator: &str, value: &str, fill: &str) -> Json {
    json!({"type": "CellIs", "operator": operator, "formula": value, "format": {"fill": {"color": fill}}, "stop_if_true": false})
}

#[test]
fn conditional_formats_are_added_changed_moved_and_deleted_each_taken_back_in_every_room() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    pair.write(&[json!({"op": "paste", "sheet": 1, "row": 1, "column": 1,
        "rows": [["West", "1"], ["East", "5"], ["West", "10"], ["North", "20"], ["West", "3"]]})]);
    let empty = pair.states();
    let rule_op = |kind: &str, range: &str, rule: Json| json!({"op": kind, "sheet": 1, "range": range, "rule": rule});
    let added = pair.write(&[rule_op("addRule", "B1:B5", cell_is("GreaterThan", "5", "#FFC7CE"))]);
    for room in [&pair.writer, &pair.follower] {
        let room = lock(room);
        assert_eq!(drawn(&room, 3, 2)["style"]["fill"]["color"], "#FFC7CE", "10 > 5");
        assert!(drawn(&room, 2, 2)["style"]["fill"]["color"].is_null(), "5 is not");
        let rules = &room.layout()[1]["rules"];
        assert_eq!(
            (rules[0]["index"].as_u64(), rules[0]["range"].as_str()),
            (Some(0), Some("B1:B5"))
        );
    }
    // a value typed afterwards is formatted by the rule
    pair.write(&[json!({"op": "input", "sheet": 1, "row": 2, "column": 2, "value": "50"})]);
    assert_eq!(drawn(&lock(&pair.follower), 2, 2)["style"]["fill"]["color"], "#FFC7CE");
    pair.write(&[json!({"op": "input", "sheet": 1, "row": 2, "column": 2, "value": "5"})]);
    pair.write(&[
        rule_op(
            "addRule",
            "A1:A5",
            json!({"type": "Text", "operator": "Contains", "value": "West",
            "format": {"font": {"b": true, "color": "#1565C0"}}, "stop_if_true": false}),
        ),
        rule_op(
            "addRule",
            "B1:B5",
            json!({"type": "DataBar", "min": null, "max": null, "positive_color": "#638EC6",
            "negative_color": "#FF0000", "is_gradient": false, "show_value": true}),
        ),
        rule_op(
            "addRule",
            "B1:B5",
            json!({"type": "Top10", "rank": 1, "percent": false,
            "format": {"fill": {"color": "#C8E6C9"}}, "stop_if_true": false}),
        ),
    ]);
    {
        let room = lock(&pair.follower);
        assert_eq!(drawn(&room, 1, 1)["style"]["font"]["color"], "#1565C0");
        assert!(drawn(&room, 4, 2)["data_bar"]["value"].as_f64().unwrap() > 0.9);
        // the top one is formatted by the later rule, which wins over the first
        assert_eq!(drawn(&room, 4, 2)["style"]["fill"]["color"], "#C8E6C9");
    }
    let four = pair.states();
    // moved: the first rule applied last, so it wins on 20
    let moved = pair.write(&[json!({"op": "moveRule", "sheet": 1, "index": 0, "up": true})]);
    let moved_up = pair.write(&[json!({"op": "moveRule", "sheet": 1, "index": 0, "up": true})]);
    let moved_up_again = pair.write(&[json!({"op": "moveRule", "sheet": 1, "index": 0, "up": true})]);
    assert_eq!(drawn(&lock(&pair.writer), 4, 2)["style"]["fill"]["color"], "#FFC7CE");
    // past the last place a move changes nothing, and its undo nothing
    let past = pair.write(&[json!({"op": "moveRule", "sheet": 1, "index": 0, "up": true})]);
    pair.undo(past);
    for seq in [moved_up_again, moved_up, moved] {
        pair.undo(seq);
    }
    assert_eq!(pair.states(), four);
    // changed, and deleted
    let changed = pair.write(&[json!({"op": "changeRule", "sheet": 1, "index": 0, "range": "B1:B3",
        "rule": cell_is("LessThan", "3", "#FFE0B2")})]);
    assert_eq!(drawn(&lock(&pair.follower), 1, 2)["style"]["fill"]["color"], "#FFE0B2");
    pair.undo(changed);
    assert_eq!(pair.states(), four);
    let deleted = pair.write(&[json!({"op": "deleteRule", "sheet": 1, "index": 1})]);
    assert_eq!(lock(&pair.writer).rules_of(1).unwrap().len(), 3);
    pair.undo(deleted);
    assert_eq!(pair.states(), four, "the rules back in their places and order");
    // a deletion of rows moves the ranges; its undo puts them back
    let rows = pair.write(&[json!({"op": "deleteRows", "sheet": 1, "row": 1, "count": 2})]);
    pair.undo(rows);
    assert_eq!(pair.states(), four);
    pair.undo(added);
    let rules = lock(&pair.writer).rules_of(1).unwrap();
    assert_eq!(rules.len(), 3, "the first rule's add taken back alone");
    assert_ne!(pair.states(), empty);

    // refused: on a data sheet, over too many cells, a range or a rule that is not one, a rule not there
    let data = json!({"op": "addRule", "sheet": 0, "range": "B2:B4", "rule": cell_is("GreaterThan", "1", "#FFC7CE")});
    assert!(refusal(&pair.writer, data).contains("conditional formats go on a sheet beside it"));
    assert!(refusal(
        &pair.writer,
        rule_op("addRule", "A1:Z10000", cell_is("GreaterThan", "1", "#FFC7CE"))
    )
    .contains("at most"));
    assert!(refusal(
        &pair.writer,
        rule_op("addRule", "B1:", cell_is("GreaterThan", "1", "#FFC7CE"))
    )
    .contains("not a range"));
    assert!(
        refusal(&pair.writer, rule_op("addRule", "B1:B2", json!({"type": "Nope"})))
            .contains("Not a conditional format")
    );
    assert!(refusal(&pair.writer, json!({"op": "deleteRule", "sheet": 1, "index": 9})).contains("out of bounds"));
    // an undo's edits are refused from a call
    for op in [
        json!({"op": "removeRule", "sheet": 1, "index": 0, "expect": {"range": "B1:B5"}}),
        json!({"op": "putRule", "sheet": 1, "range": "A1", "rule": cell_is("GreaterThan", "1", "#FFC7CE"), "higher": 0}),
        json!({"op": "ruleRanges", "sheet": 1, "rules": []}),
        json!({"op": "restore", "sheet": 1, "row": 1, "column": 1, "cells": [["x"]], "styles": [[{"fill": {"color": "x\"/>"}}]]}),
    ] {
        assert!(
            refusal(&pair.writer, op.clone()).contains("an undo's edit alone"),
            "{op}"
        );
    }
    // a rule past its bounds: thresholds, size, a colour that is not one (it goes into an export's XML as it is)
    let icons: Vec<Json> = (0..6)
        .map(|at| json!({"icon": "Circle", "cfvo": {"Number": at}, "color": "#63BE7B", "is_strict": false}))
        .collect();
    let many = json!({"type": "IconSet", "thresholds": icons, "show_value": true});
    assert!(refusal(&pair.writer, rule_op("addRule", "B1:B5", many)).contains("at most 5 thresholds"));
    let rating = json!({"type": "IconRating", "icon": "Star", "color": "#FFC000",
        "thresholds": vec![json!([{"Number": 1.0}, false]); 10], "show_value": true});
    assert!(refusal(&pair.writer, rule_op("addRule", "B1:B5", rating)).contains("at most 9 thresholds"));
    let long = json!({"type": "Text", "operator": "Contains", "value": "x".repeat(5_000), "format": {}, "stop_if_true": false});
    assert!(refusal(&pair.writer, rule_op("addRule", "B1:B5", long)).contains("bytes"));
    let injected = cell_is("GreaterThan", "1", "FF0000\"/><x a=\"");
    assert!(refusal(&pair.writer, rule_op("addRule", "B1:B5", injected)).contains("is not a colour"));
    let bar = json!({"type": "DataBar", "min": null, "max": null, "positive_color": "red", "negative_color": "#FF0000",
        "is_gradient": false, "show_value": true});
    assert!(refusal(&pair.writer, rule_op("addRule", "B1:B5", bar)).contains("is not a colour"));
    // the sheet's rules in all: ten of 100,000 cells, then no more
    let wide = |rows: i64| {
        rule_op(
            "addRule",
            &format!("A1:J{rows}"),
            cell_is("GreaterThan", "1", "#FFC7CE"),
        )
    };
    for _ in 0..9 {
        lock(&pair.writer)
            .edit("ann", std::slice::from_ref(&wide(10_000)))
            .unwrap();
    }
    assert!(refusal(&pair.writer, wide(10_000)).contains("in all"));
    lock(&pair.writer)
        .edit("ann", std::slice::from_ref(&wide(9_000)))
        .unwrap();
    // an unmerge outside the sheet
    let outside = json!({"op": "unmerge", "sheet": 1, "row": 1_048_576, "column": 1, "width": 1, "height": 2});
    assert!(refusal(&pair.writer, outside).contains("outside the sheet"));
}

#[test]
fn a_range_of_cells_is_counted_as_the_engine_reads_it() {
    assert_eq!(range_cells("B2:B20"), Some(19));
    assert_eq!(range_cells("A1:B2 D4"), Some(5));
    assert_eq!(range_cells("$C$3:A1"), Some(9));
    assert_eq!(range_cells(""), None);
    assert_eq!(range_cells("Orders[amount]"), None);
}

/// The rule a view names: its range and rule as the layout gives them.
fn view_of(room: &SharedRoom, index: usize) -> Json {
    let rules = lock(room).rules_of(1).unwrap();
    json!({"range": rules[index]["range"], "rule": rules[index]["rule"]})
}

fn kinds(room: &SharedRoom) -> Vec<String> {
    let mut rules = lock(room).rules_of(1).unwrap();
    rules.sort_by_key(|rule| rule["priority"].as_u64());
    rules
        .iter()
        .map(|rule| {
            format!(
                "{} {}",
                rule["rule"]["type"].as_str().unwrap(),
                rule["range"].as_str().unwrap()
            )
        })
        .collect()
}

/// Two people edit one sheet's rules: an undo takes back only its own edit's rule, found by what that edit left; one
/// changed or deleted since is refused with nothing applied; and the undo of a deletion of rows sets back only the
/// ranges it moved, keeping another's later change of a rule.
#[test]
fn a_rules_undo_takes_back_only_its_own_rule_and_refuses_one_changed_since() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    let rule_op = |kind: &str, range: &str, rule: Json| json!({"op": kind, "sheet": 1, "range": range, "rule": rule});
    pair.write(&[json!({"op": "paste", "sheet": 1, "row": 1, "column": 1,
        "rows": [["West", "1"], ["East", "5"], ["West", "10"], ["North", "20"], ["West", "3"]]})]);
    // Bob's rule, then Ann's, then Bob's; Bob deletes his first
    pair.write(&[rule_op("addRule", "B1:B5", cell_is("GreaterThan", "5", "#FFC7CE"))]);
    let ann = pair.write(&[rule_op(
        "addRule",
        "A1:A5",
        json!({"type": "Text", "operator": "Contains", "value": "West",
        "format": {"font": {"b": true}}, "stop_if_true": false}),
    )]);
    pair.write(&[rule_op(
        "addRule",
        "B1:B5",
        json!({"type": "Top10", "rank": 1, "percent": false,
        "format": {"fill": {"color": "#C8E6C9"}}, "stop_if_true": false}),
    )]);
    let first = view_of(&pair.writer, 0);
    pair.write(&[json!({"op": "deleteRule", "sheet": 1, "index": 0, "expect": first})]);
    assert_eq!(kinds(&pair.writer), ["Text A1:A5", "Top10 B1:B5"]);
    // Ann's undo takes her rule back, not Bob's that is now at its place
    pair.undo(ann);
    assert_eq!(kinds(&pair.writer), ["Top10 B1:B5"]);
    assert_eq!(kinds(&pair.follower), ["Top10 B1:B5"]);

    // Ann changes Bob's rule, Bob deletes it: Ann's undo is refused before anything is applied
    let top = view_of(&pair.writer, 0);
    let changed = pair.write(&[
        json!({"op": "changeRule", "sheet": 1, "index": 0, "expect": top, "range": "B1:B4",
        "rule": {"type": "Bottom10", "rank": 1, "percent": false, "format": {}, "stop_if_true": false}}),
    ]);
    let bottom = view_of(&pair.writer, 0);
    pair.write(&[json!({"op": "deleteRule", "sheet": 1, "index": 0, "expect": bottom})]);
    let before = pair.states();
    let refused = lock(&pair.writer).check_undo(changed).expect_err("changed since");
    assert!(
        refused.message().contains("changed or deleted since"),
        "{}",
        refused.message()
    );
    assert_eq!(pair.states(), before);
    // an edit naming a rule that is gone is refused too
    let gone = json!({"op": "deleteRule", "sheet": 1, "index": 0, "expect": bottom});
    assert!(refusal(&pair.writer, gone).contains("changed or deleted since"));

    // Ann deletes rows 2 and 3, Bob then changes the rule they moved; Ann's undo sets its range back and keeps Bob's
    pair.write(&[rule_op("addRule", "A1:A5", cell_is("GreaterThan", "1", "#FFC7CE"))]);
    let rows = pair.write(&[json!({"op": "deleteRows", "sheet": 1, "row": 2, "count": 2})]);
    assert_eq!(kinds(&pair.writer), ["CellIs A1:A3"]);
    let moved = view_of(&pair.writer, 0);
    pair.write(&[
        json!({"op": "changeRule", "sheet": 1, "index": 0, "expect": moved, "range": "A1:A3",
        "rule": cell_is("GreaterThan", "1", "#BBDEFB")}),
    ]);
    pair.undo(rows);
    for room in [&pair.writer, &pair.follower] {
        assert_eq!(kinds(room), ["CellIs A1:A5"]);
        assert_eq!(
            view_of(room, 0)["rule"]["format"]["fill"]["color"],
            "#BBDEFB",
            "Bob's change stays"
        );
    }
}

/// A room over `base`, a workbook as a file gives one, beside the data sheet.
fn opened(rooms: &Rooms, base: moqui_core::workbook::ironcalc::base::Model<'static>) -> SharedRoom {
    rooms
        .open(Opening {
            user_id: "ann".to_string(),
            shared: None,
            profile_id: None,
            definition_id: None,
            version: None,
            title: "File".to_string(),
            data_sheets: Vec::new(),
            charts: Vec::new(),
            base,
            rows: Vec::new(),
        })
        .unwrap()
}

/// A workbook opened with more merged cells and rules than the edits are held to keeps what fits, and the bounds of
/// a sheet and of the workbook hold for the edits after.
#[test]
fn a_workbook_opened_past_the_bounds_keeps_what_fits() {
    use moqui_core::workbook::ironcalc::base::cf_types::CfRuleInput;
    use moqui_core::workbook::ironcalc::base::expressions::types::Area;
    let mut base = fill::new_model("Notes").unwrap();
    for name in ["Two", "Three", "Four", "Five"] {
        base.add_sheet(name).unwrap();
    }
    for row in 1..=2_001 {
        base.merge_cells(&Area {
            sheet: 0,
            row,
            column: 1,
            width: 2,
            height: 1,
        })
        .unwrap();
    }
    let rule = |fill: &str| -> CfRuleInput { serde_json::from_value(cell_is("GreaterThan", "1", fill)).unwrap() };
    // a rule whose colour would go into an export's XML as it is, and one with more thresholds than a set has icons
    base.add_conditional_formatting(0, "D1", rule("FF0000\"/><x a=\""))
        .unwrap();
    let icons: Vec<Json> = (0..7)
        .map(|at| json!({"icon": "Circle", "cfvo": {"Number": at}, "color": "#63BE7B", "is_strict": false}))
        .collect();
    let many: CfRuleInput =
        serde_json::from_value(json!({"type": "IconSet", "thresholds": icons, "show_value": true})).unwrap();
    base.add_conditional_formatting(0, "D2", many).unwrap();
    // 2,001 rules of one cell over five sheets, 401 a sheet
    for at in 0..2_001_u32 {
        let row = i32::try_from(at / 5 + 1).unwrap();
        base.add_conditional_formatting(at % 5, &format!("C{row}"), rule("#FFC7CE"))
            .unwrap();
    }
    let rooms = Rooms::default();
    let room = opened(&rooms, base);
    let mut room = lock(&room);
    assert_eq!(room.merges_of(0, 1, 1, 3_000, 10).len(), 2_000);
    let all: usize = (0..5).map(|sheet| room.rules_of(sheet).unwrap().len()).sum();
    assert_eq!(all, 2_000, "the bad two left out, and the workbook's last");
    assert!(room
        .rules_of(0)
        .unwrap()
        .iter()
        .all(|rule| rule["range"].as_str().unwrap().starts_with('C')));

    let err = room
        .edit(
            "ann",
            &[json!({"op": "merge", "sheet": 0, "row": 3_000, "column": 1, "width": 2, "height": 1})],
        )
        .expect_err("past the merges of a sheet");
    assert!(err.message().contains("at most 2000 merged cells"), "{}", err.message());
    let err = room
        .edit(
            "ann",
            &[json!({"op": "addRule", "sheet": 1, "range": "E1", "rule": cell_is("GreaterThan", "1", "#FFC7CE")})],
        )
        .expect_err("past the rules of a workbook");
    assert!(
        err.message().contains("A workbook has at most 2000"),
        "{}",
        err.message()
    );
    // an unmerge of a whole column counts the merges it takes off, not the column's cells; its undo sets them back
    room.edit(
        "ann",
        &[json!({"op": "unmerge", "sheet": 0, "row": 1, "column": 1, "width": 1, "height": 1_048_576})],
    )
    .unwrap();
    assert!(room.merges_of(0, 1, 1, 3_000, 10).is_empty());
    room.edit("ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(room.merges_of(0, 1, 1, 3_000, 10).len(), 2_000);
}

/// A data sheet's column named with the characters a structured reference escapes is read by a formula that escapes
/// them, as the sheet's pointing writes it (WebrootCalc.js calc.columnReference).
#[test]
fn a_column_named_with_brackets_is_read_by_its_escaped_reference() {
    let rooms = Rooms::default();
    let mut sheet = orders(3);
    sheet.columns = vec!["region[x]".to_string(), "amount#1".to_string()];
    let room = rooms
        .open(Opening {
            user_id: "ann".to_string(),
            shared: None,
            profile_id: None,
            definition_id: None,
            version: None,
            title: "Orders".to_string(),
            data_sheets: Vec::new(),
            charts: Vec::new(),
            base: fill::new_model("Orders").unwrap(),
            rows: vec![sheet],
        })
        .unwrap();
    let mut room = lock(&room);
    room.edit("ann", &[json!({"op": "addSheet"}),
        json!({"op": "input", "sheet": 1, "row": 1, "column": 1, "value": "=SUM(Orders[amount'#1])"}),
        json!({"op": "input", "sheet": 1, "row": 2, "column": 1, "value": "=COUNTA(Orders[[region'[x']]:[amount'#1]])"})])
        .unwrap();
    assert_eq!(room.model().get_formatted_cell_value(1, 1, 1).unwrap(), "6");
    assert_eq!(room.model().get_formatted_cell_value(1, 2, 1).unwrap(), "6");
}
