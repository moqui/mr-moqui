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

//! Agent Code: the T3 Code workbench on MR Moqui (plans/agent-code/README.md, guide `docs/ai/Agent-Code.md`). This
//! crate holds the parts with no transport: the command pipeline with its receipts ([`command`]), the change feed the
//! shell stream reads ([`change`]), the effect outbox ([`effect`]), the method scopes ([`scopes`]), the shell snapshot
//! ([`shell`]), the settings ([`settings`]), the environment descriptor ([`environment`]) and the `/agentws` frame
//! protocol ([`ws`]). The host's `/agentws` route (`crates/moqui-host/src/agentws.rs`) carries the frames.
//!
//! Ported from T3 Code (MIT, see `runtime/components/base/agent-code/NOTICE`). The crate is on moqui-core, which reaches
//! it through the [`moqui_core::extension`] entry below; moqui-host links it under its `code` feature.

pub mod change;
pub mod command;
pub mod effect;
pub mod environment;
pub mod order_key;
pub mod scopes;
pub(crate) mod services;
pub mod settings;
pub mod shell;
pub(crate) mod support;
pub mod ws;

/// The `/agentws` protocol version (T3 `orchestrationProtocol`). A client sends it as `protocol=<n>`.
pub const PROTOCOL: u32 = 1;

/// The permission to use Agent Code.
pub const USE_PERMISSION: &str = "AgentCodeUse";
/// The permission to run host processes: terminals, files outside project roots, host ports.
pub const HOST_PERMISSION: &str = "AgentCodeHost";
/// The permission to change the environment: provider instances, policies, server settings, diagnostics.
pub const ADMIN_PERMISSION: &str = "AgentCodeAdmin";

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["code"],
        register_services,
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn register_services(
    map: &mut std::collections::BTreeMap<&'static str, moqui_core::extension::RustServiceFn>,
) -> Result<(), String> {
    services::register(map)
}
