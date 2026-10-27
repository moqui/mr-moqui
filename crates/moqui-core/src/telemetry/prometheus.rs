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
