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

//! The server's sockets held by the host for a page (plans/Mobile-Desktop.md §4, "One transport seam"). A page on the
//! app scheme cannot open a WebSocket through it, and a page's WebSocket cannot send the `login_key` header, so the
//! host opens `/notws` (or `/callws`) with the device key on a thread of its own, hands the page each text frame and
//! sends the page's. A dropped socket connects again, waiting longer each time up to a minute with jitter
//! (`moqui_lib::reconnect`), as `moqui tui --server`'s does; a close for authentication (1008, the key revoked or expired) does not.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::reconnect::Backoff;
use moqui_lib::sync::lock;

/// The paths a page may open a socket to.
pub const PATHS: [&str; 2] = ["/notws", "/callws"];

/// How long a read waits before the thread looks for frames to send and for a close.
const POLL: Duration = Duration::from_millis(50);

/// How long the TLS and WebSocket handshakes, and a write, may take: a server that takes the connection and then stalls
/// is dropped, and the thread can see that the host let go.
const STALL: Duration = Duration::from_secs(20);

/// Frames kept for a socket that is not open, as the page's `NotificationClient` keeps them.
const QUEUE_LIMIT: usize = 200;

/// What the page hears of its socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Open,
    Text(String),
    /// Closed; the host connects again.
    Closed,
    /// Closed for authentication (a 401 at the handshake, or `/notws`'s 1008 close), and not connected again. The server
    /// answers so also when it could not check the key (its database down), so before forgetting the key the host
    /// confirms with a request (`login::client_config`, `None` only on a 401).
    Revoked,
}

/// Where the events go: to the page, in the host (the desktop app evaluates each in the page's webview). It can be called briefly after the [`Socket`] is
/// dropped, so it must take an event for a page that is gone.
pub type Sink = Box<dyn Fn(Event) + Send>;

/// A socket the host holds for a page. Dropping it closes the socket and ends its thread.
pub struct Socket {
    outbox: Arc<Mutex<VecDeque<String>>>,
    stop: Arc<AtomicBool>,
}

impl Socket {
    /// Send a text frame, now or once the socket is open; the oldest are dropped past the queue's limit.
    pub fn send(&self, text: String) {
        let mut outbox = lock(&self.outbox);
        if outbox.len() >= QUEUE_LIMIT {
            outbox.pop_front();
        }
        outbox.push_back(text);
    }

    pub fn close(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.close();
    }
}

/// Open `path` (one of [`PATHS`], with a query if it needs one) on `base` with `key`, on a thread of its own.
///
/// # Errors
///
/// Returns an error for a path that is not one of [`PATHS`], or when the thread cannot be started.
pub fn open(base: &str, key: &str, path: &str, sink: Sink) -> Result<Socket, Error> {
    let url = socket_url(base, path)?;
    let outbox = Arc::new(Mutex::new(VecDeque::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let link = Link {
        url,
        key: key.to_string(),
        outbox: Arc::clone(&outbox),
        stop: Arc::clone(&stop),
    };
    std::thread::Builder::new()
        .name("client-socket".to_string())
        .spawn(move || link.run(&sink))
        .map_err(|err| Error::new(format!("socket thread: {err}")))?;
    Ok(Socket { outbox, stop })
}

/// `ws(s)://host:port/<base path><path>` of a page's socket path.
///
/// # Errors
///
/// Returns an error for a path that is not one of [`PATHS`], or a url that does not parse.
pub fn socket_url(base: &str, path: &str) -> Result<String, Error> {
    let bare = path.split_once('?').map_or(path, |(bare, _)| bare);
    if !PATHS.contains(&bare) || path.chars().any(|c| c.is_control() || c == '#') {
        return Err(Error::new(format!("not a socket a page may open: {path}")));
    }
    let url = reqwest::Url::parse(base).map_err(|err| Error::new(format!("{base}: {err}")))?;
    let https = url.scheme() == "https";
    let host = url.host_str().unwrap_or("localhost");
    let port = url.port().unwrap_or(if https { 443 } else { 80 });
    let scheme = if https { "wss" } else { "ws" };
    let text = format!("{scheme}://{host}:{port}{}{path}", url.path().trim_end_matches('/'));
    // the thread parses it again; one that would not parse there is refused here, not retried for ever
    text.parse::<tungstenite::http::Uri>()
        .map_err(|err| Error::new(format!("{text}: {err}")))?;
    Ok(text)
}

/// A stream the socket reads: plain, or TLS for wss.
trait Duplex: Read + Write {}

impl<T: Read + Write> Duplex for T {}

/// How one connection ended.
enum End {
    /// The socket closed or failed: connect again.
    Dropped,
    /// Closed for authentication, or by the host: do not.
    Final,
}

/// The thread's side of a socket.
struct Link {
    url: String,
    key: String,
    outbox: Arc<Mutex<VecDeque<String>>>,
    stop: Arc<AtomicBool>,
}

impl Link {
    fn run(&self, sink: &Sink) {
        let mut backoff = Backoff::new();
        while !self.stopped() {
            let mut opened = false;
            match self.connect_and_serve(sink, &mut opened) {
                Ok(End::Final) => return,
                Ok(End::Dropped) => {}
                Err(err) => tracing::debug!("Client socket {}: {err}", self.url),
            }
            // a socket that was open starts its waits over: the next drop is new trouble, not the same
            if opened {
                backoff.reset();
            }
            if self.stopped() {
                return;
            }
            sink(Event::Closed);
            let until = Instant::now() + backoff.next_wait(moqui_lib::reconnect::jitter());
            while !self.stopped() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// One connection until it closes or the host lets go; `opened` says whether it got as far as open.
    fn connect_and_serve(&self, sink: &Sink, opened: &mut bool) -> Result<End, String> {
        let uri: tungstenite::http::Uri = self
            .url
            .parse()
            .map_err(|err: tungstenite::http::uri::InvalidUri| err.to_string())?;
        // an IPv6 literal comes in brackets
        let host = uri
            .host()
            .unwrap_or("localhost")
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let https = uri.scheme_str() == Some("wss");
        let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
        let tcp = connect(&host, port)?;
        tcp.set_read_timeout(Some(STALL)).map_err(|err| err.to_string())?;
        tcp.set_write_timeout(Some(STALL)).map_err(|err| err.to_string())?;
        let stream: Box<dyn Duplex + Send> = if https {
            // the OS trust store, as reqwest checks the requests with: a private CA that signs in also opens the socket
            use rustls_platform_verifier::BuilderVerifierExt;
            let config =
                rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
                    .with_safe_default_protocol_versions()
                    .map_err(|err| err.to_string())?
                    .with_platform_verifier()
                    .map_err(|err| err.to_string())?
                    .with_no_client_auth();
            let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|err| err.to_string())?;
            let connection = rustls::ClientConnection::new(Arc::new(config), name).map_err(|err| err.to_string())?;
            Box::new(rustls::StreamOwned::new(
                connection,
                tcp.try_clone().map_err(|err| err.to_string())?,
            ))
        } else {
            Box::new(tcp.try_clone().map_err(|err| err.to_string())?)
        };
        let request = tungstenite::ClientRequestBuilder::new(uri).with_header("login_key", self.key.clone());
        let mut socket = match tungstenite::client(request, stream) {
            Ok((socket, _)) => socket,
            // the key no longer signs in
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)))
                if response.status().as_u16() == 401 =>
            {
                sink(Event::Revoked);
                return Ok(End::Final);
            }
            Err(tungstenite::HandshakeError::Failure(err)) => return Err(err.to_string()),
            Err(tungstenite::HandshakeError::Interrupted(_)) => return Err("handshake interrupted".to_string()),
        };
        // after the handshake, so it is not cut short by the poll
        tcp.set_read_timeout(Some(POLL)).map_err(|err| err.to_string())?;
        *opened = true;
        sink(Event::Open);
        loop {
            if self.stopped() {
                let _ = socket.close(None);
                let _ = socket.flush();
                return Ok(End::Final);
            }
            let pending: Vec<String> = lock(&self.outbox).drain(..).collect();
            let mut pending = pending.into_iter();
            while let Some(text) = pending.next() {
                if let Err(err) = socket.send(tungstenite::Message::Text(text.clone().into())) {
                    // not sent: kept, in order, for the next connection
                    let mut outbox = lock(&self.outbox);
                    for (at, unsent) in std::iter::once(text).chain(pending).enumerate() {
                        outbox.insert(at, unsent);
                    }
                    outbox.truncate(QUEUE_LIMIT);
                    return Err(err.to_string());
                }
            }
            match socket.read() {
                Ok(tungstenite::Message::Text(text)) => sink(Event::Text(text.as_str().to_string())),
                Ok(tungstenite::Message::Close(frame)) => {
                    // the reply to the close goes out before the socket drops
                    let _ = socket.flush();
                    if frame.is_some_and(|frame| revoked(u16::from(frame.code), frame.reason.as_str())) {
                        sink(Event::Revoked);
                        return Ok(End::Final);
                    }
                    return Ok(End::Dropped);
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(err))
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(err) => return Err(err.to_string()),
            }
        }
    }
}

/// A TCP connection to `host`, trying each of its addresses for [`crate::http::CONNECT_SECONDS`].
fn connect(host: &str, port: u16) -> Result<std::net::TcpStream, String> {
    use std::net::ToSocketAddrs;
    let timeout = Duration::from_secs(crate::http::CONNECT_SECONDS);
    let mut last = format!("{host}: no address");
    for address in (host, port).to_socket_addrs().map_err(|err| format!("{host}: {err}"))? {
        match std::net::TcpStream::connect_timeout(&address, timeout) {
            Ok(tcp) => return Ok(tcp),
            Err(err) => last = format!("{address}: {err}"),
        }
    }
    Err(last)
}

/// A close for authentication: `/notws` closes with 1008 and this reason when the key no longer signs in
/// (`crates/moqui-host/src/notws.rs`); its other 1008, a full queue, is a drop to connect again after.
fn revoked(code: u16, reason: &str) -> bool {
    code == 1008 && reason.contains("authentication")
}

#[cfg(test)]
#[path = "socket_tests.rs"]
mod tests;
