//! Prometheus text exposition (format 0.0.4) of one cumulative SDK collection, for `/metrics` scrapes.
//!
//! Names follow the OpenTelemetry compatibility rules: other characters become `_`, a unit of seconds or bytes is a
//! suffix, a monotonic sum is a counter ending in `_total`, and the resource is one `target_info` series.

use std::collections::HashSet;
use std::fmt::Write as _;

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::{metric::Data, number_data_point};

use super::Resource;

pub(super) fn render(request: &ExportMetricsServiceRequest, resource: &Resource) -> String {
    let mut out = String::new();
    out.push_str("# TYPE target_info gauge\n");
    let _ = writeln!(
        out,
        "target_info{{service_name=\"{}\",service_instance_id=\"{}\",service_version=\"{}\"}} 1",
        escape(&resource.service_name),
        escape(&resource.instance_id),
        escape(&resource.version)
    );
    let mut typed = HashSet::new();
    let metrics = request
        .resource_metrics
        .iter()
        .flat_map(|resource| &resource.scope_metrics)
        .flat_map(|scope| &scope.metrics);
    for metric in metrics {
        let base = metric_name(&metric.name, &metric.unit);
        match &metric.data {
            Some(Data::Gauge(gauge)) => {
                header(&mut out, &mut typed, &base, "gauge");
                for point in &gauge.data_points {
                    sample(&mut out, &base, &point.attributes, None, number(&point.value));
                }
            }
            Some(Data::Sum(sum)) => {
                let (name, kind) = if sum.is_monotonic {
                    (format!("{}_total", base.trim_end_matches("_total")), "counter")
                } else {
                    (base.clone(), "gauge")
                };
                header(&mut out, &mut typed, &name, kind);
                for point in &sum.data_points {
                    sample(&mut out, &name, &point.attributes, None, number(&point.value));
                }
            }
            Some(Data::Histogram(histogram)) => {
                header(&mut out, &mut typed, &base, "histogram");
                let bucket = format!("{base}_bucket");
                for point in &histogram.data_points {
                    let mut cumulative = 0_u64;
                    for (index, bound) in point.explicit_bounds.iter().enumerate() {
                        cumulative = cumulative.saturating_add(point.bucket_counts.get(index).copied().unwrap_or(0));
                        sample(
                            &mut out,
                            &bucket,
                            &point.attributes,
                            Some(&value(*bound)),
                            cumulative as f64,
                        );
                    }
                    sample(&mut out, &bucket, &point.attributes, Some("+Inf"), point.count as f64);
                    if let Some(sum) = point.sum {
                        sample(&mut out, &format!("{base}_sum"), &point.attributes, None, sum);
                    }
                    sample(
                        &mut out,
                        &format!("{base}_count"),
                        &point.attributes,
                        None,
                        point.count as f64,
                    );
                }
            }
            // The framework records no exponential histograms or summaries; a scrape leaves them out.
            _ => {}
        }
    }
    out
}

/// Received metrics after this server's own: one family per Prometheus name, labelled by their attributes and
/// `service_name`, `service_instance_id` and `source`. A family whose name `local` already has is left out (scraping
/// this server's own `/metrics` adds nothing), as is a series whose kind differs from its family's first one; the
/// series not kept show as `moqui_telemetry_received_series_dropped_total`.
pub(super) fn render_received(received: &super::received::Received, local: &str) -> String {
    use super::received::Kind;
    let taken: HashSet<&str> = local
        .lines()
        .filter_map(|line| line.strip_prefix("# TYPE "))
        .filter_map(|line| line.split(' ').next())
        .collect();
    let mut families: std::collections::BTreeMap<String, (Kind, Vec<&super::received::Series>)> = Default::default();
    let mut skipped = received.overflow;
    for series in received.series.values() {
        let base = metric_name(&series.name, &series.unit);
        let name = match series.kind {
            Kind::Counter => format!("{}_total", base.trim_end_matches("_total")),
            _ => base,
        };
        let clashes = |name: &str| {
            taken.contains(name)
                || (series.kind == Kind::Histogram
                    && ["_bucket", "_sum", "_count"]
                        .iter()
                        .any(|suffix| taken.contains(format!("{name}{suffix}").as_str())))
        };
        if clashes(&name) {
            skipped += 1;
            continue;
        }
        let family = families
            .entry(name)
            .or_insert_with(|| (series.kind.clone(), Vec::new()));
        if family.0 == series.kind {
            family.1.push(series);
        } else {
            skipped += 1;
        }
    }
    let mut out = String::new();
    for (name, (kind, mut members)) in families {
        members.sort_by(|a, b| a.labels.cmp(&b.labels));
        let _ = writeln!(
            out,
            "# TYPE {name} {}",
            match kind {
                Kind::Counter => "counter",
                Kind::Histogram => "histogram",
                Kind::UpDown | Kind::Gauge => "gauge",
            }
        );
        for series in members {
            let labels: Vec<(String, String)> = series
                .labels
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if kind == Kind::Histogram {
                let mut cumulative = 0u64;
                for (index, bound) in series.bounds.iter().enumerate() {
                    cumulative = cumulative.saturating_add(series.buckets.get(index).copied().unwrap_or(0));
                    line(
                        &mut out,
                        &format!("{name}_bucket"),
                        &labels,
                        Some(&value(*bound)),
                        cumulative as f64,
                    );
                }
                line(
                    &mut out,
                    &format!("{name}_bucket"),
                    &labels,
                    Some("+Inf"),
                    series.count as f64,
                );
                line(&mut out, &format!("{name}_sum"), &labels, None, series.value);
                line(&mut out, &format!("{name}_count"), &labels, None, series.count as f64);
            } else {
                line(&mut out, &name, &labels, None, series.value);
            }
        }
    }
    if skipped > 0 || !received.series.is_empty() {
        let _ = writeln!(out, "# TYPE moqui_telemetry_received_series_dropped_total counter");
        let _ = writeln!(out, "moqui_telemetry_received_series_dropped_total {skipped}");
    }
    out
}

fn line(out: &mut String, name: &str, labels: &[(String, String)], le: Option<&str>, number: f64) {
    out.push_str(name);
    let mut labels = labels.to_vec();
    if let Some(le) = le {
        labels.retain(|(key, _)| key != "le");
        labels.push(("le".to_string(), le.to_string()));
    }
    if !labels.is_empty() {
        out.push('{');
        for (index, (key, text)) in labels.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, "{key}=\"{}\"", escape(text));
        }
        out.push('}');
    }
    let _ = writeln!(out, " {}", value(number));
}

fn header(out: &mut String, typed: &mut HashSet<String>, name: &str, kind: &str) {
    if typed.insert(name.to_string()) {
        let _ = writeln!(out, "# TYPE {name} {kind}");
    }
}

fn sample(out: &mut String, name: &str, attributes: &[KeyValue], le: Option<&str>, number: f64) {
    out.push_str(name);
    let mut labels = attributes
        .iter()
        .map(|attribute| (label_name(&attribute.key), attribute_text(attribute)))
        .collect::<Vec<_>>();
    if let Some(le) = le {
        labels.push(("le".to_string(), le.to_string()));
    }
    if !labels.is_empty() {
        out.push('{');
        for (index, (key, text)) in labels.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, "{key}=\"{}\"", escape(text));
        }
        out.push('}');
    }
    let _ = writeln!(out, " {}", value(number));
}

fn number(point: &Option<number_data_point::Value>) -> f64 {
    match point {
        Some(number_data_point::Value::AsDouble(value)) => *value,
        Some(number_data_point::Value::AsInt(value)) => *value as f64,
        None => f64::NAN,
    }
}

fn value(number: f64) -> String {
    if number.is_nan() {
        "NaN".into()
    } else if number == f64::INFINITY {
        "+Inf".into()
    } else if number == f64::NEG_INFINITY {
        "-Inf".into()
    } else {
        number.to_string()
    }
}

fn attribute_text(attribute: &KeyValue) -> String {
    match attribute.value.as_ref().and_then(|value| value.value.as_ref()) {
        Some(any_value::Value::StringValue(text)) => text.clone(),
        Some(any_value::Value::BoolValue(value)) => value.to_string(),
        Some(any_value::Value::IntValue(value)) => value.to_string(),
        Some(any_value::Value::DoubleValue(value)) => value.to_string(),
        _ => String::new(),
    }
}

/// `[a-zA-Z_:][a-zA-Z0-9_:]*`, with a unit of seconds, milliseconds or bytes as a suffix.
fn metric_name(name: &str, unit: &str) -> String {
    let mut name = sanitize(name, true);
    let suffix = match unit {
        "s" => "_seconds",
        "ms" => "_milliseconds",
        "By" => "_bytes",
        _ => "",
    };
    if !suffix.is_empty() && !name.ends_with(suffix) {
        name.push_str(suffix);
    }
    name
}

fn label_name(key: &str) -> String {
    sanitize(key, false)
}

fn sanitize(text: &str, colon: bool) -> String {
    let mut out: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || (colon && c == ':') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

#[cfg(test)]
mod received_tests {
    use super::super::received::Received;
    use super::render_received;
    use crate::entity::document::Document;
    use moqui_lib::object::Value;

    fn document(pairs: Vec<(&str, Value)>) -> Document {
        pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect()
    }

    #[test]
    fn received_families_render_after_local_ones_without_clashing() {
        let mut received = Received::default();
        received.record(&[
            document(vec![
                ("series_fingerprint", Value::string("a")),
                ("name", Value::string("orders.placed")),
                ("aggregation", Value::string("sum")),
                ("monotonic", Value::Bool(true)),
                ("sum", Value::F64(5.0)),
                ("service.name", Value::string("shop")),
                ("source.name", Value::string("edge")),
                (
                    "attributes",
                    Value::Map([("status".to_string(), Value::string("paid"))].into_iter().collect()),
                ),
            ]),
            document(vec![
                ("series_fingerprint", Value::string("b")),
                ("name", Value::string("http.server.request.duration")),
                ("unit", Value::string("s")),
                ("aggregation", Value::string("histogram")),
                ("sum", Value::F64(0.5)),
                ("count", Value::I64(2)),
                ("bounds", Value::List(vec![Value::F64(0.1)])),
                ("buckets", Value::List(vec![Value::I64(1), Value::I64(1)])),
            ]),
            document(vec![
                ("series_fingerprint", Value::string("c")),
                ("name", Value::string("queue.depth")),
                ("aggregation", Value::string("gauge")),
                ("last", Value::F64(9.0)),
            ]),
        ]);
        let local =
            "# TYPE http_server_request_duration_seconds histogram\nhttp_server_request_duration_seconds_count 1\n";
        let text = render_received(&received, local);
        assert!(text.contains("# TYPE orders_placed_total counter\n"), "{text}");
        assert!(
            text.contains("orders_placed_total{service_name=\"shop\",source=\"edge\",status=\"paid\"} 5\n"),
            "{text}"
        );
        assert!(text.contains("# TYPE queue_depth gauge\nqueue_depth 9\n"), "{text}");
        assert!(!text.contains("http_server_request_duration_seconds"), "{text}");
        assert!(
            text.contains("moqui_telemetry_received_series_dropped_total 1\n"),
            "{text}"
        );
        // Every sample parses back with the scrape parser.
        let parsed = super::super::scrape::parse(&text);
        assert_eq!(parsed.skipped, 0);
    }
}
