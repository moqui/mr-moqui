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

//! The JavaScript test harness, Moqui Java's Spock tests on the script engine: spec files of a component's `test`
//! directory run in the same engine as its services. `moqui test` in the host drives it (`plans/archive/TestHarness.md`).
//! The evaluation harness of the agents (`moqui test --agents`, `test/agent/*.agent.js`) is moqui-llm's, on this. A
//! [`SpecHost`] runs the same specs as a client of a running server (`moqui test-client`, plans/archive/ClientTestHarness.md).

mod lines;
pub mod rewrite;
pub mod run;

pub use rewrite::{suite_order, suite_server, SuiteServer};
pub use run::{
    run_on_thread, run_spec, spec_database_free, spec_depends_on, spec_missing_features, spec_offline, spec_slow,
    CaseExtras, CaseResult, Failure, Outcome, RunOptions, SpecFile, SpecHost, SpecResult, SLOW_REASON,
};

#[cfg(test)]
mod tests;
