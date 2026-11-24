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

//! Compare two recordings of `moqui test-regression`. Rows and answers are compared as data, not as text: the same
//! columns with the same values, by table and primary key. Left out:
//!
//! - the volatile columns and tables of `test/regression/volatile.json`, which differ from one run of an engine to
//!   the next (a session id, a host name, a time measured);
//! - with a Java target, the approved differences of `test/regression/approved-differences.json`: the tables, the
//!   columns and the seed rows that one engine has and the other has not, each with its reason.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use moqui_lib::error::Error;
use serde_json::{json, Map, Value};

use super::db::{RowChange, Rows, Snapshot};
use super::record::{CaseRecord, Recording};
use crate::test_cmd::{TestComponent, REGRESSION_TREE};

/// How many differences of one table, or of one case, the report lists; the rest are counted.
const LIST_LIMIT: usize = 20;

/// A field of the rows whose values match the key: `template_differences`.
pub(crate) type RowColumn = (Map<String, Value>, String);

/// What the comparison leaves out.
#[derive(Clone, Debug, Default)]
pub(crate) struct Rules {
    /// A column of every table.
    pub columns: BTreeSet<String>,
    pub tables: BTreeSet<String>,
    pub table_columns: BTreeMap<String, BTreeSet<String>>,
    /// A column of the rows a key matches, by table: `template_differences`, a field whose text differs on purpose.
    pub row_columns: BTreeMap<String, Vec<RowColumn>>,
    /// The approved differences, read only for a Java target: by engine, `java` or `rust`.
    pub engine: BTreeMap<String, EngineRules>,
    /// The files read, for the report.
    pub files: Vec<PathBuf>,
}

/// The approved differences of one engine: what only it has.
#[derive(Clone, Debug, Default)]
pub(crate) struct EngineRules {
    pub tables: BTreeSet<String>,
    pub table_columns: BTreeMap<String, BTreeSet<String>>,
    /// Rows by table, each the column values it has.
    pub rows: BTreeMap<String, Vec<Map<String, Value>>>,
}

fn table_name(text: &str) -> String {
    text.strip_prefix("public.").unwrap_or(text).to_string()
}

fn strings(value: Option<&Value>) -> impl Iterator<Item = String> + '_ {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
}

impl Rules {
    /// The rules of the selected components' `test/regression` directories; the approved differences only when one
    /// target is Java.
    pub(crate) fn load(components: &[TestComponent], java: bool) -> Result<Self, Error> {
        let mut rules = Self::default();
        let mut dirs: Vec<PathBuf> = Vec::new();
        for component in components {
            let dir = if component.dir.ends_with(REGRESSION_TREE) {
                component.dir.clone()
            } else {
                component.dir.join(REGRESSION_TREE)
            };
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        for dir in &dirs {
            let volatile = dir.join("volatile.json");
            if volatile.is_file() {
                rules.add_volatile(&read_json(&volatile)?);
                rules.files.push(volatile);
            }
            let approved = dir.join("approved-differences.json");
            if java && approved.is_file() {
                rules.add_approved(&read_json(&approved)?);
                rules.files.push(approved);
            }
        }
        Ok(rules)
    }

    pub(crate) fn add_volatile(&mut self, value: &Value) {
        self.columns.extend(strings(value.get("columns")));
        self.tables.extend(strings(value.get("tables")).map(|table| table_name(&table)));
        if let Some(map) = value.get("tableColumns").and_then(Value::as_object) {
            for (table, columns) in map {
                self.table_columns
                    .entry(table_name(table))
                    .or_default()
                    .extend(strings(Some(columns)));
            }
        }
    }

    /// The sections of approved-differences.json about data: the tables, columns and seed rows of one engine, the
    /// fields of `template_differences`, and the rows of `theme_resources`.
    pub(crate) fn add_approved(&mut self, value: &Value) {
        for (section, engine) in [("rust_only_tables", "rust"), ("java_only_tables", "java")] {
            let rules = self.engine.entry(engine.to_string()).or_default();
            rules.tables.extend(strings(value.get(section)).map(|table| table_name(&table)));
        }
        for entry in value.get("rust_only_columns").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(table), Some(name)) = (entry.get("table").and_then(Value::as_str), entry.get("name").and_then(Value::as_str)) {
                let rules = self.engine.entry("rust".to_string()).or_default();
                rules.table_columns.entry(table_name(table)).or_default().insert(name.to_string());
            }
        }
        for (section, engine) in [("rust_seed_additions", "rust"), ("java_seed_additions", "java")] {
            for entry in value.get(section).and_then(Value::as_array).into_iter().flatten() {
                let Some(table) = entry.get("table").and_then(Value::as_str) else { continue };
                let key: Vec<String> = strings(entry.get("key")).collect();
                for values in entry.get("values").and_then(Value::as_array).into_iter().flatten() {
                    let Some(values) = values.as_array() else { continue };
                    let row: Map<String, Value> = key.iter().cloned().zip(values.iter().cloned()).collect();
                    let rules = self.engine.entry(engine.to_string()).or_default();
                    rules.rows.entry(table_name(table)).or_default().push(row);
                }
            }
        }
        for entry in value.get("reviewed_seed_rows").and_then(Value::as_array).into_iter().flatten() {
            let (Some(engine), Some(table)) = (entry.get("engine").and_then(Value::as_str), entry.get("table").and_then(Value::as_str))
            else {
                continue;
            };
            if let Some(expected) = entry.get("expected").and_then(Value::as_object) {
                let rules = self.engine.entry(engine.to_string()).or_default();
                rules.rows.entry(table_name(table)).or_default().push(expected.clone());
            }
        }
        for entry in value.get("template_differences").and_then(Value::as_array).into_iter().flatten() {
            let (Some(table), Some(field), Some(key)) = (
                entry.get("table").and_then(Value::as_str),
                entry.get("field").and_then(Value::as_str),
                entry.get("key").and_then(Value::as_object),
            ) else {
                continue;
            };
            self.row_columns.entry(table_name(table)).or_default().push((key.clone(), field.to_string()));
        }
        // a theme's resource by sequence: the row each engine has there, or null when it has none
        for entry in value.get("theme_resources").and_then(Value::as_array).into_iter().flatten() {
            let (Some(theme), Some(sequence)) = (entry.get("theme"), entry.get("sequence")) else { continue };
            for engine in ["java", "rust"] {
                let Some([kind, resource]) = entry.get(engine).and_then(Value::as_array).map(Vec::as_slice) else { continue };
                let row: Map<String, Value> = [
                    ("screen_theme_id", theme.clone()),
                    ("sequence_num", sequence.clone()),
                    ("resource_type_enum_id", kind.clone()),
                    ("resource_value", resource.clone()),
                ]
                .into_iter()
                .map(|(column, value)| (column.to_string(), value))
                .collect();
                let rules = self.engine.entry(engine.to_string()).or_default();
                rules.rows.entry("screen_theme_resource".to_string()).or_default().push(row);
            }
        }
    }

    fn skips_table(&self, engine: &str, table: &str) -> bool {
        self.tables.contains(table) || self.engine.get(engine).is_some_and(|rules| rules.tables.contains(table))
    }

    /// The row as compared: without the volatile and approved columns. `None` when the row is an approved one of
    /// the engine.
    fn masked(&self, engine: &str, table: &str, row: &Value) -> Option<Value> {
        let Value::Object(map) = row else { return Some(row.clone()) };
        let rules = self.engine.get(engine);
        if let Some(rows) = rules.and_then(|rules| rules.rows.get(table)) {
            if rows.iter().any(|approved| approved.iter().all(|(column, value)| map.get(column) == Some(value))) {
                return None;
            }
        }
        let table_columns = self.table_columns.get(table);
        let engine_columns = rules.and_then(|rules| rules.table_columns.get(table));
        let row_columns: Vec<&String> = self
            .row_columns
            .get(table)
            .into_iter()
            .flatten()
            .filter(|(key, _)| key.iter().all(|(column, value)| map.get(column) == Some(value)))
            .map(|(_, column)| column)
            .collect();
        let kept: Map<String, Value> = map
            .iter()
            .filter(|(column, _)| {
                !self.columns.contains(*column)
                    && !table_columns.is_some_and(|columns| columns.contains(*column))
                    && !engine_columns.is_some_and(|columns| columns.contains(*column))
                    && !row_columns.contains(column)
            })
            .map(|(column, value)| (column.clone(), value.clone()))
            .collect();
        Some(Value::Object(kept))
    }

    fn masked_rows(&self, engine: &str, table: &str, rows: &Rows) -> BTreeMap<String, Value> {
        rows.iter()
            .filter_map(|(key, row)| self.masked(engine, table, row).map(|row| (key.clone(), row)))
            .collect()
    }
}

fn read_json(path: &Path) -> Result<Value, Error> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| Error::new(format!("could not read {}", path.display())).with_source(err))?;
    serde_json::from_str(&text).map_err(|err| Error::new(format!("{} is not JSON: {err}", path.display())))
}

/// The engine of a recording's target, for the approved differences: `java` or `rust`.
fn engine_of(recording: &Recording) -> &'static str {
    if recording.target.starts_with("java:") {
        "java"
    } else {
        "rust"
    }
}

/// One difference, as the report lists it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Difference {
    /// `boot` or the case's `spec / case`.
    pub place: String,
    pub what: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Report {
    pub baseline: String,
    pub candidate: String,
    pub boot_tables: usize,
    pub boot_rows: usize,
    pub cases: Vec<CaseVerdict>,
    pub boot_differences: Vec<Difference>,
    pub boot_unlisted: usize,
    pub problems: Vec<String>,
    pub rules: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CaseVerdict {
    pub id: String,
    pub baseline_outcome: String,
    pub candidate_outcome: String,
    pub calls: usize,
    pub changes: usize,
    pub differences: Vec<Difference>,
    pub unlisted: usize,
}

impl CaseVerdict {
    fn same(&self) -> bool {
        self.differences.is_empty() && self.unlisted == 0
    }

    fn passed(&self) -> bool {
        let fine = |outcome: &str| outcome == "passed" || outcome.starts_with("ignored") || outcome.starts_with("skipped");
        self.same() && fine(&self.baseline_outcome) && fine(&self.candidate_outcome)
    }
}

impl Report {
    pub(crate) fn passed(&self) -> bool {
        self.boot_differences.is_empty()
            && self.boot_unlisted == 0
            && self.problems.is_empty()
            && !self.cases.is_empty()
            && self.cases.iter().all(CaseVerdict::passed)
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let boot_same = self.boot_differences.is_empty() && self.boot_unlisted == 0;
        out.push(format!(
            "boot: {} tables, {} rows: {}",
            self.boot_tables,
            self.boot_rows,
            if boot_same { "same".to_string() } else { format!("{} differences", self.boot_differences.len() + self.boot_unlisted) }
        ));
        for difference in &self.boot_differences {
            out.push(format!("    {}", difference.what));
        }
        if self.boot_unlisted > 0 {
            out.push(format!("    ... and {} more", self.boot_unlisted));
        }
        for case in &self.cases {
            let verdict = if !case.same() {
                "DIFFERENT".to_string()
            } else if case.passed() {
                "same".to_string()
            } else {
                format!("same, but the spec failed: {}", case.baseline_outcome)
            };
            out.push(format!("{}: {verdict} ({} calls, {} row changes)", case.id, case.calls, case.changes));
            if case.baseline_outcome != case.candidate_outcome {
                out.push(format!("    outcome: baseline {}; candidate {}", case.baseline_outcome, case.candidate_outcome));
            }
            for difference in &case.differences {
                out.push(format!("    {}", difference.what));
            }
            if case.unlisted > 0 {
                out.push(format!("    ... and {} more", case.unlisted));
            }
        }
        for problem in &self.problems {
            out.push(format!("problem: {problem}"));
        }
        let same = self.cases.iter().filter(|case| case.same()).count();
        out.push(format!(
            "RESULT: {}: {} of {} cases same, boot {}; {} → {}",
            if self.passed() { "PASS" } else { "FAIL" },
            same,
            self.cases.len(),
            if boot_same { "same" } else { "different" },
            self.baseline,
            self.candidate
        ));
        out
    }

    pub(crate) fn to_json(&self) -> Value {
        let difference = |d: &Difference| json!({ "place": d.place, "what": d.what });
        json!({
            "passed": self.passed(),
            "baseline": self.baseline,
            "candidate": self.candidate,
            "rules": self.rules,
            "boot": {
                "tables": self.boot_tables, "rows": self.boot_rows,
                "differences": self.boot_differences.iter().map(difference).collect::<Vec<_>>(),
                "unlisted": self.boot_unlisted,
            },
            "cases": self.cases.iter().map(|case| json!({
                "id": case.id, "same": case.same(), "passed": case.passed(),
                "baselineOutcome": case.baseline_outcome, "candidateOutcome": case.candidate_outcome,
                "calls": case.calls, "changes": case.changes,
                "differences": case.differences.iter().map(difference).collect::<Vec<_>>(),
                "unlisted": case.unlisted,
            })).collect::<Vec<_>>(),
            "problems": self.problems,
        })
    }
}

/// Compare `a` (the baseline) with `b` (the candidate).
pub(crate) fn compare(a: &Recording, b: &Recording, rules: &Rules) -> Report {
    let (ea, eb) = (engine_of(a), engine_of(b));
    let mut report = Report {
        baseline: a.target.clone(),
        candidate: b.target.clone(),
        rules: rules.files.iter().map(|file| file.display().to_string()).collect(),
        ..Report::default()
    };
    for (label, recording) in [("baseline", a), ("candidate", b)] {
        report.problems.extend(recording.problems.iter().map(|problem| format!("{label}: {problem}")));
        // an engine that wrote elsewhere, or a snapshot that read nothing, would make every case look the same
        if recording.boot.is_empty() {
            report.problems.push(format!("{label}: the database has no tables after boot"));
        }
    }
    let (differences, unlisted, tables, rows) = compare_snapshots(&a.boot, &b.boot, ea, eb, rules);
    report.boot_differences = differences;
    report.boot_unlisted = unlisted;
    report.boot_tables = tables;
    report.boot_rows = rows;
    let ids: Vec<String> = {
        let mut ids: Vec<String> = a.cases.iter().map(CaseRecord::id).collect();
        for id in b.cases.iter().map(CaseRecord::id) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    };
    for id in ids {
        let ca = a.cases.iter().find(|case| case.id() == id);
        let cb = b.cases.iter().find(|case| case.id() == id);
        report.cases.push(compare_case(&id, ca, cb, ea, eb, rules));
    }
    report
}

fn compare_snapshots(a: &Snapshot, b: &Snapshot, ea: &str, eb: &str, rules: &Rules) -> (Vec<Difference>, usize, usize, usize) {
    let mut differences = Vec::new();
    let mut unlisted = 0;
    let empty = Rows::new();
    let mut tables: BTreeSet<&String> = BTreeSet::new();
    tables.extend(a.keys().filter(|table| !rules.skips_table(ea, table)));
    tables.extend(b.keys().filter(|table| !rules.skips_table(eb, table)));
    let mut rows = 0;
    for table in &tables {
        let (ta, tb) = (a.get(*table), b.get(*table));
        if ta.is_none() || tb.is_none() {
            differences.push(Difference {
                place: "boot".to_string(),
                what: format!("table {table}: only the {}", if ta.is_some() { "baseline" } else { "candidate" }),
            });
            continue;
        }
        let ra = rules.masked_rows(ea, table, ta.unwrap_or(&empty));
        let rb = rules.masked_rows(eb, table, tb.unwrap_or(&empty));
        rows += ra.len().max(rb.len());
        let mut listed = 0;
        for line in row_differences(table, &ra, &rb) {
            if listed < LIST_LIMIT {
                differences.push(Difference { place: "boot".to_string(), what: line });
                listed += 1;
            } else {
                unlisted += 1;
            }
        }
    }
    (differences, unlisted, tables.len(), rows)
}

/// The rows of one table that differ, as report lines.
fn row_differences(table: &str, a: &BTreeMap<String, Value>, b: &BTreeMap<String, Value>) -> Vec<String> {
    let mut keys: BTreeSet<&String> = a.keys().collect();
    keys.extend(b.keys());
    let mut out = Vec::new();
    for key in keys {
        match (a.get(key), b.get(key)) {
            (Some(_), None) => out.push(format!("{table} [{key}]: only the baseline has the row")),
            (None, Some(_)) => out.push(format!("{table} [{key}]: only the candidate has the row")),
            (Some(ra), Some(rb)) if ra != rb => out.push(format!("{table} [{key}]: {}", field_differences(ra, rb))),
            _ => {}
        }
    }
    out
}

/// `column: baseline X, candidate Y` for each field that differs.
fn field_differences(a: &Value, b: &Value) -> String {
    let (Some(ma), Some(mb)) = (a.as_object(), b.as_object()) else {
        return format!("baseline {}, candidate {}", short(a), short(b));
    };
    let mut columns: BTreeSet<&String> = ma.keys().collect();
    columns.extend(mb.keys());
    columns
        .into_iter()
        .filter(|column| ma.get(*column) != mb.get(*column))
        .map(|column| {
            let side = |map: &Map<String, Value>| map.get(column).map_or_else(|| "(no column)".to_string(), short);
            format!("{column}: baseline {}, candidate {}", side(ma), side(mb))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn short(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 120 {
        format!("{}…", text.chars().take(120).collect::<String>())
    } else {
        text
    }
}

fn masked_changes(changes: &[RowChange], engine: &str, rules: &Rules) -> BTreeMap<(String, String), (String, Option<Value>)> {
    let mut out = BTreeMap::new();
    for change in changes {
        if rules.skips_table(engine, &change.table) {
            continue;
        }
        let mask = |row: &Option<Value>| row.as_ref().and_then(|row| rules.masked(engine, &change.table, row));
        let (before, after) = (mask(&change.before), mask(&change.after));
        // an approved row of the engine, or a change only of volatile columns
        if (before.is_none() && after.is_none()) || (before.is_some() && before == after) {
            continue;
        }
        out.insert((change.table.clone(), change.key.clone()), (change.kind().to_string(), after));
    }
    out
}

fn compare_case(id: &str, a: Option<&CaseRecord>, b: Option<&CaseRecord>, ea: &str, eb: &str, rules: &Rules) -> CaseVerdict {
    let mut verdict = CaseVerdict {
        id: id.to_string(),
        baseline_outcome: a.map_or_else(|| "not run".to_string(), |case| case.outcome.clone()),
        candidate_outcome: b.map_or_else(|| "not run".to_string(), |case| case.outcome.clone()),
        ..CaseVerdict::default()
    };
    let (Some(a), Some(b)) = (a, b) else {
        verdict.differences.push(Difference {
            place: id.to_string(),
            what: format!("only the {} ran this case", if a.is_some() { "baseline" } else { "candidate" }),
        });
        return verdict;
    };
    let push = |verdict: &mut CaseVerdict, what: String| {
        if verdict.differences.len() < LIST_LIMIT {
            verdict.differences.push(Difference { place: id.to_string(), what });
        } else {
            verdict.unlisted += 1;
        }
    };
    if a.outcome != b.outcome {
        push(&mut verdict, "the spec's outcome differs".to_string());
    }
    verdict.calls = a.calls.len().max(b.calls.len());
    for index in 0..verdict.calls {
        let (ca, cb) = (a.calls.get(index), b.calls.get(index));
        if ca != cb {
            let name = ca.or(cb).and_then(|call| call.get("name")).and_then(Value::as_str).unwrap_or("?").to_string();
            let what = match (ca, cb) {
                (Some(ca), Some(cb)) => value_differences(ca, cb, ""),
                _ => format!("made only by the {}", if ca.is_some() { "baseline" } else { "candidate" }),
            };
            push(&mut verdict, format!("call {} {name}: {what}", index + 1));
        }
    }
    let (ma, mb) = (masked_changes(&a.changes, ea, rules), masked_changes(&b.changes, eb, rules));
    let mut keys: BTreeSet<&(String, String)> = ma.keys().collect();
    keys.extend(mb.keys());
    verdict.changes = keys.len();
    for key in keys {
        let (table, row) = key;
        match (ma.get(key), mb.get(key)) {
            (Some((kind, _)), None) => push(&mut verdict, format!("{table} [{row}]: {kind} only on the baseline")),
            (None, Some((kind, _))) => push(&mut verdict, format!("{table} [{row}]: {kind} only on the candidate")),
            (Some((ka, ra)), Some((kb, rb))) if ka != kb || ra != rb => {
                let what = if ka == kb {
                    field_differences(ra.as_ref().unwrap_or(&Value::Null), rb.as_ref().unwrap_or(&Value::Null))
                } else {
                    format!("baseline {ka}, candidate {kb}")
                };
                push(&mut verdict, format!("{table} [{row}] {ka}: {what}"));
            }
            _ => {}
        }
    }
    verdict
}

/// Where two JSON values differ, by path: `answer.body.convertedAmount: baseline 200.0, candidate 200`.
fn value_differences(a: &Value, b: &Value, path: &str) -> String {
    let mut out = Vec::new();
    collect_differences(a, b, path, &mut out);
    out.join("; ")
}

fn collect_differences(a: &Value, b: &Value, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(ma), Value::Object(mb)) => {
            let mut keys: BTreeSet<&String> = ma.keys().collect();
            keys.extend(mb.keys());
            for key in keys {
                let sub = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                match (ma.get(key), mb.get(key)) {
                    (Some(va), Some(vb)) => collect_differences(va, vb, &sub, out),
                    (va, vb) => out.push(format!(
                        "{sub}: baseline {}, candidate {}",
                        va.map_or_else(|| "(none)".to_string(), short),
                        vb.map_or_else(|| "(none)".to_string(), short)
                    )),
                }
            }
        }
        _ if a != b => out.push(format!("{}: baseline {}, candidate {}", if path.is_empty() { "value" } else { path }, short(a), short(b))),
        _ => {}
    }
}
