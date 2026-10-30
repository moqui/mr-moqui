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

use std::collections::BTreeMap;
#[cfg(feature = "http")]
use std::io::{Read, Write};
#[cfg(feature = "http")]
use std::net::{TcpListener, TcpStream};
#[cfg(feature = "http")]
use std::thread;
#[cfg(feature = "http")]
use std::time::{Duration, Instant};

use moqui_lib::object::Value;

use super::run_remote_rest;

#[test]
fn a_parameter_in_the_url_cannot_change_the_host_or_the_path() {
    let mut parameters = BTreeMap::new();
    parameters.insert("id".to_string(), Value::string("a/../b?x=1#y"));
    let url = super::expand_location("http://example.test/items/${id}", &parameters).unwrap();
    assert_eq!(url, "http://example.test/items/a%2F..%2Fb%3Fx%3D1%23y");
}

#[test]
fn each_parameter_expands_in_place_and_an_unclosed_one_stays() {
    let parameters = BTreeMap::from([
        ("a".to_string(), Value::string("1")),
        ("b".to_string(), Value::string("é")),
    ]);
    let expand = |location: &str| super::expand_location(location, &parameters).unwrap();
    assert_eq!(expand("http://é.test/${a}/x${b}y?q=ü"), "http://é.test/1/x%C3%A9y?q=ü");
    assert_eq!(expand("http://example.test/${a}/${a"), "http://example.test/1/${a");
    assert_eq!(expand("http://example.test/${}"), "http://example.test/${}");
}

#[test]
fn json_responses_keep_non_ascii_text_and_exact_decimals() {
    let map = super::response_map(r#"{"name":"Zoë 🐘","price":12.50,"count":3}"#).unwrap();
    assert_eq!(map.get("name").and_then(Value::as_str), Some("Zoë 🐘"));
    assert_eq!(
        map.get("price").map(moqui_lib::object::to_plain_string).as_deref(),
        Some("12.50")
    );
    assert_eq!(map.get("count"), Some(&Value::I64(3)));
    let other = super::response_map("[1,2]").unwrap();
    assert!(matches!(other.get("response"), Some(Value::List(items)) if items.len() == 2));
    // As Java's `jsonObject()`, a body that is not a JSON object or array fails the call.
    for body in ["plain text", "", "  ", "\"text\"", "3"] {
        assert!(super::response_map(body).is_err(), "{body:?}");
    }
}

/// As Java's `parametersMapToString`: only null parameters are left out of a GET query.
#[test]
fn a_get_query_has_every_parameter_that_is_not_null() {
    let mut parameters = BTreeMap::new();
    parameters.insert("blank".to_string(), Value::string(""));
    parameters.insert(
        "ids".to_string(),
        Value::List(vec![Value::string("a"), Value::Null, Value::string("b")]),
    );
    parameters.insert("none".to_string(), Value::Null);
    parameters.insert("note".to_string(), Value::string("a b"));
    assert_eq!(super::encode_query(&parameters), "blank=&ids=a%2Cb&note=a%20b");
}

#[test]
fn verbs_pick_the_http_method_and_unknown_ones_post() {
    use crate::outbound::Method;
    assert_eq!(super::choose_method(None, "get"), Method::Get);
    assert_eq!(super::choose_method(None, "delete"), Method::Delete);
    assert_eq!(super::choose_method(Some("PUT"), "send"), Method::Put);
    assert_eq!(super::choose_method(None, "send"), Method::Post);
    assert_eq!(super::choose_method(Some("head"), "get"), Method::Head);
    assert_eq!(super::choose_method(None, "options"), Method::Options);
}

/// As Java's `RestClient`, HEAD and OPTIONS carry no body, so parameters for one fail before anything is sent.
#[test]
fn head_and_options_take_no_body() {
    let mut parameters = BTreeMap::new();
    parameters.insert("note".to_string(), Value::string("kept"));
    for method in ["HEAD", "OPTIONS"] {
        let err = run_remote_rest(
            "example.check#Item",
            "http://127.0.0.1:1/items",
            Some(method),
            "check",
            &parameters,
            false,
        )
        .expect_err("no body");
        assert!(
            err.to_string()
                .contains(&format!("Cannot use body text with method {method}")),
            "{err}"
        );
    }
}

#[test]
#[cfg(feature = "http")]
fn a_non_get_call_sends_typed_json() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || serve_one(listener));
    let mut parameters = BTreeMap::new();
    parameters.insert("count".to_string(), Value::I32(3));
    parameters.insert("flag".to_string(), Value::Bool(true));
    parameters.insert("name".to_string(), Value::string("Zoë"));
    run_remote_rest(
        "example.put#Item",
        &format!("http://127.0.0.1:{port}/items"),
        Some("PUT"),
        "put",
        &parameters,
        false,
    )
    .expect("stub response");
    let request = server.join().expect("server thread");
    assert!(request.starts_with("PUT /items"), "{request}");
    assert!(
        request.contains(r#""count":3"#) && request.contains(r#""flag":true"#),
        "{request}"
    );
}

#[test]
#[cfg(feature = "http")]
fn an_http_resource_is_fetched_when_read_and_not_when_resolved() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || serve_one(listener));
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let resource = ctx
        .factory()
        .resource()
        .get(&format!("http://127.0.0.1:{port}/items"))
        .expect("resolving opens no connection");
    let text = resource.read_text().expect("fetched on read");
    assert!(text.contains("DayOfWeek"), "{text}");
    let request = server.join().expect("server thread");
    assert!(request.starts_with("GET /items"), "{request}");
    // The result is kept, so reading again does not fetch again (the stub served only one).
    assert!(resource.read_text().is_ok());
    assert!(resource.exists());
    let missing = ctx.factory().resource().get("http://127.0.0.1:9/nothing").unwrap();
    assert!(!missing.exists());
}

#[test]
fn sim_denies_before_a_socket_is_required() {
    let err = run_remote_rest(
        "example.send#Ping",
        "http://127.0.0.1:1/no-such-remote",
        Some("GET"),
        "send",
        &BTreeMap::new(),
        true,
    )
    .expect_err("sim denies");
    let shown = err.to_string();
    assert!(shown.contains("sim mode is active"), "{shown}");
    assert!(shown.contains("http://127.0.0.1:1/no-such-remote"), "{shown}");
    assert!(!shown.to_ascii_lowercase().contains("refused"), "{shown}");
    assert!(!shown.contains("failed"), "{shown}");
}

#[test]
#[cfg(feature = "http")]
fn remote_rest_reads_the_stub_body() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server = thread::spawn(move || serve_one(listener));
    let mut parameters = BTreeMap::new();
    parameters.insert("note".to_string(), Value::string("kept"));
    let result = run_remote_rest(
        "example.get#Item",
        &format!("http://127.0.0.1:{port}/items/${{note}}"),
        None,
        "get",
        &parameters,
        false,
    )
    .expect("stub response");
    let body = server.join().expect("server thread");
    assert!(body.contains("GET /items/kept"), "{body}");
    assert_eq!(result.get("enumTypeId").and_then(Value::as_str), Some("DayOfWeek"));
    assert_eq!(result.get("ok").and_then(Value::as_str), Some("yes"));
}

/// A 2xx body that is not a JSON object or array fails the call, as Java's `jsonObject()`.
#[test]
#[cfg(feature = "http")]
fn a_success_that_is_not_json_fails_the_call() {
    for payload in ["plain text", ""] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = thread::spawn(move || serve_body(listener, payload));
        let err = run_remote_rest(
            "example.get#Item",
            &format!("http://127.0.0.1:{port}/items"),
            None,
            "get",
            &BTreeMap::from([("key".to_string(), Value::string("secret"))]),
            false,
        )
        .expect_err("not JSON");
        server.join().expect("server thread");
        let shown = err.to_string();
        assert!(
            shown.contains(&format!(
                "Error parsing JSON response from request to http://127.0.0.1:{port}/items"
            )),
            "{shown}"
        );
        assert!(!shown.contains("secret"), "{shown}");
    }
}

#[cfg(feature = "http")]
fn serve_one(listener: TcpListener) -> String {
    serve_body(listener, r#"{"ok":"yes","enumTypeId":"DayOfWeek"}"#)
}

#[cfg(feature = "http")]
fn serve_body(listener: TcpListener, payload: &str) -> String {
    listener.set_nonblocking(true).expect("nonblocking");
    let started = Instant::now();
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if started.elapsed() > Duration::from_secs(5) {
                    return String::from("accept timed out");
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return format!("accept failed: {err}"),
        }
    };
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let request = read_headers(&mut stream);
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(payload.as_bytes());
    let _ = stream.flush();
    request
}

#[cfg(feature = "http")]
fn read_headers(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 1024];
    while !buf.windows(4).any(|window| window == b"\r\n\r\n") && buf.len() < 65_536 {
        match stream.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(size) => buf.extend_from_slice(&tmp[..size]),
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// The `http` feature is off (the browser build): the call goes to the installed sender.
#[cfg(not(feature = "http"))]
mod installed_sender {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, MutexGuard};

    use moqui_lib::error::Error;
    use moqui_lib::object::Value;

    use super::run_remote_rest;
    use crate::outbound::{self, HttpSender, Method, Request, Response};

    /// The sender slot is process-wide, so these tests take turns.
    fn exclusive() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    struct Fake {
        seen: Mutex<Vec<Request>>,
        reply: Result<Response, String>,
    }

    impl HttpSender for Fake {
        fn send(&self, request: Request) -> Result<Response, Error> {
            self.seen.lock().unwrap().push(request);
            self.reply.clone().map_err(Error::new)
        }
    }

    fn install(reply: Result<Response, String>) -> Arc<Fake> {
        let fake = Arc::new(Fake {
            seen: Mutex::new(Vec::new()),
            reply,
        });
        outbound::install_http_sender(fake.clone());
        fake
    }

    fn ok(body: &str) -> Result<Response, String> {
        Ok(Response {
            status: 200,
            body: body.to_string(),
            ..Default::default()
        })
    }

    #[test]
    fn without_a_sender_the_call_is_not_built_into_this_host() {
        let _turn = exclusive();
        outbound::clear_http_sender();
        let err = run_remote_rest(
            "example.get#Item",
            "http://example.test/items",
            None,
            "get",
            &BTreeMap::new(),
            false,
        )
        .expect_err("no transport");
        assert!(err.to_string().contains("not built into this host"), "{err}");
    }

    #[test]
    fn a_post_goes_to_the_sender_as_json_and_the_reply_is_the_result() {
        let _turn = exclusive();
        let fake = install(ok(r#"{"ok":"yes"}"#));
        let mut parameters = BTreeMap::new();
        parameters.insert("count".to_string(), Value::I32(3));
        let result = run_remote_rest(
            "example.put#Item",
            "https://example.test/items/${count}",
            Some("PUT"),
            "put",
            &parameters,
            false,
        )
        .expect("fake reply");
        outbound::clear_http_sender();
        assert_eq!(result.get("ok").and_then(Value::as_str), Some("yes"));
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, Method::Put);
        assert_eq!(seen[0].url, "https://example.test/items/3");
        assert_eq!(seen[0].body.as_deref(), Some(r#"{"count":3}"#));
        assert!(seen[0]
            .headers
            .iter()
            .any(|(name, value)| name == "content-type" && value == "application/json"));
    }

    #[test]
    fn sim_denies_before_the_sender_and_errors_pass_through() {
        let _turn = exclusive();
        let fake = install(Err("network down".to_string()));
        let denied = run_remote_rest(
            "example.get#Item",
            "http://example.test/a?key=secret",
            None,
            "get",
            &BTreeMap::new(),
            true,
        )
        .expect_err("sim denies");
        assert!(denied.to_string().contains("sim mode is active"), "{denied}");
        assert!(!denied.to_string().contains("secret"), "{denied}");
        assert!(fake.seen.lock().unwrap().is_empty());
        let failed = run_remote_rest(
            "example.get#Item",
            "http://example.test/a",
            None,
            "get",
            &BTreeMap::new(),
            false,
        )
        .expect_err("transport error");
        assert!(failed.to_string().contains("network down"), "{failed}");
        let status = install(Ok(Response {
            status: 503,
            body: "busy".to_string(),
            ..Default::default()
        }));
        let refused = run_remote_rest(
            "example.get#Item",
            "http://example.test/a",
            None,
            "get",
            &BTreeMap::new(),
            false,
        )
        .expect_err("error status");
        assert!(
            refused
                .to_string()
                .contains("Remote service error 503: Service Unavailable"),
            "{refused}"
        );
        assert_eq!(status.seen.lock().unwrap().len(), 1);
        // In a context the error status is a message, and the call returns no results.
        let ctx = crate::context::ExecutionContextFactory::test_context();
        let entered = ctx.enter();
        let results = run_remote_rest(
            "example.get#Item",
            "http://example.test/a",
            None,
            "get",
            &BTreeMap::new(),
            false,
        )
        .unwrap();
        assert!(results.is_empty());
        assert!(ctx
            .message()
            .errors()
            .iter()
            .any(|error| error.contains("Remote service error 503")));
        drop(entered);
        outbound::clear_http_sender();
    }

    #[test]
    fn an_http_resource_reads_through_the_sender() {
        let _turn = exclusive();
        let fake = install(ok("plain body"));
        let ctx = crate::context::ExecutionContextFactory::test_context();
        let resource = ctx
            .factory()
            .resource()
            .get("http://example.test/doc.txt")
            .expect("resolves");
        let text = resource.read_text();
        outbound::clear_http_sender();
        assert_eq!(text.expect("read"), "plain body");
        assert_eq!(fake.seen.lock().unwrap()[0].method, Method::Get);
    }
}
