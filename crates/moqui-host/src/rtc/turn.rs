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

//! The embedded TURN server's sockets (plans/archive/Comms-Framework.md §6.5, W4; docs/interfaces/TURN.md), on the media node:
//! UDP and TCP on `turn_port`, TLS on `turn_tls_port` and on 443 of `turn_tls443_host` (an address of its own, since the
//! HTTPS listener does not share its port), all off while their setting is empty, and the UDP relay sockets in
//! `turn_relay_ports`. One thread owns the [`Turn`] and the UDP sockets, and waits on its bounded input queue when
//! there is nothing to do; the TCP and TLS listeners and each of their connections are tasks on the edge runtime
//! ([`crate::edge::handle`]), which read a stream a message at a time for the server and write back what it sends. TLS
//! uses the host's certificates (`runtime/cert`).
//!
//! The TCP and TLS connections are bounded ([`StreamLimits`], plans/archive/High-Level-Review.md HL-031): at most
//! `max_connections` at once and `per_source` from one source (an IPv6 /64 is one); one must send a whole message
//! (after its TLS handshake) within `handshake`, and one with nothing read or written for `idle` is closed; and what
//! waits to be written to a client is at most `max_queued` messages, past which the client is not reading and its
//! connection is closed (its allocations end with it, as on any close). A client that sends faster than the server
//! takes its messages waits on its own connection, since the server's queue is bounded. A task never blocks: a write
//! that does not finish in [`WRITE_TIMEOUT`] closes its connection. Dropping the [`TurnNode`] closes every connection.
//! The media node revokes a leg's credential when the leg leaves ([`TurnNode::revoke`]). On a node that takes links
//! the active pairings are handed in as they change ([`TurnNode::set_pairings`], E16): a local node's legs are signed
//! with its pairing's own secret, which ends with the pairing.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_lib::source_guard::SourceSlots;
use moqui_rtc::turn::{frame_len, Turn, TurnConfig, TurnOutput};
use moqui_rtc::Proto;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_rustls::TlsAcceptor;

/// The most bytes a stream may buffer without a whole message.
const MAX_STREAM_BUFFER: usize = 64 * 1024;

/// The messages from the TCP and TLS connections waiting for the server's thread.
const MAX_INPUT: usize = 4_096;

/// How often the server's thread reads its UDP sockets while it has nothing else to do, and how often a connection
/// tries a full input queue again.
const POLL_UDP: Duration = Duration::from_millis(1);

/// The longest the server's thread waits on its queue with no UDP socket open, so it sees a stop.
const MAX_WAIT: Duration = Duration::from_millis(50);

/// How long a write to a client may take before its connection is closed.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// The bounds of the TCP and TLS client connections.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamLimits {
    /// Connections at once, over every TCP and TLS listener.
    pub max_connections: usize,
    /// Of `max_connections`, those from one source.
    pub per_source: usize,
    /// How long a connection has to send its first whole message, its TLS handshake included.
    pub handshake: Duration,
    /// How long a connection may go with nothing read or written.
    pub idle: Duration,
    /// The messages waiting to be written to one client; one more closes its connection.
    pub max_queued: usize,
}

impl Default for StreamLimits {
    fn default() -> Self {
        StreamLimits {
            max_connections: 512,
            per_source: 16,
            handshake: Duration::from_secs(10),
            // an allocation is refreshed within its 10-minute lifetime, and media keeps a busy one going
            idle: Duration::from_secs(10 * 60),
            max_queued: 512,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TurnSettings {
    pub bind: IpAddr,
    pub port: Option<u16>,
    pub tls_port: Option<u16>,
    pub tls443: Option<IpAddr>,
    pub public_host: String,
    pub public_ip: IpAddr,
    pub relay_ports: (u16, u16),
    pub config: TurnConfig,
    pub limits: StreamLimits,
}

fn port(text: &str, name: &str) -> Result<Option<u16>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse()
        .map(Some)
        .map_err(|_| format!("{name} {text} is not a port"))
}

/// The TURN settings, None while neither `turn_port` nor `turn_tls_port` nor `turn_tls443_host` is set.
pub(crate) fn settings(
    property: &dyn Fn(&str) -> String,
    media_bind: SocketAddr,
    media_public: SocketAddr,
    extra_peers: &[SocketAddr],
) -> Result<Option<TurnSettings>, String> {
    let udp = port(&property("turn_port"), "turn_port")?;
    let tls_port = port(&property("turn_tls_port"), "turn_tls_port")?;
    let tls443 = property("turn_tls443_host");
    let tls443 = tls443.trim();
    let tls443: Option<IpAddr> = if tls443.is_empty() {
        None
    } else {
        Some(
            tls443
                .parse()
                .map_err(|_| format!("turn_tls443_host {tls443} is not an address"))?,
        )
    };
    if udp.is_none() && tls_port.is_none() && tls443.is_none() {
        return Ok(None);
    }
    let bind = property("turn_host");
    let bind = if bind.trim().is_empty() {
        media_bind.ip()
    } else {
        bind.trim()
            .parse()
            .map_err(|_| format!("turn_host {} is not an address", bind.trim()))?
    };
    let public_host = property("turn_public_host");
    let public_host = if public_host.trim().is_empty() {
        media_public.ip().to_string()
    } else {
        public_host.trim().to_string()
    };
    let public_ip = public_host
        .parse::<IpAddr>()
        .ok()
        .or_else(|| {
            (public_host.as_str(), 0)
                .to_socket_addrs()
                .ok()
                .and_then(|mut a| a.next())
                .map(|a| a.ip())
        })
        .unwrap_or(media_public.ip());
    let range = property("turn_relay_ports");
    let range = if range.trim().is_empty() {
        "49152-65535".to_string()
    } else {
        range.trim().to_string()
    };
    let (low, high) = range
        .split_once('-')
        .ok_or_else(|| format!("turn_relay_ports {range} is not a range low-high"))?;
    let relay_ports: (u16, u16) = (
        low.trim()
            .parse()
            .map_err(|_| format!("turn_relay_ports {range} is not a range"))?,
        high.trim()
            .parse()
            .map_err(|_| format!("turn_relay_ports {range} is not a range"))?,
    );
    if relay_ports.0 == 0 || relay_ports.0 > relay_ports.1 {
        return Err(format!("turn_relay_ports {range} is not a range"));
    }
    let secret = property("turn_auth_secret");
    if secret.trim().is_empty() || secret.ends_with("CHANGEME") {
        return Err("turn_auth_secret is not set".to_string());
    }
    let number = |name: &str, default: u64| property(name).trim().parse::<u64>().unwrap_or(default);
    // the media port only, not every port of the media node's address
    let mut allowed_peers = vec![media_public];
    if media_bind != media_public && !media_bind.ip().is_unspecified() {
        allowed_peers.push(media_bind);
    }
    allowed_peers.extend_from_slice(extra_peers);
    // another media node: `address:port`, or an address alone on this node's media port
    for extra in property("turn_allowed_peers")
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let peer = extra
            .parse::<SocketAddr>()
            .ok()
            .or_else(|| {
                extra
                    .parse::<IpAddr>()
                    .ok()
                    .map(|ip| SocketAddr::new(ip, media_public.port()))
            })
            .ok_or_else(|| format!("turn_allowed_peers {extra} is not an address"))?;
        allowed_peers.push(peer);
    }
    let config = TurnConfig {
        realm: public_host.clone(),
        secret: secret.trim().as_bytes().to_vec(),
        allowed_peers,
        max_allocations: usize::try_from(number("turn_max_allocations", 1_000)).unwrap_or(1_000),
        max_per_user: 2,
        max_kbps: number("turn_max_kbps", 4_000),
        // a node that takes links: its paired nodes' usernames name their pairings (E16)
        pairings: moqui_core::edge::NodeRole::parse(&property("node_role")).takes_links(),
    };
    Ok(Some(TurnSettings {
        bind,
        port: udp,
        tls_port,
        tls443,
        public_host,
        public_ip,
        relay_ports,
        config,
        limits: StreamLimits::default(),
    }))
}

/// The URLs browsers get for these listeners.
pub(crate) fn urls(s: &TurnSettings, bound: &Bound) -> Vec<String> {
    let host = if s.public_host.contains(':') {
        format!("[{}]", s.public_host)
    } else {
        s.public_host.clone()
    };
    let mut urls = Vec::new();
    if let Some(udp) = bound.udp {
        urls.push(format!("turn:{host}:{}?transport=udp", udp.port()));
    }
    if let Some(tcp) = bound.tcp {
        urls.push(format!("turn:{host}:{}?transport=tcp", tcp.port()));
    }
    if let Some(tls) = bound.tls {
        urls.push(format!("turns:{host}:{}?transport=tcp", tls.port()));
    }
    if let Some(tls443) = bound.tls443 {
        urls.push(format!("turns:{}:443?transport=tcp", tls443.ip()));
    }
    urls
}

/// The addresses the listeners bound.
#[derive(Debug, Clone, Default)]
pub(crate) struct Bound {
    pub udp: Option<SocketAddr>,
    pub tcp: Option<SocketAddr>,
    pub tls: Option<SocketAddr>,
    pub tls443: Option<SocketAddr>,
}

enum In {
    Stream {
        from: SocketAddr,
        listen: SocketAddr,
        data: Vec<u8>,
    },
    Opened {
        listen: SocketAddr,
        remote: SocketAddr,
        tx: mpsc::Sender<Vec<u8>>,
    },
    Closed {
        listen: SocketAddr,
        remote: SocketAddr,
    },
    Revoke(String),
    Pairings(std::collections::HashMap<String, String>),
}

/// The TURN listeners, their tasks on the edge runtime and the thread of the server, stopped when dropped.
pub(crate) struct TurnNode {
    tx: SyncSender<In>,
    stop: Arc<AtomicBool>,
    /// Tells the accept loops and every connection to end.
    stop_streams: watch::Sender<bool>,
    accepts: Vec<tokio::task::JoinHandle<()>>,
    thread: Option<JoinHandle<()>>,
    pub bound: Bound,
}

impl TurnNode {
    /// The leg left: its credential is refused from now on, and its relays close.
    pub(crate) fn revoke(&self, leg: &str) {
        // waits only while the server's queue is full, which its thread drains
        let _ = self.tx.send(In::Revoke(leg.to_string()));
    }

    /// On a node that takes links, the pairings whose legs are taken now (plans/Edge-Node2.md §4.1, E16): a revoked
    /// one's credentials are refused from then on and its relays close.
    pub(crate) fn set_pairings(&self, pairings: std::collections::HashMap<String, String>) {
        let _ = self.tx.send(In::Pairings(pairings));
    }
}

impl Drop for TurnNode {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.stop_streams.send(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // the listeners close as their loops end
        let accepts = std::mem::take(&mut self.accepts);
        let _ = crate::edge::block_on(async move {
            let _ = tokio::time::timeout(Duration::from_secs(2), async move {
                for accept in accepts {
                    let _ = accept.await;
                }
            })
            .await;
        });
    }
}

/// Starts the listeners. One that does not bind is an error: the media node logs it and goes on without TURN.
pub(crate) fn start(runtime_dir: &Path, s: &TurnSettings) -> Result<TurnNode, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = sync_channel::<In>(MAX_INPUT);
    let mut bound = Bound::default();
    let udp = match s.port {
        Some(p) => {
            let socket = UdpSocket::bind((s.bind, p)).map_err(|e| format!("turn_port {p} did not bind: {e}"))?;
            socket.set_nonblocking(true).map_err(|e| e.to_string())?;
            bound.udp = socket.local_addr().ok();
            Some(socket)
        }
        None => None,
    };
    let mut listeners: Vec<(Proto, SocketAddr)> = Vec::new();
    if let Some(addr) = bound.udp {
        listeners.push((Proto::Udp, addr));
    }
    let tls_config = if s.tls_port.is_some() || s.tls443.is_some() {
        let mut config = crate::tls::server_config(crate::tls::CertStore::new(runtime_dir.join("cert")))
            .map_err(|e| format!("TURN TLS: {}", e.message()))?;
        config.alpn_protocols = Vec::new();
        Some(TlsAcceptor::from(Arc::new(config)))
    } else {
        None
    };
    let mut stream_listeners: Vec<(std::net::TcpListener, Option<TlsAcceptor>)> = Vec::new();
    if let Some(p) = s.port {
        let l =
            std::net::TcpListener::bind((s.bind, p)).map_err(|e| format!("turn_port {p} (TCP) did not bind: {e}"))?;
        bound.tcp = l.local_addr().ok();
        stream_listeners.push((l, None));
    }
    if let Some(p) = s.tls_port {
        let l = std::net::TcpListener::bind((s.bind, p)).map_err(|e| format!("turn_tls_port {p} did not bind: {e}"))?;
        bound.tls = l.local_addr().ok();
        stream_listeners.push((l, tls_config.clone()));
    }
    if let Some(ip) = s.tls443 {
        let l = std::net::TcpListener::bind((ip, 443))
            .map_err(|e| format!("turn_tls443_host {ip}:443 did not bind: {e}"))?;
        bound.tls443 = l.local_addr().ok();
        stream_listeners.push((l, tls_config.clone()));
    }
    for (l, _) in &stream_listeners {
        if let Ok(addr) = l.local_addr() {
            listeners.push((Proto::Tcp, addr));
        }
    }
    let turn = Turn::new(s.config.clone(), &listeners);
    let slots = SourceSlots::new(s.limits.max_connections, s.limits.per_source);
    let (stop_streams, stopped) = watch::channel(false);
    let runtime = crate::edge::handle();
    let mut accepts = Vec::new();
    for (listener, tls) in stream_listeners {
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        // registered with the edge runtime's reactor
        let listener = {
            let _entered = runtime.enter();
            TcpListener::from_std(listener).map_err(|e| e.to_string())?
        };
        let streams = Streams {
            tx: tx.clone(),
            stop: stopped.clone(),
            slots: slots.clone(),
            limits: s.limits,
        };
        accepts.push(runtime.spawn(accept(listener, tls, streams)));
    }
    let (bind, public_ip, range) = (s.bind, s.public_ip, s.relay_ports);
    let stop_main = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("moqui-turn".to_string())
        .spawn(move || run(turn, udp, rx, bind, public_ip, range, &stop_main))
        .map_err(|e| e.to_string());
    let thread = match thread {
        Ok(thread) => thread,
        Err(e) => {
            let _ = stop_streams.send(true);
            return Err(e);
        }
    };
    Ok(TurnNode {
        tx,
        stop,
        stop_streams,
        accepts,
        thread: Some(thread),
        bound,
    })
}

#[allow(clippy::too_many_arguments)]
fn run(
    mut turn: Turn,
    udp: Option<UdpSocket>,
    rx: Receiver<In>,
    bind: IpAddr,
    public_ip: IpAddr,
    range: (u16, u16),
    stop: &AtomicBool,
) {
    let listen_udp = udp.as_ref().and_then(|u| u.local_addr().ok());
    // by the address the server reports (the public one), the socket bound on `bind`
    let mut relays: HashMap<SocketAddr, UdpSocket> = HashMap::new();
    let mut streams: HashMap<(SocketAddr, SocketAddr), mpsc::Sender<Vec<u8>>> = HashMap::new();
    let mut cursor = range.0;
    let mut buf = vec![0u8; 2_048];
    let mut next = Instant::now();
    // a message the wait below took
    let mut waited: Option<In> = None;
    while !stop.load(Ordering::Acquire) {
        let mut busy = false;
        let now = Instant::now();
        while let Some(message) = waited.take().or_else(|| rx.try_recv().ok()) {
            busy = true;
            match message {
                In::Stream { from, listen, data } => turn.handle_client(now, Proto::Tcp, from, listen, &data),
                In::Opened { listen, remote, tx } => {
                    streams.insert((listen, remote), tx);
                }
                In::Closed { listen, remote } => {
                    streams.remove(&(listen, remote));
                }
                In::Revoke(leg) => turn.revoke_leg(now, &leg),
                In::Pairings(pairings) => turn.set_pairings(now, pairings),
            }
        }
        if let (Some(socket), Some(listen)) = (&udp, listen_udp) {
            for _ in 0..64 {
                match socket.recv_from(&mut buf) {
                    Ok((n, from)) => {
                        busy = true;
                        turn.handle_client(now, Proto::Udp, from, listen, &buf[..n]);
                    }
                    Err(_) => break,
                }
            }
        }
        for (relayed, socket) in &relays {
            for _ in 0..16 {
                match socket.recv_from(&mut buf) {
                    Ok((n, from)) => {
                        busy = true;
                        turn.handle_peer(now, from, *relayed, &buf[..n]);
                    }
                    Err(_) => break,
                }
            }
        }
        if now >= next {
            turn.handle_timeout(now);
            next = turn.next_timeout(now);
        }
        while let Some(out) = turn.poll_output() {
            busy = true;
            match out {
                TurnOutput::Transmit { proto, from, to, data } => {
                    if Some(from) == listen_udp && proto == Proto::Udp {
                        if let Some(socket) = &udp {
                            let _ = socket.send_to(&data, to);
                        }
                    } else if let Some(socket) = relays.get(&from) {
                        let _ = socket.send_to(&data, to);
                    } else if let Some(tx) = streams.get(&(from, to)) {
                        match tx.try_send(data) {
                            Ok(()) => {}
                            // the client does not read what it is sent: dropping its sender closes the connection
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                tracing::debug!("TURN: {to} does not read what it is sent; its connection is closed");
                                streams.remove(&(from, to));
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                streams.remove(&(from, to));
                            }
                        }
                    }
                }
                TurnOutput::AllocateRelay {
                    listen,
                    proto,
                    client,
                    family,
                } => {
                    let bound = bind_relay(bind, range, &mut cursor);
                    let reported = bound.as_ref().map(|(_, addr)| SocketAddr::new(public_ip, addr.port()));
                    if let (Some((socket, _)), Some(reported)) = (bound, reported) {
                        relays.insert(reported, socket);
                    }
                    turn.relay_allocated(now, listen, proto, client, family, reported.ok_or(()));
                }
                TurnOutput::CloseRelay { relayed } => {
                    relays.remove(&relayed);
                }
                TurnOutput::CloseStream { local, remote } => {
                    streams.remove(&(local, remote));
                }
            }
        }
        if !busy {
            // UDP is read by polling its sockets, so with any open the wait is as short as before; with only TCP and
            // TLS clients, what they send ends the wait at once and the server's own timers end it otherwise
            let wait = if udp.is_some() || !relays.is_empty() {
                POLL_UDP
            } else {
                next.saturating_duration_since(Instant::now()).clamp(POLL_UDP, MAX_WAIT)
            };
            match rx.recv_timeout(wait) {
                Ok(message) => waited = Some(message),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// A UDP socket on the next free port of the range.
fn bind_relay(bind: IpAddr, range: (u16, u16), cursor: &mut u16) -> Option<(UdpSocket, SocketAddr)> {
    let span = u32::from(range.1 - range.0) + 1;
    for _ in 0..span.min(200) {
        let port = *cursor;
        *cursor = if *cursor >= range.1 { range.0 } else { *cursor + 1 };
        if let Ok(socket) = UdpSocket::bind((bind, port)) {
            if socket.set_nonblocking(true).is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    return Some((socket, addr));
                }
            }
        }
    }
    tracing::warn!("TURN: no free relay port in {}-{}", range.0, range.1);
    None
}

/// What the accept loops and connection tasks share.
#[derive(Clone)]
struct Streams {
    tx: SyncSender<In>,
    stop: watch::Receiver<bool>,
    slots: SourceSlots,
    limits: StreamLimits,
}

/// Put `message` on the server's queue, waiting while it is full (the server's thread drains it) so a client that
/// sends faster than the server reads is held back by its own connection. False once the server has stopped.
async fn deliver(tx: &SyncSender<In>, mut message: In) -> bool {
    loop {
        match tx.try_send(message) {
            Ok(()) => return true,
            Err(TrySendError::Full(back)) => {
                message = back;
                tokio::time::sleep(POLL_UDP).await;
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

/// True once `stop` says so, or its sender is gone.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// One TCP or TLS listener: each connection within the slots of [`StreamLimits`] is a task of its own, until a stop.
async fn accept(listener: TcpListener, tls: Option<TlsAcceptor>, streams: Streams) {
    let Ok(listen) = listener.local_addr() else { return };
    let mut stop = streams.stop.clone();
    loop {
        let accepted = tokio::select! {
            () = stopped(&mut stop) => return,
            accepted = listener.accept() => accepted,
        };
        let (stream, remote) = match accepted {
            Ok(accepted) => accepted,
            Err(e) => {
                // out of file descriptors and the like: try again shortly
                tracing::debug!("TURN: an accept on {listen} failed: {e}");
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
        };
        let Ok(slot) = streams.slots.take(remote.ip()) else {
            tracing::debug!("TURN: a connection from {remote} closed: too many from there or in all");
            continue;
        };
        let (tls, streams) = (tls.clone(), streams.clone());
        tokio::spawn(async move {
            connection(stream, listen, remote, tls, streams).await;
            drop(slot);
        });
    }
}

/// One connection, from its open to its close: told to the server at both ends whatever ends it, so its allocations
/// end with it.
async fn connection(
    stream: TcpStream,
    listen: SocketAddr,
    remote: SocketAddr,
    tls: Option<TlsAcceptor>,
    streams: Streams,
) {
    let _ = stream.set_nodelay(true);
    let opened = Instant::now();
    let (out_tx, mut out) = mpsc::channel::<Vec<u8>>(streams.limits.max_queued.max(1));
    if !deliver(
        &streams.tx,
        In::Opened {
            listen,
            remote,
            tx: out_tx,
        },
    )
    .await
    {
        return;
    }
    let mut stop = streams.stop.clone();
    let link = Link {
        listen,
        remote,
        tx: &streams.tx,
        limits: streams.limits,
        opened,
    };
    match tls {
        Some(acceptor) => {
            let handshake = tokio::select! {
                () = stopped(&mut stop) => None,
                done = tokio::time::timeout(streams.limits.handshake, acceptor.accept(stream)) => Some(done),
            };
            match handshake {
                Some(Ok(Ok(stream))) => serve(stream, &link, &mut out, &mut stop).await,
                Some(Ok(Err(e))) => tracing::debug!("TURN TLS with {remote} did not start: {e}"),
                Some(Err(_)) => tracing::debug!(
                    "TURN: {remote} did not finish its TLS handshake within {:?}",
                    streams.limits.handshake
                ),
                None => {}
            }
        }
        None => serve(stream, &link, &mut out, &mut stop).await,
    }
    deliver(&streams.tx, In::Closed { listen, remote }).await;
}

/// What one connection's task serves it with.
struct Link<'a> {
    listen: SocketAddr,
    remote: SocketAddr,
    tx: &'a SyncSender<In>,
    limits: StreamLimits,
    opened: Instant,
}

/// One TCP or TLS client: its stream split into messages for the server, and what the server sends it written back,
/// until it closes, misses its handshake or idle deadline, stops reading (its queue's sender dropped, or a write that
/// does not finish in [`WRITE_TIMEOUT`]) or the listeners stop.
async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    link: &Link<'_>,
    out: &mut mpsc::Receiver<Vec<u8>>,
    stop: &mut watch::Receiver<bool>,
) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4_096];
    let mut spoke = false;
    let mut active = link.opened;
    loop {
        // the first whole message is due within the handshake deadline, and something read or written within idle
        let idle_end = active + link.limits.idle;
        let deadline = if spoke {
            idle_end
        } else {
            idle_end.min(link.opened + link.limits.handshake)
        };
        tokio::select! {
            () = stopped(stop) => return,
            read = stream.read(&mut chunk) => {
                let n = match read {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                active = Instant::now();
                buf.extend_from_slice(&chunk[..n]);
                while let Some(len) = frame_len(&buf) {
                    if buf.len() < len {
                        break;
                    }
                    spoke = true;
                    let data: Vec<u8> = buf.drain(..len).collect();
                    let message = In::Stream {
                        from: link.remote,
                        listen: link.listen,
                        data,
                    };
                    if !deliver(link.tx, message).await {
                        return;
                    }
                }
                if buf.len() > MAX_STREAM_BUFFER {
                    return;
                }
            }
            sent = out.recv() => {
                // None: the server dropped the sender, as for a client that does not read what it is sent
                let Some(data) = sent else { return };
                let written = tokio::time::timeout(WRITE_TIMEOUT, async {
                    stream.write_all(&data).await?;
                    stream.flush().await
                })
                .await;
                if !matches!(written, Ok(Ok(()))) {
                    return;
                }
                active = Instant::now();
            }
            () = tokio::time::sleep_until(deadline.into()) => {
                if spoke || Instant::now() >= idle_end {
                    tracing::debug!("TURN: {} was idle for {:?}", link.remote, link.limits.idle);
                } else {
                    tracing::debug!("TURN: {} sent no message within {:?}", link.remote, link.limits.handshake);
                }
                return;
            }
        }
    }
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
