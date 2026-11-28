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
//! - The rules of a sheet's conditional formats are addressed by their place in the sheet's list, which is the same in
//!   every room of a document, since every room applies the same edits in the same order. Their undo sets the sheet's
//!   rules back as they were ([`Room::rules_back`]). They are not set on a data sheet: a rule's range is cells, which
//!   the log replays in rooms whose data has other row counts, and a refresh with another count moves them.

use moqui_core::context::Error;
use moqui_core::workbook::ironcalc::base::cf_types::CfRuleInput;
use moqui_core::workbook::ironcalc::base::expressions::types::Area;
use moqui_core::workbook::ironcalc::base::types::MergedCell;
use moqui_core::workbook::ironcalc::base::BorderArea;
use moqui_core::workbook::parse_cell;
use serde_json::{json, Value as Json};

use super::{engine, Room, MAX_EDIT_CELLS, SHEET_COLUMNS, SHEET_ROWS};

/// The most rules of one sheet's conditional formats: each is evaluated over its cells at every evaluation.
pub const MAX_SHEET_RULES: usize = 500;
/// The most cells the rules of one sheet cover in all, each counted once for each rule over it: what an evaluation
/// walks for them.
pub const MAX_SHEET_RULE_CELLS: i64 = 10 * MAX_EDIT_CELLS;
/// The most merged cells a view is sent of a sheet ([`Room::merges_of`]).
pub const MAX_VIEW_MERGES: usize = 2_000;
/// The edits of a sheet's conditional formats.
pub const RULE_EDITS: [&str; 5] = ["addRule", "changeRule", "deleteRule", "moveRule", "rules"];

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

/// The cells of a conditional format's range (`A1:B5 D2`, as the engine keeps it), or None for a range it does not
/// read: what an evaluation walks for the rule.
#[must_use]
pub fn range_cells(range: &str) -> Option<i64> {
    let mut cells: i64 = 0;
    let mut parts = 0;
    for part in range.split_whitespace() {
        let (first, last) = part.split_once(':').unwrap_or((part, part));
        let ((row, column), (last_row, last_column)) = (parse_cell(first)?, parse_cell(last)?);
        let rows = i64::from((last_row - row).abs() + 1);
        let columns = i64::from((last_column - column).abs() + 1);
        cells = cells.saturating_add(rows.saturating_mul(columns));
        parts += 1;
    }
    (parts > 0).then_some(cells)
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
            self.model.merge_cells(&area)
        } else {
            inside_sheet(&area)?;
            self.model.unmerge_cells(&area)
        }
    }

    /// The rules of a sheet's conditional formats as a `rules` edit sets them (each `range`, `rule` as the engine takes
    /// one, with its format, and `priority`), in the sheet's order; none for a sheet the workbook does not have.
    pub(super) fn rules_of(&self, sheet: u32) -> Result<Vec<Json>, Error> {
        let Ok(list) = self.model.get_conditional_formatting_list(sheet) else {
            return Ok(Vec::new());
        };
        let mut list = list;
        list.sort_by_key(|view| view.index);
        let mut rules = Vec::with_capacity(list.len());
        for view in list {
            let mut rule = serde_json::to_value(&view.cf_rule).map_err(|err| Error::new(err.to_string()))?;
            if let Some(fields) = rule.as_object_mut() {
                if fields.remove("dxf_id").is_some() {
                    let index = u32::try_from(view.index).map_err(|err| Error::new(err.to_string()))?;
                    let format = self
                        .model
                        .get_dxf_for_conditional_formatting(sheet, index)
                        .map_err(engine)?
                        .unwrap_or_default();
                    fields.insert(
                        "format".to_string(),
                        serde_json::to_value(format).map_err(|err| Error::new(err.to_string()))?,
                    );
                }
            }
            rules.push(json!({"range": view.range, "rule": rule, "priority": view.priority}));
        }
        Ok(rules)
    }

    /// The rules of a sheet's conditional formats for a view: each with its place in the sheet's list (`index`, which
    /// an edit names), `range`, `priority` and `rule` (with its format), the one applied last (which wins) first.
    #[must_use]
    pub fn rules_view(&self, sheet: u32) -> Vec<Json> {
        let mut rules: Vec<Json> = self
            .rules_of(sheet)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(index, mut rule)| {
                rule["index"] = json!(index);
                rule
            })
            .collect();
        rules.sort_by_key(|rule| std::cmp::Reverse(rule["priority"].as_u64().unwrap_or(0)));
        rules
    }

    /// The edits that set the rules of `sheet` back as they are now: one `rules` edit, or nothing when the sheet has
    /// none and `always` is not asked (a deletion of rows or columns, which changes only the ranges there are).
    pub(super) fn rules_back(&self, sheet: u32, always: bool) -> Result<Vec<Json>, Error> {
        let rules = self.rules_of(sheet)?;
        if rules.is_empty() && !always {
            return Ok(Vec::new());
        }
        Ok(vec![json!({"op": "rules", "sheet": sheet, "rules": rules})])
    }

    /// The edits that take back an edit of a sheet's conditional formats: an added rule deleted, a rule moved back,
    /// and the sheet's rules set back as they were for a change or a deletion.
    pub(super) fn rule_inverse(&self, op: &Json, kind: &str, sheet: u32) -> Result<Vec<Json>, Error> {
        let count = self
            .model()
            .workbook
            .worksheets
            .get(sheet as usize)
            .map_or(0, |worksheet| worksheet.conditional_formatting.len());
        match kind {
            "addRule" => Ok(vec![json!({"op": "deleteRule", "sheet": sheet, "index": count})]),
            "moveRule" => {
                let index = op["index"]
                    .as_u64()
                    .and_then(|at| usize::try_from(at).ok())
                    .unwrap_or(usize::MAX);
                let up = op["up"].as_bool().unwrap_or(true);
                // a move past the first or last rule changes nothing, and nothing is to be moved back
                let moves = self
                    .model()
                    .workbook
                    .worksheets
                    .get(sheet as usize)
                    .is_some_and(|worksheet| {
                        worksheet.conditional_formatting.get(index).is_some_and(|rule| {
                            worksheet.conditional_formatting.iter().any(|other| {
                                if up {
                                    other.priority > rule.priority
                                } else {
                                    other.priority < rule.priority
                                }
                            })
                        })
                    });
                Ok(if moves {
                    vec![json!({"op": "moveRule", "sheet": sheet, "index": index, "up": !up})]
                } else {
                    Vec::new()
                })
            }
            "rules" => Err(Error::new("A sheet's rules are set back only by an undo")),
            _ => self.rules_back(sheet, true),
        }
    }

    /// An edit of a sheet's conditional formats (plans/IronCalc-DB2.md CG2.5):
    /// - `addRule` (`range`, `rule`), at the end of the list, applied last;
    /// - `changeRule` (`index`, `range`, `rule`), keeping its place;
    /// - `deleteRule` (`index`);
    /// - `moveRule` (`index`, `up`): applied one later (up, so it wins over the next) or one earlier;
    /// - `rules` (`rules`: each `range`, `rule`, `priority`): the sheet's rules set to these, in this order, applied in
    ///   the order of their priorities, which is how an edit of them is taken back; an undo's alone, refused as an
    ///   edit of a call ([`Room::rule_inverse`]).
    ///
    /// A `rule` is the engine's: `type` (CellIs, Text, Top10, Bottom10, ColorScale, DataBar, IconSet, ...) and its
    /// fields, with `format` (font, fill) for those that format; a range is held to [`MAX_EDIT_CELLS`] cells, and the
    /// sheet's rules to [`MAX_SHEET_RULE_CELLS`] in all.
    pub(super) fn apply_rule(&mut self, op: &Json, kind: &str, sheet: u32) -> Result<(), String> {
        let rule = |value: &Json| -> Result<CfRuleInput, String> {
            serde_json::from_value(value.clone()).map_err(|err| format!("Not a conditional format: {err}"))
        };
        let range = |value: &Json| -> Result<String, String> {
            let text = value.as_str().unwrap_or_default().trim().to_string();
            match range_cells(&text) {
                Some(cells) if cells <= MAX_EDIT_CELLS => Ok(text),
                Some(_) => Err(format!("A conditional format covers at most {MAX_EDIT_CELLS} cells")),
                None => Err(format!("{text:?} is not a range of cells (B2:B20)")),
            }
        };
        let index = || {
            op["index"]
                .as_u64()
                .and_then(|at| u32::try_from(at).ok())
                .ok_or_else(|| "A rule is named by its index".to_string())
        };
        let ranges: Vec<i64> = self
            .model()
            .workbook
            .worksheets
            .get(sheet as usize)
            .map(|worksheet| {
                worksheet
                    .conditional_formatting
                    .iter()
                    .map(|rule| range_cells(&rule.range).unwrap_or(0))
                    .collect()
            })
            .unwrap_or_default();
        // the cells the sheet's rules cover with this range in place of the rule at `replacing`
        let total = |text: &str, replacing: Option<usize>| -> Result<(), String> {
            let others: i64 = ranges
                .iter()
                .enumerate()
                .filter(|(at, _)| Some(*at) != replacing)
                .map(|(_, cells)| *cells)
                .sum();
            if others.saturating_add(range_cells(text).unwrap_or(0)) > MAX_SHEET_RULE_CELLS {
                return Err(format!(
                    "The conditional formats of a sheet cover at most {MAX_SHEET_RULE_CELLS} cells in all"
                ));
            }
            Ok(())
        };
        match kind {
            "addRule" => {
                if ranges.len() >= MAX_SHEET_RULES {
                    return Err(format!("A sheet has at most {MAX_SHEET_RULES} conditional formats"));
                }
                let (range, rule) = (range(&op["range"])?, rule(&op["rule"])?);
                total(&range, None)?;
                self.model.add_conditional_formatting(sheet, &range, rule)
            }
            "changeRule" => {
                let (index, range, rule) = (index()?, range(&op["range"])?, rule(&op["rule"])?);
                total(&range, usize::try_from(index).ok())?;
                self.model.update_conditional_formatting(sheet, index, &range, rule)
            }
            "deleteRule" => self.model.delete_conditional_formatting(sheet, index()?),
            "moveRule" => {
                let index = index()?;
                if op["up"].as_bool().unwrap_or(true) {
                    self.model.raise_conditional_formatting_priority(sheet, index)
                } else {
                    self.model.lower_conditional_formatting_priority(sheet, index)
                }
            }
            _ => self.set_rules(op, sheet),
        }
    }

    /// A `rules` edit: the sheet's rules taken off, each added again in its order, then moved until the order they are
    /// applied in is that of their priorities.
    fn set_rules(&mut self, op: &Json, sheet: u32) -> Result<(), String> {
        let rules = op["rules"].as_array().map(Vec::as_slice).unwrap_or_default();
        if rules.len() > MAX_SHEET_RULES {
            return Err(format!("A sheet has at most {MAX_SHEET_RULES} conditional formats"));
        }
        let mut parsed = Vec::with_capacity(rules.len());
        for one in rules {
            let range = one["range"].as_str().unwrap_or_default().to_string();
            let rule: CfRuleInput = serde_json::from_value(one["rule"].clone())
                .map_err(|err| format!("Not a conditional format: {err}"))?;
            parsed.push((range, rule, one["priority"].as_u64().unwrap_or(0)));
        }
        let count = self
            .model()
            .workbook
            .worksheets
            .get(sheet as usize)
            .map_or(0, |worksheet| worksheet.conditional_formatting.len());
        for index in (0..count).rev() {
            self.model
                .delete_conditional_formatting(sheet, u32::try_from(index).map_err(|err| err.to_string())?)?;
        }
        let wanted: Vec<u64> = parsed.iter().map(|(_, _, priority)| *priority).collect();
        for (range, rule, _) in parsed {
            self.model.add_conditional_formatting(sheet, &range, rule)?;
        }
        // the rules were given priorities in the list's order: swap neighbours in the order they are applied until it
        // is that of the priorities asked for (a few rules, so a bubble of swaps)
        let priorities = |room: &Self| -> Vec<u32> {
            room.model()
                .workbook
                .worksheets
                .get(sheet as usize)
                .map(|worksheet| {
                    worksheet
                        .conditional_formatting
                        .iter()
                        .map(|rule| rule.priority)
                        .collect()
                })
                .unwrap_or_default()
        };
        for _ in 0..wanted.len().saturating_mul(wanted.len()) {
            let now = priorities(self);
            let mut swapped = false;
            for (index, priority) in now.iter().enumerate() {
                let Some(next) = now.iter().position(|other| *other == priority + 1) else {
                    continue;
                };
                if wanted.get(index) > wanted.get(next) {
                    self.model.raise_conditional_formatting_priority(
                        sheet,
                        u32::try_from(index).map_err(|err| err.to_string())?,
                    )?;
                    swapped = true;
                    break;
                }
            }
            if !swapped {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "room_formats_tests.rs"]
mod tests;
