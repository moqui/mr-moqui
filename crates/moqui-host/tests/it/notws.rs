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

//! Notification socket and one cluster cache post, and the origin check of the socket. `cluster_https_port=0` is an ephemeral bind. When the user's last
//! socket closes, the user's offline presence is published (after the 20 second grace a reload gets).

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::notify::NotificationMessage;
use moqui_host::{load_data, start_server, LoadFlags};
use moqui_lib::object::{basic_convert, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, ClientRequestBuilder, Message, WebSocket};

const SHARED_TOPIC: &str = "Wave3Shared";
const ONLY_TOPIC: &str = "Wave3Only";

#[test]
#[ignore = "slow: waits for the production 60-second credential recheck"]
fn notification_sockets_close_after_logout_key_revocation_and_expiry() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_conf("notws-revalidate", "", &[]);
    let address = gate.server.address();
    let cookie = login(address);
    let healthy_cookie = login(address);
    let mut logged_out = open_socket(address, &cookie);
    let mut healthy = open_socket(address, &healthy_cookie);
    let factory = gate.server.factory();
    let (revoked_key, expired_key) = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let revoked = moqui_core::user::get_login_key(None).unwrap();
        let expired = moqui_core::user::get_login_key(None).unwrap();
        (revoked, expired)
    };
    let open_key = |key: &str| {
        let uri: tungstenite::http::Uri = format!("ws://{address}/notws").parse().unwrap();
        connect(ClientRequestBuilder::new(uri).with_header("login_key", key))
            .unwrap()
            .0
    };
    let mut revoked = open_key(&revoked_key);
    let mut expired = open_key(&expired_key);
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _session = moqui_core::user::bind(factory, Some(&cookie), None, false).unwrap();
        moqui_core::user::logout().unwrap();
        moqui_core::user::end_request().unwrap();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let find_key = |key: &str| {
            facade
                .find("moqui.security.UserLoginKey")
                .unwrap()
                .eq(
                    "loginKey",
                    Value::string(moqui_core::user::hash_login_key(key).unwrap()),
                )
                .one()
                .unwrap()
                .unwrap()
        };
        find_key(&revoked_key).delete(&facade).unwrap();
        let mut row = find_key(&expired_key);
        row.set("thruDate", basic_convert(&Value::I64(1), "java.sql.Timestamp").value);
        row.update(&facade).unwrap();
    }
    for socket in [&mut logged_out, &mut revoked, &mut expired] {
        set_read_timeout(socket, Duration::from_secs(70));
        loop {
            match socket.read().unwrap() {
                Message::Ping(_) => socket.flush().unwrap(),
                Message::Close(Some(frame)) => {
                    assert_eq!(u16::from(frame.code), 1008);
                    break;
                }
                _ => {}
            }
        }
    }
    healthy.send(Message::Ping(vec![1].into())).unwrap();
    set_read_timeout(&mut healthy, Duration::from_secs(5));
    loop {
        match healthy.read().unwrap() {
            Message::Ping(_) => healthy.flush().unwrap(),
            Message::Pong(bytes) if bytes.as_ref() == [1] => break,
            Message::Close(frame) => panic!("a healthy session closed: {frame:?}"),
            _ => {}
        }
    }
}

#[test]
#[ignore = "slow: cargo test-slow"]
fn notws_push_and_one_cluster_invalidation() {
    if !crate::common::own_process() {
        return;
    }
    let _serial = crate::common::serial();
    let runtime = temp_dir("moqui-notws-rt");
    let cluster_dir = temp_dir("moqui-notws-pg");
    let _runtime_guard = DirGuard(runtime.clone());
    let _cluster_guard = DirGuard(cluster_dir.clone());
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::create_dir_all(&cluster_dir).unwrap();
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let base =
        std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component")).unwrap();
    std::os::unix::fs::symlink(&base, runtime.join("base-component")).unwrap();

    load_data(&runtime, &framework, &cluster_dir, &LoadFlags::default()).unwrap();
    let demo = base.join("webroot/data/WebrootSecurityDemoData.xml");
    let demo_flags = LoadFlags {
        location: Some(demo.display().to_string()),
        data_types: Some(vec!["demo".to_string()]),
        ..LoadFlags::default()
    };
    load_data(&runtime, &framework, &cluster_dir, &demo_flags).unwrap();

    // 0 binds an ephemeral cluster port. Empty would listen on 8444. This does not touch the repo .env.
    moqui_lib::env::set_property("cluster_https_port", "0");
    let server = start_server(
        &runtime,
        &framework,
        &cluster_dir,
        SocketAddr::from(([127, 0, 0, 1], 0)),
    )
    .unwrap();
    let http = server.address();
    assert_ne!(
        server.cluster_address().port(),
        0,
        "ephemeral cluster port was not recorded"
    );

    // Two logins, two sessions. A second socket on the first session (another tab) gets the notifications too.
    let cookie_all = login(http);
    let cookie_other = login(http);
    assert_ne!(cookie_all, cookie_other, "each login mints its own session");
    server.factory().flush_sessions().unwrap();

    assert_upgrade_rejected(http, None);
    assert_upgrade_rejected(http, Some("not-a-session"));

    let mut all = open_socket(http, &cookie_all);
    let mut other = open_socket(http, &cookie_other);
    all.send(Message::Text("subscribe:ALL".into())).unwrap();
    other
        .send(Message::Text(format!("subscribe:{SHARED_TOPIC}").into()))
        .unwrap();

    let factory = Arc::clone(server.factory());
    let user_id = user_id_of(&factory);
    prove_both_subscribed(&factory, &user_id, &mut all, &mut other);
    drain(&mut all);
    drain(&mut other);
    let mut extra = open_socket(http, &cookie_all);
    extra.send(Message::Text("subscribe:ALL".into())).unwrap();
    // the subscription of the extra socket arrives with the next frame the server reads, so send until it is in
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut saw_extra = false;
    while Instant::now() < deadline && !saw_extra {
        send_note(&factory, &user_id, ONLY_TOPIC);
        saw_extra = read_for(&mut extra, ONLY_TOPIC, Duration::from_millis(250));
    }
    assert!(saw_extra, "the second socket of the session got nothing");
    let frame = read_until(&mut all, ONLY_TOPIC, Duration::from_secs(3));
    // The client gets Java's wrapped message: who else it went to is not in it.
    assert!(!frame.contains("userIds") && !frame.contains(&user_id), "{frame}");
    assert_absent(&mut other, ONLY_TOPIC, Duration::from_millis(400));
    // the user is online; closing every socket of theirs publishes offline once the grace is over
    let presence = Arc::new(Mutex::new(Vec::new()));
    let _presence_tap = tap_presence(&user_id, Arc::clone(&presence));
    set_online(&factory, &user_id);
    drop(all);
    drop(other);
    drop(extra);
    let closed = Instant::now();

    let (session_fp, cert_fp) = member_fingerprints(&factory);
    let port = server.cluster_address().port();
    insert_peer(&factory, &session_fp, &cert_fp, port);
    // A member that joins is seen on the next heartbeat tick, which reads the member list again.
    factory.service_pools().tick();
    let before = server.cluster_posts().len();
    create_test_entity(&factory);
    // The post goes from the cluster sender thread after commit, not from the committing thread.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && server.cluster_posts().len() == before {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    let posts = server.cluster_posts();
    let added = &posts[before..];
    // The tick that found the peer also wrote rows of its own, which go out as invalidations too; each entity is
    // posted once (see the cluster tests), and nothing but invalidations was posted.
    assert!(
        !added.is_empty() && added.iter().all(|topic| topic == "entity-cache-invalidate"),
        "posts {posts:?} before {before}"
    );

    let offline = || presence.lock().unwrap().iter().any(|presence| presence == "offline");
    let deadline = closed + Duration::from_secs(40);
    while Instant::now() < deadline && !offline() {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        offline(),
        "no offline presence after the last socket closed: {:?}",
        presence.lock().unwrap()
    );
    assert!(
        closed.elapsed() >= Duration::from_secs(19),
        "offline came before the grace a reload gets"
    );
}

/// The origin of `/notws` and of CORS is the whole origin, scheme, host and port (Java compares the host only): the
/// same host on another port or scheme is refused, and a believed proxy's forwarded proto, host and port make the
/// request's own origin for both checks alike. The test client on loopback is the trusted proxy here.
#[test]
fn notws_and_cors_compare_the_scheme_host_and_port_of_the_origin() {
    if !crate::common::own_process() {
        return;
    }
    const CONF: &str = r#"<webapp-list><webapp name="webroot" client-ip-header="X-Forwarded-For"
        trusted-proxies="127.0.0.1"/></webapp-list>"#;
    let gate = crate::common::start_with_conf("notws-origin", CONF, &[]);
    let address = gate.server.address();
    let cookie = crate::common::sign_in(address, "john.doe", "moqui");
    gate.server.factory().flush_sessions().unwrap();
    let upgrade = |headers: &[(&str, &str)]| {
        let uri: tungstenite::http::Uri = format!("ws://{address}/notws").parse().unwrap();
        let mut request = ClientRequestBuilder::new(uri).with_header("Cookie", cookie.clone());
        for (name, value) in headers {
            request = request.with_header(*name, *value);
        }
        match connect(request) {
            Ok((socket, response)) => {
                drop(socket);
                response.status().as_u16()
            }
            Err(tungstenite::Error::Http(response)) => response.status().as_u16(),
            Err(err) => panic!("the upgrade failed: {err}"),
        }
    };
    // the request's Host is the address the browser used (common::http_request sends localhost)
    let cors = |headers: &[(&str, &str)]| {
        let mut request = format!("GET /Login HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        let (status, headers, _) = crate::common::exchange(address, &request);
        (status, crate::common::header(&headers, "access-control-allow-origin"))
    };
    let own = format!("http://{address}");
    let other_port = format!(
        "http://127.0.0.1:{}",
        if address.port() == 1 { 2 } else { address.port() - 1 }
    );
    let other_scheme = format!("https://{address}");
    assert_eq!(upgrade(&[("Origin", &own)]), 101, "the server's own origin");
    assert_eq!(cors(&[("Origin", &own)]), (200, Some(own.clone())));
    for other in [&other_port, &other_scheme] {
        assert_eq!(upgrade(&[("Origin", other)]), 403, "{other}");
        assert_eq!(cors(&[("Origin", other)]), (401, None), "{other}");
    }
    // behind the believed proxy: its forwarded proto, host and port are the origin, for the socket and CORS alike
    let proxied = |origin: &'static str| {
        [
            ("Origin", origin),
            ("X-Forwarded-Proto", "https"),
            ("X-Forwarded-Host", "shop.example"),
            ("X-Forwarded-Port", "8443"),
        ]
    };
    assert_eq!(upgrade(&proxied("https://shop.example:8443")), 101);
    assert_eq!(
        cors(&proxied("https://shop.example:8443")),
        (200, Some("https://shop.example:8443".to_string()))
    );
    for other in ["https://shop.example", "http://shop.example:8443"] {
        assert_eq!(upgrade(&proxied(other)), 403, "{other}");
        assert_eq!(cors(&proxied(other)), (401, None), "{other}");
    }
}

/// The presence the userStatus pushes to `user_id` carry, kept while the subscription lives.
fn tap_presence(user_id: &str, seen: Arc<Mutex<Vec<String>>>) -> moqui_core::notify::Subscription {
    let user_id = user_id.to_string();
    moqui_core::notify::subscribe(
        moqui_core::notify::NOTIFICATION_MESSAGE,
        Arc::new(move |message: &moqui_core::notify::TopicMessage| {
            let routed = message.body[moqui_core::notify::ROUTE_USER_IDS]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(user_id.as_str())));
            if routed && message.body["topic"].as_str() == Some("userStatus") {
                if let Some(presence) = message.body["message"]["presence"].as_str() {
                    seen.lock().unwrap().push(presence.to_string());
                }
            }
        }),
    )
    .unwrap()
}

/// The user's chat presence is online: seen and active now, and published as online.
fn set_online(factory: &Arc<ExecutionContextFactory>, user_id: &str) {
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let facade = factory.entity().unwrap();
    let millis = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(0);
    let now = basic_convert(&Value::I64(millis), "java.sql.Timestamp").value;
    let mut row = facade.make_value("moqui.security.UserStatus").unwrap();
    row.set("userId", Value::string(user_id));
    row.set("statusEnumId", Value::string("UpsOnline"));
    row.set("lastSeenDate", now.clone());
    row.set("lastActiveDate", now);
    row.set("lastPresence", Value::string("online"));
    row.create(&facade).unwrap();
    drop(entered);
}

fn prove_both_subscribed(factory: &Arc<ExecutionContextFactory>, user_id: &str, all: &mut Ws, other: &mut Ws) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut saw_all = false;
    let mut saw_other = false;
    while Instant::now() < deadline && !(saw_all && saw_other) {
        send_note(factory, user_id, SHARED_TOPIC);
        if !saw_all {
            saw_all = read_for(all, SHARED_TOPIC, Duration::from_millis(250));
        }
        if !saw_other {
            saw_other = read_for(other, SHARED_TOPIC, Duration::from_millis(250));
        }
    }
    assert!(saw_all && saw_other, "all={saw_all} other={saw_other}");
}

fn read_until(socket: &mut Ws, needle: &str, total: Duration) -> String {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if let Some(text) = next_text(socket, Duration::from_millis(200)) {
            if text.contains(needle) {
                return text;
            }
        }
    }
    panic!("missing {needle}");
}

fn read_for(socket: &mut Ws, needle: &str, timeout: Duration) -> bool {
    next_text(socket, timeout).is_some_and(|text| text.contains(needle))
}

fn assert_absent(socket: &mut Ws, needle: &str, total: Duration) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if let Some(text) = next_text(socket, Duration::from_millis(150)) {
            assert!(!text.contains(needle), "{text}");
        }
    }
}

fn drain(socket: &mut Ws) {
    while next_text(socket, Duration::from_millis(100)).is_some() {}
}

fn send_note(factory: &Arc<ExecutionContextFactory>, user_id: &str, topic: &str) {
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let mut note = NotificationMessage::new();
    note.topic(topic)
        .user_id(user_id)
        .persist(false)
        .message_json("{\"n\":1}")
        .send()
        .unwrap();
    drop(entered);
}

fn login(address: SocketAddr) -> String {
    let form = "username=john.doe&password=moqui";
    let (status, headers, body) = exchange(
        address,
        &http_request(
            "POST",
            "/rest/login",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            form,
        ),
    );
    assert_eq!(status, 200, "{body}");
    session_cookie(&headers)
}

fn user_id_of(factory: &Arc<ExecutionContextFactory>) -> String {
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let facade = factory.entity().unwrap();
    let row = facade
        .find("moqui.security.UserAccount")
        .unwrap()
        .eq("username", Value::string("john.doe"))
        .one()
        .unwrap()
        .unwrap();
    let user_id = row.get("userId").and_then(Value::as_str).unwrap().to_string();
    drop(entered);
    user_id
}

fn member_fingerprints(factory: &Arc<ExecutionContextFactory>) -> (String, String) {
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let facade = factory.entity().unwrap();
    let members = facade.find("moqui.server.ClusterMember").unwrap().list().unwrap();
    let row = members.values().first().expect("cluster member");
    let session_fp = field_text(row, "sessionKeyFingerprint");
    let cert_fp = field_text(row, "certFingerprint");
    drop(entered);
    assert!(!session_fp.is_empty());
    assert!(!cert_fp.is_empty());
    (session_fp, cert_fp)
}

fn insert_peer(factory: &Arc<ExecutionContextFactory>, session_fp: &str, cert_fp: &str, port: u16) {
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let facade = factory.entity().unwrap();
    let millis = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(0);
    let heartbeat = basic_convert(&Value::I64(millis), "java.sql.Timestamp").value;
    let mut row = facade.make_value("moqui.server.ClusterMember").unwrap();
    row.set("instanceId", Value::string("wave3-peer"));
    row.set("clusterBaseUrl", Value::string(format!("https://127.0.0.1:{port}")));
    row.set("heartbeat", heartbeat);
    row.set("sessionKeyFingerprint", Value::string(session_fp));
    row.set("certFingerprint", Value::string(cert_fp));
    facade.begin("moqui.server.ClusterMember", None).unwrap();
    row.create(&facade).unwrap();
    facade.commit("moqui.server.ClusterMember").unwrap();
    drop(entered);
}

fn create_test_entity(factory: &Arc<ExecutionContextFactory>) {
    let ctx = factory.new_context();
    let entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let facade = factory.entity().unwrap();
    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string("WAVE3"));
    row.set("testMedium", Value::string("cluster"));
    facade.begin("moqui.test.TestEntity", None).unwrap();
    row.create(&facade).unwrap();
    facade.commit("moqui.test.TestEntity").unwrap();
    drop(entered);
}

fn field_text(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name).and_then(Value::as_str).unwrap_or("").to_string()
}

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

fn open_socket(address: SocketAddr, cookie: &str) -> Ws {
    let uri: tungstenite::http::Uri = format!("ws://{address}/notws").parse().unwrap();
    let request = ClientRequestBuilder::new(uri).with_header("Cookie", format!("moqui.session={cookie}"));
    let (socket, response) = connect(request).unwrap();
    assert_eq!(response.status().as_u16(), 101);
    socket
}

fn assert_upgrade_rejected(address: SocketAddr, cookie: Option<&str>) {
    let uri: tungstenite::http::Uri = format!("ws://{address}/notws").parse().unwrap();
    let mut request = ClientRequestBuilder::new(uri);
    if let Some(cookie) = cookie {
        request = request.with_header("Cookie", format!("moqui.session={cookie}"));
    }
    match connect(request) {
        Err(tungstenite::Error::Http(response)) => {
            let status = response.status().as_u16();
            assert_ne!(status, 101);
            assert_eq!(status, 401, "{status}");
        }
        Ok(_) => panic!("websocket upgraded without a valid session"),
        Err(err) => panic!("expected HTTP 401, got {err}"),
    }
}

fn next_text(socket: &mut Ws, timeout: Duration) -> Option<String> {
    set_read_timeout(socket, timeout);
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => return Some(text.to_string()),
            Ok(Message::Ping(_) | Message::Pong(_)) => continue,
            Ok(Message::Close(_)) => return None,
            Err(tungstenite::Error::Io(err)) if timed_out(&err) => return None,
            Err(tungstenite::Error::ConnectionClosed) => return None,
            Err(err) => panic!("websocket read: {err}"),
            Ok(other) => panic!("unexpected websocket frame {other:?}"),
        }
    }
}

fn set_read_timeout(socket: &mut Ws, timeout: Duration) {
    match socket.get_mut() {
        MaybeTlsStream::Plain(tcp) => tcp.set_read_timeout(Some(timeout)).unwrap(),
        _ => panic!("notification socket is not a plain tcp stream"),
    }
}

fn timed_out(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::TimedOut || err.kind() == std::io::ErrorKind::WouldBlock
}

fn http_request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (name, value) in headers {
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    }
    if !body.is_empty() || method.eq_ignore_ascii_case("POST") {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("\r\n");
    out.push_str(body);
    out
}

fn exchange(address: SocketAddr, request: &str) -> (u16, Vec<(String, String)>, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 8192];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(count) => buf.extend_from_slice(&tmp[..count]),
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut || err.kind() == std::io::ErrorKind::WouldBlock => {
                if response_complete(&buf) {
                    break;
                }
                panic!("timed out after {} bytes: {}", buf.len(), String::from_utf8_lossy(&buf));
            }
            Err(err) => panic!("read failed after {} bytes: {err}", buf.len()),
        }
        if response_complete(&buf) {
            break;
        }
    }
    parse_response(&buf)
}

fn response_complete(buf: &[u8]) -> bool {
    let Some(split) = find_header_end(buf) else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..split]);
    let Some(length) = content_length(&head) else {
        return false;
    };
    buf.len() >= split + 4 + length
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(head: &str) -> Option<usize> {
    for line in head.split("\r\n").skip(1) {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse().ok();
        }
    }
    None
}

fn parse_response(buf: &[u8]) -> (u16, Vec<(String, String)>, String) {
    let text = String::from_utf8_lossy(buf);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_ref(), ""));
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    (status, headers, body.to_string())
}

fn session_cookie(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .find_map(|(name, value)| {
            if !name.eq_ignore_ascii_case("set-cookie") {
                return None;
            }
            let pair = value.split(';').next().unwrap_or("").trim();
            let (key, cookie) = pair.split_once('=')?;
            if key.trim() == "moqui.session" {
                Some(cookie.trim().to_string())
            } else {
                None
            }
        })
        .expect("Set-Cookie moqui.session")
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("{prefix}-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
