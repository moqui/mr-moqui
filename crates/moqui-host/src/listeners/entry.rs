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

//! One listener's wanted-against-running diff, held once for every listener (plans/Runtime-Config.md §3.1):
//!
//! | Wanted against running | Action |
//! |---|---|
//! | Same | Nothing |
//! | Off, running | Stop, with the grace or at once |
//! | On, not running | Start; a failed start is recorded and the pass goes on |
//! | Bind settings differ | Start the new, then stop the old; when they overlap, stop the old, then bind again |
//! | In-place settings differ | Apply them to what runs |
//!
//! A failed new bind keeps the old one running, or binds its settings again, and the outcome says "kept on …"; the
//! change's in-place settings apply to it either way, so a stricter one does not wait for the bind. A stop
//! a pass makes only begins here ([`Stopping`]): the pass waits for all of them together once each has begun, so their
//! graces overlap. Only an overlapping rebind waits for the old one inline, since the new one binds its port.
//!
//! [`Slot`] holds it for one [`Listener`] and the registry stores it as an object-safe [`Entry`]. Each call into the
//! listener catches a panic as that listener's failure, naming the step, so one listener's panic fails it alone.

use std::net::SocketAddr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

use moqui_core::listeners::{Action, ListenerInfo, Outcome};
use moqui_core::startup::{Section, State};
use moqui_lib::error::Error;

use super::{panic_text, Ctx, Listener, Now, Pass, Running, Stage, StopHow, StopPhase, Wanted, LIVE_STOP_GRACE};

/// How long a stop waits past its grace for the listener to report it finished; one still not finished is dropped,
/// which ends what is left.
const STOP_MARGIN: Duration = Duration::from_secs(2);

/// What a pass that may start nothing says of a listener it would have started, and the row of one the process stop
/// stopped.
const STOPPING: &str = "the server is stopping";

/// A listener and what runs of it, as the registry stores it.
pub(crate) trait Entry: Send {
    fn name(&self) -> &'static str;
    fn stage(&self) -> Stage;
    fn stop_phase(&self) -> StopPhase;
    fn stop_rank(&self) -> u8;
    fn live(&self) -> bool;
    fn fatal_at_boot(&self) -> bool;
    /// The conf settings it reads ([`Listener::selectors`]), for the test that live entries match the conf schema.
    #[cfg(test)]
    fn selectors(&self) -> &'static [&'static str];
    /// The rows its own start records beside its row.
    fn extra_rows(&self) -> &'static [&'static str];
    /// The error of its failed start in this boot pass, when it is fatal at boot: the boot's own error, with its
    /// source chain.
    fn take_boot_error(&mut self) -> Option<Error>;
    /// Whether a boot pass of its stage has acted on it: until then a later pass leaves it alone.
    fn booted(&self) -> bool;
    fn is_running(&self) -> bool;
    /// Make what runs match what the conf wants, recording each action on the listener's row. A stop it makes is
    /// begun and put in `stopping`, for the pass to wait for once every stop has begun.
    fn reconcile(&mut self, ctx: &Ctx, pass: &Pass, stopping: &mut Vec<Stopping>) -> Outcome;
    /// For a listener that is not live: note what the conf wants as pending, and start or stop nothing.
    fn hold(&mut self, ctx: &Ctx) -> Outcome;
    /// Begin to stop what runs for the process stop; never waits.
    fn begin_stop(&mut self, how: StopHow);
    /// Wait until `deadline` for the stop begun to finish, drop what ran here and record its rows off.
    fn finish_stop(&mut self, ctx: &Ctx, pass: &Pass, deadline: Instant);
    /// Take what runs out, with no stop, for the caller to drop.
    fn take_running(&mut self) -> Option<Box<dyn Send>>;
    fn address(&self) -> Option<SocketAddr>;
    fn info(&self, ctx: &Ctx) -> ListenerInfo;
    /// Tell its listener's readers what it does now ([`Listener::published`]); a panic there is logged.
    fn tell(&self, ctx: &Ctx);
}

/// A started listener as a stop waits for it, whatever its listener's types.
trait Finishing: Send {
    fn released(&mut self, wait: Duration) -> bool;
    fn finished(&mut self, wait: Duration) -> bool;
}

struct RunOf<L: Listener>(L::Run);

impl<L: Listener> Finishing for RunOf<L> {
    fn released(&mut self, wait: Duration) -> bool {
        self.0.released(wait)
    }

    fn finished(&mut self, wait: Duration) -> bool {
        self.0.finished(wait)
    }
}

/// A listener whose stop has begun, waited for until its deadline and then dropped on the calling thread. While it
/// drains, what ends its connections now is in the registry's [`super::Draining`].
pub(crate) struct Stopping {
    name: &'static str,
    run: Box<dyn Finishing>,
    deadline: Instant,
    _draining: Option<super::DrainGuard>,
}

impl Stopping {
    /// Wait until the deadline for its port to be let go, which comes before its connections finish; false when it
    /// was not by then.
    fn released(&mut self) -> bool {
        let wait = self.deadline.saturating_duration_since(Instant::now());
        match catch_unwind(AssertUnwindSafe(|| self.run.released(wait))) {
            Ok(released) => released,
            Err(panic) => {
                tracing::error!(
                    "The {} listener's stop panicked: {}",
                    self.name,
                    panic_text(panic.as_ref())
                );
                false
            }
        }
    }

    /// Wait until the deadline for the stop to finish, then drop it here; one that does not finish is dropped
    /// anyway, which ends what is left. A panic in either is logged naming the listener.
    pub(crate) fn finish(self) {
        let Self {
            name,
            mut run,
            deadline,
            _draining,
        } = self;
        let wait = deadline.saturating_duration_since(Instant::now());
        match catch_unwind(AssertUnwindSafe(|| run.finished(wait))) {
            Ok(true) => {}
            Ok(false) => tracing::warn!(
                "The {name} listener did not finish stopping within {:.1} s; its connections are dropped",
                wait.as_secs_f64()
            ),
            Err(panic) => tracing::error!(
                "The {name} listener's stop panicked, it is dropped: {}",
                panic_text(panic.as_ref())
            ),
        }
        if let Err(panic) = catch_unwind(AssertUnwindSafe(move || drop(run))) {
            tracing::error!(
                "The {name} listener panicked as it was dropped: {}",
                panic_text(panic.as_ref())
            );
        }
    }
}

/// How long after it begins a stop of `how` may take: the grace and a margin. The process stop's wait is the
/// phase's own ([`super::Registry::stop`]).
fn wait_for(how: StopHow) -> Duration {
    match how {
        StopHow::Grace(grace) => grace.saturating_add(STOP_MARGIN),
        StopHow::Now | StopHow::Process => STOP_MARGIN,
    }
}

/// What runs of a listener and the settings it was started with.
struct Current<L: Listener> {
    bind: L::Bind,
    in_place: L::InPlace,
    run: L::Run,
}

/// A listener and what runs of it.
pub(crate) struct Slot<L: Listener> {
    listener: L,
    running: Option<Current<L>>,
    /// Set by the first boot pass of its stage.
    booted: bool,
    /// The settings a wrapped start of today found off (`start` answered `Ok(None)`): not started again, and not
    /// pending, while the conf still wants them.
    found_off: Option<(L::Bind, L::InPlace)>,
    /// The state the registry itself last recorded on the listener's row. A pass that changes nothing records again
    /// only when this is stale, so a row the subsystem records itself (HTTPS standing by) is not overwritten.
    recorded: Option<State>,
    /// What the conf wants of a listener that is not live, which waits for a restart.
    pending: Option<String>,
    /// Why its last start or change failed, until one succeeds, it is turned off or the conf wants what runs.
    last_error: Option<String>,
    /// Its failed start's error in a boot pass, when it is fatal at boot, for the pass to return.
    boot_error: Option<Error>,
    /// Why it is off (or skipped), as its `wanted` said, since the pass that turned it off or found it off.
    off: Option<String>,
}

impl<L: Listener> Slot<L> {
    pub(crate) fn new(listener: L) -> Self {
        Self {
            listener,
            running: None,
            booted: false,
            found_off: None,
            recorded: None,
            pending: None,
            last_error: None,
            boot_error: None,
            off: None,
        }
    }

    fn name(&self) -> &'static str {
        self.listener.name()
    }

    fn outcome(&self, action: Action, detail: impl Into<String>) -> Outcome {
        Outcome {
            name: self.name().to_string(),
            action,
            detail: detail.into(),
        }
    }

    /// Run a call into the listener, a panic caught as an error naming `step`.
    fn guard<T>(&self, step: &str, call: impl FnOnce() -> T) -> Result<T, String> {
        catch_unwind(AssertUnwindSafe(call))
            .map_err(|panic| format!("its {step} panicked: {}", panic_text(panic.as_ref())))
    }

    /// The listener's startup report row.
    fn row(&self, ctx: &Ctx) -> Option<moqui_core::startup::Entry> {
        moqui_core::startup::report(ctx.factory)
            .entries()
            .into_iter()
            .find(|row| row.section == Section::Listener && row.name == self.name())
    }

    fn row_detail(&self, ctx: &Ctx) -> String {
        self.row(ctx)
            .map(|row| row.state.detail().to_string())
            .unwrap_or_default()
    }

    /// Record `state` on the listener's row, then what caused it.
    fn record(&mut self, ctx: &Ctx, pass: &Pass, state: State) {
        moqui_core::startup::listener(ctx.factory, self.name(), state.clone());
        moqui_core::startup::set_cause(ctx.factory, Section::Listener, self.name(), pass.cause.as_str());
        self.recorded = Some(state);
    }

    /// Record `state` in a boot pass, or when the registry's own last record says otherwise: a pass that changes
    /// nothing leaves the row's time and cause as they were.
    fn record_if_stale(&mut self, ctx: &Ctx, pass: &Pass, state: State) -> bool {
        let stale = pass.boot || self.recorded.as_ref() != Some(&state);
        if stale {
            self.record(ctx, pass, state);
        }
        stale
    }

    /// Record the rows it owns beside its own as not running, after a start that failed or panicked before its own
    /// start recorded them, so none is missing.
    fn record_extras_not_running(&self, ctx: &Ctx, pass: &Pass) {
        let reason = format!("{} is not running", self.name());
        for name in self.listener.extra_rows() {
            moqui_core::startup::listener(ctx.factory, *name, State::off(reason.as_str()));
            moqui_core::startup::set_cause(ctx.factory, Section::Listener, name, pass.cause.as_str());
        }
    }

    /// Set the cause on the listener's row and on the rows it owns beside it, which its own start recorded.
    fn set_causes(&self, ctx: &Ctx, pass: &Pass) {
        let names = std::iter::once(self.name()).chain(self.listener.extra_rows().iter().copied());
        for name in names {
            moqui_core::startup::set_cause(ctx.factory, Section::Listener, name, pass.cause.as_str());
        }
    }

    /// Record a stopped listener's row, and the rows it owns beside it, as `state`.
    fn record_stopped(&mut self, ctx: &Ctx, pass: &Pass, state: State) {
        for name in self.listener.extra_rows() {
            moqui_core::startup::listener(ctx.factory, *name, state.clone());
            moqui_core::startup::set_cause(ctx.factory, Section::Listener, name, pass.cause.as_str());
        }
        self.record(ctx, pass, state);
    }

    /// How a listener turned off in this pass stops: at once when the request named it, else with the grace.
    fn live_stop(&self, pass: &Pass) -> StopHow {
        if pass.end_now.iter().any(|name| name == self.name()) {
            StopHow::Now
        } else {
            StopHow::Grace(LIVE_STOP_GRACE)
        }
    }

    /// Begin to stop `run`: it takes no new connections, and its connections end as `how` says; while it drains, a
    /// request may end them now ([`super::Draining`]).
    fn begin(&self, ctx: &Ctx, mut run: L::Run, how: StopHow) -> Stopping {
        if let Err(err) = self.guard("stop", || run.begin_stop(how)) {
            tracing::error!(
                "The {} listener did not stop cleanly, it is dropped: {err}",
                self.name()
            );
        }
        let draining = run.end_now().map(|end_now| ctx.draining.add(self.name(), end_now));
        Stopping {
            name: self.name(),
            run: Box::new(RunOf::<L>(run)),
            deadline: super::deadline_after(wait_for(how)),
            _draining: draining,
        }
    }

    /// Turn off: begin to stop what runs, and record the row (and the rows it owns) as `state`, `Off` or `Skipped`.
    fn turn_off(&mut self, ctx: &Ctx, pass: &Pass, state: State, stopping: &mut Vec<Stopping>) -> Outcome {
        self.found_off = None;
        self.last_error = None;
        self.off = Some(state.detail().to_string());
        let detail = state.detail().to_string();
        match self.running.take() {
            Some(current) => {
                // a conf reload goes ahead whatever uses the listener (the services and the operator socket refuse
                // first, plans/Runtime-Config.md §3.4), and says what it ends with it
                let dependents = self
                    .guard("dependents", || self.listener.dependents(ctx))
                    .unwrap_or_default();
                if !pass.boot && !dependents.is_empty() {
                    tracing::warn!(
                        "The {} listener is turned off ({}) while these use it, which stop with it: {}",
                        self.name(),
                        pass.cause,
                        dependents.join("; ")
                    );
                }
                stopping.push(self.begin(ctx, current.run, self.live_stop(pass)));
                self.record_stopped(ctx, pass, state);
                self.outcome(Action::Stopped, detail)
            }
            None => {
                self.record_if_stale(ctx, pass, state);
                self.outcome(Action::Same, detail)
            }
        }
    }

    /// The listener's start with `bind` and `in_place`, a panic as a failed start.
    fn call_start(&self, ctx: &Ctx, bind: &L::Bind, in_place: &L::InPlace) -> Result<Option<L::Run>, Error> {
        match self.guard("start", || self.listener.start(ctx, bind, in_place)) {
            Ok(started) => started,
            Err(panicked) => Err(Error::new(panicked)),
        }
    }

    fn is_found_off(&self, bind: &L::Bind, in_place: &L::InPlace) -> bool {
        matches!(&self.found_off, Some((off_bind, off_in_place)) if off_bind == bind && off_in_place == in_place)
    }

    /// Start with `bind` and `in_place`, nothing running.
    fn start(&mut self, ctx: &Ctx, pass: &Pass, bind: L::Bind, in_place: L::InPlace) -> Outcome {
        if !pass.may_start {
            return self.outcome(Action::Held, STOPPING);
        }
        if self.is_found_off(&bind, &in_place) {
            // its start found these settings off already: asking again would only record the same row again
            return self.outcome(Action::Same, self.row_detail(ctx));
        }
        match self.call_start(ctx, &bind, &in_place) {
            Ok(Some(run)) => self.started(ctx, pass, Action::Started, Current { bind, in_place, run }),
            Ok(None) => {
                self.found_off(ctx, pass, bind, in_place);
                self.outcome(Action::Same, self.row_detail(ctx))
            }
            Err(err) => self.failed(ctx, pass, err),
        }
    }

    /// `current` was started: keep it and record its state, unless it records its own row (HTTPS standing by), whose
    /// cause alone is set.
    fn started(&mut self, ctx: &Ctx, pass: &Pass, action: Action, current: Current<L>) -> Outcome {
        let state = current.run.state();
        let detail = state.detail().to_string();
        let own_row = current.run.records_own_row();
        self.running = Some(current);
        self.found_off = None;
        self.last_error = None;
        self.off = None;
        if own_row {
            self.recorded = None;
        } else {
            self.record(ctx, pass, state);
        }
        self.set_causes(ctx, pass);
        self.outcome(action, detail)
    }

    /// A wrapped start of today found it off and recorded why itself: the row is the subsystem's, and only the cause
    /// is set.
    fn found_off(&mut self, ctx: &Ctx, pass: &Pass, bind: L::Bind, in_place: L::InPlace) {
        self.found_off = Some((bind, in_place));
        self.recorded = None;
        self.set_causes(ctx, pass);
    }

    /// A start failed and nothing runs: recorded Failed and logged naming the listener, in a boot pass or when the
    /// error is new (a held port retried on each pass logs and records once), and the pass goes on. One that is fatal
    /// at boot is not logged here: its error is kept as the boot's own, which the boot logs.
    fn failed(&mut self, ctx: &Ctx, pass: &Pass, err: Error) -> Outcome {
        let text = err.to_string();
        let fatal = pass.boot && self.listener.fatal_at_boot();
        if !fatal && (pass.boot || self.last_error.as_deref() != Some(text.as_str())) {
            tracing::error!("The {} listener did not start: {text}", self.name());
        }
        if self.record_if_stale(ctx, pass, State::Failed(text.clone())) {
            self.record_extras_not_running(ctx, pass);
        }
        if fatal {
            self.boot_error = Some(err);
        }
        self.last_error = Some(text.clone());
        self.outcome(Action::Failed, text)
    }

    /// A call that is not a start failed (reading its settings, applying them): logged when new, kept as the last
    /// error, and recorded on the row only when nothing runs, so a running listener is not shown as failed.
    fn faulted(&mut self, ctx: &Ctx, pass: Option<&Pass>, err: String) -> Outcome {
        let boot = pass.is_some_and(|pass| pass.boot);
        if boot || self.last_error.as_deref() != Some(err.as_str()) {
            tracing::error!("The {} listener failed: {err}", self.name());
        }
        if let (Some(pass), None) = (pass, &self.running) {
            self.record_if_stale(ctx, pass, State::Failed(err.clone()));
        }
        self.last_error = Some(err.clone());
        self.outcome(Action::Failed, err)
    }

    /// A new bind failed and `current`, on the old bind, runs again or still: recorded on with the failure, which the
    /// outcome reports as "kept on …". Logged in a boot pass or when the error is new, as a failed start is, so a
    /// retried pass does not log it again.
    fn kept(&mut self, ctx: &Ctx, pass: &Pass, current: Current<L>, err: Error) -> Outcome {
        let err = err.to_string();
        if pass.boot || self.last_error.as_deref() != Some(err.as_str()) {
            tracing::error!(
                "The {} listener stays on its old bind, the new one did not bind: {err}",
                self.name()
            );
        }
        let state = current.run.state();
        let at = current
            .run
            .address()
            .map_or_else(|| state.detail().to_string(), |address| address.to_string());
        let state = match state {
            State::On(detail) => State::On(format!("{detail}; the new settings did not bind: {err}")),
            other => other,
        };
        self.running = Some(current);
        self.last_error = Some(err.clone());
        self.record(ctx, pass, state);
        self.outcome(Action::Failed, format!("kept on {at}: {err}"))
    }

    /// A new bind failed and `current` keeps running on its old bind: the change's in-place settings still apply to it,
    /// so a stricter one (SSH's access mode) does not wait for a port that may stay busy. One that does not apply is
    /// logged, and asked again at the next pass.
    fn apply_to_kept(&self, mut current: Current<L>, in_place: L::InPlace) -> Current<L> {
        if current.in_place == in_place {
            return current;
        }
        match self.guard("apply", || current.run.apply(&in_place)) {
            Ok(()) => current.in_place = in_place,
            Err(err) => tracing::error!(
                "The {} listener did not take its new settings in place on its old bind: {err}",
                self.name()
            ),
        }
        current
    }

    /// The same bind: apply `in_place` when it differs. When the conf wants exactly what runs, a past failure is
    /// over.
    fn apply(&mut self, ctx: &Ctx, pass: &Pass, mut current: Current<L>, in_place: L::InPlace) -> Outcome {
        if current.in_place == in_place {
            let state = current.run.state();
            let detail = state.detail().to_string();
            let own_row = current.run.records_own_row();
            self.running = Some(current);
            self.last_error = None;
            if !own_row {
                self.record_if_stale(ctx, pass, state);
            }
            return self.outcome(Action::Same, detail);
        }
        let applied = self.guard("apply", || current.run.apply(&in_place));
        if let Err(err) = applied {
            // what runs keeps running with whatever it took; the conf's settings are asked again next pass
            self.running = Some(current);
            return self.faulted(ctx, Some(pass), err);
        }
        current.in_place = in_place;
        let state = current.run.state();
        let detail = state.detail().to_string();
        self.running = Some(current);
        self.last_error = None;
        self.record(ctx, pass, state);
        self.outcome(Action::Applied, detail)
    }

    /// New bind settings while `old` runs.
    fn rebind(
        &mut self,
        ctx: &Ctx,
        pass: &Pass,
        old: Current<L>,
        bind: L::Bind,
        in_place: L::InPlace,
        stopping: &mut Vec<Stopping>,
    ) -> Outcome {
        if !pass.may_start {
            self.running = Some(old);
            return self.outcome(Action::Held, STOPPING);
        }
        let how = self.live_stop(pass);
        if !self.listener.overlaps(&old.bind, &bind) {
            // a new address binds before the old one closes, so the listener is never closed between
            return match self.call_start(ctx, &bind, &in_place) {
                Ok(Some(run)) => {
                    stopping.push(self.begin(ctx, old.run, how));
                    self.started(ctx, pass, Action::Rebound, Current { bind, in_place, run })
                }
                Ok(None) => {
                    stopping.push(self.begin(ctx, old.run, how));
                    self.found_off(ctx, pass, bind, in_place);
                    self.outcome(Action::Stopped, self.row_detail(ctx))
                }
                Err(err) => {
                    let old = self.apply_to_kept(old, in_place);
                    self.kept(ctx, pass, old, err)
                }
            };
        }
        // the new bind would collide with the old: the old stops first and lets its port go, waited for here since the
        // new one takes it, then drains with the pass's other stops; it binds again when the new one fails, with the
        // change's in-place settings, which need no new bind
        let Current {
            bind: old_bind,
            in_place: _,
            run: old_run,
        } = old;
        let mut old_stop = self.begin(ctx, old_run, how);
        if old_stop.released() {
            stopping.push(old_stop);
        } else {
            // dropped now, which ends its loop and what it holds, so neither the new bind nor the old one's fallback
            // waits on a loop that still holds the port
            tracing::warn!(
                "The {} listener did not let its port go within its stop's wait; it is dropped before the new bind",
                self.name()
            );
            old_stop.finish();
        }
        match self.call_start(ctx, &bind, &in_place) {
            Ok(Some(run)) => self.started(ctx, pass, Action::Rebound, Current { bind, in_place, run }),
            Ok(None) => {
                self.found_off(ctx, pass, bind, in_place);
                self.outcome(Action::Stopped, self.row_detail(ctx))
            }
            Err(err) => match self.call_start(ctx, &old_bind, &in_place) {
                Ok(Some(run)) => {
                    let current = Current {
                        bind: old_bind,
                        in_place,
                        run,
                    };
                    self.kept(ctx, pass, current, err)
                }
                Ok(None) => self.failed(ctx, pass, Error::new(format!("{err}; the old settings were found off"))),
                Err(again) => self.failed(
                    ctx,
                    pass,
                    Error::new(format!("{err}; the old settings did not bind again: {again}")),
                ),
            },
        }
    }

    /// `bind` as the pending text names it: its host and port when it has one.
    fn describe(&self, bind: &L::Bind) -> String {
        self.listener.target(bind).map_or_else(
            || format!("{bind:?}"),
            |target| format!("{}:{}", target.host, target.port),
        )
    }
}

impl<L: Listener> Entry for Slot<L> {
    fn name(&self) -> &'static str {
        self.listener.name()
    }

    fn stage(&self) -> Stage {
        self.listener.stage()
    }

    fn stop_phase(&self) -> StopPhase {
        self.listener.stop_phase()
    }

    fn stop_rank(&self) -> u8 {
        self.listener.stop_rank()
    }

    fn live(&self) -> bool {
        self.listener.live()
    }

    fn fatal_at_boot(&self) -> bool {
        self.listener.fatal_at_boot()
    }

    #[cfg(test)]
    fn selectors(&self) -> &'static [&'static str] {
        self.listener.selectors()
    }

    fn extra_rows(&self) -> &'static [&'static str] {
        self.listener.extra_rows()
    }

    fn take_boot_error(&mut self) -> Option<Error> {
        self.boot_error.take()
    }

    fn booted(&self) -> bool {
        self.booted
    }

    fn is_running(&self) -> bool {
        self.running.is_some()
    }

    fn reconcile(&mut self, ctx: &Ctx, pass: &Pass, stopping: &mut Vec<Stopping>) -> Outcome {
        if pass.boot {
            self.booted = true;
        }
        self.pending = None;
        let wanted = match self.guard("settings", || self.listener.wanted(ctx)) {
            Ok(wanted) => wanted,
            Err(err) => return self.faulted(ctx, Some(pass), err),
        };
        match wanted {
            Wanted::NotYet => self.outcome(Action::Same, "not ready to start yet"),
            Wanted::Off(reason) => self.turn_off(ctx, pass, State::Off(reason), stopping),
            Wanted::Skipped(reason) => self.turn_off(ctx, pass, State::Skipped(reason), stopping),
            Wanted::On { bind, in_place } => match self.running.take() {
                None => self.start(ctx, pass, bind, in_place),
                Some(current) if current.bind == bind => self.apply(ctx, pass, current, in_place),
                Some(current) => self.rebind(ctx, pass, current, bind, in_place, stopping),
            },
        }
    }

    fn hold(&mut self, ctx: &Ctx) -> Outcome {
        let wanted = match self.guard("settings", || self.listener.wanted(ctx)) {
            Ok(wanted) => wanted,
            // nothing changes: what runs keeps running and its row stays as it is
            Err(err) => return self.faulted(ctx, None, err),
        };
        let pending = match (wanted, &self.running) {
            (Wanted::NotYet, _) | (Wanted::Off(_) | Wanted::Skipped(_), None) => None,
            (Wanted::Off(reason) | Wanted::Skipped(reason), Some(_)) => {
                Some(format!("waits for a restart to stop: {reason}"))
            }
            (Wanted::On { bind, in_place }, None) => {
                (!self.is_found_off(&bind, &in_place)).then(|| "waits for a restart to start".to_string())
            }
            (Wanted::On { bind, in_place }, Some(current)) => {
                if bind != current.bind {
                    Some(format!("waits for a restart to bind {}", self.describe(&bind)))
                } else if in_place != current.in_place {
                    Some("waits for a restart to apply its new settings".to_string())
                } else {
                    None
                }
            }
        };
        self.pending.clone_from(&pending);
        match pending {
            Some(pending) => self.outcome(Action::Held, pending),
            None => self.outcome(Action::Same, ""),
        }
    }

    fn begin_stop(&mut self, how: StopHow) {
        let Some(current) = &mut self.running else {
            return;
        };
        let begun = catch_unwind(AssertUnwindSafe(|| current.run.begin_stop(how)));
        if let Err(panic) = begun {
            tracing::error!(
                "The {} listener's stop panicked, it is dropped: {}",
                self.listener.name(),
                panic_text(panic.as_ref())
            );
        }
    }

    fn finish_stop(&mut self, ctx: &Ctx, pass: &Pass, deadline: Instant) {
        let Some(current) = self.running.take() else {
            return;
        };
        // dropped before the row says off: a wrapped listener of today stops in its drop
        let draining = current
            .run
            .end_now()
            .map(|end_now| ctx.draining.add(self.name(), end_now));
        Stopping {
            name: self.name(),
            run: Box::new(RunOf::<L>(current.run)),
            deadline,
            _draining: draining,
        }
        .finish();
        self.pending = None;
        self.record_stopped(ctx, pass, State::off(STOPPING));
    }

    fn take_running(&mut self) -> Option<Box<dyn Send>> {
        self.running
            .take()
            .map(|current| Box::new(current.run) as Box<dyn Send>)
    }

    fn address(&self) -> Option<SocketAddr> {
        self.running.as_ref().and_then(|current| current.run.address())
    }

    fn tell(&self, ctx: &Ctx) {
        let now = match (&self.running, &self.last_error, &self.off) {
            (Some(current), _, _) => Now::Running(&current.in_place),
            (None, Some(_), _) => Now::Failed,
            (None, None, Some(reason)) => Now::Off(reason),
            (None, None, None) => Now::NotYet,
        };
        if let Err(err) = self.guard("report", || self.listener.published(ctx, now)) {
            tracing::error!("The {} listener did not tell what it does: {err}", self.name());
        }
    }

    fn info(&self, ctx: &Ctx) -> ListenerInfo {
        let row = self.row(ctx);
        ListenerInfo {
            name: self.name().to_string(),
            state: row
                .as_ref()
                .map_or_else(|| State::off("not started yet"), |row| row.state.clone()),
            since: row.as_ref().map(|row| row.since),
            cause: row.and_then(|row| row.cause),
            address: self.address().map(|address| address.to_string()),
            live: self.listener.live(),
            pending: self.pending.clone(),
            last_error: self.last_error.clone(),
            connections: self.running.as_ref().and_then(|current| current.run.connections()),
            dependents: self.listener.dependents(ctx),
            selectors: self.listener.selectors().iter().map(ToString::to_string).collect(),
        }
    }
}
