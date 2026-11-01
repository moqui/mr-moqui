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

//! A session over its connection: TCP with STARTTLS (`xmpp_port`), direct TLS with ALPN `xmpp-client`
//! (`xmpp_tls_port`, XEP-0368), or WebSocket frames (`/xmpp-websocket`, RFC 7395, which moqui-host's HTTP listener
//! upgrades). The TLS session's exporter is taken here for channel binding; over WebSocket the host takes it at its
//! HTTPS accept. A connection that drops with resumption on leaves its session detached for the resumption timeout.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_lib::source_guard::Failure;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

use crate::framing::{self, FramingError, StreamReader};
use crate::registry::Inbound;
use crate::scram::EXPORTER_LABEL;
use crate::session::{Out, Session, Transport};
use crate::Server;

/// A connection with nothing from the client for this long is closed (clients ping or send whitespace well within it).
const IDLE: Duration = Duration::from_secs(600);
const TICK: Duration = Duration::from_secs(30);

/// Whether `ip` is blocked for failed sign-ins.
pub fn blocked(server: &Server, ip: IpAddr) -> bool {
    moqui_lib::sync::lock(&server.guard).is_blocked(ip, Instant::now())
}

/// Count a failed sign-in from `ip`; true when the source is now blocked.
pub fn note_failure(server: &Server, ip: IpAddr) -> bool {
    let failure = moqui_lib::sync::lock(&server.guard).failure(ip, Instant::now());
    if failure == Failure::Blocked {
        tracing::warn!("XMPP: {ip} is blocked for a while after failed sign-ins");
    }
    matches!(failure, Failure::Blocked | Failure::StillBlocked)
}

/// The exporter of a TLS 1.3 session (RFC 9266); None before TLS 1.3, where tls-exporter is not defined without the
/// extended master secret.
pub fn exporter(connection: &rustls::ServerConnection) -> Option<Vec<u8>> {
    if connection.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3) {
        return None;
    }
    connection
        .export_keying_material(vec![0_u8; 32], EXPORTER_LABEL, None)
        .ok()
}

/// WebSocket frames, as the host's socket gives them.
pub trait FrameIo: Send {
    /// The next text frame; None when the socket closed.
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>>;
    /// Send a text frame; false when the socket is gone.
    fn send(&mut self, text: String) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
}

enum Io {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
    Gone,
}

impl Io {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Io::Plain(stream) => stream.read(buf).await,
            Io::Tls(stream) => stream.read(buf).await,
            Io::Gone => Ok(0),
        }
    }

    async fn write(&mut self, text: &str) -> std::io::Result<()> {
        match self {
            Io::Plain(stream) => stream.write_all(text.as_bytes()).await,
            Io::Tls(stream) => {
                stream.write_all(text.as_bytes()).await?;
                stream.flush().await
            }
            Io::Gone => Err(std::io::ErrorKind::NotConnected.into()),
        }
    }
}

enum Link {
    Stream(Box<StreamLink>),
    Frames(Box<dyn FrameIo>),
}

struct StreamLink {
    io: Io,
    reader: StreamReader,
    acceptor: TlsAcceptor,
}

/// Serve a TCP connection: STARTTLS first, or TLS at once when `direct`.
pub async fn serve_tcp(server: Arc<Server>, stream: TcpStream, peer: SocketAddr, acceptor: TlsAcceptor, direct: bool) {
    let _ = stream.set_nodelay(true);
    let (io, binding) = if direct {
        match tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream)).await {
            Ok(Ok(tls)) => {
                let binding = exporter(tls.get_ref().1);
                (Io::Tls(Box::new(tls)), binding)
            }
            _ => {
                tracing::debug!("XMPP: TLS handshake with {peer} failed");
                return;
            }
        }
    } else {
        (Io::Plain(stream), None)
    };
    let session = Session::new(Arc::clone(&server), peer.ip(), Transport::Tcp { tls: direct }, binding);
    let link = Link::Stream(Box::new(StreamLink {
        io,
        reader: StreamReader::new(server.config.max_stanza_bytes),
        acceptor,
    }));
    run(server, session, link).await;
}

/// Serve a WebSocket (RFC 7395): `tls` when the request came over HTTPS, `binding` the HTTPS listener's exporter when
/// it was this process's own TLS.
pub async fn serve_frames(
    server: Arc<Server>,
    frames: Box<dyn FrameIo>,
    peer: IpAddr,
    tls: bool,
    binding: Option<Vec<u8>>,
) {
    let session = Session::new(Arc::clone(&server), peer, Transport::WebSocket { tls }, binding);
    run(server, session, Link::Frames(frames)).await;
}

async fn next_inbound(rx: &mut Option<mpsc::Receiver<Inbound>>) -> Option<Inbound> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Write what the session answered; false when the connection is to close.
async fn flush(session: &mut Session, link: &mut Link) -> bool {
    for out in session.take_out() {
        match (out, &mut *link) {
            (Out::Raw(text), Link::Stream(stream)) => {
                if stream.io.write(&text).await.is_err() {
                    return false;
                }
            }
            (Out::Element(element), Link::Stream(stream)) => {
                if stream.io.write(&framing::write(&element)).await.is_err() {
                    return false;
                }
            }
            (Out::Raw(text), Link::Frames(frames)) => {
                if !frames.send(text).await {
                    return false;
                }
            }
            (Out::Element(element), Link::Frames(frames)) => {
                if !frames.send(framing::write(&element)).await {
                    return false;
                }
            }
            (Out::StartTls, Link::Stream(link)) => {
                let Io::Plain(stream) = std::mem::replace(&mut link.io, Io::Gone) else {
                    return false;
                };
                match tokio::time::timeout(Duration::from_secs(10), link.acceptor.accept(stream)).await {
                    Ok(Ok(tls)) => {
                        let binding = exporter(tls.get_ref().1);
                        link.io = Io::Tls(Box::new(tls));
                        session.tls_started(binding);
                    }
                    _ => return false,
                }
            }
            (Out::Reset, Link::Stream(link)) => link.reader.reset(),
            (Out::Close, _) => return false,
            (Out::StartTls | Out::Reset, Link::Frames(_)) => {}
        }
    }
    true
}

async fn run(server: Arc<Server>, mut session: Session, mut link: Link) {
    let mut buf = vec![0_u8; 16 * 1024];
    let mut rx: Option<mpsc::Receiver<Inbound>> = None;
    let mut tick = tokio::time::interval(TICK);
    tick.tick().await;
    let mut last_heard = Instant::now();
    loop {
        if !flush(&mut session, &mut link).await {
            break;
        }
        if let Some(old) = session.auth.swap.take() {
            // a resumed session carries on over this connection
            session = *old;
            rx = session.take_receiver();
            continue;
        }
        if rx.is_none() {
            rx = session.take_receiver();
        }
        if session.is_closed() {
            break;
        }
        let overflow = session.overflow();
        tokio::select! {
            read = read_frames(&mut link, &mut buf) => {
                match read {
                    Read::Frames(frames) => {
                        last_heard = Instant::now();
                        for frame in frames {
                            session.on_frame(frame).await;
                        }
                    }
                    Read::Error(FramingError::TooLarge) => session.fail("policy-violation", Some("stanza too large")),
                    Read::Error(FramingError::NotWellFormed(_)) => session.fail("not-well-formed", None),
                    Read::Gone => break,
                }
            }
            inbound = next_inbound(&mut rx) => match inbound {
                Some(inbound) => session.on_inbound(inbound).await,
                None => rx = None,
            },
            () = overflow.notified() => session.fail("policy-violation", Some("too many stanzas waiting")),
            _ = tick.tick() => {
                if last_heard.elapsed() > IDLE {
                    session.fail("connection-timeout", None);
                } else {
                    session.tick().await;
                }
            }
        }
    }
    if let Some(receiver) = rx {
        session.give_receiver(receiver);
    }
    if !session.is_closed() && session.detach() {
        wait_detached(server, session).await;
    } else {
        session.end().await;
    }
}

enum Read {
    Frames(Vec<framing::Frame>),
    Error(FramingError),
    Gone,
}

async fn read_frames(link: &mut Link, buf: &mut [u8]) -> Read {
    match link {
        Link::Stream(link) => match link.io.read(buf).await {
            Ok(0) | Err(_) => Read::Gone,
            Ok(n) => match link.reader.feed(&buf[..n]) {
                Ok(frames) => Read::Frames(frames),
                Err(err) => Read::Error(err),
            },
        },
        Link::Frames(frames) => match frames.recv().await {
            None => Read::Gone,
            Some(text) => match framing::parse_frame(&text) {
                Ok(frame) => Read::Frames(vec![frame]),
                Err(err) => Read::Error(err),
            },
        },
    }
}

/// A detached session takes its pushes as unacknowledged stanzas until a new connection resumes it or the time is up.
async fn wait_detached(server: Arc<Server>, mut session: Session) {
    let Some(id) = session.resumable() else {
        return session.end().await;
    };
    let user_id = session
        .account
        .as_ref()
        .map(|account| account.user_id.clone())
        .unwrap_or_default();
    let (handover_tx, mut handover_rx) = oneshot::channel();
    server.registry.detach(&id, &user_id, handover_tx);
    let mut rx = session.take_receiver();
    let deadline = tokio::time::sleep(server.config.resume_timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            handover = &mut handover_rx => {
                if let Ok(reply) = handover {
                    if let Some(receiver) = rx.take() {
                        session.give_receiver(receiver);
                    }
                    let _ = reply.send(Box::new(session));
                    return;
                }
                break;
            }
            inbound = next_inbound(&mut rx) => match inbound {
                Some(inbound) => {
                    session.on_inbound(inbound).await;
                    // kept as unacknowledged stanzas for the resumption
                    session.take_out();
                }
                None => rx = None,
            },
            () = &mut deadline => break,
        }
    }
    server.registry.forget_detached(&id);
    session.detached = false;
    session.end().await;
}
