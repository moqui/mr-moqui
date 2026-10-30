//! Flat telemetry mappings shared by the local SDK exporters and the OTLP receiver.

use super::mapping::{DocumentMapping, FieldKind, FieldMapping, ObjectMapping};

pub const SPANS: &str = "otel_spans";
pub const LOGS: &str = "otel_logs";
pub const METRICS: &str = "otel_metrics";
pub const ERRORS: &str = "otel_errors";
pub const ANNOTATIONS: &str = "monitor_annotations";
/// Browser session replay chunks (rrweb events, gzipped). Read only through the replay services, which need
/// `MONITOR_REPLAY`: never a report dataset.
pub const REPLAYS: &str = "browser_replays";
pub const TELEMETRY_INDEXES: &[&str] = &[SPANS, LOGS, METRICS, ERRORS, ANNOTATIONS, REPLAYS];

pub fn is_telemetry_index(name: &str) -> bool {
    TELEMETRY_INDEXES.contains(&name)
}

/// The field a report filters and buckets a telemetry index by.
pub fn time_field(name: &str) -> Option<&'static str> {
    Some(match name {
        SPANS => "start",
        LOGS => "timestamp",
        METRICS => "bucket_start",
        ERRORS => "last_seen",
        ANNOTATIONS => "time",
        REPLAYS => "start",
        _ => return None,
    })
}

/// The metric granularities, each a list partition of a day-partitioned `otel_metrics`.
pub const GRANULARITIES: &[&str] = &["minute", "hour", "day"];

/// How a telemetry index is split when the Postgres backend partitions by day: an optional list column (the metric
/// granularity, whose rows keep for different times) and the day column. `otel_errors` is upserted by fingerprint
/// across days, so it is not split.
pub fn partition_by(name: &str) -> Option<(Option<&'static str>, &'static str)> {
    Some(match name {
        SPANS => (None, "start"),
        LOGS => (None, "timestamp"),
        METRICS => (Some("granularity"), "bucket_start"),
        ANNOTATIONS => (None, "time"),
        REPLAYS => (None, "start"),
        _ => return None,
    })
}

pub fn telemetry_mapping(name: &str) -> Option<DocumentMapping> {
    use FieldKind::{Binary, Boolean, Date, Double, Keyword, Long, Text};
    let fields: &[(&str, FieldKind)] = match name {
        SPANS => &[
            ("trace_id", Keyword),
            ("span_id", Keyword),
            ("parent_span_id", Keyword),
            ("name", Keyword),
            ("kind", Keyword),
            ("start", Date),
            ("start_nano", Long),
            ("duration_nano", Long),
            ("status_code", Keyword),
            ("sampled", Boolean),
            ("moqui.artifact.type", Keyword),
            ("moqui.artifact.name", Keyword),
            ("moqui.artifact.action", Keyword),
            ("moqui.artifact.sub_type", Keyword),
            ("moqui.artifact.slow", Boolean),
            ("user.id", Keyword),
            ("moqui.visit.id", Keyword),
            ("http.request.method", Keyword),
            ("http.route", Keyword),
            ("http.response.status_code", Long),
            ("url.path", Keyword),
            ("url.query", Text),
            ("url.scheme", Keyword),
            ("client.address", Keyword),
            ("server.address", Keyword),
            ("session.id", Keyword),
            ("moqui.visitor.id", Keyword),
            ("network.protocol.version", Keyword),
            ("http.response.body.size", Long),
            ("moqui.http.time_to_first_byte", Double),
            ("http.request.header.referer", Text),
            ("user_agent.original", Text),
            ("http.response.header.content-type", Keyword),
            ("db.system.name", Keyword),
            ("db.query.text", Text),
            ("moqui.db.system.uncertain", Boolean),
            ("url.full", Text),
            ("moqui.job.root", Boolean),
            ("moqui.job.name", Keyword),
            ("moqui.job.run.id", Keyword),
            ("moqui.synthetic.check", Boolean),
            ("synthetic.check.id", Keyword),
            ("synthetic.check.name", Keyword),
            ("gen_ai.operation.name", Keyword),
            ("gen_ai.provider.name", Keyword),
            ("gen_ai.request.model", Keyword),
            ("gen_ai.response.model", Keyword),
            ("gen_ai.usage.input_tokens", Long),
            ("gen_ai.usage.output_tokens", Long),
            ("gen_ai.response.finish_reasons", Keyword),
            ("exception.type", Keyword),
            ("exception.message", Text),
            ("exception.stacktrace", Text),
        ],
        LOGS => &[
            ("timestamp", Date),
            ("severity_number", Long),
            ("severity_text", Keyword),
            ("body", Text),
            ("trace_id", Keyword),
            ("span_id", Keyword),
            ("scope_name", Keyword),
            ("thread.name", Keyword),
            ("thread.id", Long),
            ("user.id", Keyword),
            ("moqui.visitor.id", Keyword),
            ("exception.type", Keyword),
            ("exception.message", Text),
            ("exception.stacktrace", Text),
        ],
        METRICS => &[
            ("name", Keyword),
            ("series_fingerprint", Keyword),
            ("bucket_start", Date),
            ("bucket_end", Date),
            ("granularity", Keyword),
            ("aggregation", Keyword),
            ("unit", Keyword),
            ("count", Long),
            ("sum", Double),
            ("min", Double),
            ("max", Double),
            ("last", Double),
            ("last_time", Date),
            ("http.route", Keyword),
            ("http.request.method", Keyword),
            ("http.response.status_code", Long),
            ("db.system.name", Keyword),
            ("outcome", Keyword),
            ("moqui.artifact.type", Keyword),
            ("moqui.artifact.name", Keyword),
            ("moqui.artifact.action", Keyword),
            ("moqui.artifact.sub_type", Keyword),
        ],
        ERRORS => &[
            ("fingerprint", Keyword),
            ("first_seen", Date),
            ("last_seen", Date),
            ("count", Long),
            ("exception.type", Keyword),
            ("message", Text),
            ("stacktrace", Text),
        ],
        ANNOTATIONS => &[
            ("time", Date),
            ("time_end", Date),
            ("source", Keyword),
            ("tags", Keyword),
            ("text", Text),
            ("scope", Keyword),
            ("dashboard", Keyword),
            ("panel", Keyword),
            ("created_by", Keyword),
            ("rule_id", Keyword),
            ("series_id", Keyword),
            ("state", Keyword),
        ],
        REPLAYS => &[
            ("replay_id", Keyword),
            ("seq", Long),
            ("start", Date),
            ("end", Date),
            ("user.id", Keyword),
            ("moqui.visit.id", Keyword),
            ("url", Keyword),
            ("trace_ids", Keyword),
            ("bytes", Long),
            ("events", Binary),
        ],
        _ => return None,
    };
    let mut root = ObjectMapping::default();
    for (field, kind) in fields.iter().copied().chain([
        ("service.name", Keyword),
        ("service.instance.id", Keyword),
        ("service.version", Keyword),
        ("vcs.ref.head.revision", Keyword),
        ("host.name", Keyword),
        ("source.name", Keyword),
        ("source.id", Keyword),
        ("moqui.identity.uncertain", Boolean),
    ]) {
        if matches!(name, ANNOTATIONS | REPLAYS) && matches!(field, "source.name" | "source.id") {
            continue;
        }
        root.fields
            .insert(field.to_string(), FieldMapping { kind, sortable: true });
    }
    if matches!(name, SPANS | LOGS | METRICS) {
        let mut values = ObjectMapping::default();
        for key in ["key", "value"] {
            values.fields.insert(
                key.into(),
                FieldMapping {
                    kind: Keyword,
                    sortable: false,
                },
            );
        }
        root.children.insert(ATTRIBUTE_VALUES.into(), values);
    }
    Some(DocumentMapping {
        document_id: name.to_string(),
        index_name: None,
        physical_name: name.to_string(),
        root,
        manual_mapping_service: None,
        display: Default::default(),
    })
}

/// Fixed nested pairs avoid dynamic per-key mappings and keep keys correlated with their values.
pub const ATTRIBUTE_VALUES: &str = "__attributes";

pub(crate) fn prepare_attributes(body: &mut crate::entity::document::Document) {
    use moqui_lib::object::{to_plain_string, Value};
    let mut pairs = Vec::new();
    if let Some(Value::Map(attributes)) = body.get("attributes") {
        for (key, value) in attributes {
            let values = match value {
                Value::List(values) => values.as_slice(),
                value => std::slice::from_ref(value),
            };
            for value in values {
                if matches!(
                    value,
                    Value::String(_)
                        | Value::Bool(_)
                        | Value::I32(_)
                        | Value::I64(_)
                        | Value::F64(_)
                        | Value::Decimal(_)
                ) {
                    pairs.push(Value::Map(indexmap::IndexMap::from_iter([
                        ("key".into(), Value::string(key)),
                        ("value".into(), Value::String(to_plain_string(value))),
                    ])));
                }
            }
        }
    }
    body.insert(ATTRIBUTE_VALUES.into(), Value::List(pairs));
}

/// Translate only attribute leaves; each compiler recursively encounters leaves inside bool/child clauses.
pub(crate) fn attribute_query(mapping: &DocumentMapping, query: &super::Query) -> Option<super::Query> {
    use super::query::{FieldOp, Query, TextFields, TextMatch};
    if !mapping.root.children.contains_key(ATTRIBUTE_VALUES) {
        return None;
    }
    let (name, mut value) = match query {
        Query::Field(field)
            if matches!(
                field.op,
                FieldOp::Eq | FieldOp::In | FieldOp::Prefix | FieldOp::Wildcard | FieldOp::Exists
            ) =>
        {
            let mut value = (**field).clone();
            value.name = "value".into();
            if !value.value.is_null() {
                value.value = moqui_lib::object::Value::String(moqui_lib::object::to_plain_string(&value.value));
            }
            value.values = value
                .values
                .iter()
                .map(|v| moqui_lib::object::Value::String(moqui_lib::object::to_plain_string(v)))
                .collect();
            (field.name.as_str(), Query::Field(Box::new(value)))
        }
        Query::Text {
            terms,
            fields: TextFields::Only(fields),
            kind,
        } if fields.len() == 1 => (
            fields[0].as_str(),
            Query::Text {
                terms: terms.clone(),
                fields: TextFields::Only(vec!["value".into()]),
                kind: *kind,
            },
        ),
        _ => return None,
    };
    let key = name.strip_prefix("attributes.").filter(|key| !key.is_empty())?;
    let negated = match &mut value {
        Query::Field(field) => {
            let negated = field.negate;
            field.negate = false;
            negated
        }
        _ => false,
    };
    // A prefix/exact/wildcard is a keyword predicate, including quoted text and contains syntax.
    if let Query::Text {
        terms,
        kind: TextMatch::Contains,
        ..
    } = &value
    {
        value = Query::Text {
            terms: format!(
                "*{}*",
                terms.replace('\\', "\\\\").replace('*', "\\*").replace('?', "\\?")
            ),
            fields: TextFields::Only(vec!["value".into()]),
            kind: TextMatch::Wildcard,
        };
    }
    let result = Query::child(
        ATTRIBUTE_VALUES,
        Query::all(vec![Query::eq("key", moqui_lib::object::Value::string(key)), value]),
    );
    Some(if negated { Query::exclude(result) } else { result })
}
