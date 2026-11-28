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

//! The edits of a room that format cells beyond a style (plans/IronCalc-DB2.md CG2.4 to CG2.6): borders, merged cells
//! and conditional formats, each applied through the engine's own call and taken back by what the room takes before
//! it, as [`Room::inverse`] does for the others.
//!
//! - A `border` changes the cells of its area and the ring of cells around it (the engine takes a neighbour's facing
//!   side off when the new border is heavier), so its undo restores both.
//! - A `merge` moves the one cell with content to the anchor and stamps the anchor's style on the box; its undo is an
//!   unmerge and the box as it was. An `unmerge`, a `fill` (which tiles the source's merges into its target) and a
//!   deletion of rows, columns or a sheet are taken back with the merges they met put back ([`Room::merges_back`]).
//! - A sheet has at most [`MAX_SHEET_MERGES`] merged cells; a workbook opened with more keeps that many
//!   ([`trim_merges`]).
//!
//! The conditional formats are in `room_rules.rs`; the tests of both are in `room_formats_tests.rs`.

use moqui_core::context::Error;
use moqui_core::workbook::ironcalc::base::expressions::types::Area;
use moqui_core::workbook::ironcalc::base::types::MergedCell;
use moqui_core::workbook::ironcalc::base::BorderArea;
use serde_json::{json, Value as Json};

use super::{Room, MAX_EDIT_CELLS, SHEET_COLUMNS, SHEET_ROWS};

/// The most merged cells of a sheet: a view looks each cell it draws up among them.
pub const MAX_SHEET_MERGES: usize = 2_000;
/// The most merged cells a view is sent of a sheet ([`Room::merges_of`]): all of them.
pub const MAX_VIEW_MERGES: usize = MAX_SHEET_MERGES;

/// The engine's name of a border's place: all, outer, inner, top, bottom, left, right, none, and the inner lines one
/// way (centerH, centerV).
fn border_type(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "all" => "All",
        "outer" => "Outer",
        "inner" => "Inner",
        "top" => "Top",
        "bottom" => "Bottom",
        "left" => "Left",
        "right" => "Right",
        "centerH" => "CenterH",
        "centerV" => "CenterV",
        "none" => "None",
        _ => return None,
    })
}

/// An area of an edit as the engine takes it, from its `row`, `column`, `width` and `height`.
fn area_of(op: &Json, sheet: u32) -> Area {
    let small = |key: &str, default: i64| i32::try_from(op[key].as_i64().unwrap_or(default)).unwrap_or(i32::MAX);
    Area {
        sheet,
        row: small("row", 1),
        column: small("column", 1),
        width: small("width", 1).max(1),
        height: small("height", 1).max(1),
    }
}

/// Refuse an area of an edit that is not inside the sheet (an `unmerge`, which the engine does not walk cell by cell, so
/// it is not held to [`MAX_EDIT_CELLS`] as the other area edits are).
fn inside_sheet(area: &Area) -> Result<(), String> {
    let (row, column) = (i64::from(area.row), i64::from(area.column));
    let (last_row, last_column) = (row + i64::from(area.height) - 1, column + i64::from(area.width) - 1);
    if row < 1 || column < 1 || last_row > SHEET_ROWS || last_column > SHEET_COLUMNS {
        return Err("An edit's area is outside the sheet".to_string());
    }
    Ok(())
}

fn merge_json(sheet: u32, merged: &MergedCell) -> Json {
    json!({"sheet": sheet, "row": merged.row, "column": merged.column, "width": merged.width, "height": merged.height})
}

impl Room {
    /// The merged cells of `sheet` that meet the rows `row` to `last_row` and columns `column` to `last_column`.
    fn merges_meeting(&self, sheet: u32, row: i32, column: i32, last_row: i32, last_column: i32) -> Vec<MergedCell> {
        self.model()
            .workbook
            .worksheets
            .get(sheet as usize)
            .map(|worksheet| {
                worksheet
                    .merged_cells
                    .iter()
                    .filter(|merged| merged.intersects(row, column, last_column - column + 1, last_row - row + 1))
                    .copied()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The merged cells of `sheet` that meet a rectangle, for a view to draw ([`Room::block`], the layout): each as its
    /// anchor's `row` and `column` and its `width` and `height`, at most [`MAX_VIEW_MERGES`].
    #[must_use]
    pub fn merges_of(&self, sheet: u32, row: i32, column: i32, last_row: i32, last_column: i32) -> Vec<Json> {
        self.merges_meeting(sheet, row, column, last_row, last_column)
            .iter()
            .take(MAX_VIEW_MERGES)
            .map(|merged| json!({"row": merged.row, "column": merged.column, "width": merged.width, "height": merged.height}))
            .collect()
    }

    /// The edits that put back the merged cells of `sheet` meeting a rectangle as they are now, after an edit that may
    /// move, cut, add or take them away: the merges over the rectangle and those boxes taken off, then each box merged
    /// again with its cells as they are now set before (so the merge finds one cell with content) and after (so each
    /// cell has its own style again, which a merge stamps over). Nothing when no merge meets it.
    ///
    /// # Errors
    ///
    /// An error when the boxes hold more than [`MAX_EDIT_CELLS`] cells, or the engine cannot read them.
    pub(super) fn merges_back(
        &self,
        sheet: u32,
        row: i32,
        column: i32,
        last_row: i32,
        last_column: i32,
    ) -> Result<Vec<Json>, Error> {
        let meeting = self.merges_meeting(sheet, row, column, last_row, last_column);
        if meeting.is_empty() {
            return Ok(Vec::new());
        }
        // what is unmerged when it is taken back: the rectangle and every box that meets it, where what the edit left
        // of a box (cut or moved within it) is
        let bounds = meeting.iter().fold((row, column, last_row, last_column), |at, merged| {
            (
                at.0.min(merged.row),
                at.1.min(merged.column),
                at.2.max(merged.last_row()),
                at.3.max(merged.last_column()),
            )
        });
        let all = self.merges_meeting(sheet, bounds.0, bounds.1, bounds.2, bounds.3);
        let cells = all.iter().fold(0_i64, |cells, merged| {
            cells.saturating_add(i64::from(merged.width) * i64::from(merged.height))
        });
        if cells > MAX_EDIT_CELLS {
            return Err(Error::new(format!(
                "The merged cells this edit meets hold more than {MAX_EDIT_CELLS} cells"
            )));
        }
        let mut edits = vec![
            json!({"op": "unmerge", "sheet": sheet, "row": bounds.0, "column": bounds.1,
            "width": bounds.3 - bounds.1 + 1, "height": bounds.2 - bounds.0 + 1}),
        ];
        for merged in &all {
            let cells = self.snapshot(sheet, merged.row, merged.column, merged.width, merged.height, false)?;
            edits.extend(cells.iter().cloned());
            let mut merge = merge_json(sheet, merged);
            merge["op"] = json!("merge");
            edits.push(merge);
            edits.extend(cells);
        }
        Ok(edits)
    }

    /// Whether `sheet` has merged cells.
    pub(super) fn has_merges(&self, sheet: u32) -> bool {
        self.model()
            .workbook
            .worksheets
            .get(sheet as usize)
            .is_some_and(|worksheet| !worksheet.merged_cells.is_empty())
    }

    /// The edits that take back a `border`: the cells of its area and of the ring around it as they are now, the cells
    /// beyond the used area cleared of what the border gives them.
    pub(super) fn border_inverse(&self, op: &Json, sheet: u32) -> Result<Vec<Json>, Error> {
        let area = area_of(op, sheet);
        let top = (area.row - 1).max(1);
        let left = (area.column - 1).max(1);
        let bottom = (area.row + area.height).min(i32::try_from(SHEET_ROWS).unwrap_or(i32::MAX));
        let right = (area.column + area.width).min(i32::try_from(SHEET_COLUMNS).unwrap_or(i32::MAX));
        let (width, height) = (right - left + 1, bottom - top + 1);
        let mut edits = vec![
            json!({"op": "clearFormatting", "sheet": sheet, "row": top, "column": left, "width": width, "height": height}),
        ];
        edits.extend(self.snapshot(sheet, top, left, width, height, false)?);
        Ok(edits)
    }

    /// A `border` (`kind`: all, outer, inner, top, bottom, left, right, centerH, centerV or none; `style`: thin,
    /// medium, thick, double, dotted, slantdashdot, mediumdashed, mediumdashdotdot or mediumdashdot; `color`:
    /// `#RRGGBB`) over an area of the sheet, not whole rows or columns (whose borders are the rows' and columns' own
    /// styles, which an undo of the cells does not take back).
    pub(super) fn apply_border(&mut self, op: &Json, sheet: u32) -> Result<(), String> {
        let area = area_of(op, sheet);
        if i64::from(area.width) >= SHEET_COLUMNS || i64::from(area.height) >= SHEET_ROWS {
            return Err("A border is drawn over cells, not whole rows or columns".to_string());
        }
        let kind = op["kind"].as_str().unwrap_or("all");
        let kind = border_type(kind).ok_or_else(|| format!("{kind:?} is not a border"))?;
        let style = op["style"].as_str().unwrap_or("thin");
        let color = op["color"].as_str().unwrap_or("#000000");
        if !hex_color(color) {
            return Err(format!("{color:?} is not a colour (#RRGGBB)"));
        }
        let border: BorderArea =
            serde_json::from_value(json!({"item": {"style": style, "color": color}, "type": kind}))
                .map_err(|err| format!("Not a border: {err}"))?;
        self.model.set_area_with_border(&area, &border)
    }

    /// The edits that take back a `merge`: the box unmerged, and its cells as they are now (the merge moves the one
    /// with content to the anchor and stamps the anchor's style on the others).
    pub(super) fn merge_inverse(&self, op: &Json, sheet: u32) -> Result<Vec<Json>, Error> {
        let area = area_of(op, sheet);
        let mut edits = vec![
            json!({"op": "unmerge", "sheet": sheet, "row": area.row, "column": area.column,
            "width": area.width, "height": area.height}),
        ];
        edits.extend(self.snapshot(sheet, area.row, area.column, area.width, area.height, true)?);
        Ok(edits)
    }

    /// The edits that take back an `unmerge`: the merges it takes off, put back.
    pub(super) fn unmerge_inverse(&self, op: &Json, sheet: u32) -> Result<Vec<Json>, Error> {
        let area = area_of(op, sheet);
        inside_sheet(&area).map_err(Error::new)?;
        self.merges_back(
            sheet,
            area.row,
            area.column,
            area.row.saturating_add(area.height - 1),
            area.column.saturating_add(area.width - 1),
        )
    }

    pub(super) fn apply_merge(&mut self, op: &Json, sheet: u32, merge: bool) -> Result<(), String> {
        let area = area_of(op, sheet);
        if merge {
            if self.merge_count(sheet) >= MAX_SHEET_MERGES {
                return Err(format!("A sheet has at most {MAX_SHEET_MERGES} merged cells"));
            }
            self.model.merge_cells(&area)
        } else {
            inside_sheet(&area)?;
            self.model.unmerge_cells(&area)
        }
    }
    /// The cells of the merged cells an `unmerge` over `op`'s area takes off, for the budget of a call: the engine only
    /// takes them off its list, and its undo sets their boxes again.
    pub(super) fn unmerge_cells(&self, op: &Json, sheet: u32) -> i64 {
        let area = area_of(op, sheet);
        if inside_sheet(&area).is_err() {
            return 0;
        }
        self.merges_meeting(
            sheet,
            area.row,
            area.column,
            area.row + area.height - 1,
            area.column + area.width - 1,
        )
        .iter()
        .map(|merged| i64::from(merged.width) * i64::from(merged.height))
        .sum()
    }

    /// The merged cells of `sheet`.
    fn merge_count(&self, sheet: u32) -> usize {
        self.model()
            .workbook
            .worksheets
            .get(sheet as usize)
            .map_or(0, |worksheet| worksheet.merged_cells.len())
    }

    /// Refuse a `fill` that would leave its sheet with more than [`MAX_SHEET_MERGES`] merged cells: the engine tiles
    /// the source's merges into the target, once for each time the source goes into it.
    pub(super) fn check_fill_merges(&self, op: &Json, sheet: u32) -> Result<(), String> {
        let area = area_of(op, sheet);
        let last_row = area.row.saturating_add(area.height - 1);
        let last_column = area.column.saturating_add(area.width - 1);
        let source = self
            .merges_meeting(sheet, area.row, area.column, last_row, last_column)
            .len();
        if source == 0 {
            return Ok(());
        }
        // how many times the source goes into the span it is filled over, at most
        let span = |first: i32, last: i32, to: i64, size: i32| {
            (i64::from(last).max(to) - i64::from(first).min(to) + 1) / i64::from(size.max(1)) + 1
        };
        let reach = match (op["toRow"].as_i64(), op["toColumn"].as_i64()) {
            (Some(to), _) => span(area.row, last_row, to, area.height),
            (None, Some(to)) => span(area.column, last_column, to, area.width),
            _ => 1,
        };
        let tiles = usize::try_from(reach).unwrap_or(usize::MAX);
        if self.merge_count(sheet).saturating_add(source.saturating_mul(tiles)) > MAX_SHEET_MERGES {
            return Err(format!("A sheet has at most {MAX_SHEET_MERGES} merged cells"));
        }
        Ok(())
    }
}

/// Whether `text` is a colour as the edits take one: `#RRGGBB`. A colour goes into the workbook's styles as it is
/// sent, and from there into an exported file's XML.
#[must_use]
pub fn hex_color(text: &str) -> bool {
    text.strip_prefix('#')
        .is_some_and(|hex| hex.len() == 6 && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
}

/// Take off the merged cells of a workbook just opened past [`MAX_SHEET_MERGES`] on a sheet, logging each sheet's, so a
/// view's search of them stays short (a file can hold any number).
pub(super) fn trim_merges(model: &mut moqui_core::workbook::ironcalc::base::Model<'static>, room: &str) {
    for worksheet in &mut model.workbook.worksheets {
        if worksheet.merged_cells.len() > MAX_SHEET_MERGES {
            tracing::warn!(room, sheet = %worksheet.get_name(), merges = worksheet.merged_cells.len(), kept = MAX_SHEET_MERGES,
                "A CalcGrid workbook's sheet had more merged cells than are kept; the rest are unmerged");
            worksheet.merged_cells.truncate(MAX_SHEET_MERGES);
        }
    }
}

#[cfg(test)]
#[path = "room_formats_tests.rs"]
mod tests;
