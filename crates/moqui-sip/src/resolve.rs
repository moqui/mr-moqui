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

//! Where a trunk's requests go (RFC 3263 §4.2, docs/interfaces/SIP.md "Trunks"): the addresses of its host, in the
//! order they are tried.
//!
//! - A host that is an address, or a trunk with a port, is that host at that port (or the transport's default port).
//! - Otherwise the SRV records of the host for the transport are looked up: `_sips._tcp` for TLS, `_sip._tcp` for
//!   TCP, `_sip._udp` for UDP (RFC 2782). They are tried by priority, the lowest first, and within a priority by
//!   weight, the highest first; each target's addresses in turn, at the record's port. Weights are an order here, not
//!   a random share: one trunk sends from one place, so what matters is the order a failover takes.
//! - With no SRV records, or when the lookup fails, the host's own addresses at the transport's default port, as
//!   before SRV.
//!
//! The lookups are behind [`Resolver`], so the tests answer them with fixed records. TLS still names the trunk's host
//! (`ua.rs`, `Inner::connect`), never an SRV target: the provider's certificate is checked against the host it was
//! set up with.

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;

use crate::ua::Transport;

/// How long one DNS query waits for its answer.
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

/// An SRV record (RFC 2782): its priority, weight, port and target host, with no trailing dot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Srv {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    pub target: String,
}

/// The DNS lookups a trunk's host needs.
#[async_trait::async_trait]
pub trait Resolver: Send + Sync {
    /// The SRV records of `name` (such as `_sips._tcp.example.com`); empty when it has none, an error when the lookup
    /// failed.
    async fn srv(&self, name: &str) -> Result<Vec<Srv>, String>;

    /// The addresses of `host`; empty when it has none or the lookup failed.
    async fn addresses(&self, host: &str) -> Vec<IpAddr>;
}

/// The system's resolver: hickory (with the system's configuration) for SRV, and the system's own lookup for
/// addresses, so `/etc/hosts` still names a trunk's host.
#[derive(Default)]
pub struct SystemResolver {
    srv: OnceLock<Option<hickory_resolver::TokioResolver>>,
}

impl SystemResolver {
    fn srv_resolver(&self) -> Option<&hickory_resolver::TokioResolver> {
        self.srv
            .get_or_init(|| {
                let built = hickory_resolver::TokioResolver::builder_tokio().and_then(|mut builder| {
                    builder.options_mut().timeout = QUERY_TIMEOUT;
                    builder.build()
                });
                match built {
                    Ok(resolver) => Some(resolver),
                    Err(err) => {
                        tracing::warn!("SIP DNS: no SRV lookups, trunk hosts are resolved by their addresses: {err}");
                        None
                    }
                }
            })
            .as_ref()
    }
}

#[async_trait::async_trait]
impl Resolver for SystemResolver {
    async fn srv(&self, name: &str) -> Result<Vec<Srv>, String> {
        let Some(resolver) = self.srv_resolver() else {
            return Ok(Vec::new());
        };
        let key = hickory_resolver::proto::rr::Name::from_str_relaxed(name)
            .map_err(|err| format!("{name} is not a DNS name: {err}"))?;
        match resolver.srv_lookup(key).await {
            Ok(lookup) => Ok(lookup
                .answers()
                .iter()
                .filter_map(|record| match &record.data {
                    hickory_resolver::proto::rr::RData::SRV(srv) => Some(Srv {
                        priority: srv.priority,
                        weight: srv.weight,
                        port: srv.port,
                        target: srv.target.to_ascii().trim_end_matches('.').to_ascii_lowercase(),
                    }),
                    _ => None,
                })
                .collect()),
            Err(err) if err.is_no_records_found() => Ok(Vec::new()),
            Err(err) => Err(format!("the SRV records of {name}: {err}")),
        }
    }

    async fn addresses(&self, host: &str) -> Vec<IpAddr> {
        match tokio::net::lookup_host((host, 0)).await {
            Ok(addrs) => addrs.map(|a| a.ip()).collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// The SRV name of `host` for `transport` (RFC 3263 §4.2).
pub fn srv_name(transport: Transport, host: &str) -> String {
    let service = match transport {
        Transport::Tls => "_sips._tcp",
        Transport::Tcp => "_sip._tcp",
        Transport::Udp => "_sip._udp",
    };
    format!("{service}.{host}")
}

/// The records in the order they are tried: by priority, the lowest first, then by weight, the highest first (the
/// target's name breaks a tie, so the order is the same every time). A target of `.` (empty here) says the service is
/// not offered; it is left out.
pub fn order(mut records: Vec<Srv>) -> Vec<Srv> {
    records.retain(|r| !r.target.is_empty());
    records.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(b.weight.cmp(&a.weight))
            .then_with(|| a.target.cmp(&b.target))
    });
    records
}

/// The host as it is looked up, and as TLS names it: no brackets around an IPv6 address, no trailing dot, lower case.
pub(crate) fn bare(host: &str) -> String {
    let host = host.trim();
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// The addresses a trunk's requests go to, in the order they are tried; empty when its host resolved to none.
pub async fn targets(resolver: &dyn Resolver, host: &str, port: Option<u16>, transport: Transport) -> Vec<SocketAddr> {
    let host = bare(host);
    let default_port = port.unwrap_or(transport.default_port());
    if let Ok(ip) = host.parse::<IpAddr>() {
        return vec![SocketAddr::new(ip, default_port)];
    }
    let mut found: Vec<SocketAddr> = Vec::new();
    if port.is_none() {
        let name = srv_name(transport, &host);
        let records = match resolver.srv(&name).await {
            Ok(records) => records,
            Err(err) => {
                tracing::warn!("SIP DNS: {err}; {host} is resolved by its addresses");
                Vec::new()
            }
        };
        // records that all say "not offered" are an answer too: no target, rather than the host's addresses
        if !records.is_empty() {
            for record in order(records) {
                let ips = match record.target.parse::<IpAddr>() {
                    Ok(ip) => vec![ip],
                    Err(_) => resolver.addresses(&record.target).await,
                };
                for ip in ips {
                    let addr = SocketAddr::new(ip, record.port);
                    if !found.contains(&addr) {
                        found.push(addr);
                    }
                }
            }
            return found;
        }
    }
    for ip in resolver.addresses(&host).await {
        let addr = SocketAddr::new(ip, default_port);
        if !found.contains(&addr) {
            found.push(addr);
        }
    }
    found
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
pub(crate) mod tests;
