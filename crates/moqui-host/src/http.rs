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
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::response::Response;
use axum::routing::{any, get};
use axum::Router;
use hyper_util::rt::{TokioIo, TokioTimer};
use moqui_core::context::ErrorKind;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::transaction::PoolStats;
use moqui_core::web::{WebRequest, WebResponse};
use moqui_lib::error::Error;
use tokio::net::TcpListener;
use tower::ServiceExt as _;

use crate::{notws, search_proxy};

pub(crate) struct HttpState {
    pub factory: Arc<ExecutionContextFactory>,
    pub shutdown: tokio::sync::watch::Receiver<bool>,
    pub limits: ListenLimits,
}

/// Bounds on the public listeners. Timeouts are whole seconds in their property, and `0` turns one off.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ListenLimits {
    /// `webapp_header_timeout`, default 30: a client sends its whole request head in this time. A kept-alive
    /// connection that sends nothing for as long is closed too.
    pub header: Option<Duration>,
    /// `webapp_body_timeout`, default 60: the whole request body arrives in this time, or the answer is 408.
    pub body: Option<Duration>,
    /// `webapp_request_timeout`, default 300: from the request head to the response head, or the answer is 504.
    /// `/notws` is exempt, and a response body, such as a long download, is sent after it.
    pub request: Option<Duration>,
    /// `webapp_max_connections`, default 1024: connections served at once, HTTP and HTTPS together. Past it the
    /// listeners stop accepting until one closes.
    pub connections: usize,
    /// `webapp_shutdown_timeout`, default 30: after a stop, how long open connections may take to finish before the
    /// listener stops without them.
    pub shutdown: Option<Duration>,
}

impl ListenLimits {
    pub(crate) fn from_properties() -> Self {
        Self {
            header: seconds_property("webapp_header_timeout", 30),
            body: seconds_property("webapp_body_timeout", 60),
            request: seconds_property("webapp_request_timeout", 300),
            connections: moqui_lib::env::get_prop_or_env("webapp_max_connections")
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|count| *count > 0)
                .unwrap_or(1024),
            shutdown: seconds_property("webapp_shutdown_timeout", 30),
        }
    }
}

/// Seconds from `name`, `default` when it is empty or not a number, `None` when it is `0`.
fn seconds_property(name: &str, default: u64) -> Option<Duration> {
    let seconds = moqui_lib::env::get_prop_or_env(name)
        .trim()
        .parse::<u64>()
        .unwrap_or(default);
    (seconds > 0).then(|| Duration::from_secs(seconds))
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
struct TlsConnection;

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
    tracing::info!(
        "Listening for HTTP on {address}{https_text} and cluster on {cluster}; routes /health, /metrics, /notws, /elastic, /kibana and the screen dispatcher"
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let limits = ListenLimits::from_properties();
    // Text responses are compressed when the client accepts gzip. Images and other types that are
    // compressed already are left alone by the layer's default predicate.
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/notws", get(notws::upgrade))
        .route("/elastic", any(search_proxy::elastic))
        .route("/elastic/", any(search_proxy::elastic))
        .route("/elastic/{*rest}", any(search_proxy::elastic))
        .route("/kibana", any(search_proxy::kibana))
        .route("/kibana/", any(search_proxy::kibana))
        .route("/kibana/{*rest}", any(search_proxy::kibana))
        .fallback(dispatch_screen)
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(Arc::new(HttpState {
            factory,
            shutdown: shutdown_rx.clone(),
            limits,
        }));
    // Both loops end when the watch turns true, which `shutdown` sets after a stop or ctrl-c.
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
        Arc::clone(&permits),
        stopped(shutdown_rx.clone()),
    );
    match secure {
        Some((listener, _, acceptor)) => {
            tokio::join!(
                plain,
                serve(listener, Some(acceptor), app, limits, permits, stopped(shutdown_rx))
            );
        }
        None => plain.await,
    }
    let _ = signal.await;
    Ok(())
}

/// Serve `app` until `stop` resolves, taking one of `permits` for each open connection, then let every open connection finish, as `axum::serve` does, for up to
/// `limits.shutdown`; the connections still open then are dropped with the runtime. Each
/// connection is served here rather than by `axum::serve`, which gives hyper no timer, so that
/// `header_read_timeout` takes effect. With `tls` the handshake runs in the connection's own task, so a slow
/// client cannot hold up accepting.
async fn serve(
    listener: TcpListener,
    tls: Option<tokio_rustls::TlsAcceptor>,
    app: Router,
    limits: ListenLimits,
    permits: Arc<tokio::sync::Semaphore>,
    stop: impl Future<Output = ()>,
) {
    let (closing, closing_rx) = tokio::sync::watch::channel(false);
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.timer(TokioTimer::new()).header_read_timeout(limits.header);
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
        let _ = stream.set_nodelay(true);
        let router = app.clone();
        let builder = builder.clone();
        let closing = closing_rx.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            match tls {
                None => run_connection(stream, remote, false, router, builder, limits, closing).await,
                Some(acceptor) => {
                    let handshake = acceptor.accept(stream);
                    let secured = match limits.header {
                        Some(limit) => tokio::time::timeout(limit, handshake).await.ok(),
                        None => Some(handshake.await),
                    };
                    match secured {
                        Some(Ok(stream)) => {
                            run_connection(stream, remote, true, router, builder, limits, closing).await
                        }
                        Some(Err(err)) => tracing::debug!("TLS handshake with {remote} failed: {err}"),
                        None => tracing::debug!("TLS handshake with {remote} timed out"),
                    }
                }
            }
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

async fn run_connection<S>(
    stream: S,
    remote: SocketAddr,
    secure: bool,
    router: Router,
    builder: hyper::server::conn::http1::Builder,
    limits: ListenLimits,
    mut closing: tokio::sync::watch::Receiver<bool>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
        request.extensions_mut().insert(ConnectInfo(remote));
        if secure {
            request.extensions_mut().insert(TlsConnection);
        }
        let deadline = limits.request.filter(|_| request.uri().path() != "/notws");
        let call = router.clone().oneshot(request);
        async move {
            match deadline {
                Some(deadline) => match tokio::time::timeout(deadline, call).await {
                    Ok(response) => response,
                    Err(_) => Ok(plain(504, "the request took too long")),
                },
                None => call.await,
            }
        }
    });
    let connection = builder.serve_connection(TokioIo::new(stream), service).with_upgrades();
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

/// Ctrl-c ends the binary. A failed handler waits on `stop`, which is how tests shut the listener down.
/// The watch tells open notification sockets to close so graceful shutdown can finish.
async fn shutdown(mut stop: tokio::sync::oneshot::Receiver<()>, notify: tokio::sync::watch::Sender<bool>) {
    tokio::select! {
        _ = &mut stop => {}
        result = tokio::signal::ctrl_c() => {
            if result.is_err() {
                let _ = stop.await;
            }
        }
    }
    let _ = notify.send(true);
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
    if !status_allowed(&peer.ip().to_string()) {
        return plain(404, "not found");
    }
    if readiness(&state.factory) {
        json(200, "{\"ready\":true}")
    } else {
        json(503, "{\"ready\":false}")
    }
}

async fn metrics(State(state): State<Arc<HttpState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>) -> Response {
    if !status_allowed(&peer.ip().to_string()) {
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
    let started = std::time::Instant::now();
    let started_millis = crate::logging::now_millis();
    let over_tls = request.extensions().get::<TlsConnection>().is_some();
    let (parts, body) = request.into_parts();
    let mut web = WebRequest::new(parts.method.as_str(), parts.uri.path());
    web.https = over_tls || forwarded_https(&parts.headers);
    web.peer_ip = peer.ip().to_string();
    web.headers = header_map(&parts.headers);
    web.query = moqui_core::web::form_pairs(parts.uri.query().unwrap_or(""));
    // The limit is `webapp_max_body` (10 MiB), or `webapp_max_upload` for a multipart body, and it stops the read.
    let limit = moqui_core::web::body_limit(&web);
    let declared = parts
        .headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > limit) {
        return plain(413, &format!("request body is larger than {limit} bytes"));
    }
    let bytes = match read_body(&state, body, limit).await {
        Ok(bytes) => bytes,
        Err(refused) => return refused.response(),
    };
    web.body = bytes.to_vec();
    let log = RequestLog {
        started_millis,
        client_ip: moqui_core::web::client_ip(&web),
        method: parts.method.as_str().to_string(),
        https: web.https,
        host: web.header("host").unwrap_or("").to_string(),
        path: parts.uri.path().to_string(),
        query: parts.uri.query().unwrap_or("").to_string(),
        version: parts.version,
        referrer: web.header("referer").unwrap_or("").to_string(),
        agent: web.header("user-agent").unwrap_or("").to_string(),
    };
    let factory = Arc::clone(&state.factory);
    let job_factory = Arc::clone(&factory);
    let serve_factory = Arc::clone(&factory);
    let (done, result) = tokio::sync::oneshot::channel();
    // The job owns `done`. A worker that panics or a pool that stops drops it, which ends the wait.
    let submitted = factory.request_pool().try_submit(job_factory, move || {
        let _ = done.send(moqui_core::screen::serve(&serve_factory, web));
        Ok(())
    });
    if let Err(err) = submitted {
        if err.kind() == ErrorKind::Unavailable {
            return plain(503, "service unavailable");
        }
        return internal_error(err.message());
    }
    match result.await {
        Ok(response) => {
            log.queue(&response, started.elapsed());
            from_web(response)
        }
        Err(_) => plain(500, "request worker stopped"),
    }
}

/// Why [`read_body`] stopped.
pub(crate) enum BodyRefused {
    /// More than this many bytes.
    TooLarge(usize),
    TimedOut,
}

impl BodyRefused {
    pub(crate) fn response(&self) -> Response {
        match self {
            Self::TooLarge(limit) => plain(413, &format!("request body is larger than {limit} bytes")),
            Self::TimedOut => plain(408, "the request body did not arrive in time"),
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
    fn queue(self, response: &WebResponse, elapsed: std::time::Duration) {
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
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.clone());
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
        entry.insert("response".into(), Value::I64(i64::from(response.status)));
        entry.insert("time_initial_ms".into(), Value::I64(millis));
        entry.insert("time_final_ms".into(), Value::I64(millis));
        entry.insert(
            "bytes".into(),
            Value::I64(i64::try_from(response.body.len()).unwrap_or(i64::MAX)),
        );
        if !self.referrer.is_empty() {
            entry.insert("referrer".into(), Value::String(self.referrer));
        }
        if !self.agent.is_empty() {
            entry.insert("agent".into(), Value::String(self.agent));
        }
        crate::logging::queue_http_request(entry);
    }
}

/// `X-Forwarded-Proto` is believed only when `webapp_client_ip_header` marks a reverse proxy in front.
/// A direct client could otherwise force `Secure` cookies and HSTS.
fn forwarded_https(headers: &axum::http::HeaderMap) -> bool {
    if moqui_lib::env::get_prop_or_env("webapp_client_ip_header").is_empty() {
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
        map.insert(name.as_str().to_ascii_lowercase(), values.join(", "));
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

fn status_allowed(ip: &str) -> bool {
    let mut allowed = Vec::new();
    let configured = moqui_lib::env::get_prop_or_env("webapp_status_ips");
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

fn from_web(response: WebResponse) -> Response {
    let mut builder = Response::builder().status(response.status);
    for (name, value) in response.headers {
        builder = builder.header(name, value);
    }
    builder
        .body(axum::body::Body::from(response.body))
        .unwrap_or_else(|_| plain(500, "could not build the response"))
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

    use super::{http_port, serve, status_allowed, ListenLimits, TlsConnection};
    use crate::tls;

    async fn read_all(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
            .await
            .expect("the listener did not close the connection")
            .unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[tokio::test]
    async fn a_slow_request_head_is_closed_and_holds_no_connection_slot() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route("/", get(|| async { "ok" }));
        let limits = ListenLimits {
            header: Some(Duration::from_millis(300)),
            body: None,
            request: None,
            connections: 1,
            shutdown: None,
        };
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let permits = Arc::new(Semaphore::new(limits.connections));
        let server = tokio::spawn(serve(listener, None, app, limits, permits, async move {
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
            body: None,
            request: None,
            connections: 4,
            shutdown: Some(Duration::from_millis(300)),
        };
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let permits = Arc::new(Semaphore::new(limits.connections));
        let server = tokio::spawn(serve(listener, None, app, limits, permits, async move {
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
            body: None,
            request: None,
            connections: 4,
            shutdown: None,
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
                    Arc::clone(&permits),
                    async move {
                        let _ = secure_stopped.await;
                    },
                ),
                serve(plain_listener, None, app, limits, permits, async move {
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

    #[test]
    fn empty_webapp_http_port_is_8080() {
        assert_eq!(http_port("").unwrap(), 8080);
        assert_eq!(http_port("  ").unwrap(), 8080);
        assert_eq!(http_port("9090").unwrap(), 9090);
        assert!(http_port("nope").is_err());
    }

    #[test]
    fn status_routes_allow_loopback_only() {
        assert!(status_allowed("127.0.0.1"));
        assert!(status_allowed("::1"));
        assert!(status_allowed("0:0:0:0:0:0:0:1"));
        let configured = moqui_lib::env::get_prop_or_env("webapp_status_ips");
        if !configured.split(',').any(|part| part.trim() == "203.0.113.5") {
            assert!(!status_allowed("203.0.113.5"));
        }
    }
}
