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

//! The edge wizard's TURN check (plans/archive/Edge-Node.md §5.7, "As built (E10, calls)"): from the local node, as a browser
//! would, an allocation on the edge node's TURN over UDP with the credential `make#IceServers` gave, then a permission
//! for the IP of the edge node's media address, then the allocation deleted. A TURN permission is for an IP alone
//! (RFC 8656); the edge node's TURN grants one only for an IP of its allowed peers, and its rule that data goes only
//! to their ports is checked on the data, which the check sends none of. A `turn-client-proto` client on a socket of
//! its own, a few seconds at most.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use turn_client_proto::api::{Transmit, TurnClientApi, TurnConfig, TurnEvent};
use turn_client_proto::types::{TransportType, TurnCredentials};

/// How long the check waits for the TURN server's answers, all together.
const WAIT: Duration = Duration::from_secs(3);
/// The port of a `turn:` URL that names none (RFC 7065).
const DEFAULT_PORT: u16 = 3478;

/// The `host:port` of a `turn:` URL over UDP (no `transport`, or `transport=udp`), or why it is not one.
pub(crate) fn udp_server(url: &str) -> Result<String, String> {
    let url = url.trim();
    let Some(rest) = url.strip_prefix("turn:") else {
        return Err(format!("{url} is not a turn: URL"));
    };
    let (authority, query) = rest.split_once('?').unwrap_or((rest, ""));
    let tcp = query
        .split('&')
        .any(|part| part.trim().eq_ignore_ascii_case("transport=tcp"));
    if tcp {
        return Err(format!("{url} is TURN over TCP"));
    }
    let authority = authority.trim();
    let (host, port) = match authority.strip_prefix('[') {
        // [v6] or [v6]:port
        Some(v6) => {
            let Some((host, after)) = v6.split_once(']') else {
                return Err(format!("{url} has no host"));
            };
            (format!("[{host}]"), after.strip_prefix(':').unwrap_or(""))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port),
            None => (authority.to_string(), ""),
        },
    };
    if host.is_empty() || host == "[]" {
        return Err(format!("{url} has no host"));
    }
    let port: u16 = if port.is_empty() {
        DEFAULT_PORT
    } else {
        port.parse().map_err(|_| format!("{url} has no port number"))?
    };
    Ok(format!("{host}:{port}"))
}

fn refused(server: SocketAddr) -> String {
    format!(
        "The edge node's TURN at {server} refused the allocation: its turn_auth_secret is not the one its link handed \
         this node, or the credential expired"
    )
}

/// A UDP allocation on the TURN server of `url` with `username` and `credential`, then a permission for the IP of `peer`
/// (`ip:port`), then the allocation deleted: what was allocated, or why not.
pub(crate) fn probe(url: &str, username: &str, credential: &str, peer: &str) -> Result<String, String> {
    let server_text = udp_server(url)?;
    let server: SocketAddr = server_text
        .to_socket_addrs()
        .map_err(|err| format!("{server_text} did not resolve: {err}"))?
        .next()
        .ok_or_else(|| format!("{server_text} has no address"))?;
    let peer: SocketAddr = peer
        .trim()
        .parse()
        .map_err(|_| format!("The media address {peer} is not an address and port"))?;
    let any: SocketAddr = if server.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0_u16; 8], 0))
    };
    let socket = UdpSocket::bind(any).map_err(|err| format!("No UDP socket for the TURN check: {err}"))?;
    socket
        .connect(server)
        .and_then(|()| socket.set_read_timeout(Some(Duration::from_millis(20))))
        .map_err(|err| format!("The TURN server {server} cannot be reached: {err}"))?;
    let local = socket
        .local_addr()
        .map_err(|err| format!("No UDP socket for the TURN check: {err}"))?;
    let mut config = TurnConfig::new(TurnCredentials::new(username, credential));
    config.set_anonymous_username(turn_client_proto::stun::auth::Feature::Disabled);
    let mut client = turn_client_proto::udp::TurnClientUdp::allocate(local, server, config);
    let start = Instant::now();
    let now = || turn_client_proto::types::Instant::from_nanos(i64::try_from(start.elapsed().as_nanos()).unwrap_or(0));
    let send = |client: &mut turn_client_proto::udp::TurnClientUdp| {
        while let Some(transmit) = client.poll_transmit(now()) {
            let _ = socket.send(transmit.data.as_ref());
        }
    };
    let mut buf = [0_u8; 2048];
    let mut relayed = None;
    // a server that answers a credential it does not take with no allocation, rather than an error
    let mut answered = false;
    let outcome = loop {
        if start.elapsed() > WAIT {
            break Err(match relayed {
                None if answered => refused(server),
                None => format!(
                    "The edge node's TURN at {server} did not answer in {} s",
                    WAIT.as_secs()
                ),
                Some(relayed) => format!(
                    "The edge node's TURN allocated {relayed} but did not answer for a permission for {} in {} s",
                    peer.ip(),
                    WAIT.as_secs()
                ),
            });
        }
        let _ = client.poll(now());
        send(&mut client);
        if let Ok(n) = socket.recv(&mut buf) {
            answered = true;
            let transmit = Transmit {
                data: &buf[..n],
                transport: TransportType::Udp,
                from: server,
                to: local,
            };
            let _ = client.recv(transmit, now());
        }
        let mut done = None;
        while let Some(event) = client.poll_event() {
            match event {
                TurnEvent::AllocationCreated(_, address) => {
                    relayed = Some(address);
                    if let Err(err) = client.create_permission(TransportType::Udp, peer.ip(), now()) {
                        done = Some(Err(format!("A permission for {} was not asked: {err}", peer.ip())));
                    }
                }
                TurnEvent::AllocationCreateFailed(_) => done = Some(Err(refused(server))),
                TurnEvent::PermissionCreated(_, ip) if ip == peer.ip() => {
                    done = Some(Ok(format!(
                        "The edge node's TURN at {server} allocated {} for this node's credential and grants a \
                         permission for its media address's IP {ip}",
                        relayed.map(|address| address.to_string()).unwrap_or_default()
                    )));
                }
                TurnEvent::PermissionCreateFailed(_, ip) if ip == peer.ip() => {
                    done = Some(Err(format!(
                        "The edge node's TURN allocated, but refused a permission for {ip}, the IP of its media address \
                         {peer}: its TURN allows only its media node's address (rtc_public_address there, or \
                         turn_allowed_peers)"
                    )));
                }
                _ => {}
            }
        }
        if let Some(done) = done {
            break done;
        }
    };
    // an allocation made is given back at once, rather than left to its lifetime
    if relayed.is_some() && client.delete(now()).is_ok() {
        send(&mut client);
    }
    outcome
}

#[cfg(test)]
#[path = "turn_probe_tests.rs"]
mod tests;
