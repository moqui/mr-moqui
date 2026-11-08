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

//! The entry kinds a script comes in by, their counts, and the one call each entry makes into quickjs-oxide
//! ([`run`]).

use crate::context::Error;

/// The entry point a script comes in by.
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
    /// A host's own use of the engine (a screen unit, a web handler).
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
}

/// Counts of scripts by entry kind, one table per thread so a script's count is a write no other thread makes: each
/// thread's table is registered once and [`scripts_by_kind`] sums them.
mod counts {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub(super) struct Counts {
        pub(super) runs: [AtomicU64; 10],
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

    /// This thread's counter `pick` gives, for a test that other tests' threads must not disturb.
    #[cfg(test)]
    pub(super) fn mine(pick: impl FnOnce(&Counts) -> &AtomicU64) -> u64 {
        MINE.with(|mine| pick(mine).load(Relaxed))
    }

    /// The sum over every thread's table of the counter `pick` gives.
    pub(super) fn sum(pick: impl Fn(&Counts) -> &AtomicU64) -> u64 {
        moqui_lib::sync::lock(&ALL)
            .iter()
            .map(|counts| pick(counts).load(Relaxed))
            .sum()
    }
}

/// Scripts run by entry kind since the process started: (kind, runs).
pub fn scripts_by_kind() -> Vec<(&'static str, u64)> {
    EntryKind::ALL
        .iter()
        .map(|kind| (kind.name(), counts::sum(|counts| &counts.runs[kind.index()])))
        .collect()
}

/// Scripts of `kind` this thread ran, for the tests.
#[cfg(test)]
pub(crate) fn runs_here(kind: EntryKind) -> u64 {
    counts::mine(|counts| &counts.runs[kind.index()])
}

/// Count a script of `kind` that ran.
pub(crate) fn count(kind: EntryKind) {
    counts::bump(|counts| &counts.runs[kind.index()]);
}

/// Run `source`, a script of `kind`, with `run` (one of quickjs-oxide's entries, given the text, its hash and whether
/// it writes back what it read by name), and count it. What the bindings do not have yet is an error naming the kind.
pub(crate) fn run<T>(
    kind: EntryKind,
    source: &str,
    run: impl FnOnce(&str, u64, bool) -> Result<T, crate::script::oxide::Outcome>,
) -> Result<T, Error> {
    use crate::script::oxide::Outcome;
    let hash = crate::script::text::hash_text(source);
    let ran = run(source, hash, true);
    count(kind);
    match ran {
        Ok(value) => Ok(value),
        Err(Outcome::Error(err)) => Err(err),
        Err(Outcome::Fallback(reason)) => Err(unsupported(kind, reason)),
    }
}

/// The error of a script of `kind` that needs `reason`, which the quickjs-oxide bindings do not have yet.
pub(crate) fn unsupported(kind: EntryKind, reason: &str) -> Error {
    Error::new(format!(
        "this {} needs {reason}, which the quickjs-oxide bindings do not have yet",
        kind.name()
    ))
}
