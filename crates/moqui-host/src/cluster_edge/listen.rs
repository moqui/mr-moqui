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

//! The cluster listener (plans/archive/Cluster.md), on the host's runtime (`crate::edge::handle`): TLS with the shared
//! cluster certificate, which the client must present too, then by ALPN gRPC on HTTP/2 (`moqui.cluster.v1.Cluster`,
//! in a build with `grpc` and unless `cluster_protocol=rest`) or `POST /cluster/{topic}` on HTTP/1.1. Every message is
//! applied through [`super::apply_topic`] on a blocking thread.
//!
//! Limits: [`MAX_CONNECTIONS`] at once (one more is closed, and its sender tries again); the TLS handshake in
//! [`super::CLIENT_IO`], before which the peer is not known; then, from a peer that holds the cluster certificate, a
//! request head within [`super::CONNECTION_IDLE`] of the last exchange on HTTP/1.1, and on HTTP/2 a connection with no
//! request for that long is closed (GOAWAY). HTTP/2 PINGs find a peer that went away.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use moqui_core::context::ExecutionContextFactory;
use tokio::sync::watch;

use super::{HostError, Protocol, BODY_LIMIT, CLIENT_IO, CONNECTION_IDLE, HEADER_LIMIT};
use crate::http::Activity;

/// Connections served at once; one more is closed, and the peer's post tries again.
pub(super) const MAX_CONNECTIONS: usize = 64;
/// HTTP/2 streams open at once on one connection.
const MAX_STREAMS: u32 = 64;
/// HTTP/2 PINGs this often find a peer that went away; one not answered in [`PING_TIMEOUT`] closes the connection.
const PING_EVERY: Duration = Duration::from_secs(15);
const PING_TIMEOUT: Duration = Duration::from_secs(10);
/// How often an HTTP/2 connection checks whether it has been idle for [`CONNECTION_IDLE`].
const IDLE_CHECK: Duration = Duration::from_secs(5);
/// How long a stop waits for the open connections to finish.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// Applies one topic message from a peer and answers its HTTP status: [`super::apply_topic`] on the factory, or a
/// test's stand-in.
pub(super) type Apply = Arc<dyn Fn(&str, &[u8]) -> u16 + Send + Sync>;

/// Bind the cluster listener. Call after [`moqui_core::cluster::start`] so the member row exists.
///
/// # Errors
///
/// When the cluster material or a cluster property is not usable, or the port cannot be bound.
pub(crate) fn listen(
    factory: Arc<ExecutionContextFactory>,
    posts: Arc<Mutex<Vec<String>>>,
) -> Result<ClusterEdge, HostError> {
    let material = super::load_material()?;
    let config = super::server_config(&material)?;
    let port = super::listen_port(&moqui_lib::env::get_prop_or_env("cluster_https_port"))?;
    let bind = SocketAddr::from((super::bind_address()?, port));
    let apply_posts = Arc::clone(&posts);
    let apply: Apply = Arc::new(move |topic, body| super::apply_topic(&factory, &apply_posts, topic, body));
    listen_with(bind, config, super::protocol()?, apply, posts)
}

/// Bind `bind` and serve the cluster port with `config` (its ALPN set here for `protocol`), each message through
/// `apply`.
pub(super) fn listen_with(
    bind: SocketAddr,
    mut config: rustls::ServerConfig,
    protocol: Protocol,
    apply: Apply,
    posts: Arc<Mutex<Vec<String>>>,
) -> Result<ClusterEdge, HostError> {
    config.alpn_protocols = super::alpn_protocols(protocol);
    let listener =
        std::net::TcpListener::bind(bind).map_err(|err| HostError::new(format!("could not bind {bind}: {err}")))?;
    let address = listener
        .local_addr()
        .map_err(|err| HostError::new(format!("cluster listener has no address: {err}")))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| HostError::new(format!("cluster listener on {address}: {err}")))?;
    let (stop, stopped) = watch::channel(false);
    let app = Arc::new(App {
        #[cfg(feature = "grpc")]
        grpc: super::grpc::Server::new(Arc::clone(&apply)),
        apply,
    });
    let task = crate::edge::handle().spawn(accept_loop(listener, Arc::new(config), app, stopped));
    Ok(ClusterEdge {
        address,
        posts,
        stop,
        task: Some(task),
    })
}

/// The running listener. Dropping it stops accepting, lets the open connections finish for up to [`STOP_WAIT`], and
/// waits for that.
pub(crate) struct ClusterEdge {
    address: SocketAddr,
    posts: Arc<Mutex<Vec<String>>>,
    stop: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ClusterEdge {
    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(crate) fn posts(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.posts)
    }
}

impl Drop for ClusterEdge {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        let Some(task) = self.task.take() else {
            return;
        };
        let waited = crate::edge::block_on(async move { tokio::time::timeout(STOP_WAIT + STOP_WAIT, task).await });
        match waited {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(err))) if err.is_panic() => tracing::error!("the cluster listener panicked"),
            Ok(Ok(Err(_))) => {}
            Ok(Err(_)) => tracing::warn!("the cluster listener did not stop in time"),
            Err(err) => tracing::warn!("the cluster listener's stop could not be waited for: {err}"),
        }
    }
}

/// What each connection serves with.
struct App {
    apply: Apply,
    /// The gRPC `Cluster` service, for a connection that chose h2.
    #[cfg(feature = "grpc")]
    grpc: super::grpc::Server,
}

async fn accept_loop(
    listener: std::net::TcpListener,
    config: Arc<rustls::ServerConfig>,
    app: Arc<App>,
    mut stopped: watch::Receiver<bool>,
) {
    let listener = match tokio::net::TcpListener::from_std(listener) {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!("the cluster listener could not start: {err}");
            return;
        }
    };
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    let permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let accepted = tokio::select! {
            () = stop_asked(&mut stopped) => break,
            accepted = listener.accept() => accepted,
        };
        while connections.try_join_next().is_some() {}
        let stream = match accepted {
            Ok((stream, _)) => stream,
            Err(err) => {
                tracing::warn!(error = %err, "cluster accept failed");
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
        };
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            tracing::warn!("cluster listener is at {MAX_CONNECTIONS} connections; one was closed");
            continue;
        };
        let (acceptor, app, stopped) = (acceptor.clone(), Arc::clone(&app), stopped.clone());
        connections.spawn(async move {
            serve_connection(stream, acceptor, app, stopped).await;
            drop(permit);
        });
    }
    drop(listener);
    // the open connections were told to stop: HTTP/1.1 after its exchange, HTTP/2 with a GOAWAY
    let finished = tokio::time::timeout(STOP_WAIT, async { while connections.join_next().await.is_some() {} }).await;
    if finished.is_err() {
        tracing::debug!("cluster connections still open at stop were closed");
    }
}

async fn serve_connection(
    stream: tokio::net::TcpStream,
    acceptor: tokio_rustls::TlsAcceptor,
    app: Arc<App>,
    stopped: watch::Receiver<bool>,
) {
    let _ = stream.set_nodelay(true);
    let tls = match tokio::time::timeout(CLIENT_IO, acceptor.accept(stream)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(err)) => {
            tracing::debug!(error = %err, "cluster TLS handshake failed");
            return;
        }
        Err(_) => {
            tracing::debug!("a cluster connection did not finish its TLS handshake in time");
            return;
        }
    };
    let h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2".as_slice());
    let activity = Arc::new(Activity::new());
    let service = {
        let activity = Arc::clone(&activity);
        hyper::service::service_fn(move |request: http::Request<Incoming>| {
            let (app, activity) = (Arc::clone(&app), Arc::clone(&activity));
            activity.started();
            async move {
                let answer = answer(&app, request).await;
                activity.ended();
                Ok::<_, Infallible>(answer)
            }
        })
    };
    let io = TokioIo::new(tls);
    if h2 {
        let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
        builder
            .timer(TokioTimer::new())
            .keep_alive_interval(PING_EVERY)
            .keep_alive_timeout(PING_TIMEOUT)
            .max_concurrent_streams(MAX_STREAMS)
            .max_header_list_size(u32::try_from(HEADER_LIMIT).unwrap_or(u32::MAX));
        let connection = builder.serve_connection(io, service);
        run_h2(connection, &activity, stopped).await;
    } else {
        let mut builder = hyper::server::conn::http1::Builder::new();
        builder
            .timer(TokioTimer::new())
            // also how long a kept-alive connection may wait for its next request
            .header_read_timeout(CONNECTION_IDLE)
            .max_buf_size(HEADER_LIMIT);
        let connection = builder.serve_connection(io, service);
        run_http1(connection, stopped).await;
    }
}

async fn run_http1<S>(
    connection: hyper::server::conn::http1::Connection<
        TokioIo<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>,
        S,
    >,
    mut stopped: watch::Receiver<bool>,
) where
    S: hyper::service::HttpService<Incoming, ResBody = axum::body::Body>,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let mut connection = std::pin::pin!(connection);
    tokio::select! {
        result = connection.as_mut() => {
            if let Err(err) = result {
                tracing::debug!("cluster connection ended: {err}");
            }
        }
        () = stop_asked(&mut stopped) => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}

async fn run_h2<S>(
    connection: hyper::server::conn::http2::Connection<
        TokioIo<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>,
        S,
        TokioExecutor,
    >,
    activity: &Activity,
    mut stopped: watch::Receiver<bool>,
) where
    S: hyper::service::HttpService<Incoming, ResBody = axum::body::Body>,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Future: Send + 'static,
{
    let mut connection = std::pin::pin!(connection);
    let mut check = tokio::time::interval(IDLE_CHECK);
    let mut closing = false;
    loop {
        tokio::select! {
            result = connection.as_mut() => {
                if let Err(err) = result {
                    tracing::debug!("cluster connection ended: {err}");
                }
                return;
            }
            () = stop_asked(&mut stopped), if !closing => {
                closing = true;
                connection.as_mut().graceful_shutdown();
            }
            _ = check.tick(), if !closing => {
                if activity.idle_for() >= CONNECTION_IDLE {
                    closing = true;
                    connection.as_mut().graceful_shutdown();
                }
            }
        }
    }
}

/// Resolves once the listener is told to stop (or is gone).
async fn stop_asked(stopped: &mut watch::Receiver<bool>) {
    let _ = stopped.wait_for(|stop| *stop).await;
}

/// One request: a gRPC call of the `Cluster` service, or `POST /cluster/{topic}`.
async fn answer(app: &App, request: http::Request<Incoming>) -> http::Response<axum::body::Body> {
    #[cfg(feature = "grpc")]
    if super::grpc::is_call(&request) {
        return app.grpc.call(request).await;
    }
    let Some(topic) = super::cluster_topic(request.method().as_str(), request.uri().path()).map(str::to_string) else {
        return status(404);
    };
    let declared = request
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > BODY_LIMIT) {
        return status(413);
    }
    let read = axum::body::to_bytes(axum::body::Body::new(request.into_body()), BODY_LIMIT.saturating_add(1));
    let body = match tokio::time::timeout(CLIENT_IO, read).await {
        Ok(Ok(body)) if body.len() <= BODY_LIMIT => body,
        Ok(_) => return status(413),
        Err(_) => return status(400),
    };
    let apply = Arc::clone(&app.apply);
    let code = tokio::task::spawn_blocking(move || apply(&topic, &body))
        .await
        .unwrap_or(500);
    // Applied is 200 with an empty body, not 204: a 204 carries no Content-Length, and a member built before gRPC
    // reads an answer by its Content-Length.
    status(if code == 204 { 200 } else { code })
}

fn status(code: u16) -> http::Response<axum::body::Body> {
    let mut response = http::Response::new(axum::body::Body::empty());
    *response.status_mut() = http::StatusCode::from_u16(code).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    response
}
