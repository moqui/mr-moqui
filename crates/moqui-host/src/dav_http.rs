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
//! (`moqui_dav::dav`). What can be answered from the head is answered before the body is read: a path outside the
//! server, `OPTIONS`, the server off (`dav-server` `enabled`), no credentials or credentials over plain HTTP, and for a
//! `PUT` credentials that do not hold (SEC-FIL-04). A `PUT`'s body, the file, is spooled as an upload's file part is, up
//! to `dav-server` `max-file` bytes (256 MiB); another method's body (XML) is read whole, up to 1 MiB, within
//! `webapp-list/webapp[webroot]/web-protection/requests@body-timeout`. The request is served on a request worker in a fresh context. Credentials are taken over
//! HTTPS, from a trusted proxy's HTTPS, from loopback, or with `dav-server` `allow-http="true"`; a refused login counts
//! for web protection as a refused screen login does.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::response::Response;
use moqui_dav::dav::{DavRequest, DavResponse, MAX_XML_BODY};

use crate::guard::Event;
use crate::http::{forwarded_https, plain, read_body, read_to_spool, ClientAddr, HttpState, TlsConnection};

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
    // a loopback client an edge node names is not this machine (SEC-CLU-01)
    let via = parts
        .extensions
        .get::<moqui_core::web::RequestVia>()
        .copied()
        .unwrap_or_default();
    let secure = over_tls
        || forwarded_https(&parts.headers, peer.ip(), state.guard.conf().addressing())
        || crate::local_peer(peer.ip(), via);
    let mut dav_request = DavRequest {
        method: parts.method.as_str().to_string(),
        path: parts.uri.path().to_string(),
        headers,
        body: Vec::new(),
        upload: None,
        secure,
        client_ip: client.to_string(),
    };
    if let Some(answer) = moqui_dav::dav::answer_before_body(&state.factory, &dav_request) {
        return respond(&state, answer);
    }
    let put = dav_request.method.eq_ignore_ascii_case("PUT");
    let max_file = Some(state.factory.conf_number("dav-server", "max-file", DEFAULT_MAX_FILE))
        .filter(|limit| *limit > 0)
        .unwrap_or(DEFAULT_MAX_FILE);
    let limit = if put { max_file } else { MAX_XML_BODY.min(max_file) };
    let declared = parts
        .headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > limit) {
        return plain(413, &format!("the request is over {limit} bytes"));
    }
    if put {
        // the file is written to disk as it comes, so its sender must be a user first
        let checked = dav_request.clone();
        let job_factory = Arc::clone(&state.factory);
        let guard = Arc::clone(&state.guard);
        let held = crate::http::admission::run(
            Arc::clone(&state.factory),
            Arc::clone(&state.guard),
            state.guard.conf().queue_timeout,
            move || {
                let held = moqui_dav::dav::credentials_hold(&job_factory, &checked);
                if moqui_core::web::take_auth_failure() {
                    guard.record(client, Event::AuthFailure);
                }
                held
            },
        )
        .await;
        match held {
            Ok(Ok(())) => {}
            Ok(Err(answer)) => return respond(&state, answer),
            Err(refusal) => return refusal.response(&state.guard),
        }
        let content_type = dav_request
            .header("content-type")
            .unwrap_or("application/octet-stream")
            .to_string();
        match read_to_spool(&state, body, limit, "put", &content_type, client).await {
            Ok(file) => dav_request.upload = Some(file),
            Err(refused) => return refused.response(),
        }
    } else {
        match read_body(&state, body, limit).await {
            Ok(body) => dav_request.body = body.to_vec(),
            Err(refused) => return refused.response(),
        }
    }
    // the shared admission (queue age, abandonment, overload; HL-006)
    let job_factory = Arc::clone(&state.factory);
    let guard = Arc::clone(&state.guard);
    let served = crate::http::admission::run(
        Arc::clone(&state.factory),
        Arc::clone(&state.guard),
        state.guard.conf().queue_timeout,
        move || {
            let response: DavResponse = moqui_dav::dav::serve(&job_factory, &dav_request);
            if moqui_core::web::take_auth_failure() {
                guard.record(client, Event::AuthFailure);
            }
            response
        },
    )
    .await;
    match served {
        Ok(answer) => respond(&state, answer),
        Err(refusal) => refusal.response(&state.guard),
    }
}

fn respond(state: &HttpState, answer: DavResponse) -> Response {
    let mut response = Response::builder().status(answer.status);
    for (name, value) in answer.headers {
        response = response.header(name, value);
    }
    // a file's content streams a window at a time, as a web download's does
    let body = match answer.stream {
        Some(stream) => crate::source_body::body(Arc::clone(&state.factory), stream),
        None => axum::body::Body::from(answer.body),
    };
    response
        .body(body)
        .unwrap_or_else(|_| plain(500, "a DAV response header was not valid"))
}
