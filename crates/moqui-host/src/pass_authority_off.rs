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

//! The PASS authority over gRPC in a host built with `grpc` but not `edge` (plans/archive/Edge-Node.md §5.5): its generated
//! service is the `edge` feature's, so the gRPC listener serves only the OTLP receiver and scripts have no sender
//! (`ec.service.passAuthority` says "gRPC not built into this host", and mr-pass takes REST). The same API as
//! `pass_authority/` for `grpc_listen.rs`.

use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;

pub(crate) const PORT_PROPERTY: &str = "pass_authority_grpc_port";
pub(crate) const HOST_PROPERTY: &str = "pass_authority_grpc_host";
pub(crate) const INSECURE_PROPERTY: &str = "pass_authority_grpc_insecure";
const ENABLED_PROPERTY: &str = "pass_authority_grpc_enabled";

/// Never: a `pass_authority_grpc_enabled=true` is logged as not built, once.
pub(crate) fn direct_enabled() -> bool {
    static LOGGED: std::sync::Once = std::sync::Once::new();
    if moqui_lib::env::get_prop_or_env(ENABLED_PROPERTY).trim() == "true" {
        LOGGED.call_once(|| {
            tracing::error!(
                "{ENABLED_PROPERTY} is true, but this host has no PASS authority over gRPC (the edge feature)"
            );
        });
    }
    false
}

pub(crate) fn add_direct(
    routes: tonic::service::Routes,
    _factory: &Arc<ExecutionContextFactory>,
) -> tonic::service::Routes {
    routes
}
