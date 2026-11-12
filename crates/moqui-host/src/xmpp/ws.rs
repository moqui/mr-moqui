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

//! `GET /xmpp-websocket` (RFC 7395, subprotocol `xmpp`): an XMPP session over a WebSocket of the HTTP listener, for
//! web clients (Converse.js and the like). It signs in with SASL like any client: no cookie, so no origin check is
//! needed. Over this process's own HTTPS the TLS session's exporter, taken at the accept ([`TlsExporter`]), gives
//! SCRAM-SHA-256-PLUS; behind a proxy that ends TLS there is none, and the plain SCRAM mechanism is offered. Exempt
//! from the request deadline, as `/notws` is. The client is the address a trusted proxy names (`ClientAddr`), so
//! failed sign-ins and the connection limits count per client, not against the proxy.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, State};
use axum::http::request::Parts;
use axum::response::Response;

use crate::http::{plain, ClientAddr, HttpState, HttpsRequest};

pub(crate) const PATH: &str = "/xmpp-websocket";

/// The exporter (RFC 9266) of the TLS session a request came over, put on it by the HTTPS listener.
#[derive(Clone)]
pub(crate) struct TlsExporter(pub Arc<Vec<u8>>);

/// The request's TLS facts.
pub(crate) struct Secured {
    https: bool,
    exporter: Option<Vec<u8>>,
}

impl axum::extract::FromRequestParts<Arc<HttpState>> for Secured {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &Arc<HttpState>) -> Result<Self, Self::Rejection> {
        Ok(Secured {
            https: parts.extensions.get::<HttpsRequest>().is_some(),
            exporter: parts
                .extensions
                .get::<TlsExporter>()
                .map(|exporter| exporter.0.to_vec()),
        })
    }
}

pub(crate) async fn upgrade(
    ws: WebSocketUpgrade,
    Extension(ClientAddr(client)): Extension<ClientAddr>,
    secured: Secured,
    State(state): State<Arc<HttpState>>,
) -> Response {
    let Some(server) = super::server_of(&state.factory) else {
        return plain(404, "not found");
    };
    if moqui_xmpp::conn::blocked(&server, client) {
        return plain(429, "too many failed sign-ins");
    }
    let Some(admitted) = super::admit_websocket(&state.factory, client) else {
        return plain(429, "too many XMPP connections");
    };
    let limit = server.config.max_stanza_bytes;
    let mut shutdown = state.shutdown.clone();
    ws.protocols(["xmpp"])
        .max_message_size(limit)
        .max_frame_size(limit)
        .on_upgrade(move |socket| async move {
            let frames: Box<dyn moqui_xmpp::conn::FrameIo> = Box::new(Frames(socket));
            // the HTTP listener's stop closes the socket, as it does `/notws`, so its drain does not wait for the
            // session (plans/archive/High-Level-Review.md HL-027); the client reconnects to the server once it is back
            tokio::select! {
                () = moqui_xmpp::conn::serve_frames(server, frames, client, secured.https, secured.exporter) => {}
                _ = shutdown.wait_for(|stopped| *stopped) => {
                    tracing::debug!("XMPP WebSocket from {client} closed: the server is stopping");
                }
            }
            drop(admitted);
        })
}

struct Frames(WebSocket);

impl moqui_xmpp::conn::FrameIo for Frames {
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(async move {
            loop {
                match self.0.recv().await? {
                    Ok(Message::Text(text)) => return Some(text.to_string()),
                    Ok(Message::Ping(payload)) => {
                        if self.0.send(Message::Pong(payload)).await.is_err() {
                            return None;
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => return None,
                    Ok(_) => {}
                }
            }
        })
    }

    fn send(&mut self, text: String) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async move { self.0.send(Message::Text(text.into())).await.is_ok() })
    }
}
