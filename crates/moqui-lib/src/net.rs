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

/// The addresses of `host`, or an error when any is link-local or a cloud metadata address
/// ([`link_local_address`]): the guard of a request that may go to the private network but never to the metadata
/// service. A caller connects to an address this returned, as with [`public_addresses`].
///
/// # Errors
///
/// Returns an error when the name does not resolve or any of its addresses is link-local.
pub fn non_link_local_addresses(
    host: &str,
    port: u16,
) -> Result<Vec<std::net::SocketAddr>, Box<dyn std::error::Error + Send + Sync>> {
    use std::net::ToSocketAddrs;

    let name = host.trim_start_matches('[').trim_end_matches(']');
    let addresses: Vec<std::net::SocketAddr> = (name, port).to_socket_addrs()?.collect();
    if addresses.iter().any(|address| link_local_address(address.ip())) {
        return Err(format!("{host} is a link-local address and is never allowed").into());
    }
    Ok(addresses)
}

/// Whether `address` is not a public unicast address: what `RestClient::deny_internal_addresses` and
/// [`public_addresses`] refuse. For IPv4: loopback, private, link-local, multicast, `0.0.0.0/8`, shared (CGNAT)
/// `100.64.0.0/10`, `192.0.0.0/24`, benchmarking `198.18.0.0/15`, documentation and `240.0.0.0/4` with broadcast. For
/// IPv6 only global unicast `2000::/3` is public, less the IETF protocol range `2001::/23` (Teredo among it) and the
/// documentation ranges; an address that carries an IPv4 address (IPv4-mapped, NAT64 `64:ff9b::/96`, 6to4 `2002::/16`)
/// is what that IPv4 address is.
pub fn internal_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_documentation()
                // "this network" 0.0.0.0/8, and shared address space (CGNAT) 100.64.0.0/10
                || a == 0
                || (a == 100 && (b & 0xc0) == 64)
                // IETF protocol assignments 192.0.0.0/24, benchmarking 198.18.0.0/15
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (b & 0xfe) == 18)
                // reserved 240.0.0.0/4, the broadcast address among them
                || a >= 240
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = embedded_ipv4(v6) {
                return internal_address(std::net::IpAddr::V4(v4));
            }
            let segments = v6.segments();
            // global unicast 2000::/3 only: loopback, unspecified, multicast, unique local fc00::/7, link-local
            // fe80::/10, site-local fec0::/10, IPv4-compatible ::/96, ::ffff:0:0:0/96 and 64:ff9b:1::/48 are outside
            (segments[0] & 0xe000) != 0x2000
                // IETF protocol assignments 2001::/23: Teredo 2001::/32, benchmarking, ORCHID
                || (segments[0] == 0x2001 && segments[1] < 0x0200)
                // documentation 2001:db8::/32 and 3fff::/20
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || (segments[0] & 0xfff0) == 0x3ff0
        }
    }
}

/// Whether `address` is link-local or a cloud metadata address: `169.254.0.0/16` (`169.254.169.254` among it),
/// `fe80::/10`, AWS's `fd00:ec2::254`, Alibaba's `100.100.100.200`, and those inside IPv6 as in [`internal_address`].
/// Refused even for a request that may go to the private network.
pub fn link_local_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => v4.is_link_local() || v4.octets() == [100, 100, 100, 200],
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = embedded_ipv4(v6) {
                return link_local_address(std::net::IpAddr::V4(v4));
            }
            (v6.segments()[0] & 0xffc0) == 0xfe80 || v6.segments() == [0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254]
        }
    }
}

/// The IPv4 address an IPv6 address carries and is reached through: IPv4-mapped `::ffff:a.b.c.d`, NAT64
/// `64:ff9b::a.b.c.d` and 6to4 `2002:aabb:ccdd::/48`.
fn embedded_ipv4(v6: std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let segments = v6.segments();
    let [.., a, b, c, d] = v6.octets();
    if segments[..6] == [0, 0, 0, 0, 0, 0xffff] || segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return Some(std::net::Ipv4Addr::new(a, b, c, d));
    }
    if segments[0] == 0x2002 {
        let [a, b] = segments[1].to_be_bytes();
        let [c, d] = segments[2].to_be_bytes();
        return Some(std::net::Ipv4Addr::new(a, b, c, d));
    }
    None
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

    #[test]
    fn internal_address_covers_every_range_that_is_not_public_unicast() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.254",
            "0.1.2.3",
            "192.0.0.8",
            "192.0.2.1",
            "198.18.0.1",
            "198.19.255.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "::ffff:10.0.0.1",
            "::ffff:0:a00:1",
            "::8.8.8.8",
            "64:ff9b::a00:1",
            "64:ff9b:1::1",
            "2002:0a00:0001::1",
            "2002:a9fe:a9fe::",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001:2::1",
            "2001:10::1",
            "2001:db8::1",
            "3fff::1",
        ] {
            assert!(internal_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "8.8.8.8",
            "93.184.216.34",
            "100.128.0.1",
            "198.20.0.1",
            "2606:4700::1111",
            "2a00:1450::1",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2002:0808:0808::1",
        ] {
            assert!(!internal_address(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn link_local_and_metadata_addresses_are_refused_and_private_ones_are_not() {
        for ip in [
            "169.254.169.254",
            "169.254.0.1",
            "100.100.100.200",
            "fe80::1",
            "fd00:ec2::254",
            "::ffff:169.254.169.254",
            "2002:a9fe:a9fe::1",
        ] {
            assert!(link_local_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["127.0.0.1", "10.0.0.5", "192.168.1.1", "fd00::1", "8.8.8.8"] {
            assert!(!link_local_address(ip.parse().unwrap()), "{ip}");
        }
        assert!(non_link_local_addresses("169.254.169.254", 80).is_err());
        assert!(non_link_local_addresses("127.0.0.1", 80).is_ok());
    }
}
