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

//! git's smart HTTP protocol at `/git/<path>.git/...` (the repository's path of one or more segments, or its id; the
//! `.git` may be left out): `GET info/refs?service=git-upload-pack` (or `git-receive-pack`) and `POST git-upload-pack`
//! / `git-receive-pack`. On a public-site host the same requests at a page's URL (`/<path>/info/refs`) are git too
//! ([`page_request`]). The dumb protocol is not served. The request
//! body streams into, and the response streams out of, a job of the git pool that runs
//! `moqui_git::server::serve_http`, so a clone or push is not held in memory, takes no request worker
//! from the screens, and is exempt from `webapp_request_timeout` (a gap of `git-server` `idle-timeout`, 120 seconds, in the
//! request body ends it). Bodies are limited by `git-server` `max-request` (64 MB, upload-pack) and `git-server` `max-push-size` (2 GB).
//! Credentials are taken only over HTTPS (the listener, or `X-Forwarded-Proto` behind the proxy that
//! `webapp_client_ip_header` names), from loopback, or with `git-server` `allow-http="true"`.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::response::Response;
use hyper::body::Body as _;
use moqui_git::server::{self, GitHttpRequest, GitService, HttpResponder};
use tokio::sync::{mpsc, oneshot};

use crate::channel_body::ChannelBody;
use crate::git_bridge::{ChannelReader, ChannelWriter, Chunk, QUEUED};
use crate::guard::Event;
use crate::http::{plain, secure_request, ClientAddr, HttpState, TlsConnection};

const DEFAULT_MAX_REQUEST: u64 = 64 * 1024 * 1024;
const DEFAULT_MAX_PUSH: u64 = 2 * 1024 * 1024 * 1024;

/// An attribute of the conf's `git-server` as a positive number, else `default`.
fn size_setting(state: &HttpState, name: &str, default: u64) -> u64 {
    Some(state.factory.conf_number("git-server", name, default))
        .filter(|size| *size > 0)
        .unwrap_or(default)
}

/// The conf's `git-server` `idle-timeout`: for the request body, and for the response a client stops reading.
fn idle_timeout(state: &HttpState) -> Duration {
    crate::git_bridge::idle_timeout(&state.factory)
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
        let mut headers: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        // a CI token the work found good, for web protection here and on an edge node in front (crate::guard)
        if moqui_core::web::take_token_verified() {
            headers.push((crate::guard::CI_TOKEN_HEADER.to_string(), "verified".to_string()));
        }
        head.send((status, headers))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the git client went away"))
    }

    fn body(&mut self) -> &mut dyn std::io::Write {
        &mut self.body
    }
}

/// An answer written in memory: a [`server::EarlyRefusal`]'s, given before the request takes a git worker.
#[derive(Default)]
struct Buffered {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponder for Buffered {
    fn head(&mut self, status: u16, headers: &[(&str, String)]) -> std::io::Result<()> {
        self.status = status;
        self.headers = headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        Ok(())
    }

    fn body(&mut self) -> &mut dyn std::io::Write {
        &mut self.body
    }
}

/// The answer to a git or LFS request the git pool need not see (SEC-GIT-18): its credentials cannot be taken over
/// its transport or are in no form this server takes, so it takes no worker; a refused credential counts as a failed
/// sign-in, as the pool's answer counts it. None when the request goes to the pool.
fn early_answer(
    state: &HttpState,
    client: std::net::IpAddr,
    authorization: Option<&str>,
    api_key: Option<&str>,
    (secure, lfs, token_shaped): (bool, bool, bool),
) -> Option<Response> {
    let refusal = server::early_refusal(authorization, api_key, secure, lfs)?;
    if refusal.auth_failure() {
        state.guard.record(client, failure_event(token_shaped));
    }
    let mut answer = Buffered::default();
    refusal.answer(&mut answer);
    let mut response = Response::builder().status(answer.status);
    for (name, value) in answer.headers {
        response = response.header(name, value);
    }
    Some(
        response
            .body(axum::body::Body::from(answer.body))
            .unwrap_or_else(|_| plain(500, "a git response header was not valid")),
    )
}

/// The web protection event of a refused credential: a well-formed CI token counts toward the `ci-token-failure` ban,
/// whose threshold allows for runners that share an address, anything else toward `auth-failure`'s (crate::guard).
fn failure_event(ci_token: bool) -> Event {
    if ci_token {
        Event::CiTokenFailure
    } else {
        Event::AuthFailure
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

/// The `/git/...` path a request at a page's URL on a public-site host is served at, when it is git's (plans/Dogfood.md
/// D4: the page URL itself clones): `GET <path>/info/refs?service=git-upload-pack` (or `git-receive-pack`), `POST
/// <path>/git-upload-pack` or `git-receive-pack`, and Git LFS under `<path>.git/info/lfs/`, each with or without
/// `.git` after the path, where `<path>` is a repository path whose first segment is not reserved by the framework
/// (`moqui_core::web::reserved`, `git` among them). The shape is checked first, with suffix tests only, so a page request costs no more; `is_site_host` is asked
/// last. The repository itself is looked up by the git server: one that is not there is answered as under `/git/`.
pub(crate) fn page_request(
    method: &str,
    path: &str,
    query: Option<&str>,
    host: &str,
    is_site_host: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let service_query = || {
        query
            .unwrap_or("")
            .split('&')
            .filter_map(|pair| pair.strip_prefix("service="))
            .any(|name| GitService::from_name(name).is_some())
    };
    let shaped = match method {
        "GET" if path.ends_with("/info/refs") => service_query(),
        "POST" if path.ends_with("/git-upload-pack") || path.ends_with("/git-receive-pack") => true,
        _ => path.contains(".git/info/lfs/"),
    };
    if !shaped {
        return None;
    }
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    // a repository's first segment is a name that owns everything under it, so a partly reserved one is refused too
    if moqui_core::web::reserved::is_reserved_name(first) {
        return None;
    }
    let (repository, _) = server::path::split_request(path)?;
    server::repository_of(repository)?;
    is_site_host(host).then(|| format!("/git{path}"))
}

/// The repository's path for the git server. A path that is not one is passed on as it came: the server finds no
/// repository for it and answers as for any repository that is not there (401 with no credentials, else 404), so an
/// answer here would tell the two apart.
fn repository_name(repository: &str) -> String {
    server::repository_of(repository).unwrap_or_else(|| repository.to_string())
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
    let (repository, tail) = server::path::split_request(path).unwrap_or((path, ""));
    let query = parts.uri.query().unwrap_or("");
    if let Some(lfs_path) = tail.strip_prefix("info/lfs/") {
        let repository = repository_name(repository);
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
    // whatever the repository: a request that is not the smart protocol's says nothing of one
    let Some(service) = service else {
        return plain(404, "not a git smart HTTP request (the dumb protocol is not served)");
    };
    let repository = repository_name(repository);
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
    let secure = secure(&state, over_tls, &parts, peer);
    let token_shaped = crate::guard::ci_credential(parts.uri.path(), &parts.headers).is_some();
    let (authorization, api_key) = (header("authorization"), header("api_key").or_else(|| header("login_key")));
    let early = (secure, false, token_shaped);
    if let Some(answer) = early_answer(&state, client, authorization.as_deref(), api_key.as_deref(), early) {
        return answer;
    }
    let git_request = GitHttpRequest {
        repository,
        service,
        advertise,
        git_protocol: header("git-protocol"),
        gzip: header("content-encoding").is_some_and(|encoding| encoding.eq_ignore_ascii_case("gzip")),
        authorization,
        api_key,
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
            body: ChannelWriter::new(out_tx, idle),
        };
        moqui_core::web::take_token_verified();
        server::serve_http(&git_request, &mut reader, &mut responder);
        let _ = std::io::Write::flush(&mut responder.body);
        moqui_core::web::take_token_verified();
        if moqui_core::web::take_auth_failure() {
            guard.record(client, failure_event(token_shaped));
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
        .body(axum::body::Body::new(ChannelBody::new(out_rx)))
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

/// Whether credentials may be taken: over TLS (the listener's, or a believed proxy's `X-Forwarded-Proto`), from a
/// client on this machine (behind a believed proxy, the client it names; never a client an edge node names, SEC-CLU-01:
/// [`secure_request`]), or with the conf's `git-server` `allow-http="true"`.
fn secure(state: &HttpState, over_tls: bool, parts: &axum::http::request::Parts, peer: SocketAddr) -> bool {
    secure_request(over_tls, parts, peer.ip(), state.guard.conf().addressing())
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
    use moqui_git::server::lfs;

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
    let secure = secure(&state, over_tls, &parts, peer);
    let token_shaped = crate::guard::ci_credential(parts.uri.path(), &parts.headers).is_some();
    let (authorization, api_key) = (header("authorization"), header("api_key").or_else(|| header("login_key")));
    let early = (secure, true, token_shaped);
    if let Some(answer) = early_answer(&state, client, authorization.as_deref(), api_key.as_deref(), early) {
        return answer;
    }
    let request = lfs::LfsHttpRequest {
        repository,
        method: parts.method.as_str().to_string(),
        path,
        query,
        authorization,
        api_key,
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
            body: ChannelWriter::new(out_tx, idle),
        };
        moqui_core::web::take_token_verified();
        lfs::serve_lfs(&request, &mut reader, &mut responder);
        let _ = std::io::Write::flush(&mut responder.body);
        moqui_core::web::take_token_verified();
        if moqui_core::web::take_auth_failure() {
            guard.record(client, failure_event(token_shaped));
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
        .body(axum::body::Body::new(ChannelBody::new(out_rx)))
        .unwrap_or_else(|_| plain(500, "an LFS response header was not valid"))
}

/// `/ci/runner/{*rest}`: the runner REST of the CI engine (docs/interfaces/Pipelines.md "The runner REST"), on the git pool
/// as a clone is, an upload's body streamed in and a cache's out. It takes no session: each request carries a job,
/// runner or pool token, taken only where credentials are ([`secure`]). The body is limited by what it is: the
/// conf's `git-ci` `max-artifact-bytes` or `max-cache-bytes` for an upload, `max-log-batch-bytes` for the rest (the
/// engine checks each kind's own limit as it stores it).
pub(crate) async fn handle_runner(
    State(state): State<Arc<HttpState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    use moqui_git::ci::runner_http;

    let over_tls = request.extensions().get::<TlsConnection>().is_some();
    let client = request
        .extensions()
        .get::<ClientAddr>()
        .map_or(peer.ip(), |client| client.0);
    let (parts, body) = request.into_parts();
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let ci_number = |name: &str, default: u64| {
        Some(state.factory.conf_number("git-ci", name, default))
            .filter(|size| *size > 0)
            .unwrap_or(default)
    };
    let upload = parts.method == axum::http::Method::PUT;
    let limit = if upload {
        ci_number("max-artifact-bytes", 1024 * 1024 * 1024).max(ci_number("max-cache-bytes", 2 * 1024 * 1024 * 1024))
    } else {
        ci_number("max-log-batch-bytes", 1024 * 1024)
    };
    let content_length = header("content-length").and_then(|length| length.parse::<u64>().ok());
    if content_length.is_some_and(|length| length > limit) {
        return plain(413, &format!("the runner request is over {limit} bytes"));
    }
    let token_shaped = crate::guard::ci_credential(parts.uri.path(), &parts.headers).is_some();
    let runner_request = runner_http::RunnerHttpRequest {
        method: parts.method.as_str().to_string(),
        path: parts.uri.path().strip_prefix("/ci/runner/").unwrap_or("").to_string(),
        query: parts.uri.query().unwrap_or("").to_string(),
        authorization: header("authorization"),
        content_length,
        secure: secure(&state, over_tls, &parts, peer),
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
            body: ChannelWriter::new(out_tx, idle),
        };
        moqui_core::web::take_token_verified();
        runner_http::serve(&runner_request, &mut reader, &mut responder);
        let _ = std::io::Write::flush(&mut responder.body);
        moqui_core::web::take_token_verified();
        if moqui_core::web::take_auth_failure() {
            guard.record(client, failure_event(token_shaped));
        }
        Ok(())
    });
    if submitted.is_err() {
        return state.guard.unavailable("the git server is busy; try again");
    }
    tokio::spawn(feed(body, body_tx, limit, idle));
    let Ok((status, headers)) = head_rx.await else {
        return plain(500, "the runner request ended without a response");
    };
    let mut response = Response::builder().status(status);
    for (name, value) in headers {
        response = response.header(name, value);
    }
    response
        .body(axum::body::Body::new(ChannelBody::new(out_rx)))
        .unwrap_or_else(|_| plain(500, "a runner response header was not valid"))
}

#[cfg(test)]
#[path = "git_http_tests.rs"]
mod tests;
