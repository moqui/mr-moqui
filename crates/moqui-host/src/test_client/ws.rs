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

//! WebSocket of a client spec (plans/archive/ClientTestHarness.md 5.4), over tungstenite as `moqui tui --server` has it. The
//! session's cookies go in the handshake. A refused handshake is an answer, not an error: the security proofs check
//! the refusals.

use std::net::TcpStream;
use std::time::Duration;

use serde_json::{json, Value};

use super::http::{Server, Session};
use super::raw::Duplex;

/// An open socket.
pub(crate) struct Socket {
    socket: tungstenite::WebSocket<Box<dyn Duplex>>,
    /// The TCP stream under it, for the read timeout of `receive`.
    tcp: TcpStream,
    closed: bool,
}

/// What `ws.connect` answers, and the socket when the handshake was accepted.
pub(crate) struct Connected {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) socket: Option<Socket>,
}

impl Connected {
    pub(crate) fn to_json(&self, id: Option<u64>) -> Value {
        let mut headers = serde_json::Map::new();
        for (name, value) in &self.headers {
            let name = name.to_ascii_lowercase();
            let joined = match headers.get(&name).and_then(Value::as_str) {
                Some(before) => format!("{before}, {value}"),
                None => value.clone(),
            };
            headers.insert(name, Value::String(joined));
        }
        json!({ "id": id, "status": self.status, "open": self.socket.is_some(), "headers": headers })
    }
}

/// The handshake for `path` on the server, with the session's cookies, the headers and the `Origin` given.
pub(crate) fn connect(
    server: &Server,
    session: &Session,
    path: &str,
    headers: &[(String, String)],
    origin: Option<&str>,
    insecure: bool,
    timeout: Duration,
) -> Result<Connected, String> {
    let http_url = server.join(path);
    let ws_url = if let Some(rest) = http_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = http_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        http_url.clone()
    };
    let uri: tungstenite::http::Uri = ws_url.parse().map_err(|err| format!("{ws_url}: {err}"))?;
    let mut request = tungstenite::ClientRequestBuilder::new(uri);
    let given = |name: &str| headers.iter().any(|(key, _)| key.eq_ignore_ascii_case(name));
    for (name, value) in &session.headers {
        if !given(name) {
            request = request.with_header(name.clone(), value.clone());
        }
    }
    for (name, value) in headers {
        request = request.with_header(name.clone(), value.clone());
    }
    if let Some(origin) = origin {
        request = request.with_header("Origin", origin);
    }
    if !given("cookie") {
        if let Some(cookie) = session.cookie_header(&http_url) {
            request = request.with_header("Cookie", cookie);
        }
    }
    let (stream, tcp) = super::raw::connect(server, insecure, timeout)?;
    match tungstenite::client(request, stream) {
        Ok((socket, response)) => Ok(Connected {
            status: response.status().as_u16(),
            headers: header_list(response.headers()),
            socket: Some(Socket {
                socket,
                tcp,
                closed: false,
            }),
        }),
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response))) => Ok(Connected {
            status: response.status().as_u16(),
            headers: header_list(response.headers()),
            socket: None,
        }),
        Err(err) => Err(format!("{ws_url}: {err}")),
    }
}

fn header_list(headers: &tungstenite::http::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| (name.to_string(), String::from_utf8_lossy(value.as_bytes()).to_string()))
        .collect()
}

impl Socket {
    pub(crate) fn send(&mut self, text: &str) -> Result<(), String> {
        if self.closed {
            return Err("the socket is closed".to_string());
        }
        self.socket
            .send(tungstenite::Message::Text(text.into()))
            .map_err(|err| format!("ws send: {err}"))
    }

    /// The next text frame (a binary one as text), or none on timeout or close.
    pub(crate) fn receive(&mut self, timeout: Duration) -> Result<Option<String>, String> {
        if self.closed {
            return Ok(None);
        }
        self.tcp
            .set_read_timeout(Some(timeout.max(Duration::from_millis(1))))
            .map_err(|err| err.to_string())?;
        loop {
            match self.socket.read() {
                Ok(tungstenite::Message::Text(text)) => return Ok(Some(text.as_str().to_string())),
                Ok(tungstenite::Message::Binary(bytes)) => {
                    return Ok(Some(String::from_utf8_lossy(&bytes).to_string()))
                }
                Ok(tungstenite::Message::Close(_)) => {
                    self.closed = true;
                    return Ok(None);
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(err))
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None)
                }
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    self.closed = true;
                    return Ok(None);
                }
                Err(err) => {
                    self.closed = true;
                    return Err(format!("ws receive: {err}"));
                }
            }
        }
    }

    pub(crate) fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            let _ = self.tcp.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = self.socket.close(None);
            let _ = self.socket.flush();
        }
    }
}
