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
//! - The room keeps an id for each rule beside the engine's list of each sheet ([`Room::rule_ids`]): a rule keeps it
//!   whatever is added, deleted or moved around it, and an undo finds its rule by it. The ids are the room's own and
//!   never leave it: each room keeps its own for the same rules (a room opened after a save numbers them afresh), and
//!   every room applies the same edits to the same rules, so each finds the same rule by its own id.
//! - An edit a call sends names its rule by its place and by what it is (`expect`: its range and rule as the layout
//!   gives them), so an edit made after another person's changes, or a redo after an undo moved it, finds it, or is
//!   refused.
//! - An undo takes back only the rule its edit touched, and only when that rule is as the edit left it: a rule another
//!   deleted or changed since is passed by (counted, [`Room::rules_passed`]) and the rest of the undo applied, the same
//!   in every room, so the person's undo moves on. A deletion of rows or columns is taken back with its rules' ranges
//!   and formulas (which it may have made `#REF!`) set back, for the rules still as it left them.
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
pub const RULE_UNDO_EDITS: [&str; 6] = [
    "removeRule",
    "setRule",
    "shiftRule",
    "putRule",
    "ruleCheck",
    "ruleRanges",
];
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
/// three, an icon set five, a rating nine, one for each of its icons), a colour that is not `#RRGGBB`.
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

/// The rule id a step names.
fn step_id(step: &Json) -> Option<u64> {
    step["id"].as_u64()
}

impl Room {
    /// Give every rule of the workbook an id of its own, by its place: a room just opened.
    pub(super) fn assign_rule_ids(&mut self) {
        let counts: Vec<usize> = self
            .model()
            .workbook
            .worksheets
            .iter()
            .map(|worksheet| worksheet.conditional_formatting.len())
            .collect();
        self.rule_ids = counts
            .into_iter()
            .map(|count| (0..count).map(|_| self.fresh_rule_id()).collect())
            .collect();
    }

    fn fresh_rule_id(&mut self) -> u64 {
        self.next_rule_id += 1;
        self.next_rule_id
    }

    /// Keep the rules' ids in step with the engine's lists after an edit that adds, deletes or moves sheets (the
    /// edit's own changes, [`Room::track_sheets`]), and, as a last resort, give a sheet whose list has another length
    /// than its ids fresh ones (logged: its pending undos of rules pass its rules by).
    pub(super) fn sync_rule_ids(&mut self) {
        let counts: Vec<usize> = self
            .model()
            .workbook
            .worksheets
            .iter()
            .map(|worksheet| worksheet.conditional_formatting.len())
            .collect();
        self.rule_ids.resize_with(counts.len(), Vec::new);
        for (sheet, count) in counts.into_iter().enumerate() {
            if self.rule_ids[sheet].len() != count {
                tracing::debug!(room = %self.id, sheet, "A CalcGrid sheet's conditional formats are given new ids");
                let fresh: Vec<u64> = (0..count).map(|_| self.fresh_rule_id()).collect();
                self.rule_ids[sheet] = fresh;
            }
        }
    }

    /// The rules' ids follow the sheets an applied edit adds (at the end), deletes or moves.
    pub(super) fn track_sheets(&mut self, kind: &str, op: &Json) {
        let index = |key: &str| usize::try_from(op[key].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
        match kind {
            "addSheet" => self.rule_ids.push(Vec::new()),
            "deleteSheet" if index("sheet") < self.rule_ids.len() => {
                self.rule_ids.remove(index("sheet"));
            }
            "moveSheet" if index("sheet") < self.rule_ids.len() => {
                let moved = self.rule_ids.remove(index("sheet"));
                let to = index("to").min(self.rule_ids.len());
                self.rule_ids.insert(to, moved);
            }
            _ => {}
        }
    }

    /// The place in `sheet`'s list of the rule `id`.
    fn rule_at(&self, sheet: u32, id: u64) -> Option<usize> {
        self.rule_ids.get(sheet as usize)?.iter().position(|found| *found == id)
    }

    fn rule_id(&self, sheet: u32, at: usize) -> Option<u64> {
        self.rule_ids.get(sheet as usize)?.get(at).copied()
    }

    /// The rule `id` of `sheet` as an undo finds it (its range and rule), when it is there.
    fn rule_named(&self, sheet: u32, id: u64) -> Option<Json> {
        let at = self.rule_at(sheet, id)?;
        self.rules_of(sheet).ok()?.get(at).map(named)
    }

    /// How many steps of the last undo passed a rule by, another having changed or deleted it since.
    #[must_use]
    pub fn rules_passed(&self) -> usize {
        self.rules_passed
    }

    /// A step of an undo passes its rule by: another changed or deleted it since.
    fn pass_by(&mut self, kind: &str, id: Option<u64>) {
        self.rules_passed += 1;
        tracing::debug!(room = %self.id, step = kind, rule = id, "An undo passed by a conditional format changed since");
    }

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
    /// an edit names with what the rule is), `range`, `priority` and `rule` (with its format), the one applied last
    /// (which wins) first.
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

    /// The place in the sheet's list of the rule an edit of a call names: by what it is (`expect`), at the place it
    /// gives (`index`) when the rule there is that, else the first that is.
    fn find_rule(&self, sheet: u32, op: &Json) -> Result<usize, String> {
        let expect = &op["expect"];
        if !expect.is_object() {
            return Err("A conditional format is named by what it is (expect) beside its index".to_string());
        }
        let views = self.rules_of(sheet).map_err(|err| err.message().to_string())?;
        let index = op["index"].as_u64().and_then(|at| usize::try_from(at).ok());
        match index.filter(|at| views.get(*at).is_some_and(|view| is_rule(view, expect))) {
            Some(at) => Ok(at),
            None => views
                .iter()
                .position(|view| is_rule(view, expect))
                .ok_or_else(|| CHANGED_SINCE.to_string()),
        }
    }

    /// The edits that take back an edit of a sheet's conditional formats, before it is applied, each naming its rule by
    /// id: an added rule removed, a changed rule set back, a deleted rule put back where it was among the rules, a
    /// moved rule moved back. What the rule is once the edit is applied is set after it ([`Room::settle_rule`]).
    pub(super) fn rule_inverse(&self, op: &Json, kind: &str, sheet: u32) -> Result<Vec<Json>, Error> {
        if RULE_UNDO_EDITS.contains(&kind) {
            return Err(Error::new(format!("{kind} is an undo's edit alone")));
        }
        if kind == "addRule" {
            return Ok(vec![
                json!({"op": "removeRule", "sheet": sheet, "id": null, "expect": null}),
            ]);
        }
        // the edit's own refusal is its error when it is applied
        let Ok(at) = self.find_rule(sheet, op) else {
            return Ok(Vec::new());
        };
        let (views, id) = (self.rules_of(sheet)?, self.rule_id(sheet, at));
        let view = &views[at];
        let priority = view["priority"].as_u64().unwrap_or(0);
        Ok(match kind {
            "changeRule" => vec![json!({"op": "setRule", "sheet": sheet, "id": id,
                "range": view["range"], "rule": view["rule"], "expect": null})],
            "deleteRule" => {
                // the rule applied just before it, and how many were, for its place when it is put back
                let lower: Vec<(u64, usize)> = views
                    .iter()
                    .enumerate()
                    .filter_map(|(index, other)| {
                        let theirs = other["priority"].as_u64().unwrap_or(0);
                        (theirs < priority).then_some((theirs, index))
                    })
                    .collect();
                let after = lower
                    .iter()
                    .max_by_key(|(theirs, _)| *theirs)
                    .and_then(|(_, index)| self.rule_id(sheet, *index));
                vec![
                    json!({"op": "putRule", "sheet": sheet, "id": id, "range": view["range"], "rule": view["rule"],
                    "after": after, "below": lower.len()}),
                ]
            }
            _ => {
                // a move past the first or last rule changes nothing, and nothing is to be moved back
                let up = op["up"].as_bool().unwrap_or(true);
                let moves = views.iter().any(|other| {
                    let theirs = other["priority"].as_u64().unwrap_or(0);
                    if up {
                        theirs > priority
                    } else {
                        theirs < priority
                    }
                });
                if moves {
                    vec![json!({"op": "shiftRule", "sheet": sheet, "id": id, "up": !up})]
                } else {
                    Vec::new()
                }
            }
        })
    }

    /// Set, once an edit is applied, what the steps of its undo find: the rule an added rule's removal names, what a
    /// changed rule is now, and what a deletion of rows or columns left each rule as.
    pub(super) fn settle_rule(&self, op: &Json, inverse: &mut [Json]) {
        let sheet = u32::try_from(op["sheet"].as_i64().unwrap_or(0)).unwrap_or(u32::MAX);
        for step in inverse.iter_mut() {
            match step["op"].as_str().unwrap_or_default() {
                "removeRule" if step["id"].is_null() => {
                    if let Some(id) = self.rule_ids.get(sheet as usize).and_then(|ids| ids.last().copied()) {
                        step["id"] = json!(id);
                        step["expect"] = self.rule_named(sheet, id).unwrap_or(Json::Null);
                    }
                }
                "setRule" if step["expect"].is_null() => {
                    if let Some(id) = step_id(step) {
                        step["expect"] = self.rule_named(sheet, id).unwrap_or(Json::Null);
                    }
                }
                "ruleCheck" => {
                    if let Some(entries) = step["rules"].as_array_mut() {
                        for entry in entries {
                            if let Some(id) = step_id(entry) {
                                entry["after"] = self.rule_named(sheet, id).unwrap_or(Json::Null);
                            }
                        }
                    }
                }
                _ => {}
            }
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

    fn priorities(&self, sheet: u32) -> Vec<u32> {
        self.model()
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
    }

    /// An edit of a sheet's conditional formats:
    /// - `addRule` (`range`, `rule`), at the end of the list, applied last;
    /// - `changeRule` (`index` and `expect`, `range`, `rule`), keeping its place;
    /// - `deleteRule` (`index` and `expect`);
    /// - `moveRule` (`index` and `expect`, `up`): applied one later (up, so it wins over the next) or one earlier;
    /// - and an undo's, each naming its rule by `id` and passing it by when it is not as its edit left it:
    ///   `removeRule` and `setRule` (`expect`), `shiftRule`, `putRule` (`range`, `rule`, and its place: `after` the
    ///   rule applied just before it, else `below` that many), `ruleCheck` and `ruleRanges` (before and after a
    ///   deletion of rows or columns).
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
                self.model.add_conditional_formatting(sheet, &range, rule)?;
                let id = self.fresh_rule_id();
                self.push_rule_id(sheet, id);
                Ok(())
            }
            "changeRule" => {
                let at = self.find_rule(sheet, op)?;
                let (range, rule) = (range(&op["range"])?, checked_rule(&op["rule"])?);
                self.check_room_for(sheet, &range, Some(at))?;
                self.model
                    .update_conditional_formatting(sheet, place(at)?, &range, rule)
            }
            "deleteRule" => {
                let at = self.find_rule(sheet, op)?;
                self.delete_rule(sheet, at)
            }
            "moveRule" => {
                let at = place(self.find_rule(sheet, op)?)?;
                self.move_rule(sheet, at, op["up"].as_bool().unwrap_or(true))
            }
            _ => self.apply_rule_undo(op, kind, sheet),
        }
    }

    fn push_rule_id(&mut self, sheet: u32, id: u64) {
        if self.rule_ids.len() <= sheet as usize {
            self.rule_ids.resize_with(sheet as usize + 1, Vec::new);
        }
        self.rule_ids[sheet as usize].push(id);
    }

    fn delete_rule(&mut self, sheet: u32, at: usize) -> Result<(), String> {
        self.model
            .delete_conditional_formatting(sheet, u32::try_from(at).map_err(|err| err.to_string())?)?;
        if let Some(ids) = self.rule_ids.get_mut(sheet as usize).filter(|ids| at < ids.len()) {
            ids.remove(at);
        }
        Ok(())
    }

    fn move_rule(&mut self, sheet: u32, at: u32, up: bool) -> Result<(), String> {
        if up {
            self.model.raise_conditional_formatting_priority(sheet, at)
        } else {
            self.model.lower_conditional_formatting_priority(sheet, at)
        }
    }

    /// The steps of an undo of rules, each passing its rule by ([`Room::pass_by`]) when it is gone or not as its edit
    /// left it.
    fn apply_rule_undo(&mut self, op: &Json, kind: &str, sheet: u32) -> Result<(), String> {
        let id = step_id(op);
        let found = id.and_then(|id| self.rule_at(sheet, id));
        let now = id.and_then(|id| self.rule_named(sheet, id));
        let place = |at: usize| u32::try_from(at).map_err(|err| err.to_string());
        match kind {
            // an added rule, still as it was added (where it is: another's insertion of rows may have moved it)
            "removeRule" => match (found, now) {
                (Some(at), Some(now)) if same_json(&now["rule"], &op["expect"]["rule"]) => self.delete_rule(sheet, at),
                _ => {
                    self.pass_by(kind, id);
                    Ok(())
                }
            },
            // a changed rule, still as the change left it: its rule back, and its range when that is as the change
            // left it too
            "setRule" => match (found, now) {
                (Some(at), Some(now)) if same_json(&now["rule"], &op["expect"]["rule"]) => {
                    let range = if now["range"] == op["expect"]["range"] {
                        &op["range"]
                    } else {
                        &now["range"]
                    };
                    let rule = input(&op["rule"])?;
                    self.model.update_conditional_formatting(
                        sheet,
                        place(at)?,
                        range.as_str().unwrap_or_default(),
                        rule,
                    )
                }
                _ => {
                    self.pass_by(kind, id);
                    Ok(())
                }
            },
            "shiftRule" => match found {
                Some(at) => self.move_rule(sheet, place(at)?, op["up"].as_bool().unwrap_or(true)),
                None => {
                    self.pass_by(kind, id);
                    Ok(())
                }
            },
            "putRule" => self.put_rule(op, sheet, id, found),
            "ruleCheck" => {
                // which rules are as the deletion left them, before the rows come back and move them again
                self.rule_restore.clear();
                for entry in op["rules"].as_array().map(Vec::as_slice).unwrap_or_default() {
                    let id = step_id(entry);
                    let now = id.and_then(|id| self.rule_named(sheet, id));
                    match (id, now) {
                        (Some(id), Some(now)) if entry["after"].is_object() && is_rule(&now, &entry["after"]) => {
                            self.rule_restore.push((id, entry["before"].clone()));
                        }
                        _ => self.pass_by(kind, id),
                    }
                }
                Ok(())
            }
            // those rules' ranges and rules (formulas the deletion made #REF! among them) as they were before it
            _ => {
                let restore = std::mem::take(&mut self.rule_restore);
                for (id, before) in restore {
                    let Some(at) = self.rule_at(sheet, id) else {
                        self.pass_by(kind, Some(id));
                        continue;
                    };
                    let rule = input(&before["rule"])?;
                    let range = before["range"].as_str().unwrap_or_default();
                    self.model
                        .update_conditional_formatting(sheet, place(at)?, range, rule)?;
                }
                Ok(())
            }
        }
    }

    /// A `putRule` step: a deleted rule added again under its id and moved down to its place among the rules (just
    /// after the rule applied before it, when that is still there, else above as many as were below it); passed by
    /// when it is there already or the sheet or workbook has no room for it.
    fn put_rule(&mut self, op: &Json, sheet: u32, id: Option<u64>, found: Option<usize>) -> Result<(), String> {
        let range = op["range"].as_str().unwrap_or_default();
        let (Some(id), None) = (id, found) else {
            self.pass_by("putRule", id);
            return Ok(());
        };
        if self.check_room_for(sheet, range, None).is_err() {
            self.pass_by("putRule", Some(id));
            return Ok(());
        }
        self.model
            .add_conditional_formatting(sheet, range, input(&op["rule"])?)?;
        self.push_rule_id(sheet, id);
        let at = self.rule_at(sheet, id).unwrap_or(0);
        let place = u32::try_from(at).map_err(|err| err.to_string())?;
        let after = op["after"].as_u64().and_then(|after| self.rule_at(sheet, after));
        let below = usize::try_from(op["below"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
        let count = self.priorities(sheet).len();
        for _ in 0..count {
            let priorities = self.priorities(sheet);
            let mine = priorities[at];
            let lower: Vec<usize> = (0..priorities.len())
                .filter(|other| priorities[*other] < mine)
                .collect();
            let next = lower.iter().copied().max_by_key(|other| priorities[*other]);
            let done = match (after, next) {
                (_, None) => true,
                (Some(after), Some(next)) => next == after,
                (None, Some(_)) => lower.len() <= below,
            };
            if done {
                break;
            }
            self.model.lower_conditional_formatting_priority(sheet, place)?;
        }
        Ok(())
    }

    /// The steps that set back a sheet's rules after a deletion of its rows or columns moves their ranges and
    /// formulas: a check before the rows or columns come back (which rules are as the deletion left them, set once it
    /// is applied, [`Room::settle_rule`]) and the ranges and rules set back after. None when the sheet has no rule.
    pub(super) fn rule_ranges_back(&self, sheet: u32) -> Result<Option<(Json, Json)>, Error> {
        let views = self.rules_of(sheet)?;
        if views.is_empty() {
            return Ok(None);
        }
        let rules: Vec<Json> = views
            .iter()
            .enumerate()
            .filter_map(|(at, view)| {
                let id = self.rule_id(sheet, at)?;
                Some(json!({"id": id, "before": named(view), "after": null}))
            })
            .collect();
        Ok(Some((
            json!({"op": "ruleCheck", "sheet": sheet, "rules": rules}),
            json!({"op": "ruleRanges", "sheet": sheet}),
        )))
    }

    /// The edits that put a deleted sheet's rules back, once it is there again: each added under its id, in the order
    /// they apply.
    pub(super) fn rules_put_back(&self, sheet: u32) -> Result<Vec<Json>, Error> {
        let views = self.rules_of(sheet)?;
        let mut order: Vec<usize> = (0..views.len()).collect();
        order.sort_by_key(|at| views[*at]["priority"].as_u64().unwrap_or(0));
        Ok(order
            .iter()
            .map(|at| {
                let view = &views[*at];
                json!({"op": "putRule", "sheet": sheet, "id": self.rule_id(sheet, *at), "range": view["range"],
                    "rule": view["rule"], "after": null, "below": usize::MAX})
            })
            .collect())
    }
}
