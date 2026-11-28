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

use super::charts::{range_name, CHART_MENU};
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
        "charts": [{"id": "c1", "title": "By order", "type": "bar", "categories": "Orders!A2:A3", "labels": ["O1", "O2"],
            "series": [{"name": "Total", "values": "Orders!B2:B3", "data": [110, 220]}]}]
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

/// SEC-CLI-83: a sheet's frozen counts are the workbook's, which an xlsx file sets to any number; the grid draws and
/// reads only the frozen rows that leave the terminal `MIN_ROWS` of the others, at most `MAX_FROZEN_ROWS`, and the
/// frozen columns that fit its width, so a frame stays the size of the terminal.
#[test]
fn a_workbooks_frozen_counts_are_held_to_what_a_terminal_shows() {
    for (rows, columns) in [(1_048_575, 0), (i64::from(i32::MAX), i64::from(i32::MAX))] {
        let mut props = props(true);
        props["room"]["sheets"][0]["frozenRows"] = json!(rows);
        props["room"]["sheets"][0]["frozenColumns"] = json!(columns);
        for (terminal, frozen) in [(30u16, 30 - CHROME_ROWS - MIN_ROWS), (1_000, MAX_FROZEN_ROWS)] {
            let state = ComponentState {
                rows: terminal,
                ..ComponentState::default()
            };
            let lines = CalcGrid.lines(&props, &state, 120, false);
            let most = usize::from(terminal);
            assert!(
                lines.len() < most,
                "{rows} frozen rows drew {} lines in {terminal}",
                lines.len()
            );
            // a span or two a column, and the columns of a 120-character line are at most 30
            assert!(lines.iter().all(|line| line.len() < 80), "{columns} frozen columns");
            let (name, read) = CalcGrid.source(&props, &state).unwrap();
            assert_eq!(name, "calcCells");
            assert_eq!(read["height"], frozen.to_string(), "{read:?}");
        }
    }
}

/// SEC-CLI-83: a frozen column that does not fit the width is not drawn, and scrolls as the others do: moving the
/// active cell to it brings it on screen.
#[test]
fn a_frozen_column_past_the_width_scrolls_into_view() {
    let (props, mut state) = loaded(true);
    // the room as the view keeps it
    state.local["room"]["sheets"][0]["frozenColumns"] = json!(50);
    for _ in 1..30 {
        CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Right), false);
    }
    let lines = texts(&props, &state, 60);
    let header = &lines[1];
    assert!(
        header.split_whitespace().any(|name| name == column_name(30)),
        "{header}"
    );
    assert!(
        !header.split_whitespace().any(|name| name == column_name(50)),
        "{header}"
    );
}

/// CG3.1: a cell's font colour, fill and italic are kept from the engine's block and drawn, the fill over the gap to a
/// neighbour of the same fill; the active cell is drawn in the theme's roles alone.
#[test]
fn a_cells_colours_and_italic_are_drawn_but_not_on_the_cursor() {
    let (props, mut state) = loaded(true);
    let main = json!({"row": 2, "column": 1, "width": 40, "height": 20, "seq": 3,
        "styles": [{"style": {"font": {}}}, {"style": {"font": {"color": "#C00000", "i": true}}}],
        "style_indices": [0, 1], "texts": [{"row": 2, "column": 2, "text": "110", "cell_type": 1}],
        "fills": [{"row": 2, "column": 1, "width": 2, "color": "#FFFF00"}, {"row": 2, "column": 3, "width": 38, "color": ""},
            {"row": 3, "column": 1, "width": 40, "color": ""}]});
    CalcGrid.receive(&props, &mut state, main);
    let found = cells(&View::of(&props, &state).block);
    let red = found[&(2, 2)].clone();
    assert_eq!(
        (red.color, red.fill, red.italic),
        (Some(Rgb(0xC0, 0, 0)), Some(Rgb(255, 255, 0)), true)
    );
    assert_eq!(found[&(2, 1)].fill, Some(Rgb(255, 255, 0)));
    assert!(!found.contains_key(&(3, 1)), "a run of no colour is no cell");
    let grid = CalcGrid.lines(&props, &state, 60, false);
    let row = &grid[3];
    let span = row.iter().find(|span| span.text.contains("110")).unwrap();
    assert_eq!(
        (span.color, span.fill, span.italic),
        (Some(Rgb(0xC0, 0, 0)), Some(Rgb(255, 255, 0)), true)
    );
    // A2 and B2 share the fill, so the gap between them has it; the gap after B2 does not
    let gaps: Vec<&Span> = row.iter().filter(|span| span.text == " ").collect();
    assert_eq!((gaps[0].fill, gaps[1].fill), (Some(Rgb(255, 255, 0)), None));
    // on the active cell the theme's roles alone show where it is
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Down), false);
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Right), false);
    let grid = CalcGrid.lines(&props, &state, 60, false);
    let cursor = grid[3].iter().find(|span| span.reverse).unwrap();
    assert!(cursor.text.contains("110"));
    assert_eq!((cursor.color, cursor.fill), (None, None));
}

/// CG3.5: the status line has the last evaluation's time, a warning past the room's `slowMs`, else past 250 ms.
#[test]
fn the_status_line_has_the_calculation_time_and_warns_when_it_is_slow() {
    let status = |room: Value| {
        let mut props = props(true);
        for (key, value) in room.as_object().unwrap() {
            props["room"][key] = value.clone();
        }
        let state = ComponentState {
            rows: 30,
            ..ComponentState::default()
        };
        CalcGrid
            .lines(&props, &state, 100, false)
            .into_iter()
            .find(|line| line_text(line).contains("calculated"))
            .unwrap()
    };
    let quick = status(json!({"evaluateMs": 12.4}));
    assert!(quick
        .iter()
        .any(|span| span.text == "calculated in 12 ms" && span.role == Role::Muted));
    let slow = status(json!({"evaluateMs": 300.0}));
    assert!(slow
        .iter()
        .any(|span| span.text.ends_with("300 ms, slow") && span.role == Role::Warning));
    let allowed = status(json!({"evaluateMs": 300.0, "slowMs": 500}));
    assert!(allowed.iter().all(|span| span.role != Role::Warning), "{allowed:?}");
}

/// CG3.4: a sheet whose session sends frames joins its room, polls only as a fallback once joined, and on another's
/// edit reads the room's state again, and its cells only when the edit changed one held.
#[test]
fn a_pushed_sheet_reads_again_only_what_anothers_edit_changed() {
    let (props, mut state) = loaded(true);
    assert!(
        CalcGrid.topics(&props, &state).is_empty(),
        "a session that sends no frames polls"
    );
    state.pushed = true;
    let topics = CalcGrid.topics(&props, &state);
    let client = View::of(&props, &state).client;
    assert_eq!(topics[0].name, "calc:d1");
    assert_eq!(topics[0].join, json!({"calc": "join", "room": "d1", "client": client}));
    assert_eq!(topics[0].leave["calc"], "leave");
    assert_eq!(
        CalcGrid.poll_millis(&props, &state),
        Some(POLL_MILLIS),
        "until the server says joined"
    );
    let acts = CalcGrid.push(
        &props,
        &mut state,
        &json!({"type": "joined", "client": client, "peers": []}),
    );
    assert!(matches!(&acts[..], [Act::Frame(frame)] if frame["calc"] == "select"));
    assert_eq!(CalcGrid.poll_millis(&props, &state), Some(PUSHED_POLL_MILLIS));
    // a place refused before its join was in leaves the view joined; a join refused does not
    let refused = |code: i64| json!({"type": "error", "client": client, "code": code});
    CalcGrid.push(&props, &mut state, &refused(409));
    assert!(View::of(&props, &state).live);
    CalcGrid.push(&props, &mut state, &refused(403));
    assert_eq!(CalcGrid.poll_millis(&props, &state), Some(POLL_MILLIS));
    CalcGrid.push(
        &props,
        &mut state,
        &json!({"type": "joined", "client": client, "peers": []}),
    );
    // the view's own edit is not read again
    assert!(CalcGrid
        .push(
            &props,
            &mut state,
            &json!({"type": "changed", "seq": 3, "kind": "edit"})
        )
        .is_empty());
    // values below the rows held: the state is read, then the cells are kept
    let below = json!({"type": "changed", "seq": 4, "from": 3, "kind": "edit",
        "changed": [{"sheet": 0, "row": 500, "column": 1, "width": 1, "height": 1}]});
    assert_eq!(CalcGrid.push(&props, &mut state, &below), [Act::Reload]);
    assert_eq!(CalcGrid.source(&props, &state).unwrap().0, "calcState");
    let room = json!({"roomId": "d1", "shared": true, "seq": 4, "sheets": props["room"]["sheets"]});
    CalcGrid.receive(&props, &mut state, room.clone());
    assert!(!View::of(&props, &state).need_cells, "the cells held stay");
    // a value held changed: the cells are read again
    let held = json!({"type": "changed", "seq": 5, "from": 4, "kind": "edit",
        "changed": [{"sheet": 0, "row": 3, "column": 2, "width": 1, "height": 1}]});
    CalcGrid.push(&props, &mut state, &held);
    let mut room = room;
    room["seq"] = json!(5);
    CalcGrid.receive(&props, &mut state, room);
    assert!(View::of(&props, &state).need_cells);
    // someone came and went
    CalcGrid.push(
        &props,
        &mut state,
        &json!({"type": "peer", "client": "b1", "name": "Bea", "joined": true}),
    );
    assert!(texts(&props, &state, 100)
        .iter()
        .any(|line| line.contains("Bea has it open")));
    CalcGrid.push(
        &props,
        &mut state,
        &json!({"type": "peer", "client": "b1", "joined": false}),
    );
    assert!(View::of(&props, &state).peers.is_empty());
}

/// The menu's item `at` chosen.
fn menu(props: &Value, state: &mut ComponentState, at: usize) -> Vec<Act> {
    CalcGrid.key(props, state, &KeyPress::plain(Key::Char('m')), false);
    for _ in 0..at {
        CalcGrid.key(props, state, &KeyPress::plain(Key::Down), false);
    }
    CalcGrid.key(props, state, &KeyPress::plain(Key::Enter), false).unwrap()
}

/// The `charts` edit an answer posts: the document's whole list of charts.
fn charts_sent(acts: &[Act]) -> Value {
    let [Act::Call { url, parameters }] = acts else {
        panic!("{acts:?}")
    };
    assert_eq!(url, "calcEdit");
    let ops: Value = serde_json::from_str(&parameters["opsJson"]).unwrap();
    assert_eq!(ops[0]["op"], "charts", "{ops}");
    ops[0]["charts"].clone()
}

fn type_text(props: &Value, state: &mut ComponentState, text: &str) {
    for ch in text.chars() {
        CalcGrid.key(props, state, &KeyPress::plain(Key::Char(ch)), false);
    }
}

/// CG3.3: a chart is added from the selection as the browser's toolbar makes one (its first column the labels, a
/// series for each other column), its type and series changed and it removed, each sent as the `charts` edit of the
/// whole list; a reader is refused, and a selection of one row makes no chart.
#[test]
fn the_menu_adds_a_chart_of_the_selection_changes_its_type_and_removes_it() {
    let (props, mut state) = loaded(true);
    // each chart keeps the id the server gave it, so a dashboard panel of it still finds it; a new one has none yet
    let by_order = json!({"id": "c1", "title": "By order", "type": "bar", "categories": "Orders!A2:A3",
        "series": [{"name": "Total", "values": "Orders!B2:B3"}]});
    // one row selected is no chart
    assert!(menu(&props, &mut state, CHART_MENU).is_empty());
    assert!(View::of(&props, &state).message.contains("Select the chart's cells"));
    let shift = |key| KeyPress {
        shift: true,
        ..KeyPress::plain(key)
    };
    for key in [Key::Down, Key::Down, Key::Right, Key::Right] {
        CalcGrid.key(&props, &mut state, &shift(key), false);
    }
    assert!(
        menu(&props, &mut state, CHART_MENU).is_empty(),
        "the title is asked first"
    );
    assert!(texts(&props, &state, 80)[0].contains("Chart title:"));
    type_text(&props, &mut state, "Totals");
    let acts = CalcGrid
        .key(&props, &mut state, &KeyPress::plain(Key::Enter), false)
        .unwrap();
    assert_eq!(
        charts_sent(&acts),
        json!([by_order, {"title": "Totals", "type": "column", "categories": "Orders!A1:A3", "series": [
            {"name": "Series 1", "values": "Orders!B1:B3"}, {"name": "Series 2", "values": "Orders!C1:C3"}]}])
    );
    assert_eq!(View::of(&props, &state).chart, 1, "the new chart is the one chosen");
    // the room still has the one chart, which the type, the series and the removal act on
    state.local["chart"] = json!(0);
    let mut columns = by_order.clone();
    columns["type"] = json!("column");
    assert_eq!(charts_sent(&menu(&props, &mut state, CHART_MENU + 4)), json!([columns]));
    assert!(menu(&props, &mut state, CHART_MENU + 3).is_empty());
    assert_eq!(View::of(&props, &state).editing.unwrap().text, "Orders!B2:B3");
    type_text(&props, &mut state, ", Orders!C2:C3");
    let acts = CalcGrid
        .key(&props, &mut state, &KeyPress::plain(Key::Enter), false)
        .unwrap();
    assert_eq!(
        charts_sent(&acts)[0]["series"],
        json!([{"name": "Total", "values": "Orders!B2:B3"}, {"name": "Series 2", "values": "Orders!C2:C3"}])
    );
    assert_eq!(charts_sent(&menu(&props, &mut state, MERGE_MENU - 1)), json!([]));
    assert_eq!(range_name("Q 4", 1, 1, 2, 1), "'Q 4'!A1:A2");
    assert_eq!(range_name("", 1, 1, 1, 1), "A1", "as calc.rangeName with no sheet");
    // a reader is refused
    let (props, mut state) = loaded(false);
    assert!(menu(&props, &mut state, CHART_MENU + 4).is_empty());
    assert!(View::of(&props, &state).message.contains("read only"));
}

/// A sheet of the author's, columns of 10 characters, with the merged cells the layout gives.
fn merged_props(frozen_rows: i32, frozen_columns: i32, merges: Value) -> Value {
    json!({"editable": true, "room": {"roomId": "m1", "shared": false, "seq": 1, "functions": [],
        "sheets": [{"index": 0, "name": "Plan", "rows": 40, "columns": 6, "widths": [80, 80, 80, 80, 80, 80],
            "frozenRows": frozen_rows, "frozenColumns": frozen_columns, "data": false, "merges": merges}]}})
}

/// What the server answers the view's reads with, from the sheet's texts, until it needs nothing more: a block holds
/// the texts in it and the merged cells, as `get#CalcCells` does. The calcCells reads asked.
fn serve(props: &Value, state: &mut ComponentState, texts: &[(i32, i32, &str)]) -> Vec<BTreeMap<String, String>> {
    let mut asked = Vec::new();
    for _ in 0..10 {
        if !View::of(props, state).needs() {
            break;
        }
        let (url, parameters) = CalcGrid.source(props, state).unwrap();
        let at = |key: &str| parameters[key].parse::<i32>().unwrap();
        let answer = if url == "calcContent" {
            let text = texts
                .iter()
                .find(|(row, column, _)| (*row, *column) == (at("row"), at("column")))
                .map_or("", |text| text.2);
            json!({"content": text})
        } else {
            let (row, height, column, width) = (at("row"), at("height"), at("column"), at("width"));
            let inside = |r: i32, c: i32| (row..row + height).contains(&r) && (column..column + width).contains(&c);
            let texts: Vec<Value> = texts
                .iter()
                .filter(|(r, c, _)| inside(*r, *c))
                .map(|(r, c, text)| json!({"row": r, "column": c, "text": text, "cell_type": 2}))
                .collect();
            asked.push(parameters.clone());
            json!({"row": row, "column": column, "width": width, "height": height, "seq": 1, "styles": [],
                "style_indices": [], "texts": texts, "merges": props["room"]["sheets"][0]["merges"]})
        };
        CalcGrid.receive(props, state, answer);
    }
    state.reload_pending = false;
    asked
}

/// The texts of the sheet with B2:C3 merged: a stale text left in C2, and a long one in the box B6:C6.
const MERGED_TEXTS: &[(i32, i32, &str)] = &[
    (1, 1, "top"),
    (2, 1, "a"),
    (2, 2, "Quarter totals"),
    (2, 3, "stale"),
    (2, 4, "d"),
    (4, 2, "below"),
    (6, 2, "A long title that runs past its box"),
];

fn merged_sheet() -> (Value, ComponentState) {
    let props = merged_props(
        0,
        0,
        json!([{"row": 2, "column": 2, "width": 2, "height": 2}, {"row": 6, "column": 2, "width": 2, "height": 1}]),
    );
    let mut state = ComponentState {
        rows: 30,
        ..ComponentState::default()
    };
    serve(&props, &mut state, MERGED_TEXTS);
    (props, state)
}

fn press(props: &Value, state: &mut ComponentState, key: Key, shift: bool) {
    let press = KeyPress {
        shift,
        ..KeyPress::plain(key)
    };
    CalcGrid.key(props, state, &press, false);
    serve(props, state, MERGED_TEXTS);
}

fn at(props: &Value, state: &ComponentState) -> (i32, i32) {
    let view = View::of(props, state);
    (view.row, view.column)
}

/// CG3.2: a merged cell is one box: its anchor's text across its columns on its first row, no gap inside it, its other
/// cells blank (a text left in one is not drawn), a long text kept inside it; the cursor on it is the whole box.
#[test]
fn a_merged_cell_is_one_box_with_its_anchors_text_and_no_line_inside() {
    let (props, mut state) = merged_sheet();
    let grid = CalcGrid.lines(&props, &state, 80, false);
    // the bar, the column letters, then rows 1 to 6; a row is the gutter then a cell and a gap for each column
    let (row1, row2, row3, row4, row6) = (&grid[2], &grid[3], &grid[4], &grid[5], &grid[7]);
    assert_eq!(row2.len(), row1.len() - 2, "B2 and C2 are one span and one gap");
    assert_eq!(row3.len(), row1.len() - 2);
    assert_eq!(row4.len(), row1.len(), "below the box each cell is its own");
    // A is the gutter's next span, the box the one after A's gap: B and C's widths and the gap between them
    assert_eq!(row2[3].text, format!("{:<21}", "Quarter totals"));
    assert_eq!(row3[3].text, " ".repeat(21));
    assert!(line_text(row2).contains(" d "), "D2 after the box: {}", line_text(row2));
    assert!(!line_text(row2).contains("stale"));
    assert!(line_text(row4).contains("below"));
    assert_eq!(row6[3].text, "A long title that ru…");
    assert!(!line_text(row6).contains("s past"), "a box's text does not run on");
    // the cursor on the box is all of it, on each of its rows
    press(&props, &mut state, Key::Down, false);
    press(&props, &mut state, Key::Right, false);
    let grid = CalcGrid.lines(&props, &state, 80, false);
    assert!(grid[3][3].reverse && grid[4][3].reverse, "{:?}", grid[4][3]);
    assert!(
        texts(&props, &state, 80)[0].starts_with("B2:C3"),
        "the box is the selection"
    );
}

/// CG3.2: moving onto a box lands on its anchor from any side, moving on from it steps past its far side, and Enter
/// after typing into it goes below it; a covered cell (a merge came in under the cursor) is typed into at its anchor.
#[test]
fn the_cursor_stands_on_a_boxs_anchor_and_steps_past_it() {
    let (props, mut state) = merged_sheet();
    press(&props, &mut state, Key::Down, false);
    press(&props, &mut state, Key::Right, false);
    assert_eq!(at(&props, &state), (2, 2));
    press(&props, &mut state, Key::Right, false);
    assert_eq!(at(&props, &state), (2, 4), "past the box's columns");
    press(&props, &mut state, Key::Left, false);
    assert_eq!(at(&props, &state), (2, 2), "back onto its anchor");
    press(&props, &mut state, Key::Down, false);
    assert_eq!(at(&props, &state), (4, 2), "past the box's rows");
    press(&props, &mut state, Key::Up, false);
    assert_eq!(at(&props, &state), (2, 2));
    // from above its covered column
    press(&props, &mut state, Key::Up, false);
    press(&props, &mut state, Key::Right, false);
    assert_eq!(at(&props, &state), (1, 3));
    press(&props, &mut state, Key::Down, false);
    assert_eq!(at(&props, &state), (2, 2));
    // the box's content is read at its anchor, and typed into there
    assert_eq!(View::of(&props, &state).current_content(), "Quarter totals");
    state.local["row"] = json!(3);
    state.local["column"] = json!(3);
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Char('e')), false);
    assert_eq!(at(&props, &state), (2, 2));
    serve(&props, &mut state, MERGED_TEXTS);
    type_text(&props, &mut state, "!");
    let acts = CalcGrid
        .key(&props, &mut state, &KeyPress::plain(Key::Enter), false)
        .unwrap();
    let [Act::Call { parameters, .. }] = acts.as_slice() else {
        panic!("{acts:?}")
    };
    let ops: Value = serde_json::from_str(&parameters["opsJson"]).unwrap();
    assert_eq!(
        ops,
        json!([{"op": "input", "sheet": 0, "row": 2, "column": 2, "value": "Quarter totals!"}])
    );
    assert_eq!(at(&props, &state), (4, 2), "Enter goes below the box");
}

/// CG3.2: a selection that meets a box takes all of it, as the browser's `calc.expandToMerges`, and the others are
/// told that place.
#[test]
fn a_selection_that_meets_a_box_takes_all_of_it() {
    let (props, mut state) = merged_sheet();
    press(&props, &mut state, Key::Down, true);
    press(&props, &mut state, Key::Right, true);
    let view = View::of(&props, &state);
    assert_eq!(((view.row, view.column), view.address()), ((2, 2), "A1:C3".to_string()));
    let (_, read) = CalcGrid.source(&props, &state).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&read["selectionJson"]).unwrap(),
        json!({"row": 1, "column": 1, "width": 3, "height": 3})
    );
    let grid = CalcGrid.lines(&props, &state, 80, false);
    assert_eq!(grid[4][1].role, Role::Selection, "A3, in the rows the box took in");
    assert!(grid[4][3].reverse, "the box is the active cell");
    // from below and right of it
    state.local["row"] = json!(4);
    state.local["column"] = json!(4);
    state.local["anchor"] = Value::Null;
    press(&props, &mut state, Key::Up, true);
    assert_eq!(View::of(&props, &state).address(), "D3:D4");
    press(&props, &mut state, Key::Left, true);
    assert_eq!(View::of(&props, &state).address(), "B2:D4");
}

/// CG3.2: an author merges and unmerges the selection from the menu, sent as the browser's `merge` and `unmerge`
/// edits; one cell is no merge; a data sheet and a reader are not offered them.
#[test]
fn the_menu_merges_and_unmerges_the_selection_but_not_on_a_data_sheet() {
    let (props, mut state) = merged_sheet();
    let sent = |acts: &[Act]| {
        let [Act::Call { url, parameters }] = acts else {
            panic!("{acts:?}")
        };
        assert_eq!(url, "calcEdit");
        serde_json::from_str::<Value>(&parameters["opsJson"]).unwrap()
    };
    state.local["row"] = json!(5);
    state.local["column"] = json!(4);
    state.local["anchor"] = json!([6, 5]);
    assert_eq!(
        sent(&menu(&props, &mut state, MERGE_MENU)),
        json!([{"op": "merge", "sheet": 0, "row": 5, "column": 4, "width": 2, "height": 2}])
    );
    // on a covered cell, the box is the selection
    state.local["row"] = json!(3);
    state.local["column"] = json!(3);
    state.local["anchor"] = Value::Null;
    assert_eq!(
        sent(&menu(&props, &mut state, MERGE_MENU + 1)),
        json!([{"op": "unmerge", "sheet": 0, "row": 2, "column": 2, "width": 2, "height": 2}])
    );
    state.local["row"] = json!(8);
    assert!(menu(&props, &mut state, MERGE_MENU).is_empty());
    assert!(View::of(&props, &state).message.contains("Select the cells to merge"));
    assert!(menu(&props, &mut state, MERGE_MENU + 1).is_empty());
    assert!(View::of(&props, &state).message.contains("No merged cells"));
    // the menu's whole round on a data sheet, and a reader's on a sheet of its own, has no merge
    let never_merge = |props: &Value, state: &mut ComponentState| {
        CalcGrid.key(props, state, &KeyPress::plain(Key::Char('m')), false);
        for _ in 0..MENU.len() {
            assert!(texts(props, state, 80)
                .iter()
                .all(|line| !line.contains("erge the cells")));
            CalcGrid.key(props, state, &KeyPress::plain(Key::Down), false);
        }
        CalcGrid.key(props, state, &KeyPress::plain(Key::Escape), false);
    };
    let (props, mut state) = loaded(true);
    never_merge(&props, &mut state);
    let (props, mut state) = loaded(false);
    CalcGrid.key(&props, &mut state, &KeyPress::plain(Key::Char(']')), false);
    never_merge(&props, &mut state);
}

/// CG3.2: a box across the frozen rows and columns is cut at their edge: its text where its first part is on screen,
/// the part past the edge blank or scrolled away; a box whose anchor scrolled above the screen is read from its anchor
/// and drawn with its text on its first row on screen.
#[test]
fn a_box_across_a_frozen_edge_is_cut_there() {
    let props = merged_props(
        1,
        1,
        json!([{"row": 1, "column": 1, "width": 3, "height": 3}, {"row": 2, "column": 4, "width": 2, "height": 24}]),
    );
    let texts_here: &[(i32, i32, &str)] = &[(1, 1, "Header of the plan"), (2, 4, "Tall box"), (12, 6, "f")];
    let mut state = ComponentState {
        rows: 30,
        ..ComponentState::default()
    };
    serve(&props, &mut state, texts_here);
    // unscrolled: one box over A1:C3, its text on the frozen row, its lower rows blank
    let grid = CalcGrid.lines(&props, &state, 80, false);
    assert_eq!(grid[2][1].text, format!("{:<32}", "Header of the plan"));
    assert_eq!(grid[3][1].text, " ".repeat(32), "the box below the frozen row");
    // scrolled right and down: the box's frozen part alone, and the tall box's text on its first row on screen
    for (key, value) in [("top", 10), ("left", 5), ("row", 12), ("column", 6)] {
        state.local[key] = json!(value);
    }
    state.local["need_cells"] = json!(true);
    state.local["need_frozen"] = json!(true);
    let asked = serve(&props, &mut state, texts_here);
    assert!(
        asked.iter().any(|read| read["row"] == "2"),
        "the tall box's anchor is read: {asked:?}"
    );
    let lines = texts(&props, &state, 80);
    let row = |number: i32| {
        lines
            .iter()
            .find(|line| line.trim_start().starts_with(&format!("{number} ")))
            .unwrap()
            .clone()
    };
    assert!(row(1).contains("Header of"), "{}", row(1));
    assert!(!row(1).contains("the plan"), "cut at column A: {}", row(1));
    assert!(row(10).contains("Tall box"), "{lines:?}");
    assert!(!row(11).contains("Tall box"));
    assert!(row(12).contains('f'));
}
