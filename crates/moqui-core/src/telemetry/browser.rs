//! Browser RUM (plans/Monitoring.md, Later: Browser RUM): what a signed-in page sends is OTLP/JSON, but the page is
//! not trusted the way an ingest key's sender is. Before it is decoded the resource attributes are replaced with the
//! server's (the browser service name and the session's user and visit), attributes that name a user, visit, source or
//! service are dropped from every record, and only CLIENT and INTERNAL spans are kept, so a page cannot pose as a
//! server or as another user. The host's transition authenticates the session and its CSRF token.

use serde_json::{json, Value as Json};

use super::ingest::Signal;

/// Who sent the batch, from the session, never from the page.
pub struct BrowserIdentity<'a> {
    pub service_name: &'a str,
    pub user_id: &'a str,
    pub visit_id: &'a str,
}

/// The resource attributes a page may set: the browser, the SDK and the page's own session; every other is the
/// server's.
fn page_resource_attribute(key: &str) -> bool {
    key.starts_with("browser.")
        || key.starts_with("telemetry.sdk.")
        || key.starts_with("os.")
        || key == "user_agent.original"
        || key == "session.id"
}

/// Record attributes a page may not set: those the server sets, or that pose as another user or source.
fn reserved_record_attribute(key: &str) -> bool {
    key == "user.id"
        || key.starts_with("moqui.visit")
        || key.starts_with("moqui.user")
        || key.starts_with("moqui.artifact")
        || key.starts_with("source.")
        || key.starts_with("service.")
}

fn kind_allowed(kind: Option<&Json>) -> bool {
    match kind {
        None | Some(Json::Null) => true,
        Some(Json::Number(number)) => matches!(number.as_u64(), Some(0 | 1 | 3)),
        Some(Json::String(text)) => matches!(
            text.as_str(),
            "0" | "1" | "3" | "SPAN_KIND_UNSPECIFIED" | "SPAN_KIND_INTERNAL" | "SPAN_KIND_CLIENT"
        ),
        Some(_) => false,
    }
}

fn keep_attributes(record: &mut Json, keep: impl Fn(&str) -> bool) {
    if let Some(Json::Array(items)) = record.get_mut("attributes") {
        items.retain(|item| item.get("key").and_then(Json::as_str).is_some_and(&keep));
    }
}

fn string_attribute(key: &str, value: &str) -> Json {
    json!({"key": key, "value": {"stringValue": value}})
}

/// Make a page's OTLP/JSON export request one the server can index as its own: answers the records refused (spans of
/// another kind; at most `max_records` are kept). The request is changed in place.
pub fn scrub(signal: Signal, request: &mut Json, identity: &BrowserIdentity<'_>, max_records: usize) -> u64 {
    let (resources, scopes, records) = match signal {
        Signal::Traces => ("resourceSpans", "scopeSpans", "spans"),
        Signal::Logs => ("resourceLogs", "scopeLogs", "logRecords"),
        Signal::Metrics => ("resourceMetrics", "scopeMetrics", "metrics"),
    };
    let mut refused = 0_u64;
    let mut kept = 0_usize;
    let Some(Json::Array(resource_list)) = request.get_mut(resources) else {
        return 0;
    };
    for resource in resource_list.iter_mut() {
        let mut attributes: Vec<Json> = resource
            .pointer("/resource/attributes")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| {
                        item.get("key")
                            .and_then(Json::as_str)
                            .is_some_and(page_resource_attribute)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        attributes.push(string_attribute("service.name", identity.service_name));
        attributes.push(string_attribute("moqui.source", "browser"));
        if !identity.user_id.is_empty() {
            attributes.push(string_attribute("user.id", identity.user_id));
        }
        if !identity.visit_id.is_empty() {
            attributes.push(string_attribute("moqui.visit.id", identity.visit_id));
        }
        resource["resource"] = json!({"attributes": attributes});
        let Some(Json::Array(scope_list)) = resource.get_mut(scopes) else {
            continue;
        };
        for scope in scope_list.iter_mut() {
            let Some(Json::Array(record_list)) = scope.get_mut(records) else {
                continue;
            };
            let before = record_list.len();
            record_list
                .retain(|record| matches!(signal, Signal::Logs | Signal::Metrics) || kind_allowed(record.get("kind")));
            refused += (before - record_list.len()) as u64;
            if kept + record_list.len() > max_records {
                let room = max_records.saturating_sub(kept);
                refused += (record_list.len() - room) as u64;
                record_list.truncate(room);
            }
            kept += record_list.len();
            for record in record_list.iter_mut() {
                keep_attributes(record, |key| !reserved_record_attribute(key));
                if let Some(Json::Array(events)) = record.get_mut("events") {
                    for event in events.iter_mut() {
                        keep_attributes(event, |key| !reserved_record_attribute(key));
                    }
                }
            }
        }
    }
    refused
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
