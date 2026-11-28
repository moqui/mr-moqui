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

//! The browser terminal: the shell offers it in the account menu, its screen is the xterm.js component, and its socket,
//! `/termws`, is refused without a session, runs the apps as the signed in user once the page sends its size, gives
//! `terminal` `max-sessions-per-user` as the close reason of a second socket, and ends on Ctrl+Q with the terminal put back.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "tui"))]

use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, ClientRequestBuilder, Message, WebSocket};

pub(crate) type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

fn request(address: SocketAddr, cookie: Option<&str>) -> ClientRequestBuilder {
    let uri: tungstenite::http::Uri = format!("ws://{address}/termws").parse().unwrap();
    let request = ClientRequestBuilder::new(uri);
    match cookie {
        Some(cookie) => request.with_header("Cookie", cookie),
        None => request,
    }
}

/// Open a socket and send the first size.
fn open(address: SocketAddr, cookie: &str, columns: u16, rows: u16) -> Ws {
    let (mut socket, response) = connect(request(address, Some(cookie))).unwrap();
    assert_eq!(response.status().as_u16(), 101);
    if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
        stream.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    }
    let size = format!(r#"{{"type":"resize","cols":{columns},"rows":{rows}}}"#);
    socket.send(Message::Text(size.into())).unwrap();
    socket
}

/// What a socket gives within 30 s, until `want` is in its bytes (`Ok`) or it closes first (`Err` with the reason).
fn read_until(socket: &mut Ws, seen: &mut Vec<u8>, want: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let found = |seen: &[u8]| seen.windows(want.len()).any(|window| window == want.as_bytes());
    while !found(seen) {
        assert!(
            Instant::now() < deadline,
            "{want:?} never came: {}",
            String::from_utf8_lossy(seen)
        );
        match socket.read() {
            Ok(Message::Binary(bytes)) => seen.extend_from_slice(&bytes),
            Ok(Message::Close(frame)) => return Err(frame.map(|frame| frame.reason.to_string()).unwrap_or_default()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(err))
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(err) => return Err(format!("read: {err}")),
        }
    }
    Ok(())
}

/// A user in no group but ALL_USERS (password moqui, as john.doe): the Terminal is for every user, not only ADMIN.
const PLAIN_USER: &str = r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="PLAIN_USER" username="plain.user" userFullName="Plain User"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
</entity-facade-xml>"#;

/// The Terminal screen rendered, with its authorization: a `.qvt` request naming the client modes, as
/// `moqui.loadComponent` sends one, answers the qvue component. (`/apps/Terminal.qvue` is the file beside the screen,
/// served as content.)
fn terminal_screen(address: SocketAddr, cookie: &str) -> (u16, Vec<(String, String)>, String) {
    crate::common::exchange(
        address,
        &crate::common::http_request(
            "GET",
            "/apps/Terminal.qvt",
            &[("Cookie", cookie), ("X-Moqui-Client-Modes", "qvue,qjs")],
            "",
        ),
    )
}

#[test]
fn the_browser_terminal_runs_the_apps_as_the_signed_in_user() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_conf_properties(
        "termws",
        Some(PLAIN_USER),
        r#"<terminal max-sessions-per-user="1"/>"#,
        &[],
    );
    let address = gate.server.address();

    // No session cookie, or one that is not a session: no socket.
    for cookie in [None, Some("moqui.session=not-a-session")] {
        match connect(request(address, cookie)) {
            Err(tungstenite::Error::Http(response)) => assert_eq!(response.status().as_u16(), 401),
            Ok(_) => panic!("the terminal socket opened without a session"),
            Err(err) => panic!("expected HTTP 401, got {err}"),
        }
    }

    // A user with no permissions may open the Terminal screen and its socket, as the account menu offers it to all.
    let plain = crate::common::sign_in(address, "plain.user", "moqui");
    gate.server.factory().flush_sessions().unwrap();
    let (status, _, component) = terminal_screen(address, &plain);
    assert_eq!(status, 200, "{component}");
    let mut socket = open(address, &plain, 120, 40);
    let mut seen = Vec::new();
    read_until(&mut socket, &mut seen, "plain.user").unwrap();
    drop(socket);

    let cookie = crate::common::sign_in(address, "john.doe", "moqui");
    gate.server.factory().flush_sessions().unwrap();

    // The shell's config names the Terminal screen and loads the terminal's store, which opens the socket, and the
    // screen shows a session through the store's view.
    let get = |path: &str| {
        crate::common::exchange(
            address,
            &crate::common::http_request("GET", path, &[("Cookie", &cookie)], ""),
        )
    };
    let (status, _, shell) = get("/qapps");
    assert_eq!(status, 200, "{shell}");
    assert!(shell.contains(r#""terminalUrl":"/qapps/Terminal""#), "{shell}");
    assert!(shell.contains("/js/WebrootTerminal.js"), "{shell}");
    let (status, _, component) = terminal_screen(address, &cookie);
    assert_eq!(status, 200, "{component}");
    assert!(component.contains("m-terminal-view"), "{component}");
    let mut socket = open(address, &cookie, 120, 40);
    let mut seen = Vec::new();
    read_until(&mut socket, &mut seen, "\x1b[?1049h").unwrap();
    read_until(&mut socket, &mut seen, "john.doe").unwrap();
    // True color, from the terminal the socket describes.
    read_until(&mut socket, &mut seen, "38;2;").unwrap();

    // A second terminal of the same user is past terminal max-sessions-per-user (1): the close reason says so.
    let mut second = open(address, &cookie, 120, 40);
    let reason = read_until(&mut second, &mut Vec::new(), "never").unwrap_err();
    assert!(reason.contains("max-sessions-per-user"), "{reason}");

    // A resize redraws whole; Ctrl+Q ends the session, the terminal put back, and the socket closes.
    socket
        .send(Message::Text(r#"{"type":"resize","cols":160,"rows":50}"#.into()))
        .unwrap();
    let mark = seen.len();
    let mut after = seen.split_off(mark);
    read_until(&mut socket, &mut after, "\x1b[2J").unwrap();
    seen.extend(after);
    socket.send(Message::Binary(b"\x11".to_vec().into())).unwrap();
    let reason = read_until(&mut socket, &mut seen, "never").unwrap_err();
    assert_eq!(reason, "the session ended");
    assert!(seen.ends_with(b"\x1b[?1049l"), "the terminal is put back");
}

/// Read `socket` on a thread of its own until it closes: when it did, and the close reason (a close frame's, or the
/// read's error).
fn closed_at(mut socket: Ws) -> std::thread::JoinHandle<(Instant, String)> {
    std::thread::spawn(move || {
        if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
            stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match socket.read() {
                Ok(Message::Close(frame)) => {
                    return (
                        Instant::now(),
                        frame.map(|frame| frame.reason.to_string()).unwrap_or_default(),
                    )
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(err))
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(err) => return (Instant::now(), format!("read: {err}")),
            }
        }
        (Instant::now(), "never closed".to_string())
    })
}

/// A stop closes every kind of upgraded socket at once (HL-027: the HTTP listener's drain waits for them): a terminal
/// running a session, one still waiting for the page's size, and an XMPP WebSocket; so the stop does not wait out the
/// shutdown timeout.
#[test]
fn a_stop_closes_terminal_and_xmpp_websockets_at_once() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_conf_properties(
        "termws-stop",
        Some(PLAIN_USER),
        // the websocket alone, no client port, as before the written ports (plans/Runtime-Config.md §3.2)
        r#"<xmpp-server port="" tls-port=""/>"#,
        &[
            ("moqui_conf__xmpp_server__enabled", "true"),
            ("moqui_conf__xmpp_server__domain", "stop.example"),
            ("moqui_conf__xmpp_server__websocket_enabled", "true"),
        ],
    );
    let address = gate.server.address();
    let cookie = crate::common::sign_in(address, "john.doe", "moqui");
    let plain = crate::common::sign_in(address, "plain.user", "moqui");
    gate.server.factory().flush_sessions().unwrap();
    let mut running = open(address, &cookie, 120, 40);
    read_until(&mut running, &mut Vec::new(), "john.doe").unwrap();
    let (waiting, response) = connect(request(address, Some(&plain))).unwrap();
    assert_eq!(response.status().as_u16(), 101);
    let uri: tungstenite::http::Uri = format!("ws://{address}/xmpp-websocket").parse().unwrap();
    let (xmpp, response) = connect(ClientRequestBuilder::new(uri).with_sub_protocol("xmpp")).unwrap();
    assert_eq!(response.status().as_u16(), 101);
    let readers = [closed_at(running), closed_at(waiting), closed_at(xmpp)];
    let stopping = Instant::now();
    drop(gate);
    let stopped = stopping.elapsed();
    for (kind, reader) in ["running terminal", "waiting terminal", "XMPP"].iter().zip(readers) {
        let (at, reason) = reader.join().unwrap();
        let after = at.saturating_duration_since(stopping);
        assert!(
            after < Duration::from_secs(1),
            "the {kind} socket closed {after:?} after the stop: {reason}"
        );
        if kind.contains("terminal") {
            assert_eq!(reason, "the server is stopping", "{kind}");
        }
    }
    assert!(stopped < Duration::from_secs(15), "the stop took {stopped:?}");
}

/// The page's session token and a JSON call of the Terminal screen's transitions, as the store makes them.
pub(crate) struct Page {
    address: SocketAddr,
    cookie: String,
    token: String,
}

impl Page {
    pub(crate) fn signed_in(address: SocketAddr, username: &str) -> Self {
        let cookie = crate::common::sign_in(address, username, "moqui");
        let (status, _, html) = crate::common::exchange(
            address,
            &crate::common::http_request("GET", "/qapps", &[("Cookie", &cookie)], ""),
        );
        assert_eq!(status, 200, "{html}");
        let token = crate::common::session_token(&html).expect("the shell carries the session token");
        Self { address, cookie, token }
    }

    pub(crate) fn call(&self, name: &str, body: Option<serde_json::Value>) -> serde_json::Value {
        self.call_with(name, body, &[])
    }

    /// A call with more headers: a proxy's, to look like a request from another machine.
    fn call_with(&self, name: &str, body: Option<serde_json::Value>, more: &[(&str, &str)]) -> serde_json::Value {
        let path = format!("/apps/Terminal/{name}");
        let (status, _, text) = match body {
            Some(mut body) => {
                body["moquiSessionToken"] = serde_json::Value::String(self.token.clone());
                crate::common::exchange(
                    self.address,
                    &crate::common::http_request(
                        "POST",
                        &path,
                        &[
                            &[
                                ("Cookie", self.cookie.as_str()),
                                ("Content-Type", "application/json"),
                                ("X-CSRF-Token", self.token.as_str()),
                                // the page's own origin, as a browser sends it on a POST
                                ("Origin", "http://localhost"),
                            ][..],
                            more,
                        ]
                        .concat(),
                        &body.to_string(),
                    ),
                )
            }
            None => crate::common::exchange(
                self.address,
                &crate::common::http_request(
                    "GET",
                    &path,
                    &[&[("Cookie", self.cookie.as_str())][..], more].concat(),
                    "",
                ),
            ),
        };
        assert_eq!(status, 200, "{name}: {text}");
        serde_json::from_str(&text).unwrap_or_else(|err| panic!("{name}: {err}: {text}"))
    }

    /// Attach a socket to the kept session `id`, as a page that drew `have` of its bytes.
    pub(crate) fn attach(&self, id: &str, have: Option<u64>) -> Ws {
        self.attach_query(&format!("session={id}"), have)
    }

    pub(crate) fn cookie(&self) -> &str {
        &self.cookie
    }

    /// A GET of the Terminal screen's transition `name` (with its query), as text.
    pub(crate) fn get_text(&self, name: &str) -> String {
        let (status, _, text) = crate::common::exchange(
            self.address,
            &crate::common::http_request(
                "GET",
                &format!("/apps/Terminal/{name}"),
                &[("Cookie", self.cookie.as_str())],
                "",
            ),
        );
        assert_eq!(status, 200, "{name}: {text}");
        text
    }

    /// Attach a window that only watches the session `id` (`observe=1`).
    pub(crate) fn observe(&self, id: &str) -> Ws {
        self.attach_query(&format!("session={id}&observe=1"), None)
    }

    fn attach_query(&self, query: &str, have: Option<u64>) -> Ws {
        let uri: tungstenite::http::Uri = format!("ws://{}/termws?{query}", self.address).parse().unwrap();
        let request = ClientRequestBuilder::new(uri)
            .with_header("Cookie", &self.cookie)
            .with_header("Origin", format!("http://{}", self.address));
        let (mut socket, response) = connect(request).unwrap();
        assert_eq!(response.status().as_u16(), 101);
        if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
            stream.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        }
        let attach = serde_json::json!({"type": "attach", "cols": 100, "rows": 30, "have": have});
        socket.send(Message::Text(attach.to_string().into())).unwrap();
        socket
    }
}

/// The text messages and bytes a socket gives until `want` is in its bytes, or its close reason.
pub(crate) fn read_messages_until(socket: &mut Ws, want: &str) -> Result<(Vec<String>, Vec<u8>), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut texts = Vec::new();
    let mut seen = Vec::new();
    while !seen.windows(want.len()).any(|window| window == want.as_bytes()) {
        assert!(
            Instant::now() < deadline,
            "{want:?} never came: {}",
            String::from_utf8_lossy(&seen)
        );
        match socket.read() {
            Ok(Message::Binary(bytes)) => seen.extend_from_slice(&bytes),
            Ok(Message::Text(text)) => texts.push(text.to_string()),
            Ok(Message::Close(frame)) => return Err(frame.map(|frame| frame.reason.to_string()).unwrap_or_default()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(err))
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(err) => return Err(format!("read: {err}")),
        }
    }
    Ok((texts, seen))
}

/// A kept apps session (plans/archive/Terminal-Fancy.md §5): opened by the screen's transition, it outlives its socket,
/// draws its whole screen again for the next one, is its own user's only, counts toward the limits, and ends when
/// closed.
#[test]
fn a_kept_session_outlives_its_socket_and_draws_again_on_reattach() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_conf_properties(
        "termws-kept",
        Some(PLAIN_USER),
        r#"<terminal max-sessions-per-user="1"/>"#,
        &[],
    );
    let address = gate.server.address();
    let page = Page::signed_in(address, "john.doe");
    let plain = Page::signed_in(address, "plain.user");
    gate.server.factory().flush_sessions().unwrap();

    let options = page.call("options", None);
    assert_eq!(options["options"]["kinds"][0], "apps", "{options}");
    let opened = page.call(
        "open",
        Some(serde_json::json!({"kind": "apps", "cols": 100, "rows": 30})),
    );
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();
    // the limit is the answer of the second open
    let refused = page.call("open", Some(serde_json::json!({"kind": "apps"})));
    assert_eq!(refused["reasonCode"], "limit", "{refused}");
    assert!(
        refused["errors"]
            .as_str()
            .unwrap_or_default()
            .contains("max-sessions-per-user"),
        "{refused}"
    );

    let mut socket = page.attach(&id, None);
    let (texts, _) = read_messages_until(&mut socket, "john.doe").unwrap();
    assert!(
        texts[0].contains(r#""type":"attached""#) && texts[0].contains(r#""kind":"apps""#),
        "{texts:?}"
    );
    drop(socket);

    // the session goes on with no window, and the next window gets its whole screen
    let listed = page.call("sessions", None);
    assert_eq!(listed["sessions"][0]["id"], id.as_str(), "{listed}");
    let mut again = page.attach(&id, None);
    let (_, bytes) = read_messages_until(&mut again, "john.doe").unwrap();
    let screen = String::from_utf8_lossy(&bytes);
    assert!(
        screen.contains("\x1b[?1049h") && screen.contains("\x1b[2J"),
        "drawn again: {screen:?}"
    );

    // another user's id answers as one that does not exist
    let mut other = plain.attach(&id, None);
    assert_eq!(
        read_messages_until(&mut other, "never").unwrap_err(),
        "no such terminal session"
    );
    assert!(plain.call("sessions", None)["sessions"].as_array().unwrap().is_empty());

    // a second window takes the session; the first is told
    let mut taker = page.attach(&id, None);
    assert_eq!(
        read_messages_until(&mut again, "never").unwrap_err(),
        "opened elsewhere"
    );
    read_messages_until(&mut taker, "john.doe").unwrap();

    page.call("close", Some(serde_json::json!({"sessionId": id})));
    let reason = read_messages_until(&mut taker, "never").unwrap_err();
    assert_eq!(reason, "the session was ended");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !page.call("sessions", None)["sessions"].as_array().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "an ended apps session is not kept");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The `moqui>` command line in the browser (plans/archive/Terminal-Fancy.md §6, §8): on a dev instance for a user with
/// SSH_SHELL, from this machine; refused to a user without it and to a request through a proxy while remote is off; a
/// window that comes back goes on with nothing repeated; an audit row is written.
#[test]
fn the_command_line_opens_on_a_dev_instance_for_a_user_with_ssh_shell() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_conf_properties(
        "termws-command",
        Some(PLAIN_USER),
        "",
        &[("moqui_conf__instance__purpose", "dev")],
    );
    let address = gate.server.address();
    let page = Page::signed_in(address, "john.doe");
    let plain = Page::signed_in(address, "plain.user");
    gate.server.factory().flush_sessions().unwrap();

    let options = page.call("options", None);
    assert!(
        options["options"]["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|kind| kind == "command"),
        "{options}"
    );
    let refused = plain.call("open", Some(serde_json::json!({"kind": "command"})));
    assert_eq!(refused["reasonCode"], "notAvailable", "{refused}");
    assert_eq!(
        plain.call("options", None)["options"]["kinds"],
        serde_json::json!(["apps"])
    );
    // through a proxy is another machine, and remote is off
    let proxied = page.call_with(
        "open",
        Some(serde_json::json!({"kind": "command"})),
        &[("X-Forwarded-For", "203.0.113.5")],
    );
    assert_eq!(proxied["reasonCode"], "notAvailable", "{proxied}");

    let opened = page.call(
        "open",
        Some(serde_json::json!({"kind": "command", "cols": 100, "rows": 30})),
    );
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();
    let mut socket = page.attach(&id, None);
    let (_, first) = read_messages_until(&mut socket, "moqui> ").unwrap();
    socket.send(Message::Binary(b"status\r".to_vec().into())).unwrap();
    let (_, mut more) = read_messages_until(&mut socket, "user john.doe").unwrap();
    // let the prompt come back before leaving
    let after = String::from_utf8_lossy(&more)
        .rsplit("user john.doe")
        .next()
        .unwrap_or_default()
        .to_string();
    if !after.contains("moqui> ") {
        more.extend(read_messages_until(&mut socket, "moqui> ").unwrap().1);
    }
    let drawn = (first.len() + more.len()) as u64;
    drop(socket);

    // back with all it drew: nothing is repeated, and the history has the line
    let mut back = page.attach(&id, Some(drawn));
    back.send(Message::Binary(b"\x1b[A".to_vec().into())).unwrap();
    let (texts, bytes) = read_messages_until(&mut back, "status").unwrap();
    assert!(texts[0].contains(r#""reset":false"#), "{texts:?}");
    assert!(
        !String::from_utf8_lossy(&bytes).contains("user john.doe"),
        "repeated: {:?}",
        String::from_utf8_lossy(&bytes)
    );

    // the audit row, read at the prompt itself
    back.send(Message::Binary(
        b"\x15entity moqui.terminal.TerminalSession\r".to_vec().into(),
    ))
    .unwrap();
    let (_, listed) = read_messages_until(&mut back, &id).unwrap();
    assert!(
        String::from_utf8_lossy(&listed).contains("\"kind\": \"command\""),
        "{}",
        String::from_utf8_lossy(&listed)
    );

    // another user's attach finds nothing
    let mut other = plain.attach(&id, None);
    assert_eq!(
        read_messages_until(&mut other, "never").unwrap_err(),
        "no such terminal session"
    );
}

/// The browser terminal's helpers, for the host sessions of these tests: the server runs this test binary with this
/// test's name (`terminal_exec_helper`), and with the program in its environment this becomes it (`term-exec`), or with
/// a holder's directory the holder (`term-hold`). Run on its own, it does nothing.
#[test]
fn term_exec_helper() {
    let mode = if std::env::var_os("MOQUI_TERM_EXEC").is_some() {
        "term-exec"
    } else if std::env::var_os("MOQUI_TERM_HOLD").is_some() {
        "term-hold"
    } else {
        return;
    };
    let status = moqui_host::terminal_helper(&[mode.to_string()]).unwrap_or(127);
    std::process::exit(status);
}

/// The `terminal_exec_helper` property that runs [`term_exec_helper`].
pub fn helper_property() -> (&'static str, String) {
    let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    let command = serde_json::json!([
        exe,
        "termws::term_exec_helper",
        "--exact",
        "--quiet",
        "--nocapture",
        "--test-threads",
        "1"
    ]);
    ("terminal_exec_helper", command.to_string())
}

/// The Terminal screen with a transition that passes a host session's first `input` too, as the Dev AIDE's Run locally
/// calls `open#TerminalSession` from its own transition (the Terminal's `open` takes no input from the page).
const OPEN_WITH_INPUT: &str = r#"<screen-extend>
    <transition name="openWithInput" method="post" begin-transaction="false" authz-action="view">
        <actions><script>ec.resource.script('component://framework/service/org/moqui/terminal/TerminalWeb.js').execute('web', {verb: 'open#TerminalSession', names: ['kind', 'profileName', 'cols', 'rows', 'input']})</script></actions>
        <default-response type="none"/>
    </transition>
</screen-extend>
"#;

/// A host session (plans/archive/Terminal-Fancy.md §7): a program on a pty for a user with TERMINAL_HOST on a dev
/// instance, refused to one without; it echoes, takes the window's size, replays its output to a window that comes
/// back, and ends with its exit status. One opened with `input` runs it as typed, and its output shows what it ran. The
/// pty is Unix's: this module is `cfg(unix)`, as the host sessions are, so Windows, with no host session, has none to
/// test.
#[test]
fn a_host_session_runs_a_program_on_a_pty() {
    if !crate::common::own_process() {
        return;
    }
    let helper = helper_property();
    let gate = crate::common::start_with_conf_files(
        "termws-host",
        Some(PLAIN_USER),
        r#"<terminal><host-profile name="plain" title="Plain sh" command="sh"/></terminal>"#,
        &[("moqui_conf__instance__purpose", "dev"), (helper.0, &helper.1)],
        &[
            (
                "components/addon/term-input/component.xml",
                r#"<component name="term-input"/>"#,
            ),
            (
                "components/addon/term-input/screen-extend/webroot/screen/webroot/apps/Terminal.xml",
                OPEN_WITH_INPUT,
            ),
        ],
    );
    let address = gate.server.address();
    let page = Page::signed_in(address, "john.doe");
    let plain = Page::signed_in(address, "plain.user");
    gate.server.factory().flush_sessions().unwrap();

    let options = page.call("options", None);
    let profiles = options["options"]["profiles"].as_array().unwrap();
    assert!(profiles.iter().any(|profile| profile["name"] == "plain"), "{options}");
    let refused = plain.call(
        "open",
        Some(serde_json::json!({"kind": "host", "profileName": "plain"})),
    );
    assert_eq!(refused["reasonCode"], "notAvailable", "{refused}");

    let opened = page.call(
        "open",
        Some(serde_json::json!({"kind": "host", "profileName": "plain", "cols": 100, "rows": 30})),
    );
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();
    let mut socket = page.attach(&id, None);
    socket
        .send(Message::Binary(b"echo hi-$((1+2))\r".to_vec().into()))
        .unwrap();
    read_messages_until(&mut socket, "hi-3").unwrap();
    socket.send(Message::Binary(b"stty size\r".to_vec().into())).unwrap();
    read_messages_until(&mut socket, "30 100").unwrap();
    let resize = serde_json::json!({"type": "resize", "cols": 120, "rows": 40});
    socket.send(Message::Text(resize.to_string().into())).unwrap();
    socket.send(Message::Binary(b"stty size\r".to_vec().into())).unwrap();
    read_messages_until(&mut socket, "40 120").unwrap();
    drop(socket);

    // a window that comes back with nothing drawn gets the output again
    let mut back = page.attach(&id, None);
    let (texts, _) = read_messages_until(&mut back, "hi-3").unwrap();
    assert!(
        texts[0].contains(r#""reset":true"#) && texts[0].contains(r#""kind":"host""#),
        "{texts:?}"
    );
    back.send(Message::Binary(b"exit 3\r".to_vec().into())).unwrap();
    let mut ended = Vec::new();
    let reason = loop {
        match back.read() {
            Ok(Message::Text(text)) => ended.push(text.to_string()),
            Ok(Message::Close(frame)) => break frame.map(|frame| frame.reason.to_string()).unwrap_or_default(),
            Ok(_) => {}
            Err(tungstenite::Error::Io(err))
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(err) => panic!("{err}"),
        }
    };
    assert!(reason.contains("status 3"), "{reason}");
    assert!(
        ended
            .iter()
            .any(|text| text.contains(r#""type":"ended""#) && text.contains(r#""status":3"#)),
        "{ended:?}"
    );
    // an ended host session stays listed, with its status
    let listed = page.call("sessions", None);
    let session = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["id"] == id.as_str())
        .cloned();
    assert_eq!(
        session.map(|session| session["exitStatus"].clone()),
        Some(serde_json::json!(3)),
        "{listed}"
    );

    // a session opened with input runs it as typed: the shell's output is what the line computes, beside its echo
    let opened = page.call(
        "openWithInput",
        Some(serde_json::json!({"kind": "host", "profileName": "plain", "input": "echo in-$((2+3))\n"})),
    );
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();
    let mut socket = page.attach(&id, None);
    let (_, seen) = read_messages_until(&mut socket, "in-5").unwrap();
    assert!(
        String::from_utf8_lossy(&seen).contains("echo in-$((2+3))"),
        "the input as typed: {}",
        String::from_utf8_lossy(&seen)
    );
    socket.send(Message::Binary(b"exit\r".to_vec().into())).unwrap();
}

/// The Terminal screen with transitions for the driving services, as a program's page would call them.
const DRIVE_TRANSITIONS: &str = r#"<screen-extend>
    <transition name="send" method="post" begin-transaction="false" authz-action="view">
        <actions><script>ec.resource.script('component://framework/service/org/moqui/terminal/TerminalWeb.js').execute('web', {verb: 'send#TerminalInput', names: ['sessionId', 'text', 'keys']})</script></actions>
        <default-response type="none"/>
    </transition>
    <transition name="wait" method="post" begin-transaction="false" authz-action="view">
        <actions><script>ec.resource.script('component://framework/service/org/moqui/terminal/TerminalWeb.js').execute('web', {verb: 'wait#TerminalSession', names: ['sessionId', 'pattern', 'prompt', 'timeoutSeconds']})</script></actions>
        <default-response type="none"/>
    </transition>
    <transition name="events" read-only="true" begin-transaction="false">
        <actions><script>ec.resource.script('component://framework/service/org/moqui/terminal/TerminalWeb.js').execute('web', {verb: 'find#TerminalEvents', names: ['sessionId', 'after']})</script></actions>
        <default-response type="none"/>
    </transition>
</screen-extend>
"#;

/// Sessions driven by a program (plans/Terminal-Fancy2.md §4.3): a shell typed into, waited on and read as text through
/// the services, by its user; and through the REST by the session's own token from inside it, which reaches the user's
/// shells and nothing with a wrong token. Another user's session answers as one that does not exist.
#[test]
fn a_shell_is_driven_through_the_services_and_by_its_token() {
    if !crate::common::own_process() {
        return;
    }
    let helper = helper_property();
    let gate = crate::common::start_with_conf_files(
        "termws-drive",
        Some(PLAIN_USER),
        r#"<terminal><host-profile name="plain" title="Plain sh" command="sh"/></terminal>"#,
        &[("moqui_conf__instance__purpose", "dev"), (helper.0, &helper.1)],
        &[
            (
                "components/addon/term-drive/component.xml",
                r#"<component name="term-drive"/>"#,
            ),
            (
                "components/addon/term-drive/screen-extend/webroot/screen/webroot/apps/Terminal.xml",
                DRIVE_TRANSITIONS,
            ),
        ],
    );
    let address = gate.server.address();
    let page = Page::signed_in(address, "john.doe");
    let plain = Page::signed_in(address, "plain.user");
    gate.server.factory().flush_sessions().unwrap();
    let opened = page.call(
        "open",
        Some(serde_json::json!({"kind": "host", "profileName": "plain", "cols": 80, "rows": 24})),
    );
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();

    let sent = page.call(
        "send",
        Some(serde_json::json!({"sessionId": id, "text": "echo drive-$((4+5))", "keys": "Enter"})),
    );
    assert!(sent["count"].as_u64().is_some(), "{sent}");
    let waited = page.call(
        "wait",
        Some(serde_json::json!({"sessionId": id, "pattern": "^drive-9$", "timeoutSeconds": 10})),
    );
    assert_eq!(waited["matched"], "pattern", "{waited}");
    let screen = page.call(&format!("screen?sessionId={id}&scope=last&lines=3"), None);
    let lines: Vec<&str> = screen["lines"]
        .as_array()
        .unwrap_or_else(|| panic!("{screen}"))
        .iter()
        .filter_map(|line| line.as_str())
        .collect();
    assert!(lines.contains(&"drive-9"), "{screen}");
    assert_eq!(screen["kind"], "host");
    let events = page.call(&format!("events?sessionId={id}&after=0"), None);
    assert!(events["events"].is_array(), "{events}");
    // another user's session is not there for them
    let refused = plain.call(&format!("screen?sessionId={id}"), None);
    assert_eq!(refused["reasonCode"], "notFound", "{refused}");
    // a bad key name is refused before anything is typed
    let bad = page.call("send", Some(serde_json::json!({"sessionId": id, "keys": "Hyper"})));
    assert_eq!(bad["reasonCode"], "badInput", "{bad}");

    // the shell's own token, from its environment: good on the REST from this machine, for this user's shells
    page.call(
        "send",
        Some(serde_json::json!({"sessionId": id, "text": "echo \"T=$MOQUI_TERMINAL_TOKEN\"", "keys": "Enter"})),
    );
    let waited = page.call(
        "wait",
        Some(serde_json::json!({"sessionId": id, "pattern": "^T=\\S+$", "timeoutSeconds": 10})),
    );
    assert_eq!(waited["matched"], "pattern", "the shell has its token: {waited}");
    let token = waited["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|line| line.as_str())
        .find_map(|line| line.strip_prefix("T="))
        .unwrap()
        .to_string();
    let rest = |token: &str| {
        crate::common::exchange(
            address,
            &crate::common::http_request(
                "GET",
                &format!("/rest/s1/moqui/terminal/{id}/screen?scope=last&lines=5"),
                &[
                    ("x-moqui-terminal-session", id.as_str()),
                    ("x-moqui-terminal-token", token),
                    ("Accept", "application/json"),
                ],
                "",
            ),
        )
    };
    let (status, _, text) = rest(&token);
    assert_eq!(status, 200, "{text}");
    let by_token: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|err| panic!("{err}: {text}"));
    assert!(
        by_token["lines"]
            .as_array()
            .is_some_and(|lines| lines.iter().any(|line| line == "drive-9")),
        "{by_token}"
    );
    let (_, _, text) = rest("not-the-token");
    assert!(!text.contains("drive-9"), "a wrong token reads nothing: {text}");
    page.call(
        "send",
        Some(serde_json::json!({"sessionId": id, "text": "exit", "keys": "Enter"})),
    );
}

/// Viewers and sharing (plans/Terminal-Fancy2.md §4.4): another user sees a shell only once its owner shares it, then
/// watches it beside the window that types, and what they type is dropped; typing in it is refused to a user without
/// TERMINAL_HOST; the owner's own second window may only watch; and a revoked share closes the user's window.
#[test]
fn a_shared_shell_is_watched_beside_its_writer_and_its_share_revoked() {
    if !crate::common::own_process() {
        return;
    }
    let helper = helper_property();
    let gate = crate::common::start_with_conf_files(
        "termws-share",
        Some(PLAIN_USER),
        r#"<terminal><host-profile name="plain" title="Plain sh" command="sh"/></terminal>"#,
        &[("moqui_conf__instance__purpose", "dev"), (helper.0, &helper.1)],
        &[],
    );
    let address = gate.server.address();
    let page = Page::signed_in(address, "john.doe");
    let plain = Page::signed_in(address, "plain.user");
    gate.server.factory().flush_sessions().unwrap();
    let opened = page.call(
        "open",
        Some(serde_json::json!({"kind": "host", "profileName": "plain", "cols": 100, "rows": 30})),
    );
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();
    let mut writer = page.attach(&id, None);
    writer
        .send(Message::Binary(b"echo first-$((1+1))\r".to_vec().into()))
        .unwrap();
    read_messages_until(&mut writer, "first-2").unwrap();

    // not shared: another user's id is no session at all
    let mut early = plain.attach(&id, None);
    assert_eq!(
        read_messages_until(&mut early, "never").unwrap_err(),
        "no such terminal session"
    );
    // typing needs the kind's permission, which plain.user has not got; watching does not
    let refused = page.call(
        "share",
        Some(serde_json::json!({"sessionId": id, "username": "plain.user", "write": "Y"})),
    );
    assert_eq!(refused["reasonCode"], "noPermission", "{refused}");
    let shared = page.call(
        "share",
        Some(serde_json::json!({"sessionId": id, "username": "plain.user"})),
    );
    assert_eq!(shared["session"]["shares"][0]["username"], "plain.user", "{shared}");
    let listed = plain.call("sessions", None);
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["id"] == id.as_str() && session["shared"] == true),
        "{listed}"
    );

    // the user watches: the replay, then what the writer's program prints; what they type is dropped
    let mut watcher = plain.attach(&id, None);
    let (texts, _) = read_messages_until(&mut watcher, "first-2").unwrap();
    assert!(
        texts[0].contains(r#""viewer":true"#) && texts[0].contains(r#""owner":"john.doe""#),
        "{texts:?}"
    );
    watcher
        .send(Message::Binary(b"echo leaked-$((3+3))\r".to_vec().into()))
        .unwrap();
    writer
        .send(Message::Binary(b"echo second-$((2+2))\r".to_vec().into()))
        .unwrap();
    read_messages_until(&mut watcher, "second-4").unwrap();
    let (_, seen) = read_messages_until(&mut writer, "second-4").unwrap();
    assert!(
        !String::from_utf8_lossy(&seen).contains("leaked-6"),
        "a watcher typed: {}",
        String::from_utf8_lossy(&seen)
    );
    // the owner's own window that only watches
    let mut own_view = page.observe(&id);
    let (texts, _) = read_messages_until(&mut own_view, "second-4").unwrap();
    assert!(texts[0].contains(r#""viewer":true"#), "{texts:?}");
    drop(own_view);

    // revoked: the user's window is let go, and the session is theirs no more
    page.call(
        "revoke",
        Some(serde_json::json!({"sessionId": id, "username": "plain.user"})),
    );
    assert_eq!(
        read_messages_until(&mut watcher, "never").unwrap_err(),
        "the share was revoked"
    );
    let listed = plain.call("sessions", None);
    assert!(
        !listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["id"] == id.as_str()),
        "{listed}"
    );
    writer.send(Message::Binary(b"exit\r".to_vec().into())).unwrap();
}

/// Recording forced by the conf (plans/Terminal-Fancy2.md §4.5, D2): every host session is recorded from its start, its
/// owner cannot stop it, the file has its output and never what was typed beyond the echo, and nobody else plays it.
#[test]
fn a_forced_recording_cannot_be_stopped_and_plays_for_its_owner_alone() {
    if !crate::common::own_process() {
        return;
    }
    let helper = helper_property();
    let gate = crate::common::start_with_conf_files(
        "termws-record",
        Some(PLAIN_USER),
        r#"<terminal recording="always"><host-profile name="plain" title="Plain sh" command="sh"/></terminal>"#,
        &[("moqui_conf__instance__purpose", "dev"), (helper.0, &helper.1)],
        &[],
    );
    let address = gate.server.address();
    let page = Page::signed_in(address, "john.doe");
    let plain = Page::signed_in(address, "plain.user");
    gate.server.factory().flush_sessions().unwrap();
    assert_eq!(page.call("options", None)["options"]["recording"], "always");
    let opened = page.call(
        "open",
        Some(serde_json::json!({"kind": "host", "profileName": "plain"})),
    );
    assert_eq!(opened["session"]["recording"], true, "{opened}");
    assert_eq!(opened["session"]["recordingForced"], true, "{opened}");
    let id = opened["sessionId"].as_str().unwrap().to_string();
    let mut socket = page.attach(&id, None);
    // a query the program asks the terminal is left out of the recording
    socket
        .send(Message::Binary(
            b"printf 'q\\033[6nq'; echo; echo rec-$((6*7))\r".to_vec().into(),
        ))
        .unwrap();
    read_messages_until(&mut socket, "rec-42").unwrap();
    let refused = page.call("rename", Some(serde_json::json!({"sessionId": id, "record": "N"})));
    assert_eq!(refused["reasonCode"], "notStopped", "{refused}");
    let recordings = page.call("recordings", None);
    let recording = &recordings["recordings"][0];
    assert_eq!(recording["forced"], true, "{recordings}");
    let recording_id = recording["recordingId"].as_str().unwrap().to_string();
    let cast = page.get_text(&format!("recording?recordingId={recording_id}"));
    assert!(cast.contains("rec-42"), "{cast}");
    assert!(!cast.contains("\\u001b[6n"), "the query is left out: {cast}");
    // another user's request is as for none
    let (status, _, _) = crate::common::exchange(
        address,
        &crate::common::http_request(
            "GET",
            &format!("/apps/Terminal/recording?recordingId={recording_id}"),
            &[("Cookie", plain.cookie())],
            "",
        ),
    );
    assert_eq!(status, 404);
    assert!(plain.call("recordings", None)["recordings"]
        .as_array()
        .unwrap()
        .is_empty());
    socket.send(Message::Binary(b"exit\r".to_vec().into())).unwrap();
}

/// A service of the terminal as `username`, in the server's process: its answer as JSON, and its errors.
fn terminal_service(
    gate: &crate::common::Gate,
    username: &str,
    service: &str,
    parameters: &[(&str, &str)],
) -> (serde_json::Value, String) {
    let context = gate.server.factory().new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(username).unwrap();
    let parameters: std::collections::BTreeMap<String, moqui_lib::object::Value> = parameters
        .iter()
        .map(|(name, value)| (name.to_string(), moqui_lib::object::Value::string(*value)))
        .collect();
    let out = moqui_core::service::sync(&format!("org.moqui.terminal.TerminalServices.{service}"), &parameters);
    let mut errors =
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().join("; ")).unwrap_or_default();
    let out = match out {
        Ok(out) => out,
        Err(err) => {
            errors.push_str(err.message());
            Default::default()
        }
    };
    (
        moqui_lib::json::to_json(&moqui_lib::object::Value::Map(out.into_iter().collect())),
        errors,
    )
}

/// OS accounts (plans/Terminal-Fancy2.md §4.6): with run-as="person", a user with no account is refused host sessions;
/// only an administrator with TERMINAL_HOST names one, and naming one again replaces it.
#[test]
fn run_as_person_refuses_a_user_with_no_os_account_and_an_administrator_names_one() {
    if !crate::common::own_process() {
        return;
    }
    let helper = helper_property();
    let gate = crate::common::start_with_conf_files(
        "termws-runas",
        Some(PLAIN_USER),
        r#"<terminal run-as="person"><host-profile name="plain" title="Plain sh" command="sh"/></terminal>"#,
        &[("moqui_conf__instance__purpose", "dev"), (helper.0, &helper.1)],
        &[],
    );
    let page = Page::signed_in(gate.server.address(), "john.doe");
    gate.server.factory().flush_sessions().unwrap();
    let refused = page.call(
        "open",
        Some(serde_json::json!({"kind": "host", "profileName": "plain"})),
    );
    assert_eq!(refused["reasonCode"], "noOsAccount", "{refused}");
    let (_, errors) = terminal_service(
        &gate,
        "plain.user",
        "store#TerminalOsAccount",
        &[("username", "plain.user"), ("osUsername", "plain")],
    );
    assert!(!errors.is_empty(), "not an administrator");
    let (_, errors) = terminal_service(
        &gate,
        "john.doe",
        "store#TerminalOsAccount",
        &[("username", "john.doe"), ("osUsername", "root")],
    );
    assert!(errors.contains("never root"), "{errors}");
    for account in ["jdoe", "johnd"] {
        let (_, errors) = terminal_service(
            &gate,
            "john.doe",
            "store#TerminalOsAccount",
            &[("username", "john.doe"), ("osUsername", account)],
        );
        assert!(errors.is_empty(), "{errors}");
    }
    let (listed, _) = terminal_service(&gate, "john.doe", "find#TerminalOsAccounts", &[]);
    let accounts = listed["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 1, "{listed}");
    assert_eq!(accounts[0]["osUsername"], "johnd");
    terminal_service(
        &gate,
        "john.doe",
        "expire#TerminalOsAccount",
        &[("username", "john.doe")],
    );
    let (listed, _) = terminal_service(&gate, "john.doe", "find#TerminalOsAccounts", &[]);
    assert!(listed["accounts"].as_array().unwrap().is_empty(), "{listed}");
}

/// A host session as another OS account, live (plans/Terminal-Fancy2.md §4.6): on a Linux server set up for it
/// (`moqui term-exec --print-setup`: the sudoers line for this test binary, and /etc/moqui/run-as.conf allowing the
/// account MOQUI_RUNAS_TEST_ACCOUNT names), the program runs as that account, in its home, and ends when the session is
/// ended. Opt-in: `cargo test -p moqui-host --test it -- --ignored termws::a_host_session_runs_as_another_account`.
#[test]
#[ignore = "needs a Linux server set up for run-as and MOQUI_RUNAS_TEST_ACCOUNT"]
fn a_host_session_runs_as_another_account() {
    if !crate::common::own_process() {
        return;
    }
    let Ok(account) = std::env::var("MOQUI_RUNAS_TEST_ACCOUNT") else {
        panic!("MOQUI_RUNAS_TEST_ACCOUNT names the account /etc/moqui/run-as.conf allows");
    };
    let gate = crate::common::start_with_conf_files(
        "termws-runas-live",
        None,
        r#"<terminal run-as="person"><host-profile name="plain" title="Plain sh" command="sh"/></terminal>"#,
        &[("moqui_conf__instance__purpose", "dev")],
        &[],
    );
    let page = Page::signed_in(gate.server.address(), "john.doe");
    gate.server.factory().flush_sessions().unwrap();
    let (_, errors) = terminal_service(
        &gate,
        "john.doe",
        "store#TerminalOsAccount",
        &[("username", "john.doe"), ("osUsername", &account)],
    );
    assert!(errors.is_empty(), "{errors}");
    let opened = page.call(
        "open",
        Some(serde_json::json!({"kind": "host", "profileName": "plain"})),
    );
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();
    let mut socket = page.attach(&id, None);
    socket
        .send(Message::Binary(b"echo \"as=$(id -un) home=$HOME\"\r".to_vec().into()))
        .unwrap();
    read_messages_until(&mut socket, &format!("as={account} home=")).unwrap();
    page.call("close", Some(serde_json::json!({"sessionId": id})));
    assert!(read_messages_until(&mut socket, "never").is_err(), "it ends when ended");
}

/// The holder (plans/archive/Terminal-Fancy.md §7.5): a host session's program runs on through a restart of the server,
/// and the next window gets its output again and goes on with it.
#[test]
fn a_host_session_outlives_a_restart_of_the_server() {
    if !crate::common::own_process() {
        return;
    }
    let helper = helper_property();
    let gate = crate::common::start_with_conf_properties(
        "termws-hold",
        None,
        r#"<terminal><host-profile name="plain" command="sh"/></terminal>"#,
        &[("moqui_conf__instance__purpose", "dev"), (helper.0, &helper.1)],
    );
    let page = Page::signed_in(gate.server.address(), "john.doe");
    gate.server.factory().flush_sessions().unwrap();
    let options = page.call("options", None);
    let opened = page.call(
        "open",
        Some(serde_json::json!({"kind": "host", "profileName": "plain"})),
    );
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened} {options}"))
        .to_string();
    let mut socket = page.attach(&id, None);
    socket
        .send(Message::Binary(
            b"marker=before-$((40+2)); echo $marker\r".to_vec().into(),
        ))
        .unwrap();
    read_messages_until(&mut socket, "before-42").unwrap();
    assert_eq!(page.call("options", None)["options"]["hostSessionsOutliveServer"], true);
    // recorded from here (plans/Terminal-Fancy2.md §4.5): the holder writes it, so the restart loses nothing of it
    let recorded = page.call("rename", Some(serde_json::json!({"sessionId": id, "record": "Y"})));
    assert_eq!(recorded["session"]["recording"], true, "{recorded}");
    socket
        .send(Message::Binary(b"echo while-$((5*5))\r".to_vec().into()))
        .unwrap();
    read_messages_until(&mut socket, "while-25").unwrap();

    let gate = gate.restart();
    drop(socket);
    let page = Page::signed_in(gate.server.address(), "john.doe");
    gate.server.factory().flush_sessions().unwrap();
    let listed = page.call("sessions", None);
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["id"] == id.as_str()),
        "the session is listed again after the restart: {listed}"
    );
    let mut back = page.attach(&id, None);
    let (texts, _) = read_messages_until(&mut back, "before-42").unwrap();
    assert!(texts[0].contains(r#""reset":true"#), "{texts:?}");
    // the same shell: its variable is still set
    back.send(Message::Binary(b"echo still-$marker\r".to_vec().into()))
        .unwrap();
    read_messages_until(&mut back, "still-before-42").unwrap();
    // still recording after the restart, said so again, and the file holds what came before and after it
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let listed = page.call("sessions", None);
        let recording = listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == id.as_str() && s["recording"] == true);
        if recording {
            break;
        }
        assert!(Instant::now() < deadline, "the recording is shown again: {listed}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let stopped = page.call("rename", Some(serde_json::json!({"sessionId": id, "record": "N"})));
    assert_eq!(stopped["session"]["recording"], false, "{stopped}");
    let recordings = page.call("recordings", None);
    let recording_id = recordings["recordings"][0]["recordingId"]
        .as_str()
        .unwrap_or_else(|| panic!("{recordings}"))
        .to_string();
    let cast = page.get_text(&format!("recording?recordingId={recording_id}"));
    assert!(cast.contains("while-25") && cast.contains("still-before-42"), "{cast}");
    assert!(
        cast.lines().next().unwrap_or_default().contains(r#""version":2"#),
        "{cast}"
    );
    page.call("close", Some(serde_json::json!({"sessionId": id})));
    assert!(read_messages_until(&mut back, "never").is_err());
}

/// Attention (plans/archive/Terminal-Fancy.md §11): bash with the shell integration says when a command runs and how it
/// ended, and a session that asks for its user while the page does not look at it sends a TerminalAttention
/// notification.
#[test]
fn a_session_says_its_status_and_notifies_when_nobody_looks() {
    if !crate::common::own_process() {
        return;
    }
    let helper = helper_property();
    let gate = crate::common::start_with_conf_properties(
        "termws-attention",
        None,
        r#"<terminal><host-profile name="plain" command="sh"/>
            <host-profile name="ib" command="bash" integration="true"/></terminal>"#,
        &[("moqui_conf__instance__purpose", "dev"), (helper.0, &helper.1)],
    );
    let page = Page::signed_in(gate.server.address(), "john.doe");
    gate.server.factory().flush_sessions().unwrap();
    let capture = moqui_core::notify::capture::NotificationCapture::of(gate.server.factory());
    capture.enable().unwrap();

    // the integration is bash's only where the server gives it one: not macOS's own /bin/bash (3.2), which it leaves alone
    if integrated_bash_on_path() {
        let opened = page.call("open", Some(serde_json::json!({"kind": "host", "profileName": "ib"})));
        let id = opened["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("{opened}"))
            .to_string();
        let mut socket = page.attach(&id, None);
        read_messages_until(&mut socket, "\x1b]133;A").unwrap();
        socket.send(Message::Binary(b"false\r".to_vec().into())).unwrap();
        let (texts, bytes) = read_messages_until(&mut socket, "\x1b]133;D;1").unwrap();
        assert!(
            String::from_utf8_lossy(&bytes).contains("\x1b]133;C"),
            "the command's start is marked"
        );
        let mut states = texts;
        while !states.iter().any(|text| text.contains(r#""status":"error""#)) {
            states.extend(read_messages_until(&mut socket, "\x1b]133;A").unwrap().0);
        }
        page.call("close", Some(serde_json::json!({"sessionId": id})));
    }

    let opened = page.call(
        "open",
        Some(serde_json::json!({"kind": "host", "profileName": "plain"})),
    );
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();
    let mut socket = page.attach(&id, None);
    socket
        .send(Message::Text(r#"{"type":"view","visible":false}"#.into()))
        .unwrap();
    socket
        .send(Message::Binary(
            b"printf '\\033]7501;state=blocked;kind=permission\\007'\r"
                .to_vec()
                .into(),
        ))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let sent = capture.messages(Some("TerminalAttention"));
        if let Some(message) = sent.first() {
            let text = moqui_lib::json::to_json(message).to_string();
            assert!(text.contains("is waiting") && text.contains(&id), "{text}");
            break;
        }
        assert!(Instant::now() < deadline, "no TerminalAttention notification");
        let _ = socket.read();
    }
    let listed = page.call("sessions", None);
    let session = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["id"] == id.as_str())
        .cloned()
        .unwrap();
    assert_eq!(
        (session["status"].clone(), session["unseen"].clone()),
        (serde_json::json!("blocked"), serde_json::json!(true))
    );
    // looking at it again clears unseen
    socket
        .send(Message::Text(r#"{"type":"view","visible":true}"#.into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while page.call("sessions", None)["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|session| session["unseen"] == true)
    {
        assert!(Instant::now() < deadline, "still unseen");
        let _ = socket.read();
    }
}

/// Whether the first `bash` on the `PATH` is one the server integrates: any but macOS's own `/bin/bash`, as
/// `term::host` decides on the resolved program.
fn integrated_bash_on_path() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join("bash"))
        .find(|candidate| candidate.is_file())
        .is_some_and(|bash| !(cfg!(target_os = "macos") && bash == std::path::Path::new("/bin/bash")))
}

/// The Desktop and Mobile apps' way in (plans/archive/Terminal-Fancy.md §12): the host signs requests and the socket in
/// by the device's login key, with no session to keep a sign-in in. From another machine a command line then takes a
/// ticket of confirm#TerminalUser (the password, in that one request) at its opening and at its socket's attach.
#[test]
fn a_device_key_opens_a_remote_command_line_with_a_ticket() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_conf_properties(
        "termws-key",
        None,
        r#"<terminal remote="true"/>"#,
        &[
            ("moqui_conf__instance__purpose", "dev"),
            (
                "moqui_conf__webapp_list__webapp__WEBROOT__client_ip_header",
                "X-Real-IP",
            ),
        ],
    );
    let address = gate.server.address();
    let key = {
        let context = gate.server.factory().new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        moqui_core::user::get_login_key(None).unwrap()
    };
    // a believed proxy says the client is another machine, over HTTPS
    let remote = [("X-Real-IP", "198.51.100.20"), ("X-Forwarded-Proto", "https")];
    let call = |name: &str, body: serde_json::Value| -> serde_json::Value {
        let headers: Vec<(&str, &str)> = [("login_key", key.as_str()), ("Content-Type", "application/json")]
            .into_iter()
            .chain(remote)
            .collect();
        let (status, _, text) = crate::common::exchange(
            address,
            &crate::common::http_request("POST", &format!("/apps/Terminal/{name}"), &headers, &body.to_string()),
        );
        assert_eq!(status, 200, "{name}: {text}");
        serde_json::from_str(&text).unwrap_or_else(|err| panic!("{name}: {err}: {text}"))
    };
    let refused = call("open", serde_json::json!({"kind": "command"}));
    assert_eq!(refused["reasonCode"], "reauthRequired", "{refused}");
    let wrong = call("confirm", serde_json::json!({"password": "not it"}));
    assert!(wrong["ticket"].is_null(), "{wrong}");
    let confirmed = call("confirm", serde_json::json!({"password": "moqui"}));
    let ticket = confirmed["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("{confirmed}"))
        .to_string();
    let opened = call("open", serde_json::json!({"kind": "command", "ticket": ticket}));
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();

    let socket = |ticket: &str| {
        let uri: tungstenite::http::Uri = format!("ws://{address}/termws?session={id}&ticket={ticket}")
            .parse()
            .unwrap();
        let mut request = ClientRequestBuilder::new(uri).with_header("login_key", key.as_str());
        for (name, value) in remote {
            request = request.with_header(name, value);
        }
        let (mut socket, response) = connect(request).unwrap();
        assert_eq!(response.status().as_u16(), 101);
        if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
            stream.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        }
        let attach = serde_json::json!({"type": "attach", "cols": 100, "rows": 30, "have": null});
        socket.send(Message::Text(attach.to_string().into())).unwrap();
        socket
    };
    // no ticket: the attach asks for a recent sign-in
    assert_eq!(
        read_messages_until(&mut socket("none"), "never").unwrap_err(),
        "reauthRequired"
    );
    let mut attached = socket(&ticket);
    read_messages_until(&mut attached, "moqui> ").unwrap();
}

/// System › Terminal sessions (plans/archive/Terminal-Fancy.md P9): an administrator lists every user's sessions and
/// ends another user's, which is an audit row; a plain user may do neither.
#[test]
fn an_administrator_lists_and_ends_another_users_session() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_conf_properties("termws-admin", Some(PLAIN_USER), "", &[]);
    let address = gate.server.address();
    let plain = Page::signed_in(address, "plain.user");
    gate.server.factory().flush_sessions().unwrap();
    let opened = plain.call("open", Some(serde_json::json!({"kind": "apps"})));
    let id = opened["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{opened}"))
        .to_string();

    let call = |username: &str, service: &str, parameters: &[(&str, &str)]| {
        let context = gate.server.factory().new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user(username).unwrap();
        let parameters: std::collections::BTreeMap<String, moqui_lib::object::Value> = parameters
            .iter()
            .map(|(name, value)| (name.to_string(), moqui_lib::object::Value::string(*value)))
            .collect();
        let out = moqui_core::service::sync(&format!("org.moqui.terminal.TerminalServices.{service}"), &parameters);
        let mut errors = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().join("; "))
            .unwrap_or_default();
        let out = match out {
            Ok(out) => out,
            Err(err) => {
                errors.push_str(err.message());
                Default::default()
            }
        };
        (
            moqui_lib::json::to_json(&moqui_lib::object::Value::Map(out.into_iter().collect())),
            errors,
        )
    };
    let (refused, errors) = call("plain.user", "find#AllTerminalSessions", &[]);
    assert!(
        !errors.is_empty() && refused["sessions"].is_null(),
        "{refused} {errors}"
    );
    let (listed, errors) = call("john.doe", "find#AllTerminalSessions", &[]);
    let row = listed["sessions"]
        .as_array()
        .unwrap_or_else(|| panic!("{listed} {errors}"))
        .iter()
        .find(|row| row["id"] == id.as_str())
        .cloned();
    assert_eq!(
        row.map(|row| row["username"].clone()),
        Some(serde_json::json!("plain.user")),
        "{listed}"
    );
    let (_, errors) = call(
        "john.doe",
        "end#TerminalSessionOf",
        &[("sessionId", &id), ("reason", "test")],
    );
    assert!(errors.is_empty(), "{errors}");
    let audited = {
        let context = gate.server.factory().new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        gate.server
            .factory()
            .entity()
            .unwrap()
            .find("moqui.entity.EntityAuditLog")
            .unwrap()
            .eq(
                "changedEntityName",
                moqui_lib::object::Value::string("moqui.terminal.TerminalSession"),
            )
            .eq("pkPrimaryValue", moqui_lib::object::Value::string(&id))
            .count()
            .unwrap()
    };
    assert_eq!(audited, 1, "the end of another user's session is an audit row");
    let deadline = Instant::now() + Duration::from_secs(10);
    while plain.call("sessions", None)["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == id.as_str())
    {
        assert!(Instant::now() < deadline, "the session did not end");
        std::thread::sleep(Duration::from_millis(50));
    }
}
