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

//! The host's gRPC listener: one port, on tonic, for the gRPC services other programs call, each a set of routes:
//! - the OTLP/gRPC receiver (`crate::monitor::grpc`, plans/archive/Monitoring.md), when
//!   `<telemetry><receiver grpc-enabled>`;
//! - the PASS authority for direct clients (`crate::pass_authority`, plans/archive/Edge-Node.md §5.5), when
//!   `pass_authority_grpc_enabled=true` (the `edge` feature; in Maximal and Enterprise).
//!
//! It opens when either is on. With the receiver on, it listens where the receiver is configured (`grpc-bind`,
//! `grpc-port`, 4317), and the PASS authority shares that port; else on `pass_authority_grpc_host` (the HTTP listener's
//! address) and `pass_authority_grpc_port` (4317). With the HTTPS listener's certificates it serves TLS (ALPN h2), else
//! plain h2c; then the PASS authority, whose clients send their secrets, is left off (an error logged) unless the
//! address is loopback or `pass_authority_grpc_insecure=true`. The edge guard admits each connection as it does for HTTP.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use moqui_core::context::ExecutionContextFactory;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpListener;

/// The address the gRPC listener bound, for `Server::grpc_address`.
#[derive(Default)]
pub struct GrpcAddress(pub std::sync::Mutex<Option<SocketAddr>>);

/// Where to listen, when a service is on: the receiver's address, else the PASS authority's. `tls`: the HTTPS listener
/// has certificates, so the listener serves TLS.
pub(crate) fn bind_address(factory: &ExecutionContextFactory, http: SocketAddr, tls: bool) -> Option<SocketAddr> {
    bind_address_by(&|path, name| factory.conf_setting(path, name), http, tls)
}

/// [`bind_address`] by a conf `setting`, for the port check before the factory is built.
pub(crate) fn bind_address_by(
    setting: crate::monitor::grpc::Setting,
    http: SocketAddr,
    tls: bool,
) -> Option<SocketAddr> {
    let otlp = crate::monitor::grpc::bind_address_by(setting, http);
    let pass =
        crate::pass_authority::direct_enabled() && pass_allowed(otlp.unwrap_or_else(|| pass_address(http)), tls, false);
    match (otlp, pass) {
        (Some(otlp), true) => {
            let port = moqui_lib::env::get_prop_or_env(crate::pass_authority::PORT_PROPERTY);
            if !port.trim().is_empty() && port.trim() != otlp.port().to_string() {
                tracing::warn!(
                    "{} is {} but the OTLP/gRPC receiver listens on {otlp}; the PASS authority shares its port",
                    crate::pass_authority::PORT_PROPERTY,
                    port.trim()
                );
            }
            Some(otlp)
        }
        (Some(otlp), false) => Some(otlp),
        (None, true) => Some(pass_address(http)),
        (None, false) => None,
    }
}

/// Whether the PASS authority may serve on `bind`: with TLS, on a loopback address, or with
/// `pass_authority_grpc_insecure=true`; else a client's secret would cross the network in clear, so it is left off
/// (logged when `log`).
fn pass_allowed(bind: SocketAddr, tls: bool, log: bool) -> bool {
    let insecure = moqui_lib::env::get_prop_or_env(crate::pass_authority::INSECURE_PROPERTY).trim() == "true";
    if tls || bind.ip().is_loopback() || insecure {
        return true;
    }
    if log {
        tracing::error!(
            "the PASS authority's gRPC is off: {bind} is not a loopback address and there is no certificate for TLS (in \
             runtime/cert), so clients' secrets would cross the network in clear; set {}=true to serve it anyway",
            crate::pass_authority::INSECURE_PROPERTY
        );
    }
    false
}

/// `pass_authority_grpc_host` (the HTTP listener's address when empty) and `pass_authority_grpc_port` (4317).
fn pass_address(http: SocketAddr) -> SocketAddr {
    let get = |name: &str| moqui_lib::env::get_prop_or_env(name).trim().to_string();
    let port = match get(crate::pass_authority::PORT_PROPERTY).as_str() {
        "" => 4317,
        text => text.parse().unwrap_or_else(|_| {
            tracing::error!(
                "{} {text} is not a port; using 4317",
                crate::pass_authority::PORT_PROPERTY
            );
            4317
        }),
    };
    let ip = match get(crate::pass_authority::HOST_PROPERTY).as_str() {
        "" => http.ip(),
        text => text.parse().unwrap_or_else(|_| {
            tracing::error!(
                "{} {text} is not an IP address; using {}",
                crate::pass_authority::HOST_PROPERTY,
                http.ip()
            );
            http.ip()
        }),
    };
    SocketAddr::new(ip, port)
}

/// The routes of the services that are on, and their names for the log.
fn routes(
    factory: &Arc<ExecutionContextFactory>,
    bind: SocketAddr,
    tls: bool,
) -> (tonic::service::Routes, Vec<&'static str>) {
    service_routes(
        factory,
        crate::pass_authority::direct_enabled() && pass_allowed(bind, tls, true),
    )
}

/// The OTLP receiver's routes when it is on, and the PASS authority's when `pass`.
fn service_routes(factory: &Arc<ExecutionContextFactory>, pass: bool) -> (tonic::service::Routes, Vec<&'static str>) {
    let mut routes = tonic::service::Routes::default();
    let mut names = Vec::new();
    if crate::monitor::grpc::enabled(factory) {
        routes = crate::monitor::grpc::add_routes(routes, factory);
        names.push("OTLP/gRPC receiver");
    }
    if pass {
        routes = crate::pass_authority::add_direct(routes, factory);
        names.push("PASS authority");
    }
    (routes, names)
}

/// The same services for the HTTPS listener, which carries a gRPC call over HTTP/2 to them (`crate::http::serve`):
/// that is always TLS, so the PASS authority's are there whenever `pass_authority_grpc_enabled=true`, with their names
/// for the startup report. `None` when no service is on. The gRPC listener's own port stays, opened by the same settings.
pub(crate) fn web_routes(factory: &Arc<ExecutionContextFactory>) -> Option<(tonic::service::Routes, String)> {
    let (routes, names) = service_routes(factory, crate::pass_authority::direct_enabled());
    if names.is_empty() {
        return None;
    }
    Some((routes, names.join(" and ")))
}

/// The gRPC listener, bound, with the routes of the services it serves.
pub(crate) struct Bound {
    listener: TcpListener,
    routes: tonic::service::Routes,
    label: String,
    acceptor: Option<tokio_rustls::TlsAcceptor>,
}

/// Bind `bind` before the startup report, so the report has its address. A bind failure is logged and recorded, and
/// the HTTP listeners go on without gRPC; a port that was busy at the start check is not tried.
pub(crate) async fn open(
    factory: &Arc<ExecutionContextFactory>,
    bind: SocketAddr,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> Option<Bound> {
    use moqui_core::startup::{listener as record, State};
    if let Some(busy) = crate::port_check::busy(LISTENER) {
        record(factory, LISTENER, State::Skipped(busy));
        return None;
    }
    let (routes, names) = routes(factory, bind, tls.is_some());
    let label = names.join(" and ");
    let listener = match TcpListener::bind(bind).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!("could not bind the gRPC listener ({label}) on {bind}: {error}");
            record(
                factory,
                LISTENER,
                State::failed(format!("{bind} did not bind: {error}")),
            );
            return None;
        }
    };
    let local = listener.local_addr().unwrap_or(bind);
    *moqui_lib::sync::lock(&factory.extension(|_| GrpcAddress::default()).0) = Some(local);
    record(
        factory,
        LISTENER,
        State::on(format!(
            "tcp {local}{}, {label}",
            if tls.is_some() { " TLS" } else { "" }
        )),
    );
    let acceptor = tls.map(|config| {
        // gRPC is HTTP/2 only; the HTTPS listener's config offers http/1.1 too.
        let mut config = (*config).clone();
        config.alpn_protocols = vec![b"h2".to_vec()];
        tokio_rustls::TlsAcceptor::from(Arc::new(config))
    });
    Some(Bound {
        listener,
        routes,
        label,
        acceptor,
    })
}

/// The startup report's name for this listener.
pub(crate) const LISTENER: &str = "gRPC";

/// Serve the bound listener until `stop`.
pub(crate) async fn serve(bound: Bound, guard: Arc<crate::guard::Guard>, stop: impl std::future::Future<Output = ()>) {
    let Bound {
        listener,
        routes,
        label,
        acceptor,
    } = bound;
    let (connections, incoming) = tokio::sync::mpsc::channel::<std::io::Result<Connection>>(64);
    let accepting = tokio::spawn(accept(listener, acceptor, guard, connections));
    let incoming = futures_util::stream::unfold(incoming, |mut incoming| async move {
        incoming.recv().await.map(|connection| (connection, incoming))
    });
    if let Err(error) = tonic::transport::Server::builder()
        .add_routes(routes)
        .serve_with_incoming_shutdown(incoming, stop)
        .await
    {
        tracing::error!("the gRPC listener ({label}) stopped: {error}");
    }
    accepting.abort();
}

/// Accept connections, each TLS handshake in a task of its own so a slow client does not hold up the others.
async fn accept(
    listener: TcpListener,
    acceptor: Option<tokio_rustls::TlsAcceptor>,
    guard: Arc<crate::guard::Guard>,
    connections: tokio::sync::mpsc::Sender<std::io::Result<Connection>>,
) {
    loop {
        let (stream, remote) = match listener.accept().await {
            Ok(pair) => pair,
            Err(error) => {
                tracing::debug!("gRPC accept failed: {error}");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            }
        };
        let Some(slot) = guard.admit(remote.ip()) else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        let connections = connections.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let io: Box<dyn Io> = match acceptor {
                None => Box::new(stream),
                Some(acceptor) => {
                    match tokio::time::timeout(std::time::Duration::from_secs(10), acceptor.accept(stream)).await {
                        Ok(Ok(stream)) => Box::new(stream),
                        Ok(Err(error)) => {
                            tracing::debug!("gRPC TLS handshake with {remote} failed: {error}");
                            return;
                        }
                        Err(_) => return,
                    }
                }
            };
            let _ = connections
                .send(Ok(Connection {
                    io,
                    remote,
                    _slot: slot,
                }))
                .await;
        });
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// A plain or TLS connection, holding its edge-guard slot while it is open.
struct Connection {
    io: Box<dyn Io>,
    remote: SocketAddr,
    _slot: crate::guard::ConnectionSlot,
}

impl tonic::transport::server::Connected for Connection {
    type ConnectInfo = SocketAddr;

    fn connect_info(&self) -> SocketAddr {
        self.remote
    }
}

impl AsyncRead for Connection {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for Connection {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.io).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
}

#[cfg(test)]
#[path = "grpc_listen_tests.rs"]
mod tests;
