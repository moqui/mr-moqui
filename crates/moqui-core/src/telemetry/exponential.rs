//! Bounded, exact-count exponential histogram reaggregation. Downscaling uses the
//! OTLP perfect-subsetting property (including negative bucket indices).
//! https://opentelemetry.io/docs/specs/otel/metrics/data-model/#exponentialhistogram

use std::collections::BTreeMap;

use moqui_lib::{error::Error, object::Value};
use serde_json::{json, Value as Json};

use crate::entity::document::Document;

pub(super) const MAX_BUCKETS: usize = 4096;

#[derive(Clone, Debug)]
pub(super) struct Histogram {
    scale: i32,
    zero_threshold: f64,
    zero: u64,
    positive: BTreeMap<i32, u64>,
    negative: BTreeMap<i32, u64>,
    count: u64,
    sum: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
}

fn fail(message: &str) -> Error {
    Error::new(message)
}

fn count(value: &Json) -> Result<u64, Error> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or_else(|| fail("exponential histogram requires exact nonnegative counts"))
}

fn signed(value: &Json) -> Result<i32, Error> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse().ok())
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| fail("invalid exponential histogram scale or offset"))
}

fn statistic(value: &Json) -> Result<Option<f64>, Error> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|value| value.is_finite())
        .map(Some)
        .ok_or_else(|| fail("non-finite exponential histogram statistic"))
}

impl Histogram {
    pub(super) fn parse(point: &Json) -> Result<Self, Error> {
        let scale = if point["scale"].is_null() {
            0
        } else {
            signed(&point["scale"])?
        };
        if !(-10..=20).contains(&scale) {
            return Err(fail("unsupported exponential histogram scale"));
        }
        let buckets = |key: &str| -> Result<BTreeMap<i32, u64>, Error> {
            let mut result = BTreeMap::new();
            if point[key].is_null() {
                return Ok(result);
            }
            let offset = if point[key]["offset"].is_null() {
                0
            } else {
                signed(&point[key]["offset"])?
            };
            if let Some(values) = point[key]["bucketCounts"].as_array() {
                if values.len() > MAX_BUCKETS {
                    return Err(fail("exponential histogram exceeds the bucket limit"));
                }
                for (index, value) in values.iter().enumerate() {
                    let at = offset
                        .checked_add(index as i32)
                        .ok_or_else(|| fail("exponential bucket index overflow"))?;
                    let value = count(value)?;
                    if value != 0 {
                        result.insert(at, value);
                    }
                }
            } else if !point[key]["bucketCounts"].is_null() {
                return Err(fail("exponential histogram buckets must be an array"));
            }
            Ok(result)
        };
        let zero = if point["zeroCount"].is_null() {
            0
        } else {
            count(&point["zeroCount"])?
        };
        let zero_threshold = statistic(&point["zeroThreshold"])?.unwrap_or(0.0);
        if zero_threshold < 0.0 {
            return Err(fail("negative exponential zero threshold"));
        }
        let result = Self {
            scale,
            zero_threshold,
            zero,
            positive: buckets("positive")?,
            negative: buckets("negative")?,
            count: count(&point["count"])?,
            sum: statistic(&point["sum"])?,
            min: statistic(&point["min"])?,
            max: statistic(&point["max"])?,
        };
        let total = result
            .positive
            .values()
            .chain(result.negative.values())
            .try_fold(result.zero, |total, count| total.checked_add(*count))
            .ok_or_else(|| fail("exponential histogram count overflow"))?;
        if total != result.count || result.min.zip(result.max).is_some_and(|(min, max)| min > max) {
            return Err(fail("inconsistent exponential histogram population"));
        }
        Ok(result)
    }

    fn downscale(&mut self, scale: i32) -> Result<(), Error> {
        let shift = u32::try_from(self.scale - scale).map_err(|_| fail("cannot upscale an exponential histogram"))?;
        let divisor = 1_i64
            .checked_shl(shift)
            .ok_or_else(|| fail("exponential downscale overflow"))?;
        for buckets in [&mut self.positive, &mut self.negative] {
            let mut reduced = BTreeMap::<i32, u64>::new();
            for (index, count) in std::mem::take(buckets) {
                let index = i64::from(index).div_euclid(divisor) as i32;
                let entry = reduced.entry(index).or_default();
                *entry = entry
                    .checked_add(count)
                    .ok_or_else(|| fail("exponential bucket overflow"))?;
            }
            *buckets = reduced;
        }
        self.scale = scale;
        Ok(())
    }

    fn align(&self, other: &Self) -> Result<(Self, Self), Error> {
        let mut left = self.clone();
        let mut right = other.clone();
        let mut scale = self.scale.min(other.scale);
        loop {
            left.downscale(scale)?;
            right.downscale(scale)?;
            let fits = [&left.positive, &left.negative]
                .into_iter()
                .zip([&right.positive, &right.negative])
                .all(|(a, b)| {
                    let first = a.keys().chain(b.keys()).min().copied();
                    let last = a.keys().chain(b.keys()).max().copied();
                    first
                        .zip(last)
                        .is_none_or(|(first, last)| i64::from(last) - i64::from(first) < MAX_BUCKETS as i64)
                });
            if fits {
                break;
            }
            if scale == -10 {
                return Err(fail("exponential histogram merged range exceeds the bucket limit"));
            }
            scale -= 1;
        }
        let mut threshold = left.zero_threshold.max(right.zero_threshold);
        // A threshold inside a populated bucket must move to its upper boundary,
        // possibly touching another bucket. Iterate to the fixed common boundary.
        if threshold > 0.0 {
            loop {
                let mut widened = threshold;
                for buckets in [&left.positive, &left.negative, &right.positive, &right.negative] {
                    for index in buckets.keys() {
                        let lower = 2_f64.powf(f64::from(*index) * 2_f64.powi(-scale));
                        if lower < threshold {
                            let upper = 2_f64.powf((f64::from(*index) + 1.0) * 2_f64.powi(-scale));
                            if !upper.is_finite() {
                                return Err(fail("exponential zero threshold cannot represent a bucket boundary"));
                            }
                            widened = widened.max(upper);
                        }
                    }
                }
                if widened == threshold {
                    break;
                }
                threshold = widened;
            }
        }
        for histogram in [&mut left, &mut right] {
            histogram.zero_threshold = threshold;
            for buckets in [&mut histogram.positive, &mut histogram.negative] {
                let absorbed: Vec<(i32, u64)> = buckets
                    .iter()
                    .filter(|(index, _)| {
                        threshold > 0.0 && 2_f64.powf(f64::from(**index) * 2_f64.powi(-scale)) < threshold
                    })
                    .map(|(&index, &count)| (index, count))
                    .collect();
                for (index, count) in absorbed {
                    buckets.remove(&index);
                    histogram.zero = histogram
                        .zero
                        .checked_add(count)
                        .ok_or_else(|| fail("exponential zero count overflow"))?;
                }
            }
        }
        Ok((left, right))
    }

    pub(super) fn merge(&self, other: &Self) -> Result<Self, Error> {
        let (mut merged, other) = self.align(other)?;
        merged.count = merged
            .count
            .checked_add(other.count)
            .filter(|count| *count <= i64::MAX as u64)
            .ok_or_else(|| fail("exponential count overflow"))?;
        merged.zero = merged
            .zero
            .checked_add(other.zero)
            .ok_or_else(|| fail("exponential zero count overflow"))?;
        for (target, incoming) in [
            (&mut merged.positive, other.positive),
            (&mut merged.negative, other.negative),
        ] {
            for (index, count) in incoming {
                let entry = target.entry(index).or_default();
                *entry = entry
                    .checked_add(count)
                    .ok_or_else(|| fail("exponential bucket count overflow"))?;
            }
        }
        merged.sum = merged.sum.zip(other.sum).map(|(a, b)| a + b);
        if merged.sum.is_some_and(|value| !value.is_finite()) {
            return Err(fail("exponential sum overflow"));
        }
        merged.min = merged.min.zip(other.min).map(|(a, b)| a.min(b));
        merged.max = merged.max.zip(other.max).map(|(a, b)| a.max(b));
        Ok(merged)
    }

    /// A decrease is a producer reset. Comparing at a common resolution avoids
    /// treating a producer's legitimate scale decrease as a reset.
    pub(super) fn delta(&self, previous: &Self) -> Result<Option<Self>, Error> {
        let (mut current, previous) = self.align(previous)?;
        if current.count < previous.count || current.zero < previous.zero {
            return Ok(None);
        }
        for (buckets, old) in [
            (&mut current.positive, previous.positive),
            (&mut current.negative, previous.negative),
        ] {
            // A bucket that decreased is a reset. `current` is this call's copy, so returning partway is safe.
            for (index, count) in old {
                match buckets.get_mut(&index) {
                    Some(value) if *value >= count => *value -= count,
                    None if count == 0 => {}
                    _ => return Ok(None),
                }
            }
            buckets.retain(|_, count| *count != 0);
        }
        current.count -= previous.count;
        current.zero -= previous.zero;
        current.sum = current.sum.zip(previous.sum).map(|(a, b)| a - b);
        if current.sum.is_some_and(|value| !value.is_finite()) {
            return Err(fail("exponential delta sum overflow"));
        }
        current.min = None;
        current.max = None;
        Ok(Some(current))
    }

    pub(super) fn json(&self) -> Json {
        let dense = |buckets: &BTreeMap<i32, u64>| {
            let (Some(&first), Some(&last)) = (buckets.keys().next(), buckets.keys().next_back()) else {
                return json!({"offset":0,"bucketCounts":[]});
            };
            json!({"offset":first,"bucketCounts":(first..=last).map(|index| buckets.get(&index).copied().unwrap_or(0)).collect::<Vec<_>>()})
        };
        let mut point = json!({"count":self.count,"scale":self.scale,"zeroCount":self.zero,
            "zeroThreshold":self.zero_threshold,"positive":dense(&self.positive),"negative":dense(&self.negative)});
        for (key, value) in [("sum", self.sum), ("min", self.min), ("max", self.max)] {
            if let Some(value) = value {
                point[key] = json!(value);
            }
        }
        point
    }

    pub(super) fn store(&self, document: &mut Document) {
        document.insert("count".into(), Value::I64(self.count as i64));
        for (key, value) in [("sum", self.sum), ("min", self.min), ("max", self.max)] {
            if let Some(value) = value {
                document.insert(key.into(), Value::F64(value));
            } else {
                document.shift_remove(key);
            }
        }
        document.insert("exponential_histogram".into(), moqui_lib::json::from_json(&self.json()));
        document.insert("aggregation_temporality".into(), Value::string("1"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points() -> (Histogram, Histogram) {
        let a = Histogram::parse(
            &json!({"scale":1,"count":33,"zeroCount":1,"sum":6.0,"min":-1.4,"max":2.8,
            "positive":{"offset":-2,"bucketCounts":[2,3,4,0,5]},"negative":{"offset":-1,"bucketCounts":[7,11]}}),
        )
        .unwrap();
        let b = Histogram::parse(
            &json!({"scale":0,"count":74,"zeroCount":2,"sum":8.0,"min":-1.9,"max":1.9,
            "positive":{"offset":-1,"bucketCounts":[13,17]},"negative":{"offset":-1,"bucketCounts":[19,23]}}),
        )
        .unwrap();
        (a, b)
    }

    #[test]
    fn minute_hour_day_rollups_preserve_exponential_populations_and_retry_ids() {
        let (a, b) = points();
        let doc = |histogram: Histogram, time| {
            let mut document = Document::from_iter([
                ("series_fingerprint".into(), Value::string("exp-fixture")),
                ("aggregation".into(), Value::string("exponential_histogram")),
                ("bucket_start".into(), Value::I64(time)),
            ]);
            histogram.store(&mut document);
            document
        };
        let input = vec![doc(a, 0), doc(b, 60_000)];
        let hour = super::super::maintenance::rollup_documents(input.clone(), "hour", 3_600_000).unwrap();
        assert_eq!(
            hour,
            super::super::maintenance::rollup_documents(input, "hour", 3_600_000).unwrap()
        );
        assert_eq!(hour[0]["count"], Value::I64(107));
        assert_eq!(hour[0]["_id"].as_str(), Some("exp-fixture:hour:0"));
        let raw = moqui_lib::json::to_json(&hour[0]["exponential_histogram"]);
        assert_eq!(raw["positive"], json!({"offset":-1,"bucketCounts":[18,21,5]}));
        assert_eq!(raw["negative"], json!({"offset":-1,"bucketCounts":[26,34]}));
        assert_eq!(raw["zeroCount"], 3);
        assert_eq!(raw["sum"], 14.0);
        assert_eq!(raw["min"], -1.9);
        assert_eq!(raw["max"], 2.8);
        let day = super::super::maintenance::rollup_documents(hour, "day", 86_400_000).unwrap();
        assert_eq!(moqui_lib::json::to_json(&day[0]["exponential_histogram"]), raw);
        assert_eq!(day[0]["_id"].as_str(), Some("exp-fixture:day:0"));
    }

    #[test]
    fn zero_thresholds_expand_to_common_populated_boundary_without_losing_counts() {
        let (a, mut b) = points();
        b.zero_threshold = 1.1;
        let merged = a.merge(&b).unwrap().json();
        assert_eq!(merged["zeroThreshold"], 2.0);
        assert_eq!(merged["zeroCount"], 102);
        assert_eq!(merged["positive"], json!({"offset":1,"bucketCounts":[5]}));
        assert_eq!(merged["negative"]["bucketCounts"], json!([]));
        assert_eq!(merged["count"], 107);
    }

    #[test]
    fn exact_large_counts_sparse_extreme_indices_and_missing_statistics_are_bounded() {
        let point = |offset, count| {
            Histogram::parse(&json!({"scale":20,"count":count,"positive":{"offset":offset,"bucketCounts":[count]}}))
                .unwrap()
        };
        let merged = point(i32::MIN, 9_007_199_254_740_993_u64)
            .merge(&point(i32::MAX, 1))
            .unwrap()
            .json();
        assert_eq!(merged["count"].as_u64(), Some(9_007_199_254_740_994));
        assert!(merged["positive"]["bucketCounts"].as_array().unwrap().len() <= MAX_BUCKETS);
        assert_eq!(merged["scale"], 0);
        assert!(merged.get("sum").is_none());
        assert!(point(0, i64::MAX as u64).merge(&point(0, 1)).is_err());
        assert!(Histogram::parse(&json!({"count":2,"positive":{"offset":i32::MAX,"bucketCounts":[1,1]}})).is_err());
    }

    #[test]
    fn cumulative_points_downscale_to_delta_reject_replay_and_accept_reset() {
        use super::super::ingest::{Decoder, Signal};
        let decoder = Decoder::default();
        let decode = |time, start, count, zero, scale, buckets: Vec<u64>, sum| {
            let body = json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[{"name":"exp.requests","exponentialHistogram":{
                "aggregationTemporality":2,"dataPoints":[{"timeUnixNano":time,"startTimeUnixNano":start,
                    "count":count,"zeroCount":zero,"scale":scale,"positive":{"offset":0,"bucketCounts":buckets},
                    "sum":sum,"min":0.0,"max":2.0}]
            }}]}]}]});
            decoder
                .decode(
                    Signal::Metrics,
                    "application/json",
                    body.to_string().as_bytes(),
                    "fixture",
                )
                .unwrap()
        };
        let first = decode(60_000_000_000_i64, 1_000_000_000_i64, 4, 1, 1, vec![1, 2], 5.0);
        assert_eq!(first.rejected, 0);
        let second = decode(120_000_000_000, 1_000_000_000, 8, 2, 0, vec![6], 9.0);
        assert_eq!(second.rejected, 0);
        assert_eq!(second.documents[0]["count"], Value::I64(4));
        assert_eq!(second.documents[0]["sum"], Value::F64(4.0));
        assert!(!second.documents[0].contains_key("min"));
        assert_eq!(second.documents[0]["bucket_start"], Value::I64(60_000));
        let raw = moqui_lib::json::to_json(&second.documents[0]["exponential_histogram"]);
        assert_eq!(raw["positive"]["bucketCounts"], json!([3]));
        assert_eq!(raw["zeroCount"], 1);
        assert_eq!(
            decode(120_000_000_000, 1_000_000_000, 8, 2, 0, vec![6], 9.0).rejected,
            1
        );
        let reset = decode(180_000_000_000, 120_000_000_000, 2, 0, 0, vec![2], 3.0);
        assert_eq!(reset.documents[0]["count"], Value::I64(2));
        assert!(reset.documents[0].contains_key("min"));
        let baseline = decode(3_600_000_000_000, 0, 2, 0, 0, vec![2], 3.0);
        // A reset with an unknown start establishes a new baseline.
        assert_eq!(baseline.rejected, 1);
        let old = json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[{"name":"old","exponentialHistogram":{"aggregationTemporality":2,
            "dataPoints":[{"timeUnixNano":"3600000000000","count":1,"positive":{"bucketCounts":[1]}}]}}]}]}]});
        let baseline = decoder
            .decode(
                Signal::Metrics,
                "application/json",
                old.to_string().as_bytes(),
                "fixture",
            )
            .unwrap();
        assert_eq!(baseline.rejected, 1);
        assert!(baseline.documents.is_empty());
    }

    fn cumulative(
        decoder: &super::super::ingest::Decoder,
        kind: &str,
        start: i64,
        time: i64,
        population: u64,
    ) -> super::super::ingest::Decoded {
        let mut point = json!({"startTimeUnixNano":start.to_string(),"timeUnixNano":time.to_string()});
        if kind == "sum" {
            point["asInt"] = json!(population.to_string());
        } else {
            point["count"] = json!(population.to_string());
            point["sum"] = json!(population as f64);
            if kind == "exponentialHistogram" {
                point["scale"] = json!(0);
                point["positive"] = json!({"offset":-1,"bucketCounts":[population.to_string()]});
            } else {
                point["explicitBounds"] = json!([10]);
                point["bucketCounts"] = json!([population.to_string(), "0"]);
            }
        }
        let mut metric = json!({"name":format!("interval-{kind}")});
        metric[kind] = json!({"aggregationTemporality":2,"isMonotonic":true,"dataPoints":[point]});
        let body = json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[metric]}]}]});
        decoder
            .decode(
                super::super::ingest::Signal::Metrics,
                "application/json",
                body.to_string().as_bytes(),
                "interval-fixture",
            )
            .unwrap()
    }

    #[test]
    fn zero_duration_cumulative_points_are_baselines_for_all_metric_kinds() {
        for kind in ["exponentialHistogram", "histogram", "sum"] {
            let decoder = super::super::ingest::Decoder::default();
            let first = cumulative(&decoder, kind, 3_600_000_000_000, 3_600_000_000_000, 5);
            assert!(
                first.documents.is_empty(),
                "{kind} unknown-start point cannot establish a measured interval"
            );
            assert_eq!(first.rejected, 1);
            assert!(first.message.contains("baseline"));
            let next = cumulative(&decoder, kind, 3_600_000_000_000, 3_660_000_000_000, 8);
            assert_eq!(next.rejected, 0);
            assert_eq!(next.documents[0]["sum"], Value::F64(3.0));
            assert_eq!(next.documents[0]["bucket_start"], Value::I64(3_600_000));
        }
    }

    #[test]
    fn same_start_decreases_never_rewrite_a_previous_hour_with_reset_population() {
        for kind in ["exponentialHistogram", "histogram", "sum"] {
            let decoder = super::super::ingest::Decoder::default();
            let start = 3_599_000_000_000;
            assert_eq!(cumulative(&decoder, kind, start, 3_600_000_000_000, 5).rejected, 0);
            assert_eq!(
                cumulative(&decoder, kind, start, 3_660_000_000_000, 8).documents[0]["sum"],
                Value::F64(3.0)
            );
            let reset = cumulative(&decoder, kind, start, 7_200_000_000_000, 2);
            assert!(
                reset.documents.is_empty(),
                "{kind} reset with stale start must establish a baseline"
            );
            assert_eq!(reset.rejected, 1);
            let next = cumulative(&decoder, kind, start, 7_260_000_000_000, 4);
            assert_eq!(next.documents[0]["bucket_start"], Value::I64(7_200_000));
            assert_eq!(next.documents[0]["sum"], Value::F64(2.0));
        }
    }

    #[test]
    fn malformed_or_oversized_ranges_do_not_discard_independent_valid_populations() {
        let point = |positive| json!({"timeUnixNano":"60000000000","count":1,"positive":positive});
        let body = json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[{"name":"range-validation","exponentialHistogram":{
            "aggregationTemporality":1,"dataPoints":[point(json!({"bucketCounts":"bad"})),
                point(json!({"bucketCounts":vec![1;MAX_BUCKETS+1]})),point(json!({"bucketCounts":[1]}))]
        }}]}]}]});
        let result = super::super::ingest::Decoder::default()
            .decode(
                super::super::ingest::Signal::Metrics,
                "application/json",
                body.to_string().as_bytes(),
                "fixture",
            )
            .unwrap();
        assert_eq!(result.rejected, 2);
        assert_eq!(result.documents.len(), 1);
        assert_eq!(result.documents[0]["count"], Value::I64(1));
    }
}
