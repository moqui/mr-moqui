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

//! `calcgrid`: a CalcGrid document's sheet (mr-moqui plans/IronCalc-DB.md), the terminal twin of the browser's
//! `moqui.calc.sheet`. The workbook is in the server; this draws the cells that fit on screen and sends edits, over the
//! same transitions of the screen the browser calls.
//!
//! Props: `room`, what `open#CalcDocument` answered when the screen was drawn (the screen's `tui` text opens it, as the
//! viewer); `editable`; `profileId`, the access profile each call is made under. The cells come from `calcCells` (the
//! rows and columns on screen), a cell's content as typed from `calcContent`, the room's state from `calcState`, and an
//! edit is posted to `calcEdit` ([`Act::Call`]); `calcRefresh`, `calcSave`, `calcRevert` and `calcExport` (as JSON)
//! are the menu's. A saved document's room is read again every second and a half for the others' edits and places:
//! the reads ask `since` the view's last edit, and a room that did not change answers only who is where.
//!
//! The data sheets are read only. Colours of the workbook are not drawn (the theme's roles are); bold and underline
//! are, numbers stand right, and a text runs on into empty cells beside it.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Act, Component, ComponentState, Line, Span};
use crate::keys::{Key, KeyPress};
use crate::layout::{display_width, truncate_width};
use crate::theme::Role;

pub struct CalcGrid;

/// The terminal rows that are not the sheet's own rows: the app's frame, the screen's title, and the sheet's bar,
/// column letters, sheet tabs and status line.
const CHROME_ROWS: i32 = 12;
/// The same in kiosk mode, where the sheet has the terminal.
const KIOSK_CHROME_ROWS: i32 = 5;
const MIN_ROWS: i32 = 5;
/// The columns asked for from the first one shown.
const COLUMNS_ASKED: i32 = 40;
/// The most cells a read asks for (the server answers 20,000).
const MAX_CELLS: i32 = 20_000;
const LAST_ROW: i32 = 1_048_576;
const LAST_COLUMN: i32 = 16_384;
const POLL_MILLIS: u64 = 1_500;
/// A column's width in characters is its width in pixels over this, as a browser's average character.
const PIXELS_PER_CHARACTER: f64 = 8.0;
const ROOM_GONE: &str = "ROOM_GONE";

/// Text being typed: into a cell (`cell`), or a sheet's new name (`rename`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Editing {
    text: String,
    /// In characters.
    cursor: usize,
    target: String,
    /// The completion chosen.
    choice: usize,
}

/// What the component keeps in its state's `local`.
/// Whether an edit's answer says that the edit changed none of the cells the view holds, which then stay: it changed
/// values only (the answer has `changed`), it is the view's own edit alone (the room did not follow another's edit
/// first), and no area it changed is on the rows of the view's block or its frozen rows, on the sheet shown.
fn changed_outside_block(view: &View, answer: &Value) -> bool {
    let Some(changed) = answer["changed"].as_array() else {
        return false;
    };
    if answer["shared"] == true && answer["seq"].as_i64() != Some(view.seq + 1) {
        return false;
    }
    let number = |value: &Value| value.as_i64().unwrap_or(0);
    let (top, height) = (number(&view.block["row"]), number(&view.block["height"]));
    if height <= 0 {
        return false;
    }
    let frozen_rows = i64::from(view.frozen_counts().0);
    changed.iter().all(|area| {
        let first = number(&area["row"]);
        let last = first + number(&area["height"]) - 1;
        area["sheet"].as_i64() != Some(i64::from(view.sheet))
            || ((last < top || first >= top + height) && first > frozen_rows)
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct View {
    /// The room as the server last answered it: its sheets, charts, edit, undo state and functions.
    room: Value,
    sheet: u32,
    /// The active cell.
    row: i32,
    column: i32,
    /// The other corner of a selection.
    anchor: Option<(i32, i32)>,
    /// The first row and column shown after the frozen ones.
    top: i32,
    left: i32,
    /// The room's last edit the view has.
    seq: i64,
    /// The cells read for the rows and columns on screen, and for the frozen rows.
    block: Value,
    frozen: Value,
    /// The active cell's content as typed, and the cell it is of (`sheet:row:column`).
    content: String,
    content_at: String,
    editing: Option<Editing>,
    menu: Option<usize>,
    charts: bool,
    kiosk: bool,
    /// The others who have the document open: `name`, `sheet`, `area`.
    peers: Vec<Value>,
    client: String,
    need_state: bool,
    need_cells: bool,
    need_frozen: bool,
    need_content: bool,
    message: String,
    /// The room is no longer on the server: Enter draws the screen again, which opens it.
    gone: bool,
}

/// One cell as drawn.
#[derive(Clone, Debug, Default, PartialEq)]
struct Cell {
    text: String,
    number: bool,
    bold: bool,
    italic: bool,
    underline: bool,
    /// `left`, `center` or `right` when the cell's style says so.
    align: String,
}

const MENU: &[&str] = &[
    "Insert a row above",
    "Delete the rows selected",
    "Insert a column to the left",
    "Delete the columns selected",
    "Add a sheet",
    "Rename the sheet",
    "Delete the sheet",
    "Unfreeze",
    "Number format: 1,234.50",
    "Number format: 12.5%",
    "Number format: 2026-11-13",
    "Number format: general",
    "Read the data again",
    "Save the document",
    "Discard the unsaved edits",
    "Export as XLSX",
];

/// 1 is A, 27 is AA.
fn column_name(number: i32) -> String {
    let mut name = String::new();
    let mut n = number;
    while n > 0 {
        let at = u8::try_from((n - 1) % 26).unwrap_or(0);
        name.insert(0, char::from(b'A' + at));
        n = (n - 1) / 26;
    }
    name
}

/// The table a data sheet gives formulas, as the server names it: the sheet's name as an identifier.
fn table_name(sheet_name: &str) -> String {
    let name: String = sheet_name
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { '_' })
        .collect();
    if name.chars().next().is_some_and(char::is_alphabetic) {
        name
    } else {
        format!("T{name}")
    }
}

/// What a formula being typed completes to at `cursor`: the function names and the data sheets' tables that begin with
/// the letters before it, and where those letters start. Empty when the text is not a formula, or a cell's name is
/// being typed.
fn completions(text: &str, cursor: usize, functions: &[String], tables: &[String]) -> (usize, Vec<String>) {
    if !text.starts_with('=') {
        return (0, Vec::new());
    }
    let before: Vec<char> = text.chars().take(cursor).collect();
    let mut from = before.len();
    while from > 0 && (before[from - 1].is_alphanumeric() || before[from - 1] == '_' || before[from - 1] == '.') {
        from -= 1;
    }
    let prefix: String = before[from..].iter().collect::<String>().to_uppercase();
    let starts_alpha = prefix.chars().next().is_some_and(char::is_alphabetic);
    let cell = {
        let letters = prefix.chars().take_while(char::is_ascii_uppercase).count();
        (1..=3).contains(&letters)
            && prefix.len() > letters
            && prefix.chars().skip(letters).all(|ch| ch.is_ascii_digit())
    };
    if prefix.is_empty() || !starts_alpha || cell {
        return (from, Vec::new());
    }
    let names: Vec<String> = functions
        .iter()
        .filter(|name| name.starts_with(&prefix))
        .map(|name| format!("{name}("))
        .chain(
            tables
                .iter()
                .filter(|name| name.to_uppercase().starts_with(&prefix))
                .map(|name| format!("{name}[")),
        )
        .take(8)
        .collect();
    (from, names)
}

/// The cells of a block the server answered, by row and column.
fn cells(block: &Value) -> HashMap<(i32, i32), Cell> {
    let number = |value: &Value| value.as_i64().and_then(|n| i32::try_from(n).ok()).unwrap_or(0);
    let (row, column, width) = (number(&block["row"]), number(&block["column"]), number(&block["width"]));
    let styles = block["styles"].as_array().map(Vec::as_slice).unwrap_or_default();
    let indices = block["style_indices"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut out = HashMap::new();
    for (at, index) in indices.iter().enumerate() {
        let Some(style) = index
            .as_u64()
            .and_then(|index| styles.get(usize::try_from(index).ok()?))
        else {
            continue;
        };
        let style = &style["style"];
        let font = &style["font"];
        let (bold, italic, underline) = (font["b"] == true, font["i"] == true, font["u"] == true);
        let align = style["alignment"]["horizontal"].as_str().unwrap_or("").to_string();
        if !(bold || italic || underline || matches!(align.as_str(), "left" | "center" | "right")) || width < 1 {
            continue;
        }
        let at = i32::try_from(at).unwrap_or(i32::MAX);
        out.insert(
            (row + at / width, column + at % width),
            Cell {
                bold,
                italic,
                underline,
                align,
                ..Cell::default()
            },
        );
    }
    for text in block["texts"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let cell = out.entry((number(&text["row"]), number(&text["column"]))).or_default();
        cell.text = text["text"].as_str().unwrap_or("").to_string();
        cell.number = text["cell_type"] == 1;
    }
    out
}

fn int(value: &Value) -> i32 {
    value.as_i64().and_then(|n| i32::try_from(n).ok()).unwrap_or(0)
}

impl View {
    /// The component's state, or a new one from the room the props carry.
    fn of(props: &Value, state: &ComponentState) -> Self {
        if let Ok(view) = serde_json::from_value::<Self>(state.local.clone()) {
            if view.room.is_object() || view.gone {
                return view;
            }
        }
        let room = props["room"].clone();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let mut view = Self {
            seq: room["seq"].as_i64().unwrap_or(0),
            room,
            row: 1,
            column: 1,
            top: 1,
            left: 1,
            client: format!("tui-{nanos:x}"),
            need_cells: true,
            need_content: true,
            ..Self::default()
        };
        view.sheet = view.first_sheet();
        view.need_frozen = view.frozen_counts().0 > 0;
        view
    }

    fn store(&self, state: &mut ComponentState) {
        state.local = serde_json::to_value(self).unwrap_or(Value::Null);
        state.reload_pending |= self.room.is_object() && self.needs();
    }

    fn needs(&self) -> bool {
        self.need_state || self.need_cells || self.need_frozen || self.need_content
    }

    fn room_id(&self) -> Option<&str> {
        self.room["roomId"].as_str().filter(|_| !self.gone)
    }

    fn sheets(&self) -> Vec<&Value> {
        self.room["sheets"]
            .as_array()
            .map(|sheets| sheets.iter().filter(|sheet| sheet["hidden"] != true).collect())
            .unwrap_or_default()
    }

    fn first_sheet(&self) -> u32 {
        self.sheets()
            .first()
            .and_then(|sheet| sheet["index"].as_u64())
            .and_then(|index| u32::try_from(index).ok())
            .unwrap_or(0)
    }

    fn layout(&self) -> Value {
        self.sheets()
            .into_iter()
            .find(|sheet| sheet["index"].as_u64() == Some(u64::from(self.sheet)))
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn is_data(&self) -> bool {
        self.layout()["data"] == true
    }

    fn frozen_counts(&self) -> (i32, i32) {
        let layout = self.layout();
        (int(&layout["frozenRows"]).max(0), int(&layout["frozenColumns"]).max(0))
    }

    /// A column's width in characters.
    fn width_of(&self, column: i32) -> usize {
        let pixels = usize::try_from(column - 1)
            .ok()
            .and_then(|at| self.layout()["widths"].get(at).and_then(Value::as_f64))
            .unwrap_or(90.0);
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let chars = (pixels / PIXELS_PER_CHARACTER).round() as usize;
        chars.clamp(3, 40)
    }

    /// The sheet rows that fit on screen below the frozen ones.
    fn rows_shown(&self, terminal_rows: u16) -> i32 {
        let chrome = if self.kiosk { KIOSK_CHROME_ROWS } else { CHROME_ROWS };
        let rows = if terminal_rows == 0 {
            30
        } else {
            i32::from(terminal_rows)
        };
        let editing = i32::from(self.editing.is_some());
        (rows - chrome - editing - self.frozen_counts().0).max(MIN_ROWS)
    }

    /// The rectangle of the rows and columns on screen: `(row, height, column, width)`.
    fn viewport(&self, terminal_rows: u16) -> (i32, i32, i32, i32) {
        let (frozen_rows, frozen_columns) = self.frozen_counts();
        let row = self.top.max(frozen_rows + 1);
        let height = self.rows_shown(terminal_rows);
        let (column, width) = if frozen_columns > 0 && (self.left + COLUMNS_ASKED) * height <= MAX_CELLS {
            (1, self.left + COLUMNS_ASKED - 1)
        } else {
            (self.left, COLUMNS_ASKED)
        };
        (row, height, column, width.min(MAX_CELLS / height.max(1)))
    }

    /// Whether the block read covers the rows and columns on screen.
    fn covers(&self, terminal_rows: u16) -> bool {
        let (row, height, column, width) = self.viewport(terminal_rows);
        let block = &self.block;
        block.is_object()
            && block["sheet_index"].as_u64() == Some(u64::from(self.sheet))
            && int(&block["row"]) <= row
            && int(&block["row"]) + int(&block["height"]) >= row + height
            && int(&block["column"]) <= column
            && int(&block["column"]) + int(&block["width"]) >= column + width
    }

    /// The selection: its top left and bottom right.
    fn area(&self) -> (i32, i32, i32, i32) {
        let (row, column) = self.anchor.unwrap_or((self.row, self.column));
        (
            row.min(self.row),
            column.min(self.column),
            row.max(self.row),
            column.max(self.column),
        )
    }

    fn area_op(&self, op: &str) -> Value {
        let (row, column, last_row, last_column) = self.area();
        json!({"op": op, "sheet": self.sheet, "row": row, "column": column,
            "width": last_column - column + 1, "height": last_row - row + 1})
    }

    fn address(&self) -> String {
        let (row, column, last_row, last_column) = self.area();
        let first = format!("{}{row}", column_name(column));
        if (row, column) == (last_row, last_column) {
            first
        } else {
            format!("{first}:{}{last_row}", column_name(last_column))
        }
    }

    fn content_key(&self) -> String {
        format!("{}:{}:{}", self.sheet, self.row, self.column)
    }

    /// The active cell as drawn, from the blocks read.
    fn cell(&self, row: i32, column: i32) -> Cell {
        let block = if row <= self.frozen_counts().0 && self.frozen.is_object() {
            &self.frozen
        } else {
            &self.block
        };
        cells(block).remove(&(row, column)).unwrap_or_default()
    }

    /// The active cell's content as typed when it was read, else its text as drawn.
    fn current_content(&self) -> String {
        if self.content_at == self.content_key() {
            self.content.clone()
        } else {
            self.cell(self.row, self.column).text
        }
    }

    fn functions(&self) -> Vec<String> {
        self.room["functions"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn tables(&self) -> Vec<String> {
        self.sheets()
            .iter()
            .filter(|sheet| sheet["data"] == true)
            .filter_map(|sheet| sheet["name"].as_str().map(table_name))
            .collect()
    }

    /// The parameters every call carries: the room and the access profile.
    fn parameters(&self, props: &Value) -> BTreeMap<String, String> {
        let mut parameters = BTreeMap::new();
        if let Some(id) = self.room_id() {
            parameters.insert("roomId".to_string(), id.to_string());
        }
        if let Some(profile) = props["profileId"].as_str().filter(|profile| !profile.is_empty()) {
            parameters.insert("profileId".to_string(), profile.to_string());
        }
        parameters
    }

    /// Move the active cell, and with `extend` keep the selection's other corner; the rows and columns shown follow.
    fn move_to(&mut self, row: i32, column: i32, extend: bool, terminal_rows: u16) {
        if extend {
            self.anchor.get_or_insert((self.row, self.column));
        } else {
            self.anchor = None;
        }
        self.row = row.clamp(1, LAST_ROW);
        self.column = column.clamp(1, LAST_COLUMN);
        let (frozen_rows, frozen_columns) = self.frozen_counts();
        let shown = self.rows_shown(terminal_rows);
        if self.row > frozen_rows {
            if self.row < self.top {
                self.top = self.row;
            } else if self.row >= self.top.max(frozen_rows + 1) + shown {
                self.top = self.row - shown + 1;
            }
        }
        if self.column > frozen_columns {
            if self.column < self.left {
                self.left = self.column;
            } else if self.column >= self.left + COLUMNS_ASKED - 1 {
                self.left = (self.column - COLUMNS_ASKED / 2).max(1);
            }
        }
        self.top = self.top.max(1);
        self.left = self.left.max(1);
        self.need_content = true;
        self.need_cells |= !self.covers(terminal_rows);
    }

    fn show_sheet(&mut self, index: u32) {
        self.sheet = index;
        self.row = 1;
        self.column = 1;
        self.anchor = None;
        self.top = 1;
        self.left = 1;
        self.block = Value::Null;
        self.frozen = Value::Null;
        self.need_cells = true;
        self.need_frozen = self.frozen_counts().0 > 0;
        self.need_content = true;
    }
}

/// An edit of the room, posted; the answer is the room's state.
fn edit(view: &mut View, props: &Value, ops: Vec<Value>) -> Vec<Act> {
    if props["editable"] != true {
        view.message = "This document is read only for you".to_string();
        return Vec::new();
    }
    let mut parameters = view.parameters(props);
    parameters.insert("opsJson".to_string(), Value::Array(ops).to_string());
    vec![Act::Call {
        url: "calcEdit".to_string(),
        parameters,
    }]
}

fn call(view: &View, props: &Value, url: &str, extra: &[(&str, String)]) -> Vec<Act> {
    let mut parameters = view.parameters(props);
    for (key, value) in extra {
        parameters.insert((*key).to_string(), value.clone());
    }
    vec![Act::Call {
        url: url.to_string(),
        parameters,
    }]
}

/// A key while text is typed: into the cell, or a sheet's name.
fn editing_key(view: &mut View, props: &Value, press: &KeyPress, terminal_rows: u16) -> Vec<Act> {
    let Some(mut editing) = view.editing.clone() else {
        return Vec::new();
    };
    let (from, names) = if editing.target == "cell" {
        completions(&editing.text, editing.cursor, &view.functions(), &view.tables())
    } else {
        (0, Vec::new())
    };
    let mut chars: Vec<char> = editing.text.chars().collect();
    let mut commit = None;
    match press.key {
        Key::Escape => {
            view.editing = None;
            return Vec::new();
        }
        Key::Char(ch) if !press.ctrl && !press.alt => {
            chars.insert(editing.cursor.min(chars.len()), ch);
            editing.cursor += 1;
            editing.choice = 0;
        }
        Key::Backspace if editing.cursor > 0 => {
            chars.remove(editing.cursor - 1);
            editing.cursor -= 1;
        }
        Key::Delete if editing.cursor < chars.len() => {
            chars.remove(editing.cursor);
        }
        Key::Left => editing.cursor = editing.cursor.saturating_sub(1),
        Key::Right => editing.cursor = (editing.cursor + 1).min(chars.len()),
        Key::Home => editing.cursor = 0,
        Key::End => editing.cursor = chars.len(),
        Key::Up | Key::Down if !names.is_empty() => {
            let count = names.len();
            editing.choice = if press.key == Key::Down {
                (editing.choice + 1) % count
            } else {
                (editing.choice + count - 1) % count
            };
        }
        Key::Tab if !names.is_empty() => {
            let name: Vec<char> = names[editing.choice.min(names.len() - 1)].chars().collect();
            let rest: Vec<char> = chars[editing.cursor.min(chars.len())..].to_vec();
            chars.truncate(from);
            chars.extend(&name);
            editing.cursor = chars.len();
            chars.extend(rest);
            editing.choice = 0;
        }
        Key::Enter => commit = Some((1, 0)),
        Key::Tab => commit = Some((0, 1)),
        Key::BackTab => commit = Some((0, -1)),
        Key::Up => commit = Some((-1, 0)),
        Key::Down => commit = Some((1, 0)),
        _ => {}
    }
    editing.text = chars.into_iter().collect();
    let Some((down, across)) = commit else {
        view.editing = Some(editing);
        return Vec::new();
    };
    view.editing = None;
    if editing.target == "rename" {
        let name = editing.text.trim().to_string();
        if name.is_empty() {
            return Vec::new();
        }
        return edit(
            view,
            props,
            vec![json!({"op": "renameSheet", "sheet": view.sheet, "name": name})],
        );
    }
    let (row, column) = (view.row, view.column);
    let changed = editing.text != view.current_content();
    view.content = editing.text.clone();
    view.content_at = view.content_key();
    view.move_to(row + down, column + across, false, terminal_rows);
    if !changed {
        return vec![Act::Reload];
    }
    edit(
        view,
        props,
        vec![json!({"op": "input", "sheet": view.sheet, "row": row, "column": column, "value": editing.text})],
    )
}

/// A choice of the menu.
fn menu_choice(view: &mut View, props: &Value, choice: usize) -> Vec<Act> {
    let (row, column, last_row, last_column) = view.area();
    let sheet = view.sheet;
    let format = |value: &str| {
        json!({"op": "style", "sheet": sheet, "row": row, "column": column,
        "width": last_column - column + 1, "height": last_row - row + 1, "path": "num_fmt", "value": value})
    };
    match choice {
        0 => edit(
            view,
            props,
            vec![json!({"op": "insertRows", "sheet": sheet, "row": row, "count": 1})],
        ),
        1 => edit(
            view,
            props,
            vec![json!({"op": "deleteRows", "sheet": sheet, "row": row, "count": last_row - row + 1})],
        ),
        2 => edit(
            view,
            props,
            vec![json!({"op": "insertColumns", "sheet": sheet, "column": column, "count": 1})],
        ),
        3 => edit(
            view,
            props,
            vec![json!({"op": "deleteColumns", "sheet": sheet, "column": column, "count": last_column - column + 1})],
        ),
        4 => edit(view, props, vec![json!({"op": "addSheet"})]),
        5 => {
            let name = view.layout()["name"].as_str().unwrap_or("").to_string();
            let cursor = name.chars().count();
            view.editing = Some(Editing {
                text: name,
                cursor,
                target: "rename".to_string(),
                choice: 0,
            });
            Vec::new()
        }
        6 => edit(view, props, vec![json!({"op": "deleteSheet", "sheet": sheet})]),
        7 => edit(
            view,
            props,
            vec![json!({"op": "freeze", "sheet": sheet, "rows": 0, "columns": 0})],
        ),
        8 => edit(view, props, vec![format("#,##0.00")]),
        9 => edit(view, props, vec![format("0.0%")]),
        10 => edit(view, props, vec![format("yyyy-mm-dd")]),
        11 => edit(view, props, vec![format("general")]),
        12 => call(view, props, "calcRefresh", &[]),
        13 => {
            let title = view.room["title"].as_str().unwrap_or("").to_string();
            call(view, props, "calcSave", &[("title", title)])
        }
        14 => call(view, props, "calcRevert", &[]),
        _ => {
            let mut url = format!("calcExport?json=true&roomId={}", view.room_id().unwrap_or(""));
            if let Some(profile) = props["profileId"].as_str().filter(|profile| !profile.is_empty()) {
                url.push_str("&profileId=");
                url.push_str(profile);
            }
            vec![Act::DownloadReport(url)]
        }
    }
}

impl Component for CalcGrid {
    fn name(&self) -> &'static str {
        "calcgrid"
    }

    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        let view = View::of(props, state);
        view.room_id()?;
        let mut parameters = view.parameters(props);
        if view.need_state {
            return Some(("calcState".to_string(), parameters));
        }
        let at = |parameters: &mut BTreeMap<String, String>, row: i32, height: i32, column: i32, width: i32| {
            for (key, value) in [
                ("sheet", i64::from(view.sheet)),
                ("row", i64::from(row)),
                ("height", i64::from(height)),
                ("column", i64::from(column)),
                ("width", i64::from(width)),
            ] {
                parameters.insert(key.to_string(), value.to_string());
            }
        };
        let (frozen_rows, _) = view.frozen_counts();
        let (row, height, column, width) = view.viewport(state.rows);
        if view.need_frozen && frozen_rows > 0 {
            at(&mut parameters, 1, frozen_rows, column, width);
            return Some(("calcCells".to_string(), parameters));
        }
        if view.need_content && !view.need_cells {
            for (key, value) in [
                ("sheet", i64::from(view.sheet)),
                ("row", i64::from(view.row)),
                ("column", i64::from(view.column)),
            ] {
                parameters.insert(key.to_string(), value.to_string());
            }
            return Some(("calcContent".to_string(), parameters));
        }
        at(&mut parameters, row, height, column, width);
        // the view's place for the others, and the others' places for it
        parameters.insert("client".to_string(), view.client.clone());
        let (top, left, bottom, right) = view.area();
        parameters.insert(
            "selectionJson".to_string(),
            json!({"row": top, "column": left, "width": right - left + 1, "height": bottom - top + 1}).to_string(),
        );
        if !view.need_cells {
            parameters.insert("since".to_string(), view.seq.to_string());
        }
        Some(("calcCells".to_string(), parameters))
    }

    fn receive(&self, props: &Value, state: &mut ComponentState, value: Value) {
        let mut view = View::of(props, state);
        let errors = value["errors"].as_str().unwrap_or("").to_string();
        if !errors.is_empty() {
            if errors.contains(ROOM_GONE) {
                view.gone = true;
                view.message = "This document is no longer open here: Enter opens it again".to_string();
                view.store(state);
                state.reload_pending = false;
                return;
            }
            view.message.clone_from(&errors);
        }
        if value["roomId"].is_string() && value["sheets"].is_array() {
            let functions = view.room["functions"].clone();
            let kept = changed_outside_block(&view, &value);
            view.room = value;
            if !view.room["functions"].is_array() {
                view.room["functions"] = functions;
            }
            view.seq = view.seq.max(view.room["seq"].as_i64().unwrap_or(0));
            if view.layout().is_null() {
                let first = view.first_sheet();
                view.show_sheet(first);
            }
            view.need_state = false;
            view.need_cells = !kept;
            view.need_frozen = !kept && view.frozen_counts().0 > 0;
            view.need_content = true;
        } else if value.get("content").is_some() {
            view.content = value["content"].as_str().unwrap_or("").to_string();
            view.content_at = view.content_key();
            view.need_content = false;
        } else if value.get("texts").is_some() || value.get("style_indices").is_some() || value["unchanged"] == true {
            if let Some(peers) = value["peers"].as_array() {
                view.peers.clone_from(peers);
            }
            if value["unchanged"] != true {
                let (frozen_rows, _) = view.frozen_counts();
                if view.need_frozen
                    && frozen_rows > 0
                    && int(&value["row"]) == 1
                    && int(&value["height"]) == frozen_rows
                {
                    view.frozen = value.clone();
                    view.need_frozen = false;
                } else {
                    view.block = value.clone();
                    view.block["sheet_index"] = json!(view.sheet);
                    view.need_cells = false;
                }
            }
            // another's edit: the room's state again (its sheets may have changed), then its cells
            if value["seq"].as_i64().is_some_and(|seq| seq > view.seq) {
                view.need_state = true;
            }
        } else if errors.is_empty() {
            state.data = Some(value);
        }
        view.store(state);
    }

    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        let view = View::of(props, state);
        if view.gone {
            return vec![vec![Span::new(view.message.clone(), Role::Warning)]];
        }
        if !view.room.is_object() {
            return vec![vec![Span::new(
                if state.error.is_empty() {
                    "Opening…".to_string()
                } else {
                    format!("! {}", state.error)
                },
                Role::Muted,
            )]];
        }
        let width = usize::from(width).max(20);
        let mut lines = Vec::new();
        bar_lines(&view, width, &mut lines);
        grid_lines(&view, state.rows, width, &mut lines);
        tab_line(&view, &mut lines);
        status_line(&view, props, &mut lines);
        if let Some(chosen) = view.menu {
            for (at, item) in MENU.iter().enumerate() {
                let span = Span::new(format!("  {item}"), Role::Text);
                lines.push(vec![if at == chosen { span.reverse() } else { span }]);
            }
        }
        if view.charts {
            chart_lines(&view, width, &mut lines);
        }
        lines
    }

    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        let mut view = View::of(props, state);
        let rows = state.rows;
        if view.gone {
            return matches!(press.key, Key::Enter).then(|| vec![Act::ReloadScreen]);
        }
        view.room_id()?;
        view.message.clear();
        let acts = if view.editing.is_some() {
            editing_key(&mut view, props, press, rows)
        } else if let Some(chosen) = view.menu {
            match press.key {
                Key::Up => {
                    view.menu = Some((chosen + MENU.len() - 1) % MENU.len());
                    Vec::new()
                }
                Key::Down => {
                    view.menu = Some((chosen + 1) % MENU.len());
                    Vec::new()
                }
                Key::Enter => {
                    view.menu = None;
                    menu_choice(&mut view, props, chosen)
                }
                Key::Escape | Key::Char('m') => {
                    view.menu = None;
                    Vec::new()
                }
                _ => return None,
            }
        } else {
            let (row, column, sheet) = (view.row, view.column, view.sheet);
            let shift = press.shift;
            let page = view.rows_shown(rows);
            let plain = press.is_plain();
            match press.key {
                Key::Up => view.move_to(row - 1, column, shift, rows),
                Key::Down => view.move_to(row + 1, column, shift, rows),
                Key::Left if plain => view.move_to(row, column - 1, shift, rows),
                Key::Right if plain => view.move_to(row, column + 1, shift, rows),
                Key::PageUp => view.move_to(row - page, column, shift, rows),
                Key::PageDown => view.move_to(row + page, column, shift, rows),
                Key::Home if press.ctrl => view.move_to(1, 1, shift, rows),
                Key::Home => view.move_to(row, 1, shift, rows),
                Key::End => {
                    let last = int(&view.layout()["columns"]).max(1);
                    view.move_to(row, last, shift, rows);
                }
                _ => {}
            }
            if (view.row, view.column) != (row, column)
                || matches!(
                    press.key,
                    Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End
                )
            {
                vec![Act::Reload]
            } else if !plain {
                return None;
            } else {
                match press.key {
                    Key::Enter | Key::Char('e') if !view.is_data() => {
                        let text = view.current_content();
                        view.editing = Some(Editing {
                            cursor: text.chars().count(),
                            text,
                            target: "cell".to_string(),
                            choice: 0,
                        });
                        Vec::new()
                    }
                    Key::Char(ch) if (ch == '=' || ch.is_ascii_digit()) && !view.is_data() => {
                        view.editing = Some(Editing {
                            text: ch.to_string(),
                            cursor: 1,
                            target: "cell".to_string(),
                            choice: 0,
                        });
                        Vec::new()
                    }
                    Key::Escape if view.anchor.is_some() => {
                        view.anchor = None;
                        Vec::new()
                    }
                    Key::Delete => {
                        let op = view.area_op("clear");
                        edit(&mut view, props, vec![op])
                    }
                    Key::Char('u') => edit(&mut view, props, vec![json!({"op": "undo"})]),
                    Key::Char('U') => edit(&mut view, props, vec![json!({"op": "redo"})]),
                    Key::Char(ch @ ('b' | 'i' | '_')) => {
                        let cell = view.cell(row, column);
                        let (path, has) = match ch {
                            'b' => ("font.b", cell.bold),
                            'i' => ("font.i", cell.italic),
                            _ => ("font.u", cell.underline),
                        };
                        let mut op = view.area_op("style");
                        op["path"] = json!(path);
                        op["value"] = json!((!has).to_string());
                        edit(&mut view, props, vec![op])
                    }
                    Key::Char(ch @ ('[' | ']')) => {
                        let indexes: Vec<u32> = view
                            .sheets()
                            .iter()
                            .filter_map(|sheet| sheet["index"].as_u64().and_then(|index| u32::try_from(index).ok()))
                            .collect();
                        let at = indexes.iter().position(|index| *index == view.sheet).unwrap_or(0);
                        let next = if ch == ']' {
                            (at + 1) % indexes.len().max(1)
                        } else {
                            (at + indexes.len().max(1) - 1) % indexes.len().max(1)
                        };
                        if let Some(index) = indexes.get(next).copied() {
                            view.show_sheet(index);
                        }
                        vec![Act::Reload]
                    }
                    Key::Char(ch @ ('<' | '>')) => {
                        #[expect(clippy::cast_precision_loss)]
                        let pixels = view.width_of(column) as f64 * PIXELS_PER_CHARACTER;
                        let width = if ch == '>' {
                            pixels + 16.0
                        } else {
                            (pixels - 16.0).max(24.0)
                        };
                        edit(
                            &mut view,
                            props,
                            vec![json!({"op": "width", "sheet": sheet, "column": column, "width": width})],
                        )
                    }
                    Key::Char('F') => edit(
                        &mut view,
                        props,
                        vec![json!({"op": "freeze", "sheet": sheet, "rows": row, "columns": column - 1})],
                    ),
                    Key::Char('d') => {
                        let (top, left, bottom, right) = view.area();
                        if bottom <= top {
                            view.message = "Select the rows to fill first (Shift+Down)".to_string();
                            Vec::new()
                        } else {
                            edit(
                                &mut view,
                                props,
                                vec![json!({"op": "fill", "sheet": sheet, "row": top, "column": left,
                                    "width": right - left + 1, "height": 1, "toRow": bottom})],
                            )
                        }
                    }
                    Key::Char('c') => {
                        view.charts = !view.charts;
                        Vec::new()
                    }
                    Key::Char('z') => {
                        view.kiosk = !view.kiosk;
                        view.need_cells = !view.covers(rows);
                        vec![Act::Reload]
                    }
                    Key::Char('R') => call(&view, props, "calcRefresh", &[]),
                    Key::Char('m') => {
                        view.menu = Some(0);
                        Vec::new()
                    }
                    _ => return None,
                }
            }
        };
        view.store(state);
        Some(acts)
    }

    fn paste(&self, props: &Value, state: &mut ComponentState, text: &str) -> Option<Vec<Act>> {
        let mut view = View::of(props, state);
        view.room_id()?;
        let acts = if let Some(editing) = view.editing.as_mut() {
            let first = text.lines().next().unwrap_or("");
            let mut chars: Vec<char> = editing.text.chars().collect();
            let at = editing.cursor.min(chars.len());
            chars.splice(at..at, first.chars());
            editing.cursor = at + first.chars().count();
            editing.text = chars.into_iter().collect();
            Vec::new()
        } else {
            let rows: Vec<Value> = text
                .replace("\r\n", "\n")
                .trim_end_matches('\n')
                .split('\n')
                .map(|line| json!(line.split('\t').collect::<Vec<&str>>()))
                .collect();
            let op = json!({"op": "paste", "sheet": view.sheet, "row": view.row, "column": view.column, "rows": rows});
            edit(&mut view, props, vec![op])
        };
        view.store(state);
        Some(acts)
    }

    fn poll_millis(&self, props: &Value, state: &ComponentState) -> Option<u64> {
        let view = View::of(props, state);
        (view.room["shared"] == true && !view.gone).then_some(POLL_MILLIS)
    }

    fn kiosk(&self, props: &Value, state: &ComponentState) -> bool {
        View::of(props, state).kiosk
    }

    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("Arrows, PgUp, PgDn, Home, End", "Move; Ctrl+Home to A1"),
            ("Shift+arrows", "Select a range"),
            (
                "Enter, e",
                "Edit the cell (Enter sets it, Esc leaves it; Tab takes a completion)",
            ),
            ("=, a digit", "Start typing a formula or a number"),
            ("Delete", "Clear the cells selected"),
            ("u, U", "Undo, redo: your own edits"),
            ("b, i, _", "Bold, italic, underline"),
            ("[, ]", "Previous, next sheet"),
            ("<, >", "Narrower, wider column"),
            ("F", "Freeze the rows above and the columns left of the cell"),
            ("d", "Fill the first row selected down"),
            ("c", "Show or hide the charts"),
            ("z", "The sheet alone on the terminal, or back"),
            ("R", "Read the data again"),
            (
                "m",
                "More: rows and columns, sheets, number formats, save, discard, export",
            ),
        ]
    }
}

/// The formula bar, and the completions while a formula is typed.
fn bar_lines(view: &View, width: usize, lines: &mut Vec<Line>) {
    let address = format!("{:<10}", view.address());
    let mut line = vec![Span::new(address, Role::Primary).bold()];
    match &view.editing {
        Some(editing) => {
            let chars: Vec<char> = editing.text.chars().collect();
            let at = editing.cursor.min(chars.len());
            let before: String = chars[..at].iter().collect();
            let under: String = chars.get(at).map_or(" ".to_string(), char::to_string);
            let after: String = chars
                .get(at + 1..)
                .map(|rest| rest.iter().collect())
                .unwrap_or_default();
            if editing.target == "rename" {
                line.push(Span::new("Sheet name: ", Role::Muted));
            }
            line.push(Span::new(before, Role::Field));
            line.push(Span::new(under, Role::Field).reverse());
            line.push(Span::new(after, Role::Field));
            lines.push(line);
            if editing.target == "cell" {
                let (_, names) = completions(&editing.text, editing.cursor, &view.functions(), &view.tables());
                if !names.is_empty() {
                    let mut suggestions = vec![Span::new(" ".repeat(10), Role::Muted)];
                    for (at, name) in names.iter().enumerate() {
                        let span = Span::new(format!(" {name} "), Role::Info);
                        suggestions.push(if at == editing.choice.min(names.len() - 1) {
                            span.reverse()
                        } else {
                            span
                        });
                    }
                    lines.push(suggestions);
                }
            }
        }
        None => {
            let content = view.current_content();
            line.push(Span::new(
                truncate_width(&content, width.saturating_sub(11)),
                Role::Text,
            ));
            lines.push(line);
        }
    }
}

/// The columns drawn: the frozen ones, then from the first shown so the active cell is on screen, as many as fit.
fn columns_shown(view: &View, room: usize) -> Vec<i32> {
    let (_, frozen_columns) = view.frozen_counts();
    let mut shown: Vec<i32> = (1..=frozen_columns).collect();
    let mut used: usize = shown.iter().map(|column| view.width_of(*column) + 1).sum();
    let mut start = view.left.max(frozen_columns + 1);
    if view.column > frozen_columns {
        // the active cell on screen: start further right while it does not fit
        while start < view.column
            && used
                + (start..=view.column)
                    .map(|column| view.width_of(column) + 1)
                    .sum::<usize>()
                > room
        {
            start += 1;
        }
    }
    let mut column = start;
    while column <= LAST_COLUMN && used + view.width_of(column) < room {
        used += view.width_of(column) + 1;
        shown.push(column);
        column += 1;
    }
    if shown.is_empty() {
        shown.push(start);
    }
    shown
}

fn grid_lines(view: &View, terminal_rows: u16, width: usize, lines: &mut Vec<Line>) {
    let (frozen_rows, _) = view.frozen_counts();
    let first = view.top.max(frozen_rows + 1);
    let rows: Vec<i32> = (1..=frozen_rows)
        .chain(first..first + view.rows_shown(terminal_rows))
        .collect();
    let gutter = rows.last().map_or(1, |row| row.to_string().len()).max(3) + 1;
    let columns = columns_shown(view, width.saturating_sub(gutter));
    let (top, left, bottom, right) = view.area();
    let mut header = vec![Span::new(" ".repeat(gutter), Role::Muted)];
    for column in &columns {
        let name = column_name(*column);
        let w = view.width_of(*column);
        let pad = w.saturating_sub(name.len());
        let text = format!("{}{name}{} ", " ".repeat(pad / 2), " ".repeat(pad - pad / 2));
        let span = Span::new(
            text,
            if (left..=right).contains(column) {
                Role::Primary
            } else {
                Role::Muted
            },
        );
        header.push(if *column == view.column { span.bold() } else { span });
    }
    lines.push(header);
    let main = cells(&view.block);
    let frozen = cells(&view.frozen);
    let peers: HashMap<(i32, i32), String> = view
        .peers
        .iter()
        .filter(|peer| peer["sheet"].as_u64() == Some(u64::from(view.sheet)))
        .filter_map(|peer| {
            let area = &peer["area"];
            Some((
                (int(&area["row"]), int(&area["column"])),
                peer["name"].as_str()?.to_string(),
            ))
        })
        .collect();
    for row in rows {
        let source = if row <= frozen_rows && view.frozen.is_object() {
            &frozen
        } else {
            &main
        };
        let label = format!("{row:>w$} ", w = gutter - 1);
        let mut line = vec![if row == view.row {
            Span::new(label, Role::Primary).bold()
        } else {
            Span::new(label, Role::Muted)
        }];
        // a text runs on into the empty cells to its right
        let mut carry = String::new();
        for column in &columns {
            let w = view.width_of(*column);
            let cell = source.get(&(row, *column)).cloned().unwrap_or_default();
            let at_cursor = (row, *column) == (view.row, view.column);
            let selected = (top..=bottom).contains(&row) && (left..=right).contains(column);
            let text = if !cell.text.is_empty() {
                carry.clear();
                let right_aligned = cell.align == "right" || (cell.number && cell.align.is_empty());
                if display_width(&cell.text) <= w {
                    match (right_aligned, cell.align.as_str()) {
                        (true, _) => format!("{:>w$}", cell.text),
                        (false, "center") => {
                            let pad = w - display_width(&cell.text);
                            format!("{}{}{}", " ".repeat(pad / 2), cell.text, " ".repeat(pad - pad / 2))
                        }
                        _ => format!("{:<w$}", cell.text),
                    }
                } else if cell.number {
                    "#".repeat(w)
                } else {
                    let shown = truncate_width(&cell.text, w);
                    carry = cell.text.chars().skip(shown.chars().count()).collect();
                    format!("{shown:<w$}")
                }
            } else if !carry.is_empty() && !at_cursor && !selected {
                let shown = truncate_width(&carry, w);
                carry = carry.chars().skip(shown.chars().count()).collect();
                format!("{shown:<w$}")
            } else {
                carry.clear();
                " ".repeat(w)
            };
            let peer = peers.get(&(row, *column));
            let mut span = Span::new(
                text,
                if at_cursor {
                    Role::Focus
                } else if selected {
                    Role::Selection
                } else if peer.is_some() {
                    Role::Warning
                } else {
                    Role::Text
                },
            );
            if at_cursor {
                span = span.reverse();
            }
            if cell.bold {
                span = span.bold();
            }
            if cell.underline || peer.is_some() {
                span = span.underline();
            }
            line.push(span);
            line.push(Span::new(" ", Role::Text));
        }
        lines.push(line);
    }
}

fn tab_line(view: &View, lines: &mut Vec<Line>) {
    let mut line = Vec::new();
    for sheet in view.sheets() {
        let name = sheet["name"].as_str().unwrap_or("");
        let text = if sheet["data"] == true {
            format!(" {name} (data) ")
        } else {
            format!(" {name} ")
        };
        let span = Span::new(text, Role::Secondary);
        line.push(if sheet["index"].as_u64() == Some(u64::from(view.sheet)) {
            span.reverse()
        } else {
            span
        });
        line.push(Span::new(" ", Role::Text));
    }
    lines.push(line);
}

fn status_line(view: &View, props: &Value, lines: &mut Vec<Line>) {
    let mut parts = Vec::new();
    let layout = view.layout();
    if layout["data"] == true {
        parts.push(format!(
            "{} rows from the database, read only",
            int(&layout["dataRows"])
        ));
    } else if props["editable"] != true {
        parts.push("Read only".to_string());
    }
    let unsaved = view.room["unsaved"].as_i64().unwrap_or(0);
    if unsaved > 0 {
        parts.push(format!(
            "{unsaved} unsaved {}",
            if unsaved == 1 { "edit" } else { "edits" }
        ));
    }
    for peer in &view.peers {
        let name = peer["name"].as_str().unwrap_or("Someone");
        let sheet = view
            .sheets()
            .into_iter()
            .find(|sheet| sheet["index"].as_u64() == peer["sheet"].as_u64())
            .and_then(|sheet| sheet["name"].as_str().map(str::to_string));
        let area = &peer["area"];
        parts.push(match (sheet, int(&area["row"])) {
            (Some(sheet), row) if row > 0 => format!("{name} at {sheet}!{}{row}", column_name(int(&area["column"]))),
            _ => format!("{name} has it open"),
        });
    }
    let mut line = vec![Span::new(parts.join(" · "), Role::Muted)];
    if !view.message.is_empty() {
        if !parts.is_empty() {
            line.push(Span::new(" · ", Role::Muted));
        }
        line.push(Span::new(view.message.clone(), Role::Warning));
    }
    lines.push(line);
}

/// The document's charts as the chart component draws a dataset: a table with a bar for the first series.
fn chart_lines(view: &View, width: usize, lines: &mut Vec<Line>) {
    let charts = view.room["charts"].as_array().map(Vec::as_slice).unwrap_or_default();
    if charts.is_empty() {
        lines.push(vec![Span::new("No chart: add one in a browser", Role::Muted)]);
        return;
    }
    for chart in charts {
        if let Some(error) = chart["error"].as_str() {
            lines.push(vec![Span::new(format!("! {error}"), Role::Negative)]);
            continue;
        }
        let props = json!({"title": chart["title"], "labels": chart["labels"],
            "series": chart["series"].as_array().map(|series| series.iter()
                .map(|one| json!({"name": one["name"], "data": one["data"]})).collect::<Vec<Value>>())});
        lines.extend(super::chart::Chart.lines(
            &props,
            &ComponentState::default(),
            u16::try_from(width).unwrap_or(u16::MAX),
            false,
        ));
    }
}

#[cfg(test)]
#[path = "calc_tests.rs"]
mod tests;
