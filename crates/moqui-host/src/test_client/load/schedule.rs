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

//! A scenario's schedule (plans/LoadRunner.md section 5, "An arrival rate", and L6): when its users may start an
//! iteration. Times are seconds from the load's start, passed in as `now`, so the schedule keeps no clock of its own and
//! a test drives it with made-up times.
//!
//! **Closed** (no `rate`): the target is a number of users over time. Without `stages` it is `users` throughout, and
//! the users start along `rampUp` as before. With `stages` (`[{ duration, users }]`) the target ramps linearly within
//! each stage from the one before (the first from 0), and holds the last stage's after it; user `u` (from 0) may start
//! an iteration while `u < floor(target)`, at most `users` (by default the largest stage's). A user above the target
//! finishes its iteration and waits, untimed and not counted active, until the target is above it again; users are
//! added as the target rises.
//!
//! **Rate** (`rate`, or stages of `[{ duration, rate }]`, not both): iterations started per second, ramped the same way
//! (and with no stages, over `rampUp` from 0 when it is given). A rate scenario's clock starts when its first user
//! begins its timed part, so that the users' boot and setup are not counted as dropped starts. The `k`-th start (from
//! 0) is scheduled where the integral of the rate from then reaches `k`, so with a constant rate `r` it is at `k / r`;
//! the first is at the first time the rate is above 0. `users` is the most that run at
//! once (by default the highest rate rounded up), and a free user takes the next scheduled start once it is due. A
//! start is **dropped** when no user has taken it by the time of the start after it (one interval late), or 25 ms after
//! its own time if that is later; a dropped start is not run, and is counted in `dropped`. `iterations` in a rate
//! scenario is the scenario's starts in all, run or dropped. When the scenario ends (its duration, or a stop), the starts
//! due before then that no user took are dropped too. A user the load adds later (a higher `users` set live) takes
//! starts on the schedule as it stands, so the starts due while its thread boots may be dropped.
//!
//! **Live target** ([`Schedule::set_target`]): a new number of users or rate holds from that moment on, in place of the
//! rest of the stages.

/// The highest rate, in iterations started per second.
pub(crate) const MAX_RATE: f64 = 10_000.0;

/// The most stages a scenario has.
pub(crate) const MAX_STAGES: usize = 100;

/// The least time, in seconds, a start may be late before it is dropped, whatever the rate: a user's wake-up is not
/// to the microsecond.
pub(crate) const MIN_LATE: f64 = 0.025;

/// How long, in seconds, a waiting user sleeps before it asks the schedule again.
pub(crate) const POLL: f64 = 0.05;

/// One stage: its length in seconds and the target it ramps to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stage {
    pub(crate) duration: f64,
    pub(crate) target: f64,
}

/// A target set while the scenario runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Target {
    /// Users at once: a closed scenario's target, or a rate scenario's most at once.
    Users(u64),
    /// Iterations started per second, of a rate scenario.
    Rate(f64),
}

/// A part of a [`Profile`], from `start` at `from` to `end` at `to`, linearly.
#[derive(Clone, Debug, PartialEq)]
struct Segment {
    from: f64,
    to: f64,
    start: f64,
    end: f64,
}

impl Segment {
    fn value(&self, t: f64) -> f64 {
        self.start + (self.end - self.start) * (t - self.from) / (self.to - self.from)
    }

    fn area_to(&self, t: f64) -> f64 {
        let x = t.clamp(self.from, self.to);
        (self.start + self.value(x)) / 2.0 * (x - self.from)
    }
}

/// A target over time: linear segments from `origin`, then `hold`. For a rate, [`Profile::count`] is its integral
/// from the load's start, kept across a change at `origin`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Profile {
    origin: f64,
    count_at_origin: f64,
    segments: Vec<Segment>,
    hold: f64,
}

impl Profile {
    pub(crate) fn constant(value: f64) -> Profile {
        Profile {
            origin: 0.0,
            count_at_origin: 0.0,
            segments: Vec::new(),
            hold: value,
        }
    }

    /// The stages from 0 at the load's start, each ramping from the one before, and the last one's target held.
    pub(crate) fn stages(stages: &[Stage]) -> Profile {
        let (mut at, mut previous) = (0.0, 0.0);
        let mut segments = Vec::new();
        for stage in stages {
            segments.push(Segment {
                from: at,
                to: at + stage.duration,
                start: previous,
                end: stage.target,
            });
            at += stage.duration;
            previous = stage.target;
        }
        Profile {
            origin: 0.0,
            count_at_origin: 0.0,
            segments,
            hold: previous,
        }
    }

    fn last_end(&self) -> f64 {
        self.segments.last().map_or(self.origin, |segment| segment.to)
    }

    pub(crate) fn value(&self, t: f64) -> f64 {
        match self.segments.iter().find(|segment| t < segment.to) {
            Some(segment) if t >= segment.from => segment.value(t),
            Some(segment) => segment.start,
            None => self.hold,
        }
    }

    /// The highest target from `t` on.
    fn max_from(&self, t: f64) -> f64 {
        self.segments
            .iter()
            .filter(|segment| segment.to > t)
            .map(|segment| segment.end)
            .fold(self.value(t).max(self.hold), f64::max)
    }

    /// The integral from the load's start to `t`: the starts scheduled before `t`, fractionally.
    pub(crate) fn count(&self, t: f64) -> f64 {
        if t <= self.origin {
            return self.count_at_origin;
        }
        let ramped: f64 = self.segments.iter().map(|segment| segment.area_to(t)).sum();
        self.count_at_origin + ramped + self.hold * (t - self.last_end()).max(0.0)
    }

    /// The time at which [`Profile::count`] reaches `k`, from `origin` on; none when it never does (a rate that ends
    /// at 0). A `k` already reached at `origin` (the first start) is at the first time the rate is above 0.
    pub(crate) fn time_of_count(&self, k: f64) -> Option<f64> {
        let mut reached = self.count_at_origin;
        if k <= reached {
            let rising = self
                .segments
                .iter()
                .find(|segment| segment.start > 0.0 || segment.end > 0.0)
                .map(|segment| segment.from.max(self.origin));
            return rising.or_else(|| (self.hold > 0.0).then(|| self.last_end()));
        }
        for segment in &self.segments {
            let area = (segment.start + segment.end) / 2.0 * (segment.to - segment.from);
            if reached + area >= k {
                // start * d + slope * d^2 / 2 = left, in the form that holds for a slope of 0
                let left = k - reached;
                let slope = (segment.end - segment.start) / (segment.to - segment.from);
                let root = (segment.start * segment.start + 2.0 * slope * left).max(0.0).sqrt();
                let denominator = segment.start + root;
                let d = if denominator > 0.0 {
                    2.0 * left / denominator
                } else {
                    0.0
                };
                return Some((segment.from + d).min(segment.to));
            }
            reached += area;
        }
        (self.hold > 0.0).then(|| self.last_end() + (k - reached) / self.hold)
    }

    /// From `t` on the target is `value`, in place of the rest; the count up to `t` is kept.
    fn hold_from(&mut self, t: f64, value: f64) {
        let count = self.count(t);
        *self = Profile {
            origin: t,
            count_at_origin: count,
            segments: Vec::new(),
            hold: value,
        };
    }
}

/// What a free user is told by [`Schedule::take`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Take {
    /// Start an iteration: in a rate scenario, the one scheduled at `scheduled`.
    Go { scheduled: Option<f64> },
    /// Ask again at this time, or sooner.
    Wait(f64),
    /// The scenario has no more iterations for this user.
    Done,
}

/// One scenario's schedule (see the module doc). Its methods take `now` in seconds from the load's start; a rate
/// scenario's starts are kept on its own clock, from `shift`.
#[derive(Clone, Debug)]
pub(crate) struct Schedule {
    rate: bool,
    profile: Profile,
    /// The most users at once.
    cap: u64,
    /// When the scenario ends, from the load's start.
    end: Option<f64>,
    /// A rate scenario's starts in all.
    total: Option<u64>,
    /// When the scenario's clock started, from the load's start: at once for a closed one, at its first user's timed
    /// part for a rate one.
    shift: Option<f64>,
    /// The index of the next start not taken or dropped, and its time on the scenario's clock; infinite when none is
    /// coming (a rate of 0).
    next_index: u64,
    next_at: f64,
    /// The total is reached: no start is left, whatever the rate.
    counted_out: bool,
    /// The next start falls at or past the end: none is left unless a new rate places one before it.
    past_end: bool,
    /// The schedule holds users back (stages, a rate, or a target set live): a plain closed one never does.
    gates: bool,
    dropped: u64,
    /// The times (from the load's start) of the starts dropped and not yet taken by [`Schedule::take_drops`].
    drops: Vec<f64>,
}

impl Schedule {
    /// A closed schedule: `profile` users, at most `cap`, until `end`.
    pub(crate) fn closed(profile: Profile, cap: u64, end: Option<f64>) -> Schedule {
        let gates = profile != Profile::constant(profile.hold);
        Schedule {
            rate: false,
            profile,
            cap,
            end,
            total: None,
            shift: Some(0.0),
            next_index: 0,
            next_at: f64::INFINITY,
            counted_out: false,
            past_end: false,
            gates,
            dropped: 0,
            drops: Vec::new(),
        }
    }

    /// A rate schedule: `profile` starts a second, at most `cap` users at once, until `end` or `total` starts. Its
    /// clock starts at [`Schedule::start_clock`], or at the first [`Schedule::take`].
    pub(crate) fn rate(profile: Profile, cap: u64, end: Option<f64>, total: Option<u64>) -> Schedule {
        let mut schedule = Schedule {
            rate: true,
            profile,
            cap,
            end,
            total,
            shift: None,
            next_index: 0,
            next_at: f64::INFINITY,
            counted_out: false,
            past_end: false,
            gates: true,
            dropped: 0,
            drops: Vec::new(),
        };
        schedule.place_next();
        schedule
    }

    pub(crate) fn is_rate(&self) -> bool {
        self.rate
    }

    pub(crate) fn cap(&self) -> u64 {
        self.cap
    }

    /// Whether a user must ask before each iteration: false for a plain closed scenario whose target was not set.
    pub(crate) fn gates(&self) -> bool {
        self.gates
    }

    #[cfg(test)]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The scenario's clock starts at `now`, unless it has.
    pub(crate) fn start_clock(&mut self, now: f64) {
        if self.shift.is_none() {
            self.shift = Some(now);
            self.place_next();
        }
    }

    /// `now` on the scenario's clock: 0 before it starts.
    fn local(&self, now: f64) -> f64 {
        (now - self.shift.unwrap_or(now)).max(0.0)
    }

    /// A time on the scenario's clock from the load's start.
    fn global(&self, at: f64) -> f64 {
        at + self.shift.unwrap_or(0.0)
    }

    /// When start `k` (from 0) is due, in seconds from the scenario's clock start; none at a rate that stays 0.
    pub(crate) fn time_of_start(&self, k: u64) -> Option<f64> {
        self.profile.time_of_count(k as f64)
    }

    /// The target at `now`: a closed scenario's users, or a rate scenario's rate.
    pub(crate) fn target(&self, now: f64) -> Target {
        if self.rate {
            Target::Rate(self.profile.value(self.local(now)))
        } else {
            Target::Users(self.allowed(now))
        }
    }

    /// The users a closed scenario allows at `now`.
    fn allowed(&self, now: f64) -> u64 {
        let target = (self.profile.value(self.local(now)) + 1e-9).floor().max(0.0);
        (target as u64).min(self.cap)
    }

    fn ended(&self, now: f64) -> bool {
        self.end.is_some_and(|end| now >= end)
    }

    fn exhausted(&self) -> bool {
        self.counted_out || self.past_end
    }

    /// Place the start at `next_index`, or mark that there is none.
    fn place_next(&mut self) {
        if self.total.is_some_and(|total| self.next_index >= total) {
            self.counted_out = true;
            self.next_at = f64::INFINITY;
            return;
        }
        self.next_at = self
            .profile
            .time_of_count(self.next_index as f64)
            .unwrap_or(f64::INFINITY);
        // none coming at a rate of 0 is not past the end: a new rate may place one
        self.past_end = self.next_at.is_finite() && self.end.is_some_and(|end| self.global(self.next_at) >= end);
        if self.past_end {
            self.next_at = f64::INFINITY;
        }
    }

    fn advance(&mut self) {
        self.next_index += 1;
        self.place_next();
    }

    fn drop_next(&mut self) {
        self.dropped += 1;
        self.drops.push(self.global(self.next_at));
        self.advance();
    }

    /// When the next start is dropped if no user takes it, on the scenario's clock: the time of the start after it,
    /// and at least [`MIN_LATE`] after its own.
    fn late_at(&self) -> f64 {
        let following = self
            .profile
            .time_of_count(self.next_index as f64 + 1.0)
            .unwrap_or(f64::INFINITY);
        following.max(self.next_at + MIN_LATE)
    }

    /// User `user`, free at `now`, asks for its next iteration. A rate scenario's scheduled start is from the load's
    /// start.
    pub(crate) fn take(&mut self, user: u64, now: f64) -> Take {
        if self.ended(now) {
            return Take::Done;
        }
        if !self.rate {
            return if user < self.allowed(now) {
                Take::Go { scheduled: None }
            } else {
                Take::Wait(now + POLL)
            };
        }
        self.start_clock(now);
        self.drop_overdue(now);
        if self.counted_out {
            return Take::Done;
        }
        let poll = self.end.map_or(now + POLL, |end| end.min(now + POLL));
        if self.past_end || user >= self.cap {
            return Take::Wait(poll);
        }
        let at = self.local(now);
        if self.next_at <= at {
            let scheduled = self.global(self.next_at);
            self.advance();
            return Take::Go {
                scheduled: Some(scheduled),
            };
        }
        Take::Wait(self.global(self.next_at).min(poll))
    }

    /// Drop the starts that no user took in time, as of `now`.
    pub(crate) fn drop_overdue(&mut self, now: f64) {
        if self.shift.is_none() {
            return;
        }
        let at = self.local(now);
        while self.rate && !self.exhausted() && self.next_at.is_finite() && at > self.late_at() {
            self.drop_next();
        }
    }

    /// The scenario is over at `until`: every start due before it that no user took is dropped.
    pub(crate) fn settle(&mut self, until: f64) {
        if self.shift.is_none() {
            return;
        }
        let at = self.local(until);
        while self.rate && !self.exhausted() && self.next_at < at {
            self.drop_next();
        }
    }

    /// The times of the starts dropped since the last call, from the load's start.
    pub(crate) fn take_drops(&mut self) -> Vec<f64> {
        std::mem::take(&mut self.drops)
    }

    /// The user threads the scenario wants at `now`.
    pub(crate) fn threads_wanted(&self, now: f64) -> u64 {
        if self.rate {
            self.cap
        } else {
            self.allowed(now)
        }
    }

    /// Whether the scenario may want more than `threads` users after `now`, so that the load waits for it.
    pub(crate) fn may_want_more(&self, now: f64, threads: u64) -> bool {
        if self.ended(now) || self.counted_out {
            return false;
        }
        let most = if self.rate {
            self.cap
        } else {
            ((self.profile.max_from(self.local(now)) + 1e-9).floor().max(0.0) as u64).min(self.cap)
        };
        most > threads
    }

    /// From `now` on the target is `target`. A closed scenario takes users (its target and most at once); a rate
    /// scenario takes a rate, or users as its most at once.
    ///
    /// # Errors
    ///
    /// For a rate given to a closed scenario.
    pub(crate) fn set_target(&mut self, now: f64, target: Target) -> Result<(), String> {
        let at = self.local(now);
        match (target, self.rate) {
            (Target::Users(users), false) => {
                self.profile.hold_from(at, users as f64);
                self.cap = users;
            }
            (Target::Users(users), true) => self.cap = users,
            (Target::Rate(_), false) => {
                return Err("it has no rate: give it users".to_string());
            }
            (Target::Rate(rate), true) => {
                // the starts overdue on the old rate are dropped on it, so the new one does not start them at once
                self.drop_overdue(now);
                self.profile.hold_from(at, rate);
                // a start already due keeps its time; the next one is placed on the new rate, before the end or not
                if !self.counted_out && (self.past_end || self.next_at > at) {
                    self.place_next();
                }
            }
        }
        self.gates = true;
        Ok(())
    }
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
