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
use crate::fill::CellData;

fn rows(count: usize) -> SheetRows {
    SheetRows {
        name: "Orders".to_string(),
        columns: vec!["region".to_string(), "amount".to_string()],
        rows: (1..=count)
            .map(|row| {
                #[allow(clippy::cast_precision_loss)]
                let amount = row as f64 * 10.0;
                vec![CellData::Text(format!("Region {}", row % 3)), CellData::Number(amount)]
            })
            .collect(),
        truncated: count >= 50,
    }
}

fn opening(user: &str, count: usize) -> Opening {
    Opening {
        user_id: user.to_string(),
        profile_id: None,
        definition_id: None,
        version: None,
        title: "Orders".to_string(),
        data_sheets: vec![DataSheet {
            name: "Orders".to_string(),
            spec: json!({"name": "Orders", "source": {"type": "entity", "entityName": "test.Order"}}),
        }],
        charts: vec![
            json!({"title": "By row", "type": "bar", "categories": "Orders!A2:A4", "series": [{"name": "Amount", "values": "Orders!B2:B4"}]}),
            json!({"title": "Broken", "type": "line", "series": [{"name": "x", "values": "Nowhere!A1:A2"}]}),
        ],
        base: fill::new_model("Orders").unwrap(),
        rows: vec![rows(count)],
    }
}

#[test]
fn a_room_is_its_users_and_answers_blocks_charts_and_its_layout() {
    let rooms = Rooms::default();
    let room = rooms.open(opening("alice", 5)).unwrap();
    let id = lock(&room).id.clone();
    assert!(rooms.get("alice", &id).is_some());
    assert!(rooms.get("bob", &id).is_none(), "another user finds no such room");
    assert!(rooms.get("alice", "r0").is_none());

    let room = lock(&room);
    assert_eq!(room.data_rows, [("Orders".to_string(), 5)]);
    assert!(room.truncated.is_empty());
    let layout = room.layout();
    assert_eq!(layout[0]["name"], "Orders");
    assert_eq!(layout[0]["rows"], 6);
    assert_eq!(layout[0]["data"], true);

    let block: serde_json::Value = serde_json::from_str(&room.block(0, 1, 1, 2, 3).unwrap()).unwrap();
    let texts: Vec<(i64, i64, &str)> = block["texts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|text| {
            (
                text["row"].as_i64().unwrap(),
                text["column"].as_i64().unwrap(),
                text["text"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        texts,
        [
            (1, 1, "region"),
            (1, 2, "amount"),
            (2, 1, "Region 1"),
            (2, 2, "10"),
            (3, 1, "Region 2"),
            (3, 2, "20")
        ]
    );
    assert_eq!(block["style_indices"].as_array().unwrap().len(), 6);
    assert!(room.block(0, 1, 1, 200, 200).is_err(), "a viewport over the cell limit");
    assert!(room.block(9, 1, 1, 2, 2).is_err(), "a sheet the workbook does not have");

    let charts = room.charts();
    assert_eq!(charts[0]["labels"], json!(["Region 1", "Region 2", "Region 0"]));
    assert_eq!(charts[0]["series"][0]["data"], json!([10.0, 20.0, 30.0]));
    assert_eq!(charts[0]["series"][0]["name"], "Amount");
    assert!(charts[1]["error"].as_str().unwrap().contains("no sheet named Nowhere"));
}

#[test]
fn a_refresh_reads_other_rows_and_the_stored_form_has_none() {
    let rooms = Rooms::default();
    let room = rooms.open(opening("alice", 5)).unwrap();
    let mut room = lock(&room);
    room.refresh(vec![rows(60)]).unwrap();
    assert_eq!(room.data_rows, [("Orders".to_string(), 60)]);
    assert_eq!(room.truncated, ["Orders"]);
    assert_eq!(room.layout()[0]["rows"], 61);
    assert_eq!(room.model().get_formatted_cell_value(0, 61, 2).unwrap(), "600");
    let stored = fill::open_stored(&room.stored_form().unwrap()).unwrap();
    assert_eq!(stored.get_formatted_cell_value(0, 1, 1).unwrap(), "region");
    assert_eq!(stored.get_formatted_cell_value(0, 2, 1).unwrap(), "");
    assert!(stored
        .workbook
        .shared_strings
        .iter()
        .all(|text| !text.starts_with("Region")));
}

#[test]
fn a_user_holds_a_limited_number_of_rooms_and_a_closed_one_is_gone() {
    let rooms = Rooms::default();
    let first = lock(&rooms.open(opening("alice", 1)).unwrap()).id.clone();
    let mut last = String::new();
    for _ in 0..MAX_USER_ROOMS {
        last = lock(&rooms.open(opening("alice", 1)).unwrap()).id.clone();
    }
    let other = lock(&rooms.open(opening("bob", 1)).unwrap()).id.clone();
    assert_eq!(rooms.count(), MAX_USER_ROOMS + 1);
    assert!(
        rooms.get("alice", &first).is_none(),
        "the room used longest ago was dropped"
    );
    assert!(rooms.get("alice", &last).is_some());
    assert!(
        rooms.get("bob", &other).is_some(),
        "another user's rooms are not counted"
    );
    rooms.close("alice", &last);
    assert!(rooms.get("alice", &last).is_none());
    rooms.close("alice", &other);
    assert!(rooms.get("bob", &other).is_some(), "a room is closed only by its user");
}

#[test]
fn edits_are_applied_in_order_evaluated_once_and_undone_and_a_data_sheet_is_not_edited() {
    let rooms = Rooms::default();
    let room = rooms.open(opening("alice", 5)).unwrap();
    let mut room = lock(&room);
    assert_eq!(room.undo_state(), (false, false));
    // a sheet beside the data, a formula over the data sheet's table, a text, a pasted block, a style and a width
    room.edit(&[
        json!({"op": "addSheet"}),
        json!({"op": "renameSheet", "sheet": 1, "name": "Summary"}),
        json!({"op": "input", "sheet": 1, "row": 1, "column": 1, "value": "=SUM(Orders[amount])"}),
        json!({"op": "input", "sheet": 1, "row": 2, "column": 1, "value": "Total above"}),
        json!({"op": "paste", "sheet": 1, "row": 4, "column": 1, "rows": [["1", "2"], ["=A4+B4", "x"]]}),
        json!({"op": "style", "sheet": 1, "row": 1, "column": 1, "width": 1, "height": 1, "path": "font.b", "value": "true"}),
        json!({"op": "style", "sheet": 1, "row": 1, "column": 1, "width": 1, "height": 1, "path": "num_fmt", "value": "#,##0.00"}),
        json!({"op": "width", "sheet": 1, "column": 1, "width": 180}),
        json!({"op": "freeze", "sheet": 1, "rows": 1, "columns": 0}),
    ])
    .unwrap();
    let cell = |room: &Room, sheet, row, column| room.model().get_formatted_cell_value(sheet, row, column).unwrap();
    assert_eq!(cell(&room, 1, 1, 1), "150.00");
    assert_eq!(cell(&room, 1, 5, 1), "3");
    assert_eq!(room.cell_content(1, 1, 1).unwrap(), "=SUM(Orders!$B$2:$B$6)");
    assert_eq!(room.cell_content(1, 2, 1).unwrap(), "Total above");
    let layout = room.layout();
    assert_eq!(layout[1]["name"], "Summary");
    assert_eq!(layout[1]["data"], false);
    assert_eq!(layout[1]["widths"][0], 180.0);
    assert_eq!(layout[1]["frozenRows"], 1);
    let block: serde_json::Value = serde_json::from_str(&room.block(1, 1, 1, 1, 1).unwrap()).unwrap();
    assert_eq!(block["styles"][0]["style"]["font"]["b"], true);
    assert_eq!(room.undo_state(), (true, false));

    // a data sheet's cells, rows and name are not edited; its widths are
    for op in [
        json!({"op": "input", "sheet": 0, "row": 2, "column": 2, "value": "999"}),
        json!({"op": "clear", "sheet": 0, "row": 2, "column": 1, "width": 2, "height": 2}),
        json!({"op": "deleteRows", "sheet": 0, "row": 2, "count": 1}),
        json!({"op": "renameSheet", "sheet": 0, "name": "Mine"}),
        json!({"op": "deleteSheet", "sheet": 0}),
    ] {
        let err = room.edit(std::slice::from_ref(&op)).expect_err("a data sheet edit");
        assert!(err.message().contains("is a data sheet"), "{op}: {}", err.message());
    }
    assert_eq!(cell(&room, 0, 2, 2), "10");
    room.edit(&[json!({"op": "width", "sheet": 0, "column": 1, "width": 200})])
        .unwrap();

    // the edits before a refused one stay, and an unknown edit is refused
    let err = room
        .edit(&[
            json!({"op": "input", "sheet": 1, "row": 9, "column": 1, "value": "kept"}),
            json!({"op": "nonsense"}),
        ])
        .expect_err("an unknown edit");
    assert!(err.message().contains("is not an edit"), "{}", err.message());
    assert_eq!(cell(&room, 1, 9, 1), "kept");

    // undo and redo, and a clear
    room.edit(&[json!({"op": "undo"})]).unwrap();
    assert_eq!(cell(&room, 1, 9, 1), "");
    room.edit(&[json!({"op": "redo"})]).unwrap();
    assert_eq!(cell(&room, 1, 9, 1), "kept");
    room.edit(&[json!({"op": "clear", "sheet": 1, "row": 9, "column": 1, "width": 1, "height": 1})])
        .unwrap();
    assert_eq!(cell(&room, 1, 9, 1), "");

    // the engine's autofill carries a formula down
    room.edit(&[
        json!({"op": "input", "sheet": 1, "row": 1, "column": 4, "value": "=Orders!B2*2"}),
        json!({"op": "fill", "sheet": 1, "row": 1, "column": 4, "width": 1, "height": 1, "toRow": 3}),
    ])
    .unwrap();
    assert_eq!(cell(&room, 1, 3, 4), "60");

    // the charts are the document's, replaced as a whole
    room.edit(&[json!({"op": "charts", "charts": [{"title": "Amounts", "type": "bar", "categories": "Orders!A2:A3", "series": [{"name": "Amount", "values": "Orders!B2:B3"}]}]})])
        .unwrap();
    assert_eq!(room.charts()[0]["series"][0]["data"], json!([10.0, 20.0]));

    // the stored form keeps the author's sheet and formula, with no row of the data; a refresh keeps them over new rows
    let stored = fill::open_stored(&room.stored_form().unwrap()).unwrap();
    assert_eq!(stored.workbook.get_worksheet_names(), ["Orders", "Summary"]);
    assert_eq!(
        stored.get_cell_formula(1, 1, 1).unwrap().as_deref(),
        Some("=SUM(Orders!$B$2:$B$3)")
    );
    assert_eq!(stored.get_formatted_cell_value(1, 1, 1).unwrap(), "0.00");
    room.refresh(vec![rows(8)]).unwrap();
    assert_eq!(cell(&room, 1, 1, 1), "360.00");
    assert_eq!(room.undo_state(), (false, false), "a refresh ends the undo history");
}
