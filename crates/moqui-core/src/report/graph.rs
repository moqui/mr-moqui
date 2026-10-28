//! Span graphs for the node graph and flame graph panels: the calls between services (a service map), and span-name
//! paths with their self time. Both are computed from spans the viewer may read, after the query's time range, query
//! string and filters, and both stop at [`SPAN_LIMIT`] spans with a notice.
use super::frame::{Field, Frame};
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The most spans one graph query reads.
pub const SPAN_LIMIT: usize = 10_000;
/// The deepest span path of a flame graph; deeper spans are folded into their ancestor at this depth.
const PATH_DEPTH: usize = 64;

fn string(span: &Json, name: &str) -> Option<String> {
    span.get(name)
        .and_then(Json::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
fn nanos(span: &Json) -> f64 {
    match &span["duration_nano"] {
        Json::Number(n) => n.as_f64().unwrap_or(0.0),
        Json::String(s) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
    .max(0.0)
}
fn field(name: &str, kind: &str, unit: Option<&str>, values: Vec<Json>) -> Field {
    Field {
        name: name.into(),
        kind: kind.into(),
        unit: unit.map(str::to_owned),
        labels: BTreeMap::new(),
        config: BTreeMap::new(),
        values,
    }
}

#[derive(Default)]
struct Edge {
    count: u64,
    errors: u64,
    nanos: f64,
}

/// One row per edge between services: `source`, `target`, `count`, `errors` and the mean `duration` (ns), most calls
/// first. A span whose parent is in another service is a call from the parent's service; a CLIENT span to a database
/// is a call to `db:<system>`; and a CLIENT span to a `server.address` that no span of the result answered is a call to
/// that address.
pub fn service_map(spans: &[Json]) -> Frame {
    let by_id: HashMap<&str, &Json> = spans
        .iter()
        .filter_map(|span| span["span_id"].as_str().map(|id| (id, span)))
        .collect();
    let parents: HashSet<&str> = spans
        .iter()
        .filter_map(|span| span["parent_span_id"].as_str())
        .collect();
    let mut edges: BTreeMap<(String, String), Edge> = BTreeMap::new();
    let mut add = |source: String, target: String, span: &Json| {
        let edge = edges.entry((source, target)).or_default();
        edge.count += 1;
        edge.nanos += nanos(span);
        if span["status_code"] == "ERROR" {
            edge.errors += 1;
        }
    };
    for span in spans {
        let Some(service) = string(span, "service.name") else {
            continue;
        };
        if let Some(parent) = span["parent_span_id"].as_str().and_then(|id| by_id.get(id)) {
            if let Some(caller) = string(parent, "service.name").filter(|caller| *caller != service) {
                add(caller, service.clone(), span);
            }
        }
        if span["kind"] != "CLIENT" {
            continue;
        }
        if let Some(system) = string(span, "db.system.name") {
            add(service, format!("db:{system}"), span);
        } else if let Some(address) = string(span, "server.address") {
            let answered = span["span_id"].as_str().is_some_and(|id| parents.contains(id));
            if !answered {
                add(service, address, span);
            }
        }
    }
    let mut rows: Vec<_> = edges.into_iter().collect();
    rows.sort_by(|a, b| b.1.count.cmp(&a.1.count).then_with(|| a.0.cmp(&b.0)));
    let (mut sources, mut targets, mut counts, mut errors, mut durations) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for ((source, target), edge) in rows {
        sources.push(json!(source));
        targets.push(json!(target));
        counts.push(json!(edge.count));
        errors.push(json!(edge.errors));
        durations.push(json!((edge.nanos / edge.count.max(1) as f64).round()));
    }
    Frame {
        fields: vec![
            field("source", "string", None, sources),
            field("target", "string", None, targets),
            field("count", "number", None, counts),
            field("errors", "number", None, errors),
            field("duration", "number", Some("ns"), durations),
        ],
    }
}

/// One row per span-name path from a root (`a;b;c`) with the self time of its spans in milliseconds: the span's
/// duration less its children's, never below zero. A span whose parent is not in the result is a root.
pub fn span_flame(spans: &[Json]) -> Frame {
    let by_id: HashMap<&str, &Json> = spans
        .iter()
        .filter_map(|span| span["span_id"].as_str().map(|id| (id, span)))
        .collect();
    let mut child_nanos: HashMap<&str, f64> = HashMap::new();
    for span in spans {
        if let Some(parent) = span["parent_span_id"].as_str().filter(|id| by_id.contains_key(id)) {
            *child_nanos.entry(parent).or_default() += nanos(span);
        }
    }
    let mut paths: BTreeMap<String, f64> = BTreeMap::new();
    for span in spans {
        let mut names = vec![span_name(span)];
        let mut seen = HashSet::from([span["span_id"].as_str().unwrap_or("")]);
        let mut at = span;
        while let Some(parent) = at["parent_span_id"].as_str().and_then(|id| by_id.get(id)) {
            if !seen.insert(parent["span_id"].as_str().unwrap_or("")) {
                break;
            }
            names.push(span_name(parent));
            at = parent;
        }
        names.reverse();
        names.truncate(PATH_DEPTH);
        let own = span["span_id"]
            .as_str()
            .and_then(|id| child_nanos.get(id))
            .copied()
            .unwrap_or(0.0);
        *paths.entry(names.join(";")).or_default() += (nanos(span) - own).max(0.0) / 1e6;
    }
    let (mut path, mut value) = (Vec::new(), Vec::new());
    for (key, millis) in paths {
        path.push(json!(key));
        value.push(json!((millis * 1000.0).round() / 1000.0));
    }
    Frame {
        fields: vec![
            field("path", "string", None, path),
            field("value", "number", Some("ms"), value),
        ],
    }
}

/// A span's name with the path separator replaced, so a name never splits into two frames.
fn span_name(span: &Json) -> String {
    span["name"].as_str().unwrap_or("(unnamed)").replace(';', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(id: &str, parent: Option<&str>, service: &str, name: &str, nanos: u64) -> Json {
        json!({"span_id":id,"parent_span_id":parent,"service.name":service,"name":name,"duration_nano":nanos,"kind":"INTERNAL"})
    }

    #[test]
    fn service_map_links_services_databases_and_unanswered_addresses() {
        let mut client = span("c", Some("a"), "web", "HTTP GET", 4_000_000);
        client["kind"] = json!("CLIENT");
        client["server.address"] = json!("api.example.com");
        let mut answered = span("d", Some("a"), "web", "HTTP POST", 2_000_000);
        answered["kind"] = json!("CLIENT");
        answered["server.address"] = json!("orders.internal");
        let mut server = span("e", Some("d"), "orders", "POST /orders", 1_000_000);
        server["status_code"] = json!("ERROR");
        let mut sql = span("f", Some("e"), "orders", "SELECT", 500_000);
        sql["kind"] = json!("CLIENT");
        sql["db.system.name"] = json!("postgresql");
        let frame = service_map(&[
            span("a", None, "web", "GET /", 9_000_000),
            client,
            answered,
            server,
            sql,
        ]);
        let rows = frame.rows();
        let edge = |source: &str, target: &str| {
            rows.iter()
                .find(|row| row["source"] == source && row["target"] == target)
                .cloned()
        };
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(edge("web", "orders").unwrap()["errors"], json!(1));
        assert_eq!(edge("orders", "db:postgresql").unwrap()["duration"], json!(500000.0));
        assert!(edge("web", "api.example.com").is_some());
        assert!(
            edge("web", "orders.internal").is_none(),
            "an answered call is drawn to its service, not its address"
        );
    }

    #[test]
    fn flame_paths_carry_self_time_and_survive_cycles() {
        let frame = span_flame(&[
            span("a", None, "s", "GET /", 10_000_000),
            span("b", Some("a"), "s", "find", 4_000_000),
            span("c", Some("b"), "s", "SELECT", 3_000_000),
            span("d", Some("a"), "s", "find", 2_000_000),
            span("x", Some("y"), "s", "loop;x", 1_000_000),
            span("y", Some("x"), "s", "loop y", 1_000_000),
        ]);
        let rows: BTreeMap<String, Json> = frame
            .rows()
            .into_iter()
            .map(|row| (row["path"].as_str().unwrap().to_owned(), row["value"].clone()))
            .collect();
        assert_eq!(rows["GET /"], json!(4.0));
        assert_eq!(rows["GET /;find"], json!(3.0), "two spans of one path add up");
        assert_eq!(rows["GET /;find;SELECT"], json!(3.0));
        assert_eq!(rows["loop y;loop,x"], json!(0.0), "a cycle stops at the first repeat");
    }
}
