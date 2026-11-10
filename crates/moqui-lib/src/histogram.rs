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

//! A log-linear histogram of durations in microseconds (plans/LoadRunner.md section 5, decision D4): counters by
//! power of two with 64 linear steps in each, from 1 µs to an hour, so a percentile is within 1/64 (about 1.6%) of
//! the value it stands for, in about 14 KB. Histograms merge by adding, also across processes through
//! [`Histogram::to_json`] and [`Histogram::from_json`].

use serde_json::{json, Value};

/// The steps in each power of two, and its bits.
const SUB_BITS: u32 = 6;
const SUB: u64 = 1 << SUB_BITS;

/// The largest value counted on its own: an hour in microseconds. A larger one counts as this.
pub const MAX_MICROS: u64 = 3_600_000_000;

/// The counter of a value: exact under 64, then 64 steps in each power of two.
fn index(value: u64) -> usize {
    let value = value.min(MAX_MICROS);
    if value < SUB {
        return value as usize;
    }
    let exponent = 63 - value.leading_zeros();
    let shift = exponent - SUB_BITS;
    (((exponent - SUB_BITS + 1) as u64) * SUB + (value >> shift) - SUB) as usize
}

/// The counters a histogram has: up to the one of [`MAX_MICROS`].
const LEN: usize = {
    // index(MAX_MICROS), spelled out for a const
    let exponent = 63 - MAX_MICROS.leading_zeros();
    let shift = exponent - SUB_BITS;
    (((exponent - SUB_BITS + 1) as u64) * SUB + (MAX_MICROS >> shift) - SUB) as usize + 1
};

/// The value a counter stands for: the middle of its range.
fn value_of(index: usize) -> u64 {
    let index = index as u64;
    if index < SUB {
        return index;
    }
    let exponent = index / SUB + u64::from(SUB_BITS) - 1;
    let step = index % SUB + SUB;
    let shift = exponent - u64::from(SUB_BITS);
    let low = step << shift;
    let width = 1u64 << shift;
    low + (width - 1) / 2
}

/// Counts of values in microseconds. The counters are made on the first value, so an empty histogram is small.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Histogram {
    counts: Vec<u64>,
    count: u64,
    sum: u64,
    min: u64,
    max: u64,
}

impl Histogram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, micros: u64) {
        if self.counts.is_empty() {
            self.counts = vec![0; LEN];
        }
        let counter = &mut self.counts[index(micros)];
        *counter = counter.saturating_add(1);
        if self.count == 0 {
            self.min = micros;
            self.max = micros;
        } else {
            self.min = self.min.min(micros);
            self.max = self.max.max(micros);
        }
        self.count = self.count.saturating_add(1);
        self.sum = self.sum.saturating_add(micros);
    }

    /// Add `other`'s counts to these.
    pub fn merge(&mut self, other: &Histogram) {
        if other.count == 0 {
            return;
        }
        if self.counts.is_empty() {
            self.counts = vec![0; LEN];
        }
        for (into, count) in self.counts.iter_mut().zip(&other.counts) {
            *into = into.saturating_add(*count);
        }
        if self.count == 0 {
            self.min = other.min;
            self.max = other.max;
        } else {
            self.min = self.min.min(other.min);
            self.max = self.max.max(other.max);
        }
        self.count = self.count.saturating_add(other.count);
        self.sum = self.sum.saturating_add(other.sum);
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// The smallest value, exact; 0 for none.
    pub fn min(&self) -> u64 {
        self.min
    }

    /// The largest value, exact; 0 for none.
    pub fn max(&self) -> u64 {
        self.max
    }

    /// The sum of the values, exact.
    pub fn sum(&self) -> u64 {
        self.sum
    }

    /// The mean, exact; 0 for none.
    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum as f64 / self.count as f64
        }
    }

    /// The value at percentile `p` (0 to 100) by nearest rank, within 1/64 of the exact one and within the min and
    /// max; 0 for none.
    pub fn percentile(&self, p: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let rank = ((p / 100.0) * self.count as f64).ceil() as u64;
        let rank = rank.clamp(1, self.count);
        let mut seen = 0u64;
        for (index, count) in self.counts.iter().enumerate() {
            seen = seen.saturating_add(*count);
            if seen >= rank {
                // max then min, not clamp, which panics on a min over the max
                return value_of(index).max(self.min).min(self.max);
            }
        }
        self.max
    }

    /// `{ count, sum, min, max, counts: [[index, count], ...] }` with only the counters that are not 0.
    pub fn to_json(&self) -> Value {
        let counts: Vec<Value> = self
            .counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(index, count)| json!([index, count]))
            .collect();
        json!({ "count": self.count, "sum": self.sum, "min": self.min, "max": self.max, "counts": counts })
    }

    /// A histogram from [`Histogram::to_json`]'s form.
    ///
    /// # Errors
    ///
    /// When the value is not of that form, or its counts do not add up to its count.
    pub fn from_json(value: &Value) -> Result<Histogram, String> {
        let number = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("a histogram's {name} is a whole number"))
        };
        let mut out = Histogram {
            counts: Vec::new(),
            count: number("count")?,
            sum: number("sum")?,
            min: number("min")?,
            max: number("max")?,
        };
        if out.min > out.max {
            return Err(format!("a histogram's min {} is over its max {}", out.min, out.max));
        }
        let pairs = value
            .get("counts")
            .and_then(Value::as_array)
            .ok_or("a histogram has counts")?;
        let mut total = 0u64;
        for pair in pairs {
            let (Some(index), Some(count)) = (pair.get(0).and_then(Value::as_u64), pair.get(1).and_then(Value::as_u64))
            else {
                return Err(format!("a histogram count is [index, count], not {pair}"));
            };
            let index = usize::try_from(index)
                .ok()
                .filter(|index| *index < LEN)
                .ok_or_else(|| format!("a histogram counter {index} is past the last, {}", LEN - 1))?;
            if out.counts.is_empty() {
                out.counts = vec![0; LEN];
            }
            out.counts[index] = out.counts[index]
                .checked_add(count)
                .ok_or("a histogram's counts overflow")?;
            total = total.checked_add(count).ok_or("a histogram's counts overflow")?;
        }
        if out.count == 0 && total > 0 {
            return Err(format!("a histogram with a count of 0 has counts adding up to {total}"));
        }
        if total != out.count {
            return Err(format!(
                "a histogram's counts add up to {total}, not its count {}",
                out.count
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "histogram_tests.rs"]
mod tests;
