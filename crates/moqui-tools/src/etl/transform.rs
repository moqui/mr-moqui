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

//! `MappingTransformer` (plans/Tools-Database.md §10.3): a row of the source becomes an entry of the target, field by
//! field: the source column (or a constant), trimmed, its default when empty, a value map (case-insensitive), a
//! format pattern for dates and numbers, a lookup (one cached find per distinct value per run, as the user), an
//! expression (JavaScript with `row`, `value` and `ec`, compiled once per run by the script cache; only for a user with
//! `GROOVY_SHELL_WEB`), and `required` /
//! `skipIfEmpty`. For an entity target the values are then converted to the fields' types. A row with an error is
//! not loaded; each of its errors is kept with its row number and field ([`ErrorLog`]).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::etl::{EntryTransform, SharedHeaders, Transformer, ROW_NUMBER_KEY};
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use moqui_core::entity::EntityFacade;

use super::mapping::{Mapping, MappingField, TargetKind};

/// The most error rows a run keeps in `EtlRunError`; the error file has every one.
pub const MAX_ERROR_ROWS: usize = 1000;

/// One error of a row.
#[derive(Clone, Debug, PartialEq)]
pub struct RowError {
    /// The row in the source, from 1; 0 when the source has no row numbers.
    pub row_number: i64,
    /// The target field, or empty for the row as a whole (its load, its type conversion).
    pub field: String,
    pub message: String,
    /// The source row.
    pub row: BTreeMap<String, Value>,
}

/// The errors of a run: the first [`MAX_ERROR_ROWS`] kept whole, every one counted, and every failed row in CSV for
/// the error file.
#[derive(Debug, Default)]
pub struct ErrorLog {
    pub kept: Vec<RowError>,
    /// The errors, one per field (a row may have several).
    pub count: u64,
    /// The rows with an error: what a run's `errorCount` is.
    pub rows: u64,
    /// The row of the error added last; rows come in order, so a new row number is a new row.
    last_row: Option<i64>,
    /// Rows of the error file: the row number, the field, the message and the source row as JSON.
    pub csv_lines: Vec<String>,
}

impl ErrorLog {
    pub fn add(&mut self, error: RowError) {
        self.count += 1;
        if self.last_row != Some(error.row_number) {
            self.rows += 1;
            self.last_row = Some(error.row_number);
        }
        self.csv_lines.push(csv_line(&[
            &error.row_number.to_string(),
            &error.field,
            &error.message,
            &row_json(&error.row),
        ]));
        if self.kept.len() < MAX_ERROR_ROWS {
            self.kept.push(error);
        }
    }

    /// The error file's text: a header and each error.
    #[must_use]
    pub fn csv_text(&self) -> String {
        let mut text = csv_line(&["rowNumber", "field", "message", "row"]);
        for line in &self.csv_lines {
            text.push_str(line);
        }
        text
    }
}

fn csv_line(cells: &[&str]) -> String {
    let mut line = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            line.push(',');
        }
        if cell.contains([',', '"', '\n', '\r']) {
            line.push('"');
            line.push_str(&cell.replace('"', "\"\""));
            line.push('"');
        } else {
            line.push_str(cell);
        }
    }
    line.push('\n');
    line
}

/// A source row as JSON, for an error row.
#[must_use]
pub fn row_json(row: &BTreeMap<String, Value>) -> String {
    let map: IndexMap<String, Value> = row.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    moqui_lib::json::to_json(&Value::Map(map)).to_string()
}

/// The errors of a run, shared by the transformer and the loader.
pub type SharedErrors = Arc<Mutex<ErrorLog>>;

/// The index (from 0) of a column named by letters (`A`, `AB`) or a number from 1.
fn column_index(name: &str) -> Option<usize> {
    if let Ok(number) = name.parse::<usize>() {
        return number.checked_sub(1);
    }
    if name.is_empty() || name.len() > 3 || !name.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return None;
    }
    let mut index = 0usize;
    for ch in name.to_ascii_uppercase().bytes() {
        index = index * 26 + usize::from(ch - b'A' + 1);
    }
    index.checked_sub(1)
}

fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        _ => false,
    }
}

/// A Java date pattern (`yyyy-MM-dd HH:mm:ss`) as a strftime one, for the tokens a data file uses.
fn strftime_of(pattern: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        let ch = chars[at];
        let run = chars[at..].iter().take_while(|next| **next == ch).count();
        let token = match (ch, run) {
            ('y', 2) => "%y",
            ('y', _) => "%Y",
            ('M', 1 | 2) => "%m",
            ('M', 3) => "%b",
            ('M', _) => "%B",
            ('d', _) => "%d",
            ('H', _) => "%H",
            ('h', _) => "%I",
            ('m', _) => "%M",
            ('s', _) => "%S",
            ('S', _) => "%f",
            ('a', _) => "%p",
            ('E', 1..=3) => "%a",
            ('E', _) => "%A",
            _ => {
                for _ in 0..run {
                    if ch == '%' {
                        out.push_str("%%");
                    } else {
                        out.push(ch);
                    }
                }
                at += run;
                continue;
            }
        };
        out.push_str(token);
        at += run;
    }
    out
}

/// Whether `pattern` is a date or time pattern (it has a date or time letter), else a number pattern.
fn is_date_pattern(pattern: &str) -> bool {
    pattern
        .chars()
        .any(|ch| matches!(ch, 'y' | 'M' | 'd' | 'H' | 'h' | 'm' | 's'))
}

/// The decimal separator of a number pattern, as Java's `DecimalFormat` reads one: of `.` and `,`, the last that is
/// followed by digits (`0`, `#`) and is not a grouping separator. A separator is grouping when the other separator
/// comes after it, or when it is the only kind, follows a `#` and every group after it is three digits (`#,##0`,
/// `#.##0`): so `#,##0` and `#.##0` are whole numbers, `#,##0.00` and `0.000` have `.`, and `#.##0,00` has `,`.
fn decimal_separator(pattern: &str) -> Option<char> {
    let digits_after = |at: usize| {
        pattern[at + 1..]
            .chars()
            .take_while(|ch| matches!(ch, '0' | '#'))
            .count()
    };
    let (at, last) = pattern.char_indices().rfind(|(_, ch)| matches!(ch, '.' | ','))?;
    if digits_after(at) == 0 {
        return None;
    }
    let other = if last == '.' { ',' } else { '.' };
    if pattern[..at].contains(other) {
        // `#,##0.00`, `#.##0,00`: the other kind before it groups, so this one is the decimal separator
        return Some(last);
    }
    // the only kind of separator: grouping when each group after one is three digits
    let groups: Vec<usize> = pattern
        .char_indices()
        .filter(|(_, ch)| *ch == last)
        .map(|(at, _)| digits_after(at))
        .collect();
    let first = pattern.find(last).unwrap_or(at);
    let grouping = groups.iter().all(|digits| *digits == 3) && pattern[..first].ends_with('#');
    (!grouping).then_some(last)
}

/// Parse `text` by `pattern`: a date pattern gives a timestamp (in the user's zone) or, with no time letters, a date;
/// a number pattern (`#,##0`, `#,##0.00`, `#.##0,00`) gives a decimal, its grouping separators dropped
/// ([`decimal_separator`]).
///
/// # Errors
///
/// Returns an error naming the pattern when the text does not match it.
pub fn parse_formatted(text: &str, pattern: &str) -> Result<Value, String> {
    let text = text.trim();
    if is_date_pattern(pattern) {
        let format = strftime_of(pattern);
        let has_time = pattern.chars().any(|ch| matches!(ch, 'H' | 'h' | 'm' | 's'));
        if has_time {
            let civil = jiff::civil::DateTime::strptime(&format, text)
                .map_err(|err| format!("{text} does not match the date pattern {pattern}: {err}"))?;
            let zoned = civil
                .to_zoned(moqui_core::l10n::time_zone())
                .map_err(|err| format!("{text}: {err}"))?;
            return Ok(Value::Timestamp(zoned.timestamp()));
        }
        let date = jiff::civil::Date::strptime(&format, text)
            .map_err(|err| format!("{text} does not match the date pattern {pattern}: {err}"))?;
        return Ok(Value::Date(date));
    }
    let decimal = decimal_separator(pattern);
    let grouping = match decimal {
        Some('.') => ',',
        Some(',') => '.',
        // a whole number: its pattern's separator groups (`#.##0` reads `1.234`), `,` when it has none
        _ => {
            if pattern.contains('.') {
                '.'
            } else {
                ','
            }
        }
    };
    let mut cleaned: String = text
        .chars()
        .filter(|ch| *ch != grouping && !ch.is_whitespace() && !matches!(ch, '$' | '€' | '£' | '%'))
        .collect();
    if decimal == Some(',') {
        cleaned = cleaned.replace(',', ".");
    }
    cleaned
        .parse::<rust_decimal::Decimal>()
        .map(Value::Decimal)
        .map_err(|_| format!("{text} does not match the number pattern {pattern}"))
}

/// The transformer of one run.
pub struct MappingTransformer {
    facade: EntityFacade,
    fields: Vec<MappingField>,
    target_kind: TargetKind,
    target_name: String,
    headers: SharedHeaders,
    /// Each lookup field's results by its value: one find per distinct value per run.
    lookups: Mutex<HashMap<(usize, String), Option<Value>>>,
    errors: SharedErrors,
    /// The user may run scripts (`GROOVY_SHELL_WEB`), for the `expression` fields.
    scripts_allowed: bool,
}

impl MappingTransformer {
    /// The transformer of `mapping`, whose value maps are resolved, reading column letters and numbers from
    /// `headers`.
    #[must_use]
    pub fn new(facade: &EntityFacade, mapping: &Mapping, headers: SharedHeaders, errors: SharedErrors) -> Self {
        Self {
            facade: facade.clone(),
            fields: mapping.fields.clone(),
            target_kind: mapping.target_kind,
            target_name: mapping.target_name.clone(),
            headers,
            lookups: Mutex::new(HashMap::new()),
            errors,
            scripts_allowed: !mapping.has_expression() || super::access::may_run_scripts(),
        }
    }

    /// The source value of a column: by its name, ignoring case, or by its letter or number.
    fn column<'a>(&self, input: &'a BTreeMap<String, Value>, name: &str) -> Option<&'a Value> {
        if let Some(value) = input.get(name) {
            return Some(value);
        }
        if let Some((_, value)) = input.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)) {
            return Some(value);
        }
        let index = column_index(name)?;
        let headers = lock(&self.headers);
        let key = headers
            .get(index)
            .filter(|header| !header.is_empty())
            .cloned()
            .unwrap_or_else(|| moqui_lib::etl::column_number_name(index));
        input.get(&key)
    }

    fn lookup(&self, index: usize, field: &MappingField, value: &Value) -> Result<Value, String> {
        let (Some(entity), Some(match_field)) = (&field.lookup_entity_name, &field.lookup_match_field) else {
            return Ok(value.clone());
        };
        let text = to_plain_string(value);
        let key = (index, text.clone());
        if let Some(found) = lock(&self.lookups).get(&key) {
            return found
                .clone()
                .ok_or_else(|| format!("no {entity} with {match_field} {text}"));
        }
        // by name here; views (a restricted member, a renamed password alias) are refused before the run, by
        // `Mapping::validate_with`
        super::access::refuse_restricted(entity, "lookup").map_err(|err| err.to_string())?;
        if super::access::is_secret_field(match_field)
            || field
                .lookup_result_field
                .as_deref()
                .is_some_and(super::access::is_secret_field)
        {
            return Err(format!("lookup in {entity}: that field is not available"));
        }
        let result_field = field.lookup_result_field.clone();
        // as the user: a lookup reads what the user may read
        let found = {
            self.facade
                .find(entity)
                .and_then(|find| find.eq(match_field, Value::string(&text)).limit(1).list())
                .map_err(|err| format!("lookup in {entity}: {err}"))?
                .iter()
                .next()
                .map(|row| match &result_field {
                    Some(result) => row.get(result).cloned().unwrap_or(Value::Null),
                    None => {
                        // the record's single primary key field
                        let pk = self
                            .facade
                            .with_entity(entity, |definition, _, _| {
                                Ok(definition.pk_field_names().first().map(|name| (*name).to_string()))
                            })
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        row.get(&pk).cloned().unwrap_or(Value::Null)
                    }
                })
        };
        lock(&self.lookups).insert(key, found.clone());
        found.ok_or_else(|| format!("no {entity} with {match_field} {text}"))
    }

    /// The value of one field for a row: `None` to leave the field out.
    fn field_value(
        &self,
        index: usize,
        field: &MappingField,
        input: &BTreeMap<String, Value>,
    ) -> Result<Option<Value>, String> {
        let mut value = if let Some(constant) = &field.constant_value {
            Value::string(constant)
        } else {
            let column = field.source_column.as_deref().unwrap_or(&field.target_field);
            self.column(input, column).cloned().unwrap_or(Value::Null)
        };
        if field.trim {
            if let Value::String(text) = &value {
                value = Value::string(text.trim());
            }
        }
        if is_empty_value(&value) {
            if let Some(default) = &field.default_value {
                value = Value::string(default);
            }
        }
        if !is_empty_value(&value) {
            if let Some(map) = &field.value_map {
                let from = to_plain_string(&value);
                match map.get(&from.trim().to_lowercase()) {
                    Some(to) => value = Value::string(to),
                    None => return Err(format!("no value map entry for {from}")),
                }
            }
            if let Some(pattern) = &field.format_pattern {
                if let Value::String(text) = &value {
                    value = parse_formatted(text, pattern)?;
                }
            }
            if field.lookup_entity_name.is_some() {
                value = self.lookup(index, field, &value)?;
            }
        }
        if let Some(expression) = &field.expression {
            if !self.scripts_allowed {
                return Err(super::access::SCRIPTS_REFUSED.to_string());
            }
            let mut entries = IndexMap::new();
            entries.insert(
                "row".to_string(),
                Value::Map(input.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            );
            entries.insert("value".to_string(), value.clone());
            let _frame = moqui_core::script::ContextFrame::push_entries(entries);
            value = moqui_core::script::eval_value(expression).map_err(|err| format!("expression: {err}"))?;
        }
        if is_empty_value(&value) {
            if field.required {
                return Err("is required".to_string());
            }
            return Ok((!field.skip_if_empty).then_some(Value::Null));
        }
        Ok(Some(value))
    }

    /// The target values of one source row, or its errors.
    ///
    /// # Errors
    ///
    /// Each field that failed, with its message.
    pub fn transform_row(
        &self,
        input: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, Vec<(String, String)>> {
        let mut out = BTreeMap::new();
        let mut failed = Vec::new();
        for (index, field) in self.fields.iter().enumerate() {
            match self.field_value(index, field, input) {
                Ok(Some(value)) => {
                    out.insert(field.target_field.clone(), value);
                }
                Ok(None) => {}
                Err(message) => failed.push((field.target_field.clone(), message)),
            }
        }
        if !failed.is_empty() {
            return Err(failed);
        }
        if self.target_kind == TargetKind::Entity {
            let mut fields = moqui_core::entity::field_map(out);
            if let Err(err) =
                moqui_core::entity::load_feed::convert_entity_fields(&self.facade, &self.target_name, &mut fields)
            {
                return Err(vec![(String::new(), err.to_string())]);
            }
            out = fields
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect();
        }
        Ok(out)
    }
}

/// The row number an extractor put on an entry.
#[must_use]
pub fn row_number_of(values: &BTreeMap<String, Value>) -> i64 {
    match values.get(ROW_NUMBER_KEY) {
        Some(Value::I64(number)) => *number,
        Some(Value::I32(number)) => i64::from(*number),
        _ => 0,
    }
}

impl Transformer for MappingTransformer {
    fn transform(&self, transform: &mut EntryTransform) -> Result<(), Error> {
        let row_number = row_number_of(&transform.entry().values);
        let mut input = transform.entry().values.clone();
        input.remove(ROW_NUMBER_KEY);
        match self.transform_row(&input) {
            Ok(mut out) => {
                out.insert(ROW_NUMBER_KEY.to_string(), Value::I64(row_number));
                let entry = transform.entry_mut();
                entry.etl_type.clone_from(&self.target_name);
                entry.values = out;
                Ok(())
            }
            Err(failed) => {
                let mut errors = lock(&self.errors);
                let summary = failed
                    .iter()
                    .map(|(field, message)| format!("{field} {message}").trim().to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                for (field, message) in failed {
                    errors.add(RowError {
                        row_number,
                        field,
                        message,
                        row: input.clone(),
                    });
                }
                Err(Error::new(format!("row {row_number}: {summary}")))
            }
        }
    }
}
