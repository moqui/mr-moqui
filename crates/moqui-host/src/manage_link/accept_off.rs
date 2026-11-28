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

//! The manager's end of the manager link in a host without the `instances` feature (Lite): it can be managed, and
//! manages nothing, so the HTTPS listener takes no `moqui-manage/1`. The same API as `accept.rs` for the listener.

use std::net::SocketAddr;
use std::sync::{Arc, Weak};

use moqui_core::context::ExecutionContextFactory;
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;

/// Never made.
pub(crate) enum Acceptor {}

// never made: no acceptor exists
#[allow(dead_code)]
pub(crate) enum Secured {
    Http(TlsStream<TcpStream>),
    Link(TlsStream<TcpStream>),
}

impl Acceptor {
    pub(crate) async fn accept(
        &self,
        _stream: TcpStream,
        _http: &Arc<rustls::ServerConfig>,
        _factory: &Weak<ExecutionContextFactory>,
    ) -> std::io::Result<Secured> {
        match *self {}
    }

    pub(crate) async fn serve(
        &self,
        _stream: TlsStream<TcpStream>,
        _remote: SocketAddr,
        _closing: tokio::sync::watch::Receiver<bool>,
    ) {
        match *self {}
    }

    // the edge link's passthrough calls it, in a host with the edge feature
    #[cfg_attr(not(feature = "edge"), allow(dead_code))]
    pub(crate) async fn serve_passthrough<S>(
        &self,
        _accepted: rustls::server::Accepted,
        _stream: S,
        _remote: SocketAddr,
        _limit: std::time::Duration,
    ) {
        match *self {}
    }
}

/// None: no acceptor exists.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn acceptor_of(_factory: &ExecutionContextFactory) -> Option<Arc<Acceptor>> {
    None
}

/// No acceptor: this host manages no instances.
pub(crate) fn start(
    _factory: &Arc<ExecutionContextFactory>,
    _shutdown: tokio::sync::watch::Receiver<bool>,
    _edge_node: bool,
) -> Option<Arc<Acceptor>> {
    None
}
