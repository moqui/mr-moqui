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

//! The embedded TURN server against `turn-client-proto` clients on simulated time (W4): a leg's credential allocates and
//! relays to and from a media node; an expired credential, a name that is not ours and a revoked leg are refused, and
//! revoking closes the relay; a relay peer that is no media node is refused; the per-username quota holds.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use turn_client_proto::api::{TurnClientApi, TurnConfig as ClientConfig, TurnEvent, TurnRecvRet};
use turn_client_proto::udp::TurnClientUdp;
use turn_server_proto::types::{TransportType, TurnCredentials};

use super::{credential, frame_len, Turn, TurnConfig, TurnOutput};
use crate::sfu::Proto;

const SECRET: &[u8] = b"turn-test-secret";

fn addr(last: u8, port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)), port)
}

fn listen() -> SocketAddr {
    addr(1, 3478)
}

fn media() -> SocketAddr {
    addr(9, 10000)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock")
        .as_secs()
}

struct Net {
    start: Instant,
    now: Instant,
    turn: Turn,
    next_relay: u16,
    /// What reached peers: (relay, peer, data).
    at_peers: Vec<(SocketAddr, SocketAddr, Vec<u8>)>,
    closed: Vec<SocketAddr>,
}

struct Client {
    client: TurnClientUdp,
    events: Vec<TurnEvent>,
    received: Vec<Vec<u8>>,
}

impl Net {
    fn new(max_per_user: usize) -> Self {
        let now = Instant::now();
        let config = TurnConfig {
            secret: SECRET.to_vec(),
            allowed_peers: vec![media().ip()],
            max_per_user,
            ..TurnConfig::default()
        };
        Net {
            start: now,
            now,
            turn: Turn::new(config, &[(Proto::Udp, listen())]),
            next_relay: 50_000,
            at_peers: Vec::new(),
            closed: Vec::new(),
        }
    }

    /// The clients' clock: the simulated time since the start.
    fn tnow(&self) -> turn_server_proto::types::Instant {
        turn_server_proto::types::Instant::from_nanos(
            i64::try_from((self.now - self.start).as_nanos()).expect("a short test"),
        )
    }

    fn client(&self, port: u16, username: &str, password: &str) -> Client {
        let mut config = ClientConfig::new(TurnCredentials::new(username, password));
        // as a browser does: USERNAME, not RFC 8489's anonymous USERHASH
        config.set_anonymous_username(turn_server_proto::stun::auth::Feature::Disabled);
        Client {
            client: TurnClientUdp::allocate(addr(2, port), listen(), config),
            events: Vec::new(),
            received: Vec::new(),
        }
    }

    /// Moves everything for `d` of simulated time, 10 ms at a time.
    fn run(&mut self, clients: &mut [&mut Client], d: Duration) {
        let end = self.now + d;
        while self.now < end {
            self.now += Duration::from_millis(10);
            let tnow = self.tnow();
            for c in clients.iter_mut() {
                let _ = c.client.poll(tnow);
                while let Some(t) = c.client.poll_transmit(tnow) {
                    let data: &[u8] = t.data.as_ref();
                    self.turn.handle_client(self.now, Proto::Udp, t.from, t.to, data);
                }
            }
            self.turn.handle_timeout(self.now);
            while let Some(out) = self.turn.poll_output() {
                match out {
                    TurnOutput::Transmit { from, to, data, .. } => {
                        if let Some(c) = clients.iter_mut().find(|c| c.client.local_addr() == to) {
                            let transmit = turn_server_proto::api::Transmit {
                                data: &data[..],
                                transport: TransportType::Udp,
                                from,
                                to,
                            };
                            if let TurnRecvRet::PeerData(peer) = c.client.recv(transmit, tnow) {
                                c.received.push(peer.data().to_vec());
                            }
                        } else {
                            self.at_peers.push((from, to, data));
                        }
                    }
                    TurnOutput::AllocateRelay {
                        listen,
                        proto,
                        client,
                        family,
                    } => {
                        let relayed = SocketAddr::new(listen.ip(), self.next_relay);
                        self.next_relay += 1;
                        self.turn
                            .relay_allocated(self.now, listen, proto, client, family, Ok(relayed));
                    }
                    TurnOutput::CloseRelay { relayed } => self.closed.push(relayed),
                    TurnOutput::CloseStream { .. } => {}
                }
            }
            for c in clients.iter_mut() {
                while let Some(e) = c.client.poll_event() {
                    c.events.push(e);
                }
            }
        }
    }
}

impl Client {
    fn allocated(&self) -> Option<SocketAddr> {
        self.events.iter().find_map(|e| match e {
            TurnEvent::AllocationCreated(_, relayed) => Some(*relayed),
            _ => None,
        })
    }

    fn permitted(&self, ip: IpAddr) -> bool {
        self.events
            .iter()
            .any(|e| matches!(e, TurnEvent::PermissionCreated(_, p) if *p == ip))
    }

    fn send(&mut self, net: &mut Net, to: SocketAddr, data: &[u8]) -> bool {
        let tnow = net.tnow();
        match self.client.send_to(TransportType::Udp, to, data, tnow) {
            Ok(Some(t)) => {
                let t = t.build();
                net.turn.handle_client(net.now, Proto::Udp, t.from, t.to, &t.data);
                true
            }
            _ => false,
        }
    }
}

#[test]
fn a_legs_credential_relays_to_and_from_a_media_node() {
    let mut net = Net::new(2);
    let (user, pass) = credential(SECRET, "LEG1", unix_now() + 600);
    let mut c = net.client(5000, &user, &pass);
    net.run(&mut [&mut c], Duration::from_secs(1));
    let relayed = c.allocated().expect("an allocation");
    assert_eq!(net.turn.allocation_count(), 1);
    let tnow = net.tnow();
    c.client
        .create_permission(TransportType::Udp, media().ip(), tnow)
        .expect("a permission request");
    net.run(&mut [&mut c], Duration::from_secs(1));
    assert!(c.permitted(media().ip()));
    assert!(c.send(&mut net, media(), b"to the media node"));
    net.run(&mut [&mut c], Duration::from_millis(100));
    assert!(net
        .at_peers
        .iter()
        .any(|(from, to, data)| *from == relayed && *to == media() && data == b"to the media node"));
    net.turn.handle_peer(net.now, media(), relayed, b"from the media node");
    net.run(&mut [&mut c], Duration::from_millis(100));
    assert_eq!(c.received, vec![b"from the media node".to_vec()]);
}

#[test]
fn an_expired_credential_or_a_name_that_is_not_ours_is_refused() {
    let mut net = Net::new(2);
    let (user, pass) = credential(SECRET, "LEG1", unix_now() - 10);
    let mut expired = net.client(5000, &user, &pass);
    let mut stranger = net.client(5001, "alice", "secret");
    let (user, _) = credential(SECRET, "LEG2", unix_now() + 600);
    let mut forged = net.client(5002, &user, "not-the-password");
    net.run(&mut [&mut expired, &mut stranger, &mut forged], Duration::from_secs(45));
    for c in [&expired, &stranger, &forged] {
        assert!(c.allocated().is_none(), "{:?}", c.events);
    }
    assert_eq!(net.turn.allocation_count(), 0);
}

#[test]
fn a_peer_that_is_no_media_node_is_refused() {
    let mut net = Net::new(2);
    let (user, pass) = credential(SECRET, "LEG1", unix_now() + 600);
    let mut c = net.client(5000, &user, &pass);
    net.run(&mut [&mut c], Duration::from_secs(1));
    let relayed = c.allocated().expect("an allocation");
    let elsewhere = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)), 443);
    let tnow = net.tnow();
    c.client
        .create_permission(TransportType::Udp, elsewhere.ip(), tnow)
        .expect("a permission request");
    net.run(&mut [&mut c], Duration::from_secs(45));
    assert!(!c.permitted(elsewhere.ip()));
    let _ = c.send(&mut net, elsewhere, b"out");
    net.turn.handle_peer(net.now, elsewhere, relayed, b"in");
    net.run(&mut [&mut c], Duration::from_millis(100));
    assert!(net.at_peers.iter().all(|(_, to, _)| *to != elsewhere));
    assert!(c.received.is_empty());
}

#[test]
fn an_xmpp_users_credential_relays_to_public_addresses_only() {
    let mut net = Net::new(2);
    let (user, pass) = credential(SECRET, "xmpp.U1", unix_now() + 600);
    let mut c = net.client(5000, &user, &pass);
    net.run(&mut [&mut c], Duration::from_secs(1));
    let relayed = c.allocated().expect("an allocation");
    // a peer XMPP client on the internet
    let public = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4)), 50000);
    let tnow = net.tnow();
    c.client
        .create_permission(TransportType::Udp, public.ip(), tnow)
        .expect("a permission request");
    net.run(&mut [&mut c], Duration::from_secs(1));
    assert!(c.permitted(public.ip()));
    net.turn.handle_peer(net.now, public, relayed, b"from the peer");
    net.run(&mut [&mut c], Duration::from_millis(100));
    assert_eq!(c.received, vec![b"from the peer".to_vec()]);
    // never into the server's own networks
    let inside = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)), 22);
    let tnow = net.tnow();
    c.client
        .create_permission(TransportType::Udp, inside.ip(), tnow)
        .expect("a permission request");
    net.run(&mut [&mut c], Duration::from_secs(45));
    assert!(!c.permitted(inside.ip()));
    for ip in [
        "127.0.0.1",
        "10.1.2.3",
        "100.64.0.1",
        "169.254.1.1",
        "::1",
        "fd00::1",
        "fe80::1",
        "::ffff:10.0.0.1",
        "192.0.2.1",
        "2002:0a00:0001::1",
        "2002:7f00:0001::",
        "::10.0.0.1",
        "64:ff9b::a00:1",
        "2001:0:4136:e378:8000:63bf:3fff:fdd2",
    ] {
        assert!(!super::is_public(ip.parse().unwrap()), "{ip}");
    }
    assert!(super::is_public("2a00:1450::1".parse().unwrap()));
    assert!(
        super::is_public("2002:0808:0404::1".parse().unwrap()),
        "6to4 of a public address"
    );
}

#[test]
fn revoking_a_leg_closes_its_relay_and_refuses_it_after() {
    let mut net = Net::new(2);
    let (user, pass) = credential(SECRET, "LEG1", unix_now() + 600);
    let mut c = net.client(5000, &user, &pass);
    net.run(&mut [&mut c], Duration::from_secs(1));
    let relayed = c.allocated().expect("an allocation");
    let tnow = net.tnow();
    c.client
        .create_permission(TransportType::Udp, media().ip(), tnow)
        .expect("a permission request");
    net.run(&mut [&mut c], Duration::from_secs(1));
    net.turn.revoke_leg(net.now, "LEG1");
    net.run(&mut [&mut c], Duration::from_millis(100));
    assert!(net.closed.contains(&relayed));
    assert_eq!(net.turn.allocation_count(), 0);
    let before = net.at_peers.len();
    let _ = c.send(&mut net, media(), b"after");
    net.turn.handle_peer(net.now, media(), relayed, b"after");
    net.run(&mut [&mut c], Duration::from_millis(100));
    assert_eq!(net.at_peers.len(), before);
    assert!(c.received.is_empty());
    // the same credential from another port allocates nothing now
    let mut again = net.client(5001, &user, &pass);
    net.run(&mut [&mut again], Duration::from_secs(45));
    assert!(again.allocated().is_none());
}

#[test]
fn a_username_holds_its_quota() {
    let mut net = Net::new(1);
    let (user, pass) = credential(SECRET, "LEG1", unix_now() + 600);
    let mut first = net.client(5000, &user, &pass);
    let mut second = net.client(5001, &user, &pass);
    net.run(&mut [&mut first], Duration::from_secs(1));
    net.run(&mut [&mut first, &mut second], Duration::from_secs(45));
    assert!(first.allocated().is_some());
    assert!(second.allocated().is_none());
    assert_eq!(net.turn.allocation_count(), 1);
    let _ = net.start;
}

#[test]
fn a_stream_is_split_into_stun_messages_and_channel_data() {
    // a STUN header of a 12-byte body, and a ChannelData of 5 bytes padded to 8
    assert_eq!(frame_len(&[0x00, 0x01, 0x00, 0x0c]), Some(32));
    assert_eq!(frame_len(&[0x40, 0x00, 0x00, 0x05]), Some(12));
    assert_eq!(frame_len(&[0x00, 0x01]), None);
}
