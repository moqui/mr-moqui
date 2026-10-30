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

//! The client's address: CIDR ranges, and the address a trusted proxy in front names in `client-ip-header`.
//!
//! With `trusted-proxies` set, the header is read only from a peer in those ranges, and the address taken is the
//! rightmost hop of it that is not a trusted proxy itself: proxies append, so the hops left of that one are whatever
//! the client wrote. With no `trusted-proxies`, the header is read from any peer and its first hop taken, as before
//! there was a list; that is meant for a proxy that overwrites the header (`X-Real-IP`, `CF-Connecting-IP`).

use std::net::IpAddr;

use crate::context::ExecutionContextFactory;

/// An address range: `10.0.0.0/8`, `fd00::/8`, or one address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// `address/prefix`, or an address alone for just that one. An IPv4 address mapped into IPv6 is its IPv4 address.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address.trim(), Some(prefix.trim().parse::<u8>().ok()?)),
            None => (text, None),
        };
        let address = address.parse::<IpAddr>().ok()?.to_canonical();
        let bits = if address.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(bits);
        if prefix > bits {
            return None;
        }
        Some(Self {
            network: masked(address, prefix),
            prefix,
        })
    }

    pub fn contains(&self, address: IpAddr) -> bool {
        let address = address.to_canonical();
        address.is_ipv4() == self.network.is_ipv4() && masked(address, self.prefix) == self.network
    }
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix)
    }
}

/// The comma separated ranges of `text`. An entry that is not a range is logged with `what` and left out; the
/// rest apply.
pub fn parse_cidrs(text: &str, what: &str) -> Vec<Cidr> {
    text.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let parsed = Cidr::parse(entry);
            if parsed.is_none() {
                tracing::error!("{what}: '{entry}' is not an address or address range; it is left out");
            }
            parsed
        })
        .collect()
}

/// The first `prefix` bits of `address`, the rest zero.
pub fn masked(address: IpAddr, prefix: u8) -> IpAddr {
    match address {
        IpAddr::V4(v4) => {
            let bits = u32::from(v4);
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(prefix.min(32)))
            };
            IpAddr::V4((bits & mask).into())
        }
        IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(prefix.min(128)))
            };
            IpAddr::V6((bits & mask).into())
        }
    }
}

/// One hop of a forwarding header: an address, maybe quoted, maybe with a port (`1.2.3.4:443`, `[::1]:443`), or a
/// `Forwarded` style `for=` pair.
pub fn parse_hop(text: &str) -> Option<IpAddr> {
    let mut text = text.trim();
    if let Some((_, rest)) = text
        .split_at_checked(4)
        .filter(|(start, _)| start.eq_ignore_ascii_case("for="))
    {
        text = rest.split(';').next().unwrap_or("").trim();
    }
    let text = text.trim_matches('"');
    if let Ok(address) = text.parse::<IpAddr>() {
        return Some(address.to_canonical());
    }
    if let Some(rest) = text.strip_prefix('[') {
        let inside = rest.split(']').next()?;
        return inside.parse::<IpAddr>().ok().map(|address| address.to_canonical());
    }
    // an IPv4 address with a port; an IPv6 address with a port is bracketed, handled above
    let (address, port) = text.rsplit_once(':')?;
    port.parse::<u16>().ok()?;
    address.parse::<std::net::Ipv4Addr>().ok().map(IpAddr::V4)
}

/// How the client's address is found: the header a proxy in front names it in, and the proxies believed.
#[derive(Clone, Debug, Default)]
pub struct ClientAddressing {
    header: String,
    trusted: Vec<Cidr>,
}

impl ClientAddressing {
    pub fn new(header: &str, trusted: Vec<Cidr>) -> Self {
        Self {
            header: header.trim().to_ascii_lowercase(),
            trusted,
        }
    }

    /// From the `webroot` webapp's `client-ip-header` and `trusted-proxies`, or the `webapp_client_ip_header` and
    /// `webapp_trusted_proxies` properties when the conf leaves them out.
    pub fn of_factory(factory: &ExecutionContextFactory) -> Self {
        let setting = |name: &str, property: &str| {
            let value = factory.webapp_setting("webroot", name);
            if value.is_empty() {
                moqui_lib::env::get_prop_or_env(property).trim().to_string()
            } else {
                value
            }
        };
        Self::new(
            &setting("client-ip-header", "webapp_client_ip_header"),
            parse_cidrs(&setting("trusted-proxies", "webapp_trusted_proxies"), "trusted-proxies"),
        )
    }

    /// From the properties alone, for a request with no factory.
    pub fn of_properties() -> Self {
        Self::new(
            &moqui_lib::env::get_prop_or_env("webapp_client_ip_header"),
            parse_cidrs(
                &moqui_lib::env::get_prop_or_env("webapp_trusted_proxies"),
                "webapp_trusted_proxies",
            ),
        )
    }

    /// The header, lowercase; empty when there is none.
    pub fn header(&self) -> &str {
        &self.header
    }

    /// True when `trusted-proxies` names at least one proxy.
    pub fn has_trusted(&self) -> bool {
        !self.trusted.is_empty()
    }

    /// True when `peer` is one of the trusted proxies.
    pub fn is_trusted(&self, peer: IpAddr) -> bool {
        self.trusted.iter().any(|range| range.contains(peer))
    }

    /// Whether the forwarding headers (the client address, `X-Forwarded-Proto`) of a request from `peer` are believed:
    /// a header is configured, and `peer` is a trusted proxy or no list of them is.
    pub fn believes(&self, peer: IpAddr) -> bool {
        !self.header.is_empty() && (self.trusted.is_empty() || self.is_trusted(peer))
    }

    /// The client of a request from `peer` whose `client-ip-header` is `value`: `peer` itself when the header is not
    /// believed or names no address.
    pub fn resolve(&self, peer: IpAddr, value: Option<&str>) -> IpAddr {
        let peer = peer.to_canonical();
        let Some(value) = value.filter(|_| self.believes(peer)) else {
            return peer;
        };
        if self.trusted.is_empty() {
            return value.split(',').next().and_then(parse_hop).unwrap_or(peer);
        }
        let mut nearest = peer;
        for hop in value.rsplit(',') {
            let Some(address) = parse_hop(hop) else {
                break;
            };
            if !self.is_trusted(address) {
                return address;
            }
            nearest = address;
        }
        nearest
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{masked, parse_cidrs, parse_hop, Cidr, ClientAddressing};

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn ranges_match_their_addresses() {
        let private = Cidr::parse("10.0.0.0/8").unwrap();
        assert!(private.contains(ip("10.200.3.4")));
        assert!(!private.contains(ip("11.0.0.1")));
        assert!(private.contains(ip("::ffff:10.1.1.1")));
        let one = Cidr::parse("203.0.113.5").unwrap();
        assert!(one.contains(ip("203.0.113.5")));
        assert!(!one.contains(ip("203.0.113.6")));
        let ula = Cidr::parse("fd00::/8").unwrap();
        assert!(ula.contains(ip("fd12:3456::1")));
        assert!(!ula.contains(ip("fe80::1")));
        assert!(!ula.contains(ip("10.0.0.1")));
        assert!(Cidr::parse("0.0.0.0/0").unwrap().contains(ip("8.8.8.8")));
        assert_eq!(Cidr::parse("10.1.2.3/8").unwrap().to_string(), "10.0.0.0/8");
        for bad in ["10.0.0.0/33", "nope", "10.0.0.0/x", "::/129", ""] {
            assert!(Cidr::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_bad_range_is_left_out_and_the_rest_apply() {
        let ranges = parse_cidrs("10.0.0.0/8, nope,, 192.168.0.0/16", "test");
        assert_eq!(ranges.len(), 2);
    }

    #[test]
    fn ipv6_prefixes_mask_the_host_bits() {
        assert_eq!(masked(ip("2001:db8:1:2:3:4:5:6"), 64), ip("2001:db8:1:2::"));
        assert_eq!(masked(ip("192.168.7.9"), 24), ip("192.168.7.0"));
        assert_eq!(masked(ip("192.168.7.9"), 0), ip("0.0.0.0"));
    }

    #[test]
    fn hops_drop_ports_quotes_and_for() {
        assert_eq!(parse_hop(" 203.0.113.5 "), Some(ip("203.0.113.5")));
        assert_eq!(parse_hop("203.0.113.5:4433"), Some(ip("203.0.113.5")));
        assert_eq!(parse_hop("[2001:db8::1]:443"), Some(ip("2001:db8::1")));
        assert_eq!(parse_hop("2001:db8::1"), Some(ip("2001:db8::1")));
        assert_eq!(parse_hop("for=\"[2001:db8::1]:443\""), Some(ip("2001:db8::1")));
        assert_eq!(parse_hop("For=198.51.100.7;proto=https"), Some(ip("198.51.100.7")));
        assert_eq!(parse_hop("::ffff:198.51.100.7"), Some(ip("198.51.100.7")));
        assert_eq!(parse_hop("unknown"), None);
        assert_eq!(parse_hop(""), None);
    }

    #[test]
    fn with_trusted_proxies_the_rightmost_untrusted_hop_is_the_client() {
        let addressing = ClientAddressing::new("X-Forwarded-For", parse_cidrs("10.0.0.0/8", "test"));
        let proxy = ip("10.0.0.2");
        // the client wrote the first hop; the proxies appended the rest
        let header = Some("6.6.6.6, 198.51.100.7, 10.0.0.9");
        assert_eq!(addressing.resolve(proxy, header), ip("198.51.100.7"));
        // the same header from a peer that is not a trusted proxy is not believed
        assert_eq!(addressing.resolve(ip("198.51.100.20"), header), ip("198.51.100.20"));
        // only trusted hops: the leftmost of them
        assert_eq!(addressing.resolve(proxy, Some("10.1.1.1, 10.2.2.2")), ip("10.1.1.1"));
        // a hop that is not an address stops the walk at the nearest trusted one
        assert_eq!(addressing.resolve(proxy, Some("unknown, 10.2.2.2")), ip("10.2.2.2"));
        assert_eq!(addressing.resolve(proxy, None), proxy);
        assert!(addressing.believes(proxy));
        assert!(!addressing.believes(ip("198.51.100.20")));
    }

    #[test]
    fn with_no_trusted_proxies_the_first_hop_is_believed_from_anyone() {
        let addressing = ClientAddressing::new("X-Real-IP", Vec::new());
        assert_eq!(
            addressing.resolve(ip("198.51.100.20"), Some("203.0.113.5:443, 10.0.0.1")),
            ip("203.0.113.5")
        );
        assert_eq!(
            addressing.resolve(ip("198.51.100.20"), Some("garbage")),
            ip("198.51.100.20")
        );
        let none = ClientAddressing::new("", parse_cidrs("10.0.0.0/8", "test"));
        assert!(!none.believes(ip("10.0.0.2")));
        assert_eq!(none.resolve(ip("10.0.0.2"), Some("203.0.113.5")), ip("10.0.0.2"));
    }
}
