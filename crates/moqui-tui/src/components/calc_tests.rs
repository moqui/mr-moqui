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
use crate::components::line_text;

/// A room as open#CalcDocument answers it: a data sheet of orders with its header frozen, and a sheet of the author's.
fn props(editable: bool) -> Value {
    json!({"editable": editable, "profileId": "sales", "room": {
        "roomId": "d1", "shared": true, "seq": 3, "unsaved": 2, "title": "Orders", "canUndo": true,
        "functions": ["SUM", "SUMIF", "COUNTA"],
        "sheets": [
            {"index": 0, "name": "Orders", "rows": 4, "columns": 2, "widths": [80, 96], "frozenRows": 1, "frozenColumns": 0, "data": true, "dataRows": 3},
            {"index": 1, "name": "Sum", "rows": 2, "columns": 1, "widths": [120], "frozenRows": 0, "frozenColumns": 0, "data": false}
        ],
        "charts": [{"title": "By order", "labels": ["O1", "O2"], "series": [{"name": "Total", "data": [110, 220]}]}]
    }})
}

/// The cells of the data sheet: its header (frozen) and the rows below.
fn blocks() -> (Value, Value) {
    let frozen = json!({"row": 1, "column": 1, "width": 40, "height": 1, "seq": 3,
        "styles": [{"style": {"font": {"b": true}, "alignment": null}}], "style_indices": [0, 0],
        "texts": [{"row": 1, "column": 1, "text": "orderId", "cell_type": 2}, {"row": 1, "column": 2, "text": "total", "cell_type": 2}]});
    let main = json!({"row": 2, "column": 1, "width": 40, "height": 20, "seq": 3, "styles": [], "style_indices": [],
        "peers": [{"client": "c1", "name": "Ann", "sheet": 0, "area": {"row": 3, "column": 2, "width": 1, "height": 1}}],
        "texts": [{"row": 2, "column": 1, "text": "O1 with a long name", "cell_type": 2}, {"row": 2, "column": 2, "text": "110", "cell_type": 1},
            {"row": 3, "column": 1, "text": "O2", "cell_type": 2}, {"row": 3, "column": 2, "text": "220", "cell_type": 1}]});
    (frozen, main)
}

/// A view with its blocks read, as the session's reads leave it.
fn loaded(editable: bool) -> (Value, ComponentState) {
    let props = props(editable);
    let mut state = ComponentState {
        rows: 30,
        ..ComponentState::default()
    };
    let (frozen, main) = blocks();
    // the frozen rows first, then the rows in view, then the active cell's content
    let first = CalcGrid.source(&props, &state).unwrap();
    assert_eq!(
        (first.0.as_str(), first.1["row"].as_str(), first.1["height"].as_str()),
        ("calcCells", "1", "1")
    );
    assert_eq!(first.1["profileId"], "sales");
    CalcGrid.receive(&props, &mut state, frozen);
    let second = CalcGrid.source(&props, &state).unwrap();
    assert_eq!((second.0.as_str(), second.1["row"].as_str()), ("calcCells", "2"));
    assert!(second.1.contains_key("client") && !second.1.contains_key("since"));
    CalcGrid.receive(&props, &mut state, main);
    assert_eq!(CalcGrid.source(&props, &state).unwrap().0, "calcContent");
    CalcGrid.receive(&props, &mut state, json!({"content": "O1 with a long name"}));
    assert!(!View::of(&props, &state).needs(), "everything on screen is read");
    // the session takes the request for more after each answer
    state.reload_pending = false;
    (props, state)
}

fn texts(props: &Value, state: &ComponentState, width: u16) -> Vec<String> {
    CalcGrid
        .lines(props, state, width, false)
        .iter()
        .map(line_text)
        .collect()
}

#[test]
fn a_sheet_is_its_cells_under_their_columns_with_the_frozen_rows_above_and_the_others_places() {
    let (props, state) = loaded(true);
    let lines = texts(&props, &state, 60);
    assert!(
        lines[0].starts_with("A1") && lines[0].contains("O1 with a long name"),
        "{lines:?}"
    );
    // the column letters over the widths (80 and 96 pixels: 10 and 12 characters)
    assert!(lines[1].contains("A") && lines[1].contains("B"), "{lines:?}");
    assert!(lines[2].trim_start().starts_with("1 orderId"), "{lines:?}");
    // a text runs on into an empty cell only; a number stands right
    assert!(lines[3].contains("O1 with a"), "{lines:?}");
    assert!(lines[3].trim_end().ends_with("110"), "{lines:?}");
    let tabs = lines.iter().find(|line| line.contains("Orders (data)")).unwrap();
    assert!(tabs.contains("Sum"));
    let status = lines.iter().find(|line| line.contains("unsaved")).unwrap();
    assert!(
        status.contains("3 rows from the database, read only") && status.contains("2 unsaved edits"),
        "{status}"
    );
    assert!(status.contains("Ann at Orders!B3"), "{status}");
    // the cursor cell is reversed, Ann's cell underlined in the warning role
    let grid = CalcGrid.lines(&props, &state, 60, false);
    let cursor = grid[2].iter().find(|span| span.reverse).unwrap();
    assert!(cursor.text.contains("orderId"));
    assert!(grid[4]
        .iter()
        .any(|span| span.role == Role::Warning && span.underline && span.text.contains("220")));
}

#[test]
fn keys_move_and_select_and_read_what_comes_into_view() {
    let (props, mut state) = loaded(true);
    let acts = CalcGrid
        .key(&props, &mut state, &KeyPress::plain(Key::Down), false)
        .unwrap();
    assert_eq!(acts, [Act::Reload]);
    let view = View::of(&props, &state);
    assert_eq!((view.row, view.column), (2, 1));
    assert!(
        view.need_content && !view.need_cells,
        "the cells in view are read already"
    );
    assert_eq!(CalcGrid.source(&props, &state).unwrap().0, "calcContent");
    let shifted = KeyPress {
        shift: true,
        ..KeyPress::plain(Key::Right)
    };
    CalcGrid.key(&props, &mut state, &shifted, false);
    assert_eq!(View::of(&props, &state).address(), "A2:B2");
    // a far page down reads the rows there
    for _ in 0..5 {
        CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::PageDown), false);
    }
    let view = View::of(&props, &state);
    assert!(view.row > 80 && view.need_cells && view.top > 60);
    assert_eq!(CalcGrid.source(&props, &state).unwrap().1["row"], view.top.to_string());
    // another sheet starts at its A1
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Char(']')), false);
    let view = View::of(&props, &state);
    assert_eq!((view.sheet, view.row, view.column, view.need_frozen), (1, 1, 1, false));
}

#[test]
fn an_edit_is_posted_a_data_sheet_is_not_typed_into_and_a_reader_does_not_edit() {
    let (props, mut state) = loaded(true);
    // the data sheet takes no typing
    assert!(CalcGrid
        .key(&props, &mut state, &KeyPress::plain(Key::Char('=')), false)
        .is_none());
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Char(']')), false);
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Char('=')), false);
    for ch in "su".chars() {
        CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Char(ch)), false);
    }
    // completion of a function, then of the data sheet's table
    let lines = texts(&props, &state, 60);
    assert!(lines[1].contains("SUM(") && lines[1].contains("SUMIF("), "{lines:?}");
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Tab), false);
    for ch in "Ord".chars() {
        CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Char(ch)), false);
    }
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Tab), false);
    assert_eq!(View::of(&props, &state).editing.unwrap().text, "=SUM(Orders[");
    for ch in "total])".chars() {
        CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Char(ch)), false);
    }
    let acts = CalcGrid
        .key(&props, &mut state, &KeyPress::plain(Key::Enter), false)
        .unwrap();
    let [Act::Call { url, parameters }] = acts.as_slice() else {
        panic!("{acts:?}")
    };
    assert_eq!(url, "calcEdit");
    assert_eq!(
        (parameters["roomId"].as_str(), parameters["profileId"].as_str()),
        ("d1", "sales")
    );
    let ops: Value = serde_json::from_str(&parameters["opsJson"]).unwrap();
    assert_eq!(
        ops,
        json!([{"op": "input", "sheet": 1, "row": 1, "column": 1, "value": "=SUM(Orders[total])"}])
    );
    assert_eq!(View::of(&props, &state).row, 2, "Enter moves down");
    // undo is posted as an edit; a paste is rows of cells
    let acts = CalcGrid
        .key(&props, &mut state, &KeyPress::plain(Key::Char('u')), false)
        .unwrap();
    assert!(matches!(&acts[0], Act::Call { parameters, .. } if parameters["opsJson"].contains("undo")));
    let acts = CalcGrid.paste(&props, &mut state, "a\tb\n1\t2\n").unwrap();
    let Act::Call { parameters, .. } = &acts[0] else {
        panic!()
    };
    let ops: Value = serde_json::from_str(&parameters["opsJson"]).unwrap();
    assert_eq!(ops[0]["rows"], json!([["a", "b"], ["1", "2"]]));
    // the room's answer is taken and the cells read again
    CalcGrid.receive(
        &props,
        &mut state,
        json!({"roomId": "d1", "shared": true, "seq": 5, "sheets": props["room"]["sheets"], "errors": null}),
    );
    let view = View::of(&props, &state);
    assert_eq!(view.seq, 5);
    assert!(view.need_cells && state.reload_pending);
    assert_eq!(
        view.functions(),
        ["SUM", "SUMIF", "COUNTA"],
        "kept from the opening answer"
    );

    let (props, mut state) = loaded(false);
    let acts = CalcGrid
        .key(&props, &mut state, &KeyPress::plain(Key::Delete), false)
        .unwrap();
    assert!(acts.is_empty());
    assert!(View::of(&props, &state).message.contains("read only"));
}

#[test]
fn another_edit_reads_the_room_again_and_a_room_gone_opens_on_enter() {
    let (props, mut state) = loaded(true);
    // a poll that finds nothing new is only the others' places
    let poll = CalcGrid.source(&props, &state).unwrap();
    assert_eq!(poll.1["since"], "3");
    CalcGrid.receive(&props, &mut state, json!({"unchanged": true, "seq": 3, "peers": []}));
    assert!(View::of(&props, &state).peers.is_empty());
    assert_eq!(CalcGrid.poll_millis(&props, &state), Some(1_500));
    // another's edit: the room's state, then its cells
    CalcGrid.receive(
        &props,
        &mut state,
        json!({"row": 2, "column": 1, "width": 40, "height": 20, "seq": 4, "texts": [], "style_indices": []}),
    );
    assert_eq!(CalcGrid.source(&props, &state).unwrap().0, "calcState");
    CalcGrid.receive(&props, &mut state, json!({"errors": "ROOM_GONE: open it again"}));
    assert!(CalcGrid.source(&props, &state).is_none());
    assert!(texts(&props, &state, 60)[0].contains("Enter opens it again"));
    assert_eq!(
        CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Enter), false),
        Some(vec![Act::ReloadScreen])
    );
}

#[test]
fn the_menu_and_the_charts() {
    let (props, mut state) = loaded(true);
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Char('m')), false);
    assert!(texts(&props, &state, 60)
        .iter()
        .any(|line| line.contains("Insert a row above")));
    for _ in 0..15 {
        CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Down), false);
    }
    let acts = CalcGrid
        .key(&props, &mut state, &KeyPress::plain(Key::Enter), false)
        .unwrap();
    assert_eq!(
        acts,
        [Act::DownloadReport(
            "calcExport?json=true&roomId=d1&profileId=sales".to_string()
        )]
    );
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Char('c')), false);
    let lines = texts(&props, &state, 60);
    assert!(lines.iter().any(|line| line.contains("By order")) && lines.iter().any(|line| line.contains("220")));
    assert_eq!(column_name(28), "AB");
    assert_eq!(completions("=A1+B", 5, &["BIN".to_string()], &[]).1, ["BIN("]);
    assert!(
        completions("=SUM(B2", 7, &["B2X".to_string()], &[]).1.is_empty(),
        "a cell's name is not completed"
    );
}
