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

//! The conditional formats of a room's sheets (plans/IronCalc-DB2.md CG2.5): their edits, each applied through the
//! engine's own call and taken back by what the room takes before it, as [`Room::inverse`] does for the others.
//!
//! - A rule is named by its place in the sheet's list, which is the same in every room of a document (every room applies
//!   the same edits in the same order), and, from a view, by what it is (`expect`: its range and rule, as the layout
//!   gives them), so an edit made after another person's changes finds the rule it was made on, or is refused.
//! - An undo takes back only the rule its edit touched, found by what the edit left it as: another's later edits of the
//!   sheet's rules stay, and a rule changed or deleted since is not taken back ("changed since"), with nothing applied.
//!   A deletion of rows or columns, which moves the rules' ranges, sets back only the ranges.
//! - A rule is bounded: its JSON, its thresholds, its colours (`#RRGGBB`, which go into an exported file's XML), its
//!   cells, and the rules of a sheet and of the workbook in all, since every evaluation walks every rule's cells. A
//!   workbook opened with more keeps what fits ([`trim_rules`]).
//! - They are not set on a data sheet: a rule's range is cells, which the log replays in rooms whose data has other row
//!   counts, and a refresh with another count moves them.
//!
//! The tests are in `room_formats_tests.rs`.

use moqui_core::context::Error;
use moqui_core::workbook::ironcalc::base::cf_types::CfRuleInput;
use moqui_core::workbook::ironcalc::base::Model;
use moqui_core::workbook::parse_cell;
use serde_json::{json, Value as Json};

use super::formats::hex_color;
use super::{engine, Room, MAX_EDIT_CELLS};

/// The most rules of one sheet's conditional formats.
pub const MAX_SHEET_RULES: usize = 500;
/// The most rules of a workbook's conditional formats.
pub const MAX_WORKBOOK_RULES: usize = 2_000;
/// The most cells the rules of one sheet cover in all, each counted once for each rule over it.
pub const MAX_SHEET_RULE_CELLS: i64 = 10 * MAX_EDIT_CELLS;
/// The most cells the rules of a workbook cover in all.
pub const MAX_WORKBOOK_RULE_CELLS: i64 = 20 * MAX_EDIT_CELLS;
/// The largest rule, as JSON text: its formulas, texts and thresholds.
pub const MAX_RULE_BYTES: usize = 4_096;
/// The edits of a sheet's conditional formats a call sends.
pub const RULE_EDITS: [&str; 4] = ["addRule", "changeRule", "deleteRule", "moveRule"];
/// The edits that take those back: an undo's alone, refused from a call.
pub const RULE_UNDO_EDITS: [&str; 5] = ["removeRule", "setRule", "shiftRule", "putRule", "ruleRanges"];
const CHANGED_SINCE: &str = "That conditional format was changed or deleted since";

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

/// Refuse a colour of a rule (any string under a key with `color` in its name) that is not `#RRGGBB`.
fn check_colors(value: &Json) -> Result<(), String> {
    match value {
        Json::Object(fields) => fields.iter().try_for_each(|(key, field)| match field.as_str() {
            Some(text) if key.contains("color") && !hex_color(text) => {
                Err(format!("{text:?} is not a colour (#RRGGBB)"))
            }
            _ => check_colors(field),
        }),
        Json::Array(items) => items.iter().try_for_each(check_colors),
        _ => Ok(()),
    }
}

/// Refuse a rule past its bounds: its JSON past [`MAX_RULE_BYTES`], more thresholds than its kind has (a colour scale
/// three, an icon set five, a rating nine, for ten icons), a colour that is not `#RRGGBB`.
fn check_bounds(rule: &Json) -> Result<(), String> {
    let size = serde_json::to_string(rule).map_or(usize::MAX, |text| text.len());
    if size > MAX_RULE_BYTES {
        return Err(format!("A conditional format is at most {MAX_RULE_BYTES} bytes"));
    }
    let kind = rule["type"].as_str().unwrap_or_default();
    let most = match kind {
        "ColorScale" => 3,
        "IconSet" => 5,
        "IconRating" => 9,
        _ => 0,
    };
    if rule["thresholds"].as_array().map_or(0, Vec::len) > most {
        return Err(format!("A {kind} rule has at most {most} thresholds"));
    }
    check_colors(rule)
}

/// A rule as an edit sends it, checked ([`check_bounds`]) and read as the engine takes one.
fn checked_rule(rule: &Json) -> Result<CfRuleInput, String> {
    check_bounds(rule)?;
    input(rule)
}

fn input(rule: &Json) -> Result<CfRuleInput, String> {
    serde_json::from_value(rule.clone()).map_err(|err| format!("Not a conditional format: {err}"))
}

/// Two values the same, a number as its value (5 and 5.0 are one) and a missing field as null: a rule a view sends back
/// has been through the views' JSON.
fn same_json(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => x.as_f64() == y.as_f64(),
        (Json::Array(x), Json::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_json(x, y)),
        (Json::Object(x), Json::Object(y)) => x
            .keys()
            .chain(y.keys())
            .all(|key| same_json(x.get(key).unwrap_or(&Json::Null), y.get(key).unwrap_or(&Json::Null))),
        _ => a == b,
    }
}

/// Whether a rule of the sheet's list (`range`, `rule`) is the one `expect` names.
fn is_rule(view: &Json, expect: &Json) -> bool {
    view["range"].as_str().map(str::trim) == expect["range"].as_str().map(str::trim)
        && same_json(&view["rule"], &expect["rule"])
}

/// A rule of the list as an undo finds it: its range and rule.
fn named(view: &Json) -> Json {
    json!({"range": view["range"], "rule": view["rule"]})
}

/// Take off the rules of a workbook just opened past the bounds an edit is held to (each rule's, a sheet's and the
/// workbook's in all), logging what each sheet lost: a file can hold any number, of any size.
pub(super) fn trim_rules(model: &mut Model<'static>, room: &str) {
    let dxfs = model.workbook.styles.dxfs.clone();
    let (mut rules, mut cells) = (0_usize, 0_i64);
    for worksheet in &mut model.workbook.worksheets {
        let before = worksheet.conditional_formatting.len();
        let (mut sheet_rules, mut sheet_cells) = (0_usize, 0_i64);
        worksheet.conditional_formatting.retain(|rule| {
            let size = range_cells(&rule.range).unwrap_or(i64::MAX);
            let stored = serde_json::to_value(&rule.cf_rule).unwrap_or(Json::Null);
            let format = stored["dxf_id"]
                .as_u64()
                .and_then(|at| usize::try_from(at).ok())
                .and_then(|at| dxfs.get(at))
                .map_or(Json::Null, |dxf| serde_json::to_value(dxf).unwrap_or(Json::Null));
            let keep = sheet_rules < MAX_SHEET_RULES
                && rules < MAX_WORKBOOK_RULES
                && size <= MAX_EDIT_CELLS
                && sheet_cells.saturating_add(size) <= MAX_SHEET_RULE_CELLS
                && cells.saturating_add(size) <= MAX_WORKBOOK_RULE_CELLS
                && check_bounds(&stored).is_ok()
                && check_colors(&format).is_ok();
            if keep {
                sheet_rules += 1;
                rules += 1;
                sheet_cells += size;
                cells += size;
            }
            keep
        });
        let dropped = before - worksheet.conditional_formatting.len();
        if dropped > 0 {
            tracing::warn!(room, sheet = %worksheet.get_name(), dropped, kept = sheet_rules,
                "A CalcGrid workbook's sheet had conditional formats past the bounds an edit is held to; they are left out");
        }
    }
}

impl Room {
    /// The rules of a sheet's conditional formats, each `range`, `rule` (as the engine takes one, with its format) and
    /// `priority`, in the sheet's order; none for a sheet the workbook does not have.
    pub(super) fn rules_of(&self, sheet: u32) -> Result<Vec<Json>, Error> {
        let Ok(mut list) = self.model.get_conditional_formatting_list(sheet) else {
            return Ok(Vec::new());
        };
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

    /// The place in the sheet's list of the rule an edit names: by what it is (`expect`) when it says, the place it
    /// gives first; else by its place (`index`).
    fn find_rule(&self, sheet: u32, op: &Json) -> Result<usize, String> {
        let views = self.rules_of(sheet).map_err(|err| err.message().to_string())?;
        let index = op["index"].as_u64().and_then(|at| usize::try_from(at).ok());
        let expect = &op["expect"];
        if expect.is_object() {
            return match index.filter(|at| views.get(*at).is_some_and(|view| is_rule(view, expect))) {
                Some(at) => Ok(at),
                None => views
                    .iter()
                    .position(|view| is_rule(view, expect))
                    .ok_or_else(|| CHANGED_SINCE.to_string()),
            };
        }
        let at = index.ok_or_else(|| "A rule is named by its index".to_string())?;
        if at >= views.len() {
            return Err(format!("Conditional formatting index {at} out of bounds"));
        }
        Ok(at)
    }

    /// Refuse an undo whose rules were changed or deleted since its edit, before any step of it is applied: each step
    /// that names a rule by what it is finds it.
    ///
    /// # Errors
    ///
    /// "changed since", when one does not.
    pub(super) fn check_rule_undo(&self, inverse: &[Json]) -> Result<(), Error> {
        for step in inverse {
            let kind = step["op"].as_str().unwrap_or_default();
            if matches!(kind, "removeRule" | "setRule" | "shiftRule") {
                let sheet = u32::try_from(step["sheet"].as_i64().unwrap_or(0)).unwrap_or(u32::MAX);
                self.find_rule(sheet, step).map_err(Error::new)?;
            }
        }
        Ok(())
    }

    /// The edits that take back an edit of a sheet's conditional formats, before it is applied: an added rule removed,
    /// a changed rule set back, a deleted rule put back where it was in the order rules apply, a moved rule moved
    /// back. The rule each finds (`expect`) is what the edit leaves, set once it is applied ([`Room::settle_rule`]).
    pub(super) fn rule_inverse(&self, op: &Json, kind: &str, sheet: u32) -> Result<Vec<Json>, Error> {
        if RULE_UNDO_EDITS.contains(&kind) {
            return Err(Error::new(format!("{kind} is an undo's edit alone")));
        }
        let views = self.rules_of(sheet)?;
        if kind == "addRule" {
            return Ok(vec![
                json!({"op": "removeRule", "sheet": sheet, "index": views.len(), "expect": null}),
            ]);
        }
        // the edit's own refusal is its error when it is applied
        let Ok(at) = self.find_rule(sheet, op) else {
            return Ok(Vec::new());
        };
        let view = &views[at];
        Ok(match kind {
            "changeRule" => vec![json!({"op": "setRule", "sheet": sheet, "index": at,
                "range": view["range"], "rule": view["rule"], "expect": null})],
            "deleteRule" => {
                let priority = view["priority"].as_u64().unwrap_or(0);
                let higher = views
                    .iter()
                    .filter(|other| other["priority"].as_u64().unwrap_or(0) > priority)
                    .count();
                vec![
                    json!({"op": "putRule", "sheet": sheet, "range": view["range"], "rule": view["rule"], "higher": higher}),
                ]
            }
            _ => {
                // a move past the first or last rule changes nothing, and nothing is to be moved back
                let up = op["up"].as_bool().unwrap_or(true);
                let priority = view["priority"].as_u64().unwrap_or(0);
                let moves = views.iter().any(|other| {
                    let theirs = other["priority"].as_u64().unwrap_or(0);
                    if up {
                        theirs > priority
                    } else {
                        theirs < priority
                    }
                });
                if moves {
                    vec![json!({"op": "shiftRule", "sheet": sheet, "index": at, "up": !up, "expect": null})]
                } else {
                    Vec::new()
                }
            }
        })
    }

    /// Set what the undo of an edit of a sheet's conditional formats finds, now that the edit is applied: the rule as
    /// it left it, at its place.
    pub(super) fn settle_rule(&self, op: &Json, inverse: &mut [Json]) {
        let kind = op["op"].as_str().unwrap_or_default();
        let Some(step) = inverse
            .first_mut()
            .filter(|_| matches!(kind, "addRule" | "changeRule" | "moveRule"))
        else {
            return;
        };
        let sheet = u32::try_from(op["sheet"].as_i64().unwrap_or(0)).unwrap_or(u32::MAX);
        let views = self.rules_of(sheet).unwrap_or_default();
        let at = if kind == "addRule" {
            views.len().saturating_sub(1)
        } else {
            step["index"]
                .as_u64()
                .and_then(|at| usize::try_from(at).ok())
                .unwrap_or(usize::MAX)
        };
        if let Some(view) = views.get(at) {
            step["index"] = json!(at);
            step["expect"] = named(view);
        }
    }

    /// The rules' cells of each sheet, and of the workbook, in all, leaving out the rule at `replacing` of `sheet`.
    fn rule_cells(&self, sheet: u32, replacing: Option<usize>) -> (usize, usize, i64, i64) {
        let (mut sheet_rules, mut rules, mut sheet_cells, mut cells) = (0, 0, 0_i64, 0_i64);
        for (index, worksheet) in self.model().workbook.worksheets.iter().enumerate() {
            let this = u32::try_from(index).ok() == Some(sheet);
            for (at, rule) in worksheet.conditional_formatting.iter().enumerate() {
                if this && Some(at) == replacing {
                    continue;
                }
                let size = range_cells(&rule.range).unwrap_or(0);
                rules += 1;
                cells += size;
                if this {
                    sheet_rules += 1;
                    sheet_cells += size;
                }
            }
        }
        (sheet_rules, rules, sheet_cells, cells)
    }

    /// Refuse a rule over `range` on `sheet` (in place of the rule at `replacing`) past the bounds of the rules of a
    /// sheet and of the workbook.
    fn check_room_for(&self, sheet: u32, range: &str, replacing: Option<usize>) -> Result<(), String> {
        let (sheet_rules, rules, sheet_cells, cells) = self.rule_cells(sheet, replacing);
        if sheet_rules >= MAX_SHEET_RULES {
            return Err(format!("A sheet has at most {MAX_SHEET_RULES} conditional formats"));
        }
        if rules >= MAX_WORKBOOK_RULES {
            return Err(format!(
                "A workbook has at most {MAX_WORKBOOK_RULES} conditional formats"
            ));
        }
        let size = range_cells(range).unwrap_or(0);
        if sheet_cells.saturating_add(size) > MAX_SHEET_RULE_CELLS {
            return Err(format!(
                "The conditional formats of a sheet cover at most {MAX_SHEET_RULE_CELLS} cells in all"
            ));
        }
        if cells.saturating_add(size) > MAX_WORKBOOK_RULE_CELLS {
            return Err(format!(
                "The conditional formats of a workbook cover at most {MAX_WORKBOOK_RULE_CELLS} cells in all"
            ));
        }
        Ok(())
    }

    /// An edit of a sheet's conditional formats:
    /// - `addRule` (`range`, `rule`), at the end of the list, applied last;
    /// - `changeRule` (`index` or `expect`, `range`, `rule`), keeping its place;
    /// - `deleteRule` (`index` or `expect`);
    /// - `moveRule` (`index` or `expect`, `up`): applied one later (up, so it wins over the next) or one earlier;
    /// - and an undo's: `removeRule`, `setRule`, `shiftRule` (each finding its rule by `expect`), `putRule` (`range`,
    ///   `rule`, `higher`: added and moved below the rules that were applied after it), `ruleRanges` (`rules`: each
    ///   rule's place, range and rule before a deletion of rows or columns, whose range is set back).
    ///
    /// A `rule` is the engine's: `type` (CellIs, Text, Top10, Bottom10, ColorScale, DataBar, IconSet, ...) and its
    /// fields, with `format` (font, fill) for those that format; a range is held to [`MAX_EDIT_CELLS`] cells.
    pub(super) fn apply_rule(&mut self, op: &Json, kind: &str, sheet: u32) -> Result<(), String> {
        let range = |value: &Json| -> Result<String, String> {
            let text = value.as_str().unwrap_or_default().trim().to_string();
            match range_cells(&text) {
                Some(cells) if cells <= MAX_EDIT_CELLS => Ok(text),
                Some(_) => Err(format!("A conditional format covers at most {MAX_EDIT_CELLS} cells")),
                None => Err(format!("{text:?} is not a range of cells (B2:B20)")),
            }
        };
        let place = |at: usize| u32::try_from(at).map_err(|err| err.to_string());
        match kind {
            "addRule" => {
                let (range, rule) = (range(&op["range"])?, checked_rule(&op["rule"])?);
                self.check_room_for(sheet, &range, None)?;
                self.model.add_conditional_formatting(sheet, &range, rule)
            }
            "changeRule" => {
                let at = self.find_rule(sheet, op)?;
                let (range, rule) = (range(&op["range"])?, checked_rule(&op["rule"])?);
                self.check_room_for(sheet, &range, Some(at))?;
                self.model
                    .update_conditional_formatting(sheet, place(at)?, &range, rule)
            }
            "deleteRule" | "removeRule" => {
                let at = self.find_rule(sheet, op)?;
                self.model.delete_conditional_formatting(sheet, place(at)?)
            }
            "moveRule" | "shiftRule" => {
                let at = place(self.find_rule(sheet, op)?)?;
                if op["up"].as_bool().unwrap_or(true) {
                    self.model.raise_conditional_formatting_priority(sheet, at)
                } else {
                    self.model.lower_conditional_formatting_priority(sheet, at)
                }
            }
            "setRule" => {
                let at = self.find_rule(sheet, op)?;
                let rule = input(&op["rule"])?;
                let range = op["range"].as_str().unwrap_or_default();
                self.model.update_conditional_formatting(sheet, place(at)?, range, rule)
            }
            "putRule" => {
                let rule = input(&op["rule"])?;
                self.model
                    .add_conditional_formatting(sheet, op["range"].as_str().unwrap_or_default(), rule)?;
                let count = self.rules_of(sheet).map_err(|err| err.message().to_string())?.len();
                let at = place(count.saturating_sub(1))?;
                let higher = usize::try_from(op["higher"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
                for _ in 0..higher.min(count.saturating_sub(1)) {
                    self.model.lower_conditional_formatting_priority(sheet, at)?;
                }
                Ok(())
            }
            _ => self.set_rule_ranges(op, sheet),
        }
    }

    /// A `ruleRanges` edit: each rule a deletion of rows or columns moved, found at its place by its kind, given the
    /// range it had back; its rule as it is now (another's later change of it stays). A rule not found is passed by.
    fn set_rule_ranges(&mut self, op: &Json, sheet: u32) -> Result<(), String> {
        let views = self.rules_of(sheet).map_err(|err| err.message().to_string())?;
        for entry in op["rules"].as_array().map(Vec::as_slice).unwrap_or_default() {
            let Some(at) = entry["index"].as_u64().and_then(|at| usize::try_from(at).ok()) else {
                continue;
            };
            let Some(view) = views
                .get(at)
                .filter(|view| view["rule"]["type"] == entry["rule"]["type"])
            else {
                tracing::info!(room = %self.id, index = at, "A conditional format moved by a deletion was not found to set its range back");
                continue;
            };
            let range = entry["range"].as_str().unwrap_or_default();
            if view["range"].as_str() == Some(range) || range_cells(range).is_none() {
                continue;
            }
            let rule = input(&view["rule"])?;
            let place = u32::try_from(at).map_err(|err| err.to_string())?;
            self.model.update_conditional_formatting(sheet, place, range, rule)?;
        }
        Ok(())
    }

    /// The edits that set back the ranges of a sheet's rules after a deletion of its rows or columns moves them; none
    /// when it has no rule.
    pub(super) fn rule_ranges_back(&self, sheet: u32) -> Result<Vec<Json>, Error> {
        let views = self.rules_of(sheet)?;
        if views.is_empty() {
            return Ok(Vec::new());
        }
        let rules: Vec<Json> = views
            .iter()
            .enumerate()
            .map(|(index, view)| json!({"index": index, "range": view["range"], "rule": view["rule"]}))
            .collect();
        Ok(vec![json!({"op": "ruleRanges", "sheet": sheet, "rules": rules})])
    }

    /// The edits that put a deleted sheet's rules back, once it is there again: each added in the order they apply.
    pub(super) fn rules_put_back(&self, sheet: u32) -> Result<Vec<Json>, Error> {
        let mut views = self.rules_of(sheet)?;
        views.sort_by_key(|view| view["priority"].as_u64().unwrap_or(0));
        Ok(views
            .iter()
            .map(|view| json!({"op": "putRule", "sheet": sheet, "range": view["range"], "rule": view["rule"], "higher": 0}))
            .collect())
    }
}
