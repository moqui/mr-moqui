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

//! The JSON request surface. One request object in, one response object out.
//!
//! The first cut is JSON for every call; the binary forms in `plans/proposals/Messaging.md` replace it where
//! they exist. A response is `{"ok":true,"result":...}` or `{"ok":false,"error":"..."}`.
//!
//! | `op` | Fields | Result |
//! |---|---|---|
//! | `init` | `runtime`, `framework`, `db` (default `:memory:`) | `null` |
//! | `checkTables` | `entities` (empty or absent: all) | `null` |
//! | `loadData` | `location`, `types` | records written |
//! | `callService` | `name`, `parameters` | out parameters |
//! | `render` | `root`, `path`, `mode` (default `text`), `parameters` | `{status, body}` |
//! | `find` | `entity`, `equals`, `orderBy`, `limit` | list of records |
//! | `store` | `entity`, `fields` | `null` |
//! | `wsDeliver` | `conn`, `text`: one inbound text frame | `{kind: "notification", body}` or `{kind: "text", text}` |
//! | `wsSend` | `conn`, `text`: queue an outbound text frame | `null` |
//!
//! Binary frames and the outbound drain are byte exports, not JSON (`src/exports.rs`, [`crate::ws`]).
//!
//! The state is per thread: the browser module has one.

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;

use moqui_core::entity::{EntityList, EntityValue};
use moqui_lib::error::Error;
use moqui_lib::json::{from_json, to_json};
use moqui_lib::object::Value;
use serde_json::{json, Map, Value as Json};

use crate::host::Browser;

thread_local! {
    static BROWSER: RefCell<Option<Browser>> = const { RefCell::new(None) };
}

/// Handle one JSON request and return the JSON response text.
///
/// A panic is caught here, since one that unwinds into an `extern "C"` export aborts the module. The response is
/// `{"ok":false,"error":"panic: ..."}`, and the browser state, which the panic may have left half-updated, is dropped,
/// so the next request must `init` again.
pub fn handle(request: &str) -> String {
    guarded(|| {
        let response = match serde_json::from_str::<Json>(request) {
            Ok(Json::Object(fields)) => match dispatch(&fields) {
                Ok(result) => json!({ "ok": true, "result": result }),
                Err(err) => json!({ "ok": false, "error": error_text(&err) }),
            },
            Ok(_) => json!({ "ok": false, "error": "the request is not a JSON object" }),
            Err(err) => json!({ "ok": false, "error": format!("the request is not JSON: {err}") }),
        };
        response.to_string()
    })
}

/// `body`'s response, or the `panic: ...` error response when it panics, after the browser state is dropped.
fn guarded(body: impl FnOnce() -> String) -> String {
    let payload = match catch_unwind(AssertUnwindSafe(body)) {
        Ok(response) => return response,
        Err(payload) => payload,
    };
    let message = panic_text(payload.as_ref());
    tracing::error!(panic = %message, "a browser request panicked; the state is reset");
    // Dropping a half-updated host can panic in turn; it is then leaked rather than let unwind.
    let dropped = catch_unwind(|| BROWSER.with(|slot| slot.borrow_mut().take()));
    match dropped {
        Ok(browser) => {
            if catch_unwind(AssertUnwindSafe(move || drop(browser))).is_err() {
                tracing::error!("the browser state panicked while it was dropped");
            }
        }
        Err(_) => tracing::error!("the browser state could not be taken after a panic"),
    }
    json!({ "ok": false, "error": format!("panic: {message}; the state was reset, run init again") }).to_string()
}

fn panic_text(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a non-text panic payload".to_string()
    }
}

/// [`handle`] with `op` set, for the per-operation exports.
pub fn handle_op(op: &str, request: &str) -> String {
    let mut fields = match serde_json::from_str::<Json>(request) {
        Ok(Json::Object(fields)) => fields,
        Ok(Json::Null) | Err(_) if request.trim().is_empty() => Map::new(),
        Ok(_) => return json!({ "ok": false, "error": "the request is not a JSON object" }).to_string(),
        Err(err) => return json!({ "ok": false, "error": format!("the request is not JSON: {err}") }).to_string(),
    };
    fields.insert("op".to_string(), Json::String(op.to_string()));
    handle(&Json::Object(fields).to_string())
}

fn dispatch(fields: &Map<String, Json>) -> Result<Json, Error> {
    let op = text(fields, "op")?;
    if op == "init" {
        let runtime = text(fields, "runtime")?;
        let framework = text(fields, "framework")?;
        let db = optional_text(fields, "db").unwrap_or(":memory:");
        let browser = Browser::open(Path::new(runtime), Path::new(framework), db)?;
        BROWSER.with(|slot| *slot.borrow_mut() = Some(browser));
        return Ok(Json::Null);
    }
    BROWSER.with(|slot| {
        let slot = slot.borrow();
        let browser = slot.as_ref().ok_or_else(|| Error::new("init has not run"))?;
        match op {
            "checkTables" => {
                let names = strings(fields, "entities");
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                browser.check_tables(&refs)?;
                Ok(Json::Null)
            }
            "loadData" => {
                let records = browser.load_data(text(fields, "location")?, &strings(fields, "types"))?;
                Ok(Json::from(records))
            }
            "callService" => {
                let out = browser.call_service(text(fields, "name")?, &map(fields, "parameters"))?;
                Ok(map_json(&out))
            }
            "render" => {
                let (status, body) = browser.render(
                    text(fields, "root")?,
                    text(fields, "path")?,
                    optional_text(fields, "mode").unwrap_or("text"),
                    map(fields, "parameters"),
                )?;
                Ok(json!({ "status": status, "body": body }))
            }
            "find" => {
                let limit = fields.get("limit").and_then(Json::as_i64);
                let rows = browser.find(
                    text(fields, "entity")?,
                    &map(fields, "equals"),
                    &strings(fields, "orderBy"),
                    limit,
                )?;
                Ok(Json::Array(rows.iter().map(map_json).collect()))
            }
            "store" => {
                browser.store(text(fields, "entity")?, &map(fields, "fields"))?;
                Ok(Json::Null)
            }
            "wsDeliver" => {
                let conn = text(fields, "conn")?;
                let frame = text(fields, "text")?;
                // Local listeners may read or write entities, so the frame is one request.
                match browser.request(true, || crate::ws::deliver_text(conn, frame))? {
                    crate::ws::Delivered::Notification(body) => Ok(json!({ "kind": "notification", "body": body })),
                    crate::ws::Delivered::Text(text) => Ok(json!({ "kind": "text", "text": text })),
                }
            }
            "wsSend" => {
                crate::ws::send_text(text(fields, "conn")?, text(fields, "text")?)?;
                Ok(Json::Null)
            }
            other => Err(Error::new(format!("unknown op {other:?}"))),
        }
    })
}

fn text<'a>(fields: &'a Map<String, Json>, name: &str) -> Result<&'a str, Error> {
    optional_text(fields, name).ok_or_else(|| Error::new(format!("{name} is required")))
}

fn optional_text<'a>(fields: &'a Map<String, Json>, name: &str) -> Option<&'a str> {
    fields.get(name).and_then(Json::as_str)
}

fn strings(fields: &Map<String, Json>, name: &str) -> Vec<String> {
    fields
        .get(name)
        .and_then(Json::as_array)
        .map(|items| items.iter().filter_map(Json::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn map(fields: &Map<String, Json>, name: &str) -> BTreeMap<String, Value> {
    match fields.get(name) {
        Some(Json::Object(entries)) => entries
            .iter()
            .map(|(key, item)| (key.clone(), from_json(item)))
            .collect(),
        _ => BTreeMap::new(),
    }
}

fn map_json(values: &BTreeMap<String, Value>) -> Json {
    Json::Object(
        values
            .iter()
            .map(|(key, value)| (key.clone(), to_json(&plain(value))))
            .collect(),
    )
}

/// Entity lists and values a service returns are host objects; JSON needs their fields.
fn plain(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            if let Some(rows) = moqui_core::script::script_list_values(value) {
                return Value::List(rows.iter().map(entity_map).collect());
            }
            if let Some(list) = object.downcast::<EntityList>() {
                return Value::List(list.iter().map(entity_map).collect());
            }
            // A script's entity value: `Mutex<EntityValue>` in the script runtime.
            if let Some(cell) = object.downcast::<Mutex<EntityValue>>() {
                let entity = cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                return entity_map(&entity);
            }
            if let Some(entity) = object.downcast::<EntityValue>() {
                return entity_map(&entity);
            }
            Value::Null
        }
        Value::List(items) => Value::List(items.iter().map(plain).collect()),
        Value::Map(entries) => Value::Map(entries.iter().map(|(key, item)| (key.clone(), plain(item))).collect()),
        other => other.clone(),
    }
}

fn entity_map(entity: &EntityValue) -> Value {
    Value::Map(
        entity
            .fields()
            .iter()
            .map(|(key, item)| (key.to_string(), plain(item)))
            .collect(),
    )
}

/// The error and its causes, one per line.
fn error_text(err: &Error) -> String {
    let mut out = err.to_string();
    let mut current = std::error::Error::source(err);
    while let Some(cause) = current {
        out.push('\n');
        out.push_str(&cause.to_string());
        current = cause.source();
    }
    out
}

#[cfg(test)]
mod tests;
