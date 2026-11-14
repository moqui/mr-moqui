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

//! Components and conf on a running server (plans/archive/Component-Deploy.md): the conf reload's services, and the loading
//! of a component without a restart. The conf reload itself is `moqui_core::context::conf_reload`; this crate holds
//! what uses the reload pipeline ([`crate::reload`]). The gate ([`require_passed_run`]) keeps a server to commits a
//! source server attests passed their pipeline (`git::deploy_attested`). Guide: docs/tools/Component-Deploy.md.

pub mod conf_edit;
pub mod gate;
#[cfg(feature = "git")]
pub mod git;
#[cfg(not(feature = "git"))]
#[path = "git_off.rs"]
pub mod git;
pub mod load;
pub mod services;

pub(crate) use services::register;

/// Whether this server deploys a commit only with an attestation that it passed its environment's pipeline: the
/// conf's `component-deploy` `require-passed-run` as it was at start ([`gate::conf`]; docs/tools/Component-Deploy.md
/// "The gate"). Then `deploy#Component` and `update#Component` refuse an unattested commit, `load#Component` (a
/// directory, no commit) refuses, and a start or another cluster member writes only an attested commit.
pub fn require_passed_run(factory: &moqui_core::context::ExecutionContextFactory) -> bool {
    gate::conf(factory).require_passed_run
}

/// What the gate says of a deploy that is not attested while it is on.
pub const GATE_REFUSAL: &str = "this server deploys a commit only with an attestation that it passed its \
     environment's pipeline (component-deploy require-passed-run): deploy it from its repository's environment";
