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

//! git's smart HTTP protocol at `/git/<repositoryId>.git/...`: `GET info/refs?service=git-upload-pack` (or
//! `git-receive-pack`) and `POST git-upload-pack` / `git-receive-pack`. The dumb protocol is not served. The request
//! body streams into, and the response streams out of, a job of the git pool that runs
//! `moqui_core::resource::git::server::serve_http`, so a clone or push is not held in memory, takes no request worker
//! from the screens, and is exempt from `webapp_request_timeout` (a gap of `git-server` `idle-timeout`, 120 seconds, in the
//! request body ends it). Bodies are limited by `git-server` `max-request` (64 MB, upload-pack) and `git-server` `max-push-size` (2 GB).
//! Credentials are taken only over HTTPS (the listener, or `X-Forwarded-Proto` behind the proxy that
//! `webapp_client_ip_header` names), from loopback, or with `git-server` `allow-http="true"`.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::response::Response;
use bytes::Bytes;
use hyper::body::{Body as _, Frame};
use moqui_core::resource::git::server::{self, GitHttpRequest, GitService, HttpResponder};
use tokio::sync::{mpsc, oneshot};

use crate::git_bridge::{ChannelReader, ChannelWriter, Chunk, QUEUED};
use crate::guard::Event;
use crate::http::{forwarded_https, plain, ClientAddr, HttpState, TlsConnection};

const DEFAULT_MAX_REQUEST: u64 = 64 * 1024 * 1024;
const DEFAULT_MAX_PUSH: u64 = 2 * 1024 * 1024 * 1024;
const DEFAULT_IDLE: Duration = Duration::from_secs(120);

/// An attribute of the conf's `git-server` as a positive number, else `default`.
fn size_setting(state: &HttpState, name: &str, default: u64) -> u64 {
    Some(state.factory.conf_number("git-server", name, default))
        .filter(|size| *size > 0)
        .unwrap_or(default)
}

/// The conf's `git-server` `idle-timeout` in seconds, else [`DEFAULT_IDLE`].
fn idle_timeout(state: &HttpState) -> Duration {
    Some(state.factory.conf_number("git-server", "idle-timeout", 0u64))
        .filter(|seconds| *seconds > 0)
        .map_or(DEFAULT_IDLE, Duration::from_secs)
}

/// The response body: chunks from the job until it drops its writer.
struct ChannelBody(mpsc::Receiver<Chunk>);

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        self.0
            .poll_recv(cx)
            .map(|chunk| chunk.map(|chunk| chunk.map(Frame::data)))
    }
}

/// A response's status and headers.
type Head = (u16, Vec<(String, String)>);

/// The job's side of the response: the status and headers once, then the body.
struct Responder {
    head: Option<oneshot::Sender<Head>>,
    body: ChannelWriter,
}

impl HttpResponder for Responder {
    fn head(&mut self, status: u16, headers: &[(&str, String)]) -> std::io::Result<()> {
        let head = self
            .head
            .take()
            .ok_or_else(|| std::io::Error::other("the response head was already sent"))?;
        let headers = headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        head.send((status, headers))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the git client went away"))
    }

    fn body(&mut self) -> &mut dyn std::io::Write {
        &mut self.body
    }
}

/// Copy the request body into `tx`: past `limit` bytes, or after `idle` with nothing, the job reads an error.
async fn feed(mut body: axum::body::Body, tx: mpsc::Sender<Chunk>, limit: u64, idle: Duration) {
    let mut total: u64 = 0;
    loop {
        let frame = tokio::time::timeout(idle, std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))).await;
        let chunk = match frame {
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the git request body stalled",
            )),
            Ok(None) => return,
            Ok(Some(Err(err))) => Err(std::io::Error::other(format!("reading the git request: {err}"))),
            Ok(Some(Ok(frame))) => match frame.into_data() {
                Ok(data) => {
                    total += data.len() as u64;
                    if total > limit {
                        Err(std::io::Error::other(format!("the git request is over {limit} bytes")))
                    } else {
                        Ok(data)
                    }
                }
                // trailers
                Err(_) => continue,
            },
        };
        let failed = chunk.is_err();
        if tx.send(chunk).await.is_err() || failed {
            return;
        }
    }
}

/// `/git/{*rest}`.
pub(crate) async fn handle(
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
    let path = parts.uri.path().strip_prefix("/git/").unwrap_or("");
    let (repository, tail) = match path.find(".git/") {
        Some(at) => (&path[..at], &path[at + 5..]),
        None => path.split_once('/').unwrap_or((path, "")),
    };
    let query = parts.uri.query().unwrap_or("");
    if let Some(lfs_path) = tail.strip_prefix("info/lfs/") {
        let Some(repository) = server::repository_of(repository) else {
            return plain(404, "not a git repository");
        };
        let lfs_path = lfs_path.to_string();
        let query = query.to_string();
        let from = Origin { peer, client, over_tls };
        return handle_lfs(state, from, parts, body, repository, lfs_path, query).await;
    }
    let (service, advertise) = match (parts.method.as_str(), tail) {
        ("GET", "info/refs") => (
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix("service="))
                .and_then(GitService::from_name),
            true,
        ),
        ("POST", name) => (GitService::from_name(name), false),
        _ => (None, false),
    };
    let (Some(service), Some(repository)) = (service, server::repository_of(repository)) else {
        return plain(404, "not a git smart HTTP request (the dumb protocol is not served)");
    };
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let limit = match (advertise, service) {
        (true, _) => 0,
        (false, GitService::UploadPack) => size_setting(&state, "max-request", DEFAULT_MAX_REQUEST),
        (false, GitService::ReceivePack) => size_setting(&state, "max-push-size", DEFAULT_MAX_PUSH),
    };
    if header("content-length")
        .and_then(|length| length.parse::<u64>().ok())
        .is_some_and(|length| length > limit)
    {
        return plain(413, &format!("the git request is over {limit} bytes"));
    }
    let secure = secure(&state, over_tls, &parts.headers, peer);
    let git_request = GitHttpRequest {
        repository,
        service,
        advertise,
        git_protocol: header("git-protocol"),
        gzip: header("content-encoding").is_some_and(|encoding| encoding.eq_ignore_ascii_case("gzip")),
        authorization: header("authorization"),
        api_key: header("api_key").or_else(|| header("login_key")),
        secure,
        client_ip: client.to_string(),
    };
    let idle = idle_timeout(&state);
    let (body_tx, body_rx) = mpsc::channel(QUEUED);
    let (out_tx, out_rx) = mpsc::channel(QUEUED);
    let (head_tx, head_rx) = oneshot::channel();
    let factory = Arc::clone(&state.factory);
    let guard = Arc::clone(&state.guard);
    let submitted = server::pool(&factory).try_submit(Arc::clone(&factory), move || {
        let mut reader = ChannelReader::new(body_rx);
        let mut responder = Responder {
            head: Some(head_tx),
            body: ChannelWriter::new(out_tx),
        };
        server::serve_http(&git_request, &mut reader, &mut responder);
        let _ = std::io::Write::flush(&mut responder.body);
        if moqui_core::web::take_auth_failure() {
            guard.record(client, Event::AuthFailure);
        }
        Ok(())
    });
    if submitted.is_err() {
        return state.guard.unavailable("the git server is busy; try again");
    }
    tokio::spawn(feed(body, body_tx, limit, idle));
    let Ok((status, headers)) = head_rx.await else {
        return plain(500, "the git request ended without a response");
    };
    let mut response = Response::builder().status(status);
    for (name, value) in headers {
        response = response.header(name, value);
    }
    response
        .body(axum::body::Body::new(ChannelBody(out_rx)))
        .unwrap_or_else(|_| plain(500, "a git response header was not valid"))
}

/// `/git/<repositoryId>.git/info/lfs/...`: Git LFS, on the git pool as a clone is, an object's body streamed in and out.
/// Where a request came from: the socket peer, the client it is for, and whether it came over this listener's TLS.
#[derive(Clone, Copy)]
struct Origin {
    peer: SocketAddr,
    client: std::net::IpAddr,
    over_tls: bool,
}

/// Whether credentials may be taken: over TLS (the listener's, or a believed proxy's `X-Forwarded-Proto`), from
/// loopback, or with the conf's `git-server` `allow-http="true"`.
fn secure(state: &HttpState, over_tls: bool, headers: &axum::http::HeaderMap, peer: SocketAddr) -> bool {
    over_tls
        || forwarded_https(headers, peer.ip(), state.guard.conf.addressing())
        || peer.ip().is_loopback()
        || state.factory.conf_flag("git-server", "allow-http", false)
}

async fn handle_lfs(
    state: Arc<HttpState>,
    from: Origin,
    parts: axum::http::request::Parts,
    body: axum::body::Body,
    repository: String,
    path: String,
    query: String,
) -> Response {
    use moqui_core::resource::git::server::lfs;

    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let upload = parts.method == axum::http::Method::PUT;
    let limit = if upload { lfs::max_object_size() } else { 1024 * 1024 };
    let content_length = header("content-length").and_then(|length| length.parse::<u64>().ok());
    if content_length.is_some_and(|length| length > limit) {
        return plain(413, &format!("the LFS request is over {limit} bytes"));
    }
    let Origin { peer, client, over_tls } = from;
    let secure = secure(&state, over_tls, &parts.headers, peer);
    let request = lfs::LfsHttpRequest {
        repository,
        method: parts.method.as_str().to_string(),
        path,
        query,
        authorization: header("authorization"),
        api_key: header("api_key").or_else(|| header("login_key")),
        range: header("range"),
        content_length,
        secure,
        client_ip: client.to_string(),
    };
    let idle = idle_timeout(&state);
    let (body_tx, body_rx) = mpsc::channel(QUEUED);
    let (out_tx, out_rx) = mpsc::channel(QUEUED);
    let (head_tx, head_rx) = oneshot::channel();
    let factory = Arc::clone(&state.factory);
    let guard = Arc::clone(&state.guard);
    let submitted = server::pool(&factory).try_submit(Arc::clone(&factory), move || {
        let mut reader = ChannelReader::new(body_rx);
        let mut responder = Responder {
            head: Some(head_tx),
            body: ChannelWriter::new(out_tx),
        };
        lfs::serve_lfs(&request, &mut reader, &mut responder);
        let _ = std::io::Write::flush(&mut responder.body);
        if moqui_core::web::take_auth_failure() {
            guard.record(client, Event::AuthFailure);
        }
        Ok(())
    });
    if submitted.is_err() {
        return state.guard.unavailable("the git server is busy; try again");
    }
    tokio::spawn(feed(body, body_tx, limit, idle));
    let Ok((status, headers)) = head_rx.await else {
        return plain(500, "the LFS request ended without a response");
    };
    let mut response = Response::builder().status(status);
    for (name, value) in headers {
        response = response.header(name, value);
    }
    response
        .body(axum::body::Body::new(ChannelBody(out_rx)))
        .unwrap_or_else(|_| plain(500, "an LFS response header was not valid"))
}
