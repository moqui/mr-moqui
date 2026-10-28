//! Running totals of the metrics this server received (OTLP and scrape targets), for the Prometheus text on `/metrics`.
//! Received points are deltas once decoded, so a sum or histogram series adds them up since start and a gauge keeps its
//! last value. Series are keyed by the decoder's fingerprint and bounded; a series past the bound is counted, not kept.
use std::collections::{BTreeMap, HashMap};

use moqui_lib::object::{to_plain_string, Value};

use crate::entity::document::Document;

/// Received series kept for `/metrics`.
pub const MAX_SERIES: usize = 5000;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Counter,
    UpDown,
    Gauge,
    Histogram,
}

#[derive(Clone, Debug)]
pub struct Series {
    pub name: String,
    pub unit: String,
    pub kind: Kind,
    /// Attributes (as label names), then `service_name`, `service_instance_id` and `source` (which win over an attribute of that name).
    pub labels: BTreeMap<String, String>,
    pub value: f64,
    pub count: u64,
    pub bounds: Vec<f64>,
    pub buckets: Vec<u64>,
}

#[derive(Default)]
pub struct Received {
    pub series: HashMap<String, Series>,
    /// Series not kept because [`MAX_SERIES`] were held, and points whose shape changed (counted once each).
    pub overflow: u64,
}

fn text(document: &Document, key: &str) -> String {
    document.get(key).map(to_plain_string).unwrap_or_default()
}

fn number(value: Option<&Value>) -> Option<f64> {
    value
        .map(to_plain_string)
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

fn list(document: &Document, key: &str) -> Vec<f64> {
    match document.get(key) {
        Some(Value::List(values)) => values.iter().filter_map(|value| number(Some(value))).collect(),
        _ => Vec::new(),
    }
}

/// A Prometheus label name, `[a-zA-Z_][a-zA-Z0-9_]*`.
fn label_name(key: &str) -> String {
    let mut out: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

impl Received {
    /// Add the points of decoded `otel_metrics` documents. Exponential histograms are left out.
    pub fn record(&mut self, documents: &[Document]) {
        for document in documents {
            let kind = match (text(document, "aggregation").as_str(), document.get("monotonic")) {
                ("sum", Some(Value::Bool(false))) => Kind::UpDown,
                ("sum", _) => Kind::Counter,
                ("gauge", _) => Kind::Gauge,
                ("histogram", _) => Kind::Histogram,
                _ => continue,
            };
            let key = text(document, "series_fingerprint");
            let name = text(document, "name");
            if key.is_empty() || name.is_empty() {
                continue;
            }
            let bounds = list(document, "bounds");
            let buckets: Vec<u64> = list(document, "buckets")
                .into_iter()
                .map(|value| value.max(0.0) as u64)
                .collect();
            if !self.series.contains_key(&key) {
                if self.series.len() >= MAX_SERIES {
                    self.overflow += 1;
                    continue;
                }
                let mut labels = BTreeMap::new();
                if let Some(Value::Map(attributes)) = document.get("attributes") {
                    for (key, value) in attributes {
                        labels.insert(label_name(key), to_plain_string(value));
                    }
                }
                for (label, field) in [
                    ("service_name", "service.name"),
                    ("service_instance_id", "service.instance.id"),
                    ("source", "source.name"),
                ] {
                    let value = text(document, field);
                    if !value.is_empty() {
                        labels.insert(label.into(), value);
                    }
                }
                self.series.insert(
                    key.clone(),
                    Series {
                        name,
                        unit: text(document, "unit"),
                        kind: kind.clone(),
                        labels,
                        value: 0.0,
                        count: 0,
                        buckets: vec![0; buckets.len()],
                        bounds: bounds.clone(),
                    },
                );
            }
            let Some(series) = self.series.get_mut(&key) else {
                continue;
            };
            match kind {
                Kind::Gauge => {
                    if let Some(value) = number(document.get("last")).or_else(|| number(document.get("sum"))) {
                        series.value = value;
                    }
                }
                Kind::Counter | Kind::UpDown => series.value += number(document.get("sum")).unwrap_or(0.0),
                Kind::Histogram => {
                    if bounds != series.bounds || buckets.len() != series.buckets.len() {
                        self.overflow += 1;
                        continue;
                    }
                    series.value += number(document.get("sum")).unwrap_or(0.0);
                    series.count += number(document.get("count")).unwrap_or(0.0) as u64;
                    for (total, add) in series.buckets.iter_mut().zip(&buckets) {
                        *total += add;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moqui_lib::object::Value;

    fn document(pairs: &[(&str, Value)]) -> Document {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn sums_add_up_gauges_keep_the_last_and_histograms_merge() {
        let mut received = Received::default();
        let base = |fingerprint: &str, aggregation: &str| {
            vec![
                ("series_fingerprint", Value::string(fingerprint)),
                ("name", Value::string("jobs")),
                ("aggregation", Value::string(aggregation)),
                ("service.name", Value::string("worker")),
                ("source.name", Value::string("edge")),
            ]
        };
        for sum in [3.0, 4.0] {
            let mut pairs = base("c", "sum");
            pairs.push(("monotonic", Value::Bool(true)));
            pairs.push(("sum", Value::F64(sum)));
            received.record(&[document(&pairs)]);
        }
        for last in [7.0, 2.0] {
            let mut pairs = base("g", "gauge");
            pairs.push(("last", Value::F64(last)));
            received.record(&[document(&pairs)]);
        }
        for _ in 0..2 {
            let mut pairs = base("h", "histogram");
            pairs.extend([
                ("sum", Value::F64(1.5)),
                ("count", Value::I64(3)),
                ("bounds", Value::List(vec![Value::F64(1.0)])),
                ("buckets", Value::List(vec![Value::I64(2), Value::I64(1)])),
            ]);
            received.record(&[document(&pairs)]);
        }
        assert_eq!(received.series["c"].value, 7.0);
        assert_eq!(received.series["c"].kind, Kind::Counter);
        assert_eq!(received.series["g"].value, 2.0);
        assert_eq!(received.series["h"].count, 6);
        assert_eq!(received.series["h"].buckets, [4, 2]);
        assert_eq!(received.series["c"].labels["service_name"], "worker");
        assert_eq!(received.series["c"].labels["source"], "edge");
    }

    #[test]
    fn series_past_the_bound_are_counted_not_kept() {
        let mut received = Received::default();
        let documents: Vec<Document> = (0..MAX_SERIES + 3)
            .map(|n| {
                document(&[
                    ("series_fingerprint", Value::string(n.to_string())),
                    ("name", Value::string("g")),
                    ("aggregation", Value::string("gauge")),
                    ("last", Value::F64(1.0)),
                ])
            })
            .collect();
        received.record(&documents);
        assert_eq!(received.series.len(), MAX_SERIES);
        assert_eq!(received.overflow, 3);
    }
}
