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

//! What a script run on this thread shares with every other, whichever engine runs it (plans/Script-Engine-Oxide.md
//! section 4): the context stack, the nesting depth, the deadline and the default limit, the stack budget, and the
//! engine of each running script. QuickJS (`script/runtime.rs`) and quickjs-oxide (`script/oxide/`) both count here,
//! so a script nested in a script of the other engine is limited as one nested in its own.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use moqui_lib::context_stack::ContextStack;

use crate::context::Error;

thread_local! {
    pub(crate) static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Whether `DEADLINE` is the default limit of a script that runs outside a transaction (see [`ScriptLimit`]).
    pub(crate) static DEADLINE_IS_DEFAULT: Cell<bool> = const { Cell::new(false) };
    /// The default limit of a script outside a transaction, read at the first script on the thread.
    pub(crate) static SCRIPT_LIMIT: Cell<Option<Option<Duration>>> = const { Cell::new(None) };
    /// Whether this thread is a request worker, whose scripts outside a transaction are limited by default.
    pub(crate) static REQUEST_THREAD: Cell<bool> = const { Cell::new(false) };
    /// Where the stack was when the outermost script on the thread was entered.
    pub(crate) static STACK_BASE: Cell<usize> = const { Cell::new(0) };
    /// The context stack both engines read and write bare names through.
    pub(crate) static STACK: RefCell<ContextStack> = RefCell::new(ContextStack::new());
    /// How deep scripts are nested on this thread, over both engines.
    pub(crate) static DEPTH: Cell<usize> = const { Cell::new(0) };
    /// The engine of each running script, innermost last.
    static RUNNING: RefCell<Vec<EngineTag>> = const { RefCell::new(Vec::new()) };
}

/// Which engine a running script is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EngineTag {
    Oxide,
}

/// A script of `tag` running, from [`EngineTag::enter`] until dropped, also by an error or a panic.
pub(crate) struct Running(());

impl EngineTag {
    pub(crate) fn enter(self) -> Running {
        RUNNING.with(|running| running.borrow_mut().push(self));
        Running(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.with(|running| {
            running.borrow_mut().pop();
        });
    }
}

/// Whether a script of `tag` is running on this thread, at any depth.
pub(crate) fn is_running(tag: EngineTag) -> bool {
    RUNNING.with(|running| running.borrow().contains(&tag))
}

/// The nesting depth of scripts on this thread now.
pub(crate) fn depth() -> usize {
    DEPTH.with(Cell::get)
}

/// One level of nesting entered: the depth goes up now and back down when the guard drops. The deepest nesting of the
/// process is counted for [`crate::script::engine_stats`]. When the outermost script on the thread ends, on either
/// engine, what its scripts held to the end goes: the response bodies opened with `send()` and not read or forwarded
/// ([`crate::web::upstream::clear`]), and the state of quickjs-oxide's resource objects
/// ([`crate::script::oxide::bind::release_run_objects`]), which a tracing collector would keep until it ran, unless a
/// [`ReleaseScope`] is open.
pub(crate) fn enter_depth(depth: usize) -> DepthGuard {
    DEPTH.with(|current| current.set(depth + 1));
    DEPTH_MAX.fetch_max(depth + 1, std::sync::atomic::Ordering::Relaxed);
    DepthGuard
}

/// The deepest nesting of scripts any thread of the process reached.
pub(crate) static DEPTH_MAX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The stack the outermost script on a thread may use, below the 2 MiB of a service worker. QuickJS counts the engine's
/// stack from where the outermost script enters it, so the scripts it nests share this budget plus [`NESTED_STACK`]
/// (on a request worker, [`REQUEST_SCRIPT_STACK`]). Less than [`MIN_NESTED_STACK`] left fails before a nested script.
pub(crate) const SCRIPT_STACK: usize = 1024 * 1024;
/// Full app renders nest widget templates, includes and services on the request worker's 8 MiB stack.
pub(crate) const REQUEST_SCRIPT_STACK: usize = 4 * 1024 * 1024;
pub(crate) const NESTED_STACK: usize = 256 * 1024;
pub(crate) const MIN_NESTED_STACK: usize = 32 * 1024;

/// Bounds script time on this thread to a transaction's remaining time.
///
/// The QuickJS interrupt handler and quickjs-oxide's `Host::interrupt` read the deadline, so a loop that never returns
/// ends with an error instead of holding the worker. Nested guards keep the earlier deadline. A transaction's limit
/// replaces the default limit of a script that runs outside one, and the time inside does not count against that default.
pub struct DeadlineGuard {
    previous: Option<Instant>,
    previous_default: bool,
    /// When the guard replaced a default limit, so the default is moved on by the time spent inside.
    replaced_default: Option<Instant>,
}

impl DeadlineGuard {
    pub fn new(remaining: Option<Duration>) -> Self {
        let previous = DEADLINE.with(Cell::get);
        let previous_default = DEADLINE_IS_DEFAULT.with(Cell::get);
        let mut replaced_default = None;
        if let Some(remaining) = remaining {
            let now = Instant::now();
            let next = now + remaining;
            let chosen = match previous {
                Some(existing) if !previous_default => existing.min(next),
                Some(_) => {
                    replaced_default = Some(now);
                    next
                }
                None => next,
            };
            DEADLINE.with(|deadline| deadline.set(Some(chosen)));
            DEADLINE_IS_DEFAULT.with(|flag| flag.set(false));
        }
        Self {
            previous,
            previous_default,
            replaced_default,
        }
    }
}

impl Drop for DeadlineGuard {
    fn drop(&mut self) {
        let previous = match (self.previous, self.replaced_default) {
            (Some(at), Some(entered)) => Some(at + entered.elapsed()),
            (previous, _) => previous,
        };
        DEADLINE.with(|deadline| deadline.set(previous));
        DEADLINE_IS_DEFAULT.with(|flag| flag.set(self.previous_default));
    }
}

/// The time left before this thread's script deadline, when it has one.
pub(crate) fn script_time_remaining() -> Option<Duration> {
    DEADLINE
        .with(Cell::get)
        .map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

/// The default of `webapp_request_timeout`, the host's limit on a whole request, read here by the same name.
const DEFAULT_REQUEST_TIMEOUT: u64 = 300;

/// Mark this thread a request worker: a script on it that runs outside a transaction is limited to the request
/// timeout unless `resource-facade` `script-timeout` says otherwise.
pub(crate) fn mark_request_thread() {
    REQUEST_THREAD.with(|flag| flag.set(true));
    SCRIPT_LIMIT.with(|cell| cell.set(None));
}

/// The seconds a script may run when no transaction limits it, read once per thread. The conf's `resource-facade`
/// `script-timeout` when set, for every thread; else the webapp's `requests` `request-timeout`
/// (`webapp_request_timeout`) on a request worker, and no limit on a service, job, or scheduler thread, which run batch
/// work outside a transaction as Java does. 0 is no limit.
pub(crate) fn default_script_limit() -> Option<Duration> {
    if let Some(limit) = SCRIPT_LIMIT.with(Cell::get) {
        return limit;
    }
    let factory = crate::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()));
    let limit = script_limit_for(
        &crate::resource::setting("script-timeout"),
        REQUEST_THREAD.with(Cell::get),
        &crate::web::protection_setting(
            factory.as_deref(),
            "requests",
            "request-timeout",
            "webapp_request_timeout",
        ),
    );
    SCRIPT_LIMIT.with(|cell| cell.set(Some(limit)));
    limit
}

pub(crate) fn script_limit_for(script_timeout: &str, request_thread: bool, request_timeout: &str) -> Option<Duration> {
    let seconds = match script_timeout.trim().parse::<u64>() {
        Ok(seconds) => seconds,
        Err(_) if request_thread => request_timeout.trim().parse().unwrap_or(DEFAULT_REQUEST_TIMEOUT),
        Err(_) => 0,
    };
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// The default deadline of the outermost script on a thread when nothing set one (see [`default_script_limit`]), so a
/// loop in a script that runs outside a transaction (a widget condition, a `transaction="ignore"` service a request
/// calls) cannot hold a request worker for good. Cleared when the script ends.
pub(crate) struct ScriptLimit {
    armed: bool,
}

impl ScriptLimit {
    pub(crate) fn arm() -> Self {
        let armed = DEADLINE.with(Cell::get).is_none()
            && default_script_limit().is_some_and(|limit| {
                DEADLINE.with(|deadline| deadline.set(Some(Instant::now() + limit)));
                DEADLINE_IS_DEFAULT.with(|flag| flag.set(true));
                true
            });
        Self { armed }
    }
}

impl Drop for ScriptLimit {
    fn drop(&mut self) {
        if self.armed {
            DEADLINE.with(|deadline| deadline.set(None));
            DEADLINE_IS_DEFAULT.with(|flag| flag.set(false));
        }
    }
}

/// An address on the current stack.
fn stack_address() -> usize {
    let marker = 0_u8;
    std::hint::black_box(std::ptr::from_ref(&marker)) as usize
}

/// The engine's stack limit when the outermost script enters it (`depth` 0), or for a nested script what is left of
/// it, an error when too little is.
pub(crate) fn stack_budget(depth: usize) -> Result<usize, Error> {
    let here = stack_address();
    let (budget, ceiling) = if REQUEST_THREAD.with(Cell::get) {
        (REQUEST_SCRIPT_STACK, REQUEST_SCRIPT_STACK)
    } else {
        (SCRIPT_STACK, NESTED_STACK)
    };
    if depth == 0 {
        STACK_BASE.with(|base| base.set(here));
        // the one runtime's limit, from here: this script and the scripts it nests
        return Ok(if REQUEST_THREAD.with(Cell::get) {
            budget
        } else {
            budget + NESTED_STACK
        });
    }
    let used = STACK_BASE.with(Cell::get).abs_diff(here);
    let left = (budget + NESTED_STACK).saturating_sub(used).min(ceiling);
    if left < MIN_NESTED_STACK {
        return Err(Error::new("scripts are nested too deeply for the thread's stack"));
    }
    Ok(left)
}

/// A script that runs a service that runs a script runs in the same engine ([`with_nested_context`]). A chain of Mantle
/// services from a shipment delivery to a ledger posting nests about 20 deep; the stack budget ([`stack_budget`]) is
/// what stops runaway recursion, so this only bounds the nesting.
pub(crate) const MAX_SCRIPT_DEPTH: usize = 64;

pub(crate) struct DepthGuard;

impl Drop for DepthGuard {
    fn drop(&mut self) {
        let left = DEPTH.with(|depth| {
            let left = depth.get().saturating_sub(1);
            depth.set(left);
            left
        });
        if left == 0 {
            crate::web::upstream::clear();
            if RELEASE_SCOPES.with(Cell::get) == 0 {
                crate::script::oxide::bind::release_run_objects();
            }
        }
    }
}

thread_local! {
    /// How many [`ReleaseScope`]s are open on this thread.
    static RELEASE_SCOPES: Cell<usize> = const { Cell::new(0) };
}

/// A span of several outermost scripts that share their objects, such as a spec's registration, `setupSpec`, features
/// and `cleanupSpec`, each its own run on a runtime that lasts: while one is open the end of an outermost script does
/// not let quickjs-oxide's resource objects go ([`crate::script::oxide::bind::release_run_objects`]); the last scope's
/// end does, once no script is running. Only that release waits: each script's own end (its write-back, the heap's
/// sample, its deadline, the response bodies it left unread) stays its own.
pub struct ReleaseScope(());

impl ReleaseScope {
    #[must_use]
    pub fn enter() -> Self {
        RELEASE_SCOPES.with(|scopes| scopes.set(scopes.get() + 1));
        Self(())
    }
}

impl Drop for ReleaseScope {
    fn drop(&mut self) {
        let left = RELEASE_SCOPES.with(|scopes| {
            let left = scopes.get().saturating_sub(1);
            scopes.set(left);
            left
        });
        if left == 0 && depth() == 0 {
            crate::script::oxide::bind::release_run_objects();
        }
    }
}

/// Whether a script is running on this thread.
pub(crate) fn in_script() -> bool {
    DEPTH.with(Cell::get) > 0
}
