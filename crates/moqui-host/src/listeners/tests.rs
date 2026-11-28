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

use crate::port_check::Proto;

/// What the fakes of one test did, in order: `start A 1`, `stop A 1 Grace(5s)`, `released A 1` (its port let go for
/// a new bind of the same one), `finish A 1 7s` (the wait it was given, to the nearest second), `now A 1` (its
/// connections ended now while it drained), `drop A 1`, `apply A 3`.
type Log = Arc<Mutex<Vec<String>>>;

/// What a fake reads, which a test changes between passes.
struct Script {
    wanted: Wanted<u16, u8>,
    /// Ports whose start fails, as a busy port's does.
    busy: Vec<u16>,
    /// The start finds it off and records why itself, as a wrapped start of today.
    found_off: bool,
    start_panics: bool,
    wanted_panics: bool,
    /// Its stop never reports finished.
    hangs: bool,
    /// Its stop drains for this long, unless its connections are ended now first.
    drain: Option<Duration>,
    /// Its next start queues a request through the factory's control, as a signal would from within a pass.
    enqueue: bool,
    /// A failed start's error carries the bind's own error as its source.
    with_source: bool,
    /// The cause of each pass that asked what it wants.
    seen: Vec<String>,
}

#[derive(Clone)]
struct Fake {
    name: &'static str,
    stage: Stage,
    phase: StopPhase,
    rank: u8,
    extras: &'static [&'static str],
    live: bool,
    fatal: bool,
    overlap: bool,
    script: Arc<Mutex<Script>>,
    log: Log,
}

impl Fake {
    /// A live listener of the Comms stage, off.
    fn new(name: &'static str, log: &Log) -> Self {
        Self {
            name,
            stage: Stage::Comms,
            phase: StopPhase::BeforeFactory,
            rank: 0,
            extras: &[],
            live: true,
            fatal: false,
            overlap: false,
            script: Arc::new(Mutex::new(Script {
                wanted: Wanted::Off("fake@enabled is false".to_string()),
                busy: Vec::new(),
                found_off: false,
                start_panics: false,
                wanted_panics: false,
                hangs: false,
                drain: None,
                enqueue: false,
                with_source: false,
                seen: Vec::new(),
            })),
            log: Arc::clone(log),
        }
    }

    fn in_stage(self, stage: Stage, phase: StopPhase) -> Self {
        Self { stage, phase, ..self }
    }

    fn ranked(self, rank: u8) -> Self {
        Self { rank, ..self }
    }

    fn want(&self, wanted: Wanted<u16, u8>) {
        lock(&self.script).wanted = wanted;
    }

    fn on(&self, port: u16, in_place: u8) {
        self.want(Wanted::On { bind: port, in_place });
    }

    fn off(&self, reason: &str) {
        self.want(Wanted::Off(reason.to_string()));
    }

    fn busy(&self, ports: &[u16]) {
        lock(&self.script).busy = ports.to_vec();
    }

    fn script(&self) -> std::sync::MutexGuard<'_, Script> {
        lock(&self.script)
    }

    fn seen(&self) -> Vec<String> {
        lock(&self.script).seen.clone()
    }
}

struct FakeRun {
    name: &'static str,
    port: u16,
    script: Arc<Mutex<Script>>,
    log: Log,
    /// Its connections were ended now.
    ended: Arc<AtomicBool>,
}

impl<P: Debug> Running<P> for FakeRun {
    fn state(&self) -> State {
        State::On(format!("tcp 127.0.0.1:{}", self.port))
    }

    fn address(&self) -> Option<SocketAddr> {
        Some(SocketAddr::from(([127, 0, 0, 1], self.port)))
    }

    fn apply(&mut self, in_place: &P) {
        lock(&self.log).push(format!("apply {} {in_place:?}", self.name));
    }

    fn begin_stop(&mut self, how: StopHow) {
        lock(&self.log).push(format!("stop {} {} {how:?}", self.name, self.port));
    }

    fn end_now(&self) -> Option<EndNow> {
        let (name, port, log, ended) = (self.name, self.port, Arc::clone(&self.log), Arc::clone(&self.ended));
        Some(Arc::new(move || {
            lock(&log).push(format!("now {name} {port}"));
            ended.store(true, Ordering::SeqCst);
        }))
    }

    fn released(&mut self, _wait: Duration) -> bool {
        lock(&self.log).push(format!("released {} {}", self.name, self.port));
        true
    }

    fn finished(&mut self, wait: Duration) -> bool {
        let seconds = (wait.as_millis() + 500) / 1000;
        lock(&self.log).push(format!("finish {} {} {seconds}s", self.name, self.port));
        let drain = lock(&self.script).drain;
        if let Some(drain) = drain {
            let started = Instant::now();
            while !self.ended.load(Ordering::SeqCst) && started.elapsed() < drain.min(wait) {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        !lock(&self.script).hangs
    }

    fn connections(&self) -> Option<usize> {
        Some(2)
    }
}

impl Drop for FakeRun {
    fn drop(&mut self) {
        lock(&self.log).push(format!("drop {} {}", self.name, self.port));
    }
}

impl Listener for Fake {
    type Bind = u16;
    type InPlace = u8;
    type Run = FakeRun;

    fn name(&self) -> &'static str {
        self.name
    }

    fn stage(&self) -> Stage {
        self.stage
    }

    fn stop_phase(&self) -> StopPhase {
        self.phase
    }

    fn stop_rank(&self) -> u8 {
        self.rank
    }

    fn extra_rows(&self) -> &'static [&'static str] {
        self.extras
    }

    fn live(&self) -> bool {
        self.live
    }

    fn fatal_at_boot(&self) -> bool {
        self.fatal
    }

    fn selectors(&self) -> &'static [&'static str] {
        &["fake@enabled", "fake@port"]
    }

    fn wanted(&self, ctx: &Ctx) -> Wanted<u16, u8> {
        let mut script = lock(&self.script);
        script.seen.push(ctx.cause.to_string());
        if script.wanted_panics {
            drop(script);
            panic!("fake settings panicked");
        }
        script.wanted.clone()
    }

    fn start(&self, ctx: &Ctx, bind: &u16, _in_place: &u8) -> Result<Option<FakeRun>, Error> {
        let (busy, found_off, panics, enqueue, with_source) = {
            let mut script = lock(&self.script);
            let enqueue = std::mem::take(&mut script.enqueue);
            (
                script.busy.contains(bind),
                script.found_off,
                script.start_panics,
                enqueue,
                script.with_source,
            )
        };
        lock(&self.log).push(format!("start {} {bind}", self.name));
        if enqueue {
            let control = moqui_core::listeners::control(ctx.factory).expect("the test installed the control");
            control.reconcile_later(ReconcileRequest::new(Cause::Signal("from a pass")), Duration::ZERO);
        }
        if panics {
            panic!("fake start panicked");
        }
        if busy {
            let err = Error::new(format!("{bind} (fake@port) on 127.0.0.1: address in use"));
            return Err(if with_source {
                err.with_source(std::io::Error::new(std::io::ErrorKind::AddrInUse, "in use"))
            } else {
                err
            });
        }
        if found_off {
            moqui_core::startup::listener(ctx.factory, self.name, State::off("fake@port is not set"));
            return Ok(None);
        }
        // as call media records TURN: the rows it owns beside its own
        for extra in self.extras {
            moqui_core::startup::listener(ctx.factory, *extra, State::On(format!("udp 127.0.0.1:{bind}")));
        }
        Ok(Some(FakeRun {
            name: self.name,
            port: *bind,
            script: Arc::clone(&self.script),
            log: Arc::clone(&self.log),
            ended: Arc::default(),
        }))
    }

    fn overlaps(&self, _old: &u16, _new: &u16) -> bool {
        self.overlap
    }

    fn dependents(&self, _ctx: &Ctx) -> Vec<String> {
        vec![format!("{} tunnel", self.name)]
    }

    fn target(&self, bind: &u16) -> Option<Target> {
        Some(Target::optional(self.name, "fake@port", "127.0.0.1", *bind, Proto::Tcp))
    }
}

fn log() -> Log {
    Arc::default()
}

/// The events logged since the last call.
fn taken(log: &Log) -> Vec<String> {
    std::mem::take(&mut *lock(log))
}

fn boot_inputs() -> BootInputs {
    BootInputs {
        bind: ListenBind::None,
        runtime_dir: PathBuf::from("runtime-of-the-test"),
        cluster_posts: Arc::default(),
    }
}

fn registry(fakes: &[&Fake]) -> Registry {
    let context = ExecutionContextFactory::test_context();
    let registry = Registry::new(Arc::clone(context.factory()), boot_inputs());
    for fake in fakes {
        registry.add((*fake).clone());
    }
    registry
}

fn row(registry: &Registry, name: &str) -> moqui_core::startup::Entry {
    moqui_core::startup::report(&registry.0.factory)
        .entries()
        .into_iter()
        .find(|row| row.section == Section::Listener && row.name == name)
        .expect("the listener's row")
}

fn info(registry: &Registry, name: &str) -> ListenerInfo {
    registry
        .snapshot()
        .into_iter()
        .find(|info| info.name == name)
        .expect("the listener in the snapshot")
}

fn actions(outcomes: &[Outcome]) -> Vec<(&str, Action)> {
    outcomes
        .iter()
        .map(|outcome| (outcome.name.as_str(), outcome.action))
        .collect()
}

fn boot(registry: &Registry) -> Vec<Outcome> {
    registry.run(Scope::All, Cause::Boot).expect("the boot pass")
}

fn reload(registry: &Registry) -> Vec<Outcome> {
    registry.run(Scope::All, Cause::ConfReload).expect("a reload pass")
}

/// A pass of the worker, as a reload's or a service's is, and its outcomes.
fn requested(registry: &Registry, request: ReconcileRequest) -> Vec<Outcome> {
    registry
        .request(request, Duration::ZERO)
        .recv_timeout(Duration::from_secs(5))
        .expect("the worker answers")
        .expect("the pass")
}

fn address(port: u16) -> Option<SocketAddr> {
    Some(SocketAddr::from(([127, 0, 0, 1], port)))
}

/// Wait up to 5 s for `done`, without a fixed sleep.
fn eventually(what: &str, done: impl Fn() -> bool) {
    let asked = Instant::now();
    while !done() {
        assert!(asked.elapsed() < Duration::from_secs(5), "{what} did not happen");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn on_while_not_running_starts_and_records_the_row() {
    let log = log();
    let fake = Fake::new("A", &log);
    let registry = registry(&[&fake]);
    let before = info(&registry, "A");
    assert_eq!(before.state, State::off("not started yet"));
    assert_eq!(before.since, None);
    fake.on(1, 0);
    let outcomes = boot(&registry);
    assert_eq!(actions(&outcomes), [("A", Action::Started)]);
    assert_eq!(outcomes[0].detail, "tcp 127.0.0.1:1");
    assert_eq!(taken(&log), ["start A 1"]);
    let row = row(&registry, "A");
    assert_eq!(row.state, State::On("tcp 127.0.0.1:1".to_string()));
    assert_eq!(row.cause.as_deref(), Some("boot"));
    assert_eq!(registry.address("A"), address(1));
    let info = info(&registry, "A");
    assert_eq!(info.state, row.state);
    assert_eq!(info.since, Some(row.since));
    assert_eq!(info.cause.as_deref(), Some("boot"));
    assert_eq!(info.address.as_deref(), Some("127.0.0.1:1"));
    assert!(info.live);
    assert_eq!(info.connections, Some(2));
    assert_eq!(info.dependents, ["A tunnel"]);
    assert_eq!(info.selectors, ["fake@enabled", "fake@port"]);
}

#[test]
fn the_same_settings_do_nothing() {
    let log = log();
    let fake = Fake::new("A", &log);
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    boot(&registry);
    let started = row(&registry, "A");
    taken(&log);
    assert_eq!(actions(&reload(&registry)), [("A", Action::Same)]);
    assert!(taken(&log).is_empty());
    // the row keeps the time and cause of the start
    assert_eq!(row(&registry, "A"), started);
    // off and still off: the row is not recorded again either
    fake.off("fake@enabled is false");
    reload(&registry);
    let stopped = row(&registry, "A");
    assert_eq!(actions(&reload(&registry)), [("A", Action::Same)]);
    assert_eq!(row(&registry, "A"), stopped);
}

/// The row a running listener records itself (HTTPS standing by records its own) is left as it is by a pass that
/// changes nothing: the registry records again only when its own last record is stale.
#[test]
fn a_row_the_listener_records_itself_is_left_as_it_is() {
    let log = log();
    let fake = Fake::new("A", &log);
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    boot(&registry);
    moqui_core::startup::listener(&registry.0.factory, "A", State::off("standing by: no certificate"));
    let own = row(&registry, "A");
    assert_eq!(actions(&reload(&registry)), [("A", Action::Same)]);
    assert_eq!(row(&registry, "A"), own);
}

#[test]
fn off_while_running_stops_with_the_grace() {
    let log = log();
    let fake = Fake::new("A", &log);
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    boot(&registry);
    taken(&log);
    fake.off("fake@enabled is false");
    let outcomes = registry
        .run(Scope::All, Cause::SetValue("fake@enabled".to_string()))
        .unwrap();
    assert_eq!(actions(&outcomes), [("A", Action::Stopped)]);
    assert_eq!(outcomes[0].detail, "fake@enabled is false");
    // the grace and its margin
    assert_eq!(taken(&log), ["stop A 1 Grace(5s)", "finish A 1 7s", "drop A 1"]);
    let row = row(&registry, "A");
    assert_eq!(row.state, State::off("fake@enabled is false"));
    assert_eq!(row.cause.as_deref(), Some("set fake@enabled"));
    assert_eq!(registry.address("A"), None);
    assert_eq!(info(&registry, "A").connections, None);
    // skipped stops it the same way, recorded as skipped
    fake.on(1, 0);
    reload(&registry);
    fake.want(Wanted::Skipped("port 1 was busy at the start check".to_string()));
    assert_eq!(actions(&reload(&registry)), [("A", Action::Stopped)]);
    assert_eq!(
        self::row(&registry, "A").state,
        State::Skipped("port 1 was busy at the start check".to_string())
    );
}

/// Each stop of a pass begins before any is waited on, so their graces run together; one turned off with "End
/// connections now" gets the margin alone, and one that does not finish is dropped all the same.
#[test]
fn the_stops_of_a_pass_wait_together() {
    let log = log();
    let (a, b, c) = (Fake::new("A", &log), Fake::new("B", &log), Fake::new("C", &log));
    a.on(1, 0);
    b.on(2, 0);
    c.on(3, 0);
    let registry = registry(&[&a, &b, &c]);
    boot(&registry);
    taken(&log);
    for fake in [&a, &b, &c] {
        fake.off("fake@enabled is false");
    }
    b.script().hangs = true;
    let outcomes = requested(
        &registry,
        ReconcileRequest {
            cause: Cause::ConfReload,
            end_now: vec!["C".to_string()],
        },
    );
    assert_eq!(
        actions(&outcomes),
        [("A", Action::Stopped), ("B", Action::Stopped), ("C", Action::Stopped)]
    );
    assert_eq!(
        taken(&log),
        [
            "stop A 1 Grace(5s)",
            "stop B 2 Grace(5s)",
            "stop C 3 Now",
            "finish A 1 7s",
            "drop A 1",
            "finish B 2 7s",
            "drop B 2",
            "finish C 3 2s",
            "drop C 3",
        ]
    );
}

/// A call media listener owns the TURN row its start records: a change sets its cause, and a stop records it off.
#[test]
fn the_rows_a_listener_owns_follow_it() {
    let log = log();
    let media = Fake {
        extras: &["TURN"],
        ..Fake::new("Call media", &log)
    };
    media.on(10000, 0);
    let registry = registry(&[&media]);
    boot(&registry);
    let turn = row(&registry, "TURN");
    assert_eq!(turn.state, State::On("udp 127.0.0.1:10000".to_string()));
    assert_eq!(turn.cause.as_deref(), Some("boot"));
    media.off("media-server@enabled is false");
    reload(&registry);
    let turn = row(&registry, "TURN");
    assert_eq!(turn.state, State::off("media-server@enabled is false"));
    assert_eq!(turn.cause.as_deref(), Some("conf reload"));
}

#[test]
fn a_new_bind_starts_before_the_old_stops() {
    let log = log();
    let fake = Fake::new("A", &log);
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    boot(&registry);
    taken(&log);
    fake.on(2, 0);
    assert_eq!(actions(&reload(&registry)), [("A", Action::Rebound)]);
    assert_eq!(
        taken(&log),
        ["start A 2", "stop A 1 Grace(5s)", "finish A 1 7s", "drop A 1"]
    );
    assert_eq!(registry.address("A"), address(2));
    assert_eq!(row(&registry, "A").state, State::On("tcp 127.0.0.1:2".to_string()));
}

#[test]
fn an_overlapping_bind_stops_the_old_first() {
    let log = log();
    let fake = Fake {
        overlap: true,
        ..Fake::new("A", &log)
    };
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    boot(&registry);
    taken(&log);
    fake.on(2, 0);
    assert_eq!(actions(&reload(&registry)), [("A", Action::Rebound)]);
    // the old port is let go before the new bind, and the old one's connections drain with the pass's stops
    assert_eq!(
        taken(&log),
        [
            "stop A 1 Grace(5s)",
            "released A 1",
            "start A 2",
            "finish A 1 7s",
            "drop A 1"
        ]
    );
    assert_eq!(registry.address("A"), address(2));
}

#[test]
fn a_failed_new_bind_keeps_the_old_running() {
    let log = log();
    let fake = Fake::new("A", &log);
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    boot(&registry);
    taken(&log);
    fake.on(2, 0);
    fake.busy(&[2]);
    let outcomes = reload(&registry);
    assert_eq!(actions(&outcomes), [("A", Action::Failed)]);
    assert_eq!(
        outcomes[0].detail,
        "kept on 127.0.0.1:1: 2 (fake@port) on 127.0.0.1: address in use"
    );
    // the old one was never stopped
    assert_eq!(taken(&log), ["start A 2"]);
    assert_eq!(registry.address("A"), address(1));
    assert_eq!(
        row(&registry, "A").state,
        State::On(
            "tcp 127.0.0.1:1; the new settings did not bind: 2 (fake@port) on 127.0.0.1: address in use".to_string()
        )
    );
    assert_eq!(
        info(&registry, "A").last_error.as_deref(),
        Some("2 (fake@port) on 127.0.0.1: address in use")
    );
    // the conf back at what runs: the failure is over and the row says what runs
    fake.on(1, 0);
    assert_eq!(actions(&reload(&registry)), [("A", Action::Same)]);
    assert_eq!(info(&registry, "A").last_error, None);
    assert_eq!(row(&registry, "A").state, State::On("tcp 127.0.0.1:1".to_string()));
    // and a later bind that works rebinds
    fake.on(2, 0);
    fake.busy(&[]);
    assert_eq!(actions(&reload(&registry)), [("A", Action::Rebound)]);
}

#[test]
fn a_failed_overlapping_bind_binds_the_old_again() {
    let log = log();
    let fake = Fake {
        overlap: true,
        ..Fake::new("A", &log)
    };
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    boot(&registry);
    taken(&log);
    fake.on(2, 0);
    fake.busy(&[2]);
    let outcomes = reload(&registry);
    assert_eq!(actions(&outcomes), [("A", Action::Failed)]);
    assert!(
        outcomes[0].detail.starts_with("kept on 127.0.0.1:1: 2 (fake@port)"),
        "{outcomes:?}"
    );
    assert_eq!(
        taken(&log),
        [
            "stop A 1 Grace(5s)",
            "released A 1",
            "start A 2",
            "start A 1",
            "finish A 1 7s",
            "drop A 1"
        ]
    );
    assert_eq!(registry.address("A"), address(1));
    // when the old settings do not bind again either, nothing runs and the row says why
    fake.on(3, 0);
    fake.busy(&[1, 3]);
    let outcomes = reload(&registry);
    assert_eq!(actions(&outcomes), [("A", Action::Failed)]);
    assert!(
        outcomes[0]
            .detail
            .contains("the old settings did not bind again: 1 (fake@port)"),
        "{outcomes:?}"
    );
    assert_eq!(registry.address("A"), None);
    assert!(matches!(row(&registry, "A").state, State::Failed(_)));
}

/// A change of the bind and of the in-place settings together whose bind fails still applies the in-place settings to
/// what keeps running, on either path: applied to the old run when the new address bound beside it, and the old bind
/// started again with them when the two overlap. So a stricter in-place setting (SSH's access mode) does not wait for
/// a port that stays busy: the retried pass finds them applied.
#[test]
fn a_failed_new_bind_still_applies_the_in_place_settings() {
    for overlap in [false, true] {
        let log = log();
        let fake = Fake {
            overlap,
            ..Fake::new("A", &log)
        };
        fake.on(1, 0);
        let registry = registry(&[&fake]);
        boot(&registry);
        taken(&log);
        fake.on(2, 3);
        fake.busy(&[2]);
        let outcomes = reload(&registry);
        assert_eq!(actions(&outcomes), [("A", Action::Failed)], "overlap {overlap}");
        assert!(outcomes[0].detail.starts_with("kept on 127.0.0.1:1"), "{outcomes:?}");
        let expected: &[&str] = if overlap {
            &[
                "stop A 1 Grace(5s)",
                "released A 1",
                "start A 2",
                "start A 1",
                "finish A 1 7s",
                "drop A 1",
            ]
        } else {
            &["start A 2", "apply A 3"]
        };
        assert_eq!(taken(&log), expected, "overlap {overlap}");
        assert_eq!(registry.address("A"), address(1));
        // the retried pass: the in-place settings are those that run, so nothing is applied again
        let outcomes = reload(&registry);
        assert_eq!(actions(&outcomes), [("A", Action::Failed)], "overlap {overlap}");
        assert!(
            !taken(&log).iter().any(|event| event.starts_with("apply")),
            "overlap {overlap}"
        );
        // the bind back at what runs, the settings in place as the change set them: nothing to apply
        fake.on(1, 3);
        assert_eq!(actions(&reload(&registry)), [("A", Action::Same)], "overlap {overlap}");
        assert!(taken(&log).is_empty(), "overlap {overlap}");
    }
}

/// "End connections now" asked while a listener drains its grace after a pass turned it off ends them at once, not
/// after the pass, whose lock it does not wait for (plans/Runtime-Config.md §3.1).
#[test]
fn ending_now_cuts_short_a_stop_already_draining() {
    let log = log();
    let fake = Fake::new("A", &log);
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    boot(&registry);
    taken(&log);
    fake.script().drain = Some(Duration::from_secs(5));
    fake.off("fake@enabled is false");
    let started = Instant::now();
    let draining = {
        let registry = registry.clone();
        std::thread::spawn(move || reload(&registry))
    };
    while !lock(&log).iter().any(|event| event.starts_with("finish A 1")) {
        assert!(started.elapsed() < Duration::from_secs(3), "the stop began to drain");
        std::thread::sleep(Duration::from_millis(5));
    }
    let ended = requested(
        &registry,
        ReconcileRequest {
            cause: Cause::Request {
                user: "operator".to_string(),
                reason: "a download that does not finish".to_string(),
            },
            end_now: vec!["A".to_string()],
        },
    );
    assert_eq!(actions(&draining.join().unwrap()), [("A", Action::Stopped)]);
    assert_eq!(actions(&ended), [("A", Action::Same)]);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert_eq!(
        taken(&log),
        ["stop A 1 Grace(5s)", "finish A 1 7s", "now A 1", "drop A 1"]
    );
}

/// A request to end a listener's connections now that came before a pass running already began its stop ends it as
/// the stop begins; once a pass has taken the request's job, the pass ends it itself and nothing is remembered.
#[test]
fn ending_now_asked_before_a_stop_begins_ends_it_as_it_begins() {
    let draining = Arc::new(Draining::default());
    let ended = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handle = |ended: &Arc<std::sync::atomic::AtomicUsize>| -> EndNow {
        let ended = Arc::clone(ended);
        Arc::new(move || {
            ended.fetch_add(1, Ordering::SeqCst);
        })
    };
    assert_eq!(draining.end_now(&["A".to_string()]), 0, "nothing drains yet");
    let a = draining.add("A", handle(&ended));
    assert_eq!(ended.load(Ordering::SeqCst), 1, "ended as its stop began");
    let b = draining.add("B", handle(&ended));
    assert_eq!(ended.load(Ordering::SeqCst), 1, "another listener is not");
    draining.taken(&["A".to_string()]);
    drop((a, b));
    let again = draining.add("A", handle(&ended));
    assert_eq!(ended.load(Ordering::SeqCst), 1, "a job taken leaves nothing remembered");
    assert_eq!(draining.end_now(&["A".to_string()]), 1);
    assert_eq!(ended.load(Ordering::SeqCst), 2);
    drop(again);
}

#[test]
fn in_place_settings_apply_without_a_new_bind() {
    let log = log();
    let fake = Fake::new("A", &log);
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    boot(&registry);
    taken(&log);
    fake.on(1, 3);
    assert_eq!(actions(&reload(&registry)), [("A", Action::Applied)]);
    assert_eq!(taken(&log), ["apply A 3"]);
    assert_eq!(registry.address("A"), address(1));
    assert_eq!(row(&registry, "A").cause.as_deref(), Some("conf reload"));
    assert_eq!(actions(&reload(&registry)), [("A", Action::Same)]);
}

#[test]
fn a_failed_start_leaves_the_others_to_go_on() {
    let log = log();
    let (a, b) = (Fake::new("A", &log), Fake::new("B", &log));
    a.on(1, 0);
    a.busy(&[1]);
    b.on(2, 0);
    let registry = registry(&[&a, &b]);
    let outcomes = boot(&registry);
    assert_eq!(actions(&outcomes), [("A", Action::Failed), ("B", Action::Started)]);
    assert_eq!(taken(&log), ["start A 1", "start B 2"]);
    assert_eq!(
        row(&registry, "A").state,
        State::failed("1 (fake@port) on 127.0.0.1: address in use")
    );
    assert_eq!(registry.address("B"), address(2));
}

/// A live listener whose port is held is tried again on each pass; its row keeps the time of the first failure
/// until the error changes.
#[test]
fn a_failing_listener_is_retried_without_moving_its_row() {
    let log = log();
    let fake = Fake::new("A", &log);
    fake.on(1, 0);
    fake.busy(&[1, 2]);
    let registry = registry(&[&fake]);
    boot(&registry);
    let failed = row(&registry, "A");
    assert_eq!(actions(&reload(&registry)), [("A", Action::Failed)]);
    assert_eq!(taken(&log), ["start A 1", "start A 1"]);
    assert_eq!(row(&registry, "A"), failed);
    fake.on(2, 0);
    reload(&registry);
    let moved = row(&registry, "A");
    assert_eq!(moved.state, State::failed("2 (fake@port) on 127.0.0.1: address in use"));
    assert_eq!(moved.cause.as_deref(), Some("conf reload"));
}

#[test]
fn a_panicking_start_fails_that_listener_alone() {
    let log = log();
    let (a, b) = (Fake::new("A", &log), Fake::new("B", &log));
    a.on(1, 0);
    a.script().start_panics = true;
    b.on(2, 0);
    let registry = registry(&[&a, &b]);
    let outcomes = boot(&registry);
    assert_eq!(actions(&outcomes), [("A", Action::Failed), ("B", Action::Started)]);
    assert_eq!(outcomes[0].detail, "its start panicked: fake start panicked");
    assert_eq!(
        row(&registry, "A").state,
        State::failed("its start panicked: fake start panicked")
    );
    assert_eq!(registry.address("B"), address(2));
}

/// Reading what the conf wants panics: the outcome says so, and a running listener keeps running with its row as it
/// was, whether it is live or waits for a restart.
#[test]
fn a_panic_reading_the_settings_does_not_fail_what_runs() {
    let log = log();
    let live = Fake::new("A", &log);
    let held = Fake {
        live: false,
        ..Fake::new("B", &log)
    };
    live.on(1, 0);
    held.on(2, 0);
    let registry = registry(&[&live, &held]);
    boot(&registry);
    let rows = (row(&registry, "A"), row(&registry, "B"));
    live.script().wanted_panics = true;
    held.script().wanted_panics = true;
    let outcomes = reload(&registry);
    assert_eq!(actions(&outcomes), [("A", Action::Failed), ("B", Action::Failed)]);
    assert_eq!(outcomes[1].detail, "its settings panicked: fake settings panicked");
    assert_eq!((row(&registry, "A"), row(&registry, "B")), rows);
    assert_eq!(registry.address("A"), address(1));
    assert_eq!(registry.address("B"), address(2));
    assert_eq!(
        info(&registry, "A").last_error.as_deref(),
        Some("its settings panicked: fake settings panicked")
    );
}

#[test]
fn entries_act_in_their_fixed_order_and_a_stage_pass_only_on_its_own() {
    let log = log();
    let ssh = Fake::new("SSH", &log).in_stage(Stage::BeforeCluster, StopPhase::AfterFactory);
    let cluster = Fake::new("Cluster", &log).in_stage(Stage::ClusterPort, StopPhase::AfterFactory);
    let sip = Fake::new("SIP", &log);
    let smtp = Fake::new("SMTP", &log);
    let http = Fake::new("HTTP", &log).in_stage(Stage::Web, StopPhase::Web);
    for (port, fake) in [&ssh, &cluster, &sip, &smtp, &http].into_iter().enumerate() {
        fake.on(u16::try_from(port).unwrap() + 1, 0);
    }
    let registry = registry(&[&ssh, &cluster, &sip, &smtp, &http]);
    let comms = registry.run(Scope::Stage(Stage::Comms), Cause::Boot).unwrap();
    assert_eq!(actions(&comms), [("SIP", Action::Started), ("SMTP", Action::Started)]);
    assert_eq!(taken(&log), ["start SIP 3", "start SMTP 4"]);
    let all = boot(&registry);
    assert_eq!(
        actions(&all),
        [
            ("SSH", Action::Started),
            ("Cluster", Action::Started),
            ("SIP", Action::Same),
            ("SMTP", Action::Same),
            ("HTTP", Action::Started),
        ]
    );
    assert_eq!(taken(&log), ["start SSH 1", "start Cluster 2", "start HTTP 5"]);
    let names: Vec<String> = registry.snapshot().into_iter().map(|info| info.name).collect();
    assert_eq!(names, ["SSH", "Cluster", "SIP", "SMTP", "HTTP"]);
}

/// A request during the boot runs a pass between the staged ones: an entry whose stage has not run is left alone,
/// with nothing started and nothing pending, until its own boot pass.
#[test]
fn a_pass_before_a_stage_has_booted_leaves_its_entries_alone() {
    let log = log();
    let ssh = Fake::new("SSH", &log).in_stage(Stage::BeforeCluster, StopPhase::AfterFactory);
    let smtp = Fake::new("SMTP", &log);
    let xmpp = Fake {
        live: false,
        ..Fake::new("XMPP", &log)
    };
    ssh.on(1, 0);
    smtp.on(2, 0);
    xmpp.on(3, 0);
    let registry = registry(&[&ssh, &smtp, &xmpp]);
    registry.run(Scope::Stage(Stage::BeforeCluster), Cause::Boot).unwrap();
    let outcomes = requested(&registry, ReconcileRequest::new(Cause::ConfReload));
    assert_eq!(
        actions(&outcomes),
        [("SSH", Action::Same), ("SMTP", Action::Same), ("XMPP", Action::Same)]
    );
    assert_eq!(outcomes[1].detail, "waits for its boot stage");
    assert_eq!(info(&registry, "XMPP").pending, None);
    assert_eq!(taken(&log), ["start SSH 1"]);
    let comms = registry.run(Scope::Stage(Stage::Comms), Cause::Boot).unwrap();
    assert_eq!(actions(&comms), [("SMTP", Action::Started), ("XMPP", Action::Started)]);
}

/// The staged boot passes are the server's own: a boot or shutdown cause asked of the worker is refused, and a
/// worker pass never acts as a boot pass would on a listener that is not live.
#[test]
fn a_boot_or_shutdown_pass_cannot_be_asked_for() {
    let log = log();
    let held = Fake {
        live: false,
        ..Fake::new("A", &log)
    };
    let registry = registry(&[&held]);
    boot(&registry);
    held.on(1, 0);
    for cause in [Cause::Boot, Cause::Shutdown] {
        let text = cause.to_string();
        let answer = registry
            .request(ReconcileRequest::new(cause), Duration::ZERO)
            .recv_timeout(Duration::from_secs(5))
            .expect("answered at once");
        assert_eq!(
            answer,
            Err(format!("a {text} pass is the server's own, not one to ask for"))
        );
    }
    assert!(taken(&log).is_empty());
    let outcomes = requested(&registry, ReconcileRequest::new(Cause::ConfReload));
    assert_eq!(actions(&outcomes), [("A", Action::Held)]);
}

#[test]
fn a_listener_not_ready_yet_is_left_alone() {
    let log = log();
    let http = Fake::new("HTTP", &log).in_stage(Stage::Web, StopPhase::Web);
    let held = Fake {
        live: false,
        ..Fake::new("HTTPS", &log).in_stage(Stage::Web, StopPhase::Web)
    };
    http.want(Wanted::NotYet);
    held.want(Wanted::NotYet);
    let registry = registry(&[&http, &held]);
    let outcomes = boot(&registry);
    assert_eq!(actions(&outcomes), [("HTTP", Action::Same), ("HTTPS", Action::Same)]);
    assert_eq!(outcomes[0].detail, "not ready to start yet");
    assert_eq!(
        actions(&reload(&registry)),
        [("HTTP", Action::Same), ("HTTPS", Action::Same)]
    );
    assert!(taken(&log).is_empty());
    assert_eq!(info(&registry, "HTTP").state, State::off("not started yet"));
    assert_eq!(info(&registry, "HTTPS").pending, None);
}

#[test]
fn a_listener_that_is_not_live_waits_for_a_restart() {
    let log = log();
    let fake = Fake {
        live: false,
        ..Fake::new("A", &log)
    };
    let never = Fake {
        live: false,
        ..Fake::new("B", &log)
    };
    fake.on(1, 0);
    let registry = registry(&[&fake, &never]);
    // a boot pass acts on every listener, live or not
    assert_eq!(actions(&boot(&registry)), [("A", Action::Started), ("B", Action::Same)]);
    taken(&log);
    let started = row(&registry, "A");
    let held = |registry: &Registry| -> Vec<(String, Action, String)> {
        reload(registry)
            .into_iter()
            .map(|outcome| (outcome.name, outcome.action, outcome.detail))
            .collect()
    };
    fake.off("fake@enabled is false");
    never.on(9, 0);
    assert_eq!(
        held(&registry),
        [
            (
                "A".to_string(),
                Action::Held,
                "waits for a restart to stop: fake@enabled is false".to_string()
            ),
            (
                "B".to_string(),
                Action::Held,
                "waits for a restart to start".to_string()
            ),
        ]
    );
    assert_eq!(
        info(&registry, "A").pending.as_deref(),
        Some("waits for a restart to stop: fake@enabled is false")
    );
    fake.on(2, 0);
    assert_eq!(held(&registry)[0].2, "waits for a restart to bind 127.0.0.1:2");
    fake.on(1, 4);
    assert_eq!(held(&registry)[0].2, "waits for a restart to apply its new settings");
    fake.on(1, 0);
    assert_eq!(held(&registry)[0].1, Action::Same);
    assert_eq!(info(&registry, "A").pending, None);
    // nothing started or stopped, and the row is the start's
    assert!(taken(&log).is_empty());
    assert_eq!(registry.address("A"), address(1));
    assert_eq!(row(&registry, "A"), started);
    assert!(!info(&registry, "A").live);
}

#[test]
fn a_wrapped_start_that_finds_it_off_records_only_the_cause() {
    let log = log();
    let held = Fake {
        live: false,
        ..Fake::new("A", &log)
    };
    let live = Fake::new("B", &log);
    for fake in [&held, &live] {
        fake.on(1, 0);
        fake.script().found_off = true;
    }
    let registry = registry(&[&held, &live]);
    let outcomes = boot(&registry);
    assert_eq!(actions(&outcomes), [("A", Action::Same), ("B", Action::Same)]);
    assert_eq!(outcomes[0].detail, "fake@port is not set");
    let row = row(&registry, "A");
    assert_eq!(row.state, State::off("fake@port is not set"));
    assert_eq!(row.cause.as_deref(), Some("boot"));
    let live_row = self::row(&registry, "B");
    assert_eq!(taken(&log), ["start A 1", "start B 1"]);
    // the conf still wants what the start found off: nothing waits for a restart, and the live one is not asked again
    assert_eq!(actions(&reload(&registry)), [("A", Action::Same), ("B", Action::Same)]);
    assert_eq!(info(&registry, "A").pending, None);
    assert!(taken(&log).is_empty());
    assert_eq!(self::row(&registry, "B"), live_row);
}

/// A boot pass stops at the first fatal failure of its stage, as its `?` did before the registry: no later entry
/// starts, what the pass started stops again (HTTP when HTTPS then failed), and the failed start's own error, its
/// source kept, is the boot's.
#[test]
fn a_fatal_listener_failing_at_boot_stops_the_boot_pass_and_what_it_started() {
    let log = log();
    let http = Fake::new("HTTP", &log).in_stage(Stage::Web, StopPhase::Web);
    let https = Fake {
        fatal: true,
        ..Fake::new("HTTPS", &log).in_stage(Stage::Web, StopPhase::Web)
    };
    let grpc = Fake::new("gRPC", &log).in_stage(Stage::Web, StopPhase::Web);
    http.on(1, 0);
    https.on(2, 0);
    https.busy(&[2]);
    https.script().with_source = true;
    grpc.on(3, 0);
    let registry = registry(&[&http, &https, &grpc]);
    let err = registry.run(Scope::Stage(Stage::Web), Cause::Boot).unwrap_err();
    assert_eq!(err.message(), "2 (fake@port) on 127.0.0.1: address in use");
    assert_eq!(
        std::error::Error::source(&err).map(ToString::to_string).as_deref(),
        Some("in use")
    );
    assert_eq!(
        taken(&log),
        [
            "start HTTP 1",
            "start HTTPS 2",
            "stop HTTP 1 Process",
            "finish HTTP 1 5s",
            "drop HTTP 1"
        ]
    );
    assert!(registry.snapshot().iter().all(|info| info.address.is_none()));
    assert!(matches!(row(&registry, "HTTPS").state, State::Failed(_)));
    let stopped = row(&registry, "HTTP");
    assert_eq!(stopped.state, State::off("the server is stopping"));
    assert_eq!(stopped.cause.as_deref(), Some("boot"));
    // after the boot the same failure is an outcome, not an error
    let outcomes = reload(&registry);
    assert_eq!(
        outcomes[1],
        Outcome {
            name: "HTTPS".to_string(),
            action: Action::Failed,
            detail: "2 (fake@port) on 127.0.0.1: address in use: in use".to_string(),
        }
    );
}

/// One that is not fatal fails alone in a boot pass: the others start, and the pass does not fail.
#[test]
fn a_listener_that_is_not_fatal_fails_alone_at_boot() {
    let log = log();
    let (sip, smtp) = (Fake::new("SIP", &log), Fake::new("SMTP", &log));
    sip.on(1, 0);
    sip.busy(&[1]);
    smtp.on(2, 0);
    let registry = registry(&[&sip, &smtp]);
    let outcomes = registry.run(Scope::Stage(Stage::Comms), Cause::Boot).unwrap();
    assert_eq!(actions(&outcomes), [("SIP", Action::Failed), ("SMTP", Action::Started)]);
    assert_eq!(registry.address("SMTP"), address(2));
}

/// Today's order (ServeProcess's drop): HTTP and HTTPS together; XMPP, SIP, then call media, each finished before the
/// next begins; SMTP, SSH, then the cluster port. One wait for the whole phase.
#[test]
fn stop_phases_stop_their_listeners_in_their_turns() {
    let log = log();
    let ssh = Fake::new("SSH", &log)
        .in_stage(Stage::BeforeCluster, StopPhase::AfterFactory)
        .ranked(1);
    let cluster = Fake::new("Cluster", &log)
        .in_stage(Stage::ClusterPort, StopPhase::AfterFactory)
        .ranked(2);
    let media = Fake {
        extras: &["TURN"],
        ..Fake::new("Call media", &log).ranked(2)
    };
    let sip = Fake::new("SIP", &log).ranked(1);
    let xmpp = Fake::new("XMPP", &log);
    let smtp = Fake::new("SMTP", &log).in_stage(Stage::Comms, StopPhase::AfterFactory);
    let http = Fake::new("HTTP", &log).in_stage(Stage::Web, StopPhase::Web);
    let https = Fake::new("HTTPS", &log).in_stage(Stage::Web, StopPhase::Web);
    let all = [&ssh, &cluster, &media, &sip, &xmpp, &smtp, &http, &https];
    for (port, fake) in all.into_iter().enumerate() {
        fake.on(u16::try_from(port).unwrap() + 1, 0);
    }
    let registry = registry(&all);
    boot(&registry);
    taken(&log);
    let wait = Duration::from_secs(30);
    registry.stop(StopPhase::Web, wait);
    assert_eq!(
        taken(&log),
        [
            "stop HTTP 7 Process",
            "stop HTTPS 8 Process",
            "finish HTTP 7 30s",
            "drop HTTP 7",
            "finish HTTPS 8 30s",
            "drop HTTPS 8",
        ]
    );
    registry.stop(StopPhase::BeforeFactory, wait);
    assert_eq!(
        taken(&log),
        [
            "stop XMPP 5 Process",
            "finish XMPP 5 30s",
            "drop XMPP 5",
            "stop SIP 4 Process",
            "finish SIP 4 30s",
            "drop SIP 4",
            "stop Call media 3 Process",
            "finish Call media 3 30s",
            "drop Call media 3",
        ]
    );
    registry.stop(StopPhase::AfterFactory, wait);
    assert_eq!(
        taken(&log),
        [
            "stop SMTP 6 Process",
            "finish SMTP 6 30s",
            "drop SMTP 6",
            "stop SSH 1 Process",
            "finish SSH 1 30s",
            "drop SSH 1",
            "stop Cluster 2 Process",
            "finish Cluster 2 30s",
            "drop Cluster 2",
        ]
    );
    for name in ["SSH", "TURN"] {
        let row = row(&registry, name);
        assert_eq!(row.state, State::off("the server is stopping"));
        assert_eq!(row.cause.as_deref(), Some("shutdown"));
    }
    assert!(registry.snapshot().iter().all(|info| info.address.is_none()));
    // a second stop finds nothing to stop
    registry.stop(StopPhase::AfterFactory, wait);
    assert!(taken(&log).is_empty());
}

#[test]
fn nothing_starts_once_the_server_is_stopping() {
    let log = log();
    let http = Fake::new("HTTP", &log).in_stage(Stage::Web, StopPhase::Web);
    let smtp = Fake::new("SMTP", &log);
    http.on(1, 0);
    smtp.on(2, 0);
    let registry = registry(&[&http, &smtp]);
    boot(&registry);
    registry.stop(StopPhase::Web, Duration::from_secs(30));
    taken(&log);
    // a reload during the drain: HTTP does not start again, SMTP does not rebind, but a stop still stops
    smtp.on(3, 0);
    let outcomes = reload(&registry);
    assert_eq!(actions(&outcomes), [("HTTP", Action::Held), ("SMTP", Action::Held)]);
    assert_eq!(outcomes[0].detail, "the server is stopping");
    assert!(taken(&log).is_empty());
    assert_eq!(registry.address("SMTP"), address(2));
    smtp.off("fake@enabled is false");
    let answer = requested(&registry, ReconcileRequest::new(Cause::ConfReload));
    assert_eq!(actions(&answer), [("HTTP", Action::Held), ("SMTP", Action::Stopped)]);
    assert_eq!(
        taken(&log),
        ["stop SMTP 2 Grace(5s)", "finish SMTP 2 7s", "drop SMTP 2"]
    );
}

#[test]
fn the_worker_merges_waiting_requests_into_one_pass() {
    let log = log();
    let fake = Fake::new("A", &log);
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    boot(&registry);
    taken(&log);
    fake.off("fake@enabled is false");
    // both requests wait while a pass (here the test) holds the lock, then run as one pass
    let held = lock(&registry.0.entries);
    let first = registry.request(
        ReconcileRequest::new(Cause::SetValue("fake@enabled".to_string())),
        Duration::ZERO,
    );
    let second = registry.request(
        ReconcileRequest {
            cause: Cause::ConfReload,
            end_now: vec!["A".to_string()],
        },
        Duration::ZERO,
    );
    drop(held);
    let wait = Duration::from_secs(5);
    let first = first.recv_timeout(wait).expect("the first answered").unwrap();
    let second = second.recv_timeout(wait).expect("the second answered").unwrap();
    assert_eq!(first, second);
    assert_eq!(actions(&first), [("A", Action::Stopped)]);
    // one pass: asked once, with the first cause, and the second's end-now kept
    assert_eq!(fake.seen(), ["boot", "set fake@enabled"]);
    assert_eq!(taken(&log), ["stop A 1 Now", "finish A 1 2s", "drop A 1"]);
    assert_eq!(
        row(&registry, "A").cause.as_deref(),
        Some("set fake@enabled; conf reload")
    );
}

/// A listener's start may queue a request (a signal) from within a pass, the boot's or the worker's own: it runs
/// after that pass, and neither waits on the other.
#[test]
fn a_request_queued_from_inside_a_pass_runs_after_it() {
    let log = log();
    let fake = Fake::new("A", &log);
    fake.on(1, 0);
    let registry = registry(&[&fake]);
    control::install_control(&registry.0.factory, &registry);
    fake.script().enqueue = true;
    boot(&registry);
    let from_a_pass = |count: usize| {
        fake.seen()
            .iter()
            .filter(|cause| *cause == "signal from a pass")
            .count()
            == count
    };
    eventually("the request queued from the boot pass", || from_a_pass(1));
    fake.on(2, 0);
    fake.script().enqueue = true;
    let outcomes = requested(&registry, ReconcileRequest::new(Cause::ConfReload));
    assert_eq!(actions(&outcomes), [("A", Action::Rebound)]);
    eventually("the request queued from the worker's pass", || from_a_pass(2));
    registry.close();
}

#[test]
fn the_control_reconciles_through_the_worker_and_a_deferred_request_waits() {
    let log = log();
    let fake = Fake::new("A", &log);
    let registry = registry(&[&fake]);
    boot(&registry);
    let factory = Arc::clone(&registry.0.factory);
    control::install_control(&factory, &registry);
    let control = moqui_core::listeners::control(&factory).expect("installed");
    fake.on(1, 0);
    let outcomes = control
        .reconcile(
            ReconcileRequest::new(Cause::Request {
                user: "admin".to_string(),
                reason: "setup".to_string(),
            }),
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(actions(&outcomes), [("A", Action::Started)]);
    assert_eq!(control.listeners(), registry.snapshot());
    assert_eq!(row(&registry, "A").cause.as_deref(), Some("request by admin: setup"));
    assert!(control
        .reconcile(ReconcileRequest::new(Cause::Boot), Duration::from_secs(5))
        .is_err());
    fake.off("fake@enabled is false");
    let asked = Instant::now();
    control.reconcile_later(ReconcileRequest::new(Cause::Signal("ACME")), Duration::from_millis(100));
    eventually("the deferred pass", || registry.address("A").is_none());
    assert!(asked.elapsed() >= Duration::from_millis(100), "{:?}", asked.elapsed());
    assert_eq!(row(&registry, "A").cause.as_deref(), Some("signal ACME"));
    // a delay too long to add to the clock waits rather than panics
    control.reconcile_later(ReconcileRequest::new(Cause::ConfReload), Duration::MAX);
    // an ended registry answers no more
    registry.close();
    assert_eq!(
        control.reconcile(ReconcileRequest::new(Cause::ConfReload), Duration::from_secs(1)),
        Err("the listener registry has stopped".to_string())
    );
}

/// The conf's `listeners` hook only queues a pass on the worker (plans/Runtime-Config.md §8.2): it returns while a pass
/// holds the registry's lock, and the pass it queued runs once the lock is free, with the cause of what ran the hook (a
/// reload, or the selectors a set changed). A registry installed on the factory later takes the hook over.
#[test]
fn the_conf_hook_queues_a_pass_without_waiting_for_it() {
    use moqui_core::context::conf_reload::{self, HookCause};
    use moqui_core::context::conf_schema::Group;
    let log = log();
    let fake = Fake::new("A", &log);
    let registry = registry(&[&fake]);
    boot(&registry);
    let factory = Arc::clone(&registry.0.factory);
    control::install_control(&factory, &registry);
    fake.on(1, 0);
    let held = lock(&registry.0.entries);
    let asked = Instant::now();
    let problems = conf_reload::run_hooks(&factory, &[Group::Listeners], &HookCause::Reload);
    assert!(problems.is_empty(), "{problems:?}");
    assert!(
        asked.elapsed() < Duration::from_secs(2),
        "the hook waited {:?}",
        asked.elapsed()
    );
    assert_eq!(registry.address("A"), None, "the queued pass waits for the lock");
    drop(held);
    eventually("the queued pass", || registry.address("A") == address(1));
    assert_eq!(row(&registry, "A").cause.as_deref(), Some("conf reload"));
    // a later registry on the factory takes the hook over; a set names its selectors on the row
    let second_fake = Fake::new("B", &log);
    second_fake.on(2, 0);
    let second = Registry::new(Arc::clone(&factory), boot_inputs());
    second.add(second_fake.clone());
    boot(&second);
    control::install_control(&factory, &second);
    second_fake.off("fake@enabled is false");
    let set = HookCause::Set(vec!["fake@enabled".to_string(), "fake@port".to_string()]);
    conf_reload::run_hooks(&factory, &[Group::Listeners], &set);
    eventually("the second registry's pass", || second.address("B").is_none());
    assert_eq!(row(&second, "B").cause.as_deref(), Some("set fake@enabled, fake@port"));
    // the first registry is asked no more: a pass asked of it now runs after anything queued before, and is the only
    // one since its conf reload
    requested(&registry, ReconcileRequest::new(Cause::Signal("probe")));
    assert_eq!(fake.seen(), ["boot", "conf reload", "signal probe"]);
    second.close();
    registry.close();
}

#[test]
fn close_drops_what_still_runs_and_ends_the_worker() {
    let log = log();
    let (a, b) = (Fake::new("A", &log), Fake::new("B", &log));
    a.on(1, 0);
    b.on(2, 0);
    let registry = registry(&[&a, &b]);
    boot(&registry);
    // the worker is running
    requested(&registry, ReconcileRequest::new(Cause::ConfReload));
    taken(&log);
    registry.close();
    assert_eq!(taken(&log), ["drop B 2", "drop A 1"]);
    assert_eq!(registry.address("A"), None);
    assert!(registry
        .request(ReconcileRequest::new(Cause::ConfReload), Duration::ZERO)
        .recv_timeout(Duration::from_secs(1))
        .is_err());
    // and nothing starts after it
    let outcomes = reload(&registry);
    assert_eq!(actions(&outcomes), [("A", Action::Held), ("B", Action::Held)]);
}

/// The worker holds the registry weakly: a registry dropped without `close` ends its worker.
#[test]
fn the_worker_ends_when_the_registry_is_dropped() {
    let log = log();
    let fake = Fake::new("A", &log);
    let registry = registry(&[&fake]);
    boot(&registry);
    requested(&registry, ReconcileRequest::new(Cause::ConfReload));
    let thread = worker::take_thread(&registry.0).expect("the worker is running");
    drop(registry);
    eventually("the worker's end", || thread.is_finished());
    thread.join().expect("the worker ended without a panic");
}

/// A listener reached in process only, as the cluster entry is when the boot has no network bind: it reads the boot's
/// inputs.
struct InProcess;

impl Listener for InProcess {
    type Bind = ();
    type InPlace = ();
    type Run = FakeRun;

    fn name(&self) -> &'static str {
        "Cluster"
    }

    fn stage(&self) -> Stage {
        Stage::ClusterPort
    }

    fn stop_phase(&self) -> StopPhase {
        StopPhase::AfterFactory
    }

    fn selectors(&self) -> &'static [&'static str] {
        &[]
    }

    fn wanted(&self, ctx: &Ctx) -> Wanted<(), ()> {
        match ctx.boot.bind {
            ListenBind::None => Wanted::Off(format!(
                "reached in process only, runtime {}",
                ctx.boot.runtime_dir.display()
            )),
            _ => Wanted::On { bind: (), in_place: () },
        }
    }

    fn start(&self, ctx: &Ctx, _bind: &(), _in_place: &()) -> Result<Option<FakeRun>, Error> {
        lock(&ctx.boot.cluster_posts).push("started".to_string());
        Ok(None)
    }

    fn overlaps(&self, _old: &(), _new: &()) -> bool {
        true
    }

    fn target(&self, _bind: &()) -> Option<Target> {
        None
    }
}

#[test]
fn the_boot_inputs_reach_each_entry() {
    let context = ExecutionContextFactory::test_context();
    let in_process = Registry::new(Arc::clone(context.factory()), boot_inputs());
    in_process.add(InProcess);
    boot(&in_process);
    assert_eq!(
        row(&in_process, "Cluster").state,
        State::off("reached in process only, runtime runtime-of-the-test")
    );
    let context = ExecutionContextFactory::test_context();
    let posts = Arc::new(Mutex::new(Vec::new()));
    let networked = Registry::new(
        Arc::clone(context.factory()),
        BootInputs {
            bind: ListenBind::Property,
            cluster_posts: Arc::clone(&posts),
            ..boot_inputs()
        },
    );
    networked.add(InProcess);
    boot(&networked);
    assert_eq!(*lock(&posts), ["started"]);
}

#[test]
fn on_runtime_runs_a_future_from_a_plain_thread_within_its_limit() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let handle = runtime.handle().clone();
    let limit = Duration::from_secs(5);
    assert_eq!(worker::on_runtime(&handle, limit, async { 7 }), Ok(7));
    // and from a blocking thread of the same runtime, where http::listen runs the web pass
    let inside = handle.clone();
    let from_blocking = runtime
        .block_on(
            async move { tokio::task::spawn_blocking(move || worker::on_runtime(&inside, limit, async { 8 })).await },
        )
        .unwrap();
    assert_eq!(from_blocking, Ok(8));
    let panicked = worker::on_runtime(&handle, limit, async {
        panic!("a bind that panics");
    });
    assert!(panicked.is_err());
    // a bind that never resolves is given up at the limit
    let never = worker::on_runtime(&handle, Duration::from_millis(100), std::future::pending::<()>());
    assert_eq!(
        never,
        Err("the listener's task did not finish within 0.1 s".to_string())
    );
}

/// A start that panics, or fails, before its own start records the rows it owns beside its own leaves none missing.
#[test]
fn a_failed_start_records_the_rows_it_owns_as_not_running() {
    let log = log();
    let media = Fake {
        extras: &["TURN"],
        ..Fake::new("Call media", &log)
    };
    media.on(10000, 0);
    media.script().start_panics = true;
    let registry = registry(&[&media]);
    assert_eq!(actions(&boot(&registry)), [("Call media", Action::Failed)]);
    let turn = row(&registry, "TURN");
    assert_eq!(turn.state, State::off("Call media is not running"));
    assert_eq!(turn.cause.as_deref(), Some("boot"));
}

/// A registry dropped without `close` (a boot that failed part way) drops what runs last added first, as the boot's
/// locals unwound before the registry.
#[test]
fn a_registry_dropped_unclosed_drops_what_runs_last_added_first() {
    let log = log();
    let (a, b) = (Fake::new("A", &log), Fake::new("B", &log));
    a.on(1, 0);
    b.on(2, 0);
    let registry = registry(&[&a, &b]);
    boot(&registry);
    taken(&log);
    drop(registry);
    assert_eq!(taken(&log), ["drop B 2", "drop A 1"]);
}

/// A boot entry changes in a pass exactly when the conf reload applies its settings there (plans/Runtime-Config.md §8.1,
/// §8.2): every selector of a live entry is `Apply(Group::Listeners)` in the conf schema, and none of one that waits for a
/// restart is. A phase that makes one live changes both together (the web listeners in RC2, SSH in RC3).
#[test]
fn live_matches_the_conf_schema() {
    use moqui_core::context::conf_schema::{attribute_class, Class, Group};
    let context = ExecutionContextFactory::test_context();
    let registry = Registry::new(Arc::clone(context.factory()), boot_inputs());
    add_boot_entries(&registry);
    let entries = lock(&registry.0.entries);
    assert!(!entries.is_empty());
    for entry in entries.iter() {
        assert!(!entry.selectors().is_empty(), "{}", entry.name());
        for selector in entry.selectors() {
            let path = moqui_lib::node::path::AttrPath::parse(selector)
                .unwrap_or_else(|err| panic!("{}: {selector} is not a selector: {err:?}", entry.name()));
            let elements: Vec<&str> = path.steps.iter().map(|step| step.element.as_str()).collect();
            let applies = attribute_class(&elements, &path.attribute) == Class::Apply(Group::Listeners);
            assert_eq!(applies, entry.live(), "{} {selector}", entry.name());
        }
    }
}

/// The boot's real entries, in their fixed order and stages, stop in ServeProcess's order of before the registry:
/// the web listeners together; XMPP, SIP, then call media; SMTP, SSH, then the cluster port. Nothing is started.
#[test]
fn the_boot_entries_stop_in_the_order_of_before_the_registry() {
    let context = ExecutionContextFactory::test_context();
    let registry = Registry::new(Arc::clone(context.factory()), boot_inputs());
    add_boot_entries(&registry);
    let entries = lock(&registry.0.entries);
    let stages: Vec<(&str, Stage)> = entries.iter().map(|entry| (entry.name(), entry.stage())).collect();
    let mut expected = Vec::new();
    #[cfg(feature = "ssh")]
    expected.push(("SSH", Stage::BeforeCluster));
    expected.extend([
        ("Cluster", Stage::ClusterPort),
        ("Call media", Stage::Comms),
        ("SIP", Stage::Comms),
        ("XMPP", Stage::Comms),
        ("SMTP", Stage::Comms),
        ("HTTP", Stage::Web),
        ("HTTPS", Stage::Web),
    ]);
    #[cfg(feature = "grpc")]
    expected.push(("gRPC", Stage::Web));
    assert_eq!(stages, expected);
    // each phase's turns: one rank's entries stop together, a lower rank first
    let turns = |phase: StopPhase| -> Vec<Vec<&str>> {
        let mut ranked: Vec<(u8, usize)> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.stop_phase() == phase)
            .map(|(index, entry)| (entry.stop_rank(), index))
            .collect();
        ranked.sort_unstable();
        ranked
            .chunk_by(|(first, _), (next, _)| first == next)
            .map(|turn| turn.iter().map(|&(_, index)| entries[index].name()).collect())
            .collect()
    };
    let mut web = vec!["HTTP", "HTTPS"];
    #[cfg(feature = "grpc")]
    web.push("gRPC");
    assert_eq!(turns(StopPhase::Web), [web]);
    assert_eq!(
        turns(StopPhase::BeforeFactory),
        [vec!["XMPP"], vec!["SIP"], vec!["Call media"]]
    );
    let mut after = vec![vec!["SMTP"]];
    #[cfg(feature = "ssh")]
    after.push(vec!["SSH"]);
    after.push(vec!["Cluster"]);
    assert_eq!(turns(StopPhase::AfterFactory), after);
    let extras: Vec<&str> = entries
        .iter()
        .flat_map(|entry| entry.extra_rows().iter().copied())
        .collect();
    assert_eq!(extras, ["TURN"]);
}
