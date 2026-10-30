//! Browser RUM and session replay (plans/Monitoring.md, Later: Browser RUM). A signed-in page posts OTLP/JSON spans
//! and logs, and with `replay-enabled` masked rrweb event chunks, to `/rum/traces`, `/rum/logs` and `/rum/replay`
//! (the webroot's `rum.xml`), whose transitions have checked its session and CSRF token. Here each is held to `<telemetry><browser>`:
//! on, within its body limit and its session's rate; the page's identity is replaced with the session's
//! (`moqui_core::telemetry::browser::scrub`); and a replay is held to its time and byte caps. Replays are read back
//! only by a user with `MONITOR_REPLAY`, and each read is logged.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;

use base64::Engine;
use moqui_core::context::ExecutionContext;
use moqui_core::entity::document::Document;
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_core::search::{self, telemetry::REPLAYS, Query, SearchRequest, SortField, Target};
use moqui_core::telemetry::browser::{scrub, BrowserIdentity};
use moqui_core::telemetry::ingest::{Decoder, Signal};
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::Value as Json;

const BROWSER: &str = "telemetry/browser";
/// Spans or log records one post may carry.
const MAX_RECORDS: usize = 512;
/// Replays whose budget is held; more evict the oldest.
const MAX_REPLAYS: usize = 4096;

pub fn register(registry: &mut NativeRegistry) -> Result<(), moqui_core::context::Error> {
    for (name, function) in [
        ("receive_browser", receive_browser as moqui_core::native::NativeService),
        ("receive_replay", receive_replay),
        ("list_replays", list_replays),
        ("get_replay", get_replay),
        ("browser_settings", browser_settings),
    ] {
        registry.service_function(&format!("moqui_core::monitor::{name}"), function)?;
    }
    Ok(())
}

/// A whole-number parameter, as a number or its text.
fn whole(value: &Value) -> Option<i64> {
    moqui_lib::object::to_plain_string(value).parse().ok()
}

fn text<'a>(p: &'a Parameters, key: &str) -> &'a str {
    p.get(key).and_then(Value::as_str).unwrap_or("")
}

fn setting(ec: &ExecutionContext, name: &str) -> String {
    ec.factory().conf_setting(BROWSER, name)
}

/// The settings a page reads to decide what to send: none unless RUM is on and someone is signed in.
pub fn page_settings(ec: &ExecutionContext) -> Option<Json> {
    if setting(ec, "enabled") != "true" || signed_in().is_err() {
        return None;
    }
    let ratio = |name: &str, default: f64| {
        setting(ec, name)
            .parse::<f64>()
            .ok()
            .filter(|ratio| (0.0..=1.0).contains(ratio))
            .unwrap_or(default)
    };
    let replay = setting(ec, "replay-enabled") == "true";
    Some(serde_json::json!({
        "sampleRatio": ratio("sample-ratio", 1.0),
        "errorSampleRatio": ratio("error-sample-ratio", 1.0),
        "replay": replay,
        "replaySampleRatio": if replay { ratio("replay-sample-ratio", 0.1) } else { 0.0 },
        "replayMaxMinutes": ec.factory().conf_number(BROWSER, "replay-max-minutes", 30_u64),
        "replayMaxBytes": ec.factory().conf_number(BROWSER, "replay-max-bytes", 5_242_880_u64),
        "replayBlockSelector": setting(ec, "replay-block-selector"),
        "replayUnmaskSelector": setting(ec, "replay-unmask-selector"),
    }))
}

/// [`page_settings`] as a service, for the qvt shell: `settings` is empty when the page sends nothing.
fn browser_settings(ec: &ExecutionContext, _: &ServiceInfo<'_>, _: &Parameters) -> Result<Parameters, Error> {
    let settings = page_settings(ec).map_or(Value::Null, |json| moqui_lib::json::from_json(&json));
    Ok(Parameters::from_iter([("settings".to_string(), settings)]))
}

fn signed_in() -> Result<String, Error> {
    moqui_core::user::current_user_id()?
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::new("browser telemetry requires a signed-in session"))
}

/// The session's user and visit, a body within the limit, and a request within the session's rate.
fn admit(ec: &ExecutionContext, body: &str) -> Result<(String, String), Error> {
    if setting(ec, "enabled") != "true" {
        return Err(Error::new("browser telemetry is off"));
    }
    let user = signed_in()?;
    let visit = moqui_core::user::visit_id()?.unwrap_or_default();
    let limit = ec.factory().conf_number(BROWSER, "body-limit", 262_144_usize);
    if body.len() > limit {
        return Err(Error::new("browser telemetry body exceeds the configured limit"));
    }
    let max = ec.factory().conf_number(BROWSER, "requests-per-minute", 120_usize);
    if !ec
        .factory()
        .extension(|_| SessionRate::default())
        .allow(&format!("{user}/{visit}"), max)
    {
        return Err(Error::new("browser telemetry rate limit exceeded"));
    }
    Ok((user, visit))
}

/// Posts per minute per user and visit, for at most 4096 a minute.
#[derive(Default)]
struct SessionRate(Mutex<HashMap<String, (i64, usize)>>);

impl SessionRate {
    fn allow(&self, key: &str, max: usize) -> bool {
        let minute = moqui_core::user::now_millis() / 60_000;
        let mut entries = moqui_lib::sync::lock(&self.0);
        entries.retain(|_, (at, _)| *at == minute);
        if entries.len() >= 4096 && !entries.contains_key(key) {
            return false;
        }
        let entry = entries.entry(key.into()).or_insert((minute, 0));
        entry.1 = entry.1.saturating_add(1);
        entry.1 <= max
    }
}

fn service_name(ec: &ExecutionContext) -> String {
    match setting(ec, "service-name") {
        name if !name.is_empty() => name,
        _ => format!(
            "{}-browser",
            moqui_core::telemetry::for_factory(ec.factory()).resource().service_name
        ),
    }
}

/// `signal` (traces or logs) and the page's OTLP/JSON `bodyText`. Answers the records refused.
fn receive_browser(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let body = text(p, "bodyText");
    let (user, visit) = admit(ec, body)?;
    let signal = match text(p, "signal") {
        "traces" => Signal::Traces,
        "logs" => Signal::Logs,
        other => {
            return Err(Error::new(format!(
                "browser telemetry takes traces or logs, not {other}"
            )))
        }
    };
    let mut json: Json =
        serde_json::from_str(body).map_err(|error| Error::new(format!("invalid OTLP JSON: {error}")))?;
    let service = service_name(ec);
    let identity = BrowserIdentity {
        service_name: &service,
        user_id: &user,
        visit_id: &visit,
    };
    let refused = scrub(signal, &mut json, &identity, MAX_RECORDS);
    let telemetry = moqui_core::telemetry::for_factory(ec.factory());
    if !telemetry.accepting() {
        return Err(Error::new("telemetry is disabled"));
    }
    let mut decoded = ec
        .factory()
        .extension(|_| Decoder::default())
        .decode_value(signal, &json, "browser", "browser");
    for document in &mut decoded.documents {
        document.insert("user.id".into(), Value::string(&user));
        if !visit.is_empty() {
            document.insert("moqui.visit.id".into(), Value::string(&visit));
        }
    }
    let rejected = decoded.rejected + refused;
    telemetry.note_dropped(rejected);
    telemetry.try_ingest(decoded.index, decoded.documents)?;
    Ok(Parameters::from_iter([(
        "rejected".to_string(),
        Value::I64(i64::try_from(rejected).unwrap_or(i64::MAX)),
    )]))
}

/// The time and bytes each replay has used, and whose it is.
#[derive(Default)]
struct ReplayBudget(Mutex<HashMap<String, Spent>>);

struct Spent {
    owner: String,
    first: i64,
    bytes: u64,
}

impl ReplayBudget {
    /// Charge `bytes` to `replay` for `owner`: refused for another owner, past `max_minutes` since its first chunk,
    /// or past `max_bytes` in all.
    fn charge(&self, replay: &str, owner: &str, bytes: u64, max_minutes: i64, max_bytes: u64) -> Result<(), Error> {
        let now = moqui_core::user::now_millis();
        let mut spent = moqui_lib::sync::lock(&self.0);
        if spent.len() >= MAX_REPLAYS && !spent.contains_key(replay) {
            let oldest = spent
                .iter()
                .min_by_key(|(_, item)| item.first)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                spent.remove(&oldest);
            }
        }
        let item = spent.entry(replay.to_string()).or_insert_with(|| Spent {
            owner: owner.to_string(),
            first: now,
            bytes: 0,
        });
        if item.owner != owner {
            return Err(Error::new("this replay belongs to another session"));
        }
        if now - item.first > max_minutes.saturating_mul(60_000) {
            return Err(Error::new("the replay reached its time limit"));
        }
        if item.bytes.saturating_add(bytes) > max_bytes {
            return Err(Error::new("the replay reached its size limit"));
        }
        item.bytes += bytes;
        Ok(())
    }
}

fn valid_replay_id(id: &str) -> bool {
    (16..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The page's address without its query or fragment, which can carry what the page was given.
fn page_path(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or("").chars().take(2048).collect()
}

/// One chunk of a replay: `bodyText` is `{replayId, seq, start, end, url, traceIds, events}`, `events` the rrweb
/// events the page recorded, already masked there.
fn receive_replay(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let body = text(p, "bodyText");
    let (user, visit) = admit(ec, body)?;
    if setting(ec, "replay-enabled") != "true" {
        return Err(Error::new("session replay is off"));
    }
    let chunk: Json =
        serde_json::from_str(body).map_err(|error| Error::new(format!("invalid replay chunk: {error}")))?;
    let replay = chunk["replayId"].as_str().unwrap_or("");
    if !valid_replay_id(replay) {
        return Err(Error::new(
            "a replay chunk needs a replayId of 16 to 64 letters, digits and dashes",
        ));
    }
    let seq = chunk["seq"]
        .as_u64()
        .filter(|seq| *seq < 1_000_000)
        .ok_or_else(|| Error::new("a replay chunk needs its seq"))?;
    let Some(events) = chunk["events"].as_array() else {
        return Err(Error::new("a replay chunk needs its events"));
    };
    let events = serde_json::to_vec(events).map_err(|error| Error::new(error.to_string()))?;
    ec.factory().extension(|_| ReplayBudget::default()).charge(
        replay,
        &format!("{user}/{visit}"),
        events.len() as u64,
        ec.factory().conf_number(BROWSER, "replay-max-minutes", 30_i64),
        ec.factory().conf_number(BROWSER, "replay-max-bytes", 5_242_880_u64),
    )?;
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&events).map_err(|error| Error::new(error.to_string()))?;
    let packed = gzip.finish().map_err(|error| Error::new(error.to_string()))?;
    let now = moqui_core::user::now_millis();
    let start = chunk["start"].as_i64().unwrap_or(now);
    let traces: Vec<Value> = chunk["traceIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .filter(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .take(200)
        .map(Value::string)
        .collect();
    let mut document = Document::from_iter([
        ("_id".into(), Value::String(format!("{replay}-{seq:06}"))),
        ("replay_id".into(), Value::string(replay)),
        ("seq".into(), Value::I64(seq as i64)),
        ("start".into(), Value::I64(start)),
        ("end".into(), Value::I64(chunk["end"].as_i64().unwrap_or(now))),
        ("user.id".into(), Value::string(&user)),
        (
            "url".into(),
            Value::String(page_path(chunk["url"].as_str().unwrap_or(""))),
        ),
        ("trace_ids".into(), Value::List(traces)),
        ("bytes".into(), Value::I64(events.len() as i64)),
        (
            "events".into(),
            Value::String(base64::engine::general_purpose::STANDARD.encode(packed)),
        ),
        ("service.name".into(), Value::String(service_name(ec))),
    ]);
    if !visit.is_empty() {
        document.insert("moqui.visit.id".into(), Value::string(&visit));
    }
    let telemetry = moqui_core::telemetry::for_factory(ec.factory());
    telemetry.try_ingest(REPLAYS, vec![document])?;
    Ok(Parameters::new())
}

fn require_replay() -> Result<String, Error> {
    let user = signed_in()?;
    if moqui_core::user::has_permission("MONITOR_REPLAY")? {
        Ok(user)
    } else {
        Err(Error::new(
            "MONITOR_REPLAY permission is required to see session replays",
        ))
    }
}

/// The replays (their first chunks), newest first, optionally of one user or visit and since `fromMillis`.
fn list_replays(_ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    require_replay()?;
    let mut filters = vec![Query::eq("seq", Value::I64(0))];
    for (parameter, field) in [("userId", "user.id"), ("visitId", "moqui.visit.id")] {
        if !text(p, parameter).is_empty() {
            filters.push(Query::eq(field, Value::string(text(p, parameter))));
        }
    }
    if let Some(from) = p.get("fromMillis").and_then(whole) {
        filters.push(Query::range("start", Some(Value::I64(from)), None));
    }
    let mut request = SearchRequest::new(Query::all(filters));
    request.sort = vec![SortField {
        field: "start".into(),
        descending: true,
    }];
    request.limit = p
        .get("limit")
        .and_then(whole)
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(100)
        .clamp(1, 500);
    let page = search::current()?.search(&Target::Index(REPLAYS.into()), &request)?;
    let replays = page
        .hits
        .iter()
        .map(|hit| {
            let field = |name: &str| hit.body.get(name).cloned().unwrap_or(Value::Null);
            Value::Map(
                [
                    ("replayId", field("replay_id")),
                    ("userId", field("user.id")),
                    ("visitId", field("moqui.visit.id")),
                    ("url", field("url")),
                    ("start", field("start")),
                    ("traceIds", field("trace_ids")),
                ]
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
            )
        })
        .collect();
    Ok(Parameters::from_iter([("replays".to_string(), Value::List(replays))]))
}

/// A replay's events in order, its page, user, visit, start and end, and the traces its page touched.
fn get_replay(_ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let viewer = require_replay()?;
    let replay = text(p, "replayId");
    if !valid_replay_id(replay) {
        return Err(Error::new("replayId is not a replay"));
    }
    let mut request = SearchRequest::new(Query::eq("replay_id", Value::string(replay)));
    request.sort = vec![SortField {
        field: "seq".into(),
        descending: false,
    }];
    request.limit = 2000;
    let page = search::current()?.search(&Target::Index(REPLAYS.into()), &request)?;
    tracing::info!(replay, viewer, chunks = page.hits.len(), "session replay viewed");
    let mut events = Vec::new();
    let mut traces = Vec::new();
    for hit in &page.hits {
        let packed = hit.body.get("events").and_then(Value::as_str).unwrap_or("");
        let unpacked = base64::engine::general_purpose::STANDARD
            .decode(packed)
            .ok()
            .and_then(|bytes| {
                let mut text = Vec::new();
                flate2::read::GzDecoder::new(bytes.as_slice())
                    .read_to_end(&mut text)
                    .ok()
                    .map(|_| text)
            })
            .and_then(|text| serde_json::from_slice::<Vec<Json>>(&text).ok());
        match unpacked {
            Some(chunk) => events.extend(chunk.iter().map(moqui_lib::json::from_json)),
            None => tracing::warn!(
                replay,
                id = hit.id,
                "a session replay chunk could not be read; it is skipped"
            ),
        }
        if let Some(Value::List(ids)) = hit.body.get("trace_ids") {
            traces.extend(ids.iter().cloned());
        }
    }
    let first = page.hits.first().map(|hit| &hit.body);
    let field = |name: &str| first.and_then(|body| body.get(name)).cloned().unwrap_or(Value::Null);
    let end = page
        .hits
        .last()
        .and_then(|hit| hit.body.get("end"))
        .cloned()
        .unwrap_or(Value::Null);
    traces.dedup();
    Ok(Parameters::from_iter([
        ("events".to_string(), Value::List(events)),
        ("userId".to_string(), field("user.id")),
        ("visitId".to_string(), field("moqui.visit.id")),
        ("url".to_string(), field("url")),
        ("start".to_string(), field("start")),
        ("end".to_string(), end),
        ("traceIds".to_string(), Value::List(traces)),
    ]))
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
