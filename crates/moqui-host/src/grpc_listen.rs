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
//!   `moqui_conf__pass_authority__grpc_enabled=true` (the `edge` feature; in Maximal and Enterprise).
//!
//! The webroot webapp's grpc-listen-enabled, grpc-listen-port and grpc-listen-bind select the dedicated socket for both
//! services (plans/Runtime-Config.md §3.2).
//! With the HTTPS listener's certificates it serves TLS (ALPN h2), else
//! plain h2c; then the PASS authority, whose clients send their secrets, is left off (an error logged) unless the
//! address is loopback or `moqui_conf__pass_authority__grpc_insecure=true`. The edge guard admits each connection as it does for HTTP.

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

/// Address shared by the enabled services on the dedicated listener.
pub(crate) fn bind_address(factory: &ExecutionContextFactory, ip: std::net::IpAddr, tls: bool) -> Option<SocketAddr> {
    bind_address_by(
        &|path, name| factory.conf_setting(path, name),
        &moqui_core::context::conf_env::webroot_setting,
        ip,
        tls,
        true,
    )
}

/// [`bind_address`] from the settings `setting` and `webroot`: none while `grpc-listen-enabled` is not true or no
/// service is on. The PASS authority's h2c refusal is logged when `log` (the listener's own start), not by the port
/// check before it, so it is logged once.
pub(crate) fn bind_address_by(
    setting: crate::monitor::grpc::Setting,
    webroot: &dyn Fn(&str) -> String,
    web_ip: std::net::IpAddr,
    tls: bool,
    log: bool,
) -> Option<SocketAddr> {
    if !moqui_core::context::conf_env::switch_on(&webroot("grpc-listen-enabled")) {
        return None;
    }
    let text = webroot("grpc-listen-port");
    let port = match text.trim().parse() {
        Ok(port) => port,
        Err(err) => {
            tracing::error!(
                "invalid grpc-listen-port \"{}\": {err} (set grpc-listen-enabled=\"false\" to keep it closed)",
                text.trim()
            );
            return None;
        }
    };
    let ip = match webroot("grpc-listen-bind").trim() {
        "" => web_ip,
        text => match text.parse() {
            Ok(ip) => ip,
            Err(err) => {
                tracing::error!("invalid grpc-listen-bind {text}: {err}");
                return None;
            }
        },
    };
    let address = SocketAddr::new(ip, port);
    (crate::monitor::grpc::enabled_by(setting)
        || (crate::pass_authority::direct_enabled() && pass_allowed(address, tls, log)))
    .then_some(address)
}

/// Whether the PASS authority may serve on `bind`: with TLS, on a loopback address, or with
/// `moqui_conf__pass_authority__grpc_insecure=true`; else a client's secret would cross the network in clear, so it is left off
/// (logged when `log`).
fn pass_allowed(bind: SocketAddr, tls: bool, log: bool) -> bool {
    let insecure = moqui_core::context::conf_env::setting(crate::pass_authority::INSECURE_PROPERTY).trim() == "true";
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
/// that is always TLS, so the PASS authority's are there whenever `moqui_conf__pass_authority__grpc_enabled=true`, with their names
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
    // the accept task owns the listener: aborted when this serve ends or is dropped (a stop past its wait aborts it),
    // so the port is freed either way
    let _accepting = AbortOnDrop(tokio::spawn(accept(listener, acceptor, guard, connections)));
    let incoming = futures_util::stream::unfold(incoming, |mut incoming| async move {
        incoming.recv().await.map(|connection| (connection, incoming))
    });
    // before any credential, on the OTLP receiver's and the PASS authority's port: a peer that stops answering pings
    // goes, a connection has at most as many streams as a cluster one, and a call waits for its answer at most
    // CALL_TIMEOUT; a connection with no call in flight for CONNECTION_IDLE closes (`Idle`) (SEC-CLU-88)
    if let Err(error) = tonic::transport::Server::builder()
        .http2_keepalive_interval(Some(PING_EVERY))
        .http2_keepalive_timeout(Some(PING_TIMEOUT))
        .max_concurrent_streams(MAX_STREAMS)
        .timeout(CALL_TIMEOUT)
        .layer(tower::layer::layer_fn(|inner| Track { inner }))
        .add_routes(routes)
        .serve_with_incoming_shutdown(incoming, stop)
        .await
    {
        tracing::error!("the gRPC listener ({label}) stopped: {error}");
    }
}

/// A task aborted when this is dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
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
        let guard = Arc::clone(&guard);
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
            let activity = Arc::new(crate::http::Activity::new());
            let _ = connections
                .send(Ok(Connection {
                    io: Idle::new(io, Arc::clone(&activity)),
                    remote,
                    guard,
                    activity,
                    _slot: slot,
                }))
                .await;
        });
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// Who a gRPC call came from, in each call's extensions, on this listener and on the HTTPS listener's HTTP/2: the
/// connection's peer, the client (the peer, or on the HTTPS listener the client a believed proxy names) and the edge
/// guard, which a service tells of a refused credential as the HTTP routes do (SEC-TLS-17).
#[derive(Clone)]
pub(crate) struct Caller {
    /// Read by the PASS authority (`crate::pass_authority`), which the edge feature builds.
    #[cfg(feature = "edge")]
    pub peer: SocketAddr,
    pub client: std::net::IpAddr,
    pub guard: Arc<crate::guard::Guard>,
    /// The calls in flight on this listener's connection ([`Track`] counts them); none on the HTTPS listener, which
    /// closes its own idle connections.
    pub activity: Option<Arc<crate::http::Activity>>,
}

/// HTTP/2 PING interval of the gRPC listener, and how long an unanswered one waits (as the cluster port).
const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
const PING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// Streams open at once on one connection (as the cluster port).
const MAX_STREAMS: u32 = 64;
/// The longest a call waits for its answer.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// A connection with no call in flight for this long closes (as the cluster port's).
const CONNECTION_IDLE: std::time::Duration = std::time::Duration::from_secs(60);
/// A connection that has sent no byte this long after it was handed over (the TLS handshake done, or at once for
/// h2c) closes.
const FIRST_BYTE: std::time::Duration = std::time::Duration::from_secs(10);

/// A plain or TLS connection, holding its edge-guard slot while it is open.
struct Connection {
    io: Idle<Box<dyn Io>>,
    remote: SocketAddr,
    guard: Arc<crate::guard::Guard>,
    activity: Arc<crate::http::Activity>,
    _slot: crate::guard::ConnectionSlot,
}

impl tonic::transport::server::Connected for Connection {
    type ConnectInfo = Caller;

    fn connect_info(&self) -> Caller {
        Caller {
            #[cfg(feature = "edge")]
            peer: self.remote,
            client: self.remote.ip().to_canonical(),
            guard: Arc::clone(&self.guard),
            activity: Some(Arc::clone(&self.activity)),
        }
    }
}

/// A connection's reads, which end (EOF, so the server closes it) once it has sent no byte for [`FIRST_BYTE`] from
/// its start, or has had no call in flight for [`CONNECTION_IDLE`]: a client that sends nothing, or only pings, keeps
/// no edge-guard slot (SEC-CLU-88).
struct Idle<T> {
    io: T,
    activity: Arc<crate::http::Activity>,
    /// A byte came from the client.
    heard: bool,
    timer: Pin<Box<tokio::time::Sleep>>,
    /// [`FIRST_BYTE`] and [`CONNECTION_IDLE`], a test's shorter ones.
    first_byte: std::time::Duration,
    idle: std::time::Duration,
}

impl<T> Idle<T> {
    fn new(io: T, activity: Arc<crate::http::Activity>) -> Self {
        Self::with_limits(io, activity, FIRST_BYTE, CONNECTION_IDLE)
    }

    fn with_limits(
        io: T,
        activity: Arc<crate::http::Activity>,
        first_byte: std::time::Duration,
        idle: std::time::Duration,
    ) -> Self {
        Self {
            io,
            activity,
            heard: false,
            timer: Box::pin(tokio::time::sleep(first_byte)),
            first_byte,
            idle,
        }
    }

    /// How much longer the connection may wait now; zero when it is to close.
    fn left(&self) -> std::time::Duration {
        let limit = if self.heard { self.idle } else { self.first_byte };
        let idle = self.activity.idle_for();
        if idle.is_zero() && self.heard {
            // a call in flight: look again after the idle period
            return self.idle;
        }
        limit.saturating_sub(idle)
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Idle<T> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        let before = buf.filled().len();
        match Pin::new(&mut this.io).poll_read(cx, buf) {
            Poll::Ready(result) => {
                if buf.filled().len() > before {
                    this.heard = true;
                }
                Poll::Ready(result)
            }
            Poll::Pending => loop {
                let left = this.left();
                if left.is_zero() {
                    tracing::debug!("gRPC connection closed: idle");
                    return Poll::Ready(Ok(()));
                }
                this.timer.as_mut().reset(tokio::time::Instant::now() + left);
                if std::future::Future::poll(this.timer.as_mut(), cx).is_pending() {
                    return Poll::Pending;
                }
            },
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Idle<T> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
}

/// Counts each call on the gRPC listener in its connection's [`Caller::activity`] until its answer's body ends.
#[derive(Clone)]
struct Track<S> {
    inner: S,
}

impl<S, B> tower::Service<http::Request<B>> for Track<S>
where
    S: tower::Service<http::Request<B>, Response = http::Response<tonic::body::Body>>,
    S::Future: Send + 'static,
{
    type Response = http::Response<tonic::body::Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        let flight = request
            .extensions()
            .get::<Caller>()
            .and_then(|caller| caller.activity.clone())
            .map(InFlight::new);
        let answer = self.inner.call(request);
        Box::pin(async move {
            let response = answer.await?;
            Ok(match flight {
                Some(flight) => response.map(|body| tonic::body::Body::new(Tracked { body, _flight: flight })),
                None => response,
            })
        })
    }
}

/// A call in flight, counted until dropped.
struct InFlight(Arc<crate::http::Activity>);

impl InFlight {
    fn new(activity: Arc<crate::http::Activity>) -> Self {
        activity.started();
        Self(activity)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.ended();
    }
}

/// An answer's body, holding its call in flight until it ends.
struct Tracked {
    body: tonic::body::Body,
    _flight: InFlight,
}

impl hyper::body::Body for Tracked {
    type Data = <tonic::body::Body as hyper::body::Body>::Data;
    type Error = <tonic::body::Body as hyper::body::Body>::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.body).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.size_hint()
    }
}

impl AsyncRead for Connection {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for Connection {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }
}

#[cfg(test)]
#[path = "grpc_listen_tests.rs"]
mod tests;
