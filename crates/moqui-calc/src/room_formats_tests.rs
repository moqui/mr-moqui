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
use crate::room::{range_cells, DataSheet, Opening, Rooms, SharedRoom, MAX_SHEET_RULES};
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
    let up =
        |pair: &Pair| json!({"op": "moveRule", "sheet": 1, "index": 0, "up": true, "expect": view_of(&pair.writer, 0)});
    let moved = pair.write(&[up(&pair)]);
    let moved_up = pair.write(&[up(&pair)]);
    let moved_up_again = pair.write(&[up(&pair)]);
    assert_eq!(drawn(&lock(&pair.writer), 4, 2)["style"]["fill"]["color"], "#FFC7CE");
    // past the last place a move changes nothing, and its undo nothing
    let past = pair.write(&[up(&pair)]);
    pair.undo(past);
    for seq in [moved_up_again, moved_up, moved] {
        pair.undo(seq);
    }
    assert_eq!(pair.states(), four);
    // changed, and deleted
    let changed = pair.write(&[
        json!({"op": "changeRule", "sheet": 1, "index": 0, "expect": view_of(&pair.writer, 0),
        "range": "B1:B3", "rule": cell_is("LessThan", "3", "#FFE0B2")}),
    ]);
    assert_eq!(drawn(&lock(&pair.follower), 1, 2)["style"]["fill"]["color"], "#FFE0B2");
    pair.undo(changed);
    assert_eq!(pair.states(), four);
    let deleted =
        pair.write(&[json!({"op": "deleteRule", "sheet": 1, "index": 1, "expect": view_of(&pair.writer, 1)})]);
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
    let nowhere = json!({"op": "deleteRule", "sheet": 1, "index": 9, "expect": {"range": "Z1", "rule": {}}});
    assert!(refusal(&pair.writer, nowhere).contains("changed or deleted since"));
    // an undo's edits are refused from a call
    for op in [
        json!({"op": "removeRule", "sheet": 1, "index": 0, "expect": {"range": "B1:B5"}}),
        json!({"op": "putRule", "sheet": 1, "range": "A1", "rule": cell_is("GreaterThan", "1", "#FFC7CE"), "higher": 0}),
        json!({"op": "ruleRanges", "sheet": 1}),
        json!({"op": "ruleCheck", "sheet": 1, "rules": []}),
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

/// The place in sheet 1's list of the rule over `range`.
fn at(room: &SharedRoom, range: &str) -> usize {
    lock(room)
        .rules_of(1)
        .unwrap()
        .iter()
        .position(|rule| rule["range"] == range)
        .unwrap()
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

/// Two people edit one sheet's rules: an undo takes back its own rule, found by its id, whatever was added, deleted or
/// moved around it; one changed or deleted by another since is passed by, counted, and the rest of the undo applied.
#[test]
fn a_rules_undo_finds_its_rule_by_id_and_passes_by_one_changed_since() {
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
    pair.write(&[json!({"op": "deleteRule", "sheet": 1, "index": 0, "expect": view_of(&pair.writer, 0)})]);
    assert_eq!(kinds(&pair.writer), ["Text A1:A5", "Top10 B1:B5"]);
    // Ann's undo takes her rule back, not Bob's that is now at its place
    pair.undo(ann);
    assert_eq!(kinds(&pair.writer), ["Top10 B1:B5"]);
    assert_eq!(kinds(&pair.follower), ["Top10 B1:B5"]);

    // Ann changes Bob's rule, Bob deletes it: Ann's undo passes it by, in both rooms
    let changed = pair.write(&[json!({"op": "changeRule", "sheet": 1, "index": 0, "expect": view_of(&pair.writer, 0),
        "range": "B1:B4", "rule": {"type": "Bottom10", "rank": 1, "percent": false, "format": {}, "stop_if_true": false}})]);
    pair.write(&[json!({"op": "deleteRule", "sheet": 1, "index": 0, "expect": view_of(&pair.writer, 0)})]);
    let before = pair.states();
    pair.undo(changed);
    assert_eq!(lock(&pair.writer).rules_passed(), 1);
    assert_eq!(pair.states(), before, "nothing of a rule that is gone");
    // an edit of a call names its rule by what it is: one gone, or one named by its place alone, is refused
    let gone =
        json!({"op": "deleteRule", "sheet": 1, "index": 0, "expect": {"range": "B1:B4", "rule": {"type": "Bottom10"}}});
    assert!(refusal(&pair.writer, gone).contains("changed or deleted since"));
    assert!(refusal(&pair.writer, json!({"op": "deleteRule", "sheet": 1, "index": 0})).contains("named by what it is"));
}

/// The reviewer's case: rules A and B; Ann deletes a row of them, then deletes A, then takes both back. A is put back at
/// the end of the list, and the deletion's undo still gives each rule its own range back, found by id.
#[test]
fn a_deletions_undo_sets_back_each_rules_range_and_formulas_by_id() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    let rule_op = |kind: &str, range: &str, rule: Json| json!({"op": kind, "sheet": 1, "range": range, "rule": rule});
    pair.write(&[
        rule_op("addRule", "B1:B10", cell_is("GreaterThan", "1", "#FFC7CE")),
        rule_op("addRule", "C1:C10", cell_is("GreaterThan", "2", "#C8E6C9")),
        // a rule whose formula reads a cell of the row deleted: the deletion makes it #REF!
        rule_op("addRule", "D1:D3", cell_is("GreaterThan", "$F$5", "#BBDEFB")),
    ]);
    let before = pair.states();
    let rows = pair.write(&[json!({"op": "deleteRows", "sheet": 1, "row": 5, "count": 1})]);
    assert_eq!(kinds(&pair.writer), ["CellIs B1:B9", "CellIs C1:C9", "CellIs D1:D3"]);
    assert!(
        view_of(&pair.writer, 2)["rule"]["formula"]
            .as_str()
            .unwrap()
            .contains("#REF!"),
        "{}",
        view_of(&pair.writer, 2)
    );
    let deleted =
        pair.write(&[json!({"op": "deleteRule", "sheet": 1, "index": 0, "expect": view_of(&pair.writer, 0)})]);
    pair.undo(deleted);
    pair.undo(rows);
    assert_eq!(
        pair.states(),
        before,
        "each rule its own range and formula, in both rooms"
    );
    let formula = view_of(&pair.follower, at(&pair.follower, "D1:D3"))["rule"]["formula"].clone();
    assert_eq!(formula, "$F$5");

    // a rule another changed since the deletion is passed by; the others are set back
    let rows = pair.write(&[json!({"op": "deleteRows", "sheet": 1, "row": 5, "count": 1})]);
    let index = at(&pair.writer, "C1:C9");
    pair.write(&[
        json!({"op": "changeRule", "sheet": 1, "index": index, "expect": view_of(&pair.writer, index), "range": "C1:C9",
        "rule": cell_is("GreaterThan", "2", "#FFE0B2")}),
    ]);
    pair.undo(rows);
    assert_eq!(lock(&pair.writer).rules_passed(), 1);
    // the rule passed by is moved by the rows coming back, as the engine moves any range
    assert_eq!(kinds(&pair.writer), ["CellIs B1:B10", "CellIs C1:C10", "CellIs D1:D3"]);
    let changed = view_of(&pair.follower, at(&pair.follower, "C1:C10"));
    assert_eq!(
        changed["rule"]["format"]["fill"]["color"], "#FFE0B2",
        "the change stays"
    );
}

/// In one room (a person's own undo stack): Bob's insertion of rows above Ann's rule moves it, and her undo still takes
/// it back; Bob's change of her next rule makes her undo pass it by, the answer says so, and her next undo goes on.
#[test]
fn an_undo_passed_by_moves_the_persons_stack_on() {
    let rooms = Rooms::default();
    let room = room(&rooms, "ann", 3);
    let mut room = lock(&room);
    let rule_op = |kind: &str, range: &str, rule: Json| json!({"op": kind, "sheet": 1, "range": range, "rule": rule});
    room.edit("ann", &[json!({"op": "addSheet"})]).unwrap();
    room.edit(
        "ann",
        &[rule_op("addRule", "A5:A8", cell_is("GreaterThan", "1", "#FFC7CE"))],
    )
    .unwrap();
    room.edit("bob", &[json!({"op": "insertRows", "sheet": 1, "row": 1, "count": 2})])
        .unwrap();
    room.edit(
        "ann",
        &[rule_op("addRule", "B1:B5", cell_is("GreaterThan", "1", "#C8E6C9"))],
    )
    .unwrap();
    let shown = room.rules_of(1).unwrap()[1].clone();
    let expect = json!({"range": shown["range"], "rule": shown["rule"]});
    room.edit(
        "bob",
        &[
            json!({"op": "changeRule", "sheet": 1, "index": 1, "expect": expect, "range": "B1:B5",
        "rule": cell_is("GreaterThan", "1", "#FFE0B2")}),
        ],
    )
    .unwrap();
    // Ann's last edit's rule was changed by Bob: passed by, and said
    room.edit("ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(room.take_rules_passed(), 1);
    assert_eq!(room.rules_of(1).unwrap().len(), 2);
    // her next undo goes on to her first rule, which Bob's rows moved down
    assert_eq!(room.rules_of(1).unwrap()[0]["range"], "A7:A10");
    room.edit("ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(room.take_rules_passed(), 0);
    let left: Vec<Json> = room
        .rules_of(1)
        .unwrap()
        .iter()
        .map(|rule| rule["range"].clone())
        .collect();
    assert_eq!(
        left,
        [json!("B1:B5")],
        "Ann's moved rule taken back, Bob's changed one left"
    );
    assert_eq!(
        room.undo_state("ann"),
        (true, true),
        "her stack goes on: the sheet, and a redo"
    );
}

/// One call's steps are taken back in turn, each finding its rule as the step before left it.
#[test]
fn a_calls_steps_over_one_rule_are_taken_back_in_turn() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    let before = pair.states();
    let call = pair.write(&[
        json!({"op": "addRule", "sheet": 1, "range": "A2:A8", "rule": cell_is("GreaterThan", "1", "#FFC7CE")}),
        json!({"op": "deleteRows", "sheet": 1, "row": 3, "count": 2}),
    ]);
    assert_eq!(kinds(&pair.writer), ["CellIs A2:A6"]);
    pair.undo(call);
    assert_eq!(pair.states(), before);
    assert_eq!(lock(&pair.writer).rules_passed(), 0);

    pair.write(&[
        json!({"op": "addRule", "sheet": 1, "range": "A1:A3", "rule": cell_is("GreaterThan", "1", "#FFC7CE")}),
    ]);
    let x = pair.states();
    let changes = pair.write(&[
        json!({"op": "changeRule", "sheet": 1, "index": 0, "expect": view_of(&pair.writer, 0), "range": "A1:A3",
            "rule": cell_is("GreaterThan", "2", "#C8E6C9")}),
        json!({"op": "changeRule", "sheet": 1, "index": 0,
            "expect": {"range": "A1:A3", "rule": cell_is("GreaterThan", "2", "#C8E6C9")}, "range": "A1:A4",
            "rule": cell_is("GreaterThan", "3", "#BBDEFB")}),
    ]);
    assert_eq!(view_of(&pair.writer, 0)["range"], "A1:A4");
    pair.undo(changes);
    assert_eq!(pair.states(), x, "X back through Y");
}

/// A redo after its undo put the rule back at the end of the list finds it by what it is; a twin another deleted is not
/// taken for the one an undo removes; a rule put back goes just after the rule applied before it, and is passed by
/// when the sheet is full.
#[test]
fn a_redo_a_twin_and_a_put_back_rule_each_find_their_own() {
    let rooms = Rooms::default();
    let room = room(&rooms, "ann", 3);
    let mut room = lock(&room);
    let rule_op = |range: &str, fill: &str| json!({"op": "addRule", "sheet": 1, "range": range, "rule": cell_is("GreaterThan", "1", fill)});
    let named = |room: &Room, at: usize| {
        let rule = room.rules_of(1).unwrap()[at].clone();
        json!({"range": rule["range"], "rule": rule["rule"]})
    };
    let order = |room: &Room| {
        let mut rules = room.rules_of(1).unwrap();
        rules.sort_by_key(|rule| rule["priority"].as_u64());
        rules
            .iter()
            .map(|rule| rule["range"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    room.edit(
        "ann",
        &[
            json!({"op": "addSheet"}),
            rule_op("A1", "#FFC7CE"),
            rule_op("B1", "#C8E6C9"),
        ],
    )
    .unwrap();
    // a redo of a deletion after its undo put the rule back at the end
    let expect = named(&room, 0);
    room.edit(
        "ann",
        &[json!({"op": "deleteRule", "sheet": 1, "index": 0, "expect": expect})],
    )
    .unwrap();
    room.edit("ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(
        room.rules_of(1).unwrap()[1]["range"],
        "A1",
        "put back at the end of the list"
    );
    room.edit("ann", &[json!({"op": "redo"})]).unwrap();
    assert_eq!(order(&room), ["B1"], "the redo deleted A1, not B1 at its old place");

    // Ann adds a twin of B1; Bob deletes Ann's copy; Ann's undo leaves the first
    room.edit("ann", &[rule_op("B1", "#C8E6C9")]).unwrap();
    let expect = named(&room, 1);
    room.edit(
        "bob",
        &[json!({"op": "deleteRule", "sheet": 1, "index": 1, "expect": expect})],
    )
    .unwrap();
    room.edit("ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(room.take_rules_passed(), 1);
    assert_eq!(order(&room), ["B1"]);

    // A1, C1, D1 apply in that order; Ann deletes C1, Bob adds E1, Ann's undo puts C1 just after A1
    room.edit("ann", &[rule_op("C1", "#BBDEFB"), rule_op("D1", "#FFE0B2")])
        .unwrap();
    assert_eq!(order(&room), ["B1", "C1", "D1"]);
    let expect = named(&room, 1);
    room.edit(
        "ann",
        &[json!({"op": "deleteRule", "sheet": 1, "index": 1, "expect": expect})],
    )
    .unwrap();
    room.edit("bob", &[rule_op("E1", "#E1BEE7")]).unwrap();
    room.edit("ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(order(&room), ["B1", "C1", "D1", "E1"]);

    // a sheet full when a deleted rule would come back: passed by
    let expect = named(&room, 0);
    room.edit(
        "ann",
        &[json!({"op": "deleteRule", "sheet": 1, "index": 0, "expect": expect})],
    )
    .unwrap();
    let fill: Vec<Json> = (0..(MAX_SHEET_RULES - 3))
        .map(|at| rule_op(&format!("F{}", at + 1), "#FFC7CE"))
        .collect();
    room.edit("bob", &fill).unwrap();
    assert_eq!(room.rules_of(1).unwrap().len(), MAX_SHEET_RULES);
    room.edit("ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(room.take_rules_passed(), 1);
    assert_eq!(room.rules_of(1).unwrap().len(), MAX_SHEET_RULES);
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
    assert_eq!(all, 2_000, "the icon set left out, and the workbook's last");
    // the rule with a colour that is not one is kept, its colour set to none
    let first = room.rules_of(0).unwrap()[0].clone();
    assert_eq!(first["range"], "D1");
    assert!(first["rule"]["format"]["fill"]["color"].is_null(), "{first}");
    assert!(room
        .rules_of(0)
        .unwrap()
        .iter()
        .all(|rule| rule["rule"]["type"] != "IconSet"));

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

/// A workbook as the xlsx import gives one from a crafted file (its rgb kept as `#` and the raw text) has every colour
/// that is not `#RRGGBB` set to none when it opens: the export would write it into the XML of the files others get.
#[test]
fn a_files_colours_that_are_not_rrggbb_are_set_to_none_when_it_opens() {
    use moqui_core::workbook::ironcalc::base::types::{BorderItem, BorderStyle, Color, Dxf, Fill};
    let bad = || Color::Rgb("#00\"/><x a=\"".to_string());
    let mut base = fill::new_model("Notes").unwrap();
    let styles = &mut base.workbook.styles;
    let mut font = styles.fonts[0].clone();
    font.color = bad();
    styles.fonts.push(font);
    styles.fills.push(Fill { color: bad() });
    let mut border = styles.borders[0].clone();
    border.left = Some(BorderItem {
        style: BorderStyle::Thin,
        color: bad(),
    });
    styles.borders.push(border);
    styles.dxfs.push(Dxf {
        fill: Some(Fill { color: bad() }),
        ..Dxf::default()
    });
    styles.fills.push(Fill {
        color: Color::Rgb("#2E7D32".to_string()),
    });
    base.workbook.worksheets[0].color = bad();
    // the theme: a colour slot the engine would fail to read (and the export would write as it is), and its name
    base.workbook.theme.accent1 = "#12\"/><x a=\"".to_string();
    base.workbook.theme.name = "Evil\"><x".to_string();
    // the rules' own colours: a data bar's, a colour scale's, an icon set's and a rating's
    let evil = "#00\"/><x a=\"";
    for rule in [
        json!({"type": "DataBar", "min": null, "max": null, "positive_color": evil, "negative_color": "#FF0000",
            "is_gradient": false, "show_value": true}),
        json!({"type": "ColorScale", "thresholds": [{"cfvo": "Min", "color": evil}, {"cfvo": "Max", "color": "#63BE7B"}]}),
        json!({"type": "IconSet", "thresholds": [{"icon": "Circle", "cfvo": {"Number": 1.0}, "color": evil, "is_strict": false}],
            "show_value": true}),
        json!({"type": "IconRating", "icon": "Star", "color": evil, "thresholds": [[{"Number": 1.0}, false]], "show_value": true}),
    ] {
        let rule: moqui_core::workbook::ironcalc::base::cf_types::CfRuleInput = serde_json::from_value(rule).unwrap();
        base.add_conditional_formatting(0, "A1:A3", rule).unwrap();
    }
    base.set_user_input(0, 1, 1, "5".to_string()).unwrap();
    let rooms = Rooms::default();
    let room = opened(&rooms, base);
    let room = lock(&room);
    let workbook = &room.model().workbook;
    assert_eq!(
        (workbook.theme.accent1.as_str(), workbook.theme.name.as_str()),
        ("#4472C4", "Office")
    );
    let rules = serde_json::to_string(&room.rules_of(0).unwrap()).unwrap();
    assert_eq!(room.rules_of(0).unwrap().len(), 4, "kept, their colours set to none");
    assert!(!rules.contains("<x"), "{rules}");
    let all = serde_json::to_string(&(
        &workbook.styles.fonts,
        &workbook.styles.fills,
        &workbook.styles.borders,
        &workbook.styles.dxfs,
    ))
    .unwrap();
    assert!(!all.contains("<x"), "{all}");
    assert!(all.contains("#2E7D32"), "a colour that is one is kept");
    assert_eq!(workbook.worksheets[0].color, Color::None);
}

/// An added rule's removal compares how often it was changed, not its content: another's insertion of rows above
/// rewrites its formula and moves its range, and it is still taken back; a redo after an undo that passed an added
/// rule by does not add it again beside the rule another changed; a change set back over a range another's rows moved
/// is told as passed by in part.
#[test]
fn an_added_rule_is_found_by_its_changes_and_a_redo_leaves_out_what_its_undo_passed_by() {
    let rooms = Rooms::default();
    let room = room(&rooms, "ann", 3);
    let mut room = lock(&room);
    let rule = |range: &str, formula: &str, fill: &str| json!({"op": "addRule", "sheet": 1, "range": range, "rule": cell_is("GreaterThan", formula, fill)});
    let named = |room: &Room, at: usize| {
        let rule = room.rules_of(1).unwrap()[at].clone();
        json!({"range": rule["range"], "rule": rule["rule"]})
    };
    room.edit("ann", &[json!({"op": "addSheet"})]).unwrap();
    room.edit("ann", &[rule("B5:B8", "$A$5", "#FFC7CE")]).unwrap();
    room.edit("bob", &[json!({"op": "insertRows", "sheet": 1, "row": 1, "count": 2})])
        .unwrap();
    assert_eq!(
        room.rules_of(1).unwrap()[0]["rule"]["formula"],
        "$A$7",
        "Bob's rows rewrote its formula"
    );
    room.edit("ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(room.take_rules_passed(), 0);
    assert!(room.rules_of(1).unwrap().is_empty(), "Ann's rule taken back");

    // Ann adds R, Bob changes it, Ann's undo passes it by, and her redo does not add R again
    room.edit("ann", &[rule("C1:C3", "1", "#FFC7CE")]).unwrap();
    let expect = named(&room, 0);
    room.edit(
        "bob",
        &[
            json!({"op": "changeRule", "sheet": 1, "index": 0, "expect": expect, "range": "C1:C3",
        "rule": cell_is("GreaterThan", "1", "#BBDEFB")}),
        ],
    )
    .unwrap();
    room.edit("ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(room.take_rules_passed(), 1);
    room.edit("ann", &[json!({"op": "redo"})]).unwrap();
    assert_eq!(
        room.rules_of(1).unwrap().len(),
        1,
        "Bob's rule alone, no copy of Ann's beside it"
    );
    assert_eq!(
        room.rules_of(1).unwrap()[0]["rule"]["format"]["fill"]["color"],
        "#BBDEFB"
    );

    // Ann changes a rule's colour, Bob inserts rows above it: her undo sets the colour back and keeps the moved range,
    // and says so
    room.edit("bob", &[rule("D5:D8", "1", "#FFC7CE")]).unwrap();
    let expect = named(&room, 1);
    room.edit(
        "ann",
        &[
            json!({"op": "changeRule", "sheet": 1, "index": 1, "expect": expect, "range": "D5:D8",
        "rule": cell_is("GreaterThan", "1", "#C8E6C9")}),
        ],
    )
    .unwrap();
    room.edit("bob", &[json!({"op": "insertRows", "sheet": 1, "row": 1, "count": 1})])
        .unwrap();
    room.edit("ann", &[json!({"op": "undo"})]).unwrap();
    assert_eq!(room.take_rules_passed(), 1, "set back in part");
    let back = room.rules_of(1).unwrap()[1].clone();
    assert_eq!(
        (back["range"].as_str(), back["rule"]["format"]["fill"]["color"].as_str()),
        (Some("D6:D9"), Some("#FFC7CE"))
    );
}

/// A step the engine refuses is passed by, logged, and the rest of the undo applied; a step whose sheet is gone, or is
/// a data sheet now, is passed by too, of any kind.
#[test]
fn an_undo_passes_by_a_step_that_fails_or_whose_sheet_is_gone() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    pair.write(&[
        json!({"op": "addRule", "sheet": 1, "range": "A1:A3", "rule": cell_is("GreaterThan", "1", "#FFC7CE")}),
    ]);
    let id = lock(&pair.writer).rule_ids[1][0];
    {
        let mut room = lock(&pair.writer);
        room.record(
            90,
            vec![
                json!({"op": "setRule", "sheet": 1, "id": id, "range": "A1:A3", "rule": {"type": "Nope"},
                "expect": {"range": "A1:A3", "rule": cell_is("GreaterThan", "1", "#FFC7CE")}, "edits": 0, "part": 0}),
                json!({"op": "input", "sheet": 1, "row": 9, "column": 1, "value": "after", "part": 1}),
            ],
        );
        assert_eq!(
            room.undo_row(90).unwrap(),
            vec![0],
            "the refused step's op passed by whole"
        );
        assert_eq!(room.rules_passed(), 1);
        assert_eq!(room.cell_content(1, 9, 1).unwrap(), "after", "the rest applied");
    }
    // Ann styles a cell of Summary, Bob moves Summary before the data sheet: her undo's step on sheet 1 is the data
    // sheet's now, and is passed by
    let styled = pair.write(&[
        json!({"op": "style", "sheet": 1, "row": 1, "column": 1, "width": 1, "height": 1,
        "path": "font.b", "value": "true"}),
    ]);
    pair.write(&[json!({"op": "moveSheet", "sheet": 1, "to": 0})]);
    pair.undo(styled);
    assert!(lock(&pair.writer).rules_passed() > 0);
    assert!(
        lock(&pair.writer).model().get_style_for_cell(0, 1, 1).unwrap().font.b,
        "Summary's cell as it is"
    );
    // the rule, now on sheet 0, is found by its id: its removal still takes it back
    assert_eq!(lock(&pair.writer).rules_of(0).unwrap().len(), 1);
    pair.undo(1 + 1);
    assert!(lock(&pair.writer).rules_of(0).unwrap().is_empty());
    assert!(lock(&pair.follower).rules_of(0).unwrap().is_empty());
}

/// An insertion of rows inside a rule's range is held to the rules' bounds (every room decides it alike); a file's
/// rules on a data sheet are left out when it opens.
#[test]
fn an_insertion_is_held_to_the_rules_bounds_and_a_data_sheets_rules_are_left_out() {
    let rooms = Rooms::default();
    let room = room(&rooms, "ann", 3);
    let mut room = lock(&room);
    room.edit(
        "ann",
        &[
            json!({"op": "addSheet"}),
            json!({"op": "addRule", "sheet": 1, "range": "A1:J10000", "rule": cell_is("GreaterThan", "1", "#FFC7CE")}),
        ],
    )
    .unwrap();
    let err = room
        .edit("ann", &[json!({"op": "insertRows", "sheet": 1, "row": 5, "count": 1})])
        .expect_err("past the bound");
    assert!(err.message().contains("more than 100000 cells"), "{}", err.message());
    room.edit("ann", &[json!({"op": "insertRows", "sheet": 1, "row": 1, "count": 1})])
        .unwrap();
    room.edit(
        "ann",
        &[json!({"op": "insertColumns", "sheet": 1, "column": 20, "count": 1})],
    )
    .unwrap();

    let mut base = fill::new_model("Orders").unwrap();
    let rule: moqui_core::workbook::ironcalc::base::cf_types::CfRuleInput =
        serde_json::from_value(cell_is("GreaterThan", "1", "#FFC7CE")).unwrap();
    base.add_conditional_formatting(0, "B2:B4", rule).unwrap();
    let data = rooms
        .open(Opening {
            user_id: "ann".to_string(),
            shared: None,
            profile_id: None,
            definition_id: None,
            version: None,
            title: "Orders".to_string(),
            data_sheets: Vec::new(),
            charts: Vec::new(),
            base,
            rows: vec![orders(3)],
        })
        .unwrap();
    assert!(lock(&data).rules_of(0).unwrap().is_empty());
}

/// Rows an undo puts back inside a rule another added since would grow it past a rule's bound: the rule is taken off
/// after the undo, as a room opened anew would trim it, in every room alike, and told.
#[test]
fn an_undo_that_grows_a_rule_past_the_bounds_takes_it_off_in_every_room() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    let deleted = pair.write(&[json!({"op": "deleteRows", "sheet": 1, "row": 5, "count": 1})]);
    pair.write(&[
        json!({"op": "addRule", "sheet": 1, "range": "A1:J10000", "rule": cell_is("GreaterThan", "1", "#FFC7CE")}),
    ]);
    pair.undo(deleted);
    assert!(lock(&pair.writer).rules_passed() >= 1);
    for room in [&pair.writer, &pair.follower] {
        assert!(lock(room).rules_of(1).unwrap().is_empty(), "past the bound, taken off");
    }
}

/// A step of an undo the engine refuses as the sheets are now is passed by and the rest applied: a sheet's old name
/// another took since, a place no longer there.
#[test]
fn an_undo_passes_by_a_sheet_step_the_engine_refuses() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    let names = |room: &SharedRoom| lock(room).model().workbook.get_worksheet_names();
    // Ann renames Summary to Budget, Bob names another sheet Summary: Ann's undo cannot take the name back
    let renamed = pair.write(&[json!({"op": "renameSheet", "sheet": 1, "name": "Budget"})]);
    pair.write(&[
        json!({"op": "addSheet"}),
        json!({"op": "renameSheet", "sheet": 2, "name": "Summary"}),
    ]);
    pair.undo(renamed);
    assert_eq!(lock(&pair.writer).rules_passed(), 1);
    assert_eq!(names(&pair.writer), ["Orders", "Budget", "Summary"]);
    assert_eq!(names(&pair.follower), ["Orders", "Budget", "Summary"]);
    // Ann moves the last sheet to 1, Bob deletes one: the move back is to a place no longer there
    pair.write(&[
        json!({"op": "addSheet"}),
        json!({"op": "renameSheet", "sheet": 3, "name": "Four"}),
    ]);
    let moved = pair.write(&[json!({"op": "moveSheet", "sheet": 3, "to": 1})]);
    pair.write(&[json!({"op": "deleteSheet", "sheet": 2})]);
    pair.undo(moved);
    assert_eq!(lock(&pair.writer).rules_passed(), 1);
    assert_eq!(names(&pair.writer), ["Orders", "Four", "Summary"]);
    assert_eq!(names(&pair.follower), names(&pair.writer));
}

/// A deleted sheet's undo names the sheet it adds wherever it lands, and an added sheet's undo deletes that sheet by
/// the engine's id: another's sheets added or deleted between are left as they are.
#[test]
fn a_sheets_undo_finds_its_own_sheet_after_anothers() {
    let rooms = Rooms::default();
    let mut pair = Pair::new(&rooms);
    let names = |room: &SharedRoom| lock(room).model().workbook.get_worksheet_names();
    // Ann's sheet Two with a cell, deleted; Bob adds a sheet with a cell of his; Ann's undo puts Two back as it was
    pair.write(&[
        json!({"op": "addSheet"}),
        json!({"op": "renameSheet", "sheet": 2, "name": "Two"}),
        json!({"op": "input", "sheet": 2, "row": 1, "column": 1, "value": "Ann's"}),
    ]);
    let deleted = pair.write(&[json!({"op": "deleteSheet", "sheet": 2})]);
    pair.write(&[
        json!({"op": "addSheet"}),
        json!({"op": "renameSheet", "sheet": 2, "name": "Bob"}),
        json!({"op": "input", "sheet": 2, "row": 1, "column": 1, "value": "Bob's"}),
    ]);
    pair.undo(deleted);
    for room in [&pair.writer, &pair.follower] {
        assert_eq!(names(room), ["Orders", "Summary", "Two", "Bob"]);
        let room = lock(room);
        assert_eq!(
            (room.cell_content(2, 1, 1).unwrap(), room.cell_content(3, 1, 1).unwrap()),
            ("Ann's".into(), "Bob's".into())
        );
    }
    // Ann adds a sheet; Bob deletes Summary and adds one; Ann's undo deletes her sheet, not his
    let added = pair.write(&[
        json!({"op": "addSheet"}),
        json!({"op": "renameSheet", "sheet": 4, "name": "Ann"}),
    ]);
    pair.write(&[
        json!({"op": "deleteSheet", "sheet": 1}),
        json!({"op": "addSheet"}),
        json!({"op": "renameSheet", "sheet": 4, "name": "Bob2"}),
    ]);
    pair.undo(added);
    for room in [&pair.writer, &pair.follower] {
        assert_eq!(names(room), ["Orders", "Two", "Bob", "Bob2"]);
    }
}
