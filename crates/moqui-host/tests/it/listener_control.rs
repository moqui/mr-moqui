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

//! The web listeners switched while the server runs (plans/Runtime-Config.md §3.3, §8.3, RC2): HTTP turned off
//! closes its `/notws` sockets at once and its requests in flight after the 5 s grace, while HTTPS goes on serving;
//! turned on again it accepts; a port change binds the new port while the old one's connections finish; a busy port
//! keeps the old and says so; "end connections now" ends them at once; and a busy HTTP port at boot is a Failed row
//! while the server answers on HTTPS. Each change is set as `System › Conf` or a service sets it (`set_conf_value`,
//! whose `listeners` hook asks the registry for a pass), and a test that needs the outcome asks for its own pass.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::listeners::{Action, Cause, Outcome, ReconcileRequest};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{ClientRequestBuilder, Message, WebSocket};

type Secure = WebSocket<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>;

const HTTP_ENABLED: &str = "webapp-list/webapp[webroot]@http-listen-enabled";
const HTTP_PORT: &str = "webapp-list/webapp[webroot]@http-listen-port";

/// A gate with HTTPS on beside HTTP: a dev instance makes its certificate.
fn with_https(label: &str) -> crate::common::Gate {
    crate::common::start_with_properties(
        label,
        None,
        &[
            ("instance@purpose", "dev"),
            ("webapp-list/webapp[webroot]@https-listen-port", "0"),
            ("webapp-list/webapp[webroot]@https-listen-enabled", "true"),
        ],
    )
}

/// A login key of john.doe, which `/notws` takes in place of a session over HTTP and HTTPS alike.
fn login_key(factory: &Arc<ExecutionContextFactory>) -> String {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    moqui_core::user::get_login_key(None).unwrap()
}

fn request(address: SocketAddr, key: &str) -> ClientRequestBuilder {
    let uri: tungstenite::http::Uri = format!("ws://localhost:{}/notws", address.port()).parse().unwrap();
    ClientRequestBuilder::new(uri).with_header("login_key", key)
}

/// A `/notws` socket over the HTTP listener at `address`, reads timing out after a second.
fn open_plain(address: SocketAddr, key: &str) -> WebSocket<MaybeTlsStream<TcpStream>> {
    let tcp = TcpStream::connect(address).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let (socket, response) = tungstenite::client(request(address, key), MaybeTlsStream::Plain(tcp)).unwrap();
    assert_eq!(response.status().as_u16(), 101);
    socket
}

/// A `/notws` socket over the HTTPS listener at `address`, taking its dev certificate.
fn open_secure(address: SocketAddr, key: &str) -> Secure {
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(crate::acme::AnyCertificate))
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let connection = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
    let tcp = TcpStream::connect(address).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let (socket, response) =
        tungstenite::client(request(address, key), rustls::StreamOwned::new(connection, tcp)).unwrap();
    assert_eq!(response.status().as_u16(), 101);
    socket
}

/// The socket is open: a ping is answered.
fn answers_ping<S: Read + Write>(socket: &mut WebSocket<S>) {
    socket.send(Message::Ping(vec![7].into())).unwrap();
    loop {
        match socket.read().unwrap() {
            Message::Pong(bytes) if bytes.as_ref() == [7] => return,
            Message::Close(frame) => panic!("the socket closed: {frame:?}"),
            _ => {}
        }
    }
}

/// The socket closes within `limit`: a close frame, its end, or a reset.
fn closes(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>, limit: Duration) {
    let deadline = Instant::now() + limit;
    loop {
        match socket.read() {
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(err))
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                assert!(Instant::now() < deadline, "the socket stayed open past {limit:?}");
            }
            Err(_) => return,
        }
    }
}

/// A request in flight on the HTTP listener at `address`: a form post whose body never comes in whole, which the
/// server waits for (its body timeout is a minute).
fn in_flight(address: SocketAddr) -> TcpStream {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .write_all(
            b"POST /rest/login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\n\
              Content-Length: 1000\r\n\r\nusername=",
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    stream
}

/// Whether `stream`'s request is still waiting: nothing answered, and not closed.
fn still_waiting(stream: &mut TcpStream) -> bool {
    stream.set_nonblocking(true).unwrap();
    let mut byte = [0_u8; 1];
    let waiting = matches!(stream.read(&mut byte), Err(err) if err.kind() == std::io::ErrorKind::WouldBlock);
    stream.set_nonblocking(false).unwrap();
    waiting
}

/// Whether `stream` is closed (its end, or a reset) within `limit`.
fn closed_within(stream: &mut TcpStream, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let mut buf = [0_u8; 512];
    while Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return true,
        }
    }
    false
}

/// Whether the HTTP listener at `address` answers `/health` with 200.
fn answers(address: SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_secs(1)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    if stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut head = [0_u8; 12];
    stream.read_exact(&mut head).is_ok() && head.starts_with(b"HTTP/1.1 200")
}

fn wait_until(limit: Duration, what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < limit, "{what} within {limit:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Set `selector` for this process's server, as a spec's `setConf` does: its `listeners` hook asks for a pass.
fn set(factory: &ExecutionContextFactory, selector: &str, value: &str) {
    factory.set_conf_value(selector, Some(value), false).unwrap();
}

/// Set `selector` with the `listeners` hook held, as the listener services do, so the pass that applies it is the
/// caller's own ([`reconcile`]).
fn set_held(factory: &ExecutionContextFactory, selector: &str, value: &str) {
    moqui_core::context::conf_reload::set_many_holding(
        factory,
        &[(selector, Some(value))],
        false,
        moqui_core::context::conf_schema::Group::Listeners,
    )
    .unwrap();
}

/// A pass for `cause`, ending `end_now`'s connections at once, and HTTP's outcome.
fn reconcile_for(factory: &ExecutionContextFactory, cause: Cause, end_now: &[&str]) -> Outcome {
    let control = moqui_core::listeners::control(factory).expect("the server's listener control");
    let request = ReconcileRequest {
        cause,
        end_now: end_now.iter().map(ToString::to_string).collect(),
    };
    let outcomes = control.reconcile(request, Duration::from_secs(10)).unwrap();
    outcomes
        .into_iter()
        .find(|outcome| outcome.name == "HTTP")
        .expect("HTTP's outcome")
}

/// A pass asked for by a person, ending `end_now`'s connections at once, and HTTP's outcome.
fn reconcile(factory: &ExecutionContextFactory, end_now: &[&str]) -> Outcome {
    let cause = Cause::Request {
        user: "listener_control".to_string(),
        reason: "the test".to_string(),
        selectors: Vec::new(),
    };
    reconcile_for(factory, cause, end_now)
}

/// HTTP turned off closes its `/notws` socket at once, which proves the socket ends on its own listener's closing and
/// not on the process-wide stop, while a request in flight keeps the 5 s grace and HTTPS keeps serving; turned on
/// again, HTTP accepts.
#[test]
fn http_turned_off_closes_its_sockets_at_once_and_on_again_accepts() {
    if !crate::common::own_process() {
        return;
    }
    let gate = with_https("listener-off-on");
    let factory = gate.server.factory();
    let http = gate.server.address();
    let https = gate
        .server
        .https_address()
        .expect("HTTPS serves with the dev certificate");
    let key = login_key(factory);
    let mut plain = open_plain(http, &key);
    let mut secure = open_secure(https, &key);
    answers_ping(&mut plain);
    answers_ping(&mut secure);
    let mut waiting = in_flight(http);

    let started = Instant::now();
    set(factory, HTTP_ENABLED, "false");
    closes(&mut plain, Duration::from_secs(3));
    let closed_in = started.elapsed();
    assert!(
        closed_in < Duration::from_secs(3),
        "the socket closed {closed_in:?} after the switch, not in the stop's 5 s grace"
    );
    assert!(still_waiting(&mut waiting), "a request in flight has the grace");
    wait_until(Duration::from_secs(2), "the HTTP port let go", || {
        TcpStream::connect(http).is_err()
    });
    answers_ping(&mut secure);
    answers_ping(&mut open_secure(https, &key));
    // the client goes, so the drain need not wait out the grace
    drop(waiting);

    set(factory, HTTP_ENABLED, "true");
    wait_until(Duration::from_secs(10), "HTTP accepting again", || {
        gate.server.listener_address("HTTP").is_some_and(answers)
    });
}

/// A new port binds while the old one's connections finish, and becomes the server's address; a port another process
/// holds keeps the listener where it is, reported as Failed "kept on" with its last error.
#[test]
fn a_port_change_binds_the_new_port_and_a_busy_one_keeps_the_old() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start("listener-port");
    let factory = gate.server.factory();
    let old = gate.server.address();
    let mut waiting = in_flight(old);

    // the hook's pass binds the new port, then gives the old one's connections their grace, which it waits for: the
    // new port answers while the old one's request is still in flight
    let port = crate::common::free_port();
    let new = SocketAddr::from(([127, 0, 0, 1], port));
    set(factory, HTTP_PORT, &port.to_string());
    wait_until(Duration::from_secs(3), "the new port answering", || answers(new));
    assert!(
        still_waiting(&mut waiting),
        "the old port's request finishes in its grace"
    );
    assert!(TcpStream::connect(old).is_err(), "the old port is let go");
    assert_eq!(gate.server.address(), new);
    drop(waiting);

    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = held.local_addr().unwrap().port();
    set(factory, HTTP_PORT, &busy.to_string());
    let outcome = reconcile(factory, &[]);
    assert_eq!(outcome.action, Action::Failed, "{}", outcome.detail);
    assert!(
        outcome.detail.starts_with(&format!("kept on {new}")),
        "{}",
        outcome.detail
    );
    assert_eq!(gate.server.address(), new);
    assert!(answers(new));
    let info = moqui_core::listeners::control(factory)
        .unwrap()
        .listeners()
        .into_iter()
        .find(|info| info.name == "HTTP")
        .unwrap();
    assert!(
        info.last_error
            .as_deref()
            .is_some_and(|err| err.contains(&busy.to_string())),
        "{:?}",
        info.last_error
    );
}

/// "End connections now" ends HTTP's requests in flight and its sockets at once, with no grace: the setting applied
/// with the `listeners` hook held, then one pass carrying it and `end_now`, as the listener services will. The server
/// started on an address with HTTP off in its conf, which the address held on: the switch set to that same value
/// turns it off all the same.
#[test]
fn ending_connections_now_closes_them_at_once() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_conf(
        "listener-end-now",
        r#"<webapp-list><webapp name="webroot" http-listen-enabled="false"/></webapp-list>"#,
        &[],
    );
    let factory = gate.server.factory();
    let http = gate.server.address();
    assert!(
        answers(http),
        "HTTP on the address it was started on, whatever the conf said"
    );
    let key = login_key(factory);
    let mut plain = open_plain(http, &key);
    answers_ping(&mut plain);
    let mut waiting = in_flight(http);

    let started = Instant::now();
    set_held(factory, HTTP_ENABLED, "false");
    let outcome = reconcile_for(factory, Cause::SetValue(HTTP_ENABLED.to_string()), &["HTTP"]);
    assert_eq!(outcome.action, Action::Stopped, "{}", outcome.detail);
    assert!(
        closed_within(&mut waiting, Duration::from_secs(1)),
        "the request in flight ended at once"
    );
    closes(&mut plain, Duration::from_secs(1));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert!(TcpStream::connect(http).is_err(), "HTTP is off");
}

/// An operator who turned HTTP off with its grace may still end a request that does not finish: "end connections now"
/// acts on the stop already draining, at once.
#[test]
fn ending_now_cuts_short_a_grace_already_running() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start("listener-end-later");
    let factory = gate.server.factory();
    let http = gate.server.address();
    let mut waiting = in_flight(http);
    set(factory, HTTP_ENABLED, "false");
    wait_until(Duration::from_secs(3), "the HTTP port let go", || {
        TcpStream::connect(http).is_err()
    });
    assert!(still_waiting(&mut waiting), "the request in flight has its grace");

    let started = Instant::now();
    reconcile(factory, &["HTTP"]);
    assert!(
        closed_within(&mut waiting, Duration::from_secs(1)),
        "the request in flight ended at once"
    );
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
}

/// The properties of a seeded runtime whose HTTP port is `busy`, held by another, on a dev instance whose HTTPS serves.
fn busy_http_properties(busy: &str) {
    for (name, value) in [
        ("instance@purpose", "dev"),
        ("webapp-list/webapp[webroot]@http-listen-enabled", "true"),
        ("webapp-list/webapp[webroot]@http-listen-port", busy),
        ("webapp-list/webapp[webroot]@http-bind", "127.0.0.1"),
        ("webapp-list/webapp[webroot]@https-listen-port", "0"),
        ("webapp-list/webapp[webroot]@https-listen-enabled", "true"),
        ("cluster-member@https-port", "0"),
    ] {
        moqui_lib::env::set_property(&crate::common::property_name(name), value.to_string());
    }
}

/// A busy HTTP port at boot no longer stops a server whose listeners come from its conf (plans/Runtime-Config.md §6):
/// HTTP's row is Failed, naming the port, and the server answers on HTTPS.
#[test]
fn a_busy_http_port_at_boot_is_a_failed_row_and_https_serves() {
    if !crate::common::own_process() {
        return;
    }
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = held.local_addr().unwrap().port().to_string();
    let seeded = crate::common::seeded("listener-busy-boot", "");
    busy_http_properties(&busy);
    let server = moqui_host::start_server_by_conf(&seeded.runtime, crate::common::framework_dir(), &seeded.cluster_dir)
        .expect("the server starts with its HTTP port held by another");
    let row = moqui_core::startup::report(server.factory())
        .entries()
        .into_iter()
        .find(|row| row.section == moqui_core::startup::Section::Listener && row.name == "HTTP")
        .expect("HTTP's row");
    assert_eq!(row.state.word(), Some("failed"), "{:?}", row.state);
    assert!(row.state.detail().contains(&busy), "{:?}", row.state);
    assert_eq!(server.listener_address("HTTP"), None);
    let https = server.https_address().expect("HTTPS serves");
    let key = login_key(server.factory());
    answers_ping(&mut open_secure(https, &key));
    drop(server);
    drop(held);
}

/// A server started on an address its caller named (`start_server`, `moqui test --serve`) still refuses to start when
/// that address is held by another, naming it, rather than start with no HTTP.
#[test]
fn a_named_address_held_by_another_stops_the_start() {
    if !crate::common::own_process() {
        return;
    }
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = held.local_addr().unwrap();
    let seeded = crate::common::seeded("listener-named-busy", "");
    busy_http_properties(&busy.port().to_string());
    let started = moqui_host::start_server(
        &seeded.runtime,
        crate::common::framework_dir(),
        &seeded.cluster_dir,
        busy,
    );
    let Err(err) = started else {
        panic!("a server started on a busy address it was given started");
    };
    assert!(err.to_string().contains(&busy.port().to_string()), "{err}");
    drop(held);
}
