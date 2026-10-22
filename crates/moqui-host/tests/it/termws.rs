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

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

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

    // The shell's config names the Terminal screen, and the screen is the component that opens the socket.
    let get = |path: &str| {
        crate::common::exchange(
            address,
            &crate::common::http_request("GET", path, &[("Cookie", &cookie)], ""),
        )
    };
    let (status, _, shell) = get("/qapps");
    assert_eq!(status, 200, "{shell}");
    assert!(shell.contains(r#""terminalUrl":"/qapps/Terminal""#), "{shell}");
    let (status, _, component) = terminal_screen(address, &cookie);
    assert_eq!(status, 200, "{component}");
    assert!(
        component.contains("/termws") && component.contains("/libs/xterm/xterm.js"),
        "{component}"
    );
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
