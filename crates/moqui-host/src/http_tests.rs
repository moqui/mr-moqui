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

use super::{
    header_map, http_port, serve, status_allowed, status_request_allowed, ListenLimits, TlsConnection, WebApp,
};
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
                token_routes: Arc::new(Arc::default),
                all_query_values: Arc::new(|| false),
            },
            super::log_http,
        ));
    app.clone()
        .oneshot(
            Request::builder()
                .uri("/screen?tab=1&password=private&q=Ann%20Smith&pageIndex=2")
                .header(
                    "referer",
                    "https://erp.example.com/apps/find?q=ann%40example.com&pageSize=50#results",
                )
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
    // a digest of the session id, never the id that signs the session in (SEC-TLS-18)
    assert_eq!(
        screen["session.id"].as_str(),
        Some(super::session_tag("session-1").as_str())
    );
    assert_eq!(super::session_tag("session-1").len(), 32);
    assert_ne!(super::session_tag("session-1"), super::session_tag("session-2"));
    assert_eq!(screen["user.id"].as_str(), Some("user-1"));
    assert_eq!(screen["moqui.visitor.id"].as_str(), Some("visitor-1"));
    // names kept, values only of paging and the like, a secret's redacted (SEC-TLS-18)
    let query = screen["url.query"].as_str().unwrap();
    assert!(query.starts_with("tab=[…]&password="), "{query}");
    assert!(query.ends_with("&q=[…]&pageIndex=2"), "{query}");
    assert!(!query.contains("private") && !query.contains("Ann"), "{query}");
    assert_eq!(
        screen["http.request.header.referer"].as_str(),
        Some("https://erp.example.com/apps/find?q=[…]&pageSize=50")
    );
    assert!(
        !entries
            .iter()
            .any(|entry| entry.get("url.path").and_then(Value::as_str) == Some("/health")),
        "a health probe writes no request root"
    );
    drop(entries);
    telemetry.shutdown();
}

/// A token route's token is its credential: the request root records `/i/:token/...` for its path, and for a Referer
/// and a sign-in step's `returnTo` that hold one; a path under no route is kept.
#[cfg(feature = "telemetry")]
#[tokio::test]
async fn a_token_routes_token_stays_out_of_the_request_root() {
    use moqui_core::{
        entity::document::Document,
        screen::token_route::{Redaction, TokenRoute},
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
    let route = TokenRoute {
        name: "i".into(),
        screen_location: "component://test/Accept.xml".into(),
        token_service: "test.check#Token".into(),
        title: None,
        post_transition: None,
    };
    // SEC-PSF-03: a component's token paths (PASS's held mail and preview links) too
    let redaction = Arc::new(Redaction {
        routes: vec![route],
        paths: ["/preview/{token}", "/**/held-mail/{token}"]
            .iter()
            .filter_map(|pattern| moqui_core::screen::token_route::TokenPath::parse(pattern))
            .collect(),
    });
    let app = Router::new()
        .fallback(|| async { "ok" })
        .layer(axum::middleware::from_fn_with_state(
            super::RequestTelemetry {
                telemetry: Arc::clone(&telemetry),
                token_routes: Arc::new(move || Arc::clone(&redaction)),
                // every query value kept, so a token in one is seen taken out (the default keeps none of these)
                all_query_values: Arc::new(|| true),
            },
            super::log_http,
        ));
    const TOKEN: &str = "InviteToken0123456789abcdefXY";
    for (uri, referer) in [
        (
            format!("/i/{TOKEN}/acceptAccount"),
            format!("https://staff.example.com/i/{TOKEN}"),
        ),
        (
            format!("/Login?returnTo=%2Fi%2F{TOKEN}%2Ffactor&tab=1"),
            format!("https://staff.example.com/i/{TOKEN}/factor"),
        ),
        ("/apps/i/Something12345".to_string(), String::new()),
        (
            format!("/forum/held-mail/{TOKEN}/claim"),
            format!("https://pass.example.com/preview/{TOKEN}/asset/a.css"),
        ),
        (
            format!("/Login?returnTo=%2Facme%2F-%2Fforum%2Fheld-mail%2F{TOKEN}"),
            String::new(),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("referer", referer)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
    }
    telemetry.flush();
    let entries = exporter.0.lock().unwrap();
    let text = format!("{entries:?}");
    assert!(!text.contains(TOKEN), "no token in the request roots: {text}");
    let attribute = |entry: &Document, name: &str| -> Option<String> {
        entry
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| match entry.get("attributes") {
                Some(Value::Map(attributes)) => attributes.get(name).and_then(Value::as_str).map(str::to_string),
                _ => None,
            })
    };
    let accept = entries
        .iter()
        .find(|entry| attribute(entry, "url.path").as_deref() == Some("/i/:token/acceptAccount"))
        .unwrap_or_else(|| panic!("the route's root: {text}"));
    assert_eq!(
        attribute(accept, "http.request.header.referer").as_deref(),
        Some("https://staff.example.com/i/:token")
    );
    let root_with = |path: &str, query: &str| {
        entries.iter().any(|entry| {
            attribute(entry, "url.path").as_deref() == Some(path)
                && attribute(entry, "url.query").as_deref() == Some(query)
        })
    };
    assert!(
        root_with("/Login", "returnTo=/i/:token/factor&tab=1"),
        "the sign-in root: {text}"
    );
    assert!(
        root_with("/Login", "returnTo=/acme/-/forum/held-mail/:token"),
        "a token path in returnTo: {text}"
    );
    let held = entries
        .iter()
        .find(|entry| attribute(entry, "url.path").as_deref() == Some("/forum/held-mail/:token/claim"))
        .unwrap_or_else(|| panic!("the token path's root: {text}"));
    assert_eq!(
        attribute(held, "http.request.header.referer").as_deref(),
        Some("https://pass.example.com/preview/:token/asset/a.css")
    );
    assert!(
        entries
            .iter()
            .any(|entry| attribute(entry, "url.path").as_deref() == Some("/apps/i/Something12345")),
        "a path under no route is kept: {text}"
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
                token_routes: Arc::new(Arc::default),
                all_query_values: Arc::new(|| false),
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
    assert_eq!(page_trace(root("/page", "bad=[…]")), None);
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
                token_routes: Arc::new(Arc::default),
                all_query_values: Arc::new(|| false),
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
                    factory: std::sync::Weak::new(),
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
    use moqui_core::web::RequestVia::{Direct, Edge};
    assert!(status_allowed("", "127.0.0.1", Direct));
    assert!(status_allowed("", "::1", Direct));
    assert!(status_allowed("", "0:0:0:0:0:0:0:1", Direct));
    // an IPv4 peer seen through a dual-stack socket is its IPv4 address
    let mapped: std::net::IpAddr = "::ffff:127.0.0.1".parse().unwrap();
    assert!(status_allowed("", &mapped.to_canonical().to_string(), Direct));
    assert!(!status_allowed("127.0.0.1", "203.0.113.5", Direct));
    // all of 127.0.0.0/8 is loopback (a client spec's own address on a test server)
    assert!(status_allowed("", "127.1.0.7", Direct));
    assert!(!status_allowed("", "128.0.0.1", Direct));
    assert!(status_allowed("10.0.0.9, 203.0.113.5", "203.0.113.5", Direct));
    // the loopback client an edge node names is not this machine (SEC-CLU-01); a listed address still is allowed
    assert!(!status_allowed("", "127.0.0.1", Edge));
    assert!(!status_allowed("", "::1", Edge));
    assert!(status_allowed("203.0.113.5", "203.0.113.5", Edge));
}

/// A proxy on this machine forwards every client from loopback: the client its header names must be listed in
/// `status-ips`, a loopback one too (SEC-WEB-11).
#[test]
fn status_routes_check_the_client_a_proxy_names() {
    use moqui_core::web::RequestVia::Direct;
    let ip = |text: &str| text.parse::<std::net::IpAddr>().unwrap();
    let local = ip("127.0.0.1");
    // a probe on this machine, with no proxy in between
    assert!(status_request_allowed("", local, local, Direct));
    // the local proxy forwarding a client from the internet, or one that names loopback
    assert!(!status_request_allowed("", local, ip("198.51.100.7"), Direct));
    assert!(!status_request_allowed("", local, ip("::1"), Direct));
    assert!(!status_request_allowed("", local, ip("127.0.0.2"), Direct));
    // the monitor named in status-ips, through the proxy
    assert!(status_request_allowed("10.0.0.9", local, ip("10.0.0.9"), Direct));
    assert!(status_request_allowed("10.0.0.9", local, ip("::ffff:10.0.0.9"), Direct));
    // a listed proxy forwarding a client that is not listed
    assert!(!status_request_allowed(
        "10.0.0.2",
        ip("10.0.0.2"),
        ip("198.51.100.7"),
        Direct
    ));
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
        via: moqui_core::web::RequestVia::Direct,
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

/// A server that `moqui test --serve` did not start takes no `X-Moqui-Test-Peer`: the request keeps its own peer and
/// client address, and the header is gone before it is served (http/test_peer.rs).
#[tokio::test]
async fn a_test_peer_header_is_dropped_and_not_honoured_off_a_test_server() {
    let guard = crate::guard::tests::guard(r#"<web-protection enabled="true"/>"#, "");
    let app = Router::new().fallback(
        |axum::extract::ConnectInfo(remote): axum::extract::ConnectInfo<std::net::SocketAddr>,
         axum::Extension(super::ClientAddr(client)): axum::Extension<super::ClientAddr>,
         headers: axum::http::HeaderMap| async move {
            format!(
                "{} {client} {}",
                remote.ip(),
                headers.contains_key(super::test_peer::HEADER)
            )
        },
    );
    let web = WebApp::new(app, ListenLimits::defaults(), guard);
    let peer = super::Peer {
        remote: "127.0.0.1:4000".parse().unwrap(),
        client: "127.0.0.1".parse().unwrap(),
        secure: false,
        https: false,
        exporter: None,
        via: moqui_core::web::RequestVia::Direct,
    };
    let request = axum::http::Request::builder()
        .uri("/")
        .header("X-Moqui-Test-Peer", "127.1.0.7")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = super::serve_request(&web, request, &peer).await.unwrap();
    let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&body), "127.0.0.1 127.0.0.1 false");
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
    let (second, head) = upgraded(address, "/callws").await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    let (third, head) = upgraded(address, "/callws").await;
    assert!(
        !head.contains(" 101 "),
        "a third socket from one address is refused: {head}"
    );
    drop(first);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let answered = ask(address, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await;
    assert!(answered.starts_with("HTTP/1.1 200"), "{answered}");
    // the stop's drain waits for open sockets
    drop((second, third));
    drop(stop);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the listener did not stop")
        .unwrap();
}

/// The stop's drain waits for an open WebSocket as for a connection, and ends once it closes (HL-027).
#[tokio::test]
async fn the_shutdown_drain_waits_for_an_open_websocket() {
    use axum::extract::ws::WebSocketUpgrade;
    let app = Router::new().route(
        "/notws",
        get(|ws: WebSocketUpgrade| async {
            ws.on_upgrade(|mut socket| async move { while let Some(Ok(_)) = socket.recv().await {} })
        }),
    );
    let limits = ListenLimits {
        shutdown: Some(Duration::from_secs(5)),
        ..ListenLimits::defaults()
    };
    let (address, stop, mut server) = listening(app, unguarded(), limits).await;
    let (socket, head) = upgraded(address, "/notws").await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    drop(stop);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut server)
            .await
            .is_err(),
        "the drain waits for the open socket"
    );
    drop(socket);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("the drain ended once the socket closed")
        .unwrap();
}

/// The first segments of the paths the host registers routes for, from its source: each `.route("/...")` literal,
/// with the routes registered by a constant named here.
fn registered_route_paths() -> Vec<String> {
    fn walk(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut paths = Vec::new();
    for file in files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with("_tests.rs") || name == "tests.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let mut rest = text.as_str();
        while let Some((_, after)) = rest.split_once(".route(") {
            rest = after;
            if let Some(literal) = after.trim_start().strip_prefix('"') {
                if let Some((path, _)) = literal.split_once('"') {
                    paths.push(path.to_string());
                }
            }
        }
    }
    #[cfg(feature = "xmpp")]
    paths.extend([
        crate::xmpp::upload::PREFIX.to_string(),
        crate::xmpp::ws::PATH.to_string(),
    ]);
    paths
}

/// Every route the host registers starts with a segment of the framework's one list of reserved paths
/// (`moqui_core::web::reserved`), so a public site's pages, git at a page's URL and an application's names (which read
/// it as `ec.web.reservedPaths`) never take a path the host answers first.
#[test]
fn every_host_route_is_under_a_reserved_first_segment() {
    let paths = registered_route_paths();
    assert!(
        paths.iter().any(|path| path == "/health") && paths.iter().any(|path| path == "/dav/{*rest}"),
        "the route literals were found: {paths:?}"
    );
    for path in &paths {
        assert!(
            moqui_core::web::reserved::is_reserved_path(path),
            "the host route {path} is not under a path moqui_core::web::reserved lists (a wholly reserved first \
             segment, or a sub-path of a partly reserved one)"
        );
    }
}

/// The request deadline: none for the paths that run as long as they need, a socket or an upload, and, on an edge
/// node, for any request it forwards to a member, which applies its own with the exemptions only it can tell (a push
/// to git at a public site's page URL, whose site rows the edge node does not have).
#[test]
fn a_forwarded_request_has_no_deadline_on_the_edge_node() {
    let limit = Some(std::time::Duration::from_secs(300));
    let deadline = |path: &str, streaming: bool, forwarded: bool| {
        super::serve::request_deadline(limit, path, streaming, forwarded)
    };
    assert_eq!(deadline("/apps/x", false, false), limit);
    assert_eq!(deadline("/acme/widget.git/git-receive-pack", false, false), limit);
    assert_eq!(deadline("/acme/widget.git/git-receive-pack", false, true), None);
    assert_eq!(deadline("/acme/widget.git/info/lfs/objects/abc", false, true), None);
    for path in [
        "/git/acme/widget.git/info/refs",
        "/ci/runner/jobs/j1/log",
        "/dav/x",
        "/llm/v1/chat",
        "/xmpp-upload/a",
    ] {
        assert_eq!(deadline(path, false, false), None, "{path}");
    }
    assert_eq!(deadline("/apps/upload", true, false), None);
    assert_eq!(super::serve::request_deadline(None, "/apps/x", false, false), None);
}

/// The answer to a request with a CI token: its marker records the token as found good (here, and kept for an edge
/// node in front when the request came down its link), a 401 or 410 forgets it; a marker on the answer of a request
/// with no CI token is removed and believes nothing.
#[test]
fn a_ci_token_answer_is_recorded_and_its_marker_removed_at_the_outermost_node() {
    use axum::response::IntoResponse as _;
    let guard = crate::guard::tests::guard(
        r#"<web-protection enabled="true"><rate-limit name="all" requests="1" seconds="60" burst="1"/></web-protection>"#,
        "",
    );
    let token = format!("mqcj_{}", "ab".repeat(32));
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let client: std::net::IpAddr = "203.0.113.70".parse().unwrap();
    let check = || {
        guard
            .check(
                client,
                client,
                "ci.example.com",
                "POST",
                "/ci/runner/jobs/j1/log",
                &headers,
            )
            .is_ok()
    };
    let marked = |status: u16| {
        let mut response = (axum::http::StatusCode::from_u16(status).unwrap(), "").into_response();
        response
            .headers_mut()
            .insert(crate::guard::CI_TOKEN_HEADER, "verified".parse().unwrap());
        response
    };
    assert!(check());
    assert!(!check(), "limited by address before it is found good");
    let mut response = marked(200);
    super::serve::ci_token_answer(&guard, "ci.example.com", Some(&token), &mut response, true);
    assert!(
        response.headers().contains_key(crate::guard::CI_TOKEN_HEADER),
        "kept for the edge node"
    );
    assert!(check() && check());
    let mut response = marked(200);
    super::serve::ci_token_answer(&guard, "ci.example.com", Some(&token), &mut response, false);
    assert!(
        !response.headers().contains_key(crate::guard::CI_TOKEN_HEADER),
        "removed at the outermost node"
    );
    let mut gone = (axum::http::StatusCode::GONE, "").into_response();
    super::serve::ci_token_answer(&guard, "ci.example.com", Some(&token), &mut gone, false);
    assert!(!check(), "a job that ended: by address again");
    let mut response = marked(200);
    super::serve::ci_token_answer(&guard, "ci.example.com", None, &mut response, true);
    assert!(!response.headers().contains_key(crate::guard::CI_TOKEN_HEADER));
}

/// Credentials are taken over TLS, a believed proxy's https, or from a client on this machine; behind a believed proxy
/// on this host the client is the one it names, so plain HTTP the proxy passes for another client is not secure
/// (SEC-AI-11, SEC-GIT-09).
#[test]
fn a_loopback_peer_is_secure_only_when_it_is_the_client() {
    use moqui_core::web::address::{parse_cidrs, ClientAddressing};
    let loopback: std::net::IpAddr = "127.0.0.1".parse().unwrap();
    let public: std::net::IpAddr = "203.0.113.7".parse().unwrap();
    let headers = |pairs: &[(&'static str, &str)]| {
        let mut map = axum::http::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().unwrap());
        }
        map
    };
    // no proxy believed: the socket peer is the client
    let direct = ClientAddressing::new("", Vec::new());
    assert!(super::secure_transport(false, &headers(&[]), loopback, &direct));
    assert!(super::secure_transport(
        false,
        &headers(&[]),
        "::1".parse().unwrap(),
        &direct
    ));
    assert!(!super::secure_transport(false, &headers(&[]), public, &direct));
    assert!(super::secure_transport(true, &headers(&[]), public, &direct));
    // a proxy on this host is believed: the client it names decides, and its X-Forwarded-Proto
    let proxied = ClientAddressing::new("x-forwarded-for", parse_cidrs("127.0.0.1/32", "test"));
    let remote = headers(&[("x-forwarded-for", "198.51.100.4")]);
    assert!(
        !super::secure_transport(false, &remote, loopback, &proxied),
        "plain HTTP for a remote client through a proxy on this host"
    );
    let remote_https = headers(&[("x-forwarded-for", "198.51.100.4"), ("x-forwarded-proto", "https")]);
    assert!(super::secure_transport(false, &remote_https, loopback, &proxied));
    assert!(super::secure_transport(
        false,
        &headers(&[("x-forwarded-for", "127.0.0.1")]),
        loopback,
        &proxied
    ));
    assert!(
        !super::secure_transport(false, &headers(&[]), loopback, &proxied),
        "a request the proxy passes without naming its client"
    );
    // a request an edge node forwards is never this machine's, whatever address it arrives from (SEC-CLU-01)
    let request = |via: moqui_core::web::RequestVia| {
        let (mut parts, ()) = axum::http::Request::new(()).into_parts();
        parts.extensions.insert(via);
        parts
    };
    assert!(super::secure_request(false, &request(moqui_core::web::RequestVia::default()), loopback, &direct));
    assert!(!super::secure_request(false, &request(moqui_core::web::RequestVia::Edge), loopback, &direct));
    assert!(super::secure_request(true, &request(moqui_core::web::RequestVia::Edge), loopback, &direct));
}

/// SEC-TLS-18: `query-values="all"` keeps every value for debugging, a secret's still redacted and a referrer's fragment
/// kept; the default keeps names and the paging values only.
#[test]
fn a_span_keeps_query_values_only_when_the_conf_says_all() {
    let routes = moqui_core::screen::token_route::Redaction::default();
    assert_eq!(
        super::redacted_query("q=Ann&api_key=k&orderByField=-name", &routes, true),
        "q=Ann&api_key=[REDACTED]&orderByField=-name"
    );
    assert_eq!(
        super::redacted_query("q=Ann&api_key=k&orderByField=-name", &routes, false),
        "q=[…]&api_key=[REDACTED]&orderByField=-name"
    );
    assert_eq!(
        super::redacted_referer("https://h/p?q=Ann#x", &routes, true),
        "https://h/p?q=Ann#x"
    );
    assert_eq!(super::redacted_referer("https://h/p#x", &routes, false), "https://h/p");
    assert_eq!(super::redacted_referer("", &routes, false), "");
}

/// SEC-RTE-91: an upload has its idle timeout, cut to when its average would fall under `upload-min-rate` once
/// `body-timeout` has passed, so a byte now and then does not hold a connection; at most `max-uploads` arrive at once.
#[test]
fn an_upload_must_keep_a_minimum_rate_and_only_so_many_arrive_at_once() {
    use super::{upload_wait, BodyRefused, UploadSlot};
    let limits = ListenLimits {
        body: Some(Duration::from_secs(60)),
        upload_idle: Some(Duration::from_secs(60)),
        upload_min_rate: 1024,
        ..ListenLimits::defaults()
    };
    let wait = |elapsed: u64, total: usize| upload_wait(&limits, Duration::from_secs(elapsed), total);
    // within body-timeout the idle timeout alone
    assert_eq!(wait(0, 0).ok().flatten(), Some(Duration::from_secs(60)));
    // then the time the bytes so far earn at the rate
    assert_eq!(wait(50, 0).ok().flatten(), Some(Duration::from_secs(10)));
    assert_eq!(wait(100, 50 * 1024).ok().flatten(), Some(Duration::from_secs(10)));
    // a byte every 50 seconds falls behind
    assert!(matches!(wait(110, 2), Err(BodyRefused::TimedOut)));
    // 0 turns the rate off
    let off = ListenLimits { upload_min_rate: 0, ..limits };
    assert_eq!(upload_wait(&off, Duration::from_secs(1000), 0).ok().flatten(), Some(Duration::from_secs(60)));

    let first = UploadSlot::take(usize::MAX).ok().expect("a slot");
    let held = super::UPLOADS_ARRIVING.load(std::sync::atomic::Ordering::Acquire);
    assert!(matches!(UploadSlot::take(held), Err(BodyRefused::SpoolFull)));
    drop(first);
}
