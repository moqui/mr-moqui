//! Log patterns: lines grouped by template, after Drain. Variable parts (numbers, hex, UUIDs and quoted text, the error
//! fingerprint's masking) become `<*>` before grouping, lines of the same length and first token are compared position
//! by position, and a line joins the most similar template when at least half its tokens agree; the positions that
//! differ become `<*>` too. Lines are clustered in sorted order, so the patterns do not depend on the order the search
//! returned them in.
use super::frame::{Field, Frame};
use moqui_lib::error::Error;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

pub const WILDCARD: &str = "<*>";
/// Lines read per call; more are left out with the count in the output's `skipped`.
pub const MAX_LINES: usize = 5000;
/// Patterns kept; a line that would start another joins an `(other)` pattern.
pub const MAX_PATTERNS: usize = 200;
const MAX_TOKENS: usize = 64;
const SIMILARITY: f64 = 0.5;

/// The line's tokens, with variable parts masked.
pub fn tokens(line: &str) -> Vec<String> {
    let masked = crate::telemetry::mask_message(line, WILDCARD);
    let mut tokens: Vec<String> = masked.split_whitespace().map(str::to_string).collect();
    if tokens.len() > MAX_TOKENS {
        tokens.truncate(MAX_TOKENS);
        tokens.push(WILDCARD.into());
    }
    tokens
}

struct Cluster {
    template: Vec<String>,
}

/// Each line's pattern index, and the patterns' templates. Lines past [`MAX_PATTERNS`] patterns get `None`.
pub fn cluster(lines: &[&str]) -> (Vec<Option<usize>>, Vec<String>) {
    let mut tokenized: Vec<(Vec<String>, usize)> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| (tokens(line), index))
        .collect();
    tokenized.sort();
    let mut clusters: Vec<Cluster> = Vec::new();
    let mut groups: HashMap<(usize, String), Vec<usize>> = HashMap::new();
    let mut assigned = vec![None; lines.len()];
    for (tokens, index) in tokenized {
        let first = tokens
            .first()
            .filter(|token| !token.contains(WILDCARD))
            .cloned()
            .unwrap_or_else(|| WILDCARD.into());
        let group = groups.entry((tokens.len(), first)).or_default();
        let best = group
            .iter()
            .map(|&at| (at, similarity(&clusters[at].template, &tokens)))
            .filter(|(_, score)| *score >= SIMILARITY)
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
        match best {
            Some((at, _)) => {
                let cluster = &mut clusters[at];
                for (slot, token) in cluster.template.iter_mut().zip(&tokens) {
                    if slot != token {
                        *slot = WILDCARD.into();
                    }
                }
                assigned[index] = Some(at);
            }
            None if clusters.len() < MAX_PATTERNS => {
                group.push(clusters.len());
                assigned[index] = Some(clusters.len());
                clusters.push(Cluster { template: tokens });
            }
            None => {}
        }
    }
    (
        assigned,
        clusters.into_iter().map(|cluster| cluster.template.join(" ")).collect(),
    )
}

/// The share of positions where `template` and `tokens` agree; a wildcard in the template agrees with anything.
fn similarity(template: &[String], tokens: &[String]) -> f64 {
    if template.is_empty() {
        return 1.0;
    }
    let same = template
        .iter()
        .zip(tokens)
        .filter(|(slot, token)| *slot == *token || slot.as_str() == WILDCARD)
        .count();
    same as f64 / template.len() as f64
}

fn millis(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|value| value as i64)),
        Value::String(text) => text
            .parse::<jiff::Timestamp>()
            .ok()
            .map(|time| time.as_millisecond())
            .or_else(|| text.parse::<i64>().ok()),
        _ => None,
    }
}

fn line_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// The `patterns` transformation: one row per pattern of the `field` column (default `body`), most frequent first, with
/// `count`, `share` (percent of the lines read), `first` and `last` time, the most common `severity`, a `sample` line
/// a `sparkline` of counts over `buckets` (default 24) equal slices of the lines' time span, and a `query` that narrows a
/// search to the pattern's lines ([`query_for`]). Options: `field`,
/// `timeField` (default `timestamp`, else `time` or `start`), `severityField` (default `severity_text`), `limit`
/// (patterns returned, default 50), `buckets`.
pub fn transform(frame: &Frame, step: &Value) -> Result<Frame, Error> {
    let column = |name: &str| frame.fields.iter().find(|field| field.name == name);
    let field_name = step["field"].as_str().unwrap_or("body");
    let Some(body) = column(field_name)
        .or_else(|| column("message"))
        .or_else(|| column("name"))
    else {
        if frame.is_empty() {
            return Ok(Frame::default());
        }
        return Err(Error::new(format!("patterns needs a {field_name} column")));
    };
    let time = step["timeField"]
        .as_str()
        .and_then(column)
        .or_else(|| column("timestamp"))
        .or_else(|| column("time"))
        .or_else(|| column("start"));
    let severity = column(step["severityField"].as_str().unwrap_or("severity_text"));
    let limit = step["limit"].as_u64().unwrap_or(50).clamp(1, MAX_PATTERNS as u64) as usize;
    let bucket_count = step["buckets"].as_u64().unwrap_or(24).clamp(1, 200) as usize;
    let mut rows: Vec<(String, Option<i64>, Option<String>)> = body
        .values
        .iter()
        .enumerate()
        .filter_map(|(row, value)| {
            line_text(value).map(|text| {
                (
                    text,
                    time.and_then(|time| time.values.get(row)).and_then(millis),
                    severity
                        .and_then(|severity| severity.values.get(row))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                )
            })
        })
        .collect();
    let skipped = rows.len().saturating_sub(MAX_LINES);
    rows.truncate(MAX_LINES);
    let lines: Vec<&str> = rows.iter().map(|(text, ..)| text.as_str()).collect();
    let (assigned, templates) = cluster(&lines);
    let span = rows
        .iter()
        .filter_map(|(_, time, _)| *time)
        .fold(None, |range: Option<(i64, i64)>, time| {
            Some(range.map_or((time, time), |(low, high)| (low.min(time), high.max(time))))
        });
    #[derive(Default)]
    struct Stats {
        count: usize,
        first: Option<i64>,
        last: Option<i64>,
        sample: Option<(Option<i64>, String)>,
        severities: BTreeMap<String, usize>,
        buckets: Vec<u64>,
    }
    let mut stats: Vec<Stats> = (0..=templates.len()).map(|_| Stats::default()).collect();
    for ((text, time, severity), pattern) in rows.iter().zip(&assigned) {
        let entry = &mut stats[pattern.unwrap_or(templates.len())];
        entry.count += 1;
        if let Some(time) = *time {
            entry.first = Some(entry.first.map_or(time, |first| first.min(time)));
            entry.last = Some(entry.last.map_or(time, |last| last.max(time)));
        }
        // The newest line is the sample.
        if entry
            .sample
            .as_ref()
            .is_none_or(|(sample_time, _)| time.is_some() && *time > *sample_time)
        {
            entry.sample = Some((*time, text.clone()));
        }
        if let Some(severity) = severity {
            *entry.severities.entry(severity.clone()).or_default() += 1;
        }
        if entry.buckets.is_empty() {
            entry.buckets = vec![0; bucket_count];
        }
        let slot = match (span, *time) {
            (Some((low, high)), Some(time)) if high > low => {
                (((time - low) as f64 / (high - low) as f64) * bucket_count as f64) as usize
            }
            _ => bucket_count - 1,
        };
        entry.buckets[slot.min(bucket_count - 1)] += 1;
    }
    let total = rows.len().max(1) as f64;
    let mut order: Vec<usize> = (0..stats.len()).filter(|&at| stats[at].count > 0).collect();
    order.sort_by(|a, b| stats[*b].count.cmp(&stats[*a].count).then(a.cmp(b)));
    order.truncate(limit);
    let iso = |time: Option<i64>| {
        time.and_then(|time| jiff::Timestamp::from_millisecond(time).ok())
            .map_or(Value::Null, |time| json!(time.to_string()))
    };
    let mut columns: [Vec<Value>; 9] = Default::default();
    for at in order {
        let entry = &stats[at];
        let pattern = templates.get(at).cloned().unwrap_or_else(|| "(other)".into());
        let severity = entry
            .severities
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map_or(Value::Null, |(name, _)| json!(name));
        let values = [
            json!(pattern),
            json!(entry.count),
            json!((entry.count as f64 * 1000.0 / total).round() / 10.0),
            iso(entry.first),
            iso(entry.last),
            severity,
            entry.sample.as_ref().map_or(Value::Null, |(_, text)| json!(text)),
            json!(serde_json::to_string(&entry.buckets).unwrap_or_default()),
            json!(if at < templates.len() {
                query_for(&pattern, &body.name)
            } else {
                String::new()
            }),
        ];
        for (column, value) in columns.iter_mut().zip(values) {
            column.push(value);
        }
    }
    let names = [
        ("pattern", "string", None),
        ("count", "number", None),
        ("share", "number", Some("percent")),
        ("first", "time", None),
        ("last", "time", None),
        ("severity", "string", None),
        ("sample", "string", None),
        ("sparkline", "string", None),
        ("query", "string", None),
    ];
    let mut out = Frame {
        fields: names
            .into_iter()
            .zip(columns)
            .map(|((name, kind, unit), values)| Field {
                name: name.into(),
                kind: kind.into(),
                unit: unit.map(str::to_string),
                labels: BTreeMap::new(),
                config: BTreeMap::new(),
                values,
            })
            .collect(),
    };
    if skipped > 0 {
        if let Some(field) = out.fields.first_mut() {
            field.config.insert(
                "notice".into(),
                json!(format!("{skipped} lines over {MAX_LINES} were not grouped")),
            );
        }
    }
    Ok(out)
}

/// The constant tokens of a pattern, for a query that narrows a search to lines like it: `body:"a b" AND body:"c"`
/// over the runs of tokens between wildcards. Empty when the pattern is all wildcards.
pub fn query_for(pattern: &str, field: &str) -> String {
    let mut phrases = Vec::new();
    let mut run: Vec<&str> = Vec::new();
    for token in pattern.split_whitespace().chain(std::iter::once(WILDCARD)) {
        if token.contains(WILDCARD) {
            if !run.is_empty() {
                phrases.push(run.join(" "));
                run.clear();
            }
        } else {
            run.push(token);
        }
    }
    phrases
        .into_iter()
        .map(|phrase| format!("{field}:\"{}\"", phrase.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(lines: &[(&str, &str, &str)]) -> Frame {
        let column = |name: &str, kind: &str, values: Vec<Value>| Field {
            name: name.into(),
            kind: kind.into(),
            unit: None,
            labels: BTreeMap::new(),
            config: BTreeMap::new(),
            values,
        };
        Frame {
            fields: vec![
                column("body", "string", lines.iter().map(|line| json!(line.0)).collect()),
                column("timestamp", "time", lines.iter().map(|line| json!(line.1)).collect()),
                column(
                    "severity_text",
                    "string",
                    lines.iter().map(|line| json!(line.2)).collect(),
                ),
            ],
        }
    }

    #[test]
    fn masks_numbers_ids_and_quotes() {
        assert_eq!(
            tokens("user 42 logged in from 10.0.0.7 as 'bob' id=7f3a9c2e-1111-2222-3333-444455556666"),
            ["user", "<*>", "logged", "in", "from", "<*>.<*>", "as", "<*>", "id=<*>"]
        );
    }

    #[test]
    fn groups_lines_that_differ_in_variable_words() {
        let lines = [
            "Connection to db1 refused after 3 tries",
            "Connection to db2 refused after 5 tries",
            "Job OrderSync finished",
            "Connection to cache refused after 1 tries",
            "Job InvoiceRun finished",
            "Something else entirely different here now",
        ];
        let (assigned, templates) = cluster(&lines);
        assert_eq!(assigned[0], assigned[1]);
        assert_eq!(assigned[0], assigned[3]);
        assert_eq!(assigned[2], assigned[4]);
        assert_ne!(assigned[0], assigned[2]);
        let connection = &templates[assigned[0].unwrap()];
        assert_eq!(connection, "Connection to <*> refused after <*> tries");
        assert_eq!(&templates[assigned[2].unwrap()], "Job <*> finished");
        assert_eq!(templates.len(), 3);
    }

    #[test]
    fn patterns_do_not_depend_on_input_order() {
        let mut lines = vec![
            "GET /orders/1 200",
            "GET /orders/2 404",
            "POST /orders 201",
            "GET /products/9 200",
            "cache miss key=a",
            "cache miss key=b",
        ];
        let (_, forward) = cluster(&lines);
        lines.reverse();
        let (_, backward) = cluster(&lines);
        let (mut forward, mut backward) = (forward, backward);
        forward.sort();
        backward.sort();
        assert_eq!(forward, backward);
    }

    #[test]
    fn transformation_counts_shares_times_severity_and_sparkline() {
        let out = transform(
            &frame(&[
                ("disk 91% full on host1", "2026-10-27T10:00:00Z", "WARN"),
                ("disk 95% full on host2", "2026-10-27T10:30:00Z", "WARN"),
                ("disk 99% full on host2", "2026-10-27T11:00:00Z", "ERROR"),
                ("started worker 3", "2026-10-27T10:10:00Z", "INFO"),
            ]),
            &json!({"type":"patterns","buckets":2}),
        )
        .unwrap();
        let rows = out.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["pattern"], "disk <*>% full on <*>");
        assert_eq!(rows[0]["count"], 3);
        assert_eq!(rows[0]["share"], 75.0);
        assert_eq!(rows[0]["severity"], "WARN");
        assert_eq!(rows[0]["first"], "2026-10-27T10:00:00Z");
        assert_eq!(rows[0]["last"], "2026-10-27T11:00:00Z");
        assert_eq!(rows[0]["sample"], "disk 99% full on host2");
        assert_eq!(rows[0]["sparkline"], "[1,2]");
        assert_eq!(rows[0]["query"], r#"body:"disk" AND body:"full on""#);
        assert_eq!(rows[1]["pattern"], "started worker <*>");
    }

    #[test]
    fn caps_patterns_and_folds_the_rest_into_other() {
        let lines: Vec<String> = (0..MAX_PATTERNS + 10)
            .map(|n| {
                "abcdefghijklmnopqrstuvwxyz"
                    .chars()
                    .cycle()
                    .skip(n % 26)
                    .take(3 + n / 26)
                    .collect::<String>()
            })
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (assigned, templates) = cluster(&refs);
        assert_eq!(templates.len(), MAX_PATTERNS);
        assert_eq!(assigned.iter().filter(|pattern| pattern.is_none()).count(), 10);
        let column = Field {
            name: "body".into(),
            kind: "string".into(),
            unit: None,
            labels: BTreeMap::new(),
            config: BTreeMap::new(),
            values: refs.iter().map(|line| json!(line)).collect(),
        };
        let out = transform(&Frame { fields: vec![column] }, &json!({"limit":200})).unwrap();
        assert!(out
            .rows()
            .iter()
            .any(|row| row["pattern"] == "(other)" && row["count"] == 10));
    }

    #[test]
    fn query_for_takes_the_constant_runs() {
        assert_eq!(
            query_for("Connection to <*> refused after <*> tries", "body"),
            r#"body:"Connection to" AND body:"refused after" AND body:"tries""#
        );
        assert_eq!(query_for("<*>", "body"), "");
        assert_eq!(query_for(r#"say "hi""#, "body"), r#"body:"say \"hi\"""#);
    }
}
