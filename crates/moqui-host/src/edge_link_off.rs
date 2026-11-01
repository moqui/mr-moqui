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

//! The edge link in a host built without the `edge` feature (Lite, Dev): no hook is installed, so
//! `org.moqui.impl.EdgeServices` answers "edge link not built into this host", and the HTTPS listener has no hand-off.
//! The same API as `edge_link/` for `http.rs`.

use std::net::SocketAddr;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;

/// Never made: no connection is handed to the link.
pub(crate) enum Acceptor {}

pub(crate) enum Secured {
    Http(TlsStream<TcpStream>),
    // made only by the edge feature's acceptor
    #[allow(dead_code)]
    Link(TlsStream<TcpStream>),
}

impl Acceptor {
    pub(crate) async fn accept(
        &self,
        _stream: TcpStream,
        _http: &Arc<rustls::ServerConfig>,
    ) -> std::io::Result<Secured> {
        match *self {}
    }

    pub(crate) async fn serve(&self, _stream: TlsStream<TcpStream>, _remote: SocketAddr) {
        match *self {}
    }
}

/// Nothing to start; on an edge node, say that this binary cannot be one.
pub(crate) fn start(
    _factory: &Arc<ExecutionContextFactory>,
    _resolver: Option<Arc<dyn rustls::server::ResolvesServerCert>>,
    _shutdown: tokio::sync::watch::Receiver<bool>,
) -> Option<Arc<Acceptor>> {
    if moqui_core::edge::is_edge_node() {
        tracing::error!(
            "node_role is edge, but this binary has no edge link (the edge feature: Maximal or Enterprise)"
        );
    }
    if !moqui_lib::env::get_prop_or_env(moqui_core::edge::NODE_URL)
        .trim()
        .is_empty()
    {
        tracing::error!(
            "edge_node_url is set, but this binary has no edge link (the edge feature: Maximal or Enterprise)"
        );
    }
    None
}
