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

//! XMPP when this build has no `xmpp` feature: nothing starts, and a set `xmpp_domain` is an error in the log.

use std::net::SocketAddr;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;

pub(crate) struct XmppNode;

pub(crate) fn start(factory: &Arc<ExecutionContextFactory>) -> Option<XmppNode> {
    if !moqui_lib::env::get_prop_or_env("xmpp_domain").trim().is_empty() {
        tracing::error!("xmpp_domain is set but this build has no XMPP server (the xmpp feature)");
    }
    moqui_core::startup::listener(factory, "XMPP", moqui_core::startup::State::NotBuilt);
    None
}

/// No XMPP server runs in this build.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn running(_factory: &ExecutionContextFactory, _direct: bool) -> bool {
    false
}

/// No XMPP server takes a tunneled stream in this build; it is dropped.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn serve_tunneled<S>(
    _factory: &ExecutionContextFactory,
    _stream: S,
    _client: SocketAddr,
    _direct: bool,
) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    false
}
