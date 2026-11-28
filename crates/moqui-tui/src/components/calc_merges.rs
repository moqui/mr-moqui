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

//! The merged cells of the terminal sheet (plans/IronCalc-DB2.md CG3.2), as the browser's `calc.boxAt` and
//! `calc.expandToMerges` take them: a box is drawn as one cell with its anchor's text and style, the cursor stands on
//! its anchor and steps past it, a selection that meets one takes all of it, and an author merges and unmerges the
//! selection from the menu with the `merge` and `unmerge` edits the browser sends.
//!
//! The boxes are the sheet's layout's `merges` (all of a sheet's, at most 2,000) and those of the blocks read; each is
//! its anchor's `row` and `column`, `width` and `height`.

use std::collections::HashSet;

use serde_json::Value;

use super::{edit, int, View, LAST_COLUMN, LAST_ROW, MAX_CELLS};
use crate::components::Act;

/// The most boxes taken from one list, the layout's or a block's: a sheet holds at most 2,000 (the server's
/// `MAX_SHEET_MERGES`), and the layout carries them all.
const MAX_LISTED: usize = 2_000;

/// A merged cell: its anchor (the top left cell, which holds its content) and its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Merged {
    pub(super) row: i32,
    pub(super) column: i32,
    pub(super) width: i32,
    pub(super) height: i32,
}

impl Merged {
    fn of(value: &Value) -> Option<Self> {
        let merged = Self {
            row: int(&value["row"]),
            column: int(&value["column"]),
            width: int(&value["width"]),
            height: int(&value["height"]),
        };
        // inside the sheet, as the server keeps them
        let inside = merged.row >= 1
            && merged.column >= 1
            && merged.width >= 1
            && merged.height >= 1
            && i64::from(merged.row) + i64::from(merged.height) - 1 <= i64::from(LAST_ROW)
            && i64::from(merged.column) + i64::from(merged.width) - 1 <= i64::from(LAST_COLUMN);
        inside.then_some(merged)
    }

    pub(super) fn last_row(self) -> i32 {
        self.row.saturating_add(self.height - 1)
    }

    pub(super) fn last_column(self) -> i32 {
        self.column.saturating_add(self.width - 1)
    }

    pub(super) fn has_row(self, row: i32) -> bool {
        (self.row..=self.last_row()).contains(&row)
    }

    pub(super) fn has_column(self, column: i32) -> bool {
        (self.column..=self.last_column()).contains(&column)
    }

    pub(super) fn contains(self, row: i32, column: i32) -> bool {
        self.has_row(row) && self.has_column(column)
    }

    /// Whether it meets the rectangle from `row`, `column` to `last_row`, `last_column`.
    fn meets(self, row: i32, column: i32, last_row: i32, last_column: i32) -> bool {
        self.row <= last_row && self.last_row() >= row && self.column <= last_column && self.last_column() >= column
    }
}

/// The boxes of the lists given (at most [`MAX_LISTED`] of each), each once (by its anchor), from the top down.
fn boxes_of(lists: &[&Value]) -> Vec<Merged> {
    let mut anchors = HashSet::new();
    let mut boxes: Vec<Merged> = lists
        .iter()
        .flat_map(|list| {
            list.as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .take(MAX_LISTED)
        })
        .filter_map(Merged::of)
        .filter(|merged| anchors.insert((merged.row, merged.column)))
        .collect();
    boxes.sort_unstable_by_key(|merged| (merged.row, merged.column));
    boxes
}

/// The boxes that meet a rectangle (top, left, bottom, right).
pub(super) fn meeting(boxes: &[Merged], area: (i32, i32, i32, i32)) -> Vec<Merged> {
    let (top, left, bottom, right) = area;
    boxes
        .iter()
        .copied()
        .filter(|merged| merged.meets(top, left, bottom, right))
        .collect()
}

/// The box that holds a cell, as `calc.boxAt`.
pub(super) fn box_at(boxes: &[Merged], row: i32, column: i32) -> Option<Merged> {
    boxes.iter().copied().find(|merged| merged.contains(row, column))
}

/// A rectangle (top, left, bottom, right) grown to the boxes it meets, as `calc.expandToMerges`: a box is selected
/// whole.
pub(super) fn expand(area: (i32, i32, i32, i32), boxes: &[Merged]) -> (i32, i32, i32, i32) {
    let (mut top, mut left, mut bottom, mut right) = area;
    let mut grown = true;
    while grown {
        grown = false;
        for merged in boxes {
            if !merged.meets(top, left, bottom, right) {
                continue;
            }
            let before = (top, left, bottom, right);
            top = top.min(merged.row);
            left = left.min(merged.column);
            bottom = bottom.max(merged.last_row());
            right = right.max(merged.last_column());
            grown |= (top, left, bottom, right) != before;
        }
    }
    (top, left, bottom, right)
}

impl View {
    /// The sheet's boxes: its layout's and those of the blocks read.
    pub(super) fn boxes(&self) -> Vec<Merged> {
        boxes_of(&[&self.layout()["merges"], &self.block["merges"], &self.frozen["merges"]])
    }

    /// The cell a box holding `row`, `column` is typed into and stood on: its anchor; else the cell itself.
    pub(super) fn anchor_of(&self, row: i32, column: i32) -> (i32, i32) {
        box_at(&self.boxes(), row, column).map_or((row, column), |merged| (merged.row, merged.column))
    }

    /// Where a move of the active cell to `row`, `column` lands: a cell of a box is its anchor, and a move from inside
    /// a box to another of its cells steps past the box the way it goes (Down from the anchor to the row below it).
    pub(super) fn landing(&self, row: i32, column: i32) -> (i32, i32) {
        let boxes = self.boxes();
        let Some(target) = box_at(&boxes, row, column) else {
            return (row, column);
        };
        let (mut row, mut column) = (row, column);
        if target.contains(self.row, self.column) && (row, column) != (self.row, self.column) {
            if row > self.row {
                row = target.last_row().saturating_add(1);
            } else if row < self.row {
                row = target.row - 1;
            }
            if column > self.column {
                column = target.last_column().saturating_add(1);
            } else if column < self.column {
                column = target.column - 1;
            }
            row = row.clamp(1, LAST_ROW);
            column = column.clamp(1, LAST_COLUMN);
        }
        box_at(&boxes, row, column).map_or((row, column), |merged| (merged.row, merged.column))
    }

    /// The active cell on its box's anchor, when a merge came in under it (the view's own, or another's), and the
    /// screen on it; the selection's other corner stays.
    pub(super) fn snap(&mut self) {
        let at = self.anchor_of(self.row, self.column);
        if at != (self.row, self.column) {
            (self.row, self.column) = at;
            self.scroll_to_cursor(self.terminal_rows);
        }
    }

    /// The rectangle read for the screen, grown up and left to the anchors of the boxes that meet it (whose text a box
    /// on screen draws) while it stays within a read's [`MAX_CELLS`]; a box whose anchor is in the frozen rows has it
    /// in their block. From the layout's boxes only, so what is read does not depend on what was read.
    pub(super) fn with_anchors(&self, viewport: (i32, i32, i32, i32), frozen_rows: i32) -> (i32, i32, i32, i32) {
        let (row, height, column, width) = viewport;
        let (last_row, last_column) = (row.saturating_add(height - 1), column.saturating_add(width - 1));
        let boxes = boxes_of(&[&self.layout()["merges"]]);
        let meeting: Vec<Merged> = boxes
            .into_iter()
            .filter(|merged| {
                merged.meets(row, column, last_row, last_column)
                    || (merged.row <= frozen_rows && merged.meets(1, column, frozen_rows, last_column))
            })
            .collect();
        let top = meeting
            .iter()
            .filter(|merged| merged.row > frozen_rows && merged.has_row(row))
            .map(|merged| merged.row)
            .fold(row, i32::min);
        let left = meeting.iter().map(|merged| merged.column).fold(column, i32::min);
        let fits = |rows: i32, columns: i32| i64::from(rows) * i64::from(columns) <= i64::from(MAX_CELLS);
        let (mut row, mut height, mut column, mut width) = viewport;
        if top < row && fits(last_row - top + 1, width) {
            (row, height) = (top, last_row - top + 1);
        }
        if left < column && fits(height, last_column - left + 1) {
            (column, width) = (left, last_column - left + 1);
        }
        (row, height, column, width)
    }
}

/// The menu's Merge (`merge`) and Unmerge: the selection sent as the browser's `merge` and `unmerge` edits. Not on a
/// data sheet, which the server refuses too (the menu does not offer them there).
pub(super) fn menu_choice(view: &mut View, props: &Value, merge: bool) -> Vec<Act> {
    if view.is_data() {
        view.message = "A data sheet's cells are not merged".to_string();
        return Vec::new();
    }
    let (row, column, last_row, last_column) = view.area();
    if merge && (row, column) == (last_row, last_column) {
        view.message = "Select the cells to merge first (Shift+arrows)".to_string();
        return Vec::new();
    }
    if !merge
        && !view
            .boxes()
            .iter()
            .any(|merged| merged.meets(row, column, last_row, last_column))
    {
        view.message = "No merged cells in the selection".to_string();
        return Vec::new();
    }
    let op = view.area_op(if merge { "merge" } else { "unmerge" });
    edit(view, props, vec![op])
}
