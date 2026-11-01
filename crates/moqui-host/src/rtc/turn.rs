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

//! The embedded TURN server's sockets (plans/Comms-Framework.md §6.5, W4; docs/interfaces/TURN.md), on the media node:
//! UDP and TCP on `turn_port`, TLS on `turn_tls_port` and on 443 of `turn_tls443_host` (an address of its own, since the
//! HTTPS listener does not share its port), all off while their setting is empty, and the UDP relay sockets in
//! `turn_relay_ports`. One thread owns the [`Turn`] and the UDP sockets; a thread per TCP or TLS connection reads its
//! stream a message at a time and writes what the server sends it. TLS uses the host's certificates (`runtime/cert`).
//! The media node revokes a leg's credential when the leg leaves ([`TurnNode::revoke`]).

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_rtc::turn::{frame_len, Turn, TurnConfig, TurnOutput};
use moqui_rtc::Proto;

/// The most bytes a stream may buffer without a whole message.
const MAX_STREAM_BUFFER: usize = 64 * 1024;

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
        tx: Sender<Vec<u8>>,
    },
    Closed {
        listen: SocketAddr,
        remote: SocketAddr,
    },
    Revoke(String),
}

/// The TURN listeners and their threads, stopped when dropped.
pub(crate) struct TurnNode {
    tx: Sender<In>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    pub bound: Bound,
}

impl TurnNode {
    /// The leg left: its credential is refused from now on, and its relays close.
    pub(crate) fn revoke(&self, leg: &str) {
        let _ = self.tx.send(In::Revoke(leg.to_string()));
    }
}

impl Drop for TurnNode {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Starts the listeners. One that does not bind is an error: the media node logs it and goes on without TURN.
pub(crate) fn start(runtime_dir: &Path, s: &TurnSettings) -> Result<TurnNode, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel::<In>();
    let mut bound = Bound::default();
    let mut threads = Vec::new();
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
        Some(Arc::new(config))
    } else {
        None
    };
    let mut stream_listeners: Vec<(TcpListener, Option<Arc<rustls::ServerConfig>>)> = Vec::new();
    if let Some(p) = s.port {
        let l = TcpListener::bind((s.bind, p)).map_err(|e| format!("turn_port {p} (TCP) did not bind: {e}"))?;
        bound.tcp = l.local_addr().ok();
        stream_listeners.push((l, None));
    }
    if let Some(p) = s.tls_port {
        let l = TcpListener::bind((s.bind, p)).map_err(|e| format!("turn_tls_port {p} did not bind: {e}"))?;
        bound.tls = l.local_addr().ok();
        stream_listeners.push((l, tls_config.clone()));
    }
    if let Some(ip) = s.tls443 {
        let l = TcpListener::bind((ip, 443)).map_err(|e| format!("turn_tls443_host {ip}:443 did not bind: {e}"))?;
        bound.tls443 = l.local_addr().ok();
        stream_listeners.push((l, tls_config.clone()));
    }
    for (l, _) in &stream_listeners {
        if let Ok(addr) = l.local_addr() {
            listeners.push((Proto::Tcp, addr));
        }
    }
    let turn = Turn::new(s.config.clone(), &listeners);
    for (listener, tls) in stream_listeners {
        let (tx, stop) = (tx.clone(), Arc::clone(&stop));
        threads.push(
            std::thread::Builder::new()
                .name("moqui-turn-accept".to_string())
                .spawn(move || accept(listener, tls, tx, &stop))
                .map_err(|e| e.to_string())?,
        );
    }
    let (bind, public_ip, range) = (s.bind, s.public_ip, s.relay_ports);
    let stop_main = Arc::clone(&stop);
    threads.push(
        std::thread::Builder::new()
            .name("moqui-turn".to_string())
            .spawn(move || run(turn, udp, rx, bind, public_ip, range, &stop_main))
            .map_err(|e| e.to_string())?,
    );
    Ok(TurnNode {
        tx,
        stop,
        threads,
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
    let mut streams: HashMap<(SocketAddr, SocketAddr), Sender<Vec<u8>>> = HashMap::new();
    let mut cursor = range.0;
    let mut buf = vec![0u8; 2_048];
    let mut next = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let mut busy = false;
        let now = Instant::now();
        while let Ok(message) = rx.try_recv() {
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
                        let _ = tx.send(data);
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
            std::thread::sleep(Duration::from_millis(1));
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

fn accept(listener: TcpListener, tls: Option<Arc<rustls::ServerConfig>>, tx: Sender<In>, stop: &Arc<AtomicBool>) {
    let Ok(listen) = listener.local_addr() else { return };
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, remote)) => {
                let (tls, tx, stop) = (tls.clone(), tx.clone(), Arc::clone(stop));
                let _ = std::thread::Builder::new()
                    .name("moqui-turn-conn".to_string())
                    .spawn(move || {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(5)));
                        let _ = stream.set_nodelay(true);
                        let (out_tx, out_rx) = channel::<Vec<u8>>();
                        let _ = tx.send(In::Opened {
                            listen,
                            remote,
                            tx: out_tx,
                        });
                        match tls {
                            Some(config) => match rustls::ServerConnection::new(config) {
                                Ok(conn) => serve(
                                    rustls::StreamOwned::new(conn, stream),
                                    listen,
                                    remote,
                                    &tx,
                                    &out_rx,
                                    &stop,
                                ),
                                Err(e) => tracing::debug!("TURN TLS did not start: {e}"),
                            },
                            None => serve(stream, listen, remote, &tx, &out_rx, &stop),
                        }
                        let _ = tx.send(In::Closed { listen, remote });
                    });
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// One TCP or TLS client: its stream split into messages for the server, and what the server sends it written back.
fn serve<S: Read + Write>(
    mut stream: S,
    listen: SocketAddr,
    remote: SocketAddr,
    tx: &Sender<In>,
    out: &Receiver<Vec<u8>>,
    stop: &AtomicBool,
) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4_096];
    while !stop.load(Ordering::Acquire) {
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(_) => return,
        }
        while let Some(len) = frame_len(&buf) {
            if buf.len() < len {
                break;
            }
            let data: Vec<u8> = buf.drain(..len).collect();
            if tx
                .send(In::Stream {
                    from: remote,
                    listen,
                    data,
                })
                .is_err()
            {
                return;
            }
        }
        if buf.len() > MAX_STREAM_BUFFER {
            return;
        }
        while let Ok(data) = out.try_recv() {
            if stream.write_all(&data).is_err() || stream.flush().is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::settings;

    fn of(pairs: &[(&str, &str)]) -> Result<Option<super::TurnSettings>, String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let property = move |name: &str| map.get(name).cloned().unwrap_or_default();
        settings(
            &property,
            "127.0.0.1:10000".parse().unwrap(),
            "203.0.113.5:10000".parse().unwrap(),
            &[],
        )
    }

    #[test]
    fn turn_is_off_without_a_listener_and_needs_a_secret() {
        assert!(of(&[]).unwrap().is_none());
        assert!(of(&[("turn_port", "3478")]).is_err());
        let s = of(&[("turn_port", "3478"), ("turn_auth_secret", "s3cret")])
            .unwrap()
            .unwrap();
        assert_eq!(s.public_host, "203.0.113.5");
        assert_eq!(s.relay_ports, (49152, 65535));
        assert_eq!(
            s.config.allowed_peers,
            vec!["203.0.113.5:10000".parse().unwrap(), "127.0.0.1:10000".parse().unwrap()]
        );
        // another media node by address alone is on the media port, or as given
        let s = of(&[
            ("turn_port", "3478"),
            ("turn_auth_secret", "s3cret"),
            ("turn_allowed_peers", "198.51.100.7, 198.51.100.8:11000"),
        ])
        .unwrap()
        .unwrap();
        assert!(s.config.allowed_peers.contains(&"198.51.100.7:10000".parse().unwrap()));
        assert!(s.config.allowed_peers.contains(&"198.51.100.8:11000".parse().unwrap()));
    }

    #[test]
    fn a_bad_range_or_address_is_an_error() {
        let base = [("turn_port", "3478"), ("turn_auth_secret", "s3cret")];
        assert!(of(&[base[0], base[1], ("turn_relay_ports", "60000-50000")]).is_err());
        assert!(of(&[base[0], base[1], ("turn_tls443_host", "not an address")]).is_err());
        assert!(of(&[base[0], base[1], ("turn_allowed_peers", "nope")]).is_err());
    }
}
