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

//! Public HTTP and HTTPS listeners. Each connection is served by hyper with axum's router, inside the `webapp_*`
//! limits of [`ListenLimits`]. The HTTPS certificates are in [`crate::tls`]. Screen and entity work runs on the request pool.

use std::collections::BTreeMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use axum::body::HttpBody as _;
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use hyper_util::rt::{TokioIo, TokioTimer};
use moqui_core::context::ErrorKind;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::transaction::PoolStats;
use moqui_core::web::{WebRequest, WebResponse};
use moqui_lib::error::Error;
use tokio::net::TcpListener;
use tower::ServiceExt as _;

use crate::guard::{setting, Event, Guard, GuardConf};
use crate::notws;

pub(crate) struct HttpState {
    pub factory: Arc<ExecutionContextFactory>,
    pub shutdown: tokio::sync::watch::Receiver<bool>,
    pub limits: ListenLimits,
    pub guard: Arc<Guard>,
}

/// The client of a request: the socket peer, or the address a trusted proxy in front names
/// (`moqui_core::web::address`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ClientAddr(pub IpAddr);

/// Bounds on the public listeners, from `web-protection`'s `connections` and `requests` in the `webroot` webapp, each
/// with its `webapp_*` property as the default. Timeouts are whole seconds, and `0` turns one off. These apply whether
/// or not the per-client parts of [`Guard`] are on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ListenLimits {
    /// `header-timeout` (`webapp_header_timeout`), default 30: a client sends its whole request head in this time.
    /// A kept-alive connection that sends nothing for as long is closed too.
    pub header: Option<Duration>,
    /// `body-timeout` (`webapp_body_timeout`), default 60: the whole request body arrives in this time, or the answer
    /// is 408. Not for a `multipart/form-data` upload, which is read as it arrives.
    pub body: Option<Duration>,
    /// `upload-idle-timeout` (`webapp_upload_idle_timeout`), default 60: an upload that sends nothing for this long
    /// is cut off with 408.
    pub upload_idle: Option<Duration>,
    /// `request-timeout` (`webapp_request_timeout`), default 300: from the request head to the response head, or the
    /// answer is 504. `/notws` and `/termws` are exempt, and a response body, such as a long download, is sent after it.
    pub request: Option<Duration>,
    /// `max` (`webapp_max_connections`), default 1024: connections served at once, HTTP and HTTPS together. Past it
    /// the listeners stop accepting until one closes.
    pub connections: usize,
    /// `shutdown-timeout` (`webapp_shutdown_timeout`), default 30: after a stop, how long open connections may take to
    /// finish before the listener stops without them.
    pub shutdown: Option<Duration>,
    /// `max-uri-length`, default 8192: a longer path and query is answered 414.
    pub max_uri: usize,
    /// `max-header-bytes`, default 64 KiB (at least 8 KiB): a longer request head is answered 431.
    pub max_header_bytes: usize,
    /// `max-headers`, default 100: a request head with more header fields is answered 431.
    pub max_headers: usize,
}

/// The smallest read buffer hyper takes.
const MIN_HEADER_BYTES: usize = 8 * 1024;

impl ListenLimits {
    /// From `web-protection` (`None` for the properties and defaults alone).
    pub(crate) fn from_node(node: Option<&moqui_lib::node::MNode>) -> Self {
        let seconds = |child: &str, name: &str, property: &str, default: u64| {
            let seconds = setting(node, child, name, property).parse::<u64>().unwrap_or(default);
            (seconds > 0).then(|| Duration::from_secs(seconds))
        };
        let count = |child: &str, name: &str, property: &str, default: usize| {
            setting(node, child, name, property)
                .parse::<usize>()
                .ok()
                .filter(|count| *count > 0)
                .unwrap_or(default)
        };
        Self {
            header: seconds("connections", "header-timeout", "webapp_header_timeout", 30),
            body: seconds("requests", "body-timeout", "webapp_body_timeout", 60),
            upload_idle: seconds("requests", "upload-idle-timeout", "webapp_upload_idle_timeout", 60),
            request: seconds("requests", "request-timeout", "webapp_request_timeout", 300),
            connections: count("connections", "max", "webapp_max_connections", 1024),
            shutdown: seconds("connections", "shutdown-timeout", "webapp_shutdown_timeout", 30),
            max_uri: count("requests", "max-uri-length", "", 8192),
            max_header_bytes: count("requests", "max-header-bytes", "", 64 * 1024).max(MIN_HEADER_BYTES),
            max_headers: count("requests", "max-headers", "", 100),
        }
    }

    /// The defaults, for tests.
    #[cfg(test)]
    pub(crate) fn defaults() -> Self {
        Self {
            header: None,
            body: None,
            upload_idle: None,
            request: None,
            connections: 1024,
            shutdown: None,
            max_uri: 8192,
            max_header_bytes: 64 * 1024,
            max_headers: 100,
        }
    }
}

/// The listener limits and the web protection, from the factory's conf.
fn protection(factory: &ExecutionContextFactory) -> (ListenLimits, Guard) {
    let node = crate::guard::protection_node(factory.conf_text());
    let addressing = factory.extension(moqui_core::web::address::ClientAddressing::of_factory);
    let limits = ListenLimits::from_node(node.as_ref());
    let guard = Guard::new(GuardConf::from_node(node.as_ref(), addressing));
    (limits, guard)
}

/// Addresses and handles the test uses after the plain HTTP listener binds.
pub(crate) struct ListenReport {
    pub http: SocketAddr,
    pub https: Option<SocketAddr>,
    pub cluster: SocketAddr,
    pub factory: Arc<ExecutionContextFactory>,
    pub posts: Arc<std::sync::Mutex<Vec<String>>>,
}

/// Marks a request that arrived over this process's own TLS listener.
#[derive(Clone, Copy)]
pub(crate) struct TlsConnection;

/// The request is HTTPS: the listener is TLS, or a trusted proxy sent `X-Forwarded-Proto: https`.
#[derive(Clone, Copy)]
struct HttpsRequest;

/// Session, visitor, and username copied from [`moqui_core::web::HttpLogIdentity`] onto the axum response.
#[derive(Clone)]
struct LoggedIdentity {
    session: Option<String>,
    visitor_id: Option<String>,
    remote_user: Option<String>,
}

/// Bind `bind` (and the HTTPS address when there is one), publish the local addresses, and serve until `stop` is
/// dropped or a ctrl-c arrives.
pub(crate) async fn listen(
    factory: Arc<ExecutionContextFactory>,
    bind: SocketAddr,
    https: Option<crate::tls::HttpsListen>,
    cluster: SocketAddr,
    posts: Arc<std::sync::Mutex<Vec<String>>>,
    stop: tokio::sync::oneshot::Receiver<()>,
    ready: Sender<Result<ListenReport, String>>,
) -> Result<(), Error> {
    let fail = |message: String| {
        let _ = ready.send(Err(message.clone()));
        Error::new(message)
    };
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|err| fail(format!("could not bind {bind}: {err}")))?;
    let address = listener
        .local_addr()
        .map_err(|err| fail(format!("bound listener has no address: {err}")))?;
    let secure = match https {
        Some(https) => {
            let listener = TcpListener::bind(https.bind)
                .await
                .map_err(|err| fail(format!("could not bind {}: {err}", https.bind)))?;
            let local = listener
                .local_addr()
                .map_err(|err| fail(format!("bound HTTPS listener has no address: {err}")))?;
            Some((listener, local, tokio_rustls::TlsAcceptor::from(https.config)))
        }
        None => None,
    };
    let https_address = secure.as_ref().map(|(_, local, _)| *local);
    let _ = ready.send(Ok(ListenReport {
        http: address,
        https: https_address,
        cluster,
        factory: Arc::clone(&factory),
        posts,
    }));
    let https_text = https_address
        .map(|secure| format!(", HTTPS on {secure}"))
        .unwrap_or_default();
    let notws = endpoint_enabled(&factory, "/notws");
    let termws = cfg!(feature = "tui") && endpoint_enabled(&factory, "/termws");
    tracing::info!(
        "Listening for HTTP on {address}{https_text} and cluster on {cluster}; routes /health, /metrics, /dav, {}{}{}the webapp's servlets",
        if notws { "/notws, " } else { "" },
        if termws { "/termws, " } else { "" },
        if cfg!(feature = "git") { "/git and " } else { "" }
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let (limits, guard) = protection(&factory);
    let guard = Arc::new(guard);
    guard.log_start();
    tokio::spawn(sweep(Arc::clone(&guard), shutdown_rx.clone()));
    // Text responses are compressed when the client accepts gzip (`Compressible`).
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics));
    // The notification socket, unless the webapp's `/notws` endpoint has enabled="false"; the html and qvt clients
    // connect when the property is set.
    let app = if notws {
        moqui_lib::env::set_property(notws::ENDPOINT_PROPERTY, "/notws");
        notws::collab::install(&factory, shutdown_rx.clone());
        app.route("/notws", get(notws::upgrade))
    } else {
        moqui_lib::env::set_property(notws::ENDPOINT_PROPERTY, "");
        app
    };
    // The browser terminal, unless the `/termws` endpoint has enabled="false"; the account menu offers it when the
    // property is set.
    #[cfg(feature = "tui")]
    let app = if termws {
        moqui_lib::env::set_property(crate::termws::ENDPOINT_PROPERTY, "/termws");
        app.route("/termws", get(crate::termws::upgrade))
    } else {
        moqui_lib::env::set_property(crate::termws::ENDPOINT_PROPERTY, "");
        app
    };
    #[cfg(feature = "git")]
    let app = app.route("/git/{*rest}", axum::routing::any(crate::git_http::handle));
    // the event stream of an agent run and its catch-up (the Assist drawer)
    #[cfg(feature = "llm")]
    let app = crate::llm::routes(app);
    // the DAV server (CalDAV, CardDAV, WebDAV over drives), with the well-known addresses clients look for
    let app = app
        .route("/dav", axum::routing::any(crate::dav_http::handle))
        .route("/dav/", axum::routing::any(crate::dav_http::handle))
        .route("/dav/{*rest}", axum::routing::any(crate::dav_http::handle))
        .route("/.well-known/caldav", axum::routing::any(crate::dav_http::handle))
        .route("/.well-known/carddav", axum::routing::any(crate::dav_http::handle));
    let app = app.fallback(dispatch_screen);
    // The request log, Java's ElasticRequestLogFilter: off when its filter element says enabled="false".
    let app = if factory.webapp_scripts().request_log() {
        app.layer(axum::middleware::from_fn(log_http))
    } else {
        app
    };
    let app = app
        .layer(tower_http::compression::CompressionLayer::new().compress_when(Compressible))
        .with_state(Arc::new(HttpState {
            factory,
            shutdown: shutdown_rx.clone(),
            limits,
            guard: Arc::clone(&guard),
        }));
    // Both loops end when the watch turns true, which `shutdown` sets after a stop, Ctrl-C, or SIGTERM.
    let stopped = |mut rx: tokio::sync::watch::Receiver<bool>| async move {
        let _ = rx.wait_for(|stopped| *stopped).await;
    };
    let signal = tokio::spawn(shutdown(stop, shutdown_tx));
    let permits = Arc::new(tokio::sync::Semaphore::new(limits.connections));
    let plain = serve(
        listener,
        None,
        app.clone(),
        limits,
        Arc::clone(&guard),
        Arc::clone(&permits),
        stopped(shutdown_rx.clone()),
    );
    match secure {
        Some((listener, _, acceptor)) => {
            tokio::join!(
                plain,
                serve(
                    listener,
                    Some(acceptor),
                    app,
                    limits,
                    guard,
                    permits,
                    stopped(shutdown_rx)
                )
            );
        }
        None => plain.await,
    }
    let _ = signal.await;
    Ok(())
}

/// Drop the idle clients from `guard` every half minute until the listeners stop.
async fn sweep(guard: Arc<Guard>, mut stopped: tokio::sync::watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = stopped.wait_for(|stopped| *stopped) => return,
            () = tokio::time::sleep(Duration::from_secs(30)) => guard.sweep(),
        }
    }
}

/// Serve `app` until `stop` resolves, taking one of `permits` for each open connection, then let every open connection finish, as `axum::serve` does, for up to
/// `limits.shutdown`; the connections still open then are dropped with the runtime. Each
/// connection is served here rather than by `axum::serve`, which gives hyper no timer, so that
/// `header_read_timeout` takes effect. With `tls` the handshake runs in the connection's own task, so a slow
/// client cannot hold up accepting. `guard` admits each connection before its handshake and checks each request
/// before the router.
async fn serve(
    listener: TcpListener,
    tls: Option<tokio_rustls::TlsAcceptor>,
    app: Router,
    limits: ListenLimits,
    guard: Arc<Guard>,
    permits: Arc<tokio::sync::Semaphore>,
    stop: impl Future<Output = ()>,
) {
    let (closing, closing_rx) = tokio::sync::watch::channel(false);
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header)
        .max_buf_size(limits.max_header_bytes)
        .max_headers(limits.max_headers);
    let mut stop = std::pin::pin!(stop);
    loop {
        let permit = tokio::select! {
            () = &mut stop => break,
            permit = Arc::clone(&permits).acquire_owned() => permit,
        };
        let Ok(permit) = permit else {
            break;
        };
        let accepted = tokio::select! {
            () = &mut stop => break,
            accepted = listener.accept() => accepted,
        };
        let (stream, remote) = match accepted {
            Ok(pair) => pair,
            Err(err) => {
                if !matches!(
                    err.kind(),
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::ConnectionReset
                ) {
                    // Out of file descriptors, most likely: wait rather than spin.
                    tracing::warn!("http accept failed: {err}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                continue;
            }
        };
        // a denied or banned client, or one over its connections, is closed before any handshake
        let Some(slot) = guard.admit(remote.ip()) else {
            drop(stream);
            continue;
        };
        let _ = stream.set_nodelay(true);
        let router = app.clone();
        let builder = builder.clone();
        let closing = closing_rx.clone();
        let tls = tls.clone();
        let guard = Arc::clone(&guard);
        tokio::spawn(async move {
            let serving = Serving {
                remote,
                router,
                builder,
                limits,
                guard,
                closing,
            };
            match tls {
                None => run_connection(stream, false, serving).await,
                Some(acceptor) => {
                    let handshake = acceptor.accept(stream);
                    let secured = match limits.header {
                        Some(limit) => tokio::time::timeout(limit, handshake).await.ok(),
                        None => Some(handshake.await),
                    };
                    match secured {
                        Some(Ok(stream)) => run_connection(stream, true, serving).await,
                        Some(Err(err)) => tracing::debug!("TLS handshake with {remote} failed: {err}"),
                        None => tracing::debug!("TLS handshake with {remote} timed out"),
                    }
                }
            }
            drop(slot);
            drop(permit);
        });
    }
    drop(closing_rx);
    let _ = closing.send(true);
    match limits.shutdown {
        Some(deadline) => {
            if tokio::time::timeout(deadline, closing.closed()).await.is_err() {
                tracing::warn!(
                    "{} HTTP connections were still open {} seconds after the stop; the listener stops without them",
                    closing.receiver_count(),
                    deadline.as_secs()
                );
            }
        }
        None => closing.closed().await,
    }
}

/// What one connection is served with.
struct Serving {
    remote: SocketAddr,
    router: Router,
    builder: hyper::server::conn::http1::Builder,
    limits: ListenLimits,
    guard: Arc<Guard>,
    closing: tokio::sync::watch::Receiver<bool>,
}

async fn run_connection<S>(stream: S, secure: bool, serving: Serving)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let Serving {
        remote,
        router,
        builder,
        limits,
        guard,
        mut closing,
    } = serving;
    let service = hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
        let client = guard.client_of(remote.ip(), request.headers());
        request.extensions_mut().insert(ConnectInfo(remote));
        request.extensions_mut().insert(ClientAddr(client));
        if secure {
            request.extensions_mut().insert(TlsConnection);
        }
        if secure || forwarded_https(request.headers(), remote.ip(), guard.conf.addressing()) {
            request.extensions_mut().insert(HttpsRequest);
        }
        let uri_length = request.uri().path_and_query().map_or(0, |whole| whole.as_str().len());
        let refused = if uri_length > limits.max_uri {
            Some(plain(414, "the request URI is too long"))
        } else {
            let (method, path) = (request.method().as_str(), request.uri().path());
            guard
                .check(remote.ip(), client, method, path, request.headers())
                .err()
                .map(|refused| *refused.response)
        };
        // a websocket, a git clone or push (which has its own idle timeout), an upload (whose screen has the
        // deadline once it has arrived), and a model call of the LLM gateway (whose own timeout bounds it, answered in
        // the OpenAI shape) run as long as they need
        let upload = request
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim_start().to_ascii_lowercase().starts_with("multipart/"));
        let socket = matches!(request.uri().path(), "/notws" | "/termws");
        let path = request.uri().path();
        let deadline = limits
            .request
            .filter(|_| !socket && !path.starts_with("/git/") && !path.starts_with("/dav/") && !path.starts_with("/llm/v1") && !upload);
        let call = refused.is_none().then(|| router.clone().oneshot(request));
        async move {
            let Some(call) = call else {
                return Ok(refused.unwrap_or_else(|| plain(500, "refused")));
            };
            match deadline {
                Some(deadline) => match tokio::time::timeout(deadline, call).await {
                    Ok(response) => response,
                    Err(_) => Ok(plain(504, "the request took too long")),
                },
                None => call.await,
            }
        }
    });
    let connection = builder
        .serve_connection(TokioIo::new(Lingering::new(stream)), service)
        .with_upgrades();
    let mut connection = std::pin::pin!(connection);
    tokio::select! {
        result = connection.as_mut() => {
            if let Err(err) = result {
                tracing::trace!("http connection from {remote} ended: {err}");
            }
        }
        _ = closing.changed() => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}

/// How long a closed connection the client has not closed yet is read and dropped, after the write side is shut down.
const LINGER: Duration = Duration::from_secs(2);

/// A connection that closes as a lingering close does. A server that closes with request bytes still unread (a
/// request head over `max-header-bytes`, answered 431) sends a reset, and a macOS or BSD client then discards the
/// answer it has not read yet. So when the connection is dropped before the client has closed its side, the write
/// side is shut down first and whatever the client still sends is read and dropped, for up to `LINGER`.
struct Lingering<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> {
    inner: Option<S>,
    /// The client closed its side, or the connection failed: nothing is left to drain.
    ended: bool,
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> Lingering<S> {
    fn new(inner: S) -> Self {
        Self {
            inner: Some(inner),
            ended: false,
        }
    }

    fn inner(&mut self) -> std::pin::Pin<&mut S> {
        std::pin::Pin::new(self.inner.as_mut().expect("the stream is taken only on drop"))
    }
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> tokio::io::AsyncRead for Lingering<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = self.inner().poll_read(cx, buf);
        match &polled {
            std::task::Poll::Ready(Ok(())) if buf.filled().len() == before && buf.remaining() > 0 => self.ended = true,
            std::task::Poll::Ready(Err(_)) => self.ended = true,
            _ => {}
        }
        polled
    }
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> tokio::io::AsyncWrite for Lingering<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = self.inner().poll_write(cx, buf);
        if matches!(polled, std::task::Poll::Ready(Err(_))) {
            self.ended = true;
        }
        polled
    }

    fn poll_write_vectored(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = self.inner().poll_write_vectored(cx, bufs);
        if matches!(polled, std::task::Poll::Ready(Err(_))) {
            self.ended = true;
        }
        polled
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.as_ref().is_some_and(|inner| inner.is_write_vectored())
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner().poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner().poll_shutdown(cx)
    }
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> Drop for Lingering<S> {
    fn drop(&mut self) {
        let Some(mut stream) = self.inner.take() else {
            return;
        };
        if self.ended {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let _ = tokio::time::timeout(LINGER, async {
                let _ = stream.shutdown().await;
                let mut sink = [0u8; 4096];
                while matches!(stream.read(&mut sink).await, Ok(read) if read > 0) {}
            })
            .await;
        });
    }
}

/// Ctrl-c or SIGTERM ends the binary gracefully. Failed handlers wait on `stop`, used by embedded servers.
/// The watch tells open notification sockets to close so graceful shutdown can finish.
async fn shutdown(mut stop: tokio::sync::oneshot::Receiver<()>, notify: tokio::sync::watch::Sender<bool>) {
    tokio::select! {
        _ = &mut stop => {}
        _ = termination_signal() => {}
    }
    let _ = notify.send(true);
}

async fn termination_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    let terminate = async {
        #[cfg(unix)]
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
            return;
        }
        std::future::pending::<()>().await;
    };
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

/// Empty `webapp_http_port` is 8080.
pub(crate) fn http_port(raw: &str) -> Result<u16, Error> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(8080);
    }
    raw.parse::<u16>()
        .map_err(|_| Error::new(format!("webapp_http_port {raw} is not a port")))
}

async fn health(State(state): State<Arc<HttpState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Response {
    if !status_allowed(
        &state.factory.webapp_setting("webroot", "status-ips"),
        &peer.ip().to_canonical().to_string(),
    ) {
        return plain(404, "not found");
    }
    if readiness(&state.factory) {
        json(200, "{\"ready\":true}")
    } else {
        json(503, "{\"ready\":false}")
    }
}

async fn metrics(State(state): State<Arc<HttpState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Response {
    if !status_allowed(
        &state.factory.webapp_setting("webroot", "status-ips"),
        &peer.ip().to_canonical().to_string(),
    ) {
        return plain(404, "not found");
    }
    let Some(stats) = pool_stats(&state.factory) else {
        return plain(503, "service unavailable");
    };
    let depth = state.factory.request_pool().queue_depth();
    let (statements, statement_ms) = moqui_core::entity::statement_stats();
    let idle = stats.ordinary_idle.saturating_add(stats.reserve_idle);
    let mut text = format!(
        "checked_out {}\nparked {}\nwaiting {}\nidle {}\ncheckouts {}\nrequest_queue {}\nstatements {}\nstatement_ms {}\n",
        stats.checked_out, stats.parked, stats.waiting, idle, stats.checkouts, depth, statements, statement_ms
    );
    text.push_str(&state.guard.metrics());
    // With a search read replica: its replay lag (-1 when it cannot be read) and the reads that ran on the primary.
    let factory = Arc::clone(&state.factory);
    let replica = tokio::task::spawn_blocking(move || {
        moqui_core::search::installed(&factory).and_then(|facade| facade.backend().replica_status())
    })
    .await
    .ok()
    .flatten();
    if let Some(replica) = replica {
        let lag = replica.lag_seconds.unwrap_or(-1.0);
        text.push_str(&format!(
            "search_replica_lag_seconds {lag}\nsearch_replica_primary_fallbacks {}\n",
            replica.primary_fallbacks
        ));
    }
    plain(200, &text)
}

async fn dispatch_screen(
    State(state): State<Arc<HttpState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let over_tls = request.extensions().get::<TlsConnection>().is_some();
    let client = request
        .extensions()
        .get::<ClientAddr>()
        .map_or(peer.ip(), |client| client.0);
    let (parts, body) = request.into_parts();
    let mut web = WebRequest::new(parts.method.as_str(), parts.uri.path());
    web.https = over_tls || forwarded_https(&parts.headers, peer.ip(), state.guard.conf.addressing());
    web.peer_ip = peer.ip().to_string();
    web.headers = header_map(&parts.headers);
    web.query = moqui_core::web::form_pairs(parts.uri.query().unwrap_or(""));
    web.raw_query = parts.uri.query().map(str::to_string);
    // The limit is `webapp_max_body` (10 MiB), or `webapp_max_upload` for a multipart body, and it stops the read.
    let limit = moqui_core::web::body_limit_for(Some(&state.factory), &web);
    let declared = parts
        .headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > limit) {
        return plain(413, &format!("request body is larger than {limit} bytes"));
    }
    let upload = moqui_core::web::is_multipart(&web);
    if upload {
        let content_type = web.header("content-type").unwrap_or("").to_string();
        match read_multipart(&state, body, limit, &content_type).await {
            Ok(parsed) => web.multipart = Some(Arc::new(parsed)),
            Err(refused) => return refused.response(),
        }
    } else {
        let bytes = match read_body(&state, body, limit).await {
            Ok(bytes) => bytes,
            Err(refused) => return refused.response(),
        };
        web.body = bytes.to_vec();
    }
    let factory = Arc::clone(&state.factory);
    let job_factory = Arc::clone(&factory);
    let serve_factory = Arc::clone(&factory);
    let (done, result) = tokio::sync::oneshot::channel::<Option<WebResponse>>();
    let guard = Arc::clone(&state.guard);
    let queued = std::time::Instant::now();
    let queue_timeout = state.guard.conf.queue_timeout;
    // The job owns `done`. A worker that panics or a pool that stops drops it, which ends the wait. A job nobody waits
    // for any more (the deadline passed, the client went away) is not run, and one that waited past the queue timeout
    // is answered `None`, a 503, instead.
    let submitted = factory.request_pool().try_submit(job_factory, move || {
        if done.is_closed() {
            guard.counts.abandoned.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        if queue_timeout.is_some_and(|wait| queued.elapsed() > wait) {
            guard.counts.queue_expired.fetch_add(1, Ordering::Relaxed);
            let _ = done.send(None);
            return Ok(());
        }
        let response = moqui_core::screen::serve(&serve_factory, web);
        if moqui_core::web::take_auth_failure() {
            guard.record(client, Event::AuthFailure);
        }
        let _ = done.send(Some(response));
        Ok(())
    });
    if let Err(err) = submitted {
        if err.kind() == ErrorKind::Unavailable {
            state.guard.counts.queue_full.fetch_add(1, Ordering::Relaxed);
            return state.guard.unavailable("service unavailable");
        }
        return internal_error(err.message());
    }
    // an upload was exempt from the request deadline while it arrived; its screen has the deadline
    let result = match state.limits.request.filter(|_| upload) {
        Some(deadline) => match tokio::time::timeout(deadline, result).await {
            Ok(result) => result,
            Err(_) => return plain(504, "the request took too long"),
        },
        None => result.await,
    };
    match result {
        Ok(Some(response)) => from_web(&state.factory, response),
        Ok(None) => state.guard.unavailable("the server is busy; try again"),
        Err(_) => plain(500, "request worker stopped"),
    }
}

/// Bytes of an upload gathered before the reader takes them, on a blocking thread (it may write a spool file).
const UPLOAD_FEED: usize = 1024 * 1024;

/// A `multipart/form-data` body read as it arrives, up to `limit` bytes: its fields, and its files in memory or (past
/// 256 KiB) spooled to `runtime/tmp/upload`. Each chunk must come within `webapp_upload_idle_timeout`.
async fn read_multipart(
    state: &HttpState,
    body: axum::body::Body,
    limit: usize,
    content_type: &str,
) -> Result<moqui_core::web::multipart::Parsed, BodyRefused> {
    use futures_util::StreamExt as _;
    use moqui_core::web::multipart::{MultipartError, MultipartReader, Spool};

    let refused = |err: MultipartError| match err {
        MultipartError::FieldTooLarge(_) => BodyRefused::TooLarge(limit),
        MultipartError::Io(why) => BodyRefused::Failed(why),
        other => BodyRefused::Malformed(other.to_string()),
    };
    let spool = Spool::under(state.factory.runtime_dir());
    let mut reader = MultipartReader::new(content_type, Some(spool)).map_err(refused)?;
    let mut stream = body.into_data_stream();
    let mut pending: Vec<u8> = Vec::with_capacity(UPLOAD_FEED);
    let mut total = 0usize;
    loop {
        let next = match state.limits.upload_idle {
            Some(idle) => tokio::time::timeout(idle, stream.next())
                .await
                .map_err(|_| BodyRefused::TimedOut)?,
            None => stream.next().await,
        };
        let done = match next {
            Some(Ok(chunk)) => {
                total += chunk.len();
                if total > limit {
                    return Err(BodyRefused::TooLarge(limit));
                }
                pending.extend_from_slice(&chunk);
                false
            }
            Some(Err(_)) => return Err(BodyRefused::Malformed("the upload was cut short".to_string())),
            None => true,
        };
        if pending.len() >= UPLOAD_FEED || done {
            let chunk = std::mem::replace(&mut pending, Vec::with_capacity(UPLOAD_FEED));
            let (back, fed) = tokio::task::spawn_blocking(move || {
                let fed = reader.feed(&chunk);
                (reader, fed)
            })
            .await
            .map_err(|err| BodyRefused::Failed(err.to_string()))?;
            reader = back;
            fed.map_err(refused)?;
        }
        if done {
            return reader.finish().map_err(refused);
        }
    }
}

/// Why [`read_body`] or [`read_multipart`] stopped.
pub(crate) enum BodyRefused {
    /// More than this many bytes.
    TooLarge(usize),
    TimedOut,
    /// An upload that does not follow its boundary.
    Malformed(String),
    /// An upload that could not be spooled.
    Failed(String),
}

impl BodyRefused {
    pub(crate) fn response(&self) -> Response {
        match self {
            Self::TooLarge(limit) => plain(413, &format!("request body is larger than {limit} bytes")),
            Self::TimedOut => plain(408, "the request body did not arrive in time"),
            Self::Malformed(why) => plain(400, why),
            Self::Failed(why) => internal_error(why),
        }
    }
}

/// The request body, up to `limit` bytes and within `webapp_body_timeout`.
pub(crate) async fn read_body(
    state: &HttpState,
    body: axum::body::Body,
    limit: usize,
) -> Result<bytes::Bytes, BodyRefused> {
    let read = axum::body::to_bytes(body, limit.saturating_add(1));
    let read = match state.limits.body {
        Some(timeout) => tokio::time::timeout(timeout, read)
            .await
            .map_err(|_| BodyRefused::TimedOut)?,
        None => read.await,
    };
    match read {
        Ok(bytes) if bytes.len() <= limit => Ok(bytes),
        _ => Err(BodyRefused::TooLarge(limit)),
    }
}

/// A 500 that names an error id instead of `detail`, which may hold SQL, paths or entity names. The detail goes
/// to the error log under the same id.
pub(crate) fn internal_error(detail: &str) -> Response {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = format!(
        "{:x}-{:x}",
        crate::logging::now_millis(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    tracing::error!("request failed, error id {id}: {detail}");
    plain(500, &format!("internal error, id {id}"))
}

/// One `moqui_http_log` entry, the fields of Java's `ElasticRequestLogFilter`. Queued only while
/// logs ship to the search backend.
struct RequestLog {
    started_millis: i64,
    client_ip: String,
    method: String,
    https: bool,
    host: String,
    path: String,
    query: String,
    version: axum::http::Version,
    referrer: String,
    agent: String,
}

impl RequestLog {
    fn queue(self, response: &Response, identity: Option<&LoggedIdentity>, elapsed: std::time::Duration) {
        use moqui_lib::object::Value;

        let millis = i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX);
        let version = match self.version {
            axum::http::Version::HTTP_09 => 0.9,
            axum::http::Version::HTTP_10 => 1.0,
            axum::http::Version::HTTP_2 => 2.0,
            axum::http::Version::HTTP_3 => 3.0,
            _ => 1.1,
        };
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| response.body().size_hint().exact())
            .unwrap_or(0);
        let mut entry = moqui_core::entity::document::Document::new();
        entry.insert("@timestamp".into(), Value::I64(self.started_millis));
        entry.insert("remote_ip".into(), Value::String(self.client_ip));
        entry.insert("server_ip".into(), Value::string(crate::logging::host_name()));
        if let Some(content_type) = content_type {
            entry.insert("content_type".into(), Value::String(content_type));
        }
        entry.insert("request_method".into(), Value::String(self.method));
        entry.insert(
            "request_scheme".into(),
            Value::string(if self.https { "https" } else { "http" }),
        );
        entry.insert("request_host".into(), Value::String(self.host));
        entry.insert("request_path".into(), Value::String(self.path));
        if !self.query.is_empty() {
            entry.insert("request_query".into(), Value::String(self.query));
        }
        entry.insert("http_version".into(), Value::F64(version));
        entry.insert("response".into(), Value::I64(i64::from(response.status().as_u16())));
        entry.insert("time_initial_ms".into(), Value::I64(millis));
        entry.insert("time_final_ms".into(), Value::I64(millis));
        entry.insert("bytes".into(), Value::I64(i64::try_from(bytes).unwrap_or(i64::MAX)));
        if !self.referrer.is_empty() {
            entry.insert("referrer".into(), Value::String(self.referrer));
        }
        if !self.agent.is_empty() {
            entry.insert("agent".into(), Value::String(self.agent));
        }
        if let Some(identity) = identity {
            if let Some(session) = identity.session.clone() {
                entry.insert("session".into(), Value::String(session));
            }
            if let Some(visitor_id) = identity.visitor_id.clone() {
                entry.insert("visitor_id".into(), Value::String(visitor_id));
            }
            if let Some(remote_user) = identity.remote_user.clone() {
                entry.insert("remote_user".into(), Value::String(remote_user));
            }
        }
        crate::logging::queue_http_request(entry);
    }
}

/// One `moqui_http_log` entry for every route, including a refusal before the screen worker.
async fn log_http(request: Request, next: Next) -> Response {
    let started = std::time::Instant::now();
    let started_millis = crate::logging::now_millis();
    let method = request.method().as_str().to_string();
    let https = request.extensions().get::<HttpsRequest>().is_some();
    let client_ip = request
        .extensions()
        .get::<ClientAddr>()
        .map(|client| client.0.to_string())
        .or_else(|| {
            request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|peer| peer.0.ip().to_string())
        })
        .unwrap_or_default();
    let host = request_header(request.headers(), "host");
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or("").to_string();
    let version = request.version();
    let referrer = request_header(request.headers(), "referer");
    let agent = request_header(request.headers(), "user-agent");
    let response = next.run(request).await;
    let identity = response.extensions().get::<LoggedIdentity>().cloned();
    RequestLog {
        started_millis,
        client_ip,
        method,
        https,
        host,
        path,
        query,
        version,
        referrer,
        agent,
    }
    .queue(&response, identity.as_ref(), started.elapsed());
    response
}

fn request_header(headers: &axum::http::HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// `X-Forwarded-Proto` is believed only when `client-ip-header` marks a reverse proxy in front, and `peer` is one of
/// the `trusted-proxies` when there are any. A direct client could otherwise force `Secure` cookies and HSTS.
pub(crate) fn forwarded_https(
    headers: &axum::http::HeaderMap,
    peer: IpAddr,
    addressing: &moqui_core::web::address::ClientAddressing,
) -> bool {
    if !addressing.believes(peer) {
        return false;
    }
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("https")
        })
}

pub(crate) fn header_map(headers: &axum::http::HeaderMap) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for name in headers.keys() {
        let values: Vec<&str> = headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect();
        if values.is_empty() {
            continue;
        }
        // Repeated headers are one list, as the servlet's `getHeader` joins them; cookies are pairs split on `;`
        // (RFC 6265), so a second `Cookie` header must not run into the last pair of the first.
        let separator = if *name == axum::http::header::COOKIE {
            "; "
        } else {
            ", "
        };
        map.insert(name.as_str().to_ascii_lowercase(), values.join(separator));
    }
    map
}

fn readiness(factory: &ExecutionContextFactory) -> bool {
    pool_stats(factory).is_some()
}

fn pool_stats(factory: &ExecutionContextFactory) -> Option<PoolStats> {
    let facade = factory.entity()?;
    let transactions = facade.transactions_for("moqui.basic.Enumeration").ok()?;
    Some(transactions.stats())
}

/// False only when the `webroot` webapp's `endpoint` at `path` has `enabled="false"`.
fn endpoint_enabled(factory: &ExecutionContextFactory, path: &str) -> bool {
    factory.webapp_child_setting("webroot", &format!("endpoint {path}"), "enabled") != "false"
}

/// True when `ip` is in `configured` (the webapp's `status-ips`, comma-separated) or is a loopback address.
fn status_allowed(configured: &str, ip: &str) -> bool {
    let mut allowed = Vec::new();
    for part in configured.split(',') {
        let part = part.trim();
        if !part.is_empty() {
            allowed.push(part.to_string());
        }
    }
    for extra in ["127.0.0.1", "::1", "0:0:0:0:0:0:0:1"] {
        if !allowed.iter().any(|item| item == extra) {
            allowed.push(extra.to_string());
        }
    }
    allowed.iter().any(|item| item == ip)
}

fn from_web(factory: &Arc<moqui_core::context::ExecutionContextFactory>, response: WebResponse) -> Response {
    let http_log = response.http_log.clone();
    let mut builder = Response::builder().status(response.status);
    for (name, value) in response.headers {
        builder = builder.header(name, value);
    }
    let body = match response.stream {
        Some(stream) => crate::source_body::body(Arc::clone(factory), stream),
        None => axum::body::Body::from(response.body),
    };
    let mut built = builder
        .body(body)
        .unwrap_or_else(|_| plain(500, "could not build the response"));
    if http_log.session.is_some() || http_log.visitor_id.is_some() || http_log.remote_user.is_some() {
        built.extensions_mut().insert(LoggedIdentity {
            session: http_log.session,
            visitor_id: http_log.visitor_id,
            remote_user: http_log.remote_user,
        });
    }
    built
}

/// The responses worth compressing: text (and JSON, JavaScript, XML, SVG, WebAssembly) over 32 bytes, not a range
/// (whose `Content-Range` names the bytes as they are). Media, archives, PDFs and git packs are compressed already, and
/// compressing one would also lose its `Content-Length` and ranges.
#[derive(Clone, Copy)]
struct Compressible;

impl tower_http::compression::Predicate for Compressible {
    fn should_compress<B>(&self, response: &axum::http::Response<B>) -> bool
    where
        B: axum::body::HttpBody,
    {
        if response.status() == axum::http::StatusCode::PARTIAL_CONTENT
            || response.headers().contains_key(axum::http::header::CONTENT_RANGE)
        {
            return false;
        }
        let small = response
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length <= 32);
        let kind = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
            .unwrap_or_default();
        // an event stream is sent as it comes; a compressor would hold it back
        !small
            && kind != "text/event-stream"
            && (kind.starts_with("text/")
                || kind.ends_with("+json")
                || kind.ends_with("+xml")
                || matches!(
                    kind.as_str(),
                    "application/json"
                        | "application/javascript"
                        | "application/xml"
                        | "application/wasm"
                        | "application/x-www-form-urlencoded"
                        | "application/graphql"
                        | "image/svg+xml"
                        | "image/x-icon"
                        | "font/ttf"
                        | "font/otf"
                        | "application/vnd.ms-fontobject"
                ))
    }
}

fn json(status: u16, body: &str) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|_| plain(500, "could not build the response"))
}

pub(crate) fn plain(status: u16, body: &str) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|_| Response::new(axum::body::Body::empty()))
}

#[cfg(all(test, unix))]
mod signal_tests {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    #[test]
    fn process_signals_notify_graceful_shutdown() {
        const CHILD: &str = "MOQUI_SHUTDOWN_SIGNAL_TEST";
        if std::env::var_os(CHILD).is_some() {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let (_stop, receiver) = tokio::sync::oneshot::channel();
                let (notify, mut notified) = tokio::sync::watch::channel(false);
                let shutdown = super::shutdown(receiver, notify);
                tokio::pin!(shutdown);
                // Poll shutdown to register the signal handlers before telling the parent to send a signal.
                assert!(tokio::time::timeout(Duration::from_millis(50), &mut shutdown)
                    .await
                    .is_err());
                println!("signal handlers ready");
                shutdown.await;
                notified.changed().await.unwrap();
                assert!(*notified.borrow());
            });
            return;
        }
        for signal in ["-TERM", "-INT"] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "http::signal_tests::process_signals_notify_graceful_shutdown",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let stdout = child.stdout.take().unwrap();
            let (ready, receiver) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                    if line == "signal handlers ready" {
                        let _ = ready.send(());
                    }
                }
            });
            let registered = receiver.recv_timeout(Duration::from_secs(10)).is_ok();
            if registered {
                assert!(Command::new("kill")
                    .args([signal, &child.id().to_string()])
                    .status()
                    .unwrap()
                    .success());
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break Some(status);
                }
                if !registered || std::time::Instant::now() > deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            reader.join().unwrap();
            assert!(
                registered && status.is_some_and(|status| status.success()),
                "{signal}: {status:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use axum::extract::Request;
    use axum::routing::get;
    use axum::Router;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Semaphore;

    use super::{header_map, http_port, serve, status_allowed, ListenLimits, TlsConnection};
    use crate::guard::Guard;
    use crate::tls;

    fn unguarded() -> Arc<Guard> {
        Arc::new(Guard::disabled())
    }

    async fn read_all(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
            .await
            .expect("the listener did not close the connection")
            .unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[tokio::test]
    async fn a_screen_request_is_logged_with_its_user_and_health_without_one() {
        use moqui_lib::object::Value;
        use tower::ServiceExt as _;

        let _hold = crate::logging::begin_http_capture().await;
        let app = Router::new()
            .route("/health", get(|| async { "ok" }))
            .route(
                "/screen",
                get(|| async {
                    let mut response = axum::response::Response::new(axum::body::Body::from("hi"));
                    response.extensions_mut().insert(super::LoggedIdentity {
                        session: Some("session-1".to_string()),
                        visitor_id: Some("visitor-1".to_string()),
                        remote_user: Some("ada".to_string()),
                    });
                    response
                }),
            )
            .layer(axum::middleware::from_fn(super::log_http));
        let screen = Request::builder()
            .uri("/screen?tab=1")
            .body(axum::body::Body::empty())
            .unwrap();
        app.clone().oneshot(screen).await.unwrap();
        let health = Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();
        app.oneshot(health).await.unwrap();
        let entries = crate::logging::take_http_capture();
        let screen = entries
            .iter()
            .find(|entry| entry.get("request_path").and_then(Value::as_str) == Some("/screen"))
            .expect("screen request was not logged");
        assert_eq!(screen.get("session").and_then(Value::as_str), Some("session-1"));
        assert_eq!(screen.get("visitor_id").and_then(Value::as_str), Some("visitor-1"));
        assert_eq!(screen.get("remote_user").and_then(Value::as_str), Some("ada"));
        assert_eq!(screen.get("request_query").and_then(Value::as_str), Some("tab=1"));
        let health = entries
            .iter()
            .find(|entry| entry.get("request_path").and_then(Value::as_str) == Some("/health"))
            .expect("health request was not logged");
        assert!(health.get("session").is_none(), "{health:?}");
        assert!(health.get("visitor_id").is_none(), "{health:?}");
        assert!(health.get("remote_user").is_none(), "{health:?}");
    }

    #[tokio::test]
    async fn a_slow_request_head_is_closed_and_holds_no_connection_slot() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/", get(|| async { "ok" }));
        let limits = ListenLimits {
            header: Some(Duration::from_millis(300)),
            connections: 1,
            shutdown: None,
            ..ListenLimits::defaults()
        };
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let permits = Arc::new(Semaphore::new(limits.connections));
        let server = tokio::spawn(serve(listener, None, app, limits, unguarded(), permits, async move {
            let _ = stopped.await;
        }));

        // The one slot is taken by a client that never finishes its request head.
        let started = Instant::now();
        let mut slow = TcpStream::connect(address).await.unwrap();
        slow.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n").await.unwrap();
        // A full request waits for that slot, which the header timeout frees.
        let mut fast = TcpStream::connect(address).await.unwrap();
        fast.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let answered = read_all(&mut fast).await;
        assert!(answered.starts_with("HTTP/1.1 200"), "{answered}");
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "{:?}",
            started.elapsed()
        );

        let closed = read_all(&mut slow).await;
        assert!(!closed.contains(" 200 "), "{closed}");
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());

        drop(stop);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the listener did not stop")
            .unwrap();
    }

    #[tokio::test]
    async fn a_stop_waits_for_open_requests_only_until_the_shutdown_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/", get(std::future::pending::<&'static str>));
        let limits = ListenLimits {
            header: None,
            connections: 4,
            shutdown: Some(Duration::from_millis(300)),
            ..ListenLimits::defaults()
        };
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let permits = Arc::new(Semaphore::new(limits.connections));
        let server = tokio::spawn(serve(listener, None, app, limits, unguarded(), permits, async move {
            let _ = stopped.await;
        }));
        let mut stuck = TcpStream::connect(address).await.unwrap();
        stuck.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let started = Instant::now();
        drop(stop);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the listener waited past its shutdown deadline")
            .unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "{:?}",
            started.elapsed()
        );
    }

    /// A request over the TLS listener reaches the router marked as secure, and one over plain HTTP does not.
    #[tokio::test]
    async fn a_request_over_tls_is_marked_secure() {
        let dir = tls::tests::temp_dir("serve").join("cert");
        tls::ensure_dev_cert(&dir).unwrap();
        let config = Arc::new(tls::server_config(tls::CertStore::new(dir.clone())).unwrap());
        let app = Router::new().route(
            "/",
            get(|request: Request| async move {
                if request.extensions().get::<TlsConnection>().is_some() {
                    "secure"
                } else {
                    "plain"
                }
            }),
        );
        let limits = ListenLimits {
            header: Some(Duration::from_secs(5)),
            connections: 4,
            shutdown: None,
            ..ListenLimits::defaults()
        };
        let permits = Arc::new(Semaphore::new(limits.connections));
        let (stop_secure, secure_stopped) = tokio::sync::oneshot::channel::<()>();
        let (stop_plain, plain_stopped) = tokio::sync::oneshot::channel::<()>();
        let secure_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let secure_address = secure_listener.local_addr().unwrap();
        let plain_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let plain_address = plain_listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            tokio::join!(
                serve(
                    secure_listener,
                    Some(tokio_rustls::TlsAcceptor::from(config)),
                    app.clone(),
                    limits,
                    unguarded(),
                    Arc::clone(&permits),
                    async move {
                        let _ = secure_stopped.await;
                    },
                ),
                serve(plain_listener, None, app, limits, unguarded(), permits, async move {
                    let _ = plain_stopped.await;
                }),
            );
        });

        let stream = TcpStream::connect(secure_address).await.unwrap();
        let mut secure = tls::tests::client_trusting(&dir.join("localhost.crt"))
            .connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(), stream)
            .await
            .expect("localhost handshake");
        secure
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut answered = String::new();
        let _ = secure.read_to_string(&mut answered).await;
        assert!(answered.starts_with("HTTP/1.1 200"), "{answered}");
        assert!(answered.ends_with("secure"), "{answered}");

        let mut plain = TcpStream::connect(plain_address).await.unwrap();
        plain
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let answered = read_all(&mut plain).await;
        assert!(answered.ends_with("plain"), "{answered}");

        drop(stop_secure);
        drop(stop_plain);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the listeners did not stop")
            .unwrap();
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// A listener with `guard`, answering `ok` to everything the guard lets through, until the sender drops.
    async fn guarded(
        guard: Arc<Guard>,
        limits: ListenLimits,
    ) -> (
        std::net::SocketAddr,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().fallback(|| async { "ok" });
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let permits = Arc::new(Semaphore::new(limits.connections));
        let server = tokio::spawn(serve(listener, None, app, limits, guard, permits, async move {
            let _ = stopped.await;
        }));
        (address, stop, server)
    }

    async fn ask(address: std::net::SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        read_all(&mut stream).await
    }

    #[tokio::test]
    async fn the_guard_and_the_request_limits_answer_on_the_socket() {
        let guard = crate::guard::tests::guard(
            r#"<web-protection enabled="true">
                <rate-limit name="all" requests="1" seconds="60" burst="2"/>
                <path-rule action="deny" path="/.env"/>
            </web-protection>"#,
            "",
        );
        let limits = ListenLimits {
            header: Some(Duration::from_secs(5)),
            max_uri: 64,
            max_header_bytes: 8192,
            ..ListenLimits::defaults()
        };
        let (address, stop, server) = guarded(guard, limits).await;
        let get = |path: &str| format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");

        let answered = ask(address, &get("/.env")).await;
        assert!(answered.starts_with("HTTP/1.1 404"), "{answered}");
        // a burst of two, then the rate of one a minute
        for _ in 0..2 {
            let answered = ask(address, &get("/")).await;
            assert!(answered.starts_with("HTTP/1.1 200"), "{answered}");
        }
        let answered = ask(address, &get("/")).await;
        assert!(answered.starts_with("HTTP/1.1 429"), "{answered}");
        assert!(answered.to_ascii_lowercase().contains("retry-after: "), "{answered}");

        let long = format!("/{}", "a".repeat(100));
        let answered = ask(address, &get(&long)).await;
        assert!(answered.starts_with("HTTP/1.1 414"), "{answered}");
        let big = format!(
            "GET / HTTP/1.1\r\nHost: x\r\nX-Big: {}\r\nConnection: close\r\n\r\n",
            "b".repeat(9000)
        );
        let answered = ask(address, &big).await;
        assert!(answered.starts_with("HTTP/1.1 431"), "{answered}");

        drop(stop);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the listener did not stop")
            .unwrap();
    }

    #[tokio::test]
    async fn a_client_over_its_connections_is_closed_before_its_request() {
        let guard = crate::guard::tests::guard(
            r#"<web-protection enabled="true"><connections max-per-client="1"/></web-protection>"#,
            "",
        );
        let limits = ListenLimits {
            header: Some(Duration::from_secs(5)),
            ..ListenLimits::defaults()
        };
        let (address, stop, server) = guarded(guard, limits).await;
        // the one connection this client may hold, open with half a request
        let mut held = TcpStream::connect(address).await.unwrap();
        held.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        // closed without an answer: an end of stream, or a reset when the request was still unread
        let mut refused = TcpStream::connect(address).await.unwrap();
        let _ = refused
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await;
        let mut buf = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(5), refused.read_to_end(&mut buf))
            .await
            .expect("the refused connection was not closed");
        assert!(read.is_err() || buf.is_empty(), "{}", String::from_utf8_lossy(&buf));
        // given back when it closes
        drop(held);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let answered = ask(address, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await;
        assert!(answered.starts_with("HTTP/1.1 200"), "{answered}");

        drop(stop);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the listener did not stop")
            .unwrap();
    }

    /// A header sent twice is one value; two `Cookie` headers join as one cookie list, so each pair stays whole.
    #[test]
    fn repeated_headers_join_and_cookies_join_as_pairs() {
        let mut headers = axum::http::HeaderMap::new();
        headers.append(axum::http::header::COOKIE, "a=1".parse().unwrap());
        headers.append(axum::http::header::COOKIE, "moqui.session=abc".parse().unwrap());
        headers.append("x-forwarded-for", "10.0.0.1".parse().unwrap());
        headers.append("x-forwarded-for", "10.0.0.2".parse().unwrap());
        let map = header_map(&headers);
        assert_eq!(map.get("cookie").map(String::as_str), Some("a=1; moqui.session=abc"));
        assert_eq!(
            map.get("x-forwarded-for").map(String::as_str),
            Some("10.0.0.1, 10.0.0.2")
        );
        assert_eq!(
            moqui_core::web::cookie_value(&map["cookie"], "moqui.session").as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn empty_webapp_http_port_is_8080() {
        assert_eq!(http_port("").unwrap(), 8080);
        assert_eq!(http_port("  ").unwrap(), 8080);
        assert_eq!(http_port("9090").unwrap(), 9090);
        assert!(http_port("nope").is_err());
    }

    #[test]
    fn status_routes_allow_loopback_only() {
        assert!(status_allowed("", "127.0.0.1"));
        assert!(status_allowed("", "::1"));
        assert!(status_allowed("", "0:0:0:0:0:0:0:1"));
        // an IPv4 peer seen through a dual-stack socket is its IPv4 address
        let mapped: std::net::IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(status_allowed("", &mapped.to_canonical().to_string()));
        assert!(!status_allowed("127.0.0.1", "203.0.113.5"));
        assert!(status_allowed("10.0.0.9, 203.0.113.5", "203.0.113.5"));
    }
}
