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

//! The seam between the two script engines (plans/Script-Engine-Oxide.md sections 2 to 4): the `script_engine`
//! setting, the route of each script entry, and the boundary an oxide script crosses when it starts inside a running
//! QuickJS script. The router is the only code that knows there are two engines; each entry asks it once, before the
//! script starts, and the route is never changed after (a fallback is taken only when the oxide run reached no host
//! call, so no side effect happened twice).

use std::cell::Cell;
use std::sync::OnceLock;

use crate::context::Error;
use crate::script::check::{self, Profile};
use crate::script::run::{self, EngineTag};

/// Which engine runs what (the `script_engine` property or environment variable, read once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptEngine {
    /// Everything on QuickJS, as before quickjs-oxide.
    Rquickjs,
    /// The small kinds the oxide bindings cover on quickjs-oxide, when their text passes the static check; everything
    /// else on QuickJS.
    SmallOxide,
    /// Everything on quickjs-oxide. A kind the oxide bindings do not cover yet fails with an error naming the kind.
    Oxide,
}

impl ScriptEngine {
    /// The setting's value as written (`rquickjs`, `small-oxide`, `oxide`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Rquickjs => "rquickjs",
            Self::SmallOxide => "small-oxide",
            Self::Oxide => "oxide",
        }
    }

    /// The setting for its value; `None` for any other text.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "rquickjs" => Some(Self::Rquickjs),
            "small-oxide" => Some(Self::SmallOxide),
            "oxide" => Some(Self::Oxide),
            _ => None,
        }
    }
}

thread_local! {
    /// The setting for this thread when a test or a benchmark set one ([`set_thread_script_engine`]).
    static THREAD_ENGINE: Cell<Option<ScriptEngine>> = const { Cell::new(None) };
}

/// The `script_engine` setting of the process, read once. Empty or unknown is `rquickjs`, the default until Phase 2's
/// gate (plans/Script-Engine-Oxide.md section 7); an unknown value is logged.
fn process_engine() -> ScriptEngine {
    static SETTING: OnceLock<ScriptEngine> = OnceLock::new();
    *SETTING.get_or_init(|| {
        let text = moqui_lib::env::get_prop_or_env("script_engine");
        if text.trim().is_empty() {
            return ScriptEngine::Rquickjs;
        }
        ScriptEngine::parse(&text).unwrap_or_else(|| {
            tracing::error!(value = %text, "script_engine is not rquickjs, small-oxide or oxide; using rquickjs");
            ScriptEngine::Rquickjs
        })
    })
}

/// The setting scripts on this thread run under: the thread's own when one was set, else the process's.
pub fn script_engine() -> ScriptEngine {
    THREAD_ENGINE.with(Cell::get).unwrap_or_else(process_engine)
}

/// Set the engine setting for this thread only (`None` goes back to the process's), returning the one it replaces: for
/// the tests that run one body under each engine and for `js_speed`. A running script keeps the route it started on.
pub fn set_thread_script_engine(engine: Option<ScriptEngine>) -> Option<ScriptEngine> {
    THREAD_ENGINE.with(|cell| cell.replace(engine))
}

/// Run `body` with this thread's setting at `engine`, put back after, also by a panic.
pub fn with_script_engine<R>(engine: ScriptEngine, body: impl FnOnce() -> R) -> R {
    struct Restore(Option<ScriptEngine>);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_thread_script_engine(self.0);
        }
    }
    let _restore = Restore(set_thread_script_engine(Some(engine)));
    body()
}

/// Run `body` once under `rquickjs` and once under `small-oxide` (plans/Script-Engine-Oxide.md section 8): the binding
/// tests check both engines from one body. The engine is named in a failure's message.
#[cfg(test)]
pub(crate) fn on_both_engines(mut body: impl FnMut(ScriptEngine) -> Result<(), Error>) -> Result<(), Error> {
    for engine in [ScriptEngine::Rquickjs, ScriptEngine::SmallOxide] {
        with_script_engine(engine, || body(engine)).map_err(|err| {
            let message = format!("under {}: {}", engine.name(), err.message());
            err.with_message(message)
        })?;
    }
    Ok(())
}

/// The entry point a script comes in by (plans/Script-Engine-Oxide.md section 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntryKind {
    Condition,
    Value,
    FieldValue,
    Expand,
    Map,
    Actions,
    Script,
    Template,
    Spec,
    /// A host's own use of the engine (`with_ctx`, a screen unit, a web handler).
    Host,
}

impl EntryKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Condition => "eval_condition",
            Self::Value => "eval_value",
            Self::FieldValue => "eval_field_value",
            Self::Expand => "expand",
            Self::Map => "eval_map",
            Self::Actions => "run_actions",
            Self::Script => "run_script",
            Self::Template => "template",
            Self::Spec => "spec",
            Self::Host => "host",
        }
    }

    /// Every kind, for the per-kind counts.
    pub const ALL: [EntryKind; 10] = [
        Self::Condition,
        Self::Value,
        Self::FieldValue,
        Self::Expand,
        Self::Map,
        Self::Actions,
        Self::Script,
        Self::Template,
        Self::Spec,
        Self::Host,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0)
    }

    /// Whether the oxide bindings run this kind yet (Phase 1: conditions and values).
    pub(crate) fn oxide_covers(self) -> bool {
        matches!(self, Self::Condition | Self::Value | Self::FieldValue)
    }

    /// The small kinds the census records (plans/Script-Engine-Oxide.md section 5.3).
    pub(crate) fn is_small(self) -> bool {
        matches!(
            self,
            Self::Condition | Self::Value | Self::FieldValue | Self::Expand | Self::Map
        )
    }
}

/// Where one script runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    QuickJs,
    Oxide,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Self::QuickJs => "quickjs",
            Self::Oxide => "oxide",
        }
    }
}

/// Counts of scripts by entry kind and engine, one table per thread so a script's count is a write no other thread
/// makes: each thread's table is registered once and [`scripts_by_kind`] sums them.
mod counts {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub(super) struct Counts {
        pub(super) quickjs: [AtomicU64; 10],
        pub(super) oxide: [AtomicU64; 10],
        /// Oxide runs that met something the bindings do not have before any host call, and ran on QuickJS instead.
        pub(super) fallbacks: AtomicU64,
        /// Runs routed to QuickJS because their text fell back too often on this thread.
        pub(super) memo: AtomicU64,
    }

    pub(super) static ALL: Mutex<Vec<Arc<Counts>>> = Mutex::new(Vec::new());

    thread_local! {
        static MINE: Arc<Counts> = {
            let mine = Arc::new(Counts::default());
            moqui_lib::sync::lock(&ALL).push(Arc::clone(&mine));
            mine
        };
    }

    /// Add one to the counter `pick` gives of this thread's table.
    pub(super) fn bump(pick: impl FnOnce(&Counts) -> &AtomicU64) {
        MINE.with(|mine| {
            let counter = pick(mine);
            counter.store(counter.load(Relaxed) + 1, Relaxed);
        });
    }

    /// The sum over every thread's table of the counter `pick` gives.
    pub(super) fn sum(pick: impl Fn(&Counts) -> &AtomicU64) -> u64 {
        moqui_lib::sync::lock(&ALL)
            .iter()
            .map(|counts| pick(counts).load(Relaxed))
            .sum()
    }
}

/// Scripts run by entry kind since the process started: (kind, on QuickJS, on quickjs-oxide).
pub fn scripts_by_kind() -> Vec<(&'static str, u64, u64)> {
    EntryKind::ALL
        .iter()
        .map(|kind| {
            let index = kind.index();
            (
                kind.name(),
                counts::sum(|counts| &counts.quickjs[index]),
                counts::sum(|counts| &counts.oxide[index]),
            )
        })
        .collect()
}

/// Oxide runs that went to QuickJS before any host call, since the process started.
pub fn oxide_fallbacks() -> u64 {
    counts::sum(|counts| &counts.fallbacks)
}

/// Runs sent to QuickJS without trying oxide because their text had fallen back [`MAX_FALLBACKS`] times on the thread.
pub fn oxide_fallback_memo_runs() -> u64 {
    counts::sum(|counts| &counts.memo)
}

/// Count a script of `kind` that ran on `route`.
pub(crate) fn count(kind: EntryKind, route: Route) {
    let index = kind.index();
    match route {
        Route::QuickJs => counts::bump(|counts| &counts.quickjs[index]),
        Route::Oxide => counts::bump(|counts| &counts.oxide[index]),
    }
}

/// Fallbacks of one text on a thread after which it goes straight to QuickJS: a fallback runs the text twice.
const MAX_FALLBACKS: u8 = 2;
/// Texts the fallback memo of a thread keeps; past it the memo starts again.
const MAX_FALLBACK_TEXTS: usize = 4096;

thread_local! {
    /// The fallbacks of each text on this thread, by kind and text hash.
    static FALLEN_BACK: std::cell::RefCell<std::collections::HashMap<(EntryKind, u64), u8>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Whether the text of `kind` and `hash` fell back [`MAX_FALLBACKS`] times on this thread.
fn fell_back_often(kind: EntryKind, hash: u64) -> bool {
    FALLEN_BACK.with(|memo| {
        memo.borrow()
            .get(&(kind, hash))
            .is_some_and(|count| *count >= MAX_FALLBACKS)
    })
}

/// Whether an `ec` path is bound on both engines, so a text that names it may run on quickjs-oxide: the host table's
/// registry ([`crate::script::host::path_allowed`]).
pub(crate) fn ec_path_allowed(path: &str) -> bool {
    crate::script::host::path_allowed(path)
}

/// Where a script of `kind` with `text` (as it will run, after [`crate::script::runtime`]'s `as_expression`) runs,
/// fixed before it starts. In `small-oxide` a covered kind goes to quickjs-oxide when its text passes the static check
/// and no oxide script is running on the thread already (a nested run would re-enter the runtime, which the fork's
/// re-entry handle F3a will allow); in `oxide` a kind the bindings do not cover is an error naming it. The census
/// ([`crate::script::census`]) records the route and the check's verdict of each small text when it is on.
pub(crate) fn route(kind: EntryKind, text: &str, hash: u64) -> Result<Route, Error> {
    let setting = script_engine();
    let census = crate::script::census::enabled() && kind.is_small();
    if setting == ScriptEngine::Rquickjs && !census {
        return Ok(Route::QuickJs);
    }
    let verdict = (census || (kind.oxide_covers() && setting == ScriptEngine::SmallOxide))
        .then(|| check::verdict(text, hash, Profile::Expression, &ec_path_allowed));
    let memo = kind.oxide_covers() && setting == ScriptEngine::SmallOxide && fell_back_often(kind, hash);
    let (route, why) = choose(setting, kind, verdict.as_ref(), memo)?;
    if memo {
        counts::bump(|counts| &counts.memo);
    }
    if census {
        if let Some(verdict) = &verdict {
            crate::script::census::record(kind, text, hash, route, verdict, why);
        }
    }
    Ok(route)
}

/// The route for `setting` and `kind`, and when it is QuickJS for a kind oxide covers, why.
fn choose(
    setting: ScriptEngine,
    kind: EntryKind,
    verdict: Option<&check::Verdict>,
    memo: bool,
) -> Result<(Route, Option<&'static str>), Error> {
    let nested = run::is_running(EngineTag::Oxide);
    let widget = crate::script::runtime::widget_wrap_on();
    match setting {
        ScriptEngine::Rquickjs => Ok((Route::QuickJs, Some("setting"))),
        ScriptEngine::SmallOxide => {
            if !kind.oxide_covers() {
                return Ok((Route::QuickJs, Some("kind")));
            }
            if widget {
                return Ok((Route::QuickJs, Some("widget check")));
            }
            if nested {
                return Ok((Route::QuickJs, Some("nested in oxide")));
            }
            if memo {
                return Ok((Route::QuickJs, Some("fell back before")));
            }
            match verdict {
                Some(verdict) if verdict.passes() => Ok((Route::Oxide, None)),
                _ => Ok((Route::QuickJs, Some("check"))),
            }
        }
        ScriptEngine::Oxide => {
            if !kind.oxide_covers() {
                return Err(Error::new(format!(
                    "script_engine=oxide: {} scripts do not run on quickjs-oxide yet",
                    kind.name()
                )));
            }
            if widget {
                return Err(Error::new(format!(
                    "script_engine=oxide: {} under the server-static widget check does not run on quickjs-oxide yet",
                    kind.name()
                )));
            }
            if nested {
                return Err(Error::new(format!(
                    "script_engine=oxide: a {} nested in a running quickjs-oxide script needs the re-entry handle (F3a)",
                    kind.name()
                )));
            }
            Ok((Route::Oxide, None))
        }
    }
}

/// For a script kind that only QuickJS runs: under `oxide` an error naming the kind, else nothing. Called where such a
/// script enters QuickJS, so a test run under `script_engine=oxide` shows what is left.
pub(crate) fn require_quickjs(kind: EntryKind) -> Result<(), Error> {
    if script_engine() == ScriptEngine::Oxide && !run::is_running(EngineTag::QuickJs) {
        return Err(Error::new(format!(
            "script_engine=oxide: {} scripts do not run on quickjs-oxide yet",
            kind.name()
        )));
    }
    count(kind, Route::QuickJs);
    Ok(())
}

/// Run a script on quickjs-oxide from inside or outside a running QuickJS script: QuickJS's tracked values are written
/// back first, kept tracked (`Flush::Keep`, for a read-only small script), so the oxide script reads by bare name what
/// the QuickJS script changed in place (plans/Script-Engine-Oxide.md section 4).
pub(crate) fn cross_into_oxide<T>(body: impl FnOnce() -> T) -> Result<T, Error> {
    if run::innermost() == Some(EngineTag::QuickJs) {
        crate::script::runtime::flush_innermost_keep()?;
    }
    Ok(body())
}

/// A small script of `kind` with `source` on quickjs-oxide when [`route`] sends it there: `Some` of its result, or
/// `None` when it runs on QuickJS instead (the route, or a fallback before any host call). A fallback after a host call
/// is an error, since running the script again would repeat what that call did; so is any fallback under
/// `script_engine=oxide`.
pub(crate) fn try_oxide<T>(
    kind: EntryKind,
    source: &str,
    run: impl FnOnce(&str, u64) -> Result<T, crate::script::oxide::Outcome>,
) -> Result<Option<T>, Error> {
    use crate::script::oxide::Outcome;
    // the default: no hash, no check, no census
    if script_engine() == ScriptEngine::Rquickjs && !crate::script::census::enabled() {
        return Ok(None);
    }
    let hash = crate::script::text::hash_text(source);
    if route(kind, source, hash)? != Route::Oxide {
        return Ok(None);
    }
    let _census = crate::script::census::begin(kind, hash);
    let calls = crate::script::census::host_calls();
    match cross_into_oxide(|| run(source, hash))? {
        Ok(value) => {
            count(kind, Route::Oxide);
            Ok(Some(value))
        }
        Err(Outcome::Error(err)) => {
            count(kind, Route::Oxide);
            Err(err)
        }
        Err(Outcome::Fallback(reason)) => {
            if script_engine() == ScriptEngine::Oxide {
                return Err(Error::new(format!(
                    "script_engine=oxide: this {} needs {reason}, which the quickjs-oxide bindings do not have yet",
                    kind.name()
                )));
            }
            if crate::script::census::host_calls() != calls {
                return Err(Error::new(format!(
                    "quickjs-oxide could not finish this {} after a host call: it needs {reason}",
                    kind.name()
                )));
            }
            note_fallback(kind, hash, reason);
            Ok(None)
        }
    }
}

/// Note an oxide run that went to QuickJS before any host call (`reason`, for the census).
pub(crate) fn note_fallback(kind: EntryKind, hash: u64, reason: &str) {
    counts::bump(|counts| &counts.fallbacks);
    FALLEN_BACK.with(|memo| {
        let mut memo = memo.borrow_mut();
        if memo.len() >= MAX_FALLBACK_TEXTS {
            memo.clear();
        }
        let count = memo.entry((kind, hash)).or_default();
        *count = count.saturating_add(1);
    });
    crate::script::census::fallback(kind, hash, reason);
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
