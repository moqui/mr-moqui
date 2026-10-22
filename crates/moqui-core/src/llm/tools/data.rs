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

//! `data` (plans/LLM-A2A-Assist.md 7.3 and 7.7, open question 3): rows without opening a screen, three ways.
//!
//! - A screen list (`path`, `list`): the screen rendered in the `tui` mode as the user, fenced with its writes held
//!   ([`super::apps::render`]), so the apps' permissions and entity filters apply; the rows of its form-list as the
//!   user sees them. `where` becomes the list's find parameters (`field`, `field_op`, `field_from`, `field_thru`,
//!   `field_not`), `order` its `orderByField`, `limit` and `offset` its `pageSize` and `pageIndex` (a list that does
//!   not page is cut here).
//! - An entity of the conf's `data-entity` patterns (`entity`; by default the `moqui.basic` package: enumerations,
//!   status items, units, geos), read directly with authorization off as Java's `find_basic`, encrypted fields never.
//! - A spilled result of the conversation (`artifact`): its lines from `offset`, or the lines that hold `grep`; a text
//!   file the user attached to the conversation (`F12`) the same way.
//!
//! The answer is a compact pipe table headed `(showing R of C)`, empty cells for nulls, columns empty on every row left
//! out, at most 8 columns unless `fields` names them, `limit` rows (25 by default or the model's `list-limit`, at most
//! 100), then a `rows link to …` line, the dates the phrases resolved to and a `more:` line.

use std::collections::BTreeMap;

use jiff::Zoned;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::Value as Json;

use super::apps;
use super::dates;
use super::format::{self, ToolError};
use super::schema::{object, Property};
use super::spill;
use super::suggest;
use super::{AgentTool, Classification, ToolOutcome, ToolSpec};
use crate::entity::Condition;
use crate::llm::run::context::TurnContext;

/// Rows when neither the call nor the model says.
pub const DEFAULT_LIMIT: usize = 25;
pub const MAX_LIMIT: usize = 100;
/// Columns shown when `fields` names none.
const MAX_COLUMNS: usize = 8;

pub struct Data(ToolSpec);

impl Default for Data {
    fn default() -> Self {
        let mut schema = object(&[
            Property::optional("path", "string", "A screen path with a list"),
            Property::optional("list", "string", "Its list, when several"),
            Property::optional("entity", "string", "A basic table, e.g. moqui.basic.StatusItem"),
            Property::optional("where", "object", "Field values to match; skill data-reads"),
            Property::optional("fields", "array", "Columns to show"),
            Property::optional("order", "string", "Sort field, -field descending"),
            Property::optional("limit", "integer", "Rows, 25 default, 100 max"),
            Property::optional("offset", "integer", "Rows to skip"),
            Property::optional("artifact", "string", "An earlier large result"),
            Property::optional("grep", "string", "Artifact lines with this text"),
        ]);
        schema["properties"]["fields"]["items"] = serde_json::json!({"type": "string"});
        Self(ToolSpec::new(
            "data",
            "Read rows of a screen's list without opening it, a basic table, or an earlier large result. Answers a table (showing R of C).",
            schema,
            Classification::Read,
        ))
    }
}

impl AgentTool for Data {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, ctx: &mut TurnContext<'_>, args: Json) -> ToolOutcome {
        let text = |key: &str| {
            args.get(key)
                .and_then(Json::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let number = |key: &str| {
            args.get(key)
                .and_then(Json::as_u64)
                .and_then(|n| usize::try_from(n).ok())
        };
        let limit = number("limit")
            .filter(|limit| *limit > 0)
            .unwrap_or(if ctx.spill.list_limit > 0 {
                ctx.spill.list_limit
            } else {
                DEFAULT_LIMIT
            })
            .min(MAX_LIMIT);
        let offset = number("offset").unwrap_or(0);
        let fields: Vec<String> = args
            .get("fields")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(|field| field.trim().to_string())
            .filter(|field| !field.is_empty())
            .collect();
        let empty = serde_json::Map::new();
        let filters = args.get("where").and_then(Json::as_object).unwrap_or(&empty);
        let read = Read {
            filters,
            fields: &fields,
            order: text("order"),
            limit,
            offset,
        };
        let outcome = if let Some(artifact) = text("artifact") {
            read_artifact(ctx, &artifact, offset, limit, text("grep").as_deref())
        } else if let Some(entity) = text("entity") {
            read_entity(ctx, &entity, &read)
        } else if let Some(path) = text("path") {
            read_list(ctx, &path, text("list").as_deref(), &read)
        } else {
            Err(ToolError::new(
                "nothing_to_read",
                "Call data with path (and list) for a list on a screen, entity for a basic table, or artifact for an earlier large result.",
            )
            .skill("data-reads"))
        };
        match outcome {
            Ok(text) => ToolOutcome::text(text),
            Err(error) => error.outcome(),
        }
    }
}

/// What a read asks for, besides where it reads.
struct Read<'a> {
    filters: &'a serde_json::Map<String, Json>,
    fields: &'a [String],
    order: Option<String>,
    limit: usize,
    offset: usize,
}

// ---- artifacts ----

fn read_artifact(
    ctx: &TurnContext<'_>,
    artifact: &str,
    offset: usize,
    limit: usize,
    grep: Option<&str>,
) -> Result<String, ToolError> {
    let missing = || {
        ToolError::new(
            "no_artifact",
            format!("There is no artifact {artifact} in this conversation. Use the name an artifact line gave, such as A12."),
        )
    };
    let Some(run) = ctx.run else {
        return Err(missing());
    };
    let facade = facade()?;
    // a file the user attached (F12) is read the same way, by lines
    if artifact.trim().starts_with(['F', 'f']) {
        let text = crate::llm::run::quietly(|| {
            crate::llm::assist::files::read_text(&facade, &run.conversation_id, artifact)
        })
        .map_err(|error| ToolError::new("not_text", error.message().to_string()))?
        .ok_or_else(missing)?;
        // the file's text is the user's file, or someone else's they attached: data, never instructions
        let lines = spill::read_lines(&text, artifact.trim(), offset, limit, grep);
        return Ok(crate::llm::prompt::untrusted("attachment", &lines));
    }
    let text = crate::llm::run::quietly(|| spill::read(&facade, &run.conversation_id, artifact))
        .map_err(|error| ToolError::new("read_failed", format!("The artifact could not be read: {error}")))?
        .ok_or_else(missing)?;
    Ok(spill::read_lines(
        &text,
        &spill::artifact_id(spill::tool_call_of(artifact)),
        offset,
        limit,
        grep,
    ))
}

fn facade() -> Result<crate::entity::EntityFacade, ToolError> {
    apps::factory()
        .ok()
        .and_then(|factory| factory.entity())
        .ok_or_else(|| ToolError::new("unavailable", "The database is not available now. Tell the user."))
}

// ---- where ----

/// One condition of `where`: the field, how it compares and the value as text (a date phrase resolved).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Test {
    Equals(String),
    Not(String),
    From(String),
    Thru(String),
    After(String),
    Before(String),
    Contains(String),
    Begins(String),
    In(Vec<String>),
    Empty,
}

/// The tests of `where`, and the date phrases they resolved, by field.
fn tests_of(filters: &serde_json::Map<String, Json>, now: &Zoned) -> (Vec<(String, Test)>, BTreeMap<String, String>) {
    let mut tests = Vec::new();
    let mut resolved = BTreeMap::new();
    let week = jiff::civil::Weekday::Monday;
    let mut date = |field: &str, text: &str, end: bool| -> String {
        if !dates::is_date_name(field) {
            return text.to_string();
        }
        match dates::resolve(text, now, week, end || dates::is_end_name(field)) {
            Some(value) => {
                resolved.insert(field.to_string(), value.clone());
                value
            }
            None => text.to_string(),
        }
    };
    for (field, value) in filters {
        let test = match value {
            Json::Null => Test::Empty,
            Json::Array(items) => Test::In(items.iter().map(plain).collect()),
            Json::Object(range) => {
                for (key, end) in [("from", false), ("thru", true)] {
                    if let Some(bound) = range.get(key) {
                        let bound = date(field, &plain(bound), end);
                        tests.push((field.clone(), if end { Test::Thru(bound) } else { Test::From(bound) }));
                    }
                }
                continue;
            }
            other => {
                let text = plain(other);
                let text = text.trim();
                if let Some(rest) = text.strip_prefix(">=") {
                    Test::From(date(field, rest.trim(), false))
                } else if let Some(rest) = text.strip_prefix("<=") {
                    Test::Thru(date(field, rest.trim(), true))
                } else if let Some(rest) = text.strip_prefix("!=") {
                    Test::Not(date(field, rest.trim(), false))
                } else if let Some(rest) = text.strip_prefix('>') {
                    Test::After(date(field, rest.trim(), true))
                } else if let Some(rest) = text.strip_prefix('<') {
                    Test::Before(date(field, rest.trim(), false))
                } else if text.len() > 1 && text.starts_with('%') && text.ends_with('%') {
                    Test::Contains(text.trim_matches('%').to_string())
                } else if text.len() > 1 && text.ends_with('%') && !text[..text.len() - 1].contains('%') {
                    Test::Begins(text.trim_end_matches('%').to_string())
                } else {
                    Test::Equals(date(field, text, false))
                }
            }
        };
        tests.push((field.clone(), test));
    }
    (tests, resolved)
}

fn plain(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

/// The find parameters of a screen list for the tests: `field`, `field_op`, `field_not`, `field_from`, `field_thru`.
fn find_parameters(tests: &[(String, Test)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (field, test) in tests {
        match test {
            Test::Equals(value) => out.push((field.clone(), value.clone())),
            Test::Not(value) => {
                out.push((field.clone(), value.clone()));
                out.push((format!("{field}_not"), "Y".to_string()));
            }
            Test::From(value) | Test::After(value) => out.push((format!("{field}_from"), value.clone())),
            Test::Thru(value) | Test::Before(value) => out.push((format!("{field}_thru"), value.clone())),
            Test::Contains(value) => {
                out.push((field.clone(), value.clone()));
                out.push((format!("{field}_op"), "contains".to_string()));
            }
            Test::Begins(value) => {
                out.push((field.clone(), value.clone()));
                out.push((format!("{field}_op"), "begins".to_string()));
            }
            Test::In(values) => {
                out.push((field.clone(), values.join(",")));
                out.push((format!("{field}_op"), "in".to_string()));
            }
            Test::Empty => out.push((format!("{field}_op"), "empty".to_string())),
        }
    }
    out
}

/// The user's time now in their time zone (UTC when they have none).
pub(crate) fn user_now(ctx: &TurnContext<'_>) -> Zoned {
    let zone = facade()
        .ok()
        .and_then(|facade| {
            crate::llm::run::quietly(|| {
                facade
                    .find("moqui.security.UserAccount")
                    .and_then(|find| find.eq("userId", Value::string(&ctx.user.user_id)).one())
                    .ok()
                    .flatten()
            })
        })
        .and_then(|user| crate::llm::text_of(&user, "timeZone"))
        .and_then(|zone| jiff::tz::TimeZone::get(&zone).ok())
        .unwrap_or(jiff::tz::TimeZone::UTC);
    jiff::Timestamp::now().to_zoned(zone)
}

fn resolved_line(resolved: &BTreeMap<String, String>) -> Option<String> {
    (!resolved.is_empty()).then(|| {
        let pairs: Vec<String> = resolved
            .iter()
            .map(|(field, value)| format!("{field}: \"{value}\""))
            .collect();
        format!("resolved: {{{}}}", pairs.join(", "))
    })
}

// ---- tables ----

/// A table to answer: its title, columns and rows of text.
struct Rows {
    title: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    total: usize,
}

/// The columns to show: those `fields` names (matched by name or title, case and spaces ignored) when it names any,
/// else the first [`MAX_COLUMNS`] with a value on some row. Out: the column indexes and the names that matched none.
fn pick_columns(columns: &[String], rows: &[Vec<String>], fields: &[String]) -> (Vec<usize>, Vec<String>) {
    let squash = |text: &str| -> String {
        text.chars()
            .filter(|ch| ch.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    if !fields.is_empty() {
        let mut picked = Vec::new();
        let mut missing = Vec::new();
        for field in fields {
            match columns.iter().position(|column| squash(column) == squash(field)) {
                Some(at) if !picked.contains(&at) => picked.push(at),
                Some(_) => {}
                None => missing.push(field.clone()),
            }
        }
        if !picked.is_empty() {
            return (picked, missing);
        }
        let near = missing.clone();
        return (default_columns(columns, rows), near);
    }
    (default_columns(columns, rows), Vec::new())
}

fn default_columns(columns: &[String], rows: &[Vec<String>]) -> Vec<usize> {
    (0..columns.len())
        .filter(|at| rows.iter().any(|row| row.get(*at).is_some_and(|cell| !cell.is_empty())))
        .take(MAX_COLUMNS)
        .collect()
}

/// The answer: the title line with `(showing R of C, order X)`, the table, the lines after it.
fn answer(rows: &Rows, read: &Read<'_>, after: &[String]) -> String {
    let (picked, missing) = pick_columns(&rows.columns, &rows.rows, read.fields);
    let header: Vec<String> = picked.iter().map(|at| rows.columns[*at].clone()).collect();
    let body: Vec<Vec<String>> = rows
        .rows
        .iter()
        .map(|row| {
            picked
                .iter()
                .map(|at| row.get(*at).cloned().unwrap_or_default())
                .collect()
        })
        .collect();
    let order = read
        .order
        .as_ref()
        .map(|order| format!(", order {order}"))
        .unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();
    if rows.rows.is_empty() && read.offset > 0 {
        // past the end: a paged list's count is not known there (it counts the pages skipped)
        lines.push(format!("{} (no rows after {}{order})", rows.title, read.offset));
    } else {
        lines.push(format!(
            "{} (showing {} of {}{order})",
            rows.title,
            rows.rows.len(),
            rows.total
        ));
        if rows.rows.is_empty() {
            lines.push("no rows".to_string());
        } else {
            lines.push(format::table(&header, &body));
        }
    }
    if !missing.is_empty() {
        let names: Vec<&str> = rows.columns.iter().map(String::as_str).collect();
        let mut near: Vec<String> = Vec::new();
        for field in &missing {
            near.extend(suggest::nearest(field, names.iter().copied(), 1));
        }
        lines.push(format!(
            "ignored fields: {}{}",
            missing.join(", "),
            if near.is_empty() {
                format!(" (columns: {})", names.join(", "))
            } else {
                format!(" (did you mean: {})", near.join(", "))
            }
        ));
    }
    lines.extend(after.iter().cloned());
    let next = read.offset + rows.rows.len();
    if next < rows.total && !rows.rows.is_empty() {
        lines.push(format!("more: data offset={next}"));
    }
    lines.join("\n")
}

// ---- entities ----

fn read_entity(ctx: &TurnContext<'_>, entity: &str, read: &Read<'_>) -> Result<String, ToolError> {
    let factory = apps::factory().map_err(|_| ToolError::new("unavailable", "The database is not available now."))?;
    let facade = facade()?;
    let conf = crate::llm::facade(&factory);
    let readable: Vec<String> = crate::entity::info::all_entity_names(&facade, "")
        .into_iter()
        .filter(|name| {
            let (package, short) = name.rsplit_once('.').unwrap_or(("", name));
            conf.conf().data_entity_readable(package, short)
        })
        .collect();
    let short_names: Vec<&str> = readable.iter().map(String::as_str).collect();
    let Some(definition) = crate::entity::info::definition_info(&facade, entity) else {
        // a short name of a readable entity is taken
        if let Some(full) = readable.iter().find(|name| name.rsplit('.').next() == Some(entity)) {
            return read_entity(ctx, full, read);
        }
        return Err(ToolError::new(
            "unknown_entity",
            format!("There is no entity {entity}. Use a full name such as moqui.basic.Enumeration, or read a list on a screen with path."),
        )
        .did_you_mean(suggest::nearest(entity, short_names.iter().copied(), 3)));
    };
    let text_of = |key: &str| definition.get(key).map(to_plain_string).unwrap_or_default();
    let full = text_of("fullEntityName");
    if !conf
        .conf()
        .data_entity_readable(&text_of("packageName"), &text_of("entityName"))
    {
        return Err(ToolError::new(
            "entity_not_readable",
            format!("{full} is not a table data reads directly. Read it through a list on a screen: search for the screen, then data with path."),
        )
        .skill("data-reads"));
    }
    let mut all_fields: Vec<(String, bool, String)> = Vec::new();
    if let Some(Value::List(fields)) = definition.get("fields") {
        for field in fields {
            if let Value::Map(field) = field {
                let name = field.get("name").map(to_plain_string).unwrap_or_default();
                let encrypted = matches!(field.get("encrypt"), Some(Value::Bool(true)));
                let kind = field.get("type").map(to_plain_string).unwrap_or_default();
                all_fields.push((name, encrypted, kind));
            }
        }
    }
    let secrets = conf
        .conf()
        .data_entity_secrets(&text_of("packageName"), &text_of("entityName"));
    let names: Vec<&str> = all_fields
        .iter()
        .filter(|(name, encrypted, _)| !encrypted && (secrets || !secret_name(name)))
        .map(|(name, _, _)| name.as_str())
        .collect();
    let unknown = |field: &str| {
        ToolError::new(
            "unknown_field",
            format!("{full} has no field {field} that may be read. Use one of its fields."),
        )
        .did_you_mean(suggest::nearest(field, names.iter().copied(), 3))
    };
    let now = user_now(ctx);
    let (tests, resolved) = tests_of(read.filters, &now);
    let mut conditions = Vec::new();
    for (field, test) in &tests {
        if !names.contains(&field.as_str()) {
            return Err(unknown(field));
        }
        let value = Value::string;
        conditions.push(match test {
            Test::Equals(text) => Condition::equals(field.as_str(), value(text)),
            Test::Not(text) => Condition::not_equals(field.as_str(), value(text)),
            Test::From(text) => Condition::greater_equal(field.as_str(), value(text)),
            Test::Thru(text) => Condition::less_equal(field.as_str(), value(text)),
            Test::After(text) => Condition::greater(field.as_str(), value(text)),
            Test::Before(text) => Condition::less(field.as_str(), value(text)),
            Test::Contains(text) => {
                Condition::ignore_case(Condition::like(field.as_str(), value(&format!("%{text}%"))))
            }
            Test::Begins(text) => Condition::ignore_case(Condition::like(field.as_str(), value(&format!("{text}%")))),
            Test::In(values) => Condition::is_in(field.as_str(), values.iter().map(value).collect()),
            Test::Empty => Condition::is_null(field.as_str()),
        });
    }
    let mut order: Vec<(String, bool)> = Vec::new();
    if let Some(text) = &read.order {
        for part in text.split(',').map(str::trim).filter(|part| !part.is_empty()) {
            let (field, descending) = match part.strip_prefix('-') {
                Some(field) => (field.trim(), true),
                None => (part.trim_start_matches('+'), false),
            };
            let field = field.split_whitespace().next().unwrap_or(field);
            let descending = descending || part.to_lowercase().ends_with(" desc");
            if !names.contains(&field) {
                return Err(unknown(field));
            }
            order.push((field.to_string(), descending));
        }
    }
    let shown: Vec<String> = if read.fields.is_empty() {
        names
            .iter()
            .filter(|name| !matches!(**name, "lastUpdatedStamp"))
            .map(|name| (*name).to_string())
            .collect()
    } else {
        for field in read.fields {
            if !names.contains(&field.as_str()) {
                return Err(unknown(field));
            }
        }
        read.fields.to_vec()
    };
    let find = || -> Result<crate::entity::EntityFind, moqui_lib::error::Error> {
        let mut find = facade.find(&full)?.disable_authz();
        for condition in &conditions {
            find = find.condition(condition.clone());
        }
        Ok(find)
    };
    let failed = |error: moqui_lib::error::Error| {
        ToolError::new(
            "read_failed",
            format!("{full} could not be read: {error}. Check the where values."),
        )
        .skill("data-reads")
    };
    let total = find().and_then(|find| find.count()).map_err(failed)?;
    let mut list = find().map_err(failed)?;
    for (field, descending) in &order {
        list = list.order_by(field, *descending);
    }
    let rows = list
        .offset(i64::try_from(read.offset).unwrap_or(0))
        .limit(i64::try_from(read.limit).unwrap_or(25))
        .list()
        .map_err(failed)?;
    let rows: Vec<Vec<String>> = rows
        .values()
        .iter()
        .map(|row| {
            shown
                .iter()
                .map(|field| {
                    row.get(field)
                        .filter(|value| !value.is_null())
                        .map(to_plain_string)
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();
    let table = Rows {
        title: full.clone(),
        columns: shown,
        rows,
        total: usize::try_from(total).unwrap_or(0),
    };
    let after: Vec<String> = resolved_line(&resolved).into_iter().collect();
    Ok(answer(&table, read, &after))
}

// ---- screen lists ----

fn read_list(ctx: &TurnContext<'_>, path: &str, list: Option<&str>, read: &Read<'_>) -> Result<String, ToolError> {
    let path = apps::app_path(path);
    let (screen, query) = path.split_once('?').unwrap_or((path.as_str(), ""));
    // a screen only: the screen tool's check refuses other hosts, `..`, transitions and files
    let factory = apps::factory().map_err(|_| ToolError::new("unavailable", "The screens are not available now."))?;
    if let Err(problem) = crate::llm::screen::check_screen_path(&factory, &path) {
        let entries = apps::shell_entries(ctx);
        let paths: Vec<&str> = entries.screens.iter().map(|entry| entry.path.as_str()).collect();
        return Err(
            ToolError::new("no_screen", format!("data reads a list on a screen, and {problem}."))
                .did_you_mean(suggest::nearest(screen, paths.iter().copied(), 3)),
        );
    }
    let mut parameters: BTreeMap<String, Value> = BTreeMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        parameters.insert(
            crate::web::percent_decode(key),
            Value::string(crate::web::percent_decode(value)),
        );
    }
    let now = user_now(ctx);
    let (tests, resolved) = tests_of(read.filters, &now);
    for (key, value) in find_parameters(&tests) {
        parameters.insert(key, Value::string(value));
    }
    let aligned = read.offset.is_multiple_of(read.limit);
    let page_size = if aligned { read.limit } else { read.offset + read.limit };
    parameters.insert("pageSize".to_string(), Value::string(page_size.to_string()));
    parameters.insert(
        "pageIndex".to_string(),
        Value::string(if aligned { read.offset / read.limit } else { 0 }.to_string()),
    );
    if let Some(order) = &read.order {
        parameters.insert("orderByField".to_string(), Value::string(order));
    }
    let output = match apps::render(ctx, screen, parameters, Some("tui")) {
        Ok(output) => output,
        Err(error) => return Err(render_error(ctx, screen, &error)),
    };
    if output.status != 200 {
        let error = crate::context::Error::new(output.body_text());
        return Err(match output.status {
            401 | 403 => not_permitted(screen),
            404 => no_screen(ctx, screen),
            _ => ToolError::new(
                "render_failed",
                format!(
                    "{screen} answered {}: {}",
                    output.status,
                    format::cut(error.message(), 200)
                ),
            ),
        });
    }
    let document: Json = serde_json::from_slice(&output.body)
        .map_err(|error| ToolError::new("render_failed", format!("{screen} did not render as a screen: {error}")))?;
    let lists = lists_of(&document);
    let names: Vec<&str> = lists.iter().map(|list| list.name.as_str()).collect();
    let chosen = match list {
        Some(wanted) => lists
            .iter()
            .find(|list| list.name.eq_ignore_ascii_case(wanted) || list.id.eq_ignore_ascii_case(wanted))
            .ok_or_else(|| {
                ToolError::new(
                    "no_list",
                    format!("{screen} has no list named {wanted}. Call data again with one of its lists."),
                )
                .did_you_mean(if names.is_empty() {
                    Vec::new()
                } else {
                    let near = suggest::nearest(wanted, names.iter().copied(), 3);
                    if near.is_empty() {
                        names.iter().map(|name| (*name).to_string()).collect()
                    } else {
                        near
                    }
                })
            })?,
        None => {
            match lists.as_slice() {
                [] => {
                    return Err(ToolError::new(
                    "no_list",
                    format!("{screen} has no list. Open it with screen to read it, or search for the screen with the list."),
                ));
                }
                [first, ..] => first,
            }
        }
    };
    let mut after: Vec<String> = Vec::new();
    if list.is_none() && lists.len() > 1 {
        after.push(format!("other lists: {} (data list=name)", names[1..].join(", ")));
    }
    let errors = document
        .get("messages")
        .and_then(|messages| messages.get("errors"))
        .and_then(Json::as_array);
    for message in errors.into_iter().flatten().filter_map(Json::as_str).take(3) {
        after.push(format!("message: {}", format::cut(message, 200)));
    }
    let mut columns: Vec<String> = chosen.columns.clone();
    let total_rows = chosen.count.max(chosen.rows.len());
    // a list that does not page is cut here; one that pages answered the page asked for, from its start when the
    // offset is not a multiple of the limit
    // offset; a list that holds all its rows (its count is what it answered) is cut here too, so an offset past its end
    // reads none rather than its first rows again
    let whole = chosen.count <= chosen.rows.len();
    let skip = if !aligned || chosen.rows.len() > read.limit || whole {
        read.offset
    } else {
        0
    };
    let kept: Vec<&ListRow> = if read.offset >= total_rows && read.offset > 0 {
        Vec::new()
    } else {
        chosen.rows.iter().skip(skip).take(read.limit).collect()
    };
    let mut rows: Vec<Vec<String>> = kept.iter().map(|row| row.cells.clone()).collect();
    // the link of the rows: one url with the row's id in it, the same for every row
    let mut template: Option<String> = None;
    let mut ids: Vec<String> = Vec::new();
    for row in &kept {
        let made = match (&row.link, row.id.is_empty()) {
            (Some(link), false) => link_template(link, &row.id),
            _ => None,
        };
        match (&template, made) {
            (_, None) => {
                template = None;
                ids.clear();
                break;
            }
            (None, Some(made)) => template = Some(made),
            (Some(known), Some(made)) if *known != made => {
                template = None;
                ids.clear();
                break;
            }
            _ => {}
        }
        ids.push(row.id.clone());
    }
    if let Some((template, parameter)) = template.as_ref().and_then(|template| {
        let parameter = template.split('<').nth(1)?.split('>').next()?.to_string();
        Some((template.clone(), parameter))
    }) {
        let shown = rows.iter().zip(&ids).all(|(row, id)| row.iter().any(|cell| cell == id));
        if !shown {
            columns.insert(0, parameter.clone());
            for (row, id) in rows.iter_mut().zip(&ids) {
                row.insert(0, id.clone());
            }
        }
        after.insert(0, format!("rows link to {template}"));
    }
    // the field names a canvas Query's rows are keyed by (skill openui-lang#query), where the drawer can show a canvas
    if ctx.caps.has(super::caps::UI_OPENUI) && !chosen.keys.is_empty() {
        after.push(format!("row keys: {}", chosen.keys.join(", ")));
    }
    if let Some(line) = resolved_line(&resolved) {
        after.push(line);
    }
    let table = Rows {
        title: format!("{} on {screen}", chosen.name),
        columns,
        rows,
        total: total_rows,
    };
    Ok(answer(&table, read, &after))
}

/// A form-list of a `tui` render, as `data` reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ListData {
    id: String,
    name: String,
    columns: Vec<String>,
    rows: Vec<ListRow>,
    /// The rows of the whole list, as its paging says.
    count: usize,
    /// The field names its columns show, in order: the keys of a canvas read's rows.
    keys: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ListRow {
    id: String,
    /// Each cell's text: a stacked column's parts joined with ` / `.
    cells: Vec<String>,
    /// The row's first link.
    link: Option<String>,
}

/// The form-lists of a `tui` document (`{"t": "formList", …}` nodes wherever they are), in order.
fn lists_of(document: &Json) -> Vec<ListData> {
    fn walk(value: &Json, out: &mut Vec<ListData>) {
        match value {
            Json::Array(items) => items.iter().for_each(|item| walk(item, out)),
            Json::Object(map) => {
                if map.get("t").and_then(Json::as_str) == Some("formList") {
                    out.push(list_of(value));
                }
                for (key, item) in map {
                    if key != "rows" {
                        walk(item, out);
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(document.get("c").unwrap_or(document), &mut out);
    out
}

fn list_of(node: &Json) -> ListData {
    let text = |value: &Json, key: &str| value.get(key).and_then(Json::as_str).unwrap_or_default().to_string();
    let rows: Vec<ListRow> = node
        .get("rows")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .map(|row| {
            let cells: Vec<&Json> = row
                .get("cells")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .collect();
            ListRow {
                id: text(row, "id"),
                cells: cells.iter().map(|cell| cell_text(cell)).collect(),
                link: cells
                    .iter()
                    .find_map(|cell| cell_link(cell))
                    .map(|url| apps::app_path(&url)),
            }
        })
        .collect();
    ListData {
        id: text(node, "id"),
        name: text(node, "name"),
        columns: node
            .get("columns")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .map(|column| text(column, "title"))
            .collect(),
        keys: node
            .get("columns")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .flat_map(|column| column.get("fields").and_then(Json::as_array).cloned().unwrap_or_default())
            .filter_map(|field| field.as_str().map(str::to_string))
            .collect(),
        count: node
            .get("paging")
            .and_then(|paging| paging.get("count"))
            .and_then(Json::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(rows.len()),
        rows,
    }
}

/// A cell's text as the user reads it: plain text, a link's or a text node's text, a field's value, a stack's parts.
fn cell_text(cell: &Json) -> String {
    match cell {
        Json::String(text) => text.trim().to_string(),
        Json::Array(parts) => parts
            .iter()
            .map(cell_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" / "),
        Json::Object(node) => {
            let text = node.get("text").and_then(Json::as_str);
            let value = node
                .get("w")
                .and_then(|widget| widget.get("text").or_else(|| widget.get("value")))
                .and_then(Json::as_str);
            text.or(value).unwrap_or_default().trim().to_string()
        }
        _ => String::new(),
    }
}

fn cell_link(cell: &Json) -> Option<String> {
    match cell {
        Json::Array(parts) => parts.iter().find_map(cell_link),
        Json::Object(node) if node.get("t").and_then(Json::as_str) == Some("link") => node
            .get("url")
            .and_then(Json::as_str)
            .filter(|url| !url.is_empty() && node.get("disabled").and_then(Json::as_bool) != Some(true))
            .map(str::to_string),
        _ => None,
    }
}

/// Whether a field's name says it holds a secret: a password, token, secret, credential or key (`apiKey`,
/// `privateKey`), which `data` leaves out of a directly read entity unless its `data-entity` allows them.
pub fn secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["password", "passwd", "token", "secret", "credential", "salt"]
        .iter()
        .any(|word| lower.contains(word))
        || lower.ends_with("key")
}

/// `url` with the query value that is `id` written `<name>`: `…?userId=<userId>`.
fn link_template(url: &str, id: &str) -> Option<String> {
    let (path, query) = url.split_once('?')?;
    let mut found = false;
    let pairs: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) if !found && crate::web::percent_decode(value) == id => {
                found = true;
                format!("{name}=<{name}>")
            }
            _ => pair.to_string(),
        })
        .collect();
    found.then(|| format!("{path}?{}", pairs.join("&")))
}

fn not_permitted(screen: &str) -> ToolError {
    ToolError::new(
        "not_permitted",
        format!("The user may not view {screen}. That is the answer about permission; tell the user."),
    )
}

/// A path that is not a screen: the nearest screens of the user's menus.
fn no_screen(ctx: &TurnContext<'_>, screen: &str) -> ToolError {
    let entries = apps::shell_entries(ctx);
    let paths: Vec<&str> = entries.screens.iter().map(|entry| entry.path.as_str()).collect();
    ToolError::new(
        "no_screen",
        format!("There is no screen at {screen}. Use a path that search or a screen returned."),
    )
    .did_you_mean(suggest::nearest(screen, paths.iter().copied(), 3))
}

fn render_error(ctx: &TurnContext<'_>, screen: &str, error: &crate::context::Error) -> ToolError {
    use crate::context::ErrorKind;
    match error.kind() {
        ErrorKind::AuthenticationRequired | ErrorKind::Unauthorized | ErrorKind::Tarpit => not_permitted(screen),
        ErrorKind::NotFound => no_screen(ctx, screen),
        _ => ToolError::new(
            "render_failed",
            format!("{screen} could not be read: {}", format::cut(error.message(), 300)),
        ),
    }
}

crate::submit_tool!(Data);

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn where_values_become_find_parameters_with_dates_resolved() {
        let now: Zoned = "2026-10-21T14:30:00[UTC]".parse().unwrap();
        let filters = json!({"statusId": ["Open", "Held"], "username": "john%", "name": "%acme%",
            "thruDate": "end of last month", "fromDate": ">=2026-01-01", "partyId": "!=X", "note": null,
            "entryDate": {"from": "start of this week", "thru": "today"}});
        let (tests, resolved) = tests_of(filters.as_object().unwrap(), &now);
        let parameters = find_parameters(&tests);
        let has = |key: &str, value: &str| parameters.iter().any(|(k, v)| k == key && v == value);
        assert!(
            has("statusId", "Open,Held") && has("statusId_op", "in"),
            "{parameters:?}"
        );
        assert!(has("username", "john") && has("username_op", "begins"));
        assert!(has("name", "acme") && has("name_op", "contains"));
        assert!(has("thruDate", "2026-09-30 23:59:59"));
        assert!(has("fromDate_from", "2026-01-01"));
        assert!(has("partyId", "X") && has("partyId_not", "Y"));
        assert!(has("note_op", "empty"));
        assert!(has("entryDate_from", "2026-10-19 00:00") && has("entryDate_thru", "2026-10-21 23:59:59"));
        assert_eq!(
            resolved.get("thruDate").map(String::as_str),
            Some("2026-09-30 23:59:59")
        );
        assert_eq!(
            resolved_line(&BTreeMap::from([(
                "thruDate".to_string(),
                "2026-09-30 23:59:59".to_string()
            )]))
            .unwrap(),
            "resolved: {thruDate: \"2026-09-30 23:59:59\"}"
        );
    }

    #[test]
    fn fields_named_like_secrets_are_known() {
        for name in [
            "currentPassword",
            "accessToken",
            "clientSecret",
            "apiKey",
            "privateKey",
            "passwordSalt",
        ] {
            assert!(secret_name(name), "{name}");
        }
        for name in ["enumId", "description", "keyword", "statusId"] {
            assert!(!secret_name(name), "{name}");
        }
    }

    #[test]
    fn a_row_link_with_the_rows_id_is_a_template() {
        assert_eq!(
            link_template("/apps/system/User/Detail?userId=EX_JOHN&tab=a", "EX_JOHN").as_deref(),
            Some("/apps/system/User/Detail?userId=<userId>&tab=a")
        );
        assert!(link_template("/apps/system/User/Detail?userId=EX_JOHN", "OTHER").is_none());
        assert!(link_template("/apps/system/User/Detail", "EX_JOHN").is_none());
    }

    #[test]
    fn columns_are_picked_by_name_or_title_and_empty_ones_left_out() {
        let columns: Vec<String> = ["User Id", "Email Address", "Note"]
            .iter()
            .map(|c| (*c).to_string())
            .collect();
        let rows = vec![
            vec!["a".to_string(), "a@x".to_string(), String::new()],
            vec!["b".to_string(), String::new(), String::new()],
        ];
        assert_eq!(pick_columns(&columns, &rows, &[]).0, [0, 1]);
        let (picked, missing) = pick_columns(&columns, &rows, &["emailAddress".to_string(), "bogus".to_string()]);
        assert_eq!(picked, [1]);
        assert_eq!(missing, ["bogus"]);
    }
}
