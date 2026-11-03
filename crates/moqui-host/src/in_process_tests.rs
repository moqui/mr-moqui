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

use std::time::Duration;

use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

use super::*;
use crate::guard::Guard;
use crate::http::ListenLimits;

/// A small app on a runtime of its own: a page that echoes the request's host and cookie, a socket that echoes, and
/// a socket that is refused without a cookie.
fn app() -> (tokio::runtime::Runtime, InProcess, tokio::sync::watch::Sender<bool>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let router = Router::new()
        .route(
            "/page",
            get(|headers: axum::http::HeaderMap| async move {
                let text = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string()
                };
                ([("x-seen", format!("{}|{}", text("host"), text("cookie")))], "the page")
            }),
        )
        .route(
            "/echo",
            get(|upgrade: WebSocketUpgrade, headers: axum::http::HeaderMap| async move {
                if headers.get("cookie").is_none() {
                    return axum::http::StatusCode::UNAUTHORIZED.into_response();
                }
                upgrade.on_upgrade(|mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        let echo = match message {
                            Message::Text(text) => Message::Text(format!("echo {}", text.as_str()).into()),
                            Message::Binary(bytes) => Message::Binary(bytes),
                            _ => return,
                        };
                        if socket.send(echo).await.is_err() {
                            return;
                        }
                    }
                })
            }),
        );
    let (closing_tx, closing) = tokio::sync::watch::channel(false);
    let web = WebApp::new(router, ListenLimits::defaults(), Arc::new(Guard::disabled()));
    let local = InProcess::new(web, runtime.handle().clone(), closing);
    (runtime, local, closing_tx)
}

fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn a_request_in_process_reaches_the_router_with_the_host_s_headers() {
    let (_runtime, local, _closing) = app();
    let answer = local
        .request(
            "GET",
            "/page?x=1",
            &headers(&[("Host", "moqui-node.localhost"), ("Cookie", "s=1")]),
            Vec::new(),
        )
        .unwrap();
    assert_eq!(answer.status, 200);
    assert_eq!(answer.header("X-Seen"), Some("moqui-node.localhost|s=1"));
    assert_eq!(answer.bytes().unwrap(), b"the page");

    let mut streamed = Vec::new();
    let answer = local
        .request("GET", "/page", &headers(&[("Host", "h")]), Vec::new())
        .unwrap();
    assert_eq!(answer.copy_to(&mut streamed).unwrap(), 8);
    assert_eq!(streamed, b"the page");
    assert_eq!(local.request("GET", "/none", &[], Vec::new()).unwrap().status, 404);

    // once the listeners stop, nothing is served
    local.stop();
    assert!(local.request("GET", "/page", &[], Vec::new()).is_err());
}

#[test]
fn a_socket_in_process_opens_echoes_and_is_refused_without_a_session() {
    let (_runtime, local, _closing) = app();
    let (events, heard) = std::sync::mpsc::channel();
    let sink_events = Mutex::new(events);
    let socket = local
        .open_socket(
            "/echo",
            &headers(&[("Host", "moqui-node.localhost"), ("Cookie", "s=1")]),
            Box::new(move |event| {
                let _ = moqui_lib::sync::lock(&sink_events).send(event);
            }),
        )
        .unwrap();
    let next = || heard.recv_timeout(Duration::from_secs(5)).expect("an event");
    assert_eq!(next(), SocketEvent::Open);
    socket.send("hello".to_string());
    assert_eq!(next(), SocketEvent::Text("echo hello".to_string()));
    socket.send_binary(vec![0, 159, 255]);
    assert_eq!(next(), SocketEvent::Binary(vec![0, 159, 255]));
    drop(socket);

    let (events, heard) = std::sync::mpsc::channel();
    let sink_events = Mutex::new(events);
    let _refused = local
        .open_socket(
            "/echo",
            &headers(&[("Host", "moqui-node.localhost")]),
            Box::new(move |event| {
                let _ = moqui_lib::sync::lock(&sink_events).send(event);
            }),
        )
        .unwrap();
    assert_eq!(
        heard.recv_timeout(Duration::from_secs(5)).unwrap(),
        SocketEvent::Unauthorized
    );
}
