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

//! The thread's oxide runtime and a run on it: started on the thread's first oxide script with [`MoquiHost`] and a
//! 64 MiB heap cap, counted in the thread's shared depth, deadline and default limit ([`crate::script::run`]),
//! dropped and started again after a host-function panic poisons it, and reported in the engine stats.
//!
//! A run is not nested in another oxide run on the thread (the router sends such a script to QuickJS until the fork's
//! re-entry handle F3a lands), so every Context call here is the runtime's one entry.

use std::cell::{Cell, RefCell};
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

use moqui_lib::object::Value as HostValue;
use quickjs_oxide::engine::api::core::{
    CallableRef, Context, HeapLimits, Hook, HostClass, Js, JsResult, ObjectRef, OrdinaryPropertyDescriptor,
    RuntimeError, RuntimeOptions, Scope, Value,
};

use super::compiled::Compiled;
use super::convert::{key, to_host, truthy, Conv};
use super::errors::{panic_message, runtime_error, throw_plain};
use super::host::MoquiHost;
use super::tracked::Flush;
use crate::context::Error;
use crate::script::run::{self, EngineTag, ScriptLimit, MAX_SCRIPT_DEPTH};
use crate::script::runtime::EntityCell;

/// The runtime's heap cap, as QuickJS's memory limit.
const HEAP_CAP: usize = 64 * 1024 * 1024;
/// The collection floor: below it the runtime does not collect. Lower than the fork's 1 MiB default, since every
/// thread that runs a small script holds a runtime (plans/archive/Script-Engine-Oxide.md section 4).
const MIN_HEAP: usize = 512 * 1024;
/// What a collection lets the heap grow by before the next, as a multiple of the live bytes it left: 1 where the fork's
/// default is 2. Over the mr-mantle suite under `oxide` the runtimes' summed peak fell from 157 MB to 117 MB (the main
/// thread's from 39 MB to 29 MB) at the same run time; the reserved bytes follow the budget, and the arenas give little
/// back once grown.
const GROWTH: f64 = 1.0;
/// Runs between looks at the clock for a sample of the runtime's reserved bytes, and the least time between two
/// samples: a memory report walks the runtime's code store, so its cost grows with the texts compiled (4000 texts made
/// a sample every 32 runs cost an expression about twice its time).
const HEAP_SAMPLE_RUNS: u64 = 32;
const HEAP_SAMPLE_EVERY: std::time::Duration = std::time::Duration::from_millis(250);
/// The cache pressure check, as QuickJS's (`runtime.rs`, `CACHE_HEAP_PRESSURE`, three quarters of its limit) but on the
/// reserved bytes the cap compares against: when a sample at the end of an outermost run finds them past this, the
/// runtime collects, and if they are still past it its caches go (the transient code, the screen units, the property
/// keys) and it collects again. The heap's arenas give memory back only from their ends, so once a run has made many
/// values the reserved bytes stay two to three times the live ones (the app suites' main thread: 45 MB for 15 MB); a
/// runtime still past this after its caches went is dropped and started again on the next script, rather than failing
/// every later script with `out of memory` at the cap.
const PRESSURE_BYTES: usize = HEAP_CAP / 2;

/// A runtime holding at least this many reserved bytes, and [`FRAGMENTED_RATIO`] times what its last collection left
/// live, is dropped at an outermost run's end and starts fresh at the next script: its arenas keep a peak's slots, with
/// the survivors scattered through them (Profiling Round 44: a main thread's objects in 40 of 46 chunks), and give
/// back only a trailing free run. Below it a runtime keeps its caches, however fragmented.
const FRAGMENTED_BYTES: usize = 8 * 1024 * 1024;
/// See [`FRAGMENTED_BYTES`].
const FRAGMENTED_RATIO: usize = 5;
/// Ratios are of halves: 5 halves, 2.5 times live.
const FRAGMENTED_DIVISOR: usize = 2;
/// The least time between two such drops on a thread, so a thread whose work keeps rebuilding its peak does not pay a
/// start for every run.
const FRAGMENTED_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// The least time between two collections that look for a fragmented runtime on a thread.
const FRAGMENTED_CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

thread_local! {
    /// When this thread last dropped a fragmented runtime ([`FRAGMENTED_BYTES`]).
    static LAST_FRAGMENTED_DROP: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
    /// When this thread last collected to look for one.
    static LAST_FRAGMENTED_CHECK: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// `toNumber(x)`, as the QuickJS host's (`runtime/helpers.rs`): a JavaScript number from a decimal, a number or
/// numeric text, and null from null or empty text. Read-only, as the QuickJS host's globals are.
const TO_NUMBER_JS: &str = r#"Object.defineProperty(globalThis, "toNumber", {
    value: function toNumber(value) {
        if (value == null || value === "") return null;
        if (typeof value === "number") return value;
        return toDecimal(value).toNumber();
    },
    writable: true, enumerable: true, configurable: true
})"#;

/// The host globals made read-only once the shims have run, as QuickJS's `FREEZE_HOST_GLOBALS` (`runtime.rs`) makes
/// them: every global the runtime did not have at its start (`builtins`) becomes a getter of its value with a setter
/// that throws, in sloppy code too, so one script's assignment does not change it for the later scripts on the thread.
/// A global defined read-only already (`ec`, `toNumber`) is left as it is, and `sri` (the `sri` shim's, read on every
/// statement of a screen unit) is a plain read-only value, as on QuickJS.
const FREEZE_HOST_GLOBALS_JS: &str = r#"(function (builtins) {
    const known = new Set(builtins);
    for (const name of Object.getOwnPropertyNames(globalThis)) {
        if (known.has(name) || name === "ec") continue;
        const held = Object.getOwnPropertyDescriptor(globalThis, name);
        if (!held.configurable) continue;
        if (name === "sri") {
            Object.defineProperty(globalThis, name, { writable: false, configurable: false });
            continue;
        }
        const value = globalThis[name];
        Object.defineProperty(globalThis, name, {
            get() { return value; },
            set() { throw new TypeError(name + " is a host global and cannot be assigned"); },
            enumerable: true,
            configurable: false
        });
    }
})"#;

/// What QuickJS's engine puts under `ec` outside the host table, which the host table's shims extend (`runtime.rs`,
/// `install_ec`): `ec.context`, the context stack's `get` and the two names a form reads. A small text that names it
/// goes to QuickJS (the static check does not allow it), so it is what the shims need, not the whole QuickJS object.
/// The global `sri` is the `sri` shim's own.
const EC_EXTRAS_JS: &str = r#"(function (ec, context) {
    const made = { get: (name) => context.get(String(name)) };
    for (const name of ["formListFindId", "orderByField"]) {
        Object.defineProperty(made, name, { get: () => context[name], enumerable: true });
    }
    ec.context = made;
})"#;

/// Why an oxide run gave no result.
pub(crate) enum Outcome {
    /// The script's own error, as the QuickJS path would give it.
    Error(Error),
    /// The run met something the oxide bindings do not have yet; when it reached no host call, the router runs the
    /// script on QuickJS.
    Fallback(&'static str),
}

/// Objects of the runtime the bindings use, made once when it starts.
pub(super) struct Protos {
    pub(super) decimal: ObjectRef,
    pub(super) entities: HostClass<super::names::EntityValues>,
    /// Entity lists (`bind::entities`).
    pub(super) lists: HostClass<super::bind::entities::ListValues>,
    /// Native tools (`ec.getTool`), the host table's exotic class (`bind::tool`).
    pub(super) tools: HostClass<super::bind::tool::ToolClass>,
    /// The bare name `context`, the stack itself ([`super::context`]), and its `Map` methods.
    pub(super) context: ObjectRef,
    pub(super) context_methods: ObjectRef,
    /// Host objects the bindings pass through but do not bind yet (an upload, a stream).
    pub(super) opaque: HostClass<super::names::OpaqueValues>,
    pub(super) set_prototype: ObjectRef,
    /// `Set`, for a Set the stack holds under `oxide` ([`OxideEngine::whole`]).
    pub(super) set: CallableRef,
    pub(super) object_prototype: ObjectRef,
    pub(super) date_prototype: ObjectRef,
    pub(super) date_get_time: CallableRef,
    pub(super) array_from: CallableRef,
    pub(super) error: CallableRef,
    /// The helper objects the shims extend, as QuickJS's `install_context` makes them: `CollectionUtilities`,
    /// `StringUtilities` and `WebUtilities` by name.
    pub(super) helpers: ObjectRef,
    /// The same helper objects by name, which a bare name reads when the stack does not hold it.
    pub(super) helper_objects: Vec<(&'static str, ObjectRef)>,
}

pub(super) struct OxideEngine {
    runtime: quickjs_oxide::engine::api::core::Runtime,
    context: Context,
    pub(super) protos: Protos,
    compiled: RefCell<Compiled>,
    /// The values this run read or assigned by name ([`super::tracked`]). Emptied when the run ends.
    run: RefCell<super::tracked::RunValues>,
    /// The entity values this run made, with their cells, so one converts back to its cell.
    entities: RefCell<EntityObjects>,
    /// The entity value and list prototypes, kept once made ([`OxideEngine::entity_prototype`]).
    entity_proto: std::cell::OnceCell<ObjectRef>,
    list_proto: std::cell::OnceCell<ObjectRef>,
    /// The reserved bytes last sampled, counted in [`counts::HEAP`], and the most sampled.
    heap: Cell<usize>,
    heap_peak: Cell<usize>,
    runs: Cell<u64>,
    /// The tools shim's `__moquiWrap(name, data)`, which gives a tools map its methods back as on QuickJS
    /// ([`super::convert`]); set once the shims installed.
    tools_wrap: std::cell::OnceCell<CallableRef>,
    /// The script servlets' invoker, made at the first request ([`super::web`]).
    web_invoker: std::cell::OnceCell<CallableRef>,
    /// When the reserved bytes were last sampled.
    sampled: Cell<Option<std::time::Instant>>,
    /// The screen units' compiled functions by unit id, the mode runtimes run so far by id, and the depth of the unit
    /// running now ([`super::units`]).
    pub(super) units: RefCell<crate::script::BoundedMap<u64, CallableRef>>,
    pub(super) unit_runtimes: RefCell<std::collections::HashSet<u64>>,
    pub(super) unit_depth: Cell<u32>,
    /// The values inert modules gave in the outermost run, by location, with the text each was made from
    /// (`ec.resource.script`, [`super::resource`]); emptied when the outermost run ends.
    modules: RefCell<std::collections::HashMap<String, (Arc<str>, Value)>>,
    /// Property keys of the map keys converted ([`OxideEngine::key`]): a key made once interns once.
    keys: RefCell<
        std::collections::HashMap<Box<str>, quickjs_oxide::engine::api::core::PropertyKey, foldhash::fast::RandomState>,
    >,
    /// Set when the runtime is dropped to start again on the same thread ([`PRESSURE_BYTES`]): the thread's peak stays.
    restarting: Cell<bool>,
    /// Set when anything in the outermost run ran out of heap ([`OxideEngine::note_out_of_memory`]), a screen unit's
    /// compile or call or a nested run among them; its end then relieves the heap as for a run that ran out.
    ran_out: Cell<bool>,
}

/// Map keys kept as property keys, at most; past it the cache starts again. Only a key of at most [`MAX_KEY_BYTES`] is
/// kept, so the cache holds at most about 8192 * 64 bytes of text: a map key is a field name, and a long one (a text
/// used as a key) is made each time.
const MAX_KEYS: usize = 8192;
const MAX_KEY_BYTES: usize = 64;

/// Process-wide counts of the oxide runtimes, for [`stats`].
mod counts {
    use super::{AtomicU64, AtomicUsize};
    pub(super) static STARTED: AtomicUsize = AtomicUsize::new(0);
    pub(super) static LIVE: AtomicUsize = AtomicUsize::new(0);
    pub(super) static LIVE_MAX: AtomicUsize = AtomicUsize::new(0);
    pub(super) static HEAP: AtomicUsize = AtomicUsize::new(0);
    pub(super) static HEAP_MAX: AtomicUsize = AtomicUsize::new(0);
    pub(super) static COMPILED: AtomicU64 = AtomicU64::new(0);
    pub(super) static POISONED: AtomicU64 = AtomicU64::new(0);
    pub(super) static RESTARTED: AtomicU64 = AtomicU64::new(0);
    pub(super) static SHED_FRAGMENTED: AtomicU64 = AtomicU64::new(0);
    pub(super) static RELEASED_IDLE: AtomicU64 = AtomicU64::new(0);
}

/// The most reserved heap bytes each thread's oxide runtime reached, by thread, with the thread's name (or id), for the
/// peak heap of a run (`MOQUI_SCRIPT_STATS_FILE`). An entry goes with its runtime.
type ThreadPeaks = std::collections::HashMap<std::thread::ThreadId, (String, usize)>;

static THREAD_PEAKS: std::sync::LazyLock<std::sync::Mutex<ThreadPeaks>> = std::sync::LazyLock::new(Default::default);

fn note_thread_peak(bytes: usize) {
    let thread = std::thread::current();
    let mut peaks = moqui_lib::sync::lock(&THREAD_PEAKS);
    let peak = peaks.entry(thread.id()).or_insert_with(|| {
        let name = thread
            .name()
            .map_or_else(|| format!("{:?}", thread.id()), str::to_string);
        (name, 0)
    });
    peak.1 = peak.1.max(bytes);
}

/// Each live runtime's reserved heap bytes as last sampled, by thread and runtime, with the thread's name: what the
/// process's total is made of, for [`heap_at_max`].
type RuntimeHeaps = std::collections::HashMap<(std::thread::ThreadId, usize), (String, usize)>;

static RUNTIME_HEAPS: std::sync::LazyLock<std::sync::Mutex<RuntimeHeaps>> = std::sync::LazyLock::new(Default::default);

/// The runtimes' heaps (by thread name) when the process's total last rose a MiB past the one before: what made the
/// peak a run's stats report.
static HEAP_AT_MAX: std::sync::Mutex<(usize, Vec<(String, usize)>)> = std::sync::Mutex::new((0, Vec::new()));

/// Note `bytes` as the runtime at `key`'s heap now, and when the process's `total` is a new high by a MiB, what each
/// runtime held at it.
fn note_runtime_heap(key: usize, bytes: usize, total: usize) {
    let thread = std::thread::current();
    let mut heaps = moqui_lib::sync::lock(&RUNTIME_HEAPS);
    let entry = heaps.entry((thread.id(), key)).or_insert_with(|| {
        let name = thread
            .name()
            .map_or_else(|| format!("{:?}", thread.id()), str::to_string);
        (name, 0)
    });
    entry.1 = bytes;
    let mut at_max = moqui_lib::sync::lock(&HEAP_AT_MAX);
    if total >= at_max.0 + 1024 * 1024 {
        let mut held: Vec<(String, usize)> = heaps.values().cloned().collect();
        held.sort_by_key(|held| std::cmp::Reverse(held.1));
        *at_max = (total, held);
    }
}

/// The process's highest total of reserved runtime heap bytes (to a MiB), and each runtime's share of it by thread
/// name, the largest first.
pub(crate) fn heap_at_max() -> (usize, Vec<(String, usize)>) {
    moqui_lib::sync::lock(&HEAP_AT_MAX).clone()
}

/// The peak reserved heap bytes of each live thread's oxide runtime, as sampled, by thread name.
pub(crate) fn thread_heap_peaks() -> Vec<(String, usize)> {
    let mut peaks: Vec<(String, usize)> = moqui_lib::sync::lock(&THREAD_PEAKS).values().cloned().collect();
    peaks.sort();
    peaks
}

/// What the process's oxide runtimes have done since it started.
#[derive(Clone, Debug, Default)]
pub(crate) struct OxideStats {
    pub(crate) engines_started: usize,
    pub(crate) engines_live: usize,
    pub(crate) engines_live_max: usize,
    /// The runtimes' reserved heap bytes as last sampled, now and at their highest.
    pub(crate) heap_bytes: usize,
    pub(crate) heap_bytes_max: usize,
    /// Texts compiled.
    pub(crate) compiled: u64,
    /// Runtimes dropped after a host function panicked.
    pub(crate) poisoned: u64,
    /// Runtimes dropped and started again because their heap stayed past [`PRESSURE_BYTES`] with their caches gone.
    pub(crate) restarted: u64,
    /// Runtimes dropped and started fresh because their reserved bytes were far past their live ones
    /// ([`FRAGMENTED_BYTES`]).
    pub(crate) shed_fragmented: u64,
    /// Runtimes let go by a thread that waited for work ([`release_idle`]).
    pub(crate) released_idle: u64,
    /// The process's code images ([`super::compiled`]).
    pub(crate) images: super::compiled::ImageStats,
}

pub(crate) fn stats() -> OxideStats {
    OxideStats {
        engines_started: counts::STARTED.load(Relaxed),
        engines_live: counts::LIVE.load(Relaxed),
        engines_live_max: counts::LIVE_MAX.load(Relaxed),
        heap_bytes: counts::HEAP.load(Relaxed),
        heap_bytes_max: counts::HEAP_MAX.load(Relaxed),
        compiled: counts::COMPILED.load(Relaxed),
        poisoned: counts::POISONED.load(Relaxed),
        restarted: counts::RESTARTED.load(Relaxed),
        shed_fragmented: counts::SHED_FRAGMENTED.load(Relaxed),
        released_idle: counts::RELEASED_IDLE.load(Relaxed),
        images: super::compiled::image_stats(),
    }
}

/// What this thread's oxide runtime holds.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OxideMemory {
    pub(crate) engines: usize,
    /// Bytes the heap's arenas and tables reserve, and the live bytes the last collection left.
    pub(crate) reserved_bytes: usize,
    pub(crate) live_bytes: usize,
    /// Compiled units and the bytes of their instructions, positions and source.
    pub(crate) code_units: usize,
    pub(crate) code_bytes: usize,
    /// Texts in the compiled-expression cache, and their source bytes.
    pub(crate) compiled_scripts: usize,
    pub(crate) compiled_script_bytes: usize,
}

/// [`OxideMemory`] of this thread's runtime, after a collection when `collect`. Not while a script runs on it.
pub(crate) fn engine_memory(collect: bool) -> OxideMemory {
    if run::is_running(EngineTag::Oxide) {
        return OxideMemory::default();
    }
    // the main runtime's and the spec runtime's ([`super::spec`]), summed
    let engines = [ENGINE.with(|engine| engine.borrow().clone()), super::spec::current()];
    let mut memory = OxideMemory::default();
    for engine in engines.into_iter().flatten() {
        if collect {
            let _ = engine.runtime.run_gc();
        }
        memory.engines += 1;
        if let Ok(report) = engine.runtime.memory_report() {
            memory.reserved_bytes += report.reserved_bytes();
            memory.live_bytes += report.gc.live_bytes;
            memory.code_units += report.code.units;
            memory.code_bytes +=
                report.code.instruction_bytes + report.code.position_bytes + report.code.source_bytes;
        }
        let compiled = engine.compiled.borrow();
        memory.compiled_scripts += compiled.len();
        memory.compiled_script_bytes += compiled.bytes();
    }
    memory
}

/// One line of this thread's oxide runtimes' heaps (the main one and the spec runtime), after a collection, for a
/// per-spec heap log (`MOQUI_SCRIPT_HEAP_LOG`). Empty while a script runs on the thread.
pub(crate) fn heap_line() -> String {
    use std::fmt::Write;
    if run::is_running(EngineTag::Oxide) {
        return String::new();
    }
    let mut line = String::new();
    let engines = [("main", ENGINE.with(|engine| engine.borrow().clone())), ("spec", super::spec::current())];
    for (name, engine) in engines {
        let Some(engine) = engine else { continue };
        let _ = engine.runtime.run_gc();
        let Ok(report) = engine.runtime.memory_report() else { continue };
        let _ = write!(
            line,
            "{name}: reserved={} live={} dyncode={} atoms={} pinned={} ic={} tables={} compiled={:?} units={}/{} modules={} keys={} entities={} |",
            report.reserved_bytes(),
            report.gc.live_bytes,
            report.code.dynamic_bytes,
            report.atoms,
            report.pinned_atoms,
            report.ic_entries,
            report.table_bytes,
            engine.compiled.borrow().split(),
            engine.units.borrow().len(),
            engine.units.borrow().bytes(),
            engine.modules.borrow().len(),
            engine.keys.borrow().len(),
            engine.entities.borrow().list.len(),
        );
        for (arena, held) in report.arenas() {
            let _ = write!(line, " {arena}={}/{}/{}", held.live, held.used_bytes, held.reserved_bytes);
        }
        let _ = write!(
            line,
            " values={}/{}/{} rope={} regexp={} budget={} threshold={}; ",
            report.values.used_slots,
            report.values.capacity_slots,
            report.values.reserved_bytes,
            report.rope_bytes,
            report.code.regexp_bytes,
            report.budget,
            report.threshold
        );
    }
    line
}

thread_local! {
    static ENGINE: RefCell<Option<Rc<OxideEngine>>> = const { RefCell::new(None) };
    /// What the running oxide script met that the bindings do not have, set by a host function before it throws.
    static FALLBACK: Cell<Option<&'static str>> = const { Cell::new(None) };
    /// When the runtime last failed to start on this thread: it is not tried again for [`START_BACKOFF`].
    static START_FAILED: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
    /// Whether the runtime is starting on this thread: the deadline does not stop it ([`starting`]).
    static STARTING: Cell<bool> = const { Cell::new(false) };
}

/// How long a thread whose oxide runtime failed to start runs its small scripts on QuickJS before it tries again.
const START_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);

/// Why a run fell back when the runtime did not start: no fallback of the text's own, so the router does not count it
/// against the text.
pub(crate) const DID_NOT_START: &str = "the quickjs-oxide runtime did not start";

/// When a start failure was last logged as an error in this process: once per [`START_BACKOFF`], the rest as debug
/// lines.
static START_FAILURE_LOGGED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

#[cfg(test)]
thread_local! {
    static FAIL_START: Cell<bool> = const { Cell::new(false) };
}

/// Whether this thread's oxide runtime is starting: its install runs without the script deadline's interrupt (a
/// nested start under a QuickJS script's deadline would otherwise be cut off part way), and its host calls note no
/// census row.
pub(super) fn starting() -> bool {
    STARTING.with(Cell::get)
}

/// Whether this thread's runtime failed to start within the last [`START_BACKOFF`]: its small scripts go straight to
/// QuickJS, before the router hashes, checks or flushes anything for an oxide run.
pub(crate) fn start_failed_recently() -> bool {
    START_FAILED.with(Cell::get).is_some_and(|failed| failed.elapsed() < START_BACKOFF)
}

/// The start of the runtime, from [`engine`] until dropped: no interrupt. It ends what it
/// began, so it gives back what was there before.
struct Starting {
    before: bool,
    /// The fallback mark of a run of another runtime the start is under (a spec's lent host call): set aside, so the
    /// shims' host calls are not refused as calls of a run that fell back.
    fallback: Option<&'static str>,
}

impl Starting {
    fn enter() -> Self {
        Self {
            before: STARTING.with(|starting| starting.replace(true)),
            fallback: FALLBACK.with(Cell::take),
        }
    }
}

impl Drop for Starting {
    fn drop(&mut self) {
        STARTING.with(|starting| starting.set(self.before));
        FALLBACK.with(|fallback| fallback.set(self.fallback));
    }
}

/// A script run while the runtime starts (from a shim's host call) is a script, with its deadline: the start's
/// [`STARTING`] is set aside for it, and given back when it ends.
struct StartSetAside(bool);

impl StartSetAside {
    fn enter() -> Self {
        Self(STARTING.with(|starting| starting.replace(false)))
    }
}

impl Drop for StartSetAside {
    fn drop(&mut self) {
        STARTING.with(|starting| starting.set(self.0));
    }
}

/// Set the start's [`STARTING`] for the tests of the interrupt, until dropped.
#[cfg(test)]
pub(super) fn starting_for_tests() -> impl Drop {
    Starting::enter()
}

/// Whether the running oxide script met what the bindings do not have: a `catch` of the throw must not make side
/// effects, so a bare-name read or write and an entity field write then throw again, and the deadline hook stops the
/// script at its next poll ([`super::host::MoquiHost`]). The run then falls back.
pub(super) fn fallback_pending() -> bool {
    FALLBACK.with(|fallback| fallback.get().is_some())
}

/// Mark the running oxide script as having met what the bindings do not have, for the tests.
#[cfg(test)]
pub(super) fn set_fallback_for_tests(reason: &'static str) {
    FALLBACK.with(|fallback| fallback.set(Some(reason)));
}

/// What the running oxide script met that the bindings do not have, taken: a run that does not fall back reports it as
/// its error ([`super::spec`]).
pub(super) fn take_fallback() -> Option<&'static str> {
    FALLBACK.with(Cell::take)
}

/// The oxide runtime of the innermost run on this thread ([`Active`]), else the thread's main runtime when it has
/// started: what the conversions, the names and the binder's hooks work in.
pub(super) fn current() -> Option<Rc<OxideEngine>> {
    if let Some((active, _)) = ACTIVE.with(|active| active.borrow().last().cloned()) {
        return Some(active);
    }
    ENGINE.with(|engine| engine.borrow().clone())
}

thread_local! {
    /// The runtimes entered on this thread, innermost last, each with whether it was entered for a run (else for its
    /// start). A thread may hold two: the main one, and the test harness's spec runtime ([`super::spec`]), whose host
    /// calls run the main one's scripts.
    static ACTIVE: RefCell<Vec<(Rc<OxideEngine>, bool)>> = const { RefCell::new(Vec::new()) };
}

/// A runtime entered for a run or its start, until dropped: [`current`] is it.
pub(super) struct Active(Rc<OxideEngine>);

impl Active {
    /// For a run.
    pub(super) fn enter(engine: &Rc<OxideEngine>) -> Self {
        ACTIVE.with(|active| active.borrow_mut().push((Rc::clone(engine), true)));
        Active(Rc::clone(engine))
    }

    /// For work on it that is not a run: the rest of its start (its shims' host calls convert in it), a global's
    /// install.
    pub(super) fn starting(engine: &Rc<OxideEngine>) -> Self {
        ACTIVE.with(|active| active.borrow_mut().push((Rc::clone(engine), false)));
        Active(Rc::clone(engine))
    }

    /// The thread's main runtime when it has a run in progress (a screen unit nested in it runs in that run).
    pub(super) fn main_running() -> Option<Rc<OxideEngine>> {
        ENGINE
            .with(|engine| engine.borrow().clone())
            .filter(Self::holds)
    }

    /// Whether a runtime's start ([`Active::starting`]) is on this thread's stack.
    fn starting_on_stack() -> bool {
        ACTIVE.with(|active| active.borrow().iter().any(|(_, run)| !run))
    }

    /// Whether a runtime other than `engine` has a run in progress on this thread.
    fn other_run(engine: &OxideEngine) -> bool {
        ACTIVE.with(|active| {
            active
                .borrow()
                .iter()
                .any(|(held, run)| *run && !std::ptr::eq(Rc::as_ptr(held), engine))
        })
    }

    /// Whether `engine` has a run in progress on this thread: a run on it now is nested in that one.
    pub(super) fn holds(engine: &Rc<OxideEngine>) -> bool {
        ACTIVE.with(|active| {
            active
                .borrow()
                .iter()
                .any(|(held, run)| *run && Rc::ptr_eq(held, engine))
        })
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            let popped = active.borrow_mut().pop();
            debug_assert!(
                popped.is_some_and(|(held, _)| Rc::ptr_eq(&held, &self.0)),
                "the oxide runtimes entered are left in another order than they were entered"
            );
        });
    }
}

/// The runtimes entered on this thread now, for the tests.
#[cfg(test)]
pub(super) fn active_depth() -> usize {
    ACTIVE.with(|active| active.borrow().len())
}

/// Let this thread's runtime go, when no script runs on it: for a worker that has waited for work a while, so the heap
/// a pool's idle threads hold goes back. Its next script starts a runtime again, from the process's code images.
/// Whether one was let go.
pub(crate) fn release_idle() -> bool {
    if run::is_running(EngineTag::Oxide) {
        return false;
    }
    let Some(engine) = ENGINE.with(|engine| engine.borrow().clone()) else {
        return false;
    };
    if Active::holds(&engine) {
        return false;
    }
    // the thread's peak stays in the stats: the runtime starts again here
    engine.restarting.set(true);
    ENGINE.with(|current| *current.borrow_mut() = None);
    counts::RELEASED_IDLE.fetch_add(1, Relaxed);
    true
}

/// This thread's oxide runtime, started if need be. A start that fails is remembered for the thread
/// ([`START_BACKOFF`]) and logged once for the process; nothing of a script has run then, so its caller falls back.
fn engine() -> Result<Rc<OxideEngine>, Error> {
    // the main one, whichever runtime has the innermost run
    if let Some(engine) = ENGINE.with(|engine| engine.borrow().clone()) {
        return Ok(engine);
    }
    if start_failed_recently() {
        return Err(Error::new(DID_NOT_START));
    }
    let starting = Starting::enter();
    let started = start_engine();
    drop(starting);
    match started {
        Ok(started) => {
            START_FAILED.with(|failed| failed.set(None));
            Ok(started)
        }
        Err(err) => {
            START_FAILED.with(|failed| failed.set(Some(std::time::Instant::now())));
            let log_error = {
                let mut logged = moqui_lib::sync::lock(&START_FAILURE_LOGGED);
                let due = logged.is_none_or(|at| at.elapsed() >= START_BACKOFF);
                if due {
                    *logged = Some(std::time::Instant::now());
                }
                due
            };
            if log_error {
                tracing::error!(
                    error = %err,
                    "{DID_NOT_START}; small scripts on the thread run on QuickJS, and it is tried again after a minute"
                );
            } else {
                tracing::debug!(error = %err, "{DID_NOT_START}");
            }
            Err(Error::new(format!("{DID_NOT_START}: {err}")))
        }
    }
}

/// Declares the entity model's names (entity names, field and alias names, relationship short aliases) as the
/// process's shared keys, once a process: the code images that name them then share their key tables in every runtime,
/// and a runtime that reads an entity's fields finds the shared atoms. The list is bounded by the model; a key made from
/// run-time data is never declared. Skipped while no entity facade is installed (tried again at the next start).
fn declare_model_keys(runtime: &quickjs_oxide::engine::api::core::Runtime) {
    static DECLARED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DECLARED.load(Relaxed) {
        return;
    }
    let Ok(facade) = crate::script::host::entity_facade() else { return };
    let names: std::collections::BTreeSet<String> = facade.with_model(|model| {
        let mut names = std::collections::BTreeSet::new();
        for entity in model.entities() {
            names.insert(entity.entity_name.clone());
            names.insert(entity.full_name.clone());
            names.extend(entity.short_alias.iter().cloned());
            names.extend(entity.fields.iter().map(|field| field.name.clone()));
            names.extend(entity.aliases.iter().map(|alias| alias.name.clone()));
            names.extend(entity.relationships.iter().filter_map(|relationship| relationship.short_alias.clone()));
        }
        names
    });
    if runtime.share_keys(names.iter().map(String::as_str)).is_ok() {
        DECLARED.store(true, Relaxed);
    }
}

fn start_engine() -> Result<Rc<OxideEngine>, RuntimeError> {
    #[cfg(test)]
    if FAIL_START.with(Cell::get) {
        return Err(RuntimeError::Invariant("a start the test fails"));
    }
    let (started, builtins) = OxideEngine::start()?;
    let started = Rc::new(started);
    // the shims call host functions as they install, whose conversions read the thread's engine
    ENGINE.with(|engine| *engine.borrow_mut() = Some(Rc::clone(&started)));
    // and the innermost, when it starts under a run of the spec runtime
    let active = Active::starting(&started);
    let finished = started.finish_start(&builtins);
    drop(active);
    if let Err(err) = finished {
        ENGINE.with(|engine| *engine.borrow_mut() = None);
        return Err(err);
    }
    Ok(started)
}

/// A runtime of its own for whole scripts, not the thread's main one: `ec`, the shims and the bare names as the main
/// one has them. The test harness's spec runtime ([`super::spec`]).
pub(super) fn start_whole() -> Result<Rc<OxideEngine>, RuntimeError> {
    let _starting = Starting::enter();
    let (started, builtins) = OxideEngine::start()?;
    let started = Rc::new(started);
    // the shims call host functions as they install, whose conversions read the innermost runtime
    let _active = Active::starting(&started);
    started.finish_start(&builtins)?;
    Ok(started)
}

/// Drop this thread's runtime and its start failure, and make the next start fail or not, for the tests.
#[cfg(test)]
pub(super) fn restart_for_tests(fail: bool) {
    ENGINE.with(|engine| *engine.borrow_mut() = None);
    START_FAILED.with(|failed| failed.set(None));
    FAIL_START.with(|failing| failing.set(fail));
}

/// This thread's runtime, started if need be, for the tests.
#[cfg(test)]
pub(super) fn start_for_tests() -> Result<Rc<OxideEngine>, Error> {
    engine()
}

/// Run the thread's runtime's pending jobs (promise reactions, `queueMicrotask`) until none is left, outside any run.
#[cfg(test)]
pub(crate) fn run_jobs_for_tests() -> Result<(), Error> {
    let engine = engine()?;
    while engine
        .runtime()
        .run_pending_job()
        .map_err(|err| Error::new(format!("a pending job: {err:?}")))?
        .executed()
    {}
    Ok(())
}

impl OxideEngine {
    /// The runtime with `ec` and the helper objects, and the names of its own globals read before the host added any
    /// (for [`Self::finish_start`]'s freeze of the host's).
    fn start() -> Result<(Self, ObjectRef), RuntimeError> {
        let options = RuntimeOptions {
            heap: HeapLimits { max_bytes: HEAP_CAP },
            min_heap: MIN_HEAP,
            growth: GROWTH,
            ..RuntimeOptions::default()
        };
        let runtime = quickjs_oxide::engine::api::core::Runtime::with_host(MoquiHost, options);
        // the start's keys and code are the same in every runtime: shared atoms of the process, until
        // [`Self::finish_start`] ends the start (a key made from run-time data later stays this runtime's)
        runtime.set_share_atoms(true)?;
        let context = runtime.context();
        // QuickJS's builtins quickjs-oxide lacks: builtins here too, not frozen host globals
        super::builtins::install(&context)?;
        // the runtime's own globals, before the host adds its own ([`FREEZE_HOST_GLOBALS_JS`])
        let names = super::compiled::eval_start(&mut context.clone(), "Object.getOwnPropertyNames(globalThis)")?;
        let builtins = match names {
            Value::Object(names) => names,
            _ => return Err(RuntimeError::Invariant("the builtin names")),
        };
        let helpers = super::compiled::eval_start(
            &mut context.clone(),
            "({set: Set, setPrototype: Set.prototype, objectPrototype: Object.prototype, \
             datePrototype: Date.prototype, getTime: Date.prototype.getTime, arrayFrom: Array.from, error: Error})",
        )?;
        let Value::Object(helpers) = helpers else {
            return Err(RuntimeError::Invariant("the helper object"));
        };
        let object = |name: &str| -> Result<ObjectRef, RuntimeError> {
            match context.get_property(&helpers, &key(name).map_err(conv_runtime)?)? {
                Value::Object(object) => Ok(object),
                _ => Err(RuntimeError::Invariant("a helper")),
            }
        };
        let function = |name: &str| -> Result<CallableRef, RuntimeError> {
            context
                .as_callable(&object(name)?)?
                .ok_or(RuntimeError::Invariant("a helper function"))
        };
        let (stack_object, context_methods) = super::context::make(&context)?;
        let helpers = context.new_object()?;
        let mut helper_objects = Vec::new();
        for name in ["CollectionUtilities", "StringUtilities", "WebUtilities"] {
            let helper = context.new_object()?;
            context.set_property(&helpers, &key(name).map_err(conv_runtime)?, Value::Object(helper.clone()))?;
            helper_objects.push((name, helper));
        }
        let protos = Protos {
            helpers,
            helper_objects,
            context: stack_object.clone(),
            context_methods,
            decimal: super::decimal::make_prototype(&context)?,
            entities: super::names::register_entities(&context)?,
            lists: context.register_exotic_class(super::bind::entities::ListValues)?,
            tools: super::bind::tool::register(&mut context.clone(), std::rc::Rc::new(EngineHooks))?,
            opaque: context.register_exotic_class(super::names::OpaqueValues)?,
            set_prototype: object("setPrototype")?,
            set: function("set")?,
            object_prototype: object("objectPrototype")?,
            date_prototype: object("datePrototype")?,
            date_get_time: function("getTime")?,
            array_from: function("arrayFrom")?,
            error: function("error")?,
        };
        let ec = context.new_object()?;
        let global = context.global_object()?;
        context.define_own_property(
            &global,
            &key("ec").map_err(conv_runtime)?,
            &OrdinaryPropertyDescriptor::data(Value::Object(ec.clone()), false, false, true),
        )?;
        super::bind::install_ec(&mut context.clone(), &ec, std::rc::Rc::new(EngineHooks))?;
        super::resource::install(&context, &ec)?;
        let extras = super::compiled::eval_start(&mut context.clone(), EC_EXTRAS_JS)?;
        let extras = match &extras {
            Value::Object(object) => context.as_callable(object)?,
            _ => None,
        }
        .ok_or(RuntimeError::Invariant("the ec extras"))?;
        context.call(&extras, Value::Undefined, &[Value::Object(ec), Value::Object(stack_object)])?;
        super::decimal::install_to_decimal(&context)?;
        super::compiled::eval_start(&mut context.clone(), TO_NUMBER_JS)?;
        counts::STARTED.fetch_add(1, Relaxed);
        let live = counts::LIVE.fetch_add(1, Relaxed) + 1;
        counts::LIVE_MAX.fetch_max(live, Relaxed);
        Ok((Self {
            runtime,
            context,
            protos,
            compiled: RefCell::new(Compiled::new()),
            run: RefCell::default(),
            entities: RefCell::new(EntityObjects::default()),
            entity_proto: std::cell::OnceCell::new(),
            list_proto: std::cell::OnceCell::new(),
            heap: Cell::new(0),
            heap_peak: Cell::new(0),
            runs: Cell::new(0),
            sampled: Cell::new(None),
            tools_wrap: std::cell::OnceCell::new(),
            web_invoker: std::cell::OnceCell::new(),
            modules: RefCell::default(),
            units: RefCell::new(crate::script::BoundedMap::with_bytes(
                super::units::MAX_UNITS,
                super::units::MAX_UNIT_BYTES,
            )),
            unit_runtimes: RefCell::default(),
            unit_depth: Cell::new(0),
            keys: RefCell::default(),
            restarting: Cell::new(false),
            ran_out: Cell::new(false),
        }, builtins))
    }

    /// The rest of the start, once the runtime is the thread's, in QuickJS's order (`runtime.rs`,
    /// `install_context`): the host table's shims over `ec`, `sri` and the helper objects (each tried; any failure
    /// fails the start), then the lockdown for small scripts, which no shim's code needs, then the bare names last, so
    /// a global a shim makes (`EdiHandler`, `__moquiWrap`) is the global object's own rather than a write to the
    /// context stack.
    fn finish_start(&self, builtins: &ObjectRef) -> Result<(), RuntimeError> {
        let finished = self.install_host(builtins);
        // from here keys come from run-time data: this runtime's own, collected as any atom
        let shared = self.runtime.set_share_atoms(false);
        finished?;
        shared?;
        declare_model_keys(&self.runtime);
        Ok(())
    }

    /// [`Self::finish_start`]'s installs.
    fn install_host(&self, builtins: &ObjectRef) -> Result<(), RuntimeError> {
        let mut context = self.context.clone();
        super::bind::install_shims(&mut context, &self.protos.helpers, std::rc::Rc::new(EngineHooks))?;
        // the host's globals and helper functions QuickJS installs outside the table, then its helper script over them
        super::bind::install_one_shim(
            &mut context,
            &self.protos.helpers,
            &crate::script::host::globals::SHIM,
            std::rc::Rc::new(EngineHooks),
        )?;
        super::compiled::eval_start(&mut context, crate::script::runtime::HELPERS_JS)?;
        let global = context.global_object()?;
        // a script names the helper objects as QuickJS's bare names answered them. Frozen below.
        for name in ["ObjectUtilities", "CollectionUtilities", "StringUtilities", "WebUtilities", "UserSecurityChecks"] {
            let name = key(name).map_err(conv_runtime)?;
            let helper = context.get_property(&self.protos.helpers, &name)?;
            context.set_property(&global, &name, helper)?;
        }
        match context.get_property(&global, &key("__moquiWrap").map_err(conv_runtime)?)? {
            Value::Object(object) => {
                let wrap = context
                    .as_callable(&object)?
                    .ok_or(RuntimeError::Invariant("__moquiWrap is not a function"))?;
                let _ = self.tools_wrap.set(wrap);
            }
            _ => return Err(RuntimeError::Invariant("the tools shim made no __moquiWrap")),
        }
        let freeze = match super::compiled::eval_start(&mut context, FREEZE_HOST_GLOBALS_JS)? {
            Value::Object(object) => context.as_callable(&object)?,
            _ => None,
        }
        .ok_or(RuntimeError::Invariant("the host globals' freeze"))?;
        context.call(&freeze, Value::Undefined, &[Value::Object(builtins.clone())])?;
        super::names::install_names(&context)
    }

    /// The runtime's realm, for a host function that lent the engine and a runner outside this file
    /// ([`super::spec`]).
    pub(super) fn context(&self) -> &Context {
        &self.context
    }

    pub(super) fn runtime(&self) -> &quickjs_oxide::engine::api::core::Runtime {
        &self.runtime
    }

    /// The compiled form of `source`, named `file_name` and kept by `hash`, through the runtime's cache.
    pub(super) fn compile(
        &self,
        source: &str,
        hash: u64,
        file_name: &str,
    ) -> Result<quickjs_oxide::engine::api::core::FunctionBytecodeRef, RuntimeError> {
        let mut cache = self.compiled.borrow_mut();
        let before = cache.compiles();
        let compiled = cache.get_or_compile(&self.context, source, hash, file_name);
        counts::COMPILED.fetch_add(cache.compiles() - before, Relaxed);
        compiled.map_err(|err| {
            let err = super::errors::compile_out_of_memory(err);
            if matches!(err, RuntimeError::OutOfMemory) {
                self.note_out_of_memory();
            }
            err
        })
    }

    /// Note that something in this outermost run ran out of heap, for the pressure check at its end.
    pub(super) fn note_out_of_memory(&self) {
        self.ran_out.set(true);
    }

    /// The value the inert module at `location` gave earlier in this outermost run, when made from the same text.
    pub(super) fn kept_module(&self, location: &str, text: &Arc<str>) -> Option<Value> {
        let modules = self.modules.borrow();
        let (kept, value) = modules.get(location)?;
        (Arc::ptr_eq(kept, text) || **kept == **text).then(|| value.clone())
    }

    /// Keep an inert module's value for the rest of the outermost run.
    pub(super) fn keep_module(&self, location: &str, text: Arc<str>, value: Value) {
        self.modules.borrow_mut().insert(location.to_string(), (text, value));
    }

    /// The script servlets' invoker, once made.
    pub(super) fn web_invoker(&self) -> &std::cell::OnceCell<CallableRef> {
        &self.web_invoker
    }

    /// The tools shim's wrapper, which gives a tools map (a map with `__wrap`) its methods back.
    pub(super) fn tools_wrap(&self) -> Option<&CallableRef> {
        self.tools_wrap.get()
    }

    /// The property key of a map key, kept for the runtime.
    pub(super) fn key(&self, name: &str) -> Result<quickjs_oxide::engine::api::core::PropertyKey, Conv> {
        if name.len() > MAX_KEY_BYTES {
            return super::convert::key(name);
        }
        if let Some(found) = self.keys.borrow().get(name) {
            return Ok(found.clone());
        }
        let made = super::convert::key(name)?;
        let mut keys = self.keys.borrow_mut();
        if keys.len() >= MAX_KEYS {
            keys.clear();
        }
        keys.insert(Box::from(name), made.clone());
        Ok(made)
    }

    /// The values of the running script, by name.
    pub(super) fn run_values(&self) -> &RefCell<super::tracked::RunValues> {
        &self.run
    }

    /// The entity value object this run made for a cell.
    pub(super) fn entity_proxy(&self, cell: &Arc<EntityCell>) -> Option<ObjectRef> {
        let entities = self.entities.borrow();
        entities.by_cell.get(&cell_key(cell)).map(|&at| entities.list[at].0.clone())
    }

    pub(super) fn keep_entity(&self, proxy: ObjectRef, cell: Arc<EntityCell>) {
        let mut entities = self.entities.borrow_mut();
        let at = entities.list.len();
        entities.by_cell.insert(cell_key(&cell), at);
        entities.list.push((proxy, cell));
    }

    /// The prototype of `class` (the entity value's or the list's), kept on the engine once made; another class's
    /// is the binder's.
    pub(super) fn entity_prototype(
        &self,
        js: &mut impl quickjs_oxide::engine::api::core::Js,
        class: &'static crate::script::host::HostClass,
    ) -> quickjs_oxide::engine::api::core::JsResult<ObjectRef> {
        let kept = if std::ptr::eq(class, &crate::script::host::entities::ENTITY_VALUE) {
            &self.entity_proto
        } else if std::ptr::eq(class, &crate::script::host::entities::LIST) {
            &self.list_proto
        } else {
            return super::bind::class_prototype(js, class);
        };
        if let Some(found) = kept.get() {
            return Ok(found.clone());
        }
        let made = super::bind::class_prototype(js, class)?;
        let _ = kept.set(made.clone());
        Ok(made)
    }

    /// Sample the reserved bytes for the stats: at the first run, then at most every [`HEAP_SAMPLE_EVERY`], looked at
    /// every [`HEAP_SAMPLE_RUNS`] runs.
    pub(super) fn sample_heap(&self) {
        let runs = self.runs.get() + 1;
        self.runs.set(runs);
        if runs % HEAP_SAMPLE_RUNS != 1 {
            return;
        }
        self.sample_heap_now();
    }

    /// Sample the reserved bytes unless the last sample is younger than [`HEAP_SAMPLE_EVERY`]: at the end of each
    /// outermost run too, so its pressure check never reads a sample older than that, however few the runs (a screen
    /// render is one outermost run around many nested ones).
    fn sample_heap_now(&self) {
        let now = std::time::Instant::now();
        if self
            .sampled
            .get()
            .is_some_and(|last| now.duration_since(last) < HEAP_SAMPLE_EVERY)
        {
            return;
        }
        self.sampled.set(Some(now));
        if let Ok(report) = self.runtime.memory_report() {
            self.note_heap(report.reserved_bytes());
        }
    }

    /// Count `now` reserved bytes as this runtime's in the stats.
    fn note_heap(&self, now: usize) {
        let before = self.heap.replace(now);
        let total = if now >= before {
            counts::HEAP.fetch_add(now - before, Relaxed) + (now - before)
        } else {
            counts::HEAP.fetch_sub(before - now, Relaxed) - (before - now)
        };
        counts::HEAP_MAX.fetch_max(total, Relaxed);
        note_runtime_heap(std::ptr::from_ref(self) as usize, now, total);
        if now > self.heap_peak.get() {
            self.heap_peak.set(now);
            note_thread_peak(now);
        }
    }

    /// Let the caches' code and objects go and collect: the transient code, the screen units' functions and the
    /// property keys. The static code stays in the runtime whatever the caches hold.
    pub(super) fn shed(&self) {
        if let Ok(mut compiled) = self.compiled.try_borrow_mut() {
            compiled.clear_transient();
        }
        if let Ok(mut units) = self.units.try_borrow_mut() {
            units.clear();
        }
        if let Ok(mut keys) = self.keys.try_borrow_mut() {
            keys.clear();
        }
        let _ = self.runtime.run_gc();
    }

    /// The reserved bytes now, sampled for the stats.
    fn reserved_now(&self) -> Option<usize> {
        let reserved = self.runtime.memory_report().ok()?.reserved_bytes();
        self.note_heap(reserved);
        Some(reserved)
    }

    /// At the end of an outermost run: the cache pressure check ([`PRESSURE_BYTES`]) on the last sample, or on a sample
    /// taken now when `sample_now` (an error whose text names `out of memory`, which may be a script's own throw or a
    /// database's message), or with the caches let go at once after the run ran out of memory. Whether the runtime is
    /// still past it with its caches gone, so is to be dropped.
    fn relieve(&self, out_of_memory: bool, sample_now: bool) -> bool {
        if !out_of_memory && !sample_now && self.heap.get() < PRESSURE_BYTES {
            return false;
        }
        if !out_of_memory {
            let _ = self.runtime.run_gc();
            if self.reserved_now().is_some_and(|reserved| reserved < PRESSURE_BYTES) {
                return false;
            }
        }
        self.shed();
        self.reserved_now().is_none_or(|reserved| reserved >= PRESSURE_BYTES)
    }
}

impl OxideEngine {
    /// At the end of an outermost run, after its pressure check: whether the runtime's reserved bytes are far past its
    /// live ones ([`FRAGMENTED_BYTES`]), so it is to be dropped and started fresh. Read from the last sample, then from a
    /// collection: at most every [`FRAGMENTED_CHECK_EVERY`] on the thread (the last collection's live bytes may be a
    /// burst's, still live then), and no drop within [`FRAGMENTED_EVERY`] of the last.
    fn fragmented(&self) -> bool {
        if self.heap.get() < FRAGMENTED_BYTES {
            return false;
        }
        let now = std::time::Instant::now();
        let recent = |cell: &std::cell::Cell<Option<std::time::Instant>>, every| {
            cell.get().is_some_and(|at| now.duration_since(at) < every)
        };
        if LAST_FRAGMENTED_DROP.with(|last| recent(last, FRAGMENTED_EVERY))
            || LAST_FRAGMENTED_CHECK.with(|last| recent(last, FRAGMENTED_CHECK_EVERY))
        {
            return false;
        }
        LAST_FRAGMENTED_CHECK.with(|last| last.set(Some(now)));
        let _ = self.runtime.run_gc();
        let Ok(report) = self.runtime.memory_report() else {
            return false;
        };
        let reserved = report.reserved_bytes();
        self.note_heap(reserved);
        if reserved < FRAGMENTED_BYTES
            || reserved * FRAGMENTED_DIVISOR < report.gc.live_bytes * FRAGMENTED_RATIO
        {
            return false;
        }
        LAST_FRAGMENTED_DROP.with(|last| last.set(Some(now)));
        true
    }
}

impl Drop for OxideEngine {
    fn drop(&mut self) {
        counts::LIVE.fetch_sub(1, Relaxed);
        counts::HEAP.fetch_sub(self.heap.get(), Relaxed);
        moqui_lib::sync::lock(&RUNTIME_HEAPS).remove(&(std::thread::current().id(), std::ptr::from_ref(self) as usize));
        // the runtime is dropped on its own thread: its thread's peak goes with it, unless one starts again there
        if !self.restarting.get() {
            moqui_lib::sync::lock(&THREAD_PEAKS).remove(&std::thread::current().id());
        }
    }
}

/// The conversions the host table's binder makes on this engine ([`super::bind::OxideHooks`]): those of
/// [`super::convert`], so a decimal is the decimal host object and an entity value the entity exotic object. A value the bindings
/// do not have yet marks the run for a fallback, as in a bare-name read.
struct EngineHooks;

impl super::bind::OxideHooks for EngineHooks {
    fn to_host(&self, scope: &mut Scope<'_>, value: &Value) -> JsResult<HostValue> {
        let Some(engine) = current() else {
            return Err(RuntimeError::Invariant("no oxide engine"));
        };
        to_host(scope, &engine, value).map_err(|conv| host_error(scope, conv))
    }

    fn to_js(&self, scope: &mut Scope<'_>, value: HostValue) -> JsResult<Value> {
        let Some(engine) = current() else {
            return Err(RuntimeError::Invariant("no oxide engine"));
        };
        super::convert::to_js(scope, &engine, &value).map_err(|conv| host_error(scope, conv))
    }

    /// The tracked values written back before a method that reads the stack (kept) or may change it (released).
    fn flush(&self, scope: &mut Scope<'_>, mode: crate::script::host::FlushBefore) -> JsResult<()> {
        use crate::script::host::FlushBefore;
        let mode = match mode {
            FlushBefore::None => return Ok(()),
            FlushBefore::Keep => Flush::Keep,
            FlushBefore::Release => Flush::Release,
        };
        super::tracked::flush(scope, mode).map_err(|conv| host_error(scope, conv))
    }
}

fn conv_runtime(conv: Conv) -> RuntimeError {
    match conv {
        Conv::Js(err) => err,
        Conv::Unsupported(what) => RuntimeError::Invariant(what),
        Conv::Error(message) => RuntimeError::Internal(message),
    }
}

/// Run a host function's `body`: a panic becomes a thrown `Error` (the runtime is not poisoned), a value the bindings
/// do not have marks the run for a fallback and throws, and a conversion error throws a plain `Error` with its message.
pub(super) fn guarded<J: Js, T>(js: &mut J, body: impl FnOnce(&mut J) -> Result<T, Conv>) -> JsResult<T> {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| body(&mut *js)));
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(conv)) => Err(host_error(js, conv)),
        Err(payload) => {
            let message = panic_message(payload);
            Err(plain(js, &message))
        }
    }
}

/// [`guarded`] for an exotic hook, whose answer is handled.
pub(super) fn guarded_hook<J: Js, T>(js: &mut J, body: impl FnOnce(&mut J) -> Result<T, Conv>) -> JsResult<Hook<T>> {
    guarded(js, body).map(Hook::Handled)
}

pub(super) fn host_error<J: Js>(scope: &mut J, conv: Conv) -> RuntimeError {
    match conv {
        Conv::Js(err) => err,
        Conv::Unsupported(reason) => {
            FALLBACK.with(|fallback| {
                if fallback.get().is_none() {
                    fallback.set(Some(reason));
                }
            });
            RuntimeError::Internal(format!("not on quickjs-oxide yet: {reason}"))
        }
        Conv::Error(message) => plain(scope, &message),
    }
}

fn plain<J: Js>(scope: &mut J, message: &str) -> RuntimeError {
    match current() {
        Some(engine) => throw_plain(scope, &engine.protos.error, message),
        None => scope.throw_error(quickjs_oxide::engine::api::core::NativeErrorKind::Type, message),
    }
}

/// Forgets what one run made (the names' objects, the entity values, a fallback mark) when it ends, however it ends.
/// A nested run (inside a host call of a running oxide script that lent the engine) sets the outer run's values aside
/// and gives them back, as QuickJS's `OuterRun` does.
pub(super) struct RunState<'a> {
    engine: &'a OxideEngine,
    outer: Option<Outer>,
}

/// The entity value objects a run made, with their cells, so one converts back to its cell; by the cell's address
/// too, which the kept cell keeps from being reused.
#[derive(Default)]
pub(super) struct EntityObjects {
    list: Vec<(ObjectRef, Arc<EntityCell>)>,
    by_cell: std::collections::HashMap<usize, usize, foldhash::fast::RandomState>,
}

fn cell_key(cell: &Arc<EntityCell>) -> usize {
    Arc::as_ptr(cell) as *const () as usize
}

/// What a nested run set aside of the run around it.
struct Outer {
    values: super::tracked::RunValues,
    /// The screen unit depth of the outer run: a render the nested run makes starts afresh, as on QuickJS.
    unit_depth: u32,
    entities: EntityObjects,
    fallback: Option<&'static str>,
}

impl<'a> RunState<'a> {
    pub(super) fn enter(engine: &'a OxideEngine, nested: bool) -> Self {
        // a run of another runtime (the spec runtime's, lent to this one's run) keeps its fallback mark too: the mark
        // is the thread's
        let outer = (nested || Active::other_run(engine)).then(|| Outer {
            values: engine.run.take(),
            unit_depth: engine.unit_depth.replace(0),
            entities: engine.entities.take(),
            fallback: FALLBACK.with(Cell::take),
        });
        FALLBACK.with(|fallback| fallback.set(None));
        Self { engine, outer }
    }
}

impl Drop for RunState<'_> {
    fn drop(&mut self) {
        match self.outer.take() {
            Some(outer) => {
                if let Ok(mut values) = self.engine.run.try_borrow_mut() {
                    *values = outer.values;
                }
                if let Ok(mut entities) = self.engine.entities.try_borrow_mut() {
                    *entities = outer.entities;
                }
                self.engine.unit_depth.set(outer.unit_depth);
                FALLBACK.with(|fallback| fallback.set(outer.fallback));
            }
            None => {
                if let Ok(mut values) = self.engine.run.try_borrow_mut() {
                    values.clear();
                }
                // the modules kept for this run go with it: the next run makes them again
                if let Ok(mut modules) = self.engine.modules.try_borrow_mut() {
                    modules.clear();
                }
                if let Ok(mut entities) = self.engine.entities.try_borrow_mut() {
                    *entities = EntityObjects::default();
                }
                FALLBACK.with(|fallback| fallback.set(None));
            }
        }
    }
}

/// This thread's oxide runtime's realm, started if need be, for the tests that define their own host functions.
#[cfg(test)]
pub(super) fn context_for_tests() -> Context {
    engine().expect("the oxide runtime starts").context.clone()
}

/// Whether this thread's oxide runtime is lent by a running host function (`Scope::lend`, the fork's F3a): a script
/// may then run on it, nested in the running one.
pub(crate) fn is_lent() -> bool {
    super::units::in_render_body() || current().is_some_and(|engine| engine.runtime.is_lent())
}

/// Run `source` on the thread's oxide runtime and give its completion value to `finish`. When the text `writes` (the
/// static check's answer, or any script under `script_engine=oxide`), its tracked values are written back at its end.
fn run<T>(
    source: &str,
    hash: u64,
    writes: bool,
    file_name: &str,
    widget: bool,
    finish: impl FnOnce(&mut Context, &OxideEngine, Value) -> Result<T, Conv>,
) -> Result<T, Outcome> {
    with_run(writes, source, |engine, context, note| {
        let compiled = match engine.compile(source, hash, file_name) {
            // nothing of the script has run: its compile is tried again once the caches went
            Err(RuntimeError::OutOfMemory) => {
                engine.shed();
                engine.compile(source, hash, file_name)
            }
            compiled => compiled,
        };
        // a small kind's expression runs, and its value converts, inside the widget check's wrapper of `ec`, as on
        // QuickJS ([`super::widget`]); a script or actions run does not
        let ran = compiled.and_then(|compiled| {
            let execute = || context.execute(&compiled).map(|value| finish(context, engine, value));
            if widget {
                super::widget::with_widget_ec(engine, None, execute)
            } else {
                execute()
            }
        });
        match ran {
            Ok(finished) => finished.map_err(|conv| {
                if let Conv::Js(err) = &conv {
                    note(err);
                }
                conv_outcome(context, conv)
            }),
            Err(err) => {
                note(&err);
                Err(Outcome::Error(runtime_error(context, err)))
            }
        }
    })
}

/// Drop the thread's runtime when it is `engine`, so the next script starts a new one: a run left it in a state the
/// later scripts must not see (the widget check's wrapper of `ec` that could not be taken off, [`super::widget`]).
/// Counted as a poisoned runtime.
/// It goes when the outermost run of it ends, as a poisoned one does, not under the runs still on the stack.
pub(super) fn discard(engine: &OxideEngine) {
    DISCARD.with(|pending| pending.set(std::ptr::from_ref(engine)));
}

thread_local! {
    /// The runtime [`discard`] marked, dropped when its outermost run ends.
    static DISCARD: Cell<*const OxideEngine> = const { Cell::new(std::ptr::null()) };
}

/// Whether `engine` was marked by [`discard`]; the mark is taken.
fn take_discard(engine: &Rc<OxideEngine>) -> bool {
    DISCARD.with(|pending| {
        let marked = std::ptr::eq(pending.get(), Rc::as_ptr(engine));
        if marked {
            pending.set(std::ptr::null());
        }
        marked
    })
}

/// One run on the thread's oxide runtime, from `body`: counted in the thread's depth, deadline and default limit, its
/// run state its own (a nested run, inside a host function that lent the engine, sets the outer run's aside and gives
/// it back, its screen unit depth too). When the run `writes`, its tracked values are written back at its end. `body`
/// notes each engine error it met, so a poisoned runtime is dropped when the outermost run ends and one short of memory
/// lets its transient code go. `what` names the run in the log.
pub(super) fn with_run<T>(
    writes: bool,
    what: &str,
    body: impl FnOnce(&OxideEngine, &mut Context, &mut dyn FnMut(&RuntimeError)) -> Result<T, Outcome>,
) -> Result<T, Outcome> {
    let depth = run::depth();
    if depth >= MAX_SCRIPT_DEPTH {
        return Err(Outcome::Error(Error::new("scripts are nested too deeply")));
    }
    // nothing of the script has run: it runs on QuickJS
    let engine = engine().map_err(|_| Outcome::Fallback(DID_NOT_START))?;
    // nested in a run of this runtime (per runtime: a run of the spec runtime is another's), which lent it or is a
    // render's Rust body with the runtime free ([`super::units::in_render_body`])
    let nested = Active::holds(&engine);
    if nested && !engine.runtime.is_lent() && !super::units::in_render_body() {
        return Err(Outcome::Fallback(
            "a run nested in a running oxide script that did not lend its engine",
        ));
    }
    run::stack_budget(depth).map_err(Outcome::Error)?;
    let _active = Active::enter(&engine);
    let _tag = EngineTag::Oxide.enter();
    let _not_starting = StartSetAside::enter();
    let _limit = (depth == 0).then(ScriptLimit::arm);
    let _depth = run::enter_depth(depth);
    let mut context = engine.context.clone();
    // A nested run sets the outer run's tracked values aside as they are, as QuickJS's nested run does
    // (`runtime.rs`, `with_nested_context`): they are written back at the outer run's next flush or its end, to its own
    // frame. Written back here, they would go to whatever frame the host call left on top (a service's context), and
    // the outer run would lose them (`setup#UserOrganizationInfo`'s `userOrgIds.push` across an entity find). The
    // binder flushes before a method that reads or changes the stack, before the call changes it.
    let state = RunState::enter(&engine, nested);
    let _real_ec = nested.then(|| super::widget::RealEcForNested::enter(&engine));
    let _render = super::units::set_aside_render_body();
    let mut poisoned = false;
    let mut out_of_memory = false;
    let mut note = |err: &RuntimeError| {
        poisoned |= matches!(err, RuntimeError::Poisoned);
        out_of_memory |= matches!(err, RuntimeError::OutOfMemory);
    };
    let mut outcome = body(&engine, &mut context, &mut note);
    if out_of_memory {
        engine.note_out_of_memory();
    }
    // what ran out inside the run without reaching its outcome (a section's error is written in the page, a nested
    // run's is its caller's to handle) is the outermost run's to relieve
    if !nested {
        out_of_memory |= engine.ran_out.take();
    }
    // a script that caught the engine's `out of memory` and threw it on, or a screen unit's error, may have run out
    // too; the text alone may also be a script's own throw or a database's message, so it only samples the heap now
    let names_out_of_memory = !out_of_memory
        && matches!(&outcome, Err(Outcome::Error(err)) if err.message().contains("out of memory"));
    if FALLBACK.with(Cell::get).is_none() && !poisoned {
        if writes {
            // as QuickJS does at a script's end, whether it ended well or not
            let flushed = super::tracked::flush(&mut context, Flush::End);
            if let (Ok(_), Err(conv)) = (&outcome, flushed) {
                outcome = Err(conv_outcome(&mut context, conv));
            }
        } else if cfg!(debug_assertions) {
            check_kept_names(&mut context, &engine, what);
        }
    }
    let fallback = FALLBACK.with(Cell::take);
    drop(state);
    // a runtime a run left as the later scripts must not see goes when its outermost run ends, its peak kept
    if !nested && take_discard(&engine) {
        counts::POISONED.fetch_add(1, Relaxed);
        tracing::error!(script = %what, "the quickjs-oxide runtime was left with the widget check's wrapper of ec; it is dropped and restarts on the next script");
        engine.restarting.set(true);
        ENGINE.with(|current| {
            let mut current = current.borrow_mut();
            if current.as_ref().is_some_and(|held| Rc::ptr_eq(held, &engine)) {
                *current = None;
            }
        });
    }
    // a poisoned runtime goes when the outermost oxide run ends, which sees the poison too
    if poisoned && !nested {
        counts::POISONED.fetch_add(1, Relaxed);
        tracing::error!(script = %what, "a host function panicked in quickjs-oxide; the runtime is dropped and restarts on the next script");
        ENGINE.with(|current| *current.borrow_mut() = None);
    } else if !poisoned {
        if out_of_memory && nested {
            // the caches go, so the next script on the thread has room; this one fails alone (the outermost run's
            // pressure check below does the same)
            engine.shed();
        }
        engine.sample_heap();
        if !nested {
            engine.sample_heap_now();
        }
        // the pressure check of the outermost run: a runtime its caches' going leaves past it starts again
        // (not while a runtime's start is on the stack: its shims' host calls run scripts on it)
        let outermost = !nested && !Active::starting_on_stack();
        let pressed = outermost && engine.relieve(out_of_memory, names_out_of_memory);
        // a runtime its peak left far larger than what lives in it starts fresh, its arenas given back whole
        let fragmented = outermost && !pressed && engine.fragmented();
        if pressed || fragmented {
            let dropped = ENGINE.with(|current| {
                let mut current = current.borrow_mut();
                let held = current.as_ref().is_some_and(|held| Rc::ptr_eq(held, &engine));
                if held {
                    *current = None;
                }
                held
            });
            if dropped && pressed {
                tracing::info!(script = %what, "the quickjs-oxide runtime's heap stays near its cap with its caches gone; it is dropped and restarts on the next script");
                counts::RESTARTED.fetch_add(1, Relaxed);
                engine.restarting.set(true);
            } else if dropped {
                tracing::debug!(script = %what, "the quickjs-oxide runtime's reserved heap is far past its live bytes; it is dropped and starts fresh on the next script");
                counts::SHED_FRAGMENTED.fetch_add(1, Relaxed);
                engine.restarting.set(true);
            }
        }
    }
    match fallback {
        Some(reason) => Err(Outcome::Fallback(reason)),
        None => outcome,
    }
}

/// The safety net behind the static check's answer that a text does not write, in debug and test builds, for a run
/// that skips the flush at its end: a list or map it read by name converts back to what the stack holds, or the run
/// changed one and the change is lost. A difference is logged with the script, and counted for the tests
/// ([`changed_copies`]).
fn check_kept_names(context: &mut Context, engine: &OxideEngine, source: &str) {
    let kept = engine.run.borrow().tracked();
    for (name, value) in kept {
        let held = crate::script::run::STACK.with(|stack| stack.borrow().with_value(&name, HostValue::clone));
        let Some(held) = held.filter(|held| matches!(held, HostValue::List(_) | HostValue::Map(_))) else {
            continue;
        };
        // a tools map by the stack's own keys, as QuickJS writes it back ([`super::tracked::kept_matches`])
        if let Ok(false) = super::tracked::kept_matches(context, engine, &value, &held, 0) {
            CHANGED_COPIES.with(|changed| changed.set(changed.get() + 1));
            tracing::error!(name = %name, script = %source, "a quickjs-oxide expression changed its copy of a context value; QuickJS would have written it back");
        }
    }
}

thread_local! {
    static CHANGED_COPIES: Cell<u64> = const { Cell::new(0) };
}

/// Runs on this thread whose copy of a context value changed ([`check_kept_names`]).
#[cfg(test)]
pub(super) fn changed_copies() -> u64 {
    CHANGED_COPIES.with(Cell::get)
}

fn conv_outcome(context: &mut Context, conv: Conv) -> Outcome {
    match conv {
        Conv::Js(err) => Outcome::Error(runtime_error(context, err)),
        Conv::Unsupported(reason) => Outcome::Fallback(reason),
        Conv::Error(message) => Outcome::Error(Error::new(message)),
    }
}

/// [`crate::script::eval_condition`] on quickjs-oxide: `source` (as it runs, after `as_expression`) is true by
/// JavaScript's truth.
pub(crate) fn eval_condition(source: &str, hash: u64, writes: bool) -> Result<bool, Outcome> {
    run(source, hash, writes, super::compiled::FILE_NAME, true, |_, _, value| {
        Ok(truthy(&value))
    })
}

/// A live-form template function's call (`super::templates::run_live`) on quickjs-oxide, its value the text: as
/// [`eval_text`], but not a widget expression, as QuickJS's `run_template_function` runs it.
pub(super) fn eval_template(source: &str, hash: u64, writes: bool) -> Result<String, Outcome> {
    run(
        source,
        hash,
        writes,
        super::compiled::FILE_NAME,
        false,
        |context, _, value| match value {
            Value::String(text) => Ok(text.to_utf8_lossy()),
            other => Ok(context.to_js_string(&other)?.to_utf8_lossy()),
        },
    )
}

/// [`crate::script::expand`] on quickjs-oxide: `source` is the template as a template literal, its value the text.
pub(crate) fn eval_text(source: &str, hash: u64, writes: bool) -> Result<String, Outcome> {
    run(
        source,
        hash,
        writes,
        super::compiled::FILE_NAME,
        true,
        |context, _, value| match value {
            Value::String(text) => Ok(text.to_utf8_lossy()),
            other => Ok(context.to_js_string(&other)?.to_utf8_lossy()),
        },
    )
}

/// [`crate::script::eval_map`] on quickjs-oxide: the value of `source`, an object, as a map. Anything else is the
/// QuickJS path's error.
pub(crate) fn eval_map(
    source: &str,
    hash: u64,
    writes: bool,
) -> Result<std::collections::BTreeMap<String, HostValue>, Outcome> {
    run(
        source,
        hash,
        writes,
        super::compiled::FILE_NAME,
        true,
        |context, engine, value| {
            let not_an_object = || Conv::Error("script value is not an object".to_string());
            let Value::Object(object) = &value else {
                return Err(not_an_object());
            };
            if context.is_array_object(object)? || context.as_callable(object)?.is_some() {
                return Err(not_an_object());
            }
            match to_host(context, engine, &value)? {
                HostValue::Map(map) => Ok(map.into_iter().collect()),
                _ => Err(not_an_object()),
            }
        },
    )
}

/// A generated actions script on quickjs-oxide, named `file_name` in stack traces as on QuickJS. Not strict, as there:
/// a bare assignment is how a value reaches the context stack. Kept by the hash of its text and name.
pub(crate) fn run_actions(source: &str, hash: u64, file_name: &str) -> Result<(), Outcome> {
    let key = crate::script::text::hash_of(hash, file_name);
    run(source, key, true, file_name, false, |_, _, _| Ok(()))
}

/// [`crate::script::run_script`] on quickjs-oxide, under `script_engine=oxide`: one script, not strict, named
/// `eval_script` in stack traces as on QuickJS; what it wrote by name is written back at its end.
pub(crate) fn run_script(source: &str, hash: u64) -> Result<(), Outcome> {
    run(source, hash, true, super::compiled::FILE_NAME, false, |_, _, _| Ok(()))
}

/// [`crate::script::eval_value`] on quickjs-oxide: the value of `source` as a host value.
pub(crate) fn eval_value(source: &str, hash: u64, writes: bool) -> Result<HostValue, Outcome> {
    run(
        source,
        hash,
        writes,
        super::compiled::FILE_NAME,
        true,
        |context, engine, value| to_host(context, engine, &value),
    )
}
