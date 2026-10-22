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

//! The DAV server's routes: `/dav` and below, and `/.well-known/caldav` and `/.well-known/carddav`
//! (`moqui_core::dav`). The body is read whole, up to `dav-server` `max-file` bytes (256 MiB), within
//! `webapp_body_timeout`, and the request is served on a request worker in a fresh context. Credentials are taken over
//! HTTPS, from a trusted proxy's HTTPS, from loopback, or with `dav-server` `allow-http="true"`; a refused login counts
//! for web protection as a refused screen login does.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::response::Response;
use moqui_core::dav::{DavRequest, DavResponse};

use crate::guard::Event;
use crate::http::{forwarded_https, plain, read_body, ClientAddr, HttpState, TlsConnection};

const DEFAULT_MAX_FILE: usize = 256 * 1024 * 1024;

/// `/dav`, `/dav/{*rest}`, `/.well-known/caldav` and `/.well-known/carddav`.
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
    let limit = Some(state.factory.conf_number("dav-server", "max-file", DEFAULT_MAX_FILE))
        .filter(|limit| *limit > 0)
        .unwrap_or(DEFAULT_MAX_FILE);
    let declared = parts
        .headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > limit) {
        return plain(413, &format!("the request is over {limit} bytes"));
    }
    let body = match read_body(&state, body, limit).await {
        Ok(body) => body,
        Err(refused) => return refused.response(),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for (name, value) in &parts.headers {
        let Ok(value) = value.to_str() else { continue };
        let name = name.as_str().to_ascii_lowercase();
        match headers.iter_mut().find(|(known, _)| *known == name) {
            Some((_, joined)) => {
                joined.push_str(", ");
                joined.push_str(value);
            }
            None => headers.push((name, value.to_string())),
        }
    }
    let secure = over_tls
        || forwarded_https(&parts.headers, peer.ip(), state.guard.conf.addressing())
        || peer.ip().is_loopback();
    let dav_request = DavRequest {
        method: parts.method.as_str().to_string(),
        path: parts.uri.path().to_string(),
        headers,
        body: body.to_vec(),
        secure,
        client_ip: client.to_string(),
    };
    let factory = Arc::clone(&state.factory);
    let job_factory = Arc::clone(&factory);
    let guard = Arc::clone(&state.guard);
    let (done, answer) = tokio::sync::oneshot::channel::<DavResponse>();
    let submitted = factory.request_pool().try_submit(Arc::clone(&factory), move || {
        let response = moqui_core::dav::serve(&job_factory, &dav_request);
        if moqui_core::web::take_auth_failure() {
            guard.record(client, Event::AuthFailure);
        }
        let _ = done.send(response);
        Ok(())
    });
    if submitted.is_err() {
        return state.guard.unavailable("the server is busy; try again");
    }
    let Ok(answer) = answer.await else {
        return plain(500, "the DAV request ended without a response");
    };
    let mut response = Response::builder().status(answer.status);
    for (name, value) in answer.headers {
        response = response.header(name, value);
    }
    response
        .body(axum::body::Body::from(answer.body))
        .unwrap_or_else(|_| plain(500, "a DAV response header was not valid"))
}
