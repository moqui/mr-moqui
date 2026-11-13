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

//! The embedded TURN server (plans/archive/Comms-Framework.md §6.5, W4, D-W4), on `turn-server-proto`, sans-IO like the SFU:
//! the caller owns the listeners (UDP, TCP, TLS) and the relay sockets, and hands in what arrives.
//!
//! - **Credentials** are the TURN REST form coturn's `use-auth-secret` takes: the username is `<expiry>:<legId>` (the
//!   expiry in Unix seconds) and the password is base64 of HMAC-SHA1 of the username with `turn_auth_secret`
//!   ([`password`]). The call services make them only for an admitted leg (`get#IceServers`). A username is taken the
//!   first time it is seen while it has not expired and its leg is not revoked; an expired or revoked one is refused,
//!   refreshes and new allocations included.
//! - **Relay peers** are only the media nodes' media ports ([`TurnConfig::allowed_peers`], address and port): a
//!   permission to any other address, or a channel to any other address and port, is refused before the server sees
//!   it, and nothing is relayed to or from one (another port on a media node's address included). Standard TURN credentials do
//!   not bind a relay to one call; this rule does. The one exception is an XMPP user's own credential (leg
//!   `xmpp.<userId>`, `XmppServices.get#IceServers`, XEP-0215), for calls between XMPP clients: it may relay to public
//!   unicast addresses too, never to loopback, private, link-local, shared or documentation ranges ([`is_public`]), nor
//!   another port of a media node's address or of the listeners' own, so it cannot reach into the server's own networks
//!   or the server itself.
//! - **Quotas:** allocations per username and in all, and a bandwidth per allocation, past which data is dropped.
//! - **Revocation** ([`Turn::revoke_leg`]): the leg's username is refused from then on, its allocations' relay sockets
//!   close, and nothing more is relayed for them.
//!
//! TCP and TLS clients are handed in a message at a time ([`frame_len`] splits the stream: a STUN message or a
//! ChannelData, padded to four bytes on a stream).

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use turn_server_proto::api::{
    SocketAllocateError, TcpConnectError, Transmit as TurnTransmit, TurnServerApi, TurnServerPollRet,
};
use turn_server_proto::server::TurnServer;
use turn_server_proto::types::attribute::XorPeerAddress;
use turn_server_proto::types::message::{ALLOCATE, CHANNEL_BIND, CREATE_PERMISSION, REFRESH};
use turn_server_proto::types::stun::attribute::Username;
use turn_server_proto::types::stun::message::{Message, MessageClass};
use turn_server_proto::types::{AddressFamily, TransportType};

use crate::sfu::Proto;

/// How long a revoked leg stays refused (past any credential made for it).
const REVOKED_FOR: Duration = Duration::from_secs(24 * 3600);
/// The longest wait [`Turn::next_timeout`] gives, so revocations and expiries are looked at.
const MAX_WAIT: Duration = Duration::from_secs(1);

/// The leg prefix of an XMPP user's own credential, which may relay to public addresses.
pub const XMPP_LEG: &str = "xmpp.";

/// A public unicast address: not unspecified, loopback, private, shared (100.64/10), link-local, multicast, broadcast,
/// documentation, benchmarking or unique-local; an IPv4-mapped IPv6 address and a 6to4 one (2002::/16) as the IPv4
/// address they carry; no IPv4-compatible (::/96), NAT64 (64:ff9b::/96 and /48) or Teredo (2001::/32) address, which
/// carry an IPv4 address that may be inside.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
                || a == 0
                || a >= 240
                || (a == 100 && (64..128).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && c == 0))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let segments = v6.segments();
            let first = segments[0];
            if first == 0x2002 {
                let [a, b] = segments[1].to_be_bytes();
                let [c, d] = segments[2].to_be_bytes();
                return is_public(IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d)));
            }
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || segments[..6].iter().all(|segment| *segment == 0)
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || first == 0x2001 && segments[1] == 0x0db8
                || first == 0x2001 && segments[1] == 0
                || first == 0x0064)
        }
    }
}

/// The password of a TURN REST username: base64 of HMAC-SHA1 of the username with the shared secret.
pub fn password(secret: &[u8], username: &str) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC takes a key of any length");
    mac.update(username.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

/// A credential for a leg, good until `expiry` (Unix seconds): (username, password).
pub fn credential(secret: &[u8], leg: &str, expiry: u64) -> (String, String) {
    let username = format!("{expiry}:{leg}");
    let pass = password(secret, &username);
    (username, pass)
}

/// The length of the first message of a TURN stream (TCP or TLS): a STUN message or a ChannelData padded to four
/// bytes; None until its header has arrived.
pub fn frame_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([buf[2], buf[3]]));
    match buf[0] >> 6 {
        0 => Some(20 + length),
        1 => Some(4 + length.div_ceil(4) * 4),
        _ => Some(buf.len()),
    }
}

#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub realm: String,
    pub secret: Vec<u8>,
    /// The only addresses and ports relayed to and from: the media nodes' media ports. A permission (by address
    /// alone, RFC 8656) is taken for any of their addresses; a channel and the data need the port too.
    pub allowed_peers: Vec<SocketAddr>,
    pub max_allocations: usize,
    pub max_per_user: usize,
    /// Each allocation's relayed data, each way, in kilobits a second.
    pub max_kbps: u64,
}

impl Default for TurnConfig {
    fn default() -> Self {
        TurnConfig {
            realm: "moqui".to_string(),
            secret: Vec::new(),
            allowed_peers: Vec::new(),
            max_allocations: 1_000,
            max_per_user: 2,
            max_kbps: 4_000,
        }
    }
}

/// What the caller does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutput {
    /// Send from `from` (a listener or a relay socket) to `to`.
    Transmit {
        proto: Proto,
        from: SocketAddr,
        to: SocketAddr,
        data: Vec<u8>,
    },
    /// Bind a UDP relay socket for the client at `client` of the listener `listen` and call
    /// [`Turn::relay_allocated`] with its address.
    AllocateRelay {
        listen: SocketAddr,
        proto: Proto,
        client: SocketAddr,
        family: AddressFamily,
    },
    /// Close the relay socket bound at `relayed`.
    CloseRelay { relayed: SocketAddr },
    /// Close the stream from `remote` on the listener `local`.
    CloseStream { local: SocketAddr, remote: SocketAddr },
}

/// A bucket of bytes for one allocation's data.
#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

#[derive(Debug, Clone)]
struct Allocation {
    username: String,
    leg: Arc<str>,
    relayed: Option<SocketAddr>,
    bucket: Bucket,
}

pub struct Turn {
    config: TurnConfig,
    /// The time the server's own clock counts from.
    epoch: Instant,
    servers: Vec<TurnServer>,
    users: HashSet<String>,
    revoked: HashMap<Arc<str>, Instant>,
    /// By (listener, client).
    allocations: HashMap<(SocketAddr, SocketAddr), Allocation>,
    /// Requests for an allocation, by (listener, client), with their username.
    asked: HashMap<(SocketAddr, SocketAddr), String>,
    relays: HashMap<SocketAddr, (SocketAddr, SocketAddr)>,
    out: VecDeque<TurnOutput>,
    next: Option<Instant>,
}

impl std::fmt::Debug for Turn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Turn")
            .field("allocations", &self.allocations.len())
            .finish()
    }
}

fn proto_of(t: TransportType) -> Proto {
    match t {
        TransportType::Udp => Proto::Udp,
        _ => Proto::Tcp,
    }
}

fn transport_of(p: Proto) -> TransportType {
    match p {
        Proto::Udp => TransportType::Udp,
        Proto::Tcp => TransportType::Tcp,
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The leg of a username that is still good (not expired); None for one that is not ours.
fn leg_of(username: &str, now_unix: u64) -> Option<&str> {
    let (expiry, leg) = username.split_once(':')?;
    let expiry: u64 = expiry.parse().ok()?;
    (expiry > now_unix && !leg.is_empty() && leg.len() <= 64).then_some(leg)
}

impl Turn {
    /// A TURN server on these listeners (a transport and the address clients reach).
    pub fn new(config: TurnConfig, listeners: &[(Proto, SocketAddr)]) -> Self {
        let servers = listeners
            .iter()
            .map(|(proto, addr)| {
                let mut server = TurnServer::new(transport_of(*proto), *addr, config.realm.clone());
                server.set_nonce_expiry_duration(Duration::from_secs(600));
                server
            })
            .collect();
        Turn {
            config,
            epoch: Instant::now(),
            servers,
            users: HashSet::new(),
            revoked: HashMap::new(),
            allocations: HashMap::new(),
            asked: HashMap::new(),
            relays: HashMap::new(),
            out: VecDeque::new(),
            next: None,
        }
    }

    /// The server's clock for `now`: the time since the epoch.
    fn clock(&self, now: Instant) -> turn_server_proto::types::Instant {
        let nanos = i64::try_from(now.saturating_duration_since(self.epoch).as_nanos()).unwrap_or(i64::MAX);
        turn_server_proto::types::Instant::from_nanos(nanos)
    }

    pub fn allocation_count(&self) -> usize {
        self.allocations.len()
    }

    fn server_at(&mut self, listen: SocketAddr, transport: TransportType) -> Option<&mut TurnServer> {
        self.servers
            .iter_mut()
            .find(|s| s.listen_address() == listen && s.transport() == transport)
    }

    /// A peer `addr` may be relayed to: a media node's media port, or for an XMPP user's own credential a public
    /// address. With `any_port` (a permission, which RFC 8656 keys by address alone) only the address is matched.
    /// The XMPP rule never reaches a media node's other ports, nor this server's own addresses (its listeners' ports
    /// or any other): a relay there would come from inside, past a firewall that keeps the outside off them.
    fn peer_allowed(&self, addr: SocketAddr, any_port: bool, leg: Option<&str>) -> bool {
        let ip = addr.ip().to_canonical();
        let mut media_node = false;
        for p in &self.config.allowed_peers {
            if p.ip().to_canonical() == ip {
                if any_port || p.port() == addr.port() {
                    return true;
                }
                media_node = true;
            }
        }
        leg.is_some_and(|leg| leg.starts_with(XMPP_LEG))
            && is_public(ip)
            && !media_node
            && !self
                .servers
                .iter()
                .any(|s| s.listen_address().ip().to_canonical() == ip)
    }

    fn is_revoked(&self, leg: &str) -> bool {
        self.revoked.contains_key(leg)
    }

    /// A message from a client arrived at the listener `listen`: on UDP a datagram, on a stream one message.
    pub fn handle_client(&mut self, now: Instant, proto: Proto, from: SocketAddr, listen: SocketAddr, data: &[u8]) {
        let transport = transport_of(proto);
        if let Ok(message) = Message::from_bytes(data) {
            if !self.admit_message(&message, listen, from) {
                return;
            }
        } else if self
            .allocations
            .get(&(listen, from))
            .is_some_and(|a| self.is_revoked(&a.leg))
        {
            return;
        }
        let tnow = self.clock(now);
        let Some(server) = self.server_at(listen, transport) else {
            return;
        };
        let built = server
            .recv(
                TurnTransmit {
                    data,
                    transport,
                    from,
                    to: listen,
                },
                tnow,
            )
            .map(|t| t.build());
        if let Some(t) = built {
            self.send_out(now, proto_of(t.transport), t.from, t.to, t.data);
        }
        self.poll_servers(now);
    }

    /// Checks a STUN request before the server sees it: its username, the allocation quotas and the relay peers.
    fn admit_message(&mut self, message: &Message<'_>, listen: SocketAddr, from: SocketAddr) -> bool {
        if message.class() != MessageClass::Request {
            return true;
        }
        let username = message.attribute::<Username>().ok().map(|u| u.username().to_string());
        if let Some(username) = &username {
            // a username the server has never been given gets its 401 from it; one it has (it expired or its leg was
            // revoked since) is dropped, since the server would still take it
            let good = leg_of(username, unix_now()).filter(|leg| !self.is_revoked(leg));
            if good.is_none() {
                return !self.users.contains(username);
            }
            if !self.users.contains(username) {
                let pass = password(&self.config.secret, username);
                for server in &mut self.servers {
                    server.add_user(username.clone(), pass.clone());
                }
                self.users.insert(username.clone());
            }
        }
        let method = message.method();
        if method == ALLOCATE {
            if let Some(username) = username {
                let mine = self.allocations.values().filter(|a| a.username == username).count();
                if !self.allocations.contains_key(&(listen, from))
                    && (mine >= self.config.max_per_user || self.allocations.len() >= self.config.max_allocations)
                {
                    tracing::debug!("TURN allocation refused for {username}: over its quota");
                    return false;
                }
                self.asked.insert((listen, from), username);
            }
        } else if method == CREATE_PERMISSION || method == CHANNEL_BIND {
            let transaction = message.transaction_id();
            let leg = self.allocations.get(&(listen, from)).map(|a| Arc::clone(&a.leg));
            let mut n = 0;
            while let Ok(peer) = message.nth_attribute::<XorPeerAddress>(n) {
                let peer = peer.addr(transaction);
                if !self.peer_allowed(peer, method == CREATE_PERMISSION, leg.as_deref()) {
                    tracing::debug!("TURN permission refused for {peer}: not a media node's media port");
                    return false;
                }
                n += 1;
            }
        } else if method == REFRESH {
            if let Some(a) = self.allocations.get(&(listen, from)) {
                if self.is_revoked(&a.leg) {
                    return false;
                }
            }
        }
        true
    }

    /// Data from a peer arrived at the relay socket `relayed`.
    pub fn handle_peer(&mut self, now: Instant, from: SocketAddr, relayed: SocketAddr, data: &[u8]) {
        let Some(&(listen, client)) = self.relays.get(&relayed) else {
            return;
        };
        let leg = self.allocations.get(&(listen, client)).map(|a| Arc::clone(&a.leg));
        if !self.peer_allowed(from, false, leg.as_deref()) {
            return;
        }
        let Some(transport) = self
            .servers
            .iter()
            .find(|s| s.listen_address() == listen)
            .map(|s| s.transport())
        else {
            return;
        };
        if !self.take_tokens(&(listen, client), data.len(), now) {
            return;
        }
        let tnow = self.clock(now);
        let Some(server) = self.server_at(listen, transport) else {
            return;
        };
        let built = server
            .recv(
                TurnTransmit {
                    data,
                    transport: TransportType::Udp,
                    from,
                    to: relayed,
                },
                tnow,
            )
            .map(|t| t.build());
        if let Some(t) = built {
            self.out.push_back(TurnOutput::Transmit {
                proto: proto_of(t.transport),
                from: t.from,
                to: t.to,
                data: t.data,
            });
        }
        self.poll_servers(now);
    }

    /// The relay socket for the client at `client` of `listen` is bound at `result` (or could not be).
    pub fn relay_allocated(
        &mut self,
        now: Instant,
        listen: SocketAddr,
        proto: Proto,
        client: SocketAddr,
        family: AddressFamily,
        result: Result<SocketAddr, ()>,
    ) {
        let transport = transport_of(proto);
        let tnow = self.clock(now);
        let username = self.asked.remove(&(listen, client));
        let socket = result.map_err(|()| SocketAllocateError::InsufficientCapacity);
        if let (Ok(relayed), Some(username)) = (&socket, username) {
            let leg: Arc<str> = leg_of(&username, unix_now()).unwrap_or_default().into();
            let rate = self.rate();
            self.allocations.insert(
                (listen, client),
                Allocation {
                    username,
                    leg,
                    relayed: Some(*relayed),
                    bucket: Bucket { tokens: rate, at: now },
                },
            );
            self.relays.insert(*relayed, (listen, client));
        }
        let Some(server) = self.server_at(listen, transport) else {
            return;
        };
        server.allocated_socket(transport, listen, client, TransportType::Udp, family, socket, tnow);
        self.poll_servers(now);
    }

    /// A leg left or was removed: its username is refused from now on, and its relays close.
    pub fn revoke_leg(&mut self, now: Instant, leg: &str) {
        self.revoked.insert(leg.into(), now + REVOKED_FOR);
        let gone: Vec<(SocketAddr, SocketAddr)> = self
            .allocations
            .iter()
            .filter(|(_, a)| &*a.leg == leg)
            .map(|(k, _)| *k)
            .collect();
        for key in gone {
            if let Some(a) = self.allocations.remove(&key) {
                if let Some(relayed) = a.relayed {
                    self.relays.remove(&relayed);
                    self.out.push_back(TurnOutput::CloseRelay { relayed });
                }
            }
        }
    }

    /// Time passed.
    pub fn handle_timeout(&mut self, now: Instant) {
        self.revoked.retain(|_, until| *until > now);
        self.poll_servers(now);
    }

    pub fn poll_output(&mut self) -> Option<TurnOutput> {
        self.out.pop_front()
    }

    pub fn next_timeout(&self, now: Instant) -> Instant {
        self.next.map_or(now + MAX_WAIT, |t| t.min(now + MAX_WAIT))
    }

    fn rate(&self) -> f64 {
        self.config.max_kbps as f64 * 1000.0 / 8.0
    }

    /// Takes `bytes` from the allocation's bucket; false when it is empty (the data is dropped).
    fn take_tokens(&mut self, key: &(SocketAddr, SocketAddr), bytes: usize, now: Instant) -> bool {
        let rate = self.rate();
        let Some(a) = self.allocations.get_mut(key) else {
            return false;
        };
        let elapsed = now.saturating_duration_since(a.bucket.at).as_secs_f64();
        a.bucket.tokens = (a.bucket.tokens + elapsed * rate).min(rate);
        a.bucket.at = now;
        if a.bucket.tokens < bytes as f64 {
            return false;
        }
        a.bucket.tokens -= bytes as f64;
        true
    }

    /// Something the server sends: to a client from a listener, or to a peer from a relay socket (only a media node,
    /// within the allocation's bandwidth).
    fn send_out(&mut self, now: Instant, proto: Proto, from: SocketAddr, to: SocketAddr, data: Vec<u8>) {
        if let Some(&key) = self.relays.get(&from) {
            let leg = self.allocations.get(&key).map(|a| Arc::clone(&a.leg));
            if !self.peer_allowed(to, false, leg.as_deref()) || !self.take_tokens(&key, data.len(), now) {
                return;
            }
        } else if self.servers.iter().all(|s| s.listen_address() != from) {
            // from a relay socket that was closed (a revoked leg): nothing more for it
            return;
        }
        self.out.push_back(TurnOutput::Transmit { proto, from, to, data });
    }

    fn poll_servers(&mut self, now: Instant) {
        let tnow = self.clock(now);
        let epoch = self.epoch;
        let mut next: Option<Instant> = None;
        let mut sends = Vec::new();
        let mut outs = Vec::new();
        for server in &mut self.servers {
            loop {
                match server.poll(tnow) {
                    TurnServerPollRet::WaitUntil(t) => {
                        let at = epoch + t.saturating_duration_since(turn_server_proto::types::Instant::from_nanos(0));
                        next = Some(next.map_or(at, |n| n.min(at)));
                        break;
                    }
                    TurnServerPollRet::AllocateSocket {
                        transport,
                        listen_addr,
                        client_addr,
                        allocation_transport,
                        family,
                    } => {
                        if allocation_transport != TransportType::Udp {
                            server.allocated_socket(
                                transport,
                                listen_addr,
                                client_addr,
                                allocation_transport,
                                family,
                                Err(SocketAllocateError::InsufficientCapacity),
                                tnow,
                            );
                            continue;
                        }
                        outs.push(TurnOutput::AllocateRelay {
                            listen: listen_addr,
                            proto: proto_of(transport),
                            client: client_addr,
                            family,
                        });
                    }
                    TurnServerPollRet::TcpConnect {
                        relayed_addr,
                        peer_addr,
                        listen_addr,
                        client_addr,
                    } => {
                        // no TCP allocations: a TCP relay to a peer is not offered
                        server.tcp_connected(
                            relayed_addr,
                            peer_addr,
                            listen_addr,
                            client_addr,
                            Err(TcpConnectError::Forbidden),
                            tnow,
                        );
                    }
                    TurnServerPollRet::TcpClose {
                        local_addr,
                        remote_addr,
                    } => {
                        outs.push(TurnOutput::CloseStream {
                            local: local_addr,
                            remote: remote_addr,
                        });
                    }
                    TurnServerPollRet::SocketClose { listen_addr, .. } => {
                        outs.push(TurnOutput::CloseRelay { relayed: listen_addr });
                    }
                }
            }
            while let Some(t) = server.poll_transmit(tnow) {
                sends.push((proto_of(t.transport), t.from, t.to, t.data));
            }
        }
        for out in outs {
            if let TurnOutput::CloseRelay { relayed } = &out {
                if let Some(key) = self.relays.remove(relayed) {
                    self.allocations.remove(&key);
                }
            }
            self.out.push_back(out);
        }
        for (proto, from, to, data) in sends {
            self.send_out(now, proto, from, to, data);
        }
        self.next = next;
    }
}

#[cfg(test)]
mod tests;
