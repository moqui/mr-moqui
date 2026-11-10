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

//! A load's time series (plans/LoadRunner.md section 5): one bin a second from the run's start, with its active users,
//! requests, errors, the scheduled starts dropped in it (a rate scenario's), mean, p50, p95, p99 and max, computed from that second's samples when it closes, after which they
//! are dropped. Past [`BIN_CAP`] bins, neighbours merge and the bin length doubles. Per name, [`Series`] keeps count,
//! errors and mean in bins of 10 seconds, merged the same way.

use std::collections::BTreeMap;

use serde_json::{json, Value};

/// The bins a run keeps before neighbours merge.
pub(crate) const BIN_CAP: usize = 7_200;

/// A name's bin length to start with, in seconds.
pub(crate) const NAME_BIN_SECONDS: u64 = 10;

/// The points of a live snapshot's series, at most.
pub(crate) const SNAPSHOT_POINTS: usize = 300;

/// Seconds a second stays open after it ends, for the samples a user thread has not merged yet (each merges at most
/// once a second).
pub(crate) const CLOSE_LAG: u64 = 2;

fn ms(micros: f64) -> f64 {
    (micros / 1000.0 * 1000.0).round() / 1000.0
}

/// One closed bin; times in microseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Bin {
    pub(crate) users: u64,
    pub(crate) requests: u64,
    pub(crate) errors: u64,
    /// A rate scenario's scheduled starts of this bin that were dropped.
    pub(crate) dropped: u64,
    pub(crate) sum: u64,
    pub(crate) p50: u64,
    pub(crate) p95: u64,
    pub(crate) p99: u64,
    pub(crate) max: u64,
}

/// The value at `p` of sorted samples, by nearest rank; 0 for none.
fn nearest_rank(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

impl Bin {
    fn of_second(mut open: Open) -> Bin {
        open.micros.sort_unstable();
        Bin {
            users: open.users,
            requests: open.micros.len() as u64,
            errors: open.errors,
            dropped: open.dropped,
            sum: open.micros.iter().sum(),
            p50: nearest_rank(&open.micros, 50.0),
            p95: nearest_rank(&open.micros, 95.0),
            p99: nearest_rank(&open.micros, 99.0),
            max: open.micros.last().copied().unwrap_or(0),
        }
    }

    /// Two neighbours as one. The counts add and the max is the larger; a percentile of the two is the mean of theirs
    /// weighted by requests, an approximation, since a bin keeps no samples once closed.
    pub(crate) fn merge(&self, other: &Bin) -> Bin {
        let requests = self.requests + other.requests;
        let weighted = |a: u64, b: u64| {
            if requests == 0 {
                0
            } else {
                ((a as f64 * self.requests as f64 + b as f64 * other.requests as f64) / requests as f64).round() as u64
            }
        };
        Bin {
            users: self.users.max(other.users),
            requests,
            errors: self.errors + other.errors,
            dropped: self.dropped + other.dropped,
            sum: self.sum + other.sum,
            p50: weighted(self.p50, other.p50),
            p95: weighted(self.p95, other.p95),
            p99: weighted(self.p99, other.p99),
            max: self.max.max(other.max),
        }
    }

    /// `t` is the bin's start in seconds from the run's; times in milliseconds.
    fn to_json(self, t: u64) -> Value {
        let mean = if self.requests == 0 {
            0.0
        } else {
            self.sum as f64 / self.requests as f64
        };
        json!({
            "t": t, "users": self.users, "requests": self.requests, "errors": self.errors, "dropped": self.dropped, "mean": ms(mean),
            "p50": ms(self.p50 as f64), "p95": ms(self.p95 as f64), "p99": ms(self.p99 as f64), "max": ms(self.max as f64),
        })
    }
}

/// A second not yet closed: its samples.
#[derive(Default)]
struct Open {
    micros: Vec<u64>,
    errors: u64,
    dropped: u64,
    users: u64,
}

/// A run's bins: closed ones, and the seconds still open.
pub(crate) struct Bins {
    cap: usize,
    /// The length of a closed bin in seconds: 1, doubled each time the bins pass the cap.
    seconds: u64,
    /// `closed[i]` is seconds `i * seconds` to `(i + 1) * seconds`.
    closed: Vec<Bin>,
    /// The seconds before this one are closed.
    closed_through: u64,
    open: BTreeMap<u64, Open>,
}

impl Default for Bins {
    fn default() -> Self {
        Bins::new(BIN_CAP)
    }
}

impl Bins {
    pub(crate) fn new(cap: usize) -> Bins {
        Bins {
            cap: cap.max(2),
            seconds: 1,
            closed: Vec::new(),
            closed_through: 0,
            open: BTreeMap::new(),
        }
    }

    /// A sample at `second`. One for a second already closed adds to its bin's counts, not its percentiles.
    pub(crate) fn add(&mut self, second: u64, micros: u64, error: bool) {
        if second < self.closed_through {
            if let Some(bin) = self.closed.get_mut((second / self.seconds) as usize) {
                bin.requests += 1;
                bin.errors += u64::from(error);
                bin.sum += micros;
                bin.max = bin.max.max(micros);
            }
            return;
        }
        let open = self.open.entry(second).or_default();
        open.micros.push(micros);
        open.errors += u64::from(error);
    }

    /// A scheduled start of `second` dropped.
    pub(crate) fn add_dropped(&mut self, second: u64) {
        if second < self.closed_through {
            if let Some(bin) = self.closed.get_mut((second / self.seconds) as usize) {
                bin.dropped += 1;
            }
            return;
        }
        self.open.entry(second).or_default().dropped += 1;
    }

    /// The active users at `second`: the bin keeps the most seen.
    pub(crate) fn note_users(&mut self, second: u64, users: u64) {
        if second >= self.closed_through {
            let open = self.open.entry(second).or_default();
            open.users = open.users.max(users);
        }
    }

    /// Close every second before `second`, one with no samples as an empty bin.
    pub(crate) fn close_before(&mut self, second: u64) {
        while self.closed_through < second {
            let at = self.closed_through;
            let open = self.open.remove(&at).unwrap_or_default();
            self.push(at, Bin::of_second(open));
            self.closed_through += 1;
        }
    }

    fn push(&mut self, second: u64, bin: Bin) {
        let index = (second / self.seconds) as usize;
        if let Some(last) = self.closed.get_mut(index) {
            *last = last.merge(&bin);
        } else {
            self.closed.resize(index, Bin::default());
            self.closed.push(bin);
        }
        if self.closed.len() > self.cap {
            self.closed = merge_groups(&self.closed, 2);
            self.seconds *= 2;
        }
    }

    #[cfg(test)]
    pub(crate) fn seconds(&self) -> u64 {
        self.seconds
    }

    #[cfg(test)]
    pub(crate) fn closed(&self) -> &[Bin] {
        &self.closed
    }

    /// The closed bins as JSON, merged to at most `points` when given, and their length in seconds.
    pub(crate) fn to_json(&self, points: Option<usize>) -> (Vec<Value>, u64) {
        let group = points.map_or(1, |points| self.closed.len().div_ceil(points.max(1)).max(1));
        let seconds = self.seconds * group as u64;
        let bins = merge_groups(&self.closed, group);
        let out = bins
            .into_iter()
            .enumerate()
            .map(|(index, bin)| bin.to_json(index as u64 * seconds))
            .collect();
        (out, seconds)
    }
}

fn merge_groups(bins: &[Bin], group: usize) -> Vec<Bin> {
    if group <= 1 {
        return bins.to_vec();
    }
    bins.chunks(group)
        .map(|chunk| chunk.iter().skip(1).fold(chunk[0], |into, bin| into.merge(bin)))
        .collect()
}

/// A name's counts in one bin of its [`Series`]; times in microseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    pub(crate) count: u64,
    pub(crate) errors: u64,
    pub(crate) sum: u64,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.count += other.count;
        self.errors += other.errors;
        self.sum += other.sum;
    }
}

/// A name's bins: count, errors and mean, which add, so a sample goes straight in whenever it comes.
pub(crate) struct Series {
    cap: usize,
    seconds: u64,
    bins: Vec<Tally>,
}

impl Default for Series {
    fn default() -> Self {
        Series::new(NAME_BIN_SECONDS, BIN_CAP)
    }
}

impl Series {
    pub(crate) fn new(seconds: u64, cap: usize) -> Series {
        Series {
            cap: cap.max(2),
            seconds: seconds.max(1),
            bins: Vec::new(),
        }
    }

    pub(crate) fn add(&mut self, second: u64, micros: u64, error: bool) {
        let mut index = (second / self.seconds) as usize;
        while index >= self.cap {
            self.bins = tally_groups(&self.bins, 2);
            self.seconds *= 2;
            index = (second / self.seconds) as usize;
        }
        if self.bins.len() <= index {
            self.bins.resize(index + 1, Tally::default());
        }
        let bin = &mut self.bins[index];
        bin.count += 1;
        bin.errors += u64::from(error);
        bin.sum += micros;
    }

    #[cfg(test)]
    pub(crate) fn seconds(&self) -> u64 {
        self.seconds
    }

    #[cfg(test)]
    pub(crate) fn bins(&self) -> &[Tally] {
        &self.bins
    }

    /// `[{ t, count, errors, mean }]`, mean in milliseconds, merged to at most `points` when given; and the bin length.
    pub(crate) fn to_json(&self, points: Option<usize>) -> (Vec<Value>, u64) {
        let group = points.map_or(1, |points| self.bins.len().div_ceil(points.max(1)).max(1));
        let seconds = self.seconds * group as u64;
        let out = tally_groups(&self.bins, group)
            .into_iter()
            .enumerate()
            .map(|(index, tally)| {
                let mean = if tally.count == 0 {
                    0.0
                } else {
                    tally.sum as f64 / tally.count as f64
                };
                json!({ "t": index as u64 * seconds, "count": tally.count, "errors": tally.errors, "mean": ms(mean) })
            })
            .collect();
        (out, seconds)
    }
}

fn tally_groups(bins: &[Tally], group: usize) -> Vec<Tally> {
    if group <= 1 {
        return bins.to_vec();
    }
    bins.chunks(group)
        .map(|chunk| {
            let mut out = Tally::default();
            for tally in chunk {
                out.add(tally);
            }
            out
        })
        .collect()
}
