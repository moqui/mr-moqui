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

//! The chat bridges' web routes (plans/Comms-Framework.md §4.3 "Webhooks", C4-SMS; docs/interfaces/SMS.md), off unless
//! `channels_webhooks_enabled` is true:
//!
//! - `POST /channels/{chatBridgeId}/webhook`: a network's webhook. The raw body is read first, within
//!   `channels_webhook_max_bytes` (a larger declared or sent body is 413 before anything else), then handed with the
//!   headers to the bridge's adapter on a request worker in a fresh context
//!   ([`moqui_core::service::chat_bridge::webhook`]), which checks the network's signature over those bytes and a replay
//!   window before it parses them. A refused signature counts for web protection as a refused login does. A bridge id
//!   that is no bridge's (or not even an id), one without its adapter in this build, and a disabled bridge to anyone but
//!   its network all get the answer of an unsigned request, so bridge ids cannot be probed.
//! - `GET /channels/{chatBridgeId}/media/{token}`: a file of a message that went to that bridge, for the network to
//!   fetch (an MMS picture); the token is signed and runs out ([`moqui_core::service::chat_bridge::media`]).
//!
//! No session, cookie or CSRF token is read: a network is no user. The adapters are the `channels` feature's
//! (crates/moqui-channels); without it every bridge answers as an unsigned request.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Request, State};
use axum::response::Response;
use axum::Router;
use moqui_core::service::chat_bridge::{WebhookRequest, WebhookResponse};

use crate::guard::Event;
use crate::http::{plain, read_body, ClientAddr, HttpState};

/// The switch of both routes.
const ENABLED_PROPERTY: &str = "channels_webhooks_enabled";
/// The largest webhook body read.
const MAX_BYTES_PROPERTY: &str = "channels_webhook_max_bytes";
const DEFAULT_MAX_BYTES: usize = 256 * 1024;

fn enabled() -> bool {
    moqui_lib::env::get_prop_or_env(ENABLED_PROPERTY).trim() == "true"
}

/// Add the routes when `channels_webhooks_enabled` is true.
pub(crate) fn routes(
    app: Router<Arc<HttpState>>,
    factory: &moqui_core::context::ExecutionContextFactory,
) -> Router<Arc<HttpState>> {
    use moqui_core::startup::{integration, State};
    if !enabled() {
        integration(
            factory,
            "Chat bridge webhooks",
            State::off(format!("{ENABLED_PROPERTY} is not true")),
        );
        return app;
    }
    integration(
        factory,
        "Chat bridge webhooks",
        State::on("/channels/{chatBridgeId}/webhook and /media/"),
    );
    app.route("/channels/{bridge}/webhook", axum::routing::post(webhook))
        .route("/channels/{bridge}/media/{token}", axum::routing::get(media))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn response_of(answer: WebhookResponse) -> Response {
    Response::builder()
        .status(answer.status)
        .header("content-type", answer.content_type)
        .header("cache-control", "no-store")
        .body(axum::body::Body::from(answer.body))
        .unwrap_or_else(|_| plain(500, "the response was not valid"))
}

async fn webhook(
    State(state): State<Arc<HttpState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(bridge): Path<String>,
    request: Request,
) -> Response {
    let client = request
        .extensions()
        .get::<ClientAddr>()
        .map_or(peer.ip(), |client| client.0);
    if !valid_id(&bridge) {
        state.guard.record(client, Event::AuthFailure);
        return response_of(WebhookResponse::not_signed());
    }
    let (parts, body) = request.into_parts();
    let limit = moqui_lib::env::get_prop_or_env(MAX_BYTES_PROPERTY)
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|limit| *limit > 0)
        .unwrap_or(DEFAULT_MAX_BYTES);
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
    let request = WebhookRequest {
        method: parts.method.as_str().to_string(),
        headers,
        body: body.to_vec(),
        client_ip: client.to_string(),
        now_millis: jiff::Timestamp::now().as_millisecond(),
    };
    let factory = Arc::clone(&state.factory);
    let job_factory = Arc::clone(&factory);
    let (done, answer) = tokio::sync::oneshot::channel::<WebhookResponse>();
    let submitted = factory.request_pool().try_submit(Arc::clone(&factory), move || {
        let _ = done.send(moqui_core::service::chat_bridge::webhook(
            &job_factory,
            &bridge,
            &request,
        ));
        Ok(())
    });
    if submitted.is_err() {
        return state.guard.unavailable("the server is busy; try again");
    }
    let Ok(answer) = answer.await else {
        return plain(500, "the webhook ended without a response");
    };
    if answer.auth_failure {
        state.guard.record(client, Event::AuthFailure);
    }
    response_of(answer)
}

async fn media(State(state): State<Arc<HttpState>>, Path((bridge, token)): Path<(String, String)>) -> Response {
    if !valid_id(&bridge) || token.len() > 200 {
        return plain(404, "not found");
    }
    let factory = Arc::clone(&state.factory);
    let job_factory = Arc::clone(&factory);
    let (done, answer) = tokio::sync::oneshot::channel();
    let submitted = factory.request_pool().try_submit(Arc::clone(&factory), move || {
        let now = jiff::Timestamp::now().as_millisecond();
        let _ = done.send(moqui_core::service::chat_bridge::media(
            &job_factory,
            &bridge,
            &token,
            now,
        ));
        Ok(())
    });
    if submitted.is_err() {
        return state.guard.unavailable("the server is busy; try again");
    }
    match answer.await {
        Ok(Some(file)) => {
            let name: String = file
                .file_name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
                .collect();
            Response::builder()
                .status(200)
                .header("content-type", file.content_type)
                .header("content-disposition", format!("attachment; filename=\"{name}\""))
                .header("x-content-type-options", "nosniff")
                .header("cache-control", "private, no-store")
                .body(axum::body::Body::from(file.bytes))
                .unwrap_or_else(|_| plain(500, "the response was not valid"))
        }
        _ => plain(404, "not found"),
    }
}
