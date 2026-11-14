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
//! revoking closes the relay; a relay peer that is no media node is refused; the per-username quota holds; a pairing's
//! credential is taken only with its own secret while it is paired (E16).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use turn_client_proto::api::{TurnClientApi, TurnConfig as ClientConfig, TurnEvent, TurnRecvRet};
use turn_client_proto::udp::TurnClientUdp;
use turn_server_proto::types::{TransportType, TurnCredentials};

use super::{credential, frame_len, pairing_of, pairing_secret, Turn, TurnConfig, TurnOutput};
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
            allowed_peers: vec![media()],
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
fn another_port_on_a_media_node_is_refused() {
    let mut net = Net::new(2);
    let (user, pass) = credential(SECRET, "LEG1", unix_now() + 600);
    let mut c = net.client(5000, &user, &pass);
    net.run(&mut [&mut c], Duration::from_secs(1));
    let relayed = c.allocated().expect("an allocation");
    // the permission is by address (RFC 8656), so the media node's is taken
    let tnow = net.tnow();
    c.client
        .create_permission(TransportType::Udp, media().ip(), tnow)
        .expect("a permission request");
    net.run(&mut [&mut c], Duration::from_secs(1));
    assert!(c.permitted(media().ip()));
    // but nothing goes to or comes from another port of it: ssh, its database, the HTTP port
    let other = SocketAddr::new(media().ip(), 22);
    assert!(c.send(&mut net, other, b"to another port"));
    net.turn.handle_peer(net.now, other, relayed, b"from another port");
    net.run(&mut [&mut c], Duration::from_millis(100));
    assert!(net.at_peers.iter().all(|(_, to, _)| *to != other));
    assert!(c.received.is_empty());
    // the media port still relays both ways
    assert!(c.send(&mut net, media(), b"to the media port"));
    net.turn.handle_peer(net.now, media(), relayed, b"from the media port");
    net.run(&mut [&mut c], Duration::from_millis(100));
    assert!(net.at_peers.iter().any(|(_, to, _)| *to == media()));
    assert_eq!(c.received, vec![b"from the media port".to_vec()]);
    // and a channel to another port is refused before the server sees it
    let tnow = net.tnow();
    let bound = c.client.bind_channel(TransportType::Udp, other, tnow);
    if bound.is_ok() {
        net.run(&mut [&mut c], Duration::from_secs(45));
        assert!(!c
            .events
            .iter()
            .any(|e| matches!(e, turn_client_proto::api::TurnEvent::ChannelCreated(_, p) if *p == other)));
    }
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

#[test]
fn an_xmpp_users_credential_reaches_no_other_port_of_the_servers_own_addresses() {
    let media: SocketAddr = "9.9.9.9:10000".parse().unwrap();
    let listen: SocketAddr = "1.0.0.1:3478".parse().unwrap();
    let config = TurnConfig {
        secret: SECRET.to_vec(),
        allowed_peers: vec![media],
        ..TurnConfig::default()
    };
    let turn = Turn::new(config, &[(Proto::Udp, listen)]);
    // public addresses, so the XMPP rule would take them but for a media node's or a listener's address
    let xmpp = Some("xmpp.U1");
    assert!(turn.peer_allowed(media, false, xmpp));
    assert!(turn.peer_allowed(media, false, Some("LEG1")));
    for other in ["9.9.9.9:22", "[::ffff:9.9.9.9]:22", "1.0.0.1:5060", "1.0.0.1:3478"] {
        let other: SocketAddr = other.parse().unwrap();
        assert!(!turn.peer_allowed(other, false, xmpp), "{other}");
        assert!(!turn.peer_allowed(other, false, Some("LEG1")), "{other}");
    }
    // a permission for the media node's address is taken; its other ports still are not relayed to
    assert!(turn.peer_allowed("[::ffff:9.9.9.9]:1".parse().unwrap(), true, Some("LEG1")));
    // an XMPP peer on the internet still is
    assert!(turn.peer_allowed("8.8.4.4:50000".parse().unwrap(), false, xmpp));
}

/// The pairings `ids`, each with its salt `ids`'s second part.
fn pairings(ids: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
    ids.iter()
        .map(|(id, salt)| ((*id).to_string(), (*salt).to_string()))
        .collect()
}

impl Net {
    /// A node that takes links: usernames may name pairings.
    fn edge(max_per_user: usize) -> Self {
        let mut net = Net::new(max_per_user);
        let mut config = net.turn.config.clone();
        config.pairings = true;
        net.turn = Turn::new(config, &[(Proto::Udp, listen())]);
        net
    }
}

#[test]
fn a_pairings_credential_is_taken_only_with_its_own_secret_while_it_is_paired() {
    let mut net = Net::edge(2);
    let pairing = |id: &str| pairing_secret(SECRET, id, "k1");
    assert_ne!(pairing("local-1"), pairing("local-2"), "a secret of each pairing");
    assert_eq!(pairing_of("100010@local-1"), Some("local-1"));
    assert_eq!(pairing_of("100010"), None);
    net.turn
        .set_pairings(net.now, pairings(&[("local-1", "k1"), ("local-2", "k1")]));
    let (user, pass) = credential(pairing("local-1").as_bytes(), "100010@local-1", unix_now() + 600);
    let mut own = net.client(5000, &user, &pass);
    // a leg named for another pairing, or signed with the shared secret, is refused
    let (other, other_pass) = credential(pairing("local-1").as_bytes(), "100011@local-2", unix_now() + 600);
    let mut crossed = net.client(5001, &other, &other_pass);
    let (shared, shared_pass) = credential(SECRET, "100012@local-1", unix_now() + 600);
    let mut by_shared = net.client(5002, &shared, &shared_pass);
    net.run(&mut [&mut own, &mut crossed, &mut by_shared], Duration::from_secs(45));
    let relayed = own.allocated().expect("the pairing's own allocation");
    assert!(crossed.allocated().is_none() && by_shared.allocated().is_none());
    // revoked: its relays close and its credentials are refused from then on, with the shared secret unchanged
    net.turn.set_pairings(net.now, pairings(&[("local-2", "k1")]));
    net.run(&mut [&mut own], Duration::from_millis(100));
    assert!(net.closed.contains(&relayed));
    assert_eq!(net.turn.allocation_count(), 0);
    let (user, pass) = credential(pairing("local-1").as_bytes(), "100013@local-1", unix_now() + 600);
    let mut after = net.client(5003, &user, &pass);
    net.run(&mut [&mut after], Duration::from_secs(45));
    assert!(after.allocated().is_none(), "a revoked pairing's new credential");
    let (user, pass) = credential(pairing("local-2").as_bytes(), "100014@local-2", unix_now() + 600);
    let mut still = net.client(5004, &user, &pass);
    net.run(&mut [&mut still], Duration::from_secs(1));
    assert!(still.allocated().is_some(), "the other pairing goes on");
}

#[test]
fn a_pairing_paired_again_under_its_id_refuses_the_credentials_from_before() {
    let mut net = Net::edge(2);
    net.turn.set_pairings(net.now, pairings(&[("local-1", "key-a|t1")]));
    let before = pairing_secret(SECRET, "local-1", "key-a|t1");
    let (user, pass) = credential(before.as_bytes(), "100010@local-1", unix_now() + 600);
    let mut first = net.client(5000, &user, &pass);
    net.run(&mut [&mut first], Duration::from_secs(1));
    let relayed = first.allocated().expect("the first pairing's allocation");
    // revoked, then paired again under the same id with a new key: the credentials from before, the one seen
    // already included, are refused, and its relay is closed
    net.turn.set_pairings(net.now, pairings(&[]));
    net.turn.set_pairings(net.now, pairings(&[("local-1", "key-b|t2")]));
    net.run(&mut [&mut first], Duration::from_millis(100));
    assert!(net.closed.contains(&relayed));
    let mut again = net.client(5001, &user, &pass);
    let (other, other_pass) = credential(before.as_bytes(), "100011@local-1", unix_now() + 600);
    let mut fresh_name = net.client(5002, &other, &other_pass);
    net.run(&mut [&mut again, &mut fresh_name], Duration::from_secs(45));
    assert!(
        again.allocated().is_none() && fresh_name.allocated().is_none(),
        "the old secret's credentials"
    );
    let after = pairing_secret(SECRET, "local-1", "key-b|t2");
    let (user, pass) = credential(after.as_bytes(), "100012@local-1", unix_now() + 600);
    let mut now = net.client(5003, &user, &pass);
    net.run(&mut [&mut now], Duration::from_secs(1));
    assert!(now.allocated().is_some(), "the new pairing's credential");
    // the salt changed while the pairing stayed (no revoke between): a username seen before takes the new secret
    net.turn.set_pairings(net.now, pairings(&[("local-1", "key-c|t3")]));
    let mut seen = net.client(5004, &user, &pass);
    net.run(&mut [&mut seen], Duration::from_secs(45));
    assert!(
        seen.allocated().is_none(),
        "a username seen before, with the secret before"
    );
}

#[test]
fn a_node_that_takes_no_links_reads_no_pairing_in_a_username() {
    // an XMPP user id with an @ is the server's own leg: signed with the shared secret, relaying as an XMPP user's
    let mut net = Net::new(2);
    let public: SocketAddr = "8.8.8.8:3478".parse().unwrap();
    assert!(net.turn.peer_allowed(public, false, Some("xmpp.ann@example")));
    let (user, pass) = credential(SECRET, "xmpp.ann@example", unix_now() + 600);
    let mut c = net.client(5000, &user, &pass);
    net.run(&mut [&mut c], Duration::from_secs(1));
    assert!(c.allocated().is_some());
}
#[test]
fn a_pairings_leg_is_never_an_xmpp_users_own() {
    let net = Net::edge(2);
    let public: SocketAddr = "8.8.8.8:3478".parse().unwrap();
    assert!(net.turn.peer_allowed(public, false, Some("xmpp.ann")));
    assert!(!net.turn.peer_allowed(public, false, Some("xmpp.ann@local-1")));
}
