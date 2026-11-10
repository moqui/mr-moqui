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
//! `org.moqui.edge.EdgeServices` answers "edge link not built into this host", and the HTTPS listener has no hand-off.
//! The same API as `edge_link/` for `http.rs`.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use axum::response::Response;
use moqui_core::context::ExecutionContextFactory;
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;

use crate::http::{DownServe, Peer};

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

/// Never made: no host goes down a link.
pub(crate) enum WebFront {}

/// Never made.
#[derive(Clone)]
pub(crate) enum Target {}

impl WebFront {
    pub(crate) fn target<B>(&self, _request: &http::Request<B>) -> Option<Target> {
        match *self {}
    }

    pub(crate) fn forward<B>(
        &self,
        target: Target,
        _request: http::Request<B>,
        _peer: &Peer,
    ) -> Pin<Box<dyn Future<Output = Response> + Send>> {
        match target {}
    }
}

/// What [`start`] gives the listeners: nothing.
#[derive(Default)]
pub(crate) struct Started {
    pub acceptor: Option<Arc<Acceptor>>,
    pub front: Option<Arc<WebFront>>,
}

/// Nothing to start; on an edge node, say that this binary cannot be one.
pub(crate) fn start(
    factory: &Arc<ExecutionContextFactory>,
    _resolver: Option<Arc<dyn rustls::server::ResolvesServerCert>>,
    _shutdown: tokio::sync::watch::Receiver<bool>,
    _down: DownServe,
) -> Started {
    if moqui_core::edge::takes_links() {
        tracing::error!(
            "node_role is edge or all-in-one, but this binary has no edge link (the edge feature: Maximal or Enterprise)"
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
    moqui_core::startup::integration(factory, "Edge link", moqui_core::startup::State::NotBuilt);
    moqui_core::startup::listener(factory, "Edge tunnels", moqui_core::startup::State::NotBuilt);
    Started::default()
}

/// Nothing to let go of.
pub(crate) fn stopped(_factory: &ExecutionContextFactory) {}
