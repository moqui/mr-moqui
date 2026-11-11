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

//! The node's web app reached in this process, with no TCP between: for a program that hosts the node, as the
//! desktop app's window does through its app scheme (plans/Mobile-Desktop.md, configuration 2 and "One transport
//! seam"). A request goes through the same checks and router as one from a listener ([`crate::http::serve_request`]),
//! from the loopback address; a socket (`/notws`, `/callws`, `/termws`) is an HTTP/1.1 connection over an in-memory
//! pipe, served as a listener's connection is, with a WebSocket client on the other end.
//!
//! Every server has one, with ports or without ([`crate::start_embedded_with`]). It holds the app only while the
//! server runs: once the listeners stop, a request is refused, so the factory is not kept past its shutdown.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use moqui_lib::error::Error;
use tokio_tungstenite::tungstenite;

use crate::http::{http1_builder, run_connection, serve_request, Http, Peer, Serving, WebApp};

/// The address a request in process comes from.
const LOOPBACK: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

/// The size of a socket's in-memory pipe, each way.
const PIPE: usize = 64 * 1024;

/// The server's web app in this process. Cheap to clone; every clone is the same app.
#[derive(Clone)]
pub struct InProcess {
    live: Arc<Mutex<Option<Live>>>,
}

#[derive(Clone)]
struct Live {
    web: WebApp,
    handle: tokio::runtime::Handle,
    closing: tokio::sync::watch::Receiver<bool>,
}

/// The status and headers of an answer, with its body still to read: whole ([`Answer::bytes`]) or streamed
/// ([`Answer::copy_to`]).
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    body: axum::body::Body,
    handle: tokio::runtime::Handle,
}

/// What the holder of a socket hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketEvent {
    Open,
    Text(String),
    /// A binary frame (the terminal's, `/termws`).
    Binary(Vec<u8>),
    /// Closed, by the server or because it stopped.
    Closed,
    /// Refused at the handshake (401: no session), or closed for authentication (1008).
    Unauthorized,
}

/// Where a socket's events go. It may be called after the [`LocalSocket`] is dropped.
pub type SocketSink = Box<dyn Fn(SocketEvent) + Send + Sync>;

/// A socket held in process. Dropping it closes the socket.
pub struct LocalSocket {
    outbox: tokio::sync::mpsc::UnboundedSender<tungstenite::Message>,
    _close: tokio::sync::oneshot::Sender<()>,
}

impl LocalSocket {
    /// Send a text frame; one sent after the socket closed is dropped.
    pub fn send(&self, text: String) {
        let _ = self.outbox.send(tungstenite::Message::Text(text.into()));
    }

    /// Send a binary frame; one sent after the socket closed is dropped.
    pub fn send_binary(&self, bytes: Vec<u8>) {
        let _ = self.outbox.send(tungstenite::Message::Binary(bytes.into()));
    }
}

impl InProcess {
    pub(crate) fn new(
        web: WebApp,
        handle: tokio::runtime::Handle,
        closing: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            live: Arc::new(Mutex::new(Some(Live { web, handle, closing }))),
        }
    }

    /// The listeners stopped: let the app go.
    pub(crate) fn stop(&self) {
        moqui_lib::sync::lock(&self.live).take();
    }

    fn live(&self) -> Result<Live, Error> {
        moqui_lib::sync::lock(&self.live)
            .clone()
            .ok_or_else(|| Error::new("the node has stopped"))
    }

    /// Serve one request: `path` with its query, the request's headers (the host's own, such as `Host` and `Cookie`)
    /// and body. Blocks until the answer's head is there; call it from a plain thread, not an async task.
    ///
    /// # Errors
    ///
    /// Returns an error when the node has stopped, or the method, path or a header is not valid HTTP.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<Answer, Error> {
        let live = self.live()?;
        let mut builder = axum::http::Request::builder().method(method).uri(path);
        for (name, value) in headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let request = builder
            .body(axum::body::Body::from(body))
            .map_err(|err| Error::new(format!("{method} {path}: {err}")))?;
        let peer = local_peer();
        let response = match live.handle.block_on(serve_request(&live.web, request, &peer)) {
            Ok(response) => response,
            Err(never) => match never {},
        };
        let (parts, body) = response.into_parts();
        let headers = parts
            .headers
            .iter()
            .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_string())))
            .collect();
        Ok(Answer {
            status: parts.status.as_u16(),
            headers,
            body,
            handle: live.handle,
        })
    }

    /// Open a socket at `path` (with its query) with the host's `headers` (`Host`, `Cookie`, `Origin`), on the node's
    /// runtime; its events go to `sink`, the first `Open`, or `Unauthorized` when it is refused.
    ///
    /// # Errors
    ///
    /// Returns an error when the node has stopped or the path is not valid.
    pub fn open_socket(
        &self,
        path: &str,
        headers: &[(String, String)],
        sink: SocketSink,
    ) -> Result<LocalSocket, Error> {
        let live = self.live()?;
        let host = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("host"))
            .map_or("localhost", |(_, value)| value.as_str());
        let mut request = tungstenite::ClientRequestBuilder::new(
            format!("ws://{host}{path}")
                .parse()
                .map_err(|err| Error::new(format!("{path}: {err}")))?,
        );
        for (name, value) in headers.iter().filter(|(name, _)| !name.eq_ignore_ascii_case("host")) {
            request = request.with_header(name.as_str(), value.as_str());
        }
        let (client, server) = tokio::io::duplex(PIPE);
        let serving = Serving {
            remote: LOOPBACK,
            builder: http1_builder(&live.web.limits),
            web: live.web,
            closing: live.closing,
            exporter: None,
            hold: None,
            via: moqui_core::web::RequestVia::InProcess,
        };
        live.handle.spawn(run_connection(server, false, Http::One, serving));
        let (outbox, mut queued) = tokio::sync::mpsc::unbounded_channel::<tungstenite::Message>();
        let (close, mut closed) = tokio::sync::oneshot::channel::<()>();
        live.handle.spawn(async move {
            let mut socket = match tokio_tungstenite::client_async(request, client).await {
                Ok((socket, _)) => socket,
                Err(tungstenite::Error::Http(response)) if response.status().as_u16() == 401 => {
                    sink(SocketEvent::Unauthorized);
                    return;
                }
                Err(err) => {
                    tracing::debug!("An in-process socket did not open: {err}");
                    sink(SocketEvent::Closed);
                    return;
                }
            };
            sink(SocketEvent::Open);
            let ended = loop {
                tokio::select! {
                    _ = &mut closed => {
                        let _ = socket.close(None).await;
                        return;
                    }
                    Some(message) = queued.recv() => {
                        if socket.send(message).await.is_err() {
                            break SocketEvent::Closed;
                        }
                    }
                    frame = socket.next() => match frame {
                        Some(Ok(tungstenite::Message::Text(text))) => sink(SocketEvent::Text(text.as_str().to_string())),
                        Some(Ok(tungstenite::Message::Binary(bytes))) => sink(SocketEvent::Binary(bytes.to_vec())),
                        Some(Ok(tungstenite::Message::Close(frame))) => {
                            let auth = frame.is_some_and(|frame| {
                                u16::from(frame.code) == 1008 && frame.reason.as_str().contains("authentication")
                            });
                            break if auth { SocketEvent::Unauthorized } else { SocketEvent::Closed };
                        }
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => break SocketEvent::Closed,
                    },
                }
            };
            sink(ended);
        });
        Ok(LocalSocket { outbox, _close: close })
    }
}

impl Answer {
    /// A header of the answer, by any case of its name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The whole body. Blocks; from a plain thread.
    ///
    /// # Errors
    ///
    /// Returns the body's error when it does not finish.
    pub fn bytes(self) -> Result<Vec<u8>, Error> {
        let whole = self.handle.block_on(axum::body::to_bytes(self.body, usize::MAX));
        whole
            .map(|bytes| bytes.to_vec())
            .map_err(|err| Error::new(err.to_string()))
    }

    /// Stream the body to `out`, a frame at a time; answers the bytes written. Blocks; from a plain thread.
    ///
    /// # Errors
    ///
    /// Returns the body's or the writer's error.
    pub fn copy_to(self, out: &mut dyn std::io::Write) -> Result<u64, Error> {
        let Self { body, handle, .. } = self;
        let mut stream = body.into_data_stream();
        let mut written = 0u64;
        while let Some(data) = handle.block_on(stream.next()) {
            let data = data.map_err(|err| Error::new(err.to_string()))?;
            out.write_all(&data).map_err(|err| Error::new(err.to_string()))?;
            written += data.len() as u64;
        }
        Ok(written)
    }
}

/// The peer of a request in process: the loopback address, plain HTTP.
fn local_peer() -> Peer {
    Peer {
        remote: LOOPBACK,
        client: LOOPBACK.ip(),
        secure: false,
        https: false,
        exporter: None,
        via: moqui_core::web::RequestVia::InProcess,
    }
}

#[cfg(test)]
#[path = "in_process_tests.rs"]
mod tests;
