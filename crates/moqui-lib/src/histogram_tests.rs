/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

use super::*;

/// The exact value at `p` by nearest rank.
fn exact(sorted: &[u64], p: f64) -> u64 {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// A small xorshift, so the test needs no generator crate.
struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn check_against_sorted(values: &[u64]) {
    let mut histogram = Histogram::new();
    for value in values {
        histogram.record(*value);
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    assert_eq!(histogram.count(), sorted.len() as u64);
    assert_eq!(histogram.min(), sorted[0]);
    assert_eq!(histogram.max(), *sorted.last().unwrap());
    let mean = sorted.iter().sum::<u64>() as f64 / sorted.len() as f64;
    assert!((histogram.mean() - mean).abs() < 1e-6);
    for p in [0.0, 1.0, 10.0, 25.0, 50.0, 75.0, 90.0, 95.0, 99.0, 99.9, 100.0] {
        let want = exact(&sorted, p).min(MAX_MICROS);
        let got = histogram.percentile(p);
        let diff = got.abs_diff(want);
        assert!(
            diff as f64 <= want as f64 / 64.0,
            "p{p}: {got} is not within 1/64 of {want} ({} values)",
            sorted.len()
        );
    }
}

#[test]
fn percentiles_are_within_a_64th_of_the_sorted_samples() {
    let mut random = Xorshift(0x9E37_79B9_7F4A_7C15);
    // spread over many powers of two: 1 µs to about 16 s
    let wide: Vec<u64> = (0..20_000)
        .map(|_| random.next() >> (random.next() % 40 + 24))
        .collect();
    check_against_sorted(&wide);
    // a narrow band, as one endpoint's latency is
    let narrow: Vec<u64> = (0..5_000).map(|_| 20_000 + random.next() % 3_000).collect();
    check_against_sorted(&narrow);
    // under 64 every value is its own counter
    let small: Vec<u64> = (0..64).collect();
    check_against_sorted(&small);
    let mut exactly = Histogram::new();
    for value in 0..64 {
        exactly.record(value);
    }
    assert_eq!(exactly.percentile(50.0), 31);
}

#[test]
fn edge_values() {
    let empty = Histogram::new();
    assert_eq!(empty.percentile(50.0), 0);
    assert_eq!(empty.mean(), 0.0);
    assert_eq!(empty.max(), 0);
    let mut one = Histogram::new();
    one.record(7_777);
    for p in [0.0, 50.0, 100.0] {
        assert_eq!(one.percentile(p), 7_777, "clamped to the min and max");
    }
    let mut ends = Histogram::new();
    ends.record(0);
    ends.record(1);
    ends.record(MAX_MICROS);
    ends.record(u64::MAX);
    assert_eq!(ends.min(), 0);
    assert_eq!(ends.max(), u64::MAX);
    assert_eq!(ends.percentile(25.0), 0);
    assert_eq!(ends.percentile(50.0), 1);
    // past an hour counts as an hour
    let top = ends.percentile(100.0);
    assert!(top.abs_diff(MAX_MICROS) <= MAX_MICROS / 64, "{top}");
    for value in [63, 64, 65, 127, 128, 129, 1_000_000, MAX_MICROS - 1, MAX_MICROS] {
        let at = value_of(index(value));
        assert!(at.abs_diff(value) as f64 <= value as f64 / 64.0, "{value}: {at}");
    }
    assert_eq!(index(MAX_MICROS), LEN - 1);
}

#[test]
fn merging_adds_and_survives_json() {
    let mut random = Xorshift(42);
    let values: Vec<u64> = (0..3_000).map(|_| random.next() % 500_000).collect();
    let (left, right) = values.split_at(1_000);
    let mut a = Histogram::new();
    let mut b = Histogram::new();
    let mut whole = Histogram::new();
    for value in left {
        a.record(*value);
        whole.record(*value);
    }
    for value in right {
        b.record(*value);
        whole.record(*value);
    }
    let mut merged = a.clone();
    merged.merge(&b);
    assert_eq!(merged, whole);
    let mut into_empty = Histogram::new();
    into_empty.merge(&whole);
    into_empty.merge(&Histogram::new());
    assert_eq!(into_empty, whole);
    let back = Histogram::from_json(&whole.to_json()).unwrap();
    assert_eq!(back, whole);
    assert_eq!(
        Histogram::from_json(&Histogram::new().to_json()).unwrap(),
        Histogram::new()
    );
    let broken = serde_json::json!({ "count": 2, "sum": 1, "min": 0, "max": 1, "counts": [[1, 1]] });
    assert!(Histogram::from_json(&broken).unwrap_err().contains("add up to 1"));
    let past = serde_json::json!({ "count": 1, "sum": 1, "min": 0, "max": 1, "counts": [[LEN, 1]] });
    assert!(Histogram::from_json(&past).is_err());
}

#[test]
fn bad_json_is_refused_and_bad_state_does_not_panic() {
    let refused = |value: serde_json::Value| Histogram::from_json(&value).unwrap_err();
    assert!(
        refused(serde_json::json!({ "count": 1, "sum": 1, "min": 5, "max": 1, "counts": [[1, 1]] }))
            .contains("min 5 is over its max 1")
    );
    assert!(
        refused(serde_json::json!({ "count": 0, "sum": 0, "min": 0, "max": 0, "counts": [[1, 2]] }))
            .contains("count of 0")
    );
    assert!(refused(
        serde_json::json!({ "count": 1, "sum": 0, "min": 0, "max": 9, "counts": [[1, u64::MAX], [2, 2]] })
    )
    .contains("overflow"));
    assert!(refused(
        serde_json::json!({ "count": 1, "sum": 0, "min": 0, "max": 9, "counts": [[1, u64::MAX], [1, 1]] })
    )
    .contains("overflow"));
    // a state no record makes: a min over the max answers within them, not a panic
    let odd = Histogram {
        counts: vec![1; LEN],
        count: 3,
        sum: 3,
        min: 9,
        max: 2,
    };
    assert_eq!(odd.percentile(50.0), 2);
}
