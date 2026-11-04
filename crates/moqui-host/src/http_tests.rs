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

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::routing::get;
use axum::Router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use super::{header_map, http_port, serve, status_allowed, ListenLimits, TlsConnection, WebApp};
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

#[cfg(feature = "telemetry")]
#[tokio::test]
async fn a_screen_request_has_a_root_span_with_identity_and_health_is_independent() {
    use moqui_core::{
        entity::document::Document,
        telemetry::{Exporter, Resource, Telemetry},
    };
    use moqui_lib::{error::Error, object::Value};
    use tower::ServiceExt as _;
    #[derive(Default)]
    struct Capture(std::sync::Mutex<Vec<Document>>);
    impl Exporter for Capture {
        fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
            let count = documents.len();
            if index == "otel_spans" {
                self.0.lock().unwrap().extend(documents);
            }
            Ok(count)
        }
    }
    let exporter = Arc::new(Capture::default());
    let telemetry = Arc::new(Telemetry::from_conf(
        "<moqui-conf><telemetry sample-ratio=\"0\" export-interval=\"10\"/></moqui-conf>",
        Resource::default(),
    ));
    telemetry.set_exporter(exporter.clone());
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(
            "/screen",
            get(|| async {
                let mut response = axum::response::Response::new(axum::body::Body::from("hi"));
                response.extensions_mut().insert(super::LoggedIdentity {
                    session: Some("session-1".into()),
                    visitor_id: Some("visitor-1".into()),
                    user_id: Some("user-1".into()),
                    ..Default::default()
                });
                response
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            super::RequestTelemetry {
                telemetry: Arc::clone(&telemetry),
            },
            super::log_http,
        ));
    app.clone()
        .oneshot(
            Request::builder()
                .uri("/screen?tab=1&password=private")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    app.oneshot(
        Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    telemetry.flush();
    let entries = exporter.0.lock().unwrap();
    let screen = entries
        .iter()
        .find(|entry| entry.get("url.path").and_then(Value::as_str) == Some("/screen"))
        .expect("root span");
    assert_eq!(screen["kind"].as_str(), Some("SERVER"));
    assert_eq!(screen["session.id"].as_str(), Some("session-1"));
    assert_eq!(screen["user.id"].as_str(), Some("user-1"));
    assert_eq!(screen["moqui.visitor.id"].as_str(), Some("visitor-1"));
    assert!(!screen["url.query"].as_str().unwrap().contains("private"));
    assert!(
        !entries
            .iter()
            .any(|entry| entry.get("url.path").and_then(Value::as_str) == Some("/health")),
        "a health probe writes no request root"
    );
    drop(entries);
    telemetry.shutdown();
}

/// A server error's root keeps the request's own error, and a page's later request is its own trace that names the
/// page's: the client's traceparent sets the parent, not the sampling, and a bad page trace id is ignored.
#[cfg(feature = "telemetry")]
#[tokio::test]
async fn request_roots_keep_real_errors_and_name_the_page_without_joining_its_trace() {
    use moqui_core::{
        entity::document::Document,
        telemetry::{Exporter, Resource, Telemetry},
    };
    use moqui_lib::{error::Error, object::Value};
    use tower::ServiceExt as _;
    #[derive(Default)]
    struct Capture(std::sync::Mutex<Vec<Document>>);
    impl Exporter for Capture {
        fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
            let count = documents.len();
            if index == "otel_spans" {
                self.0.lock().unwrap().extend(documents);
            }
            Ok(count)
        }
    }
    let exporter = Arc::new(Capture::default());
    let telemetry = Arc::new(Telemetry::from_conf(
        "<moqui-conf><telemetry sample-ratio=\"0\"/></moqui-conf>",
        Resource::default(),
    ));
    telemetry.set_exporter(exporter.clone());
    let failing = |message: Option<&'static str>| async move {
        let mut response = axum::response::Response::new(axum::body::Body::from("failed"));
        *response.status_mut() = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
        let mut identity = super::LoggedIdentity::default();
        if let Some(message) = message {
            identity
                .attributes
                .insert("exception.message".into(), Value::string(message));
        }
        response.extensions_mut().insert(identity);
        response
    };
    let app = Router::new()
        .route("/real", get(move || failing(Some("order 7 could not be shipped"))))
        .route("/bare", get(move || failing(None)))
        .route("/page", get(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(
            super::RequestTelemetry {
                telemetry: Arc::clone(&telemetry),
            },
            super::log_http,
        ));
    let page = "0123456789abcdef0123456789abcdef";
    for (path, page_header) in [
        ("/real", page),
        ("/bare", page),
        ("/page", page),
        ("/page?bad", "0123456789ABCDEF0123456789ABCDEF"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("x-moqui-page-trace", page_header)
                    .header("traceparent", "00-34343434343434343434343434343434-5656565656565656-01")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
    }
    telemetry.flush();
    let entries = exporter.0.lock().unwrap();
    let root = |path: &str, query: &str| {
        entries
            .iter()
            .find(|entry| {
                entry.get("url.path").and_then(Value::as_str) == Some(path)
                    && entry.get("url.query").and_then(Value::as_str) == Some(query)
            })
            .unwrap_or_else(|| panic!("no root for {path}?{query}: {entries:?}"))
    };
    let real = root("/real", "");
    assert_eq!(real["status_code"].as_str(), Some("ERROR"));
    assert_eq!(real["exception.message"].as_str(), Some("order 7 could not be shipped"));
    assert_eq!(
        root("/bare", "")["exception.message"].as_str(),
        Some("HTTP server error")
    );
    let page_trace = |entry: &Document| match entry.get("attributes") {
        Some(Value::Map(attributes)) => attributes
            .get("moqui.page.trace_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    };
    let named = root("/page", "");
    assert_eq!(page_trace(named).as_deref(), Some(page));
    assert_ne!(named["trace_id"].as_str(), Some(page), "the page trace is not a parent");
    assert_eq!(
        named["trace_id"].as_str(),
        Some("34343434343434343434343434343434"),
        "the client's traceparent still sets the parent"
    );
    assert_eq!(
        named["sampled"],
        Value::Bool(false),
        "the client's sampled flag does not turn sampling on"
    );
    assert_eq!(page_trace(root("/page", "bad=")), None);
    drop(entries);
    telemetry.shutdown();
}

#[tokio::test]
#[cfg(feature = "telemetry")]
async fn streamed_request_roots_finish_on_consumption_failure_or_cancellation() {
    use moqui_core::{
        entity::document::Document,
        telemetry::{Exporter, Resource, Telemetry},
    };
    use moqui_lib::{error::Error, object::Value};
    use tower::ServiceExt as _;
    #[derive(Default)]
    struct Capture(std::sync::Mutex<Vec<Document>>);
    impl Exporter for Capture {
        fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
            let count = documents.len();
            if index == "otel_spans" {
                self.0.lock().unwrap().extend(documents);
            }
            Ok(count)
        }
    }
    let capture = Arc::new(Capture::default());
    let telemetry = Arc::new(Telemetry::from_conf(
        "<moqui-conf><telemetry sample-ratio=\"0\"/></moqui-conf>",
        Resource::default(),
    ));
    telemetry.set_exporter(capture.clone());
    let app = Router::new()
        .route(
            "/delayed",
            get(|| async {
                axum::body::Body::from_stream(futures_util::stream::once(async {
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"payload"))
                }))
            }),
        )
        .route(
            "/failed",
            get(|| async {
                axum::body::Body::from_stream(futures_util::stream::iter([
                    Ok(bytes::Bytes::from_static(b"partial")),
                    Err(std::io::Error::other("fixture body failure")),
                ]))
            }),
        )
        .route(
            "/cancelled",
            get(|| async {
                axum::body::Body::from_stream(futures_util::stream::pending::<Result<bytes::Bytes, std::io::Error>>())
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            super::RequestTelemetry {
                telemetry: telemetry.clone(),
            },
            super::log_http,
        ));
    let request = |path| Request::builder().uri(path).body(axum::body::Body::empty()).unwrap();
    let response = app.clone().oneshot(request("/delayed")).await.unwrap();
    telemetry.flush();
    assert!(
        capture.0.lock().unwrap().is_empty(),
        "response headers must not end a streaming root"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 100).await.unwrap();
    assert_eq!(&bytes[..], b"payload");
    let response = app.clone().oneshot(request("/failed")).await.unwrap();
    assert!(axum::body::to_bytes(response.into_body(), 100).await.is_err());
    let response = app.oneshot(request("/cancelled")).await.unwrap();
    drop(response);
    telemetry.flush();
    let spans = capture.0.lock().unwrap();
    assert_eq!(spans.len(), 3, "every request root survives zero child sampling");
    for (path, completed, bytes) in [("/delayed", true, 7), ("/failed", false, 7), ("/cancelled", false, 0)] {
        let span = spans
            .iter()
            .find(|span| span.get("url.path").and_then(Value::as_str) == Some(path))
            .unwrap();
        assert_eq!(span["http.response.body.size"], Value::I64(bytes));
        let attributes = moqui_lib::json::to_json(span.get("attributes").unwrap());
        assert_eq!(attributes["moqui.http.response.completed"], completed);
        if !completed {
            assert_eq!(span["status_code"].as_str(), Some("ERROR"));
        } else {
            let number = |key: &str| moqui_lib::object::to_plain_string(&span[key]).parse::<f64>().unwrap();
            assert!(number("duration_nano") >= number("moqui.http.time_to_first_byte") * 1_000_000.0 + 20_000_000.0);
        }
    }
    drop(spans);
    telemetry.shutdown();
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
    let server = tokio::spawn(serve(
        listener,
        None,
        WebApp::new(app, limits, unguarded()),
        permits,
        async move {
            let _ = stopped.await;
        },
    ));

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
    let server = tokio::spawn(serve(
        listener,
        None,
        WebApp::new(app, limits, unguarded()),
        permits,
        async move {
            let _ = stopped.await;
        },
    ));
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
                Some(super::Tls {
                    acceptor: tokio_rustls::TlsAcceptor::from(config),
                    edge: None,
                }),
                WebApp::new(app.clone(), limits, unguarded()),
                Arc::clone(&permits),
                async move {
                    let _ = secure_stopped.await;
                },
            ),
            serve(
                plain_listener,
                None,
                WebApp::new(app, limits, unguarded()),
                permits,
                async move {
                    let _ = plain_stopped.await;
                }
            ),
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
    let server = tokio::spawn(serve(
        listener,
        None,
        WebApp::new(app, limits, guard),
        permits,
        async move {
            let _ = stopped.await;
        },
    ));
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

#[tokio::test]
async fn a_websocket_over_extended_connect_is_checked_as_a_get() {
    let guard = crate::guard::tests::guard(
        r#"<web-protection enabled="true"><requests methods="GET,POST"/></web-protection>"#,
        "",
    );
    let app = Router::new().fallback(|| async { "ok" });
    let web = WebApp::new(app, ListenLimits::defaults(), guard);
    let peer = super::Peer {
        remote: "203.0.113.9:4000".parse().unwrap(),
        client: "203.0.113.9".parse().unwrap(),
        secure: true,
        https: true,
        exporter: None,
    };
    let connect = || {
        axum::http::Request::builder()
            .method(axum::http::Method::CONNECT)
            .uri("/notws")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    // a client's own CONNECT is not an allowed method
    let refused = super::serve_request(&web, connect(), &peer).await.unwrap();
    assert_eq!(refused.status(), 405);
    // the down connection's WebSocket, marked as such, is checked as the GET of an upgrade
    let mut websocket = connect();
    websocket.extensions_mut().insert(super::ExtendedConnect);
    let taken = super::serve_request(&web, websocket, &peer).await.unwrap();
    assert_eq!(taken.status(), 200);
}

// ---- connection limits after the response head (HL-027) -----------------------------------

/// A listener serving `app` with `guard` and `limits`, until the sender drops.
async fn listening(
    app: Router,
    guard: Arc<Guard>,
    limits: ListenLimits,
) -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let permits = Arc::new(Semaphore::new(limits.connections));
    let server = tokio::spawn(serve(
        listener,
        None,
        WebApp::new(app, limits, guard),
        permits,
        async move {
            let _ = stopped.await;
        },
    ));
    (address, stop, server)
}

/// A client with a small receive buffer that sends `request` and then reads nothing, so the server's writes stall.
async fn not_reading(address: std::net::SocketAddr, request: &str) -> TcpStream {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let mut stream = socket.connect(address).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream
}

/// The answer to an upgrade to a WebSocket at `path`, read up to the end of its head.
async fn upgraded(address: std::net::SocketAddr, path: &str) -> (TcpStream, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut byte)).await {
            Ok(Ok(1)) => head.push(byte[0]),
            _ => break,
        }
    }
    (stream, String::from_utf8_lossy(&head).into_owned())
}

const EIGHT_MIB: usize = 8 * 1024 * 1024;

/// With two connection slots, two clients that ask for 8 MiB and never read are closed by the write timeout, and a
/// third connection, which waited for a slot, is served.
#[tokio::test]
async fn clients_that_never_read_a_large_answer_are_closed_by_the_write_timeout() {
    let app = Router::new()
        .route("/big", get(|| async { vec![b'x'; EIGHT_MIB] }))
        .route("/", get(|| async { "ok" }));
    let limits = ListenLimits {
        connections: 2,
        write: Some(Duration::from_millis(300)),
        ..ListenLimits::defaults()
    };
    let (address, stop, server) = listening(app, unguarded(), limits).await;
    let started = Instant::now();
    let _first = not_reading(address, "GET /big HTTP/1.1\r\nHost: x\r\n\r\n").await;
    let _second = not_reading(address, "GET /big HTTP/1.1\r\nHost: x\r\n\r\n").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let answered = ask(address, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await;
    assert!(answered.starts_with("HTTP/1.1 200"), "{answered}");
    assert!(
        started.elapsed() >= Duration::from_millis(250) && started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    drop(stop);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the listener did not stop")
        .unwrap();
}

/// The same for a WebSocket that pushes (as `/notws` does) to a client that reads nothing: the upgraded socket keeps
/// its connection slot while open, and the write timeout closes it and frees the slot.
#[tokio::test]
async fn a_websocket_whose_client_never_reads_is_closed_by_the_write_timeout() {
    use axum::extract::ws::{Message, WebSocketUpgrade};
    let app = Router::new()
        .route(
            "/notws",
            get(|ws: WebSocketUpgrade| async {
                ws.on_upgrade(|mut socket| async move {
                    let chunk = vec![7u8; 64 * 1024];
                    while socket.send(Message::Binary(chunk.clone().into())).await.is_ok() {}
                })
            }),
        )
        .route("/", get(|| async { "ok" }));
    let limits = ListenLimits {
        connections: 1,
        write: Some(Duration::from_millis(300)),
        ..ListenLimits::defaults()
    };
    let (address, stop, server) = listening(app, unguarded(), limits).await;
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let mut pushed = socket.connect(address).await.unwrap();
    pushed
        .write_all(
            b"GET /notws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .await
        .unwrap();
    let started = Instant::now();
    tokio::time::sleep(Duration::from_millis(50)).await;
    // the one slot is the socket's until the write timeout closes it
    let answered = ask(address, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await;
    assert!(answered.starts_with("HTTP/1.1 200"), "{answered}");
    assert!(
        started.elapsed() >= Duration::from_millis(200) && started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    drop(stop);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the listener did not stop")
        .unwrap();
}

/// An upgraded socket stays inside `max-per-client`: with two `/callws` sockets open from one address, a third
/// connection is closed before its request, and one more is served once a socket closes.
#[tokio::test]
async fn upgraded_sockets_count_against_max_per_client() {
    use axum::extract::ws::WebSocketUpgrade;
    let guard = crate::guard::tests::guard(
        r#"<web-protection enabled="true"><connections max-per-client="2"/></web-protection>"#,
        "",
    );
    let app = Router::new()
        .route(
            "/callws",
            get(|ws: WebSocketUpgrade| async {
                ws.on_upgrade(|mut socket| async move { while let Some(Ok(_)) = socket.recv().await {} })
            }),
        )
        .fallback(|| async { "ok" });
    let limits = ListenLimits {
        header: Some(Duration::from_secs(5)),
        ..ListenLimits::defaults()
    };
    let (address, stop, server) = listening(app, guard, limits).await;
    let (first, head) = upgraded(address, "/callws").await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    let (_second, head) = upgraded(address, "/callws").await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    let (_third, head) = upgraded(address, "/callws").await;
    assert!(
        !head.contains(" 101 "),
        "a third socket from one address is refused: {head}"
    );
    drop(first);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let answered = ask(address, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await;
    assert!(answered.starts_with("HTTP/1.1 200"), "{answered}");
    drop(stop);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the listener did not stop")
        .unwrap();
}
