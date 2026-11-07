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

//! The quickjs-oxide bindings (plans/Script-Engine-Oxide.md section 5): one runtime per thread, started on the
//! thread's first oxide script, beside the thread's QuickJS engine (`script/runtime.rs`). Phase 1 runs conditions and
//! values: bare names through an exotic object that is the global object's prototype ([`names`], [`tracked`]), host
//! values converted on read ([`convert`]), decimals as host objects and entity values as exotic ones ([`decimal`],
//! [`names`]), compiled expressions kept per
//! runtime by their text's hash ([`compiled`]), errors as the QuickJS path makes them ([`errors`]), and the clock,
//! time zone and deadline from [`host`]. `ec` is the host table (`script/host`) through its binder ([`bind`]), with
//! [`convert`] as its conversions.
//!
//! A run that meets what these bindings do not have yet (a member of a host object it does not bind, an entity
//! relationship) before it reached any host call reports a fallback, and the router runs the script on QuickJS instead.

pub(crate) mod bind;
mod compiled;
mod context;
mod convert;
mod decimal;
mod engine;
mod errors;
mod host;
mod names;
mod reads;
pub(crate) mod spec;
pub(crate) mod templates;
mod tracked;
pub(crate) mod walker;

pub(crate) use engine::{
    start_failed_recently, DID_NOT_START,
    engine_memory, eval_condition, eval_map, eval_text, eval_value, is_lent, release_for_quickjs, run_actions, stats,
    thread_heap_peaks, Outcome,
};
pub(crate) use reads::reads_are_held;

#[cfg(test)]
mod tests;
