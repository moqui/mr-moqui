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
//! [`agents`] is the evaluation harness of the agents (`moqui test --agents`, `test/agent/*.agent.js`).

#[cfg(feature = "llm")]
pub mod agents;
mod lines;
pub mod rewrite;
pub mod run;

pub use rewrite::suite_order;
pub use run::{
    run_spec, spec_depends_on, spec_missing_features, CaseResult, Failure, Outcome, RunOptions, SpecFile, SpecResult,
};

#[cfg(test)]
mod tests;
