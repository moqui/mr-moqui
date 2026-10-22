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

//! The one read proxy of the canvases (plans/LLM-A2A-Assist.md 4.10, J12): every read a canvas makes (an OpenUI
//! Query, a Lookup's options, a Display's value, later a Vue SFC's reads) is `GET /assist/read`, never a request of the
//! page to a screen. The read renders an `/apps` path in-process as the user from the agent's origin, under the read
//! fence with every write transition held, as the agent's own `data` reads do: a transition that writes answers 403 and
//! runs nothing, and a read that tries to write fails at the fence. It reads as the user ∩ the agent: the agent's role
//! groups must permit the screen or the transition, and the user must be allowed it
//! ([`super::write::check_read`]). Reads are counted per user and node (at most [`READS_PER_MINUTE`]).
//!
//! A screen answers the rows of one of its lists (`list`, else the first) from its `tui` render, each row an object
//! keyed by the names of the fields its columns show (`ScreenTui.js` gives each column its `fields`), with `_id` (the
//! row's id) and `_link` (its first link, as an `/apps` path): `{rows, totalCount, list, lists, columns: [{key,
//! title}]}`. A transition (a lookup that answers options, a value) answers its own JSON.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::object::Value;
use serde_json::{json, Map, Value as Json};

use crate::context::ExecutionContextFactory;
use crate::screen::{make_render, RenderOptions, RenderOrigin, RenderOutput, TransitionGate};

/// Reads a user may make on a node in a minute.
pub const READS_PER_MINUTE: u32 = 240;
/// Rows a list read answers when it does not say, and at most.
pub const DEFAULT_ROWS: usize = 100;
pub const MAX_ROWS: usize = 500;

/// Why a read did not answer: an HTTP status and the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError {
    pub status: u16,
    pub message: String,
}

impl ReadError {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// A read: the path (`/apps/…`, a `/qapps/…` one is read as its twin), the list of a screen, and the request
/// parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadRequest {
    pub path: String,
    pub list: Option<String>,
    pub parameters: BTreeMap<String, String>,
}

struct Counts(Mutex<HashMap<String, (Instant, u32)>>);

/// Count one read of `user_id`: false when the user is past [`READS_PER_MINUTE`] in this minute.
fn counted(factory: &ExecutionContextFactory, user_id: &str) -> bool {
    let counts = factory.extension(|_| Counts(Mutex::new(HashMap::new())));
    let mut map = counts.0.lock().unwrap_or_else(|err| err.into_inner());
    let now = Instant::now();
    if map.len() > 10_000 {
        map.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
    }
    let entry = map.entry(user_id.to_string()).or_insert((now, 0));
    if now.duration_since(entry.0) >= Duration::from_secs(60) {
        *entry = (now, 0);
    }
    entry.1 += 1;
    entry.1 <= READS_PER_MINUTE
}

/// Read `request` as the current user (`user_id`) from the agent `agent_id`'s origin.
///
/// # Errors
///
/// 400 for a path that is not a plain path of the apps, 403 for a transition that writes or a screen the user may not
/// open, 404 for none, 429 past the rate, 502 for an answer that is not JSON.
pub fn read(
    factory: &Arc<ExecutionContextFactory>,
    user_id: &str,
    agent: &crate::llm::run::AgentDef,
    request: &ReadRequest,
) -> Result<Json, ReadError> {
    let agent_id = agent.agent_id.as_str();
    if !counted(factory, user_id) {
        return Err(ReadError::new(429, format!("at most {READS_PER_MINUTE} canvas reads a minute")));
    }
    let path = crate::llm::tools::apps::app_path(&request.path);
    let (bare, query) = path.split_once('?').unwrap_or((path.as_str(), ""));
    if let Some(problem) = crate::llm::screen::path_problem(bare) {
        return Err(ReadError::new(400, format!("{bare} is not a path a canvas may read: {problem}")));
    }
    if !bare.starts_with("/apps/") {
        return Err(ReadError::new(400, format!("{bare} is not a screen of the apps; a canvas reads /apps paths")));
    }
    let root = crate::llm::screen::root_screen(factory);
    let hit = crate::screen::locate(factory, &root, bare)
        .map_err(|_| ReadError::new(404, format!("there is no screen at {bare}")))?;
    super::write::check_read(factory, &agent.role_groups(), bare).map_err(|why| ReadError::new(403, why))?;
    let mut parameters: BTreeMap<String, Value> = BTreeMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        parameters.insert(crate::web::percent_decode(key), Value::string(crate::web::percent_decode(value)));
    }
    for (key, value) in &request.parameters {
        parameters.insert(key.clone(), Value::string(value));
    }
    let transition = hit.transition.is_some();
    if !transition {
        let rows = request
            .parameters
            .get("pageSize")
            .and_then(|size| size.parse::<usize>().ok())
            .unwrap_or(DEFAULT_ROWS)
            .clamp(1, MAX_ROWS);
        parameters.insert("pageSize".to_string(), Value::string(rows.to_string()));
    }
    let output = render(factory, agent_id, bare, parameters, (!transition).then_some("tui"))
        .map_err(|error| match crate::context::ErrorKind::of(&error) {
            crate::context::ErrorKind::NotFound => ReadError::new(404, format!("there is no screen at {bare}")),
            crate::context::ErrorKind::Unauthorized | crate::context::ErrorKind::AuthenticationRequired => {
                ReadError::new(403, format!("the user may not read {bare}"))
            }
            _ => ReadError::new(500, format!("{bare} could not be read: {}", error.message())),
        })?;
    match output.status {
        200 => {}
        409 => {
            return Err(ReadError::new(
                403,
                format!("{bare} writes, and a canvas only reads; a write is a Mutation the user's click runs"),
            ));
        }
        401 | 403 => return Err(ReadError::new(403, format!("the user may not read {bare}"))),
        404 => return Err(ReadError::new(404, format!("there is no screen at {bare}"))),
        status => {
            return Err(ReadError::new(
                502,
                format!("{bare} answered {status}: {}", crate::llm::tools::format::cut(&output.body_text(), 200)),
            ));
        }
    }
    let body: Json = serde_json::from_slice(&output.body).map_err(|_| {
        ReadError::new(502, format!("{bare} did not answer JSON; a canvas reads lists of screens and transitions that answer JSON"))
    })?;
    if transition {
        return Ok(body);
    }
    let lists = lists_of(&body);
    let names: Vec<String> = lists.iter().map(|list| list["name"].as_str().unwrap_or_default().to_string()).collect();
    let chosen = match &request.list {
        Some(wanted) => lists.iter().find(|list| {
            list["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case(wanted))
                || list["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(wanted))
        }),
        None => lists.first(),
    };
    let Some(chosen) = chosen else {
        return Err(ReadError::new(
            404,
            match &request.list {
                Some(wanted) if !names.is_empty() => format!("{bare} has no list {wanted}; its lists are {}", names.join(", ")),
                _ => format!("{bare} has no list"),
            },
        ));
    };
    Ok(rows_of(chosen, &names))
}

/// Render `path` as the current user from the agent's origin: fenced, every write held.
fn render(
    factory: &Arc<ExecutionContextFactory>,
    agent_id: &str,
    path: &str,
    parameters: BTreeMap<String, Value>,
    mode: Option<&str>,
) -> Result<RenderOutput, crate::context::Error> {
    let root = crate::llm::screen::root_screen(factory);
    let mut render = make_render(factory)
        .root_screen(root)
        .screen_path(path)
        .parameters(parameters)
        .options(RenderOptions {
            origin: RenderOrigin::Agent {
                agent_id: agent_id.to_string(),
                run_id: None,
            },
            gate: TransitionGate::Hold,
            write_fence: true,
        });
    if let Some(mode) = mode {
        render = render.render_mode(mode);
    }
    render.render()
}

/// The form-list nodes of a `tui` document, in order.
fn lists_of(document: &Json) -> Vec<Json> {
    fn walk(value: &Json, out: &mut Vec<Json>) {
        match value {
            Json::Array(items) => items.iter().for_each(|item| walk(item, out)),
            Json::Object(map) => {
                if map.get("t").and_then(Json::as_str) == Some("formList") {
                    out.push(value.clone());
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

/// A cell's text as the user reads it.
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
        Json::Number(number) => number.to_string(),
        Json::Bool(flag) => flag.to_string(),
        Json::Null => String::new(),
    }
}

fn cell_link(cell: &Json) -> Option<String> {
    match cell {
        Json::Array(parts) => parts.iter().find_map(cell_link),
        Json::Object(node) if node.get("t").and_then(Json::as_str) == Some("link") => node
            .get("url")
            .and_then(Json::as_str)
            .filter(|url| !url.is_empty() && node.get("disabled").and_then(Json::as_bool) != Some(true))
            .map(crate::llm::tools::apps::app_path),
        _ => None,
    }
}

/// A key of a column with no field names: its title in lower camel case (`Hit Count` → `hitCount`).
pub fn title_key(title: &str) -> String {
    let mut out = String::new();
    for (index, word) in title.split(|ch: char| !ch.is_alphanumeric()).filter(|word| !word.is_empty()).enumerate() {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            if index == 0 {
                out.extend(first.to_lowercase());
            } else {
                out.extend(first.to_uppercase());
            }
            out.push_str(chars.as_str());
        }
    }
    out
}

/// The answer of a list: its rows keyed by field name.
pub fn rows_of(list: &Json, names: &[String]) -> Json {
    let columns: Vec<(Vec<String>, String)> = list["columns"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|column| {
            let title = column["title"].as_str().unwrap_or_default().to_string();
            let mut fields: Vec<String> = column["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect();
            if fields.is_empty() {
                fields.push(title_key(&title));
            }
            (fields, title)
        })
        .collect();
    let mut rows: Vec<Json> = Vec::new();
    for row in list["rows"].as_array().into_iter().flatten() {
        let mut out = Map::new();
        if let Some(id) = row["id"].as_str().filter(|id| !id.is_empty()) {
            out.insert("_id".to_string(), json!(id));
        }
        let cells: Vec<&Json> = row["cells"].as_array().into_iter().flatten().collect();
        for (index, cell) in cells.iter().enumerate() {
            let Some((fields, _)) = columns.get(index) else {
                continue;
            };
            match cell {
                Json::Array(parts) if parts.len() == fields.len() && fields.len() > 1 => {
                    for (field, part) in fields.iter().zip(parts) {
                        out.insert(field.clone(), json!(cell_text(part)));
                    }
                }
                _ => {
                    out.insert(fields[0].clone(), json!(cell_text(cell)));
                    for field in &fields[1..] {
                        out.entry(field.clone()).or_insert(json!(""));
                    }
                }
            }
            if !out.contains_key("_link") {
                if let Some(link) = cell_link(cell) {
                    out.insert("_link".to_string(), json!(link));
                }
            }
        }
        rows.push(Json::Object(out));
    }
    let total = list["paging"]["count"].as_u64().unwrap_or(rows.len() as u64);
    json!({
        "rows": rows,
        "totalCount": total,
        "list": list["name"],
        "lists": names,
        "columns": columns.iter().flat_map(|(fields, title)| {
            let titles: Vec<&str> = title.split(" / ").collect();
            fields.iter().enumerate().map(move |(at, key)| {
                json!({"key": key, "title": if titles.len() == fields.len() { titles[at] } else { title.as_str() }})
            })
        }).collect::<Vec<_>>(),
    })
}

/// The parameters of a read from the drawer's call: `query` as JSON text of flat values, beside `path` and `list`.
pub fn request_of(path: &str, list: Option<String>, query: Option<&Json>) -> ReadRequest {
    let mut parameters = BTreeMap::new();
    if let Some(Json::Object(map)) = query {
        for (key, value) in map.iter().take(50) {
            let text = match value {
                Json::Null => continue,
                Json::String(text) => text.clone(),
                Json::Array(items) => items
                    .iter()
                    .map(|item| item.as_str().map_or_else(|| item.to_string(), str::to_string))
                    .collect::<Vec<_>>()
                    .join(","),
                other => other.to_string(),
            };
            if !text.is_empty() {
                parameters.insert(key.clone(), text);
            }
        }
    }
    ReadRequest {
        path: path.to_string(),
        list,
        parameters,
    }
}
