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

//! Which network addresses are internal, for the outbound guards (the HTTP client's in `http`, and the clients
//! that connect by themselves: SMTP, IPP). std only, so a crate without the `http` feature has it.

/// The addresses of `host`, or an error when any is internal: one internal address among public ones could still be
/// the one connected to. A caller that must not look the name up again (the lookup could answer differently) connects
/// to an address this returned.
///
/// # Errors
///
/// Returns an error when the name does not resolve or any of its addresses is internal.
pub fn public_addresses(
    host: &str,
    port: u16,
) -> Result<Vec<std::net::SocketAddr>, Box<dyn std::error::Error + Send + Sync>> {
    use std::net::ToSocketAddrs;

    let name = host.trim_start_matches('[').trim_end_matches(']');
    let addresses: Vec<std::net::SocketAddr> = (name, port).to_socket_addrs()?.collect();
    if addresses.iter().any(|address| internal_address(address.ip())) {
        return Err(format!("{host} is an internal address and is not allowed here").into());
    }
    Ok(addresses)
}

/// Whether `address` is loopback, private, link-local, shared (CGNAT), unspecified or otherwise not public, also
/// inside IPv6: what `RestClient::deny_internal_addresses` and [`public_addresses`] refuse.
pub fn internal_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                // "this network" 0.0.0.0/8, and shared address space (CGNAT) 100.64.0.0/10
                || octets[0] == 0
                || (octets[0] == 100 && (octets[1] & 0xc0) == 64)
                // reserved 240.0.0.0/4, the broadcast address among them
                || octets[0] >= 240
        }
        std::net::IpAddr::V6(v6) => {
            let segments = v6.segments();
            let embedded = || {
                let [.., a, b, c, d] = v6.octets();
                internal_address(std::net::IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d)))
            };
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // unique local fc00::/7 and link local fe80::/10
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                // IPv4-mapped ::ffff:0:0/96 and IPv4-compatible ::/96
                || (segments[..5] == [0; 5] && matches!(segments[5], 0 | 0xffff) && embedded())
                // NAT64: the well-known 64:ff9b::/96 by the address inside, the local-use 64:ff9b:1::/48 whole
                || (segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] && embedded())
                || segments[..3] == [0x64, 0xff9b, 1]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_addresses_refuses_an_internal_address_and_gives_a_public_one() {
        for host in ["127.0.0.1", "10.1.2.3", "169.254.169.254", "[::1]", "localhost"] {
            let err = public_addresses(host, 25).unwrap_err().to_string();
            assert!(err.contains("internal address"), "{host}: {err}");
        }
        let found = public_addresses("93.184.216.34", 587).unwrap();
        assert_eq!(found, vec!["93.184.216.34:587".parse().unwrap()]);
    }
}
