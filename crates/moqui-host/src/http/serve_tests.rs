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

//! HTTP/2 on the HTTPS listener: chosen by ALPN, a request in the form the app reads from HTTP/1.1, WebSockets by
//! extended CONNECT, a refusal and an idle connection ended with a GOAWAY, and gRPC calls to the listener's services.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::Request;
use axum::routing::{any, get};
use axum::Router;
use futures_util::{SinkExt as _, StreamExt as _};
use hyper::body::Incoming;
use hyper::client::conn::http2::SendRequest;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use super::{TlsConnection, WebApp};
use crate::guard::Guard;
use crate::http::ListenLimits;
use crate::tls;

/// The app the listener serves: `/` names the Host it read and whether the request came over TLS; `/ws` echoes
/// WebSocket text.
fn app() -> Router {
    Router::new()
        .route(
            "/",
            get(|request: Request| async move {
                let host = request
                    .headers()
                    .get(http::header::HOST)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("-")
                    .to_string();
                let tls = request.extensions().get::<TlsConnection>().is_some();
                format!("host={host} tls={tls} uri={}", request.uri())
            }),
        )
        .route(
            "/ws",
            any(|upgrade: WebSocketUpgrade| async move {
                upgrade.on_upgrade(|mut socket| async move {
                    while let Some(Ok(Message::Text(text))) = socket.recv().await {
                        if socket.send(Message::Text(format!("echo {text}").into())).await.is_err() {
                            break;
                        }
                    }
                })
            }),
        )
}

/// A running HTTPS listener and what a client needs to reach it.
struct Listener {
    address: std::net::SocketAddr,
    cert: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl Listener {
    async fn stop(mut self) {
        drop(self.stop.take());
        tokio::time::timeout(Duration::from_secs(5), &mut self.task)
            .await
            .expect("the listener did not stop")
            .unwrap();
        let _ = std::fs::remove_dir_all(self.cert.parent().and_then(|dir| dir.parent()).unwrap());
    }

    /// A TLS connection offering `alpn`, and the protocol the handshake chose.
    async fn connect(&self, alpn: &[&[u8]]) -> (tokio_rustls::client::TlsStream<TcpStream>, Option<Vec<u8>>) {
        let stream = TcpStream::connect(self.address).await.unwrap();
        let tls = tls::tests::client_offering(&self.cert, alpn)
            .connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(), stream)
            .await
            .expect("localhost handshake");
        let chosen = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
        (tls, chosen)
    }

    /// An HTTP/2 client on a new connection.
    async fn h2(&self) -> SendRequest<axum::body::Body> {
        let (tls, chosen) = self.connect(&[b"h2", b"http/1.1"]).await;
        assert_eq!(chosen.as_deref(), Some(b"h2".as_slice()));
        let (send, connection) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .handshake(TokioIo::new(tls))
            .await
            .unwrap();
        tokio::spawn(connection);
        send
    }
}

async fn listen(label: &str, web: impl FnOnce(Router, ListenLimits) -> WebApp, http2: bool) -> Listener {
    let dir = tls::tests::temp_dir(label).join("cert");
    tls::ensure_dev_cert(&dir).unwrap();
    let mut config = tls::server_config(tls::CertStore::new(dir.clone())).unwrap();
    config.alpn_protocols = tls::web_alpn(http2);
    let limits = ListenLimits {
        header: Some(Duration::from_secs(1)),
        connections: 8,
        shutdown: Some(Duration::from_secs(2)),
        ..ListenLimits::defaults()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let web = web(app(), limits);
    let task = tokio::spawn(super::super::serve(
        listener,
        Some(super::super::Tls {
            acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(config)),
            edge: None,
            factory: std::sync::Weak::new(),
        }),
        web,
        Arc::new(Semaphore::new(limits.connections)),
        async move {
            let _ = stopped.await;
        },
    ));
    Listener {
        address,
        cert: dir.join("localhost.crt"),
        stop: Some(stop),
        task,
    }
}

fn plain_app(app: Router, limits: ListenLimits) -> WebApp {
    WebApp::new(app, limits, Arc::new(Guard::disabled()))
}

async fn text(response: http::Response<Incoming>) -> String {
    let bytes = axum::body::to_bytes(axum::body::Body::new(response.into_body()), 1 << 20)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn get_request(uri: &str) -> http::Request<axum::body::Body> {
    http::Request::get(uri)
        .version(http::Version::HTTP_2)
        .body(axum::body::Body::empty())
        .unwrap()
}

#[tokio::test]
async fn h2_is_chosen_by_alpn_and_a_request_reads_as_from_http1() {
    let listener = listen("h2-alpn", plain_app, true).await;
    let mut send = listener.h2().await;
    send.ready().await.unwrap();
    let response = send.send_request(get_request("https://localhost/?a=1")).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.version(), http::Version::HTTP_2);
    assert_eq!(text(response).await, "host=localhost tls=true uri=/?a=1");
    // a client that offers only HTTP/1.1 still gets it
    let (_, chosen) = listener.connect(&[b"http/1.1"]).await;
    assert_eq!(chosen.as_deref(), Some(b"http/1.1".as_slice()));
    listener.stop().await;
}

#[tokio::test]
async fn with_http2_off_the_listener_offers_only_http1() {
    let listener = listen("h2-off", plain_app, false).await;
    let (_, chosen) = listener.connect(&[b"h2", b"http/1.1"]).await;
    assert_eq!(chosen.as_deref(), Some(b"http/1.1".as_slice()));
    listener.stop().await;
}

#[tokio::test]
async fn a_websocket_opens_by_extended_connect() {
    let listener = listen("h2-ws", plain_app, true).await;
    let mut send = listener.h2().await;
    send.ready().await.unwrap();
    let request = http::Request::connect("https://localhost/ws")
        .version(http::Version::HTTP_2)
        .header(http::header::SEC_WEBSOCKET_VERSION, "13")
        .extension(hyper::ext::Protocol::from_static("websocket"))
        .body(axum::body::Body::empty())
        .unwrap();
    // the server's SETTINGS (extended CONNECT allowed) may come just after the handshake
    let mut response = None;
    for _ in 0..20 {
        match send.send_request(request.clone_for_retry()).await {
            Ok(answer) => {
                response = Some(answer);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let response = response.expect("the extended CONNECT was sent");
    assert_eq!(response.status(), 200, "an HTTP/2 WebSocket is answered 200, not 101");
    let upgraded = hyper::upgrade::on(response).await.unwrap();
    let mut socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
        TokioIo::new(upgraded),
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text("hi".into()))
        .await
        .unwrap();
    let echoed = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("an echo")
        .expect("the socket is open")
        .unwrap();
    assert_eq!(echoed.into_text().unwrap().as_str(), "echo hi");
    drop(socket);
    listener.stop().await;
}

#[tokio::test]
async fn a_refusal_closes_the_connection_with_a_goaway() {
    let listener = listen(
        "h2-refuse",
        |app, limits| {
            let guard = crate::guard::tests::guard(
                r#"<web-protection enabled="true"><user-agent-rule action="deny" contains="sqlmap"/></web-protection>"#,
                "",
            );
            WebApp::new(app, limits, guard)
        },
        true,
    )
    .await;
    let mut send = listener.h2().await;
    send.ready().await.unwrap();
    // a client the rules deny is refused, and over HTTP/1.1 told `Connection: close`
    let mut refused = get_request("https://localhost/");
    refused
        .headers_mut()
        .insert(http::header::USER_AGENT, http::HeaderValue::from_static("sqlmap/1.7"));
    let response = send.send_request(refused).await.unwrap();
    assert_eq!(response.status(), 403);
    assert!(
        response.headers().get(http::header::CONNECTION).is_none(),
        "HTTP/2 has no Connection header"
    );
    // well before the 1 s idle timeout would close it
    let closed = tokio::time::timeout(Duration::from_millis(600), async {
        while !send.is_closed() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(closed.is_ok(), "the refused client's connection was not closed");
    listener.stop().await;
}

#[tokio::test]
async fn an_idle_connection_is_closed_after_the_header_timeout() {
    let listener = listen("h2-idle", plain_app, true).await;
    let mut send = listener.h2().await;
    send.ready().await.unwrap();
    let response = send.send_request(get_request("https://localhost/")).await.unwrap();
    assert_eq!(response.status(), 200);
    let started = std::time::Instant::now();
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while !send.is_closed() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(closed.is_ok(), "an idle connection was kept past the header timeout");
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "{:?}",
        started.elapsed()
    );
    listener.stop().await;
}

#[tokio::test]
async fn a_grpc_call_goes_to_the_listeners_services_and_the_rest_to_the_app() {
    let listener = listen(
        "h2-grpc",
        |app, limits| {
            let mut web = plain_app(app, limits);
            web.grpc = Some(tonic::service::Routes::default());
            web
        },
        true,
    )
    .await;
    let mut send = listener.h2().await;
    send.ready().await.unwrap();
    let call = http::Request::post("https://localhost/moqui.test.v1.Nothing/Call")
        .version(http::Version::HTTP_2)
        .header(http::header::CONTENT_TYPE, "application/grpc")
        .header("te", "trailers")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = send.send_request(call).await.unwrap();
    // no such service: tonic answers the call itself, as a gRPC status, where the app would have answered 404
    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get("grpc-status")
            .and_then(|value| value.to_str().ok()),
        Some("12")
    );
    send.ready().await.unwrap();
    let response = send.send_request(get_request("https://localhost/")).await.unwrap();
    assert_eq!(text(response).await, "host=localhost tls=true uri=/");
    listener.stop().await;
}

/// A gRPC call to a host whose root screen refuses the framework's routes (plans/Edge-Node2.md §4.2) does not reach the
/// listener's services: it goes to the app, which has no such route.
#[tokio::test]
async fn a_grpc_call_to_a_refusing_host_goes_to_the_app() {
    let listener = listen(
        "h2-grpc-refuse",
        |app, limits| {
            let mut web = plain_app(app, limits);
            web.grpc = Some(tonic::service::Routes::default());
            web.refusing_host = Some(Arc::new(|host: &str| host.starts_with("site.")));
            web
        },
        true,
    )
    .await;
    let mut send = listener.h2().await;
    for (host, served) in [("site.localhost", false), ("localhost", true)] {
        send.ready().await.unwrap();
        let call = http::Request::post(format!("https://{host}/moqui.test.v1.Nothing/Call"))
            .version(http::Version::HTTP_2)
            .header(http::header::CONTENT_TYPE, "application/grpc")
            .header("te", "trailers")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = send.send_request(call).await.unwrap();
        // the services answer an unknown one as a gRPC status; the app answers 404 with no gRPC status
        assert_eq!(response.headers().contains_key("grpc-status"), served, "{host}");
        assert_eq!(response.status() == 404, !served, "{host}");
    }
    listener.stop().await;
}

/// A request to send again: `http::Request` is not `Clone`, and its extensions are needed.
trait CloneForRetry {
    fn clone_for_retry(&self) -> Self;
}

impl CloneForRetry for http::Request<axum::body::Body> {
    fn clone_for_retry(&self) -> Self {
        let mut request = http::Request::new(axum::body::Body::empty());
        *request.method_mut() = self.method().clone();
        *request.uri_mut() = self.uri().clone();
        *request.version_mut() = self.version();
        *request.headers_mut() = self.headers().clone();
        if let Some(protocol) = self.extensions().get::<hyper::ext::Protocol>() {
            request.extensions_mut().insert(protocol.clone());
        }
        request
    }
}
