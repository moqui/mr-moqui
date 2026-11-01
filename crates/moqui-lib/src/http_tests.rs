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
fn sim_true_and_allow_false_denies_and_does_not_produce_a_send() {
    assert!(!sim_allows_http(true, false));
    let client = RestClient::new();
    let error = client
        .post("http://example.test/denied")
        .json(&serde_json::json!({"name": "nope"}))
        .prepare(true)
        .expect_err("sim denies");
    assert!(error.message().contains("denied because sim mode is active"), "{error}");
    assert!(error.message().contains("http://example.test/denied"));
    assert!(client.transport.get().is_none());
}

#[test]
fn redaction_drops_user_password_and_query() {
    assert_eq!(
        redacted("https://user:secret@example.test:8443/a/b?token=1#x"),
        "https://example.test:8443/a/b"
    );
    assert_eq!(redacted("not a url"), "(invalid URL)");
}

#[test]
fn only_http_and_https_are_sent_and_methods_parse() {
    let client = RestClient::new();
    let err = client.get("file:///etc/passwd").prepare(false).expect_err("scheme");
    assert!(err.message().contains("http or https"), "{err}");
    assert_eq!(Method::parse("put"), Some(Method::Put));
    assert_eq!(Method::parse("TRACE"), None);
    let delete = client.delete("http://example.test/x").prepare(false).unwrap();
    assert_eq!(delete.method(), Method::Delete);
}

#[test]
fn internal_addresses_are_recognized() {
    for text in [
        "127.0.0.1",
        "10.1.2.3",
        "192.168.0.9",
        "169.254.169.254",
        "0.0.0.0",
        "::1",
        "fd00::1",
        "fe80::1",
        "0.1.2.3",
        "100.64.0.1",
        "100.127.255.254",
        "::ffff:127.0.0.1",
        "::ffff:169.254.169.254",
        "::10.0.0.1",
        "64:ff9b::7f00:1",
        "64:ff9b::a9fe:a9fe",
        "64:ff9b:1::808:808",
        "224.0.0.1",
        "239.255.255.250",
        "240.0.0.1",
        "255.255.255.255",
        "ff02::1",
        "::ffff:224.0.0.251",
    ] {
        assert!(internal_address(text.parse().unwrap()), "{text}");
    }
    for text in [
        "93.184.216.34",
        "100.128.0.1",
        "100.63.255.255",
        "::ffff:93.184.216.34",
        "64:ff9b::808:808",
        "2606:4700::1111",
    ] {
        assert!(!internal_address(text.parse().unwrap()), "{text}");
    }
    let client = RestClient::new().deny_internal_addresses(true);
    for url in [
        "http://127.0.0.1:9/x",
        "http://0x7f.1:9/x",
        "http://[::ffff:7f00:1]:9/x",
        "http://[64:ff9b::a9fe:a9fe]/latest",
    ] {
        let request = client.get(url).prepare(false).unwrap();
        let err = client
            .execute(request)
            .expect_err("an internal literal is denied before a socket");
        assert!(err.message().contains("internal address"), "{url}: {err}");
    }
}

#[test]
fn an_internal_literal_is_found_in_a_url_and_a_name_is_left_to_the_resolver() {
    for url in [
        "http://127.0.0.1/x",
        "http://0x7f.1/x",
        "https://[::1]:8443/",
        "http://169.254.169.254/latest",
        "http://224.0.0.1/",
        "http://[ff02::1]/",
    ] {
        assert!(internal_literal(&reqwest::Url::parse(url).unwrap()), "{url}");
    }
    for url in [
        "http://93.184.216.34/",
        "https://example.com/",
        "http://localhost/",
        "https://[2606:4700::1111]/",
    ] {
        assert!(!internal_literal(&reqwest::Url::parse(url).unwrap()), "{url}");
    }
}

/// A server on a loopback port answering each connection with the next of `responses`, the whole response as
/// written. Its port.
fn serve(responses: Vec<String>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for (stream, response) in listener.incoming().zip(responses) {
            let Ok(mut stream) = stream else { return };
            let mut buffer = [0_u8; 4096];
            let _ = std::io::Read::read(&mut stream, &mut buffer);
            let _ = std::io::Write::write_all(&mut stream, response.replace("{port}", &port.to_string()).as_bytes());
        }
    });
    port
}

#[test]
fn a_large_body_is_an_error_or_its_start_when_truncated() {
    let body = "x".repeat(2000);
    let ok = |body: &str| {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    let port = serve(vec![ok(&body), ok(&body)]);
    let url = format!("http://127.0.0.1:{port}/page");
    let strict = RestClient::new().max_response_bytes(1000);
    let err = strict.get(&url).send(false).expect_err("over the limit");
    assert!(err.message().contains("larger than the limit"), "{err}");
    let truncating = RestClient::new().max_response_bytes(1000).truncate_body(true);
    let response = truncating.get(&url).send(false).expect("its start");
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().len(), 1000);
}

#[test]
fn a_call_can_bound_its_own_response_below_the_clients() {
    let body = "x".repeat(2000);
    let ok = |body: &str| {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    let port = serve(vec![ok(&body), ok(&body), ok(&body)]);
    let url = format!("http://127.0.0.1:{port}/file");
    let client = RestClient::new().max_response_bytes(10_000);
    let err = client
        .get(&url)
        .max_response_bytes(1000)
        .send(false)
        .expect_err("over this call's bound");
    assert!(err.message().contains("larger than the limit"), "{err}");
    assert_eq!(
        client.get(&url).send(false).expect("the client's bound").bytes().len(),
        2000
    );
    // a call cannot raise the client's bound
    let err = RestClient::new()
        .max_response_bytes(1000)
        .get(&url)
        .max_response_bytes(5000)
        .send(false)
        .expect_err("over the client's bound");
    assert!(err.message().contains("larger than the limit"), "{err}");
}

#[test]
fn a_redirect_to_another_host_is_followed_only_when_asked() {
    // 127.0.0.1 and localhost are two hosts to the redirect policy
    let moved = "HTTP/1.1 302 Found\r\nlocation: http://localhost:{port}/next\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
    let ok = "HTTP/1.1 200 OK\r\ncontent-length: 4\r\nconnection: close\r\n\r\nhere";
    let port = serve(vec![moved.to_string(), moved.to_string(), ok.to_string()]);
    let url = format!("http://127.0.0.1:{port}/first");
    let same_host = RestClient::new().get(&url).send(false).expect("the 3xx comes back");
    assert_eq!(same_host.status(), 302);
    let any_host = RestClient::new()
        .redirects(3, true)
        .get(&url)
        .send(false)
        .expect("followed");
    assert_eq!(any_host.status(), 200);
    assert_eq!(any_host.body(), "here");
}

/// The whole chain of `err`, for an error that comes from inside reqwest.
fn chain(err: &Error) -> String {
    let mut text = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(next) = source {
        text.push_str(": ");
        text.push_str(&next.to_string());
        source = next.source();
    }
    text
}

#[test]
fn a_name_that_resolves_internal_is_refused_by_the_resolver() {
    // `localhost` is a name, so only the resolver sees that it is loopback; nothing listens on port 9.
    let client = RestClient::new().deny_internal_addresses(true);
    let err = client.get("http://localhost:9/x").send(false).expect_err("denied");
    assert!(
        chain(&err).contains("localhost is an internal address"),
        "{}",
        chain(&err)
    );
    assert!(resolve_public("localhost").is_err());
    // Off, the same name is resolved and connected to, and only the connect fails.
    let open = RestClient::new()
        .get("http://localhost:9/x")
        .send(false)
        .expect_err("nothing listens");
    assert!(!chain(&open).contains("internal address"), "{}", chain(&open));
}

#[test]
fn sim_true_and_allow_true_prepares_a_request() {
    assert!(sim_allows_http(true, true));
    let client = RestClient::new();
    let request = client
        .get("http://example.test/allowed")
        .allow_in_sim(true)
        .prepare(true)
        .expect("allow in sim");
    assert_eq!(request.method(), Method::Get);
    assert_eq!(request.url(), "http://example.test/allowed");
    assert!(client.transport.get().is_none());
}

#[test]
fn sim_false_allows() {
    assert!(sim_allows_http(false, false));
    let client = RestClient::new();
    let request = client.get("http://example.test/live").prepare(false).expect("not sim");
    assert_eq!(request.method(), Method::Get);
    assert_eq!(request.url(), "http://example.test/live");
    assert!(request.body().is_none());
    assert!(client.transport.get().is_none());
}

#[test]
fn get_and_post_json_show_method_url_and_body() {
    let client = RestClient::new();
    let get = client
        .get("http://example.test/items")
        .header("accept", "application/json")
        .prepare(false)
        .expect("get");
    assert_eq!(get.method(), Method::Get);
    assert_eq!(get.url(), "http://example.test/items");
    assert_eq!(get.body(), None);
    assert_eq!(get.headers(), [("accept".to_string(), "application/json".to_string())]);

    let post = client
        .post("http://example.test/items")
        .header("x-request-id", "one")
        .json(&serde_json::json!({"name": "widget"}))
        .prepare(false)
        .expect("post");
    assert_eq!(post.method(), Method::Post);
    assert_eq!(post.url(), "http://example.test/items");
    assert_eq!(post.body(), Some(r#"{"name":"widget"}"#));
    assert_eq!(
        post.headers(),
        [
            ("x-request-id".to_string(), "one".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ]
    );
    assert!(client.transport.get().is_none());
}
