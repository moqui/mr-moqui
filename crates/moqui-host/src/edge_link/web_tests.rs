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

use super::*;

#[test]
fn hop_by_hop_and_forwarding_headers_are_not_passed_on() {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("connection", "keep-alive, X-Custom-Hop"),
        ("keep-alive", "timeout=5"),
        ("x-custom-hop", "1"),
        ("transfer-encoding", "chunked"),
        ("upgrade", "websocket"),
        ("te", "trailers"),
        ("x-forwarded-for", "198.51.100.1"),
        ("forwarded", "for=198.51.100.1"),
        ("x-real-ip", "198.51.100.1"),
        ("x-moqui-edge-client", "198.51.100.1:1"),
        ("x-moqui-edge-proto", "https"),
        ("cf-connecting-ip", "198.51.100.1"),
        ("cookie", "a=b"),
        ("accept", "text/html"),
    ] {
        headers.append(
            http::header::HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    strip_hop_by_hop(&mut headers);
    strip_forwarding(&mut headers, "CF-Connecting-IP");
    let mut left: Vec<&str> = headers.keys().map(http::header::HeaderName::as_str).collect();
    left.sort_unstable();
    assert_eq!(left, ["accept", "cookie"]);
}

#[test]
fn a_websocket_upgrade_is_seen_and_answered_by_its_key() {
    assert_eq!(
        websocket_accept(b"dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
    let upgrade = http::Request::get("/notws")
        .header("connection", "keep-alive, Upgrade")
        .header("upgrade", "WebSocket")
        .body(())
        .unwrap();
    assert!(is_websocket(&upgrade));
    let plain_get = http::Request::get("/notws").body(()).unwrap();
    assert!(!is_websocket(&plain_get));
    let post = http::Request::post("/notws")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .body(())
        .unwrap();
    assert!(!is_websocket(&post));
}

#[test]
fn the_offline_page_says_when_to_try_again() {
    let response = offline(DEFAULT_OFFLINE_PAGE);
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], RETRY_AFTER);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
}

/// A pool member's down connection in a test: an HTTP/2 server on an in-memory pipe that answers each request 200 with
/// `name`, or resets each stream before any byte of an answer (`answers` false), counting the requests it saw.
async fn member(
    name: &'static str,
    answers: bool,
) -> (
    SendRequest<tonic::body::Body>,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let (client, server) = tokio::io::duplex(1 << 16);
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&seen);
    let serving = tokio::spawn(async move {
        let service = hyper::service::service_fn(move |_request: http::Request<hyper::body::Incoming>| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                if answers {
                    Ok(http::Response::new(name.to_string()))
                } else {
                    Err(std::io::Error::other("reset before any answer"))
                }
            }
        });
        let _ = hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
            .serve_connection(TokioIo::new(server), service)
            .await;
    });
    let (send, connection) = hyper::client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
        .handshake(TokioIo::new(client))
        .await
        .unwrap();
    tokio::spawn(connection);
    (send, seen, serving)
}

fn picked(member: &str, send: &SendRequest<tonic::body::Body>) -> Picked {
    Picked {
        node: "pool-1".to_string(),
        member: member.to_string(),
        generation: 1,
        send: send.clone(),
        flight: super::super::balance::Flight::start(Arc::default(), super::super::balance::Rules::default()),
    }
}

fn down_request(method: http::Method) -> http::Request<tonic::body::Body> {
    let mut request = http::Request::new(tonic::body::Body::empty());
    *request.method_mut() = method;
    *request.uri_mut() = "https://shop.localhost/".parse().unwrap();
    request
}

/// One try and its outcomes: the answer's body or the failure, each member's outcome in order, and whether another
/// member was asked for.
async fn tried(
    first: &SendRequest<tonic::body::Body>,
    second: &SendRequest<tonic::body::Body>,
    method: http::Method,
) -> (Result<String, String>, Vec<(String, bool)>, bool) {
    let outcomes = std::sync::Mutex::new(Vec::new());
    let asked = std::sync::atomic::AtomicBool::new(false);
    let again = may_send_again(&method, true, false).then(|| {
        let method = method.clone();
        move || down_request(method)
    });
    let (answered, _) = send_once_more(
        picked("member-a", first),
        down_request(method),
        again,
        |failed| {
            assert_eq!(failed, "member-a");
            asked.store(true, std::sync::atomic::Ordering::SeqCst);
            Some(picked("member-b", second))
        },
        Duration::from_secs(5),
        |picked, ok| outcomes.lock().unwrap().push((picked.member.clone(), ok)),
    )
    .await;
    let answered = match answered {
        Ok(response) => {
            use hyper::body::Body as _;
            let mut body = response.into_body();
            let mut text = Vec::new();
            while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
                if let Ok(data) = frame.unwrap().into_data() {
                    text.extend_from_slice(&data);
                }
            }
            Ok(String::from_utf8(text).unwrap())
        }
        Err(failed) => Err(format!("{failed:?}")),
    };
    (
        answered,
        outcomes.into_inner().unwrap(),
        asked.load(std::sync::atomic::Ordering::SeqCst),
    )
}

/// A request that fails before any byte of an answer goes once more to another member only when it is a GET or HEAD
/// with no body (plans/Edge-Node2.md §4.4); a POST is never sent twice; one never sent at all (its member's connection
/// closed) goes to another member whatever it is.
#[tokio::test]
async fn a_get_that_fails_before_an_answer_goes_once_more_and_a_post_never() {
    let (failing, failing_seen, _a) = member("member-a", false).await;
    let (answering, answering_seen, _b) = member("member-b", true).await;
    let (answered, outcomes, asked) = tried(&failing, &answering, http::Method::GET).await;
    assert_eq!(answered.as_deref(), Ok("member-b"));
    assert!(asked);
    assert_eq!(
        outcomes,
        [("member-a".to_string(), false), ("member-b".to_string(), true)]
    );
    assert_eq!(failing_seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(answering_seen.load(std::sync::atomic::Ordering::SeqCst), 1);

    let (answered, outcomes, asked) = tried(&failing, &answering, http::Method::POST).await;
    assert!(answered.is_err(), "{answered:?}");
    assert!(!asked, "no other member is asked for a POST that was sent");
    assert_eq!(outcomes, [("member-a".to_string(), false)]);
    assert_eq!(
        answering_seen.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the POST reached no other member"
    );
    assert_eq!(failing_seen.load(std::sync::atomic::Ordering::SeqCst), 2);

    // a GET that fails on the second member too is not sent a third time
    let (other_failing, other_seen, _c) = member("member-c", false).await;
    let (answered, outcomes, _) = tried(&failing, &other_failing, http::Method::HEAD).await;
    assert!(answered.is_err());
    assert_eq!(outcomes.len(), 2);
    assert_eq!(other_seen.load(std::sync::atomic::Ordering::SeqCst), 1);

    // a member whose connection closed sent nothing: even a POST goes to another
    let (closed, closed_seen, serving) = member("member-a", true).await;
    serving.abort();
    let started = std::time::Instant::now();
    while !closed.is_closed() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the connection did not close"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let (answered, outcomes, asked) = tried(&closed, &answering, http::Method::POST).await;
    assert_eq!(answered.as_deref(), Ok("member-b"));
    assert!(asked);
    assert_eq!(outcomes[0], ("member-a".to_string(), false));
    assert_eq!(closed_seen.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn only_a_get_or_head_with_no_body_may_go_twice() {
    assert!(may_send_again(&http::Method::GET, true, false));
    assert!(may_send_again(&http::Method::HEAD, true, false));
    assert!(!may_send_again(&http::Method::GET, false, false), "a GET with a body");
    assert!(!may_send_again(&http::Method::GET, true, true), "a WebSocket");
    for method in [
        http::Method::POST,
        http::Method::PUT,
        http::Method::DELETE,
        http::Method::PATCH,
    ] {
        assert!(!may_send_again(&method, true, false), "{method}");
    }
}

#[test]
fn the_sticky_cookie_is_read_from_every_cookie_header() {
    let mut headers = HeaderMap::new();
    assert_eq!(sticky_value(&headers, "moqui.session"), None);
    headers.append(header::COOKIE, HeaderValue::from_static("a=1"));
    headers.append(header::COOKIE, HeaderValue::from_static("moqui.session=abc.def; b=2"));
    assert_eq!(sticky_value(&headers, "moqui.session").as_deref(), Some("abc.def"));
    assert_eq!(sticky_value(&headers, "a").as_deref(), Some("1"));
    assert_eq!(sticky_value(&headers, "JSESSIONID"), None);
    headers.insert(header::COOKIE, HeaderValue::from_static("moqui.session="));
    assert_eq!(sticky_value(&headers, "moqui.session"), None, "an empty one is none");
}
