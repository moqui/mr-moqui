//! Shared server-side frames. Transforms operate on the complete input, before presentation limits.
use moqui_lib::error::Error;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
type PartitionGroups = BTreeMap<String, (BTreeMap<String, String>, Vec<BTreeMap<String, Value>>)>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, Value>,
    pub values: Vec<Value>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Frame {
    pub fields: Vec<Field>,
}
impl Frame {
    pub fn len(&self) -> usize {
        self.fields.first().map_or(0, |f| f.values.len())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn rows(&self) -> Vec<BTreeMap<String, Value>> {
        (0..self.len())
            .map(|i| {
                self.fields
                    .iter()
                    .map(|f| (f.name.clone(), f.values.get(i).cloned().unwrap_or(Value::Null)))
                    .collect()
            })
            .collect()
    }
    pub fn from_rows(rows: &[BTreeMap<String, Value>]) -> Self {
        let keys: BTreeSet<String> = rows.iter().flat_map(|r| r.keys().cloned()).collect();
        Self {
            fields: keys
                .into_iter()
                .map(|name| {
                    let values: Vec<Value> = rows
                        .iter()
                        .map(|r| r.get(&name).cloned().unwrap_or(Value::Null))
                        .collect();
                    let sample = values.iter().find(|v| !v.is_null());
                    let kind = if sample
                        .and_then(Value::as_str)
                        .is_some_and(|s| s.parse::<jiff::Timestamp>().is_ok())
                    {
                        "time"
                    } else {
                        match sample {
                            Some(Value::Number(_)) => "number",
                            Some(Value::Bool(_)) => "boolean",
                            _ => "string",
                        }
                    };
                    Field {
                        name,
                        kind: kind.into(),
                        unit: None,
                        labels: BTreeMap::new(),
                        config: BTreeMap::new(),
                        values,
                    }
                })
                .collect(),
        }
    }
    fn select_rows(&mut self, indices: &[usize]) {
        for field in &mut self.fields {
            field.values = indices.iter().map(|i| field.values[*i].clone()).collect();
            if let Some(bucket) = field.config.get_mut("bucket").and_then(Value::as_object_mut) {
                for key in ["filters", "labels"] {
                    if let Some(values) = bucket.get_mut(key).and_then(Value::as_array_mut) {
                        *values = indices
                            .iter()
                            .map(|index| values.get(*index).cloned().unwrap_or(Value::Null))
                            .collect();
                    }
                }
            }
        }
    }
}

/// Flatten bucket trees by carrying each bucket key and every leaf metric into a row.
pub fn tabify(result: &Value) -> Frame {
    tabify_with_subtotals(result, false)
}

/// Subtotal metrics are computed by the backend at each grain, preserving weighted means,
/// distinct counts and percentiles instead of reducing the rows visible in a table.
pub fn tabify_with_subtotals(result: &Value, subtotals: bool) -> Frame {
    fn walk(
        value: &Value,
        prefix: &str,
        base: &BTreeMap<String, Value>,
        out: &mut Vec<BTreeMap<String, Value>>,
        subtotals: bool,
        depth: usize,
    ) {
        let Some(map) = value.as_object() else {
            return;
        };
        let mut row = base.clone();
        let mut branches = Vec::new();
        for (name, value) in map {
            if matches!(name.as_str(), "__bucket" | "__provenance") {
                continue;
            }
            let name = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            if let (Some(bounds), Some(counts)) = (value["bounds"].as_array(), value["buckets"].as_array()) {
                let bounds: Vec<f64> = bounds.iter().filter_map(Value::as_f64).collect();
                let counts: Vec<u64> = counts.iter().filter_map(Value::as_u64).collect();
                for (percent, quantile) in [("p50", 0.50), ("p95", 0.95), ("p99", 0.99)] {
                    row.insert(
                        format!("{name}.{percent}"),
                        moqui_core::telemetry::maintenance::histogram_percentile(&bounds, &counts, quantile)
                            .map(numeric)
                            .unwrap_or(Value::Null),
                    );
                }
                for (index, count) in counts.iter().enumerate() {
                    row.insert(format!("{name}.bucket.{index}"), json!(count));
                }
                let mut lower = bounds.first().copied().unwrap_or(0.0).min(0.0);
                let mut sum = 0.0;
                let mut sum2 = 0.0;
                let mut total = 0_u64;
                for (index, count) in counts.iter().enumerate() {
                    let upper = bounds
                        .get(index)
                        .copied()
                        .or_else(|| bounds.last().copied())
                        .unwrap_or(0.0);
                    let midpoint = if index < bounds.len() {
                        (lower + upper) / 2.0
                    } else {
                        upper
                    };
                    sum += midpoint * (*count as f64);
                    sum2 += midpoint * midpoint * (*count as f64);
                    total = total.saturating_add(*count);
                    lower = upper;
                }
                row.insert(
                    format!("{name}.stddev"),
                    if total > 0 {
                        numeric((sum2 / total as f64 - (sum / total as f64).powi(2)).max(0.0).sqrt())
                    } else {
                        Value::Null
                    },
                );
            } else if contains_buckets(value) {
                branches.push((name, value));
            } else if value.is_object() {
                for (k, v) in value.as_object().unwrap() {
                    if !v.is_array() && !v.is_object() {
                        row.insert(format!("{name}.{k}"), v.clone());
                    }
                }
            } else if value.is_array() {
                row.insert(name, Value::String(value.to_string()));
            } else {
                row.insert(name, value.clone());
            }
        }
        if branches.is_empty() {
            if subtotals {
                row.insert("__subtotal".into(), json!(false));
                row.insert("__level".into(), json!(depth));
            }
            out.push(row);
            return;
        }
        if subtotals {
            let mut subtotal = row.clone();
            subtotal.insert("__subtotal".into(), json!(true));
            subtotal.insert("__level".into(), json!(depth));
            out.push(subtotal);
        }
        for (name, branch) in branches {
            if let Some(buckets) = branch["buckets"].as_array() {
                for bucket in buckets {
                    let mut next = row.clone();
                    next.insert(name.clone(), bucket["key"].clone());
                    if bucket["__bucket"].is_object() {
                        let provenance = next.entry("__provenance".into()).or_insert_with(|| json!({}));
                        provenance[&name] = bucket["__bucket"].clone();
                    }
                    if let Some(count) = bucket.get("doc_count") {
                        next.insert("count".into(), count.clone());
                    }
                    let mut child = bucket.as_object().cloned().unwrap_or_default();
                    child.remove("key");
                    child.remove("key_display");
                    child.remove("doc_count");
                    child.remove("__bucket");
                    walk(&Value::Object(child), "", &next, out, subtotals, depth + 1);
                }
            } else {
                walk(branch, &name, &row, out, subtotals, depth);
            }
        }
    }
    fn contains_buckets(value: &Value) -> bool {
        value.as_object().is_some_and(|map| {
            map.iter()
                .any(|(name, value)| name == "buckets" || contains_buckets(value))
        })
    }
    let mut rows = Vec::new();
    walk(result, "", &BTreeMap::new(), &mut rows, subtotals, 0);
    let mut frame = Frame::from_rows(&rows);
    let provenance = frame
        .fields
        .iter()
        .position(|field| field.name == "__provenance")
        .map(|index| frame.fields.remove(index).values);
    if let Some(provenance) = provenance {
        for field in &mut frame.fields {
            let values = provenance
                .iter()
                .map(|row| row.get(&field.name).cloned().unwrap_or(Value::Null))
                .collect::<Vec<_>>();
            if let Some(first) = values.iter().find(|value| value.is_object()) {
                field.config.insert("bucket".into(),json!({"datasetId":first["datasetId"],"field":first["field"],"kind":first["kind"],"filters":values.iter().map(|value|value["query"].clone()).collect::<Vec<_>>(),"labels":values.iter().map(|value|value["label"].clone()).collect::<Vec<_>>() }));
            }
        }
    }
    frame
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}
fn text<'a>(value: &'a Value, key: &str, default: &'a str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or(default)
}
fn numeric(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}
pub(crate) fn reduce(values: &[Value], calculation: &str) -> Value {
    let nums: Vec<f64> = values.iter().filter_map(number).collect();
    match calculation {
        "count" => json!(values.iter().filter(|v| !v.is_null()).count()),
        "last" => values
            .iter()
            .rev()
            .find(|v| !v.is_null())
            .cloned()
            .unwrap_or(Value::Null),
        "first" => values.iter().find(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
        _ if nums.is_empty() => Value::Null,
        "sum" => numeric(nums.iter().sum()),
        "mean" | "avg" => numeric(nums.iter().sum::<f64>() / nums.len() as f64),
        "min" => numeric(nums.iter().copied().fold(f64::INFINITY, f64::min)),
        "max" => numeric(nums.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        "delta" => numeric(nums.last().unwrap() - nums.first().unwrap()),
        name if name.starts_with('p') => {
            let Some(p) = name[1..].parse::<f64>().ok().filter(|p| (0.0..=100.0).contains(p)) else {
                return Value::Null;
            };
            let mut nums = nums;
            nums.sort_by(f64::total_cmp);
            let position = p / 100.0 * (nums.len() - 1) as f64;
            let low = position.floor() as usize;
            let hi = position.ceil() as usize;
            numeric(nums[low] + (nums[hi] - nums[low]) * position.fract())
        }
        _ => Value::Null,
    }
}

/// The calculations of a reduce or group-by step: `calculations: ["last", "max"]`, or the one `calculation`
/// (a name, or a list of names) older definitions have.
fn calculation_names(step: &Value, default: &str) -> Vec<String> {
    let names = |value: &Value| -> Option<Vec<String>> {
        match value {
            Value::String(name) => Some(vec![name.clone()]),
            Value::Array(names) => Some(names.iter().filter_map(Value::as_str).map(str::to_owned).collect()),
            _ => None,
        }
    };
    names(&step["calculations"])
        .or_else(|| names(&step["calculation"]))
        .filter(|names| !names.is_empty())
        .unwrap_or_else(|| vec![default.to_owned()])
}

/// Grafana's "series to rows": a row for each number field, with its name in `field`, a column for each calculation,
/// and the field's labels.
fn series_to_rows(frame: &Frame, calculations: &[String]) -> Frame {
    let series: Vec<&Field> = frame.fields.iter().filter(|field| field.kind == "number").collect();
    let label_names: BTreeSet<&String> = series.iter().flat_map(|field| field.labels.keys()).collect();
    let column = |name: &str, kind: &str, values: Vec<Value>| Field {
        name: name.into(),
        kind: kind.into(),
        unit: None,
        labels: BTreeMap::new(),
        config: BTreeMap::new(),
        values,
    };
    let mut fields = vec![column(
        "field",
        "string",
        series.iter().map(|field| json!(field.name)).collect(),
    )];
    for calculation in calculations {
        let mut out = column(
            calculation,
            "number",
            series.iter().map(|field| reduce(&field.values, calculation)).collect(),
        );
        // The unit carries over when every series has the same one, and is not a count.
        let units: BTreeSet<_> = series.iter().map(|field| &field.unit).collect();
        if calculation != "count" && units.len() == 1 {
            out.unit = units.into_iter().next().cloned().flatten();
        }
        fields.push(out);
    }
    for label in label_names {
        fields.push(column(
            label,
            "string",
            series
                .iter()
                .map(|field| field.labels.get(label).map_or(Value::Null, |value| json!(value)))
                .collect(),
        ));
    }
    Frame { fields }
}

/// One added field: binary math (`left` with `right` or a constant `value`, by `operator`), or a running total,
/// moving average, delta or percent of total of `field`. In a `calculate` step an item picks the running kinds with
/// `mode`. `listed` is an item of `calculations`, whose default name says which field it came from.
fn calculate_field(frame: &Frame, kind: &str, item: &Value, listed: bool) -> Result<Field, Error> {
    let kind = if kind == "calculate" {
        match text(item, "mode", "binary") {
            "binary" => "calculate",
            mode @ ("runningTotal" | "movingAverage" | "delta" | "percentOfTotal") => mode,
            mode => return Err(Error::new(format!("calculation mode {mode} is not known"))),
        }
    } else {
        kind
    };
    let field = |name: String, values: Vec<Value>| Field {
        name,
        kind: "number".into(),
        unit: None,
        labels: BTreeMap::new(),
        config: BTreeMap::new(),
        values,
    };
    if kind == "calculate" {
        let left = text(item, "left", "");
        let right = text(item, "right", "");
        let op = text(item, "operator", "+");
        let left_field = frame
            .fields
            .iter()
            .find(|f| f.name == left)
            .ok_or_else(|| Error::new(format!("calculation left field {left} is missing")))?;
        let right_field = frame.fields.iter().find(|f| f.name == right);
        let constant = number(&item["value"]);
        let values = left_field
            .values
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let b = right_field.and_then(|f| f.values.get(i)).and_then(number).or(constant);
                match (number(a), b) {
                    (Some(a), Some(b)) => match op {
                        "+" => numeric(a + b),
                        "-" => numeric(a - b),
                        "*" => numeric(a * b),
                        "/" | "ratio" if b != 0.0 => numeric(a / b),
                        _ => Value::Null,
                    },
                    _ => Value::Null,
                }
            })
            .collect();
        let name = match item["name"].as_str() {
            Some(name) => name.to_owned(),
            None if listed => {
                let other = if right_field.is_some() {
                    right.to_owned()
                } else {
                    item["value"].to_string()
                };
                format!("{left} {op} {other}")
            }
            None => "calculated".to_owned(),
        };
        return Ok(field(name, values));
    }
    let name = text(item, "field", "");
    let source = frame
        .fields
        .iter()
        .find(|f| f.name == name)
        .ok_or_else(|| Error::new(format!("transform field {name} is missing")))?;
    let total = source.values.iter().filter_map(number).sum::<f64>();
    let mut sum = 0.0;
    let mut previous = None;
    let window = item["window"].as_u64().unwrap_or(5).max(1) as usize;
    let values = source
        .values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let Some(value) = number(value) else {
                return Value::Null;
            };
            match kind {
                "runningTotal" => {
                    sum += value;
                    numeric(sum)
                }
                "movingAverage" => reduce(&source.values[i.saturating_add(1).saturating_sub(window)..=i], "mean"),
                "delta" => {
                    let result = previous.map(|p| numeric(value - p)).unwrap_or(Value::Null);
                    previous = Some(value);
                    result
                }
                _ if total != 0.0 => numeric(value / total * 100.0),
                _ => Value::Null,
            }
        })
        .collect();
    let name = match item["name"].as_str() {
        Some(name) => name.to_owned(),
        None if listed => format!("{name}.{kind}"),
        None => kind.to_owned(),
    };
    Ok(field(name, values))
}

/// Order two values: numbers by value, times by instant, other text as text.
fn compare_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    if let (Some(a), Some(b)) = (a.as_f64(), b.as_f64()) {
        return a.partial_cmp(&b);
    }
    match (a, b) {
        (Value::String(a), Value::String(b)) => match (a.parse::<jiff::Timestamp>(), b.parse::<jiff::Timestamp>()) {
            (Ok(a), Ok(b)) => Some(a.cmp(&b)),
            _ => match (a.parse::<f64>(), b.parse::<f64>()) {
                (Ok(a), Ok(b)) => a.partial_cmp(&b),
                _ => Some(a.cmp(b)),
            },
        },
        _ => number(a).zip(number(b)).and_then(|(a, b)| a.partial_cmp(&b)),
    }
}

/// The rows a filter-by-value step keeps. A step is one condition (`field`, `operator`, `value`, and `from`/`to`
/// for `range`), or several in `conditions` joined by `match` (`all`, the default, or `any`); `exclude` drops the
/// matching rows instead.
fn filter_rows(frame: &Frame, step: &Value) -> Result<Vec<usize>, Error> {
    struct Condition<'s> {
        field: &'s Field,
        operator: &'s str,
        spec: &'s Value,
        pattern: Option<regex::Regex>,
    }
    let specs: Vec<&Value> = match step["conditions"].as_array() {
        Some(conditions) => conditions.iter().collect(),
        None => vec![step],
    };
    let mut conditions = Vec::with_capacity(specs.len());
    for spec in specs {
        let name = text(spec, "field", "");
        let field = frame
            .fields
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| Error::new(format!("filter field {name} is missing")))?;
        let operator = text(spec, "operator", "eq");
        if !matches!(
            operator,
            "eq" | "ne" | "gt" | "gte" | "lt" | "lte" | "range" | "regex" | "exists" | "missing"
        ) {
            return Err(Error::new(format!("filter operator {operator} is not known")));
        }
        let pattern = if operator == "regex" {
            let source = spec["value"]
                .as_str()
                .ok_or_else(|| Error::new("a regex filter needs a pattern in value"))?;
            Some(
                regex::RegexBuilder::new(source)
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|e| Error::new(format!("filter regex {source} is not valid: {e}")))?,
            )
        } else {
            None
        };
        conditions.push(Condition {
            field,
            operator,
            spec,
            pattern,
        });
    }
    let any = match text(step, "match", "all") {
        "all" => false,
        "any" => true,
        other => return Err(Error::new(format!("filter match {other} is not all or any"))),
    };
    let exclude = step["exclude"] == true;
    let holds = |condition: &Condition<'_>, row: usize| -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        let value = condition.field.values.get(row).unwrap_or(&Value::Null);
        let expected = &condition.spec["value"];
        let order = |bound: &Value| compare_values(value, bound);
        match condition.operator {
            "eq" => value == expected,
            "ne" => value != expected,
            _ if value.is_null() => condition.operator == "missing",
            "gt" => order(expected) == Some(Greater),
            "gte" => matches!(order(expected), Some(Greater | Equal)),
            "lt" => order(expected) == Some(Less),
            "lte" => matches!(order(expected), Some(Less | Equal)),
            "range" => {
                let from = &condition.spec["from"];
                let to = &condition.spec["to"];
                (from.is_null() || matches!(order(from), Some(Greater | Equal)))
                    && (to.is_null() || matches!(order(to), Some(Less | Equal)))
            }
            "regex" => {
                let text = value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string());
                condition
                    .pattern
                    .as_ref()
                    .is_some_and(|pattern| pattern.is_match(&text))
            }
            "exists" => true,
            _ => false,
        }
    };
    Ok((0..frame.len())
        .filter(|row| {
            let matched = if any {
                conditions.iter().any(|condition| holds(condition, *row))
            } else {
                conditions.iter().all(|condition| holds(condition, *row))
            };
            matched != exclude
        })
        .collect())
}

pub fn transform(mut frame: Frame, steps: &[Value]) -> Result<Frame, Error> {
    for (i, step) in steps.iter().enumerate().filter(|(_, s)| s["disabled"] != true) {
        let kind = text(step, "type", "");
        if frame.is_empty()
            && matches!(
                kind,
                "organize"
                    | "limit"
                    | "sort"
                    | "reduce"
                    | "filter"
                    | "calculate"
                    | "runningTotal"
                    | "movingAverage"
                    | "delta"
                    | "percentOfTotal"
                    | "groupBy"
                    | "pivot"
                    | "timeShift"
            )
        {
            continue;
        }
        match kind {
            "organize" => {
                if let Some(hidden) = step["hide"].as_array() {
                    frame
                        .fields
                        .retain(|f| !hidden.iter().any(|n| n.as_str() == Some(&f.name)));
                }
                for field in &mut frame.fields {
                    if let Some(rename) = step["rename"][&field.name].as_str() {
                        field.name = rename.into();
                    }
                }
                if let Some(order) = step["order"].as_array() {
                    frame.fields.sort_by_key(|f| {
                        order
                            .iter()
                            .position(|v| v.as_str() == Some(&f.name))
                            .unwrap_or(usize::MAX)
                    });
                }
            }
            "limit" => {
                let limit = step["count"].as_u64().unwrap_or(100) as usize;
                for field in &mut frame.fields {
                    field.values.truncate(limit);
                }
            }
            "sort" => {
                let name = text(step, "field", "");
                let field = frame
                    .fields
                    .iter()
                    .find(|f| f.name == name)
                    .ok_or_else(|| Error::new(format!("sort field {name} is missing")))?;
                let mut indices: Vec<usize> = (0..frame.len()).collect();
                indices.sort_by(|a, b| match (number(&field.values[*a]), number(&field.values[*b])) {
                    (Some(a), Some(b)) => a.total_cmp(&b),
                    _ => field.values[*a].to_string().cmp(&field.values[*b].to_string()),
                });
                if step["descending"] == true {
                    indices.reverse();
                }
                frame.select_rows(&indices);
            }
            "reduce" => {
                let calculations = calculation_names(step, "last");
                if step["mode"] == "rows" {
                    frame = series_to_rows(&frame, &calculations);
                } else {
                    let several = calculations.len() > 1;
                    frame.fields = frame
                        .fields
                        .into_iter()
                        .flat_map(|field| {
                            calculations.iter().map(move |calculation| {
                                let mut out = field.clone();
                                out.values = vec![reduce(&field.values, calculation)];
                                if several {
                                    out.name = format!("{}.{calculation}", field.name);
                                }
                                if calculation != "last" && calculation != "first" {
                                    out.kind = "number".into();
                                }
                                out
                            })
                        })
                        .collect();
                }
            }
            "filter" => {
                let indices = filter_rows(&frame, step)?;
                frame.select_rows(&indices);
            }
            "calculate" | "runningTotal" | "movingAverage" | "delta" | "percentOfTotal" => {
                // One calculation as the step itself, or several in `calculations` (and for the running kinds,
                // `fields`), each able to use the fields the ones before it added.
                let listed = step["calculations"].as_array().cloned().or_else(|| {
                    (kind != "calculate")
                        .then(|| step["fields"].as_array())
                        .flatten()
                        .map(|fields| fields.iter().map(|field| json!({"field":field})).collect())
                });
                let several = listed.is_some();
                for item in listed.unwrap_or_else(|| vec![step.clone()]) {
                    let field = calculate_field(&frame, kind, &item, several)?;
                    frame.fields.push(field);
                }
            }
            "groupBy" => {
                let keys: Vec<&str> = step["fields"]
                    .as_array()
                    .map(|v| v.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let calculations = calculation_names(step, "sum");
                let mut groups: BTreeMap<String, Vec<BTreeMap<String, Value>>> = BTreeMap::new();
                for row in frame.rows() {
                    let key = serde_json::to_string(&keys.iter().map(|k| row.get(*k)).collect::<Vec<_>>()).unwrap();
                    groups.entry(key).or_default().push(row);
                }
                let rows: Vec<_> = groups
                    .into_values()
                    .map(|rows| {
                        let mut out = BTreeMap::new();
                        for field in &frame.fields {
                            if keys.contains(&field.name.as_str()) {
                                out.insert(
                                    field.name.clone(),
                                    rows[0].get(&field.name).cloned().unwrap_or(Value::Null),
                                );
                                continue;
                            }
                            let values = rows.iter().map(|r| r[&field.name].clone()).collect::<Vec<_>>();
                            for calculation in &calculations {
                                let name = if calculations.len() > 1 {
                                    format!("{}.{calculation}", field.name)
                                } else {
                                    field.name.clone()
                                };
                                out.insert(name, reduce(&values, calculation));
                            }
                        }
                        out
                    })
                    .collect();
                frame = Frame::from_rows(&rows);
            }
            "pivot" => {
                let row = text(step, "row", "");
                let column = text(step, "column", "");
                let cell = text(step, "cell", "");
                let mut groups: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
                for item in frame.rows() {
                    let key = item.get(row).cloned().unwrap_or(Value::Null);
                    let name = item
                        .get(column)
                        .map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))
                        .unwrap_or_default();
                    let group = groups.entry(key.to_string()).or_default();
                    group.insert(row.into(), key);
                    let current = group.get(&name).and_then(number).unwrap_or(0.0);
                    group.insert(name, numeric(current + item.get(cell).and_then(number).unwrap_or(0.0)));
                }
                let mut rows = groups.into_values().collect::<Vec<_>>();
                if step["rowTotals"] == true {
                    for item in &mut rows {
                        let total = item
                            .iter()
                            .filter(|(key, _)| key.as_str() != row)
                            .filter_map(|(_, value)| number(value))
                            .sum::<f64>();
                        item.insert("Total".into(), numeric(total));
                    }
                }
                if step["columnTotals"] == true {
                    let mut totals = BTreeMap::from([(row.to_owned(), json!("Total"))]);
                    for item in &rows {
                        for (key, value) in item.iter().filter(|(key, _)| key.as_str() != row) {
                            if let Some(value) = number(value) {
                                let current = totals.get(key).and_then(number).unwrap_or(0.0);
                                totals.insert(key.clone(), numeric(current + value));
                            }
                        }
                    }
                    rows.push(totals);
                }
                frame = Frame::from_rows(&rows);
            }
            "timeShift" => {
                let millis = step["millis"]
                    .as_i64()
                    .ok_or_else(|| Error::new("timeShift needs millis"))?;
                let name = text(step, "field", "time");
                if let Some(field) = frame.fields.iter_mut().find(|f| f.name == name) {
                    for v in &mut field.values {
                        if let Some(n) = v.as_i64() {
                            *v = json!(n.saturating_add(millis));
                        } else if let Some(time) = v.as_str().and_then(|s| s.parse::<jiff::Timestamp>().ok()) {
                            *v = json!(
                                jiff::Timestamp::from_millisecond(time.as_millisecond().saturating_add(millis))
                                    .map_err(|e| Error::new(e.to_string()))?
                                    .to_string()
                            );
                        }
                    }
                }
            }
            "patterns" => frame = super::patterns::transform(&frame, step)?,
            _ => {
                return Err(Error::new(format!(
                    "report transformation {i} has unsupported type {kind}"
                )))
            }
        }
    }
    Ok(frame)
}

/// Multi-query transformations run on the server too: an alert, export and panel see the same
/// joined values. Every join is bounded before constructing its output.
pub fn transform_frames(mut frames: Vec<Frame>, steps: &[Value]) -> Result<Vec<Frame>, Error> {
    for step in steps.iter().filter(|s| s["disabled"] != true) {
        moqui_core::search::limits::remaining()?;
        match text(step, "type", "") {
            "comparePrevious" => {
                if !frames.len().is_multiple_of(2) {
                    return Err(Error::new("comparePrevious needs paired current and previous frames"));
                }
                let previous = frames.split_off(frames.len() / 2);
                let keys = step["fields"].as_array().map(|fields| {
                    fields
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                });
                frames = compare_previous(
                    frames,
                    previous,
                    step["shiftMillis"].as_i64().unwrap_or(0),
                    keys.as_deref(),
                )?;
            }
            "join" | "joinByTime" => {
                let keys: Vec<String> = step["fields"]
                    .as_array()
                    .map(|values| values.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                    .unwrap_or_else(|| vec![text(step, "field", "time").to_owned()]);
                if keys.is_empty() {
                    return Err(Error::new("join requires at least one key field"));
                }
                let mut joined: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
                for (index, frame) in frames.iter().enumerate() {
                    let mut seen = BTreeSet::new();
                    for row in frame.rows() {
                        let key: Vec<_> = keys
                            .iter()
                            .map(|key| {
                                row.get(key)
                                    .cloned()
                                    .ok_or_else(|| Error::new(format!("join key {key} is missing")))
                            })
                            .collect::<Result<_, _>>()?;
                        let token = serde_json::to_string(&key).unwrap();
                        if !seen.insert(token.clone()) {
                            return Err(Error::new("join key is not unique in one input frame; group it first"));
                        }
                        let output = joined.entry(token).or_default();
                        for (name, value) in row {
                            let target = if !keys.contains(&name) && output.contains_key(&name) {
                                format!("{name}.{}", index + 1)
                            } else {
                                name
                            };
                            output.insert(target, value);
                        }
                        if joined.len() > 65_536 {
                            return Err(Error::new("joined report has too many rows"));
                        }
                    }
                }
                frames = vec![Frame::from_rows(&joined.into_values().collect::<Vec<_>>())];
            }
            "merge" => {
                let rows: Vec<_> = frames.iter().flat_map(Frame::rows).collect();
                if rows.len() > 65_536 {
                    return Err(Error::new("merged report has too many rows"));
                }
                frames = vec![Frame::from_rows(&rows)];
            }
            "partition" | "partitionByValues" => {
                let keys: Vec<_> = step["fields"]
                    .as_array()
                    .map(|values| values.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let mut groups = PartitionGroups::new();
                for frame in &frames {
                    for row in frame.rows() {
                        let labels: BTreeMap<_, _> = keys
                            .iter()
                            .map(|key| {
                                (
                                    key.to_string(),
                                    row.get(*key)
                                        .map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))
                                        .unwrap_or_default(),
                                )
                            })
                            .collect();
                        let token = serde_json::to_string(&labels).unwrap();
                        groups.entry(token).or_insert_with(|| (labels, Vec::new())).1.push(row);
                    }
                }
                frames = groups
                    .into_values()
                    .map(|(labels, rows)| {
                        let mut frame = Frame::from_rows(&rows);
                        for field in &mut frame.fields {
                            field.labels = labels.clone();
                        }
                        frame
                    })
                    .collect();
            }
            "longToWide" => {
                let pivot = json!({"type":"pivot","row":text(step,"timeField","time"),"column":text(step,"seriesField","series"),"cell":text(step,"valueField","value")});
                frames = frames
                    .into_iter()
                    .map(|frame| transform(frame, std::slice::from_ref(&pivot)))
                    .collect::<Result<_, _>>()?;
            }
            "wideToLong" => {
                let time = text(step, "timeField", "time");
                let mut rows = Vec::new();
                for frame in &frames {
                    for row in frame.rows() {
                        for field in frame
                            .fields
                            .iter()
                            .filter(|field| field.kind == "number" && field.name != time)
                        {
                            let mut output = BTreeMap::from([
                                ("series".into(), json!(field.name)),
                                ("value".into(), row.get(&field.name).cloned().unwrap_or(Value::Null)),
                            ]);
                            if let Some(value) = row.get(time) {
                                output.insert(time.into(), value.clone());
                            }
                            for (key, value) in &field.labels {
                                output.insert(key.clone(), json!(value));
                            }
                            rows.push(output);
                        }
                    }
                }
                frames = vec![Frame::from_rows(&rows)];
            }
            "timeSeriesToTable" => {
                let mut rows = Vec::new();
                for frame in &frames {
                    for field in frame.fields.iter().filter(|field| field.kind == "number") {
                        let mut row = BTreeMap::from([
                            ("series".into(), json!(field.name)),
                            ("last".into(), reduce(&field.values, "last")),
                            ("min".into(), reduce(&field.values, "min")),
                            ("max".into(), reduce(&field.values, "max")),
                            ("mean".into(), reduce(&field.values, "mean")),
                            ("sparkline".into(), json!(serde_json::to_string(&field.values).unwrap())),
                        ]);
                        for (key, value) in &field.labels {
                            row.insert(key.clone(), json!(value));
                        }
                        rows.push(row);
                    }
                }
                frames = vec![Frame::from_rows(&rows)];
            }
            _ => {
                frames = frames
                    .into_iter()
                    .map(|frame| transform(frame, std::slice::from_ref(step)))
                    .collect::<Result<_, _>>()?;
            }
        }
    }
    Ok(frames)
}

/// Align the previous period to the current time axis and calculate changes from the same
/// joined values used by panels and alerts. A missing/zero baseline leaves percent change null.
pub fn compare_previous(
    mut current: Vec<Frame>,
    mut previous: Vec<Frame>,
    shift: i64,
    explicit_keys: Option<&[String]>,
) -> Result<Vec<Frame>, Error> {
    let count = current.len().max(previous.len());
    current.resize_with(count, Frame::default);
    previous.resize_with(count, Frame::default);
    let mut output = Vec::new();
    for (mut current, mut previous) in current.into_iter().zip(previous) {
        let mut keys = current
            .fields
            .iter()
            .chain(&previous.fields)
            .filter(|field| match explicit_keys {
                Some(keys) => keys.contains(&field.name),
                None => {
                    field.kind != "number"
                        && !field.values.iter().find_map(Value::as_str).is_some_and(|s| {
                            serde_json::from_str::<Value>(s).is_ok_and(|v| v.is_array() || v.is_object())
                        })
                }
            })
            .map(|field| field.name.clone())
            .collect::<BTreeSet<_>>();
        for field in previous
            .fields
            .iter_mut()
            .filter(|field| field.kind == "time" && keys.contains(&field.name))
        {
            for value in &mut field.values {
                if let Some(millis) = value.as_i64() {
                    *value = json!(millis.saturating_add(shift));
                } else if let Some(time) = value.as_str().and_then(|s| s.parse::<jiff::Timestamp>().ok()) {
                    *value = json!(
                        jiff::Timestamp::from_millisecond(time.as_millisecond().saturating_add(shift))
                            .map_err(|e| Error::new(e.to_string()))?
                            .to_string()
                    );
                }
            }
        }
        let metrics = current
            .fields
            .iter()
            .chain(&previous.fields)
            .filter(|field| !keys.contains(&field.name))
            .map(|field| field.name.clone())
            .collect::<BTreeSet<_>>();
        let numeric_metrics = current
            .fields
            .iter()
            .chain(&previous.fields)
            .filter(|field| field.kind == "number" && !keys.contains(&field.name))
            .map(|field| field.name.clone())
            .collect::<BTreeSet<_>>();
        for field in &mut previous.fields {
            if metrics.contains(&field.name) {
                field.name = format!("{}.previous", field.name);
            }
        }
        let scalar = keys.is_empty();
        if scalar {
            keys.insert("__comparison_row".into());
            for frame in [&mut current, &mut previous] {
                let len = frame.len();
                frame.fields.push(Field {
                    name: "__comparison_row".into(),
                    kind: "number".into(),
                    unit: None,
                    labels: BTreeMap::new(),
                    config: BTreeMap::new(),
                    values: (0..len).map(|index| json!(index)).collect(),
                });
            }
        }
        let mut joined = transform_frames(
            vec![current, previous],
            &[json!({"type":"join","fields":keys.into_iter().collect::<Vec<_>>()})],
        )?
        .pop()
        .unwrap_or_default();
        let mut rows = joined.rows();
        for row in &mut rows {
            for metric in &metrics {
                let previous = format!("{metric}.previous");
                let pair = row
                    .get(metric)
                    .and_then(number)
                    .zip(row.get(&previous).and_then(number));
                row.entry(metric.clone()).or_insert(Value::Null);
                row.entry(previous).or_insert(Value::Null);
                if numeric_metrics.contains(metric) {
                    let delta = pair.map(|(current, previous)| current - previous);
                    let percent = pair
                        .filter(|(_, previous)| *previous != 0.0)
                        .map(|(current, previous)| (current - previous) / previous.abs() * 100.0);
                    row.insert(format!("{metric}.change"), delta.map(numeric).unwrap_or(Value::Null));
                    row.insert(
                        format!("{metric}.percent_change"),
                        percent.map(numeric).unwrap_or(Value::Null),
                    );
                }
            }
            if scalar {
                row.remove("__comparison_row");
            }
        }
        joined = Frame::from_rows(&rows);
        for field in &mut joined.fields {
            if field.name.ends_with(".percent_change") {
                field.unit = Some("percent".into());
            }
        }
        output.push(joined);
    }
    Ok(output)
}

fn inspection_snapshot(frames: &[Frame], remaining: &mut usize) -> Value {
    if *remaining < 256 {
        return json!({"frames":[],"truncated":true,"reason":"inspection snapshot budget exhausted"});
    }
    let mut output = Vec::new();
    let mut truncated = frames.len() > 8;
    let start = *remaining;
    for frame in frames.iter().take(8) {
        let mut fields = Vec::new();
        truncated |= frame.fields.len() > 32 || frame.len() > 20;
        for field in frame.fields.iter().take(32) {
            let values = field
                .values
                .iter()
                .take(20)
                .map(|value| match value {
                    Value::String(text) if text.len() > 256 => {
                        truncated = true;
                        json!(text.chars().take(256).collect::<String>())
                    }
                    Value::Array(_) | Value::Object(_) => {
                        truncated = true;
                        json!(value.to_string().chars().take(256).collect::<String>())
                    }
                    _ => value.clone(),
                })
                .collect::<Vec<_>>();
            let item =
                json!({"name":field.name,"type":field.kind,"unit":field.unit,"labels":field.labels,"values":values});
            let bytes = item.to_string().len();
            if bytes > *remaining || start.saturating_sub(*remaining) + bytes > 65_536 {
                truncated = true;
                break;
            }
            *remaining -= bytes;
            fields.push(item);
        }
        output.push(json!({"fields":fields,"rowCount":frame.len()}));
    }
    json!({"frames":output,"truncated":truncated})
}

/// Snapshots are opt-in, sampled and bounded to one MiB for a pipeline. They never affect the
/// actual values transformed or exported; every stage explicitly identifies truncated samples.
pub fn transform_inspected(mut frames: Vec<Frame>, steps: &[Value]) -> Result<(Vec<Frame>, Vec<Value>), Error> {
    let mut remaining = 1_048_576;
    let mut inspection = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let input = inspection_snapshot(&frames, &mut remaining);
        frames = transform_frames(frames, std::slice::from_ref(step))?;
        let output = inspection_snapshot(&frames, &mut remaining);
        inspection.push(
            json!({"index":index,"type":step["type"],"disabled":step["disabled"]==true,"input":input,"output":output}),
        );
    }
    Ok((frames, inspection))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn negative_histogram_first_bucket_uses_its_nonpositive_upper_bound() {
        let frame = tabify(&json!({"distribution":{"bounds":[-5,5],"buckets":[2,2,0]}}));
        assert_eq!(frame.rows()[0]["distribution.stddev"], 2.5);
    }
    #[test]
    fn nested_buckets_tabify() {
        let frame = tabify(
            &json!({"month":{"buckets":[{"key":"Jan","doc_count":2,"status":{"buckets":[{"key":"paid","doc_count":2,"sales":15}]}}]}}),
        );
        assert_eq!(frame.rows()[0]["month"], "Jan");
        assert_eq!(frame.rows()[0]["status"], "paid");
        assert_eq!(frame.rows()[0]["sales"], 15);
    }
    #[test]
    fn intermediate_subtotals_use_backend_metrics_and_filters_keep_their_buckets() {
        let frame = tabify_with_subtotals(
            &json!({"average":34.6666666667,"region":{"buckets":[{"key":"A","doc_count":2,"average":2,"product":{"buckets":[{"key":"x","doc_count":1,"average":1},{"key":"y","doc_count":1,"average":3}]}},{"key":"B","doc_count":1,"average":100,"product":{"buckets":[{"key":"z","doc_count":1,"average":100}]}}]}}),
            true,
        );
        let rows = frame.rows();
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0]["average"], 34.6666666667);
        assert_eq!(rows[0]["__subtotal"], true);
        assert_eq!(rows[1]["average"], 2);
        assert_eq!(rows[2]["__subtotal"], false);
        let filtered =
            tabify(&json!({"paid":{"doc_count":1,"region":{"buckets":[{"key":"A","doc_count":1,"value":7}]}}}));
        assert_eq!(filtered.rows()[0]["paid.region"], "A");
        assert_eq!(filtered.rows()[0]["value"], 7);
    }
    #[test]
    fn ratio_null_and_totals_before_limit() {
        let frame = Frame::from_rows(&[
            BTreeMap::from([("sales".into(), json!(3)), ("count".into(), json!(0))]),
            BTreeMap::from([("sales".into(), json!(1)), ("count".into(), json!(2))]),
        ]);
        let result = transform(
            frame,
            &[
                json!({"type":"percentOfTotal","field":"sales"}),
                json!({"type":"calculate","left":"sales","right":"count","operator":"/","name":"ratio"}),
                json!({"type":"limit","count":1}),
            ],
        )
        .unwrap();
        assert_eq!(result.rows()[0]["percentOfTotal"], 75.0);
        assert_eq!(result.rows()[0]["ratio"], Value::Null);
    }
    #[test]
    fn disabled_and_unknown_steps() {
        let frame = Frame::default();
        assert!(transform(frame.clone(), &[json!({"type":"unknown"})]).is_err());
        assert_eq!(
            transform(frame.clone(), &[json!({"type":"unknown","disabled":true})]).unwrap(),
            frame
        );
    }
    #[test]
    fn joins_query_frames_before_shared_calculations() {
        let duration = Frame::from_rows(&[BTreeMap::from([
            ("name".into(), json!("service")),
            ("hits".into(), json!(4)),
            ("total".into(), json!(10)),
        ])]);
        let slow = Frame::from_rows(&[BTreeMap::from([
            ("name".into(), json!("service")),
            ("slow_hits".into(), json!(1)),
        ])]);
        let result = transform_frames(
            vec![duration, slow],
            &[
                json!({"type":"join","fields":["name"]}),
                json!({"type":"calculate","left":"total","right":"hits","operator":"/","name":"avg"}),
            ],
        )
        .unwrap();
        assert_eq!(result[0].rows()[0]["avg"], 2.5);
        assert_eq!(result[0].rows()[0]["slow_hits"], 1);
    }
    #[test]
    fn pivot_totals_come_from_complete_input() {
        let input = Frame::from_rows(&[
            BTreeMap::from([
                ("row".into(), json!("A")),
                ("column".into(), json!("X")),
                ("value".into(), json!(2)),
            ]),
            BTreeMap::from([
                ("row".into(), json!("A")),
                ("column".into(), json!("Y")),
                ("value".into(), json!(3)),
            ]),
        ]);
        let pivot=transform(input,&[json!({"type":"pivot","row":"row","column":"column","cell":"value","rowTotals":true,"columnTotals":true})]).unwrap();
        assert_eq!(pivot.rows()[0]["Total"], 5.0);
        assert_eq!(pivot.rows()[1]["Total"], 5.0);
    }
    #[test]
    fn long_and_wide_roundtrip_values() {
        let input = Frame::from_rows(&[BTreeMap::from([
            ("time".into(), json!("2026-01-01T00:00:00Z")),
            ("a".into(), json!(3)),
            ("b".into(), json!(5)),
        ])]);
        let output = transform_frames(
            vec![input],
            &[json!({"type":"wideToLong"}), json!({"type":"longToWide"})],
        )
        .unwrap();
        assert_eq!(output[0].rows()[0]["a"], 3.0);
        assert_eq!(output[0].rows()[0]["b"], 5.0);
        assert_eq!(output[0].fields.iter().find(|f| f.name == "time").unwrap().kind, "time");
    }
    #[test]
    fn incompatible_histogram_is_not_confused_with_bucket_tree() {
        let frame = tabify(&json!({"latency":{"bounds":[1.0,2.0],"buckets":[2,2,0]}}));
        let row = &frame.rows()[0];
        assert_eq!(row["latency.p50"], 1.0);
        assert!(row["latency.stddev"].as_f64().is_some_and(|v| v > 0.0));
    }
    #[test]
    fn previous_period_aligns_time_and_preserves_missing_baselines() {
        let current = Frame::from_rows(&[BTreeMap::from([
            ("time".into(), json!("2026-01-02T00:00:00Z")),
            ("value".into(), json!(6)),
            ("last_active".into(), json!("2026-01-02T00:05:00Z")),
        ])]);
        let previous = Frame::from_rows(&[BTreeMap::from([
            ("time".into(), json!("2026-01-01T00:00:00Z")),
            ("value".into(), json!(3)),
            ("last_active".into(), json!("2026-01-01T00:03:00Z")),
        ])]);
        let keys = vec!["time".to_owned()];
        let result = compare_previous(vec![current.clone()], vec![previous], 86_400_000, Some(&keys)).unwrap();
        let row = &result[0].rows()[0];
        assert_eq!(row["value.previous"], 3);
        assert_eq!(row["value.change"], 3.0);
        assert_eq!(row["value.percent_change"], 100.0);
        assert_eq!(row["last_active.previous"], "2026-01-01T00:03:00Z");
        let missing = compare_previous(vec![current], Vec::new(), 86_400_000, Some(&keys)).unwrap();
        assert_eq!(missing[0].rows()[0]["value.previous"], Value::Null);
        assert_eq!(missing[0].rows()[0]["value.percent_change"], Value::Null);
    }
    #[test]
    fn inspector_samples_are_bounded_without_changing_results() {
        let frame = Frame::from_rows(&[BTreeMap::from([
            ("body".into(), json!("a".repeat(1000))),
            ("value".into(), json!(2)),
        ])]);
        let (frames, inspection) = transform_inspected(
            vec![frame],
            &[
                json!({"type":"calculate","left":"value","value":3,"operator":"*","name":"total"}),
                json!({"type":"limit","count":0,"disabled":true}),
            ],
        )
        .unwrap();
        assert_eq!(frames[0].rows()[0]["total"], 6.0);
        assert_eq!(frames[0].rows()[0]["body"].as_str().unwrap().len(), 1000);
        assert_eq!(inspection.len(), 2);
        assert_eq!(inspection[0]["input"]["truncated"], true);
        assert_eq!(inspection[1]["disabled"], true);
    }

    fn series() -> Frame {
        Frame::from_rows(&[
            BTreeMap::from([
                ("time".into(), json!("2026-01-01T00:00:00Z")),
                ("a".into(), json!(1)),
                ("b".into(), json!(10)),
                ("name".into(), json!("alpha")),
            ]),
            BTreeMap::from([
                ("time".into(), json!("2026-01-02T00:00:00Z")),
                ("a".into(), json!(5)),
                ("b".into(), json!(20)),
                ("name".into(), json!("beta")),
            ]),
            BTreeMap::from([
                ("time".into(), json!("2026-01-03T00:00:00Z")),
                ("a".into(), json!(3)),
                ("b".into(), Value::Null),
                ("name".into(), json!("gamma")),
            ]),
        ])
    }

    #[test]
    fn reduce_takes_several_calculations() {
        // The one-calculation form keeps each field's name.
        let one = transform(series(), &[json!({"type":"reduce","calculation":"max"})]).unwrap();
        assert_eq!(one.rows()[0]["a"], 5.0);
        let same = transform(series(), &[json!({"type":"reduce","calculations":["max"]})]).unwrap();
        assert_eq!(one, same);
        // Several give a field per calculation, `<field>.<calculation>`.
        let several = transform(
            series(),
            &[json!({"type":"reduce","calculations":["last","max","mean"]})],
        )
        .unwrap();
        let row = &several.rows()[0];
        assert_eq!(row["a.last"], 3);
        assert_eq!(row["a.max"], 5.0);
        assert_eq!(row["a.mean"], 3.0);
        assert_eq!(row["b.last"], 20);
        assert_eq!(row["time.last"], "2026-01-03T00:00:00Z");
        assert_eq!(several.fields.len(), 12);
        // Series to rows: a row per number field, a column per calculation in the order given.
        let rows = transform(
            series(),
            &[json!({"type":"reduce","mode":"rows","calculations":["last","max","mean"]})],
        )
        .unwrap();
        let names: Vec<_> = rows.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["field", "last", "max", "mean"]);
        assert_eq!(rows.fields[0].values, vec![json!("a"), json!("b")]);
        assert_eq!(rows.fields[2].values, vec![json!(5.0), json!(20.0)]);
        assert_eq!(rows.fields[3].values, vec![json!(3.0), json!(15.0)]);
        // A group-by with several calculations.
        let grouped = transform(
            series(),
            &[
                json!({"type":"organize","hide":["time","name"]}),
                json!({"type":"calculate","left":"a","value":0,"operator":"*","name":"k"}),
                json!({"type":"groupBy","fields":["k"],"calculations":["sum","max"]}),
            ],
        )
        .unwrap();
        assert_eq!(grouped.rows()[0]["a.sum"], 9.0);
        assert_eq!(grouped.rows()[0]["b.max"], 20.0);
    }

    #[test]
    fn calculate_takes_several_calculations() {
        let frame = transform(
            series(),
            &[json!({"type":"calculate","calculations":[
                {"left":"b","right":"a","operator":"/","name":"ratio"},
                {"left":"ratio","value":2,"operator":"*"},
                {"mode":"runningTotal","field":"a"},
                {"mode":"percentOfTotal","field":"a","name":"share"}
            ]})],
        )
        .unwrap();
        let rows = frame.rows();
        assert_eq!(rows[1]["ratio"], 4.0);
        assert_eq!(rows[1]["ratio * 2"], 8.0);
        assert_eq!(rows[2]["a.runningTotal"], 9.0);
        assert_eq!(rows[0]["share"].as_f64().unwrap().round(), 11.0);
        assert!(rows[2]["ratio"].is_null());
        // The running kinds take a list of fields too.
        let frame = transform(series(), &[json!({"type":"runningTotal","fields":["a","b"]})]).unwrap();
        assert_eq!(frame.rows()[1]["a.runningTotal"], 6.0);
        assert_eq!(frame.rows()[1]["b.runningTotal"], 30.0);
        assert!(transform(
            series(),
            &[json!({"type":"calculate","calculations":[{"mode":"bogus","field":"a"}]})]
        )
        .is_err());
    }

    #[test]
    fn filter_by_value_compares_ranges_and_patterns() {
        let names = |step: Value| -> Vec<Value> {
            let frame = transform(series(), &[step]).unwrap();
            frame.fields.iter().find(|f| f.name == "name").unwrap().values.clone()
        };
        assert_eq!(
            names(json!({"type":"filter","field":"a","operator":"gte","value":3})),
            [json!("beta"), json!("gamma")]
        );
        assert_eq!(
            names(json!({"type":"filter","field":"a","operator":"lte","value":3})),
            [json!("alpha"), json!("gamma")]
        );
        assert_eq!(
            names(json!({"type":"filter","field":"a","operator":"gt","value":3})),
            [json!("beta")]
        );
        assert_eq!(
            names(json!({"type":"filter","field":"a","operator":"range","from":2,"to":5})),
            [json!("beta"), json!("gamma")]
        );
        assert_eq!(
            names(json!({"type":"filter","field":"a","operator":"range","to":3})),
            [json!("alpha"), json!("gamma")]
        );
        assert_eq!(
            names(
                json!({"type":"filter","field":"time","operator":"range","from":"2026-01-02T00:00:00Z","to":"2026-01-02T12:00:00Z"})
            ),
            [json!("beta")]
        );
        assert_eq!(
            names(json!({"type":"filter","field":"name","operator":"regex","value":"^(al|ga)"})),
            [json!("alpha"), json!("gamma")]
        );
        assert_eq!(
            names(json!({"type":"filter","field":"b","operator":"missing"})),
            [json!("gamma")]
        );
        // Several conditions, any or all, and exclude.
        assert_eq!(
            names(json!({"type":"filter","match":"any","conditions":[
                {"field":"a","operator":"eq","value":1},{"field":"name","operator":"regex","value":"mm"}]})),
            [json!("alpha"), json!("gamma")]
        );
        assert_eq!(
            names(json!({"type":"filter","exclude":true,"conditions":[
                {"field":"a","operator":"gte","value":3},{"field":"b","operator":"exists"}]})),
            [json!("alpha"), json!("gamma")]
        );
        assert!(transform(
            series(),
            &[json!({"type":"filter","field":"name","operator":"regex","value":"("})]
        )
        .is_err());
        assert!(transform(
            series(),
            &[json!({"type":"filter","field":"a","operator":"near","value":1})]
        )
        .is_err());
    }
}
